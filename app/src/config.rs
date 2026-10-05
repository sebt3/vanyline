use std::env;

#[derive(Clone)]
pub struct Config {
    pub oidc_issuer_url: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
    pub oidc_redirect_url: String,
    pub oidc_scopes: Vec<String>,
    pub oidc_ca_cert: Option<String>,
    /// Délai de connexion du client OIDC — `OIDC_CONNECT_TIMEOUT_SECS` (@u64 de
    /// secondes, défaut 5) ; absente/vide/non parseable ⇒ défaut, jamais d'échec.
    pub oidc_connect_timeout: std::time::Duration,
    /// Délai global du client OIDC — `OIDC_TIMEOUT_SECS` (@u64 de secondes, défaut
    /// 15) ; même contrat de parse et de repli que `oidc_connect_timeout`.
    pub oidc_timeout: std::time::Duration,
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
        Self::from_env_with(|key| env::var(key))
    }

    fn from_env_with(read: impl Fn(&str) -> Result<String, env::VarError>) -> Result<Self, String> {
        // Timeout à défaut : parse @u64 de secondes, repli silencieux sur le défaut quand
        // la variable est absente, vide ou non parseable — jamais d'échec, jamais de log.
        let secs_or_default = |key: &str, default: u64| -> u64 {
            read(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        Ok(Self {
            oidc_issuer_url: read("OIDC_ISSUER_URL")
                .map_err(|_| "VNL-CFG-001: OIDC_ISSUER_URL is required".to_string())?,
            oidc_client_id: read("OIDC_CLIENT_ID")
                .map_err(|_| "VNL-CFG-002: OIDC_CLIENT_ID is required".to_string())?,
            oidc_client_secret: read("OIDC_CLIENT_SECRET")
                .map_err(|_| "VNL-CFG-003: OIDC_CLIENT_SECRET is required".to_string())?,
            oidc_redirect_url: read("OIDC_REDIRECT_URL")
                .map_err(|_| "VNL-CFG-004: OIDC_REDIRECT_URL is required".to_string())?,
            oidc_scopes: read("OIDC_SCOPES")
                .unwrap_or_else(|_| "openid,email,profile".to_string())
                .split(',')
                .map(|s| s.trim().to_string())
                .collect(),
            oidc_ca_cert: read("OIDC_CA_CERT").ok(),
            oidc_connect_timeout: std::time::Duration::from_secs(secs_or_default(
                "OIDC_CONNECT_TIMEOUT_SECS",
                5,
            )),
            oidc_timeout: std::time::Duration::from_secs(secs_or_default("OIDC_TIMEOUT_SECS", 15)),
            cookie_secret: read("COOKIE_SECRET")
                .map_err(|_| "VNL-CFG-005: COOKIE_SECRET is required".to_string())?,
            database_url: read("DATABASE_URL")
                .map_err(|_| "VNL-CFG-006: DATABASE_URL is required".to_string())?,
            listen_addr: read("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            static_dir: read("STATIC_DIR").unwrap_or_else(|_| "./static".to_string()),
            k8s_namespace: read("VNL_K8S_NAMESPACE").ok(),
            application_name: read("VNL_APPLICATION_NAME").ok(),
            default_home_storage_class: read("VNL_DEFAULT_HOME_STORAGE_CLASS").ok(),
            default_home_access_mode: read("VNL_DEFAULT_HOME_ACCESS_MODE").ok(),
            default_project_storage_class: read("VNL_DEFAULT_PROJECT_STORAGE_CLASS").ok(),
            default_project_access_mode: read("VNL_DEFAULT_PROJECT_ACCESS_MODE").ok(),
        })
    }

    /// Mapping Config → `OidcConfig` miryad-core. Les redirects post-login/logout ne sont plus
    /// portés par `OidcConfig` (miryad-core ≥ 0.1.5) : `MiryadAuthState` en est la seule
    /// source de vérité et ./main.rs les fige sur `"/#/"` (comportement SPA historique).
    /// Les timeouts (`connect_timeout`, `timeout`) sont copiés champ à champ depuis
    /// `@Config` (issus de `OIDC_CONNECT_TIMEOUT_SECS` / `OIDC_TIMEOUT_SECS`, défauts
    /// 5 s / 15 s, défauts applicatifs recommandés par miryad-core).
    pub fn oidc_config(&self) -> miryad_core::auth::OidcConfig {
        miryad_core::auth::OidcConfig {
            issuer_url: self.oidc_issuer_url.clone(),
            client_id: self.oidc_client_id.clone(),
            // L'app exige OIDC_CLIENT_SECRET (VNL-CFG-003) : toujours confidentiel, jamais
            // client public (le `None` de miryad-core est réservé aux flux PKCE seul).
            client_secret: Some(self.oidc_client_secret.clone()),
            redirect_url: self.oidc_redirect_url.clone(),
            scopes: self.oidc_scopes.clone(),
            ca_cert: self.oidc_ca_cert.clone(),
            connect_timeout: self.oidc_connect_timeout,
            timeout: self.oidc_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::HashMap;
    use std::env::VarError;

    /// Environnement en mémoire pour le noyau `from_env_with` : variable absente de la
    /// carte ⇒ `VarError::NotPresent` (déterministe, parallèle-sûr, zéro `unsafe`).
    fn env_read(map: HashMap<&'static str, String>) -> impl Fn(&str) -> Result<String, VarError> {
        move |key| map.get(key).cloned().ok_or(VarError::NotPresent)
    }

    /// Les six requises posées (contrat des Scenarios « environnement complet en
    /// requises »).
    fn required_env() -> HashMap<&'static str, String> {
        HashMap::from([
            ("OIDC_ISSUER_URL", "https://issuer.example.com".to_string()),
            ("OIDC_CLIENT_ID", "client-id".to_string()),
            ("OIDC_CLIENT_SECRET", "client-secret".to_string()),
            (
                "OIDC_REDIRECT_URL",
                "https://app.example.com/auth/callback".to_string(),
            ),
            ("COOKIE_SECRET", "0".repeat(64)),
            ("DATABASE_URL", "postgres://localhost/test".to_string()),
        ])
    }

    fn full_config() -> Config {
        Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/auth/callback".to_string(),
            oidc_scopes: vec!["openid".to_string(), "email".to_string()],
            oidc_ca_cert: Some("pem".to_string()),
            // Timeouts distincts des défauts (5 s / 15 s) : prouvent la copie champ à
            // champ dans `oidc_config_copie_les_huit_champs`.
            oidc_connect_timeout: std::time::Duration::from_secs(7),
            oidc_timeout: std::time::Duration::from_secs(23),
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

    /// Scenario: `oidc_config_copie_les_huit_champs`
    #[test]
    fn oidc_config_copie_les_huit_champs() {
        let config = full_config();
        let oidc = config.oidc_config();

        assert_eq!(oidc.issuer_url, "https://issuer.example.com");
        assert_eq!(oidc.client_id, "client-id");
        assert_eq!(oidc.client_secret.as_deref(), Some("client-secret"));
        assert_eq!(oidc.redirect_url, "https://app.example.com/auth/callback");
        assert_eq!(oidc.scopes, vec!["openid".to_string(), "email".to_string()]);
        assert_eq!(oidc.ca_cert.as_deref(), Some("pem"));
        // 7 s / 23 s (valeurs du littéral, distinctes des défauts 5 s / 15 s) : la copie
        // des champs @Duration de @Config est prouvée, pas une valeur figée.
        assert_eq!(oidc.connect_timeout, std::time::Duration::from_secs(7));
        assert_eq!(oidc.timeout, std::time::Duration::from_secs(23));
    }

    /// Scenario: `from_env_defaut_timeouts`
    #[test]
    fn from_env_defaut_timeouts() {
        // Requises présentes, OIDC_CONNECT_TIMEOUT_SECS / OIDC_TIMEOUT_SECS absentes.
        let config = Config::from_env_with(env_read(required_env())).unwrap();

        assert_eq!(
            config.oidc_connect_timeout,
            std::time::Duration::from_secs(5)
        );
        assert_eq!(config.oidc_timeout, std::time::Duration::from_secs(15));
    }

    /// Scenario: `from_env_timeouts_fournis`
    #[test]
    fn from_env_timeouts_fournis() {
        let mut env = required_env();
        env.insert("OIDC_CONNECT_TIMEOUT_SECS", "7".to_string());
        env.insert("OIDC_TIMEOUT_SECS", "30".to_string());

        let config = Config::from_env_with(env_read(env)).unwrap();

        assert_eq!(
            config.oidc_connect_timeout,
            std::time::Duration::from_secs(7)
        );
        assert_eq!(config.oidc_timeout, std::time::Duration::from_secs(30));
        // Propagation dans @oidc_config (champs homonymes).
        let oidc = config.oidc_config();
        assert_eq!(oidc.connect_timeout, std::time::Duration::from_secs(7));
        assert_eq!(oidc.timeout, std::time::Duration::from_secs(30));
    }

    /// Scenario: `from_env_timeout_non_parseable_defaut`
    #[test]
    fn from_env_timeout_non_parseable_defaut() {
        let mut env = required_env();
        env.insert("OIDC_TIMEOUT_SECS", "abc".to_string());
        // Posée vide : le défaut s'applique aussi (la valeur vide n'est pas conservée).
        env.insert("OIDC_CONNECT_TIMEOUT_SECS", String::new());

        // @Ok : aucune erreur pour une valeur de défaut non parseable.
        let config = Config::from_env_with(env_read(env)).unwrap();

        assert_eq!(config.oidc_timeout, std::time::Duration::from_secs(15));
        assert_eq!(
            config.oidc_connect_timeout,
            std::time::Duration::from_secs(5)
        );
    }

    /// Scenario: `from_env_timeouts_classees_a_defaut`
    #[test]
    fn from_env_timeouts_classees_a_defaut() {
        // Requises présentes, variables à défaut absentes, les SEPT optionnelles absentes
        // (`OIDC_CA_CERT`, `VNL_K8S_NAMESPACE`, `VNL_APPLICATION_NAME`, les quatre
        // `VNL_DEFAULT_*`).
        let config = Config::from_env_with(env_read(required_env())).unwrap();

        // Les timeouts ne sont PAS des @Option : @Duration bruts aux défauts 5 s / 15 s,
        // classement « à défaut » verrouillé (jamais d'échec, jamais @None).
        assert_eq!(
            config.oidc_connect_timeout,
            std::time::Duration::from_secs(5)
        );
        assert_eq!(config.oidc_timeout, std::time::Duration::from_secs(15));
        // Distinction avec les sept @Option, toutes @None ici.
        assert!(config.oidc_ca_cert.is_none());
        assert!(config.k8s_namespace.is_none());
        assert!(config.application_name.is_none());
        assert!(config.default_home_storage_class.is_none());
        assert!(config.default_home_access_mode.is_none());
        assert!(config.default_project_storage_class.is_none());
        assert!(config.default_project_access_mode.is_none());
    }
}
