use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::{
    Deployment, DeploymentCondition, DeploymentSpec, StatefulSet,
    StatefulSetPersistentVolumeClaimRetentionPolicy, StatefulSetSpec,
};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, EnvVarSource, HTTPGetAction, PersistentVolumeClaim,
    PersistentVolumeClaimSpec, PodSpec, PodTemplateSpec, Probe, Secret, SecretKeySelector, Service,
    ServiceAccount, ServicePort, ServiceSpec, VolumeMount, VolumeResourceRequirements,
};
use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec, IngressTLS, NetworkPolicy, NetworkPolicyIngressRule,
    NetworkPolicyPeer, NetworkPolicyPort, NetworkPolicySpec, ServiceBackendPort,
};
use k8s_openapi::api::rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{
    Condition, LabelSelector, LabelSelectorRequirement, ObjectMeta, OwnerReference,
};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::runtime::controller::{Action, Controller};
use kube::{Client, Resource, ResourceExt};

use crate::error::ControllerError;
use crate::namespace::APP_NAMESPACE_LABEL;
use crate::owner::FIELD_MANAGER;
use crate::sandbox::{
    RESTATE_INGRESS_PORT, RESTATE_LABEL, restate_ingress_url, restate_service_name,
};
use vanyline_crds::{Application, ApplicationStatus};

/// Port HTTP du Deployment `app` — défaut de `LISTEN_ADDR` côté
/// `app/src/config.rs` (`0.0.0.0:8080`).
pub const APP_PORT: i32 = 8080;

/// Port du endpoint de services workflow (Restate appelle l'app ici) — posé
/// seulement quand `spec.restate` est présent (`application.sdd`, « Endpoint de
/// services »). Nommé `workflow` côté Service et conteneur.
pub const WORKFLOW_PORT: i32 = 9080;

/// Label unique porté par le Deployment `app` et utilisé comme sélecteur du
/// Service — même patron que `build_sandbox_service`.
pub const APP_LABEL: &str = "vanyline.solidite.fr/application";

/// Port d'admin HTTP du moteur Restate (enregistrement des services, statut
/// des runs) — exposé par le Service `<application>-restate` et joignable par
/// le pod `app` (`application.sdd` « Moteur de workflow »).
pub const RESTATE_ADMIN_PORT: i32 = 9070;

/// Port de nœud (socket worker) du moteur Restate — troisième port du Service
/// `<application>-restate`.
pub const RESTATE_WORKER_PORT: i32 = 5122;

/// Taille par défaut du `volumeClaimTemplate` de données de Restate quand
/// `spec.restate.storageSize` est absent (repli appliqué par ce reconciler,
/// jamais par `vanyline-crds`).
const RESTATE_DEFAULT_STORAGE_SIZE: &str = "5Gi";

/// Point de montage du volume de données dans le pod Restate (`RESTATE_DATA_DIR`).
const RESTATE_DATA_DIR: &str = "/var/restate";

/// Nom commun des objets créés pour une Application : Deployment, Service et
/// Ingress — `application-<name>` (même convention que `sandbox-<name>`).
pub fn application_name(app_name: &str) -> String {
    format!("application-{app_name}")
}

/// Nom du Secret cookie auto-généré : `<application-name>-cookie`.
pub fn cookie_secret_name(app_name: &str) -> String {
    format!("{app_name}-cookie")
}

/// Nom du Secret TLS cert-manager de l'Ingress : `<application-name>-cert` —
/// même convention que les autres Ingress du cluster (`penpot-cert`, etc.).
pub fn tls_secret_name(app_name: &str) -> String {
    format!("{app_name}-cert")
}

/// Nom de l'annotation cert-manager pour poser l'issuer, selon `tls_issuer_kind`
/// (`Some("Issuer")` => namespaced, sinon `ClusterIssuer`). `pub` : réutilisé par
/// `sandbox::build_sandbox_ingress` (même mécanisme TLS, un Ingress = un Certificate).
pub fn cert_manager_annotation_key(tls_issuer_kind: Option<&str>) -> &'static str {
    match tls_issuer_kind {
        Some("Issuer") => "cert-manager.io/issuer",
        _ => "cert-manager.io/cluster-issuer",
    }
}

/// Nom du Secret cookie effectif : `spec.cookie_secret_ref` s'il est fourni,
/// sinon le Secret auto-généré `<application-name>-cookie`.
pub fn effective_cookie_secret_ref(app: &Application) -> String {
    app.spec
        .cookie_secret_ref
        .clone()
        .unwrap_or_else(|| cookie_secret_name(&app.name_any()))
}

/// `ownerReference` vers cette Application, pour la GC en cascade des objets
/// créés par le controller (Deployment, Service, Ingress, Secret cookie).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
fn app_owner_ref(app: &Application) -> OwnerReference {
    app.controller_owner_ref(&())
        .expect("Application a apiVersion/kind renseignés par le derive CustomResource")
}

/// `ServiceAccount` du Deployment `app` — `application-<name>`, référencé par
/// `build_application_deployment` (`spec.serviceAccountName`). Sans lui `app`
/// tourne avec le `default` du namespace, qui n'a aucun droit sur les CRDs
/// vanyline (`VNL-K8S-002: ... is forbidden`).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_service_account(app: &Application) -> ServiceAccount {
    ServiceAccount {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `Role` (namespaced, jamais `ClusterRole` — `app` ne gère que les CRs de son
/// propre namespace) à DEUX règles sur l'unique apiGroup
/// `vanyline.solidite.fr` (`app` ne modifie jamais sa propre CR) :
/// `owners` (get/list/create/delete) et `applications` (get seul). Les droits
/// sur `projects`, `sandboxes` et `serviceaccounts/token` ne sont PLUS ici :
/// ils vivent dans les namespaces d'Owner, par le `RoleBinding`
/// `vanyline-app` que le controller y crée (`namespace.sdd`, `ClusterRole`
/// `vanyline-app-owner-scope`), donc limités aux Owners que l'app sert
/// (UN OWNER = UN NAMESPACE, décision 2026-10-04).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_role(app: &Application) -> Role {
    Role {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        rules: Some(vec![
            PolicyRule {
                api_groups: Some(vec!["vanyline.solidite.fr".to_string()]),
                resources: Some(vec!["owners".to_string()]),
                verbs: vec![
                    "get".to_string(),
                    "list".to_string(),
                    "create".to_string(),
                    "delete".to_string(),
                ],
                ..Default::default()
            },
            PolicyRule {
                api_groups: Some(vec!["vanyline.solidite.fr".to_string()]),
                resources: Some(vec!["applications".to_string()]),
                verbs: vec!["get".to_string()],
                ..Default::default()
            },
        ]),
    }
}

/// `RoleBinding` liant le `ServiceAccount` `app` au `Role` ci-dessus — même nom
/// (`application-<name>`) pour les trois objets.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_role_binding(app: &Application) -> RoleBinding {
    let name = application_name(&app.name_any());
    RoleBinding {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        role_ref: RoleRef {
            api_group: Some("rbac.authorization.k8s.io".to_string()),
            kind: "Role".to_string(),
            name: name.clone(),
        },
        subjects: Some(vec![Subject {
            kind: "ServiceAccount".to_string(),
            name,
            namespace: app.namespace(),
            ..Default::default()
        }]),
    }
}

