use auau::{api, config::Config, db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let _pool = db::pool(&config.database_url).await?;
    db::migrate(&_pool).await?;
    tracing::info!("database migrations applied");
    let app = api::router();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
