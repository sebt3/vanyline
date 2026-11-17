//! Agents, profils et providers sur des fichiers, en deux niveaux. Contrat : `host/catalog_store.sdd`.

use std::fs;
use std::future::{self, Future};
use std::path::Path;

use lymaille::{
    AgentDefinition, AgentKind, AgentStore, AgentsError, AgentsResult, DefaultSecretDetector,
    ModelProfile, ProfileStore, Provider, ProviderStore,
};

use super::HostError;
use super::context_derive::CLI_AGENT;
use super::files;
use super::layers::{Category, Layers, Level};

/// Extension des agents (Markdown avec frontmatter YAML).
const AGENT_EXT: &str = "md";
/// Extension des profils et des providers (YAML).
const YAML_EXT: &str = "yaml";
/// Droits d'un fichier du niveau partagé : lisible dans le dépôt.
const MODE_PARTAGE: u32 = 0o644;
/// Droits d'un fichier du niveau utilisateur : privé.
const MODE_PRIVE: u32 = 0o600;

/// Marqueur d'un nom qui sortirait de son répertoire ou serait vide.
const NAME: &str = "VNL-CLI-HST-021";
/// Marqueur d'une écriture au niveau partagé hors dépôt.
const NO_SHARED: &str = "VNL-CLI-HST-022";
/// Marqueur d'un fichier qui n'a pas la forme attendue.
const FORMAT: &str = "VNL-CLI-HST-023";
/// Marqueur d'une sérialisation impossible.
const ENCODE: &str = "VNL-CLI-HST-024";
/// Marqueur d'une référence qui ressemble à un secret.
const SECRET: &str = "VNL-CLI-HST-025";
/// Marqueur d'un fichier qui est un lien symbolique.
const LINK: &str = "VNL-CLI-HST-026";
/// Marqueur d'une définition refusée par la validation de lymaille.
const DEFINITION: &str = "VNL-CLI-HST-027";

/// Une entrée du catalogue, avec le niveau d'où elle vient.
#[derive(Debug, Clone, PartialEq)]
pub struct Entree<T> {
    /// Niveau du fichier.
    pub niveau: Level,
    /// Définition lue.
    pub valeur: T,
}

/// Catalogue des agents, profils et providers sur `layers`. Ne crée rien à la construction.
#[derive(Debug, Clone)]
pub struct FileCatalog {
    layers: Layers,
}

/// Décodeur d'un fichier : son texte vers la définition, ou la cause du refus (sans contenu).
pub(super) type Decode<T> = fn(&str) -> Result<T, String>;

fn decode_agent(texte: &str) -> Result<AgentDefinition, String> {
    AgentDefinition::from_text(texte).map_err(|erreur| erreur.to_string())
}

fn decode_profile(texte: &str) -> Result<ModelProfile, String> {
    yaml_serde::from_str(texte).map_err(|erreur| erreur.to_string())
}

fn decode_provider(texte: &str) -> Result<Provider, String> {
    yaml_serde::from_str(texte).map_err(|erreur| erreur.to_string())
}

impl FileCatalog {
    /// Catalogue rangé sous les niveaux de `layers`.
    #[must_use]
    pub fn new(layers: &Layers) -> Self {
        Self {
            layers: layers.clone(),
        }
    }

    /// Écrit un agent au niveau `niveau`.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name or definition or a missing level, [`HostError::Io`] for the
    /// write.
    pub fn put_agent_at(
        &self,
        niveau: Level,
        definition: &AgentDefinition,
    ) -> Result<(), HostError> {
        check_name(definition.id())?;
        definition.validate().map_err(|erreur| HostError::Invalid {
            cause: format!("{DEFINITION}: {erreur}"),
        })?;
        let texte = definition.to_text().map_err(|erreur| HostError::Invalid {
            cause: format!("{ENCODE}: {erreur}"),
        })?;
        ecrire(
            &self.layers,
            niveau,
            Category::Agents,
            AGENT_EXT,
            definition.id(),
            &texte,
        )
    }

    /// Écrit un profil au niveau `niveau`.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name or a missing level, [`HostError::Io`] for the write.
    pub fn put_profile_at(&self, niveau: Level, profile: &ModelProfile) -> Result<(), HostError> {
        let texte = encode(profile)?;
        ecrire(
            &self.layers,
            niveau,
            Category::Profiles,
            YAML_EXT,
            profile.name(),
            &texte,
        )
    }

