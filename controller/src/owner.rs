use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::{
    PersistentVolumeClaim, PersistentVolumeClaimSpec, ServiceAccount, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, OwnerReference, Time};
use kube::api::{Api, ObjectMeta, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::{Client, Resource, ResourceExt};

use crate::crds::{Owner, OwnerStatus};
use crate::error::ControllerError;

/// Field manager utilisé pour tous les server-side apply du controller.
pub const FIELD_MANAGER: &str = "vanyline-controller";
const DEFAULT_HOME_SIZE: &str = "1Gi";

/// Point de montage du PVC home (Owner) — sert de `$HOME` partout où il est monté
/// (Jobs git du Project, futur pod Sandbox).
pub const HOME_MOUNT_PATH: &str = "/home/vanyline";

pub struct Context {
    pub client: Client,
}

/// Nom du PVC home créé par le controller — utilisé seulement quand
/// `spec.existing_pvc` est `None`.
pub fn home_pvc_name(owner_name: &str) -> String {
    format!("owner-{owner_name}-home")
}

/// Nom du ServiceAccount de l'Owner — toujours `owner-<name>`, que le PVC home
/// soit créé ou référencé.
pub fn service_account_name(owner_name: &str) -> String {
    format!("owner-{owner_name}")
}

/// Nom de PVC exposé au status : `existing_pvc` si fourni, sinon le nom généré.
pub fn effective_pvc_name(owner: &Owner) -> String {
    owner
        .spec
        .existing_pvc
        .clone()
        .unwrap_or_else(|| home_pvc_name(&owner.name_any()))
}

/// `ownerReference` vers cet Owner, pour la GC en cascade des objets créés
/// par le controller (PVC home si créé, ServiceAccount).
fn owner_reference(owner: &Owner) -> OwnerReference {
    owner
        .controller_owner_ref(&())
        .expect("Owner a apiVersion/kind renseignés par le derive CustomResource")
}

/// Construit le PVC home à créer. Retourne `None` si `existing_pvc` est fourni :
/// un PVC référencé n'est jamais créé ni géré par le controller.
pub fn build_home_pvc(owner: &Owner) -> Option<PersistentVolumeClaim> {
    if owner.spec.existing_pvc.is_some() {
        return None;
    }
    let size = owner
        .spec
        .home_size
        .clone()
        .unwrap_or_else(|| DEFAULT_HOME_SIZE.to_string());
    let mut requests = BTreeMap::new();
    requests.insert("storage".to_string(), Quantity(size));

    Some(PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some(home_pvc_name(&owner.name_any())),
            namespace: owner.namespace(),
            owner_references: Some(vec![owner_reference(owner)]),
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteMany".to_string()]),
            storage_class_name: owner.spec.home_storage_class.clone(),
            resources: Some(VolumeResourceRequirements {
                requests: Some(requests),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// Construit le ServiceAccount de l'Owner — toujours créé par le controller,
/// que le PVC home soit créé ou référencé.
pub fn build_service_account(owner: &Owner) -> ServiceAccount {
    ServiceAccount {
        metadata: ObjectMeta {
            name: Some(service_account_name(&owner.name_any())),
            namespace: owner.namespace(),
            owner_references: Some(vec![owner_reference(owner)]),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Status attendu une fois PVC et ServiceAccount en place : `pvc_name`,
/// `service_account`, condition `Ready: True`.
pub fn compute_status(owner: &Owner) -> OwnerStatus {
    OwnerStatus {
        pvc_name: Some(effective_pvc_name(owner)),
        service_account: Some(service_account_name(&owner.name_any())),
        conditions: vec![Condition {
            type_: "Ready".to_string(),
            status: "True".to_string(),
            reason: "Reconciled".to_string(),
            message: "PVC home et ServiceAccount en place".to_string(),
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: owner.meta().generation,
        }],
    }
}

/// Reconciler kube-rs : garantit le PVC home (si besoin) et le ServiceAccount via
/// server-side apply (idempotent), puis met à jour le status. Pas de finalizer —
/// la GC K8s (ownerReferences) nettoie les objets créés à la suppression de l'Owner.
pub async fn reconcile(owner: Arc<Owner>, ctx: Arc<Context>) -> Result<Action, ControllerError> {
    let ns = owner.namespace().unwrap_or_else(|| "default".to_string());
    let pp = PatchParams::apply(FIELD_MANAGER).force();

    if let Some(pvc) = build_home_pvc(&owner) {
        let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(ctx.client.clone(), &ns);
        pvcs.patch(&home_pvc_name(&owner.name_any()), &pp, &Patch::Apply(&pvc))
            .await?;
    }

    let sas: Api<ServiceAccount> = Api::namespaced(ctx.client.clone(), &ns);
    let sa = build_service_account(&owner);
    sas.patch(&service_account_name(&owner.name_any()), &pp, &Patch::Apply(&sa))
        .await?;

    let owners: Api<Owner> = Api::namespaced(ctx.client.clone(), &ns);
    let status = compute_status(&owner);
    let patch = serde_json::json!({ "status": status });
    owners
        .patch_status(&owner.name_any(), &pp, &Patch::Merge(&patch))
        .await?;

    Ok(Action::requeue(Duration::from_secs(300)))
}

/// Politique d'erreur : requeue à 30s, quelle que soit l'erreur (pas de
/// distinction transitoire/fatale en v1 — le design prévoit un requeue exponentiel
/// ultérieur si nécessaire).
pub fn error_policy(_owner: Arc<Owner>, error: &ControllerError, _ctx: Arc<Context>) -> Action {
    tracing::warn!(%error, "owner reconcile error, requeue in 30s");
    Action::requeue(Duration::from_secs(30))
}

/// Construit le `Controller<Owner>` prêt à tourner (`.run(...)` reste à l'appelant
/// dans `main.rs`, qui pilote aussi l'arrêt).
pub fn build_controller(client: Client) -> Controller<Owner> {
    let owners: Api<Owner> = Api::all(client);
    Controller::new(owners, kube::runtime::watcher::Config::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_owner(existing_pvc: Option<String>, home_size: Option<String>, home_storage_class: Option<String>) -> Owner {
        let mut owner = Owner::new("alice", crate::crds::OwnerSpec {
            existing_pvc,
            home_size,
            home_storage_class,
            project_defaults: None,
        });
        owner.meta_mut().namespace = Some("ns".into());
        owner.meta_mut().uid = Some("test-uid-alice".into());
        owner
    }

    #[test]
    fn home_pvc_name_and_sa_name() {
        assert_eq!(home_pvc_name("alice"), "owner-alice-home");
        assert_eq!(service_account_name("alice"), "owner-alice");
    }

    #[test]
    fn effective_pvc_name_generated() {
        let owner = make_owner(None, None, None);
        assert_eq!(effective_pvc_name(&owner), "owner-alice-home");
    }

    #[test]
    fn effective_pvc_name_existing() {
        let owner = make_owner(Some("code-server-home".into()), None, None);
        assert_eq!(effective_pvc_name(&owner), "code-server-home");
    }

    #[test]
    fn build_home_pvc_none_when_existing() {
        let owner = make_owner(Some("code-server-home".into()), None, None);
        assert!(build_home_pvc(&owner).is_none());
    }

    #[test]
    fn build_home_pvc_default_size() {
        let owner = make_owner(None, None, None);
        let pvc = build_home_pvc(&owner).expect("should build PVC when no existing_pvc");
        assert_eq!(
            pvc.spec.as_ref().unwrap().resources.as_ref().unwrap()
                .requests.as_ref().unwrap()["storage"],
            Quantity("1Gi".into())
        );
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &["ReadWriteMany".to_string()]
        );
    }

    #[test]
    fn build_home_pvc_custom_size_and_class() {
        let owner = make_owner(None, Some("5Gi".into()), Some("cephfs".into()));
        let pvc = build_home_pvc(&owner).expect("should build PVC");
        assert_eq!(
            pvc.spec.as_ref().unwrap().resources.as_ref().unwrap()
                .requests.as_ref().unwrap()["storage"],
            Quantity("5Gi".into())
        );
        assert_eq!(
            pvc.spec.as_ref().unwrap().storage_class_name.as_ref().unwrap(),
            "cephfs"
        );
    }

    #[test]
    fn build_home_pvc_owner_reference() {
        let owner = make_owner(None, None, None);
        let pvc = build_home_pvc(&owner).expect("should build PVC");
        let refs = pvc.metadata.owner_references.as_ref().expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "alice");
        assert_eq!(refs[0].kind, "Owner");
    }

    #[test]
    fn build_service_account_always_created() {
        // Existing PVC
        let owner_existing = make_owner(Some("x".into()), None, None);
        let sa_existing = build_service_account(&owner_existing);
        assert_eq!(sa_existing.metadata.name, Some("owner-alice".to_string()));

        // No existing PVC
        let owner_no_existing = make_owner(None, None, None);
        let sa_no_existing = build_service_account(&owner_no_existing);
        assert_eq!(sa_no_existing.metadata.name, Some("owner-alice".to_string()));
    }

    #[test]
    fn compute_status_ready() {
        let owner = make_owner(None, None, None);
        let status = compute_status(&owner);
        assert_eq!(status.pvc_name.as_ref().unwrap(), "owner-alice-home");
        assert_eq!(status.service_account.as_ref().unwrap(), "owner-alice");
        assert_eq!(status.conditions.len(), 1);
        assert_eq!(status.conditions[0].type_, "Ready");
        assert_eq!(status.conditions[0].status, "True");
    }
}