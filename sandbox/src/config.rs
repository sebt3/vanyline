use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Parser;

/// Défaut effectif de `jwks_url` quand ni `jwks_url` ni `jwks_file` ne sont
/// posés : le JWKS de l'apiserver, servi sur le service `kubernetes` du
/// cluster (jamais le `jwks_uri` annoncé par la découverte de l'apiserver, qui
/// expose l'IP du
/// nœud — vérifié sur cluster, cf. auth.sdd tâche `[-]`).
pub const DEFAULT_JWKS_URL: &str = "https://kubernetes.default.svc/openid/v1/jwks";

/// Chemin par défaut du CA du cluster (monté par Kubernetes dans tout pod).
const DEFAULT_JWKS_CA_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";

/// Chemin par défaut du jeton de `ServiceAccount` du pod (monté par Kubernetes) —
/// sert UNIQUEMENT de Bearer pour LIRE le JWKS, rien d'autre (auth.sdd).
const DEFAULT_JWKS_TOKEN_FILE: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";

/// Source des clés de vérification de jetons, résolue par `Config::jwks_source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwksSource {
    /// JWKS servi par une URL (cluster : `DEFAULT_JWKS_URL` par défaut).
    Url(String),
    /// Fichier JWKS local (développement hors cluster, tests).
    File(PathBuf),
}

/// Configuration complète du serveur sandbox : surface CLI/env, validation
/// d'amorçage et résolution de la source des clés (spec : `./config.sdd`).
///
/// AUCUNE combinaison de drapeaux ne démarre un serveur sans vérification de
/// jeton : aucun drapeau de contournement, aucun jeton statique, aucun
/// fournisseur d'identité externe.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "vanyline-sandbox",
    about = "Sandbox server — MCP, git, editor WebSocket, Owner-token secured"
)]
pub struct Config {
    /// Adresse du port principal (MCP, git, éditeur, `/code-health`)
    #[arg(long, default_value = "0.0.0.0:3000", env = "MCP_LISTEN")]
    pub listen: String,

    /// Certificat TLS (PEM) — absent ⇒ HTTP simple (reverse proxy en prod)
    #[arg(long, env = "MCP_TLS_CERT")]
    pub tls_cert: Option<String>,

    /// Clé TLS (PEM)
    #[arg(long, env = "MCP_TLS_KEY")]
    pub tls_key: Option<String>,

    /// Port d'exploitation : `/health` et `/metrics`, SANS authentification,
    /// jamais publié (ni Service, ni Ingress)
    #[arg(long, default_value = "0.0.0.0:9090", env = "OPS_LISTEN")]
    pub ops_listen: String,

    /// Sujet exact du jeton accepté : `system:serviceaccount:<ns>:owner-<nom>`
    #[arg(long, env = "VNL_OWNER_SUBJECT")]
    pub owner_subject: Option<String>,

    /// Claim `iss` attendu (le `--service-account-issuer` de l'apiserver)
    #[arg(long, env = "VNL_TOKEN_ISSUER")]
    pub token_issuer: Option<String>,

    /// Audience attendue des jetons d'Owner
    #[arg(long, default_value = "vanyline-sandbox", env = "VNL_TOKEN_AUDIENCE")]
    pub token_audience: String,

    /// URL du JWKS ; défaut effectif [`DEFAULT_JWKS_URL`] si ni `jwks_url` ni
    /// `jwks_file` ne sont posés
    #[arg(long, env = "VNL_JWKS_URL")]
    pub jwks_url: Option<String>,

    /// Fichier JWKS local (développement hors cluster, tests) — EXCLUSIF avec
    /// `jwks_url`
    #[arg(long, env = "VNL_JWKS_FILE")]
    pub jwks_file: Option<String>,

    /// CA PEM vérifiant le TLS du point JWKS
    #[arg(long, env = "VNL_JWKS_CA_FILE", default_value = DEFAULT_JWKS_CA_FILE)]
    pub jwks_ca_file: Option<String>,

    /// Fichier du jeton de `ServiceAccount` du pod — Bearer pour LIRE le JWKS,
    /// rien d'autre
    #[arg(long, env = "VNL_JWKS_TOKEN_FILE", default_value = DEFAULT_JWKS_TOKEN_FILE)]
    pub jwks_token_file: Option<String>,

    /// Endpoint OTLP gRPC pour l'export des traces OpenTelemetry ; absent ⇒
    /// `OTel` désactivé. Une panne du collecteur au démarrage n'est pas fatale.
    #[arg(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    pub otel_endpoint: Option<String>,

