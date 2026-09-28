use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

/// Build the application API router.
pub fn router() -> Router {
    Router::new().route("/api/v1/health", get(health))
}

/// Liveness probe.
async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let response = router()
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
}
