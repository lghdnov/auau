use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::session::SessionSecret;
use crate::auth::CurrentUser;

/// Shared state threaded through extractors and handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub session_secret: SessionSecret,
}

/// Build the application API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/me", get(me))
        .with_state(state)
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

    const TEST_SECRET: &str = "test-session-secret";

    fn state(pool: PgPool) -> AppState {
        AppState {
            pool,
            session_secret: SessionSecret::new(TEST_SECRET),
        }
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let response = router(state(pool))
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body, json!({"status": "ok"}));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn me_without_cookie_returns_401(pool: PgPool) {
        let response = router(state(pool))
            .oneshot(Request::get("/api/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], json!("unauthorized"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn me_with_valid_cookie_returns_current_user(pool: PgPool) {
        let user = users::upsert(&pool, "pocketid|7", "Мария", "maria@example.com")
            .await
            .unwrap();
        let token = issue(user.id, &SessionSecret::new(TEST_SECRET), now_ts());

        let response = router(state(pool))
            .oneshot(
                Request::get("/api/v1/me")
                    .header("cookie", format!("auau_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["id"], json!(user.id.to_string()));
        assert_eq!(body["name"], json!("Мария"));
        assert_eq!(body["email"], json!("maria@example.com"));
    }

    fn now_ts() -> i64 {
        chrono::Utc::now().timestamp()
    }
}