    /// Écrit un provider au niveau `niveau`. Refuse une référence qui ressemble à un secret.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name or a secret-like reference, [`HostError::Io`] for the write.
    pub fn put_provider_at(&self, niveau: Level, provider: &Provider) -> Result<(), HostError> {
        check_name(provider.name())?;
        if !provider.check(&DefaultSecretDetector::default()) {
            return Err(HostError::Invalid {
                cause: format!(
                    "{SECRET}: a reference of provider `{}` looks like a secret; name the secret instead",
                    provider.name()
                ),
            });
        }
        let texte = encode(provider)?;
        ecrire(
            &self.layers,
            niveau,
            Category::Providers,
            YAML_EXT,
            provider.name(),
            &texte,
        )
    }

    /// Liste les agents de tous les niveaux. Un fichier illisible rend son erreur sans empêcher les autres.
    #[must_use]
    pub fn list_agents(&self) -> Vec<Result<Entree<AgentDefinition>, HostError>> {
        lister(&self.layers, Category::Agents, AGENT_EXT, decode_agent)
    }

    /// Liste les profils de tous les niveaux.
    #[must_use]
    pub fn list_profiles(&self) -> Vec<Result<Entree<ModelProfile>, HostError>> {
        lister(&self.layers, Category::Profiles, YAML_EXT, decode_profile)
    }

    /// Liste les providers de tous les niveaux.
    #[must_use]
    pub fn list_providers(&self) -> Vec<Result<Entree<Provider>, HostError>> {
        lister(&self.layers, Category::Providers, YAML_EXT, decode_provider)
    }

    /// Supprime un agent d'un niveau. `true` s'il existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name, [`HostError::Io`] for the removal.
    pub fn delete_agent(&self, niveau: Level, nom: &str) -> Result<bool, HostError> {
        supprimer(&self.layers, niveau, Category::Agents, AGENT_EXT, nom)
    }

    /// Supprime un profil d'un niveau. `true` s'il existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name, [`HostError::Io`] for the removal.
    pub fn delete_profile(&self, niveau: Level, nom: &str) -> Result<bool, HostError> {
        supprimer(&self.layers, niveau, Category::Profiles, YAML_EXT, nom)
    }

    /// Supprime un provider d'un niveau. `true` s'il existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name, [`HostError::Io`] for the removal.
    pub fn delete_provider(&self, niveau: Level, nom: &str) -> Result<bool, HostError> {
        supprimer(&self.layers, niveau, Category::Providers, YAML_EXT, nom)
    }
}

/// Texte YAML d'une entité sérialisable, ou un refus de l'hôte.
pub(super) fn encode<T: serde::Serialize>(valeur: &T) -> Result<String, HostError> {
    yaml_serde::to_string(valeur).map_err(|erreur| HostError::Invalid {
        cause: format!("{ENCODE}: {erreur}"),
    })
}

/// Lit `nom` dans chaque niveau, le partagé d'abord. `None` si aucun niveau ne le porte.
pub(super) fn lire<T>(
    layers: &Layers,
    categorie: Category,
    ext: &str,
    nom: &str,
    decode: Decode<T>,
) -> Result<Option<T>, HostError> {
    check_name(nom)?;
    for niveau in [Level::Shared, Level::User] {
        let Some(dossier) = layers.dir(niveau, categorie) else {
            continue;
        };
        if let Some(valeur) = read_file(&dossier.join(format!("{nom}.{ext}")), decode)? {
            return Ok(Some(valeur));
        }
    }
    Ok(None)
}

/// Liste les fichiers de la catégorie, niveau partagé puis utilisateur. Une entrée illisible rend son erreur seule.
pub(super) fn lister<T>(
    layers: &Layers,
    categorie: Category,
    ext: &str,
    decode: Decode<T>,
) -> Vec<Result<Entree<T>, HostError>> {
    let mut sortie = Vec::new();
    for niveau in [Level::Shared, Level::User] {
        let Some(dossier) = layers.dir(niveau, categorie) else {
            continue;
        };
        for entree in files::list_files(&dossier, ext) {
            match entree.and_then(|chemin| read_file(&chemin, decode)) {
                Ok(Some(valeur)) => sortie.push(Ok(Entree { niveau, valeur })),
                Ok(None) => {}
                Err(erreur) => sortie.push(Err(erreur)),
            }
        }
    }
    sortie
}

