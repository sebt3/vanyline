use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{PersistentVolumeClaim, PersistentVolumeClaimSpec, VolumeResourceRequirements};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::ObjectMeta;
use kube::{Resource, ResourceExt};

use crate::crds::Project;

/// Point de montage du PVC workspace dans les pods (Jobs et, plus tard, Sandbox).
#[allow(dead_code)]
pub const WORKSPACE_MOUNT_PATH: &str = "/workspace";
#[allow(dead_code)]
const DEFAULT_WORKSPACE_SIZE: &str = "10Gi";

/// Nom du PVC workspace créé par le controller — utilisé seulement quand
/// `spec.existing_pvc` est `None`.
#[allow(dead_code)]
pub fn project_pvc_name(project_name: &str) -> String {
    format!("project-{project_name}")
}

/// Nom de PVC exposé au status : `existing_pvc.name` si fourni, sinon le nom généré.
#[allow(dead_code)]
pub fn effective_pvc_name(project: &Project) -> String {
    project
        .spec
        .existing_pvc
        .as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| project_pvc_name(&project.name_any()))
}

/// `subPath` à utiliser lors du montage du PVC — `Some` uniquement quand le repo
/// vit dans un sous-répertoire d'un PVC existant (cas kydah-code). `None` quand le
/// PVC est créé par le controller (le mount utilise tout le volume) ou que
/// `existing_pvc.sub_path` n'est pas renseigné.
#[allow(dead_code)]
pub fn effective_sub_path(project: &Project) -> Option<String> {
    project.spec.existing_pvc.as_ref().and_then(|r| r.sub_path.clone())
}

/// Chemin (relatif à la racine du volume, donc à `WORKSPACE_MOUNT_PATH` une fois
/// monté) du clone bare du remote.
#[allow(dead_code)]
pub fn bare_repo_path() -> &'static str {
    "repo.git"
}

/// Chemin (relatif) du worktree d'une sandbox donnée.
#[allow(dead_code)]
pub fn worktree_path(sandbox_name: &str) -> String {
    format!("worktrees/{sandbox_name}")
}

/// Nom du répertoire de cache pour un identifiant de `ProjectSpec.caches` donné.
/// La plupart des identifiants sont utilisés tels quels comme nom de répertoire ;
/// `"pnpm"` est le seul actuellement mappé (répertoire `pnpm-store`, convention de
/// la toolchain pnpm — cf. design § Layout des volumes).
#[allow(dead_code)]
pub fn cache_dir_name(cache: &str) -> String {
    match cache {
        "pnpm" => "pnpm-store".to_string(),
        other => other.to_string(),
    }
}

/// Chemin (relatif) du répertoire de cache pour un identifiant donné.
#[allow(dead_code)]
pub fn cache_path(cache: &str) -> String {
    format!("cache/{}", cache_dir_name(cache))
}

/// Liste effective des caches à provisionner : `spec.caches` si fourni, sinon
/// `["cargo", "pnpm"]`.
#[allow(dead_code)]
pub fn effective_caches(project: &Project) -> Vec<String> {
    project
        .spec
        .caches
        .clone()
        .unwrap_or_else(|| vec!["cargo".to_string(), "pnpm".to_string()])
}

/// `ownerReference` vers ce Project, pour la GC en cascade du PVC créé.
#[allow(dead_code)]
fn owner_reference(project: &Project) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
    project
        .controller_owner_ref(&())
        .expect("Project a apiVersion/kind renseignés par le derive CustomResource")
}

