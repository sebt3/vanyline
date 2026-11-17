//! Commande `vanyline script` : un fichier Rhai exécuté sur une sandbox, sans LLM ni session.
//! Contrat : `script.sdd`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lymaille::{
    CancelHandle, Clock, DefaultSecretDetector, EventSink, NullSink, Redactor, RunEnv, RunOutcome,
    RunStatus, Scratchpad, ScratchpadEngine, ScratchpadLimits, ScratchpadResult, SecretDetector,
    SystemClock,
};

use crate::chat::{GlobalOptions, owner_du_chat};
use crate::exit_code::{self, Outcome as ExitOutcome};
use crate::host::audit::{AuditEntry, AuditLog};
use crate::host::context_derive::derive_context;
use crate::host::layers::Layers;
use crate::session_open::{KubeEnv, OpenEnv, accorder, nouvel_identifiant};

/// Marqueur d'un usage invalide : fichier illisible ou vide, paramètre mal formé, identifiant ou doublon, owner absent.
const USAGE: &str = "VNL-CLI-SCR-001";
/// Marqueur d'une valeur de paramètre que le détecteur de secrets reconnaît (usage, code 2).
const SECRET: &str = "VNL-CLI-SCR-002";
/// Marqueur d'une exécution qui ne peut pas commencer : cluster, contexte, moteur.
const EXECUTION: &str = "VNL-CLI-SCR-003";
/// Marqueur d'un journal d'audit inaccessible après l'exécution.
const AUDIT: &str = "VNL-CLI-SCR-004";
/// Nom de l'agent porté par les enregistrements de script hors session.
const AGENT: &str = "script";
/// Mots réservés de Rhai : un nom de paramètre qui en est un n'est pas un identifiant utilisable.
const MOTS_RESERVES: &[&str] = &[
    "as", "break", "case", "catch", "const", "continue", "default", "do", "else", "export",
    "false", "fn", "for", "global", "if", "import", "in", "is", "let", "loop", "nil", "private",
    "return", "switch", "this", "throw", "true", "try", "while",
];

/// Arguments de `vanyline script` (grammaire : `main.sdd`).
///
/// `params` garde les paires `clé=valeur` brutes : leur lecture et leur refus (secret, identifiant, doublon) se font à
/// l'exécution, sans jamais reprendre la valeur dans un message (`script.sdd`, `FR-5`).
#[derive(Debug, Clone)]
pub struct ScriptArgs {
    /// Fichier Rhai à exécuter.
    pub file: PathBuf,
    /// Projet du contexte de travail.
    pub context: String,
    /// Sandbox de travail (obligatoire).
    pub sandbox: String,
    /// `--param clé=valeur`, répétable.
    pub params: Vec<String>,
    /// `-t/--timeout <s>` : délai de l'exécution, en secondes ; absent ou `0` ⇒ aucun délai.
    pub timeout: Option<u64>,
    /// `--json` : un seul objet `{"status", "text"}` sur la sortie standard.
    pub json: bool,
}

/// Flux d'E/S de la commande : permet de tester sans toucher au terminal.
pub struct Flux {
    /// Sortie standard : texte rendu par le script, ou objet JSON en `--json`.
    pub sortie: Box<dyn Write + Send>,
    /// Sortie d'erreur : erreur de script avec sa classe et sa position.
    pub erreur: Box<dyn Write + Send>,
}

/// Lit une paire `clé=valeur` : le nom avant le premier `=`, la valeur après. Le texte d'une erreur ne reprend jamais
/// l'entrée.
///
/// # Errors
///
/// Returns a message with its `VNL-CLI-SCR-*` marker when the pair has no `=`.
pub fn parse_param(texte: &str) -> Result<(String, String), String> {
    texte
        .split_once('=')
        .map(|(nom, valeur)| (nom.to_owned(), valeur.to_owned()))
        .ok_or_else(|| format!("{USAGE}: --param attend la forme clé=valeur"))
}

