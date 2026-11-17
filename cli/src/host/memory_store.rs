//! Mémoires d'agent sur des fichiers Markdown, par portée. Contrat : `host/memory_store.sdd`.

use std::future::{self, Future};
use std::path::{Path, PathBuf};

use lymaille::{
    Memory, MemoryLevel, MemoryScope, MemoryStore, MemorySummary, ModulesError, ModulesResult,
};
use serde::{Deserialize, Serialize};

use super::HostError;
use super::catalog_store::check_name;
use super::files;
use super::layers::{Category, Layers, Level};

/// Extension des mémoires (Markdown avec frontmatter YAML).
const EXT: &str = "md";
/// Ligne qui ouvre et ferme le frontmatter.
const FENCE: &str = "---";
/// Droits d'une mémoire du niveau partagé : lisible dans le dépôt.
const MODE_PARTAGE: u32 = 0o644;
/// Droits d'une mémoire du niveau utilisateur : privée.
const MODE_PRIVE: u32 = 0o600;

/// Marqueur d'une portée de session, qui n'est pas rangée dans `memories/`.
const SESSION: &str = "VNL-CLI-HST-031";
/// Marqueur d'un fichier de mémoire qui n'a pas la forme attendue.
const FORMAT: &str = "VNL-CLI-HST-032";
/// Marqueur d'une portée de niveau partagé hors dépôt.
const NO_SHARED: &str = "VNL-CLI-HST-033";
/// Marqueur d'une sérialisation impossible.
const ENCODE: &str = "VNL-CLI-HST-034";

/// Mémoires d'agent rangées sous `memories/<référence>/<id>.md` du niveau de la portée. Ne crée rien à la construction.
#[derive(Debug, Clone)]
pub struct FileMemories {
    layers: Layers,
}

/// Frontmatter écrit : l'identifiant et le titre. Le corps suit, tel quel.
#[derive(Serialize)]
struct EnteteEcrite<'a> {
    id: &'a str,
    title: &'a str,
}

/// Frontmatter lu. Un champ manquant ou non textuel rend un refus de format.
#[derive(Deserialize)]
struct EnteteLue {
    id: String,
    title: String,
}

impl FileMemories {
    /// Stockage rangé sous les niveaux de `layers`.
    #[must_use]
    pub fn new(layers: &Layers) -> Self {
        Self {
            layers: layers.clone(),
        }
    }

    /// Écrit une mémoire dans une portée, en remplaçant celle de même identifiant.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused scope, reference or identifier or a missing shared level,
    /// [`HostError::Io`] for the write.
    pub fn put_at(&self, portee: &MemoryScope, memoire: &Memory) -> Result<(), HostError> {
        check_name(memoire.id())?;
        let dossier = self.dossier(portee)?;
        let texte = render(memoire)?;
        let mode = match portee.level() {
            MemoryLevel::Shared => MODE_PARTAGE,
            _ => MODE_PRIVE,
        };
        files::write_atomic(
            &dossier.join(format!("{}.{EXT}", memoire.id())),
            texte.as_bytes(),
            mode,
        )
    }

    /// Lit les résumés d'une portée, triés par identifiant, sans les corps.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused scope, [`HostError::Format`] for a memory file that is not readable,
    /// [`HostError::Io`] for the reading.
    pub fn list_at(&self, portee: &MemoryScope) -> Result<Vec<MemorySummary>, HostError> {
        let dossier = self.dossier(portee)?;
        let mut resumes = Vec::new();
        for entree in files::list_files(&dossier, EXT) {
            let chemin = entree?;
            let memoire = lire_fichier(&chemin)?;
            resumes.push(MemorySummary::from(&memoire));
        }
        resumes.sort_by(|a, b| a.id().cmp(b.id()));
        Ok(resumes)
    }

    /// Lit la mémoire de la portée qui porte `id`, `None` si elle n'existe pas.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused scope or identifier, [`HostError::Format`] or [`HostError::Io`] for
    /// a file that cannot be read.
    pub fn get_at(&self, portee: &MemoryScope, id: &str) -> Result<Option<Memory>, HostError> {
        check_name(id)?;
        let chemin = self.dossier(portee)?.join(format!("{id}.{EXT}"));
        if !chemin.exists() {
            return Ok(None);
        }
        lire_fichier(&chemin).map(Some)
    }

    /// Supprime la mémoire de la portée qui porte `id`. `true` si elle existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused scope or identifier, [`HostError::Io`] for the removal.
    pub fn delete_at(&self, portee: &MemoryScope, id: &str) -> Result<bool, HostError> {
        check_name(id)?;
        files::remove_file(&self.dossier(portee)?.join(format!("{id}.{EXT}")))
    }