/// Écrit `texte` sous `nom` au niveau demandé, avec les droits du niveau.
pub(super) fn ecrire(
    layers: &Layers,
    niveau: Level,
    categorie: Category,
    ext: &str,
    nom: &str,
    texte: &str,
) -> Result<(), HostError> {
    check_name(nom)?;
    let dossier = layers
        .dir(niveau, categorie)
        .ok_or_else(|| HostError::Invalid {
            cause: format!("{NO_SHARED}: no shared level outside a repository"),
        })?;
    let mode = match niveau {
        Level::Shared => MODE_PARTAGE,
        Level::User => MODE_PRIVE,
    };
    files::write_atomic(
        &dossier.join(format!("{nom}.{ext}")),
        texte.as_bytes(),
        mode,
    )
}

/// Supprime `nom` d'un niveau. Un niveau absent n'a rien à supprimer.
pub(super) fn supprimer(
    layers: &Layers,
    niveau: Level,
    categorie: Category,
    ext: &str,
    nom: &str,
) -> Result<bool, HostError> {
    check_name(nom)?;
    let Some(dossier) = layers.dir(niveau, categorie) else {
        return Ok(false);
    };
    files::remove_file(&dossier.join(format!("{nom}.{ext}")))
}

/// Refuse un nom qui ne soit pas composé de lettres, chiffres, `-` et `_` : c'est le nom du fichier.
pub(super) fn check_name(nom: &str) -> Result<(), HostError> {
    let valide = !nom.is_empty()
        && nom.chars().all(|caractere| {
            caractere.is_ascii_alphanumeric() || caractere == '-' || caractere == '_'
        });
    if valide {
        Ok(())
    } else {
        Err(HostError::Invalid {
            cause: format!("{NAME}: a name must be made of letters, digits, `-` and `_`"),
        })
    }
}

/// Lit et décode `chemin`. `None` s'il n'existe pas. Un lien symbolique est refusé, jamais suivi.
pub(super) fn read_file<T>(chemin: &Path, decode: Decode<T>) -> Result<Option<T>, HostError> {
    match fs::symlink_metadata(chemin) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(HostError::Invalid {
                cause: format!(
                    "{LINK}: {} is a symbolic link, it is not followed",
                    chemin.display()
                ),
            });
        }
        Ok(_) => {}
        Err(erreur) if erreur.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(erreur) => {
            return Err(HostError::Io {
                chemin: chemin.to_path_buf(),
                cause: format!("{FORMAT}: {erreur}"),
            });
        }
    }
    let Some(texte) = files::read_text(chemin)? else {
        return Ok(None);
    };
    decode(&texte).map(Some).map_err(|cause| HostError::Format {
        chemin: chemin.to_path_buf(),
        cause: format!("{FORMAT}: {cause}"),
    })
}

/// Erreur de store de la bibliothèque, pour un refus de l'hôte.
pub(super) fn store(erreur: &HostError) -> AgentsError {
    AgentsError::Store(erreur.to_string())
}

/// Agent livré avec la CLI (`catalog_store.sdd`, `@shipped_agent`) : nature `Work`, prompt d'une phrase, sans profil
/// nommé. Il n'est jamais écrit ni listé ; un fichier d'un niveau le remplace.
fn shipped_agent() -> AgentDefinition {
    AgentDefinition::new(
        CLI_AGENT.to_owned(),
        AgentKind::Work,
        "Tu es l'agent de développement de la CLI vanyline : tu travailles dans la sandbox de la session, par ses outils."
            .to_owned(),
    )
}

impl AgentStore for FileCatalog {
    fn get_agent(
        &self,
        id: &str,
    ) -> impl Future<Output = AgentsResult<Option<AgentDefinition>>> + Send {
        future::ready(
            lire(&self.layers, Category::Agents, AGENT_EXT, id, decode_agent)
                .map(|trouve| trouve.or_else(|| (id == CLI_AGENT).then(shipped_agent)))
                .map_err(|erreur| store(&erreur)),
        )
    }

    fn put_agent(
        &self,
        definition: &AgentDefinition,
    ) -> impl Future<Output = AgentsResult<()>> + Send {
        future::ready(definition.validate().and_then(|()| {
            definition.to_text().and_then(|texte| {
                ecrire(
                    &self.layers,
                    Level::User,
                    Category::Agents,
                    AGENT_EXT,
                    definition.id(),
                    &texte,
                )
                .map_err(|erreur| store(&erreur))
            })
        }))
    }
}

