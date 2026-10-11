//! Couche workflow de l'app : moteur DAG de miryad-core sur Restate — contrat : `mod.sdd`.
//! Le moteur n'est démarré que si `WorkflowConfigApp::from_config` rend un configuré
//! (`VNL_RESTATE_*`) ; sinon l'app tourne sans workflows, aucun port ouvert.

#[allow(dead_code)]
// W4 consommateurs à venir (`api/workflows.rs` : `materialize`, `allowed_kinds`) — tooling.sdd
pub mod run_context;
pub mod runtime;
#[allow(dead_code)]
// W4 consommateurs à venir (`api/workflows.rs` : `fetch_statuses`, `fetch_status`, `cancel` sur @WorkflowRuntime) — tooling.sdd
pub mod status;
#[allow(dead_code)]
// `build_registry` consommé par `runtime.rs` (W3) ; reste items internes W4 (`SandboxAccess::delete`, constantes `KIND`, garde `StepLimit` d'`agent`) — tooling.sdd
pub mod steps;

pub use runtime::{WorkflowConfigApp, WorkflowRuntime};
