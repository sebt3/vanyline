//! `vanyline sync push|pull` (`cli/src/sync/command.sdd`) : lit les deux arbres, construit le plan, l'imprime,
//! puis l'exécute fichier par fichier si rien ne diverge. Jamais de réconciliation automatique.

use std::io::Write;
use std::path::Path;

use clap::{Args, Subcommand};
use lymaille::DefaultSecretDetector;
use secrecy::SecretString;
use vanyline_lib::k8s::VnlK8sClient;
use vanyline_lib::sandbox_fs::{FsError, Listing, SandboxFs};

use super::manifest::Manifest;
use super::plan::{Action, Direction, Plan, PlanOptions, build_plan, find_secrets};
use super::tree::{DOSSIER, DistantFs, Entry, RelPath, Tree, read_local, read_remote};
use crate::chat::GlobalOptions;
use crate::exit_code::Outcome;
use crate::host::files;
use crate::host::layers::Layers;
use crate::host::owner_auth::OwnerTokenSource;
use crate::owner_cmd;

/// Options communes à `push` et `pull`.
#[derive(Args, Debug)]
pub struct Options {
    /// Projet auquel appartient la sandbox
    #[arg(long)]
    pub context: String,
    /// Sandbox dont le worktree porte le `.vanyline/` distant
    #[arg(long)]
    pub sandbox: String,
    /// Ne jamais supprimer dans la destination
    #[arg(long)]
    pub no_delete: bool,
    /// Afficher la liste des changements et s'arrêter sans rien écrire
    #[arg(long)]
    pub dry_run: bool,
    /// Ajouter le diff de chaque fichier à la liste
    #[arg(long)]
    pub diff: bool,
}

/// Grammaire de `vanyline sync` (`command.sdd`, `@Commands`).
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Copier le `.vanyline/` du dépôt vers la sandbox
    Push {
        #[command(flatten)]
        options: Options,
        /// Remplacer la copie de la sandbox en cas de divergence
        #[arg(long)]
        force_local: bool,
    },
    /// Copier le `.vanyline/` de la sandbox vers le dépôt
    Pull {
        #[command(flatten)]
        options: Options,
        /// Remplacer la copie locale en cas de divergence
        #[arg(long)]
        force_remote: bool,
    },
}

/// Paramètres résolus d'une synchronisation, sans la connexion.
// `command.sdd` (`@Commands`) : options de CLI, booléens indépendants par nature.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parametres {
    /// Sens de la synchronisation.
    pub direction: Direction,
    /// Sandbox (nom, pour le manifeste).
    pub sandbox: String,
    /// Trancher en faveur de la source en cas de divergence.
    pub force: bool,
    /// Ne jamais supprimer dans la destination.
    pub no_delete: bool,
    /// Arrêter après la liste.
    pub dry_run: bool,
    /// Diff de chaque fichier dans la liste.
    pub diff: bool,
}

