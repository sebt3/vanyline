//! Stockage de la configuration vanyline : types du domaine typé (`domain`),
//! erreur unique à identifiants stables (`error`), résolution en couches
//! global/workspace (`layers`), trait d'accès lecture/écriture (`store`) et
//! son implémentation sur fichiers YAML (`fs_store`).
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::assert_is_empty
    )
)]

/// Types du domaine de la configuration : providers, modèles, serveurs MCP,
/// toolsets, agents, skills — avec leurs formes de (dé)sérialisation.
pub mod domain;
/// Erreur unique du crate, à identifiants stables `VNL-CFG-*`.
pub mod error;
/// Store `ConfigStore` sur fichiers : `config.yaml` en couches + fichiers
/// `agents/*.md`, `toolsets/*.yaml`, `skills/<nom>/SKILL.md`.
pub mod fs_store;
/// Résolution des couches de configuration (global + workspace optionnel) :
/// lecture, fusion et découverte de fichiers par couche.
pub mod layers;
/// Trait `ConfigStore` (lecture/écriture des entités du domaine) et store en
/// mémoire `InMemoryConfigStore` utilisé comme fixture.
pub mod store;

pub use error::CfgStoreError;
