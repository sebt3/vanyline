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
    pub listen_addr: String,
    pub static_dir: String,
    pub k8s_namespace: Option<String>,
    /// Nom de la CR Application dont ce pod `app` est le Deployment — posé par
    /// le reconciler Application (`VNL_APPLICATION_NAME`). Utilisé par
    /// `ensure_owner` pour remplir `OwnerSpec.application_ref` lors de ses
    /// créations lazily (condition d'exposition publique des Sandboxes).
    pub application_name: Option<String>,
    /// Défauts de stockage propagés vers `OwnerSpec`/`ProjectSpec` lors des
    /// créations lazily par `ensure_owner`/`create_project` — posés en env
    /// par le reconciler Application depuis `Application.spec.storageDefaults`
    /// (None si le champ CRD est absent). Jamais requis : les reconcilers
    /// Owner/Project ont leurs propres défauts historiques (RWX/RWO).
    pub default_home_storage_class: Option<String>,
    pub default_home_access_mode: Option<String>,
    pub default_project_storage_class: Option<String>,
    pub default_project_access_mode: Option<String>,
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
            listen_addr: env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            static_dir: env::var("STATIC_DIR").unwrap_or_else(|_| "./static".to_string()),
            k8s_namespace: env::var("VNL_K8S_NAMESPACE").ok(),
            application_name: env::var("VNL_APPLICATION_NAME").ok(),
            default_home_storage_class: env::var("VNL_DEFAULT_HOME_STORAGE_CLASS").ok(),
            default_home_access_mode: env::var("VNL_DEFAULT_HOME_ACCESS_MODE").ok(),
            default_project_storage_class: env::var("VNL_DEFAULT_PROJECT_STORAGE_CLASS").ok(),
            default_project_access_mode: env::var("VNL_DEFAULT_PROJECT_ACCESS_MODE").ok(),
        })
    }

    /// Mapping Config → OidcConfig miryad-core. Les redirects post-login/logout sont
    /// figés sur le comportement SPA actuel de vanyline (`/#/`).
    pub fn oidc_config(&self) -> miryad_core::auth::OidcConfig {
        miryad_core::auth::OidcConfig {
            issuer_url: self.oidc_issuer_url.clone(),
            client_id: self.oidc_client_id.clone(),
            client_secret: self.oidc_client_secret.clone(),
            redirect_url: self.oidc_redirect_url.clone(),
            scopes: self.oidc_scopes.clone(),
            ca_cert: self.oidc_ca_cert.clone(),
            post_login_redirect: "/#/".to_string(),
            post_logout_redirect: "/#/".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn full_config() -> Config {
        Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/auth/callback".to_string(),
            oidc_scopes: vec!["openid".to_string(), "email".to_string()],
            oidc_ca_cert: Some("pem".to_string()),
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
        }
    }

    #[test]
    fn oidc_config_maps_all_fields() {
        let config = full_config();
        let oidc = config.oidc_config();

        assert_eq!(oidc.issuer_url, "https://issuer.example.com");
        assert_eq!(oidc.client_id, "client-id");
        assert_eq!(oidc.client_secret, "client-secret");
        assert_eq!(oidc.redirect_url, "https://app.example.com/auth/callback");
        assert_eq!(oidc.scopes, vec!["openid".to_string(), "email".to_string()]);
        assert_eq!(oidc.ca_cert.as_deref(), Some("pem"));
        assert_eq!(oidc.post_login_redirect, "/#/");
        assert_eq!(oidc.post_logout_redirect, "/#/");
    }
}
