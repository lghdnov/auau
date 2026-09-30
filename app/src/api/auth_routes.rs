use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use serde::Deserialize;

use crate::api::AppState;
use crate::auth::oidc::new_login_challenge;
use crate::auth::session;
use crate::auth::{cookie_value, SESSION_COOKIE};
use crate::db::users;
use crate::error::AppError;

/// Name of the short-lived cookie carrying `state|verifier` through the
/// authorization-code redirect.
const STATE_COOKIE: &str = "auau_oidc_state";

/// State-cookie lifetime: 10 minutes.
const STATE_COOKIE_MAX_AGE: u32 = 600;

/// Session-cookie lifetime: 12 hours (mirrors the session-token TTL).
const SESSION_COOKIE_MAX_AGE: u32 = 12 * 3600;

/// Public authentication routes (no `CurrentUser` required).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", get(login))
        .route("/api/v1/auth/callback", get(callback))
        .route("/api/v1/auth/logout", post(logout))
}

/// Start the OIDC flow: redirect to the authorize URL and stash
/// `state|verifier` in a short-lived http-only cookie.
async fn login(State(state): State<AppState>) -> Response {
    let challenge = new_login_challenge();
    let authorize_url = state
        .oidc
        .authorize_url(&challenge.state, &challenge.code_challenge);
    let state_cookie = format!(
        "{STATE_COOKIE}={}|{}; Path=/; HttpOnly; SameSite=Lax; Max-Age={STATE_COOKIE_MAX_AGE}",
        challenge.state, challenge.verifier
    );
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, authorize_url),
            (header::SET_COOKIE, state_cookie),
        ],
    )
        .into_response()
}

/// Finish the OIDC flow: verify state, exchange the code, sync the user,
/// and set the session cookie.
async fn callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (code, returned_state) = match (params.code, params.state) {
        (Some(code), Some(returned_state)) => (code, returned_state),
        _ => return Err(invalid_state("missing code or state parameter")),
    };
    let state_cookie = cookie_value(&headers, STATE_COOKIE)
        .ok_or_else(|| invalid_state("state cookie is missing"))?;
    let (cookie_state, verifier) = state_cookie
        .split_once('|')
        .ok_or_else(|| invalid_state("state cookie is malformed"))?;
    if cookie_state != returned_state {
        return Err(invalid_state("state does not match the cookie"));
    }

    let exchanged = state.exchanger.exchange(&code, verifier).await?;
    let user = users::upsert(
        &state.pool,
        &exchanged.subject,
        &exchanged.name,
        &exchanged.email,
    )
    .await?;
    let token = session::issue(user.id, &state.session_secret, Utc::now().timestamp());

    let session_cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_COOKIE_MAX_AGE}"
    );
    let mut headers = HeaderMap::new();
    headers.insert(header::LOCATION, HeaderValue::from_static("/"));
    headers.append(header::SET_COOKIE, set_cookie(&session_cookie)?);
    headers.append(header::SET_COOKIE, set_cookie(&clear_cookie(STATE_COOKIE))?);
    Ok((StatusCode::FOUND, headers).into_response())
}

/// Log out: drop the session cookie and, when known, redirect to the
/// provider's end-session endpoint.
async fn logout(State(state): State<AppState>) -> Response {
    let clear_session_cookie = clear_cookie(SESSION_COOKIE);
    match &state.oidc.end_session_url {
        Some(end_session_url) => (
            StatusCode::FOUND,
            [
                (header::LOCATION, end_session_url.clone()),
                (header::SET_COOKIE, clear_session_cookie),
            ],
        )
            .into_response(),
        None => (
            StatusCode::NO_CONTENT,
            [(header::SET_COOKIE, clear_session_cookie)],
        )
            .into_response(),
    }
}

/// A `Set-Cookie` header value that expires `name` immediately.
fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Build a `Set-Cookie` header value (cookie payloads are ASCII, but be safe).
fn set_cookie(value: &str) -> Result<HeaderValue, AppError> {
    HeaderValue::from_str(value)
        .map_err(|err| AppError::Internal(anyhow::anyhow!("invalid Set-Cookie header: {err}")))
}

/// CSRF protection failure: the state round-trip did not check out.
fn invalid_state(reason: &str) -> AppError {
    AppError::BadRequest {
        code: "invalid_state".into(),
        message: format!("OIDC state verification failed: {reason}"),
    }
}

