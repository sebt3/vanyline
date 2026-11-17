//! Journal d'audit des scripts : une ligne JSONL par script, par session. Contrat : `host/audit.sdd`.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use lymaille::{Event, EventKind, EventSink, ScriptRecord, SessionId, Timestamp};
use serde::{Deserialize, Serialize};

use super::HostError;
use super::files;
use super::layers::Layers;

/// Sous-répertoire du journal, sous l'état local.
const AUDIT_DIR: &str = "audit";
/// Droits du journal : privé à l'utilisateur.
const MODE_PRIVE: u32 = 0o600;
/// Marqueur d'un identifiant de session qui n'est pas un nom de fichier.
const INVALID: &str = "VNL-CLI-HST-011";
/// Marqueur d'un journal inaccessible en écriture ou en lecture.
const IO: &str = "VNL-CLI-HST-012";
/// Marqueur d'une entrée qui ne se sérialise pas.
const ENCODE: &str = "VNL-CLI-HST-013";
/// Marqueur d'une ligne complète du journal qui n'est pas une entrée.
const FORMAT: &str = "VNL-CLI-HST-014";

/// Une entrée du journal : le tour de la session au moment du script, et l'enregistrement de lymaille.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Tour de la session au moment où le script a tourné.
    pub turn: u64,
    /// Enregistrement du script, masqué par lymaille.
    pub record: ScriptRecord,
}

/// Journal d'audit sous `<état>/audit/`.
#[derive(Debug, Clone)]
pub struct AuditLog {
    dossier: PathBuf,
}

impl AuditLog {
    /// Journal rangé sous l'état local de `layers`. Ne crée rien.
    #[must_use]
    pub fn new(layers: &Layers) -> Self {
        Self {
            dossier: layers.state().join(AUDIT_DIR),
        }
    }

    /// Ajoute `entree` en fin du journal de `session`.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] when the identifier is not a file name, and [`HostError::Io`] or
    /// [`HostError::Format`] when the entry cannot be appended.
    pub fn record(&self, session: &SessionId, entree: &AuditEntry) -> Result<(), HostError> {
        let chemin = self.chemin(session)?;
        ajouter(&chemin, entree)
    }

    /// Ajoute un script sans session dans `script-<horodatage>.jsonl`.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Io`] or [`HostError::Format`] when the entry cannot be appended.
    pub fn record_standalone(&self, entree: &AuditEntry, at: Timestamp) -> Result<(), HostError> {
        let chemin = self
            .dossier
            .join(format!("script-{}.jsonl", at.as_millis()));
        ajouter(&chemin, entree)
    }

    /// Relit les entrées de `id`, dans l'ordre d'écriture. Une dernière ligne sans fin de ligne (arrêt brutal) est
    /// ignorée ; un journal absent rend une liste vide.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Format`] when a complete line is not an entry, [`HostError::Invalid`] when the identifier is
    /// not a file name, and [`HostError::Io`] when the journal cannot be read.
    pub fn for_session(&self, id: &SessionId) -> Result<Vec<AuditEntry>, HostError> {
        let chemin = self.chemin(id)?;
        let Some(texte) = files::read_text(&chemin)? else {
            return Ok(Vec::new());
        };
        let mut entrees = Vec::new();
        for segment in texte.split_inclusive('\n') {
            // Un segment sans fin de ligne est la queue d'un arrêt brutal : il est ignoré.
            let Some(ligne) = segment.strip_suffix('\n') else {
                break;
            };
            if ligne.trim().is_empty() {
                continue;
            }
            let entree =
                serde_json::from_str::<AuditEntry>(ligne).map_err(|erreur| HostError::Format {
                    chemin: chemin.clone(),
                    cause: format!(
                        "{FORMAT}: a complete journal line is not an audit entry: {erreur}"
                    ),
                })?;
            entrees.push(entree);
        }
        Ok(entrees)
    }

    /// Chemin du journal de `session`. Refuse un identifiant qui ne serait pas un nom de fichier.
    /// Supprime le journal de `session` ; `false` s'il n'existait pas (`sessions_cmd.sdd`, `delete`).
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] when the identifier is not a file name, and [`HostError::Io`] when the removal
    /// fails.
    pub fn delete(&self, session: &SessionId) -> Result<bool, HostError> {
        files::remove_file(&self.chemin(session)?)
    }

    fn chemin(&self, session: &SessionId) -> Result<PathBuf, HostError> {
        if !files::est_un_nom_de_fichier(session.as_str()) {
            return Err(HostError::Invalid {
                cause: format!("{INVALID}: a session identifier must be a single file name"),
            });
        }
        Ok(self.dossier.join(format!("{}.jsonl", session.as_str())))
    }
}

