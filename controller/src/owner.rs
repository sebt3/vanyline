use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::core::v1::{
    PersistentVolumeClaim, PersistentVolumeClaimSpec, ServiceAccount, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::api::{Api, ListParams, ObjectMeta, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::{Client, Resource, ResourceExt};

use crate::error::ControllerError;
use crate::namespace::{self, OWNER_FINALIZER};
use vanyline_crds::{Owner, OwnerStatus, Project, Sandbox};

/// Field manager utilisé pour tous les server-side apply du controller.
pub const FIELD_MANAGER: &str = "vanyline-controller";
const DEFAULT_HOME_SIZE: &str = "1Gi";
const DEFAULT_HOME_ACCESS_MODE: &str = "ReadWriteMany";

/// Point de montage du PVC home (Owner) — sert de `$HOME` partout où il est monté
/// (Jobs git du Project, pod Sandbox).
pub const HOME_MOUNT_PATH: &str = "/home/vanyline";

/// Contexte unique du reconciler Owner.
pub struct Context {
    pub client: Client,
}

/// Nom du PVC home créé par le controller (`owner-<name>-home`).
pub fn home_pvc_name(owner_name: &str) -> String {
    format!("owner-{owner_name}-home")
}

/// Nom du `ServiceAccount` de l'Owner — toujours `owner-<name>`.
pub fn service_account_name(owner_name: &str) -> String {
    format!("owner-{owner_name}")
}

/// Nom de PVC exposé au status — toujours le PVC home créé par le controller
/// (`existing_pvc` retiré, décision 2026-10-04).
pub fn effective_pvc_name(owner: &Owner) -> String {
    home_pvc_name(&owner.name_any())
}

/// Namespace d'application : celui de la CR Owner (repli défensif `"default"`
/// — la CRD étant namespaced, le watcher fournit toujours un namespace).
fn app_namespace_of_cr(owner: &Owner) -> String {
    owner.namespace().unwrap_or_else(|| "default".to_string())
}

/// Namespace de l'Owner calculé (`{app_namespace}-{owner}`). `None` seulement
/// si le nom est hors contrat (`VNL-CTL-007`) — branche inaccessible dans le
/// reconcile, qui valide le nom dans `ensure_namespace` AVANT d'appeler ces
/// constructeurs ; les constructeurs purs restent sans erreur (`owner.sdd`).
fn owner_namespace_of_cr(owner: &Owner) -> Option<String> {
    namespace::owner_namespace_name(&app_namespace_of_cr(owner), &owner.name_any()).ok()
}

/// Construit le PVC home de l'Owner — toujours créé par le controller dans le
/// namespace de l'Owner, sans `ownerReference` (inter-namespaces interdite ;
/// la suppression du namespace est le seul nettoyage).
pub fn build_home_pvc(owner: &Owner) -> PersistentVolumeClaim {
    let size = owner
        .spec
        .home_size
        .clone()
        .unwrap_or_else(|| DEFAULT_HOME_SIZE.to_string());
    let mut requests = BTreeMap::new();
    requests.insert("storage".to_string(), Quantity(size));

    PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some(home_pvc_name(&owner.name_any())),
            namespace: owner_namespace_of_cr(owner),
            // AUCUNE owner_reference vers la CR Owner (cross-namespace
            // interdite) : la durée de vie du PVC est celle du namespace.
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec![
                owner
                    .spec
                    .home_access_mode
                    .clone()
                    .unwrap_or_else(|| DEFAULT_HOME_ACCESS_MODE.to_string()),
            ]),
            storage_class_name: owner.spec.home_storage_class.clone(),
            resources: Some(VolumeResourceRequirements {
                requests: Some(requests),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Construit le `ServiceAccount` de l'Owner — toujours créé par le controller,
/// dans le namespace de l'Owner, sans `ownerReference`. Pure identité
/// d'authentification (aucun droit RBAC ne lui est jamais attaché).
pub fn build_service_account(owner: &Owner) -> ServiceAccount {
    ServiceAccount {
        metadata: ObjectMeta {
            name: Some(service_account_name(&owner.name_any())),
            namespace: owner_namespace_of_cr(owner),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Status attendu une fois PVC et `ServiceAccount` en place : `namespace`
/// (celui de l'Owner, calculé — UN OWNER = UN NAMESPACE), `pvc_name`,
/// `service_account`, condition `Ready: True`.
pub fn compute_status(owner: &Owner) -> OwnerStatus {
    OwnerStatus {
        namespace: owner_namespace_of_cr(owner),
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

/// Reconciler kube-rs — ordre exact (`namespace.sdd`, `owner.sdd`) :
/// (0) finalizer posé AVANT toute création, puis `ensure_namespace` ;
/// (1) PVC home ; (2) `ServiceAccount` — les deux DANS le namespace de
/// l'Owner, sans `ownerReference` ; (3) status (`namespace`, `pvcName`,
/// `serviceAccount`) ; (4) bump egress ; (5) requeue 300 s.
/// Suppression : `cleanup_namespace` jusqu'à `Done`, le finalizer n'est
/// retiré qu'alors. Un Owner sans namespace créé n'est jamais `Ready`
/// (toute erreur aux étapes (1)–(3) abandonne les suivantes, sans status).
pub async fn reconcile(owner: Arc<Owner>, ctx: Arc<Context>) -> Result<Action, ControllerError> {
    let app_ns = app_namespace_of_cr(&owner);
    let owners: Api<Owner> = Api::namespaced(ctx.client.clone(), &app_ns);
    let name = owner.name_any();

    // Suppression de l'Owner : nettoyage du namespace, finalizer retiré
    // seulement à `Done` (jamais le namespace d'un autre propriétaire).
    if owner.meta().deletion_timestamp.is_some() {
        return match namespace::cleanup_namespace(&ctx.client, &owner, &app_ns).await? {
            namespace::Cleanup::Pending => Ok(Action::requeue(Duration::from_secs(30))),
            namespace::Cleanup::Done => {
                let remaining: Vec<String> = owner
                    .finalizers()
                    .to_vec()
                    .into_iter()
                    .filter(|f| f != OWNER_FINALIZER)
                    .collect();
                let patch = serde_json::json!({ "metadata": { "finalizers": remaining } });
                owners
                    .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
                    .await?;
                Ok(Action::await_change())
            }
        };
    }

    // (0) Finalizer posé AVANT de créer le namespace.
    if !owner
        .finalizers()
        .iter()
        .any(|f| f.as_str() == OWNER_FINALIZER)
    {
        let mut finalizers: Vec<String> = owner.finalizers().to_vec();
        finalizers.push(OWNER_FINALIZER.to_string());
        let patch = serde_json::json!({ "metadata": { "finalizers": finalizers } });
        owners
            .patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?;
    }

    // (1) Namespace de l'Owner — et RoleBinding de l'app seulement si
    // `application_ref` résout une Application (namespace.sdd).
    namespace::ensure_namespace(&ctx.client, &owner, &app_ns).await?;
    let owner_ns = namespace::owner_namespace_name(&app_ns, &name)?;

    let pp = PatchParams::apply(FIELD_MANAGER).force();

    // (2) PVC home DANS le namespace de l'Owner.
    let pvc = build_home_pvc(&owner);
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(ctx.client.clone(), &owner_ns);
    pvcs.patch(&home_pvc_name(&name), &pp, &Patch::Apply(&pvc))
        .await?;

    // (2) ServiceAccount DANS le namespace de l'Owner.
    let sas: Api<ServiceAccount> = Api::namespaced(ctx.client.clone(), &owner_ns);
    let sa = build_service_account(&owner);
    sas.patch(&service_account_name(&name), &pp, &Patch::Apply(&sa))
        .await?;

    // (3) Status (`namespace`, `pvcName`, `serviceAccount`, Ready).
    let status = compute_status(&owner);
    let patch = serde_json::json!({ "status": status });
    owners
        .patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;

    // (4) Bump inconditionnel, 2 sauts (Owner -> ses Projects -> leurs
    // Sandboxes) : force le reconcile immediat de toutes les Sandboxes de
    // cet Owner. Meme logique que project.rs::apply(), cf. design
    // ws13-sandbox-runtime section 2. Projects et Sandboxes vivent desormais
    // dans le namespace de l'Owner, plus dans celui de la CR.
    let projects: Api<Project> = Api::namespaced(ctx.client.clone(), &owner_ns);
    let project_names: Vec<String> = projects
        .list(&ListParams::default())
        .await?
        .into_iter()
        .filter(|p| p.spec.owner == name)
        .map(|p| p.name_any())
        .collect();

    if !project_names.is_empty() {
        let sandboxes: Api<Sandbox> = Api::namespaced(ctx.client.clone(), &owner_ns);
        let bump = serde_json::json!({
            "metadata": {
                "annotations": {
                    "vanyline.solidite.fr/egress-bump": k8s_openapi::jiff::Timestamp::now().to_string()
                }
            }
        });
        for sandbox in sandboxes.list(&ListParams::default()).await? {
            if project_names.contains(&sandbox.spec.project) {
                sandboxes
                    .patch(
                        &sandbox.name_any(),
                        &PatchParams::default(),
                        &Patch::Merge(&bump),
                    )
                    .await?;
            }
        }
    }

    // (5) Requeue nominal.
    Ok(Action::requeue(Duration::from_mins(5)))
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn make_owner(home_size: Option<String>, home_storage_class: Option<String>) -> Owner {
        let mut owner = Owner::new(
            "alice",
            vanyline_crds::OwnerSpec {
                home_size,
                home_storage_class,
                home_access_mode: None,
                project_defaults: None,
                application_ref: None,
                egress: Vec::new(),
            },
        );
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
        let owner = make_owner(None, None);
        assert_eq!(effective_pvc_name(&owner), "owner-alice-home");
    }

    #[test]
    fn build_home_pvc_default_size() {
        let owner = make_owner(None, None);
        let pvc = build_home_pvc(&owner);
        assert_eq!(
            pvc.spec
                .as_ref()
                .unwrap()
                .resources
                .as_ref()
                .unwrap()
                .requests
                .as_ref()
                .unwrap()["storage"],
            Quantity("1Gi".into())
        );
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &["ReadWriteMany".to_string()]
        );
    }

    #[test]
    fn build_home_pvc_custom_access_mode() {
        let mut owner = make_owner(None, None);
        owner.spec.home_access_mode = Some("ReadWriteOnce".to_string());
        let pvc = build_home_pvc(&owner);
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &["ReadWriteOnce".to_string()]
        );
    }

    #[test]
    fn build_home_pvc_custom_size_and_class() {
        let owner = make_owner(Some("5Gi".into()), Some("cephfs".into()));
        let pvc = build_home_pvc(&owner);
        assert_eq!(
            pvc.spec
                .as_ref()
                .unwrap()
                .resources
                .as_ref()
                .unwrap()
                .requests
                .as_ref()
                .unwrap()["storage"],
            Quantity("5Gi".into())
        );
        assert_eq!(
            pvc.spec
                .as_ref()
                .unwrap()
                .storage_class_name
                .as_ref()
                .unwrap(),
            "cephfs"
        );
    }

    // Scenario owner.sdd: build_home_pvc_sans_owner_reference — le PVC vit dans
    // le namespace de l'Owner et disparaît avec lui (ownerReference
    // inter-namespaces interdite).
    #[test]
    fn build_home_pvc_sans_owner_reference() {
        let owner = make_owner(None, None);
        let pvc = build_home_pvc(&owner);
        assert!(
            pvc.metadata.owner_references.is_none(),
            "aucune ownerReference vers la CR Owner (inter-namespaces interdite)"
        );
    }

    // Scenario owner.sdd: build_home_pvc_metadata_champs_nus — contrat du
    // constructeur verrouillé champ par champ.
    #[test]
    fn build_home_pvc_metadata_champs_nus() {
        let owner = make_owner(None, None);
        let pvc = build_home_pvc(&owner);
        assert_eq!(pvc.metadata.name.as_deref(), Some("owner-alice-home"));
        assert_eq!(pvc.metadata.namespace.as_deref(), Some("ns-alice"));
        assert!(pvc.metadata.labels.is_none());
        assert!(pvc.metadata.annotations.is_none());
        assert!(pvc.metadata.finalizers.is_none());
        let spec = pvc.spec.as_ref().unwrap();
        assert_eq!(
            spec.access_modes.as_ref().unwrap(),
            &["ReadWriteMany".to_string()]
        );
        assert!(spec.storage_class_name.is_none());
        let resources = spec.resources.as_ref().unwrap();
        assert!(resources.limits.is_none());
        assert_eq!(
            resources.requests.as_ref().unwrap()["storage"],
            Quantity("1Gi".into())
        );
        assert!(spec.selector.is_none());
        assert!(spec.data_source.is_none());
        assert!(spec.volume_name.is_none());
        assert!(spec.volume_mode.is_none());
    }

    #[test]
    fn build_service_account_always_created() {
        let owner = make_owner(None, None);
        let sa = build_service_account(&owner);
        assert_eq!(sa.metadata.name, Some("owner-alice".to_string()));
    }

    // Scenario owner.sdd: build_service_account_champs_nus — le SA ne porte QUE
    // name + namespace (celui de l'Owner), spec par défaut, sans ownerReferences.
    #[test]
    fn build_service_account_champs_nus() {
        let owner = make_owner(None, None);
        let sa = build_service_account(&owner);
        assert_eq!(sa.metadata.name.as_deref(), Some("owner-alice"));
        assert_eq!(sa.metadata.namespace.as_deref(), Some("ns-alice"));
        assert!(sa.metadata.owner_references.is_none());
        assert!(sa.metadata.labels.is_none());
        assert!(sa.metadata.annotations.is_none());
        assert!(sa.secrets.is_none());
        assert!(sa.image_pull_secrets.is_none());
        assert!(sa.automount_service_account_token.is_none());
    }

    #[test]
    fn compute_status_ready() {
        let owner = make_owner(None, None);
        let status = compute_status(&owner);
        assert_eq!(status.pvc_name.as_ref().unwrap(), "owner-alice-home");
        assert_eq!(status.service_account.as_ref().unwrap(), "owner-alice");
        assert_eq!(
            status.namespace.as_deref(),
            Some("ns-alice"),
            "UN OWNER = UN NAMESPACE : le status porte le namespace calculé"
        );
        assert_eq!(status.conditions.len(), 1);
        assert_eq!(status.conditions[0].type_, "Ready");
        assert_eq!(status.conditions[0].status, "True");
    }

    #[test]
    fn compute_status_observed_generation_reprise() {
        let mut owner = make_owner(None, None);
        owner.meta_mut().generation = Some(7);
        let before = k8s_openapi::jiff::Timestamp::now();
        let first = compute_status(&owner);
        let second = compute_status(&owner);
        let ready = &first.conditions[0];
        assert_eq!(ready.observed_generation, Some(7));
        // horodatage frais : posé à l'appel, pas figé à une valeur antérieure
        assert!(ready.last_transition_time.0 >= before);
        assert!(second.conditions[0].last_transition_time.0 >= ready.last_transition_time.0);
    }

    #[test]
    fn compute_status_serialisation_merge_patch() {
        let mut owner = make_owner(None, None);
        // génération posée : sans elle, observedGeneration est omis (skip_serializing_if)
        owner.meta_mut().generation = Some(1);
        let status = compute_status(&owner);
        let value = serde_json::to_value(&status).expect("serialisation du status");
        let obj = value.as_object().expect("status en objet JSON");

        assert_eq!(
            obj.get("pvcName").and_then(|v| v.as_str()),
            Some("owner-alice-home")
        );
        assert_eq!(
            obj.get("serviceAccount").and_then(|v| v.as_str()),
            Some("owner-alice")
        );
        assert_eq!(
            obj.get("namespace").and_then(|v| v.as_str()),
            Some("ns-alice")
        );

        let conditions = obj
            .get("conditions")
            .and_then(|v| v.as_array())
            .expect("conditions en tableau");
        assert_eq!(conditions.len(), 1);
        let c = conditions[0].as_object().expect("condition en objet");
        for key in [
            "type",
            "status",
            "reason",
            "message",
            "lastTransitionTime",
            "observedGeneration",
        ] {
            assert!(c.contains_key(key), "condition sans clé {key}");
        }

        // enveloppe Merge : aucun null d'effacement dans le status produit
        let envelope = serde_json::json!({ "status": status });
        assert!(
            !envelope.to_string().contains("null"),
            "null d'effacement présent"
        );
    }
}
