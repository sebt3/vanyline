use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use vanyline_crds::{Application, Owner, OwnerSpec, Project, ProjectSpec, Sandbox, SandboxSpec};

use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use k8s_openapi::api::authentication::v1::{TokenRequest, TokenRequestSpec};
use k8s_openapi::api::core::v1::ServiceAccount;
use kube_runtime::watcher::{self, Config as WatcherConfig, Event};

use crate::error::VnyError;

/// Audience unique des jetons d'Owner (`POST …/serviceaccounts/{sa}/token`,
///cf. spec `owner_token`) — même valeur que `token_audience` par défaut de la
/// sandbox (`sandbox/src/config.sdd`).
pub const OWNER_TOKEN_AUDIENCE: &str = "vanyline-sandbox";

/// Durée de vie demandée à l'apiserver pour un jeton d'Owner, en secondes
/// (l'apiserver impose un minimum de 600 ; 900 = trois fois la marge).
const OWNER_TOKEN_EXPIRATION_SECS: i64 = 900;

/// Marge de réutilisation du cache, en secondes : un jeton est réutilisé tant
/// qu'il en reste STRICTEMENT plus que cette valeur à courir ; sinon un
/// nouveau jeton est demandé et remplace l'ancien.
const OWNER_TOKEN_REFRESH_MARGIN_SECS: i64 = 120;

/// Event de watch kube-runtime pour un Sandbox — normalisé pour la diffusion WS.
#[derive(Clone, Debug)]
pub enum WatchEvent<T> {
    /// Objet créé (replay initial `InitApply` ou création en cours de flux).
    Added(T),
    /// Objet modifié (`Apply`).
    Modified(T),
    /// Objet supprimé (`Delete`).
    Deleted(T),
    /// Erreur interne du watcher (message de l'erreur) ; le stream n'est pas
    /// terminé par la lib.
    Error(String),
}

/// Jeton d'authentification d'un Owner auprès de SES sandboxes (unique
/// credential accepté, cf. `sandbox/src/auth.sdd`). Son @Debug MASQUE la
/// valeur du jeton : il ne doit jamais être journalisé, sérialisé ni exposé.
#[derive(Clone)]
pub struct OwnerToken {
    /// Le jeton JWT émis par l'apiserver (jamais affiché par Debug/Display).
    pub token: String,
    /// Expiration posée par l'apiserver (`status.expirationTimestamp`), ou
    /// `maintenant + 900 s` en repli si la réponse ne la porte pas.
    pub expires_at: DateTime<Utc>,
}

/// Debug contractuel : la valeur de `token` est masquée (`"<redacted>"`),
/// seule la date d'expiration reste visible.
impl std::fmt::Debug for OwnerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Décision pure du cache de jetons : un jeton expirant à `expires_at`
/// est-il encore réutilisable à `now` (plus de
/// `OWNER_TOKEN_REFRESH_MARGIN_SECS` secondes de validité strictement
/// restantes) ?
#[must_use]
fn token_is_fresh(now: DateTime<Utc>, expires_at: DateTime<Utc>) -> bool {
    expires_at
        .checked_sub_signed(chrono::Duration::seconds(OWNER_TOKEN_REFRESH_MARGIN_SECS))
        .is_some_and(|refresh_at| now < refresh_at)
}