    /// Racine de confinement des outils, fs, git, PTY (existence non vérifiée ici)
    #[arg(
        long,
        default_value = "/home/vanyline/workspace",
        env = "VNL_SANDBOX_ROOT"
    )]
    pub sandbox_root: PathBuf,

    /// Délai de calme (s) avant recalcul de la santé du code (0 = immédiat)
    #[arg(long, default_value = "30", env = "VNL_HEALTH_COOLDOWN_SECS")]
    pub health_cooldown_secs: u64,

    /// Période de sondage de l'empreinte du worktree (s) (0 = désactivé)
    #[arg(long, default_value = "60", env = "VNL_HEALTH_POLL_SECS")]
    pub health_poll_secs: u64,

    /// URL d'ingress du Restate de l'application, seule destination des
    /// callbacks d'awakeable des jobs ; absente ⇒ jobs sans callback
    #[arg(long, env = "VNL_RESTATE_INGRESS_URL")]
    pub restate_ingress_url: Option<String>,
}

impl Config {
    /// Parse argv/env puis valide (helper de démarrage ; `main.rs` sépare les
    /// deux autour de l'init tracing pour ne pas perdre les logs).
    ///
    /// # Errors
    ///
    /// Comme [`Config::validate`] : identité d'Owner ou source de clés
    /// inexploitable.
    pub fn parse_and_validate() -> Result<Self> {
        let config = Self::parse();
        config.validate()?;
        Ok(config)
    }

    /// Résolution de la source des clés de vérification : `File` si `jwks_file`
    /// est posé, sinon `Url` (valeur posée, sinon [`DEFAULT_JWKS_URL`]).
    #[must_use]
    pub fn jwks_source(&self) -> JwksSource {
        if let Some(path) = &self.jwks_file {
            return JwksSource::File(PathBuf::from(path));
        }
        JwksSource::Url(
            self.jwks_url
                .clone()
                .unwrap_or_else(|| DEFAULT_JWKS_URL.to_string()),
        )
    }

