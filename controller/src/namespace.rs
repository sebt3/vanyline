//! Cycle de vie du NAMESPACE d'un Owner — UN OWNER = UN NAMESPACE (décision
//! 2026-10-04). Nommage (fonction pure partagée de `vanyline-crds`), construction
//! et application du `Namespace` et du `RoleBinding` qui y délègue à `app` ses
//! droits, refus d'adoption d'un namespace étranger, nettoyage par finalizer.
//! Contrat détaillé : `namespace.sdd`. Appelé par `owner.rs` (qui n'en garde que
//! l'orchestration).
//!
//! Découpage : toute DÉCISION (adoption, collision, terminating, nettoyage,
//! résolution du label) est une fonction pure sur objets k8s passés en
//! paramètre ; les fonctions `async` ne sont que le transport (get/apply/delete).

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::api::rbac::v1::{RoleBinding, RoleRef, Subject};
use kube::api::{Api, ObjectMeta, Patch, PatchParams};
use kube::{Client, Resource, ResourceExt};

use crate::application::application_name;
use crate::error::ControllerError;
use crate::owner::FIELD_MANAGER;
use vanyline_crds::{Application, Owner};

/// Label portant le nom de l'Owner sur son namespace.
pub const OWNER_LABEL: &str = "vanyline.solidite.fr/owner";
/// Label portant le namespace d'application sur le namespace de l'Owner.
pub const APP_NAMESPACE_LABEL: &str = "vanyline.solidite.fr/app-namespace";
/// Finalizer porté par la CR `Owner` : le namespace n'est abandonné qu'une fois
/// réellement disparu.
pub const OWNER_FINALIZER: &str = "vanyline.solidite.fr/owner-namespace";
/// Nom du `ClusterRole` fourni par le déploiement
/// (`deploy/controller/controller.yaml`) — seul droit jamais délégué à `app`
/// dans un namespace d'Owner.
pub const APP_SCOPE_CLUSTER_ROLE: &str = "vanyline-app-owner-scope";

/// Nom du `RoleBinding` de l'app dans chaque namespace d'Owner.
const APP_ROLE_BINDING_NAME: &str = "vanyline-app";
/// Label de traçabilité posé sur le namespace (non discriminant pour l'adoption).
const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
/// Valeur du label `managed-by` : ce controller.
const MANAGED_BY: &str = "vanyline-controller";

/// Nom du namespace d'un Owner — `"{app_namespace}-{owner}"`, par la fonction
/// pure de `vanyline-crds` (source unique partagée avec `app`), l'erreur de
/// validation étant convertie en `InvalidNamespaceName` (`VNL-CTL-007`).
///
/// # Errors
///
/// [`ControllerError::InvalidNamespaceName`] si la concaténation n'est pas un
/// label DNS-1123 d'au plus 63 caractères.
pub fn owner_namespace_name(app_namespace: &str, owner: &str) -> Result<String, ControllerError> {
    vanyline_crds::owner_namespace_name(app_namespace, owner).map_err(|reason| {
        ControllerError::InvalidNamespaceName {
            name: format!("{app_namespace}-{owner}"),
            reason,
        }
    })
}