#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::oidc::{CodeExchanger, ExchangedUser, OidcSettings};
    use axum::body::Body;
    use axum::http::Request;
    use sqlx::PgPool;
    use std::sync::Arc;
    use tower::ServiceExt;

    const TEST_SECRET: &str = "test-session-secret";
    const ISSUER: &str = "https://sso.example.com";
    const CLIENT_ID: &str = "auau-client";
    const BASE_URL: &str = "http://localhost:5173";

    /// Fixed-claims stand-in for the production exchanger: no network.
    struct MockExchanger;

    #[async_trait::async_trait]
    impl CodeExchanger for MockExchanger {
        async fn exchange(&self, _code: &str, _verifier: &str) -> Result<ExchangedUser, AppError> {
            Ok(ExchangedUser {
                subject: "pocketid|42".into(),
                name: "Дима".into(),
                email: "dima@example.com".into(),
            })
        }
    }

    fn state(pool: PgPool) -> AppState {
        AppState {
            pool,
            session_secret: session::SessionSecret::new(TEST_SECRET),
            oidc: OidcSettings {
                issuer: ISSUER.into(),
                client_id: CLIENT_ID.into(),
                redirect_uri: format!("{BASE_URL}/api/v1/auth/callback"),
                end_session_url: None,
            },
            exchanger: Arc::new(MockExchanger),
        }
    }

    fn set_cookies(response: &Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect()
    }

    /// Value of `name` among `Set-Cookie` headers (attributes stripped).
    fn cookie_value(cookies: &[String], name: &str) -> String {
        cookies
            .iter()
            .find(|cookie| cookie.starts_with(&format!("{name}=")))
            .and_then(|cookie| cookie.split_once(';'))
            .and_then(|(pair, _)| pair.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("Set-Cookie for {name} missing"))
            .to_string()
    }

    fn query_param(url: &str, name: &str) -> String {
        url.split_once('?')
            .and_then(|(_, query)| {
                query.split('&').find_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    (key == name).then_some(value.to_string())
                })
            })
            .unwrap_or_else(|| panic!("query param {name} missing in {url}"))
    }

    async fn login(app: &Router, cookies: &mut Vec<String>) -> String {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/v1/auth/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        *cookies = set_cookies(&response);
        location
    }

    #[tokio::test]
    async fn login_redirects_to_authorize_url_with_state_cookie() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();
        let app = crate::api::router(state(pool));
        let mut cookies = Vec::new();

        let location = login(&app, &mut cookies).await;

        assert!(
            location.starts_with(&format!("{ISSUER}/authorize?")),
            "{location}"
        );
        assert!(location.contains("client_id=auau-client"), "{location}");
        assert!(
            location.contains(&format!(
                "redirect_uri={}",
                urlencoding::encode(&format!("{BASE_URL}/api/v1/auth/callback"))
            )),
            "{location}"
        );
        assert!(location.contains("response_type=code"), "{location}");
        assert!(
            location.contains("scope=openid%20profile%20email"),
            "{location}"
        );
        assert!(
            location.contains("code_challenge_method=S256"),
            "{location}"
        );

        let state_value = cookie_value(&cookies, STATE_COOKIE);
        assert_eq!(
            state_value.split('|').next().unwrap(),
            query_param(&location, "state"),
            "state cookie must mirror the authorize-URL state"
        );
        assert!(state_value.contains('|'), "cookie holds state|verifier");
        let cookie = cookies
            .iter()
            .find(|cookie| cookie.starts_with(&format!("{STATE_COOKIE}=")))
            .unwrap();
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Max-Age=600"), "{cookie}");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn callback_creates_user_and_session(pool: PgPool) {
        let app = crate::api::router(state(pool));
        let mut cookies = Vec::new();

        let location = login(&app, &mut cookies).await;
        let state_cookie_value = cookie_value(&cookies, STATE_COOKIE);
        let state = query_param(&location, "state");

        let callback = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/auth/callback?code=c&state={state}"))
                    .header(
                        header::COOKIE,
                        format!("{STATE_COOKIE}={state_cookie_value}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(callback.status(), StatusCode::FOUND);
        assert_eq!(
            callback
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "/"
        );
        let cookies = set_cookies(&callback);
        let session_cookie = cookie_value(&cookies, SESSION_COOKIE);
        assert!(!session_cookie.is_empty());
        let raw_cookie = cookies
            .iter()
            .find(|cookie| cookie.starts_with(&format!("{SESSION_COOKIE}=")))
            .unwrap();
        assert!(raw_cookie.contains("HttpOnly"), "{raw_cookie}");
        assert!(raw_cookie.contains("SameSite=Lax"), "{raw_cookie}");
        assert!(raw_cookie.contains("Max-Age=43200"), "{raw_cookie}");

        let me = app
            .oneshot(
                Request::get("/api/v1/me")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}={session_cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(me.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(me.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["name"], serde_json::json!("Дима"));
        assert_eq!(body["email"], serde_json::json!("dima@example.com"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn callback_rejects_wrong_state(pool: PgPool) {
        let app = crate::api::router(state(pool));
        let mut cookies = Vec::new();

        login(&app, &mut cookies).await;
        let state_cookie_value = cookie_value(&cookies, STATE_COOKIE);

        let response = app
            .oneshot(
                Request::get("/api/v1/auth/callback?code=c&state=tampered")
                    .header(
                        header::COOKIE,
                        format!("{STATE_COOKIE}={state_cookie_value}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn callback_without_state_cookie_is_rejected(pool: PgPool) {
        let app = crate::api::router(state(pool));

        let response = app
            .oneshot(
                Request::get("/api/v1/auth/callback?code=c&state=anything")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn logout_clears_session_cookie_and_redirects_when_endpoint_known() {
        let pool = PgPool::connect_lazy("postgres://localhost/auau").unwrap();

        let mut app_state = state(pool.clone());
        app_state.oidc.end_session_url = Some(format!("{ISSUER}/api/v1/end-session"));
        let response = crate::api::router(app_state)
            .oneshot(
                Request::post("/api/v1/auth/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("{ISSUER}/api/v1/end-session")
        );
        let cookies = set_cookies(&response);
        assert_eq!(cookie_value(&cookies, SESSION_COOKIE), "");
        let raw_cookie = cookies.first().unwrap();
        assert!(raw_cookie.contains("Max-Age=0"), "{raw_cookie}");

        let response = crate::api::router(state(pool))
            .oneshot(
                Request::post("/api/v1/auth/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(cookie_value(&set_cookies(&response), SESSION_COOKIE), "");
    }
}
