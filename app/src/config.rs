use std::env;

#[derive(Clone)]
pub struct Config {
    pub oidc_issuer_url: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
    pub oidc_redirect_url: String,
    pub oidc_scopes: Vec<String>,
    pub oidc_ca_cert: Option<String>,
    pub cookie_secret: String,
    pub database_url: String,
    pub admin_secret: String,
    pub listen_addr: String,
    pub static_dir: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            oidc_issuer_url: env::var("OIDC_ISSUER_URL")
                .map_err(|_| "VNL-CFG-001: OIDC_ISSUER_URL is required".to_string())?,
            oidc_client_id: env::var("OIDC_CLIENT_ID")
                .map_err(|_| "VNL-CFG-002: OIDC_CLIENT_ID is required".to_string())?,
            oidc_client_secret: env::var("OIDC_CLIENT_SECRET")
                .map_err(|_| "VNL-CFG-003: OIDC_CLIENT_SECRET is required".to_string())?,
            oidc_redirect_url: env::var("OIDC_REDIRECT_URL")
                .map_err(|_| "VNL-CFG-004: OIDC_REDIRECT_URL is required".to_string())?,
            oidc_scopes: env::var("OIDC_SCOPES")
                .unwrap_or_else(|_| "openid,email,profile".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .collect(),
            oidc_ca_cert: env::var("OIDC_CA_CERT").ok(),
            cookie_secret: env::var("COOKIE_SECRET")
                .map_err(|_| "VNL-CFG-005: COOKIE_SECRET is required".to_string())?,
            database_url: env::var("DATABASE_URL")
                .map_err(|_| "VNL-CFG-006: DATABASE_URL is required".to_string())?,
            admin_secret: env::var("ADMIN_SECRET")
                .map_err(|_| "VNL-CFG-007: ADMIN_SECRET is required".to_string())?,
            listen_addr: env::var("LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            static_dir: env::var("STATIC_DIR")
                .unwrap_or_else(|_| "./static".to_string()),
        })
    }
}
