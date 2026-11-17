//! Contexte éphémère de session dérivé du cluster, et type de contexte livré avec la CLI.
//! Contrat : `host/context_derive.sdd`.

use std::future::Future;
use std::pin::Pin;

use std::fmt::Display;

use lymaille::{ContextLevel, ContextType, RoleSpec, ToolsetContext};
use serde_json::{Value, json};
use vanyline_crds::{Project, Sandbox, sandbox_cr_name};
use vanyline_lib::VnyError;
use vanyline_lib::k8s::VnlK8sClient;

use super::HostError;

/// Nom du type de contexte livré avec la CLI (`context_derive.sdd`, `cli_context_type`).
pub const CLI_CONTEXT_TYPE: &str = "vanyline-cli";
/// Agent principal par défaut du type livré, et agent livré avec la CLI.
pub const CLI_AGENT: &str = "vanyline-cli";
/// Version du type livré avec la CLI.
pub const CLI_CONTEXT_TYPE_VERSION: &str = "0.1.0";
/// Nom du rôle MCP lié à la sandbox.
pub const SANDBOX_ROLE: &str = "sandbox";

/// Marqueur d'un projet ou d'une sandbox refusé par le cluster (introuvable, RBAC, panne de lecture).
const CLUSTER: &str = "VNL-CLI-HST-044";
/// Marqueur d'une sandbox qui n'appartient pas au projet, ou qui n'est pas `Running`.
const SANDBOX_STATE: &str = "VNL-CLI-HST-045";
/// Marqueur d'un nom de sandbox refusé par le nommage de `vanyline-crds`.
const NAME: &str = "VNL-CLI-HST-046";
/// Marqueur d'un contexte ou d'un type refusé par la validation de lymaille.
const CONTEXT: &str = "VNL-CLI-HST-047";
/// Marqueur de la version constante du type livré, illisible.
const VERSION: &str = "VNL-CLI-HST-048";

/// Refus d'une lecture du cluster, sans URL ni jeton.
#[derive(Debug)]
pub enum ClusterRefusal {
    /// L'objet n'existe pas (`K8sNotFound`).
    NotFound,
    /// Autre refus de l'apiserver, avec sa cause.
    Other(String),
}

/// Lectures du cluster nécessaires à la dérivation (seam, `context_derive.sdd`). Implémenté par le client K8s de la lib.
pub trait ClusterSource: Send + Sync {
    /// Lit le Project `name` de l'Owner `owner`.
    fn project<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Project, ClusterRefusal>> + Send + 'a>>;
    /// Lit la Sandbox `name` (nom de CR) de l'Owner `owner`.
    fn sandbox<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Sandbox, ClusterRefusal>> + Send + 'a>>;
    /// URL MCP interne de la sandbox `name` (nom de CR) de l'Owner `owner`.
    fn sandbox_mcp_url<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ClusterRefusal>> + Send + 'a>>;
}

/// Contexte dérivé pour une session : le contexte éphémère, son type, et l'URL MCP si une sandbox est liée.
#[derive(Debug)]
pub struct DerivedContext {
    /// Contexte éphémère de la session (paramètres et rôles, jamais de jeton).
    pub context: ToolsetContext,
    /// Type de contexte livré avec la CLI.
    pub context_type: ContextType,
    /// URL MCP de la sandbox, si `--sandbox` est donné.
    pub mcp_url: Option<String>,
}

/// Le type de contexte livré avec la CLI : constante compilée, sans maintenance à la main.
///
/// # Errors
///
/// `HostError::Invalid` si lymaille refuse la construction (impossible pour la constante livrée).
pub fn cli_context_type() -> Result<ContextType, HostError> {
    let version = vynil_core::Semver::parse(CLI_CONTEXT_TYPE_VERSION)
        .map_err(|erreur| invalid(VERSION, erreur))?;
    // `project` et `namespace` sont requis ; `branch` ne l'est pas (un projet peut ne pas avoir de branche par défaut).
    let schema = json!({
        "type": "object",
        "properties": {
            "project": {"type": "string"},
            "branch": {"type": "string"},
            "namespace": {"type": "string"}
        },
        "required": ["project", "namespace"]
    });
    let contexte = ContextType::new(CLI_CONTEXT_TYPE.to_owned(), version, schema)
        .map_err(|erreur| invalid(CONTEXT, erreur))?;
    let contexte = contexte
        .with_role(RoleSpec::new(SANDBOX_ROLE.to_owned(), false))
        .map_err(|erreur| invalid(CONTEXT, erreur))?;
    contexte
        .with_default_agent(CLI_AGENT.to_owned())
        .map_err(|erreur| invalid(CONTEXT, erreur))
}

