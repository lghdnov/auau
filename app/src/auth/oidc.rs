use async_trait::async_trait;
use openidconnect::core::CoreClient;
use openidconnect::reqwest::async_http_client;
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, ProviderMetadataWithLogout,
    RedirectUrl, TokenResponse,
};

use crate::config::Config;
use crate::error::AppError;

/// Identity claims extracted from a successful authorization-code exchange.
pub struct ExchangedUser {
    pub subject: String,
    pub name: String,
    pub email: String,
}

/// Turns an authorization code plus PKCE verifier into verified user claims.
///
/// The production implementation talks to the OIDC provider; tests substitute
/// a mock so no network access is needed.
#[async_trait]
pub trait CodeExchanger: Send + Sync {
    async fn exchange(&self, code: &str, verifier: &str) -> Result<ExchangedUser, AppError>;
}

/// Static OIDC wiring the auth routes need to build redirect URLs.
///
/// Kept as plain strings (no discovery) so login redirects work without
/// network access and are trivially testable.
#[derive(Debug, Clone)]
pub struct OidcSettings {
    /// Issuer base URL, e.g. `https://sso.example.com`.
    pub issuer: String,
    pub client_id: String,
    /// `{base_url}/api/v1/auth/callback`.
    pub redirect_uri: String,
    /// `end_session_endpoint` from discovery, when the provider publishes one.
    pub end_session_url: Option<String>,
}

impl OidcSettings {
    /// Derive the static settings from the app config.
    ///
    /// `end_session_url` comes from discovery at startup and is `None` when
    /// the provider does not publish an end-session endpoint.
    pub fn new(config: &Config, end_session_url: Option<String>) -> Self {
        Self {
            issuer: config.oidc_issuer.clone(),
            client_id: config.oidc_client_id.clone(),
            redirect_uri: redirect_uri(config),
            end_session_url,
        }
    }

    /// Pocket ID mounts the authorize endpoint at `{issuer}/authorize`.
    pub fn authorize_url(&self, state: &str, code_challenge: &str) -> String {
        let params = [
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid profile email"),
            ("state", state),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
        ];
        let query = params
            .iter()
            .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{}/authorize?{query}", self.issuer)
    }
}

/// A fresh `state` token and S256 PKCE challenge/verifier pair for one login.
///
/// All values are base64url-encoded, so they are safe to embed in the
/// `state|verifier` cookie and in the authorize URL.
pub struct LoginChallenge {
    pub state: String,
    pub verifier: String,
    pub code_challenge: String,
}

/// Generate a random state token and PKCE pair (both base64url-encoded).
pub fn new_login_challenge() -> LoginChallenge {
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    LoginChallenge {
        state: CsrfToken::new_random().secret().to_string(),
        verifier: verifier.secret().to_string(),
        code_challenge: challenge.as_str().to_string(),
    }
}

/// Redirect URI registered with the OIDC provider for the callback route.
fn redirect_uri(config: &Config) -> String {
    format!("{}/api/v1/auth/callback", config.base_url)
}

/// Accept any (or no) nonce: this flow is protected by `state` + PKCE.
fn no_nonce(_: Option<&Nonce>) -> Result<(), String> {
    Ok(())
}

/// Production `CodeExchanger` for Pocket ID.
///
/// Performs OIDC discovery (including the JWKS) once at startup and validates
/// ID-token signatures on every exchange.
pub struct PocketIdExchanger {
    client: CoreClient,
    end_session_url: Option<String>,
}

impl PocketIdExchanger {
    /// Discover the provider metadata at startup.
    ///
    /// Fails fast when the issuer is unreachable or misconfigured.
    pub async fn from_config(config: &Config) -> anyhow::Result<Self> {
        let issuer = IssuerUrl::new(config.oidc_issuer.clone())
            .map_err(|err| anyhow::anyhow!("invalid OIDC issuer {}: {err}", config.oidc_issuer))?;
        let metadata = ProviderMetadataWithLogout::discover_async(issuer, async_http_client)
            .await
            .map_err(|err| anyhow::anyhow!("OIDC discovery failed: {err}"))?;
        let end_session_url = metadata
            .additional_metadata()
            .end_session_endpoint
            .as_ref()
            .map(|url| url.url().to_string());

        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.oidc_client_id.clone()),
            Some(ClientSecret::new(config.oidc_client_secret.clone())),
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri(config))
                .map_err(|err| anyhow::anyhow!("invalid redirect URI: {err}"))?,
        );
        Ok(Self {
            client,
            end_session_url,
        })
    }

    /// The provider's `end_session_endpoint`, when it publishes one.
    pub fn end_session_url(&self) -> Option<String> {
        self.end_session_url.clone()
    }
}

