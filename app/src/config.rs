use std::path::PathBuf;

/// Runtime configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub storage_dir: PathBuf,
    pub base_url: String,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
    pub session_secret: String,
    pub max_upload_mb: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = required("DATABASE_URL")?;
        let storage_dir = std::env::var("STORAGE_DIR").unwrap_or_else(|_| "./data".into());
        let base_url = required("BASE_URL")?;
        let oidc_issuer = required("OIDC_ISSUER")?;
        let oidc_client_id = required("OIDC_CLIENT_ID")?;
        let oidc_client_secret = required("OIDC_CLIENT_SECRET")?;
        let session_secret = required("SESSION_SECRET")?;
        let max_upload_mb = std::env::var("MAX_UPLOAD_MB")
            .unwrap_or_else(|_| "500".into())
            .parse::<u64>()
            .map_err(|err| anyhow::anyhow!("invalid MAX_UPLOAD_MB: {err}"))?;

        Ok(Self {
            database_url,
            storage_dir: storage_dir.into(),
            base_url,
            oidc_issuer,
            oidc_client_id,
            oidc_client_secret,
            session_secret,
            max_upload_mb,
        })
    }

    pub fn max_upload_bytes(&self) -> usize {
        self.max_upload_mb as usize * 1024 * 1024
    }
}

/// Read a required environment variable, failing with a descriptive error.
fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .map_err(|_| anyhow::anyhow!("required environment variable {name} is not set"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Env var mutation is process-global: serialize env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_reads_env_with_defaults() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DATABASE_URL", "postgres://localhost/auau");
        std::env::set_var("BASE_URL", "http://localhost:5173");
        std::env::set_var("OIDC_ISSUER", "https://sso.example.com");
        std::env::set_var("OIDC_CLIENT_ID", "auau-client");
        std::env::set_var("OIDC_CLIENT_SECRET", "oidc-secret");
        std::env::set_var("SESSION_SECRET", "session-secret");
        std::env::remove_var("STORAGE_DIR");
        std::env::remove_var("MAX_UPLOAD_MB");

        let config = Config::from_env().unwrap();

        assert_eq!(config.database_url, "postgres://localhost/auau");
        assert_eq!(config.storage_dir, PathBuf::from("./data"));
        assert_eq!(config.base_url, "http://localhost:5173");
        assert_eq!(config.oidc_issuer, "https://sso.example.com");
        assert_eq!(config.oidc_client_id, "auau-client");
        assert_eq!(config.oidc_client_secret, "oidc-secret");
        assert_eq!(config.session_secret, "session-secret");
        assert_eq!(config.max_upload_mb, 500);
        assert_eq!(config.max_upload_bytes(), 500 * 1024 * 1024);
    }

    #[test]
    fn config_reads_max_upload_mb_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("DATABASE_URL", "postgres://localhost/auau");
        std::env::set_var("BASE_URL", "http://localhost:5173");
        std::env::set_var("OIDC_ISSUER", "https://sso.example.com");
        std::env::set_var("OIDC_CLIENT_ID", "auau-client");
        std::env::set_var("OIDC_CLIENT_SECRET", "oidc-secret");
        std::env::set_var("SESSION_SECRET", "session-secret");
        std::env::set_var("STORAGE_DIR", "/tmp/auau-data");
        std::env::set_var("MAX_UPLOAD_MB", "2");

        let config = Config::from_env().unwrap();

        assert_eq!(config.storage_dir, PathBuf::from("/tmp/auau-data"));
        assert_eq!(config.max_upload_mb, 2);
        assert_eq!(config.max_upload_bytes(), 2 * 1024 * 1024);
    }

    #[test]
    fn config_missing_required_var_returns_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("DATABASE_URL");

        let result = Config::from_env();

        assert!(result.is_err());
    }
}
