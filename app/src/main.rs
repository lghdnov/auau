use std::sync::Arc;

use auau::{
    api::{self, AppState},
    auth::oidc::{OidcSettings, PocketIdExchanger},
    auth::session::SessionSecret,
    config::Config,
    db,
    jobs::{self, JobQueue},
    media::store::MediaStore,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env()?;
    let pool = db::pool(&config.database_url).await?;
    db::migrate(&pool).await?;
    tracing::info!("database migrations applied");

    let store = MediaStore::new(config.storage_dir.clone());
    let (jobs, job_rx) = JobQueue::new();
    let _worker = jobs::spawn_worker(pool.clone(), store.clone(), job_rx);
    let max_upload_bytes = config.max_upload_bytes();

    let exchanger = PocketIdExchanger::from_config(&config).await?;
    let state = AppState {
        pool,
        oidc: OidcSettings::new(&config, exchanger.end_session_url()),
        session_secret: SessionSecret::new(config.session_secret),
        exchanger: Arc::new(exchanger),
        store,
        jobs,
        max_upload_bytes,
    };
    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
