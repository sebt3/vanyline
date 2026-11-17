//! Commande `vanyline run` : un seul tour de l'agent du contexte, sans TUI.
//! Contrat : `run.sdd`.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lymaille::{Event, EventKind, EventSink, InferenceProvider};

use crate::chat::{ChatArgs, GlobalOptions, owner_du_chat};
use crate::exit_code::{self, Outcome as ExitOutcome};
use crate::host::layers::Layers;
use crate::session_open::{
    self, KubeEnv, OpenEnv, OpenError, OpenedSession, Resume, SessionRequest,
};

/// Marqueur d'un message absent, vide ou illisible (usage, code 2).
const MESSAGE: &str = "VNL-CLI-RUN-001";
/// Marqueur d'un réglage de l'utilisateur illisible (usage, code 2).
const USAGE: &str = "VNL-CLI-RUN-002";
/// Marqueur d'un tour terminé par une erreur.
const TOUR: &str = "VNL-CLI-RUN-003";

/// Arguments de `vanyline run` (grammaire : `main.sdd`).
#[derive(Debug, Clone)]
pub struct RunArgs {
    /// Message du tour ; `-` lit l'entrée standard en entier.
    pub message: String,
    /// Projet du contexte de travail.
    pub context: String,
    /// Sandbox de travail (`--sandbox`).
    pub sandbox: Option<String>,
    /// `-c/--continue` : dernière session du contexte.
    pub continue_: bool,
    /// `--resume <id>` : session précise.
    pub resume: Option<String>,
    /// `-m/--model` : profil de modèle du catalogue.
    pub model: Option<String>,
    /// `-t/--timeout <s>` : délai du tour, en secondes ; absent ou `0` ⇒ aucun délai.
    pub timeout: Option<u64>,
    /// `--json` : un événement par ligne sur la sortie standard.
    pub json: bool,
}

/// Flux d'E/S de la commande : permet de tester sans toucher au terminal.
pub struct Flux {
    /// Entrée standard (lue seulement si le message vaut `-`).
    pub entree: Box<dyn Read + Send>,
    /// Sortie standard : réponse finale, ou événements en `--json`.
    pub sortie: Box<dyn Write + Send>,
    /// Sortie d'erreur : déroulé, erreurs, identifiant de session.
    pub erreur: Box<dyn Write + Send>,
}

/// Contenu partagé du puits : le mode et les deux sorties.
struct Sorties {
    json: bool,
    sortie: Box<dyn Write + Send>,
    erreur: Box<dyn Write + Send>,
}

/// Puits d'événements de `run` (`run.sdd`, `@RunSink`) : en texte, le déroulé va sur la sortie d'erreur ; en
/// `--json`, chaque événement est une ligne JSON sur la sortie standard. Cloneable : la commande garde un
/// second accès pour écrire la réponse finale.
#[derive(Clone)]
pub struct RunSink {
    inner: Arc<Mutex<Sorties>>,
}

impl RunSink {
    /// Puits sur les sorties données, en mode texte ou `--json`.
    #[must_use]
    pub fn new(json: bool, sortie: Box<dyn Write + Send>, erreur: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Sorties {
                json,
                sortie,
                erreur,
            })),
        }
    }
}

impl RunSink {
    /// Écrit une ligne sur la sortie d'erreur (jamais sur la sortie standard).
    pub fn ecrire_erreur(&self, texte: &str) {
        if let Ok(mut sorties) = self.inner.lock() {
            let _ = writeln!(sorties.erreur, "{texte}");
        }
    }

    /// Écrit la réponse finale sur la sortie standard, en mode texte seulement (`--json` ne porte que les événements).
    pub fn ecrire_reponse(&self, texte: &str) {
        if let Ok(mut sorties) = self.inner.lock()
            && !sorties.json
        {
            let _ = writeln!(sorties.sortie, "{texte}");
        }
    }
}

impl EventSink for RunSink {
    fn emit(&self, event: Event) {
        let Ok(mut sorties) = self.inner.lock() else {
            return;
        };
        if sorties.json {
            if let Ok(ligne) = serde_json::to_string(&event) {
                let _ = writeln!(sorties.sortie, "{ligne}");
            }
        } else if let Some(texte) = deroulement(&event) {
            let _ = writeln!(sorties.erreur, "{texte}");
        }
    }
}