/// Ajoute une ligne JSON de `entree` en fin de `chemin`, en le créant au besoin.
fn ajouter(chemin: &Path, entree: &AuditEntry) -> Result<(), HostError> {
    let io_error = |erreur: std::io::Error| HostError::Io {
        chemin: chemin.to_path_buf(),
        cause: format!("{IO}: {erreur}"),
    };
    if let Some(dossier) = chemin.parent() {
        fs::create_dir_all(dossier).map_err(io_error)?;
    }
    let mut ligne = serde_json::to_string(entree).map_err(|erreur| HostError::Format {
        chemin: chemin.to_path_buf(),
        cause: format!("{ENCODE}: {erreur}"),
    })?;
    ligne.push('\n');
    let mut fichier = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(MODE_PRIVE)
        .open(chemin)
        .map_err(io_error)?;
    fichier.write_all(ligne.as_bytes()).map_err(io_error)
}

/// Puits d'événements qui écrit les scripts dans le journal, en suivant le tour de chaque session, et transmet les
/// autres événements à `suivant`. Un échec d'écriture ne peut pas remonter à la bibliothèque : il est retenu et se
/// récupère par [`AuditSink::take_failures`].
#[derive(Debug)]
pub struct AuditSink<N: EventSink> {
    journal: AuditLog,
    suivant: N,
    tours: Mutex<BTreeMap<SessionId, u64>>,
    echecs: Mutex<Vec<HostError>>,
}

impl<N: EventSink> AuditSink<N> {
    /// Puits qui écrit dans `journal` (qu'il possède) et transmet le reste à `suivant`.
    #[must_use]
    pub const fn new(journal: AuditLog, suivant: N) -> Self {
        Self {
            journal,
            suivant,
            tours: Mutex::new(BTreeMap::new()),
            echecs: Mutex::new(Vec::new()),
        }
    }

    /// Rend et vide les échecs d'écriture survenus depuis le dernier appel.
    #[must_use]
    pub fn take_failures(&self) -> Vec<HostError> {
        let mut guard = self.echecs.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *guard)
    }
}

impl<N: EventSink> EventSink for AuditSink<N> {
    fn emit(&self, event: Event) {
        if let EventKind::TurnStarted { turn } = &event.kind {
            let numero = turn.get();
            self.tours
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(event.session.clone(), numero);
            self.suivant.emit(event);
            return;
        }
        let EventKind::Script(record) = &event.kind else {
            self.suivant.emit(event);
            return;
        };
        let tour = self
            .tours
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&event.session)
            .copied()
            .unwrap_or(0);
        let entree = AuditEntry {
            turn: tour,
            record: record.clone(),
        };
        if let Err(erreur) = self.journal.record(&event.session, &entree) {
            self.echecs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(erreur);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use lymaille::{
        Event, EventKind, EventSink, RecordingSink, Redactor, ScriptInput, ScriptRecord, SessionId,
        Timestamp, TurnId,
    };

    use super::{AuditEntry, AuditLog, AuditSink};
    use crate::host::layers::Layers;

    fn session(texte: &str) -> SessionId {
        SessionId::new(texte).expect("a non-empty text makes a session identifier")
    }

    fn enregistrement(script: &str, resultat: &str, redactor: &Redactor) -> ScriptRecord {
        ScriptRecord::new(
            redactor,
            1_000,
            ScriptInput::new("main-agent", script, resultat, Duration::from_millis(5)),
        )
    }

    fn entree(tour: u64, script: &str, resultat: &str, redactor: &Redactor) -> AuditEntry {
        AuditEntry {
            turn: tour,
            record: enregistrement(script, resultat, redactor),
        }
    }

    fn journal(home: &std::path::Path) -> (Layers, AuditLog) {
        let layers = Layers::discover(home, home, None, |_| false);
        let log = AuditLog::new(&layers);
        (layers, log)
    }

    fn tour_commence(session_id: &str, tour: u64) -> Event {
        Event::new(
            session(session_id),
            Timestamp::from_millis(tour),
            EventKind::TurnStarted {
                turn: TurnId::new(tour),
            },
        )
    }

    #[test]
    fn one_record_per_script_in_order_with_its_turn_and_the_full_text() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (_layers, log) = journal(home.path());
        let redactor = Redactor::new();
        let long_script = format!("let x = \"{}\";", "a".repeat(400));
        let premier = entree(1, "let x = 1;", "1", &redactor);
        let second = entree(2, &long_script, "ok", &redactor);
        let puits = AuditSink::new(log.clone(), RecordingSink::new());

        puits.emit(tour_commence("s-1", 1));
        puits.emit(Event::new(
            session("s-1"),
            Timestamp::from_millis(2),
            EventKind::Script(premier.record.clone()),
        ));
        puits.emit(Event::new(
            session("s-1"),
            Timestamp::from_millis(3),
            EventKind::TodoChanged,
        ));
        puits.emit(tour_commence("s-1", 2));
        puits.emit(Event::new(
            session("s-1"),
            Timestamp::from_millis(4),
            EventKind::Script(second.record.clone()),
        ));

        let relues = log
            .for_session(&session("s-1"))
            .expect("the journal must be readable");
        assert_eq!(
            relues,
            vec![premier, second],
            "the two entries must be read back in order, with their turn"
        );
        assert_eq!(
            relues.get(1).map(|entree| entree.record.script.len()),
            Some(long_script.len()),
            "the complete script text must be kept, never truncated"
        );
    }

