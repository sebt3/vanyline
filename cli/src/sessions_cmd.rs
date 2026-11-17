//! Commandes `vanyline sessions list|delete` — contrat complet : `sessions_cmd.sdd`.
//!
//! Listage et suppression des sessions de l'hôte (lymaille laisse ces opérations
//! à l'hôte). Aucun appel au cluster, aucune session lymaille ouverte.

use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Utc};
use lymaille::{Redactor, SessionId};
use serde_json::json;

use crate::exit_code::Outcome;
use crate::host::HostError;
use crate::host::audit::AuditLog;
use crate::host::layers::Layers;
use crate::host::process_lock::SessionLock;
use crate::host::session_index::{FileSessionIndex, SessionEntry};
use crate::host::session_store::FileSessionStore;

/// Sous-commandes de `sessions` (grammaire : `main.sdd`).
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Commands {
    /// Liste les sessions, la plus récente d'abord
    List {
        /// Ne lister que les sessions de ce projet
        #[arg(long)]
        context: Option<String>,
        /// Sortie JSON (tableau d'objets)
        #[arg(long)]
        json: bool,
    },
    /// Supprime une session libre (refusée si un autre processus l'ouvre)
    Delete {
        /// Identifiant de la session
        id: String,
    },
}

/// Une ligne de listage : la session et le projet de son état.
struct Ligne {
    entree: SessionEntry,
    contexte: String,
}

/// Exécute la sous-commande `sessions` et rend son issue.
pub fn run_sessions(commande: Commands, repo: Option<&Path>) -> Outcome {
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let layers = Layers::discover(&home, &cwd, repo, Path::exists);
    match commande {
        Commands::List { context, json } => lister(&layers, context.as_deref(), json),
        Commands::Delete { id } => supprimer(&layers, &id),
    }
}

/// `sessions list` : sessions du contexte (ou de tous), la plus récente d'abord.
fn lister(layers: &Layers, contexte: Option<&str>, json: bool) -> Outcome {
    let store = FileSessionStore::new(layers);
    let mut lignes: Vec<Ligne> = Vec::new();
    if let Some(ctx) = contexte {
        let audit = AuditLog::new(layers);
        for resultat in FileSessionIndex::new(&store, &audit).sessions_for_context(ctx) {
            match resultat {
                Ok(entree) => lignes.push(Ligne {
                    entree,
                    contexte: ctx.to_owned(),
                }),
                Err(erreur) => signaler(&erreur),
            }
        }
    } else {
        for (id, modifie) in store.list_with_dates() {
            match store.read(&id) {
                Ok(Some(etat)) => lignes.push(Ligne {
                    entree: SessionEntry {
                        id,
                        title: etat.display_title(&Redactor::new()),
                        turns: u64::try_from(etat.signals().len()).unwrap_or(u64::MAX),
                        agent: etat.agent().name().to_owned(),
                        modified: modifie,
                    },
                    contexte: etat.toolset_context().to_owned(),
                }),
                Ok(None) => {}
                Err(erreur) => signaler(&erreur),
            }
        }
        lignes.sort_by_key(|ligne| std::cmp::Reverse(ligne.entree.modified));
    }
    let mut sortie = std::io::stdout();
    if json {
        let tableau: Vec<_> = lignes.iter().map(objet_json).collect();
        let _ = writeln!(sortie, "{}", serde_json::Value::Array(tableau));
    } else {
        for ligne in &lignes {
            let _ = writeln!(sortie, "{}", texte_de(ligne));
        }
    }
    Outcome::Success
}

/// `sessions delete <id>` : refus si la session est ouverte ailleurs ; sinon état,
/// journal et verrou sont supprimés.
fn supprimer(layers: &Layers, texte: &str) -> Outcome {
    let Some(id) = SessionId::new(texte) else {
        ecrire_erreur("VNL-CLI-SES-005: identifiant de session vide");
        return Outcome::Failure;
    };
    let store = FileSessionStore::new(layers);
    match store.read(&id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            ecrire_erreur(&format!("VNL-CLI-SES-001: session inconnue : {texte}"));
            return Outcome::Failure;
        }
        Err(erreur) => {
            signaler(&erreur);
            return Outcome::Failure;
        }
    }
    let verrou = match SessionLock::acquire(layers, &id) {
        Ok(verrou) => verrou,
        Err(erreur) => {
            signaler(&erreur);
            return Outcome::Failure;
        }
    };
    let echec = store
        .delete(&id)
        .and_then(|_| AuditLog::new(layers).delete(&id));
    if let Err(erreur) = echec {
        signaler(&erreur);
        return Outcome::Failure;
    }
    drop(verrou);
    let chemin_verrou = layers.state().join("locks").join(format!("{texte}.lock"));
    let _ = crate::host::files::remove_file(&chemin_verrou);
    Outcome::Success
}

