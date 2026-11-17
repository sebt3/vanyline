//! `SessionStore` de lymaille sur des fichiers : un état de session par fichier JSON, écrit atomiquement. Contrat :
//! `host/session_store.sdd`.

use std::fs;
use std::future::{self, Future};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lymaille::{SessionError, SessionId, SessionResult, SessionState};

use super::HostError;
use super::files;
use super::layers::Layers;

/// Sous-répertoire des états, sous l'état local.
const SESSIONS_DIR: &str = "sessions";
/// Extension des fichiers d'état.
const EXTENSION: &str = "json";
/// Droits des fichiers d'état : privés à l'utilisateur.
const MODE_PRIVE: u32 = 0o600;

/// Marqueur d'un identifiant de session qui n'est pas un nom de fichier.
const INVALID: &str = "VNL-CLI-HST-009";
/// Marqueur d'un état illisible ou d'un format que lymaille ne lit pas.
const FORMAT: &str = "VNL-CLI-HST-010";

/// Stockage des états de session sur `<état>/sessions/<id>.json`.
#[derive(Debug, Clone)]
pub struct FileSessionStore {
    dossier: PathBuf,
}

impl FileSessionStore {
    /// Stockage rangé sous l'état local de `layers`. Ne crée rien.
    #[must_use]
    pub fn new(layers: &Layers) -> Self {
        Self {
            dossier: layers.state().join(SESSIONS_DIR),
        }
    }

    /// Chemin du fichier d'état de `id`. Refuse un identifiant qui ne serait pas un nom de fichier.
    fn chemin(&self, id: &SessionId) -> Result<PathBuf, HostError> {
        if !files::est_un_nom_de_fichier(id.as_str()) {
            return Err(HostError::Invalid {
                cause: format!("{INVALID}: a session identifier must be a single file name"),
            });
        }
        Ok(self.dossier.join(format!("{}.{EXTENSION}", id.as_str())))
    }

    /// Lit l'état de `id`, `None` s'il n'existe pas.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Format`] with the path when the file is not a readable state, and [`HostError::Io`] or
    /// [`HostError::Invalid`] for the other failures.
    pub fn read(&self, id: &SessionId) -> Result<Option<SessionState>, HostError> {
        let chemin = self.chemin(id)?;
        let Some(texte) = files::read_text(&chemin)? else {
            return Ok(None);
        };
        SessionState::from_json(&texte)
            .map(Some)
            .map_err(|erreur| HostError::Format {
                chemin,
                cause: format!("{FORMAT}: {erreur}"),
            })
    }

    /// Écrit `state` en remplaçant le fichier précédent. `rename` fait le renommage final (injectable pour les tests).
    fn save_with<R>(&self, state: &SessionState, rename: R) -> SessionResult<()>
    where
        R: FnOnce(&Path, &Path) -> io::Result<()>,
    {
        let texte = state.to_json()?;
        let chemin = self
            .chemin(state.id())
            .map_err(|erreur| store_error(&erreur))?;
        files::write_atomic_with(&chemin, texte.as_bytes(), MODE_PRIVE, rename)
            .map_err(|erreur| store_error(&erreur))
    }

    /// Identifiants des états présents, le plus récemment modifié d'abord. Un fichier dont la date est illisible est
    /// omis de la liste.
    #[must_use]
    pub fn list_ids(&self) -> Vec<SessionId> {
        self.list_with_dates()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Comme [`FileSessionStore::list_ids`], avec la date de modification de chaque fichier d'état.
    #[must_use]
    pub fn list_with_dates(&self) -> Vec<(SessionId, SystemTime)> {
        let mut entrees: Vec<(SystemTime, SessionId)> = files::list_files(&self.dossier, EXTENSION)
            .into_iter()
            .filter_map(|entree| {
                let chemin = entree.ok()?;
                let id = chemin
                    .file_stem()
                    .and_then(|nom| nom.to_str())
                    .and_then(SessionId::new)?;
                let date = fs::metadata(&chemin)
                    .and_then(|meta| meta.modified())
                    .ok()?;
                Some((date, id))
            })
            .collect();
        entrees.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        entrees.into_iter().map(|(date, id)| (id, date)).collect()
    }

    /// Supprime l'état de `id`. `true` s'il existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] when the identifier is not a file name, and [`HostError::Io`] when the removal
    /// fails.
    pub fn delete(&self, id: &SessionId) -> Result<bool, HostError> {
        let chemin = self.chemin(id)?;
        files::remove_file(&chemin)
    }
}

impl lymaille::SessionStore for FileSessionStore {
    fn load(
        &self,
        id: &SessionId,
    ) -> impl Future<Output = SessionResult<Option<SessionState>>> + Send {
        future::ready(self.read(id).map_err(|erreur| store_error(&erreur)))
    }

