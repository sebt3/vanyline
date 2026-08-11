use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::ByteString;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentCondition, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, EnvVarSource, HTTPGetAction, PodSpec, PodTemplateSpec, Probe,
    Secret, SecretKeySelector, Service, ServicePort, ServiceSpec,
};
use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec, ServiceBackendPort,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{
    Condition, LabelSelector, ObjectMeta, OwnerReference,
};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::runtime::controller::{Action, Controller};
use kube::{Client, Resource, ResourceExt};

use crate::error::ControllerError;
use crate::owner::FIELD_MANAGER;
use vanyline_crds::{Application, ApplicationStatus};

/// Port HTTP du Deployment `app` — défaut de `LISTEN_ADDR` côté
/// `app/src/config.rs` (`0.0.0.0:8080`).
pub const APP_PORT: i32 = 8080;

/// Label unique porté par le Deployment `app` et utilisé comme sélecteur du
/// Service — même patron que `build_sandbox_service`.
pub const APP_LABEL: &str = "vanyline.solidite.fr/application";

/// Image par défaut du Deployment `app` quand `spec.image` est `None`
/// (repli du design : `ghcr.io/…-app:latest` — valeur à ajuster au déploiement).
pub const DEFAULT_APP_IMAGE: &str = "ghcr.io/vanyline/vanyline-app:latest";

/// Nom commun des objets créés pour une Application : Deployment, Service et
/// Ingress — `application-<name>` (même convention que `sandbox-<name>`).
pub fn application_name(app_name: &str) -> String {
    format!("application-{app_name}")
}

