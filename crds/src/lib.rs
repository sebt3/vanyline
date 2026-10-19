//! Définitions des quatre CRDs namespacées du controller
//! (`vanyline.solidite.fr/v1alpha1` — Owner, Project, Sandbox, Application) :
//! specs, statuses et leurs schémas `OpenAPI`, génération des manifests
//! (`crd_manifests`) et conventions de nommage partagées du monorepo
//! (`owner_namespace_name`, `service_name`, `MCP_PORT`). Aucune logique de
//! reconcile : le comportement observable est intégralement de la
//! (dé)sérialisation. Contrat détaillé : `lib.sdd`.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

use std::collections::BTreeMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, LabelSelector, Time};
use kube::{CustomResource, CustomResourceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Spec de l'`Owner` (identité cluster d'un utilisateur) — entièrement
/// optionnelle : un `spec: {}` est valide et produit le `OwnerSpec` par défaut.
/// Les défauts métier (`"1Gi"`, modes d'accès) sont appliqués au reconcile,
/// jamais ici.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "vanyline.solidite.fr",
    version = "v1alpha1",
    kind = "Owner",
    namespaced,
    status = "OwnerStatus",
    printcolumn = r#"{"name":"SA","type":"string","jsonPath":".status.serviceAccount"}"#,
    printcolumn = r#"{"name":"PVC","type":"string","jsonPath":".status.pvcName"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct OwnerSpec {
    /// Taille du PVC home. None => le défaut métier `"1Gi"` est appliqué au
    /// reconcile, jamais ici.
    pub home_size: Option<String>,
    /// `StorageClass` du PVC home. None => classe par défaut du cluster
    /// (RWX recommandé, ex. `CephFS`).
    pub home_storage_class: Option<String>,
    /// Mode d'accès du PVC home. None => "`ReadWriteMany`" (défaut historique —
    /// le home est partagé entre les sandboxes concurrentes d'un même Owner).
    /// À ajuster (ex. "`ReadWriteOnce`") sur un cluster sans `StorageClass` RWX
    /// (ex. single-node local-path) — aucun partage concurrent alors possible.
    pub home_access_mode: Option<String>,
    /// Replis de stockage appliqués aux Projects futurs de cet Owner.
    pub project_defaults: Option<ProjectDefaults>,
    /// Nom de la CR Application dont l'Ingress sert de base aux sous-domaines
    /// de sandbox (`{sandbox}.sandboxes.{application.host}`). None => la
    /// Sandbox reste ClusterIP-only (comportement actuel, pas d'erreur).
    pub application_ref: Option<String>,
    /// White-list egress du namespace de l'Owner. Absent => vec vide.
    #[serde(default)]
    pub egress: Vec<EgressRule>,
}

/// Replis de stockage servant de défaut pour les champs de [`ProjectSpec`] quand
/// il ne les pose pas ; le repli final est interprété par le controller.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDefaults {
    /// Taille de PVC workspace par défaut pour les Projects de l'Owner.
    pub storage_size: Option<String>,
    /// `StorageClass` de PVC workspace par défaut pour les Projects de l'Owner.
    pub storage_class: Option<String>,
    /// Défaut de repli pour `ProjectSpec.storage_access_mode` quand ce
    /// dernier est absent. None => "`ReadWriteOnce`" (défaut historique).
    pub storage_access_mode: Option<String>,
}

/// Référence au controller d'Ingress du cluster (pods à autoriser comme peer
/// `NetworkPolicy` des sandboxes). Une map `pod_labels` vide sérialisée `{}`
/// est valide ; aucun `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IngressControllerRef {
    /// Namespace des pods du controller d'Ingress (ex. "kydah-core").
    pub namespace: String,
    /// Labels identifiant les pods du controller (ex.
    /// `app.kubernetes.io/name: traefik`, `app.kubernetes.io/component:
    /// controller`).
    pub pod_labels: BTreeMap<String, String>,
}

/// Une règle d'une white-list egress. `cidr` et `pod_selector` sont
/// exclusifs (l'un ou l'autre, jamais les deux) ; `namespace_selector` est
/// optionnel et se combine avec `pod_selector`. Aucune validation de cette
/// exclusivité au niveau du type — portée par la construction de la
/// `NetworkPolicy` (tâche `netpol-builder`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EgressRule {
    /// Description humaine de la règle (non vide — NON vérifiée ici).
    pub description: String,
    /// Bloc CIDR destinataire. Exclusif avec `pod_selector` (porté par le
    /// netpol-builder, pas par ce type).
    pub cidr: Option<String>,
    /// Pods destinataires. Exclusif avec `cidr`.
    pub pod_selector: Option<LabelSelector>,
    /// Sélecteur de namespaces, combinable avec `pod_selector`.
    pub namespace_selector: Option<LabelSelector>,
    /// Ports autorisés. Absent => vec vide ; TCP-pour-`None` est une
    /// interprétation du netpol-builder, pas une valeur posée ici.
    #[serde(default)]
    pub ports: Vec<EgressPort>,
}

/// Port autorisé dans une [`EgressRule`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EgressPort {
    /// Numéro de port ; aucune validation de contenu ici (c'est l'apiserver
    /// qui rejette un hors-plage à l'usage).
    pub port: i32,
    /// "TCP" | "UDP". `None` => TCP (interprété par `netpol-builder`, pas ici).
    pub protocol: Option<String>,
}

/// Status de l'Owner, posé par son reconciler.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OwnerStatus {
    /// Nom du PVC home de l'Owner.
    pub pvc_name: Option<String>,
    /// Nom du `ServiceAccount` de l'Owner.
    pub service_account: Option<String>,
    /// Namespace de l'Owner (`{app_namespace}-{owner}`, calculé par
    /// [`owner_namespace_name`]) — posé par le reconciler. UN OWNER = UN NAMESPACE
    /// (décision 2026-10-04).
    pub namespace: Option<String>,
    /// Conditions K8s standard. Absent => vec vide.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Spec du `Project` (dépôt git d'un Owner) : seuls `owner` et `repoUrl` sont
