//! Toolsets actifs et brouillons en répertoires, types de contexte en brouillon. Contrat : `host/toolset_store.sdd`.

use std::fs;
use std::future::{self, Future};
use std::io;
use std::path::{Path, PathBuf};

use lymaille::{
    ContextType, DraftEntry, DraftKind, DraftStore, Skill, Toolset, ToolsetStore, ToolsetsError,
    ToolsetsResult, check_template, extract_contract,
};
use serde_json::Value;

use super::HostError;
use super::catalog_store::{Entree, check_name, ecrire, encode, lire, supprimer};
use super::context_store::nom_de;
use super::files;
use super::layers::{Category, Layers, Level};

/// Fichier de métadonnées d'un toolset.
const TOOLSET_FILE: &str = "toolset.yaml";
/// Fichier du code (module Rhai) d'un toolset.
const MODULE_FILE: &str = "module.rhai";
/// Sous-répertoire des skills d'un toolset.
const SKILLS_DIR: &str = "skills";
/// Fichier d'un skill.
const SKILL_FILE: &str = "SKILL.md";
/// Extension des fichiers YAML.
const YAML_EXT: &str = "yaml";
/// Marqueur d'une écriture ou d'une suppression impossible.
const IO: &str = "VNL-CLI-HST-042";
/// Extension des brouillons de types de contexte : `<nom>.type.yaml` dans `toolsets-drafts/`.
const TYPE_EXT: &str = "type.yaml";
/// Suffixe qui identifie un brouillon de type dans `toolsets-drafts/`.
const TYPE_SUFFIX: &str = ".type";
/// Préfixe des répertoires temporaires, ignorés par les listages.
const TMP_PREFIX: &str = ".tmp-";
/// Droits d'un fichier du niveau partagé : lisible dans le dépôt.
const MODE_PARTAGE: u32 = 0o644;
/// Droits d'un fichier du niveau utilisateur : privé.
const MODE_PRIVE: u32 = 0o600;

/// Marqueur d'un répertoire de toolset incomplet ou illisible.
const FORMAT: &str = "VNL-CLI-HST-035";
/// Marqueur d'un toolset refusé par la validation de lymaille.
const INVALID: &str = "VNL-CLI-HST-036";
/// Marqueur d'un brouillon absent à la promotion.
const NO_DRAFT: &str = "VNL-CLI-HST-037";
/// Marqueur d'un niveau partagé absent.
const NO_SHARED: &str = "VNL-CLI-HST-038";
/// Marqueur d'un répertoire de toolset qui est un lien symbolique.
const LINK: &str = "VNL-CLI-HST-039";
/// Marqueur d'une sérialisation impossible.
const ENCODE: &str = "VNL-CLI-HST-040";
/// Marqueur d'un annexe au nom qui sortirait de son répertoire.
const ANNEX: &str = "VNL-CLI-HST-041";

/// Stockage des toolsets actifs et des brouillons. Ne crée rien à la construction.
#[derive(Debug, Clone)]
pub struct FileToolsets {
    layers: Layers,
    render_cap_chars: u64,
}

impl FileToolsets {
    /// Stockage rangé sous les niveaux de `layers`, validé avec le plafond de rendu `render_cap_chars`.
    #[must_use]
    pub fn new(layers: &Layers, render_cap_chars: u64) -> Self {
        Self {
            layers: layers.clone(),
            render_cap_chars,
        }
    }

    /// Promeut le brouillon `nom` du niveau `niveau` en toolset actif.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for an invalid draft, a missing draft or a missing shared level, and
    /// [`HostError::Io`] for the copy or the rename.
    pub fn promote(&self, niveau: Level, nom: &str) -> Result<(), HostError> {
        self.promote_with(niveau, nom, |de, vers| fs::rename(de, vers))
    }

