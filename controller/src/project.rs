use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use k8s_openapi::api::batch::v1::{CronJob, CronJobSpec, Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, PersistentVolumeClaim, PersistentVolumeClaimSpec,
    PersistentVolumeClaimVolumeSource, PodSecurityContext, PodSpec, PodTemplateSpec,
    SecretVolumeSource, ServiceAccount, Volume, VolumeMount,
    VolumeResourceRequirements,
};
use k8s_openapi::api::rbac::v1::{PolicyRule, Role, RoleBinding, RoleRef, Subject};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, ObjectMeta, Time};
use kube::api::{Api, ListParams, Patch, PatchParams, PostParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::finalizer::{Event, finalizer};
use kube::{Client, Resource, ResourceExt};

use crate::error::ControllerError;
use crate::owner;
use crate::owner::HOME_MOUNT_PATH;
use vanyline_crds::Project;
use vanyline_crds::{Owner, ProjectStatus, Sandbox};

/// Point de montage du PVC workspace dans les pods (Jobs et, plus tard, Sandbox).
#[allow(dead_code)]
pub const WORKSPACE_MOUNT_PATH: &str = "/workspace";
#[allow(dead_code)]
const DEFAULT_WORKSPACE_SIZE: &str = "10Gi";
#[allow(dead_code)]
const DEFAULT_WORKSPACE_ACCESS_MODE: &str = "ReadWriteOnce";

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
        .map_or_else(|| project_pvc_name(&project.name_any()), |r| r.name.clone())
}

/// `subPath` à utiliser lors du montage du PVC — `Some` uniquement quand le repo
/// vit dans un sous-répertoire d'un PVC existant (cas kydah-code). `None` quand le
/// PVC est créé par le controller (le mount utilise tout le volume) ou que
/// `existing_pvc.sub_path` n'est pas renseigné.
#[allow(dead_code)]
pub fn effective_sub_path(project: &Project) -> Option<String> {
    project
        .spec
        .existing_pvc
        .as_ref()
        .and_then(|r| r.sub_path.clone())
}

/// Chemin (relatif à la racine du volume, donc à `WORKSPACE_MOUNT_PATH` une fois
/// monté) du clone bare du remote.
#[allow(dead_code)]
pub const fn bare_repo_path() -> &'static str {
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
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
fn owner_reference(
    project: &Project,
) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
    project
        .controller_owner_ref(&())
        .expect("Project a apiVersion/kind renseignés par le derive CustomResource")
}

/// Nom commun des objets RBAC des Jobs de maintenance d'un Project :
/// ServiceAccount, Role et RoleBinding — `project-<name>-maint` (même patron
/// que `owner-<name>` / `application-<name>`).
#[allow(dead_code)]
pub fn maint_sa_name(project_name: &str) -> String {
    format!("project-{project_name}-maint")
}

/// `ServiceAccount` des Jobs git (init/fetch exécutent `detect`, qui patche
/// `Project.status` via l'API K8s). `ownerReference` vers le Project — la GC
/// en cascade nettoie SA/Role/RoleBinding à la suppression du Project.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
#[allow(dead_code)]
pub fn build_maint_service_account(project: &Project) -> ServiceAccount {
    ServiceAccount {
        metadata: ObjectMeta {
            name: Some(maint_sa_name(&project.name_any())),
            namespace: project.namespace(),
            owner_references: Some(vec![owner_reference(project)]),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// `Role` (namespaced) donnant aux Jobs de maintenance exactement le droit
/// utilisé par `vanyline-maint detect --project` : `patch` sur le status
/// subresource des Projects (`projects/status: patch`). Rien de plus — les
/// Jobs n'ont pas besoin de CRUD sur les CRs.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
#[allow(dead_code)]
pub fn build_maint_role(project: &Project) -> Role {
    Role {
        metadata: ObjectMeta {
            name: Some(maint_sa_name(&project.name_any())),
            namespace: project.namespace(),
            owner_references: Some(vec![owner_reference(project)]),
            ..Default::default()
        },
        rules: Some(vec![PolicyRule {
            api_groups: Some(vec!["vanyline.solidite.fr".to_string()]),
            resources: Some(vec!["projects/status".to_string()]),
            verbs: vec!["patch".to_string()],
            ..Default::default()
        }]),
    }
}

/// `RoleBinding` liant le `ServiceAccount` des Jobs au `Role` ci-dessus —
/// même nom (`project-<name>-maint`) pour les trois objets.
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
#[allow(dead_code)]
pub fn build_maint_role_binding(project: &Project) -> RoleBinding {
    let name = maint_sa_name(&project.name_any());
    RoleBinding {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: project.namespace(),
            owner_references: Some(vec![owner_reference(project)]),
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
            namespace: project.namespace(),
            ..Default::default()
        }]),
    }
}