/// requis ; les replis (`caches`, `fetchInterval`, modes d'accès) vivent côté
/// controller.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "vanyline.solidite.fr",
    version = "v1alpha1",
    kind = "Project",
    namespaced,
    status = "ProjectStatus",
    printcolumn = r#"{"name":"Owner","type":"string","jsonPath":".spec.owner"}"#,
    printcolumn = r#"{"name":"Repo","type":"string","jsonPath":".spec.repoUrl"}"#,
    printcolumn = r#"{"name":"Cloned","type":"boolean","jsonPath":".status.cloned"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSpec {
    /// Owner (utilisateur) auquel appartient le projet. REQUIS.
    pub owner: String,
    /// URL du dépôt git. REQUIS.
    pub repo_url: String,
    /// Branche par défaut du repo.
    pub default_branch: Option<String>,
    /// Taille du PVC workspace. Repli : `Owner.spec.project_defaults`, puis
    /// défaut du controller.
    pub storage_size: Option<String>,
    /// `StorageClass` du PVC workspace. Repli : `Owner.spec.project_defaults`.
    pub storage_class: Option<String>,
    /// Mode d'accès du PVC workspace. None => repli sur
    /// `Owner.spec.project_defaults.storage_access_mode`, puis
    /// "`ReadWriteOnce`" (défaut historique).
    pub storage_access_mode: Option<String>,
    /// DÉPRÉCIÉ — plus consommé par le controller depuis git-integration
    /// (la clé SSH vit dans le PVC Owner, cf. docs/features/git-integration.md
    /// section 0). Champ conservé pour la migration, pas supprimé.
    pub git_secret: Option<String>,
    /// Caches partagés. None => `["cargo", "pnpm", "pip"]`.
    pub caches: Option<Vec<String>>,
    /// Intervalle du `CronJob` de fetch. None => "1h".
    pub fetch_interval: Option<String>,
    /// White-list egress du namespace du projet. Absent => vec vide.
    #[serde(default)]
    pub egress: Vec<EgressRule>,
}

/// Status du Project. Les couples `skip_serializing_if` de
/// `languages`/`detected_at` sont LE contrat anti-écrasement du merge-patch
/// de routine (ils protègent les valeurs écrites par le Job `detect`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    /// Nom du PVC workspace (toujours celui créé par le controller).
    pub pvc_name: Option<String>,
    /// Le clone bare initial est en place. Sérialisé même à `false` :
    /// `compute_status` est la seule source de ce champ.
    #[serde(default)]
    pub cloned: bool,
    /// Horodatage du dernier fetch réussi.
    pub last_fetch: Option<Time>,
    /// Worktree présents dans le workspace. Absent => vec vide.
    #[serde(default)]
    pub worktrees: Vec<String>,
    /// Conditions K8s standard. Absent => vec vide.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Langages détectés. Valeurs possibles : "rust", "js-ts", "vue",
    /// "python", "dockerfile" (marqueur `dockerfile` : noms
    /// Dockerfile/Containerfile/variants dans l'arbre HEAD). Ordre fixe
    /// `["rust", "js-ts", "vue", "python", "dockerfile"]` filtré — cf.
    /// `vanyline_sandbox::maint::detect_languages`. Écrit uniquement par le
    /// Job `detect` (tâche 03) via un patch dédié — jamais par
    /// `compute_status`. `skip_serializing_if` : voir note "Point
    /// d'attention" dans le fichier de tâche — indispensable pour que le
    /// merge patch de routine du reconciler Project n'écrase pas cette
    /// valeur.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Horodatage du dernier `detect` réussi. Même raison `skip_serializing_if`
    /// que `languages`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_at: Option<Time>,
}

/// Spec de la `Sandbox` (pod de travail d'un Owner sur un Project) : seuls
/// `project` et `branch` sont requis ; l'image par défaut et la sémantique de
/// `suspended` sont du ressort du controller.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "vanyline.solidite.fr",
    version = "v1alpha1",
    kind = "Sandbox",
    namespaced,
    status = "SandboxStatus",
    printcolumn = r#"{"name":"Project","type":"string","jsonPath":".spec.project"}"#,
    printcolumn = r#"{"name":"Branch","type":"string","jsonPath":".spec.branch"}"#,
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSpec {
    /// Project (nom de la CR) dont le workspace est monté. REQUIS.
    pub project: String,
    /// Branche checkout dans le worktree de la sandbox. REQUIS.
    pub branch: String,
    /// Toolchains montées via `volumes[].image`. Absent => vec vide.
    #[serde(default)]
    pub toolchains: Vec<Toolchain>,
    /// Image du serveur sandbox. None => défaut du controller (env).
    pub image: Option<String>,
    /// `ResourceRequirements` du conteneur, transmis tels quels à l'apiserver.
    pub resources: Option<k8s_openapi::api::core::v1::ResourceRequirements>,
    /// White-list egress du pod. Absent => vec vide.
    #[serde(default)]
    pub egress: Vec<EgressRule>,
    /// Arrêt manuel : true => le reconciler supprime le Pod (worktree, PVC,
    /// Service, `NetworkPolicies` conservés), status.phase devient
    /// "Suspended". false => le Pod est recréé (chemin nominal).
    #[serde(default)]
    pub suspended: bool,
}

/// Spec LSP d'une toolchain : `image` est l'image OCI du serveur LSP (montée sur
/// `/toolchains/<name>-lsp`), `bin` un chemin absolu dans le pod (point de montage
/// inclus), `args` les arguments du serveur. None sur `Toolchain.lsp` => preset
/// name-keyed résolu par le controller (`resolve_toolchain_lsp`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspSpec {
    /// Image OCI du serveur LSP (montée sur `/toolchains/<name>-lsp`).
    pub image: String,
    /// Binaire du serveur LSP : chemin absolu dans le pod (point de montage
    /// inclus), non validé ici.
    pub bin: String,
    /// Arguments du serveur. Absent => vec vide.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Toolchain montée dans une sandbox via `volumes[].image`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Toolchain {
    /// Nom du mount /toolchains/<name>. "rust" et "node" ont des presets d'env.
    pub name: String,
    /// Image OCI de la toolchain.
    pub image: String,
    /// Env à injecter, template `{root}` = point de montage. Vide => preset.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Spec LSP de la toolchain. None => preset par `name` (rust/node), ou pas de
    /// LSP si le nom est inconnu (repli dégradé : pas de route /ws/lsp montée pour
    /// cette toolchain).
    #[serde(default)]
    pub lsp: Option<LspSpec>,
}

/// Status de la Sandbox, posé par son reconciler.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStatus {
    /// Provisioning | Running | Failed | Suspended — chaîne libre, pas d'enum,
    /// pas de validation.
    pub phase: Option<String>,
    /// Nom du `Service` K8s exposant la sandbox (`sandbox-<name>`).
    pub service: Option<String>,
    /// Conditions K8s standard. Absent => vec vide.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Spec de l'`Application` (instance déployée d'`app`) : les cinq refs