    /// Comme [`FileToolsets::promote`], avec le renommage final injecté pour tester son échec.
    fn promote_with<R>(&self, niveau: Level, nom: &str, rename: R) -> Result<(), HostError>
    where
        R: FnOnce(&Path, &Path) -> io::Result<()>,
    {
        check_name(nom)?;
        let racine = self.dossier(niveau, Category::Toolsets)?;
        let brouillon = self.dossier(niveau, Category::ToolsetDrafts)?.join(nom);
        let toolset = lire_repertoire(&brouillon)?.ok_or_else(|| HostError::Invalid {
            cause: format!("{NO_DRAFT}: no draft named `{nom}` at this level"),
        })?;
        valider(&toolset, self.render_cap_chars).map_err(|erreur| HostError::Invalid {
            cause: format!("{INVALID}: {erreur}"),
        })?;
        fs::create_dir_all(&racine).map_err(|erreur| io_error(&racine, &erreur))?;
        let pid = std::process::id();
        let tmp = racine.join(format!("{TMP_PREFIX}{nom}-{pid}"));
        let ancien = racine.join(format!("{TMP_PREFIX}ancien-{nom}-{pid}"));
        let cible = racine.join(nom);
        fs::remove_dir_all(&tmp).ok();
        ecrire_repertoire(&tmp, &toolset, mode_de(niveau))?;
        let existait = cible.exists();
        if existait && let Err(erreur) = fs::rename(&cible, &ancien) {
            fs::remove_dir_all(&tmp).ok();
            return Err(io_error(&cible, &erreur));
        }
        if let Err(erreur) = rename(&tmp, &cible) {
            if existait {
                fs::rename(&ancien, &cible).ok();
            }
            fs::remove_dir_all(&tmp).ok();
            return Err(io_error(&cible, &erreur));
        }
        if existait {
            fs::remove_dir_all(&ancien).ok();
        }
        fs::remove_dir_all(&brouillon).map_err(|erreur| io_error(&brouillon, &erreur))
    }

    /// Liste les toolsets actifs de tous les niveaux, un fichier illisible rendant son erreur seule.
    #[must_use]
    pub fn list_active(&self) -> Vec<Result<Entree<Toolset>, HostError>> {
        let mut sortie = Vec::new();
        for niveau in [Level::Shared, Level::User] {
            let Some(racine) = self.layers.dir(niveau, Category::Toolsets) else {
                continue;
            };
            for dossier in sous_dossiers(&racine) {
                match dossier.and_then(|chemin| lire_repertoire(&chemin)) {
                    Ok(Some(valeur)) => sortie.push(Ok(Entree { niveau, valeur })),
                    Ok(None) => {}
                    Err(erreur) => sortie.push(Err(erreur)),
                }
            }
        }
        sortie
    }

    /// Répertoire d'une catégorie à un niveau. Un niveau absent est une erreur explicite.
    fn dossier(&self, niveau: Level, categorie: Category) -> Result<PathBuf, HostError> {
        self.layers
            .dir(niveau, categorie)
            .ok_or_else(|| HostError::Invalid {
                cause: format!("{NO_SHARED}: no shared level outside a repository"),
            })
    }

    /// Lit le toolset `nom` dans une catégorie, le partagé d'abord.
    fn lire_en_couches(
        &self,
        categorie: Category,
        nom: &str,
    ) -> Result<Option<Toolset>, HostError> {
        check_name(nom)?;
        for niveau in [Level::Shared, Level::User] {
            let Some(racine) = self.layers.dir(niveau, categorie) else {
                continue;
            };
            if let Some(toolset) = lire_repertoire(&racine.join(nom))? {
                return Ok(Some(toolset));
            }
        }
        Ok(None)
    }

    /// Écrit le brouillon de `toolset` au niveau utilisateur, en remplaçant le précédent.
    fn ecrire_brouillon(&self, toolset: &Toolset) -> Result<(), HostError> {
        check_name(toolset.name())?;
        let racine = self.dossier(Level::User, Category::ToolsetDrafts)?;
        let tmp = racine.join(format!(
            "{TMP_PREFIX}{}-{}",
            toolset.name(),
            std::process::id()
        ));
        let cible = racine.join(toolset.name());
        fs::remove_dir_all(&tmp).ok();
        ecrire_repertoire(&tmp, toolset, MODE_PRIVE)?;
        if cible.exists()
            && let Err(erreur) = fs::remove_dir_all(&cible)
        {
            fs::remove_dir_all(&tmp).ok();
            return Err(io_error(&cible, &erreur));
        }
        fs::rename(&tmp, &cible).map_err(|erreur| {
            fs::remove_dir_all(&tmp).ok();
            io_error(&cible, &erreur)
        })
    }
}