/// Une ligne de déroulé pour un événement, en mode texte. Les fragments de texte n'en ont pas.
fn deroulement(event: &Event) -> Option<String> {
    match &event.kind {
        EventKind::Script(enregistrement) => Some(match &enregistrement.error {
            Some(erreur) => format!("script : erreur : {erreur}"),
            None => format!("script : ok ({} ms)", enregistrement.duration_ms),
        }),
        EventKind::Overrun { elapsed_ms } => Some(format!(
            "avertissement : tour au-delà de sa durée configurée ({elapsed_ms} ms)"
        )),
        EventKind::InferenceRetry {
            attempt,
            delay_ms,
            error_class,
        } => Some(format!(
            "avertissement : nouvel essai d'inférence {attempt} dans {delay_ms} ms ({error_class})"
        )),
        EventKind::ResponseTruncated { .. } => {
            Some("avertissement : réponse coupée par la limite de sortie du modèle".to_owned())
        }
        EventKind::EmptyResponse { .. } => {
            Some("avertissement : réponse vide du modèle".to_owned())
        }
        EventKind::TurnLeaseLost { .. } => Some("avertissement : bail du tour perdu".to_owned()),
        EventKind::McpReconnect { role, attempt, .. } => Some(format!(
            "avertissement : reconnexion au serveur MCP `{role}` (essai {attempt})"
        )),
        _ => None,
    }
}

/// Lit le message : `-` ⇒ l'entrée standard en entier (fin de ligne finale retirée) ; vide ⇒ erreur d'usage.
///
/// # Errors
///
/// Returns a message with its `VNL-CLI-RUN-*` marker when the message is empty or unreadable.
pub fn lire_message(argument: &str, entree: &mut dyn Read) -> Result<String, String> {
    let message = if argument == "-" {
        let mut lu = String::new();
        entree
            .read_to_string(&mut lu)
            .map_err(|erreur| format!("{MESSAGE}: entrée standard illisible : {erreur}"))?;
        lu.strip_suffix("\r\n")
            .or_else(|| lu.strip_suffix('\n'))
            .unwrap_or(&lu)
            .to_owned()
    } else {
        argument.to_owned()
    };
    if message.trim().is_empty() {
        return Err(format!("{MESSAGE}: message vide"));
    }
    Ok(message)
}

/// Point d'entrée de `vanyline run` avec l'environnement de production.
pub async fn run_run(args: RunArgs, global: GlobalOptions) -> ExitOutcome {
    let flux = Flux {
        entree: Box::new(std::io::stdin()),
        sortie: Box::new(std::io::stdout()),
        erreur: Box::new(std::io::stderr()),
    };
    run_with(&KubeEnv, args, global, flux).await
}

/// `vanyline run` avec un environnement d'ouverture et des flux donnés (tests et production).
pub async fn run_with<E: OpenEnv>(
    env: &E,
    args: RunArgs,
    global: GlobalOptions,
    flux: Flux,
) -> ExitOutcome {
    let Flux {
        mut entree,
        sortie,
        erreur,
    } = flux;
    let puits = RunSink::new(args.json, sortie, erreur);

    // Usage : tout est vérifié avant toute ouverture de session ni appel au cluster.
    let (message, demande) = match preparer(env, &args, &global, &mut *entree) {
        Ok(preparation) => preparation,
        Err(message) => {
            puits.ecrire_erreur(&message);
            return ExitOutcome::Usage;
        }
    };
    let mut ouverte = match session_open::open(env, &demande, puits.clone()).await {
        Ok(ouverte) => ouverte,
        Err(erreur) => {
            puits.ecrire_erreur(&erreur.to_string());
            return match erreur {
                OpenError::Unknown { .. } => ExitOutcome::Usage,
                _ => ExitOutcome::Failure,
            };
        }
    };
    for notice in &ouverte.notices {
        puits.ecrire_erreur(notice);
    }
    let identifiant = ouverte.runner.state().id().as_str().to_owned();

    let issue = executer_tour(&mut ouverte, &message, args.timeout, &puits).await;
    // La session est sauvegardée par le tour, même annulé : son identifiant est toujours rendu.
    puits.ecrire_erreur(&format!("session: {identifiant}"));
    for echec in ouverte.audit.take_failures() {
        puits.ecrire_erreur(&echec.to_string());
    }
    issue
}