/// Erreur de validation, marquée par sa cause.
fn invalid(marqueur: &str, erreur: impl Display) -> HostError {
    HostError::Invalid {
        cause: format!("{marqueur}: {erreur}"),
    }
}

/// Erreur de lecture du cluster pour la cible `cible` (projet ou nom de CR de sandbox).
fn cluster_error(cible: &str, refus: ClusterRefusal) -> HostError {
    let cause = match refus {
        ClusterRefusal::NotFound => format!("{CLUSTER}: `{cible}` introuvable"),
        ClusterRefusal::Other(cause) => format!("{CLUSTER}: lecture de `{cible}` refusée: {cause}"),
    };
    HostError::Cluster { cause }
}

/// Refus de l'apiserver, réduit à la forme attendue par le seam : `K8sNotFound` devient `NotFound`.
fn refus(erreur: VnyError) -> ClusterRefusal {
    match erreur {
        VnyError::K8sNotFound(_) => ClusterRefusal::NotFound,
        autre => ClusterRefusal::Other(autre.to_string()),
    }
}

impl ClusterSource for VnlK8sClient {
    fn project<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Project, ClusterRefusal>> + Send + 'a>> {
        Box::pin(async move {
            VnlK8sClient::get_project(self, owner, name)
                .await
                .map_err(refus)
        })
    }

    fn sandbox<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Sandbox, ClusterRefusal>> + Send + 'a>> {
        Box::pin(async move {
            VnlK8sClient::get_sandbox(self, owner, name)
                .await
                .map_err(refus)
        })
    }

    fn sandbox_mcp_url<'a>(
        &'a self,
        owner: &'a str,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ClusterRefusal>> + Send + 'a>> {
        Box::pin(async move {
            VnlK8sClient::sandbox_mcp_url(self, owner, name)
                .await
                .map_err(refus)
        })
    }
}