/// Mode des fichiers d'un toolset au niveau donné.
const fn mode_de(niveau: Level) -> u32 {
    match niveau {
        Level::Shared => MODE_PARTAGE,
        Level::User => MODE_PRIVE,
    }
}

/// Erreur d'E/S sur `chemin`, avec son marqueur d'écriture.
fn io_error(chemin: &Path, erreur: &io::Error) -> HostError {
    HostError::Io {
        chemin: chemin.to_path_buf(),
        cause: format!("{IO}: {erreur}"),
    }
}

/// Erreur de store de la bibliothèque, pour un refus de l'hôte.
fn store(erreur: &HostError) -> ToolsetsError {
    ToolsetsError::Store(erreur.to_string())
}

/// Valide un toolset comme lymaille valide un brouillon : le module donne un contrat, les modèles des skills se
/// compilent, les skills tiennent dans le plafond de rendu.
fn valider(toolset: &Toolset, render_cap_chars: u64) -> ToolsetsResult<()> {
    match extract_contract(toolset.source()) {
        Ok(_contract) => {}
        Err(ToolsetsError::InvalidToolset { reason, detail, .. }) => {
            return Err(ToolsetsError::InvalidToolset {
                name: toolset.name().to_owned(),
                reason,
                detail,
            });
        }
        Err(erreur) => return Err(erreur),
    }
    for skill in toolset.skills().values() {
        skill.check_size(render_cap_chars)?;
        check_template(
            &format!("skill {} description", skill.name()),
            skill.description(),
        )?;
        check_template(&format!("skill {} body", skill.name()), skill.body())?;
    }
    Ok(())
}

/// Sous-répertoires d'une racine, triés, sans liens symboliques ni répertoires temporaires. Une racine absente n'en a pas.
fn sous_dossiers(racine: &Path) -> Vec<Result<PathBuf, HostError>> {
    let lecture = match fs::read_dir(racine) {
        Ok(lecture) => lecture,
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(erreur) => return vec![Err(io_error(racine, &erreur))],
    };
    let mut dossiers: Vec<PathBuf> = Vec::new();
    let mut erreurs: Vec<Result<PathBuf, HostError>> = Vec::new();
    for entree in lecture {
        let entree = match entree {
            Ok(entree) => entree,
            Err(erreur) => {
                erreurs.push(Err(io_error(racine, &erreur)));
                continue;
            }
        };
        if entree.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
            continue;
        }
        match entree.file_type() {
            Ok(type_fichier) if type_fichier.is_dir() => dossiers.push(entree.path()),
            Ok(_) => {}
            Err(erreur) => erreurs.push(Err(io_error(&entree.path(), &erreur))),
        }
    }
    dossiers.sort();
    dossiers.into_iter().map(Ok).chain(erreurs).collect()
}

/// Fichiers réguliers d'un répertoire, sans liens symboliques, triés. Une entrée illisible rend son erreur seule.
fn fichiers(racine: &Path) -> Vec<Result<PathBuf, HostError>> {
    let lecture = match fs::read_dir(racine) {
        Ok(lecture) => lecture,
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(erreur) => return vec![Err(io_error(racine, &erreur))],
    };
    let mut trouves: Vec<PathBuf> = Vec::new();
    let mut erreurs: Vec<Result<PathBuf, HostError>> = Vec::new();
    for entree in lecture {
        match entree {
            Ok(entree) => match entree.file_type() {
                Ok(type_fichier) if type_fichier.is_file() => trouves.push(entree.path()),
                Ok(_) => {}
                Err(erreur) => erreurs.push(Err(io_error(&entree.path(), &erreur))),
            },
            Err(erreur) => erreurs.push(Err(io_error(racine, &erreur))),
        }
    }
    trouves.sort();
    trouves.into_iter().map(Ok).chain(erreurs).collect()
}

