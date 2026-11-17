//! `SessionIndex` de lymaille et requêtes de reprise, par relecture des fichiers d'état. Contrat :
//! `host/session_index.sdd`.

use std::future::{self, Future};
use std::time::SystemTime;

use lymaille::{
    Redactor, ReviewState, ScriptError, ScriptErrorClass, SessionError, SessionFilter, SessionId,
    SessionResult, SessionState, SessionSummary, TurnId,
};

use super::HostError;
use super::audit::AuditLog;
use super::session_store::FileSessionStore;

/// Une session listée : identifiant, titre, nombre de tours, agent et date de modification de son fichier d'état.
/// Le `SessionSummary` de lymaille ne porte pas de date : elle vient du fichier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// Identifiant de la session.
    pub id: SessionId,
    /// Titre de la session, masqué.
    pub title: String,
    /// Nombre de tours.
    pub turns: u64,
    /// Nom de l'agent de la session.
    pub agent: String,
    /// Date de modification du fichier d'état.
    pub modified: SystemTime,
}

/// Construit une entrée de listage depuis l'état et la date de son fichier.
fn entree(state: &SessionState, id: SessionId, modified: SystemTime) -> SessionEntry {
    let resume = SessionSummary::from_state(state, &Redactor::new());
    SessionEntry {
        id,
        title: resume.title().to_owned(),
        turns: resume.turns(),
        agent: resume.agent().to_owned(),
        modified,
    }
}

/// Index des sessions, adossé au stockage de sessions et au journal d'audit.
#[derive(Debug, Clone, Copy)]
pub struct FileSessionIndex<'a> {
    store: &'a FileSessionStore,
    audit: &'a AuditLog,
}

impl<'a> FileSessionIndex<'a> {
    /// Index sur `store` et `audit`. Ne lit ni n'écrit rien à la construction.
    #[must_use]
    pub const fn new(store: &'a FileSessionStore, audit: &'a AuditLog) -> Self {
        Self { store, audit }
    }

    /// Dernière session (la plus récemment modifiée) du contexte `contexte`.
    #[must_use]
    pub fn latest_for_context(&self, contexte: &str) -> Option<SessionId> {
        self.sessions_for_context(contexte)
            .into_iter()
            .find_map(|entree| entree.ok().map(|entree| entree.id))
    }

    /// Sessions du contexte `contexte`, la plus récente d'abord. Un fichier d'état illisible apparaît comme une entrée
    /// `Err` dans le résultat, sans interrompre le listage.
    #[must_use]
    pub fn sessions_for_context(&self, contexte: &str) -> Vec<Result<SessionEntry, HostError>> {
        self.store
            .list_with_dates()
            .into_iter()
            .filter_map(|(id, modified)| match self.store.read(&id) {
                Ok(Some(state)) if state.toolset_context() == contexte => {
                    Some(Ok(entree(&state, id, modified)))
                }
                Ok(Some(_) | None) => None,
                Err(erreur) => Some(Err(erreur)),
            })
            .collect()
    }

    /// Listage filtré pour `lymaille::SessionIndex::list`, plus récent d'abord. Les fichiers illisibles sont omis.
    fn list_filtered(&self, filter: &SessionFilter) -> Vec<SessionId> {
        let redacteur = Redactor::new();
        let mut ids = Vec::new();
        for (id, _) in self.store.list_with_dates() {
            if filter.limit().is_some_and(|limite| ids.len() >= limite) {
                break;
            }
            let Ok(Some(state)) = self.store.read(&id) else {
                continue;
            };
            let resume = SessionSummary::from_state(&state, &redacteur);
            if filter.agent().is_some_and(|agent| agent != resume.agent()) {
                continue;
            }
            if filter.kind().is_some_and(|nature| nature != resume.kind()) {
                continue;
            }
            if filter.review_requested()
                && !matches!(resume.review(), Some(ReviewState::Requested(_)))
            {
                continue;
            }
            ids.push(id);
        }
        ids
    }

