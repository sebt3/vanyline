//! Cœur partagé LLM / MCP / chat du monorepo : types d'événements de session
//! ([`event`]), moteur de tour ([`session`]), construction de modèles ([`model`]),
//! outillage MCP préfixé ([`prefixed_mcp`]), tools builtins ([`builtin`]), types de
//! persistance et erreur unique ([`VnyError`] re-exportée à la racine). La racine porte
//! son propre helper [`encode_git_path`] et son jeu d'encodage [`GIT_ENCODE_SET`],
//! le câblage des modules et les re-exports canoniques (cf. `./lib.sdd`). Le module
//! `k8s` n'existe que sous la feature cargo `k8s`.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

use percent_encoding::SIMPLE_ENCODE_SET;
use percent_encoding::define_encode_set;
use percent_encoding::utf8_percent_encode;

define_encode_set! {
    /// Jeu d'encodage percent de [`encode_git_path`] : [`SIMPLE_ENCODE_SET`] étendu
    /// à l'espace et à `%` (cf. `./lib.sdd`, section `Must`).
    pub GIT_ENCODE_SET = [SIMPLE_ENCODE_SET] | {' ', '%'}
}

/// Encodage percent d'un chemin git, segment par segment, en PRÉSERVANT les
/// séparateurs `/` et les caractères sûrs `-`, `_`, `.`, `~`.
#[must_use]
pub fn encode_git_path(path: &str) -> String {
    utf8_percent_encode(path, GIT_ENCODE_SET).to_string()
}

/// Tools builtins du harnais : `skill`, `task`, `todo` (specs sœurs `./builtin/`).
pub mod builtin;
pub use vanyline_cfgstore::{domain, store};
mod error;
/// Modèle d'événements de session ([`ChatEvent`]) et pipeline de stream d'un tour.
pub mod event;
#[cfg(feature = "k8s")]
/// Client K8s typé pour les CRDs vanyline (découverte, CRUD namespaced,
/// constructeurs d'URL de sandbox, watch normalisé) — cf. `./k8s.sdd`.
pub mod k8s;
/// Construction des modèles rig (ollama / openai-compatible) et paramètres d'agent.
pub mod model;
/// Résolution/connexion MCP filtrée et tools préfixés exposés au LLM.
pub mod prefixed_mcp;
/// Moteur de tour : [`SessionContext`], résolution de config et [`run_agent_turn`].
pub mod session;
mod types;

pub use error::*;
pub use prefixed_mcp::*;
pub use types::*;

pub use rig_core::tool::server::ToolServerHandle;

#[cfg(test)]
mod tests {
    use super::encode_git_path;

    /// Scenario `encode_git_path_preserve_separateurs_et_ascii_surs`.
    #[test]
    fn encode_git_path_preserve_separateurs_et_ascii_surs() {
        assert_eq!(encode_git_path("branches/feature-x"), "branches/feature-x");
        assert_eq!(
            encode_git_path("refs/heads/feature_x.1~2"),
            "refs/heads/feature_x.1~2"
        );
        assert_eq!(encode_git_path("status"), "status");
    }

    /// Scenario `encode_git_path_encode_espace_et_percent`.
    #[test]
    fn encode_git_path_encode_espace_et_percent() {
        assert_eq!(
            encode_git_path("branches/feature x"),
            "branches/feature%20x"
        );
        assert_eq!(encode_git_path("a%20b"), "a%2520b");
    }

    /// Scenario `encode_git_path_encode_controles`.
    #[test]
    fn encode_git_path_encode_controles() {
        assert_eq!(encode_git_path("a\tb\nc\rd"), "a%09b%0Ac%0Dd");
    }

    /// Scenario `encode_git_path_encode_non_ascii_octet_par_octet`.
    #[test]
    fn encode_git_path_encode_non_ascii_octet_par_octet() {
        assert_eq!(encode_git_path("café/é"), "caf%C3%A9/%C3%A9");
    }

    /// Scenario `encode_git_path_ascii_imprimables_dangereux_passent`.
    #[test]
    fn encode_git_path_ascii_imprimables_dangereux_passent() {
        let input = "#?&=+@[]\\<>{}^|;";
        assert_eq!(encode_git_path(input), input);
    }

    /// Scenario `encode_git_path_entree_vide`.
    #[test]
    fn encode_git_path_entree_vide() {
        assert_eq!(encode_git_path(""), "");
    }
}