/// Rendu d'un SKILL.md : frontmatter YAML (nom, description, licence, compatibilité, métadonnées), puis le corps exact.
fn rendu_skill(skill: &Skill) -> Result<String, HostError> {
    let mut entete = serde_json::Map::new();
    entete.insert("name".to_owned(), Value::String(skill.name().to_owned()));
    entete.insert(
        "description".to_owned(),
        Value::String(skill.description().to_owned()),
    );
    if let Some(licence) = skill.license() {
        entete.insert("license".to_owned(), Value::String(licence.to_owned()));
    }
    if let Some(compatibilite) = skill.compatibility() {
        entete.insert(
            "compatibility".to_owned(),
            Value::String(compatibilite.to_owned()),
        );
    }
    if !skill.metadata().is_empty() {
        entete.insert("metadata".to_owned(), encode_json(skill.metadata())?);
    }
    let mut yaml = encode(&Value::Object(entete))?;
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    Ok(format!("---\n{yaml}---\n{}", skill.body()))
}

/// Valeur JSON d'une entité sérialisable, ou un refus d'encodage.
fn encode_json<T: serde::Serialize>(valeur: &T) -> Result<Value, HostError> {
    serde_json::to_value(valeur).map_err(|erreur| HostError::Invalid {
        cause: format!("{ENCODE}: {erreur}"),
    })
}

/// Écrit un toolset dans `dossier` : `toolset.yaml`, `module.rhai`, puis `skills/<nom>/SKILL.md` et ses annexes. Chaque
/// fichier est écrit de façon atomique.
fn ecrire_repertoire(dossier: &Path, toolset: &Toolset, mode: u32) -> Result<(), HostError> {
    let mut metadonnees = encode_json(toolset)?;
    if let Some(objet) = metadonnees.as_object_mut() {
        objet.remove("source");
        objet.remove("skills");
    }
    let yaml = encode(&metadonnees)?;
    files::write_atomic(&dossier.join(TOOLSET_FILE), yaml.as_bytes(), mode)?;
    files::write_atomic(
        &dossier.join(MODULE_FILE),
        toolset.source().as_bytes(),
        mode,
    )?;
    for skill in toolset.skills().values() {
        let base = dossier.join(SKILLS_DIR).join(skill.name());
        files::write_atomic(&base.join(SKILL_FILE), rendu_skill(skill)?.as_bytes(), mode)?;
        for (annexe, texte) in skill.annexes() {
            if !files::est_un_nom_de_fichier(annexe) || annexe == SKILL_FILE {
                return Err(HostError::Invalid {
                    cause: format!(
                        "{ANNEX}: an annex name must be a plain file name other than SKILL.md"
                    ),
                });
            }
            files::write_atomic(&base.join(annexe), texte.as_bytes(), mode)?;
        }
    }
    Ok(())
}

/// Lit un toolset depuis son répertoire. `None` si le répertoire n'existe pas.
fn lire_repertoire(dossier: &Path) -> Result<Option<Toolset>, HostError> {
    match fs::symlink_metadata(dossier) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(HostError::Invalid {
                cause: format!(
                    "{LINK}: {} is a symbolic link, it is not followed",
                    dossier.display()
                ),
            });
        }
        Ok(meta) if !meta.is_dir() => return Ok(None),
        Ok(_) => {}
        Err(erreur) if erreur.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(erreur) => return Err(io_error(dossier, &erreur)),
    }
    let format = |chemin: PathBuf| HostError::Format {
        cause: format!("{FORMAT}: {} is missing or unreadable", chemin.display()),
        chemin,
    };
    let metadonnees_chemin = dossier.join(TOOLSET_FILE);
    let texte =
        files::read_text(&metadonnees_chemin)?.ok_or_else(|| format(metadonnees_chemin.clone()))?;
    let source = files::read_text(&dossier.join(MODULE_FILE))?
        .ok_or_else(|| format(dossier.join(MODULE_FILE)))?;
    let lu: yaml_serde::Value =
        yaml_serde::from_str(&texte).map_err(|_| format(metadonnees_chemin.clone()))?;
    let mut forme = encode_json(&lu).map_err(|_| format(metadonnees_chemin.clone()))?;
    let objet = forme
        .as_object_mut()
        .ok_or_else(|| format(metadonnees_chemin.clone()))?;
    objet.insert("source".to_owned(), Value::String(source));
    objet.insert("skills".to_owned(), Value::Object(serde_json::Map::new()));
    let mut toolset: Toolset =
        serde_json::from_value(forme).map_err(|erreur| HostError::Format {
            chemin: metadonnees_chemin.clone(),
            cause: format!("{FORMAT}: {erreur}"),
        })?;
    for skill in lire_skills(&dossier.join(SKILLS_DIR))? {
        toolset = toolset
            .with_skill(skill)
            .map_err(|erreur| HostError::Format {
                chemin: dossier.to_path_buf(),
                cause: format!("{FORMAT}: {erreur}"),
            })?;
    }
    Ok(Some(toolset))
}

