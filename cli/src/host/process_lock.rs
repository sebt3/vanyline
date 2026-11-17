//! Exclusion d'une session entre processus de la CLI (verrou consultatif du système). Contrat : `host/process_lock.sdd`.

use std::fs::{self, OpenOptions, TryLockError};
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use lymaille::SessionId;

use super::HostError;
use super::files::est_un_nom_de_fichier;
use super::layers::Layers;

/// Répertoire des verrous, sous l'état local.
const LOCKS_DIR: &str = "locks";

/// Marqueur d'un verrou impossible à prendre pour une raison d'E/S.
const IO: &str = "VNL-CLI-HST-006";

/// Marqueur d'un identifiant de session qui n'est pas un nom de fichier.
const INVALID: &str = "VNL-CLI-HST-008";

/// Garde d'une session : tant qu'elle vit, aucun autre processus de la CLI n'ouvre la session. Le verrou est libéré à
/// sa chute.
#[derive(Debug)]
pub struct SessionLock {
    _verrou: std::fs::File,
}

impl SessionLock {
    /// Prend le verrou de la session `id`, sans attendre.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Locked`] when another process holds the session, [`HostError::Invalid`] when the identifier
    /// is not a single file name, and [`HostError::Io`] when the lock file cannot be used.
    pub fn acquire(layers: &Layers, id: &SessionId) -> Result<Self, HostError> {
        let session = id.as_str();
        if !est_un_nom_de_fichier(session) {
            return Err(HostError::Invalid {
                cause: format!("{INVALID}: a session identifier must be a single file name"),
            });
        }
        let dossier = layers.state().join(LOCKS_DIR);
        fs::create_dir_all(&dossier).map_err(|erreur| io_error(&dossier, &erreur))?;
        let chemin = dossier.join(format!("{session}.lock"));
        // Pas de troncature à l'ouverture : le fichier d'un autre processus garde son PID tant que le verrou n'est pas pris.
        let mut fichier = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&chemin)
            .map_err(|erreur| io_error(&chemin, &erreur))?;
        match fichier.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(HostError::Locked {
                    session: session.to_owned(),
                    pid: pid_inscrit(&chemin),
                });
            }
            Err(TryLockError::Error(erreur)) => return Err(io_error(&chemin, &erreur)),
        }
        fichier
            .set_len(0)
            .map_err(|erreur| io_error(&chemin, &erreur))?;
        fichier
            .write_all(format!("{}\n", std::process::id()).as_bytes())
            .map_err(|erreur| io_error(&chemin, &erreur))?;
        Ok(Self { _verrou: fichier })
    }
}

/// Erreur d'E/S sur le fichier de verrou, avec son marqueur.
fn io_error(chemin: &Path, erreur: &io::Error) -> HostError {
    HostError::Io {
        chemin: chemin.to_path_buf(),
        cause: format!("{IO}: {erreur}"),
    }
}

/// PID inscrit dans le fichier de verrou, à titre indicatif. `None` si absent ou illisible.
fn pid_inscrit(chemin: &Path) -> Option<u32> {
    fs::read_to_string(chemin)
        .ok()
        .and_then(|texte| texte.trim().parse::<u32>().ok())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use lymaille::SessionId;

    use super::{HostError, SessionLock};
    use crate::host::layers::Layers;

    fn session(texte: &str) -> SessionId {
        SessionId::new(texte).expect("a non-empty text makes a session identifier")
    }

    fn layers(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    #[test]
    fn second_opening_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let _premiere =
            SessionLock::acquire(&layers, &session("s-1")).expect("the first acquire must succeed");

        let seconde = SessionLock::acquire(&layers, &session("s-1"));

        assert!(
            matches!(&seconde, Err(HostError::Locked { session, .. }) if session == "s-1"),
            "a second acquire on the same session must be refused with Locked naming the session"
        );
        let pid_inscrit = match seconde {
            Err(HostError::Locked { pid, .. }) => pid,
            _ => None,
        };
        assert_eq!(
            pid_inscrit,
            Some(std::process::id()),
            "the refusal must carry the pid written in the lock file, here the pid of this test process"
        );
    }

    #[test]
    fn lock_is_released_when_dropped() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        {
            let _garde = SessionLock::acquire(&layers, &session("s-1"))
                .expect("the first acquire must succeed");
        }

        let apres = SessionLock::acquire(&layers, &session("s-1"));

        assert!(
            apres.is_ok(),
            "once the guard is dropped, the session must be acquirable again"
        );
    }

    #[test]
    fn lock_file_left_by_a_dead_process_does_not_block() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let dossier = layers.state().join("locks");
        fs::create_dir_all(&dossier).expect("the locks directory must be created for the fixture");
        fs::write(dossier.join("s-1.lock"), "4242\n").expect("the stale lock file must be written");

        let acquis = SessionLock::acquire(&layers, &session("s-1"));

        assert!(
            acquis.is_ok(),
            "a lock file without a live holder must not make the session unusable"
        );
    }

    #[test]
    fn identifier_that_is_not_a_file_name_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());

        let refusee = SessionLock::acquire(&layers, &session("../hors"));

        assert!(
            matches!(refusee, Err(HostError::Invalid { .. })),
            "a lock must only ever be taken on a session file name, never on a path"
        );
    }
}
