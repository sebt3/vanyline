//! Hôte lymaille de l'app — contrat : `host.sdd`.
//!
//! Le point par lequel `app` fournit à miryad-core ce qu'elle seule connaît : l'URL et le
//! jeton du serveur MCP d'une sandbox (`resolve_mcp`, contexte `H_sandbox_<owner>_<nom>`,
//! rôle `"sandbox"`, lien utilisateur↔Owner vérifié À CHAQUE appel) et les contextes
//! projet/sandbox `H_` (`resolve_context`, jamais d'écriture). Aucun second client K8s ni
//! second cache de jeton : le gateway passe par `VnlK8sClient` (`DEC-3`).

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use lymaille::{ContextLevel, ToolsetsResult};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use zeroize::Zeroizing;

use miryad_core::lymaille::host::{HostError, LymailleHost, McpDecision, McpGrant, McpRequest};
use miryad_core::lymaille::identity::CallerPrincipal;
use vanyline_lib::k8s::{OwnerToken, VnlK8sClient};

use crate::db::entities::owner_links;

/// Préfixe des identifiants de contexte projet (`FR-3`).
pub(crate) const PROJECT_PREFIX: &str = "H_project_";
/// Préfixe des identifiants de contexte sandbox (`FR-2`, `FR-3`).
pub(crate) const SANDBOX_PREFIX: &str = "H_sandbox_";
/// Seul rôle MCP que l'hôte pourvoit (`FR-2`).
pub(crate) const SANDBOX_ROLE: &str = "sandbox";

/// Pannes du gateway, classées pour la décision de l'hôte : `Refused` = objet absent ou
/// illisible, mint refusé — refus fermé propre à l'appel (`Denied`/`None`) ; `Internal` =
/// panne interne (cluster injoignable) — `HostError` (`FR-1`, `Handles`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostGatewayError {
    /// Refus propre à l'appel : entité absente/illisible, `TokenRequest` en échec.
    Refused,
    /// Panne interne sans contenu propagé : journalisée ici, `HostError` rendu à miryad-core.
    Internal,
}

/// Ce que l'hôte relit de l'entité réelle (Project/Sandbox) : son nom porté par l'objet et
/// le namespace où il vit — rien de plus (`FR-3`, parcimonie des paramètres).
#[derive(Clone, Debug)]
pub(crate) struct HostEntity {
    /// Nom porté par l'objet (DNS-1123) — champ contractuel de la relecture (`host.sdd`,
    /// « son nom porté par l'objet »), pas encore lu en production : les contextes `H_`
    /// nomment l'entité par l'identifiant résolu, pas par la relecture — tooling.sdd
    #[allow(dead_code)]
    pub name: String,
    /// `metadata.namespace` de l'objet (peut être absent hors cluster).
    pub namespace: Option<String>,
}

/// Seam injectable des accès K8s de l'hôte (jurisprudence des seams de
/// `../workflow/runtime.rs`) : le harnais teste l'hôte sans faux `VnlK8sClient` (convention
/// anti-mock `../../../lib/src/k8s.sdd`) ; la branche apiserver réelle est portée par
/// `K8sHostGateway`, non testée en unitaire.
///
/// Futures `Box`ées à la main (motif `watch_owner_objects` de `lib/src/k8s.rs`) plutôt que
/// `#[async_trait]` : le code généré par la macro échoue le harnais `must_use_unit`.
pub(crate) trait HostGateway: Send + Sync {
    /// URL MCP de la sandbox `name` de l'`owner` (garde d'existence incluse).
    fn sandbox_mcp_url<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<String, HostGatewayError>> + Send + 'a>>;
    /// Jeton frais de l'`owner` (le cache est celui de `VnlK8sClient`, pas un cache de l'hôte).
    fn owner_token<'a>(
        &'a self,
        owner: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<OwnerToken, HostGatewayError>> + Send + 'a>>;
    /// Relecture de la Sandbox `name` de l'`owner`.
    fn sandbox<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>>;
    /// Relecture du Project `name` de l'`owner`.
    fn project<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>>;
}

/// Gateway réel : le client K8s existant de l'app (slot lazy partagé avec `AppState::k8s` —
/// aucun second client, `DEC-3`), découvert à la première requête comme `crate::k8s::client`.
pub(crate) struct K8sHostGateway {
    slot: Arc<tokio::sync::Mutex<Option<VnlK8sClient>>>,
    namespace: Option<String>,
}

impl K8sHostGateway {
    /// Attache le gateway au slot lazy existant (clone de `AppState::k8s`) et au namespace
    /// configuré (celui que `crate::k8s::client` donne à `discover`).
    #[must_use]
    pub(crate) fn new(
        slot: Arc<tokio::sync::Mutex<Option<VnlK8sClient>>>,
        namespace: Option<String>,
    ) -> Self {
        Self { slot, namespace }
    }

    /// Client du slot, découvert à la première requête comme `crate::k8s::client` — jamais un
    /// second client (`DEC-3`). Échec de découverte = panne interne.
    async fn client(&self) -> Result<VnlK8sClient, HostGatewayError> {
        let mut guard = self.slot.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        let client = VnlK8sClient::discover(self.namespace.clone())
            .await
            .map_err(|e| {
                tracing::warn!("VNL-APP-SES-107: hôte lymaille — découverte K8s échouée: {e}");
                HostGatewayError::Internal
            })?;
        *guard = Some(client.clone());
        Ok(client)
    }
}