    /// Erreurs de script d'une session, lues dans le journal : une par entrée qui porte une erreur.
    fn errors_of(&self, id: &SessionId) -> SessionResult<Vec<ScriptError>> {
        let entrees = self
            .audit
            .for_session(id)
            .map_err(|erreur| SessionError::Store(erreur.to_string()))?;
        Ok(entrees
            .into_iter()
            .filter_map(|entree| {
                let texte = entree.record.error?;
                Some(ScriptError::new(
                    TurnId::new(entree.turn),
                    ScriptErrorClass::Runtime,
                    texte,
                ))
            })
            .collect())
    }
}

impl lymaille::SessionIndex for FileSessionIndex<'_> {
    fn list(
        &self,
        filter: &SessionFilter,
    ) -> impl Future<Output = SessionResult<Vec<SessionId>>> + Send {
        future::ready(Ok(self.list_filtered(filter)))
    }

    fn script_errors(
        &self,
        id: &SessionId,
    ) -> impl Future<Output = SessionResult<Vec<ScriptError>>> + Send {
        future::ready(self.errors_of(id))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{Duration, SystemTime};

    use lymaille::{
        AgentRef, Redactor, ScriptErrorClass, ScriptInput, ScriptRecord, SessionFilter, SessionId,
        SessionIndex as _, SessionState, SessionStore as _, Timestamp, TurnId,
    };

    use super::{FileSessionIndex, SessionEntry};
    use crate::host::audit::{AuditEntry, AuditLog};
    use crate::host::layers::Layers;
    use crate::host::session_store::FileSessionStore;

    fn session(texte: &str) -> SessionId {
        SessionId::new(texte).expect("a non-empty text makes a session identifier")
    }

    /// Un état neuf de l'agent `main-agent` dans le contexte `contexte`.
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

    /// Enregistre un état de session, daté de `secondes` depuis l'époque.
    async fn enregistre(
        store: &FileSessionStore,
        layers: &Layers,
        id: &str,
        contexte: &str,
        secondes: u64,
    ) {
        store
            .save(&etat(id, contexte))
            .await
            .expect("the fixture state must be saved");
        fs::File::options()
            .write(true)
            .open(layers.state().join("sessions").join(format!("{id}.json")))
            .expect("the fixture file must be reopened")
            .set_modified(date(secondes))
            .expect("the modification date must be set");
    }

    /// Date de `secondes` après l'époque.
    fn date(secondes: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(secondes))
            .expect("a fixture date must be representable")
    }

    fn ids(entrees: &[Result<SessionEntry, crate::host::HostError>]) -> Vec<String> {
        entrees
            .iter()
            .filter_map(|entree| entree.as_ref().ok())
            .map(|entree| entree.id.as_str().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn latest_session_of_a_context_is_the_most_recent_one_of_that_context() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let audit = AuditLog::new(&layers);
        enregistre(&store, &layers, "a-ancienne", "A", 1_000).await;
        enregistre(&store, &layers, "a-recente", "A", 3_000).await;
        enregistre(&store, &layers, "b-seule", "B", 5_000).await;
        let index = FileSessionIndex::new(&store, &audit);

        assert_eq!(
            index
                .latest_for_context("A")
                .map(|id| id.as_str().to_owned()),
            Some("a-recente".to_owned()),
            "the latest session of context A must be a-recente, not the more recent one of context B"
        );
        assert_eq!(
            index
                .latest_for_context("B")
                .map(|id| id.as_str().to_owned()),
            Some("b-seule".to_owned())
        );
        assert_eq!(
            index.latest_for_context("inconnu"),
            None,
            "an unknown context has no latest session"
        );
    }

    #[tokio::test]
    async fn deleted_session_is_absent_from_the_listing() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let audit = AuditLog::new(&layers);
        enregistre(&store, &layers, "s-1", "A", 1_000).await;
        enregistre(&store, &layers, "s-2", "A", 2_000).await;
        store
            .delete(&session("s-1"))
            .expect("the deletion must succeed");
        let index = FileSessionIndex::new(&store, &audit);

        let listees = index.sessions_for_context("A");

        assert_eq!(
            ids(&listees),
            ["s-2"],
            "a deleted session must no longer be listed"
        );
    }

    #[tokio::test]
    async fn listing_carries_title_turns_agent_and_date_most_recent_first() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let audit = AuditLog::new(&layers);
        enregistre(&store, &layers, "s-1", "A", 1_000).await;
        enregistre(&store, &layers, "s-2", "A", 2_000).await;
        let index = FileSessionIndex::new(&store, &audit);

        let listees = index.sessions_for_context("A");

        let premiere = listees
            .first()
            .and_then(|entree| entree.as_ref().ok())
            .expect("the most recent session must be listed first and readable");
        assert_eq!(
            premiere.id.as_str(),
            "s-2",
            "the most recent session must come first"
        );
        assert_eq!(
            premiere.agent, "main-agent",
            "the agent of the session must be listed"
        );
        assert_eq!(premiere.turns, 0, "a new session has no turn");
        assert_eq!(
            premiere.modified,
            date(2_000),
            "the date must be the modification date of the state file"
        );
    }

    #[tokio::test]
    async fn unreadable_state_is_an_error_entry_and_does_not_stop_the_listing() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let audit = AuditLog::new(&layers);
        enregistre(&store, &layers, "s-1", "A", 1_000).await;
        fs::write(
            layers.state().join("sessions").join("s-casse.json"),
            "{ pas du json",
        )
        .expect("the corrupt fixture must be written");
        let index = FileSessionIndex::new(&store, &audit);

        let listees = index.sessions_for_context("A");

        assert_eq!(
            ids(&listees),
            ["s-1"],
            "the readable session must still be listed"
        );
        assert!(
            listees
                .iter()
                .any(|entree| matches!(entree, Err(crate::host::HostError::Format { .. }))),
            "the unreadable state must appear as an error entry in the result"
        );
    }

    #[tokio::test]
    async fn trait_listing_applies_the_filter_limit_and_recency() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let audit = AuditLog::new(&layers);
        enregistre(&store, &layers, "s-1", "A", 1_000).await;
        enregistre(&store, &layers, "s-2", "A", 2_000).await;
        let index = FileSessionIndex::new(&store, &audit);

        let limitee = index
            .list(&SessionFilter::new().with_agent("main-agent").with_limit(1))
            .await
            .expect("the listing must succeed");

        assert_eq!(
            limitee.iter().map(SessionId::as_str).collect::<Vec<_>>(),
            ["s-2"],
            "the filter must keep the most recent session only"
        );
    }

    #[tokio::test]
    async fn script_errors_come_from_the_journal_with_their_turn() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = layers(home.path());
        let store = FileSessionStore::new(&layers);
        let audit = AuditLog::new(&layers);
        let redactor = Redactor::new();
        let script = |resultat: &str, erreur: Option<&str>| {
            let entree = match erreur {
                Some(texte) => {
                    ScriptInput::new("main-agent", "f()", resultat, Duration::from_millis(3))
                        .with_error(texte)
                }
                None => ScriptInput::new("main-agent", "f()", resultat, Duration::from_millis(3)),
            };
            ScriptRecord::new(&redactor, 1_000, entree)
        };
        audit
            .record(
                &session("s-1"),
                &AuditEntry {
                    turn: 1,
                    record: script("ok", None),
                },
            )
            .expect("the first entry must be appended");
        audit
            .record(
                &session("s-1"),
                &AuditEntry {
                    turn: 2,
                    record: script("", Some("boom")),
                },
            )
            .expect("the failing entry must be appended");
        audit
            .record(
                &session("s-1"),
                &AuditEntry {
                    turn: 3,
                    record: script("fin", None),
                },
            )
            .expect("the last entry must be appended");
        let index = FileSessionIndex::new(&store, &audit);

        let erreurs = index
            .script_errors(&session("s-1"))
            .await
            .expect("the errors must be read from the journal");

        assert_eq!(
            erreurs.len(),
            1,
            "only the entry with an error must be reported"
        );
        let erreur = erreurs.first().expect("one error is expected");
        assert_eq!(
            erreur.turn(),
            TurnId::new(2),
            "the error must carry the turn of its script"
        );
        assert_eq!(
            erreur.class(),
            ScriptErrorClass::Runtime,
            "the class is Runtime, the only one lymaille keeps"
        );
        assert_eq!(
            erreur.text(),
            "boom",
            "the error must carry the text of the error"
        );
        let absentes = index
            .script_errors(&session("sans-journal"))
            .await
            .expect("a missing journal must not be an error");
        assert!(
            absentes.is_empty(),
            "a session without a journal has no script error"
        );
    }
}
