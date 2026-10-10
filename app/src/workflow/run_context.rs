//! Fabrication du DAG d'un run : injection du contexte `_run` et validation de portée —
//! contrat : `run_context.sdd`.
//!
//! `MiryadWorkflowStep::run` ne reçoit que `config` et `inputs` : ce module matérialise le DAG
//! envoyé à `trigger_run` en posant `config._run` (le contexte du run, sérialisé) à chaque step,
//! et refuse les kinds incompatibles avec la portée (projet vs sandbox) avant tout appel Restate.
//! Pur et déterministe — la génération du `run_id` reste à l'appelant.

use miryad_core::workflow::DagSteps;
use miryad_core::workflow::WorkflowError;

/// Portée d'un workflow : un workflow de projet n'atteint jamais un kind qui exige une sandbox
/// (décision 2026-10-04, `run_context.sdd`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunScope {
    /// Workflow de projet : kinds `agent` et `rhai` seulement, contexte sans sandbox.
    Project,
    /// Workflow de sandbox : kinds `agent`, `rhai`, `shell` et `gate`, contexte avec sandbox.
    Sandbox,
}

/// Contexte d'UN run, injecté sous `config._run` dans chaque step du DAG matérialisé.
/// Identifiants seulement : aucun secret n'y figure (l'identité d'accès à la sandbox est
/// obtenue par chaque step via le jeton de l'Owner — `run_context.sdd` `Must`).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RunContext {
    /// Identifiant du run, UUID v4 généré par l'appelant (`materialize` ne crée rien).
    pub run_id: String,
    /// Subject OIDC du déclencheur.
    pub user: String,
    /// Clé primaire de l'utilisateur déclencheur.
    pub user_id: i32,
    /// Nom de l'Owner Kubernetes pour qui le run s'exécute.
    pub owner: String,
    /// Nom du projet visé.
    pub project: String,
    /// Sandbox visée — `Some` exactement en portée `Sandbox`.
    pub sandbox: Option<String>,
    /// Origine du déclenchement : `"ui"`, `"agent:<nom>"` ou `"api"`.
    pub triggered_by: String,
    /// Nom de la définition de workflow exécutée.
    pub definition: String,
}

/// Fabrique le DAG exécuté par un run : copie `definition`, valide la portée, puis pose
/// `config._run` (le contexte sérialisé) à chaque step. Une clé `_run` déjà fournie par la
/// définition est écrasée — un auteur de workflow ne peut pas usurper un contexte. La
/// définition d'origine n'est jamais modifiée.
///
/// # Errors
///
/// `MRD-WORKFLOW-004` (`WorkflowError::InvalidDag`) : `scope = Sandbox` sans `ctx.sandbox`,
/// `scope = Project` avec `ctx.sandbox`, ou `kind` d'un step non autorisé pour la portée
/// (message nommant le step et le kind).
pub fn materialize(
    definition: &DagSteps,
    scope: RunScope,
    ctx: &RunContext,
) -> Result<DagSteps, WorkflowError> {
    // La portée est validée AVANT toute injection : un workflow de projet ne doit jamais
    // atteindre un kind qui exige une sandbox (décision 2026-10-04).
    validate_scope(scope, ctx)?;
    let run = serde_json::to_value(ctx).map_err(|e| {
        WorkflowError::InvalidDag(format!("contexte de run non sérialisable : {e}"))
    })?;
    let autorises = allowed_kinds(scope);
    let mut etapes = definition.0.clone();
    for etape in &etapes {
        if !autorises.contains(&etape.kind.as_str()) {
            return Err(WorkflowError::InvalidDag(format!(
                "step {} : kind « {} » non autorisé pour la portée {} (autorisés : {})",
                etape.id,
                etape.kind,
                libelle_portee(scope),
                autorises.join(", ")
            )));
        }
    }
    for etape in &mut etapes {
        // Écrasement contractuel : une `_run` fournie par la définition ne survit pas ; une
        // `config` non-objet (hors contrat `StepDefinition`) reçoit un objet portant seul `_run`.
        match &mut etape.config {
            serde_json::Value::Object(map) => {
                map.insert("_run".to_string(), run.clone());
            }
            autre => {
                *autre = serde_json::Value::Object(serde_json::Map::from_iter([(
                    "_run".to_string(),
                    run.clone(),
                )]));
            }
        }
    }
    Ok(DagSteps(etapes))
}

/// Kinds de step autorisés pour une portée (`run_context.sdd` `Exposes`).
#[must_use]
pub fn allowed_kinds(scope: RunScope) -> &'static [&'static str] {
    match scope {
        RunScope::Project => &["agent", "rhai"],
        RunScope::Sandbox => &["agent", "rhai", "shell", "gate"],
    }
}

