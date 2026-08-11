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
    pub project_defaults: Option<ProjectDefaults>,
    #[serde(default)]
    pub egress: Vec<EgressRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDefaults {
    pub storage_size: Option<String>,
    pub storage_class: Option<String>,
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
    /// Auth git dédiée (Secret). Défaut: ~/.ssh du home Owner.
    pub git_secret: Option<String>,
    /// Caches partagés. None => ["cargo", "pnpm"].
    pub caches: Option<Vec<String>>,
    /// Intervalle du CronJob de fetch. None => "1h".
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
    /// Service, NetworkPolicies conservés), status.phase devient
    /// "Suspended". false => le Pod est recréé (chemin nominal).
    #[serde(default)]
    pub suspended: bool,
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
    /// Secret contenant `databaseUrl` (chaîne de connexion complète).
    pub database_secret_ref: String,
    /// Secret contenant le cookie secret (clé `cookieSecret`). None => généré et
    /// stocké par le reconciler lui-même (`<application-name>-cookie`).
    pub cookie_secret_ref: Option<String>,
    /// Nom de domaine public de l'Ingress. Sert aussi à dériver
    /// `OIDC_REDIRECT_URL` (`https://{host}/auth/callback`).
    pub host: String,
    pub ingress_class_name: String,
    /// Annotations libres posées sur l'Ingress (ex. cert-manager.io/cluster-issuer) —
    /// même esprit que `Toolchain.env`, pas de champ dédié par convention connue.
    #[serde(default)]
    pub ingress_annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStatus {
    pub phase: Option<String>, // Provisioning | Running | Failed
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Returns the four CRD manifests as YAML, separated by `---\n`.
#[allow(clippy::unwrap_used)] // serialisation YAML d un schema Rust connu a la compilation, sans entree externe : ne peut pas echouer en pratique
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
    }

    #[test]
    fn application_defaults() {
        let spec: ApplicationSpec =
            serde_json::from_str(r#"{"host":"app.example.com","ingressClassName":"nginx","oidcSecretRef":"oidc","databaseSecretRef":"db"}"#).expect("should deserialize");
        assert!(spec.image.is_none());
        assert!(spec.replicas.is_none());
        assert!(spec.cookie_secret_ref.is_none());
        assert!(spec.ingress_annotations.is_empty());
    }

    #[test]
    fn application_camel_case() {
        let mut annotations = BTreeMap::new();
        annotations.insert("cert-manager.io/cluster-issuer".to_string(), "letsencrypt".to_string());
        let spec = ApplicationSpec {
            image: Some("vanyline-app:latest".to_string()),
            replicas: Some(3),
            oidc_secret_ref: "my-oidc".to_string(),
            database_secret_ref: "my-db".to_string(),
            cookie_secret_ref: Some("my-cookie".to_string()),
            host: "app.example.com".to_string(),
            ingress_class_name: "nginx".to_string(),
            ingress_annotations: annotations,
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
            json.contains(r#""ingressAnnotations""#),
            "should contain ingressAnnotations (camelCase), got: {json}"
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
    }
}