/// Vrai pour un identifiant Rhai utilisable : lettre ou `_` puis lettres, chiffres ou `_`, et non réservé.
fn est_identifiant(nom: &str) -> bool {
    let mut caracteres = nom.chars();
    matches!(caracteres.next(), Some(premier) if premier == '_' || premier.is_ascii_alphabetic())
        && caracteres.all(|caractere| caractere == '_' || caractere.is_ascii_alphanumeric())
        && !MOTS_RESERVES.contains(&nom)
}

/// Paires validées : identifiant valide, unique, valeur que le détecteur ne reconnaît pas. Une erreur nomme la clé,
/// jamais la valeur.
fn valider_parametres(paires: Vec<(String, String)>) -> Result<BTreeMap<String, String>, String> {
    let mut valides = BTreeMap::new();
    for (nom, valeur) in paires {
        if !est_identifiant(&nom) {
            return Err(format!(
                "{USAGE}: `{nom}` n'est pas un identifiant Rhai valide"
            ));
        }
        if DefaultSecretDetector::default().looks_like_secret(&valeur) {
            return Err(format!(
                "{SECRET}: la valeur de `{nom}` ressemble à un secret : refusée avant toute exécution"
            ));
        }
        if valides.insert(nom.clone(), valeur).is_some() {
            return Err(format!("{USAGE}: `{nom}` est donné plusieurs fois"));
        }
    }
    Ok(valides)
}

/// Erreur de script pour une clé absente : classe `NotFound` de genre variable, qui porte le nom de la clé et la
/// position de l'appel, jamais la valeur.
fn cle_absente(nom: &str) -> rhai::EvalAltResult {
    rhai::EvalAltResult::ErrorVariableNotFound(nom.to_owned(), rhai::Position::NONE)
}

#[allow(clippy::result_large_err)] // script.sdd, Handles : `with_registration` de lymaille impose `ScratchpadResult<()>`.
fn enregistrer_param(
    paires: BTreeMap<String, String>,
) -> impl FnOnce(&mut rhai::Engine) -> ScratchpadResult<()> + 'static {
    move |moteur: &mut rhai::Engine| {
        moteur.register_fn(
            "param",
            move |nom: &str| -> Result<String, Box<rhai::EvalAltResult>> {
                paires
                    .get(nom)
                    .cloned()
                    .ok_or_else(|| Box::new(cle_absente(nom)))
            },
        );
        Ok(())
    }
}

/// Nom du statut de l'exécution dans la sortie JSON.
const fn statut_nom(statut: RunStatus) -> &'static str {
    match statut {
        RunStatus::Ok => "ok",
        RunStatus::Cancelled => "cancelled",
        _ => "failed",
    }
}

/// Point d'entrée de `vanyline script` avec l'environnement de production.
pub async fn run_script(args: ScriptArgs, global: GlobalOptions) -> ExitOutcome {
    let flux = Flux {
        sortie: Box::new(std::io::stdout()),
        erreur: Box::new(std::io::stderr()),
    };
    run_script_with(&KubeEnv, args, global, flux).await
}