/// Décision pure de l'horodatage d'expiration du jeton : la valeur rendue par
/// l'apiserver (`apiserver_expiry`) prime ; en son absence (ou si elle est
/// hors plage chrono), repli sur `now + OWNER_TOKEN_EXPIRATION_SECS`.
///
/// # Errors
///
/// `VnyError::InternalError` si ni l'expiration de l'apiserver ni le repli
/// `now + 900 s` ne sont représentables (horodatage hors plage chrono).
fn token_expires_at(
    now: DateTime<Utc>,
    apiserver_expiry: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>, VnyError> {
    apiserver_expiry.map_or_else(
        || {
            now.checked_add_signed(chrono::Duration::seconds(OWNER_TOKEN_EXPIRATION_SECS))
                .ok_or_else(|| {
                    VnyError::InternalError(
                        "expiration du jeton hors de la plage chrono".to_string(),
                    )
                })
        },
        Ok,
    )
}

/// Résolution pure du namespace d'un Owner : `{app_namespace}-{owner}` par la
/// fonction partagée `vanyline_crds::owner_namespace_name` (aucun appel
/// cluster). L'erreur de validation (nom hors DNS-1123 ou > 63 caractères)
/// est aplatie en `VnyError::K8sApiError` (`VNL-K8S-002`), comme les rejets
/// de nom par l'apiserver.
///
/// # Errors
///
/// `VnyError::K8sApiError` si le nom calculé n'est pas un label DNS-1123
/// valide d'au plus 63 caractères.
fn owner_namespace_in(app_namespace: &str, owner: &str) -> Result<String, VnyError> {
    vanyline_crds::owner_namespace_name(app_namespace, owner).map_err(VnyError::K8sApiError)
}

/// Client K8s typé pour les CRDs Owner/Project/Sandbox/Application — UN OWNER
/// = UN NAMESPACE (2026-10-04, `/controller/src/namespace.sdd`). Le client
/// retient `app_namespace` (namespace des CR Owner/Application : flag
/// `--namespace`, `defaults.namespace` ou contexte kubeconfig, cf. `discover`)
/// et résout le namespace de chaque Owner par
/// `vanyline_crds::owner_namespace_name` — sans appel cluster. Les opérations
/// Project/Sandbox prennent le NOM D'OWNER en premier paramètre et opèrent
/// dans `<app-ns>-<owner>` ; les opérations Owner/Application opèrent dans
/// `app_namespace`.
#[derive(Clone)]
pub struct VnlK8sClient {
    client: kube::Client,
    app_namespace: String,
    /// Cache des jetons d'Owner, partagé par tous les clones du client (jamais
    /// persisté, jamais loggé).
    owner_tokens: Arc<Mutex<HashMap<String, OwnerToken>>>,
}

impl VnlK8sClient {
    /// Découvre la config K8s (in-cluster, ou `~/.kube/config` sinon —
    /// `kube::Config::infer()` gère les deux). `namespace_override` prime
    /// sur le namespace du contexte kubeconfig courant si fourni (`Some`) ;
    /// sinon `Config::infer()` fournit déjà le default namespace résolu du
    /// contexte courant. La valeur retenue est le NAMESPACE D'APPLICATION
    /// (CR Owner/Application) : les namespaces d'Owners en sont dérivés.
    /// Erreur `VNL-K8S-001` si aucune config n'est joignable (pas de cluster,
    /// pas de kubeconfig).
    ///
    /// # Errors
    ///
    /// `VnyError::K8sConfigError` (`VNL-K8S-001`) si aucune config K8s n'est
    /// joignable (ni in-cluster, ni kubeconfig).
    pub async fn discover(namespace_override: Option<String>) -> Result<Self, VnyError> {
        let config = kube::Config::infer()
            .await
            .map_err(|e| VnyError::K8sConfigError(e.to_string()))?;
        let app_namespace = namespace_override.unwrap_or_else(|| config.default_namespace.clone());
        let client =
            kube::Client::try_from(config).map_err(|e| VnyError::K8sConfigError(e.to_string()))?;
        Ok(Self {
            client,
            app_namespace,
            owner_tokens: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Namespace de l'Owner `owner` (`{app_namespace}-{owner}`), validé sans
    /// appel cluster.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom calculé n'est pas un
    /// label DNS-1123 valide d'au plus 63 caractères.
    fn owner_namespace(&self, owner: &str) -> Result<String, VnyError> {
        owner_namespace_in(&self.app_namespace, owner)
    }

    /// Liste les Owners du namespace d'application.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si l'appel API échoue.
    pub async fn list_owners(&self) -> Result<Vec<Owner>, VnyError> {
        list(&self.client, &self.app_namespace).await
    }
    /// Lit un Owner du namespace d'application (erreur si absent).
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si l'appel API échoue (absent =
    /// `NotFound` aplati dans ce marqueur).
    pub async fn get_owner(&self, name: &str) -> Result<Owner, VnyError> {
        get(&self.client, &self.app_namespace, name).await
    }
    /// Crée un Owner `name` (spec `spec`) dans le namespace d'application.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si l'appel API échoue.
    pub async fn create_owner(&self, name: &str, spec: OwnerSpec) -> Result<Owner, VnyError> {
        create(&self.client, &self.app_namespace, Owner::new(name, spec)).await
    }
    /// Supprime un Owner du namespace d'application.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si l'appel API échoue.
    pub async fn delete_owner(&self, name: &str) -> Result<(), VnyError> {
        delete::<Owner>(&self.client, &self.app_namespace, name).await
    }

    /// Lit une Application du namespace d'application (seule opération sur
    /// Application : lecture seule).
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si l'appel API échoue (absente
    /// = `NotFound` aplati dans ce marqueur).
    pub async fn get_application(&self, name: &str) -> Result<Application, VnyError> {
        get(&self.client, &self.app_namespace, name).await
    }

    /// Liste les Projects du namespace de l'Owner `owner`.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn list_projects(&self, owner: &str) -> Result<Vec<Project>, VnyError> {
        let ns = self.owner_namespace(owner)?;
        list(&self.client, &ns).await
    }
    /// Lit un Project du namespace de l'Owner `owner` (erreur si absent).
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn get_project(&self, owner: &str, name: &str) -> Result<Project, VnyError> {
        let ns = self.owner_namespace(owner)?;
        get(&self.client, &ns, name).await
    }
    /// Crée un Project `name` (spec `spec`) dans le namespace de l'Owner
    /// `owner`.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn create_project(
        &self,
        owner: &str,
        name: &str,
        spec: ProjectSpec,
    ) -> Result<Project, VnyError> {
        let ns = self.owner_namespace(owner)?;
        create(&self.client, &ns, Project::new(name, spec)).await
    }
    /// Supprime un Project du namespace de l'Owner `owner`.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn delete_project(&self, owner: &str, name: &str) -> Result<(), VnyError> {
        let ns = self.owner_namespace(owner)?;
        delete::<Project>(&self.client, &ns, name).await
    }

    /// Liste les Sandboxes du namespace de l'Owner `owner`.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn list_sandboxes(&self, owner: &str) -> Result<Vec<Sandbox>, VnyError> {
        let ns = self.owner_namespace(owner)?;
        list(&self.client, &ns).await
    }
    /// Lit une Sandbox du namespace de l'Owner `owner` (erreur si absente).
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn get_sandbox(&self, owner: &str, name: &str) -> Result<Sandbox, VnyError> {
        let ns = self.owner_namespace(owner)?;
        get(&self.client, &ns, name).await
    }
    /// Crée une Sandbox `name` (spec `spec`) dans le namespace de l'Owner
    /// `owner`.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn create_sandbox(
        &self,
        owner: &str,
        name: &str,
        spec: SandboxSpec,
    ) -> Result<Sandbox, VnyError> {
        let ns = self.owner_namespace(owner)?;
        create(&self.client, &ns, Sandbox::new(name, spec)).await
    }
    /// Supprime une Sandbox du namespace de l'Owner `owner`.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue.
    pub async fn delete_sandbox(&self, owner: &str, name: &str) -> Result<(), VnyError> {
        let ns = self.owner_namespace(owner)?;
        delete::<Sandbox>(&self.client, &ns, name).await
    }

    /// Patch `spec.suspended` d'un Sandbox de l'Owner `owner` (merge patch
    /// JSON, pas de remplacement complet de la spec). Retourne l'objet patché
    /// (contrairement à `delete_sandbox` qui retourne `()`) — stop/start est
    /// une transition d'état, l'appelant (CLI/RPC) veut voir le nouveau statut
    /// immédiatement. Pas de vérification d'existence préalable.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si l'appel API échoue (absente = `NotFound` aplati
    /// dans ce marqueur).
    pub async fn set_sandbox_suspended(
        &self,
        owner: &str,
        name: &str,
        suspended: bool,
    ) -> Result<Sandbox, VnyError> {
        let ns = self.owner_namespace(owner)?;
        let api: kube::Api<Sandbox> = kube::Api::namespaced(self.client.clone(), &ns);
        let patch = serde_json::json!({ "spec": { "suspended": suspended } });
        api.patch(
            name,
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&patch),
        )
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))
    }

    /// URL MCP HTTP-streamable de la sandbox `name` de l'Owner `owner`, posée
    /// par le controller (`vanyline_crds::service_name`/`MCP_PORT`, namespace
    /// de l'Owner). Vérifie d'abord que la sandbox existe (`get_sandbox`) —
    /// erreur claire si ce n'est pas le cas, plutôt qu'un échec de connexion
    /// confus plus tard dans le tour d'inférence.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si la lecture de garde de la sandbox échoue
    /// (absente = `NotFound` aplati dans ce marqueur).
    pub async fn sandbox_mcp_url(&self, owner: &str, name: &str) -> Result<String, VnyError> {
        let ns = self.owner_namespace(owner)?;
        get::<Sandbox>(&self.client, &ns, name).await?;
        Ok(format!(
            "http://{}.{}.svc:{}/mcp",
            vanyline_crds::service_name(name),
            ns,
            vanyline_crds::MCP_PORT
        ))
    }

    /// URL interne de `POST /ws/ticket` de la sandbox `name` de l'Owner
    /// `owner` (même patron que `sandbox_mcp_url`, chemin `/ws/ticket`,
    /// namespace de l'Owner). Vérifie d'abord que la sandbox existe — erreur
    /// claire si ce n'est pas le cas, plutôt qu'un échec de connexion confus
    /// plus tard.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si la lecture de garde de la sandbox échoue.
    pub async fn sandbox_ws_ticket_url(&self, owner: &str, name: &str) -> Result<String, VnyError> {
        let ns = self.owner_namespace(owner)?;
        get::<Sandbox>(&self.client, &ns, name).await?;
        Ok(format!(
            "http://{}.{}.svc:{}/ws/ticket",
            vanyline_crds::service_name(name),
            ns,
            vanyline_crds::MCP_PORT
        ))
    }

    /// URL interne de `/git/*` de la sandbox `name` de l'Owner `owner` (même
    /// patron que `sandbox_ws_ticket_url`, chemin `/git/<raw_path>`, namespace
    /// de l'Owner). Vérifie d'abord que la sandbox existe — erreur claire si
    /// ce n'est pas le cas, plutôt qu'un échec de connexion confus plus tard.
    ///
    /// `raw_path` doit être **déjà** un chemin percent-encodé valide et
    /// validé par l'appelant (pas de segment `.`/`..`) — cette fonction ne
    /// ré-encode plus rien (contrairement à une version précédente qui
    /// appelait `encode_git_path` ici : décoder puis ré-encoder perdait la
    /// distinction entre un `%2F` légitime à l'intérieur d'un segment, ex.
    /// un nom de branche contenant `/`, et un vrai séparateur de chemin).
    /// Voir `app::api::sandboxes::raw_git_tail` pour la construction et la
    /// validation de `raw_path` à partir de la requête brute.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide ou si la lecture de garde de la sandbox échoue.
    pub async fn sandbox_git_url(
        &self,
        owner: &str,
        name: &str,
        raw_path: &str,
    ) -> Result<String, VnyError> {
        let ns = self.owner_namespace(owner)?;
        get::<Sandbox>(&self.client, &ns, name).await?;
        Ok(format!(
            "http://{}.{}.svc:{}/git/{}",
            vanyline_crds::service_name(name),
            ns,
            vanyline_crds::MCP_PORT,
            raw_path
        ))
    }

    /// Retourne un stream de watch sur les CRD Sandbox du namespace de
    /// l'Owner `owner` — un flux par Owner suivi, jamais cluster-wide
    /// (UN OWNER = UN NAMESPACE). Le stream émet `WatchEvent::Added`/
    /// `Modified`/`Deleted` sur chaque objet et `WatchEvent::Error` sur toute
    /// erreur de connexion kube (l'appelant doit relancer la boucle externe en
    /// cas d'erreur). Un nom d'Owner produisant un namespace invalide est
    /// rendu par un unique item `WatchEvent::Error` (le watch ne lève jamais).
    ///
    /// Boxé (`Pin<Box<dyn Stream>>`) plutôt que `impl Stream` : le stream est
    /// stocké derrière un champ dans le hub WS de `app`, un type nommé y est
    /// plus commode qu'un `impl Trait` opaque propagé partout.
    pub fn watch_sandboxes(
        &self,
        owner: &str,
    ) -> std::pin::Pin<Box<dyn Stream<Item = WatchEvent<Sandbox>> + Send>> {
        let ns = match self.owner_namespace(owner) {
            Ok(ns) => ns,
            Err(e) => {
                return Box::pin(futures::stream::once(async move {
                    WatchEvent::Error(e.to_string())
                }));
            }
        };
        let api: kube::Api<Sandbox> = kube::Api::namespaced(self.client.clone(), &ns);
        // timeout serveur 5 min : un reconnect trop court re-liste toutes les
        // sandboxes à chaque tour (bookmark perdu) → salve de refetch côté WS.
        let cfg = WatcherConfig::default().timeout(300);
        let stream = watcher::watcher(api, cfg).filter_map(|result| async move {
            match result {
                Ok(Event::Apply(obj)) => Some(WatchEvent::Modified(obj)),
                Ok(Event::Delete(obj)) => Some(WatchEvent::Deleted(obj)),
                Ok(Event::InitApply(obj)) => Some(WatchEvent::Added(obj)),
                // Init/InitDone : pas d'objet — ignorés (le replay initial se fait
                // via les InitApply ci-dessus, un par sandbox existante).
                Ok(Event::Init | Event::InitDone) => None,
                Err(e) => Some(WatchEvent::Error(e.to_string())),
            }
        });
        Box::pin(stream)
    }

    /// Lit un jeton d'Owner valide depuis le cache partagé (tous les clones
    /// du client) si celui-ci a plus de `OWNER_TOKEN_REFRESH_MARGIN_SECS` de
    /// validité restante à `now`.
    fn cached_owner_token(&self, owner: &str, now: DateTime<Utc>) -> Option<OwnerToken> {
        let guard = self
            .owner_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(owner)
            .filter(|t| token_is_fresh(now, t.expires_at))
            .cloned()
    }

    /// Lit le jeton d'authentification de l'Owner `owner` (uprès de SES
    /// sandboxes — cf. `sandbox/src/auth.sdd`) :
    /// 1. cache partagé réutilisé s'il reste plus de 120 s de validité ;
    /// 2. sinon lecture de `owner.status.serviceAccount` dans le namespace
    ///    d'application (jamais un nom reconstruit à la main) — Owner ou SA
    ///    absent = erreur explicite sans demande de jeton ;
    /// 3. `TokenRequest` sur ce SA dans le namespace de l'Owner
    ///    (`audiences = [OWNER_TOKEN_AUDIENCE]`, `expirationSeconds = 900`,
    ///    sans `boundObjectRef`), puis mise en cache (jamais persisté, jamais
    ///    loggé — le Debug de `OwnerToken` masque la valeur).
    ///
    /// L'appelant est responsable du jeton : il le journalise pas, ne le
    /// renvoie à aucun navigateur, et vérifie AVANT de le demander que son
    /// propre utilisateur a le droit d'agir en tant que cet Owner.
    ///
    /// # Errors
    ///
    /// `VnyError::K8sApiError` (`VNL-K8S-002`) si le nom d'Owner produit un
    /// namespace invalide, si l'Owner ou son `status.serviceAccount` est
    /// absent (aucune demande de jeton émise dans ce cas), ou si l'appel
    /// `TokenRequest` échoue.
    pub async fn owner_token(&self, owner: &str) -> Result<OwnerToken, VnyError> {
        let now = Utc::now();
        if let Some(cached) = self.cached_owner_token(owner, now) {
            return Ok(cached);
        }
        let owner_obj = self.get_owner(owner).await.map_err(|e| {
            VnyError::K8sApiError(format!("Owner «{owner}» introuvable pour son jeton: {e}"))
        })?;
        let service_account = owner_obj
            .status
            .as_ref()
            .and_then(|s| s.service_account.clone())
            .ok_or_else(|| {
                VnyError::K8sApiError(format!(
                    "Owner «{owner}» sans status.serviceAccount (controller pas encore reconcilié ?)"
                ))
            })?;
        let ns = self.owner_namespace(owner)?;
        let api: kube::Api<ServiceAccount> = kube::Api::namespaced(self.client.clone(), &ns);
        let request = TokenRequest {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta::default(),
            spec: Some(TokenRequestSpec {
                audiences: Some(vec![OWNER_TOKEN_AUDIENCE.to_string()]),
                bound_object_ref: None,
                expiration_seconds: Some(OWNER_TOKEN_EXPIRATION_SECS),
            }),
            status: None,
        };
        let response = api
            .create_token_request(&service_account, &kube::api::PostParams::default(), &request)
            .await
            .map_err(|e| {
                VnyError::K8sApiError(format!(
                    "TokenRequest émis pour le ServiceAccount «{service_account}» de l'Owner «{owner}»: {e}"
                ))
            })?;
        let token = response
            .status
            .as_ref()
            .and_then(|s| s.token.clone())
            .ok_or_else(|| {
                VnyError::K8sApiError("réponse TokenRequest sans status.token".to_string())
            })?;
        let apiserver_expiry = response
            .status
            .as_ref()
            .and_then(|s| s.expiration_timestamp.as_ref())
            .and_then(|t| {
                DateTime::from_timestamp(
                    t.0.as_second(),
                    u32::try_from(t.0.subsec_nanosecond()).unwrap_or(0),
                )
            });
        let expires_at = token_expires_at(now, apiserver_expiry)?;
        let jeton = OwnerToken { token, expires_at };
        self.owner_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(owner.to_string(), jeton.clone());
        Ok(jeton)
    }
}

