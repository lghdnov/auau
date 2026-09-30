//! Shared scaffolding for API handler tests: an [`AppState`] wired with a
//! no-op OIDC exchanger, a signed-in user with a session cookie, and small
//! request/response helpers.

use std::sync::Arc;

use axum::body::Body;
use axum::http::request::Request;
use axum::http::{header, StatusCode};
use axum::response::Response;
use sqlx::PgPool;

use crate::api::AppState;
use crate::auth::oidc::{CodeExchanger, ExchangedUser, OidcSettings};
use crate::auth::session::{self, SessionSecret};
use crate::auth::SESSION_COOKIE;
use crate::db::users;
use crate::error::AppError;
use crate::models::User;

/// Signing key for test session tokens; matches [`state`].
pub(crate) const TEST_SECRET: &str = "test-session-secret";

/// Handler tests under `/me`, `/projects`, ... never exchange codes; this
/// stands in for the production exchanger so `AppState` needs no network.
struct UnusedExchanger;

#[async_trait::async_trait]
impl CodeExchanger for UnusedExchanger {
    async fn exchange(&self, _code: &str, _verifier: &str) -> Result<ExchangedUser, AppError> {
        Err(AppError::Internal(anyhow::anyhow!(
            "code exchange is not exercised by these tests"
        )))
    }
}

/// Build `AppState` with the [`TEST_SECRET`] session key and no-op OIDC wiring.
pub(crate) fn state(pool: PgPool) -> AppState {
    AppState {
        pool,
        session_secret: SessionSecret::new(TEST_SECRET),
        oidc: OidcSettings {
            issuer: "https://sso.example.com".into(),
            client_id: "auau-client".into(),
            redirect_uri: "http://localhost:5173/api/v1/auth/callback".into(),
            end_session_url: None,
        },
        exchanger: Arc::new(UnusedExchanger),
    }
}

/// Create a fresh user and mint its `auau_session` cookie header value.
pub(crate) async fn signed_in(pool: &PgPool) -> anyhow::Result<(User, String)> {
    let subject = format!("pocketid|{}", uuid::Uuid::new_v4());
    let user = users::upsert(pool, &subject, "Тест Тестов", "tester@example.com").await?;
    let token = session::issue(
        user.id,
        &SessionSecret::new(TEST_SECRET),
        chrono::Utc::now().timestamp(),
    );
    Ok((user, format!("{SESSION_COOKIE}={token}")))
}

/// A GET request authorized by the given cookie header value.
pub(crate) fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// A POST request with a JSON body, authorized by the given cookie header value.
pub(crate) fn post_json(uri: &str, cookie: &str, body: impl serde::Serialize) -> Request<Body> {
    Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap()
}

/// Drain a response into its status and parsed JSON body.
pub(crate) async fn json(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
