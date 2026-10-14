//! Authentification HTTP du serveur sandbox (spec : `./auth.sdd`).
//!
//! Un seul mécanisme : le jeton JWT émis par l'apiserver Kubernetes pour le
//! `ServiceAccount` de l'Owner de la sandbox (`owner-<nom>`, API `TokenRequest`,
//! audience `vanyline-sandbox`). La sandbox n'a PAS de niveaux d'accès :
//! soit l'appelant prouve qu'il est cet Owner, soit il n'entre pas.
//!
//! Pas de mode sans authentification, pas de jeton statique, pas de
//! fournisseur d'identité externe, pas
//! de groupes. Le développement hors cluster passe par `jwks_file` + jetons de
//! test signés (`test_support`), jamais par un contournement.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, EllipticCurve, JwkSet},
};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::AppState;
use crate::config::{Config, JwksSource};

/// TTL du cache JWKS, horloge monotone [`Instant`] (comparaison stricte `>`).
const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Bornage anti-amplification du refresh forcé déclenché par un `kid` inconnu :
/// un seul refresh par fenêtre de 30 s (un jeton poubelle à `kid` inconnu ne
/// peut pas déclencher de trafic sortant illimité).
const FORCED_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Plancher d'intervalle des recharges déclenchées par l'expiration du cache
/// (revue 2026-10-14, auth.sdd `Handles`) : même valeur que le refresh forcé —
/// en échec (apiserver injoignable), une requête sur deux ne peut plus bloquer
/// 5 s sur un connect timed-out entre deux tentatives rapprochées.
const EXPIRED_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Timeout explicite de toute requête réseau du fichier (connect + lecture).
const JWKS_HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Tolérance d'horloge maximale sur `exp`/`nbf` (secondes).
const CLOCK_LEEWAY_SECS: u64 = 30;

/// Valeur `WWW-Authenticate` des 401. Sans `resource_metadata` : aucune
/// découverte OAuth n'existe (cf. `./mcp.sdd`) — d'où une valeur statique,
/// indépendante de toute URL d'opérateur.
const WWW_AUTHENTICATE_VALUE: &str = "Bearer error=\"invalid_token\"";

/// Identité authentifiée, injectée par `require_auth` dans les extensions de la
/// requête. Volontairement minimal : aucune notion de rôle ne traverse ce
/// fichier (`auth.sdd` `Must`).
#[derive(Debug, Clone)]
pub struct AuthInfo {
    /// Claim `sub` du jeton — vaut nécessairement `config.owner_subject`.
    pub subject: String,
}

/// Erreurs d'authentification, enveloppées en réponses HTTP par `IntoResponse`.
/// Les `Display` sont verrouillés byte par byte avec leur identifiant
/// `VNL-SBX-AUTH-NNN` (auth.sdd `Exposes`).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// En-tête `Authorization: Bearer` absent, non UTF-8 ou sans le préfixe
    /// exact `Bearer ` (401).
    #[error("VNL-SBX-AUTH-001: missing or malformed Authorization header")]
    MissingToken,
    /// Jeton invalide ou expiré : header indécodable, sans `kid`, signature,
    /// `iss`, `aud`, `exp`, `nbf` non conformes — toutes causes effondrées (401).
    #[error("VNL-SBX-AUTH-002: invalid or expired token")]
    InvalidToken,
    /// Clés de vérification indisponibles et aucun cache : panne amont, jamais
    /// un 401 qui signalerait à tort un jeton éventuellement valide (503).
    #[error("VNL-SBX-AUTH-003: cannot obtain token verification keys")]
    KeysUnavailable,
    /// Le `kid` du jeton n'est dans aucune clé du JWKS, après au plus un
    /// refresh forcé (401).
    #[error("VNL-SBX-AUTH-004: signing key not found")]
    KeyNotFound,
    /// Type de clé ou algorithme non conformes au pinning par type du JWKS
    /// (RSA → RS256, EC P-256 → ES256 ; jamais `none`, `HS*`) (401).
    #[error("VNL-SBX-AUTH-005: unsupported signing algorithm")]
    UnsupportedAlgorithm,
    /// Jeton parfaitement valide d'un AUTRE Owner : identité prouvée,
    /// appartenance non (403).
    #[error("VNL-SBX-AUTH-006: token does not belong to this sandbox's owner")]
    NotOwner,
}

