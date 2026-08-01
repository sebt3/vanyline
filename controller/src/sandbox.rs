use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, HTTPGetAction, PersistentVolumeClaimVolumeSource, Pod,
    PodSpec, Probe, Service, Volume, VolumeMount,
};
use k8s_openapi::api::networking::v1::{
    IPBlock, NetworkPolicy, NetworkPolicyEgressRule, NetworkPolicyIngressRule, NetworkPolicyPeer,
    NetworkPolicyPort, NetworkPolicySpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, LabelSelector};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, DeleteParams, ObjectMeta, Patch, PatchParams, PostParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::finalizer::{finalizer, Event};
use kube::{Client, Resource, ResourceExt};

use vanyline_crds::{
    EgressPort, EgressRule, MCP_PORT, Owner, Project, Sandbox, SandboxStatus, Toolchain,
    service_name,
};
use crate::error::ControllerError;
use crate::owner;
use crate::owner::HOME_MOUNT_PATH;
use crate::project::{self, ProjectJobContext};
use crate::project::{
    cache_dir_name, effective_caches, effective_pvc_name, effective_sub_path, worktree_path,
    WORKSPACE_MOUNT_PATH,
};

/// Fin de `PATH` commune à tous les pods sandbox (PATH standard Debian), reprise
/// telle quelle de `deploy/sandbox-test.yaml` (recette validée).
const BASE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub fn pod_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}")
}

/// Point de montage d'un volume toolchain — `/toolchains/<name>`.
pub fn toolchain_root(name: &str) -> String {
    format!("/toolchains/{name}")
}

/// Preset d'environnement pour une toolchain connue (`"rust"`, `"node"`) —
/// valeurs recopiées de la recette validée `deploy/sandbox-test.yaml`. Les clés
/// `PATH`/`LD_LIBRARY_PATH` sont concaténées entre toolchains par
/// `aggregate_toolchain_env` ; les autres clés (ex. `RUSTUP_HOME`) sont posées
/// telles quelles.
fn toolchain_preset(name: &str) -> Option<BTreeMap<String, String>> {
    match name {
        "rust" => Some(BTreeMap::from([
            (
                "PATH".to_string(),
                "{root}/usr/local/cargo/bin:{root}/usr/bin".to_string(),
            ),
            (
                "LD_LIBRARY_PATH".to_string(),
                "{root}/usr/lib/x86_64-linux-gnu:{root}/usr/lib/aarch64-linux-gnu:{root}/usr/local/lib".to_string(),
            ),
            (
                "RUSTUP_HOME".to_string(),
                "{root}/usr/local/rustup".to_string(),
            ),
        ])),
        "node" => Some(BTreeMap::from([
            ("PATH".to_string(), "{root}/usr/local/bin".to_string()),
            (
                "LD_LIBRARY_PATH".to_string(),
                "{root}/usr/lib/x86_64-linux-gnu:{root}/usr/lib/aarch64-linux-gnu:{root}/usr/local/lib".to_string(),
            ),
        ])),
        _ => None,
    }
}

/// Résout l'environnement d'une toolchain : `toolchain.env` s'il est non vide
/// (le contrat de `SandboxSpec.toolchains[].env`, cf. `crds.rs`), sinon le
/// preset s'il existe pour `toolchain.name`, sinon aucune variable. Substitue
/// `{root}` par `toolchain_root(&toolchain.name)` dans chaque valeur.
fn resolve_toolchain_env(toolchain: &Toolchain) -> BTreeMap<String, String> {
    let root = toolchain_root(&toolchain.name);
    let raw = if !toolchain.env.is_empty() {
        toolchain.env.clone()
    } else {
        toolchain_preset(&toolchain.name).unwrap_or_default()
    };
    raw.into_iter()
        .map(|(k, v)| (k, v.replace("{root}", &root)))
        .collect()
}

/// Agrège l'environnement de toutes les toolchains d'une Sandbox, dans l'ordre
/// de `spec.toolchains` : les segments `PATH` de chaque toolchain sont
/// concaténés (`:`) puis suivis de `BASE_PATH` ; les segments
/// `LD_LIBRARY_PATH` sont concaténés (sans suffixe) ; toute autre clé
/// (`RUSTUP_HOME`, etc.) est posée telle quelle — en cas de collision entre
/// deux toolchains sur une même clé non `PATH`/`LD_LIBRARY_PATH`, la dernière
/// toolchain de la liste gagne.
pub fn aggregate_toolchain_env(toolchains: &[Toolchain]) -> Vec<EnvVar> {
    let mut path_segments = Vec::new();
    let mut ld_segments = Vec::new();
    let mut others: BTreeMap<String, String> = BTreeMap::new();

    for toolchain in toolchains {
        for (k, v) in resolve_toolchain_env(toolchain) {
            match k.as_str() {
                "PATH" => path_segments.push(v),
                "LD_LIBRARY_PATH" => ld_segments.push(v),
                _ => {
                    others.insert(k, v);
                }
            }
        }
    }

    let mut env = Vec::new();
    path_segments.push(BASE_PATH.to_string());
    env.push(EnvVar {
        name: "PATH".to_string(),
        value: Some(path_segments.join(":")),
        ..Default::default()
    });
    if !ld_segments.is_empty() {
        env.push(EnvVar {
            name: "LD_LIBRARY_PATH".to_string(),
            value: Some(ld_segments.join(":")),
            ..Default::default()
        });
    }
    for (k, v) in others {
        env.push(EnvVar {
            name: k,
            value: Some(v),
            ..Default::default()
        });
    }
    env
}

/// Variable d'env pour un cache donné, `None` si le cache n'a pas de convention
/// connue (seuls `"cargo"` et `"pnpm"` en ont une en v1).
fn cache_env_var(cache: &str) -> Option<(&'static str, String)> {
    let path = format!("/project-cache/{}", crate::project::cache_dir_name(cache));
    match cache {
        "cargo" => Some(("CARGO_HOME", path)),
        "pnpm" => Some(("npm_config_store_dir", path)),
        _ => None,
    }
}

/// Combine le `subPath` existant du Project (cas `existing_pvc`, ex.
/// kydah-code) avec un chemin relatif du layout (`worktree_path`, `cache_path`).
/// `None` côté Project => le chemin relatif est utilisé tel quel (le PVC créé
/// n'appartient qu'à ce Project, pas de préfixe nécessaire).
fn combine_sub_path(project: &Project, relative: &str) -> String {
    match effective_sub_path(project) {
        Some(base) => format!("{base}/{relative}"),
        None => relative.to_string(),
    }
}

/// Context résolu par l'appelant (le reconciler Sandbox ira chercher l'Owner du
/// Project référencé — cette fonction reste pure, aucun appel réseau).
pub struct SandboxPodContext {
    pub owner_name: String,
    pub owner_pvc_name: String,
    pub owner_service_account: String,
    /// Image sandbox par défaut (env `SANDBOX_IMAGE` du controller), utilisée
    /// quand `sandbox.spec.image` est `None`.
    pub default_image: String,
}