/// Lit les skills d'un répertoire `skills/`, avec leurs annexes (les autres fichiers de leur répertoire).
fn lire_skills(racine: &Path) -> Result<Vec<Skill>, HostError> {
    let mut skills = Vec::new();
    for dossier in sous_dossiers(racine) {
        let dossier = dossier?;
        let chemin = dossier.join(SKILL_FILE);
        let texte = files::read_text(&chemin)?.ok_or_else(|| HostError::Format {
            chemin: chemin.clone(),
            cause: format!("{FORMAT}: the skill has no {SKILL_FILE}"),
        })?;
        let mut skill = Skill::parse(&texte).map_err(|erreur| HostError::Format {
            chemin: chemin.clone(),
            cause: format!("{FORMAT}: {erreur}"),
        })?;
        for annexe in fichiers(&dossier) {
            let annexe = annexe?;
            let nom = annexe
                .file_name()
                .map(|nom| nom.to_string_lossy().into_owned())
                .unwrap_or_default();
            if nom == SKILL_FILE {
                continue;
            }
            if let Some(texte) = files::read_text(&annexe)? {
                skill = skill
                    .with_annex(nom, texte)
                    .map_err(|erreur| HostError::Format {
                        chemin: annexe.clone(),
                        cause: format!("{FORMAT}: {erreur}"),
                    })?;
            }
        }
        skills.push(skill);
    }
    Ok(skills)
}

impl ToolsetStore for FileToolsets {
    fn get_active(
        &self,
        name: &str,
    ) -> impl Future<Output = ToolsetsResult<Option<Toolset>>> + Send {
        future::ready(
            self.lire_en_couches(Category::Toolsets, name)
                .map_err(|erreur| store(&erreur)),
        )
    }

    fn get_draft(
        &self,
        name: &str,
    ) -> impl Future<Output = ToolsetsResult<Option<Toolset>>> + Send {
        future::ready(
            self.lire_en_couches(Category::ToolsetDrafts, name)
                .map_err(|erreur| store(&erreur)),
        )
    }

    fn put_draft(&self, toolset: &Toolset) -> impl Future<Output = ToolsetsResult<()>> + Send {
        future::ready(valider(toolset, self.render_cap_chars).and_then(|()| {
            self.ecrire_brouillon(toolset)
                .map_err(|erreur| store(&erreur))
        }))
    }
}

impl DraftStore for FileToolsets {
    fn list_drafts(&self) -> impl Future<Output = ToolsetsResult<Vec<DraftEntry>>> + Send {
        future::ready(self.lister_brouillons().map_err(|erreur| store(&erreur)))
    }

    fn delete_draft(
        &self,
        kind: DraftKind,
        name: &str,
    ) -> impl Future<Output = ToolsetsResult<bool>> + Send {
        future::ready(
            self.supprimer_brouillon(kind, name)
                .map_err(|erreur| store(&erreur)),
        )
    }