impl AuthError {
    /// Étiquette du compteur `mcp_auth_errors_total{reason}` pour cette erreur.
    fn reason(&self) -> &'static str {
        match self {
            AuthError::MissingToken => "missing_token",
            AuthError::InvalidToken => "invalid_token",
            AuthError::KeysUnavailable => "keys_unavailable",
            AuthError::KeyNotFound => "key_not_found",
            AuthError::UnsupportedAlgorithm => "unsupported_algorithm",
            AuthError::NotOwner => "not_owner",
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_code) = match &self {
            AuthError::MissingToken
            | AuthError::InvalidToken
            | AuthError::KeyNotFound
            | AuthError::UnsupportedAlgorithm => (StatusCode::UNAUTHORIZED, "invalid_token"),
            AuthError::NotOwner => (StatusCode::FORBIDDEN, "forbidden"),
            AuthError::KeysUnavailable => {
                (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable")
            }
        };

        let mut resp = (
            status,
            Json(serde_json::json!({
                "error": error_code,
                "error_description": self.to_string(),
            })),
        )
            .into_response();

        if status == StatusCode::UNAUTHORIZED {
            resp.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                HeaderValue::from_static(WWW_AUTHENTICATE_VALUE),
            );
        }

        resp
    }
}

// ── État d'authentification ───────────────────────────────────────────────────

struct JwksCache {
    jwk_set: JwkSet,
    fetched_at: Instant,
}

/// Claims minimales lues après vérification : seul `sub` intéresse (comparé à
/// `owner_subject` par la suite).
#[derive(Deserialize)]
struct TokenClaims {
    sub: String,
}

/// Cache d'un résultat de lookup de clé, classé pour l'ordre d'évaluation de
/// `Raises` : absent (→ refresh forcé puis `KeyNotFound`), présent et
/// utilisable, ou présent mais de type non autorisé (→ `UnsupportedAlgorithm`,
/// SANS refresh — la clé est là, c'est son type qui est refusé).
enum KeyLookup {
    Missing,
    Found(DecodingKey, Algorithm),
    WrongType,
}

/// État d'authentification, partagé par `Arc` dans `AppState`. Non-`Clone` : un
/// seul cache et un seul throttle par processus.
pub struct AuthState {
    config: Arc<Config>,
    cache: RwLock<Option<JwksCache>>,
    /// Horodatage du dernier refresh FORCÉ (déclenché par `kid` inconnu) ;
    /// borné à un toutes les 30 s. Le garde est tenu À TRAVERS le refresh :
    /// serialise aussi les tentatives concurrentes.
    last_forced_refresh: Mutex<Option<Instant>>,
    /// Horodatage de la dernière TENTATIVE de recharge d'expiration (cache
    /// périmé) ; même plancher de 30 s que le refresh forcé (revue
    /// 2026-10-14, auth.sdd `Handles`) : en échec, une requête sur deux ne
    /// peut plus bloquer 5 s sur un connect timed-out. Le verrou est tenu À
    /// TRAVERS la tentative : sérialise les requêtes concurrentes.
    last_expired_refresh: Mutex<Option<Instant>>,
    http: reqwest::Client,
}

