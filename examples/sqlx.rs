use actix_web::{web, App, HttpResponse, HttpServer, Responder};
use health::{self, Reporter};
use sqlx::Connection;
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
    hc: web::Data<health::PeriodicChecker<PoolHealthChecker<sqlx::MySql>>>,
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

struct PoolHealthChecker<C>
where
    C: sqlx::Database,
{
    pool: sqlx::Pool<C>,
}

impl<C> PoolHealthChecker<C>
where
    C: sqlx::Database,
{
    fn new(pool: sqlx::Pool<C>) -> Self {
        Self { pool }
    }
}

impl<C> fmt::Debug for PoolHealthChecker<C>
where
    C: sqlx::Database,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PoolHealthChecker")
            .field("pool", &self.pool)
            .finish()
    }
}

impl<C> Clone for PoolHealthChecker<C>
where
    C: sqlx::Database,
{
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

#[async_trait::async_trait]
impl<C> health::Checkable for PoolHealthChecker<C>
where
    C: sqlx::Database,
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
    Ok(sqlx::pool::PoolOptions::new()
        .idle_timeout(Some(Duration::from_secs(60)))
        .connect_timeout(Duration::from_secs(5))
        .min_connections(1)
        .test_before_acquire(true)
        .connect(db_url)
        .await?)
}

#[actix_web::main]
async fn main() -> color_eyre::Result<()> {
    let pool = create_pool(&std::env::var("DB_URL").unwrap()).await?;
    let sql_health_check = PoolHealthChecker::new(pool);
    let periodic_check = health::PeriodicChecker::new(sql_health_check, health::Config::default());

    tokio::spawn(periodic_check.clone().run());

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