/// Ligne texte d'une session : identifiant, date, projet, tours, agent, titre.
fn texte_de(ligne: &Ligne) -> String {
    let date: DateTime<Utc> = ligne.entree.modified.into();
    format!(
        "{}  {}  {}  {} tours  {}  {}",
        ligne.entree.id.as_str(),
        date.format("%Y-%m-%d %H:%M"),
        ligne.contexte,
        ligne.entree.turns,
        ligne.entree.agent,
        ligne.entree.title
    )
}

/// Objet JSON d'une session.
fn objet_json(ligne: &Ligne) -> serde_json::Value {
    let date: DateTime<Utc> = ligne.entree.modified.into();
    json!({
        "id": ligne.entree.id.as_str(),
        "title": ligne.entree.title,
        "context": ligne.contexte,
        "turns": ligne.entree.turns,
        "agent": ligne.entree.agent,
        "modified": date.to_rfc3339(),
    })
}

/// Signale une erreur de lecture sur la sortie d'erreur, sans interrompre le listage.
fn signaler(erreur: &HostError) {
    ecrire_erreur(&erreur.to_string());
}

/// Écrit sur la sortie d'erreur (jamais `eprintln!`).
fn ecrire_erreur(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use lymaille::{AgentRef, SessionState, Timestamp};

    fn etat(id: &str, contexte: &str) -> SessionState {
        SessionState::new(
            SessionId::new(id).expect("identifiant non vide"),
            AgentRef::new("main-agent", "sha-1"),
            contexte,
            BTreeMap::new(),
            Timestamp::from_millis(1_000),
        )
    }

    fn layers(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    async fn enregistre(store: &FileSessionStore, id: &str, contexte: &str) {
        lymaille::SessionStore::save(store, &etat(id, contexte))
            .await
            .expect("l'état de test est enregistré");
    }

    /// Scenario: `liste d'un contexte` — seules les sessions du contexte demandé.
    #[tokio::test]
    async fn liste_filtre_par_contexte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let l = layers(dir.path());
        let store = FileSessionStore::new(&l);
        enregistre(&store, "s-a1", "A").await;
        enregistre(&store, "s-a2", "A").await;
        enregistre(&store, "s-b1", "B").await;
        let audit = AuditLog::new(&l);
        let de_a: Vec<String> = FileSessionIndex::new(&store, &audit)
            .sessions_for_context("A")
            .into_iter()
            .filter_map(Result::ok)
            .map(|e| e.id.as_str().to_owned())
            .collect();
        assert_eq!(de_a.len(), 2, "deux sessions de A: {de_a:?}");
        assert!(de_a.iter().all(|id| id.starts_with("s-a")));
        assert_eq!(lister(&l, Some("A"), false), Outcome::Success);
    }

    /// Scenario: `suppression` — état et journal supprimés, le verrou libéré.
    #[tokio::test]
    async fn suppression_libre() {
        let dir = tempfile::tempdir().expect("tempdir");
        let l = layers(dir.path());
        let store = FileSessionStore::new(&l);
        enregistre(&store, "s-1", "A").await;
        assert_eq!(supprimer(&l, "s-1"), Outcome::Success);
        assert!(
            store
                .read(&SessionId::new("s-1").expect("id"))
                .expect("lecture")
                .is_none()
        );
    }

    /// Scenario: `suppression d'une session ouverte` — refus, session intacte.
    #[tokio::test]
    async fn suppression_refusee_si_ouverte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let l = layers(dir.path());
        let store = FileSessionStore::new(&l);
        enregistre(&store, "s-2", "A").await;
        let id = SessionId::new("s-2").expect("id");
        let tenu = SessionLock::acquire(&l, &id).expect("verrou de test");
        assert_eq!(supprimer(&l, "s-2"), Outcome::Failure, "refus si ouverte");
        assert!(
            store.read(&id).expect("lecture").is_some(),
            "session intacte"
        );
        drop(tenu);
    }

    /// Identifiant inconnu : code 1, « session inconnue ».
    #[test]
    fn suppression_d_une_session_inconnue() {
        let dir = tempfile::tempdir().expect("tempdir");
        let l = layers(dir.path());
        assert_eq!(supprimer(&l, "absente"), Outcome::Failure);
    }
}
