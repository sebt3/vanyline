//! Commande `vanyline chat` — contrat complet : `chat.sdd`.
//!
//! Vérifie le terminal AVANT toute ouverture, ouvre la session par
//! `session_open`, branche le pilotage du tour (`TurnDriver`) sur le
//! `TurnRunner` de lymaille et lance le TUI. Aucun mode REPL, aucun sélecteur
//! d'agent : l'agent découle du contexte.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use lymaille::{
    CancelHandle, DefaultStrategy, Message, RigProvider, SessionId, TodoList, TurnRunner, UserInbox,
};
use tokio::sync::Mutex;

use crate::exit_code::Outcome as ExitOutcome;
use crate::host::audit::{AuditLog, AuditSink};
use crate::host::context_derive::DerivedContext;
use crate::host::layers::Layers;
use crate::host::process_lock::SessionLock;
use crate::host::session_index::{FileSessionIndex, SessionEntry};
use crate::host::session_store::FileSessionStore;
use crate::session_open::{
    self, KubeEnv, OpenEnv, OpenError, OpenedSession, RefusingSummarizer, Resume, SessionRequest,
};
use crate::tui::sink::TuiSink;
use crate::tui::state::{TodoItem, TranscriptItem};
use crate::tui::{
    self, DirtyFiles, Opened, SessionHost, TuiExit, TuiOptions, TurnDriver, TurnStart,
};

/// Arguments de `vanyline chat` (grammaire : `main.sdd`).
#[derive(Debug, Clone)]
pub struct ChatArgs {
    /// Projet du contexte de travail.
    pub context: String,
    /// Sandbox de travail (`--sandbox`).
    pub sandbox: Option<String>,
    /// `-c/--continue` : dernière session du contexte.
    pub continue_: bool,
    /// `--resume <id>` : session précise.
    pub resume: Option<String>,
}

/// Options globales utiles à `chat` (`main.sdd` : `--owner`, `--repo`, `-n`).
#[derive(Debug, Clone, Default)]
pub struct GlobalOptions {
    /// Owner K8s (`--owner`).
    pub owner: Option<String>,
    /// Dépôt de configuration partagé (`--repo`).
    pub repo: Option<std::path::PathBuf>,
    /// Namespace d'application (`-n`).
    pub namespace: Option<String>,
}

/// Vrai quand l'entrée ET la sortie standard sont des terminaux (`chat#FR-1`).
#[must_use]
pub const fn terminal_requis(entree: bool, sortie: bool) -> bool {
    entree && sortie
}

/// Mode de reprise demandé : `-c` ⇒ dernière, `--resume` ⇒ identifiée, sinon neuve.
///
/// # Errors
///
/// Returns a message when the identifier given to `--resume` is empty.
pub fn choix_de_reprise(args: &ChatArgs) -> Result<Resume, String> {
    match (&args.resume, args.continue_) {
        (Some(id), _) => SessionId::new(id.as_str())
            .map(Resume::Id)
            .ok_or_else(|| "VNL-CLI-CHT-003: identifiant de session vide".to_string()),
        (None, true) => Ok(Resume::Last),
        (None, false) => Ok(Resume::New),
    }
}

/// Historique affiché à la reprise : messages utilisateur et textes de l'assistant.
#[must_use]
pub fn elements_de_l_historique(messages: &[Message]) -> Vec<TranscriptItem> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(TranscriptItem::User {
                text: content.clone(),
                during_turn: false,
            }),
            Message::Assistant { content, .. } if !content.is_empty() => {
                Some(TranscriptItem::Assistant {
                    text: content.clone(),
                    done: true,
                })
            }
            _ => None,
        })
        .collect()
}

/// Écrit un message d'erreur sur la sortie d'erreur (jamais `eprintln!`).
fn ecrire_erreur(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Ce que garde le pilotage d'une session ouverte : verrou, puits d'audit, contexte dérivé.
struct Gardes {
    _lock: SessionLock,
    _audit: Arc<AuditSink<TuiSink>>,
    _derived: DerivedContext,
}

/// Pilotage d'une session lymaille : le `TurnRunner` partagé derrière un mutex
/// tokio, pour qu'une panique de la tâche de tour ne le perde pas.
struct RunnerDriver {
    runner:
        Arc<Mutex<TurnRunner<RigProvider, DefaultStrategy<RefusingSummarizer>, FileSessionStore>>>,
    root: CancelHandle,
    inbox: UserInbox,
    _gardes: Gardes,
}

impl TurnDriver for RunnerDriver {
    fn begin_turn(&self, text: String) -> TurnStart {
        let cancel = self.root.child();
        let runner = Arc::clone(&self.runner);
        let turn_cancel = cancel.clone();
        TurnStart {
            cancel,
            run: Box::pin(async move {
                let mut guard = runner.lock().await;
                guard
                    .run_turn(&text, &turn_cancel)
                    .await
                    .map(|sortie| sortie.outcome())
                    .map_err(|erreur| erreur.to_string())
            }),
        }
    }

    fn inject(&self, text: String) {
        self.inbox.send(text);
    }

    fn todo(&self) -> BoxFuture<'static, Result<Vec<TodoItem>, String>> {
        let runner = Arc::clone(&self.runner);
        Box::pin(async move {
            let guard = runner.lock().await;
            let liste = TodoList::from_values(guard.state().todo()).map_err(|e| e.to_string())?;
            let items = liste.items().map_err(|e| e.to_string())?;
            Ok(items
                .iter()
                .map(|item| TodoItem {
                    content: item.text().to_string(),
                    status: item.status().as_str().to_string(),
                })
                .collect())
        })
    }

    fn cancel_session(&self) {
        self.root.cancel();
    }
}