    /// Répertoire d'une portée : `memories/<référence>/` du niveau de la portée. Refuse une portée de session, un niveau
    /// partagé absent et une référence qui sortirait du répertoire.
    fn dossier(&self, portee: &MemoryScope) -> Result<PathBuf, HostError> {
        let niveau = match portee.level() {
            MemoryLevel::User => Level::User,
            MemoryLevel::Shared => Level::Shared,
            _ => {
                return Err(HostError::Invalid {
                    cause: format!(
                        "{SESSION}: a session memory is kept in the session state, not in memories/"
                    ),
                });
            }
        };
        check_name(portee.reference())?;
        let racine =
            self.layers
                .dir(niveau, Category::Memories)
                .ok_or_else(|| HostError::Invalid {
                    cause: format!("{NO_SHARED}: no shared level outside a repository"),
                })?;
        Ok(racine.join(portee.reference()))
    }
}

/// Texte d'une mémoire : frontmatter YAML (identifiant, titre), puis le corps exact.
fn render(memoire: &Memory) -> Result<String, HostError> {
    let mut entete = yaml_serde::to_string(&EnteteEcrite {
        id: memoire.id(),
        title: memoire.title(),
    })
    .map_err(|erreur| HostError::Invalid {
        cause: format!("{ENCODE}: {erreur}"),
    })?;
    if !entete.ends_with('\n') {
        entete.push('\n');
    }
    Ok(format!("{FENCE}\n{entete}{FENCE}\n{}", memoire.body()))
}

/// Sépare un texte de son frontmatter : le texte entre les deux lignes `---`, puis le corps. `None` si la forme manque.
fn split_fence(texte: &str) -> Option<(&str, &str)> {
    let suite = texte.strip_prefix("---\n")?;
    let mut reste = suite;
    loop {
        let (ligne, apres) = reste.split_once('\n').unwrap_or((reste, ""));
        if ligne == FENCE {
            let debut = suite.len().saturating_sub(reste.len());
            return suite.get(..debut).map(|entete| (entete, apres));
        }
        if !reste.contains('\n') {
            return None;
        }
        reste = apres;
    }
}

/// Lit une mémoire d'un fichier. Une erreur nomme le fichier, jamais le corps.
fn lire_fichier(chemin: &Path) -> Result<Memory, HostError> {
    let texte = files::read_text(chemin)?.ok_or_else(|| HostError::Io {
        chemin: chemin.to_path_buf(),
        cause: format!("{FORMAT}: the memory file disappeared"),
    })?;
    let refus = |cause: String| HostError::Format {
        chemin: chemin.to_path_buf(),
        cause: format!("{FORMAT}: {cause}"),
    };
    let (entete, corps) = split_fence(&texte).ok_or_else(|| {
        refus("the frontmatter is missing or is not closed by a `---` line".to_owned())
    })?;
    let lue: EnteteLue = yaml_serde::from_str(entete).map_err(|_| {
        refus("the frontmatter is not readable YAML with an id and a title".to_owned())
    })?;
    Memory::new(&lue.id, &lue.title, corps).map_err(|erreur| refus(erreur.to_string()))
}

/// Erreur de store de la bibliothèque, pour un refus de l'hôte.
fn store(erreur: &HostError) -> ModulesError {
    ModulesError::Store(erreur.to_string())
}

impl MemoryStore for FileMemories {
    fn list(
        &self,
        scope: &MemoryScope,
    ) -> impl Future<Output = ModulesResult<Vec<MemorySummary>>> + Send {
        future::ready(self.list_at(scope).map_err(|erreur| store(&erreur)))
    }

    fn get(
        &self,
        scope: &MemoryScope,
        id: &str,
    ) -> impl Future<Output = ModulesResult<Option<Memory>>> + Send {
        future::ready(self.get_at(scope, id).map_err(|erreur| store(&erreur)))
    }

    fn put(
        &self,
        scope: &MemoryScope,
        memory: &Memory,
    ) -> impl Future<Output = ModulesResult<()>> + Send {
        future::ready(self.put_at(scope, memory).map_err(|erreur| store(&erreur)))
    }