/// Contrôle du contexte contre la portée : `Sandbox` sans `ctx.sandbox`, ou `Project` AVEC
/// `ctx.sandbox`, est une erreur de contexte (`MRD-WORKFLOW-004`).
fn validate_scope(scope: RunScope, ctx: &RunContext) -> Result<(), WorkflowError> {
    match (scope, &ctx.sandbox) {
        (RunScope::Sandbox, None) => Err(WorkflowError::InvalidDag(format!(
            "portée {} exige une sandbox : le contexte du run {} n'en porte aucune",
            libelle_portee(scope),
            ctx.run_id
        ))),
        (RunScope::Project, Some(sandbox)) => Err(WorkflowError::InvalidDag(format!(
            "portée {} exclut toute sandbox : le contexte du run {} en porte une ({sandbox})",
            libelle_portee(scope),
            ctx.run_id
        ))),
        _ => Ok(()),
    }
}

fn libelle_portee(scope: RunScope) -> &'static str {
    match scope {
        RunScope::Project => "projet",
        RunScope::Sandbox => "sandbox",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use miryad_core::workflow::StepDefinition;

    fn step(id: &str, kind: &str, config: serde_json::Value) -> StepDefinition {
        StepDefinition {
            id: id.to_string(),
            depends_on: Vec::new(),
            kind: kind.to_string(),
            config,
        }
    }

    fn ctx() -> RunContext {
        RunContext {
            run_id: "11111111-2222-4333-8444-555555555555".to_string(),
            user: "alice".to_string(),
            user_id: 7,
            owner: "alice".to_string(),
            project: "mono".to_string(),
            sandbox: None,
            triggered_by: "ui".to_string(),
            definition: "deploy".to_string(),
        }
    }

    fn ctx_sandbox() -> RunContext {
        RunContext {
            sandbox: Some("sbx-1".to_string()),
            ..ctx()
        }
    }

    fn dag(steps: Vec<StepDefinition>) -> DagSteps {
        DagSteps(steps)
    }

    // ── Scenario: injection_du_contexte ─────────────────────────────────────
    // Chaque step porte `config._run` égal au contexte ; les autres champs de `config`
    // intacts. Cas limite porté par la tranche : un step dont la `config` est `null`
    // (non-objet) reçoit un objet ne contenant que `_run` — signalé au rapport.
    #[test]
    fn injection_du_contexte() {
        let definition = dag(vec![
            step("a", "agent", serde_json::json!({"prompt": "x"})),
            step("b", "rhai", serde_json::json!({"n": 1})),
            step("c", "agent", serde_json::Value::Null),
        ]);
        let ctx = ctx();

        let out = materialize(&definition, RunScope::Project, &ctx)
            .expect("un DAG projet agent/rhai est valide");

        let expected = serde_json::to_value(&ctx).unwrap();
        assert_eq!(
            out.0[0].config.get("_run"),
            Some(&expected),
            "step a porte le contexte"
        );
        assert_eq!(out.0[0].config.get("prompt"), Some(&serde_json::json!("x")));
        assert_eq!(
            out.0[1].config.get("_run"),
            Some(&expected),
            "step b porte le contexte"
        );
        assert_eq!(out.0[1].config.get("n"), Some(&serde_json::json!(1)));
        assert_eq!(
            out.0[2].config.get("_run"),
            Some(&expected),
            "step c (config null) reçoit un objet avec _run"
        );
        assert_eq!(
            out.0[2].config.as_object().unwrap().len(),
            1,
            "seule la clé _run est posée sur une config non-objet"
        );
    }

    // ── Scenario: cle_run_usurpee_ecrasee ───────────────────────────────────
    // Une `_run` fournie par la définition est écrasée : impossible d'usurper un contexte.
    #[test]
    fn cle_run_usurpee_ecrasee() {
        let definition = dag(vec![step(
            "a",
            "agent",
            serde_json::json!({"_run": {"owner": "autrui"}, "prompt": "ok"}),
        )]);
        let ctx = ctx();

        let out = materialize(&definition, RunScope::Project, &ctx).unwrap();

        let run = out.0[0].config.get("_run").unwrap().as_object().unwrap();
        assert_eq!(
            run.get("owner"),
            Some(&serde_json::json!("alice")),
            "_run.owner est l'Owner du RunContext, pas celui fourni"
        );
        assert_eq!(
            run.get("run_id"),
            Some(&serde_json::json!(ctx.run_id)),
            "toute la `_run` fournie est écrasée, pas champ à champ"
        );
        assert_eq!(
            out.0[0].config.get("prompt"),
            Some(&serde_json::json!("ok"))
        );
    }

    // ── Scenario: shell_refuse_dans_un_workflow_de_projet ───────────────────
    // InvalidDag (MRD-WORKFLOW-004) nommant le step et le kind `shell`.
    #[test]
    fn shell_refuse_dans_un_workflow_de_projet() {
        let definition = dag(vec![
            step("a", "agent", serde_json::json!({})),
            step("compiler", "shell", serde_json::json!({"cmd": "make"})),
        ]);

        let err = materialize(&definition, RunScope::Project, &ctx())
            .expect_err("shell est refusé en portée projet");

        match &err {
            WorkflowError::InvalidDag(msg) => {
                assert!(msg.contains("compiler"), "le message nomme le step : {msg}");
                assert!(msg.contains("shell"), "le message nomme le kind : {msg}");
            }
            other => panic!("attendu InvalidDag, rendu {other:?}"),
        }
        assert!(
            err.to_string().starts_with("MRD-WORKFLOW-004"),
            "code MRD-WORKFLOW-004 rendu : {err}"
        );
    }

    // ── Scenario: gate_refuse_dans_un_workflow_de_projet ────────────────────
    #[test]
    fn gate_refuse_dans_un_workflow_de_projet() {
        let definition = dag(vec![step(
            "g",
            "gate",
            serde_json::json!({"check": "health"}),
        )]);

        let err = materialize(&definition, RunScope::Project, &ctx())
            .expect_err("gate est refusé en portée projet");

        match &err {
            WorkflowError::InvalidDag(msg) => {
                assert!(msg.contains('g'), "le message nomme le step : {msg}");
                assert!(msg.contains("gate"), "le message nomme le kind : {msg}");
            }
            other => panic!("attendu InvalidDag, rendu {other:?}"),
        }
    }

    // ── Scenario: portee_sandbox_exige_une_sandbox ──────────────────────────
    #[test]
    fn portee_sandbox_exige_une_sandbox() {
        let definition = dag(vec![step("a", "agent", serde_json::json!({}))]);

        let err = materialize(&definition, RunScope::Sandbox, &ctx())
            .expect_err("la portée Sandbox sans ctx.sandbox est une erreur de contexte");

        match &err {
            WorkflowError::InvalidDag(msg) => {
                assert!(msg.contains("sandbox"), "le message nomme la cause : {msg}");
            }
            other => panic!("attendu InvalidDag, rendu {other:?}"),
        }
    }

    // Rejet symétrique du Raises : portée Projet AVEC ctx.sandbox.
    #[test]
    fn portee_projet_refuse_un_contexte_sandbox() {
        let definition = dag(vec![step("a", "agent", serde_json::json!({}))]);

        let err = materialize(&definition, RunScope::Project, &ctx_sandbox())
            .expect_err("la portée Project avec ctx.sandbox est une erreur de contexte");

        assert!(
            matches!(err, WorkflowError::InvalidDag(_)),
            "attendu InvalidDag, rendu {err:?}"
        );
    }

    // ── Scenario: kinds_de_la_portee_sandbox ────────────────────────────────
    #[test]
    fn kinds_de_la_portee_sandbox() {
        assert_eq!(
            allowed_kinds(RunScope::Sandbox),
            &["agent", "rhai", "shell", "gate"]
        );
    }

    // Volet Projet du même `Exposes` : agent et rhai seulement.
    #[test]
    fn kinds_de_la_portee_projet() {
        assert_eq!(allowed_kinds(RunScope::Project), &["agent", "rhai"]);
    }

    // `Must` : ne jamais inclure de secret dans `_run` — l'objet injecté porte exactement
    // les huit champs du contrat `@RunContext`, rien d'autre.
    #[test]
    fn secrets_jamais_dans_run() {
        let definition = dag(vec![step("a", "agent", serde_json::json!({}))]);
        let out = materialize(&definition, RunScope::Sandbox, &ctx_sandbox()).unwrap();

        let run = out.0[0].config.get("_run").unwrap().as_object().unwrap();
        let mut cles: Vec<&str> = run.keys().map(String::as_str).collect();
        cles.sort_unstable();
        assert_eq!(
            cles,
            vec![
                "definition",
                "owner",
                "project",
                "run_id",
                "sandbox",
                "triggered_by",
                "user",
                "user_id"
            ],
            "_run ne porte que les identifiants du contrat"
        );
    }

    // `Must not` : la définition stockée n'est jamais modifiée par la matérialisation.
    #[test]
    fn la_definition_stockee_n_est_pas_modifiee() {
        let definition = dag(vec![step("a", "agent", serde_json::json!({"prompt": "x"}))]);

        materialize(&definition, RunScope::Project, &ctx()).unwrap();

        assert_eq!(
            definition.0[0].config,
            serde_json::json!({"prompt": "x"}),
            "la copie est faite pour le run, l'origine reste intacte"
        );
    }
}
