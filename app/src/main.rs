use std::sync::Arc;

use auau::{
    api::{self, AppState},
    auth::oidc::{CodeExchanger, OidcSettings},
    auth::session::SessionSecret,
    config::Config,
    db,
    jobs::{self, JobQueue},
    media::store::MediaStore,
};

#[cfg(not(feature = "e2e-auth-bypass"))]
use auau::auth::oidc::PocketIdExchanger;
#[cfg(feature = "e2e-auth-bypass")]
use auau::auth::oidc::NoIdpExchanger;

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

    {
        let sweeper_pool = pool.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                match jobs::sweep_expired_leases(&sweeper_pool).await {
                    Ok(expired) if expired > 0 => {
                        tracing::info!(expired, "returned expired leases to the queue");
                    }
                    Ok(_) => {}
                    Err(err) => tracing::error!(error = ?err, "lease sweep failed"),
                }
            }
        });
    }

    let max_upload_bytes = config.max_upload_bytes();

    #[cfg(not(feature = "e2e-auth-bypass"))]
    let (exchanger, end_session_url) = {
        let exchanger = PocketIdExchanger::from_config(&config).await?;
        let end_session_url = exchanger.end_session_url();
        (Arc::new(exchanger) as Arc<dyn CodeExchanger>, end_session_url)
    };
    #[cfg(feature = "e2e-auth-bypass")]
    let (exchanger, end_session_url): (Arc<dyn CodeExchanger>, Option<String>) =
        (Arc::new(NoIdpExchanger), None);
    let state = AppState {
        pool,
        oidc: OidcSettings::new(&config, end_session_url),
        session_secret: SessionSecret::new(config.session_secret),
        exchanger,
        store,
        jobs,
        max_upload_bytes,
    };
    let app = api::build_router(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
