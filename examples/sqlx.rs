#![cfg_attr(not(feature = "tokio_0_2"), allow(dead_code, unused_imports))]
use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use health::{self, Reporter};
use std::{fmt, time::Duration};

/// Health check endpoint
///
/// Based on the health check result, returns a `503 Service Unavailable` if
/// the health status is `Unhealthy` or unreliable. Otherwise returns a `200`.
///
/// If the health check is reliable, the body will indicate the result of the
/// most recently completed health check, reporting either "OK" or "ERROR".
#[actix_web::get("/healthz")]
async fn healthz(
    hc: web::Data<health::PeriodicChecker<PoolHealthChecker<sqlx::MySqlConnection>>>,
) -> impl Responder {
    let mut resp = match hc.status() {
        Some(health::Status::Healthy) => HttpResponse::Ok(),
        Some(health::Status::Unhealthy) => HttpResponse::ServiceUnavailable(),
        None => return HttpResponse::ServiceUnavailable().body("UNRELIABLE"),
    };

    let report = match hc.last_check() {
        health::Check::Pass => "OK",
        health::Check::Failed => "ERROR",
    };

    resp.body(report)
}

struct PoolHealthChecker<C> {
    pool: sqlx::Pool<C>,
}

impl<C> PoolHealthChecker<C> {
    fn new(pool: sqlx::Pool<C>) -> Self {
        Self { pool }
    }
}

impl<C> fmt::Debug for PoolHealthChecker<C>
where
    C: sqlx::Connect,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PoolHealthChecker")
            .field("pool", &self.pool)
            .finish()
    }
}

impl<C> Clone for PoolHealthChecker<C> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

#[async_trait::async_trait]
impl<C> health::Checkable for PoolHealthChecker<C>
where
    C: sqlx::Connect,
{
    type Error = sqlx::Error;

    fn name(&self) -> std::borrow::Cow<str> {
        std::borrow::Cow::Borrowed("sql")
    }

    async fn check(&self) -> Result<(), Self::Error> {
        let mut connection = self.pool.acquire().await?;
        connection.ping().await?;
        Ok(())
    }
}

async fn create_pool(db_url: &str) -> color_eyre::Result<sqlx::MySqlPool> {
    Ok(sqlx::MySqlPool::builder()
        .idle_timeout(Some(Duration::from_secs(60)))
        .connect_timeout(Duration::from_secs(5))
        .min_size(1)
        .test_on_acquire(true)
        .build(db_url)
        .await?)
}

async fn wrap<C>(p: health::PeriodicChecker<C>)
where
    C: health::Checkable,
{
    p.run().await;
}

#[cfg(feature = "tokio_0_2")]
#[actix_web::main]
async fn main() -> color_eyre::Result<()> {
    let pool = create_pool(&std::env::var("DB_URL").unwrap()).await?;
    let sql_health_check = PoolHealthChecker::new(pool);
    let periodic_check = health::PeriodicChecker::new(sql_health_check, health::Config::default());

    actix_web::rt::spawn(wrap(periodic_check.clone()));

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(periodic_check.clone()))
            .service(healthz)
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await?;
    Ok(())
}

#[cfg(not(feature = "tokio_0_2"))]
fn main() {
    println!("This example requires the use of the `tokio_0_2` feature.")
}