    #[test]
    fn the_sink_is_shareable_as_a_static_event_sink() {
        // lymaille impose `Arc<dyn EventSink>` (`'static`) : le puits possède son journal (`audit.sdd`, `@AuditSink`).
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (_layers, log) = journal(home.path());
        let redactor = Redactor::new();
        let puits: std::sync::Arc<dyn EventSink> =
            std::sync::Arc::new(AuditSink::new(log.clone(), RecordingSink::new()));
        puits.emit(tour_commence("s-1", 1));
        puits.emit(Event::new(
            session("s-1"),
            Timestamp::from_millis(2),
            EventKind::Script(enregistrement("let x = 1;", "1", &redactor)),
        ));
        let lues = log
            .for_session(&session("s-1"))
            .expect("the journal must be readable");
        assert_eq!(lues, vec![entree(1, "let x = 1;", "1", &redactor)]);
    }

    #[test]
    fn events_other_than_scripts_go_to_the_next_sink() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (_layers, log) = journal(home.path());
        let puits = AuditSink::new(log.clone(), RecordingSink::new());

        puits.emit(Event::new(
            session("s-1"),
            Timestamp::from_millis(1),
            EventKind::TodoChanged,
        ));

        let relues = log
            .for_session(&session("s-1"))
            .expect("the journal must be readable");
        assert!(
            relues.is_empty(),
            "a todo event is not a script and must not be audited"
        );
    }

    #[test]
    fn truncated_last_line_is_ignored() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, log) = journal(home.path());
        let redactor = Redactor::new();
        let un =
            serde_json::to_string(&entree(1, "a", "1", &redactor)).expect("the entry must encode");
        let deux =
            serde_json::to_string(&entree(2, "b", "2", &redactor)).expect("the entry must encode");
        let dossier = layers.state().join("audit");
        fs::create_dir_all(&dossier).expect("the audit directory must be created for the fixture");
        fs::write(
            dossier.join("s-1.jsonl"),
            format!("{un}\n{deux}\n{{\"turn\":3,\"rec"),
        )
        .expect("the journal fixture must be written");

        let relues = log.for_session(&session("s-1"));

        assert!(
            matches!(&relues, Ok(entries) if entries.len() == 2),
            "the two complete entries must be read, the truncated tail ignored, got {relues:?}"
        );
    }

    #[test]
    fn no_secret_is_written_to_the_journal() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, log) = journal(home.path());
        let mut redactor = Redactor::new();
        redactor
            .add_secret("sk-SECRET-VALUE-123")
            .expect("a six characters secret must be accepted");
        let record = entree(1, "print(1)", "resultat sk-SECRET-VALUE-123 fin", &redactor);

        log.record(&session("s-1"), &record)
            .expect("the entry must be appended");

        let contenu = fs::read_to_string(layers.state().join("audit").join("s-1.jsonl"))
            .expect("the journal file must exist");
        assert!(
            !contenu.contains("sk-SECRET-VALUE-123"),
            "the journal must never hold the value of a secret"
        );
        assert!(
            contenu.contains("resultat"),
            "the rest of the result must still be recorded"
        );
    }

    #[test]
    fn standalone_script_goes_to_a_timestamped_file() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, log) = journal(home.path());
        let record = entree(0, "print(2)", "2", &Redactor::new());

        log.record_standalone(&record, Timestamp::from_millis(42))
            .expect("the standalone entry must be appended");

        let relues = fs::read_to_string(layers.state().join("audit").join("script-42.jsonl"))
            .expect("the timestamped journal must exist");
        assert_eq!(
            relues.lines().count(),
            1,
            "one standalone script is one line"
        );
    }

    #[test]
    fn unwritable_audit_directory_failure_is_not_silent() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (layers, log) = journal(home.path());
        fs::create_dir_all(layers.state())
            .expect("the state directory must be created for the fixture");
        fs::write(layers.state().join("audit"), "pas un répertoire")
            .expect("a blocking file must be written");
        let puits = AuditSink::new(log.clone(), RecordingSink::new());

        puits.emit(tour_commence("s-1", 1));
        puits.emit(Event::new(
            session("s-1"),
            Timestamp::from_millis(2),
            EventKind::Script(enregistrement("x", "y", &Redactor::new())),
        ));

        assert_eq!(
            puits.take_failures().len(),
            1,
            "a failed append must be kept for the caller, not lost"
        );
    }

    #[test]
    fn identifier_outside_a_file_name_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let (_layers, log) = journal(home.path());
        let record = entree(1, "x", "y", &Redactor::new());

        let refusee = log.record(&session("../hors"), &record);

        assert!(
            matches!(refusee, Err(super::HostError::Invalid { .. })),
            "a journal entry must never be written outside the audit directory"
        );
    }
}
