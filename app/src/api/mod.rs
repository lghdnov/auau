use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::PgPool;

use crate::assets;
use crate::auth::oidc::{CodeExchanger, OidcSettings};
use crate::auth::session::SessionSecret;
use crate::auth::CurrentUser;
use crate::jobs::JobQueue;
use crate::media::store::MediaStore;

pub mod annotations;
pub mod auth_routes;
pub mod dto;
pub mod export;
pub mod files;
pub mod labels;
pub mod media;
pub mod projects;
pub mod queue;

#[cfg(test)]
pub(crate) mod test_support;

/// Shared state threaded through extractors and handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub session_secret: SessionSecret,
    /// Static OIDC wiring (issuer, client id, redirect URI, end-session URL).
    pub oidc: OidcSettings,
    /// Production code exchanger (mocked in tests).
    pub exchanger: Arc<dyn CodeExchanger>,
    /// Local media storage shared by handlers and the processing worker.
    pub store: MediaStore,
    /// Sender side of the processing pipeline; uploads enqueue here.
    pub jobs: JobQueue,
    /// Per-file upload size cap.
    pub max_upload_bytes: usize,
}

/// Build the final application router: the `/api/v1/*` API, the top-level
/// `/media/{id}` streaming routes, and — for anything else — the embedded
/// SPA (see [`crate::assets`]). `main` calls this one function.
pub fn build_router(state: AppState) -> Router {
    let body_limit = state.max_upload_bytes.saturating_add(1024 * 1024);
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/me", get(me))
        .merge(auth_routes::router())
        .merge(projects::router())
        .merge(labels::router())
        .merge(files::router())
        .merge(queue::router())
        .merge(annotations::router())
        .merge(export::router())
        .merge(media::router())
        .fallback(assets::spa_fallback)
        .with_state(state)
        .layer(DefaultBodyLimit::max(body_limit))
}

/// Liveness probe.
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

/// Identity of the signed-in user.
async fn me(user: CurrentUser) -> Json<serde_json::Value> {
    Json(json!({
        "id": user.0.id,
        "name": user.0.name,
        "email": user.0.email,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    use crate::auth::session::{issue, SessionSecret};
    use crate::db::users;

    #[tokio::test]
    async fn spa_served_and_api_guarded() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = build_router(test_support::state(pool));

        let response = app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, headers, body) = test_support::text(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html")
        );
        assert!(
            body.to_lowercase().contains("auau"),
            "index.html should mention auau"
        );

        let response = app
            .clone()
            .oneshot(Request::get("/some/spa/route").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, _, body) = test_support::text(response).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.to_lowercase().contains("auau"),
            "deep links should get the shell"
        );

        let response = app
            .clone()
            .oneshot(Request::get("/api/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let response = build_router(test_support::state(pool))
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let (status, body) = test_support::json(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"status": "ok"}));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn me_without_cookie_returns_401(pool: PgPool) {
        let response = build_router(test_support::state(pool))
            .oneshot(Request::get("/api/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let (status, body) = test_support::json(response).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], json!("unauthorized"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn me_with_valid_cookie_returns_current_user(pool: PgPool) {
        let user = users::upsert(&pool, "pocketid|7", "Мария", "maria@example.com")
            .await
            .unwrap();
        let token = issue(
            user.id,
            &SessionSecret::new(test_support::TEST_SECRET),
            chrono::Utc::now().timestamp(),
        );

        let response = build_router(test_support::state(pool))
            .oneshot(
                Request::get("/api/v1/me")
                    .header("cookie", format!("auau_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let (status, body) = test_support::json(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["id"], json!(user.id.to_string()));
        assert_eq!(body["name"], json!("Мария"));
        assert_eq!(body["email"], json!("maria@example.com"));
    }
}