/// Construit le PVC workspace à créer. Retourne `None` si `existing_pvc` est
/// fourni : un PVC référencé n'est jamais créé ni géré par le controller (mêmes
/// règles que `owner::build_home_pvc`).
#[allow(dead_code)]
///
/// `default_size`/`default_class`/`default_access_mode` sont les valeurs
/// `Owner.spec.project_defaults` résolues par l'appelant (le reconciler
/// Project ira chercher l'Owner — cette fonction reste pure et ne fait aucun
/// appel réseau). Priorité : `spec.storage_size`/`spec.storage_class`/
/// `spec.storage_access_mode` > défauts passés en paramètre >
/// `DEFAULT_WORKSPACE_SIZE` ("10Gi")/`DEFAULT_WORKSPACE_ACCESS_MODE`
/// ("`ReadWriteOnce`") ; pas de valeur de repli pour la storage class (`None`
/// => `StorageClass` par défaut du cluster).
pub fn build_workspace_pvc(
    project: &Project,
    default_size: Option<&str>,
    default_class: Option<&str>,
    default_access_mode: Option<&str>,
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
    let access_mode = project
        .spec
        .storage_access_mode
        .clone()
        .or_else(|| default_access_mode.map(str::to_string))
        .unwrap_or_else(|| DEFAULT_WORKSPACE_ACCESS_MODE.to_string());

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
            access_modes: Some(vec![access_mode]),
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

/// Context nécessaire pour les builders de Jobs git. Résolu par l'appelant (le reconciler Project
/// ira chercher l'image sandbox par défaut du controller, éventuellement surchargée).
#[allow(dead_code)]
pub struct ProjectJobContext {
    /// Image utilisée par tous les Jobs git (elle contient git — c'est l'image sandbox).
    pub sandbox_image: String,
    /// Nom du PVC home de l'Owner du Project (résolu par l'appelant depuis `owner::effective_pvc_name`).
    pub owner_pvc_name: String,
}

#[allow(dead_code)]
pub fn init_job_name(project_name: &str) -> String {
    format!("project-{project_name}-init")
}

#[allow(dead_code)]
pub fn fetch_cronjob_name(project_name: &str) -> String {
    format!("project-{project_name}-fetch")
}

#[allow(dead_code)]
pub fn purge_job_name(project_name: &str) -> String {
    format!("project-{project_name}-purge")
}

/// `schedule` de `CronJob` pour `spec.fetch_interval` (défaut `"1h"`) — cf. note `@every` dans le contexte de cette tâche.
#[allow(dead_code)]
pub fn fetch_schedule(project: &Project) -> String {
    let interval = project
        .spec
        .fetch_interval
        .clone()
        .unwrap_or_else(|| "1h".to_string());
    format!("@every {interval}")
}

/// UID/GID non-root partagé par tous les workloads utilisant l'image sandbox
/// (pod sandbox long-running + Jobs `vanyline-maint` : init/fetch/purge/
/// checkout/remove) — ni le terminal utilisateur ni le tool shell MCP ne
/// doivent tourner root. `fs_group` aligne la propriété des PVC déjà montés
/// en écriture (home/workspace) sur ce même GID au montage (mécanisme K8s
/// standard, cf. `fsGroupPolicy` du CSI driver).
pub const SANDBOX_UID: i64 = 1000;

pub fn sandbox_security_context() -> PodSecurityContext {
    PodSecurityContext {
        run_as_user: Some(SANDBOX_UID),
        run_as_group: Some(SANDBOX_UID),
        run_as_non_root: Some(true),
        fs_group: Some(SANDBOX_UID),
        ..Default::default()
    }
}

/// Construit le `PodTemplateSpec` commun aux Jobs git. `init_command` :
/// si `Some`, un initContainer nommé `"git"` l'exécute AVANT le container
/// principal nommé `"detect"` qui exécute `command` (argv direct, jamais de
/// shell) — c'est le mécanisme qui garantit `detect` après `init`/`fetch`.
/// Si `None`, un seul container nommé `"git"` exécute `command` (comportement
/// actuel, ex: `purge`). `service_account_name` : posé sur `PodSpec` quand le
/// job appelle l'API K8s (`detect`). Les env (HOME, GIT_SSH_COMMAND) et les
/// volumeMounts sont appliqués aux deux containers (init et principal).
#[allow(dead_code)]
pub fn git_pod_template(
    project: &Project,
    ctx: &ProjectJobContext,
    init_command: Option<Vec<String>>,
    command: Vec<String>,
    service_account_name: Option<&str>,
) -> PodTemplateSpec {
    let mut volumes = vec![
        Volume {
            name: "workspace".to_string(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: effective_pvc_name(project),
                ..Default::default()
            }),
            ..Default::default()
        },
        Volume {
            name: "home".to_string(),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: ctx.owner_pvc_name.clone(),
                ..Default::default()
            }),
            ..Default::default()
        },
    ];
    let mut mounts = vec![
        VolumeMount {
            name: "workspace".to_string(),
            mount_path: WORKSPACE_MOUNT_PATH.to_string(),
            sub_path: effective_sub_path(project),
            ..Default::default()
        },
        VolumeMount {
            name: "home".to_string(),
            mount_path: HOME_MOUNT_PATH.to_string(),
            ..Default::default()
        },
    ];
    let mut env = vec![EnvVar {
        name: "HOME".to_string(),
        value: Some(HOME_MOUNT_PATH.to_string()),
        ..Default::default()
    }];

    if let Some(secret_name) = &project.spec.git_secret {
        volumes.push(Volume {
            name: "git-secret".to_string(),
            secret: Some(SecretVolumeSource {
                secret_name: Some(secret_name.clone()),
                ..Default::default()
            }),
            ..Default::default()
        });
        mounts.push(VolumeMount {
            name: "git-secret".to_string(),
            mount_path: "/git-secret".to_string(),
            read_only: Some(true),
            ..Default::default()
        });
        env.push(EnvVar {
            name: "GIT_SSH_COMMAND".to_string(),
            value: Some(
                "ssh -i /git-secret/ssh-privatekey -o StrictHostKeyChecking=no".to_string(),
            ),
            ..Default::default()
        });
    }

    let mut labels = BTreeMap::new();
    labels.insert(
        "vanyline.solidite.fr/project".to_string(),
        project.name_any(),
    );

    let (init_containers, containers) = if let Some(init) = init_command {
        let init_env = env.clone();
        let init_mounts = mounts.clone();

        // Re-add git-secret env/mounts to init if they exist (they're in `env`/`mounts`)
        if project.spec.git_secret.is_some() {
            // git-secret env/mounts are already in the cloned `env`/`mounts`
        }

        (
            Some(vec![Container {
                name: "git".to_string(),
                image: Some(ctx.sandbox_image.clone()),
                command: Some(init),
                env: Some(init_env),
                volume_mounts: Some(init_mounts),
                ..Default::default()
            }]),
            vec![Container {
                name: "detect".to_string(),
                image: Some(ctx.sandbox_image.clone()),
                command: Some(command),
                env: Some(env),
                volume_mounts: Some(mounts),
                ..Default::default()
            }],
        )
    } else {
        (
            None,
            vec![Container {
                name: "git".to_string(),
                image: Some(ctx.sandbox_image.clone()),
                command: Some(command),
                env: Some(env),
                volume_mounts: Some(mounts),
                ..Default::default()
            }],
        )
    };

    PodTemplateSpec {
        metadata: Some(ObjectMeta {
            labels: Some(labels),
            ..Default::default()
        }),
        spec: Some(PodSpec {
            init_containers,
            containers,
            restart_policy: Some("Never".to_string()),
            volumes: Some(volumes),
            security_context: Some(sandbox_security_context()),
            service_account_name: service_account_name.map(str::to_string),
            ..Default::default()
        }),
    }
}