/// `vanyline script` avec un environnement et des flux donnés (tests et production).
pub async fn run_script_with<E: OpenEnv>(
    env: &E,
    args: ScriptArgs,
    global: GlobalOptions,
    flux: Flux,
) -> ExitOutcome {
    let Flux {
        mut sortie,
        mut erreur,
    } = flux;

    // Usage : tout est vérifié avant tout appel au cluster ni exécution.
    let (texte, owner, paires, layers) = match preparer(env, &args, &global) {
        Ok(preparation) => preparation,
        Err(message) => {
            let _ = writeln!(erreur, "{message}");
            return ExitOutcome::Usage;
        }
    };
    let cluster = match env.connect(global.namespace.as_deref()).await {
        Ok(cluster) => cluster,
        Err(cause) => {
            let _ = writeln!(erreur, "{cause}");
            return ExitOutcome::Failure;
        }
    };
    let derived = match derive_context(&cluster, &owner, &args.context, Some(&args.sandbox)).await {
        Ok(derived) => derived,
        Err(cause) => {
            let _ = writeln!(erreur, "{EXECUTION}: {cause}");
            return ExitOutcome::Failure;
        }
    };
    let id = match nouvel_identifiant() {
        Ok(id) => id,
        Err(cause) => {
            let _ = writeln!(erreur, "{EXECUTION}: {cause}");
            return ExitOutcome::Failure;
        }
    };

    // Modules accordés au contexte, comme à une session (`cli#BR-3`), puis moteur avec `param`.
    let puits: Arc<dyn EventSink> = Arc::new(NullSink::new());
    let horloge: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let grants = match accorder(env, &cluster, &derived, &owner, &puits, &horloge, &id).await {
        Ok(grants) => grants,
        Err(cause) => {
            let _ = writeln!(erreur, "{cause}");
            return ExitOutcome::Failure;
        }
    };
    let moteur = match ScratchpadEngine::builder(ScratchpadLimits::new(), grants)
        .with_registration(enregistrer_param(paires))
        .build()
    {
        Ok(moteur) => moteur,
        Err(cause) => {
            let _ = writeln!(erreur, "{EXECUTION}: {cause}");
            return ExitOutcome::Failure;
        }
    };

    let pad = Scratchpad::new(Arc::new(moteur));
    let redacteur = Redactor::new();
    let environnement = RunEnv::new(AGENT, &redacteur, &*horloge);
    let (resultat, echeance, interruption) =
        executer(&pad, &texte, args.timeout, &environnement).await;

    // Chaque exécution est inscrite au journal : un journal inaccessible fait échouer la commande.
    let mut issue = exit_code::from_script(resultat.status(), echeance, interruption);
    let entree = AuditEntry {
        turn: 0,
        record: resultat.record().clone(),
    };
    if let Err(cause) = AuditLog::new(&layers).record_standalone(&entree, horloge.now()) {
        let _ = writeln!(erreur, "{AUDIT}: journal d'audit inaccessible : {cause}");
        issue = ExitOutcome::Failure;
    }

    let reussi = matches!(resultat.status(), RunStatus::Ok);
    if !reussi {
        let _ = writeln!(erreur, "{}", resultat.text());
    }
    if args.json {
        let objet =
            serde_json::json!({"status": statut_nom(resultat.status()), "text": resultat.text()});
        let _ = writeln!(sortie, "{objet}");
    } else if reussi {
        let _ = writeln!(sortie, "{}", resultat.text());
    }
    issue
}

/// Texte du script, owner, paramètres validés et couches : erreur d'usage avec son marqueur sinon.
fn preparer<E: OpenEnv>(
    env: &E,
    args: &ScriptArgs,
    global: &GlobalOptions,
) -> Result<(String, String, BTreeMap<String, String>, Layers), String> {
    let texte = std::fs::read_to_string(&args.file).map_err(|cause| {
        format!(
            "{USAGE}: fichier `{}` illisible : {cause}",
            args.file.display()
        )
    })?;
    if texte.trim().is_empty() {
        return Err(format!("{USAGE}: le script est vide"));
    }
    let paires = args
        .params
        .iter()
        .map(|brut| parse_param(brut.as_str()))
        .collect::<Result<Vec<_>, _>>()?;
    let paires = valider_parametres(paires)?;
    let layers = Layers::discover(
        &env.home(),
        &env.cwd(),
        global.repo.as_deref(),
        Path::exists,
    );
    let owner = owner_du_chat(global, &layers).map_err(|cause| format!("{USAGE}: {cause}"))?;
    Ok((texte, owner, paires, layers))
}