impl HostGateway for K8sHostGateway {
    fn sandbox_mcp_url<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<String, HostGatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let client = self.client().await?;
            client.sandbox_mcp_url(owner, name).await.map_err(|e| {
                tracing::debug!("VNL-APP-SES-103: hôte lymaille — URL MCP de la sandbox {owner}/{name} indisponible: {e}");
                HostGatewayError::Refused
            })
        })
    }

    fn owner_token<'a>(
        &'a self,
        owner: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<OwnerToken, HostGatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let client = self.client().await?;
            client.owner_token(owner).await.map_err(|e| {
                // le message de la panne ne porte jamais le jeton (lib/src/k8s.sdd)
                tracing::debug!(
                    "VNL-APP-SES-104: hôte lymaille — jeton de l'Owner {owner} non mint: {e}"
                );
                HostGatewayError::Refused
            })
        })
    }

    fn sandbox<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let client = self.client().await?;
            client
                .get_sandbox(owner, name)
                .await
                .map_err(|_| HostGatewayError::Refused)
                .map(|sandbox| HostEntity {
                    // l'objet relu est celui demandé par ces identifiants ; `metadata.name`
                    // optionnel n'apporte rien de plus
                    name: name.to_string(),
                    namespace: sandbox.metadata.namespace,
                })
        })
    }

    fn project<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let client = self.client().await?;
            client
                .get_project(owner, name)
                .await
                .map_err(|_| HostGatewayError::Refused)
                .map(|project| HostEntity {
                    name: name.to_string(),
                    namespace: project.metadata.namespace,
                })
        })
    }
}

/// L'hôte lymaille de l'app : connexion DB (lecture des liens `vanyline_owner_links` à
/// chaque appel, aucune écriture — `DEC-9`) et gateway K8s.
pub(crate) struct VnlLymailleHost {
    db: DatabaseConnection,
    gateway: Arc<dyn HostGateway>,
}

impl VnlLymailleHost {
    /// Attache l'hôte à la connexion (celle de l'état miryad-core) et au gateway.
    #[must_use]
    pub(crate) fn new(db: DatabaseConnection, gateway: Arc<dyn HostGateway>) -> Self {
        Self { db, gateway }
    }

    /// Relecture À CHAQUE appel du lien `vanyline_owner_links` du principal (`FR-2`) :
    /// l'Owner auquel il est lié, `None` sans lien ni ligne à nom NULL. Table illisible =
    /// panne interne journalisée, jamais propagée avec son texte (`HostError` sans contenu).
    async fn linked_owner(&self, user_id: i32) -> Result<Option<String>, HostError> {
        let link = owner_links::Entity::find()
            .filter(owner_links::Column::UserId.eq(user_id))
            .one(&self.db)
            .await
            .map_err(|e| {
                tracing::warn!(
                    "VNL-APP-SES-101: hôte lymaille — lecture de vanyline_owner_links échouée: {e}"
                );
                HostError
            })?;
        Ok(link.and_then(|l| l.k8s_owner_name))
    }
}

/// Encodage réversible `-` ↔ `_` des noms K8s dans les identifiants `H_` (`DEC-9`, `[?]`
/// des Tasks de `host.sdd`) — alphabet `identity::classify` : alphanumérique + `_`.
/// Constructeur d'identifiants lu par les tests ; appelants de production = tranche DEC-8
/// (ouverture par écran) — tooling.sdd
#[allow(dead_code)]
#[must_use]
pub(crate) fn encode_k8s_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Décodage `_` → `-` d'une partie d'identifiant.
#[must_use]
pub(crate) fn decode_k8s_name(part: &str) -> String {
    part.replace('_', "-")
}

/// Étiquette DNS-1123 : minuscules, chiffres, `-` aux milieux, ≤ 63 — l'alphabet des noms
/// que l'encodage est censé reconstruire.
fn is_dns_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.ends_with(|c: char| c.is_ascii_alphanumeric())
}

/// Identifiant `H_project_…` d'un projet (encodé, `FR-3`). Constructeurs d'identifiants lus
/// par les tests et les Scenario de `host.sdd` ; leurs appelants de production (ouverture par
/// écran, tranche DEC-8/`DEC-6`) ne sont pas dans B9 — tooling.sdd
#[allow(dead_code)]
#[must_use]
pub(crate) fn h_project_id(name: &str) -> String {
    format!("{PROJECT_PREFIX}{}", encode_k8s_name(name))
}

/// Identifiant `H_sandbox_<owner>_<nom>` d'une sandbox (deux parties encodées, `FR-2`).
/// Constructeurs d'identifiants lus par les tests ; appelants de production = tranche DEC-8
/// (ouverture par écran) — tooling.sdd
#[allow(dead_code)]
#[must_use]
pub(crate) fn h_sandbox_id(owner: &str, name: &str) -> String {
    format!(
        "{SANDBOX_PREFIX}{}_{}",
        encode_k8s_name(owner),
        encode_k8s_name(name)
    )
}

/// Résolution inverse d'un identifiant projet : le suffixe décodé (`_` → `-`) d'un seul nom,
/// `None` s'il ne reconstruit pas une étiquette DNS-1123 — pas d'ambiguïté possible ici, la
/// jointure de deux noms n'existe que pour les sandbox.
#[must_use]
pub(crate) fn parse_project_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix(PROJECT_PREFIX)?;
    let name = decode_k8s_name(rest);
    is_dns_label(&name).then_some(name)
}