    fn delete(
        &self,
        scope: &MemoryScope,
        id: &str,
    ) -> impl Future<Output = ModulesResult<bool>> + Send {
        future::ready(self.delete_at(scope, id).map_err(|erreur| store(&erreur)))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use lymaille::{Memory, MemoryScope, MemoryStore as _, ModulesError, SessionId};

    use super::{FileMemories, HostError};
    use crate::host::layers::Layers;

    fn sans_depot(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    fn avec_depot(home: &Path, racine: &Path) -> Layers {
        let git = racine.join(".git");
        Layers::discover(home, racine, None, |chemin| chemin == git)
    }

    fn memoire(id: &str, titre: &str, corps: &str) -> Memory {
        Memory::new(id, titre, corps).expect("a valid memory must be built")
    }

    #[tokio::test]
    async fn memory_round_trips_with_its_summary_and_its_file() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileMemories::new(&sans_depot(home.path()));
        let portee = MemoryScope::user("agent-x");
        let original = memoire("notes-1", "Titre court", "Corps\n\nligne deux\n");

        stock
            .put(&portee, &original)
            .await
            .expect("a memory in the user scope must be stored");

        let resumes = stock.list(&portee).await.expect("the listing must succeed");
        assert_eq!(resumes.len(), 1, "one memory must be listed");
        assert_eq!(
            resumes[0].id(),
            "notes-1",
            "the summary must carry the identifier"
        );
        assert_eq!(
            resumes[0].title(),
            "Titre court",
            "the summary must carry the title"
        );
        assert_eq!(
            stock
                .get(&portee, "notes-1")
                .await
                .expect("the lookup must succeed"),
            Some(original),
            "the memory read back must equal the one stored"
        );
        let fichier = home
            .path()
            .join(".config/vanyline/memories/agent-x/notes-1.md");
        let texte = fs::read_to_string(&fichier).expect("the memory must be a markdown file");
        assert!(
            texte.starts_with("---\n"),
            "the file must open with a frontmatter: {texte}"
        );
    }

    #[tokio::test]
    async fn delete_reports_true_then_false() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileMemories::new(&sans_depot(home.path()));
        let portee = MemoryScope::user("agent-x");
        stock
            .put(&portee, &memoire("a", "Un", "corps"))
            .await
            .expect("the memory must be stored");

        assert!(
            stock
                .delete(&portee, "a")
                .await
                .expect("the first removal must succeed"),
            "the first removal must report that the memory existed"
        );
        assert!(
            !stock
                .delete(&portee, "a")
                .await
                .expect("the second removal must succeed"),
            "the second removal must report that nothing existed"
        );
    }

    #[tokio::test]
    async fn shared_scope_without_repository_is_refused_and_writes_nothing() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileMemories::new(&sans_depot(home.path()));

        let resultat = stock.put_at(&MemoryScope::shared("projet"), &memoire("a", "Un", "corps"));

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "a shared scope outside a repository must be an explicit refusal, got {resultat:?}"
        );
        assert!(
            !home.path().join(".config/vanyline/memories").exists(),
            "no file must be written for a refused scope"
        );
    }

    #[tokio::test]
    async fn shared_scope_is_written_in_the_repository() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let depot = tempfile::tempdir().expect("a temporary repository must be created");
        let stock = FileMemories::new(&avec_depot(home.path(), depot.path()));
        let portee = MemoryScope::shared("projet");

        stock
            .put(&portee, &memoire("partage", "Partagee", "corps"))
            .await
            .expect("the shared scope must be stored in the repository");

        assert!(
            depot
                .path()
                .join(".vanyline/memories/projet/partage.md")
                .is_file(),
            "a shared memory must be a file under the repository's .vanyline"
        );
    }

    #[tokio::test]
    async fn session_scope_and_hostile_references_are_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileMemories::new(&sans_depot(home.path()));
        let session = SessionId::new("s-1").expect("a session identifier must be built");

        assert!(
            stock
                .put(
                    &MemoryScope::session(&session),
                    &memoire("a", "Un", "corps")
                )
                .await
                .is_err(),
            "a session scope is kept in the session state, never in memories/"
        );
        assert!(
            stock
                .put(&MemoryScope::user("../x"), &memoire("a", "Un", "corps"))
                .await
                .is_err(),
            "a reference that leaves its directory must be refused"
        );
        assert!(
            stock
                .get(&MemoryScope::user("agent-x"), "../a")
                .await
                .is_err(),
            "an identifier that leaves its directory must be refused"
        );
        assert!(
            !home.path().join(".config/vanyline/x").exists(),
            "no file must be written outside memories/"
        );
    }

    #[tokio::test]
    async fn a_broken_memory_file_fails_the_listing_with_its_path() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileMemories::new(&sans_depot(home.path()));
        let portee = MemoryScope::user("agent-x");
        let dossier = home.path().join(".config/vanyline/memories/agent-x");
        fs::create_dir_all(&dossier).expect("the memories directory must be created");
        fs::write(dossier.join("casse.md"), "pas de frontmatter\n")
            .expect("the fixture must be written");

        let resultat = stock.list(&portee).await;

        assert!(
            matches!(&resultat, Err(ModulesError::Store(texte)) if texte.contains("casse.md")),
            "a broken memory must fail the listing and name its file, got {resultat:?}"
        );
    }
}