/// Exécute `sync` : connexion à la sandbox, puis [`synchroniser`].
pub async fn run_sync(commande: Commands, global: &GlobalOptions) -> Outcome {
    let (direction, options, force) = match commande {
        Commands::Push {
            options,
            force_local,
        } => (Direction::Push, options, force_local),
        Commands::Pull {
            options,
            force_remote,
        } => (Direction::Pull, options, force_remote),
    };
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_default();
    let layers = Layers::discover(&home, &cwd, global.repo.as_deref(), Path::exists);
    let Some(local) = layers.shared().map(Path::to_path_buf) else {
        return usage(
            "VNL-CLI-SYN-001: aucun dépôt : `.vanyline/` introuvable (dépôt git ou --repo)",
        );
    };
    let depot = local.parent().unwrap_or(&local).to_path_buf();
    let connexion = match owner_cmd::connexion(global).await {
        Ok(connexion) => connexion,
        Err(issue) => return issue,
    };
    let owner = match owner_cmd::owner_de(global, &connexion.settings) {
        Ok(owner) => owner,
        Err(issue) => return issue,
    };
    let client: &VnlK8sClient = &connexion.client;
    let sandbox = match client.get_sandbox(&owner, &options.sandbox).await {
        Ok(sandbox) => sandbox,
        Err(erreur) => return echec_cluster(&erreur.to_string()),
    };
    if sandbox.spec.project != options.context {
        return usage(&format!(
            "VNL-CLI-SYN-003: la sandbox `{}` n'appartient pas au projet `{}`",
            options.sandbox, options.context
        ));
    }
    let url = match client.sandbox_ws_ticket_url(&owner, &options.sandbox).await {
        Ok(url) => url,
        Err(erreur) => return echec_cluster(&erreur.to_string()),
    };
    let Some(base) = url.strip_suffix("/ws/ticket") else {
        return echec_cluster("VNL-CLI-SYN-004: URL de la sandbox inattendue");
    };
    // Le jeton ne sort de cette fonction que sous forme de `SecretString` : jamais dans un message.
    let jeton = match OwnerTokenSource::owner_token(client, &owner).await {
        Ok(jeton) => SecretString::from(jeton.token),
        Err(_) => return echec_cluster("VNL-CLI-SYN-004: jeton d'Owner indisponible"),
    };
    let http = reqwest::Client::new();
    let mut distant = match SandboxFs::connect(base, &jeton, &http).await {
        Ok(distant) => distant,
        Err(erreur) => {
            return echec_cluster(&format!(
                "VNL-CLI-SYN-005: connexion à la sandbox impossible : {erreur}"
            ));
        }
    };
    let parametres = Parametres {
        direction,
        sandbox: options.sandbox.clone(),
        force,
        no_delete: options.no_delete,
        dry_run: options.dry_run,
        diff: options.diff,
    };
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    synchroniser(
        &parametres,
        &local,
        &depot,
        &layers,
        &mut distant,
        &mut out,
        &mut err,
    )
    .await
}

/// Issue d'usage : message sur la sortie d'erreur, code 2.
fn usage(message: &str) -> Outcome {
    ecrire(&mut std::io::stderr(), message);
    Outcome::Usage
}

/// Issue d'échec du cluster ou de la connexion : message sur la sortie d'erreur, code 1.
fn echec_cluster(message: &str) -> Outcome {
    ecrire(&mut std::io::stderr(), message);
    Outcome::Failure
}

/// Écrit une ligne ; un échec d'écriture de la sortie n'est pas une erreur de synchronisation.
fn ecrire(w: &mut dyn Write, texte: &str) {
    let _ = writeln!(w, "{texte}");
}