/// Résolution inverse d'un identifiant sandbox : TOUTES les lectures (owner, nom) compatibles
/// avec l'encodage (le `_` de jointure est indistinguable d'un `_` encodé d'un nom à tiret —
/// `[?]` de `host.sdd`) ; l'appelant lève l'ambiguïté par le lien d'owner et la relecture.
#[must_use]
pub(crate) fn parse_sandbox_id(id: &str) -> Vec<(String, String)> {
    let Some(rest) = id.strip_prefix(SANDBOX_PREFIX) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for (i, _) in rest.match_indices('_') {
        if i == 0 {
            // Owner vide : lecture impossible, pas une jointure
            continue;
        }
        let (owner_part, join_rest) = rest.split_at(i);
        let owner = decode_k8s_name(owner_part);
        let name = decode_k8s_name(&join_rest[1..]);
        if is_dns_label(&owner) && is_dns_label(&name) {
            candidates.push((owner, name));
        }
    }
    candidates
}

/// Contexte projet `id` : paramètres parcimonieux dérivés de l'entité relue (`FR-3`) —
/// level partagé, aucun rôle MCP.
fn project_context(
    id: &str,
    owner: &str,
    name: &str,
    entity: &HostEntity,
) -> Result<lymaille::ToolsetContext, HostError> {
    let built = lymaille::ToolsetContext::new(id.to_string(), ContextLevel::Shared)
        .and_then(|c| c.with_parameter("owner".to_string(), serde_json::json!(owner)))
        .and_then(|c| c.with_parameter("name".to_string(), serde_json::json!(name)))
        .and_then(|c| with_namespace(c, entity));
    built.map_err(|e| {
        tracing::warn!("VNL-APP-SES-105: hôte lymaille — contexte projet {id} refusé par la crate toolsets: {e}");
        HostError
    })
}

/// Contexte sandbox `id` : mêmes paramètres, plus le rôle MCP que `resolve_mcp` pourvoit ;
/// la référence du rôle est l'identifiant lui-même (résolue par l'hôte, jamais un secret).
fn sandbox_context(
    id: &str,
    owner: &str,
    name: &str,
    entity: &HostEntity,
) -> Result<lymaille::ToolsetContext, HostError> {
    let built = lymaille::ToolsetContext::new(id.to_string(), ContextLevel::Shared)
        .and_then(|c| c.with_parameter("owner".to_string(), serde_json::json!(owner)))
        .and_then(|c| c.with_parameter("name".to_string(), serde_json::json!(name)))
        .and_then(|c| with_namespace(c, entity))
        .and_then(|c| c.with_role(SANDBOX_ROLE.to_string(), id.to_string()));
    built.map_err(|e| {
        tracing::warn!("VNL-APP-SES-105: hôte lymaille — contexte sandbox {id} refusé par la crate toolsets: {e}");
        HostError
    })
}

fn with_namespace(
    ctx: lymaille::ToolsetContext,
    entity: &HostEntity,
) -> ToolsetsResult<lymaille::ToolsetContext> {
    match &entity.namespace {
        Some(ns) => ctx.with_parameter("namespace".to_string(), serde_json::json!(ns)),
        None => Ok(ctx),
    }
}

#[async_trait]
impl LymailleHost for VnlLymailleHost {
    async fn resolve_mcp(
        &self,
        request: &McpRequest<'_>,
        caller: &CallerPrincipal,
    ) -> Result<McpDecision, HostError> {
        if request.role != SANDBOX_ROLE {
            return Ok(McpDecision::Unprovided);
        }
        let candidates = parse_sandbox_id(request.context);
        if candidates.is_empty() {
            return Ok(McpDecision::Unprovided);
        }
        // Lien vérifié À CHAQUE appel (`FR-2`) : la révocation est effective à l'appel
        // suivant, pas à la fin de la session (`AC-2`). Le lien de l'appelant tranche aussi
        // l'ambiguïté de jointure de l'encodage (`[?]` de `host.sdd`).
        let Some(linked) = self.linked_owner(caller.user_id).await? else {
            tracing::debug!(
                "VNL-APP-SES-102: hôte lymaille — principal sans lien d'owner, rôle refusé"
            );
            return Ok(McpDecision::Denied);
        };
        let Some((owner, name)) = candidates.into_iter().find(|(o, _)| *o == linked) else {
            tracing::debug!(
                "VNL-APP-SES-102: hôte lymaille — le lien d'owner ne couvre pas le contexte {}",
                request.context
            );
            return Ok(McpDecision::Denied);
        };
        let url = match self.gateway.sandbox_mcp_url(&owner, &name).await {
            Ok(url) => url,
            Err(HostGatewayError::Refused) => {
                tracing::debug!(
                    "VNL-APP-SES-103: hôte lymaille — sandbox {owner}/{name} absente, rôle refusé"
                );
                return Ok(McpDecision::Denied);
            }
            Err(HostGatewayError::Internal) => {
                tracing::warn!(
                    "VNL-APP-SES-103: hôte lymaille — panne interne sur l'URL MCP de {owner}/{name}"
                );
                return Err(HostError);
            }
        };
        let token = match self.gateway.owner_token(&owner).await {
            Ok(token) => token,
            Err(HostGatewayError::Refused) => {
                tracing::debug!(
                    "VNL-APP-SES-104: hôte lymaille — jeton de l'Owner {owner} non mint, rôle refusé"
                );
                return Ok(McpDecision::Denied);
            }
            Err(HostGatewayError::Internal) => {
                tracing::warn!(
                    "VNL-APP-SES-104: hôte lymaille — panne interne sur le jeton de l'Owner {owner}"
                );
                return Err(HostError);
            }
        };
        Ok(McpDecision::Granted(McpGrant {
            url,
            token: Zeroizing::new(token.token),
            expires_at: Some(token.expires_at),
        }))
    }