/// Construit le `Namespace` de l'Owner (labels contractuels, AUCUNE
/// `ownerReference` : un objet namespacé ne peut pas posséder un objet
/// cluster-scopé).
///
/// # Errors
///
/// [`ControllerError::InvalidNamespaceName`] (`VNL-CTL-007`) si le nom calculé
/// n'est pas valide.
pub fn build_owner_namespace(
    owner: &Owner,
    app_namespace: &str,
) -> Result<Namespace, ControllerError> {
    let name = owner_namespace_name(app_namespace, &owner.name_any())?;
    let mut labels = BTreeMap::new();
    labels.insert(OWNER_LABEL.to_string(), owner.name_any());
    labels.insert(APP_NAMESPACE_LABEL.to_string(), app_namespace.to_string());
    labels.insert(MANAGED_BY_LABEL.to_string(), MANAGED_BY.to_string());
    Ok(Namespace {
        metadata: ObjectMeta {
            name: Some(name),
            labels: Some(labels),
            // AUCUNE owner_reference : la CR Owner est namespacée, elle ne peut
            // pas posséder ce objet cluster-scopé (namespace.sdd).
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Construit le `RoleBinding` déléguant à `app` le `ClusterRole`
/// [`APP_SCOPE_CLUSTER_ROLE`] dans le namespace de l'Owner : nom `vanyline-app`,
/// un seul sujet, le `ServiceAccount` d'application du namespace d'application.
#[must_use]
pub fn build_app_role_binding(
    owner_namespace: &str,
    app_service_account: &str,
    app_namespace: &str,
) -> RoleBinding {
    RoleBinding {
        metadata: ObjectMeta {
            name: Some(APP_ROLE_BINDING_NAME.to_string()),
            namespace: Some(owner_namespace.to_string()),
            ..Default::default()
        },
        role_ref: RoleRef {
            api_group: Some("rbac.authorization.k8s.io".to_string()),
            kind: "ClusterRole".to_string(),
            name: APP_SCOPE_CLUSTER_ROLE.to_string(),
        },
        subjects: Some(vec![Subject {
            kind: "ServiceAccount".to_string(),
            name: app_service_account.to_string(),
            namespace: Some(app_namespace.to_string()),
            ..Default::default()
        }]),
    }
}

/// Le namespace porte-t-il les labels de cet Owner (aux valeurs attendues) ?
fn owned_by(namespace: &Namespace, owner_name: &str, app_namespace: &str) -> bool {
    let labels = namespace.labels();
    labels.get(OWNER_LABEL).map(String::as_str) == Some(owner_name)
        && labels.get(APP_NAMESPACE_LABEL).map(String::as_str) == Some(app_namespace)
}

/// Décision pure d'`ensure_namespace` sur le `Namespace` existant (l'appel API
/// n'est que le transport) : `Ok(())` autorise le server-side apply (création
/// ou adoption idempotente) ; toute erreur interdit tout patch.
fn ensure_decision(
    existing: Option<&Namespace>,
    owner_name: &str,
    app_namespace: &str,
) -> Result<(), ControllerError> {
    match existing {
        None => Ok(()),
        Some(namespace) => {
            if !owned_by(namespace, owner_name, app_namespace) {
                // Jamais adopter, modifier ou supprimer un namespace qui ne
                // porte pas les labels de cet Owner (VNL-CTL-008).
                return Err(ControllerError::NamespaceCollision {
                    namespace: namespace.name_any(),
                    owner: owner_name.to_string(),
                });
            }
            if namespace.meta().deletion_timestamp.is_some() {
                // Terminating : ni création ni adoption, l'Owner est requeué
                // (VNL-CTL-009).
                return Err(ControllerError::NamespaceTerminating {
                    namespace: namespace.name_any(),
                });
            }
            Ok(())
        }
    }
}

/// Décision pure de `cleanup_namespace`.
#[derive(Debug, PartialEq, Eq)]
enum CleanupStep {
    /// Le namespace n'existe plus — le finalizer peut être retiré.
    Done,
    /// Le namespace existe, il faut émettre la suppression.
    Delete,
    /// Le namespace existe déjà en `Terminating` — attendre, sans re-patch.
    Wait,
}

/// Étape de suppression rendue par [`cleanup_namespace`].
#[derive(Debug, PartialEq, Eq)]
pub enum Cleanup {
    /// Le namespace n'existe plus.
    Done,
    /// Le namespace existe encore (en suppression) — l'Owner est requeué.
    Pending,
}

/// Décision pure de `cleanup_namespace` sur le `Namespace` existant : jamais de
/// suppression d'un namespace qui ne porte pas les labels de cet Owner.
fn cleanup_decision(
    existing: Option<&Namespace>,
    owner_name: &str,
    app_namespace: &str,
) -> Result<CleanupStep, ControllerError> {
    match existing {
        None => Ok(CleanupStep::Done),
        Some(namespace) => {
            if !owned_by(namespace, owner_name, app_namespace) {
                // Aucune suppression émise sur un namespace étranger
                // (VNL-CTL-008).
                return Err(ControllerError::NamespaceCollision {
                    namespace: namespace.name_any(),
                    owner: owner_name.to_string(),
                });
            }
            if namespace.meta().deletion_timestamp.is_some() {
                Ok(CleanupStep::Wait)
            } else {
                Ok(CleanupStep::Delete)
            }
        }
    }
}

/// Décision pure : le `RoleBinding` de l'app est-il dû, et lequel ? Créé
/// seulement si `spec.application_ref` résout une Application (le nom du SA de
/// l'app est celui de l'Application, `application.sdd`) ; sans Application
/// aucun droit n'est délégué.
fn app_role_binding_for(
    owner: &Owner,
    application_resolves: bool,
    owner_namespace: &str,
    app_namespace: &str,
) -> Option<RoleBinding> {
    match owner.spec.application_ref.as_deref() {
        Some(ref_name) if application_resolves => Some(build_app_role_binding(
            owner_namespace,
            &application_name(ref_name),
            app_namespace,
        )),
        _ => None,
    }
}

/// Extrait le namespace d'application du label [`APP_NAMESPACE_LABEL`] d'un
/// `Namespace` (partie pure d'`app_namespace_of`).
fn app_namespace_of_namespace(namespace: &Namespace) -> Result<String, ControllerError> {
    namespace
        .labels()
        .get(APP_NAMESPACE_LABEL)
        .cloned()
        .ok_or_else(|| ControllerError::NamespaceNotManaged {
            namespace: namespace.name_any(),
        })
}

/// Cache processus `namespace → app_namespace` : le label est posé à la
/// création par ce controller et n'est plus modifié depuis.
fn app_namespace_cache() -> &'static Mutex<BTreeMap<String, String>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Le namespace d'application d'un namespace d'Owner : la valeur du label
/// [`APP_NAMESPACE_LABEL`] du `Namespace` (mise en cache). Utilisée par
/// `project.rs` et `sandbox.rs`, dont les CR vivent dans le namespace de
/// l'Owner, pour retrouver la CR `Owner` et l'`Application` du namespace
/// d'application.
///
/// # Errors
///
/// [`ControllerError::NamespaceNotManaged`] (`VNL-CTL-010`) si le namespace est
/// dépourvu de ce label ; [`ControllerError::Kube`] (`VNL-CTL-001`) sur erreur
/// d'apiserver.
pub async fn app_namespace_of(client: &Client, namespace: &str) -> Result<String, ControllerError> {
    {
        let cache = app_namespace_cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(app_namespace) = cache.get(namespace) {
            return Ok(app_namespace.clone());
        }
    }
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns = namespaces.get(namespace).await?;
    let app_namespace = app_namespace_of_namespace(&ns)?;
    app_namespace_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(namespace.to_string(), app_namespace.clone());
    Ok(app_namespace)
}

/// Garantit le Namespace de l'Owner (et le `RoleBinding` de l'app si
/// `application_ref` résout une Application) par server-side apply idempotent.
///
/// # Errors
///
/// `VNL-CTL-007` (nom invalide, validé avant tout appel), `VNL-CTL-008`
/// (namespace étranger, aucun patch émis), `VNL-CTL-009` (`Terminating`),
/// `VNL-CTL-001` (erreurs d'apiserver).
pub async fn ensure_namespace(
    client: &Client,
    owner: &Owner,
    app_namespace: &str,
) -> Result<(), ControllerError> {
    // Validation du nom AVANT tout appel à l'apiserver (VNL-CTL-007).
    let owner_namespace = owner_namespace_name(app_namespace, &owner.name_any())?;
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let existing = namespaces.get_opt(&owner_namespace).await?;
    // Décision pure : collision (VNL-CTL-008) ou Terminating (VNL-CTL-009) ⇒
    // aucun patch émis, le namespace existant reste intact.
    ensure_decision(existing.as_ref(), &owner.name_any(), app_namespace)?;
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    let ns = build_owner_namespace(owner, app_namespace)?;
    namespaces
        .patch(&owner_namespace, &pp, &Patch::Apply(&ns))
        .await?;
    // RoleBinding de l'app : seulement si `application_ref` résout une
    // Application ; sans Application aucun droit n'est délégué.
    let application_resolves = match owner.spec.application_ref.as_deref() {
        Some(ref_name) => {
            let applications: Api<Application> = Api::namespaced(client.clone(), app_namespace);
            applications.get_opt(ref_name).await?.is_some()
        }
        None => false,
    };
    if let Some(role_binding) =
        app_role_binding_for(owner, application_resolves, &owner_namespace, app_namespace)
    {
        let role_bindings: Api<RoleBinding> = Api::namespaced(client.clone(), &owner_namespace);
        role_bindings
            .patch(APP_ROLE_BINDING_NAME, &pp, &Patch::Apply(&role_binding))
            .await?;
    }
    Ok(())
}

/// Supprime le Namespace de l'Owner (s'il porte bien ses labels) et rend
/// [`Cleanup::Pending`] tant qu'il existe, [`Cleanup::Done`] quand il a disparu.
///
/// # Errors
///
/// `VNL-CTL-007` (nom invalide), `VNL-CTL-008` (namespace homonyme d'un autre
/// propriétaire — aucune suppression émise), `VNL-CTL-001` (apiserver).
pub async fn cleanup_namespace(
    client: &Client,
    owner: &Owner,
    app_namespace: &str,
) -> Result<Cleanup, ControllerError> {
    let owner_namespace = owner_namespace_name(app_namespace, &owner.name_any())?;
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let existing = namespaces.get_opt(&owner_namespace).await?;
    match cleanup_decision(existing.as_ref(), &owner.name_any(), app_namespace)? {
        CleanupStep::Done => Ok(Cleanup::Done),
        CleanupStep::Wait => Ok(Cleanup::Pending),
        CleanupStep::Delete => {
            namespaces
                .delete(&owner_namespace, &kube::api::DeleteParams::default())
                .await?;
            Ok(Cleanup::Pending)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    fn make_owner(name: &str, application_ref: Option<&str>) -> Owner {
        let mut owner = Owner::new(
            name,
            vanyline_crds::OwnerSpec {
                home_size: None,
                home_storage_class: None,
                home_access_mode: None,
                project_defaults: None,
                application_ref: application_ref.map(str::to_string),
                egress: Vec::new(),
            },
        );
        owner.meta_mut().namespace = Some("media-test".into());
        owner
    }

    fn make_namespace(name: &str, labels: &[(&str, &str)], deletion_timestamp: bool) -> Namespace {
        let mut ns = Namespace {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        if !labels.is_empty() {
            ns.metadata.labels = Some(
                labels
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            );
        }
        if deletion_timestamp {
            ns.metadata.deletion_timestamp = Some(Time(k8s_openapi::jiff::Timestamp::now()));
        }
        ns
    }

    fn owner_labels<'a>(owner: &'a str, app_ns: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![(OWNER_LABEL, owner), (APP_NAMESPACE_LABEL, app_ns)]
    }

    // Scenario: nom_du_namespace
    #[test]
    fn nom_du_namespace() {
        assert_eq!(
            owner_namespace_name("media-test", "seb").unwrap(),
            "media-test-seb"
        );
    }

    // Scenario: nom_trop_long — InvalidNamespaceName (VNL-CTL-007), aucun appel
    // à l'apiserver (la validation est une fonction pure, appelée avant toute
    // décision d'appel par ensure/cleanup).
    #[test]
    fn nom_trop_long() {
        let err =
            owner_namespace_name(&"a".repeat(32), &"b".repeat(31)).expect_err("64 caractères > 63");
        assert!(
            matches!(err, ControllerError::InvalidNamespaceName { .. }),
            "variante attendue InvalidNamespaceName, obtenu {err:?}"
        );
        if let ControllerError::InvalidNamespaceName { name, .. } = &err {
            assert_eq!(name.len(), 64);
        }
        assert!(err.to_string().starts_with("VNL-CTL-007: "));
    }

    // Scenario: nom_invalide — majuscule ou `_` rejetés (VNL-CTL-007).
    #[test]
    fn nom_invalide() {
        let err =
            owner_namespace_name("media-Test", "seb").expect_err("majuscule interdite en DNS-1123");
        assert!(err.to_string().starts_with("VNL-CTL-007: "));
        assert!(owner_namespace_name("media-test", "se_b").is_err());
    }

    // Scenario: namespace_construit_avec_labels_sans_owner_reference
    #[test]
    fn namespace_construit_avec_labels_sans_owner_reference() {
        let owner = make_owner("seb", None);
        let ns = build_owner_namespace(&owner, "media-test").unwrap();
        assert_eq!(ns.metadata.name.as_deref(), Some("media-test-seb"));
        assert_eq!(ns.metadata.namespace, None, "objet cluster-scopé");
        let labels = ns.metadata.labels.as_ref().unwrap();
        assert_eq!(labels.get(OWNER_LABEL).map(String::as_str), Some("seb"));
        assert_eq!(
            labels.get(APP_NAMESPACE_LABEL).map(String::as_str),
            Some("media-test")
        );
        assert_eq!(
            labels.get(MANAGED_BY_LABEL).map(String::as_str),
            Some("vanyline-controller")
        );
        assert!(
            ns.metadata.owner_references.is_none(),
            "un objet namespacé ne peut pas posséder un objet cluster-scopé"
        );
    }

    // Scenario: role_binding_de_l_app
    #[test]
    fn role_binding_de_l_app() {
        let rb =
            build_app_role_binding("media-test-seb", "application-vanyline-test", "media-test");
        assert_eq!(rb.metadata.name.as_ref().unwrap(), "vanyline-app");
        assert_eq!(rb.metadata.namespace.as_deref(), Some("media-test-seb"));
        assert_eq!(rb.role_ref.kind, "ClusterRole");
        assert_eq!(rb.role_ref.name, APP_SCOPE_CLUSTER_ROLE);
        assert_eq!(
            rb.role_ref.api_group.as_deref(),
            Some("rbac.authorization.k8s.io")
        );
        let subjects = rb.subjects.as_ref().unwrap();
        assert_eq!(subjects.len(), 1, "un seul sujet");
        assert_eq!(subjects[0].kind, "ServiceAccount");
        assert_eq!(subjects[0].name, "application-vanyline-test");
        assert_eq!(
            subjects[0].namespace.as_deref(),
            Some("media-test"),
            "le SA de l'app vit dans le namespace d'application"
        );
    }

    // Scenario: pas_de_role_binding_sans_application — la décision est pure :
    // sans `application_ref`, aucun RoleBinding n'est dû, même si une
    // Application existe ; avec ref résolvant une Application, le RoleBinding
    // est construit sur le SA `application-<ref>` du namespace d'application.
    #[test]
    fn pas_de_role_binding_sans_application() {
        let owner = make_owner("seb", None);
        assert!(app_role_binding_for(&owner, true, "media-test-seb", "media-test").is_none());

        let owner_with_ref = make_owner("seb", Some("vanyline-test"));
        // L'application_ref ne résout pas (Application absente) : pas de RB.
        assert!(
            app_role_binding_for(&owner_with_ref, false, "media-test-seb", "media-test").is_none()
        );
        // Elle résout : RoleBinding sur le SA de l'Application.
        let rb = app_role_binding_for(&owner_with_ref, true, "media-test-seb", "media-test")
            .expect("application_ref résolvant une Application ⇒ RoleBinding");
        assert_eq!(rb.metadata.name.as_ref().unwrap(), APP_ROLE_BINDING_NAME);
        assert_eq!(rb.metadata.namespace.as_deref(), Some("media-test-seb"));
        let subjects = rb.subjects.as_ref().unwrap();
        assert_eq!(subjects[0].name, application_name("vanyline-test"));
        assert_eq!(subjects[0].namespace.as_deref(), Some("media-test"));
    }

    // Scenario: collision_avec_un_namespace_etranger — NamespaceCollision
    // (VNL-CTL-008), le namespace existant est intact (aucun patch : la
    // décision rend l'erreur AVANT tout appel d'apply).
    #[test]
    fn collision_avec_un_namespace_etranger() {
        let foreign = make_namespace("media-test-seb", &[("app", "quelqu-un-d-autre")], false);
        let err = ensure_decision(Some(&foreign), "seb", "media-test")
            .expect_err("namespace sans les labels de l'Owner ⇒ collision");
        assert!(err.to_string().starts_with("VNL-CTL-008: "));
        // Labels partiellement présents (owner mis, app-namespace faux) : idem.
        let half = make_namespace("media-test-seb", &[(OWNER_LABEL, "seb")], false);
        assert!(ensure_decision(Some(&half), "seb", "media-test").is_err());
        // Nom d'Owner visant le namespace d'un autre Owner : même refus.
        let other_owner = make_namespace(
            "media-test-seb",
            &owner_labels("alice", "media-test"),
            false,
        );
        assert!(ensure_decision(Some(&other_owner), "bob", "media-test").is_err());
    }

    // Scenario: adoption_d_un_namespace_a_nous — bons labels ⇒ décision Ok,
    // l'apply est idempotent (SSA), et absent ⇒ Ok aussi (création).
    #[test]
    fn adoption_d_un_namespace_a_nous() {
        let ours = make_namespace("media-test-seb", &owner_labels("seb", "media-test"), false);
        ensure_decision(Some(&ours), "seb", "media-test").expect("adoption Ok");
        ensure_decision(None, "seb", "media-test").expect("création Ok");
    }

    // Scenario: namespace_en_terminating — NamespaceTerminating (VNL-CTL-009),
    // l'Owner est requeué sans erreur fatale (error_policy : warn + 30 s).
    #[test]
    fn namespace_en_terminating() {
        let terminating =
            make_namespace("media-test-seb", &owner_labels("seb", "media-test"), true);
        let err = ensure_decision(Some(&terminating), "seb", "media-test")
            .expect_err("Terminating ⇒ ni création ni adoption");
        assert!(err.to_string().starts_with("VNL-CTL-009: "));
    }

    // Scenario: suppression_par_finalizer — la décision de cleanup est pure :
    // disparu ⇒ Done ; présent et à nous ⇒ Delete (le finalizer reste, l'appel
    // rend Pending) ; déjà Terminating ⇒ Wait sans re-suppression (Pending).
    #[test]
    fn suppression_par_finalizer() {
        let ours = make_namespace("media-test-seb", &owner_labels("seb", "media-test"), false);
        assert_eq!(
            cleanup_decision(Some(&ours), "seb", "media-test").unwrap(),
            CleanupStep::Delete
        );
        let terminating =
            make_namespace("media-test-seb", &owner_labels("seb", "media-test"), true);
        assert_eq!(
            cleanup_decision(Some(&terminating), "seb", "media-test").unwrap(),
            CleanupStep::Wait
        );
        assert_eq!(
            cleanup_decision(None, "seb", "media-test").unwrap(),
            CleanupStep::Done
        );
    }

    // Scenario: suppression_ne_touche_pas_un_namespace_etranger — aucune
    // suppression n'est émise, l'erreur VNL-CTL-008 est rendue.
    #[test]
    fn suppression_ne_touche_pas_un_namespace_etranger() {
        let foreign = make_namespace("media-test-seb", &[], false);
        let err = cleanup_decision(Some(&foreign), "seb", "media-test")
            .expect_err("le namespace d'un autre propriétaire n'est jamais supprimé");
        assert!(err.to_string().starts_with("VNL-CTL-008: "));
    }

    // Scenario: app_namespace_depuis_le_label
    #[test]
    fn app_namespace_depuis_le_label() {
        let ns = make_namespace("media-test-seb", &owner_labels("seb", "media-test"), false);
        assert_eq!(app_namespace_of_namespace(&ns).unwrap(), "media-test");
    }

    // Scenario: namespace_non_gere — NamespaceNotManaged (VNL-CTL-010).
    #[test]
    fn namespace_non_gere() {
        let ns = make_namespace("default", &[], false);
        let err =
            app_namespace_of_namespace(&ns).expect_err("un namespace non géré ne se résout pas");
        assert!(err.to_string().starts_with("VNL-CTL-010: "));
    }
}