impl ProfileStore for FileCatalog {
    fn get_profile(
        &self,
        name: &str,
    ) -> impl Future<Output = AgentsResult<Option<ModelProfile>>> + Send {
        future::ready(
            lire(
                &self.layers,
                Category::Profiles,
                YAML_EXT,
                name,
                decode_profile,
            )
            .map_err(|erreur| store(&erreur)),
        )
    }

    fn put_profile(&self, profile: &ModelProfile) -> impl Future<Output = AgentsResult<()>> + Send {
        future::ready(
            self.put_profile_at(Level::User, profile)
                .map_err(|erreur| store(&erreur)),
        )
    }
}

impl ProviderStore for FileCatalog {
    fn get_provider(
        &self,
        name: &str,
    ) -> impl Future<Output = AgentsResult<Option<Provider>>> + Send {
        future::ready(
            lire(
                &self.layers,
                Category::Providers,
                YAML_EXT,
                name,
                decode_provider,
            )
            .map_err(|erreur| store(&erreur)),
        )
    }

    fn put_provider(&self, provider: &Provider) -> impl Future<Output = AgentsResult<()>> + Send {
        future::ready(
            self.put_provider_at(Level::User, provider)
                .map_err(|erreur| store(&erreur)),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::{Path, PathBuf};

    use lymaille::{
        AgentDefinition, AgentKind, AgentStore as _, AgentsError, DefaultSecretDetector,
        ModelProfile, ProfileStore as _, Provider, ProviderLevel, ProviderStore as _,
        SecretDetector,
    };

    use super::{FileCatalog, HostError};
    use crate::host::layers::{Layers, Level};

    /// Détecteur de l'hôte qui ne connaît que le préfixe `tok_` : `sk-abc` y passe.
    struct TokDetector;

    impl SecretDetector for TokDetector {
        fn looks_like_secret(&self, text: &str) -> bool {
            text.starts_with("tok_")
        }
    }

    /// Dépôt fictif sous `racine` : le `.git` existe, `.vanyline/` est le niveau partagé.
    fn avec_depot(home: &Path, racine: &Path) -> Layers {
        let git = racine.join(".git");
        Layers::discover(home, racine, None, |chemin| chemin == git)
    }

    /// Hors dépôt : seul le niveau utilisateur existe.
    fn sans_depot(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    fn agent(id: &str, prompt: &str) -> AgentDefinition {
        AgentDefinition::new(id.to_owned(), AgentKind::Work, prompt.to_owned())
    }

    fn mode(chemin: &Path) -> u32 {
        fs::metadata(chemin)
            .expect("the file must exist")
            .permissions()
            .mode()
            & 0o777
    }

    #[tokio::test]
    async fn shared_level_overrides_user_and_user_shows_again_after_removal() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let depot = tempfile::tempdir().expect("a temporary repository must be created");
        let layers = avec_depot(home.path(), depot.path());
        let catalogue = FileCatalog::new(&layers);
        catalogue
            .put_agent(&agent("a", "utilisateur"))
            .await
            .expect("the user level must accept the agent");
        catalogue
            .put_agent_at(Level::Shared, &agent("a", "partage"))
            .expect("the shared level must accept the agent");

        let lu = catalogue
            .get_agent("a")
            .await
            .expect("the lookup must succeed")
            .expect("the agent must be found");
        assert_eq!(lu.prompt(), "partage", "the shared level must win");

        assert!(
            catalogue
                .delete_agent(Level::Shared, "a")
                .expect("the shared removal must succeed"),
            "the shared file must have existed"
        );
        let repli = catalogue
            .get_agent("a")
            .await
            .expect("the lookup must succeed")
            .expect("the agent must be found");
        assert_eq!(
            repli.prompt(),
            "utilisateur",
            "once removed from shared, the user definition must come back"
        );
    }

    #[tokio::test]
    async fn shipped_agent_is_served_until_a_file_overrides_it() {
        // Scenario « agent livré et surcharge » (`catalog_store.sdd`). Ordre de lecture : partagé, puis utilisateur (`lire`),
        // puis l'agent livré en repli. Le partagé l'emporte sur l'utilisateur, qui redevient actif quand le partagé est supprimé.
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let racine = tempfile::tempdir().expect("a temporary repository must be created");
        let depot = avec_depot(home.path(), racine.path());
        let catalogue = FileCatalog::new(&depot);

        let livre = catalogue
            .get_agent("vanyline-cli")
            .await
            .expect("the lookup must succeed")
            .expect("the shipped agent must be served without any file");
        assert_eq!(livre.id(), "vanyline-cli");
        assert_eq!(livre.kind(), AgentKind::Work);
        assert!(!livre.prompt().trim().is_empty());

        catalogue
            .put_agent_at(Level::User, &agent("vanyline-cli", "surcharge utilisateur"))
            .expect("the user level must accept the agent");
        let utilisateur = catalogue
            .get_agent("vanyline-cli")
            .await
            .expect("the lookup must succeed")
            .expect("the user agent must be found");
        assert_eq!(utilisateur.prompt(), "surcharge utilisateur");

        catalogue
            .put_agent_at(
                Level::Shared,
                &agent("vanyline-cli", "agent partagé du dépôt"),
            )
            .expect("the shared level must accept the agent");
        let partage = catalogue
            .get_agent("vanyline-cli")
            .await
            .expect("the lookup must succeed")
            .expect("the shared agent must be found");
        assert_eq!(partage.prompt(), "agent partagé du dépôt");
    }

    #[tokio::test]
    async fn shipped_agent_is_never_written_nor_listed() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = sans_depot(home.path());
        let catalogue = FileCatalog::new(&layers);

        let _ = catalogue
            .get_agent("vanyline-cli")
            .await
            .expect("the lookup must succeed");

        assert!(
            catalogue.list_agents().is_empty(),
            "the shipped agent must not be listed as a file"
        );
        assert!(
            !home
                .path()
                .join(".config/vanyline/agents/vanyline-cli.md")
                .exists(),
            "the shipped agent must never be written on disk"
        );
    }

    #[tokio::test]
    async fn agent_round_trips_through_its_markdown_file() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = sans_depot(home.path());
        let catalogue = FileCatalog::new(&layers);
        let original = agent("doc", "Ligne un\n\nLigne deux\n");

        catalogue
            .put_agent(&original)
            .await
            .expect("a valid agent must be accepted");

        let lu = catalogue
            .get_agent("doc")
            .await
            .expect("the lookup must succeed")
            .expect("the agent must be found");
        assert_eq!(
            lu, original,
            "the definition read back must equal the one written"
        );
        let fichier = home.path().join(".config/vanyline/agents/doc.md");
        let texte = fs::read_to_string(&fichier).expect("the agent must be a markdown file");
        assert!(
            texte.starts_with("---\n"),
            "the file must open with a YAML frontmatter: {texte}"
        );
    }

