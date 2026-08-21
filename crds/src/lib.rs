#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, LabelSelector, Time};
use kube::{CustomResource, CustomResourceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// PVC home existant (ex. celui de code-server). None: créé (`owner-<name>-home`).
    pub existing_pvc: Option<String>,
    pub home_size: Option<String>, // défaut appliqué au reconcile: "1Gi"
    pub home_storage_class: Option<String>, // RWX recommandé (CephFS)
    /// Mode d'accès du PVC home. None => "`ReadWriteMany`" (défaut historique —
    /// le home est partagé entre les sandboxes concurrentes d'un même Owner).
    /// À ajuster (ex. "`ReadWriteOnce`") sur un cluster sans `StorageClass` RWX
    /// (ex. single-node local-path) — aucun partage concurrent alors possible.
    pub home_access_mode: Option<String>,
    pub project_defaults: Option<ProjectDefaults>,
    /// Nom de la CR Application dont l'Ingress sert de base aux sous-domaines
    /// de sandbox (`{sandbox}.sandboxes.{application.host}`). None => la
    /// Sandbox reste ClusterIP-only (comportement actuel, pas d'erreur).
    pub application_ref: Option<String>,
    #[serde(default)]
    pub egress: Vec<EgressRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDefaults {
    pub storage_size: Option<String>,
    pub storage_class: Option<String>,
    /// Défaut de repli pour `ProjectSpec.storage_access_mode` quand ce
    /// dernier est absent. None => "`ReadWriteOnce`" (défaut historique).
    pub storage_access_mode: Option<String>,
}

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
    pub description: String,
    pub cidr: Option<String>,
    pub pod_selector: Option<LabelSelector>,
    pub namespace_selector: Option<LabelSelector>,
    #[serde(default)]
    pub ports: Vec<EgressPort>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EgressPort {
    pub port: i32,
    /// "TCP" | "UDP". `None` => TCP (interprété par `netpol-builder`, pas ici).
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OwnerStatus {
    pub pvc_name: Option<String>,
    pub service_account: Option<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

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
    pub owner: String,
    pub repo_url: String,
    pub default_branch: Option<String>,
    /// Le repo vit dans un PVC existant (cas kydah-code). None: PVC créé (`project-<name>`).
    pub existing_pvc: Option<PvcRef>,
    pub storage_size: Option<String>,
    pub storage_class: Option<String>,
    /// Mode d'accès du PVC workspace. None => repli sur
    /// `Owner.spec.project_defaults.storage_access_mode`, puis
    /// "`ReadWriteOnce`" (défaut historique).
    pub storage_access_mode: Option<String>,
    /// DÉPRÉCIÉ — plus consommé par le controller depuis git-integration
    /// (la clé SSH vit dans le PVC Owner, cf. docs/features/git-integration.md
    /// section 0). Champ conservé pour la migration, pas supprimé.
    pub git_secret: Option<String>,
    /// Caches partagés. None => ["cargo", "pnpm"].
    pub caches: Option<Vec<String>>,
    /// Intervalle du `CronJob` de fetch. None => "1h".
    pub fetch_interval: Option<String>,
    #[serde(default)]
    pub egress: Vec<EgressRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PvcRef {
    pub name: String,
    pub sub_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    pub pvc_name: Option<String>,
    #[serde(default)]
    pub cloned: bool,
    pub last_fetch: Option<Time>,
    #[serde(default)]
    pub worktrees: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Langages détectés ("rust", "js-ts", ordre fixe — cf.
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
    pub project: String,
    pub branch: String,
    #[serde(default)]
    pub toolchains: Vec<Toolchain>,
    /// Image du serveur sandbox. None => défaut du controller (env).
    pub image: Option<String>,
    pub resources: Option<k8s_openapi::api::core::v1::ResourceRequirements>,
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
    pub image: String,
    pub bin: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Toolchain {
    /// Nom du mount /toolchains/<name>. "rust" et "node" ont des presets d'env.
    pub name: String,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStatus {
    pub phase: Option<String>, // Provisioning | Running | Failed
    pub service: Option<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

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
}

/// Défauts de stockage propagés par `app` lors de ses créations lazily
/// d'Owner/Project — permet à l'outillage de déploiement de fournir la
/// StorageClass/accessMode adaptée au cluster cible (ex. RWO + local-path
/// sur un cluster single-node, RWX + `CephFS` ailleurs) sans toucher au code.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStorageDefaults {
    pub home_storage_class: Option<String>,
    pub home_access_mode: Option<String>,
    pub project_storage_class: Option<String>,
    pub project_access_mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStatus {
    pub phase: Option<String>, // Provisioning | Running | Failed
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Returns the four CRD manifests as YAML, separated by `---\n`.
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

/// Port MCP expose par `vanyline-sandbox` (`MCP_LISTEN` par defaut `0.0.0.0:3000
/// cote binaire — cf. `sandbox/src/config.rs`).
pub const MCP_PORT: i32 = 3000;

/// Nom du Service K8s exposeant le port MCP d'une sandbox.
#[must_use]
pub fn service_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
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
            existing_pvc: None,
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
            spec_props.contains_key("existingPvc"),
            "Owner schema should contain 'existingPvc', got: {spec_props:?}"
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
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            json.contains(r#""oidcSecretRef""#),
            "should contain oidcSecretRef (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""databaseSecretRef""#),
            "should contain databaseSecretRef (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""cookieSecretRef""#),
            "should contain cookieSecretRef (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""ingressClassName""#),
            "should contain ingressClassName (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""tlsIssuerName""#),
            "should contain tlsIssuerName (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""tlsIssuerKind""#),
            "should contain tlsIssuerKind (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""ingressAnnotations""#),
            "should contain ingressAnnotations (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""ingressController""#),
            "should contain ingressController (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""podLabels""#),
            "should contain podLabels (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""storageDefaults""#),
            "should contain storageDefaults (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""homeStorageClass""#),
            "should contain homeStorageClass (camelCase), got: {json}"
        );
        assert!(
            json.contains(r#""homeAccessMode""#),
            "should contain homeAccessMode (camelCase), got: {json}"
        );
        assert!(
            !json.contains("oidc_secret_ref"),
            "should not contain oidc_secret_ref (snake_case), got: {json}"
        );
        assert!(
            !json.contains("database_secret_ref"),
            "should not contain database_secret_ref (snake_case), got: {json}"
        );
        assert!(
            !json.contains("cookie_secret_ref"),
            "should not contain cookie_secret_ref (snake_case), got: {json}"
        );
        assert!(
            !json.contains("ingress_class_name"),
            "should not contain ingress_class_name (snake_case), got: {json}"
        );
        assert!(
            !json.contains("ingress_annotations"),
            "should not contain ingress_annotations (snake_case), got: {json}"
        );
        assert!(
            !json.contains("ingress_controller"),
            "should not contain ingress_controller (snake_case), got: {json}"
        );
        assert!(
            !json.contains("pod_labels"),
            "should not contain pod_labels (snake_case), got: {json}"
        );
    }

    #[test]
    fn owner_application_ref_camel_case() {
        let spec = OwnerSpec {
            existing_pvc: None,
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
        assert!(status.languages.is_empty());
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

    // 19. lsp_spec_absent_defaults_to_none
    #[test]
    fn lsp_spec_absent_defaults_to_none() {
        let tc: Toolchain =
            serde_json::from_str(r#"{"name":"rust","image":"x"}"#).expect("should deserialize");
        assert!(tc.lsp.is_none());
        assert!(tc.env.is_empty());
    }
}