/// Synchronise `local` (le `.vanyline/` du dépôt `depot`) avec `.vanyline/` de la sandbox `distant`.
pub async fn synchroniser<D: DistantFs>(
    parametres: &Parametres,
    local: &Path,
    depot: &Path,
    layers: &Layers,
    distant: &mut D,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Outcome {
    let arbre_local = match read_local(local) {
        Ok(arbre) => arbre,
        Err(erreur) => return echec_lecture(err, &erreur),
    };
    let arbre_distant = match read_remote(distant).await {
        Ok(arbre) => arbre,
        Err(erreur) => return echec_lecture(err, &erreur),
    };
    let manifeste = match Manifest::load(layers, depot, &parametres.sandbox) {
        Ok(manifeste) => manifeste,
        Err(erreur) => {
            ecrire(
                err,
                &format!("VNL-CLI-SYN-006: manifeste illisible : {erreur}"),
            );
            return Outcome::Failure;
        }
    };
    let (source, destination) = match parametres.direction {
        Direction::Push => (&arbre_local, &arbre_distant),
        Direction::Pull => (&arbre_distant, &arbre_local),
    };
    // Contrôle avant toute écriture : une valeur ressemblant à un secret fait refuser l'ensemble (`sync#FR-6`).
    let secrets = find_secrets(source, &DefaultSecretDetector::default());
    if !secrets.is_empty() {
        let liste: Vec<String> = secrets.iter().map(ToString::to_string).collect();
        ecrire(
            err,
            &format!(
                "VNL-CLI-SYN-007: valeur ressemblant à un secret dans {} : aucune écriture",
                liste.join(", ")
            ),
        );
        return Outcome::Failure;
    }
    let plan = build_plan(
        parametres.direction,
        source,
        destination,
        &manifeste,
        PlanOptions {
            force: parametres.force,
            no_delete: parametres.no_delete,
        },
    );
    afficher_plan(out, &plan, source, destination, parametres.diff);
    if plan.has_divergence() && !parametres.force {
        let nombre = plan
            .actions
            .values()
            .filter(|a| **a == Action::Diverged)
            .count();
        ecrire(
            err,
            &format!(
                "VNL-CLI-SYN-008: divergence sur {nombre} fichier(s) : aucune écriture. \
                 --force-local (push) ou --force-remote (pull) tranche en faveur de la source"
            ),
        );
        return Outcome::Failure;
    }
    if parametres.dry_run {
        return Outcome::Success;
    }
    executer(
        parametres,
        &plan,
        source,
        destination,
        local,
        depot,
        layers,
        distant,
        manifeste,
        out,
        err,
    )
    .await
}

/// Erreur de lecture d'un arbre : message sur la sortie d'erreur, échec (aucune écriture n'a eu lieu).
fn echec_lecture(err: &mut dyn Write, erreur: &super::tree::TreeError) -> Outcome {
    ecrire(err, &erreur.to_string());
    Outcome::Failure
}

/// Affiche le plan (toujours la liste, `sync#FR-10`), puis le diff des fichiers divergents, et celui de chaque
/// fichier changé si `diff`.
fn afficher_plan(out: &mut dyn Write, plan: &Plan, source: &Tree, destination: &Tree, diff: bool) {
    if plan.is_empty() {
        ecrire(out, "rien à synchroniser");
        return;
    }
    ecrire(out, &plan.summary());
    for (chemin, action) in &plan.actions {
        let montrer = match action {
            Action::Diverged => true,
            Action::Create | Action::Update | Action::Delete => diff,
            Action::Nothing => false,
        };
        if !montrer {
            continue;
        }
        let ancien = destination.get(chemin).map(texte_de);
        let nouveau = source.get(chemin).map(texte_de);
        ecrire(out, &format!("--- {chemin} (destination)"));
        ecrire(out, &format!("+++ {chemin} (source)"));
        for ligne in diff_lignes(
            ancien.as_deref().unwrap_or(""),
            nouveau.as_deref().unwrap_or(""),
        ) {
            ecrire(out, &ligne);
        }
    }
}

/// Texte d'une entrée (UTF-8 garanti par la lecture de l'arbre).
fn texte_de(entree: &Entry) -> String {
    String::from_utf8_lossy(&entree.octets).into_owned()
}

/// Exécute le plan fichier par fichier. Le manifeste est sauvegardé après chaque copie réussie (`FR-9`).
#[allow(clippy::too_many_arguments)]
async fn executer<D: DistantFs>(
    parametres: &Parametres,
    plan: &Plan,
    source: &Tree,
    destination: &Tree,
    local: &Path,
    depot: &Path,
    layers: &Layers,
    distant: &mut D,
    mut manifeste: Manifest,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Outcome {
    let mut copies = 0_usize;
    let mut suppressions = 0_usize;
    for (chemin, action) in &plan.actions {
        match action {
            Action::Nothing => {
                // Deux copies identiques : le manifeste les reconnaît (aucune écriture).
                if let (Some(src), Some(dst)) = (source.get(chemin), destination.get(chemin))
                    && src.sha256 == dst.sha256
                {
                    manifeste.record(chemin.clone(), src.sha256);
                }
                continue;
            }
            Action::Diverged => continue,
            Action::Create | Action::Update => {
                let Some(entree) = source.get(chemin) else {
                    continue;
                };
                if let Err(erreur) =
                    ecrire_destination(parametres.direction, local, distant, chemin, &entree.octets)
                        .await
                {
                    ecrire(err, &erreur);
                    return Outcome::Failure;
                }
                manifeste.record(chemin.clone(), entree.sha256);
                copies = copies.saturating_add(1);
            }
            Action::Delete => {
                if let Err(erreur) =
                    supprimer_destination(parametres.direction, local, distant, chemin).await
                {
                    ecrire(err, &erreur);
                    return Outcome::Failure;
                }
                manifeste.forget(chemin);
                suppressions = suppressions.saturating_add(1);
            }
        }
        if let Err(erreur) = manifeste.save(layers, depot, &parametres.sandbox) {
            ecrire(
                err,
                &format!("VNL-CLI-SYN-006: manifeste non sauvegardé : {erreur}"),
            );
            return Outcome::Failure;
        }
    }
    if let Err(erreur) = manifeste.save(layers, depot, &parametres.sandbox) {
        ecrire(
            err,
            &format!("VNL-CLI-SYN-006: manifeste non sauvegardé : {erreur}"),
        );
        return Outcome::Failure;
    }
    ecrire(
        out,
        &format!("synchronisé : {copies} fichier(s) copié(s), {suppressions} supprimé(s)"),
    );
    Outcome::Success
}

/// Écrit `octets` à `chemin` du côté destination (sandbox pour `push`, dépôt pour `pull`).
async fn ecrire_destination<D: DistantFs>(
    sens: Direction,
    local: &Path,
    distant: &mut D,
    chemin: &RelPath,
    octets: &[u8],
) -> Result<(), String> {
    match sens {
        Direction::Push => distant
            .write(&format!("{DOSSIER}/{chemin}"), octets)
            .await
            .map_err(|erreur| format!("VNL-CLI-SYN-009: écriture distante `{chemin}` : {erreur}")),
        Direction::Pull => files::write_atomic(&local.join(chemin.as_str()), octets, 0o644)
            .map_err(|erreur| format!("VNL-CLI-SYN-009: écriture locale `{chemin}` : {erreur}")),
    }
}

/// Supprime `chemin` du côté destination.
async fn supprimer_destination<D: DistantFs>(
    sens: Direction,
    local: &Path,
    distant: &mut D,
    chemin: &RelPath,
) -> Result<(), String> {
    match sens {
        Direction::Push => distant
            .delete(&format!("{DOSSIER}/{chemin}"))
            .await
            .map(|_| ())
            .map_err(|erreur| {
                format!("VNL-CLI-SYN-009: suppression distante `{chemin}` : {erreur}")
            }),
        Direction::Pull => files::remove_file(&local.join(chemin.as_str()))
            .map(|_| ())
            .map_err(|erreur| format!("VNL-CLI-SYN-009: suppression locale `{chemin}` : {erreur}")),
    }
}

impl DistantFs for SandboxFs {
    async fn list(&mut self, chemin: &str) -> Result<Listing, FsError> {
        SandboxFs::list(self, chemin).await
    }

    async fn read_raw(&mut self, chemin: &str) -> Result<Vec<u8>, FsError> {
        SandboxFs::read_raw(self, chemin).await
    }

    async fn write(&mut self, chemin: &str, octets: &[u8]) -> Result<(), FsError> {
        SandboxFs::write(self, chemin, octets).await
    }

    async fn delete(&mut self, chemin: &str) -> Result<bool, FsError> {
        SandboxFs::delete(self, chemin).await
    }
}

/// Diff ligne à ligne (`-` pour l'ancien, `+` pour le nouveau, ` ` pour le commun) de `ancien` vers `nouveau`.
/// Au-delà de 2 000 000 de cases de la table, le diff est remplacé par une ligne d'information (`sync#FR-10`).
#[must_use]
pub fn diff_lignes(ancien: &str, nouveau: &str) -> Vec<String> {
    /// Taille maximale de la table LCS (lignes de l'ancien × lignes du nouveau).
    const LIMITE_CASES: usize = 2_000_000;
    let lignes_ancien: Vec<&str> = ancien.lines().collect();
    let lignes_nouveau: Vec<&str> = nouveau.lines().collect();
    let (nb_ancien, nb_nouveau) = (lignes_ancien.len(), lignes_nouveau.len());
    if nb_ancien.saturating_mul(nb_nouveau) > LIMITE_CASES {
        return vec![format!("(diff omis : {nb_ancien} et {nb_nouveau} lignes)")];
    }
    // table[(i, j)] = longueur de la plus longue sous-suite commune de ancien[i..] et nouveau[j..], à plat.
    let largeur = nb_nouveau.saturating_add(1);
    let case = |i: usize, j: usize| i.saturating_mul(largeur).saturating_add(j);
    let mut table = vec![0_usize; nb_ancien.saturating_add(1).saturating_mul(largeur)];
    for i in (0..nb_ancien).rev() {
        for j in (0..nb_nouveau).rev() {
            let valeur = if lignes_ancien[i] == lignes_nouveau[j] {
                table[case(i.saturating_add(1), j.saturating_add(1))].saturating_add(1)
            } else {
                table[case(i.saturating_add(1), j)].max(table[case(i, j.saturating_add(1))])
            };
            table[case(i, j)] = valeur;
        }
    }
    let mut sortie = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < nb_ancien && j < nb_nouveau {
        if lignes_ancien[i] == lignes_nouveau[j] {
            sortie.push(format!(" {}", lignes_ancien[i]));
            i = i.saturating_add(1);
            j = j.saturating_add(1);
        } else if table[case(i.saturating_add(1), j)] >= table[case(i, j.saturating_add(1))] {
            sortie.push(format!("-{}", lignes_ancien[i]));
            i = i.saturating_add(1);
        } else {
            sortie.push(format!("+{}", lignes_nouveau[j]));
            j = j.saturating_add(1);
        }
    }
    for ligne in lignes_ancien.iter().skip(i) {
        sortie.push(format!("-{ligne}"));
    }
    for ligne in lignes_nouveau.iter().skip(j) {
        sortie.push(format!("+{ligne}"));
    }
    sortie
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use vanyline_lib::sandbox_fs::{FsError, Listing};

    use super::*;
    use crate::sync::manifest::chemin_manifeste;
    use crate::sync::tree::empreinte;
    use crate::sync::tree::essai::FauxDistant;

    /// Dépôt temporaire : `$HOME` et `repo` sous un même dossier ; `local` = `repo/.vanyline`.
    struct Banc {
        _home: tempfile::TempDir,
        repo: PathBuf,
        local: PathBuf,
        layers: Layers,
    }

    fn banc() -> Banc {
        let home = tempfile::tempdir().expect("tempdir");
        let repo = home.path().join("depot");
        std::fs::create_dir_all(&repo).expect("dépôt");
        let layers = Layers::discover(home.path(), home.path(), Some(&repo), |_| false);
        let local = repo.join(DOSSIER);
        Banc {
            _home: home,
            repo,
            local,
            layers,
        }
    }

    fn parametres(direction: Direction) -> Parametres {
        Parametres {
            direction,
            sandbox: "alpha".to_owned(),
            force: false,
            no_delete: false,
            dry_run: false,
            diff: false,
        }
    }

    fn ecrire_local(banc: &Banc, chemin: &str, texte: &str) {
        let cible = banc.local.join(chemin);
        std::fs::create_dir_all(cible.parent().expect("parent")).expect("dossier");
        std::fs::write(cible, texte).expect("écriture locale");
    }

    /// Lance `synchroniser` ; rend l'issue, la sortie standard et la sortie d'erreur.
    async fn lancer<D: DistantFs>(
        p: &Parametres,
        banc: &Banc,
        distant: &mut D,
    ) -> (Outcome, String, String) {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let issue = synchroniser(
            p,
            &banc.local,
            &banc.repo,
            &banc.layers,
            distant,
            &mut out,
            &mut err,
        )
        .await;
        (
            issue,
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )
    }

    /// Sandbox qui échoue à l'écriture d'un chemin donné (coupure simulée).
    struct Panne {
        interne: FauxDistant,
        en_panne: Option<String>,
    }

    // Faux sans E/S : `async` imposé par le trait `DistantFs` (`sync/command.sdd`).
    #[allow(clippy::unused_async_trait_impl)]
    impl DistantFs for Panne {
        async fn list(&mut self, chemin: &str) -> Result<Listing, FsError> {
            self.interne.list(chemin).await
        }
        async fn read_raw(&mut self, chemin: &str) -> Result<Vec<u8>, FsError> {
            self.interne.read_raw(chemin).await
        }
        async fn write(&mut self, chemin: &str, octets: &[u8]) -> Result<(), FsError> {
            if self.en_panne.as_deref() == Some(chemin) {
                return Err(FsError::Transport);
            }
            self.interne.write(chemin, octets).await
        }
        async fn delete(&mut self, chemin: &str) -> Result<bool, FsError> {
            self.interne.delete(chemin).await
        }
    }

    fn rel(texte: &str) -> RelPath {
        RelPath::new(texte).expect("chemin")
    }

    /// Scenario `push sur une sandbox vierge` (`AC-1`) : octets exacts, saut de ligne final compris.
    #[tokio::test]
    async fn push_sandbox_vierge() {
        let banc = banc();
        ecrire_local(&banc, "agents/a.md", "x\n\ny\n");
        ecrire_local(&banc, "config.yaml", "k: v\n");
        let mut distant = FauxDistant::default();
        let (issue, _, _) = lancer(&parametres(Direction::Push), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert_eq!(
            distant
                .fichiers
                .get(".vanyline/agents/a.md")
                .map(Vec::as_slice),
            Some(&b"x\n\ny\n"[..])
        );
        assert_eq!(
            distant
                .fichiers
                .get(".vanyline/config.yaml")
                .map(Vec::as_slice),
            Some(&b"k: v\n"[..])
        );
        let manifeste = Manifest::load(&banc.layers, &banc.repo, "alpha").expect("manifeste");
        assert_eq!(
            manifeste.get(&rel("agents/a.md")),
            Some(&empreinte(b"x\n\ny\n"))
        );
    }

    /// Scenario `divergence` (`AC-2`) : rien n'est écrit sans force ; `--force-local` remplace la copie distante.
    #[tokio::test]
    async fn divergence_sans_force_n_ecrit_rien() {
        let banc = banc();
        ecrire_local(&banc, "a.md", "local\n");
        let mut ancien = Manifest::new();
        ancien.record(rel("a.md"), empreinte(b"ancien\n"));
        ancien
            .save(&banc.layers, &banc.repo, "alpha")
            .expect("manifeste");
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("a.md", false)]);
        distant
            .fichiers
            .insert(".vanyline/a.md".to_owned(), b"distant\n".to_vec());

        let (issue, out, err) = lancer(&parametres(Direction::Push), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Failure);
        assert_eq!(
            distant.fichiers.get(".vanyline/a.md").map(Vec::as_slice),
            Some(&b"distant\n"[..])
        );
        assert!(
            format!("{out}{err}").contains("a.md"),
            "sortie : {out}{err}"
        );

        let mut force = parametres(Direction::Push);
        force.force = true;
        let (issue, _, _) = lancer(&force, &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert_eq!(
            distant.fichiers.get(".vanyline/a.md").map(Vec::as_slice),
            Some(&b"local\n"[..])
        );
    }

    /// Le diff d'un fichier divergent est montré, sans écriture ni `--diff`.
    #[tokio::test]
    async fn divergence_montre_le_diff() {
        let banc = banc();
        ecrire_local(&banc, "a.md", "local\n");
        let mut ancien = Manifest::new();
        ancien.record(rel("a.md"), empreinte(b"ancien\n"));
        ancien
            .save(&banc.layers, &banc.repo, "alpha")
            .expect("manifeste");
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("a.md", false)]);
        distant
            .fichiers
            .insert(".vanyline/a.md".to_owned(), b"distant\n".to_vec());
        let (issue, out, err) = lancer(&parametres(Direction::Push), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Failure);
        let sortie = format!("{out}{err}");
        assert!(sortie.contains("-distant"), "sortie : {sortie}");
        assert!(sortie.contains("+local"), "sortie : {sortie}");
    }

    /// Scenario `suppression et --no-delete` (`AC-3`).
    #[tokio::test]
    async fn suppression_puis_no_delete() {
        let banc = banc();
        let mut ancien = Manifest::new();
        ancien.record(rel("gone.md"), empreinte(b"x"));
        ancien
            .save(&banc.layers, &banc.repo, "alpha")
            .expect("manifeste");
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("gone.md", false)]);
        distant
            .fichiers
            .insert(".vanyline/gone.md".to_owned(), b"x".to_vec());

        let mut garde = parametres(Direction::Push);
        garde.no_delete = true;
        let (issue, _, _) = lancer(&garde, &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert!(distant.fichiers.contains_key(".vanyline/gone.md"));

        let (issue, _, _) = lancer(&parametres(Direction::Push), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert!(!distant.fichiers.contains_key(".vanyline/gone.md"));
        let manifeste = Manifest::load(&banc.layers, &banc.repo, "alpha").expect("manifeste");
        assert!(manifeste.get(&rel("gone.md")).is_none());
    }

    /// Scenario `secret détecté` (`AC-4`) : une valeur ressemblant à une clé fait refuser tout le push.
    #[tokio::test]
    async fn secret_refuse_tout_sans_ecriture() {
        let banc = banc();
        ecrire_local(&banc, "a.md", "ok\n");
        ecrire_local(&banc, "b.md", "sk-abcdef0123456789\n");
        let mut distant = FauxDistant::default();
        let (issue, _, err) = lancer(&parametres(Direction::Push), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Failure);
        assert!(err.contains("b.md"), "sortie : {err}");
        assert!(distant.fichiers.is_empty(), "rien ne doit être écrit");
        assert!(!chemin_manifeste(&banc.layers, &banc.repo, "alpha").exists());
    }

    /// Scenario `chemin hostile au pull` (`AC-5`) : refus avant toute écriture locale.
    #[tokio::test]
    async fn pull_chemin_hostile_refuse() {
        let banc = banc();
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("../x", false)]);
        let (issue, _, err) = lancer(&parametres(Direction::Pull), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Failure);
        assert!(err.contains("../x"), "sortie : {err}");
        assert!(!banc.local.exists(), "aucune écriture locale");
    }

    /// Listage distant tronqué (`AC-6`) : le pull échoue au lieu de copier un arbre partiel.
    #[tokio::test]
    async fn pull_listage_tronque_echoue() {
        let banc = banc();
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("a.md", false)]);
        if let Some(l) = distant.listages.get_mut(DOSSIER) {
            l.tronque = true;
        }
        distant
            .fichiers
            .insert(".vanyline/a.md".to_owned(), b"a".to_vec());
        let (issue, _, err) = lancer(&parametres(Direction::Pull), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Failure);
        assert!(err.contains("VNL-SYNC-004"), "sortie : {err}");
        assert!(!banc.local.exists());
    }

    /// Scenario `coupure` (`AC-7`) : un push interrompu laisse les fichiers entiers et le manifeste à jour ;
    /// le push suivant reprend sans divergence.
    #[tokio::test]
    async fn coupure_puis_reprise() {
        let banc = banc();
        ecrire_local(&banc, "a.md", "1\n");
        ecrire_local(&banc, "b.md", "2\n");
        ecrire_local(&banc, "c.md", "3\n");
        let mut panne = Panne {
            interne: FauxDistant::default(),
            en_panne: Some(".vanyline/b.md".to_owned()),
        };
        let (issue, _, _) = lancer(&parametres(Direction::Push), &banc, &mut panne).await;
        assert_eq!(issue, Outcome::Failure);
        assert_eq!(panne.interne.fichiers.len(), 1, "seul a.md est copié");
        let manifeste = Manifest::load(&banc.layers, &banc.repo, "alpha").expect("manifeste");
        assert_eq!(manifeste.chemins().count(), 1);
        assert!(manifeste.get(&rel("a.md")).is_some());

        panne.en_panne = None;
        let (issue, out, _) = lancer(&parametres(Direction::Push), &banc, &mut panne).await;
        assert_eq!(issue, Outcome::Success);
        assert!(!out.contains("DIVERGE"), "sortie : {out}");
        assert_eq!(panne.interne.fichiers.len(), 3);
    }

    /// Diff : lignes communes en espace, retirées en `-`, ajoutées en `+`, dans l'ordre du fichier.
    #[test]
    fn diff_lignes_marque_les_changements() {
        let diff = diff_lignes("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(diff, vec![" a", "-b", "+B", " c", "+d"]);
        assert!(diff_lignes("x\n", "x\n").iter().all(|l| l.starts_with(' ')));
    }

    /// Scenario `dry-run` (`AC-8`) : la liste est imprimée, aucun fichier n'est écrit ; sans `--dry-run`, la même
    /// liste précède l'écriture.
    #[tokio::test]
    async fn dry_run_n_ecrit_rien() {
        let banc = banc();
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("a.md", false)]);
        distant
            .fichiers
            .insert(".vanyline/a.md".to_owned(), b"dist\n".to_vec());

        let mut essai = parametres(Direction::Pull);
        essai.dry_run = true;
        let (issue, out, _) = lancer(&essai, &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert!(out.contains("a.md"), "sortie : {out}");
        assert!(!banc.local.join("a.md").exists());

        let (issue, out_reel, _) = lancer(&parametres(Direction::Pull), &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert!(out_reel.contains("a.md"), "sortie : {out_reel}");
        assert_eq!(
            std::fs::read(banc.local.join("a.md")).expect("local"),
            b"dist\n"
        );
    }

    /// `--diff` ajoute le diff des créations à la liste ; `--dry-run` sur une divergence échoue quand même.
    #[tokio::test]
    async fn diff_et_divergence_en_dry_run() {
        let banc = banc();
        let mut distant = FauxDistant::default();
        distant.dossier(DOSSIER, &[("a.md", false)]);
        distant
            .fichiers
            .insert(".vanyline/a.md".to_owned(), b"dist\n".to_vec());
        let mut essai = parametres(Direction::Pull);
        essai.dry_run = true;
        essai.diff = true;
        let (issue, out, _) = lancer(&essai, &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Success);
        assert!(out.contains("+dist"), "sortie : {out}");

        // Divergence en dry-run : échec (le push réel échouerait aussi).
        let mut ancien = Manifest::new();
        ancien.record(rel("a.md"), empreinte(b"vieux\n"));
        ancien
            .save(&banc.layers, &banc.repo, "alpha")
            .expect("manifeste");
        ecrire_local(&banc, "a.md", "local\n");
        let mut essai_push = parametres(Direction::Push);
        essai_push.dry_run = true;
        let (issue, _, _) = lancer(&essai_push, &banc, &mut distant).await;
        assert_eq!(issue, Outcome::Failure);
    }
}