/// secrets/Ingress (`oidcSecretRef`, `databaseSecretRef`, `host`,
/// `ingressClassName`, `tlsIssuerName`) sont requises ; le dérivé
/// `redirectUrl` et les replis d'image sont du ressort du reconciler
/// `controller/src/application.rs`.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "vanyline.solidite.fr",
    version = "v1alpha1",
    kind = "Application",
    namespaced,
    status = "ApplicationStatus",
    printcolumn = r#"{"name":"Host","type":"string","jsonPath":".spec.host"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.phase"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationSpec {
    /// Image du Deployment `app`. None => défaut du controller (constante, cf.
    /// controller/src/application.rs). Même convention de repli que `SANDBOX_IMAGE`
    /// côté sandbox.rs.
    pub image: Option<String>,
    /// Réplicas du Deployment `app`. None => défaut du controller.
    pub replicas: Option<i32>,
    /// Secret contenant issuerUrl/clientId/clientSecret/scopes (+ caCert optionnel).
    /// PAS redirectUrl — dérivé de `host` au reconcile, jamais dupliqué dans le
    /// secret (source unique de vérité, évite un désync silencieux si `host` change).
    pub oidc_secret_ref: String,
    /// Secret contenant `uri` (chaîne de connexion complète) — clé produite
    /// nativement par le secret `<cluster>-app` de CNPG, pointable directement
    /// sans arrimage manuel.
    pub database_secret_ref: String,
    /// Secret contenant le cookie secret (clé `cookieSecret`). None => généré et
    /// stocké par le reconciler lui-même (`<application-name>-cookie`).
    pub cookie_secret_ref: Option<String>,
    /// Nom de domaine public de l'Ingress. Sert aussi à dériver
    /// `OIDC_REDIRECT_URL` (`https://{host}/auth/callback`).
    pub host: String,
    /// `IngressClass` utilisé pour l'Ingress de l'application.
    pub ingress_class_name: String,
    /// Nom du `CertManager` (Cluster)Issuer utilisé pour l'Ingress de l'application
    /// (`cert-manager.io/cluster-issuer` ou `cert-manager.io/issuer` selon
    /// `tls_issuer_kind`). Pose aussi `spec.tls` sur l'Ingress (secret
    /// `<application-name>-cert`, même convention que les autres Ingress du
    /// cluster). On part du principe que cert-manager est présent.
    pub tls_issuer_name: String,
    /// `ClusterIssuer` (défaut, None) ou `Issuer` (namespaced).
    pub tls_issuer_kind: Option<String>,
    /// Annotations libres posées sur l'Ingress, en plus de l'annotation
    /// cert-manager dérivée de `tls_issuer_name`/`tls_issuer_kind` — même
    /// esprit que `Toolchain.env`, pas de champ dédié par convention connue.
    #[serde(default)]
    pub ingress_annotations: BTreeMap<String, String>,
    /// Pods du controller d'Ingress (ex. traefik) : namespace + labels.
    /// Utilisé comme peer `NetworkPolicy` sur la netpol ingress de chaque
    /// Sandbox (le trafic navigateur transite par l'Ingress avant d'atteindre
    /// le Service). None => pas de peer dédié (la netpol sandbox ne laisse
    /// passer que les pods du même Owner + le pod app).
    pub ingress_controller: Option<IngressControllerRef>,
    /// Défauts de stockage (storageClass + accessMode) posés en env sur le
    /// Deployment `app` — utilisés par `app` quand il crée lazily un Owner ou
    /// un Project pour un utilisateur (jamais lus par le reconciler
    /// Application lui-même). None => `app` ne pose aucun défaut, les
    /// reconcilers Owner/Project retombent sur leurs valeurs historiques
    /// (RWX pour le home, RWO pour le workspace projet).
    pub storage_defaults: Option<ApplicationStorageDefaults>,
    /// Moteur de workflow comme sous-ressource de l'Application (décision du
    /// 2026-10-04). L'absence de `restate` EST le contrat « Application sans
    /// moteur de workflow » ; sa présence commande les sous-ressources Restate
    /// du reconciler (`controller/src/application.rs`, contrat comportemental :
    /// `controller/src/application.sdd`).
    pub restate: Option<RestateSpec>,
}

/// Défauts de stockage propagés par `app` lors de ses créations lazily
/// d'Owner/Project — permet à l'outillage de déploiement de fournir la
/// StorageClass/accessMode adaptée au cluster cible (ex. RWO + local-path
/// sur un cluster single-node, RWX + `CephFS` ailleurs) sans toucher au code.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStorageDefaults {
    /// `StorageClass` du PVC home des Owners, propagée en env à `app`.
    pub home_storage_class: Option<String>,
    /// Mode d'accès du PVC home des Owners, propagé en env à `app`.
    pub home_access_mode: Option<String>,
    /// `StorageClass` des PVC workspace de Projects, propagée en env à `app`.
    pub project_storage_class: Option<String>,
    /// Mode d'accès des PVC workspace de Projects, propagé en env à `app`.
    pub project_access_mode: Option<String>,
}

/// Spec du moteur de workflow Restate déployé comme sous-ressource de
/// l'Application (décision du 2026-10-04). Aucun des trois replis n'est
/// appliqué par ce crate, qui ne transporte que des `Option` : `image` `None`
/// ⇒ défaut du controller (flag `--restate-image` / env `RESTATE_IMAGE`),
/// `storageSize` `None` ⇒ `"5Gi"`, `storageClass` `None` ⇒ `StorageClass` par
/// défaut du cluster — tous trois appliqués par
/// `controller/src/application.rs`. `storageSize` est une chaîne libre à la
/// `Quantity` K8s (même convention `homeSize` : c'est l'apiserver qui rejette
/// une quantité invalide à l'usage, aucune validation ici).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestateSpec {
    /// Image du `StatefulSet` Restate. None ⇒ défaut du controller (flag
    /// `--restate-image` / env `RESTATE_IMAGE`).
    pub image: Option<String>,
    /// Taille du `volumeClaimTemplate` de données. None ⇒ `"5Gi"` (repli
    /// appliqué par le controller, jamais ici).
    pub storage_size: Option<String>,
    /// `StorageClass` du `volumeClaimTemplate`. None ⇒ classe par défaut du
    /// cluster.
    pub storage_class: Option<String>,
}

/// Status de l'Application, posé par son reconciler.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStatus {
    /// Provisioning | Running | Failed — chaîne libre, mêmes valeurs
    /// documentées que `SandboxStatus::phase`, pas de validation.
    pub phase: Option<String>,
    /// Conditions K8s standard. Absent => vec vide.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Returns the four CRD manifests as YAML, separated by `---\n`.