/// Dérive le contexte de la session sur `projet` (et `sandbox` si donné) depuis le cluster.
///
/// # Errors
///
/// `HostError::Cluster` si le projet ou la sandbox est absent, refusé, ou si la sandbox n'appartient pas au projet ou
/// n'est pas `Running` ; `HostError::Invalid` si le nom de sandbox n'est pas un label DNS valide.
pub async fn derive_context<S: ClusterSource>(
    source: &S,
    owner: &str,
    projet: &str,
    sandbox: Option<&str>,
) -> Result<DerivedContext, HostError> {
    // Le nom de CR est validé avant tout appel au cluster.
    let nom_cr = sandbox
        .map(|nom| sandbox_cr_name(projet, nom).map_err(|erreur| invalid(NAME, erreur)))
        .transpose()?;

    let projet_cr = source
        .project(owner, projet)
        .await
        .map_err(|refus| cluster_error(projet, refus))?;
    let namespace = projet_cr
        .metadata
        .namespace
        .clone()
        .ok_or_else(|| HostError::Cluster {
            cause: format!("{CLUSTER}: le namespace du projet `{projet}` est absent"),
        })?;

    // Sans sandbox : la branche du projet. Avec sandbox : la branche de la sandbox, et le rôle MCP lié à son Service.
    let (branche, nom_contexte, mcp_url) = match nom_cr {
        None => (
            projet_cr.spec.default_branch.clone(),
            projet.to_owned(),
            None,
        ),
        Some(nom_cr) => {
            let sandbox_cr = source
                .sandbox(owner, &nom_cr)
                .await
                .map_err(|refus| cluster_error(&nom_cr, refus))?;
            if sandbox_cr.spec.project != projet {
                return Err(HostError::Cluster {
                    cause: format!(
                        "{SANDBOX_STATE}: la sandbox `{nom_cr}` n'appartient pas au projet `{projet}`"
                    ),
                });
            }
            let phase = sandbox_cr
                .status
                .as_ref()
                .and_then(|status| status.phase.as_deref());
            if phase != Some("Running") {
                return Err(HostError::Cluster {
                    cause: format!(
                        "{SANDBOX_STATE}: la sandbox `{nom_cr}` n'est pas Running (phase : {})",
                        phase.unwrap_or("absente")
                    ),
                });
            }
            let url = source
                .sandbox_mcp_url(owner, &nom_cr)
                .await
                .map_err(|refus| cluster_error(&nom_cr, refus))?;
            (
                Some(sandbox_cr.spec.branch.clone()),
                nom_cr.clone(),
                Some(url),
            )
        }
    };

    let mut parametres: Vec<(&str, Value)> =
        vec![("project", json!(projet)), ("namespace", json!(namespace))];
    if let Some(branche) = branche {
        parametres.push(("branch", json!(branche)));
    }
    let mut context = ToolsetContext::new(nom_contexte, ContextLevel::Session)
        .map_err(|erreur| invalid(CONTEXT, erreur))?;
    for (nom, valeur) in parametres {
        context = context
            .with_parameter(nom.to_owned(), valeur)
            .map_err(|erreur| invalid(CONTEXT, erreur))?;
    }
    if let Some(url) = &mcp_url {
        context = context
            .with_role(SANDBOX_ROLE.to_owned(), url.clone())
            .map_err(|erreur| invalid(CONTEXT, erreur))?;
    }
    Ok(DerivedContext {
        context,
        context_type: cli_context_type()?,
        mcp_url,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use vanyline_crds::{Project, ProjectSpec, Sandbox, SandboxSpec, SandboxStatus};

    use super::{
        CLI_AGENT, CLI_CONTEXT_TYPE, ClusterRefusal, ClusterSource, HostError, cli_context_type,
        derive_context,
    };
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;

    /// Faux cluster : un Project, des sandboxes par nom de CR, et une URL MCP. Absent = `NotFound`.
    struct FauxCluster {
        projet: Option<Project>,
        sandboxes: HashMap<String, Sandbox>,
    }

    fn refus_absent() -> ClusterRefusal {
        ClusterRefusal::NotFound
    }

    impl ClusterSource for FauxCluster {
        fn project<'a>(
            &'a self,
            _owner: &'a str,
            _name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Project, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move { self.projet.clone().ok_or_else(refus_absent) })
        }

        fn sandbox<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Sandbox, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move { self.sandboxes.get(name).cloned().ok_or_else(refus_absent) })
        }

        fn sandbox_mcp_url<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move {
                if self.sandboxes.contains_key(name) {
                    Ok(format!(
                        "http://sandbox-{name}.vanyline-app-alice.svc:3000/mcp"
                    ))
                } else {
                    Err(refus_absent())
                }
            })
        }
    }

    fn projet(branche: Option<&str>) -> Project {
        let mut spec = json!({"owner": "alice", "repoUrl": "https://example.invalid/demo.git"});
        if let Some(branche) = branche {
            spec["defaultBranch"] = json!(branche);
        }
        let spec: ProjectSpec = serde_json::from_value(spec).expect("spec de projet valide");
        let mut projet = Project::new("demo", spec);
        projet.metadata.namespace = Some("vanyline-app-alice".to_owned());
        projet
    }

    /// Sandbox de CR `nom`, du projet `projet_de_la_sandbox`, sur `branche`, dans la phase `phase`.
    fn sandbox(nom: &str, projet_de_la_sandbox: &str, branche: &str, phase: &str) -> Sandbox {
        let spec: SandboxSpec =
            serde_json::from_value(json!({"project": projet_de_la_sandbox, "branch": branche}))
                .expect("spec de sandbox valide");
        let mut sandbox = Sandbox::new(nom, spec);
        sandbox.metadata.namespace = Some("vanyline-app-alice".to_owned());
        sandbox.status = Some(SandboxStatus {
            phase: Some(phase.to_owned()),
            ..SandboxStatus::default()
        });
        sandbox
    }

    fn cluster(projet: Option<Project>, sandboxes: Vec<Sandbox>) -> FauxCluster {
        FauxCluster {
            projet,
            sandboxes: sandboxes
                .into_iter()
                .map(|s| (s.metadata.name.clone().unwrap_or_default(), s))
                .collect(),
        }
    }

    /// Paramètres du contexte, sérialisé : le contexte ne porte que des noms et des identifiants publics.
    fn parametres(contexte: &lymaille::ToolsetContext) -> Value {
        serde_json::to_value(contexte).expect("contexte sérialisable")["parameters"].clone()
    }

    /// Scenario: contexte de projet sans sandbox
    #[tokio::test]
    async fn contexte_de_projet_sans_sandbox() {
        let source = cluster(Some(projet(Some("main"))), vec![]);
        let derive = derive_context(&source, "alice", "demo", None).await;
        let Ok(derive) = derive else {
            panic!("contexte attendu, obtenu {derive:?}");
        };
        let params = parametres(&derive.context);
        assert_eq!(params["project"], json!("demo"));
        assert_eq!(params["branch"], json!("main"));
        assert_eq!(params["namespace"], json!("vanyline-app-alice"));
        assert_eq!(derive.mcp_url, None);
        let roles = serde_json::to_value(&derive.context).expect("sérialisable")["roles"].clone();
        assert!(
            roles.get("sandbox").is_none(),
            "aucun rôle MCP sans sandbox : {roles}"
        );
    }

    /// Scenario: contexte avec sandbox
    #[tokio::test]
    async fn contexte_avec_sandbox() {
        let source = cluster(
            Some(projet(Some("main"))),
            vec![sandbox("demo-feature", "demo", "feature", "Running")],
        );
        let derive = derive_context(&source, "alice", "demo", Some("feature")).await;
        let Ok(derive) = derive else {
            panic!("contexte attendu, obtenu {derive:?}");
        };
        let url = "http://sandbox-demo-feature.vanyline-app-alice.svc:3000/mcp";
        assert_eq!(derive.mcp_url.as_deref(), Some(url));
        let roles = serde_json::to_value(&derive.context).expect("sérialisable")["roles"].clone();
        assert_eq!(roles["sandbox"], json!(url));
        assert_eq!(parametres(&derive.context)["branch"], json!("feature"));
    }

    /// Scenario: sandbox d'un autre projet ou arrêtée
    #[tokio::test]
    async fn sandbox_d_un_autre_projet_ou_arretee_est_refusee() {
        let autre = cluster(
            Some(projet(Some("main"))),
            vec![sandbox("demo-feature", "autre", "feature", "Running")],
        );
        assert!(matches!(
            derive_context(&autre, "alice", "demo", Some("feature")).await,
            Err(HostError::Cluster { .. })
        ));
        let suspendue = cluster(
            Some(projet(Some("main"))),
            vec![sandbox("demo-feature", "demo", "feature", "Suspended")],
        );
        assert!(matches!(
            derive_context(&suspendue, "alice", "demo", Some("feature")).await,
            Err(HostError::Cluster { .. })
        ));
    }

    /// Scenario: projet introuvable
    #[tokio::test]
    async fn projet_introuvable() {
        let source = cluster(None, vec![]);
        match derive_context(&source, "alice", "demo", None).await {
            Err(HostError::Cluster { cause }) => assert!(cause.contains("introuvable"), "{cause}"),
            autre => panic!("Cluster « introuvable » attendu, obtenu {autre:?}"),
        }
    }

    /// Nom de sandbox refusé par le nommage avant tout appel au cluster.
    #[tokio::test]
    async fn nom_de_sandbox_invalide_est_refuse_avant_le_cluster() {
        let source = cluster(None, vec![]);
        assert!(matches!(
            derive_context(&source, "alice", "demo", Some("Feature_x")).await,
            Err(HostError::Invalid { .. })
        ));
    }

    /// Le type livré : nom, agent par défaut, rôle `sandbox` optionnel.
    #[test]
    fn type_livre_avec_agent_et_role_sandbox() {
        let ct = cli_context_type().expect("constante valide");
        let valeur = serde_json::to_value(&ct).expect("sérialisable");
        assert_eq!(valeur["name"], json!(CLI_CONTEXT_TYPE));
        assert_eq!(ct.default_agent(), Some(CLI_AGENT));
        assert_eq!(valeur["roles"][0]["name"], json!("sandbox"));
        assert_eq!(valeur["roles"][0]["required"], json!(false));
    }
}