async fn list<K>(client: &kube::Client, ns: &str) -> Result<Vec<K>, VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    let list = api
        .list(&kube::api::ListParams::default())
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))?;
    Ok(list.items)
}

async fn get<K>(client: &kube::Client, ns: &str, name: &str) -> Result<K, VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    api.get(name)
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))
}

async fn create<K>(client: &kube::Client, ns: &str, obj: K) -> Result<K, VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + serde::Serialize
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    api.create(&kube::api::PostParams::default(), &obj)
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))
}

async fn delete<K>(client: &kube::Client, ns: &str, name: &str) -> Result<(), VnyError>
where
    K: kube::Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    let api: kube::Api<K> = kube::Api::namespaced(client.clone(), ns);
    api.delete(name, &kube::api::DeleteParams::default())
        .await
        .map_err(|e| VnyError::K8sApiError(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::encode_git_path;
    use chrono::Duration;

    #[test]
    fn encode_git_path_preserves_separators() {
        assert_eq!(encode_git_path("branches/feature-x"), "branches/feature-x");
        assert_eq!(encode_git_path("status"), "status");
    }

    #[test]
    fn encode_git_path_encodes_special() {
        assert_eq!(
            encode_git_path("branches/feature x"),
            "branches/feature%20x"
        );
        // Le '%' est codé en %25, donc %20 devient %2520
        assert_eq!(encode_git_path("a%20b"), "a%2520b");
    }

    // ── UN OWNER = UN NAMESPACE (2026-10-04) ──

    /// Scenario: nom du namespace résolu depuis `app_namespace` + nom d'Owner,
    /// sans appel cluster (fonction pure partagée avec le controller).
    #[test]
    fn owner_namespace_nom_valide() {
        assert_eq!(
            owner_namespace_in("vanyline-app", "alice").unwrap(),
            "vanyline-app-alice"
        );
    }

    /// Scenario: nom de namespace invalide (majuscule, trop long) → erreur
    /// typée VNL-K8S-002 rendue AVANT tout appel à l'apiserver.
    #[test]
    fn owner_namespace_nom_invalide_vnl_k8s_002() {
        let err = owner_namespace_in("vanyline-app", "Alice").expect_err("majuscule interdite");
        assert!(matches!(err, VnyError::K8sApiError(_)));
        assert!(err.to_string().contains("VNL-K8S-002"));
        assert!(owner_namespace_in(&"a".repeat(40), &"b".repeat(40)).is_err());
    }

    // ── owner_token ──

    /// Scenario: `owner_token_demande_a_l_apiserver_avec_la_bonne_audience`
    /// (portion constante vérifiable sans cluster — la demande réelle est un
    /// Scenario d'intégration cluster).
    #[test]
    fn owner_token_audience_constante() {
        assert_eq!(OWNER_TOKEN_AUDIENCE, "vanyline-sandbox");
    }

    /// Scenario: `owner_token_debug_masque_le_jeton` — le `Debug` de `OwnerToken`
    /// ne doit jamais exposer la valeur du jeton.
    #[test]
    fn owner_token_debug_masque_le_jeton() {
        let jeton = OwnerToken {
            token: "secret-jwt".to_string(),
            expires_at: Utc::now(),
        };
        let rendu = format!("{jeton:?}");
        assert!(
            !rendu.contains("secret-jwt"),
            "Debug fuit le jeton: {rendu}"
        );
    }

    /// Scenario: `owner_token_cache_par_owner` (décision pure de réutilisation —
    /// le même jeton sans nouvel appel quand il reste plus de 120 s).
    #[test]
    fn owner_token_cache_reutilise_jeton_frais() {
        let now = Utc::now();
        assert!(token_is_fresh(now, now + Duration::seconds(900)));
        assert!(token_is_fresh(now, now + Duration::seconds(121)));
    }

    /// Scenario: `owner_token_renouvele_pres_de_l_expiration` (décision pure —
    /// marge de 120 s : à 120 s ou moins restants, un nouveau jeton est dû).
    #[test]
    fn owner_token_cache_renouvelle_sous_la_marge() {
        let now = Utc::now();
        assert!(!token_is_fresh(now, now + Duration::seconds(120)));
        assert!(!token_is_fresh(now, now + Duration::seconds(60)));
        assert!(!token_is_fresh(now, now - Duration::seconds(1)));
    }
}
