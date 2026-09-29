use auau::{
    api::{self, AppState},
    auth::session::SessionSecret,
    config::Config,
    db,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let pool = db::pool(&config.database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("database migrations applied");
    let state = AppState {
        pool,
        session_secret: SessionSecret::new(config.session_secret),
    };
    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