///
/// # Panics
///
/// En théorie si `serde_yaml` échouait sur un schéma connu à la compilation —
/// jugé impossible en pratique (justification de l'`#[allow]` ci-dessous, seul
/// point de panique du code de production du crate).
#[allow(clippy::unwrap_used)]
// serialisation YAML d un schema Rust connu a la compilation, sans entree externe : ne peut pas echouer en pratique
#[must_use]
pub fn crd_manifests() -> String {
    let docs = [
        serde_yaml::to_string(&Owner::crd()).unwrap(),
        serde_yaml::to_string(&Project::crd()).unwrap(),
        serde_yaml::to_string(&Sandbox::crd()).unwrap(),
        serde_yaml::to_string(&Application::crd()).unwrap(),
    ];
    docs.join("---\n")
}

/// Port MCP exposé par `vanyline-sandbox` (`MCP_LISTEN` par défaut
/// `0.0.0.0:3000` côté binaire — cf. `sandbox/src/config.rs`).
pub const MCP_PORT: i32 = 3000;

/// Port d'exploitation `vanyline-sandbox` (`OPS_LISTEN` par défaut
/// `0.0.0.0:9090` côté binaire — cf. `sandbox/src/config.sdd`) : `/health` et
/// `/metrics` sans authentification. Jamais publié (ni Service, ni Ingress) ;
/// seule la `NetworkPolicy` ingress de la Sandbox l'ouvre au cluster.
pub const OPS_PORT: i32 = 9090;

/// Nom du Service K8s exposeant le port MCP d'une sandbox.
#[must_use]
pub fn service_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}")
}