    fn save(&self, state: &SessionState) -> impl Future<Output = SessionResult<()>> + Send {
        future::ready(self.save_with(state, |from, to| std::fs::rename(from, to)))
    }
}

/// Erreur de l'hôte rendue à lymaille comme erreur de stockage, sans contenu de session.
fn store_error(erreur: &HostError) -> SessionError {
    SessionError::Store(erreur.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, SystemTime};

    use lymaille::{AgentRef, SessionId, SessionState, Timestamp};

    use super::{FileSessionStore, HostError};
    use crate::host::layers::Layers;
    use lymaille::SessionStore as _;

    fn session(texte: &str) -> SessionId {
        SessionId::new(texte).expect("a non-empty text makes a session identifier")
    }

    /// Un état neuf de contexte `contexte`.
    fn etat(id: &str, contexte: &str) -> SessionState {
        SessionState::new(
            session(id),
            AgentRef::new("main-agent", "sha-1"),
            contexte,
            BTreeMap::new(),
            Timestamp::from_millis(1_000),
        )
    }

    fn layers(home: &std::path::Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    #[tokio::test]
    async fn saved_state_is_read_back_by_a_new_process_store() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let original = etat("s-1", "ctx-a");
        FileSessionStore::new(&layers)
            .save(&original)
            .await
            .expect("saving into a new directory tree must succeed");

        let autre_processus = FileSessionStore::new(&layers);
        let relu = autre_processus
            .load(&session("s-1"))
            .await
            .expect("loading a saved identifier must succeed")
            .expect("the saved state must be found by its identifier");

        assert!(
            relu == original,
            "the state read back must equal the state that was saved"
        );
        let chemin = layers.state().join("sessions").join("s-1.json");
        let mode = fs::metadata(&chemin)
            .expect("the state file must exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "a state file must be private to its owner");
    }

    #[test]
    fn interrupted_write_leaves_the_previous_state_intact() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let precedent = etat("s-1", "ctx-a");
        store
            .save_with(&precedent, |from, to| std::fs::rename(from, to))
            .expect("the first save must succeed");

        let nouvel = etat("s-1", "ctx-b");
        let resultat = store.save_with(&nouvel, |_, _| {
            Err(std::io::Error::other("injected rename failure"))
        });

        assert!(resultat.is_err(), "a failed rename must be reported");
        let relu = store
            .read(&session("s-1"))
            .expect("the state must still be readable")
            .expect("the previous state must still be there");
        assert!(
            relu == precedent,
            "the state read after an interrupted write must be the previous one, intact"
        );
    }

    #[test]
    fn unknown_identifier_is_none_and_corrupt_file_is_a_format_error_with_its_path() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        assert!(
            matches!(store.read(&session("inconnu")), Ok(None)),
            "an unknown identifier must read as None, without error"
        );
        let dossier = layers.state().join("sessions");
        fs::create_dir_all(&dossier)
            .expect("the sessions directory must be created for the fixture");
        fs::write(dossier.join("s-2.json"), "{ ceci n'est pas du json")
            .expect("the corrupt fixture must be written");

        let resultat = store.read(&session("s-2"));

        let casse = matches!(
            &resultat,
            Err(HostError::Format { chemin, .. }) if chemin.ends_with("s-2.json")
        );
        assert!(
            casse,
            "a corrupt state must be a format error carrying the path of its file, got {resultat:?}"
        );
    }

    #[test]
    fn identifier_outside_a_file_name_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);

        assert!(
            matches!(
                store.read(&session("../hors")),
                Err(HostError::Invalid { .. })
            ),
            "reading a state must refuse an identifier that is not a file name"
        );
        assert!(
            matches!(
                store.delete(&session("a/b")),
                Err(HostError::Invalid { .. })
            ),
            "deleting a state must refuse an identifier that is not a file name"
        );
    }

    #[test]
    fn list_is_most_recent_first_and_delete_removes_from_it() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        for (id, secondes) in [("s-1", 1_000_u64), ("s-2", 2_000), ("s-3", 3_000)] {
            store
                .save_with(&etat(id, "ctx-a"), |from, to| std::fs::rename(from, to))
                .expect("the fixture state must be saved");
            let chemin = layers.state().join("sessions").join(format!("{id}.json"));
            fs::File::options()
                .write(true)
                .open(&chemin)
                .expect("the fixture file must be reopened")
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secondes))
                .expect("the modification date must be set");
        }

        let ids: Vec<String> = store
            .list_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        assert_eq!(
            ids,
            ["s-3", "s-2", "s-1"],
            "the most recent session must come first"
        );

        assert!(
            matches!(store.delete(&session("s-2")), Ok(true)),
            "deleting an existing session must report true"
        );
        let apres: Vec<String> = store
            .list_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        assert_eq!(
            apres,
            ["s-3", "s-1"],
            "a deleted session must be gone from the listing"
        );
        assert!(
            matches!(store.delete(&session("s-2")), Ok(false)),
            "deleting an absent session must report false"
        );
    }
}
