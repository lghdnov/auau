use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Build a connection pool for the given Postgres URL.
pub async fn pool(url: &str) -> sqlx::Result<PgPool> {
    PgPoolOptions::new().max_connections(10).connect(url).await
}

/// Apply pending migrations (embedded from `migrations/` at compile time).
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