/// Nom du namespace d'un Owner — `"{app_namespace}-{owner}"`. Source unique du
/// nommage UN OWNER = UN NAMESPACE (décision 2026-10-04), partagée par le
/// controller et `app` (qui calcule le namespace d'un Owner sans interroger le
/// cluster) : elles ne doivent jamais diverger.
///
/// Le résultat n'est accepté que s'il est un label DNS-1123 valide (minuscules,
/// chiffres, `-`, commence et finit par un alphanumérique) d'au plus 63
/// caractères.
///
/// # Errors
///
/// Chaîne décrivant la raison du rejet : longueur supérieure à 63 caractères,
/// ou contenu hors label DNS-1123.
pub fn owner_namespace_name(app_namespace: &str, owner: &str) -> Result<String, String> {
    let name = format!("{app_namespace}-{owner}");
    if name.len() > 63 {
        return Err(format!(
            "namespace d'Owner «{name}» invalide : {} caracteres > 63",
            name.len()
        ));
    }
    let valid = name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.ends_with(|c: char| c.is_ascii_alphanumeric());
    if !valid {
        return Err(format!(
            "namespace d'Owner «{name}» invalide : un label DNS-1123 n'admet que des \
             minuscules, des chiffres et des `-`, et doit commencer et finir par un alphanumérique"
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
    use serde_yaml::Value;

    #[test]
    fn crd_names() {
        assert_eq!(
            Owner::crd().metadata.name.as_deref(),
            Some("owners.vanyline.solidite.fr")
        );
        assert_eq!(
            Project::crd().metadata.name.as_deref(),
            Some("projects.vanyline.solidite.fr")
        );
        assert_eq!(
            Sandbox::crd().metadata.name.as_deref(),
            Some("sandboxes.vanyline.solidite.fr")
        );
        assert_eq!(
            Application::crd().metadata.name.as_deref(),
            Some("applications.vanyline.solidite.fr")
        );
    }

    #[test]
    fn crd_manifests_yaml() {
        let m = crd_manifests();
        let parts: Vec<_> = m.split("---").filter(|s| !s.trim().is_empty()).collect();
        assert_eq!(parts.len(), 4);
        for part in parts {
            serde_yaml::from_str::<Value>(part).expect("each CRD section must be valid YAML");
        }
        let count = m.matches("kind: CustomResourceDefinition").count();
        assert_eq!(count, 4);
        assert!(m.contains("vanyline.solidite.fr"));
    }

    #[test]
    fn spec_camel_case() {
        let spec = ProjectSpec {
            owner: "test".to_string(),
            repo_url: "https://github.com/test/repo".to_string(),
            default_branch: None,
            storage_size: None,
            storage_class: None,
            storage_access_mode: None,
            git_secret: None,
            caches: None,
            fetch_interval: None,
            egress: Vec::new(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            json.contains(r#""repoUrl""#),
            "should contain repoUrl (camelCase), got: {json}"
        );
        assert!(
            !json.contains("repo_url"),
            "should not contain repo_url (snake_case), got: {json}"
        );
    }

    #[test]
    fn sandbox_defaults() {
        let spec: SandboxSpec =
            serde_json::from_str(r#"{"project":"p","branch":"main"}"#).expect("should deserialize");
        assert!(spec.toolchains.is_empty());
        assert!(spec.image.is_none());
        assert!(spec.resources.is_none());
    }

    #[test]
    fn owner_schema_fields() {
        let crd = Owner::crd();
        let oas = &crd.spec.versions[0]
            .schema
            .as_ref()
            .unwrap()
            .open_api_v3_schema
            .as_ref()
            .unwrap();
        // In k8s-openapi 0.28 the top-level schema properties are "spec" and "status"
        let spec_props = &oas.properties.as_ref().unwrap()["spec"]
            .properties
            .as_ref()
            .unwrap();

        assert!(
            !spec_props.contains_key("existingPvc"),
            "Owner schema should NOT contain 'existingPvc' (retrait 2026-10-04), got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("projectDefaults"),
            "Owner schema should contain 'projectDefaults', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("egress"),
            "Owner schema should contain 'egress', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("applicationRef"),
            "Owner schema should contain 'applicationRef', got: {spec_props:?}"
        );
    }

    #[test]
    fn egress_camel_case() {
        let ls = LabelSelector {
            match_labels: Some({
                let mut m = std::collections::BTreeMap::new();
                m.insert("app".to_string(), "myapp".to_string());
                m
            }),
            match_expressions: None,
        };
        let spec = SandboxSpec {
            project: "test".to_string(),
            branch: "main".to_string(),
            toolchains: Vec::new(),
            image: None,
            resources: None,
            egress: vec![EgressRule {
                description: "allow dns".to_string(),
                cidr: None,
                pod_selector: Some(ls.clone()),
                namespace_selector: Some(ls),
                ports: vec![EgressPort {
                    port: 53,
                    protocol: Some("UDP".to_string()),
                }],
            }],
            suspended: false,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            json.contains(r#""podSelector""#),
            "should contain podSelector (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""namespaceSelector""#),
            "should contain namespaceSelector (camelCase), got: {json}"
        );
        assert!(!json.contains("pod_selector"));
        assert!(!json.contains("namespace_selector"));
    }

    #[test]
    fn egress_defaults_to_empty() {
        let spec: SandboxSpec =
            serde_json::from_str(r#"{"project":"p","branch":"main"}"#).expect("should deserialize");
        assert!(spec.egress.is_empty());
    }

    #[test]
    fn suspended_defaults_to_false() {
        let spec: SandboxSpec =
            serde_json::from_str(r#"{"project":"p","branch":"main"}"#).expect("should deserialize");
        assert!(!spec.suspended);
    }

    #[test]
    fn sandbox_schema_fields_suspended() {
        let crd = Sandbox::crd();
        let oas = &crd.spec.versions[0]
            .schema
            .as_ref()
            .unwrap()
            .open_api_v3_schema
            .as_ref()
            .unwrap();
        let spec_props = &oas.properties.as_ref().unwrap()["spec"]
            .properties
            .as_ref()
            .unwrap();

        assert!(
            spec_props.contains_key("suspended"),
            "Sandbox schema should contain 'suspended', got: {spec_props:?}"
        );
    }

    #[test]
    fn application_schema_fields() {
        let crd = Application::crd();
        let oas = &crd.spec.versions[0]
            .schema
            .as_ref()
            .unwrap()
            .open_api_v3_schema
            .as_ref()
            .unwrap();
        let spec_props = &oas.properties.as_ref().unwrap()["spec"]
            .properties
            .as_ref()
            .unwrap();

        assert!(
            spec_props.contains_key("host"),
            "Application schema should contain 'host', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("ingressClassName"),
            "Application schema should contain 'ingressClassName', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("oidcSecretRef"),
            "Application schema should contain 'oidcSecretRef', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("databaseSecretRef"),
            "Application schema should contain 'databaseSecretRef', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("cookieSecretRef"),
            "Application schema should contain 'cookieSecretRef', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("ingressAnnotations"),
            "Application schema should contain 'ingressAnnotations', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("tlsIssuerName"),
            "Application schema should contain 'tlsIssuerName', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("tlsIssuerKind"),
            "Application schema should contain 'tlsIssuerKind', got: {spec_props:?}"
        );
        assert!(
            spec_props.contains_key("ingressController"),
            "Application schema should contain 'ingressController', got: {spec_props:?}"
        );
        // S6c : `restate` doit être une propriété du schéma (extension du test,
        // Scenario `application_schema_fields`).
        assert!(
            spec_props.contains_key("restate"),
            "Application schema should contain 'restate', got: {spec_props:?}"
        );
    }

    #[test]
    fn application_defaults() {
        let spec: ApplicationSpec =
            serde_json::from_str(r#"{"host":"app.example.com","ingressClassName":"nginx","oidcSecretRef":"oidc","databaseSecretRef":"db","tlsIssuerName":"self-sign"}"#).expect("should deserialize");
        assert!(spec.image.is_none());
        assert!(spec.replicas.is_none());
        assert!(spec.cookie_secret_ref.is_none());
        assert!(spec.ingress_annotations.is_empty());
        assert!(spec.ingress_controller.is_none());
        assert!(spec.tls_issuer_kind.is_none());
        assert!(spec.storage_defaults.is_none());
        // S6c : l'absence de `restate` EST le contrat « Application sans moteur
        // de workflow » (extension du test, Scenario `application_defaults`).
        assert!(spec.restate.is_none());
    }

    #[test]
    fn application_camel_case() {
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "cert-manager.io/cluster-issuer".to_string(),
            "letsencrypt".to_string(),
        );
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("app.kubernetes.io/name".to_string(), "traefik".to_string());
        pod_labels.insert(
            "app.kubernetes.io/component".to_string(),
            "controller".to_string(),
        );
        let spec = ApplicationSpec {
            image: Some("vanyline-app:latest".to_string()),
            replicas: Some(3),
            oidc_secret_ref: "my-oidc".to_string(),
            database_secret_ref: "my-db".to_string(),
            cookie_secret_ref: Some("my-cookie".to_string()),
            host: "app.example.com".to_string(),
            ingress_class_name: "nginx".to_string(),
            tls_issuer_name: "letsencrypt".to_string(),
            tls_issuer_kind: Some("Issuer".to_string()),
            ingress_annotations: annotations,
            ingress_controller: Some(IngressControllerRef {
                namespace: "kydah-core".to_string(),
                pod_labels,
            }),
            storage_defaults: Some(ApplicationStorageDefaults {
                home_storage_class: Some("local-path".to_string()),
                home_access_mode: Some("ReadWriteOnce".to_string()),
                project_storage_class: None,
                project_access_mode: None,
            }),
            restate: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        for key in [
            "oidcSecretRef",
            "databaseSecretRef",
            "cookieSecretRef",
            "ingressClassName",
            "tlsIssuerName",
            "tlsIssuerKind",
            "ingressAnnotations",
            "ingressController",
            "podLabels",
            "storageDefaults",
            "homeStorageClass",
            "homeAccessMode",
        ] {
            assert!(
                json.contains(key),
                "should contain {key} (camelCase), got: {json}"
            );
        }
        for key in [
            "oidc_secret_ref",
            "database_secret_ref",
            "cookie_secret_ref",
            "ingress_class_name",
            "ingress_annotations",
            "ingress_controller",
            "pod_labels",
        ] {
            assert!(
                !json.contains(key),
                "should not contain {key} (snake_case), got: {json}"
            );
        }
    }

    #[test]
    fn owner_application_ref_camel_case() {
        let spec = OwnerSpec {
            home_size: None,
            home_storage_class: None,
            home_access_mode: None,
            project_defaults: None,
            application_ref: Some("my-app".to_string()),
            egress: Vec::new(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            json.contains(r#""applicationRef""#),
            "should contain applicationRef (camelCase), got: {json}"
        );
        assert!(
            !json.contains("application_ref"),
            "should not contain application_ref (snake_case), got: {json}"
        );
    }

    #[test]
    fn ingress_controller_ref_camel_case() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("app".to_string(), "traefik".to_string());
        let ref_ = IngressControllerRef {
            namespace: "kydah-core".to_string(),
            pod_labels,
        };
        let json = serde_json::to_string(&ref_).unwrap();
        assert!(
            json.contains(r#""namespace""#),
            "should contain namespace, got: {json}"
        );
        assert!(
            json.contains(r#""podLabels""#),
            "should contain podLabels (camelCase), got: {json}"
        );
        assert!(
            !json.contains("pod_labels"),
            "should not contain pod_labels (snake_case), got: {json}"
        );
    }

    // 16. project_status_languages_default_empty
    #[test]
    fn project_status_languages_default_empty() {
        let status: ProjectStatus =
            serde_json::from_str("{}").expect("should deserialize from empty object");
        assert_eq!(
            status.languages,
            Vec::<String>::new(),
            "languages vide depuis {{}}"
        );
        assert!(status.detected_at.is_none());
    }

    // 17. project_status_languages_skipped_when_empty
    #[test]
    fn project_status_languages_skipped_when_empty() {
        let status = ProjectStatus::default();
        let json = serde_json::to_string(&status).unwrap();
        assert!(
            !json.contains("languages"),
            "should not contain 'languages' key when empty, got: {json}"
        );
        assert!(
            !json.contains("detectedAt"),
            "should not contain 'detectedAt' key when none, got: {json}"
        );
    }

    // 18. project_status_languages_present_when_set
    #[test]
    fn project_status_languages_present_when_set() {
        let mut status = ProjectStatus {
            pvc_name: Some("workspace".to_string()),
            cloned: true,
            last_fetch: None,
            worktrees: Vec::new(),
            conditions: Vec::new(),
            languages: vec!["rust".to_string(), "js-ts".to_string()],
            detected_at: Some(Time(k8s_openapi::jiff::Timestamp::now())),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(
            json.contains(r#""languages""#),
            "should contain 'languages' key when set, got: {json}"
        );
        assert!(
            json.contains(r#""rust""#),
            "should contain 'rust' in languages, got: {json}"
        );
        assert!(
            json.contains(r#""js-ts""#),
            "should contain 'js-ts' in languages, got: {json}"
        );
        assert!(
            json.contains("detectedAt"),
            "should contain 'detectedAt' key when set, got: {json}"
        );

        // Now test with empty languages and detected_at = None — should be omitted again
        status.languages = Vec::new();
        status.detected_at = None;
        let json2 = serde_json::to_string(&status).unwrap();
        assert!(
            !json2.contains("languages"),
            "should not contain 'languages' when cleared, got: {json2}"
        );
        assert!(
            !json2.contains("detectedAt"),
            "should not contain 'detectedAt' when cleared, got: {json2}"
        );
    }

    // Scenario: existing_pvc_n_existe_plus
    // Retire (decision 2026-10-04, un Owner = un namespace) : la cle existingPvc est
    // inconnue de OwnerSpec et ProjectSpec — serde l'ignore et elle n'est jamais re-emise.
    #[test]
    fn existing_pvc_n_existe_plus() {
        let spec: OwnerSpec =
            serde_json::from_str(r#"{"existingPvc":"home-pvc"}"#).expect("should deserialize");
        assert!(spec.home_size.is_none());
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            !json.contains("existingPvc"),
            "existingPvc doit etre une cle inconnue de OwnerSpec, got: {json}"
        );
        let spec: ProjectSpec = serde_json::from_str(
            r#"{"owner":"o","repoUrl":"r","existingPvc":{"name":"pvc","subPath":"sub"}}"#,
        )
        .expect("should deserialize");
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            !json.contains("existingPvc"),
            "existingPvc doit etre une cle inconnue de ProjectSpec, got: {json}"
        );
    }

    // 19. lsp_spec_absent_defaults_to_none
    #[test]
    fn lsp_spec_absent_defaults_to_none() {
        let tc: Toolchain =
            serde_json::from_str(r#"{"name":"rust","image":"x"}"#).expect("should deserialize");
        assert!(tc.lsp.is_none());
        assert!(tc.env.is_empty());
    }

    // Scenario: crd_manifests_ordre_documents
    #[test]
    fn crd_manifests_ordre_documents() {
        let m = crd_manifests();
        let mut prev = 0usize;
        for kind in [
            "kind: Owner",
            "kind: Project",
            "kind: Sandbox",
            "kind: Application",
        ] {
            let at = m.find(kind).unwrap();
            assert!(
                at > prev,
                "{kind} doit apparaitre apres le precedent (ordre fige Owner, Project, Sandbox, Application)"
            );
            prev = at;
        }
    }

    // Scenario: crd_scope_namespaced_version_v1alpha1 (+ declaration du sous-ressource status, cf. Must)
    #[test]
    fn crd_scope_namespaced_version_v1alpha1() {
        for crd in [
            Owner::crd(),
            Project::crd(),
            Sandbox::crd(),
            Application::crd(),
        ] {
            assert_eq!(crd.spec.scope, "Namespaced");
            assert_eq!(crd.spec.group, "vanyline.solidite.fr");
            assert_eq!(crd.spec.versions.len(), 1);
            assert_eq!(crd.spec.versions[0].name, "v1alpha1");
            assert!(
                crd.spec.versions[0]
                    .subresources
                    .as_ref()
                    .is_some_and(|s| s.status.is_some()),
                "la version doit declarer le sous-ressource status"
            );
        }
    }

    // Scenario: crd_printcolumns_et_absence_shortnames
    #[test]
    fn crd_printcolumns_et_absence_shortnames() {
        fn columns(crd: &CustomResourceDefinition) -> Vec<(String, String, String)> {
            crd.spec.versions[0]
                .additional_printer_columns
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.name, c.type_, c.json_path))
                .collect()
        }
        fn assert_no_shortnames(crd: &CustomResourceDefinition) {
            assert_eq!(
                crd.spec.names.short_names.clone().unwrap_or_default(),
                Vec::<String>::new()
            );
        }
        let owner = Owner::crd();
        assert_eq!(
            columns(&owner),
            vec![
                (
                    "SA".to_string(),
                    "string".to_string(),
                    ".status.serviceAccount".to_string()
                ),
                (
                    "PVC".to_string(),
                    "string".to_string(),
                    ".status.pvcName".to_string()
                ),
            ]
        );
        assert_no_shortnames(&owner);
        let project = Project::crd();
        assert_eq!(
            columns(&project),
            vec![
                (
                    "Owner".to_string(),
                    "string".to_string(),
                    ".spec.owner".to_string()
                ),
                (
                    "Repo".to_string(),
                    "string".to_string(),
                    ".spec.repoUrl".to_string()
                ),
                (
                    "Cloned".to_string(),
                    "boolean".to_string(),
                    ".status.cloned".to_string()
                ),
            ]
        );
        assert_no_shortnames(&project);
        let sandbox = Sandbox::crd();
        assert_eq!(
            columns(&sandbox),
            vec![
                (
                    "Project".to_string(),
                    "string".to_string(),
                    ".spec.project".to_string()
                ),
                (
                    "Branch".to_string(),
                    "string".to_string(),
                    ".spec.branch".to_string()
                ),
                (
                    "Phase".to_string(),
                    "string".to_string(),
                    ".status.phase".to_string()
                ),
            ]
        );
        assert_no_shortnames(&sandbox);
        let app = Application::crd();
        assert_eq!(
            columns(&app),
            vec![
                (
                    "Host".to_string(),
                    "string".to_string(),
                    ".spec.host".to_string()
                ),
                (
                    "Ready".to_string(),
                    "string".to_string(),
                    ".status.phase".to_string()
                ),
            ]
        );
        assert_no_shortnames(&app);
    }

    // Scenario: owner_spec_deserialise_depuis_objet_vide
    #[test]
    fn owner_spec_deserialise_depuis_objet_vide() {
        let spec: OwnerSpec = serde_json::from_str("{}").expect("un Owner sans spec est valide");
        assert!(spec.home_size.is_none());
        assert!(spec.home_storage_class.is_none());
        assert!(spec.home_access_mode.is_none());
        assert!(spec.project_defaults.is_none());
        assert!(spec.application_ref.is_none());
        assert!(spec.egress.is_empty());
    }

    // Scenario: project_spec_champs_requis
    #[test]
    fn project_spec_champs_requis() {
        let without_owner: Result<ProjectSpec, _> =
            serde_json::from_str(r#"{"repoUrl":"https://x/repo"}"#);
        assert!(without_owner.is_err(), "owner est requis");
        let without_repo: Result<ProjectSpec, _> = serde_json::from_str(r#"{"owner":"o"}"#);
        assert!(without_repo.is_err(), "repoUrl est requis");
    }

    // Scenario: sandbox_spec_champs_requis
    #[test]
    fn sandbox_spec_champs_requis() {
        let without_project: Result<SandboxSpec, _> = serde_json::from_str(r#"{"branch":"main"}"#);
        assert!(without_project.is_err(), "project est requis");
        let without_branch: Result<SandboxSpec, _> = serde_json::from_str(r#"{"project":"p"}"#);
        assert!(without_branch.is_err(), "branch est requis");
    }

    // Scenario: application_spec_champs_requis
    #[test]
    fn application_spec_champs_requis() {
        let docs = [
            r#"{"ingressClassName":"nginx","oidcSecretRef":"oidc","databaseSecretRef":"db","tlsIssuerName":"self-sign"}"#,
            r#"{"host":"app.example.com","oidcSecretRef":"oidc","databaseSecretRef":"db","tlsIssuerName":"self-sign"}"#,
            r#"{"host":"app.example.com","ingressClassName":"nginx","databaseSecretRef":"db","tlsIssuerName":"self-sign"}"#,
            r#"{"host":"app.example.com","ingressClassName":"nginx","oidcSecretRef":"oidc","tlsIssuerName":"self-sign"}"#,
            r#"{"host":"app.example.com","ingressClassName":"nginx","oidcSecretRef":"oidc","databaseSecretRef":"db"}"#,
        ];
        for doc in docs {
            let r: Result<ApplicationSpec, _> = serde_json::from_str(doc);
            assert!(r.is_err(), "une cle requise amputee doit echouer : {doc}");
        }
    }

    // Scenario: project_status_autres_champs_optionnels_en_null
    #[test]
    fn project_status_autres_champs_optionnels_en_null() {
        let json = serde_json::to_string(&ProjectStatus::default()).unwrap();
        for key in [
            r#""pvcName":null"#,
            r#""lastFetch":null"#,
            r#""cloned":false"#,
            r#""worktrees":[]"#,
            r#""conditions":[]"#,
        ] {
            assert!(
                json.contains(key),
                "le ProjectStatus par defaut doit emettre {key} (verrou anti-reression merge-patch), got: {json}"
            );
        }
    }

    // Scenario: lsp_spec_complet_roundtrip
    #[test]
    fn lsp_spec_complet_roundtrip() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "{root}/bin".to_string());
        let tc = Toolchain {
            name: "rust".to_string(),
            image: "img".to_string(),
            env,
            lsp: Some(LspSpec {
                image: "img".to_string(),
                bin: "/u/b".to_string(),
                args: Vec::new(),
            }),
        };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(
            json.contains(r#""args":[]"#),
            "lsp.args vide doit etre serialise [], got: {json}"
        );
        assert!(
            json.contains("{root}"),
            "le template entre accolades est transporte verbatim, got: {json}"
        );
        let back: Toolchain = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(
            json, json2,
            "env/lsp.image/lsp.bin/lsp.args survivent au roundtrip"
        );
    }

    // Scenario: egress_rule_minimale_defauts
    #[test]
    fn egress_rule_minimale_defauts() {
        let rule: EgressRule = serde_json::from_str(r#"{"description":"allow dns"}"#)
            .expect("une regle minimale doit se deserialiser");
        assert!(rule.cidr.is_none());
        assert!(rule.pod_selector.is_none());
        assert!(rule.namespace_selector.is_none());
        assert!(rule.ports.is_empty());
    }

    // Scenario: service_name_prefixe_sandbox
    #[test]
    fn service_name_prefixe_sandbox() {
        assert_eq!(service_name("web"), "sandbox-web");
        assert_eq!(service_name("db-2"), "sandbox-db-2");
        assert_eq!(service_name(""), "sandbox-");
    }

    // Scenario: mcp_port_vaut_3000
    #[test]
    fn mcp_port_vaut_3000() {
        assert_eq!(MCP_PORT, 3000);
    }

    // Scenario: ops_port_vaut_9090
    #[test]
    fn ops_port_vaut_9090() {
        assert_eq!(OPS_PORT, 9090);
        assert_ne!(OPS_PORT, MCP_PORT);
    }

    // Scenario: status_structures_defaut_deserialisables
    // (+ OwnerStatus.namespace deserialisable et serialise en null explicite, cf. Must/Returns)
    #[test]
    fn status_structures_defaut_deserialisables() {
        let owner: OwnerStatus = serde_json::from_str("{}").expect("OwnerStatus depuis {}");
        assert!(owner.pvc_name.is_none());
        assert!(owner.service_account.is_none());
        assert!(owner.namespace.is_none());
        assert_eq!(
            owner.conditions,
            Vec::<Condition>::new(),
            "conditions vides depuis {{}}"
        );
        let sandbox: SandboxStatus = serde_json::from_str("{}").expect("SandboxStatus depuis {}");
        assert!(sandbox.phase.is_none());
        assert!(sandbox.service.is_none());
        assert_eq!(
            sandbox.conditions,
            Vec::<Condition>::new(),
            "conditions vides depuis {{}}"
        );
        let app: ApplicationStatus =
            serde_json::from_str("{}").expect("ApplicationStatus depuis {}");
        assert!(app.phase.is_none());
        assert_eq!(
            app.conditions,
            Vec::<Condition>::new(),
            "conditions vides depuis {{}}"
        );
        let json = serde_json::to_string(&owner).unwrap();
        assert!(
            json.contains(r#""namespace":null"#),
            "namespace se serialise en null explicite comme ses freres, got: {json}"
        );
    }

    // Contrat Must (pas de Scenario dedie — remonte dans la synthese) :
    // owner_namespace_name = "{app_namespace}-{owner}", label DNS-1123 <= 63 caracteres.
    #[test]
    fn owner_namespace_name_valide() {
        assert_eq!(
            owner_namespace_name("vanyline-app", "alice").unwrap(),
            "vanyline-app-alice"
        );
        assert_eq!(
            owner_namespace_name("media-test", "seb").unwrap(),
            "media-test-seb"
        );
        // 63 caracteres exacts : dans la limite.
        let name = owner_namespace_name(&"a".repeat(31), &"b".repeat(31))
            .expect("63 caracteres est dans la limite");
        assert_eq!(name.len(), 63);
    }

    #[test]
    fn owner_namespace_name_hors_dns_1123() {
        let err = owner_namespace_name("Vanyline-App", "alice").expect_err("majuscules interdites");
        assert!(
            err.contains("DNS-1123"),
            "l'erreur porte la raison, got: {err}"
        );
        assert!(
            owner_namespace_name("app", "Alice").is_err(),
            "majuscule dans owner"
        );
        assert!(
            owner_namespace_name("app", "ali_ce").is_err(),
            "underscore interdit"
        );
        assert!(
            owner_namespace_name("app", "alice.").is_err(),
            "point final interdit"
        );
        assert!(
            owner_namespace_name("-app", "alice").is_err(),
            "tiret en debut interdit"
        );
        assert!(
            owner_namespace_name("app", "alice-").is_err(),
            "tiret en fin interdit"
        );
        assert!(
            owner_namespace_name("", "").is_err(),
            "- seul n'est pas un label"
        );
    }

    #[test]
    fn git_secret_transport_conservé() {
        let json = r#"{"owner":"o","repoUrl":"https://x/y.git","gitSecret":"cle-git","egress":[]}"#;
        let spec: ProjectSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.git_secret.as_deref(), Some("cle-git"));
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["gitSecret"], serde_json::json!("cle-git"));

        let defaut = ProjectSpec {
            owner: "o".to_string(),
            repo_url: "https://x/y.git".to_string(),
            default_branch: None,
            storage_size: None,
            storage_class: None,
            storage_access_mode: None,
            git_secret: None,
            caches: None,
            fetch_interval: None,
            egress: Vec::new(),
        };
        let v = serde_json::to_value(&defaut).unwrap();
        assert_eq!(v["gitSecret"], serde_json::Value::Null);
    }

    #[test]
    fn owner_namespace_name_trop_long() {
        let err =
            owner_namespace_name(&"a".repeat(32), &"b".repeat(31)).expect_err("64 caracteres > 63");
        assert!(err.contains("63"), "l'erreur porte la raison, got: {err}");
    }

    // Scenario: restate_spec_camel_case_roundtrip
    #[test]
    fn restate_spec_camel_case_roundtrip() {
        let spec = ApplicationSpec {
            image: None,
            replicas: None,
            oidc_secret_ref: "oidc".to_string(),
            database_secret_ref: "db".to_string(),
            cookie_secret_ref: None,
            host: "app.example.com".to_string(),
            ingress_class_name: "nginx".to_string(),
            tls_issuer_name: "self-sign".to_string(),
            tls_issuer_kind: None,
            ingress_annotations: BTreeMap::new(),
            ingress_controller: None,
            storage_defaults: None,
            restate: Some(RestateSpec {
                image: Some("img".to_string()),
                storage_size: Some("8Gi".to_string()),
                storage_class: Some("local-path".to_string()),
            }),
        };
        let json = serde_json::to_string(&spec).unwrap();
        for key in ["restate", "storageSize", "storageClass"] {
            assert!(
                json.contains(key),
                "should contain {key} (camelCase), got: {json}"
            );
        }
        assert!(
            !json.contains("storage_size"),
            "should not contain storage_size (snake_case), got: {json}"
        );
        assert!(
            !json.contains("storage_class"),
            "should not contain storage_class (snake_case), got: {json}"
        );
        let back: ApplicationSpec = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(
            json, json2,
            "image/storageSize/storageClass survivent au roundtrip a l'identique"
        );
    }

    // Scenario: restate_spec_champs_tous_optionnels
    #[test]
    fn restate_spec_champs_tous_optionnels() {
        let spec: RestateSpec =
            serde_json::from_str("{}").expect("un RestateSpec vide doit se deserialiser");
        assert!(spec.image.is_none());
        assert!(spec.storage_size.is_none());
        assert!(spec.storage_class.is_none());
        // Aucun skip_serializing_if : le default rend les trois cles a null.
        let json = serde_json::to_string(&RestateSpec::default()).unwrap();
        for key in [
            r#""image":null"#,
            r#""storageSize":null"#,
            r#""storageClass":null"#,
        ] {
            assert!(
                json.contains(key),
                "le RestateSpec par defaut doit emettre {key} (aucun skip_serializing_if), got: {json}"
            );
        }
    }

    // Scenario: restate_schema_proprietes_optionnelles
    #[test]
    fn restate_schema_proprietes_optionnelles() {
        let crd = Application::crd();
        let oas = &crd.spec.versions[0]
            .schema
            .as_ref()
            .unwrap()
            .open_api_v3_schema
            .as_ref()
            .unwrap();
        let spec_props_owner = oas.properties.as_ref().unwrap()["spec"]
            .properties
            .as_ref()
            .unwrap();
        // `restate` absent du `required` de la spec (Option => optionnel).
        let spec_required = oas.properties.as_ref().unwrap()["spec"]
            .required
            .clone()
            .unwrap_or_default();
        assert!(
            !spec_required.contains(&"restate".to_string()),
            "restate ne doit pas etre requis au niveau spec, got: {spec_required:?}"
        );
        // Sous-schema inliné (même forme que `projectDefaults`/`storageDefaults`) :
        // objet aux trois proprietes, aucune requise (toutes Option).
        let restate = &spec_props_owner["restate"];
        assert_eq!(restate.type_.as_deref(), Some("object"));
        let props = restate
            .properties
            .as_ref()
            .expect("restate doit exposer des proprietes");
        for key in ["image", "storageSize", "storageClass"] {
            assert!(
                props.contains_key(key),
                "restate doit exposer la propriete {key}, got: {props:?}"
            );
        }
        assert!(
            restate.required.clone().unwrap_or_default().is_empty(),
            "aucune sous-proprieté de restate ne doit etre requise (toutes Option), got: {:?}",
            restate.required
        );
    }
}