#[async_trait]
impl CodeExchanger for PocketIdExchanger {
    async fn exchange(&self, code: &str, verifier: &str) -> Result<ExchangedUser, AppError> {
        let token_response = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_string()))
            .request_async(async_http_client)
            .await
            .map_err(|err| {
                AppError::Internal(anyhow::anyhow!("OIDC code exchange failed: {err}"))
            })?;

        let id_token = token_response.id_token().ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!("OIDC token response has no ID token"))
        })?;
        let claims = id_token
            .claims(&self.client.id_token_verifier(), no_nonce)
            .map_err(|err| {
                AppError::Internal(anyhow::anyhow!("ID token validation failed: {err}"))
            })?;

        if let Some(expected_hash) = claims.access_token_hash() {
            let actual_hash = AccessTokenHash::from_token(
                token_response.access_token(),
                &id_token.signing_alg().map_err(|err| {
                    AppError::Internal(anyhow::anyhow!("ID token has no signing alg: {err}"))
                })?,
            )
            .map_err(|err| {
                AppError::Internal(anyhow::anyhow!("cannot compute access token hash: {err}"))
            })?;
            if actual_hash != *expected_hash {
                return Err(AppError::Internal(anyhow::anyhow!(
                    "access token hash mismatch"
                )));
            }
        }

        let subject = claims.subject().as_str().to_string();
        let name = claims
            .name()
            .and_then(|name| name.get(None))
            .map(|name| name.as_str().to_string())
            .or_else(|| {
                claims
                    .preferred_username()
                    .map(|username| username.as_str().to_string())
            })
            .unwrap_or_else(|| subject.clone());
        let email = claims
            .email()
            .map(|email| email.as_str().to_string())
            .unwrap_or_default();
        Ok(ExchangedUser {
            subject,
            name,
            email,
        })
    }
}

/// E2E-only stand-in for the production exchanger (feature `e2e-auth-bypass`).
///
/// The Playwright stack runs without any IdP, so startup skips OIDC discovery
/// and this exchanger takes its place: a code exchange can never succeed —
/// sessions come from `POST /api/v1/auth/dev-login` instead.
#[cfg(feature = "e2e-auth-bypass")]
pub struct NoIdpExchanger;

#[cfg(feature = "e2e-auth-bypass")]
#[async_trait]
impl CodeExchanger for NoIdpExchanger {
    async fn exchange(&self, _code: &str, _verifier: &str) -> Result<ExchangedUser, AppError> {
        Err(AppError::Internal(anyhow::anyhow!(
            "OIDC login is unavailable in e2e builds; use /api/v1/auth/dev-login"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> OidcSettings {
        OidcSettings {
            issuer: "https://sso.example.com".into(),
            client_id: "auau-client".into(),
            redirect_uri: "http://localhost:5173/api/v1/auth/callback".into(),
            end_session_url: None,
        }
    }

    #[test]
    fn authorize_url_contains_all_required_params() {
        let url = settings().authorize_url("abc-state", "xyz-challenge");

        assert_eq!(
            url,
            "https://sso.example.com/authorize\
             ?client_id=auau-client\
             &redirect_uri=http%3A%2F%2Flocalhost%3A5173%2Fapi%2Fv1%2Fauth%2Fcallback\
             &response_type=code\
             &scope=openid%20profile%20email\
             &state=abc-state\
             &code_challenge=xyz-challenge\
             &code_challenge_method=S256"
        );
    }

    #[test]
    fn login_challenges_are_distinct_across_calls() {
        let first = new_login_challenge();
        let second = new_login_challenge();

        assert_ne!(first.state, second.state);
        assert_ne!(first.verifier, second.verifier);
        assert_ne!(first.code_challenge, second.code_challenge);
        assert!(!first.state.is_empty());
        assert!(!first.verifier.is_empty());
    }
}