    async fn resolve_context(
        &self,
        id: &str,
        caller: &CallerPrincipal,
    ) -> Result<Option<lymaille::ToolsetContext>, HostError> {
        if let Some(name) = parse_project_id(id) {
            let Some(owner) = self.linked_owner(caller.user_id).await? else {
                return Ok(None);
            };
            // Relecture du Project dans le namespace de l'Owner de l'appelant : absent du
            // namespace, absent ou interdit — `None` sans distinction (`AC-3`).
            let entity = match self.gateway.project(&owner, &name).await {
                Ok(entity) => entity,
                Err(HostGatewayError::Refused) => return Ok(None),
                Err(HostGatewayError::Internal) => {
                    tracing::warn!(
                        "VNL-APP-SES-106: hôte lymaille — panne interne sur la relecture du projet {owner}/{name}"
                    );
                    return Err(HostError);
                }
            };
            return Ok(Some(project_context(id, &owner, &name, &entity)?));
        }
        let candidates = parse_sandbox_id(id);
        if candidates.is_empty() {
            return Ok(None);
        }
        let Some(linked) = self.linked_owner(caller.user_id).await? else {
            return Ok(None);
        };
        let Some((owner, name)) = candidates.into_iter().find(|(o, _)| *o == linked) else {
            return Ok(None);
        };
        let entity = match self.gateway.sandbox(&owner, &name).await {
            Ok(entity) => entity,
            Err(HostGatewayError::Refused) => return Ok(None),
            Err(HostGatewayError::Internal) => {
                tracing::warn!(
                    "VNL-APP-SES-106: hôte lymaille — panne interne sur la relecture de la sandbox {owner}/{name}"
                );
                return Err(HostError);
            }
        };
        Ok(Some(sandbox_context(id, &owner, &name, &entity)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::real_db;
    use chrono::{TimeZone as _, Utc};
    use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait as _, Set};
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    // ── Gateway factice (seam HostGateway — pas de faux VnlK8sClient, convention
    // anti-mock lib/src/k8s.sdd ; jurisprudence des seams de workflow/runtime.rs) ──

    #[derive(Clone)]
    struct GatewayState {
        /// Décision de `sandbox_mcp_url` (garde d'existence + URL).
        mcp_url: Result<String, HostGatewayError>,
        /// Décision de `owner_token` (mint du jeton).
        token: Result<OwnerToken, HostGatewayError>,
        /// (owner, nom) des Projects existants.
        projects: HashSet<(String, String)>,
        /// (owner, nom) des Sandboxes existantes.
        sandboxes: HashSet<(String, String)>,
        /// (owner, nom) de chaque appel reçu — preuve de l'absence de cache de l'hôte.
        mcp_calls: Vec<(String, String)>,
        token_calls: usize,
        sandbox_calls: Vec<(String, String)>,
        project_calls: Vec<(String, String)>,
    }

    impl Default for GatewayState {
        fn default() -> Self {
            Self {
                mcp_url: Ok("http://sandbox-web.ana-ns.svc:3000/mcp".to_string()),
                token: Ok(OwnerToken {
                    token: "jwt-du-sa-d-ana".to_string(),
                    expires_at: Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0).unwrap(),
                }),
                projects: HashSet::new(),
                sandboxes: HashSet::new(),
                mcp_calls: Vec::new(),
                token_calls: 0,
                sandbox_calls: Vec::new(),
                project_calls: Vec::new(),
            }
        }
    }

    struct FakeGateway(Arc<Mutex<GatewayState>>);

    impl HostGateway for FakeGateway {
        fn sandbox_mcp_url<'a>(
            &'a self,
            owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<String, HostGatewayError>> + Send + 'a>>
        {
            Box::pin(async move {
                let mut g = self.0.lock().unwrap();
                g.mcp_calls.push((owner.to_string(), name.to_string()));
                g.mcp_url.clone()
            })
        }
        fn owner_token<'a>(
            &'a self,
            owner: &'a str,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<OwnerToken, HostGatewayError>> + Send + 'a>,
        > {
            Box::pin(async move {
                let _ = owner;
                let mut g = self.0.lock().unwrap();
                g.token_calls = g.token_calls.saturating_add(1);
                g.token.clone()
            })
        }
        fn sandbox<'a>(
            &'a self,
            owner: &'a str,
            name: &'a str,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>,
        > {
            Box::pin(async move {
                let key = (owner.to_string(), name.to_string());
                let mut g = self.0.lock().unwrap();
                g.sandbox_calls.push(key.clone());
                if g.sandboxes.contains(&key) {
                    Ok(HostEntity {
                        name: name.to_string(),
                        namespace: Some(format!("vanyline-{owner}")),
                    })
                } else {
                    Err(HostGatewayError::Refused)
                }
            })
        }
        fn project<'a>(
            &'a self,
            owner: &'a str,
            name: &'a str,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<HostEntity, HostGatewayError>> + Send + 'a>,
        > {
            Box::pin(async move {
                let key = (owner.to_string(), name.to_string());
                let mut g = self.0.lock().unwrap();
                g.project_calls.push(key.clone());
                if g.projects.contains(&key) {
                    Ok(HostEntity {
                        name: name.to_string(),
                        namespace: Some(format!("vanyline-{owner}")),
                    })
                } else {
                    Err(HostGatewayError::Refused)
                }
            })
        }
    }

    /// Hôte sur base réelle + gateway factice ; retourne l'hôte, l'état observable du
    /// gateway, les id des utilisateurs et la connexion (pour retoucher les liens en cours
    /// de test).
    async fn host_with(
        links: &[(&str, &str)],
        configure: impl FnOnce(&mut GatewayState),
    ) -> (
        VnlLymailleHost,
        Arc<Mutex<GatewayState>>,
        HashMap<String, i32>,
        sea_orm::DatabaseConnection,
    ) {
        let db = real_db().await;
        let mut ids = HashMap::new();
        for (subject, owner) in links {
            let user = miryad_core::users::resolve_user(&db, subject, None)
                .await
                .expect("utilisateur miryad résolu");
            owner_links::ActiveModel {
                id: NotSet,
                user_id: Set(user.id),
                k8s_owner_name: Set(Some((*owner).to_string())),
            }
            .insert(&db)
            .await
            .expect("lien d'owner posé");
            ids.insert((*subject).to_string(), user.id);
        }
        let mut state = GatewayState::default();
        configure(&mut state);
        let shared = Arc::new(Mutex::new(state));
        let host = VnlLymailleHost::new(db.clone(), Arc::new(FakeGateway(Arc::clone(&shared))));
        (host, shared, ids, db)
    }

    fn caller_for(ids: &HashMap<String, i32>, subject: &str) -> CallerPrincipal {
        CallerPrincipal {
            user_id: *ids
                .get(subject)
                .unwrap_or_else(|| panic!("utilisateur {subject} absent des fixtures")),
            subject: subject.to_string(),
            is_admin: false,
        }
    }

    fn request<'a>(context: &'a str, role: &'a str) -> McpRequest<'a> {
        McpRequest { context, role }
    }

    fn params(
        ctx: &lymaille::ToolsetContext,
    ) -> std::collections::BTreeMap<String, serde_json::Value> {
        ctx.resolve(Vec::<(String, String, serde_json::Value)>::new())
            .expect("la résolution sans default réussit")
            .shared()
            .clone()
    }

    fn role_names(ctx: &lymaille::ToolsetContext) -> Vec<String> {
        ctx.resolve(Vec::<(String, String, serde_json::Value)>::new())
            .expect("la résolution sans default réussit")
            .roles()
            .to_vec()
    }

    // ── Scenario « utilisateur lié obtient le serveur sandbox » ──

    #[tokio::test]
    async fn utilisateur_lie_obtient_le_grant_sandbox() {
        let (host, state, ids, _) = host_with(&[("sub-ana", "ana")], |_| {}).await;
        let decision = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &caller_for(&ids, "sub-ana"),
            )
            .await
            .expect("la résolution réussit");
        let McpDecision::Granted(grant) = decision else {
            panic!("attendu Granted, obtenu : {decision:?}");
        };
        assert_eq!(grant.url, "http://sandbox-web.ana-ns.svc:3000/mcp");
        assert_eq!(&*grant.token, "jwt-du-sa-d-ana");
        assert_eq!(
            grant.expires_at,
            Some(Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0).unwrap())
        );
        let g = state.lock().unwrap();
        assert_eq!(g.mcp_calls, vec![("ana".to_string(), "web".to_string())]);
        assert_eq!(g.token_calls, 1);
    }

    /// Le jeton transite par un `Zeroizing` et le `Debug` du grant reste opaque (`FR-4`).
    #[tokio::test]
    async fn grant_ne_divulgue_ni_jeton_ni_url_en_debug() {
        let (host, _state, ids, _) = host_with(&[("sub-ana", "ana")], |_| {}).await;
        let decision = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &caller_for(&ids, "sub-ana"),
            )
            .await
            .expect("la résolution réussit");
        let rendu = format!("{decision:?}");
        assert!(
            !rendu.contains("jwt-du-sa-d-ana"),
            "le jeton fuit : {rendu}"
        );
        assert!(!rendu.contains("sandbox-web"), "l'URL fuit : {rendu}");
    }

    /// Le `Must not` « mettre en cache le `McpGrant` » : chaque appel remint et relit —
    /// aucun cache propre à l'hôte (`FR-2`, `FR-4`).
    #[tokio::test]
    async fn grant_jamais_cache_par_l_hote() {
        let (host, state, ids, _) = host_with(&[("sub-ana", "ana")], |_| {}).await;
        let caller = caller_for(&ids, "sub-ana");
        let id = h_sandbox_id("ana", "web");
        for _ in 0..2 {
            let decision = host
                .resolve_mcp(&request(&id, "sandbox"), &caller)
                .await
                .expect("la résolution réussit");
            assert!(matches!(decision, McpDecision::Granted(_)));
        }
        let g = state.lock().unwrap();
        assert_eq!(g.token_calls, 2, "l'hôte a cache le jeton");
        assert_eq!(g.mcp_calls.len(), 2, "l'hôte a cache l'URL");
    }

    // ── Scenario « lien supprimé en cours de session » (`AC-2`) ──

    #[tokio::test]
    async fn lien_supprime_en_cours_de_session_denie_sans_reutiliser_le_jeton() {
        let (host, state, ids, db) = host_with(&[("sub-ana", "ana")], |_| {}).await;
        let caller = caller_for(&ids, "sub-ana");
        let id = h_sandbox_id("ana", "web");
        let decision = host
            .resolve_mcp(&request(&id, "sandbox"), &caller)
            .await
            .expect("la résolution réussit");
        assert!(matches!(decision, McpDecision::Granted(_)));

        // suppression du lien — la révocation est effective à l'appel suivant (`AC-2`)
        db.execute_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "DELETE FROM vanyline_owner_links".to_string(),
        ))
        .await
        .expect("le lien est supprimé");

        let decision = host
            .resolve_mcp(&request(&id, "sandbox"), &caller)
            .await
            .expect("la résolution réussit");
        assert!(
            matches!(decision, McpDecision::Denied),
            "attendu Denied, obtenu : {decision:?}"
        );
        let g = state.lock().unwrap();
        assert_eq!(g.token_calls, 1, "le jeton a été reminté après révocation");
        assert_eq!(
            g.mcp_calls.len(),
            1,
            "la vérification du lien n'a pas précédé l'accès K8s"
        );
    }

    #[tokio::test]
    async fn principal_sans_lien_est_denie() {
        let (host, state, _ids, _) = host_with(&[], |_| {}).await;
        let decision = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &CallerPrincipal {
                    user_id: 42,
                    subject: "inconnu".to_string(),
                    is_admin: false,
                },
            )
            .await
            .expect("la résolution réussit");
        assert!(
            matches!(decision, McpDecision::Denied),
            "attendu Denied, obtenu : {decision:?}"
        );
        let g = state.lock().unwrap();
        assert!(g.mcp_calls.is_empty(), "un mint a eu lieu sans lien");
    }

    /// Lien vers un autre owner que celui de l'identifiant ⇒ Denied (`FR-2`).
    #[tokio::test]
    async fn lien_d_un_autre_owner_ne_pourvoit_pas() {
        let (host, state, ids, _) = host_with(&[("sub-bob", "bob")], |_| {}).await;
        let decision = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &caller_for(&ids, "sub-bob"),
            )
            .await
            .expect("la résolution réussit");
        assert!(
            matches!(decision, McpDecision::Denied),
            "attendu Denied, obtenu : {decision:?}"
        );
        let g = state.lock().unwrap();
        assert!(g.mcp_calls.is_empty());
    }

    /// Autres contextes et autres rôles ⇒ `Unprovided` (`FR-2`, `DEC-2`) — repli sur la
    /// liaison `mcp_servers` de miryad-core.
    #[tokio::test]
    async fn hors_contexte_sandbox_ou_role_autre_ne_pourvoit_rien() {
        let (host, state, ids, _) = host_with(&[("sub-ana", "ana")], |_| {}).await;
        let caller = caller_for(&ids, "sub-ana");
        let cases = [
            ("G_prod", "sandbox"),
            (&h_project_id("mon-projet"), "sandbox"),
            (&h_sandbox_id("ana", "web"), "fetch"),
            ("H_sandbox_anaweb", "sandbox"),
            ("H_sandbox_", "sandbox"),
        ];
        for (context, role) in cases {
            let decision = host
                .resolve_mcp(&request(context, role), &caller)
                .await
                .expect("la résolution réussit");
            assert!(
                matches!(decision, McpDecision::Unprovided),
                "contexte {context:?} rôle {role:?} : attendu Unprovided, obtenu {decision:?}"
            );
        }
        let g = state.lock().unwrap();
        assert!(g.mcp_calls.is_empty());
    }

    /// Sandbox absente ⇒ Denied, pas de mint (`FR-2`).
    #[tokio::test]
    async fn sandbox_absente_denie_sans_mint() {
        let (host, state, ids, _) = host_with(&[("sub-ana", "ana")], |g| {
            g.mcp_url = Err(HostGatewayError::Refused);
        })
        .await;
        let decision = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &caller_for(&ids, "sub-ana"),
            )
            .await
            .expect("la résolution réussit");
        assert!(
            matches!(decision, McpDecision::Denied),
            "attendu Denied, obtenu : {decision:?}"
        );
        let g = state.lock().unwrap();
        assert_eq!(g.token_calls, 0, "mint émis pour une sandbox absente");
    }

    /// `TokenRequest` en échec ⇒ Denied (Must de `host.sdd`).
    #[tokio::test]
    async fn token_en_echec_denie() {
        let (host, _state, ids, _) = host_with(&[("sub-ana", "ana")], |g| {
            g.token = Err(HostGatewayError::Refused);
        })
        .await;
        let decision = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &caller_for(&ids, "sub-ana"),
            )
            .await
            .expect("la résolution réussit");
        assert!(
            matches!(decision, McpDecision::Denied),
            "attendu Denied, obtenu : {decision:?}"
        );
    }

    /// Panne interne (cluster injoignable) ⇒ `HostError` sans contenu (`Handles`, `FR-1`) :
    /// ni texte, ni jeton.
    #[tokio::test]
    async fn panne_interne_hoteerror_sans_contenu() {
        let (host, _state, ids, _) = host_with(&[("sub-ana", "ana")], |g| {
            g.mcp_url = Err(HostGatewayError::Internal);
        })
        .await;
        let result = host
            .resolve_mcp(
                &request(&h_sandbox_id("ana", "web"), "sandbox"),
                &caller_for(&ids, "sub-ana"),
            )
            .await;
        let erreur = result.expect_err("attendu HostError");
        assert_eq!(format!("{erreur}"), "host resolution failed");
        assert_eq!(format!("{erreur:?}"), "HostError");
    }

    // ── Scenario « hors portée et inconnu indiscernables » (`AC-3`) + resolve_context ──

    #[tokio::test]
    async fn hors_portee_et_inconnu_indiscernables() {
        let (host, _state, ids, _) = host_with(&[("sub-bob", "bob")], |g| {
            g.projects
                .insert(("ana".to_string(), "secret-proj".to_string()));
        })
        .await;
        let caller = caller_for(&ids, "sub-bob");
        // projet d'un autre utilisateur : absent du namespace de bob ⇒ None
        let interdit = host
            .resolve_context(&h_project_id("secret-proj"), &caller)
            .await
            .expect("la résolution réussit");
        // projet inexistant : même réponse, sans distinction
        let absent = host
            .resolve_context(&h_project_id("inexistant"), &caller)
            .await
            .expect("la résolution réussit");
        assert!(interdit.is_none(), "projet d'autrui : {interdit:?}");
        assert_eq!(
            format!("{interdit:?}"),
            format!("{absent:?}"),
            "inexistant et interdit doivent être indiscernables"
        );
    }

    #[tokio::test]
    async fn resolve_context_projet_rend_les_parametres_sans_role() {
        let (host, _state, ids, _) = host_with(&[("sub-ana", "ana")], |g| {
            g.projects
                .insert(("ana".to_string(), "mon-projet".to_string()));
        })
        .await;
        let id = h_project_id("mon-projet");
        let ctx = host
            .resolve_context(&id, &caller_for(&ids, "sub-ana"))
            .await
            .expect("la résolution réussit")
            .expect("le contexte du projet est rendu");
        let p = params(&ctx);
        assert_eq!(p["owner"], serde_json::json!("ana"));
        assert_eq!(p["name"], serde_json::json!("mon-projet"));
        assert_eq!(p["namespace"], serde_json::json!("vanyline-ana"));
        assert!(role_names(&ctx).is_empty(), "un projet ne lie pas de rôle");
    }

    #[tokio::test]
    async fn resolve_context_sandbox_rend_le_role_sandbox() {
        let (host, _state, ids, _) = host_with(&[("sub-ana", "ana")], |g| {
            g.sandboxes.insert(("ana".to_string(), "web".to_string()));
        })
        .await;
        let id = h_sandbox_id("ana", "web");
        let ctx = host
            .resolve_context(&id, &caller_for(&ids, "sub-ana"))
            .await
            .expect("la résolution réussit")
            .expect("le contexte de la sandbox est rendu");
        let p = params(&ctx);
        assert_eq!(p["owner"], serde_json::json!("ana"));
        assert_eq!(p["name"], serde_json::json!("web"));
        assert_eq!(role_names(&ctx), vec!["sandbox".to_string()]);
    }

    /// Hors portée de l'appelant (lien vers un autre owner) ⇒ `None` sans distinction.
    #[tokio::test]
    async fn resolve_context_sandbox_hors_portee_none() {
        let (host, _state, ids, _) = host_with(&[("sub-bob", "bob")], |g| {
            g.sandboxes.insert(("ana".to_string(), "web".to_string()));
        })
        .await;
        let ctx = host
            .resolve_context(&h_sandbox_id("ana", "web"), &caller_for(&ids, "sub-bob"))
            .await
            .expect("la résolution réussit");
        assert!(ctx.is_none(), "attendu None, obtenu : {ctx:?}");
    }

    /// Tout autre identifiant ⇒ `None` (`FR-3`) : catalogue, hors alphabet, troncature,
    /// sandbox sans jointure.
    #[tokio::test]
    async fn resolve_context_hors_portee_none() {
        let (host, _state, ids, _) = host_with(&[("sub-ana", "ana")], |_| {}).await;
        let caller = caller_for(&ids, "sub-ana");
        for id in [
            "G_prod",
            "H_foo",
            "H_project_",
            "H_sandbox_ana",
            "H_sandbox_ana_",
            "H_sandbox__web",
            "h_project_monojet",
            "",
        ] {
            let ctx = host
                .resolve_context(id, &caller)
                .await
                .expect("la résolution réussit");
            assert!(ctx.is_none(), "{id:?} : attendu None, obtenu {ctx:?}");
        }
    }

    /// Un utilisateur sans lien ne résout aucun contexte (`FR-3`).
    #[tokio::test]
    async fn resolve_context_sans_lien_none() {
        let (host, _state, _ids, _) = host_with(&[], |_| {}).await;
        let ctx = host
            .resolve_context(
                &h_project_id("mon-projet"),
                &CallerPrincipal {
                    user_id: 42,
                    subject: "inconnu".to_string(),
                    is_admin: false,
                },
            )
            .await
            .expect("la résolution réussit");
        assert!(ctx.is_none(), "attendu None, obtenu : {ctx:?}");
    }

    // ── Scenario « H_ jamais écrit » ──
    // L'écriture est refusée par les règles de miryad-core : un identifiant `H_` est classé
    // `IdKind::Host` (jamais Private/Catalogue), donc jamais stocké. L'app n'ouvre aucun
    // chemin d'écriture ; l'assertion de montage (aucune route/MCP/config n'écrit de `H_`)
    // est portée par la tâche `[ ]` B9 des Tasks de `host.sdd`.

    #[test]
    fn identifiants_h_classes_hote_donc_jamais_ecrits() {
        use miryad_core::lymaille::identity::IdKind;
        for id in [
            h_project_id("mon-projet"),
            h_sandbox_id("ana", "web"),
            h_sandbox_id("jean-marc", "mon-projet"),
        ] {
            assert!(
                matches!(
                    miryad_core::lymaille::identity::classify(&id),
                    Ok(IdKind::Host)
                ),
                "{id} doit être un identifiant d'hôte, jamais écrivable"
            );
        }
    }

    // ── Encodage des identifiants (`DEC-9`, `[?]` des Tasks) ──

    #[test]
    fn encodage_h_aller_retour_sur_noms_dns() {
        let noms = ["web", "mon-projet", "a1", "abc-def-ghi"];
        for nom in noms {
            assert_eq!(
                parse_project_id(&h_project_id(nom)).as_deref(),
                Some(nom),
                "aller-retour projet sur {nom:?}"
            );
        }
        for owner in ["ana", "jean-marc"] {
            for nom in noms {
                assert!(
                    parse_sandbox_id(&h_sandbox_id(owner, nom))
                        .iter()
                        .any(|(o, n)| o == owner && n == nom),
                    "aller-retour sandbox sur {owner:?} × {nom:?}"
                );
            }
        }
    }

    /// Sur noms DNS-1123 sans tiret, l'encodage est injectif : deux paires distinctes,
    /// deux identifiants distincts.
    #[test]
    fn encodage_injectif_sur_noms_sans_tiret() {
        let mut vus = HashSet::new();
        for owner in ["ana", "bob", "jean"] {
            for nom in ["web", "api", "front"] {
                assert!(
                    vus.insert(h_sandbox_id(owner, nom)),
                    "collision sur {owner:?} × {nom:?}"
                );
            }
        }
        for nom in ["web", "api", "front"] {
            assert!(vus.insert(h_project_id(nom)), "collision sur {nom:?}");
        }
    }

    #[test]
    fn parse_project_id_rejette_les_formes_invalides() {
        for id in [
            "H_project_",
            "H_project_Édouard",
            "H_project_-web",
            "H_project_web-",
            "H_sandbox_ana_web",
            "G_prod",
        ] {
            assert!(
                parse_project_id(id).is_none(),
                "{id:?} ne doit pas se résoudre"
            );
        }
        // le `_` du suffixe est l'encodage d'un `-` : il se décode, sans ambiguïté
        assert_eq!(
            parse_project_id("H_project_a_b").as_deref(),
            Some("a-b"),
            "le `_` du suffixe encode un `-`"
        );
    }

    /// Le `_` de jointure est indistinguable d'un `_` encodé : la résolution inverse rend
    /// toutes les lectures valides ; l'appelant tranche par le lien d'owner.
    #[test]
    fn parse_sandbox_id_rend_les_lectures_valides() {
        assert_eq!(
            parse_sandbox_id("H_sandbox_ana_web"),
            vec![("ana".to_string(), "web".to_string())]
        );
        let mut attendus = parse_sandbox_id("H_sandbox_jean_marc_web");
        attendus.sort();
        assert_eq!(
            attendus,
            vec![
                ("jean".to_string(), "marc-web".to_string()),
                ("jean-marc".to_string(), "web".to_string()),
            ]
        );
        for id in [
            "H_sandbox_",
            "H_sandbox_ana",
            "H_sandbox__web",
            "H_sandbox_anaweb",
        ] {
            assert!(
                parse_sandbox_id(id).is_empty(),
                "{id:?} ne doit rien rendre"
            );
        }
    }

    /// Le lien de l'appelant lève l'ambiguïté de jointure : c'est l'Owner de l'appelant qui
    /// décide de la lecture candidate (`FR-2`).
    #[tokio::test]
    async fn le_lien_de_l_appelant_leve_l_ambiguite_de_jointure() {
        let (host, state, ids, _) = host_with(&[("sub-jean-marc", "jean-marc")], |g| {
            g.sandboxes
                .insert(("jean-marc".to_string(), "web".to_string()));
            g.sandboxes
                .insert(("jean".to_string(), "marc-web".to_string()));
        })
        .await;
        let id = h_sandbox_id("jean-marc", "web");
        let decision = host
            .resolve_mcp(&request(&id, "sandbox"), &caller_for(&ids, "sub-jean-marc"))
            .await
            .expect("la résolution réussit");
        assert!(
            matches!(decision, McpDecision::Granted(_)),
            "attendu Granted, obtenu : {decision:?}"
        );
        let g = state.lock().unwrap();
        assert_eq!(
            g.mcp_calls,
            vec![("jean-marc".to_string(), "web".to_string())],
            "la lecture candidate n'a pas été tranchée par le lien"
        );
    }
}