/// Construit le Deployment `app` : un seul container, image résolue
/// (`spec.image` ou `default_image`, le tag précis lu par le controller via
/// `--app-image-tag`/`APP_IMAGE_TAG`), env depuis les trois `secretRef`
/// (`secretKeyRef` par variable) + `VNL_K8S_NAMESPACE` + `OIDC_REDIRECT_URL`
/// calculé (`https://{spec.host}/auth/callback`, jamais lu du secret). Probe
/// readiness/liveness sur `GET /health`. `ownerReference` vers l'Application.
/// `replicas` = `spec.replicas` ou 1. Label `vanyline.solidite.fr/application`
/// posé sur metadata/selector/template. `serviceAccountName` = même nom que
/// `build_application_service_account` (le compte que `VnlK8sClient` utilise
/// en in-cluster pour ses propres appels K8s).
///
/// Notes d'adaptation k8s-openapi 0.28 :
/// - `DeploymentSpec.selector` sans wrapper `Option` (type `LabelSelector` direct)
/// - `DeploymentSpec.template` sans wrapper `Option` (type `PodTemplateSpec` direct)
/// - `PodTemplateSpec.metadata` optionnel (type `Option<ObjectMeta>`)
// tooling.sdd D4 — table des env contractée application.sdd §"Construire
// @build_application_deployment" (container unique `app`, image effective, secrets
// refs) — la taille suit la spec, la scinder diluerait le motif unique du builder.
#[allow(clippy::too_many_lines)]
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_deployment(app: &Application, default_image: &str) -> Deployment {
    let mut labels = BTreeMap::new();
    labels.insert(APP_LABEL.to_string(), app.name_any());

    let mut env = vec![
        EnvVar {
            name: "OIDC_REDIRECT_URL".to_string(),
            value: Some(format!("https://{}/auth/callback", app.spec.host)),
            ..Default::default()
        },
        EnvVar {
            name: "VNL_K8S_NAMESPACE".to_string(),
            value: Some(app.namespace().unwrap_or_else(|| "default".to_string())),
            ..Default::default()
        },
        // Nom de cette CR Application elle-même — `app` s'en sert pour poser
        // `applicationRef` sur les Owner qu'il crée lazily (ensure_owner),
        // condition nécessaire à l'exposition publique des Sandboxes
        // (`{sandbox}.sandboxes.{application.host}`, cf. ws_ticket).
        EnvVar {
            name: "VNL_APPLICATION_NAME".to_string(),
            value: Some(app.name_any()),
            ..Default::default()
        },
    ];
    // Défauts de stockage (storageClass/accessMode) posés en env, seulement
    // si présents — `app` les lit pour ses créations lazily d'Owner/Project
    // (jamais lus par ce reconciler lui-même).
    if let Some(defaults) = &app.spec.storage_defaults {
        for (value, name) in [
            (
                &defaults.home_storage_class,
                "VNL_DEFAULT_HOME_STORAGE_CLASS",
            ),
            (&defaults.home_access_mode, "VNL_DEFAULT_HOME_ACCESS_MODE"),
            (
                &defaults.project_storage_class,
                "VNL_DEFAULT_PROJECT_STORAGE_CLASS",
            ),
            (
                &defaults.project_access_mode,
                "VNL_DEFAULT_PROJECT_ACCESS_MODE",
            ),
        ] {
            if let Some(value) = value {
                env.push(EnvVar {
                    name: name.to_string(),
                    value: Some(value.clone()),
                    ..Default::default()
                });
            }
        }
    }
    for (key, name) in [
        ("issuerUrl", "OIDC_ISSUER_URL"),
        ("clientId", "OIDC_CLIENT_ID"),
        ("clientSecret", "OIDC_CLIENT_SECRET"),
        ("scopes", "OIDC_SCOPES"),
    ] {
        env.push(EnvVar {
            name: name.to_string(),
            value_from: Some(EnvVarSource {
                secret_key_ref: Some(SecretKeySelector {
                    name: app.spec.oidc_secret_ref.clone(),
                    key: key.to_string(),
                    optional: None,
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
    }
    // OIDC_CA_CERT : optionnel — si la clé `caCert` manque dans le secret,
    // l'env n'est pas posée et `app` démarre quand même (`env::var(...).ok()`).
    env.push(EnvVar {
        name: "OIDC_CA_CERT".to_string(),
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: app.spec.oidc_secret_ref.clone(),
                key: "caCert".to_string(),
                optional: Some(true),
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
    env.push(EnvVar {
        name: "DATABASE_URL".to_string(),
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: app.spec.database_secret_ref.clone(),
                key: "uri".to_string(),
                optional: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
    env.push(EnvVar {
        name: "COOKIE_SECRET".to_string(),
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: effective_cookie_secret_ref(app),
                key: "cookieSecret".to_string(),
                optional: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
    // Moteur de workflow (« Moteur de workflow », S6c) : URL internes du
    // Service `<nom CR>-restate` seulement quand `spec.restate` est Some —
    // sinon l'app démarre sans moteur (aucune variable `VNL_RESTATE_*`).
    if app.spec.restate.is_some() {
        let ns_for_url = app.namespace().unwrap_or_else(|| "default".to_string());
        env.push(EnvVar {
            name: "VNL_RESTATE_ADMIN_URL".to_string(),
            value: Some(format!(
                "http://{}.{}.svc:{RESTATE_ADMIN_PORT}",
                restate_service_name(&app.name_any()),
                ns_for_url
            )),
            ..Default::default()
        });
        env.push(EnvVar {
            name: "VNL_RESTATE_INGRESS_URL".to_string(),
            value: Some(restate_ingress_url(&app.name_any(), &ns_for_url)),
            ..Default::default()
        });
        // Endpoint de services : URL que Restate utilise pour appeler l'app
        // (Service `application-<nom>`, port `workflow`).
        env.push(EnvVar {
            name: "VNL_WORKFLOW_DEPLOYMENT_URL".to_string(),
            value: Some(format!(
                "http://{}.{}.svc:{WORKFLOW_PORT}",
                application_name(&app.name_any()),
                ns_for_url
            )),
            ..Default::default()
        });
    }

    // Port `http` en tête (inchangé) ; port `workflow` seulement avec `spec.restate`.
    let mut container_ports = vec![ContainerPort {
        container_port: APP_PORT,
        name: Some("http".to_string()),
        ..Default::default()
    }];
    if app.spec.restate.is_some() {
        container_ports.push(ContainerPort {
            container_port: WORKFLOW_PORT,
            name: Some("workflow".to_string()),
            ..Default::default()
        });
    }

    let probe = Probe {
        http_get: Some(HTTPGetAction {
            path: Some("/health".to_string()),
            port: IntOrString::Int(APP_PORT),
            ..Default::default()
        }),
        initial_delay_seconds: Some(5),
        period_seconds: Some(10),
        ..Default::default()
    };

    // Note : k8s-openapi 0.28 (v1_36) avec feature "latest"
    // - DeploymentSpec.selector : LabelSelector (pas Option)
    // - DeploymentSpec.template : PodTemplateSpec (pas Option)
    Deployment {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            labels: Some(labels.clone()),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas: Some(app.spec.replicas.unwrap_or(1)),
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            // PodTemplateSpec a metadata (Option<ObjectMeta>) et spec (Option<PodSpec>)
            // tous deux spécifiés ici — pas de ..Default::default()
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    service_account_name: Some(application_name(&app.name_any())),
                    containers: vec![Container {
                        name: "app".to_string(),
                        image: Some(
                            app.spec
                                .image
                                .clone()
                                .unwrap_or_else(|| default_image.to_string()),
                        ),
                        ports: Some(container_ports),
                        env: Some(env),
                        readiness_probe: Some(probe.clone()),
                        liveness_probe: Some(probe),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        status: None,
    }
}

/// Service `ClusterIP` exposant le port HTTP du Deployment `app` (sélecteur =
/// `vanyline.solidite.fr/application: <name>`, seul label garanti unique posé
/// par `build_application_deployment`). `ownerReference` vers l'Application.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_service(app: &Application) -> Service {
    let mut selector = BTreeMap::new();
    selector.insert(APP_LABEL.to_string(), app.name_any());

    // Port `http` en tête (inchangé) ; port `workflow` seulement avec `spec.restate`.
    let mut ports = vec![ServicePort {
        name: Some("http".to_string()),
        port: APP_PORT,
        target_port: Some(IntOrString::Int(APP_PORT)),
        ..Default::default()
    }];
    if app.spec.restate.is_some() {
        ports.push(ServicePort {
            name: Some("workflow".to_string()),
            port: WORKFLOW_PORT,
            target_port: Some(IntOrString::Int(WORKFLOW_PORT)),
            ..Default::default()
        });
    }

    Service {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(selector),
            ports: Some(ports),
            ..Default::default()
        }),
        status: None,
    }
}

/// Ingress : host `spec.host`, `ingressClassName: spec.ingress_class_name`,
/// annotations `spec.ingress_annotations` fusionnées avec l'annotation
/// cert-manager dérivée de `spec.tls_issuer_name`/`spec.tls_issuer_kind`
/// (`cert-manager.io/cluster-issuer` ou `cert-manager.io/issuer`), bloc `tls`
/// (host `spec.host`, secret `<application-name>-cert`) — on part du principe
/// que cert-manager est présent, même convention que les autres Ingress du
/// cluster. Backend = le Service `application-<name>` port 8080, path `/`
/// Prefix. `ownerReference` vers l'Application.
///
/// Notes d'adaptation k8s-openapi 0.28 :
/// - `HTTPIngressRuleValue.paths` : `Vec<HTTPIngressPath>` (pas Option)
/// - `HTTPIngressPath.path_type` : `String` (pas Option)
/// - `HTTPIngressPath.backend` : `IngressBackend` (pas Option)
/// - `IngressServiceBackend.name` : `String` (pas Option)
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_ingress(app: &Application) -> Ingress {
    let mut annotations = app.spec.ingress_annotations.clone();
    annotations.insert(
        cert_manager_annotation_key(app.spec.tls_issuer_kind.as_deref()).to_string(),
        app.spec.tls_issuer_name.clone(),
    );
    let tls_secret = tls_secret_name(&application_name(&app.name_any()));

    Ingress {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            annotations: Some(annotations),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(IngressSpec {
            ingress_class_name: Some(app.spec.ingress_class_name.clone()),
            tls: Some(vec![IngressTLS {
                hosts: Some(vec![app.spec.host.clone()]),
                secret_name: Some(tls_secret),
            }]),
            rules: Some(vec![IngressRule {
                host: Some(app.spec.host.clone()),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        path: Some("/".to_string()),
                        path_type: "Prefix".to_string(),
                        backend: IngressBackend {
                            service: Some(IngressServiceBackend {
                                name: application_name(&app.name_any()),
                                port: Some(ServiceBackendPort {
                                    name: None,
                                    number: Some(APP_PORT),
                                }),
                            }),
                            ..Default::default()
                        },
                    }],
                }),
            }]),
            ..Default::default()
        }),
        status: None,
    }
}

/// Secret cookie auto-généré (`<application-name>-cookie`, clé `cookieSecret`).
/// `cookie_data_value` est la valeur à poser dans `data["cookieSecret"]` (déjà
/// encodée par l'appelant — voir décision double-encodage au contexte). Ce
/// builder est pur : il ne génère rien, il pose la valeur reçue.
/// `ownerReference` vers l'Application.
///
/// Note d'adaptation k8s-openapi 0.28 : `Secret.data` est
/// `Option<BTreeMap<String, ByteString>>` — les valeurs sont encodées en
/// base64 lors de la sérialisation, mais le stockage interne (`ByteString`)
/// utilise un `Vec<u8>`. Le builder prend une `String` et la convertit en bytes.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_cookie_secret(app: &Application, cookie_data_value: String) -> Secret {
    let mut data = BTreeMap::new();
    data.insert(
        "cookieSecret".to_string(),
        ByteString(cookie_data_value.into_bytes()),
    );

    Secret {
        metadata: ObjectMeta {
            name: Some(cookie_secret_name(&app.name_any())),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    }
}

/// `StatefulSet` Restate de l'Application (`<nom CR>-restate`, UN réplica) —
/// sous-ressource contractée par `application.sdd` « Moteur de workflow »,
/// créée seulement quand `spec.restate` est présent (le gate est à l'appelant,
/// `reconcile`). OBLIGATION S6b : le label `vanyline.solidite.fr/restate:
/// <nom CR nu>` (`RESTATE_LABEL` de `./sandbox.rs`) est posé sur le POD
/// TEMPLATE (et le selector) — cible des règles egress des sandboxes et de la
/// `NetworkPolicy` ci-dessous ; sans lui les règles ne matchent rien.
/// Image : `spec.restate.image` sinon `default_image` (flag `--restate-image`
/// / env `RESTATE_IMAGE` du controller). `volumeClaimTemplate` « data » :
/// taille `spec.restate.storageSize` (défaut `"5Gi"`), `storageClass`
/// optionnelle (classe par défaut du cluster).
// tooling.sdd D4 — application.sdd §« Moteur de workflow » : le StatefulSet
// Restate est contracté d'un seul tenant (labels/ports/env/volumeClaimTemplate
// du pod unique) — la scinder diluerait le motif de construction.
#[allow(clippy::too_many_lines)]
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_restate_statefulset(app: &Application, default_image: &str) -> StatefulSet {
    let name = restate_service_name(&app.name_any());
    let restate = app.spec.restate.clone().unwrap_or_default();
    let image = restate.image.unwrap_or_else(|| default_image.to_string());
    let storage_size = restate
        .storage_size
        .unwrap_or_else(|| RESTATE_DEFAULT_STORAGE_SIZE.to_string());

    let mut labels = BTreeMap::new();
    labels.insert(RESTATE_LABEL.to_string(), app.name_any());

    let mut requests = BTreeMap::new();
    requests.insert("storage".to_string(), Quantity(storage_size));

    StatefulSet {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(StatefulSetSpec {
            replicas: Some(1),
            service_name: Some(name.clone()),
            // Sort du PVC contracté 2026-10-14 (application.sdd « Moteur de
            // workflow ») : Retain explicite aux deux occasions — le défaut
            // K8s est Delete ; sans pose explicite, le retrait de
            // `spec.restate` detruirait le journal des runs Restate. Purge
            // manuelle assumée.
            persistent_volume_claim_retention_policy: Some(
                StatefulSetPersistentVolumeClaimRetentionPolicy {
                    when_deleted: Some("Retain".to_string()),
                    when_scaled: Some("Retain".to_string()),
                },
            ),
            selector: LabelSelector {
                match_labels: Some(labels.clone()),
                ..Default::default()
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![Container {
                        name: "restate".to_string(),
                        image: Some(image),
                        ports: Some(vec![
                            ContainerPort {
                                name: Some("ingress".to_string()),
                                container_port: RESTATE_INGRESS_PORT,
                                ..Default::default()
                            },
                            ContainerPort {
                                name: Some("admin".to_string()),
                                container_port: RESTATE_ADMIN_PORT,
                                ..Default::default()
                            },
                            ContainerPort {
                                name: Some("worker".to_string()),
                                container_port: RESTATE_WORKER_PORT,
                                ..Default::default()
                            },
                        ]),
                        env: Some(vec![
                            EnvVar {
                                name: "RESTATE_CLUSTER_NAME".to_string(),
                                value: Some(app.name_any()),
                                ..Default::default()
                            },
                            EnvVar {
                                name: "RESTATE_BOOTSTRAP_CLUSTER".to_string(),
                                value: Some("true".to_string()),
                                ..Default::default()
                            },
                            EnvVar {
                                name: "RESTATE_INGRESS_SOCKET".to_string(),
                                value: Some(format!("0.0.0.0:{RESTATE_INGRESS_PORT}")),
                                ..Default::default()
                            },
                            EnvVar {
                                name: "RESTATE_ADMIN_SOCKET".to_string(),
                                value: Some(format!("0.0.0.0:{RESTATE_ADMIN_PORT}")),
                                ..Default::default()
                            },
                            EnvVar {
                                name: "RESTATE_WORKER_SOCKET".to_string(),
                                value: Some(format!("0.0.0.0:{RESTATE_WORKER_PORT}")),
                                ..Default::default()
                            },
                            EnvVar {
                                name: "RESTATE_DATA_DIR".to_string(),
                                value: Some(RESTATE_DATA_DIR.to_string()),
                                ..Default::default()
                            },
                        ]),
                        volume_mounts: Some(vec![VolumeMount {
                            name: "data".to_string(),
                            mount_path: RESTATE_DATA_DIR.to_string(),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            volume_claim_templates: Some(vec![PersistentVolumeClaim {
                metadata: ObjectMeta {
                    name: Some("data".to_string()),
                    ..Default::default()
                },
                spec: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_string()]),
                    storage_class_name: restate.storage_class,
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(requests),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        status: None,
    }
}

/// Service Restate de l'Application : `<nom CR>-restate`
/// (`restate_service_name` de `./sandbox.rs`, PAS `application_name`),
/// sélecteur `{RESTATE_LABEL: <nom CR nu>}`, trois ports — ingress 8080,
/// admin 9070, port de nœud (worker) 5122. Type non posé (`ClusterIP`) : ce
/// Service n'est JAMAIS exposé par un Ingress (`application.sdd` « Moteur de
/// workflow »).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_restate_service(app: &Application) -> Service {
    let mut selector = BTreeMap::new();
    selector.insert(RESTATE_LABEL.to_string(), app.name_any());

    let port = |name: &str, number: i32| ServicePort {
        name: Some(name.to_string()),
        port: number,
        target_port: Some(IntOrString::Int(number)),
        ..Default::default()
    };

    Service {
        metadata: ObjectMeta {
            name: Some(restate_service_name(&app.name_any())),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(selector),
            ports: Some(vec![
                port("ingress", RESTATE_INGRESS_PORT),
                port("admin", RESTATE_ADMIN_PORT),
                port("worker", RESTATE_WORKER_PORT),
            ]),
            ..Default::default()
        }),
        status: None,
    }
}

/// `NetworkPolicy` du namespace d'application ciblant les pods Restate de
/// cette Application (label `RESTATE_LABEL`) — tâche S6c : l'ingress (8080) et
/// l'admin (9070) de Restate sont joignables par le pod `app` (pair même
/// namespace, label `APP_LABEL`) ; les pods sandbox des namespaces d'Owner
/// (labels `APP_NAMESPACE_LABEL` sur le Namespace,
/// `vanyline.solidite.fr/sandbox` sur le pod) n'appellent QUE l'ingress 8080 —
/// callback d'awakeable des jobs (`../../sandbox/src/jobs.sdd`), miroir de la
/// règle egress livrée S6b (`sandbox.rs::restate_egress_rule`). La netpol
/// sélectionnant les pods restate, tout autre trafic ingress vers eux est
/// fermé par le sémantisme même des `NetworkPolicy` K8s.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_restate_netpol(app: &Application) -> NetworkPolicy {
    let mut pod_selector_labels = BTreeMap::new();
    pod_selector_labels.insert(RESTATE_LABEL.to_string(), app.name_any());

    // Pair app : pod `app` du même namespace (le Service/le pod vivent dans le
    // namespace d'application) — ingress + admin.
    let app_peer = NetworkPolicyPeer {
        pod_selector: Some(LabelSelector {
            match_labels: Some(BTreeMap::from([(APP_LABEL.to_string(), app.name_any())])),
            ..Default::default()
        }),
        ..Default::default()
    };
    // Pair sandbox : tous les namespaces d'Owner de CETTE application
    // (`APP_NAMESPACE_LABEL` posé par `namespace.rs`), pods portant le label
    // sandbox — ingress seule.
    let sandbox_peer = NetworkPolicyPeer {
        namespace_selector: Some(LabelSelector {
            match_labels: Some(BTreeMap::from([(
                APP_NAMESPACE_LABEL.to_string(),
                app.namespace().unwrap_or_else(|| "default".to_string()),
            )])),
            ..Default::default()
        }),
        pod_selector: Some(LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![LabelSelectorRequirement {
                key: "vanyline.solidite.fr/sandbox".to_string(),
                operator: "Exists".to_string(),
                values: None,
            }]),
        }),
        ..Default::default()
    };

    let port = |number: i32| NetworkPolicyPort {
        port: Some(IntOrString::Int(number)),
        protocol: Some("TCP".to_string()),
        end_port: None,
    };

    NetworkPolicy {
        metadata: ObjectMeta {
            name: Some(format!("{}-netpol", restate_service_name(&app.name_any()))),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(NetworkPolicySpec {
            pod_selector: Some(LabelSelector {
                match_labels: Some(pod_selector_labels),
                ..Default::default()
            }),
            policy_types: Some(vec!["Ingress".to_string()]),
            ingress: Some(vec![
                NetworkPolicyIngressRule {
                    from: Some(vec![app_peer]),
                    ports: Some(vec![port(RESTATE_INGRESS_PORT), port(RESTATE_ADMIN_PORT)]),
                },
                NetworkPolicyIngressRule {
                    from: Some(vec![sandbox_peer]),
                    ports: Some(vec![port(RESTATE_INGRESS_PORT)]),
                },
            ]),
            ..Default::default()
        }),
    }
}

pub struct Context {
    pub client: Client,
    /// Image `app` par défaut (`--app-image-tag`/`APP_IMAGE_TAG` du
    /// controller), utilisée quand `Application.spec.image` est `None`.
    pub default_image: String,
    /// Image Restate par défaut (`--restate-image`/`RESTATE_IMAGE` du
    /// controller), utilisée quand `Application.spec.restate.image` est `None`
    /// (« Moteur de workflow », `application.sdd`).
    pub restate_image: String,
}

/// Génère la valeur de `data["cookieSecret"]` pour le Secret cookie auto-généré :
/// 64 octets aléatoires encodés en base64 standard (encodage simple). Voir la
/// décision "Encodage du cookie" au contexte : `build_cookie_secret` enveloppe
/// cette valeur dans un `ByteString` que la sérialisation K8s encode une seconde
/// fois — l'env `COOKIE_SECRET` du pod sera `base64(raw)`.
fn generate_cookie_secret_value() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Garantit le Secret cookie auto-généré quand `spec.cookie_secret_ref` est `None` :
/// cherche `<application-name>-cookie`, le crée s'il est absent, le laisse tel quel
/// s'il existe (jamais régénéré — une régénération invaliderait les sessions cookie
/// actives ; le check "existe déjà ?" précède toute écriture). Quand
/// `spec.cookie_secret_ref` est `Some(...)`, ne fait rien (secret fourni par ailleurs).
async fn ensure_cookie_secret(
    app: &Application,
    ctx: &Context,
    ns: &str,
) -> Result<(), ControllerError> {
    if app.spec.cookie_secret_ref.is_some() {
        return Ok(());
    }
    let secrets: Api<Secret> = Api::namespaced(ctx.client.clone(), ns);
    let name = cookie_secret_name(&app.name_any());
    if secrets.get_opt(&name).await?.is_some() {
        return Ok(());
    }
    let value = generate_cookie_secret_value();
    let secret = build_cookie_secret(app, value);
    secrets.create(&PostParams::default(), &secret).await?;
    Ok(())
}

/// Mappe les conditions `Available` d'un Deployment sur une phase CRD :
/// `"Running"` si une condition `Available/True` existe, `"Failed"` si une
/// condition `Available/False` a `reason == "Failed"` ou
/// `"ProgressDeadlineExceeded"`, sinon `"Provisioning"`. `None`/vide =>
/// `"Provisioning"`.
///
/// Note : k8s-openapi 0.28 utilise `DeploymentCondition` (apps/v1) pour
/// `DeploymentStatus.conditions`, avec les mêmes champs `type_`/`status`/`reason`.
fn deployment_phase(conditions: Option<&Vec<DeploymentCondition>>) -> String {
    let available = conditions.is_some_and(|cs| {
        cs.iter()
            .any(|c| c.type_ == "Available" && c.status == "True")
    });
    let failed = conditions.is_some_and(|cs| {
        cs.iter().any(|c| {
            c.type_ == "Available"
                && c.status == "False"
                && (c.reason.as_deref() == Some("Failed")
                    || c.reason.as_deref() == Some("ProgressDeadlineExceeded"))
        })
    });

    if available {
        "Running".to_string()
    } else if failed {
        "Failed".to_string()
    } else {
        "Provisioning".to_string()
    }
}

/// Status attendu : `phase`, condition `Ready` reflétant `phase == "Running"`
/// (`True`/`"DeploymentReady"`, sinon `False`/`"NotReady"`).
pub fn compute_status(app: &Application, phase: &str) -> ApplicationStatus {
    let (status, reason) = if phase == "Running" {
        ("True", "DeploymentReady")
    } else {
        ("False", "NotReady")
    };
    ApplicationStatus {
        phase: Some(phase.to_string()),
        conditions: vec![Condition {
            type_: "Ready".to_string(),
            status: status.to_string(),
            reason: reason.to_string(),
            message: format!("phase={phase}"),
            last_transition_time: k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            ),
            observed_generation: app.meta().generation,
        }],
    }
}

/// Reconciler kube-rs : garantit le Secret cookie auto-généré (si
/// `spec.cookie_secret_ref` est `None`), le ServiceAccount/Role/RoleBinding
/// d'`app` (droits sur les CRs owners/projects/sandboxes/applications), puis
/// server-side apply idempotent du Deployment, du Service et de l'Ingress,
/// puis status. Pas de finalizer — la GC K8s (ownerReferences) nettoie les
/// objets créés à la suppression de l'Application.
pub async fn reconcile(
    app: Arc<Application>,
    ctx: Arc<Context>,
) -> Result<Action, ControllerError> {
    let ns = app.namespace().unwrap_or_else(|| "default".to_string());
    let pp = PatchParams::apply(FIELD_MANAGER).force();

    ensure_cookie_secret(&app, &ctx, &ns).await?;

    let service_accounts: Api<ServiceAccount> = Api::namespaced(ctx.client.clone(), &ns);
    let service_account = build_application_service_account(&app);
    service_accounts
        .patch(
            &application_name(&app.name_any()),
            &pp,
            &Patch::Apply(&service_account),
        )
        .await?;

    let roles: Api<Role> = Api::namespaced(ctx.client.clone(), &ns);
    let role = build_application_role(&app);
    roles
        .patch(
            &application_name(&app.name_any()),
            &pp,
            &Patch::Apply(&role),
        )
        .await?;

    let role_bindings: Api<RoleBinding> = Api::namespaced(ctx.client.clone(), &ns);
    let role_binding = build_application_role_binding(&app);
    role_bindings
        .patch(
            &application_name(&app.name_any()),
            &pp,
            &Patch::Apply(&role_binding),
        )
        .await?;

    let deployments: Api<Deployment> = Api::namespaced(ctx.client.clone(), &ns);
    let name = application_name(&app.name_any());
    let phase = match deployments.get_opt(&name).await? {
        None => "Provisioning".to_string(),
        Some(d) => deployment_phase(d.status.as_ref().and_then(|s| s.conditions.as_ref())),
    };

    let deployment = build_application_deployment(&app, &ctx.default_image);
    deployments
        .patch(&name, &pp, &Patch::Apply(&deployment))
        .await?;

    let services: Api<Service> = Api::namespaced(ctx.client.clone(), &ns);
    let service = build_application_service(&app);
    services.patch(&name, &pp, &Patch::Apply(&service)).await?;

    let ingresses: Api<Ingress> = Api::namespaced(ctx.client.clone(), &ns);
    let ingress = build_application_ingress(&app);
    ingresses.patch(&name, &pp, &Patch::Apply(&ingress)).await?;

    // Moteur de workflow (« Moteur de workflow », application.sdd) : quand
    // `spec.restate` est présent, le StatefulSet, le Service et la
    // NetworkPolicy Restate sont appliqués comme sous-ressources de
    // l'Application ; sans `spec.restate`, aucune de ces ressources n'existe
    // (suppression conditionnelle, même patron que la netpol egress de
    // sandbox.rs).
    let restate_name = restate_service_name(&app.name_any());
    let restate_netpol_name = format!("{restate_name}-netpol");
    let stateful_sets: Api<StatefulSet> = Api::namespaced(ctx.client.clone(), &ns);
    let netpols: Api<NetworkPolicy> = Api::namespaced(ctx.client.clone(), &ns);
    if app.spec.restate.is_some() {
        let sts = build_restate_statefulset(&app, &ctx.restate_image);
        stateful_sets
            .patch(&restate_name, &pp, &Patch::Apply(&sts))
            .await?;
        let restate_service = build_restate_service(&app);
        services
            .patch(&restate_name, &pp, &Patch::Apply(&restate_service))
            .await?;
        let restate_netpol = build_restate_netpol(&app);
        netpols
            .patch(&restate_netpol_name, &pp, &Patch::Apply(&restate_netpol))
            .await?;
    } else {
        if stateful_sets.get_opt(&restate_name).await?.is_some() {
            stateful_sets
                .delete(&restate_name, &kube::api::DeleteParams::default())
                .await?;
        }
        if services.get_opt(&restate_name).await?.is_some() {
            services
                .delete(&restate_name, &kube::api::DeleteParams::default())
                .await?;
        }
        if netpols.get_opt(&restate_netpol_name).await?.is_some() {
            netpols
                .delete(&restate_netpol_name, &kube::api::DeleteParams::default())
                .await?;
        }
    }

    let applications: Api<Application> = Api::namespaced(ctx.client.clone(), &ns);
    let status = compute_status(&app, &phase);
    let patch = serde_json::json!({ "status": status });
    applications
        .patch_status(
            &app.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(Action::requeue(Duration::from_secs(
        if phase == "Running" { 300 } else { 15 },
    )))
}

/// Politique d'erreur : requeue à 30s, quelle que soit l'erreur (patron
/// `owner::error_policy` — pas de distinction transitoire/fatale en v1).
pub fn error_policy(_app: Arc<Application>, error: &ControllerError, _ctx: Arc<Context>) -> Action {
    tracing::warn!(%error, "application reconcile error, requeue in 30s");
    Action::requeue(Duration::from_secs(30))
}

/// Construit le `Controller<Application>` prêt à tourner (`.run(...)` reste à
/// l'appelant dans `main.rs`, qui pilote aussi l'arrêt).
pub fn build_controller(client: Client) -> Controller<Application> {
    let applications: Api<Application> = Api::all(client);
    Controller::new(applications, kube::runtime::watcher::Config::default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use vanyline_crds::ApplicationSpec;

    const TEST_APP_IMAGE: &str = "ghcr.io/sebt3/vanyline-app:test";

    fn make_application(name: &str) -> Application {
        let mut app = Application::new(
            name,
            ApplicationSpec {
                image: None,
                replicas: None,
                oidc_secret_ref: "oidc-secret".to_string(),
                database_secret_ref: "db-secret".to_string(),
                cookie_secret_ref: None,
                host: "app.example.com".to_string(),
                ingress_class_name: "nginx".to_string(),
                tls_issuer_name: "self-sign".to_string(),
                tls_issuer_kind: None,
                ingress_annotations: BTreeMap::new(),
                ingress_controller: None,
                storage_defaults: None,
                restate: None,
            },
        );
        app.meta_mut().namespace = Some("ns".into());
        app.meta_mut().uid = Some(format!("test-uid-{name}"));
        app
    }

    // 1. application_name_and_cookie_secret_name
    #[test]
    fn application_name_and_cookie_secret_name() {
        assert_eq!(application_name("demo"), "application-demo");
        assert_eq!(cookie_secret_name("demo"), "demo-cookie");
    }

    // 2. effective_cookie_secret_ref_some
    #[test]
    fn effective_cookie_secret_ref_some() {
        let mut app = make_application("demo");
        app.spec.cookie_secret_ref = Some("custom-cookie".to_string());
        assert_eq!(effective_cookie_secret_ref(&app), "custom-cookie");
    }

    // 3. effective_cookie_secret_ref_none
    #[test]
    fn effective_cookie_secret_ref_none() {
        let app = make_application("demo");
        assert_eq!(effective_cookie_secret_ref(&app), "demo-cookie");
    }

    // 4. build_application_deployment_shape
    #[test]
    fn build_application_deployment_shape() {
        let app = make_application("demo");
        let deployment = build_application_deployment(&app, TEST_APP_IMAGE);

        assert_eq!(
            deployment.metadata.name,
            Some("application-demo".to_string())
        );
        assert_eq!(deployment.metadata.namespace, Some("ns".to_string()));

        let refs = deployment
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo");
        assert_eq!(refs[0].kind, "Application");

        assert_eq!(deployment.spec.as_ref().unwrap().replicas, Some(1));

        let pod_spec = deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap();
        assert_eq!(
            pod_spec.service_account_name,
            Some("application-demo".to_string())
        );

        let container = pod_spec
            .containers
            .first()
            .expect("should have 1 container");
        assert_eq!(container.name, "app");

        let ports = container.ports.as_ref().expect("should have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].container_port, APP_PORT);
        assert_eq!(ports[0].name, Some("http".to_string()));

        let rp = container
            .readiness_probe
            .as_ref()
            .expect("should have readiness probe");
        let lg = container
            .liveness_probe
            .as_ref()
            .expect("should have liveness probe");
        assert_eq!(
            rp.http_get.as_ref().unwrap().path,
            Some("/health".to_string())
        );
        assert_eq!(
            lg.http_get.as_ref().unwrap().path,
            Some("/health".to_string())
        );
        assert_eq!(
            rp.http_get.as_ref().unwrap().port,
            IntOrString::Int(APP_PORT)
        );
        assert_eq!(
            lg.http_get.as_ref().unwrap().port,
            IntOrString::Int(APP_PORT)
        );
    }

    // 5. build_application_deployment_replicas_and_image
    #[test]
    fn build_application_deployment_replicas_and_image() {
        // replicas = Some(3)
        let mut app = make_application("demo");
        app.spec.replicas = Some(3);
        let dep = build_application_deployment(&app, TEST_APP_IMAGE);
        assert_eq!(dep.spec.as_ref().unwrap().replicas, Some(3));

        // image = None -> default
        let mut app = make_application("demo");
        app.spec.image = None;
        let dep = build_application_deployment(&app, TEST_APP_IMAGE);
        let container = dep
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .unwrap();
        assert_eq!(container.image, Some(TEST_APP_IMAGE.to_string()));

        // image = Some("custom:tag")
        let mut app = make_application("demo");
        app.spec.image = Some("custom:tag".to_string());
        let dep = build_application_deployment(&app, TEST_APP_IMAGE);
        let container = dep
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .unwrap();
        assert_eq!(container.image, Some("custom:tag".to_string()));
    }

    // 6. build_application_deployment_env
    // tooling.sdd D4 — application.sdd §Scenario "build_application_deployment_env" :
    // le test énumère les 10 env contractées une par une (un bloc par variable) ;
    // la taille suit la table des env de la spec.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn build_application_deployment_env() {
        let app = make_application("demo");
        let deployment = build_application_deployment(&app, TEST_APP_IMAGE);
        let container = deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .expect("should have 1 container");
        let env = container.env.as_ref().expect("should have env");
        assert_eq!(
            env.len(),
            10,
            "expected 10 environment variables, got {}",
            env.len()
        );

        let find = |name: &str| {
            env.iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("should have env var {name}"))
        };

        // OIDC_REDIRECT_URL
        let oidc_redir = find("OIDC_REDIRECT_URL");
        assert_eq!(
            oidc_redir.value,
            Some("https://app.example.com/auth/callback".to_string())
        );
        assert!(oidc_redir.value_from.is_none());

        // VNL_K8S_NAMESPACE
        let vnl_ns = find("VNL_K8S_NAMESPACE");
        assert_eq!(vnl_ns.value, Some("ns".to_string()));
        assert!(vnl_ns.value_from.is_none());

        // VNL_APPLICATION_NAME
        let vnl_app_name = find("VNL_APPLICATION_NAME");
        assert_eq!(vnl_app_name.value, Some("demo".to_string()));
        assert!(vnl_app_name.value_from.is_none());

        // OIDC_ISSUER_URL (name=issuerUrl)
        let oidc_issuer = find("OIDC_ISSUER_URL");
        assert!(oidc_issuer.value.is_none());
        let skr = oidc_issuer
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.name, "oidc-secret");
        assert_eq!(skr.key, "issuerUrl");
        assert!(skr.optional.is_none());

        // OIDC_CLIENT_ID
        let oidc_id = find("OIDC_CLIENT_ID");
        let skr = oidc_id
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.name, "oidc-secret");
        assert_eq!(skr.key, "clientId");
        assert!(skr.optional.is_none());

        // OIDC_CLIENT_SECRET
        let oidc_sec = find("OIDC_CLIENT_SECRET");
        let skr = oidc_sec
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.name, "oidc-secret");
        assert_eq!(skr.key, "clientSecret");
        assert!(skr.optional.is_none());

        // OIDC_SCOPES
        let oidc_scopes = find("OIDC_SCOPES");
        let skr = oidc_scopes
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.name, "oidc-secret");
        assert_eq!(skr.key, "scopes");
        assert!(skr.optional.is_none());

        // OIDC_CA_CERT (optional: true)
        let ca_cert = find("OIDC_CA_CERT");
        let skr = ca_cert
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.key, "caCert");
        assert_eq!(skr.optional, Some(true));

        // DATABASE_URL
        let db_url = find("DATABASE_URL");
        let skr = db_url
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.name, "db-secret");
        assert_eq!(skr.key, "uri");

        // COOKIE_SECRET (auto-generated cookie secret)
        let cookie = find("COOKIE_SECRET");
        let skr = cookie
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(skr.name, "demo-cookie");
        assert_eq!(skr.key, "cookieSecret");
    }

    #[test]
    fn build_application_deployment_storage_defaults_env() {
        let mut app = make_application("demo");
        app.spec.storage_defaults = Some(vanyline_crds::ApplicationStorageDefaults {
            home_storage_class: Some("local-path".to_string()),
            home_access_mode: Some("ReadWriteOnce".to_string()),
            project_storage_class: None,
            project_access_mode: Some("ReadWriteOnce".to_string()),
        });
        let deployment = build_application_deployment(&app, TEST_APP_IMAGE);
        let container = deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .expect("should have 1 container");
        let env = container.env.as_ref().expect("should have env");

        let find = |name: &str| env.iter().find(|e| e.name == name);

        assert_eq!(
            find("VNL_DEFAULT_HOME_STORAGE_CLASS")
                .expect("should have VNL_DEFAULT_HOME_STORAGE_CLASS")
                .value,
            Some("local-path".to_string())
        );
        assert_eq!(
            find("VNL_DEFAULT_HOME_ACCESS_MODE")
                .expect("should have VNL_DEFAULT_HOME_ACCESS_MODE")
                .value,
            Some("ReadWriteOnce".to_string())
        );
        assert!(
            find("VNL_DEFAULT_PROJECT_STORAGE_CLASS").is_none(),
            "should not push env var for a None field"
        );
        assert_eq!(
            find("VNL_DEFAULT_PROJECT_ACCESS_MODE")
                .expect("should have VNL_DEFAULT_PROJECT_ACCESS_MODE")
                .value,
            Some("ReadWriteOnce".to_string())
        );
    }

    // Scenario (Must « Moteur de workflow », S6c) : quand `spec.restate` est
    // `Some`, l'env du Deployment `app` reçoit `VNL_RESTATE_ADMIN_URL` et
    // `VNL_RESTATE_INGRESS_URL` (URL internes du Service `<nom CR>-restate`) ;
    // sans `spec.restate`, aucune de ces deux variables.
    #[test]
    fn build_application_deployment_env_restate_urls() {
        let container_env = |app: &Application| {
            build_application_deployment(app, TEST_APP_IMAGE)
                .spec
                .as_ref()
                .unwrap()
                .template
                .spec
                .as_ref()
                .unwrap()
                .containers
                .first()
                .expect("should have 1 container")
                .env
                .as_ref()
                .expect("should have env")
                .clone()
        };

        // restate Some ⇒ 13 variables : 10 de base + 2 URL restate + l'URL de
        // déploiement workflow (workflow_port_et_url_poses_avec_restate).
        let mut app = make_application("demo");
        app.spec.restate = Some(vanyline_crds::RestateSpec {
            image: None,
            storage_size: None,
            storage_class: None,
        });
        let env = container_env(&app);
        assert_eq!(
            env.len(),
            13,
            "10 variables de base + 2 URL restate + 1 URL workflow, got {}",
            env.len()
        );
        let find = |name: &str| env.iter().find(|e| e.name == name);
        let ingress = find("VNL_RESTATE_INGRESS_URL").expect("should have VNL_RESTATE_INGRESS_URL");
        assert_eq!(
            ingress.value,
            Some("http://demo-restate.ns.svc:8080".to_string())
        );
        assert!(ingress.value_from.is_none());
        let admin = find("VNL_RESTATE_ADMIN_URL").expect("should have VNL_RESTATE_ADMIN_URL");
        assert_eq!(
            admin.value,
            Some("http://demo-restate.ns.svc:9070".to_string())
        );
        assert!(admin.value_from.is_none());

        // restate None ⇒ aucune de ces deux variables (contrat 10 variables).
        let app_none = make_application("demo");
        let env_none = container_env(&app_none);
        assert_eq!(env_none.len(), 10, "sans restate : exactement 10 variables");
        assert!(
            !env_none
                .iter()
                .any(|e| e.name == "VNL_RESTATE_INGRESS_URL" || e.name == "VNL_RESTATE_ADMIN_URL"),
            "aucune URL restate sans spec.restate"
        );
    }

    // Scenario: workflow_port_et_url_poses_avec_restate
    #[test]
    fn workflow_port_et_url_poses_avec_restate() {
        let mut app = make_application("demo");
        app.spec.restate = Some(vanyline_crds::RestateSpec {
            image: None,
            storage_size: None,
            storage_class: None,
        });

        let service = build_application_service(&app);
        let ports = service
            .spec
            .as_ref()
            .unwrap()
            .ports
            .as_ref()
            .expect("should have ports");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].name, Some("http".to_string()));
        assert_eq!(ports[0].port, APP_PORT);
        assert_eq!(ports[1].name, Some("workflow".to_string()));
        assert_eq!(ports[1].port, WORKFLOW_PORT);
        assert_eq!(ports[1].target_port, Some(IntOrString::Int(WORKFLOW_PORT)));

        let deployment = build_application_deployment(&app, TEST_APP_IMAGE);
        let container = deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .expect("should have 1 container");
        let container_ports = container.ports.as_ref().expect("container ports");
        assert!(
            container_ports.iter().any(|p| p.container_port == WORKFLOW_PORT
                && p.name.as_deref() == Some("workflow")),
            "containerPort 9080 nommé workflow attendu"
        );
        let env = container.env.as_ref().expect("should have env");
        let url = env
            .iter()
            .find(|e| e.name == "VNL_WORKFLOW_DEPLOYMENT_URL")
            .expect("should have VNL_WORKFLOW_DEPLOYMENT_URL");
        assert_eq!(
            url.value,
            Some("http://application-demo.ns.svc:9080".to_string())
        );
    }

    // Scenario: workflow_port_et_url_absents_sans_restate
    #[test]
    fn workflow_port_et_url_absents_sans_restate() {
        let app = make_application("demo");

        let service = build_application_service(&app);
        let ports = service
            .spec
            .as_ref()
            .unwrap()
            .ports
            .as_ref()
            .expect("should have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].name, Some("http".to_string()));

        let deployment = build_application_deployment(&app, TEST_APP_IMAGE);
        let container = deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .expect("should have 1 container");
        let has_workflow_port = container
            .ports
            .iter()
            .flatten()
            .any(|p| p.container_port == WORKFLOW_PORT);
        assert!(!has_workflow_port, "aucun containerPort 9080 sans restate");
        let has_workflow_url = container
            .env
            .iter()
            .flatten()
            .any(|e| e.name == "VNL_WORKFLOW_DEPLOYMENT_URL");
        assert!(
            !has_workflow_url,
            "aucune VNL_WORKFLOW_DEPLOYMENT_URL sans restate"
        );
    }

    // 7. build_application_service_shape
    #[test]
    fn build_application_service_shape() {
        let app = make_application("demo");
        let service = build_application_service(&app);

        assert_eq!(service.metadata.name, Some("application-demo".to_string()));

        let selector = service.spec.as_ref().unwrap().selector.as_ref().unwrap();
        assert_eq!(selector.get(APP_LABEL), Some(&"demo".to_string()));

        let ports = service
            .spec
            .as_ref()
            .unwrap()
            .ports
            .as_ref()
            .expect("should have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, APP_PORT);
        assert_eq!(ports[0].name, Some("http".to_string()));
    }

    #[test]
    fn build_application_service_account_shape() {
        let app = make_application("demo");
        let sa = build_application_service_account(&app);

        assert_eq!(sa.metadata.name, Some("application-demo".to_string()));
        assert_eq!(sa.metadata.namespace, Some("ns".to_string()));
        let refs = sa
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo");
    }

    // 7bis. Scenario: build_application_role_shape — contrat à DEUX règles
    // (décision 2026-10-04, S6c) : l'ancien Scenario à trois règles est
    // remplacé. Les droits sur `projects`, `sandboxes` et
    // `serviceaccounts/token` ne sont PLUS ici : ils vivent dans les namespaces
    // d'Owner, par le RoleBinding `vanyline-app` que le controller y crée
    // (namespace.sdd, ClusterRole `vanyline-app-owner-scope`).
    #[test]
    fn build_application_role_shape() {
        let app = make_application("demo");
        let role = build_application_role(&app);

        assert_eq!(role.metadata.name, Some("application-demo".to_string()));
        let rules = role.rules.as_ref().expect("should have rules");
        assert_eq!(rules.len(), 2, "le Role porte exactement DEUX règles");

        for rule in rules {
            assert_eq!(
                rule.api_groups,
                Some(vec!["vanyline.solidite.fr".to_string()]),
                "unique apiGroup vanyline.solidite.fr par règle"
            );
        }

        let owners = rules
            .iter()
            .find(|r| {
                r.resources
                    .as_ref()
                    .is_some_and(|r| r.contains(&"owners".to_string()))
            })
            .expect("should have a rule for owners");
        assert_eq!(
            owners.resources,
            Some(vec!["owners".to_string()]),
            "la règle owners ne porte QUE owners (plus projects)"
        );
        assert_eq!(owners.verbs, vec!["get", "list", "create", "delete"]);

        let applications = rules
            .iter()
            .find(|r| {
                r.resources
                    .as_ref()
                    .is_some_and(|r| r.contains(&"applications".to_string()))
            })
            .expect("should have a rule for applications");
        assert_eq!(applications.verbs, vec!["get"]);

        assert!(
            rules.iter().all(|r| !r
                .resources
                .as_ref()
                .is_some_and(|res| res.contains(&"projects".to_string()))),
            "projects n'est PLUS dans le Role de l'app (namespace.sdd)"
        );
        assert!(
            rules.iter().all(|r| !r
                .resources
                .as_ref()
                .is_some_and(|res| res.contains(&"sandboxes".to_string()))),
            "sandboxes n'est PLUS dans le Role de l'app (namespace.sdd)"
        );
    }

    #[test]
    fn build_application_role_binding_shape() {
        let app = make_application("demo");
        let rb = build_application_role_binding(&app);

        assert_eq!(rb.metadata.name, Some("application-demo".to_string()));
        assert_eq!(rb.role_ref.kind, "Role");
        assert_eq!(rb.role_ref.name, "application-demo");
        assert_eq!(
            rb.role_ref.api_group,
            Some("rbac.authorization.k8s.io".to_string())
        );

        let subjects = rb.subjects.as_ref().expect("should have subjects");
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].kind, "ServiceAccount");
        assert_eq!(subjects[0].name, "application-demo");
        assert_eq!(subjects[0].namespace, Some("ns".to_string()));
    }

    // 8. build_application_ingress_shape
    #[test]
    fn build_application_ingress_shape() {
        let app = make_application("demo");
        let ingress = build_application_ingress(&app);

        assert_eq!(ingress.metadata.name, Some("application-demo".to_string()));
        assert_eq!(
            ingress.spec.as_ref().unwrap().ingress_class_name,
            Some("nginx".to_string())
        );

        let rules = ingress.spec.as_ref().unwrap().rules.as_ref().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].host.as_ref().unwrap().as_str(), "app.example.com");

        // HTTPIngressRuleValue.paths est Vec (pas Option) en k8s-openapi 0.28
        let paths = &rules[0].http.as_ref().unwrap().paths;
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].path.as_ref().unwrap().as_str(), "/");
        // path_type est String (pas Option) en k8s-openapi 0.28
        assert_eq!(paths[0].path_type.as_str(), "Prefix");

        let svc = paths[0].backend.service.as_ref().unwrap();
        assert_eq!(svc.name, "application-demo");
        assert_eq!(svc.port.as_ref().unwrap().number, Some(APP_PORT));

        let tls = ingress.spec.as_ref().unwrap().tls.as_ref().unwrap();
        assert_eq!(tls.len(), 1);
        assert_eq!(tls[0].hosts, Some(vec!["app.example.com".to_string()]));
        assert_eq!(
            tls[0].secret_name,
            Some("application-demo-cert".to_string())
        );
    }

    // 9. build_application_ingress_annotations
    #[test]
    fn build_application_ingress_annotations() {
        // tls_issuer_kind None => cert-manager.io/cluster-issuer, plus les
        // annotations libres fournies par l'utilisateur
        let mut app = make_application("demo");
        app.spec.ingress_annotations =
            BTreeMap::from([("custom.example.com/foo".to_string(), "bar".to_string())]);
        let ingress = build_application_ingress(&app);
        let ann = ingress
            .metadata
            .annotations
            .as_ref()
            .expect("should have annotations");
        assert_eq!(
            ann.get("cert-manager.io/cluster-issuer"),
            Some(&"self-sign".to_string())
        );
        assert_eq!(ann.get("custom.example.com/foo"), Some(&"bar".to_string()));

        // tls_issuer_kind Some("Issuer") => cert-manager.io/issuer (namespaced)
        let mut app = make_application("demo");
        app.spec.tls_issuer_kind = Some("Issuer".to_string());
        let ingress = build_application_ingress(&app);
        let ann = ingress
            .metadata
            .annotations
            .as_ref()
            .expect("should have annotations");
        assert_eq!(
            ann.get("cert-manager.io/issuer"),
            Some(&"self-sign".to_string())
        );
        assert!(ann.get("cert-manager.io/cluster-issuer").is_none());
    }

    // 10. build_cookie_secret_shape
    #[test]
    fn build_cookie_secret_shape() {
        let app = make_application("demo");
        let secret = build_cookie_secret(&app, "YWJj".to_string());

        assert_eq!(secret.metadata.name, Some("demo-cookie".to_string()));
        assert_eq!(secret.metadata.namespace, Some("ns".to_string()));

        let refs = secret
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo");
        assert_eq!(refs[0].kind, "Application");

        let data = secret.data.as_ref().expect("should have data");
        // ByteString stocke les bytes bruts (Vec<u8>), pas de String
        assert_eq!(data.get("cookieSecret").unwrap().0.as_slice(), b"YWJj");
    }

    // 11. deployment_phase_running
    #[test]
    fn deployment_phase_running() {
        let conditions = Some(vec![DeploymentCondition {
            type_: "Available".to_string(),
            status: "True".to_string(),
            reason: Some("MinimumReplicasAvailable".to_string()),
            message: Some("Deployment has minimum availability".to_string()),
            last_transition_time: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            )),
            last_update_time: None,
        }]);
        assert_eq!(deployment_phase(conditions.as_ref()), "Running");
    }

    // 12. deployment_phase_failed
    #[test]
    fn deployment_phase_failed() {
        // reason "Failed"
        let conditions = Some(vec![DeploymentCondition {
            type_: "Available".to_string(),
            status: "False".to_string(),
            reason: Some("Failed".to_string()),
            message: Some("replicas are failed".to_string()),
            last_transition_time: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            )),
            last_update_time: None,
        }]);
        assert_eq!(deployment_phase(conditions.as_ref()), "Failed");

        // reason "ProgressDeadlineExceeded"
        let conditions2 = Some(vec![DeploymentCondition {
            type_: "Available".to_string(),
            status: "False".to_string(),
            reason: Some("ProgressDeadlineExceeded".to_string()),
            message: Some("Deadline exceeded".to_string()),
            last_transition_time: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            )),
            last_update_time: None,
        }]);
        assert_eq!(deployment_phase(conditions2.as_ref()), "Failed");
    }

    // 13. deployment_phase_provisioning
    #[test]
    fn deployment_phase_provisioning() {
        // None
        assert_eq!(deployment_phase(None), "Provisioning");

        // empty vec
        assert_eq!(deployment_phase(Some(&vec![])), "Provisioning");

        // Available/False with reason "ScalingReplicaSet" => not "Failed"
        let conditions = Some(vec![DeploymentCondition {
            type_: "Available".to_string(),
            status: "False".to_string(),
            reason: Some("ScalingReplicaSet".to_string()),
            message: Some("replica set is scaling".to_string()),
            last_transition_time: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            )),
            last_update_time: None,
        }]);
        assert_eq!(deployment_phase(conditions.as_ref()), "Provisioning");
    }

    // 14. compute_status_running
    #[test]
    fn compute_status_running() {
        let app = make_application("demo");
        let status = compute_status(&app, "Running");
        assert_eq!(status.phase.as_ref().unwrap(), "Running");
        assert_eq!(status.conditions.len(), 1);
        assert_eq!(status.conditions[0].type_, "Ready");
        assert_eq!(status.conditions[0].status, "True");
        assert_eq!(status.conditions[0].reason, "DeploymentReady");
    }

    // 15. compute_status_provisioning
    #[test]
    fn compute_status_provisioning() {
        let app = make_application("demo");
        let status = compute_status(&app, "Provisioning");
        assert_eq!(status.phase.as_ref().unwrap(), "Provisioning");
        assert_eq!(status.conditions.len(), 1);
        assert_eq!(status.conditions[0].type_, "Ready");
        assert_eq!(status.conditions[0].status, "False");
        assert_eq!(status.conditions[0].reason, "NotReady");
    }

    // 16. test_generate_cookie_secret_value
    #[test]
    fn test_generate_cookie_secret_value() {
        use base64::Engine;

        let value = generate_cookie_secret_value();
        assert_ne!(value, "");

        // STANDARD.decode gives exactly 64 bytes
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&value)
            .expect("decode");
        assert_eq!(decoded.len(), 64);

        // Decoded bytes must be nonZero (random)
        assert!(decoded.iter().any(|&b| b != 0));

        // Two calls should produce different values
        let value2 = generate_cookie_secret_value();
        assert_ne!(value, value2);
    }

    // ===== S6c — Moteur de workflow (Must « Moteur de workflow ») =====

    fn restate_app() -> Application {
        let mut app = make_application("demo");
        app.spec.restate = Some(vanyline_crds::RestateSpec {
            image: None,
            storage_size: None,
            storage_class: None,
        });
        app
    }

    // Scenario (Must) : StatefulSet `<nom CR>-restate` à UN réplica, label
    // `vanyline.solidite.fr/restate: <nom CR nu>` sur le POD TEMPLATE (obligation
    // S6b — cible des règles egress), ownerReference de l'Application, image
    // `spec.restate.image` sinon le défaut de l'appelant, volumeClaimTemplate
    // (5Gi par défaut, storageClass optionnelle).
    // tooling.sdd D4 — le test énumère les champs contractés du StatefulSet un
    // par un (un bloc par champ) ; la taille suit la table du Must.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn build_restate_statefulset_shape() {
        let app = restate_app();
        let sts = build_restate_statefulset(&app, "restate:test");

        assert_eq!(sts.metadata.name, Some("demo-restate".to_string()));
        assert_eq!(sts.metadata.namespace, Some("ns".to_string()));
        let refs = sts
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, "Application");
        assert_eq!(refs[0].name, "demo");
        assert_eq!(refs[0].controller, Some(true));

        let spec = sts.spec.as_ref().expect("should have spec");
        assert_eq!(spec.replicas, Some(1), "StatefulSet mono-réplica");
        assert_eq!(
            spec.service_name.as_deref(),
            Some("demo-restate"),
            "serviceName = le Service jumeau"
        );
        // Label RESTATE_LABEL posé sur le selector ET le pod template.
        let selector_labels = spec
            .selector
            .match_labels
            .as_ref()
            .expect("selector match_labels");
        assert_eq!(
            selector_labels
                .get(crate::sandbox::RESTATE_LABEL)
                .map(String::as_str),
            Some("demo")
        );
        let template_labels = spec
            .template
            .metadata
            .as_ref()
            .expect("template metadata")
            .labels
            .as_ref()
            .expect("template labels");
        assert_eq!(
            template_labels
                .get(crate::sandbox::RESTATE_LABEL)
                .map(String::as_str),
            Some("demo"),
            "OBLIGATION S6b : label restate sur le pod template"
        );

        let pod_spec = spec.template.spec.as_ref().expect("template spec").clone();
        let container = pod_spec
            .containers
            .first()
            .expect("should have 1 container");
        assert_eq!(container.name, "restate");
        assert_eq!(container.image, Some("restate:test".to_string()));
        let ports = container.ports.as_ref().expect("should have ports");
        let mut by_port: Vec<i32> = ports.iter().map(|p| p.container_port).collect();
        by_port.sort_unstable();
        assert_eq!(by_port, vec![5122, 8080, 9070]);

        // volumeClaimTemplate « data » : RWO, 5Gi par défaut, classe absente.
        let pvcs = spec
            .volume_claim_templates
            .as_ref()
            .expect("should have volumeClaimTemplates");
        assert_eq!(pvcs.len(), 1);
        assert_eq!(pvcs[0].metadata.name.as_deref(), Some("data"));
        let pvc_spec = pvcs[0].spec.as_ref().expect("pvc spec");
        assert_eq!(
            pvc_spec.access_modes,
            Some(vec!["ReadWriteOnce".to_string()])
        );
        assert_eq!(pvc_spec.storage_class_name, None);
        let requests = pvc_spec
            .resources
            .as_ref()
            .expect("resources")
            .requests
            .as_ref()
            .expect("requests");
        assert_eq!(
            requests.get("storage"),
            Some(&k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                "5Gi".to_string()
            ))
        );

        // Montage du volume de données sur le conteneur.
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volumeMounts");
        assert!(
            mounts
                .iter()
                .any(|m| m.name == "data" && !m.mount_path.is_empty()),
            "le conteneur monte le volumeClaimTemplate « data »"
        );

        // Image de `spec.restate.image` gagne sur le défaut de l'appelant.
        let mut app2 = restate_app();
        app2.spec.restate.as_mut().expect("restate").image = Some("custom/restate:tag".to_string());
        let sts2 = build_restate_statefulset(&app2, "restate:test");
        let container2 = sts2
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .unwrap();
        assert_eq!(container2.image, Some("custom/restate:tag".to_string()));
    }

    // Scenario (Must) : replis du volumeClaimTemplate — `storageSize` "8Gi" et
    // `storageClass` "local-path" posés dans la CR traversent le builder.
    #[test]
    fn build_restate_statefulset_storage_from_spec() {
        let mut app = restate_app();
        app.spec.restate.as_mut().expect("restate").storage_size = Some("8Gi".to_string());
        app.spec.restate.as_mut().expect("restate").storage_class = Some("local-path".to_string());
        let sts = build_restate_statefulset(&app, "restate:test");
        let pvc = &sts
            .spec
            .as_ref()
            .unwrap()
            .volume_claim_templates
            .as_ref()
            .unwrap()[0];
        let pvc_spec = pvc.spec.as_ref().unwrap();
        assert_eq!(pvc_spec.storage_class_name.as_deref(), Some("local-path"));
        assert_eq!(
            pvc_spec
                .resources
                .as_ref()
                .unwrap()
                .requests
                .as_ref()
                .unwrap()
                .get("storage"),
            Some(&k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                "8Gi".to_string()
            ))
        );
    }

    // Scenario (Must) : Service `<nom CR>-restate` (restate_service_name de
    // ./sandbox.rs, PAS application_name) exposant ingress 8080, admin 9070,
    // port de nœud 5122 ; sélecteur sur le label restate ; jamais de type
    // explicite (ClusterIP — le Service n'est jamais exposé par un Ingress).
    #[test]
    fn build_restate_service_shape() {
        let app = restate_app();
        let svc = build_restate_service(&app);

        assert_eq!(svc.metadata.name, Some("demo-restate".to_string()));
        assert_eq!(svc.metadata.namespace, Some("ns".to_string()));
        let refs = svc
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo");

        let spec = svc.spec.as_ref().expect("should have spec");
        assert!(spec.type_.is_none(), "ClusterIP laissé à l'apiserver");
        let selector = spec.selector.as_ref().expect("selector");
        assert_eq!(
            selector
                .get(crate::sandbox::RESTATE_LABEL)
                .map(String::as_str),
            Some("demo")
        );
        let ports = spec.ports.as_ref().expect("should have ports");
        assert_eq!(ports.len(), 3);
        for (name, port) in [("ingress", 8080), ("admin", 9070), ("worker", 5122)] {
            let p = ports
                .iter()
                .find(|p| p.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("should have port {name}"));
            assert_eq!(p.port, port);
            assert_eq!(p.target_port, Some(IntOrString::Int(port)));
        }
    }

    // Scenario (tâche S6c) : NetworkPolicy du namespace d'application ciblant
    // les pods restate — l'ingress (8080) et l'admin (9070) de Restate sont
    // joignables par le Deployment `app` ; les pods sandbox des namespaces
    // d'Owner n'appellent QUE l'ingress 8080 (callback d'awakeable).
    #[test]
    fn build_restate_netpol_shape() {
        let app = restate_app();
        let netpol = build_restate_netpol(&app);

        assert_eq!(
            netpol.metadata.name,
            Some("demo-restate-netpol".to_string())
        );
        assert_eq!(netpol.metadata.namespace, Some("ns".to_string()));
        let refs = netpol
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs[0].name, "demo");

        let spec = netpol.spec.as_ref().expect("should have spec");
        assert_eq!(
            spec.pod_selector
                .as_ref()
                .and_then(|s| s.match_labels.as_ref())
                .and_then(|m| m.get(crate::sandbox::RESTATE_LABEL))
                .map(String::as_str),
            Some("demo"),
            "la netpol cible les pods restate de cette Application"
        );
        assert_eq!(spec.policy_types, Some(vec!["Ingress".to_string()]));

        let rules = spec.ingress.as_ref().expect("should have ingress rules");
        assert_eq!(rules.len(), 2);

        // Règle 1 : le pod `app` (même namespace) → 8080 + 9070.
        let app_rule = rules
            .iter()
            .find(|r| {
                r.from.as_ref().is_some_and(|from| {
                    from.iter().any(|p| {
                        p.pod_selector
                            .as_ref()
                            .and_then(|s| s.match_labels.as_ref())
                            .is_some_and(|m| m.contains_key(APP_LABEL))
                    })
                })
            })
            .expect("règle depuis le pod app");
        assert!(
            app_rule
                .from
                .as_ref()
                .unwrap()
                .iter()
                .all(|p| p.namespace_selector.is_none()),
            "pair même namespace (le pod app vit dans le namespace d'application)"
        );
        let mut app_ports: Vec<i32> = app_rule
            .ports
            .as_ref()
            .expect("ports")
            .iter()
            .map(|p| match p.port {
                Some(IntOrString::Int(n)) => n,
                _ => panic!("port numérique attendu"),
            })
            .collect();
        app_ports.sort_unstable();
        assert_eq!(app_ports, vec![8080, 9070]);

        // Règle 2 : les pods sandbox des namespaces d'Owner → 8080 seul.
        let sandbox_rule = rules
            .iter()
            .find(|r| {
                r.from.as_ref().is_some_and(|from| {
                    from.iter().any(|p| {
                        p.namespace_selector
                            .as_ref()
                            .and_then(|s| s.match_labels.as_ref())
                            .is_some_and(|m| m.contains_key(crate::namespace::APP_NAMESPACE_LABEL))
                    })
                })
            })
            .expect("règle depuis les namespaces d'Owner");
        let peer = sandbox_rule.from.as_ref().unwrap().first().unwrap();
        assert_eq!(
            peer.namespace_selector
                .as_ref()
                .unwrap()
                .match_labels
                .as_ref()
                .unwrap()
                .get(crate::namespace::APP_NAMESPACE_LABEL)
                .map(String::as_str),
            Some("ns"),
            "namespaces d'Owner de CETTE application"
        );
        let exprs = peer
            .pod_selector
            .as_ref()
            .and_then(|s| s.match_expressions.as_ref())
            .expect("pod_selector par matchExpressions");
        assert_eq!(exprs.len(), 1);
        assert_eq!(exprs[0].key, "vanyline.solidite.fr/sandbox");
        assert_eq!(exprs[0].operator, "Exists");
        let sandbox_ports = sandbox_rule.ports.as_ref().expect("ports");
        assert_eq!(sandbox_ports.len(), 1, "les sandboxes n'appellent QUE 8080");
        assert!(matches!(
            sandbox_ports[0].port,
            Some(IntOrString::Int(8080))
        ));
    }

    // Scenario: build_restate_statefulset_retention_pvc_retain — le sort du PVC
    // au retrait de `spec.restate` est contracté Retain/Retain (décidé
    // 2026-10-14) : le défaut K8s est Delete ; sans pose explicite, le retrait
    // de `spec.restate` detruirait le journal des runs Restate.
    #[test]
    fn build_restate_statefulset_retention_pvc_retain() {
        let app = restate_app();
        let sts = build_restate_statefulset(&app, "restate:test");
        let spec = sts.spec.as_ref().expect("should have spec");
        let retention = spec
            .persistent_volume_claim_retention_policy
            .as_ref()
            .expect("politique de retention explicite (defaut K8s = Delete)");
        assert_eq!(
            retention.when_deleted.as_deref(),
            Some("Retain"),
            "whenDeleted: Retain — le PVC survit a la suppression du StatefulSet"
        );
        assert_eq!(
            retention.when_scaled.as_deref(),
            Some("Retain"),
            "whenScaled: Retain — le PVC survit a toute variation de replicas"
        );
    }

    // Scenario: build_restate_statefulset_env_exhaustive — la table d'env du
    // conteneur `restate` est EXHAUSTIVE (contractée 2026-10-14, elle
    // conditionne tout changement d'image) : exactement six variables dans cet
    // ordre, et RESTATE_DATA_DIR egale au mountPath du volume « data ».
    #[test]
    fn build_restate_statefulset_env_exhaustive() {
        let app = restate_app();
        let sts = build_restate_statefulset(&app, "restate:test");
        let spec = sts.spec.as_ref().expect("should have spec");
        let container = spec
            .template
            .spec
            .as_ref()
            .expect("template spec")
            .containers
            .first()
            .expect("should have 1 container");
        let env = container.env.as_ref().expect("should have env");
        let expected = [
            ("RESTATE_CLUSTER_NAME", "demo"),
            ("RESTATE_BOOTSTRAP_CLUSTER", "true"),
            ("RESTATE_INGRESS_SOCKET", "0.0.0.0:8080"),
            ("RESTATE_ADMIN_SOCKET", "0.0.0.0:9070"),
            ("RESTATE_WORKER_SOCKET", "0.0.0.0:5122"),
            ("RESTATE_DATA_DIR", "/var/restate"),
        ];
        assert_eq!(env.len(), 6, "table exhaustive = exactement 6 variables");
        for (i, (name, value)) in expected.iter().enumerate() {
            assert_eq!(env[i].name, *name, "ordre de construction, rang {i}");
            assert_eq!(env[i].value.as_deref(), Some(*value), "valeur de {name}");
            assert!(
                env[i].value_from.is_none(),
                "{name} porte une valeur litterale, jamais une source"
            );
        }
        // RESTATE_DATA_DIR egale au mountPath du volume « data ».
        let mount = container
            .volume_mounts
            .as_ref()
            .expect("should have volumeMounts")
            .iter()
            .find(|m| m.name == "data")
            .expect("montage du volumeClaimTemplate « data »");
        assert_eq!(
            mount.mount_path, "/var/restate",
            "RESTATE_DATA_DIR == mountPath du volume data"
        );
    }

    // Scenarios de application.sdd sans test existant (tâche « Convertir en tests … »).

    #[test]
    fn tls_secret_name_suffixe_cert() {
        assert_eq!(tls_secret_name("application-demo"), "application-demo-cert");
        assert_eq!(tls_secret_name("sandbox-web"), "sandbox-web-cert");
    }

    #[test]
    fn cert_manager_annotation_key_selon_kind() {
        assert_eq!(
            cert_manager_annotation_key(None),
            "cert-manager.io/cluster-issuer"
        );
        assert_eq!(
            cert_manager_annotation_key(Some("Issuer")),
            "cert-manager.io/issuer"
        );
        assert_eq!(
            cert_manager_annotation_key(Some("ClusterIssuer")),
            "cert-manager.io/cluster-issuer"
        );
        assert_eq!(
            cert_manager_annotation_key(Some("nimporte_quoi")),
            "cert-manager.io/cluster-issuer"
        );
    }

    #[test]
    fn build_application_deployment_namespace_absent() {
        let mut app = make_application("demo");
        app.meta_mut().namespace = None;
        let deployment = build_application_deployment(&app, TEST_APP_IMAGE);

        assert_eq!(deployment.metadata.namespace, None);
        let container = deployment
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers
            .first()
            .expect("should have 1 container");
        let env = container.env.as_ref().expect("should have env");
        let ns_env = env
            .iter()
            .find(|e| e.name == "VNL_K8S_NAMESPACE")
            .expect("VNL_K8S_NAMESPACE present");
        assert_eq!(ns_env.value.as_deref(), Some("default"));
    }

    #[test]
    fn deployment_phase_available_true_prioritaire_sur_failed() {
        let ts = || {
            Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            ))
        };
        let conditions = Some(vec![
            DeploymentCondition {
                type_: "Available".to_string(),
                status: "True".to_string(),
                reason: Some("MinimumReplicasAvailable".to_string()),
                message: None,
                last_transition_time: ts(),
                last_update_time: None,
            },
            DeploymentCondition {
                type_: "Available".to_string(),
                status: "False".to_string(),
                reason: Some("Failed".to_string()),
                message: None,
                last_transition_time: ts(),
                last_update_time: None,
            },
        ]);
        assert_eq!(deployment_phase(conditions.as_ref()), "Running");
    }

    #[test]
    fn compute_status_failed() {
        let app = make_application("demo");
        let status = compute_status(&app, "Failed");
        assert_eq!(status.phase.as_deref(), Some("Failed"));
        assert_eq!(status.conditions.len(), 1);
        let ready = &status.conditions[0];
        assert_eq!(ready.type_, "Ready");
        assert_eq!(ready.status, "False");
        assert_eq!(ready.reason, "NotReady");
        assert_eq!(ready.message, "phase=Failed");
    }

    #[tokio::test]
    async fn error_policy_requeue_trente_secondes() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = kube::Config::new(
            "http://127.0.0.1:9"
                .parse()
                .expect("URI morte syntaxiquement valide"),
        );
        let client = Client::try_from(config).expect("client hors connexion");
        let ctx = Arc::new(Context {
            client,
            default_image: TEST_APP_IMAGE.to_string(),
            restate_image: String::new(),
        });
        let app = Arc::new(make_application("demo"));
        let a = ControllerError::PurgePending {
            project: "p".to_string(),
        };
        let b = ControllerError::Finalizer("boom".to_string());
        assert_eq!(
            error_policy(app.clone(), &a, ctx.clone()),
            Action::requeue(Duration::from_secs(30))
        );
        assert_eq!(
            error_policy(app, &b, ctx),
            Action::requeue(Duration::from_secs(30))
        );
    }

    #[test]
    fn owner_reference_controller_sur_tous_les_builders() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;

        let app = make_application("demo");
        let mut all: Vec<(&str, Option<Vec<OwnerReference>>)> = vec![
            (
                "service_account",
                build_application_service_account(&app)
                    .metadata
                    .owner_references,
            ),
            (
                "role",
                build_application_role(&app).metadata.owner_references,
            ),
            (
                "role_binding",
                build_application_role_binding(&app)
                    .metadata
                    .owner_references,
            ),
            (
                "deployment",
                build_application_deployment(&app, TEST_APP_IMAGE)
                    .metadata
                    .owner_references,
            ),
            (
                "service",
                build_application_service(&app).metadata.owner_references,
            ),
            (
                "ingress",
                build_application_ingress(&app).metadata.owner_references,
            ),
            (
                "cookie_secret",
                build_cookie_secret(&app, "YWJj".to_string())
                    .metadata
                    .owner_references,
            ),
        ];
        for (label, refs) in all.drain(..) {
            let refs = refs.expect("ownerReferences posées");
            assert_eq!(refs.len(), 1, "{label}: exactement une ownerReference");
            let r: &OwnerReference = &refs[0];
            assert_eq!(r.api_version, "vanyline.solidite.fr/v1alpha1", "{label}");
            assert_eq!(r.kind, "Application", "{label}");
            assert_eq!(r.name, "demo", "{label}");
            assert_eq!(r.controller, Some(true), "{label}");
            assert_eq!(
                r.block_owner_deletion, None,
                "{label}: block_owner_deletion non posé"
            );
        }
    }
}
