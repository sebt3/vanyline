use thiserror::Error;

#[derive(Debug, Error)]
pub enum ControllerError {
    #[error("VNL-CTL-001: Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error(
        "VNL-CTL-002: Owner '{owner}' not found or not Ready (no pvc_name in status) for Project '{project}'"
    )]
    OwnerNotReady { owner: String, project: String },

    #[error("VNL-CTL-003: purge job for Project '{project}' not finished yet")]
    PurgePending { project: String },

    #[error("VNL-CTL-004: finalizer error: {0}")]
    Finalizer(String),

    #[error("VNL-CTL-005: Project '{project}' not cloned yet for Sandbox '{sandbox}'")]
    ProjectNotReady { project: String, sandbox: String },

    #[error("VNL-CTL-006: worktree removal job for Sandbox '{sandbox}' not finished yet")]
    WorktreeRemovalPending { sandbox: String },

    /// Le nom `"{app_namespace}-{owner}"` n'est pas un label DNS-1123 valide ou
    /// dépasse 63 caractères (namespace.sdd, VNL-CTL-007).
    #[error("VNL-CTL-007: invalid Owner namespace name '{name}': {reason}")]
    InvalidNamespaceName { name: String, reason: String },

    /// Un namespace de ce nom existe déjà mais ne porte pas les labels de cet
    /// Owner : il ne lui appartient pas, adoption/modification/suppression
    /// refusées (namespace.sdd, VNL-CTL-008).
    #[error(
        "VNL-CTL-008: namespace '{namespace}' already exists without the labels of Owner '{owner}' — refusing to adopt it"
    )]
    NamespaceCollision { namespace: String, owner: String },

    /// Le namespace est en suppression : `ensure_namespace` ne peut pas le
    /// (re)créer, l'Owner est requeué (namespace.sdd, VNL-CTL-009).
    #[error("VNL-CTL-009: namespace '{namespace}' is terminating, cannot be created or reused")]
    NamespaceTerminating { namespace: String },

    /// Namespace dépourvu des labels de ce controller (une CR Project/Sandbox
    /// posée hors d'un namespace d'Owner) — `app_namespace_of` refuse de le
    /// résoudre (namespace.sdd, VNL-CTL-010).
    #[error(
        "VNL-CTL-010: namespace '{namespace}' is not a vanyline Owner namespace (missing label vanyline.solidite.fr/app-namespace)"
    )]
    NamespaceNotManaged { namespace: String },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::application;
    use crate::owner;
    use crate::project;
    use crate::sandbox;
    use kube::Client;
    use kube::runtime::controller::Action;
    use std::sync::Arc;
    use std::time::Duration;

    /// Instance de chaque variante avec des payloads fixés — balayage des
    /// Scenario `display_exhaustif_des_marqueurs_vnl_ctl`,
    /// `source_absente_sur_les_cinq_autres_variantes`,
    /// `error_policy_owner_et_application_toujours_trente_secondes` et
    /// `aucune_erreur_ne_stoppe_le_reconcile`. Les dix variantes (001..010)
    /// depuis l'étape 4b (namespace.sdd).
    fn all_variants() -> Vec<ControllerError> {
        vec![
            ControllerError::Kube(kube::Error::LinesCodecMaxLineLengthExceeded),
            ControllerError::OwnerNotReady {
                owner: "alice".to_string(),
                project: "depot".to_string(),
            },
            ControllerError::PurgePending {
                project: "depot".to_string(),
            },
            ControllerError::Finalizer("cause".to_string()),
            ControllerError::ProjectNotReady {
                project: "depot".to_string(),
                sandbox: "sb1".to_string(),
            },
            ControllerError::WorktreeRemovalPending {
                sandbox: "sb1".to_string(),
            },
            ControllerError::InvalidNamespaceName {
                name: "media-test-seb".to_string(),
                reason: "raison".to_string(),
            },
            ControllerError::NamespaceCollision {
                namespace: "media-test-seb".to_string(),
                owner: "seb".to_string(),
            },
            ControllerError::NamespaceTerminating {
                namespace: "media-test-seb".to_string(),
            },
            ControllerError::NamespaceNotManaged {
                namespace: "default".to_string(),
            },
        ]
    }

    /// Client hors connexion (URI morte, aucune requête jamais émise — patron
    /// owner.sdd `error_policy_requeue_trente_secondes`) pour les appels directs
    /// des quatre `error_policy`.
    fn offline_client() -> Client {
        // rustls peut voir les features aws_lc_rs ET ring activées selon les
        // dépendances (axum-server/reqwest activent aws_lc_rs, kube active
        // ring) : l'auto-détection du provider échoue alors. Installe
        // explicitement le provider ring avant de construire le client TLS —
        // idempotent (un Err ici signifie qu'un provider est déjà installé,
        // acceptable). Motif de sandbox/src/maint.rs.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = kube::Config::new(
            "http://127.0.0.1:9"
                .parse()
                .expect("URI morte syntaxiquement valide"),
        );
        Client::try_from(config).expect("construction hors connexion d'un Client inerte")
    }

    fn dummy_owner() -> vanyline_crds::Owner {
        vanyline_crds::Owner::new(
            "alice",
            vanyline_crds::OwnerSpec {
                home_size: None,
                home_storage_class: None,
                home_access_mode: None,
                project_defaults: None,
                application_ref: None,
                egress: Vec::new(),
            },
        )
    }

    fn dummy_project() -> vanyline_crds::Project {
        vanyline_crds::Project::new(
            "depot",
            vanyline_crds::ProjectSpec {
                owner: "alice".to_string(),
                repo_url: "https://example.invalid/repo".to_string(),
                default_branch: None,
                storage_size: None,
                storage_class: None,
                storage_access_mode: None,
                git_secret: None,
                caches: None,
                fetch_interval: None,
                egress: Vec::new(),
            },
        )
    }

    fn dummy_sandbox() -> vanyline_crds::Sandbox {
        vanyline_crds::Sandbox::new(
            "sb1",
            vanyline_crds::SandboxSpec {
                project: "depot".to_string(),
                branch: "main".to_string(),
                toolchains: Vec::new(),
                image: None,
                resources: None,
                egress: Vec::new(),
                suspended: false,
            },
        )
    }

    fn dummy_application() -> vanyline_crds::Application {
        vanyline_crds::Application::new(
            "app-test",
            vanyline_crds::ApplicationSpec {
                image: None,
                replicas: None,
                oidc_secret_ref: "oidc".to_string(),
                database_secret_ref: "db".to_string(),
                cookie_secret_ref: None,
                host: "example.invalid".to_string(),
                ingress_class_name: "traefik".to_string(),
                tls_issuer_name: "letsencrypt".to_string(),
                tls_issuer_kind: None,
                ingress_annotations: std::collections::BTreeMap::new(),
                ingress_controller: None,
                storage_defaults: None,
            },
        )
    }

    // Scenario: display_exhaustif_des_marqueurs_vnl_ctl
    #[test]
    fn display_exhaustif_des_marqueurs_vnl_ctl() {
        let exacts = [
            (
                "001",
                "VNL-CTL-001: Kubernetes API error: Error finding newline character",
            ),
            (
                "002",
                "VNL-CTL-002: Owner 'alice' not found or not Ready (no pvc_name in status) for Project 'depot'",
            ),
            (
                "003",
                "VNL-CTL-003: purge job for Project 'depot' not finished yet",
            ),
            ("004", "VNL-CTL-004: finalizer error: cause"),
            (
                "005",
                "VNL-CTL-005: Project 'depot' not cloned yet for Sandbox 'sb1'",
            ),
            (
                "006",
                "VNL-CTL-006: worktree removal job for Sandbox 'sb1' not finished yet",
            ),
            (
                "007",
                "VNL-CTL-007: invalid Owner namespace name 'media-test-seb': raison",
            ),
            (
                "008",
                "VNL-CTL-008: namespace 'media-test-seb' already exists without the labels of Owner 'seb' — refusing to adopt it",
            ),
            (
                "009",
                "VNL-CTL-009: namespace 'media-test-seb' is terminating, cannot be created or reused",
            ),
            (
                "010",
                "VNL-CTL-010: namespace 'default' is not a vanyline Owner namespace (missing label vanyline.solidite.fr/app-namespace)",
            ),
        ];
        let variants = all_variants();
        assert_eq!(variants.len(), exacts.len(), "dix variantes attendues");
        let mut markers = Vec::new();
        for ((marker, exact), error) in exacts.iter().zip(&variants) {
            let rendered = error.to_string();
            assert_eq!(&rendered, exact, "rendu exact de la variante {marker}");
            assert!(
                rendered.starts_with(&format!("VNL-CTL-{marker}: ")),
                "variante {marker}: le Display commence par son marqueur suivi de ': '"
            );
            markers.push(format!("VNL-CTL-{marker}"));
        }
        // Marqueurs deux à deux distincts.
        let mut unique = markers.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            markers.len(),
            "les dix marqueurs sont distincts"
        );
    }

    // Scenario: conversion_from_kube_error_et_source_chainee
    #[test]
    fn conversion_from_kube_error_et_source_chainee() {
        fn propagate() -> Result<(), ControllerError> {
            let source = kube::Error::LinesCodecMaxLineLengthExceeded;
            Err(source)?; // conversion par le From généré du #[from]
            Ok(())
        }
        let err = propagate().expect_err("le ? propage le kube::Error");
        assert!(
            matches!(err, ControllerError::Kube(_)),
            "variante attendue Kube, obtenu: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "VNL-CTL-001: Kubernetes API error: Error finding newline character"
        );
        let source = std::error::Error::source(&err).expect("source chaînée pour Kube");
        assert_eq!(
            source.to_string(),
            kube::Error::LinesCodecMaxLineLengthExceeded.to_string()
        );
    }

    // Scenario: source_absente_sur_les_cinq_autres_variantes
    // (neuf variantes hors Kube depuis l'ajout de VNL-CTL-007..010)
    #[test]
    fn source_absente_sur_les_cinq_autres_variantes() {
        for error in all_variants() {
            if matches!(error, ControllerError::Kube(_)) {
                continue;
            }
            assert!(
                std::error::Error::source(&error).is_none(),
                "aucun chaînage de cause hors #[from] de Kube : {error:?}"
            );
        }
    }

    // Scenario: error_policy_project_purge_pending_requeue_dix_secondes
    #[tokio::test]
    async fn error_policy_project_purge_pending_requeue_dix_secondes() {
        let ctx = Arc::new(project::Context {
            client: offline_client(),
            sandbox_image: "img:tag".to_string(),
        });
        let project = Arc::new(dummy_project());
        let pending = project::error_policy(
            Arc::clone(&project),
            &ControllerError::PurgePending {
                project: "depot".to_string(),
            },
            Arc::clone(&ctx),
        );
        assert_eq!(pending, Action::requeue(Duration::from_secs(10)));
        let other =
            project::error_policy(project, &ControllerError::Finalizer("x".to_string()), ctx);
        assert_eq!(other, Action::requeue(Duration::from_secs(30)));
    }

    // Scenario: error_policy_sandbox_discipline_les_variantes_de_controle
    #[tokio::test]
    async fn error_policy_sandbox_discipline_les_variantes_de_controle() {
        let ctx = Arc::new(sandbox::Context {
            client: offline_client(),
            default_image: "img:tag".to_string(),
            toolchain_image_rust: String::new(),
            toolchain_image_node: String::new(),
            toolchain_image_docker: String::new(),
            toolchain_image_python: String::new(),
            lsp_image_rust: String::new(),
            lsp_image_node: String::new(),
            lsp_image_docker: String::new(),
            lsp_image_python: String::new(),
        });
        let sandbox = Arc::new(dummy_sandbox());
        let removal = sandbox::error_policy(
            Arc::clone(&sandbox),
            &ControllerError::WorktreeRemovalPending {
                sandbox: "sb1".to_string(),
            },
            Arc::clone(&ctx),
        );
        assert_eq!(removal, Action::requeue(Duration::from_secs(10)));
        let not_cloned = sandbox::error_policy(
            Arc::clone(&sandbox),
            &ControllerError::ProjectNotReady {
                project: "depot".to_string(),
                sandbox: "sb1".to_string(),
            },
            Arc::clone(&ctx),
        );
        assert_eq!(not_cloned, Action::requeue(Duration::from_secs(15)));
        let other =
            sandbox::error_policy(sandbox, &ControllerError::Finalizer("x".to_string()), ctx);
        assert_eq!(other, Action::requeue(Duration::from_secs(30)));
    }

    // Scenario: error_policy_owner_et_application_toujours_trente_secondes
    #[tokio::test]
    async fn error_policy_owner_et_application_toujours_trente_secondes() {
        let owner_ctx = Arc::new(owner::Context {
            client: offline_client(),
        });
        let app_ctx = Arc::new(application::Context {
            client: offline_client(),
            default_image: "img:tag".to_string(),
        });
        let owner_cr = Arc::new(dummy_owner());
        let app_cr = Arc::new(dummy_application());
        for error in all_variants() {
            let action = owner::error_policy(Arc::clone(&owner_cr), &error, Arc::clone(&owner_ctx));
            assert_eq!(action, Action::requeue(Duration::from_secs(30)), "{error}");
            let action = application::error_policy(Arc::clone(&app_cr), &error, app_ctx.clone());
            assert_eq!(action, Action::requeue(Duration::from_secs(30)), "{error}");
        }
    }

    // Scenario: aucune_erreur_ne_stoppe_le_reconcile
    #[tokio::test]
    async fn aucune_erreur_ne_stoppe_le_reconcile() {
        let owner_ctx = Arc::new(owner::Context {
            client: offline_client(),
        });
        let project_ctx = Arc::new(project::Context {
            client: offline_client(),
            sandbox_image: "img:tag".to_string(),
        });
        let sandbox_ctx = Arc::new(sandbox::Context {
            client: offline_client(),
            default_image: "img:tag".to_string(),
            toolchain_image_rust: String::new(),
            toolchain_image_node: String::new(),
            toolchain_image_docker: String::new(),
            toolchain_image_python: String::new(),
            lsp_image_rust: String::new(),
            lsp_image_node: String::new(),
            lsp_image_docker: String::new(),
            lsp_image_python: String::new(),
        });
        let app_ctx = Arc::new(application::Context {
            client: offline_client(),
            default_image: "img:tag".to_string(),
        });
        let owner_cr = Arc::new(dummy_owner());
        let project_cr = Arc::new(dummy_project());
        let sandbox_cr = Arc::new(dummy_sandbox());
        let app_cr = Arc::new(dummy_application());
        // 4 politiques × dix variantes = 40 appels.
        for error in all_variants() {
            for action in [
                owner::error_policy(Arc::clone(&owner_cr), &error, Arc::clone(&owner_ctx)),
                project::error_policy(Arc::clone(&project_cr), &error, Arc::clone(&project_ctx)),
                sandbox::error_policy(Arc::clone(&sandbox_cr), &error, Arc::clone(&sandbox_ctx)),
                application::error_policy(Arc::clone(&app_cr), &error, Arc::clone(&app_ctx)),
            ] {
                assert_ne!(
                    action,
                    Action::await_change(),
                    "toute erreur est requeueée, jamais un arrêt du reconciler : {error}"
                );
            }
        }
    }
}