/// Transforme une session ouverte en pièces pour le TUI.
fn en_ouverte(
    session: OpenedSession<RigProvider, TuiSink>,
    events: tokio::sync::mpsc::UnboundedReceiver<lymaille::Event>,
) -> Opened {
    let OpenedSession {
        runner,
        cancel,
        audit,
        lock,
        derived,
        notices,
    } = session;
    let id = runner.state().id().as_str().to_string();
    let agent = runner.state().agent().name().to_string();
    let title = runner.state().title().map(str::to_owned);
    let transcript = elements_de_l_historique(runner.state().history().messages());
    let inbox = runner.inbox();
    let driver = RunnerDriver {
        runner: Arc::new(Mutex::new(runner)),
        root: cancel,
        inbox,
        _gardes: Gardes {
            _lock: lock,
            _audit: audit,
            _derived: derived,
        },
    };
    Opened {
        driver: Arc::new(driver),
        events,
        id,
        agent,
        title,
        transcript,
        notices,
    }
}

/// Ouverture des sessions pour `/new`, `/sessions` et la reprise du TUI.
struct HoteDeSessions {
    owner: String,
    context: String,
    sandbox: Option<String>,
    repo: Option<std::path::PathBuf>,
    namespace: Option<String>,
}

impl HoteDeSessions {
    fn demande(&self, resume: Resume) -> SessionRequest {
        SessionRequest {
            context: self.context.clone(),
            owner: self.owner.clone(),
            sandbox: self.sandbox.clone(),
            resume,
            profile: None,
            repo: self.repo.clone(),
            namespace: self.namespace.clone(),
        }
    }
}

impl SessionHost for HoteDeSessions {
    fn open(&self, resume: Resume) -> BoxFuture<'static, Result<Opened, String>> {
        let demande = self.demande(resume);
        Box::pin(async move {
            let (sink, events) = TuiSink::channel();
            let session = session_open::open(&KubeEnv, &demande, sink)
                .await
                .map_err(|erreur| erreur.to_string())?;
            Ok(en_ouverte(session, events))
        })
    }

    fn list(&self) -> BoxFuture<'static, Result<Vec<SessionEntry>, String>> {
        let context = self.context.clone();
        let repo = self.repo.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let home = dirs::home_dir().unwrap_or_default();
                let cwd = std::env::current_dir().unwrap_or_default();
                let layers = Layers::discover(&home, &cwd, repo.as_deref(), Path::exists);
                let store = FileSessionStore::new(&layers);
                let audit = AuditLog::new(&layers);
                FileSessionIndex::new(&store, &audit)
                    .sessions_for_context(&context)
                    .into_iter()
                    .filter_map(Result::ok)
                    .collect()
            })
            .await
            .map_err(|erreur| erreur.to_string())
        })
    }
}

/// Worktree de la sandbox : `GET /git/status` avec le jeton de l'Owner.
struct WorktreeSandbox {
    client: Arc<vanyline_lib::k8s::VnlK8sClient>,
    owner: String,
    base: String,
}

impl DirtyFiles for WorktreeSandbox {
    fn count(&self) -> BoxFuture<'static, Result<usize, String>> {
        let client = Arc::clone(&self.client);
        let owner = self.owner.clone();
        let base = self.base.clone();
        Box::pin(async move {
            let jeton = client
                .owner_token(&owner)
                .await
                .map_err(|erreur| format!("jeton de l'Owner indisponible : {erreur}"))?;
            let reponse = reqwest::Client::new()
                .get(format!("{base}/git/status"))
                .bearer_auth(&jeton.token)
                .send()
                .await
                .map_err(|erreur| erreur.to_string())?;
            if !reponse.status().is_success() {
                return Err(format!("git/status : HTTP {}", reponse.status()));
            }
            let corps = reponse.text().await.map_err(|erreur| erreur.to_string())?;
            tui::dirty_count(&corps)
        })
    }
}