/// Job une fois : clone bare le remote (initContainer git) puis lance
/// `detect --project` (container detect) qui patche `Project.status`.
#[allow(dead_code)]
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_init_job(project: &Project, ctx: &ProjectJobContext) -> Job {
    let mut init_command = vec![
        "vanyline-maint".to_string(),
        "init".to_string(),
        "--repo".to_string(),
        project.spec.repo_url.clone(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
    ];
    for cache in effective_caches(project) {
        init_command.push("--cache".to_string());
        init_command.push(cache);
    }
    let detect_command = vec![
        "vanyline-maint".to_string(),
        "detect".to_string(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
        "--project".to_string(),
        project.name_any(),
    ];
    Job {
        metadata: ObjectMeta {
            name: Some(init_job_name(&project.name_any())),
            namespace: project.namespace(),
            owner_references: Some(vec![
                project
                    .controller_owner_ref(&())
                    .expect("Project a apiVersion/kind"),
            ]),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(3),
            ttl_seconds_after_finished: Some(3600),
            template: git_pod_template(
                project,
                ctx,
                Some(init_command),
                detect_command,
                Some(&maint_sa_name(&project.name_any())),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `CronJob` périodique : `git fetch --prune` (initContainer git) puis
/// `detect --project` (container detect) pour rafraîchir la détection.
#[allow(dead_code)]
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_fetch_cronjob(project: &Project, ctx: &ProjectJobContext) -> CronJob {
    let init_command = vec![
        "vanyline-maint".to_string(),
        "fetch".to_string(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
    ];
    let detect_command = vec![
        "vanyline-maint".to_string(),
        "detect".to_string(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
        "--project".to_string(),
        project.name_any(),
    ];

    CronJob {
        metadata: ObjectMeta {
            name: Some(fetch_cronjob_name(&project.name_any())),
            namespace: project.namespace(),
            owner_references: Some(vec![
                project
                    .controller_owner_ref(&())
                    .expect("Project a apiVersion/kind"),
            ]),
            ..Default::default()
        },
        spec: CronJobSpec {
            schedule: fetch_schedule(project),
            job_template: k8s_openapi::api::batch::v1::JobTemplateSpec {
                metadata: None,
                spec: Some(JobSpec {
                    backoff_limit: Some(3),
                    ttl_seconds_after_finished: Some(3600),
                    template: git_pod_template(
                        project,
                        ctx,
                        Some(init_command),
                        detect_command,
                        Some(&maint_sa_name(&project.name_any())),
                    ),
                    ..Default::default()
                }),
            },
            ..Default::default()
        },
        status: None,
    }
}

/// Job de purge : supprime `repo.git`, `worktrees` et `cache` sous le point de
/// montage. Invoqué par le finalizer du reconciler Project (tâche 05) avant
/// suppression d'un PVC créé par le controller — sûr aussi pour un PVC référencé
/// grâce au `subPath` (voir note de contexte).
#[allow(dead_code)]
#[allow(clippy::expect_used)] // garanti par #[derive(CustomResource)] : apiVersion/kind toujours renseignes
pub fn build_purge_job(project: &Project, ctx: &ProjectJobContext) -> Job {
    let command = vec![
        "vanyline-maint".to_string(),
        "purge".to_string(),
        "--workspace".to_string(),
        WORKSPACE_MOUNT_PATH.to_string(),
    ];

    Job {
        metadata: ObjectMeta {
            name: Some(purge_job_name(&project.name_any())),
            namespace: project.namespace(),
            owner_references: Some(vec![
                project
                    .controller_owner_ref(&())
                    .expect("Project a apiVersion/kind"),
            ]),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(3),
            ttl_seconds_after_finished: Some(3600),
            template: git_pod_template(project, ctx, None, command, None),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Identifiant finalizer (tâche 05).
const FINALIZER: &str = "vanyline.solidite.fr/project-cleanup";

/// Contexte client Kubernetes + image sandbox par défaut.
pub struct Context {
    pub client: Client,
    /// Image sandbox par défaut pour tous les Jobs git (contient git). Résolu au
    /// démarrage du controller (`main.rs`) depuis l'env `SANDBOX_IMAGE`.
    pub sandbox_image: String,
}

/// Status attendu : `pvc_name`, `cloned`, condition `Ready` reflétant `cloned`
/// (`True`/`"Init job succeeded"` une fois cloné, `False`/`"Waiting for init job"`
/// sinon). `last_fetch` et `worktrees` restent vides en v1 (pas encore suivis —
/// `last_fetch` viendrait du status du `CronJob`, `worktrees` du reconciler Sandbox,
/// tous deux hors scope ici).
pub fn compute_status(project: &Project, cloned: bool) -> ProjectStatus {
    let (status, reason, message) = if cloned {
        (
            "True",
            "InitJobSucceeded",
            "PVC workspace et clone bare en place",
        )
    } else {
        (
            "False",
            "WaitingForInitJob",
            "En attente de la fin du job project-init",
        )
    };
    ProjectStatus {
        pvc_name: Some(effective_pvc_name(project)),
        cloned,
        last_fetch: None,
        worktrees: Vec::new(),
        conditions: vec![Condition {
            type_: "Ready".to_string(),
            status: status.to_string(),
            reason: reason.to_string(),
            message: message.to_string(),
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
            observed_generation: project.meta().generation,
        }],
        languages: Vec::new(),
        detected_at: None,
    }
}

/// Récupère l'Owner référencé par le Project (même namespace).
async fn fetch_owner(project: &Project, ctx: &Context, ns: &str) -> Result<Owner, ControllerError> {
    let owners: Api<Owner> = Api::namespaced(ctx.client.clone(), ns);
    Ok(owners.get(&project.spec.owner).await?)
}

/// `Event::Apply` : PVC workspace (si à créer) + Job init (une fois) + `CronJob`
/// fetch (une fois cloné) + status.
async fn apply(project: &Project, ctx: &Context, ns: &str) -> Result<Action, ControllerError> {
    let owner = fetch_owner(project, ctx, ns).await?;
    let owner_pvc_name = owner
        .status
        .as_ref()
        .and_then(|s| s.pvc_name.clone())
        .ok_or_else(|| ControllerError::OwnerNotReady {
            owner: project.spec.owner.clone(),
            project: project.name_any(),
        })?;

    let pp = PatchParams::apply(owner::FIELD_MANAGER).force();

    // RBAC des Jobs de maintenance : SA + Role (projects/status: patch) + RoleBinding
    let service_accounts: Api<ServiceAccount> = Api::namespaced(ctx.client.clone(), ns);
    let service_account = build_maint_service_account(project);
    service_accounts
        .patch(&maint_sa_name(&project.name_any()), &pp, &Patch::Apply(&service_account))
        .await?;

    let roles: Api<Role> = Api::namespaced(ctx.client.clone(), ns);
    let role = build_maint_role(project);
    roles
        .patch(&maint_sa_name(&project.name_any()), &pp, &Patch::Apply(&role))
        .await?;

    let role_bindings: Api<RoleBinding> = Api::namespaced(ctx.client.clone(), ns);
    let role_binding = build_maint_role_binding(project);
    role_bindings
        .patch(&maint_sa_name(&project.name_any()), &pp, &Patch::Apply(&role_binding))
        .await?;

    if let Some(pvc) = build_workspace_pvc(
        project,
        owner
            .spec
            .project_defaults
            .as_ref()
            .and_then(|d| d.storage_size.as_deref()),
        owner
            .spec
            .project_defaults
            .as_ref()
            .and_then(|d| d.storage_class.as_deref()),
        owner
            .spec
            .project_defaults
            .as_ref()
            .and_then(|d| d.storage_access_mode.as_deref()),
    ) {
        let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(ctx.client.clone(), ns);
        pvcs.patch(
            &project_pvc_name(&project.name_any()),
            &pp,
            &Patch::Apply(&pvc),
        )
        .await?;
    }

    let job_ctx = ProjectJobContext {
        sandbox_image: ctx.sandbox_image.clone(),
        owner_pvc_name,
    };

    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let cloned = if let Some(job) = jobs.get_opt(&init_job_name(&project.name_any())).await? {
        job.status.and_then(|s| s.succeeded).unwrap_or(0) > 0
    } else {
        let job = build_init_job(project, &job_ctx);
        jobs.create(&PostParams::default(), &job).await?;
        false
    };

    if cloned {
        let cronjobs: Api<CronJob> = Api::namespaced(ctx.client.clone(), ns);
        let mut cronjob = build_fetch_cronjob(project, &job_ctx);
        cronjob.spec.concurrency_policy = Some("Forbid".to_string());
        cronjobs
            .patch(
                &fetch_cronjob_name(&project.name_any()),
                &pp,
                &Patch::Apply(&cronjob),
            )
            .await?;
    }

    let projects: Api<Project> = Api::namespaced(ctx.client.clone(), ns);
    let status = compute_status(project, cloned);
    let patch = serde_json::json!({ "status": status });
    projects
        .patch_status(
            &project.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    // Bump inconditionnel : force le reconcile immediat des Sandboxes de ce
    // Project (watch natif kube-runtime sur leur propre objet), sans watch
    // inter-CRD permanent. Cf. design ws13-sandbox-runtime section 2.
    let sandboxes: Api<Sandbox> = Api::namespaced(ctx.client.clone(), ns);
    let bump = serde_json::json!({
        "metadata": {
            "annotations": {
                "vanyline.solidite.fr/egress-bump": k8s_openapi::jiff::Timestamp::now().to_string()
            }
        }
    });
    for sandbox in sandboxes.list(&ListParams::default()).await? {
        if sandbox.spec.project == project.name_any() {
            sandboxes
                .patch(
                    &sandbox.name_any(),
                    &PatchParams::default(),
                    &Patch::Merge(&bump),
                )
                .await?;
        }
    }

    Ok(Action::requeue(Duration::from_secs(if cloned {
        300
    } else {
        15
    })))
}

/// `Event::Cleanup` : Job de purge, en attente de succès (voir note sur la
/// sémantique du finalizer : un `Ok` retire le finalizer, donc on renvoie
/// `Err(PurgePending)` tant que ce n'est pas terminé).
async fn cleanup(project: &Project, ctx: &Context, ns: &str) -> Result<Action, ControllerError> {
    let owner_pvc_name = match fetch_owner(project, ctx, ns).await {
        Ok(owner) => owner.status.and_then(|s| s.pvc_name),
        Err(_) => None,
    };
    let Some(owner_pvc_name) = owner_pvc_name else {
        tracing::warn!(
            project = %project.name_any(),
            "owner introuvable pendant le cleanup — purge ignorée (best-effort v1)"
        );
        return Ok(Action::await_change());
    };

    let job_ctx = ProjectJobContext {
        sandbox_image: ctx.sandbox_image.clone(),
        owner_pvc_name,
    };
    let jobs: Api<Job> = Api::namespaced(ctx.client.clone(), ns);
    let name = purge_job_name(&project.name_any());
    match jobs.get_opt(&name).await? {
        Some(job) if job.status.as_ref().and_then(|s| s.succeeded).unwrap_or(0) > 0 => {
            Ok(Action::await_change())
        }
        Some(_) => Err(ControllerError::PurgePending {
            project: project.name_any(),
        }),
        None => {
            let job = build_purge_job(project, &job_ctx);
            jobs.create(&PostParams::default(), &job).await?;
            Err(ControllerError::PurgePending {
                project: project.name_any(),
            })
        }
    }
}

pub async fn reconcile(
    project: Arc<Project>,
    ctx: Arc<Context>,
) -> Result<Action, ControllerError> {
    let ns = project.namespace().unwrap_or_else(|| "default".to_string());
    let api: Api<Project> = Api::namespaced(ctx.client.clone(), &ns);
    finalizer(&api, FINALIZER, project, |event| async {
        match event {
            Event::Apply(p) => apply(&p, &ctx, &ns).await,
            Event::Cleanup(p) => cleanup(&p, &ctx, &ns).await,
        }
    })
    .await
    .map_err(|e| ControllerError::Finalizer(e.to_string()))
}

pub fn error_policy(_project: Arc<Project>, error: &ControllerError, _ctx: Arc<Context>) -> Action {
    if let ControllerError::PurgePending { .. } = error {
        tracing::info!(%error, "purge job pending, requeue in 10s");
        Action::requeue(Duration::from_secs(10))
    } else {
        tracing::warn!(%error, "project reconcile error, requeue in 30s");
        Action::requeue(Duration::from_secs(30))
    }
}

pub fn build_controller(client: Client) -> Controller<Project> {
    let projects: Api<Project> = Api::all(client);
    Controller::new(projects, kube::runtime::watcher::Config::default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use vanyline_crds::{ProjectSpec, PvcRef};

    fn make_project(existing_pvc: Option<PvcRef>, storage_size: Option<String>) -> Project {
        let mut project = Project::new(
            "demo",
            ProjectSpec {
                owner: "alice".to_string(),
                repo_url: "https://github.com/owner/repo".to_string(),
                default_branch: None,
                existing_pvc,
                storage_size,
                storage_class: None,
                storage_access_mode: None,
                git_secret: None,
                caches: None,
                fetch_interval: None,
                egress: Vec::new(),
            },
        );
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
        assert_eq!(
            effective_caches(&project),
            vec!["cargo".to_string(), "pnpm".to_string()]
        );
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
            Some(PvcRef {
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
            Some(PvcRef {
                name: "x".into(),
                sub_path: Some("repo".into()),
            }),
            None,
        );
        assert_eq!(effective_sub_path(&project), Some("repo".to_string()));

        // existing_pvc without sub_path
        let project = make_project(
            Some(PvcRef {
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
            Some(PvcRef {
                name: "code-server-home".into(),
                sub_path: None,
            }),
            None,
        );
        assert!(build_workspace_pvc(&project, None, None, None).is_none());
    }

    // 11. build_workspace_pvc_default_size
    #[test]
    fn build_workspace_pvc_default_size() {
        let project = make_project(None, None);
        let pvc = build_workspace_pvc(&project, None, None, None).expect("PVC should be built");
        let requests = pvc
            .spec
            .as_ref()
            .unwrap()
            .resources
            .as_ref()
            .unwrap()
            .requests
            .as_ref()
            .unwrap();
        assert_eq!(requests.get("storage").unwrap(), &Quantity("10Gi".into()));
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &vec!["ReadWriteOnce".to_string()]
        );
    }

    // 12. build_workspace_pvc_owner_default_size
    #[test]
    fn build_workspace_pvc_owner_default_size() {
        let project = make_project(None, None);
        let pvc =
            build_workspace_pvc(&project, Some("20Gi"), None, None).expect("PVC should be built");
        let requests = pvc
            .spec
            .as_ref()
            .unwrap()
            .resources
            .as_ref()
            .unwrap()
            .requests
            .as_ref()
            .unwrap();
        assert_eq!(requests.get("storage").unwrap(), &Quantity("20Gi".into()));
    }

    #[test]
    fn build_workspace_pvc_access_mode_owner_default() {
        let project = make_project(None, None);
        let pvc = build_workspace_pvc(&project, None, None, Some("ReadWriteMany"))
            .expect("PVC should be built");
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &vec!["ReadWriteMany".to_string()]
        );
    }

    #[test]
    fn build_workspace_pvc_access_mode_spec_overrides_default() {
        let mut project = make_project(None, None);
        project.spec.storage_access_mode = Some("ReadWriteOncePod".to_string());
        let pvc = build_workspace_pvc(&project, None, None, Some("ReadWriteMany"))
            .expect("PVC should be built");
        assert_eq!(
            pvc.spec.as_ref().unwrap().access_modes.as_ref().unwrap(),
            &vec!["ReadWriteOncePod".to_string()]
        );
    }

    // 13. build_workspace_pvc_spec_overrides_default
    #[test]
    fn build_workspace_pvc_spec_overrides_default() {
        let project = make_project(None, Some("5Gi".to_string()));
        let pvc =
            build_workspace_pvc(&project, Some("20Gi"), None, None).expect("PVC should be built");
        let requests = pvc
            .spec
            .as_ref()
            .unwrap()
            .resources
            .as_ref()
            .unwrap()
            .requests
            .as_ref()
            .unwrap();
        assert_eq!(requests.get("storage").unwrap(), &Quantity("5Gi".into()));
    }

    // 14. build_workspace_pvc_owner_reference
    #[test]
    fn build_workspace_pvc_owner_reference() {
        let project = make_project(None, None);
        let pvc = build_workspace_pvc(&project, None, None, None).expect("PVC should be built");
        let refs = pvc
            .metadata
            .owner_references
            .as_ref()
            .expect("owner_references should be present");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name.as_str(), "demo");
        assert_eq!(refs[0].kind.as_str(), "Project");
    }

    // ===== Tests git jobs (tâche 04) =====

    fn make_ctx() -> ProjectJobContext {
        ProjectJobContext {
            sandbox_image: "registry.example/vanyline-sandbox:latest".to_string(),
            owner_pvc_name: "owner-alice-home".to_string(),
        }
    }

    // 15. job_names
    #[test]
    fn job_names() {
        assert_eq!(init_job_name("demo"), "project-demo-init");
        assert_eq!(fetch_cronjob_name("demo"), "project-demo-fetch");
        assert_eq!(purge_job_name("demo"), "project-demo-purge");
    }

    // 16. fetch_schedule_default
    #[test]
    fn fetch_schedule_default() {
        let project = make_project(None, None);
        assert_eq!(fetch_schedule(&project), "@every 1h");
    }

    // 17. fetch_schedule_custom
    #[test]
    fn fetch_schedule_custom() {
        let mut project = make_project(None, None);
        project.spec.fetch_interval = Some("30m".to_string());
        assert_eq!(fetch_schedule(&project), "@every 30m");
    }

    #[test]
    fn git_pod_template_runs_as_non_root() {
        let project = make_project(None, None);
        let ctx = make_ctx();
        let template = git_pod_template(
            &project,
            &ctx,
            None,                           // no init container (purge path)
            vec!["vanyline-maint".to_string()],
            None,                           // no service account
        );
        let sc = template
            .spec
            .as_ref()
            .unwrap()
            .security_context
            .as_ref()
            .unwrap();
        assert_eq!(sc.run_as_user, Some(SANDBOX_UID));
        assert_eq!(sc.run_as_group, Some(SANDBOX_UID));
        assert_eq!(sc.run_as_non_root, Some(true));
        assert_eq!(sc.fs_group, Some(SANDBOX_UID));
    }

    // 18. build_init_job_shape
    #[test]
    fn build_init_job_shape() {
        let project = make_project(None, None);
        let ctx = make_ctx();
        let job = build_init_job(&project, &ctx);

        assert_eq!(job.metadata.name, Some("project-demo-init".to_string()));

        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();

        // initContainer[0] = "git", command = init argv
        let init_cmd = pod_spec
            .init_containers
            .as_ref()
            .unwrap()
            .first()
            .unwrap();
        assert_eq!(init_cmd.name, "git");
        assert_eq!(
            init_cmd.command.as_ref().unwrap(),
            &vec![
                "vanyline-maint".to_string(),
                "init".to_string(),
                "--repo".to_string(),
                "https://github.com/owner/repo".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
                "--cache".to_string(),
                "cargo".to_string(),
                "--cache".to_string(),
                "pnpm".to_string(),
            ]
        );
        assert_eq!(init_cmd.image, Some(ctx.sandbox_image.clone()));

        // containers[0] = "detect", command = detect argv
        let detect_cmd = pod_spec.containers[0].command.as_ref().unwrap();
        assert_eq!(
            pod_spec.containers[0].name,
            "detect".to_string()
        );
        assert_eq!(
            detect_cmd,
            &vec![
                "vanyline-maint".to_string(),
                "detect".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
                "--project".to_string(),
                "demo".to_string(),
            ]
        );

        // service_account_name
        assert_eq!(
            pod_spec.service_account_name,
            Some("project-demo-maint".to_string())
        );

        let volumes = pod_spec.volumes.as_ref().unwrap();
        assert_eq!(volumes.len(), 2);
        let names: Vec<_> = volumes.iter().map(|v| &v.name).collect();
        assert!(names.contains(&&"workspace".to_string()));
        assert!(names.contains(&&"home".to_string()));

        let refs = job.metadata.owner_references.as_ref().unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo");
        assert_eq!(refs[0].kind, "Project");
    }

    // 19. build_init_job_with_git_secret
    #[test]
    fn build_init_job_with_git_secret() {
        let mut project = make_project(None, None);
        project.spec.git_secret = Some("demo-deploy-key".to_string());
        let ctx = make_ctx();

        let job = build_init_job(&project, &ctx);

        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
        let volumes = pod_spec.volumes.as_ref().unwrap();
        assert_eq!(volumes.len(), 3);

        let secret_vol = volumes
            .iter()
            .find(|v| v.name == "git-secret")
            .expect("should have git-secret volume");
        assert_eq!(
            secret_vol.secret.as_ref().unwrap().secret_name,
            Some("demo-deploy-key".to_string())
        );

        let env_vars = job
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        let ssh_env = env_vars
            .iter()
            .find(|e| e.name == "GIT_SSH_COMMAND")
            .expect("should have GIT_SSH_COMMAND");
        assert!(
            ssh_env
                .value
                .as_ref()
                .unwrap()
                .contains("/git-secret/ssh-privatekey")
        );
    }

    // 20. build_init_job_existing_pvc_sub_path
    #[test]
    fn build_init_job_existing_pvc_sub_path() {
        let project = make_project(
            Some(PvcRef {
                name: "code-server-home".into(),
                sub_path: Some("demo".into()),
            }),
            None,
        );
        let ctx = make_ctx();

        let job = build_init_job(&project, &ctx);
        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();

        // Volume "workspace" should reference the existing PVC
        let workspace_vol = pod_spec
            .volumes
            .as_ref()
            .unwrap()
            .iter()
            .find(|v| v.name == "workspace")
            .unwrap();
        assert_eq!(
            workspace_vol
                .persistent_volume_claim
                .as_ref()
                .unwrap()
                .claim_name,
            "code-server-home"
        );

        // VolumeMount "workspace" should have sub_path
        let mounts = pod_spec.containers[0].volume_mounts.as_ref().unwrap();
        let ws_mount = mounts.iter().find(|m| m.name == "workspace").unwrap();
        assert_eq!(ws_mount.sub_path, Some("demo".to_string()));
    }

    // 21. build_fetch_cronjob_shape
    #[test]
    fn build_fetch_cronjob_shape() {
        let project = make_project(None, None);
        let ctx = make_ctx();

        let cronjob = build_fetch_cronjob(&project, &ctx);

        assert_eq!(
            cronjob.metadata.name,
            Some("project-demo-fetch".to_string())
        );
        assert_eq!(cronjob.spec.schedule, "@every 1h");

        let job_spec = cronjob.spec.job_template.spec.as_ref().unwrap();
        let pod_spec = job_spec
            .template
            .spec
            .as_ref()
            .unwrap();

        // init_containers[0] = "git" runs fetch argv
        let init_cmd = pod_spec
            .init_containers
            .as_ref()
            .unwrap()
            .first()
            .unwrap();
        assert_eq!(init_cmd.name, "git");
        assert_eq!(
            init_cmd.command.as_ref().unwrap(),
            &vec![
                "vanyline-maint".to_string(),
                "fetch".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
            ]
        );

        // containers[0] = "detect" runs detect argv with --project demo
        let detect_cmd = pod_spec.containers[0].command.as_ref().unwrap();
        assert_eq!(
            pod_spec.containers[0].name,
            "detect".to_string()
        );
        assert_eq!(
            detect_cmd,
            &vec![
                "vanyline-maint".to_string(),
                "detect".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
                "--project".to_string(),
                "demo".to_string(),
            ]
        );

        // service_account_name
        assert_eq!(
            pod_spec.service_account_name,
            Some("project-demo-maint".to_string())
        );
    }

    // 22. build_purge_job_shape
    #[test]
    fn build_purge_job_shape() {
        let project = make_project(None, None);
        let ctx = make_ctx();

        let job = build_purge_job(&project, &ctx);

        let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
        let container = pod_spec.containers.first().unwrap();
        let command = container.command.as_ref().unwrap();
        assert_eq!(
            command,
            &vec![
                "vanyline-maint".to_string(),
                "purge".to_string(),
                "--workspace".to_string(),
                "/workspace".to_string(),
            ]
        );
    }

    // 23. git_jobs_service_account
    #[test]
    fn git_jobs_service_account() {
        let project = make_project(None, None);
        let ctx = make_ctx();

        let init_job = build_init_job(&project, &ctx);
        assert_eq!(
            init_job
                .spec
                .as_ref()
                .unwrap()
                .template
                .spec
                .as_ref()
                .unwrap()
                .service_account_name,
            Some("project-demo-maint".to_string())
        );

        let fetch_cronjob = build_fetch_cronjob(&project, &ctx);
        assert_eq!(
            fetch_cronjob
                .spec
                .job_template
                .spec
                .as_ref()
                .unwrap()
                .template
                .spec
                .as_ref()
                .unwrap()
                .service_account_name,
            Some("project-demo-maint".to_string())
        );

        let purge_job = build_purge_job(&project, &ctx);
        assert!(
            purge_job
                .spec
                .as_ref()
                .unwrap()
                .template
                .spec
                .as_ref()
                .unwrap()
                .service_account_name
                .is_none()
        );
    }

    // 24. git_pod_template_no_shell
    #[test]
    fn git_pod_template_no_shell() {
        let project = make_project(None, None);
        let ctx = make_ctx();

        let init_job = build_init_job(&project, &ctx);
        let init_p = init_job
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap();
        // initContainer[0] (git)
        let init_cmd = init_p
            .init_containers
            .as_ref()
            .unwrap()
            .first()
            .unwrap()
            .command
            .as_ref()
            .unwrap();
        assert_eq!(init_cmd[0], "vanyline-maint");
        assert!(!init_cmd.iter().any(|a| a == "sh"));
        assert!(!init_cmd.iter().any(|a| a == "-c"));
        // containers[0] (detect)
        let detect_cmd = init_p.containers[0].command.as_ref().unwrap();
        assert_eq!(detect_cmd[0], "vanyline-maint");
        assert!(!detect_cmd.iter().any(|a| a == "sh"));
        assert!(!detect_cmd.iter().any(|a| a == "-c"));

        let fetch_cronjob = build_fetch_cronjob(&project, &ctx);
        let fetch_p = fetch_cronjob
            .spec
            .job_template
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap();
        // initContainer[0] (git)
        let fetch_init_cmd = fetch_p
            .init_containers
            .as_ref()
            .unwrap()
            .first()
            .unwrap()
            .command
            .as_ref()
            .unwrap();
        assert_eq!(fetch_init_cmd[0], "vanyline-maint");
        assert!(!fetch_init_cmd.iter().any(|a| a == "sh"));
        assert!(!fetch_init_cmd.iter().any(|a| a == "-c"));
        // containers[0] (detect)
        let detect_cmd2 = fetch_p.containers[0].command.as_ref().unwrap();
        assert_eq!(detect_cmd2[0], "vanyline-maint");
        assert!(!detect_cmd2.iter().any(|a| a == "sh"));
        assert!(!detect_cmd2.iter().any(|a| a == "-c"));

        let purge_job = build_purge_job(&project, &ctx);
        let purge_p = purge_job
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap();
        // single container (git)
        let purge_cmd = purge_p.containers[0].command.as_ref().unwrap();
        assert_eq!(purge_cmd[0], "vanyline-maint");
        assert!(!purge_cmd.iter().any(|a| a == "sh"));
        assert!(!purge_cmd.iter().any(|a| a == "-c"));
    }

    // 25. compute_status_cloned_true
    #[test]
    fn compute_status_cloned_true() {
        let project = make_project(None, None);
        let status = compute_status(&project, true);
        assert!(status.cloned);
        assert_eq!(status.pvc_name.as_ref().unwrap(), "project-demo");
        assert_eq!(status.conditions.len(), 1);
        let cond = &status.conditions[0];
        assert_eq!(cond.type_, "Ready");
        assert_eq!(cond.status, "True");
        assert_eq!(cond.reason, "InitJobSucceeded");
    }

    // 26. compute_status_cloned_false
    #[test]
    fn compute_status_cloned_false() {
        let project = make_project(None, None);
        let status = compute_status(&project, false);
        assert!(!status.cloned);
        assert_eq!(status.pvc_name.as_ref().unwrap(), "project-demo");
        assert_eq!(status.conditions.len(), 1);
        let cond = &status.conditions[0];
        assert_eq!(cond.type_, "Ready");
        assert_eq!(cond.status, "False");
        assert_eq!(cond.reason, "WaitingForInitJob");
    }

    // 27. compute_status_worktrees_and_last_fetch_empty
    #[test]
    fn compute_status_worktrees_and_last_fetch_empty() {
        let project = make_project(None, None);
        let status_true = compute_status(&project, true);
        assert!(status_true.worktrees.is_empty());
        assert!(status_true.last_fetch.is_none());

        let status_false = compute_status(&project, false);
        assert!(status_false.worktrees.is_empty());
        assert!(status_false.last_fetch.is_none());
    }

    // ===== New tests for RBAC + detect chaining =====

    // 28. maint_sa_name_value
    #[test]
    fn maint_sa_name_value() {
        assert_eq!(maint_sa_name("demo"), "project-demo-maint");
    }

    // 29. build_maint_service_account_shape
    #[test]
    fn build_maint_service_account_shape() {
        let project = make_project(None, None);
        let sa = build_maint_service_account(&project);

        assert_eq!(sa.metadata.name, Some("project-demo-maint".to_string()));
        assert_eq!(sa.metadata.namespace, Some("ns".to_string()));
        let refs = sa
            .metadata
            .owner_references
            .as_ref()
            .expect("should have ownerReferences");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "demo");
        assert_eq!(refs[0].kind, "Project");
    }

    // 30. build_maint_role_shape
    #[test]
    fn build_maint_role_shape() {
        let project = make_project(None, None);
        let role = build_maint_role(&project);

        assert_eq!(role.metadata.name, Some("project-demo-maint".to_string()));
        let rules = role.rules.as_ref().expect("should have rules");
        assert_eq!(rules.len(), 1);
        let rule = &rules[0];
        assert_eq!(
            rule.api_groups.as_ref().unwrap().as_slice(),
            &["vanyline.solidite.fr".to_string()]
        );
        assert_eq!(
            rule.resources.as_ref().unwrap().as_slice(),
            &["projects/status".to_string()]
        );
        assert_eq!(
            rule.verbs.as_slice(),
            &["patch".to_string()]
        );
    }

    // 31. build_maint_role_binding_shape
    #[test]
    fn build_maint_role_binding_shape() {
        let project = make_project(None, None);
        let rb = build_maint_role_binding(&project);

        assert_eq!(rb.metadata.name, Some("project-demo-maint".to_string()));
        assert_eq!(rb.role_ref.kind, "Role");
        assert_eq!(rb.role_ref.name, "project-demo-maint");
        assert_eq!(
            rb.role_ref.api_group,
            Some("rbac.authorization.k8s.io".to_string())
        );

        let subjects = rb.subjects.as_ref().expect("should have subjects");
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0].kind, "ServiceAccount");
        assert_eq!(subjects[0].name, "project-demo-maint");
        assert_eq!(subjects[0].namespace, Some("ns".to_string()));
    }

    // 32. init_job_detect_container_order
    #[test]
    fn init_job_detect_container_order() {
        let project = make_project(None, None);
        let ctx = make_ctx();
        let job = build_init_job(&project, &ctx);
        let p = job
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap();

        // initContainer[0] runs "init"
        let init_cmd = p
            .init_containers
            .as_ref()
            .unwrap()
            .first()
            .unwrap()
            .command
            .as_ref()
            .unwrap();
        assert_eq!(init_cmd[1], "init");

        // containers[0] runs "detect" with --project demo
        let detect_cmd = p.containers[0].command.as_ref().unwrap();
        assert_eq!(detect_cmd[1], "detect");
        // argv order: --project -> demo
        let project_idx = detect_cmd
            .iter()
            .position(|a| a == "--project")
            .expect("should have --project");
        assert_eq!(detect_cmd[project_idx + 1], "demo");
    }
}