/// Construit le PVC workspace à créer. Retourne `None` si `existing_pvc` est
/// fourni : un PVC référencé n'est jamais créé ni géré par le controller (mêmes
/// règles que `owner::build_home_pvc`).
#[allow(dead_code)]
///
/// `default_size`/`default_class` sont les valeurs `Owner.spec.project_defaults`
/// résolues par l'appelant (le reconciler Project ira chercher l'Owner — cette
/// fonction reste pure et ne fait aucun appel réseau). Priorité :
/// `spec.storage_size`/`spec.storage_class` > défauts passés en paramètre >
/// `DEFAULT_WORKSPACE_SIZE` ("10Gi") pour la taille ; pas de valeur de repli pour
/// la storage class (`None` => StorageClass par défaut du cluster).
pub fn build_workspace_pvc(
    project: &Project,
    default_size: Option<&str>,
    default_class: Option<&str>,
) -> Option<PersistentVolumeClaim> {
    if project.spec.existing_pvc.is_some() {
        return None;
    }
    let size = project
        .spec
        .storage_size
        .clone()
        .or_else(|| default_size.map(str::to_string))
        .unwrap_or_else(|| DEFAULT_WORKSPACE_SIZE.to_string());
    let class = project
        .spec
        .storage_class
        .clone()
        .or_else(|| default_class.map(str::to_string));

    let mut requests = BTreeMap::new();
    requests.insert("storage".to_string(), Quantity(size));

    Some(PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some(project_pvc_name(&project.name_any())),
            namespace: project.namespace(),
            owner_references: Some(vec![owner_reference(project)]),
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".to_string()]),
            storage_class_name: class,
            resources: Some(VolumeResourceRequirements {
                requests: Some(requests),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    })
}

#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use crate::crds::{PvcRef, ProjectSpec};
    use super::*;

    fn make_project(existing_pvc: Option<PvcRef>, storage_size: Option<String>) -> Project {
        let mut project = Project::new("demo", ProjectSpec {
            owner: "alice".to_string(),
            repo_url: "https://github.com/owner/repo".to_string(),
            default_branch: None,
            existing_pvc,
            storage_size,
            storage_class: None,
            git_secret: None,
            caches: None,
            fetch_interval: None,
        });
        project.meta_mut().namespace = Some("ns".into());
        project.meta_mut().uid = Some("test-uid-demo".into());
        project
    }

    // 1. project_pvc_name_format
    #[test]
    fn project_pvc_name_format() {
        assert_eq!(project_pvc_name("demo"), "project-demo");
    }

    // 2. bare_repo_and_worktree_paths
    #[test]
    fn bare_repo_and_worktree_paths() {
        assert_eq!(bare_repo_path(), "repo.git");
        assert_eq!(worktree_path("sb1"), "worktrees/sb1");
    }

    // 3. cache_dir_name_mapping
    #[test]
    fn cache_dir_name_mapping() {
        assert_eq!(cache_dir_name("cargo"), "cargo");
        assert_eq!(cache_dir_name("pnpm"), "pnpm-store");
        assert_eq!(cache_dir_name("custom"), "custom");
    }

    // 4. cache_path_uses_mapping
    #[test]
    fn cache_path_uses_mapping() {
        assert_eq!(cache_path("pnpm"), "cache/pnpm-store");
        assert_eq!(cache_path("cargo"), "cache/cargo");
    }

    // 5. effective_caches_default
    #[test]
    fn effective_caches_default() {
        let project = make_project(None, None);
        assert_eq!(effective_caches(&project), vec!["cargo".to_string(), "pnpm".to_string()]);
    }

    // 6. effective_caches_custom
    #[test]
    fn effective_caches_custom() {
        let project = make_project(None, None);
        let mut project = project;
        project.spec.caches = Some(vec!["cargo".into()]);
        assert_eq!(effective_caches(&project), vec!["cargo".to_string()]);
    }

    // 7. effective_pvc_name_generated
    #[test]
    fn effective_pvc_name_generated() {
        let project = make_project(None, None);
        assert_eq!(effective_pvc_name(&project), "project-demo");
    }

    // 8. effective_pvc_name_existing
    #[test]
    fn effective_pvc_name_existing() {
        let project = make_project(
            Some(crate::crds::PvcRef {
                name: "code-server-home".into(),
                sub_path: Some("repo".into()),
            }),
            None,
        );
        assert_eq!(effective_pvc_name(&project), "code-server-home");
    }

    // 9. effective_sub_path_cases
    #[test]
    fn effective_sub_path_cases() {
        // existing_pvc: None => None
        let project = make_project(None, None);
        assert_eq!(effective_sub_path(&project), None);

        // existing_pvc with sub_path
        let project = make_project(
            Some(crate::crds::PvcRef {
                name: "x".into(),
                sub_path: Some("repo".into()),
            }),
            None,
        );
        assert_eq!(effective_sub_path(&project), Some("repo".to_string()));

        // existing_pvc without sub_path
        let project = make_project(
            Some(crate::crds::PvcRef {
                name: "x".into(),
                sub_path: None,
            }),
            None,
        );
        assert_eq!(effective_sub_path(&project), None);
    }

    // 10. build_workspace_pvc_none_when_existing
    #[test]
    fn build_workspace_pvc_none_when_existing() {
        let project = make_project(
            Some(crate::crds::PvcRef {
                name: "code-server-home".into(),
                sub_path: None,
            }),
            None,
        );
        assert!(build_workspace_pvc(&project, None, None).is_none());
    }

    // 11. build_workspace_pvc_default_size
    #[test]
    fn build_workspace_pvc_default_size() {
        let project = make_project(None, None);
        let pvc = build_workspace_pvc(&project, None, None).expect("PVC should be built");
        let requests = pvc.spec.as_ref().unwrap().resources.as_ref().unwrap().requests.as_ref().unwrap();
        assert_eq!(
            requests.get("storage").unwrap(),
            &Quantity("10Gi".into())
        );
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &vec!["ReadWriteOnce".to_string()]
        );
    }

    // 12. build_workspace_pvc_owner_default_size
    #[test]
    fn build_workspace_pvc_owner_default_size() {
        let project = make_project(None, None);
        let pvc = build_workspace_pvc(&project, Some("20Gi"), None).expect("PVC should be built");
        let requests = pvc.spec.as_ref().unwrap().resources.as_ref().unwrap().requests.as_ref().unwrap();
        assert_eq!(
            requests.get("storage").unwrap(),
            &Quantity("20Gi".into())
        );
    }

    // 13. build_workspace_pvc_spec_overrides_default
    #[test]
    fn build_workspace_pvc_spec_overrides_default() {
        let project = make_project(None, Some("5Gi".to_string()));
        let pvc = build_workspace_pvc(&project, Some("20Gi"), None).expect("PVC should be built");
        let requests = pvc.spec.as_ref().unwrap().resources.as_ref().unwrap().requests.as_ref().unwrap();
        assert_eq!(
            requests.get("storage").unwrap(),
            &Quantity("5Gi".into())
        );
    }

    // 14. build_workspace_pvc_owner_reference
    #[test]
    fn build_workspace_pvc_owner_reference() {
        let project = make_project(None, None);
        let pvc = build_workspace_pvc(&project, None, None).expect("PVC should be built");
        let refs = pvc.metadata.owner_references.as_ref().expect("owner_references should be present");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name.as_str(), "demo");
        assert_eq!(refs[0].kind.as_str(), "Project");
    }
}
