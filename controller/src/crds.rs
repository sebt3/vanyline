use std::collections::BTreeMap;

use kube::{CustomResource, CustomResourceExt};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
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
    pub home_size: Option<String>,            // défaut appliqué au reconcile: "1Gi"
    pub home_storage_class: Option<String>,   // RWX recommandé (CephFS)
    pub project_defaults: Option<ProjectDefaults>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDefaults {
    pub storage_size: Option<String>,
    pub storage_class: Option<String>,
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
    pub phase: Option<String>,      // Provisioning | Running | Failed
    pub service: Option<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// Returns the three CRD manifests as YAML, separated by `---\n`.
pub fn crd_manifests() -> String {
    let docs = [
        serde_yaml::to_string(&Owner::crd()).unwrap(),
        serde_yaml::to_string(&Project::crd()).unwrap(),
        serde_yaml::to_string(&Sandbox::crd()).unwrap(),
    ];
    docs.join("---\n")
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn crd_manifests_yaml() {
        let m = crd_manifests();
        let parts: Vec<_> = m.split("---").filter(|s| !s.trim().is_empty()).collect();
        assert_eq!(parts.len(), 3);
        for part in parts {
            serde_yaml::from_str::<Value>(part).expect("each CRD section must be valid YAML");
        }
        let count = m.matches("kind: CustomResourceDefinition").count();
        assert_eq!(count, 3);
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
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""repoUrl""#), "should contain repoUrl (camelCase), got: {json}");
        assert!(!json.contains("repo_url"), "should not contain repo_url (snake_case), got: {json}");
    }

    #[test]
    fn sandbox_defaults() {
        let spec: SandboxSpec = serde_json::from_str(
            r#"{"project":"p","branch":"main"}"#,
        )
        .expect("should deserialize");
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
    }
}