impl AuthState {
    /// Construit l'état. Échoue (erreur `anyhow`, fatale au démarrage) si un
    /// paramètre requis est vide, si `jwks_ca_file` est illisible ou non-PEM
    /// (source `Url`), si `jwks_file` est illisible ou ne parse pas comme JWKS
    /// (source `File` — pré-lecture fail-fast, `auth.sdd` `Raises` 2026-10-08),
    /// ou si le client HTTP ne peut pas être construit.
    ///
    /// # Errors
    ///
    /// Erreur `anyhow` si un paramètre requis est vide, si le fichier CA est
    /// illisible ou non-PEM, si le fichier JWKS est illisible ou non-JWKS, ou
    /// si le client HTTP est impossible à construire.
    pub fn new(config: Arc<Config>) -> Result<Self> {
        Self::check_params(&config)?;
        // Fail-fast 2026-10-08 (`auth.sdd` Raises / Scenario
        // `aucun_contournement`) : en mode `File`, la source de clés est
        // pré-lue et parsée ici — on ne démarre pas sur du matériel de
        // vérification non exploitable (jamais un serveur ouvert rendant 503
        // à chaque requête).
        if let JwksSource::File(path) = config.jwks_source() {
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("reading JWKS file: {}", path.display()))?;
            serde_json::from_str::<JwkSet>(&body)
                .with_context(|| format!("parsing JWKS file: {}", path.display()))?;
        }
        let mut builder = reqwest::Client::builder().timeout(JWKS_HTTP_TIMEOUT);
        // Le CA ne sert qu'à vérifier le TLS du point JWKS : inutile (et
        // contre-productif hors cluster) de l'exiger en mode `jwks_file`.
        if let JwksSource::Url(_) = config.jwks_source()
            && let Some(ca_path) = &config.jwks_ca_file
        {
            let pem = std::fs::read(ca_path)
                .with_context(|| format!("reading JWKS CA file: {ca_path}"))?;
            // Garde explicite : `reqwest::Certificate::from_pem` peut accepter
            // des octets sans bloc PEM ; la spec exige le refus d'un CA
            // « illisible OU non-PEM ».
            if !pem.windows("-----BEGIN".len()).any(|w| w == b"-----BEGIN") {
                bail!("JWKS CA file is not a PEM document: {ca_path}");
            }
            let cert = reqwest::Certificate::from_pem(&pem)
                .with_context(|| format!("parsing JWKS CA file (PEM expected): {ca_path}"))?;
            builder = builder.add_root_certificate(cert);
        }
        let http = builder.build().context("building JWKS HTTP client")?;
        Ok(Self {
            config,
            cache: RwLock::new(None),
            last_forced_refresh: Mutex::new(None),
            last_expired_refresh: Mutex::new(None),
            http,
        })
    }

    /// Revérifie les paramètres requis (jamais de repli sur une chaîne vide) —
    /// ``Config::validate`` est la porte d'entrée, `new` ne fait pas confiance.
    fn check_params(config: &Config) -> Result<()> {
        let blank = |s: &Option<String>| s.as_deref().unwrap_or("").trim().is_empty();
        if blank(&config.owner_subject) {
            bail!("--owner-subject (or VNL_OWNER_SUBJECT) is required");
        }
        if blank(&config.token_issuer) {
            bail!("--token-issuer (or VNL_TOKEN_ISSUER) is required");
        }
        if config.token_audience.trim().is_empty() {
            bail!("token_audience must not be empty");
        }
        match config.jwks_source() {
            JwksSource::Url(u) if u.is_empty() => bail!("jwks source must not be empty"),
            JwksSource::File(p) if p.as_os_str().is_empty() => {
                bail!("jwks source must not be empty")
            }
            _ => {}
        }
        Ok(())
    }

    /// Valide `token` selon l'ordre d'évaluation verrouillé de `Raises` :
    /// header + `kid` (sans réseau) → clés disponibles → `kid` (un seul
    /// refresh forcé borné 30 s) → type et algorithme épinglés → signature,
    /// `iss`, `aud`, `exp`, `nbf` → `sub == owner_subject`.
    ///
    /// Le jeton n'est jamais loggé, à aucun niveau ; les causes de refus sont
    /// tracées en `debug`.
    ///
    /// # Errors
    ///
    /// Une variante de `AuthError` selon le point d'échec (`InvalidToken`,
    /// `KeysUnavailable`, `KeyNotFound`, `UnsupportedAlgorithm`,
    /// `NotOwner`), dans l'ordre `Raises` de `auth.sdd`.
    pub async fn validate_token(&self, token: &str) -> std::result::Result<AuthInfo, AuthError> {
        // 1. Décodage du header et présence d'un `kid` — AVANT tout réseau.
        let header = decode_header(token).map_err(|e| {
            tracing::debug!("undecodable JWT header: {e}");
            AuthError::InvalidToken
        })?;
        let kid = header.kid.ok_or(AuthError::InvalidToken)?;

        // 2. Clés de vérification disponibles (à défaut : 503, pas 401).
        self.ensure_keys().await?;

        // 3. Recherche du `kid` ; au plus UN refresh forcé, borné à un toutes
        //    les 30 s (le throttle interdit l'amplification par jeton poubelle).
        let mut key = self.find_key(&kid).await;
        if matches!(key, KeyLookup::Missing) && self.throttled_forced_refresh().await {
            key = self.find_key(&kid).await;
        }

        // 4. Type de clé et algorithme, épinglés par le type du JWKS :
        //    RSA → RS256, EC P-256 → ES256 ; incohérence avec l'`alg` du
        //    header → refus ; jamais `none`, `HS*`, ni une clé du jeton.
        let (decoding_key, pinned_alg) = match key {
            KeyLookup::Found(k, alg) => (k, alg),
            KeyLookup::WrongType => return Err(AuthError::UnsupportedAlgorithm),
            KeyLookup::Missing => return Err(AuthError::KeyNotFound),
        };
        if header.alg != pinned_alg {
            tracing::debug!(
                "token alg {:?} rejected: key {kid} is pinned to {pinned_alg:?}",
                header.alg
            );
            return Err(AuthError::UnsupportedAlgorithm);
        }

        // 5. Signature, `iss`, `aud`, `exp`, `nbf` — toute cause est effondrée
        //    en `InvalidToken`, la cause réelle en `debug` sans le jeton.
        let mut validation = Validation::new(pinned_alg);
        validation.validate_nbf = true; // jsonwebtoken 11 : faux par défaut
        validation.set_audience(&[self.config.token_audience.as_str()]);
        // `token_issuer` est non vide : vérifié par check_params au démarrage.
        let issuer = self.config.token_issuer.clone().unwrap_or_default();
        validation.set_issuer(&[issuer.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss"]);
        validation.leeway = CLOCK_LEEWAY_SECS;

        let data = decode::<TokenClaims>(token, &decoding_key, &validation).map_err(|e| {
            tracing::debug!("JWT rejected: {e}");
            AuthError::InvalidToken
        })?;

        // 6. Appartenance : `sub` égal à `owner_subject`, comparaison exacte
        //    (ni sous-chaîne, ni insensible à la casse).
        if Some(data.claims.sub.as_str()) != self.config.owner_subject.as_deref() {
            tracing::debug!("valid token from a foreign owner");
            return Err(AuthError::NotOwner);
        }

        Ok(AuthInfo {
            subject: data.claims.sub,
        })
    }

    // ── interne : cache et fraîcheur des clés ─────────────────────────────

    /// S'assure qu'un cache existe (périmé → recharge, échec CONSERVANT
    /// l'ancien cache). Rend `KeysUnavailable` seulement si aucun cache
    /// n'existe du tout. La recharge d'expiration est bornée par le même
    /// plancher de 30 s que le refresh forcé (revue 2026-10-14) : en échec,
    /// les requêtes suivantes dans la fenêtre retombent sur le cache conservé
    /// sans nouvelle tentative réseau.
    async fn ensure_keys(&self) -> std::result::Result<(), AuthError> {
        if self.cache_is_stale().await {
            self.throttled_expired_refresh().await;
        }
        if self.cache.read().await.is_some() {
            Ok(())
        } else {
            Err(AuthError::KeysUnavailable)
        }
    }

    /// Recharge déclenchée par l'expiration du cache : limitée à une tentative
    /// toutes les 30 s (`EXPIRED_REFRESH_MIN_INTERVAL`), succès comme échec
    /// consignant l'instant de tentative. `true` si une recharge a été tentée,
    /// `false` si la fenêtre l'a bloquée (l'appelant retombe alors sur le cache
    /// conservé, ou `KeysUnavailable` s'il n'y a aucun cache). Le verrou reste
    /// tenu À TRAVERS la tentative : les requêtes concurrentes sont sérialisées
    /// et tombent ensuite dans la fenêtre.
    async fn throttled_expired_refresh(&self) -> bool {
        let mut gate = self.last_expired_refresh.lock().await;
        if gate.is_some_and(|prev| prev.elapsed() < EXPIRED_REFRESH_MIN_INTERVAL) {
            return false;
        }
        *gate = Some(Instant::now());
        if let Err(e) = self.refresh().await {
            tracing::error!("JWKS refresh failed, keeping previous cache if any: {e:#}");
        }
        true
    }

    async fn cache_is_stale(&self) -> bool {
        self.cache
            .read()
            .await
            .as_ref()
            .is_none_or(|c| c.fetched_at.elapsed() > JWKS_CACHE_TTL)
    }

    /// Refresh forcé par un `kid` inconnu : limité à une fois toutes les 30 s.
    /// `true` si un refresh a été tenté, `false` si la fenêtre de throttle l'a
    /// bloqué. Le verrou de throttle reste tenu pendant le refresh : les
    /// tentatives concurrentes sont sérialisées et tombent ensuite dans la
    /// fenêtre.
    async fn throttled_forced_refresh(&self) -> bool {
        let mut gate = self.last_forced_refresh.lock().await;
        if gate.is_some_and(|prev| prev.elapsed() < FORCED_REFRESH_MIN_INTERVAL) {
            return false;
        }
        *gate = Some(Instant::now());
        if let Err(e) = self.refresh().await {
            tracing::error!("forced JWKS refresh failed: {e:#}");
        }
        true
    }

    /// Recharge les clés et ne remplace le cache qu'en cas de succès.
    async fn refresh(&self) -> Result<()> {
        let jwk_set = self
            .fetch_jwks()
            .await
            .context("fetching verification keys")?;
        *self.cache.write().await = Some(JwksCache {
            jwk_set,
            fetched_at: Instant::now(),
        });
        Ok(())
    }

    async fn fetch_jwks(&self) -> Result<JwkSet> {
        match self.config.jwks_source() {
            JwksSource::File(path) => {
                let body = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading JWKS file: {}", path.display()))?;
                serde_json::from_str(&body).context("parsing JWKS file")
            }
            JwksSource::Url(url) => {
                let mut req = self.http.get(&url);
                if let Some(token_file) = &self.config.jwks_token_file {
                    let sa_token = std::fs::read_to_string(token_file)
                        .with_context(|| format!("reading JWKS bearer token file: {token_file}"))?;
                    req = req.bearer_auth(sa_token.trim());
                }
                let resp = req
                    .send()
                    .await
                    .context("JWKS request failed")?
                    .error_for_status()
                    .context("JWKS endpoint returned an error status")?;
                resp.json::<JwkSet>().await.context("parsing JWKS response")
            }
        }
    }

    /// Cherche `kid` dans le cache courant et épingle l'algorithme sur le type
    /// de la clé trouvée. Un échec de construction de la clé (paramètres
    /// malformés) compte comme type non exploitable → `UnsupportedAlgorithm`.
    async fn find_key(&self, kid: &str) -> KeyLookup {
        let cache = self.cache.read().await;
        let Some(jwk) = cache.as_ref().and_then(|c| c.jwk_set.find(kid)) else {
            return KeyLookup::Missing;
        };
        match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => {
                match DecodingKey::from_rsa_components(&rsa.n, &rsa.e) {
                    Ok(k) => KeyLookup::Found(k, Algorithm::RS256),
                    Err(e) => {
                        tracing::debug!("malformed RSA key {kid}: {e}");
                        KeyLookup::WrongType
                    }
                }
            }
            AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => {
                match DecodingKey::from_ec_components(&ec.x, &ec.y) {
                    Ok(k) => KeyLookup::Found(k, Algorithm::ES256),
                    Err(e) => {
                        tracing::debug!("malformed EC key {kid}: {e}");
                        KeyLookup::WrongType
                    }
                }
            }
            other => {
                let _ = other;
                KeyLookup::WrongType
            }
        }
    }
}