    fn put_type_draft(
        &self,
        context_type: &ContextType,
    ) -> impl Future<Output = ToolsetsResult<()>> + Send {
        future::ready(
            nom_de(context_type)
                .and_then(|nom| {
                    let texte = encode(context_type)?;
                    ecrire(
                        &self.layers,
                        Level::User,
                        Category::ToolsetDrafts,
                        TYPE_EXT,
                        &nom,
                        &texte,
                    )
                })
                .map_err(|erreur| store(&erreur)),
        )
    }

    fn get_type_draft(
        &self,
        name: &str,
    ) -> impl Future<Output = ToolsetsResult<Option<ContextType>>> + Send {
        future::ready(
            lire(
                &self.layers,
                Category::ToolsetDrafts,
                TYPE_EXT,
                name,
                decode_type_draft,
            )
            .map_err(|erreur| store(&erreur)),
        )
    }
}

fn decode_type_draft(texte: &str) -> Result<ContextType, String> {
    yaml_serde::from_str(texte).map_err(|erreur| erreur.to_string())
}

impl FileToolsets {
    /// Liste les brouillons de toolsets (répertoires) et de types de contexte (`<nom>.type.yaml`), de tous les niveaux.
    fn lister_brouillons(&self) -> Result<Vec<DraftEntry>, HostError> {
        let mut entrees = Vec::new();
        for niveau in [Level::Shared, Level::User] {
            let Some(racine) = self.layers.dir(niveau, Category::ToolsetDrafts) else {
                continue;
            };
            for dossier in sous_dossiers(&racine) {
                let dossier = dossier?;
                if let Some(nom) = dossier
                    .file_name()
                    .map(|nom| nom.to_string_lossy().into_owned())
                {
                    entrees.push(DraftEntry::new(DraftKind::Toolset, nom));
                }
            }
            for fichier in files::list_files(&racine, YAML_EXT) {
                let fichier = fichier?;
                let nom = fichier
                    .file_stem()
                    .map(|nom| nom.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Some(nom) = nom.strip_suffix(TYPE_SUFFIX) {
                    entrees.push(DraftEntry::new(DraftKind::ContextType, nom.to_owned()));
                }
            }
        }
        Ok(entrees)
    }

    /// Supprime le brouillon `name` de `kind` au niveau utilisateur. `true` s'il existait.
    fn supprimer_brouillon(&self, kind: DraftKind, name: &str) -> Result<bool, HostError> {
        if kind == DraftKind::ContextType {
            return supprimer(
                &self.layers,
                Level::User,
                Category::ToolsetDrafts,
                TYPE_EXT,
                name,
            );
        }
        check_name(name)?;
        let dossier = self
            .dossier(Level::User, Category::ToolsetDrafts)?
            .join(name);
        if !dossier.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(&dossier)
            .map(|()| true)
            .map_err(|erreur| io_error(&dossier, &erreur))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use lymaille::{
        ContextType, DraftEntry, DraftKind, DraftStore as _, Skill, Toolset, ToolsetStore as _,
        ToolsetsError,
    };
    use vynil_core::Semver;

    use super::{FileToolsets, HostError, rendu_skill};
    use crate::host::layers::{Layers, Level};

    /// Plafond de rendu des tests : large, sauf quand un test le baisse.
    const CAP: u64 = 262_144;

    fn sans_depot(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    fn avec_depot(home: &Path, racine: &Path) -> Layers {
        let git = racine.join(".git");
        Layers::discover(home, racine, None, |chemin| chemin == git)
    }

    /// Toolset valide, nommé `nom`, avec un skill `revue`.
    fn toolset(nom: &str, description: &str) -> Toolset {
        let base = Toolset::new(
            nom.to_owned(),
            description.to_owned(),
            Semver::parse("1.0.0").expect("a sample version must parse"),
            "fn ping() { 1 }".to_owned(),
        )
        .expect("a valid toolset must be built");
        let skill = Skill::new(
            "revue".to_owned(),
            "Relit une modification".to_owned(),
            "Corps\n\nsuite\n".to_owned(),
        )
        .expect("a valid skill must be built");
        base.with_skill(skill).expect("the skill must be attached")
    }

    fn racine(home: &Path) -> PathBuf {
        home.join(".config").join("vanyline")
    }

    #[tokio::test]
    async fn an_agent_writes_a_draft_that_is_not_active() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), CAP);
        let original = toolset("outils", "Outils de test");

        stock
            .put_draft(&original)
            .await
            .expect("a valid draft must be stored");

        assert_eq!(
            stock
                .get_active("outils")
                .await
                .expect("the active lookup must succeed"),
            None,
            "a draft must never be active"
        );
        assert_eq!(
            stock
                .get_draft("outils")
                .await
                .expect("the draft lookup must succeed"),
            Some(original),
            "the draft read back must equal the one stored"
        );
        assert!(
            racine(home.path())
                .join("toolsets-drafts/outils/module.rhai")
                .is_file(),
            "the draft must be a directory under toolsets-drafts/"
        );
        assert!(
            !racine(home.path()).join("toolsets").exists(),
            "a draft write must not create anything under toolsets/"
        );
    }