/// Construit le Pod sandbox : home Owner + worktree + caches + toolchains,
/// `--no-auth` (voir décisions de cette tâche), probes `/health`, labels
/// `vanyline.solidite.fr/{owner,project,sandbox}`, `ownerReference` vers la
/// Sandbox (GC en cascade).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_sandbox_pod(sandbox: &Sandbox, project: &Project, ctx: &SandboxPodContext) -> Pod {
    let workspace_pvc = effective_pvc_name(project);
    let mut volumes = vec![
        Volume {
            name: "home".to_string(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: ctx.owner_pvc_name.clone(),
                ..Default::default()
            }),
            ..Default::default()
        },
        Volume {
            name: "workspace".to_string(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: workspace_pvc,
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    let mut mounts = vec![
        VolumeMount {
            name: "home".to_string(),
            mount_path: HOME_MOUNT_PATH.to_string(),
            ..Default::default()
        },
        VolumeMount {
            name: "workspace".to_string(),
            mount_path: format!("{HOME_MOUNT_PATH}/workspace"),
            sub_path: Some(combine_sub_path(
                project,
                &worktree_path(&sandbox.name_any()),
            )),
            ..Default::default()
        },
    ];

    // Monté le clone bare du repo pour que le pointeur .git du worktree
    // (gitdir absolu écrit par `git worktree add`) pointe vers un chemin
    // visible dans le pod sandbox. Lecture-écriture : git doit pouvoir
    // écrire HEAD/index (sous worktrees/<sandbox>/) et de nouveaux objets.
    mounts.push(VolumeMount {
        name: "workspace".to_string(),
        mount_path: format!("{WORKSPACE_MOUNT_PATH}/{}", project::bare_repo_path()),
        sub_path: Some(combine_sub_path(project, project::bare_repo_path())),
        ..Default::default()
    });

    let mut env = vec![EnvVar {
        name: "VNL_SANDBOX_ROOT".to_string(),
        value: Some(format!("{HOME_MOUNT_PATH}/workspace")),
        ..Default::default()
    }];

    for cache in effective_caches(project) {
        mounts.push(VolumeMount {
            name: "workspace".to_string(),
            mount_path: format!("/project-cache/{}", cache_dir_name(&cache)),
            sub_path: Some(combine_sub_path(
                project,
                &crate::project::cache_path(&cache),
            )),
            ..Default::default()
        });
        if let Some((key, value)) = cache_env_var(&cache) {
            env.push(EnvVar {
                name: key.to_string(),
                value: Some(value),
                ..Default::default()
            });
        }
    }

    for toolchain in &sandbox.spec.toolchains {
        volumes.push(Volume {
            name: format!("toolchain-{}", toolchain.name),
            image: Some(k8s_openapi::api::core::v1::ImageVolumeSource {
                reference: Some(toolchain.image.clone()),
                pull_policy: Some("IfNotPresent".to_string()),
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount {
            name: format!("toolchain-{}", toolchain.name),
            mount_path: toolchain_root(&toolchain.name),
            ..Default::default()
        });
    }
    env.extend(aggregate_toolchain_env(&sandbox.spec.toolchains));

    let mut labels = BTreeMap::new();
    labels.insert(
        "vanyline.solidite.fr/owner".to_string(),
        ctx.owner_name.clone(),
    );
    labels.insert(
        "vanyline.solidite.fr/project".to_string(),
        sandbox.spec.project.clone(),
    );
    labels.insert(
        "vanyline.solidite.fr/sandbox".to_string(),
        sandbox.name_any(),
    );

    let probe = Probe {
        http_get: Some(HTTPGetAction {
            path: Some("/health".to_string()),
            port: IntOrString::Int(MCP_PORT),
            ..Default::default()
        }),
        initial_delay_seconds: Some(5),
        period_seconds: Some(10),
        ..Default::default()
    };

    Pod {
        metadata: ObjectMeta {
            name: Some(pod_name(&sandbox.name_any())),
            namespace: sandbox.namespace(),
            labels: Some(labels),
            owner_references: Some(vec![sandbox
                .controller_owner_ref(&())
                .expect("Sandbox a apiVersion/kind")]),
            ..Default::default()
        },
        spec: Some(PodSpec {
            service_account_name: Some(ctx.owner_service_account.clone()),
            containers: vec![Container {
                name: "sandbox".to_string(),
                image: Some(
                    sandbox
                        .spec
                        .image
                        .clone()
                        .unwrap_or_else(|| ctx.default_image.clone()),
                ),
                args: Some(vec!["--no-auth".to_string()]),
                ports: Some(vec![ContainerPort {
                    container_port: MCP_PORT,
                    name: Some("mcp".to_string()),
                    ..Default::default()
                }]),
                env: Some(env),
                volume_mounts: Some(mounts),
                resources: sandbox.spec.resources.clone(),
                readiness_probe: Some(probe.clone()),
                liveness_probe: Some(probe),
                ..Default::default()
            }],
            volumes: Some(volumes),
            ..Default::default()
        }),
        status: None,
    }
}

const FINALIZER: &str = "vanyline.solidite.fr/sandbox-cleanup";
const SERVICE_PORT_NAME: &str = "mcp";

pub struct Context {
    pub client: Client,
    pub default_image: String,
}

pub fn checkout_job_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}-checkout")
}

pub fn worktree_remove_job_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}-remove")
}

pub fn netpol_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}")
}

/// Nom de la NetworkPolicy egress — objet distinct de la netpol ingress
/// (`netpol_name`), car elle est conditionnelle (absente si aucun niveau ne
/// déclare d'egress) alors que l'ingress est toujours produite.
pub fn netpol_egress_name(sandbox_name: &str) -> String {
    format!("sandbox-{sandbox_name}-egress")
}