/// Une exécution du script, avec `-t` et Ctrl+C qui annulent par le handle. Rend le résultat et les deux causes
/// d'annulation connues.
async fn executer(
    pad: &Scratchpad,
    texte: &str,
    timeout: Option<u64>,
    environnement: &RunEnv<'_>,
) -> (RunOutcome, bool, bool) {
    let annulation = CancelHandle::new();
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
    let resultat = pad.run(texte, &annulation, environnement).await;
    if let Some(minuterie) = minuterie {
        minuterie.abort();
    }
    ctrl_c.abort();
    (
        resultat,
        echeance.load(Ordering::SeqCst),
        interruption.load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use lymaille::{Provider, ScriptedConnector, ScriptedProvider};
    use vanyline_crds::{Project, ProjectSpec, Sandbox, SandboxSpec, SandboxStatus};

    use super::{Flux, ScriptArgs, parse_param, run_script_with};
    use crate::chat::GlobalOptions;
    use crate::exit_code::Outcome;
    use crate::host::context_derive::{ClusterRefusal, ClusterSource};
    use crate::host::layers::Layers;
    use crate::session_open::OpenEnv;

    /// Tampon partagé : le test relit ce que la commande a écrit.
    #[derive(Clone, Default)]
    struct Tampon(Arc<Mutex<Vec<u8>>>);

    impl Tampon {
        fn texte(&self) -> String {
            String::from_utf8(
                self.0
                    .lock()
                    .expect("le tampon n'est pas empoisonné")
                    .clone(),
            )
            .expect("une sortie UTF-8")
        }
    }

    impl Write for Tampon {
        fn write(&mut self, octets: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("le tampon n'est pas empoisonné")
                .extend_from_slice(octets);
            Ok(octets.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Faux cluster : le projet `demo` et la sandbox `demo-dev` (Running).
    struct FauxCluster {
        sandboxes: Vec<Sandbox>,
    }

    impl ClusterSource for FauxCluster {
        fn project<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Project, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move {
                if name == "demo" {
                    Ok(projet())
                } else {
                    Err(ClusterRefusal::NotFound)
                }
            })
        }

        fn sandbox<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Sandbox, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move {
                self.sandboxes
                    .iter()
                    .find(|sandbox| sandbox.metadata.name.as_deref() == Some(name))
                    .cloned()
                    .ok_or(ClusterRefusal::NotFound)
            })
        }

        fn sandbox_mcp_url<'a>(
            &'a self,
            _owner: &'a str,
            name: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, ClusterRefusal>> + Send + 'a>> {
            Box::pin(async move { Ok(format!("http://{name}.vanyline-app-alice.svc:3000/mcp")) })
        }
    }

    fn projet() -> Project {
        let spec: ProjectSpec = serde_json::from_value(serde_json::json!({
            "owner": "alice",
            "repoUrl": "https://example.invalid/demo.git",
            "defaultBranch": "main"
        }))
        .expect("spec de projet valide");
        let mut projet = Project::new("demo", spec);
        projet.metadata.namespace = Some("vanyline-app-alice".to_owned());
        projet
    }

    fn sandbox_running() -> Sandbox {
        let spec: SandboxSpec =
            serde_json::from_value(serde_json::json!({"project": "demo", "branch": "main"}))
                .expect("spec de sandbox valide");
        let mut sandbox = Sandbox::new("demo-dev", spec);
        sandbox.metadata.namespace = Some("vanyline-app-alice".to_owned());
        sandbox.status = Some(SandboxStatus {
            phase: Some("Running".to_owned()),
            ..SandboxStatus::default()
        });
        sandbox
    }

    /// Environnement de test : cluster simulé, MCP simulé, répertoire personnel temporaire.
    struct FakeEnv {
        home: PathBuf,
    }

    impl OpenEnv for FakeEnv {
        type Cluster = FauxCluster;
        type Provider = ScriptedProvider;
        type Connector = ScriptedConnector;

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
                Ok(FauxCluster {
                    sandboxes: vec![sandbox_running()],
                })
            })
        }

        fn provider(
            &self,
            _provider: &Provider,
            _key: Option<&str>,
        ) -> Result<ScriptedProvider, String> {
            Ok(ScriptedProvider::new(Vec::new()))
        }

        fn sandbox_connector(
            &self,
            _cluster: &FauxCluster,
            _owner: &str,
            _url: &str,
        ) -> Result<ScriptedConnector, String> {
            Ok(ScriptedConnector::new(Vec::new()))
        }
    }

    fn global() -> GlobalOptions {
        GlobalOptions {
            owner: Some("alice".to_owned()),
            repo: None,
            namespace: None,
        }
    }

    /// Un fichier de script `nom` dans `dossier`, avec `texte`.
    fn ecrire_script(dossier: &Path, nom: &str, texte: &str) -> PathBuf {
        let chemin = dossier.join(nom);
        std::fs::write(&chemin, texte).expect("le script de test doit s'écrire");
        chemin
    }

    fn demande(fichier: PathBuf, params: &[&str], timeout: Option<u64>, json: bool) -> ScriptArgs {
        ScriptArgs {
            file: fichier,
            context: "demo".to_owned(),
            sandbox: "dev".to_owned(),
            params: params.iter().map(|texte| (*texte).to_owned()).collect(),
            timeout,
            json,
        }
    }

    /// Exécute la commande ; rend le code, la sortie standard, la sortie d'erreur.
    async fn executer(home: &Path, args: ScriptArgs) -> (Outcome, String, String) {
        let sortie = Tampon::default();
        let erreur = Tampon::default();
        let flux = Flux {
            sortie: Box::new(sortie.clone()),
            erreur: Box::new(erreur.clone()),
        };
        let issue = run_script_with(
            &FakeEnv {
                home: home.to_path_buf(),
            },
            args,
            global(),
            flux,
        )
        .await;
        (issue, sortie.texte(), erreur.texte())
    }

    /// Nombre de lignes du journal d'audit de `home`, tous fichiers confondus.
    fn entrees_d_audit(home: &Path) -> usize {
        let dossier = Layers::discover(home, home, None, Path::exists)
            .state()
            .join("audit");
        let Ok(fichiers) = std::fs::read_dir(dossier) else {
            return 0;
        };
        fichiers
            .filter_map(Result::ok)
            .map(|fichier| {
                std::fs::read_to_string(fichier.path()).map_or(0, |texte| texte.lines().count())
            })
            .sum()
    }

    /// Contenu brut du journal d'audit de `home`, tous fichiers confondus.
    fn contenu_d_audit(home: &Path) -> String {
        let dossier = Layers::discover(home, home, None, Path::exists)
            .state()
            .join("audit");
        let Ok(fichiers) = std::fs::read_dir(dossier) else {
            return String::new();
        };
        fichiers
            .filter_map(Result::ok)
            .filter_map(|fichier| std::fs::read_to_string(fichier.path()).ok())
            .collect()
    }

    // Scenario: script correct
    #[tokio::test]
    async fn script_correct_rend_son_texte_code_0_et_une_entree_d_audit() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "ok.rhai", "\"ok\"");

        let (issue, sortie, erreur) =
            executer(home.path(), demande(fichier, &[], None, false)).await;

        assert_eq!(
            issue,
            Outcome::Success,
            "un script correct sort avec 0, erreur : {erreur:?}"
        );
        assert_eq!(
            sortie.trim(),
            "ok",
            "le texte du script est sur la sortie standard"
        );
        assert_eq!(
            entrees_d_audit(home.path()),
            1,
            "l'exécution est inscrite au journal"
        );
    }

    // Scenario: script en erreur
    #[tokio::test]
    async fn script_en_erreur_donne_classe_et_position_code_1() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(
            home.path(),
            "erreur.rhai",
            "let a = 1;\nlet b = 2;\nthrow \"boom\";\n",
        );

        let (issue, sortie, erreur) =
            executer(home.path(), demande(fichier, &[], None, false)).await;

        assert_eq!(issue, Outcome::Failure, "un script en erreur sort avec 1");
        assert!(
            sortie.is_empty(),
            "rien sur la sortie standard, obtenu {sortie:?}"
        );
        assert!(
            erreur.contains("runtime error") && erreur.contains("line 3"),
            "la classe et la ligne 3 sont sur la sortie d'erreur, obtenu {erreur:?}"
        );
        assert_eq!(
            entrees_d_audit(home.path()),
            1,
            "l'exécution en erreur est inscrite au journal"
        );
    }

    // Scenario: paramètre lu par la fonction param, valeur absente du journal
    #[tokio::test]
    async fn parametre_lu_par_param_et_absent_du_journal() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "param.rhai", "let v = param(\"nom\");\n\"ok\"");

        let (issue, sortie, erreur) = executer(
            home.path(),
            demande(fichier, &["nom=valeur-unique-7f3a"], None, false),
        )
        .await;

        assert_eq!(
            issue,
            Outcome::Success,
            "le script lit son paramètre, erreur : {erreur:?}"
        );
        assert_eq!(sortie.trim(), "ok");
        assert!(
            !contenu_d_audit(home.path()).contains("valeur-unique-7f3a"),
            "la valeur du paramètre n'est écrite dans aucun journal"
        );
    }

    // Scenario: paramètre absent du script (clé inconnue) : erreur de script, classe NotFound
    #[tokio::test]
    async fn parametre_absent_est_une_erreur_de_script_code_1() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "absent.rhai", "let x = 1;\nparam(\"absent\")");

        let (issue, _sortie, erreur) =
            executer(home.path(), demande(fichier, &[], None, false)).await;

        assert_eq!(
            issue,
            Outcome::Failure,
            "une clé absente est une erreur de script"
        );
        assert!(
            erreur.contains("not found") && erreur.contains("absent"),
            "la classe NotFound nomme la clé, obtenu {erreur:?}"
        );
        assert!(
            erreur.contains("line 2"),
            "la position de l'appel à param est donnée, obtenu {erreur:?}"
        );
    }

    // Scenario: paramètre ressemblant à un secret
    #[tokio::test]
    async fn parametre_ressemblant_a_un_secret_refuse_avant_execution() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "secret.rhai", "\"ok\"");

        let (issue, sortie, erreur) = executer(
            home.path(),
            demande(fichier, &["api_key=sk-0123456789abcdef"], None, false),
        )
        .await;

        assert_eq!(
            issue,
            Outcome::Usage,
            "un secret en paramètre est refusé avec le code 2"
        );
        assert!(sortie.is_empty(), "le script n'est pas exécuté");
        assert_eq!(
            entrees_d_audit(home.path()),
            0,
            "aucune exécution n'est journalisée"
        );
        assert!(
            !erreur.contains("0123456789abcdef"),
            "le message d'erreur ne reprend pas la valeur, obtenu {erreur:?}"
        );
    }

    // Scenario: paramètre invalide ou répété
    #[tokio::test]
    async fn parametre_invalide_ou_repete_code_2() {
        for params in [
            vec!["1x=a"],
            vec!["a=1", "a=2"],
            vec!["sans-egal"],
            vec!["bonjour monde=a"],
        ] {
            let home = tempfile::tempdir().expect("répertoire personnel temporaire");
            let fichier = ecrire_script(home.path(), "param.rhai", "\"ok\"");
            let (issue, sortie, _erreur) =
                executer(home.path(), demande(fichier, &params, None, false)).await;

            assert_eq!(
                issue,
                Outcome::Usage,
                "les paramètres {params:?} sont une erreur d'usage"
            );
            assert!(sortie.is_empty(), "rien n'est exécuté pour {params:?}");
            assert_eq!(
                entrees_d_audit(home.path()),
                0,
                "aucune exécution journalisée pour {params:?}"
            );
        }
    }

    // Scenario: module non accordé
    #[tokio::test]
    async fn module_non_accorde_echoue_sans_effet() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "module.rhai", "memoire::lire(\"x\")");

        let (issue, sortie, _erreur) =
            executer(home.path(), demande(fichier, &[], None, false)).await;

        assert_eq!(
            issue,
            Outcome::Failure,
            "un module absent du contexte échoue avec 1"
        );
        assert!(sortie.is_empty(), "aucun effet visible");
    }

    // Scenario: échéance
    #[tokio::test]
    async fn echeance_annule_le_script_code_3() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "boucle.rhai", "loop {}");
        let debut = Instant::now();

        let (issue, _sortie, erreur) =
            executer(home.path(), demande(fichier, &[], Some(1), false)).await;

        assert_eq!(
            issue,
            Outcome::Timeout,
            "à l'échéance de -t, le script est annulé avec 3, erreur : {erreur:?}"
        );
        assert!(
            debut.elapsed().as_secs() < 30,
            "l'annulation intervient peu après l'échéance"
        );
    }

    // Scenario: sortie JSON, un seul objet
    #[tokio::test]
    async fn json_donne_un_seul_objet_sur_la_sortie_standard() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let fichier = ecrire_script(home.path(), "json.rhai", "\"ok\"");

        let (issue, sortie, _erreur) =
            executer(home.path(), demande(fichier, &[], None, true)).await;

        assert_eq!(issue, Outcome::Success);
        assert_eq!(
            sortie.lines().count(),
            1,
            "un seul objet, une ligne : {sortie:?}"
        );
        let objet: serde_json::Value =
            serde_json::from_str(sortie.trim()).expect("la sortie standard est un objet JSON");
        assert_eq!(objet["status"], "ok", "le statut du script");
        assert!(
            objet["text"]
                .as_str()
                .is_some_and(|texte| texte.contains("ok")),
            "le texte du script"
        );
    }

    // Scenario: fichier vide ou illisible
    #[tokio::test]
    async fn fichier_vide_ou_illisible_code_2() {
        let home = tempfile::tempdir().expect("répertoire personnel temporaire");
        let vide = ecrire_script(home.path(), "vide.rhai", "   \n");

        let (vide_issue, _sortie, _erreur) =
            executer(home.path(), demande(vide, &[], None, false)).await;
        let (absent_issue, _sortie, _erreur) = executer(
            home.path(),
            demande(home.path().join("inexistant.rhai"), &[], None, false),
        )
        .await;

        assert_eq!(
            vide_issue,
            Outcome::Usage,
            "un script vide est une erreur d'usage"
        );
        assert_eq!(
            absent_issue,
            Outcome::Usage,
            "un fichier illisible est une erreur d'usage"
        );
    }

    // Must: le nom est avant le premier `=`, la valeur garde les `=` suivants ; une valeur vide est permise.
    #[test]
    fn parse_param_coupe_au_premier_egal() {
        assert_eq!(
            parse_param("a=b=c"),
            Ok(("a".to_owned(), "b=c".to_owned())),
            "la valeur garde les `=` suivants"
        );
        assert_eq!(
            parse_param("cle="),
            Ok(("cle".to_owned(), String::new())),
            "une valeur vide est permise"
        );
    }

    // Must: une paire sans `=` est refusée, et l'erreur ne reprend pas l'entrée.
    #[test]
    fn parse_param_refuse_sans_egal_sans_reprendre_l_entree() {
        match parse_param("sk-secret-sans-egal") {
            Err(message) => assert!(
                !message.contains("sk-secret-sans-egal"),
                "l'erreur ne reprend pas l'entrée, obtenu {message:?}"
            ),
            autre => panic!("une paire sans `=` est refusée, obtenu {autre:?}"),
        }
    }
}