/// Nom du Secret cookie auto-généré : `<application-name>-cookie`.
pub fn cookie_secret_name(app_name: &str) -> String {
    format!("{app_name}-cookie")
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

/// Construit le Deployment `app` : un seul container, image résolue
/// (`spec.image` ou `DEFAULT_APP_IMAGE`), env depuis les trois `secretRef`
/// (`secretKeyRef` par variable) + `VNL_K8S_NAMESPACE` + `OIDC_REDIRECT_URL`
/// calculé (`https://{spec.host}/auth/callback`, jamais lu du secret). Probe
/// readiness/liveness sur `GET /health`. `ownerReference` vers l'Application.
/// `replicas` = `spec.replicas` ou 1. Label `vanyline.solidite.fr/application`
/// posé sur metadata/selector/template.
///
/// Notes d'adaptation k8s-openapi 0.28 :
/// - `DeploymentSpec.selector` sans wrapper `Option` (type `LabelSelector` direct)
/// - `DeploymentSpec.template` sans wrapper `Option` (type `PodTemplateSpec` direct)
/// - `PodTemplateSpec.metadata` optionnel (type `Option<ObjectMeta>`)
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_deployment(app: &Application) -> Deployment {
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
    ];
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
                key: "databaseUrl".to_string(),
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
                    containers: vec![Container {
                        name: "app".to_string(),
                        image: Some(
                            app.spec
                                .image
                                .clone()
                                .unwrap_or_else(|| DEFAULT_APP_IMAGE.to_string()),
                        ),
                        ports: Some(vec![ContainerPort {
                            container_port: APP_PORT,
                            name: Some("http".to_string()),
                            ..Default::default()
                        }]),
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

/// Service ClusterIP exposant le port HTTP du Deployment `app` (sélecteur =
/// `vanyline.solidite.fr/application: <name>`, seul label garanti unique posé
/// par `build_application_deployment`). `ownerReference` vers l'Application.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_service(app: &Application) -> Service {
    let mut selector = BTreeMap::new();
    selector.insert(APP_LABEL.to_string(), app.name_any());

    Service {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(selector),
            ports: Some(vec![ServicePort {
                name: Some("http".to_string()),
                port: APP_PORT,
                target_port: Some(IntOrString::Int(APP_PORT)),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        status: None,
    }
}

/// Ingress : host `spec.host`, `ingressClassName: spec.ingress_class_name`,
/// annotations `spec.ingress_annotations` (None si vide), backend = le Service
/// `application-<name>` port 8080, path `/` Prefix. `ownerReference` vers
/// l'Application.
///
/// Notes d'adaptation k8s-openapi 0.28 :
/// - `HTTPIngressRuleValue.paths` : `Vec<HTTPIngressPath>` (pas Option)
/// - `HTTPIngressPath.path_type` : `String` (pas Option)
/// - `HTTPIngressPath.backend` : `IngressBackend` (pas Option)
/// - `IngressServiceBackend.name` : `String` (pas Option)
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_application_ingress(app: &Application) -> Ingress {
    Ingress {
        metadata: ObjectMeta {
            name: Some(application_name(&app.name_any())),
            namespace: app.namespace(),
            annotations: if app.spec.ingress_annotations.is_empty() {
                None
            } else {
                Some(app.spec.ingress_annotations.clone())
            },
            owner_references: Some(vec![app_owner_ref(app)]),
            ..Default::default()
        },
        spec: Some(IngressSpec {
            ingress_class_name: Some(app.spec.ingress_class_name.clone()),
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
/// base64 lors de la sérialisation, mais le stockage interne (ByteString)
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

pub struct Context {
    pub client: Client,
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
    let available = conditions
        .map(|cs| {
            cs.iter()
                .any(|c| c.type_ == "Available" && c.status == "True")
        })
        .unwrap_or(false);
    let failed = conditions
        .map(|cs| {
            cs.iter().any(|c| {
                c.type_ == "Available"
                    && c.status == "False"
                    && (c.reason.as_deref() == Some("Failed")
                        || c.reason.as_deref() == Some("ProgressDeadlineExceeded"))
            })
        })
        .unwrap_or(false);

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
/// `spec.cookie_secret_ref` est `None`), puis server-side apply idempotent du
/// Deployment, du Service et de l'Ingress, puis status. Pas de finalizer — la GC
/// K8s (ownerReferences) nettoie les objets créés à la suppression de l'Application.
pub async fn reconcile(
    app: Arc<Application>,
    ctx: Arc<Context>,
) -> Result<Action, ControllerError> {
    let ns = app.namespace().unwrap_or_else(|| "default".to_string());
    let pp = PatchParams::apply(FIELD_MANAGER).force();

    ensure_cookie_secret(&app, &ctx, &ns).await?;

    let deployments: Api<Deployment> = Api::namespaced(ctx.client.clone(), &ns);
    let name = application_name(&app.name_any());
    let phase = match deployments.get_opt(&name).await? {
        None => "Provisioning".to_string(),
        Some(d) => deployment_phase(d.status.as_ref().and_then(|s| s.conditions.as_ref())),
    };

    let deployment = build_application_deployment(&app);
    deployments
        .patch(&name, &pp, &Patch::Apply(&deployment))
        .await?;

    let services: Api<Service> = Api::namespaced(ctx.client.clone(), &ns);
    let service = build_application_service(&app);
    services.patch(&name, &pp, &Patch::Apply(&service)).await?;

    let ingresses: Api<Ingress> = Api::namespaced(ctx.client.clone(), &ns);
    let ingress = build_application_ingress(&app);
    ingresses.patch(&name, &pp, &Patch::Apply(&ingress)).await?;

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
                ingress_annotations: BTreeMap::new(),
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
        let deployment = build_application_deployment(&app);

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
        let dep = build_application_deployment(&app);
        assert_eq!(dep.spec.as_ref().unwrap().replicas, Some(3));

        // image = None -> default
        let mut app = make_application("demo");
        app.spec.image = None;
        let dep = build_application_deployment(&app);
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
        assert_eq!(container.image, Some(DEFAULT_APP_IMAGE.to_string()));

        // image = Some("custom:tag")
        let mut app = make_application("demo");
        app.spec.image = Some("custom:tag".to_string());
        let dep = build_application_deployment(&app);
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
    #[test]
    fn build_application_deployment_env() {
        let app = make_application("demo");
        let deployment = build_application_deployment(&app);
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
            9,
            "expected 9 environment variables, got {}",
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
        assert_eq!(skr.key, "databaseUrl");

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
    }

    // 9. build_application_ingress_annotations
    #[test]
    fn build_application_ingress_annotations() {
        // With annotations
        let mut app = make_application("demo");
        app.spec.ingress_annotations = BTreeMap::from([(
            "cert-manager.io/cluster-issuer".to_string(),
            "letsencrypt".to_string(),
        )]);
        let ingress = build_application_ingress(&app);
        let ann = ingress
            .metadata
            .annotations
            .as_ref()
            .expect("should have annotations");
        assert_eq!(
            ann.get("cert-manager.io/cluster-issuer"),
            Some(&"letsencrypt".to_string())
        );

        // Without annotations
        let app = make_application("demo");
        let ingress = build_application_ingress(&app);
        assert!(ingress.metadata.annotations.is_none());
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
        assert!(!value.is_empty());

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
}
