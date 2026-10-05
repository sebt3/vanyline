#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

//! Santé du code d'un dépôt — la « synthèse verte/rouge » calculée par UN SEUL code,
//! exécuté à l'identique en local (sandbox, cli) et au central (service de revue de MR).
//!
//! Ce crate est la source unique des INVOCATIONS de vérification ; les règles elles-mêmes
//! (lints, tests, seuils) restent dans le projet analysé. Crate feuille : aucune dépendance
//! interne au workspace, aucun harnais LLM (`rig-core`/`rmcp`/`kube`).
//!
//! Organisation (cf. `./lib.sdd`, section Structure) : [`error`] (erreur `VNL-HLT-NNN`)
//! et [`report`] (rapport sérialisable, source des types TypeScript) forment le modèle ;
//! les parseurs purs [`lcov`], [`cargo_messages`], [`tsc_text`], [`ruff_json`] et
//! [`hadolint_json`] transforment les sorties d'outils en diagnostics ; [`diff`] restreint
//! le rapport aux lignes modifiées d'un diff ; [`checks`] déclare les presets de
//! vérifications par langage (argv, jamais shell) ; et [`runner`] — seule I/O du crate —
//! exécute les vérifications et assemble le rapport.

pub mod cargo_messages;
pub mod checks;
pub mod diff;
pub mod error;
pub mod hadolint_json;
pub mod lcov;
pub mod report;
pub mod ruff_json;
pub mod runner;
pub mod tsc_text;