    /// Validation d'amorçage : sans l'identité de l'Owner et une source de clés
    /// exploitables, le serveur ne démarre pas. Aucun drapeau ne permet de
    /// désactiver la vérification de jeton (config.sdd, auth.sdd).
    ///
    /// # Errors
    ///
    /// Message d'erreur unique décrivant le premier paramètre non exploitable
    /// (`owner_subject`, `token_issuer`, algorithme épinglé, paires TLS,
    /// `jwks_url`).
    pub fn validate(&self) -> Result<()> {
        let owner = self.owner_subject.as_deref().unwrap_or("");
        if owner.trim().is_empty() {
            bail!("--owner-subject (or VNL_OWNER_SUBJECT) is required");
        }
        if !owner.starts_with("system:serviceaccount:") {
            bail!("owner_subject must be system:serviceaccount:<namespace>:<name>");
        }
        let rest = &owner["system:serviceaccount:".len()..];
        let segments: Vec<&str> = rest.split(':').collect();
        if segments.len() != 2
            || segments[0].is_empty()
            || segments[1].is_empty()
            || owner.trim() != owner
        {
            bail!("owner_subject must be system:serviceaccount:<namespace>:<name>");
        }
        let issuer = self.token_issuer.as_deref().unwrap_or("");
        if issuer.trim().is_empty() {
            bail!("--token-issuer (or VNL_TOKEN_ISSUER) is required");
        }
        if self.token_audience.trim().is_empty() {
            bail!("token_audience must not be empty");
        }
        if self.jwks_url.is_some() && self.jwks_file.is_some() {
            bail!("--jwks-url and --jwks-file are mutually exclusive");
        }
        if self.jwks_url.as_deref().is_some_and(str::is_empty)
            || self.jwks_file.as_deref().is_some_and(str::is_empty)
        {
            bail!("jwks source must not be empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::path::Path;

    fn base() -> Config {
        Config {
            listen: "0.0.0.0:3000".into(),
            tls_cert: None,
            tls_key: None,
            ops_listen: "0.0.0.0:9090".into(),
            owner_subject: Some("system:serviceaccount:ns:owner-alice".into()),
            token_issuer: Some("https://kubernetes.default.svc.cluster.local".into()),
            token_audience: "vanyline-sandbox".into(),
            jwks_url: None,
            jwks_file: None,
            jwks_ca_file: Some(DEFAULT_JWKS_CA_FILE.into()),
            jwks_token_file: Some(DEFAULT_JWKS_TOKEN_FILE.into()),
            otel_endpoint: None,
            sandbox_root: Path::new("/home/vanyline/workspace").into(),
            health_cooldown_secs: 30,
            health_poll_secs: 60,
            restate_ingress_url: None,
        }
    }

    // Scenario: configuration_nominale
    #[test]
    fn configuration_nominale() {
        let c = base();
        c.validate().unwrap();
        assert_eq!(c.token_audience, "vanyline-sandbox");
        assert_eq!(c.jwks_source(), JwksSource::Url(DEFAULT_JWKS_URL.into()));
    }

    // Scenario: owner_subject_requis
    #[test]
    fn owner_subject_requis() {
        for owner in [None, Some(String::new()), Some("   ".into())] {
            let mut c = base();
            c.owner_subject = owner;
            let err = c.validate().unwrap_err().to_string();
            assert_eq!(err, "--owner-subject (or VNL_OWNER_SUBJECT) is required");
        }
    }

    // Scenario: owner_subject_forme_invalide
    #[test]
    fn owner_subject_forme_invalide() {
        for owner in [
            "alice",
            "system:serviceaccount:ns",
            "system:serviceaccount::owner-a",
        ] {
            let mut c = base();
            c.owner_subject = Some(owner.into());
            let err = c.validate().unwrap_err().to_string();
            assert_eq!(
                err,
                "owner_subject must be system:serviceaccount:<namespace>:<name>"
            );
        }
    }

    // Scenario: issuer_requis
    #[test]
    fn issuer_requis() {
        for issuer in [None, Some(String::new()), Some("  ".into())] {
            let mut c = base();
            c.token_issuer = issuer;
            let err = c.validate().unwrap_err().to_string();
            assert_eq!(err, "--token-issuer (or VNL_TOKEN_ISSUER) is required");
        }
    }

    #[test]
    fn audience_vide_refusee() {
        let mut c = base();
        c.token_audience = String::new();
        let err = c.validate().unwrap_err().to_string();
        assert_eq!(err, "token_audience must not be empty");
    }

    // Scenario: sources_de_cles_exclusives
    #[test]
    fn sources_de_cles_exclusives() {
        let mut c = base();
        c.jwks_url = Some("https://example.com/jwks".into());
        c.jwks_file = Some("/tmp/jwks.json".into());
        let err = c.validate().unwrap_err().to_string();
        assert_eq!(err, "--jwks-url and --jwks-file are mutually exclusive");
    }

    // Scenario: jwks_file_prime_en_mode_dev
    #[test]
    fn jwks_file_prime_en_mode_dev() {
        let mut c = base();
        c.jwks_url = None;
        c.jwks_file = Some("/tmp/jwks.json".into());
        assert_eq!(c.jwks_source(), JwksSource::File("/tmp/jwks.json".into()));
    }

    // Scenario: source_vide_refusee
    #[test]
    fn source_vide_refusee() {
        let mut c = base();
        c.jwks_url = Some(String::new());
        let err = c.validate().unwrap_err().to_string();
        assert_eq!(err, "jwks source must not be empty");

        let mut c = base();
        c.jwks_file = Some(String::new());
        c.jwks_url = None;
        let err = c.validate().unwrap_err().to_string();
        assert_eq!(err, "jwks source must not be empty");
    }

    // Scenario: aucun_drapeau_de_contournement
    // La liste noire nomme les drapeaux supprimés — c'est l'énoncé même du
    // Scenario (config.sdd) : ils ne doivent plus exister dans `Config`.
    #[test]
    fn aucun_drapeau_de_contournement() {
        use clap::CommandFactory;
        let forbidden = [
            "no-auth",
            "static-token",
            "insecure",
            "public-url",
            "oidc-issuer",
            "oidc-audience",
            "oidc-ca-cert",
            "auth-groups-admin",
            "auth-groups-read",
        ];
        for arg in Config::command().get_arguments() {
            if let Some(long) = arg.get_long() {
                assert!(
                    !forbidden.contains(&long),
                    "drapeau de contournement retrouvé dans `Config`: --{long}"
                );
            }
        }
    }

    // Scenario: ports_par_defaut
    #[test]
    fn ports_par_defaut() {
        let c = Config::try_parse_from(["vanyline-sandbox"]).unwrap();
        assert_eq!(c.listen, "0.0.0.0:3000");
        assert_eq!(c.ops_listen, "0.0.0.0:9090");
    }

    // Scenario: sandbox_root_par_defaut
    #[test]
    fn sandbox_root_par_defaut() {
        let c = Config::try_parse_from(["vanyline-sandbox"]).unwrap();
        assert_eq!(c.sandbox_root, Path::new("/home/vanyline/workspace"));
    }

    // Scenario: cooldown_par_defaut
    #[test]
    fn scenario_cooldown_par_defaut() {
        let c = Config::try_parse_from(["vanyline-sandbox"]).unwrap();
        assert_eq!(c.health_cooldown_secs, 30);
        assert_eq!(c.health_poll_secs, 60);
    }

    #[test]
    fn validate_pass_with_jwks_file_only() {
        let mut c = base();
        c.jwks_file = Some("/tmp/jwks.json".into());
        c.validate().unwrap();
    }
}
