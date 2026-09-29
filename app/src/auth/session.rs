use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Session token lifetime: 12 hours.
const TTL_SECONDS: i64 = 12 * 3600;

/// HS256 secret used to sign and verify session tokens.
///
/// No `Debug` impl on purpose: the raw key must not leak into logs.
#[derive(Clone)]
pub struct SessionSecret(String);

impl SessionSecret {
    pub fn new(secret: impl Into<String>) -> Self {
        SessionSecret(secret.into())
    }
}

/// Why a session token was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// Malformed token, bad signature, or non-UUID subject.
    Invalid,
    /// `exp` has passed.
    Expired,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Invalid => write!(f, "invalid session token"),
            SessionError::Expired => write!(f, "session token expired"),
        }
    }
}

impl std::error::Error for SessionError {}

/// JWT claims for a session: `sub` is the user id, `exp` is unix seconds.
#[derive(Serialize, Deserialize)]
struct SessionClaims {
    sub: String,
    exp: i64,
}

/// Sign a session token for `user_id`, expiring 12 hours after `now_ts`.
pub fn issue(user_id: Uuid, secret: &SessionSecret, now_ts: i64) -> String {
    let claims = SessionClaims {
        sub: user_id.to_string(),
        exp: now_ts + TTL_SECONDS,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.0.as_bytes()),
    )
    .expect("HS256 signing with a valid secret cannot fail")
}

/// Verify a session token as of `now_ts` and return the user id it was issued for.
pub fn verify(token: &str, secret: &SessionSecret, now_ts: i64) -> Result<Uuid, SessionError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    let data = decode::<SessionClaims>(
        token,
        &DecodingKey::from_secret(secret.0.as_bytes()),
        &validation,
    )
    .map_err(|_| SessionError::Invalid)?;
    if data.claims.exp <= now_ts {
        return Err(SessionError::Expired);
    }
    data.claims
        .sub
        .parse::<Uuid>()
        .map_err(|_| SessionError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> SessionSecret {
        SessionSecret::new("test-session-secret")
    }

    #[test]
    fn issue_verify_roundtrip() {
        let user_id = Uuid::new_v4();
        let token = issue(user_id, &secret(), 1_000_000);

        assert_eq!(verify(&token, &secret(), 1_000_001).unwrap(), user_id);
    }

    #[test]
    fn expired_token_fails() {
        let user_id = Uuid::new_v4();
        let now = 1_000_000;
        let token = issue(user_id, &secret(), now);

        assert_eq!(
            verify(&token, &secret(), now + TTL_SECONDS - 1).unwrap(),
            user_id
        );
        assert_eq!(
            verify(&token, &secret(), now + TTL_SECONDS).unwrap_err(),
            SessionError::Expired
        );
        assert_eq!(
            verify(&token, &secret(), now + TTL_SECONDS + 1).unwrap_err(),
            SessionError::Expired
        );
    }

    #[test]
    fn tampered_token_fails() {
        let token = issue(Uuid::new_v4(), &secret(), 1_000_000);

        let (body, signature) = token.rsplit_once('.').unwrap();
        let (prefix, last) = signature.split_at(signature.len() - 1);
        let flipped_last = if last == "A" { "B" } else { "A" };
        let tampered = format!("{body}.{prefix}{flipped_last}");
        assert_ne!(tampered, token, "tampering must actually change the token");

        assert_eq!(
            verify(&tampered, &secret(), 1_000_001).unwrap_err(),
            SessionError::Invalid
        );
    }

    #[test]
    fn wrong_secret_fails() {
        let token = issue(Uuid::new_v4(), &SessionSecret::new("right"), 1_000_000);

        assert_eq!(
            verify(&token, &SessionSecret::new("wrong"), 1_000_001).unwrap_err(),
            SessionError::Invalid
        );
    }
}