/// Point d'entrée de `vanyline chat` : rend le code de sortie de la commande.
pub async fn run_chat(args: ChatArgs, global: GlobalOptions) -> ExitOutcome {
    if !terminal_requis(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        ecrire_erreur("VNL-CLI-CHT-001: chat exige un terminal sur l'entrée et la sortie standard");
        return ExitOutcome::Usage;
    }
    let Some(owner) = global.owner.clone() else {
        ecrire_erreur(
            "VNL-CLI-CHT-002: --owner requis (la valeur par défaut n'est pas lue dans ce lot)",
        );
        return ExitOutcome::Usage;
    };
    let resume = match choix_de_reprise(&args) {
        Ok(resume) => resume,
        Err(message) => {
            ecrire_erreur(&message);
            return ExitOutcome::Usage;
        }
    };
    let hote = HoteDeSessions {
        owner: owner.clone(),
        context: args.context.clone(),
        sandbox: args.sandbox.clone(),
        repo: global.repo.clone(),
        namespace: global.namespace.clone(),
    };
    let demande = hote.demande(resume);
    let (sink, events) = TuiSink::channel();
    let session = match session_open::open(&KubeEnv, &demande, sink).await {
        Ok(session) => session,
        Err(erreur) => {
            ecrire_erreur(&erreur.to_string());
            return match erreur {
                OpenError::Unknown { .. } => ExitOutcome::Usage,
                _ => ExitOutcome::Failure,
            };
        }
    };
    let mcp_url = session.derived.mcp_url.clone();
    let mut dirty: Option<Arc<dyn DirtyFiles>> = None;
    if let (Some(_), Some(url)) = (&args.sandbox, mcp_url) {
        match OpenEnv::connect(&KubeEnv, global.namespace.as_deref()).await {
            Ok(client) => {
                let base = url.strip_suffix("/mcp").map_or(url.clone(), str::to_owned);
                dirty = Some(Arc::new(WorktreeSandbox {
                    client: Arc::new(client),
                    owner: owner.clone(),
                    base,
                }));
            }
            Err(message) => ecrire_erreur(&message),
        }
    }
    let opened = en_ouverte(session, events);
    let options = TuiOptions {
        mouse: true,
        sandbox: dirty,
        host: Arc::new(hote),
        context: args.context,
    };
    match tui::run_tui(opened, options).await {
        Ok(TuiExit::Normal) => ExitOutcome::Success,
        Ok(TuiExit::Signal) => ExitOutcome::Interrupted,
        Err(erreur) => {
            ecrire_erreur(&erreur.to_string());
            ExitOutcome::Failure
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(continue_: bool, resume: Option<&str>) -> ChatArgs {
        ChatArgs {
            context: "p".into(),
            sandbox: None,
            continue_,
            resume: resume.map(str::to_owned),
        }
    }

    /// Scenario: `sans terminal` — erreur d'usage sans ouvrir de session.
    #[test]
    fn terminal_exige_entree_et_sortie() {
        assert!(terminal_requis(true, true));
        assert!(!terminal_requis(false, true));
        assert!(!terminal_requis(true, false));
        assert!(!terminal_requis(false, false));
    }

    /// Scenario: `sans terminal` — sous `cargo test` l'entrée n'est pas un terminal :
    /// code 2, aucune requête au cluster (rien n'est ouvert).
    #[tokio::test]
    async fn sans_terminal_erreur_d_usage() {
        let sortie = run_chat(args(false, None), GlobalOptions::default()).await;
        assert_eq!(sortie, ExitOutcome::Usage);
    }

    /// Scenario: `reprise` — `-c` ⇒ dernière, `--resume` ⇒ identifiée, sinon neuve.
    #[test]
    fn choix_de_reprise_selon_les_options() {
        assert!(matches!(
            choix_de_reprise(&args(false, None)),
            Ok(Resume::New)
        ));
        assert!(matches!(
            choix_de_reprise(&args(true, None)),
            Ok(Resume::Last)
        ));
        assert!(matches!(
            choix_de_reprise(&args(false, Some("s-1"))),
            Ok(Resume::Id(id)) if id.as_str() == "s-1"
        ));
        assert!(choix_de_reprise(&args(false, Some(""))).is_err());
    }

    /// L'historique repris : messages utilisateur et textes de l'assistant seulement.
    #[test]
    fn historique_affiche_a_la_reprise() {
        let messages = vec![
            Message::system("consigne"),
            Message::user("bonjour"),
            Message::assistant("salut", None, Vec::new()),
            Message::assistant("", None, Vec::new()),
        ];
        let items = elements_de_l_historique(&messages);
        assert_eq!(
            items,
            vec![
                TranscriptItem::User {
                    text: "bonjour".into(),
                    during_turn: false,
                },
                TranscriptItem::Assistant {
                    text: "salut".into(),
                    done: true,
                },
            ]
        );
    }
}
