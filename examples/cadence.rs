#[cfg(feature = "tracing")]
mod example {
    use cadence::MetricClient;
    use std::fmt;
    use tracing::{
        field::{Field, Visit},
        Event, Metadata, Subscriber,
    };
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    struct HealthCheckMetricsReporter {
        metrics: Box<dyn MetricClient + Send + Sync>,
    }

    impl<S> Layer<S> for HealthCheckMetricsReporter
    where
        S: Subscriber,
        Self: 'static,
    {
        fn enabled(&self, metadata: &Metadata, _ctx: Context<S>) -> bool {
            metadata.target() == stringify!(health)
        }

        fn on_event(&self, event: &Event, _ctx: Context<S>) {
            let mut x = HealthCheckMetricsVisitor::default();
            event.record(&mut x);

            let explode = match x {
                HealthCheckMetricsVisitor {
                    duration: Some(d),
                    module: Some(m),
                    passing: Some(p),
                    healthy: Some(h),
                } => Some((d, m, p, h)),
                _ => None,
            };

            if let Some((duration, module, passing, healthy)) = explode {
                // Telegraf format
                // let health = format!("health,module={},passing={},healthy={}", module, passing, healthy);
                // let health_status = format!("health_status,module={}", module);

                // self.metrics.time(&health, duration).unwrap();
                // self.metrics.gauge(&health_status, if healthy { 1 } else { 0 }).unwrap();

                // Datadog format
                self.metrics
                    .time_with_tags("health", duration)
                    .with_tag("module", &module)
                    .with_tag("passing", if passing { "true" } else { "false" })
                    .with_tag("healthy", if healthy { "true" } else { "false" })
                    .send();
                self.metrics
                    .gauge_with_tags("health_status", if healthy { 1 } else { 0 })
                    .with_tag("module", &module)
                    .send();
            } else {
                println!("Odd... Missing fields in health check event.");
            }
        }
    }

    #[derive(Default)]
    struct HealthCheckMetricsVisitor {
        duration: Option<u64>,
        module: Option<String>,
        passing: Option<bool>,
        healthy: Option<bool>,
    }

    impl Visit for HealthCheckMetricsVisitor {
        fn record_u64(&mut self, field: &Field, value: u64) {
            if field.name() == "duration" {
                self.duration = Some(value)
            }
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            if field.name() == "passing" {
                self.passing = Some(value);
            } else if field.name() == "healthy" {
                self.healthy = Some(value);
            }
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "module" {
                self.module = Some(String::from(value));
            }
        }

        fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {}
    }

    #[derive(Debug, Eq, PartialEq)]
    struct FlakyError;

    impl fmt::Display for FlakyError {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("oops, error!")
        }
    }

    impl std::error::Error for FlakyError {}

    #[derive(Default)]
    struct FlakyHealthCheck {
        x: std::sync::atomic::AtomicU64,
    }

    #[async_trait::async_trait]
    impl health::Checkable for FlakyHealthCheck {
        type Error = FlakyError;

        fn name(&self) -> std::borrow::Cow<str> {
            std::borrow::Cow::Borrowed("flaky")
        }

        async fn check(&self) -> Result<(), Self::Error> {
            let x = self.x.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if x % 7 == 0 || x % 8 == 0 || x % 9 == 0 || x % 10 == 0 {
                Err(FlakyError)
            } else {
                Ok(())
            }
        }
    }

    pub async fn main() -> color_eyre::Result<()> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;

        let host = (std::env::var("STATSD_HOST").unwrap(), cadence::DEFAULT_PORT);
        let udp_sink = cadence::BufferedUdpMetricSink::from(host, socket)?;
        let queuing_sink = cadence::QueuingMetricSink::from(udp_sink);
        let client = cadence::StatsdClient::from_sink("my_service", queuing_sink);

        let layer = HealthCheckMetricsReporter {
            metrics: Box::new(client),
        };

        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .finish()
            .with(layer);

        tracing::subscriber::set_global_default(subscriber)?;

        let check = FlakyHealthCheck::default();
        let periodic_check = health::PeriodicChecker::new(
            check,
            health::Config {
                check_interval: std::time::Duration::from_secs(1),
                min_successes: 2,
                ..health::Config::default()
            },
        );

        periodic_check.run().await;

        Ok(())
    }
}

#[cfg(not(feature = "tracing"))]
mod example {
    pub async fn main() -> color_eyre::Result<()> {
        Err(color_eyre::Report::msg(
            "This example requires the use of the `tracing` feature",
        ))
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    example::main().await
}
