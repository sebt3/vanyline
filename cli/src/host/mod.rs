//! Hôte lymaille de la CLI : déclarations des sous-modules et erreur commune.
//! Contrat : `host/mod.sdd`. Aucune logique ni E/S ici.

use std::path::PathBuf;

pub mod audit;
pub mod catalog_store;
pub mod context_derive;
pub mod context_store;
pub mod files;
pub mod layers;
pub mod memory_store;
pub mod owner_auth;
pub mod process_lock;
pub mod secrets;
pub mod session_index;
pub mod session_store;
pub mod toolset_store;

/// Erreur commune aux modules de l'hôte (`host/mod.sdd`, `HostError`).
///
/// Les causes portent le marqueur `VNL-CLI-HST-NNN` de l'enfant qui les lève. Aucune variante ne porte la valeur d'un
/// secret.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Lecture ou écriture d'un fichier impossible.
    #[error("{chemin}: {cause}")]
    Io {
        /// Chemin du fichier concerné.
        chemin: PathBuf,
        /// Cause, avec son marqueur.
        cause: String,
    },
    /// Fichier illisible ou invalide (contenu non UTF-8, format inconnu).
    #[error("{chemin}: {cause}")]
    Format {
        /// Chemin du fichier concerné.
        chemin: PathBuf,
        /// Cause, avec son marqueur.
        cause: String,
    },
    /// Définition refusée par la validation de lymaille.
    #[error("{cause}")]
    Invalid {
        /// Cause, avec son marqueur.
        cause: String,
    },
    /// Résolution de secret refusée ou impossible.
    #[error("{cause}")]
    Secret {
        /// Cause, sans valeur de secret, avec son marqueur.
        cause: String,
    },
    /// Lecture de ressources du cluster impossible.
    #[error("{cause}")]
    Cluster {
        /// Cause, avec son marqueur.
        cause: String,
    },
    /// Session déjà ouverte par un autre processus de la CLI.
    #[error(
        "VNL-CLI-HST-007: session `{session}` is already open by another vanyline process (pid {})",
        pid.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    )]
    Locked {
        /// Identifiant de la session.
        session: String,
        /// PID inscrit dans le verrou, à titre indicatif.
        pid: Option<u32>,
    },
}