/// Message, demande d'ouverture (Owner, reprise, profil) : erreur d'usage avec son marqueur sinon.
fn preparer<E: OpenEnv>(
    env: &E,
    args: &RunArgs,
    global: &GlobalOptions,
    entree: &mut dyn Read,
) -> Result<(String, SessionRequest), String> {
    let message = lire_message(&args.message, entree)?;
    let layers = Layers::discover(
        &env.home(),
        &env.cwd(),
        global.repo.as_deref(),
        Path::exists,
    );
    let owner = owner_du_chat(global, &layers).map_err(|erreur| format!("{USAGE}: {erreur}"))?;
    let resume = reprise_de(args)?;
    let demande = SessionRequest {
        context: args.context.clone(),
        owner,
        sandbox: args.sandbox.clone(),
        resume,
        profile: args.model.clone(),
        repo: global.repo.clone(),
        namespace: global.namespace.clone(),
    };
    Ok((message, demande))
}

/// Un tour de la session. `-t N` (N > 0) et Ctrl+C annulent le tour par son handle ; l'issue est projetée par
/// `exit_code::from_turn`, la réponse n'est écrite que si le tour réussit.
async fn executer_tour<P: InferenceProvider + 'static>(
    ouverte: &mut OpenedSession<P, RunSink>,
    message: &str,
    timeout: Option<u64>,
    puits: &RunSink,
) -> exit_code::Outcome {
    let annulation = ouverte.cancel.clone();
    let echeance = Arc::new(AtomicBool::new(false));
    let interruption = Arc::new(AtomicBool::new(false));
    let minuterie = timeout.filter(|secondes| *secondes > 0).map(|secondes| {
        let annulation = annulation.clone();
        let echeance = Arc::clone(&echeance);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secondes)).await;
            echeance.store(true, Ordering::SeqCst);
            annulation.cancel();
        })
    });
    let ctrl_c = {
        let annulation = annulation.clone();
        let interruption = Arc::clone(&interruption);
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                interruption.store(true, Ordering::SeqCst);
                annulation.cancel();
            }
        })
    };

    let resultat = ouverte.runner.run_turn(message, &annulation).await;
    if let Some(minuterie) = minuterie {
        minuterie.abort();
    }
    ctrl_c.abort();

    match resultat {
        Ok(sortie) => {
            let issue = exit_code::from_turn(
                sortie.outcome(),
                echeance.load(Ordering::SeqCst),
                interruption.load(Ordering::SeqCst),
            );
            if issue == exit_code::Outcome::Success {
                puits.ecrire_reponse(sortie.response());
            }
            issue
        }
        Err(erreur) => {
            puits.ecrire_erreur(&format!("{TOUR}: {erreur}"));
            exit_code::Outcome::Failure
        }
    }
}

