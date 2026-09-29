pub mod session;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use chrono::Utc;

use crate::api::AppState;
use crate::db::users;
use crate::error::AppError;
use crate::models::User;

/// Name of the http-only session cookie.
pub const SESSION_COOKIE: &str = "auau_session";

/// Extractor resolving the signed-in user from the `auau_session` cookie.
///
/// Any missing/invalid cookie or unknown user yields `AppError::Unauthorized`;
/// a database failure is reported as an internal error instead.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_cookie(parts).ok_or(AppError::Unauthorized)?;
        let now_ts = Utc::now().timestamp();
        let user_id = session::verify(token, &state.session_secret, now_ts)
            .map_err(|_| AppError::Unauthorized)?;
        let user = users::find_by_id(&state.pool, user_id)
            .await
            .map_err(AppError::from)?
            .ok_or(AppError::Unauthorized)?;
        Ok(CurrentUser(user))
    }
}

/// Pull the `auau_session` cookie value out of the request headers.
fn session_cookie(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|header| header.split(';'))
        .map(|cookie| cookie.trim())
        .find_map(|cookie| cookie.strip_prefix(SESSION_COOKIE))
        .and_then(|rest| rest.strip_prefix('='))
}