    #[tokio::test]
    async fn promotion_makes_the_toolset_active_and_removes_the_draft() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), CAP);
        let original = toolset("outils", "Outils de test");
        stock
            .put_draft(&original)
            .await
            .expect("the draft must be stored");

        stock
            .promote(Level::User, "outils")
            .expect("a valid draft must be promoted");

        let dossier = racine(home.path()).join("toolsets/outils");
        assert!(
            dossier.join("toolset.yaml").is_file(),
            "toolset.yaml must be written"
        );
        assert!(
            dossier.join("module.rhai").is_file(),
            "module.rhai must be written"
        );
        assert!(
            dossier.join("skills/revue/SKILL.md").is_file(),
            "the skill must be written under skills/"
        );
        assert!(
            !racine(home.path()).join("toolsets-drafts/outils").exists(),
            "the draft must be removed once promoted"
        );
        assert_eq!(
            stock
                .get_active("outils")
                .await
                .expect("the active lookup must succeed"),
            Some(original),
            "the active toolset must read back identical to the draft"
        );
    }

    #[tokio::test]
    async fn an_interrupted_promotion_keeps_the_draft_and_activates_nothing() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), CAP);
        let original = toolset("outils", "Outils de test");
        stock
            .put_draft(&original)
            .await
            .expect("the draft must be stored");
        let echec = |_de: &Path, _vers: &Path| Err(io::Error::other("injected rename failure"));

        let resultat = stock.promote_with(Level::User, "outils", echec);

        assert!(resultat.is_err(), "a failed final rename must be reported");
        assert_eq!(
            stock
                .get_draft("outils")
                .await
                .expect("the draft lookup must succeed"),
            Some(original),
            "the draft must be intact after an interrupted promotion"
        );
        assert_eq!(
            stock
                .get_active("outils")
                .await
                .expect("the active lookup must succeed"),
            None,
            "no partial active toolset may exist after an interrupted promotion"
        );
    }

    #[tokio::test]
    async fn an_invalid_draft_is_refused_and_the_active_toolset_stays() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), CAP);
        let premiere = toolset("outils", "Version une");
        stock
            .put_draft(&premiere)
            .await
            .expect("the first draft must be stored");
        stock
            .promote(Level::User, "outils")
            .expect("the first version must be promoted");
        stock
            .put_draft(&toolset("outils", "Version deux"))
            .await
            .expect("the second draft must be stored");
        fs::write(
            racine(home.path()).join("toolsets-drafts/outils/module.rhai"),
            "fn (",
        )
        .expect("the broken module must be written by hand");

        let resultat = stock.promote(Level::User, "outils");

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "a draft whose module is refused must not be promoted, got {resultat:?}"
        );
        assert_eq!(
            stock
                .get_active("outils")
                .await
                .expect("the active lookup must succeed"),
            Some(premiere),
            "the active toolset must be unchanged by a refused promotion"
        );
    }

    #[tokio::test]
    async fn a_draft_with_a_broken_module_is_refused_at_put_draft() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), CAP);
        let casse = Toolset::new(
            "casse".to_owned(),
            "Module casse".to_owned(),
            Semver::parse("1.0.0").expect("a sample version must parse"),
            "fn (".to_owned(),
        )
        .expect("the toolset itself must be built");

        let resultat = stock.put_draft(&casse).await;

        assert!(
            matches!(resultat, Err(ToolsetsError::InvalidToolset { .. })),
            "put_draft must apply the library checks, got {resultat:?}"
        );
        assert!(
            !racine(home.path()).join("toolsets-drafts").exists(),
            "a refused draft must write nothing"
        );
    }

    #[tokio::test]
    async fn a_skill_over_the_render_cap_is_refused_at_put_draft() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), 10);

        let resultat = stock.put_draft(&toolset("outils", "Outils de test")).await;

        assert!(
            matches!(resultat, Err(ToolsetsError::SkillTooLarge { .. })),
            "a skill longer than the render cap must be refused, got {resultat:?}"
        );
    }

    #[tokio::test]
    async fn active_toolset_is_read_shared_first() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let depot = tempfile::tempdir().expect("a temporary repository must be created");
        let stock = FileToolsets::new(&avec_depot(home.path(), depot.path()), CAP);
        stock
            .put_draft(&toolset("outils", "Version utilisateur"))
            .await
            .expect("the user draft must be stored");
        stock
            .promote(Level::User, "outils")
            .expect("the user version must be promoted");
        super::ecrire_repertoire(
            &depot.path().join(".vanyline/toolsets-drafts/outils"),
            &toolset("outils", "Version partagee"),
            0o644,
        )
        .expect("the shared draft must be written");
        stock
            .promote(Level::Shared, "outils")
            .expect("the shared version must be promoted");

        let actif = stock
            .get_active("outils")
            .await
            .expect("the active lookup must succeed")
            .expect("an active toolset must exist");

        assert_eq!(
            actif.description(),
            "Version partagee",
            "the shared level must answer first"
        );
    }

    #[tokio::test]
    async fn type_drafts_round_trip_and_are_listed_then_deleted() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileToolsets::new(&sans_depot(home.path()), CAP);
        stock
            .put_draft(&toolset("outils", "Outils de test"))
            .await
            .expect("the toolset draft must be stored");
        let type_brouillon: ContextType = serde_json::from_value(serde_json::json!({
            "name": "doc-type",
            "version": "1.0.0",
            "schema": {"type": "object"},
            "roles": [],
            "includes": []
        }))
        .expect("a valid context type must read from its form");

        stock
            .put_type_draft(&type_brouillon)
            .await
            .expect("the type draft must be stored");

        assert_eq!(
            stock
                .get_type_draft("doc-type")
                .await
                .expect("the type draft lookup must succeed"),
            Some(type_brouillon),
            "the type draft read back must equal the one stored"
        );
        let liste = stock.list_drafts().await.expect("the listing must succeed");
        assert!(
            liste.contains(&DraftEntry::new(DraftKind::Toolset, "outils".to_owned())),
            "the toolset draft must be listed as a toolset"
        );
        assert!(
            liste.contains(&DraftEntry::new(
                DraftKind::ContextType,
                "doc-type".to_owned()
            )),
            "the type draft must be listed as a context type"
        );
        assert!(
            stock
                .delete_draft(DraftKind::Toolset, "outils")
                .await
                .expect("the removal must succeed"),
            "the toolset draft must have existed"
        );
        assert!(
            !stock
                .delete_draft(DraftKind::Toolset, "outils")
                .await
                .expect("the second removal must succeed"),
            "the second removal must report that nothing existed"
        );
    }

    #[test]
    fn a_skill_round_trips_through_its_rendered_text() {
        let texte = "---\nname: revue\ndescription: Relit une modification\nlicense: MIT\ncompatibility: vanyline >= 1\nmetadata:\n  auteur: equipe\n---\nPremiere ligne\n\nSeconde ligne\n";
        let lu = Skill::parse(texte).expect("the sample SKILL.md must parse");

        let rendu = rendu_skill(&lu).expect("the skill must render");
        let relu = Skill::parse(&rendu).expect("the rendered SKILL.md must parse");

        assert!(
            relu == lu,
            "parse, render, parse must give the same skill, rendered: {rendu}"
        );
    }
}