/// Mode de reprise de `run`, avec les règles de `chat` (`-c`, `--resume`, sinon neuve).
fn reprise_de(args: &RunArgs) -> Result<Resume, String> {
    crate::chat::choix_de_reprise(&ChatArgs {
        context: args.context.clone(),
        sandbox: args.sandbox.clone(),
        continue_: args.continue_,
        resume: args.resume.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use lymaille::{
        Event, EventKind, EventSink, InferenceError, InferenceProvider, InferenceRequest,
        InferenceResponse, Message, ModelProfile, Provider, ProviderLevel, SessionId, Timestamp,
        TurnId,
    };
    use serde_json::json;
    use vanyline_crds::{Project, ProjectSpec};

    use super::{Flux, GlobalOptions, RunArgs, RunSink, lire_message, run_with};
    use crate::exit_code::Outcome as ExitOutcome;
    use crate::host::catalog_store::FileCatalog;
    use crate::host::context_derive::{ClusterRefusal, ClusterSource};
    use crate::host::layers::{Layers, Level};
    use crate::session_open::OpenEnv;

    /// Cluster simulé : le projet `demo`, sans sandbox.
    struct FauxCluster;

    impl ClusterSource for FauxCluster {
        fn project<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Project, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move {
                if name == "demo" {
                    let spec: ProjectSpec = serde_json::from_value(json!({
                        "owner": "alice",
                        "repoUrl": "https://example.invalid/demo.git",
                        "defaultBranch": "main"
                    }))
                    .expect("spec de projet valide");
                    let mut projet = Project::new("demo", spec);
                    projet.metadata.namespace = Some("vanyline-app-alice".to_owned());
                    Ok(projet)
                } else {
                    Err(ClusterRefusal::NotFound)
                }
            })
        }

        fn sandbox<'a>(
            &'a self,
            _owner: &'a str,
            _name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<vanyline_crds::Sandbox, ClusterRefusal>> + Send + 'a>>
        {
            Box::pin(async move { Err(ClusterRefusal::NotFound) })
        }

        fn sandbox_mcp_url<'a>(
            &'a self,
            _owner: &'a str,
            _name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move { Err(ClusterRefusal::NotFound) })
        }
    }

    /// Comportement du modèle simulé : répond un texte, ou ne répond jamais (le tour ne finit que par annulation).
    #[derive(Clone, Copy)]
    enum Modele {
        Repond(&'static str),
        Apres(Duration, &'static str),
        Attend,
    }

    /// Fournisseur simulé : chaque requête est enregistrée, la réponse suit `Modele`.
    struct FauxModele {
        mode: Modele,
        requetes: Arc<Mutex<Vec<InferenceRequest>>>,
    }

    type Attente = Pin<Box<dyn Future<Output = Result<InferenceResponse, InferenceError>> + Send>>;

    impl InferenceProvider for FauxModele {
        fn complete(
            &self,
            request: InferenceRequest,
        ) -> impl Future<Output = Result<InferenceResponse, InferenceError>> + Send {
            self.requetes.lock().expect("requêtes").push(request);
            let attente: Attente = match self.mode {
                Modele::Repond(texte) => Box::pin(std::future::ready(Ok(InferenceResponse::new(
                    Message::assistant(texte, None, Vec::new()),
                    None,
                )))),
                Modele::Apres(delai, texte) => Box::pin(async move {
                    tokio::time::sleep(delai).await;
                    Ok(InferenceResponse::new(
                        Message::assistant(texte, None, Vec::new()),
                        None,
                    ))
                }),
                Modele::Attend => Box::pin(std::future::pending()),
            };
            attente
        }
    }

    /// Environnement de test : cluster joignable ou non, modèle simulé, répertoire personnel temporaire.
    struct FakeEnv {
        home: PathBuf,
        joignable: bool,
        modele: Modele,
        requetes: Arc<Mutex<Vec<InferenceRequest>>>,
    }

    impl OpenEnv for FakeEnv {
        type Cluster = FauxCluster;
        type Provider = FauxModele;
        type Connector = lymaille::ScriptedConnector;

        fn home(&self) -> PathBuf {
            self.home.clone()
        }

        fn cwd(&self) -> PathBuf {
            self.home.clone()
        }

        fn connect<'a>(
            &'a self,
            _namespace: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<FauxCluster, String>> + Send + 'a>> {
            Box::pin(async move {
                if self.joignable {
                    Ok(FauxCluster)
                } else {
                    Err("VNL-K8S-001: kubeconfig injoignable".to_owned())
                }
            })
        }

        fn provider(&self, _provider: &Provider, _key: Option<&str>) -> Result<FauxModele, String> {
            Ok(FauxModele {
                mode: self.modele,
                requetes: Arc::clone(&self.requetes),
            })
        }

        fn sandbox_connector(
            &self,
            _cluster: &FauxCluster,
            _owner: &str,
            _url: &str,
        ) -> Result<lymaille::ScriptedConnector, String> {
            Ok(lymaille::ScriptedConnector::new(Vec::new()))
        }
    }

    fn environnement(home: &Path, modele: Modele, joignable: bool) -> FakeEnv {
        FakeEnv {
            home: home.to_path_buf(),
            joignable,
            modele,
            requetes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Un profil `test` et un provider `fournisseur` au niveau utilisateur, sans secret.
    fn catalogue_de_test(home: &Path) {
        let layers = Layers::discover(home, home, None, Path::exists);
        let catalogue = FileCatalog::new(&layers);
        let profil = ModelProfile::new(
            "test".to_owned(),
            "fournisseur".to_owned(),
            "modele".to_owned(),
        )
        .with_context_size(131_072)
        .with_max_response(8_192);
        catalogue
            .put_profile_at(Level::User, &profil)
            .expect("le profil de test doit s'écrire");
        let fournisseur = Provider::new(
            "fournisseur".to_owned(),
            "openai".to_owned(),
            "http://llm.invalid/v1".to_owned(),
            None,
            ProviderLevel::User,
        )
        .expect("le provider de test doit être valide");
        catalogue
            .put_provider_at(Level::User, &fournisseur)
            .expect("le provider de test doit s'écrire");
    }

    fn arguments(message: &str) -> RunArgs {
        RunArgs {
            message: message.to_owned(),
            context: "demo".to_owned(),
            sandbox: None,
            continue_: false,
            resume: None,
            model: Some("test".to_owned()),
            timeout: None,
            json: false,
        }
    }

    fn globales() -> GlobalOptions {
        GlobalOptions {
            owner: Some("alice".to_owned()),
            repo: None,
            namespace: None,
        }
    }

    /// Flux de test : entrée donnée, sorties capturées.
    fn flux(entree: &str) -> (Flux, Tampon, Tampon) {
        let sortie = Tampon::default();
        let erreur = Tampon::default();
        let flux = Flux {
            entree: Box::new(std::io::Cursor::new(entree.as_bytes().to_vec())),
            sortie: Box::new(sortie.clone()),
            erreur: Box::new(erreur.clone()),
        };
        (flux, sortie, erreur)
    }

    /// Scenario `message sur l'entrée standard` : `-` lit l'entrée en entier, sans la fin de ligne finale.
    #[test]
    fn message_lu_sur_l_entree_standard() {
        let mut entree = "bonjour\n".as_bytes();
        assert_eq!(lire_message("-", &mut entree).as_deref(), Ok("bonjour"));
    }

    /// Scenario `message vide` : message vide en argument ou entrée standard vide, erreur d'usage.
    #[test]
    fn message_vide_est_une_erreur() {
        let mut vide: &[u8] = b"  \n";
        assert!(lire_message("-", &mut vide).is_err());
        let mut rien: &[u8] = b"";
        assert!(lire_message("", &mut rien).is_err());
        let mut arguments_seul: &[u8] = b"";
        assert_eq!(
            lire_message("bonjour", &mut arguments_seul).as_deref(),
            Ok("bonjour")
        );
    }

    /// Scenario `message vide` : sans message, code 2 et aucune session ouverte (le cluster n'est même pas joint).
    #[tokio::test]
    async fn message_vide_code_2_sans_session() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = environnement(home.path(), Modele::Repond("ok"), false);
        let (flux, _sortie, erreur) = flux("");

        let code = run_with(&env, arguments("-"), globales(), flux).await;

        assert_eq!(code, ExitOutcome::Usage);
        assert!(
            !erreur.texte().contains("VNL-K8S-001"),
            "{}",
            erreur.texte()
        );
    }

    /// Scenario `message sur l'entrée standard` : réponse sur la sortie standard, `session:` sur l'erreur, code 0.
    #[tokio::test]
    async fn reponse_sur_sortie_standard_session_sur_erreur() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = environnement(home.path(), Modele::Repond("ok"), true);
        let (flux, sortie, erreur) = flux("bonjour");

        let code = run_with(&env, arguments("-"), globales(), flux).await;

        assert_eq!(code, ExitOutcome::Success, "stderr : {}", erreur.texte());
        assert_eq!(sortie.texte(), "ok\n");
        assert!(erreur.texte().contains("session: "), "{}", erreur.texte());
    }

    /// Scenario `sortie JSONL` : chaque ligne de la sortie standard est un événement valide, rien d'autre.
    #[tokio::test]
    async fn sortie_jsonl_une_ligne_par_evenement() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = environnement(home.path(), Modele::Repond("ok"), true);
        let (flux, sortie, _erreur) = flux("bonjour");
        let mut args = arguments("-");
        args.json = true;

        let code = run_with(&env, args, globales(), flux).await;

        assert_eq!(code, ExitOutcome::Success);
        let texte = sortie.texte();
        assert!(!texte.is_empty(), "des événements doivent être écrits");
        for ligne in texte.lines() {
            serde_json::from_str::<Event>(ligne)
                .unwrap_or_else(|erreur| panic!("ligne non événement : {ligne} ({erreur})"));
        }
    }

    /// Scenario `échéance` : `-t 1` et un tour qui ne finit pas ⇒ annulé, code 3.
    #[tokio::test]
    async fn echeance_annule_le_tour_code_3() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = environnement(home.path(), Modele::Attend, true);
        let (flux, _sortie, _erreur) = flux("bonjour");
        let mut args = arguments("-");
        args.timeout = Some(1);
        let depart = Instant::now();

        let code = run_with(&env, args, globales(), flux).await;

        assert_eq!(code, ExitOutcome::Timeout);
        assert!(depart.elapsed() >= Duration::from_secs(1));
    }

    /// Garde de l'ancien test d'intégration `timeout_zero_desactive_la_limite` : `-t 0` ne pose aucun délai ; un tour
    /// qui dure plus d'une seconde va à son terme (code 0, réponse visible).
    #[tokio::test]
    async fn timeout_zero_desactive_la_limite() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = environnement(
            home.path(),
            Modele::Apres(Duration::from_millis(1200), "ok"),
            true,
        );
        let (flux, sortie, erreur) = flux("bonjour");
        let mut args = arguments("-");
        args.timeout = Some(0);

        let code = run_with(&env, args, globales(), flux).await;

        assert_eq!(code, ExitOutcome::Success, "stderr : {}", erreur.texte());
        assert_eq!(sortie.texte(), "ok\n");
    }

    /// Scenario `reprise` : `-c` reprend la session précédente, dont l'échange est visible du modèle (`AC-6`).
    #[tokio::test]
    async fn reprise_avec_c_connait_l_echange_precedent() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        catalogue_de_test(home.path());
        let env = environnement(home.path(), Modele::Repond("ok"), true);
        let (premier, _s1, erreur1) = flux("premier");
        assert_eq!(
            run_with(&env, arguments("-"), globales(), premier).await,
            ExitOutcome::Success
        );
        let (second, _s2, erreur2) = flux("second");
        let mut args = arguments("-");
        args.continue_ = true;

        let code = run_with(&env, args, globales(), second).await;

        assert_eq!(code, ExitOutcome::Success, "stderr : {}", erreur2.texte());
        let identifiant = |texte: String| {
            texte
                .lines()
                .find_map(|l| l.strip_prefix("session: ").map(str::to_owned))
                .unwrap_or_default()
        };
        assert_eq!(identifiant(erreur1.texte()), identifiant(erreur2.texte()));
        let requetes = env.requetes.lock().expect("requêtes");
        let derniere = requetes.last().expect("une requête du second tour");
        assert!(
            derniere
                .messages()
                .iter()
                .any(|m| matches!(m, Message::User { content } if content == "premier")),
            "le modèle doit voir le message du premier tour"
        );
    }

    /// Scenario `texte` : seule la réponse finale va sur la sortie standard ; le déroulé reste sur l'erreur.
    #[test]
    fn puits_texte_ne_garde_que_le_deroulé_sur_l_erreur() {
        let sortie = Tampon::default();
        let erreur = Tampon::default();
        let puits = RunSink::new(false, Box::new(sortie.clone()), Box::new(erreur.clone()));
        let session = SessionId::new("s-1").expect("identifiant");
        puits.emit(Event::new(
            session.clone(),
            Timestamp::from_millis(1),
            EventKind::TextDelta {
                turn: TurnId::new(1),
                text: "fragment".to_owned(),
            },
        ));
        puits.emit(Event::new(
            session,
            Timestamp::from_millis(2),
            EventKind::Overrun { elapsed_ms: 900 },
        ));

        assert_eq!(sortie.texte(), "", "aucun fragment sur la sortie standard");
        assert!(erreur.texte().contains("900"), "{}", erreur.texte());
        assert!(!erreur.texte().contains("fragment"));
    }

    /// Scenario `sortie JSONL` au niveau du puits : une ligne JSON par événement, dans l'ordre.
    #[test]
    fn puits_json_une_ligne_par_evenement_dans_l_ordre() {
        let sortie = Tampon::default();
        let erreur = Tampon::default();
        let puits = RunSink::new(true, Box::new(sortie.clone()), Box::new(erreur.clone()));
        let session = SessionId::new("s-1").expect("identifiant");
        puits.emit(Event::new(
            session.clone(),
            Timestamp::from_millis(1),
            EventKind::TextDelta {
                turn: TurnId::new(1),
                text: "a".to_owned(),
            },
        ));
        puits.emit(Event::new(
            session,
            Timestamp::from_millis(2),
            EventKind::TurnStarted {
                turn: TurnId::new(1),
            },
        ));

        let lignes: Vec<Event> = sortie
            .texte()
            .lines()
            .map(|l| serde_json::from_str(l).expect("ligne JSON"))
            .collect();
        assert_eq!(lignes.len(), 2);
        assert!(matches!(lignes[0].kind, EventKind::TextDelta { .. }));
        assert!(matches!(lignes[1].kind, EventKind::TurnStarted { .. }));
        assert_eq!(erreur.texte(), "");
    }

    /// Sortie capturée : clonable, lisible après l'écriture.
    #[derive(Clone, Default)]
    struct Tampon(Arc<Mutex<Vec<u8>>>);

    impl Tampon {
        fn texte(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("tampon")).into_owned()
        }
    }

    impl Write for Tampon {
        fn write(&mut self, octets: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("tampon").extend_from_slice(octets);
            Ok(octets.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