// ── Middleware ────────────────────────────────────────────────────────────────

/// Middleware d'authentification du router protégé (`/mcp`, `/git/*`,
/// `/code-health/*`, `POST /ws/ticket` — cf. `./lib.rs`). Les routes `/ws/*`
/// sont sous `ws_auth_middleware` (`./ws/ticket.rs`) ; `/health` et `/metrics`
/// vivent sur le port d'exploitation, sans authentification.
///
/// Injecte `AuthInfo` dans les extensions avant `Next::run`. Le rejet est écrit
/// AVANT tout passage au handler : jamais de réponse d'erreur après un upgrade
/// WebSocket (piège axum 401→426).
pub async fn require_auth(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(token) = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        metrics::counter!("mcp_auth_errors_total", "reason" => "missing_token").increment(1);
        return AuthError::MissingToken.into_response();
    };

    match state.auth.validate_token(token).await {
        Ok(info) => {
            req.extensions_mut().insert(info);
            next.run(req).await
        }
        Err(err) => {
            // Le jeton n'est jamais loggé ; la raison du refus, seulement.
            metrics::counter!("mcp_auth_errors_total", "reason" => err.reason()).increment(1);
            err.into_response()
        }
    }
}

// ── Fabrication de clés et jetons de test ─────────────────────────────────────