/// Job une fois : crée le worktree de la branche (le crée depuis
/// `default_branch` si fourni, sinon `vanyline-maint` le résolve par
/// `symbolic-ref --short HEAD`).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_checkout_job(
    sandbox: &Sandbox,
    project: &Project,
    job_ctx: &ProjectJobContext,
) -> Job {
    let mut command = vec![
        "vanyline-maint".to_string(),
        "checkout".to_string(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
        "--sandbox".to_string(),
        sandbox.name_any(),
        "--branch".to_string(),
        sandbox.spec.branch.clone(),
    ];
    if let Some(db) = &project.spec.default_branch {
        if !db.is_empty() {
            command.push("--default-branch".to_string());
            command.push(db.clone());
        }
    }

    Job {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(checkout_job_name(&sandbox.name_any())),
            namespace: sandbox.namespace(),
            owner_references: Some(vec![sandbox
                .controller_owner_ref(&())
                .expect("Sandbox a apiVersion/kind")]),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(3),
            ttl_seconds_after_finished: Some(3600),
            template: project::git_pod_template(project, job_ctx, command),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Job de retrait du worktree — invoqué par le finalizer. `worktree remove
/// --force` gère l'état non commité ; repli sur `rm -rf` + `worktree prune` si
/// les métadonnées git sont dans un état incohérent. La résolution est faite
/// par `vanyline-maint`.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_worktree_remove_job(
    sandbox: &Sandbox,
    project: &Project,
    job_ctx: &ProjectJobContext,
) -> Job {
    let command = vec![
        "vanyline-maint".to_string(),
        "remove".to_string(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
        "--sandbox".to_string(),
        sandbox.name_any(),
    ];

    Job {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(worktree_remove_job_name(&sandbox.name_any())),
            namespace: sandbox.namespace(),
            owner_references: Some(vec![sandbox
                .controller_owner_ref(&())
                .expect("Sandbox a apiVersion/kind")]),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(3),
            ttl_seconds_after_finished: Some(3600),
            template: project::git_pod_template(project, job_ctx, command),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Service ClusterIP exposant le port MCP du Pod sandbox (sélecteur =
/// `vanyline.solidite.fr/sandbox: <sandbox>`, seul label garanti unique à ce
/// pod parmi ceux posés par `build_sandbox_pod`).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_sandbox_service(sandbox: &Sandbox) -> Service {
    let mut selector = BTreeMap::new();
    selector.insert(
        "vanyline.solidite.fr/sandbox".to_string(),
        sandbox.name_any(),
    );

    Service {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(service_name(&sandbox.name_any())),
            namespace: sandbox.namespace(),
            owner_references: Some(vec![sandbox
                .controller_owner_ref(&())
                .expect("Sandbox a apiVersion/kind")]),
            ..Default::default()
        },
        spec: Some(k8s_openapi::api::core::v1::ServiceSpec {
            selector: Some(selector),
            ports: Some(vec![k8s_openapi::api::core::v1::ServicePort {
                name: Some(SERVICE_PORT_NAME.to_string()),
                port: MCP_PORT,
                target_port: Some(IntOrString::Int(MCP_PORT)),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        status: None,
    }
}

/// Convertit une `EgressRule` en `NetworkPolicyPeer`. `cidr` et
/// `pod_selector`/`namespace_selector` sont exclusifs au niveau du type
/// CRD (non validé à la construction, cf. `vanyline-crds`) — ici la
/// précédence est explicite : si `cidr` est renseigné, il gagne (ipBlock),
/// `pod_selector`/`namespace_selector` sont ignorés même si présents.
fn egress_rule_to_peer(rule: &EgressRule) -> NetworkPolicyPeer {
    if let Some(cidr) = &rule.cidr {
        NetworkPolicyPeer {
            ip_block: Some(IPBlock {
                cidr: cidr.clone(),
                except: None,
            }),
            pod_selector: None,
            namespace_selector: None,
        }
    } else {
        NetworkPolicyPeer {
            ip_block: None,
            pod_selector: rule.pod_selector.clone(),
            namespace_selector: rule.namespace_selector.clone(),
        }
    }
}

/// Convertit les `EgressPort` d'une `EgressRule` en `NetworkPolicyPort`.
/// Liste vide => `None` (pas de restriction de port, sémantique K8s :
/// "If this field is empty or missing, this rule matches all ports").
/// `protocol: None` sur un `EgressPort` => `"TCP"` (défaut K8s explicite,
/// documenté dans `EgressPort::protocol`).
fn egress_ports_to_netpol_ports(ports: &[EgressPort]) -> Option<Vec<NetworkPolicyPort>> {
    if ports.is_empty() {
        return None;
    }
    Some(
        ports
            .iter()
            .map(|p| NetworkPolicyPort {
                port: Some(IntOrString::Int(p.port)),
                protocol: Some(p.protocol.clone().unwrap_or_else(|| "TCP".to_string())),
                end_port: None,
            })
            .collect(),
    )
}

/// Règle egress DNS toujours présente dès qu'au moins une netpol egress est
/// produite — sans restriction de destination (décision 2026-08-01, cf.
/// contexte de cette tâche), seulement le port 53 UDP+TCP.
fn dns_egress_rule() -> NetworkPolicyEgressRule {
    NetworkPolicyEgressRule {
        to: None,
        ports: Some(vec![
            NetworkPolicyPort {
                port: Some(IntOrString::Int(53)),
                protocol: Some("UDP".to_string()),
                end_port: None,
            },
            NetworkPolicyPort {
                port: Some(IntOrString::Int(53)),
                protocol: Some("TCP".to_string()),
                end_port: None,
            },
        ]),
    }
}

/// NetworkPolicy : cible le Pod de cette Sandbox, autorise l'ingress
/// uniquement depuis les pods du même namespace portant
/// `vanyline.solidite.fr/owner: <owner_name>` (même Owner — code-server,
/// autres sandboxes du même utilisateur).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_sandbox_netpol(sandbox: &Sandbox, owner_name: &str) -> NetworkPolicy {
    let mut pod_selector_labels = BTreeMap::new();
    pod_selector_labels.insert(
        "vanyline.solidite.fr/sandbox".to_string(),
        sandbox.name_any(),
    );

    let mut peer_labels = BTreeMap::new();
    peer_labels.insert(
        "vanyline.solidite.fr/owner".to_string(),
        owner_name.to_string(),
    );

    NetworkPolicy {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(netpol_name(&sandbox.name_any())),
            namespace: sandbox.namespace(),
            owner_references: Some(vec![sandbox
                .controller_owner_ref(&())
                .expect("Sandbox a apiVersion/kind")]),
            ..Default::default()
        },
        spec: Some(NetworkPolicySpec {
            pod_selector: Some(LabelSelector {
                match_labels: Some(pod_selector_labels),
                ..Default::default()
            }),
            policy_types: Some(vec!["Ingress".to_string()]),
            ingress: Some(vec![NetworkPolicyIngressRule {
                from: Some(vec![NetworkPolicyPeer {
                    pod_selector: Some(LabelSelector {
                        match_labels: Some(peer_labels),
                        ..Default::default()
                    }),
                    ..Default::default()
                }]),
                ports: Some(vec![NetworkPolicyPort {
                    port: Some(IntOrString::Int(MCP_PORT)),
                    protocol: Some("TCP".to_string()),
                    ..Default::default()
                }]),
            }]),
            ..Default::default()
        }),
    }
}

/// Construit la NetworkPolicy egress d'une Sandbox à partir de l'union des
/// règles `owner_egress` + `project_egress` + `sandbox.spec.egress` (dans
/// cet ordre). `None` si les trois sont vides — pas de netpol egress
/// produite (l'egress du pod reste libre, cf. design section 2). Sinon,
/// une règle par `EgressRule` de l'union, plus toujours `dns_egress_rule()`
/// en tête. `ownerReference` vers la Sandbox (GC en cascade), même
/// pod_selector ciblant `vanyline.solidite.fr/sandbox` que la netpol
/// ingress (`build_sandbox_netpol`).
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_sandbox_egress_netpol(
    sandbox: &Sandbox,
    owner_egress: &[EgressRule],
    project_egress: &[EgressRule],
) -> Option<NetworkPolicy> {
    let all_rules: Vec<&EgressRule> = owner_egress
        .iter()
        .chain(project_egress.iter())
        .chain(sandbox.spec.egress.iter())
        .collect();

    if all_rules.is_empty() {
        return None;
    }

    let mut egress_rules = vec![dns_egress_rule()];
    egress_rules.extend(all_rules.iter().map(|rule| NetworkPolicyEgressRule {
        to: Some(vec![egress_rule_to_peer(rule)]),
        ports: egress_ports_to_netpol_ports(&rule.ports),
    }));

    let mut pod_selector_labels = BTreeMap::new();
    pod_selector_labels.insert(
        "vanyline.solidite.fr/sandbox".to_string(),
        sandbox.name_any(),
    );

    Some(NetworkPolicy {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(netpol_egress_name(&sandbox.name_any())),
            namespace: sandbox.namespace(),
            owner_references: Some(vec![sandbox
                .controller_owner_ref(&())
                .expect("Sandbox a apiVersion/kind")]),
            ..Default::default()
        },
        spec: Some(NetworkPolicySpec {
            pod_selector: Some(LabelSelector {
                match_labels: Some(pod_selector_labels),
                ..Default::default()
            }),
            policy_types: Some(vec!["Egress".to_string()]),
            egress: Some(egress_rules),
            ..Default::default()
        }),
    })
}

/// Status attendu : `service`, condition `Ready` reflétant `phase == "Running"`.
pub fn compute_status(sandbox: &Sandbox, phase: &str) -> SandboxStatus {
    let (status, reason) = if phase == "Running" {
        ("True", "PodRunning")
    } else {
        ("False", "NotRunning")
    };
    SandboxStatus {
        phase: Some(phase.to_string()),
        service: Some(service_name(&sandbox.name_any())),
        conditions: vec![Condition {
            type_: "Ready".to_string(),
            status: status.to_string(),
            reason: reason.to_string(),
            message: format!("phase={phase}"),
            last_transition_time: k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            ),
            observed_generation: sandbox.meta().generation,
        }],
    }
}

async fn fetch_project(
    sandbox: &Sandbox,
    ctx: &Context,
    ns: &str,
) -> Result<Project, ControllerError> {
    let projects: Api<Project> = Api::namespaced(ctx.client.clone(), ns);
    Ok(projects.get(&sandbox.spec.project).await?)
}

async fn fetch_owner(project: &Project, ctx: &Context, ns: &str) -> Result<Owner, ControllerError> {
    let owners: Api<Owner> = Api::namespaced(ctx.client.clone(), ns);
    Ok(owners.get(&project.spec.owner).await?)
}

async fn apply(sandbox: &Sandbox, ctx: &Context, ns: &str) -> Result<Action, ControllerError> {
    let project = fetch_project(sandbox, ctx, ns).await?;
    if !project.status.as_ref().map(|s| s.cloned).unwrap_or(false) {
        return Err(ControllerError::ProjectNotReady {
            project: sandbox.spec.project.clone(),
            sandbox: sandbox.name_any(),
        });
    }
    let owner = fetch_owner(&project, ctx, ns).await?;
    let owner_pvc_name = owner
        .status
        .as_ref()
        .and_then(|s| s.pvc_name.clone())
        .ok_or_else(|| ControllerError::OwnerNotReady {
            owner: project.spec.owner.clone(),
            project: sandbox.spec.project.clone(),
        })?;
    let owner_service_account = owner
        .status
        .as_ref()
        .and_then(|s| s.service_account.clone())
        .ok_or_else(|| ControllerError::OwnerNotReady {
            owner: project.spec.owner.clone(),
            project: sandbox.spec.project.clone(),
        })?;

    let job_ctx = ProjectJobContext {
        sandbox_image: ctx.default_image.clone(),
        owner_pvc_name: owner_pvc_name.clone(),
    };

    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let checked_out = match jobs
        .get_opt(&checkout_job_name(&sandbox.name_any()))
        .await?
    {
        Some(job) => job.status.and_then(|s| s.succeeded).unwrap_or(0) > 0,
        None => {
            let job = build_checkout_job(sandbox, &project, &job_ctx);
            jobs.create(&PostParams::default(), &job).await?;
            false
        }
    };

    let pp = PatchParams::apply(owner::FIELD_MANAGER).force();
    let phase = if !checked_out {
        "Provisioning".to_string()
    } else {
        let pods: Api<k8s_openapi::api::core::v1::Pod> = Api::namespaced(ctx.client.clone(), ns);
        let pod_ctx = SandboxPodContext {
            owner_name: project.spec.owner.clone(),
            owner_pvc_name,
            owner_service_account,
            default_image: ctx.default_image.clone(),
        };
        match pods.get_opt(&pod_name(&sandbox.name_any())).await? {
            Some(pod) => match pod.status.and_then(|s| s.phase) {
                Some(p) if p == "Running" => "Running".to_string(),
                Some(p) if p == "Failed" => "Failed".to_string(),
                _ => "Provisioning".to_string(),
            },
            None => {
                let pod = build_sandbox_pod(sandbox, &project, &pod_ctx);
                pods.create(&PostParams::default(), &pod).await?;
                "Provisioning".to_string()
            }
        }
    };

    let services: Api<Service> = Api::namespaced(ctx.client.clone(), ns);
    let service = build_sandbox_service(sandbox);
    services
        .patch(
            &service_name(&sandbox.name_any()),
            &pp,
            &Patch::Apply(&service),
        )
        .await?;

    let netpols: Api<NetworkPolicy> = Api::namespaced(ctx.client.clone(), ns);
    let netpol = build_sandbox_netpol(sandbox, &project.spec.owner);
    netpols
        .patch(
            &netpol_name(&sandbox.name_any()),
            &pp,
            &Patch::Apply(&netpol),
        )
        .await?;

    match build_sandbox_egress_netpol(sandbox, &owner.spec.egress, &project.spec.egress) {
        Some(egress_netpol) => {
            netpols
                .patch(
                    &netpol_egress_name(&sandbox.name_any()),
                    &pp,
                    &Patch::Apply(&egress_netpol),
                )
                .await?;
        }
        None => {
            let name = netpol_egress_name(&sandbox.name_any());
            if netpols.get_opt(&name).await?.is_some() {
                netpols.delete(&name, &DeleteParams::default()).await?;
            }
        }
    }

    let sandboxes: Api<Sandbox> = Api::namespaced(ctx.client.clone(), ns);
    let status = compute_status(sandbox, &phase);
    let patch = serde_json::json!({ "status": status });
    sandboxes
        .patch_status(
            &sandbox.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(Action::requeue(Duration::from_secs(
        if phase == "Running" { 300 } else { 15 },
    )))
}

async fn cleanup(sandbox: &Sandbox, ctx: &Context, ns: &str) -> Result<Action, ControllerError> {
    let project = match fetch_project(sandbox, ctx, ns).await {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!(
                sandbox = %sandbox.name_any(),
                "project introuvable pendant le cleanup — retrait de worktree ignoré (best-effort v1)"
            );
            return Ok(Action::await_change());
        }
    };
    let owner_pvc_name = match fetch_owner(&project, ctx, ns).await {
        Ok(o) => o.status.and_then(|s| s.pvc_name),
        Err(_) => None,
    };
    let Some(owner_pvc_name) = owner_pvc_name else {
        tracing::warn!(
            sandbox = %sandbox.name_any(),
            "owner introuvable pendant le cleanup — retrait de worktree ignoré (best-effort v1)"
        );
        return Ok(Action::await_change());
    };

    let job_ctx = ProjectJobContext {
        sandbox_image: ctx.default_image.clone(),
        owner_pvc_name,
    };
    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let name = worktree_remove_job_name(&sandbox.name_any());
    let job = jobs.get_opt(&name).await?;
    match job {
        Some(job) if job.status.clone().and_then(|s| s.succeeded).unwrap_or(0) > 0 => {
            Ok(Action::await_change())
        }
        Some(_) => {
            jobs.create(
                &PostParams::default(),
                &build_worktree_remove_job(sandbox, &project, &job_ctx),
            )
            .await?;
            Err(ControllerError::WorktreeRemovalPending {
                sandbox: sandbox.name_any(),
            })
        }
        None => {
            jobs.create(
                &PostParams::default(),
                &build_worktree_remove_job(sandbox, &project, &job_ctx),
            )
            .await?;
            Err(ControllerError::WorktreeRemovalPending {
                sandbox: sandbox.name_any(),
            })
        }
    }
}

pub async fn reconcile(
    sandbox: Arc<Sandbox>,
    ctx: Arc<Context>,
) -> Result<Action, ControllerError> {
    let ns = sandbox.namespace().unwrap_or_else(|| "default".to_string());
    let api: Api<Sandbox> = Api::namespaced(ctx.client.clone(), &ns);
    finalizer(&api, FINALIZER, sandbox, |event| async {
        match event {
            Event::Apply(s) => apply(&s, &ctx, &ns).await,
            Event::Cleanup(s) => cleanup(&s, &ctx, &ns).await,
        }
    })
    .await
    .map_err(|e| ControllerError::Finalizer(e.to_string()))
}

pub fn error_policy(_sandbox: Arc<Sandbox>, error: &ControllerError, _ctx: Arc<Context>) -> Action {
    match error {
        ControllerError::WorktreeRemovalPending { .. } => {
            tracing::info!(%error, "worktree removal pending, requeue in 10s");
            Action::requeue(Duration::from_secs(10))
        }
        ControllerError::ProjectNotReady { .. } => {
            tracing::info!(%error, "project not cloned yet, requeue in 15s");
            Action::requeue(Duration::from_secs(15))
        }
        _ => {
            tracing::warn!(%error, "sandbox reconcile error, requeue in 30s");
            Action::requeue(Duration::from_secs(30))
        }
    }
}

pub fn build_controller(client: Client) -> Controller<Sandbox> {
    let sandboxes: Api<Sandbox> = Api::all(client);
    Controller::new(sandboxes, kube::runtime::watcher::Config::default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use vanyline_crds::{ProjectSpec, PvcRef, SandboxSpec};

    fn make_ctx() -> SandboxPodContext {
        SandboxPodContext {
            owner_name: "alice".to_string(),
            owner_pvc_name: "owner-alice-home".to_string(),
            owner_service_account: "owner-alice".to_string(),
            default_image: "registry.example/vanyline-sandbox:latest".to_string(),
        }
    }

    fn make_sandbox(name: &str, toolchains: Vec<Toolchain>, image: Option<String>) -> Sandbox {
        let mut sandbox = Sandbox::new(
            name,
            SandboxSpec {
                project: "demo".to_string(),
                branch: "main".to_string(),
                toolchains,
                image,
                resources: None,
                egress: Vec::new(),
            },
        );
        sandbox.meta_mut().namespace = Some("ns".into());
        sandbox.meta_mut().uid = Some(format!("test-uid-{name}"));
        sandbox
    }

    fn make_rust_tc(env: std::collections::BTreeMap<String, String>) -> Toolchain {
        Toolchain {
            name: "rust".to_string(),
            image: "rust:slim-trixie".to_string(),
            env,
        }
    }

    fn make_node_tc(env: std::collections::BTreeMap<String, String>) -> Toolchain {
        Toolchain {
            name: "node".to_string(),
            image: "node:trixie-slim".to_string(),
            env,
        }
    }

    fn make_project(existing_pvc: Option<PvcRef>, caches: Option<Vec<String>>) -> Project {
        let mut project = Project::new(
            "demo",
            ProjectSpec {
                owner: "alice".to_string(),
                repo_url: "https://github.com/owner/repo".to_string(),
                default_branch: None,
                existing_pvc,
                storage_size: None,
                storage_class: None,
                git_secret: None,
                caches,
                fetch_interval: None,
                egress: Vec::new(),
            },
        );
        project.meta_mut().namespace = Some("ns".into());
        project
    }

    fn make_job_ctx() -> ProjectJobContext {
        ProjectJobContext {
            sandbox_image: "registry.example/vanyline-sandbox:latest".to_string(),
            owner_pvc_name: "owner-alice-home".to_string(),
        }
    }

    // ===== aggregate_toolchain_env =====

    #[test]
    fn rust_preset() {
        let toolchain = make_rust_tc(Default::default());
        let env = aggregate_toolchain_env(&[toolchain]);

        // PATH must contain resolved paths
        let path = env
            .iter()
            .find(|e| e.name == "PATH")
            .expect("must have PATH");
        let path_val = path.value.as_ref().expect("PATH must have value");
        assert!(path_val.contains("/toolchains/rust/usr/local/cargo/bin"));
        assert!(path_val.contains("/toolchains/rust/usr/bin"));
        assert!(path_val.ends_with(BASE_PATH));

        // LD_LIBRARY_PATH
        let ld = env
            .iter()
            .find(|e| e.name == "LD_LIBRARY_PATH")
            .expect("must have LD_LIBRARY_PATH");
        let ld_val = ld.value.as_ref().expect("LD_LIBRARY_PATH must have value");
        assert!(ld_val.contains("/toolchains/rust/usr/lib/x86_64-linux-gnu"));
        assert!(ld_val.contains("/toolchains/rust/usr/lib/aarch64-linux-gnu"));

        // RUSTUP_HOME
        let rustup = env
            .iter()
            .find(|e| e.name == "RUSTUP_HOME")
            .expect("must have RUSTUP_HOME");
        let rustup_val = rustup.value.as_ref().expect("RUSTUP_HOME must have value");
        assert_eq!(rustup_val, "/toolchains/rust/usr/local/rustup");
    }

    #[test]
    fn node_preset() {
        let toolchain = make_node_tc(Default::default());
        let env = aggregate_toolchain_env(&[toolchain]);

        let path = env
            .iter()
            .find(|e| e.name == "PATH")
            .expect("must have PATH");
        let path_val = path.value.as_ref().expect("PATH must have value");
        assert!(path_val.contains("/toolchains/node/usr/local/bin"));

        let ld = env
            .iter()
            .find(|e| e.name == "LD_LIBRARY_PATH")
            .expect("must have LD_LIBRARY_PATH");
        let ld_val = ld.value.as_ref().expect("LD_LIBRARY_PATH must have value");
        assert!(ld_val.contains("/toolchains/node/usr/lib/x86_64-linux-gnu"));
        assert!(ld_val.contains("/toolchains/node/usr/lib/aarch64-linux-gnu"));

        // No RUSTUP_HOME for node
        assert!(env.iter().find(|e| e.name == "RUSTUP_HOME").is_none());
    }

    #[test]
    fn multiple_toolchains_order_preserved() {
        let rust = make_rust_tc(Default::default());
        let node = make_node_tc(Default::default());
        let env = aggregate_toolchain_env(&[rust, node]);

        let path = env
            .iter()
            .find(|e| e.name == "PATH")
            .expect("must have PATH");
        let path_val = path.value.as_ref().expect("PATH must have value");
        // rust first, then node, then BASE_PATH
        let rust_bin = "/toolchains/rust/usr/local/cargo/bin";
        let rust_bin2 = "/toolchains/rust/usr/bin";
        let node_bin = "/toolchains/node/usr/local/bin";
        // rust paths must appear before node path
        assert!(path_val.find(rust_bin) < path_val.find(node_bin));
        assert!(path_val.find(rust_bin2) < path_val.find(node_bin));
        assert!(path_val.ends_with(BASE_PATH));

        let ld = env
            .iter()
            .find(|e| e.name == "LD_LIBRARY_PATH")
            .expect("must have LD_LIBRARY_PATH");
        let ld_val = ld.value.as_ref().expect("LD_LIBRARY_PATH must have value");
        assert!(ld_val.find("/toolchains/rust") < ld_val.find("/toolchains/node"));
    }

    #[test]
    fn explicit_env_overrides_preset() {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "{root}/bin".to_string());
        let toolchain = Toolchain {
            name: "custom".to_string(),
            image: "x".to_string(),
            env,
        };
        let env_result = aggregate_toolchain_env(&[toolchain]);

        let path = env_result
            .iter()
            .find(|e| e.name == "PATH")
            .expect("must have PATH");
        let path_val = path.value.as_ref().expect("PATH must have value");
        assert!(path_val.contains("/toolchains/custom/bin"));
        assert!(path_val.ends_with(BASE_PATH));

        // No LD_LIBRARY_PATH because custom has none
        assert!(env_result
            .iter()
            .find(|e| e.name == "LD_LIBRARY_PATH")
            .is_none());
    }

    #[test]
    fn unknown_toolchain_no_preset_no_env() {
        let toolchain = Toolchain {
            name: "unknown".to_string(),
            image: "x".to_string(),
            env: Default::default(),
        };
        let env_result = aggregate_toolchain_env(&[toolchain]);

        // PATH == BASE_PATH exactly (no segments added)
        let path = env_result
            .iter()
            .find(|e| e.name == "PATH")
            .expect("must have PATH");
        let path_val = path.value.as_ref().expect("PATH must have value");
        assert_eq!(path_val, BASE_PATH);

        // No LD_LIBRARY_PATH
        assert!(env_result
            .iter()
            .find(|e| e.name == "LD_LIBRARY_PATH")
            .is_none());
    }

    // ===== build_sandbox_pod =====

    #[test]
    fn pod_name_and_namespace() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        assert_eq!(pod.metadata.name, Some("sandbox-demo-branch".to_string()));
        assert_eq!(pod.metadata.namespace, Some("ns".to_string()));
    }

    #[test]
    fn owner_reference_and_labels() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let refs = pod
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo-branch");
        assert_eq!(refs[0].kind, "Sandbox".to_string());

        let labels = pod.metadata.labels.as_ref().expect("should have labels");
        assert_eq!(
            labels.get("vanyline.solidite.fr/owner").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            labels
                .get("vanyline.solidite.fr/project")
                .map(String::as_str),
            Some("demo")
        );
        assert_eq!(
            labels
                .get("vanyline.solidite.fr/sandbox")
                .map(String::as_str),
            Some("demo-branch")
        );
    }

    #[test]
    fn service_account_from_context() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let spec = pod.spec.as_ref().expect("should have spec");
        assert_eq!(spec.service_account_name, Some("owner-alice".to_string()));
    }

    #[test]
    fn image_default_vs_spec() {
        // No image -> use default
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);
        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        assert_eq!(container.image, Some(ctx.default_image.clone()));

        // With image -> use spec
        let sandbox_with_image =
            make_sandbox("demo-branch", vec![], Some("custom:tag".to_string()));
        let pod2 = build_sandbox_pod(&sandbox_with_image, &project, &ctx);
        let container2 = pod2.spec.as_ref().unwrap().containers[0].clone();
        assert_eq!(container2.image, Some("custom:tag".to_string()));
    }

    #[test]
    fn no_auth_arg() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let args = container.args.as_ref().expect("should have args");
        assert_eq!(args, &["--no-auth".to_string()]);
    }

    #[test]
    fn mcp_port_and_probes() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();

        // Port
        let ports = container.ports.as_ref().expect("should have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].container_port, MCP_PORT);
        assert_eq!(ports[0].name, Some("mcp".to_string()));

        // Probes
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
            IntOrString::Int(MCP_PORT)
        );
    }

    #[test]
    fn vnl_sandbox_root_env() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let env = container.env.as_ref().expect("should have env");
        let vnl = env
            .iter()
            .find(|e| e.name == "VNL_SANDBOX_ROOT")
            .expect("should have VNL_SANDBOX_ROOT");
        let val = vnl
            .value
            .as_ref()
            .expect("VNL_SANDBOX_ROOT should have value");
        assert_eq!(val, &format!("{HOME_MOUNT_PATH}/workspace"));
    }

    #[test]
    fn volumes_home_workspace_and_toolchains() {
        let rust = make_rust_tc(Default::default());
        let node = make_node_tc(Default::default());
        let sandbox = make_sandbox("demo-branch", vec![rust, node], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let volumes = pod
            .spec
            .as_ref()
            .unwrap()
            .volumes
            .as_ref()
            .expect("should have volumes");

        // Should have: home, workspace, toolchain-rust, toolchain-node
        assert_eq!(volumes.len(), 4);

        let names: Vec<_> = volumes.iter().map(|v| v.name.as_str()).collect();
        assert!(names.contains(&"home"));
        assert!(names.contains(&"workspace"));
        assert!(names.contains(&"toolchain-rust"));
        assert!(names.contains(&"toolchain-node"));

        // Check toolchain volumes have image reference and pull policy
        for vol in volumes {
            if vol.name.starts_with("toolchain-") {
                let src = vol
                    .image
                    .as_ref()
                    .expect("toolchain volume should have image source");
                assert!(src.reference.is_some());
                assert_eq!(src.pull_policy, Some("IfNotPresent".to_string()));
            }
        }
    }

    #[test]
    fn worktree_mount_no_existing_pvc() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volume_mounts");
        let ws_mount = mounts
            .iter()
            .find(|m| m.name == "workspace")
            .expect("should have workspace mount");

        assert_eq!(
            ws_mount.mount_path.as_str(),
            &(format!("{HOME_MOUNT_PATH}/workspace"))
        );
        assert_eq!(ws_mount.sub_path, Some("worktrees/demo-branch".to_string()));
    }

    #[test]
    fn worktree_mount_with_existing_pvc_subpath() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(
            Some(PvcRef {
                name: "code-server-home".into(),
                sub_path: Some("demo".into()),
            }),
            None,
        );
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volume_mounts");
        let ws_mount = mounts
            .iter()
            .find(|m| m.name == "workspace")
            .expect("should have workspace mount");

        assert_eq!(
            ws_mount.mount_path.as_str(),
            &(format!("{HOME_MOUNT_PATH}/workspace"))
        );
        assert_eq!(
            ws_mount.sub_path,
            Some("demo/worktrees/demo-branch".to_string())
        );
    }

    #[test]
    fn cache_mounts_and_env() {
        // No caches specified -> default ["cargo", "pnpm"]
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None); // caches: None -> defaults
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volume_mounts");

        // Should have: workspace, cargo cache, pnpm cache + toolchain mounts
        let cargo_mount = mounts
            .iter()
            .find(|m| m.mount_path == "/project-cache/cargo")
            .expect("should have cargo mount");
        assert_eq!(cargo_mount.sub_path, Some("cache/cargo".to_string()));

        let pnpm_mount = mounts
            .iter()
            .find(|m| m.mount_path == "/project-cache/pnpm-store")
            .expect("should have pnpm-store mount");
        assert_eq!(pnpm_mount.sub_path, Some("cache/pnpm-store".to_string()));

        // Env vars
        let env = container.env.as_ref().expect("should have env");
        let cargo_home = env
            .iter()
            .find(|e| e.name == "CARGO_HOME")
            .expect("should have CARGO_HOME");
        assert_eq!(cargo_home.value, Some("/project-cache/cargo".to_string()));

        let pnpm_dir = env
            .iter()
            .find(|e| e.name == "npm_config_store_dir")
            .expect("should have npm_config_store_dir");
        assert_eq!(
            pnpm_dir.value,
            Some("/project-cache/pnpm-store".to_string())
        );
    }

    #[test]
    fn resources_passthrough() {
        use k8s_openapi::api::core::v1::ResourceRequirements;
        use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
        use std::collections::BTreeMap;

        // With resources
        let mut requests = BTreeMap::new();
        requests.insert("cpu".to_string(), Quantity("100m".into()));
        let rr = ResourceRequirements {
            limits: None,
            requests: Some(requests),
            claims: None,
        };

        let sandbox = make_sandbox("demo-branch", vec![], None);
        let mut sandbox = sandbox;
        sandbox.spec.resources = Some(rr.clone());

        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        assert!(container.resources.is_some());
        let res = container.resources.as_ref().unwrap();
        assert_eq!(
            res.requests.as_ref().unwrap().get("cpu"),
            Some(&Quantity("100m".into()))
        );

        // Without resources
        let sandbox_no_res = make_sandbox("demo-branch", vec![], None);
        let pod_no_res = build_sandbox_pod(&sandbox_no_res, &project, &ctx);
        assert!(pod_no_res.spec.as_ref().unwrap().containers[0]
            .resources
            .is_none());
    }

    // ===== Job name helpers =====

    #[test]
    fn checkout_job_name_and_worktree_remove_job_name() {
        assert_eq!(
            checkout_job_name("demo-branch"),
            "sandbox-demo-branch-checkout"
        );
        assert_eq!(
            worktree_remove_job_name("demo-branch"),
            "sandbox-demo-branch-remove"
        );
    }

    // ===== build_checkout_job =====

    #[test]
    fn build_checkout_job_argv_no_default_branch() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let job_ctx = make_job_ctx();
        let job = build_checkout_job(&sandbox, &project, &job_ctx);

        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
        let command = pod_spec.containers[0].command.as_ref().unwrap();
        assert_eq!(
            command,
            &vec![
                "vanyline-maint".to_string(),
                "checkout".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
                "--sandbox".to_string(),
                "demo-branch".to_string(),
                "--branch".to_string(),
                "main".to_string(),
            ]
        );
    }

    #[test]
    fn build_checkout_job_argv_with_default_branch() {
        let mut project = make_project(None, None);
        project.spec.default_branch = Some("develop".to_string());
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let job_ctx = make_job_ctx();
        let job = build_checkout_job(&sandbox, &project, &job_ctx);

        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
        let command = pod_spec.containers[0].command.as_ref().unwrap();
        assert_eq!(
            command,
            &vec![
                "vanyline-maint".to_string(),
                "checkout".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
                "--sandbox".to_string(),
                "demo-branch".to_string(),
                "--branch".to_string(),
                "main".to_string(),
                "--default-branch".to_string(),
                "develop".to_string(),
            ]
        );
    }

    #[test]
    fn build_checkout_job_owner_reference() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let job_ctx = make_job_ctx();
        let job = build_checkout_job(&sandbox, &project, &job_ctx);

        let refs = job
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo-branch");
        assert_eq!(refs[0].kind, "Sandbox");
    }

    // ===== build_worktree_remove_job =====

    #[test]
    fn build_worktree_remove_job_argv() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let job_ctx = make_job_ctx();
        let job = build_worktree_remove_job(&sandbox, &project, &job_ctx);

        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
        let command = pod_spec.containers[0].command.as_ref().unwrap();
        assert_eq!(
            command,
            &vec![
                "vanyline-maint".to_string(),
                "remove".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
                "--sandbox".to_string(),
                "demo-branch".to_string(),
            ]
        );
    }

    // ===== build_sandbox_service =====

    #[test]
    fn build_sandbox_service_shape() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let service = build_sandbox_service(&sandbox);

        assert_eq!(
            service.metadata.name,
            Some("sandbox-demo-branch".to_string())
        );

        let spec = service.spec.as_ref().expect("should have spec");
        let selector = spec.selector.as_ref().expect("should have selector");
        assert_eq!(
            selector
                .get("vanyline.solidite.fr/sandbox")
                .map(String::as_str),
            Some("demo-branch")
        );

        let ports = spec.ports.as_ref().expect("should have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, MCP_PORT);
        assert_eq!(ports[0].name, Some("mcp".to_string()));
    }

    // ===== build_sandbox_netpol =====

    #[test]
    fn build_sandbox_netpol_shape() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let netpol = build_sandbox_netpol(&sandbox, "alice");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let pod_sel = spec
            .pod_selector
            .as_ref()
            .expect("should have pod_selector");
        let labels = pod_sel
            .match_labels
            .as_ref()
            .expect("should have match_labels");
        assert_eq!(
            labels
                .get("vanyline.solidite.fr/sandbox")
                .map(String::as_str),
            Some("demo-branch")
        );

        let ingress = spec.ingress.as_ref().expect("should have ingress");
        assert_eq!(ingress.len(), 1);

        let from = ingress[0].from.as_ref().expect("should have from");
        assert_eq!(from.len(), 1);
        let peer_pod_sel = from[0]
            .pod_selector
            .as_ref()
            .expect("should have pod_selector in peer");
        let peer_labels = peer_pod_sel
            .match_labels
            .as_ref()
            .expect("should have match_labels");
        assert_eq!(
            peer_labels
                .get("vanyline.solidite.fr/owner")
                .map(String::as_str),
            Some("alice")
        );

        let ports = ingress[0].ports.as_ref().expect("should have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, Some(IntOrString::Int(MCP_PORT)));

        assert_eq!(spec.policy_types, Some(vec!["Ingress".to_string()]));
    }

    // ===== compute_status =====

    #[test]
    fn compute_status_running() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let status = compute_status(&sandbox, "Running");

        assert_eq!(status.phase, Some("Running".to_string()));
        assert_eq!(status.service, Some("sandbox-demo-branch".to_string()));
        assert_eq!(status.conditions.len(), 1);
        let cond = &status.conditions[0];
        assert_eq!(cond.status, "True");
        assert_eq!(cond.reason, "PodRunning");
    }

    #[test]
    fn compute_status_provisioning() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let status = compute_status(&sandbox, "Provisioning");

        assert_eq!(status.phase, Some("Provisioning".to_string()));
        assert_eq!(status.service, Some("sandbox-demo-branch".to_string()));
        assert_eq!(status.conditions.len(), 1);
        let cond = &status.conditions[0];
        assert_eq!(cond.status, "False");
        assert_eq!(cond.reason, "NotRunning");
    }

    // ===== bare_repo mount (tâche 00) =====

    #[test]
    fn bare_repo_mount_present_no_existing_pvc() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volume_mounts");

        // Find the mount by exact mount_path (unique for this bare_repo mount)
        let bare_repo_mount = mounts
            .iter()
            .find(|m| m.mount_path == "/workspace/repo.git")
            .expect("should have bare repo mount at /workspace/repo.git");

        assert_eq!(
            bare_repo_mount.sub_path,
            Some("repo.git".to_string())
        );
    }

    #[test]
    fn bare_repo_mount_present_with_existing_pvc_subpath() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(
            Some(PvcRef {
                name: "code-server-home".into(),
                sub_path: Some("demo".into()),
            }),
            None,
        );
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volume_mounts");

        let bare_repo_mount = mounts
            .iter()
            .find(|m| m.mount_path == "/workspace/repo.git")
            .expect("should have bare repo mount at /workspace/repo.git");

        assert_eq!(
            bare_repo_mount.sub_path,
            Some("demo/repo.git".to_string())
        );
    }

    #[test]
    fn bare_repo_mount_is_writable() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let project = make_project(None, None);
        let ctx = make_ctx();
        let pod = build_sandbox_pod(&sandbox, &project, &ctx);

        let container = pod.spec.as_ref().unwrap().containers[0].clone();
        let mounts = container
            .volume_mounts
            .as_ref()
            .expect("should have volume_mounts");

        let bare_repo_mount = mounts
            .iter()
            .find(|m| m.mount_path == "/workspace/repo.git")
            .expect("should have bare repo mount at /workspace/repo.git");

        // read_only must NOT be Some(true)
        assert_ne!(bare_repo_mount.read_only, Some(true));
    }

    // ===== netpol_egress_name =====

    #[test]
    fn netpol_egress_name_format() {
        assert_eq!(
            netpol_egress_name("demo-branch"),
            "sandbox-demo-branch-egress"
        );
    }

    // ===== build_sandbox_egress_netpol =====

    #[test]
    fn build_sandbox_egress_netpol_none_when_all_empty() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress: &[EgressRule] = &[];
        let project_egress: &[EgressRule] = &[];

        let result = build_sandbox_egress_netpol(&sandbox, owner_egress, project_egress);
        assert!(result.is_none());
    }

    #[test]
    fn build_sandbox_egress_netpol_dns_always_present() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "test".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];
        let project_egress: &[EgressRule] = &[];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, project_egress)
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");
        assert!(egress.len() >= 2, "should have at least DNS + owner rule");

        // First rule is DNS: to = None, ports must include 53 UDP and 53 TCP
        let dns_rule = &egress[0];
        assert!(dns_rule.to.is_none(), "DNS rule must have no destination restriction");
        let dns_ports = dns_rule.ports.as_ref().expect("DNS rule must have ports");
        assert!(dns_ports.len() >= 2, "DNS rule must have UDP and TCP");

        let has_udp = dns_ports.iter().any(|p| {
            p.port == Some(IntOrString::Int(53)) && p.protocol.as_deref() == Some("UDP")
        });
        let has_tcp = dns_ports.iter().any(|p| {
            p.port == Some(IntOrString::Int(53)) && p.protocol.as_deref() == Some("TCP")
        });
        assert!(has_udp, "DNS rule must have 53/UDP");
        assert!(has_tcp, "DNS rule must have 53/TCP");
    }

    #[test]
    fn build_sandbox_egress_netpol_union_three_levels() {
        let mut sandbox = make_sandbox("demo-branch", vec![], None);
        sandbox.spec.egress = vec![EgressRule {
            description: "sandbox rule".into(),
            cidr: Some("10.0.2.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];

        let owner_egress = vec![EgressRule {
            description: "owner rule".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];
        let project_egress = vec![EgressRule {
            description: "project rule".into(),
            cidr: Some("10.0.1.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &project_egress)
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");
        // DNS + 3 rules = 4
        assert_eq!(egress.len(), 4);

        // Collect all CIDRs from the non-DNS rules (egress[1..4])
        let mut found_cidrs: Vec<String> = egress[1..]
            .iter()
            .filter_map(|rule| {
                rule.to.as_ref()
                    .and_then(|peers| peers.first())
                    .and_then(|peer| peer.ip_block.as_ref())
                    .map(|ip| ip.cidr.clone())
            })
            .collect();
        found_cidrs.sort();

        assert!(found_cidrs.contains(&"10.0.0.0/24".to_string()));
        assert!(found_cidrs.contains(&"10.0.1.0/24".to_string()));
        assert!(found_cidrs.contains(&"10.0.2.0/24".to_string()));
        assert_eq!(found_cidrs.len(), 3);
    }

    #[test]
    fn build_sandbox_egress_netpol_cidr_rule() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "cidr rule".into(),
            cidr: Some("10.42.7.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");

        // The CIDR rule is egress[1] (second after DNS)
        let cidr_rule = &egress[1];
        let peer = cidr_rule.to.as_ref().expect("rule must have to").first().expect("must have one peer");
        let ip_block = peer.ip_block.as_ref().expect("peer must have ip_block");
        assert_eq!(ip_block.cidr, "10.42.7.0/24");
        assert!(peer.pod_selector.is_none());
    }

    #[test]
    fn build_sandbox_egress_netpol_selector_rule() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "selector rule".into(),
            cidr: None,
            pod_selector: Some(LabelSelector {
                match_labels: Some(BTreeMap::from([("app".into(), "registry".into())])),
                ..Default::default()
            }),
            namespace_selector: Some(LabelSelector {
                match_labels: Some(BTreeMap::from([(
                    "kubernetes.io/metadata.name".into(),
                    "tools".into(),
                )])),
                ..Default::default()
            }),
            ports: vec![],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");

        let selector_rule = &egress[1];
        let peer = selector_rule.to.as_ref().expect("rule must have to").first().expect("must have one peer");
        assert!(peer.ip_block.is_none());

        let pod_sel = peer.pod_selector.as_ref().expect("pod_selector must be set");
        let pod_labels = pod_sel.match_labels.as_ref().expect("pod_selector match_labels");
        assert_eq!(pod_labels.get("app"), Some(&"registry".to_string()));

        let ns_sel = peer.namespace_selector.as_ref().expect("namespace_selector must be set");
        let ns_labels = ns_sel.match_labels.as_ref().expect("namespace_selector match_labels");
        assert_eq!(
            ns_labels.get("kubernetes.io/metadata.name"),
            Some(&"tools".to_string())
        );
    }

    #[test]
    fn build_sandbox_egress_netpol_cidr_precedence_over_selector() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "both cidr and selector".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: Some(LabelSelector {
                match_labels: Some(BTreeMap::from([("app".into(), "registry".into())])),
                ..Default::default()
            }),
            namespace_selector: None,
            ports: vec![],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");

        let rule = &egress[1];
        let peer = rule.to.as_ref().expect("rule must have to").first().expect("must have one peer");
        let ip_block = peer.ip_block.as_ref().expect("cidr wins, ip_block must be set");
        assert_eq!(ip_block.cidr, "10.0.0.0/24");
        assert!(peer.pod_selector.is_none(), "pod_selector must be None when cidr wins");
    }

    #[test]
    fn build_sandbox_egress_netpol_ports_restriction() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "port rule".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![EgressPort {
                port: 443,
                protocol: Some("TCP".into()),
            }],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");
        let rule = &egress[1];

        let ports = rule.ports.as_ref().expect("rule must have ports");
        assert_eq!(ports.len(), 1);
        assert_eq!(
            *ports,
            vec![NetworkPolicyPort {
                port: Some(IntOrString::Int(443)),
                protocol: Some("TCP".to_string()),
                end_port: None,
            }]
        );
    }

    #[test]
    fn build_sandbox_egress_netpol_empty_ports_means_all_ports() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "no ports".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");

        // Rule at index 1 (second after DNS) has no port restriction
        let rule = &egress[1];
        assert!(rule.ports.is_none(), "empty ports => None (all ports allowed)");
    }

    #[test]
    fn build_sandbox_egress_netpol_protocol_defaults_to_tcp() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "tcp default".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![EgressPort {
                port: 8080,
                protocol: None,
            }],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        let spec = netpol.spec.as_ref().expect("should have spec");
        let egress = spec.egress.as_ref().expect("should have egress");

        let rule = &egress[1];
        let ports = rule.ports.as_ref().expect("rule must have ports");
        assert_eq!(
            ports[0].protocol,
            Some("TCP".to_string()),
            "protocol should default to TCP"
        );
    }

    #[test]
    fn build_sandbox_egress_netpol_shape() {
        let sandbox = make_sandbox("demo-branch", vec![], None);
        let owner_egress = vec![EgressRule {
            description: "simple rule".into(),
            cidr: Some("10.0.0.0/24".into()),
            pod_selector: None,
            namespace_selector: None,
            ports: vec![],
        }];

        let netpol = build_sandbox_egress_netpol(&sandbox, &owner_egress, &[])
            .expect("should return Some");

        assert_eq!(
            netpol.metadata.name,
            Some(netpol_egress_name("demo-branch"))
        );

        // Verify pod_selector matches sandbox
        let spec = netpol.spec.as_ref().expect("should have spec");
        let pod_sel = spec
            .pod_selector
            .as_ref()
            .expect("should have pod_selector");
        let labels = pod_sel
            .match_labels
            .as_ref()
            .expect("should have match_labels");
        assert_eq!(
            labels
                .get("vanyline.solidite.fr/sandbox")
                .map(String::as_str),
            Some("demo-branch")
        );

        assert_eq!(
            spec.policy_types,
            Some(vec!["Egress".to_string()])
        );

        // Verify owner_references
        let refs = netpol
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, "Sandbox");
        assert_eq!(refs[0].name, "demo-branch");
    }
}