    #[test]
    fn provider_holding_a_secret_is_refused_and_nothing_is_written() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = sans_depot(home.path());
        let catalogue = FileCatalog::new(&layers);
        let provider = Provider::new_with_detector(
            "distant".to_owned(),
            "openai".to_owned(),
            "sk-abc123".to_owned(),
            None,
            ProviderLevel::User,
            &TokDetector,
        )
        .expect("the permissive host detector must accept the provider");

        let resultat = catalogue.put_provider_at(Level::User, &provider);

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "a provider whose address looks like a secret must be refused, got {resultat:?}"
        );
        assert!(
            !home.path().join(".config/vanyline/providers").exists(),
            "a refused provider must not create any file"
        );
        let trait_resultat = futures_block(catalogue.put_provider(&provider));
        assert!(
            trait_resultat.is_err(),
            "the library trait must refuse the same provider"
        );
        assert!(
            DefaultSecretDetector::default().looks_like_secret("sk-abc123"),
            "the default detector also reads sk- as a secret"
        );
    }

    #[tokio::test]
    async fn hostile_names_are_refused_before_any_disk_access() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = sans_depot(home.path());
        let catalogue = FileCatalog::new(&layers);
        let hostile = agent("../x", "piege");

        assert!(
            matches!(
                catalogue.put_agent_at(Level::User, &hostile),
                Err(HostError::Invalid { .. })
            ),
            "a name that leaves its directory must be refused on write"
        );
        assert!(
            catalogue.get_agent("../x").await.is_err(),
            "a name that leaves its directory must be refused on read"
        );
        assert!(
            !home.path().join(".config/vanyline/x.md").exists(),
            "no file must be written outside the agents directory"
        );
    }

    #[test]
    fn unreadable_file_is_reported_and_the_others_are_listed() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let layers = sans_depot(home.path());
        let catalogue = FileCatalog::new(&layers);
        catalogue
            .put_agent_at(Level::User, &agent("bon", "ok"))
            .expect("the valid agent must be written");
        let dossier = home.path().join(".config/vanyline/agents");
        fs::write(dossier.join("casse.md"), "sans frontmatter\n")
            .expect("the broken fixture must be written");

        let liste = catalogue.list_agents();

        assert_eq!(liste.len(), 2, "both files must be listed, one by one");
        let valide = liste.iter().filter_map(|entree| entree.as_ref().ok());
        assert!(
            valide.clone().any(|entree| entree.valeur.id() == "bon"),
            "the valid agent must be listed with its level"
        );
        assert!(
            liste.iter().any(|entree| matches!(
                entree,
                Err(HostError::Format { chemin, .. }) if chemin.ends_with("casse.md")
            )),
            "the broken file must be reported with its path"
        );
    }

    #[test]
    fn shared_files_are_0644_and_user_files_are_0600() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let depot = tempfile::tempdir().expect("a temporary repository must be created");
        let catalogue = FileCatalog::new(&avec_depot(home.path(), depot.path()));

        catalogue
            .put_agent_at(Level::Shared, &agent("partage", "p"))
            .expect("the shared write must succeed");
        catalogue
            .put_agent_at(Level::User, &agent("prive", "p"))
            .expect("the user write must succeed");

        assert_eq!(
            mode(&depot.path().join(".vanyline/agents/partage.md")),
            0o644,
            "a shared file must be 0644"
        );
        assert_eq!(
            mode(&home.path().join(".config/vanyline/agents/prive.md")),
            0o600,
            "a user file must be 0600"
        );
    }

    #[test]
    fn writing_to_a_missing_shared_level_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let catalogue = FileCatalog::new(&sans_depot(home.path()));

        let resultat = catalogue.put_agent_at(Level::Shared, &agent("a", "p"));

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "outside a repository the shared level must refuse writes, got {resultat:?}"
        );
    }

    #[tokio::test]
    async fn profile_and_provider_round_trip_in_their_yaml_files() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let catalogue = FileCatalog::new(&sans_depot(home.path()));
        let profil = ModelProfile::new(
            "rapide".to_owned(),
            "distant".to_owned(),
            "modele-x".to_owned(),
        );
        let provider = Provider::new(
            "distant".to_owned(),
            "openai".to_owned(),
            "https://exemple.invalide/v1".to_owned(),
            Some("cle-secrete-nom".to_owned()),
            ProviderLevel::User,
        )
        .expect("a provider holding only references must be built");

        catalogue
            .put_profile(&profil)
            .await
            .expect("the profile must be accepted");
        catalogue
            .put_provider(&provider)
            .await
            .expect("the provider must be accepted");

        assert_eq!(
            catalogue
                .get_profile("rapide")
                .await
                .expect("the profile lookup must succeed"),
            Some(profil),
            "the profile must read back identical"
        );
        assert_eq!(
            catalogue
                .get_provider("distant")
                .await
                .expect("the provider lookup must succeed"),
            Some(provider),
            "the provider must read back identical"
        );
        assert!(
            home.path()
                .join(".config/vanyline/providers/distant.yaml")
                .is_file(),
            "a provider must be a YAML file named after it"
        );
    }

    #[tokio::test]
    async fn symlinked_definition_is_not_followed() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let hors: PathBuf = home.path().join("hors.md");
        fs::write(&hors, "---\nid: x\nkind: work\n---\nprompt\n")
            .expect("the outside file must be written");
        let dossier = home.path().join(".config/vanyline/agents");
        fs::create_dir_all(&dossier).expect("the agents directory must be created");
        symlink(&hors, dossier.join("lien.md")).expect("the symbolic link must be created");
        let catalogue = FileCatalog::new(&sans_depot(home.path()));

        let resultat = catalogue.get_agent("lien").await;

        assert!(
            matches!(resultat, Err(AgentsError::Store(_))),
            "a symbolic link must be refused, not followed, got {resultat:?}"
        );
    }

    /// Attend un futur prêt, sans runtime : les stores de l'hôte ne font pas d'attente.
    fn futures_block<F: std::future::Future>(futur: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime must be built");
        runtime.block_on(futur)
    }
}