/// Fabrication de clés et jetons de test (auth.sdd `Exposes`) : c'est AINSI que
/// les tests s'authentifient — un vrai jeton signé par une clé de test, présenté
/// au chemin normal de `require_auth`. Il n'existe AUCUN mode de contournement.
///
/// Jamais présent dans le binaire de production : `cfg(test)` ou la feature
/// `test` du crate (auto-dev-dependence pour `sandbox/tests/`).
#[cfg(any(test, feature = "test"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]
// test_support = code de test (auth.sdd `Exposes`) : fixtures statiques dont
// l'échec serait un bug du fichier lui-même ; il n'est jamais linké dans le
// binaire de production.
pub mod test_support {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use jsonwebtoken::jwk::JwkSet;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};

    use super::{AuthState, JWKS_HTTP_TIMEOUT, JwksCache};
    use crate::config::Config;

    /// Kid de la clé de test principale.
    pub const TEST_KID: &str = "vnl-test-rs256";
    /// Kid de la seconde clé de test (scénario de rotation).
    pub const TEST_KID2: &str = "vnl-test-rs256-rotated";
    /// Owner de test standard (`system:serviceaccount:<ns>:owner-<nom>`).
    pub const TEST_OWNER: &str = "system:serviceaccount:vanyline:owner-alice";
    /// Issuer de test (celui de l'apiserver, vérifié sur cluster).
    pub const TEST_ISSUER: &str = "https://kubernetes.default.svc.cluster.local";
    /// Audience de la sandbox.
    pub const TEST_AUDIENCE: &str = "vanyline-sandbox";

    /// Clé privée RSA de test (PKCS#8). Publiée volontairement : c'est une clé
    /// de fixture, uniquement valable contre les `AuthState` de test.
    pub const TEST_KEY_PEM: &[u8] = include_bytes!("../tests/fixtures/owner-test-key.pem");
    /// Seconde clé privée (rotation).
    pub const TEST_KEY2_PEM: &[u8] = include_bytes!("../tests/fixtures/owner-test-key2.pem");
    /// Clé publique de la première paire — sert de « secret » pour fabriquer un
    /// jeton HS256 (clé fournie par l'attaquant, jamais acceptée).
    pub const TEST_PUBLIC_PEM: &[u8] = include_bytes!("../tests/fixtures/owner-test-key.pub.pem");

    const TEST_KEY_N: &str = "oCwWU-svwQI1GxU6fD_Gx5cSmh90tyvM4BrB25ily2gDEKpXr8yVGk7ItATxqk9KUcfAkKMdRq0fPfEDIdgclSvPy2Z668okwZHQFOTGJCtYNT9fQH18yDq7H2AxhjF7dMrfqLF3axUnV9gD8UJysWJ8DuNr33SG2vAqYk-B915ADSRz3akDGhHQvfdfqbgfpdKF6X4uL2KssPRORUkCRgJLmtAjKaTMZ-htDgWzum9ztqKs4AaKbwyeyJtIAquwQiPPWnVikODY1ZVp5_x0uprTX-hYXiiocLKjc0ZVjlwuHQuZpnz2v_3kFi47VTbvErnjyNpIutuJ9i_Osi1Gew";
    const TEST_KEY2_N: &str = "nBKqQp-kP5BfLUg54JYutrLLgl3Bk0_pyqJjueH9YIQQ8mvZsB4IPJAEF2rjblXU4VDpZW_ITFy0q_LFGZmKFWgGYrbjSId2ez4EEVKaA_9UiqjgLZhjsH4MazrIHRLtK5-I6pRJWqv6a_pVSjG0V3D3nyfo1uq9G7YofJZ2JDF9adQGPEpzH9Tu4ArJ9IzoigUZER8jd8qQGnlT3iLOl8WjVt0wHOYHp0pco0cp9KTcQvjOYktewTii8k_0M_w-0zhhwjI4kW_MGQJWvRwu2ZVR7JxLQEtDTa2ml4JVZaFiJv9TazkZKd91uqQu1mKvIMLa3b9kHb9n_mX4T92xew";
    const TEST_KEY_E: &str = "AQAB";

    fn jwk_json(kid: &str, n: &str) -> String {
        format!(
            r#"{{"kty":"RSA","use":"sig","kid":"{kid}","alg":"RS256","n":"{n}","e":"{TEST_KEY_E}"}}"#
        )
    }

    /// JWKS des tests : la clé de test principale sous `TEST_KID`.
    ///
    /// # Panics
    ///
    /// Si la fixture JWKS interne n'est pas un JSON valide (constante du crate).
    #[must_use]
    pub fn test_jwks() -> JwkSet {
        serde_json::from_str(&format!(
            r#"{{"keys":[{}]}}"#,
            jwk_json(TEST_KID, TEST_KEY_N)
        ))
        .unwrap()
    }

    /// JWKS d'une clé RSA de test sous un `kid` arbitraire.
    ///
    /// # Panics
    ///
    /// Si la fixture JWKS interne n'est pas un JSON valide (constante du crate).
    #[must_use]
    pub fn test_jwks_with_kid(kid: &str) -> JwkSet {
        serde_json::from_str(&format!(r#"{{"keys":[{}]}}"#, jwk_json(kid, TEST_KEY_N))).unwrap()
    }

    /// Corps JWKS du serveur simulé après rotation : ne contient plus que la
    /// SECONDE clé (`TEST_KID2`), pas l'ancienne.
    #[must_use]
    pub fn rotated_jwks_json() -> String {
        format!(r#"{{"keys":[{}]}}"#, jwk_json(TEST_KID2, TEST_KEY2_N))
    }

    /// Spécification complète d'un jeton de test (tous les cas tordus des
    /// Scenario : `iss`/`aud`/`exp`/`nbf`/`kid`/algorithme/clé au choix).
    #[derive(Debug, Clone)]
    pub struct TokenSpec {
        pub subject: String,
        pub audience: String,
        pub issuer: String,
        pub kid: Option<String>,
        /// Algorithme du header qui ne sait pas être produit par la clé
        /// privée (ex. `"HS256"` signé avec la clé publique — clé fournie par
        /// l'attaquant).
        pub alg_override: Option<&'static str>,
        /// Durée de validité `exp - iat`, en secondes ; négatif = expiré.
        pub exp: i64,
        pub nbf: Option<i64>,
        pub key_pem: &'static [u8],
    }

    /// Spécification nominale pour l'`TEST_OWNER`, valide `exp` secondes.
    #[must_use]
    pub fn owner_spec(exp: i64) -> TokenSpec {
        TokenSpec {
            subject: TEST_OWNER.to_string(),
            audience: TEST_AUDIENCE.to_string(),
            issuer: TEST_ISSUER.to_string(),
            kid: Some(TEST_KID.to_string()),
            alg_override: None,
            exp,
            nbf: None,
            key_pem: TEST_KEY_PEM,
        }
    }

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap()
    }

    fn claims_json(spec: &TokenSpec) -> serde_json::Value {
        let now = now_secs();
        let mut claims = serde_json::json!({
            "sub": spec.subject,
            "iss": spec.issuer,
            "aud": spec.audience,
            "iat": now,
            "exp": now.checked_add(spec.exp).unwrap_or(i64::MIN),
        });
        if let Some(nbf) = spec.nbf {
            claims["nbf"] = serde_json::json!(now.checked_add(nbf).unwrap_or(i64::MIN));
        }
        claims
    }

    /// Fabrique un jeton signé selon `spec`.
    ///
    /// # Panics
    ///
    /// Si la clé de test est illisible, si `alg_override` n'est pas un
    /// algorithme de test prévu, ou si l'énoncé du jeton est incohérent
    /// (famille de clé ≠ algorithme).
    #[must_use]
    pub fn mint_token(spec: &TokenSpec) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid.clone_from(&spec.kid);
        let key = match spec.alg_override {
            None => EncodingKey::from_rsa_pem(spec.key_pem).unwrap(),
            Some("HS256") => {
                header.alg = Algorithm::HS256;
                EncodingKey::from_secret(TEST_PUBLIC_PEM)
            }
            Some(other) => panic!("algorithme de test non prévu: {other}"),
        };
        assert_eq!(
            key.family(),
            header.alg.family(),
            "spécification de jeton incohérente"
        );
        jsonwebtoken::encode(&header, &claims_json(spec), &key).unwrap()
    }

    /// Fabrique un jeton d'Owner de test, signé par la clé de test
    /// (auth.sdd `Exposes` : `mint_owner_token(subject, audience, issuer, ttl)`).
    ///
    /// # Panics
    ///
    /// Comme `mint_token`.
    #[must_use]
    pub fn mint_owner_token(subject: &str, audience: &str, issuer: &str, ttl_secs: i64) -> String {
        mint_token(&TokenSpec {
            subject: subject.to_string(),
            audience: audience.to_string(),
            issuer: issuer.to_string(),
            ..owner_spec(ttl_secs)
        })
    }

    /// Fabrique un jeton `alg: none` (signature vide), header et payload
    /// encodés en base64url à la main — jsonwebtoken refuse de signer en
    /// `none` (et refuse de le lire : `InvalidToken`).
    ///
    /// # Panics
    ///
    /// Si le header ou le payload de la spec n'est pas sérialisable.
    #[must_use]
    pub fn mint_none_token(spec: &TokenSpec) -> String {
        let mut header = serde_json::json!({"alg": "none", "typ": "JWT"});
        if let Some(kid) = &spec.kid {
            header["kid"] = serde_json::json!(kid);
        }
        format!(
            "{}.{}.",
            b64url_no_pad(&serde_json::to_vec(&header).unwrap()),
            b64url_no_pad(&serde_json::to_vec(&claims_json(spec)).unwrap()),
        )
    }

    fn b64url_no_pad(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let bits = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            out.push(TABLE[(bits >> 26) as usize & 63] as char);
            out.push(TABLE[(bits >> 20) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(TABLE[(bits >> 14) as usize & 63] as char);
            }
            if chunk.len() > 2 {
                out.push(TABLE[(bits >> 8) as usize & 63] as char);
            }
        }
        out
    }

    /// `Config` nominale de test : Owner/issuer/audience posés, pas de source de
    /// clés explicite (les `AuthState` de test injectent leur JWKS ; les tests
    /// « live » la rechargent ensuite via `jwks_url`).
    #[must_use]
    pub fn test_config(sandbox_root: &Path) -> Config {
        Config {
            listen: "127.0.0.1:0".into(),
            tls_cert: None,
            tls_key: None,
            ops_listen: "127.0.0.1:0".into(),
            owner_subject: Some(TEST_OWNER.into()),
            token_issuer: Some(TEST_ISSUER.into()),
            token_audience: TEST_AUDIENCE.into(),
            jwks_url: None,
            jwks_file: None,
            jwks_ca_file: Some("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt".into()),
            jwks_token_file: Some("/var/run/secrets/kubernetes.io/serviceaccount/token".into()),
            otel_endpoint: None,
            sandbox_root: sandbox_root.to_path_buf(),
            health_cooldown_secs: 30,
            health_poll_secs: 60,
            health_base_ref: None,
            restate_ingress_url: None,
        }
    }

    /// Client de test : timeout contractuel, sans CA personnalisé (les serveur
    /// simulés sont en HTTP local).
    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(JWKS_HTTP_TIMEOUT)
            .build()
            .unwrap()
    }

    /// `AuthState` de test avec le JWKS de test injecté dans le cache, frais.
    #[must_use]
    pub fn auth_state(config: Arc<Config>) -> AuthState {
        auth_state_with_jwks(config, test_jwks(), None)
    }

    /// `AuthState` de test avec un JWKS injecté ; `age`=backdate le cache (ex.
    /// `JWKS_CACHE_TTL` + un pour le rendre périmé).
    ///
    /// # Panics
    ///
    /// Si l'horloge de test est plus vieille que l'âge demandé.
    #[must_use]
    pub fn auth_state_with_jwks(
        config: Arc<Config>,
        jwk_set: JwkSet,
        age: Option<Duration>,
    ) -> AuthState {
        let fetched_at = match age {
            None => Instant::now(),
            Some(age) => Instant::now()
                .checked_sub(age)
                .expect("horloge de test plus vieille que l'âge demandé"),
        };
        AuthState {
            config,
            cache: tokio::sync::RwLock::new(Some(JwksCache {
                jwk_set,
                fetched_at,
            })),
            last_forced_refresh: tokio::sync::Mutex::new(None),
            last_expired_refresh: tokio::sync::Mutex::new(None),
            http: test_client(),
        }
    }

    /// `AuthState` de test SANS cache, pointant sur le `jwks_source` de la
    /// config (serveur simulé ou fichier local).
    #[must_use]
    pub fn auth_state_without_cache(config: Arc<Config>) -> AuthState {
        AuthState {
            config,
            cache: tokio::sync::RwLock::new(None),
            last_forced_refresh: tokio::sync::Mutex::new(None),
            last_expired_refresh: tokio::sync::Mutex::new(None),
            http: test_client(),
        }
    }
}
