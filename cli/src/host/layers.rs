//! Racines de fichiers de l'hôte : niveau utilisateur, niveau partagé et état. Contrat : `host/layers.sdd`.

use std::path::{Path, PathBuf};

/// Nom du répertoire partagé à la racine d'un dépôt.
const SHARED_DIR: &str = ".vanyline";

/// Niveau de configuration (la session est un troisième niveau porté par son état, pas par un répertoire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// `$HOME/.config/vanyline/`.
    User,
    /// `.vanyline/` du dépôt courant, ou de `--repo`.
    Shared,
}

/// Catégorie de configuration : un répertoire par catégorie sous chaque niveau.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// `agents/`.
    Agents,
    /// `profiles/`.
    Profiles,
    /// `providers/`.
    Providers,
    /// `toolsets/`.
    Toolsets,
    /// `toolsets-drafts/`.
    ToolsetDrafts,
    /// `context-types/`.
    ContextTypes,
    /// `contexts/`.
    Contexts,
    /// `memories/`.
    Memories,
}

impl Category {
    /// Nom du répertoire de la catégorie.
    #[must_use]
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Profiles => "profiles",
            Self::Providers => "providers",
            Self::Toolsets => "toolsets",
            Self::ToolsetDrafts => "toolsets-drafts",
            Self::ContextTypes => "context-types",
            Self::Contexts => "contexts",
            Self::Memories => "memories",
        }
    }
}

/// Racines de fichiers de l'hôte. Construit par [`Layers::discover`] ; ne crée rien.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layers {
    user: PathBuf,
    shared: Option<PathBuf>,
    state: PathBuf,
}

impl Layers {
    /// Découvre les racines. `exists` dit si un chemin existe ; il sert à trouver le `.git` du dépôt en remontant depuis
    /// `cwd`. `repo` (`--repo`) désigne le dépôt et l'emporte sur la recherche. Sans dépôt, `shared` vaut `None`.
    #[must_use]
    pub fn discover(
        home: &Path,
        cwd: &Path,
        repo: Option<&Path>,
        exists: impl Fn(&Path) -> bool,
    ) -> Self {
        let shared = match repo {
            Some(depot) => Some(depot.join(SHARED_DIR)),
            None => cwd
                .ancestors()
                .find(|dir| exists(&dir.join(".git")))
                .map(|racine| racine.join(SHARED_DIR)),
        };
        Self {
            user: home.join(".config").join("vanyline"),
            shared,
            state: home.join(".local").join("state").join("vanyline"),
        }
    }

    /// Racine du niveau utilisateur.
    #[must_use]
    pub fn user(&self) -> &Path {
        &self.user
    }

    /// Racine du niveau partagé, `None` hors dépôt.
    #[must_use]
    pub fn shared(&self) -> Option<&Path> {
        self.shared.as_deref()
    }

    /// Racine de l'état local (`sessions/`, `audit/`, `locks/` en dessous, gérés par leurs modules).
    #[must_use]
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Racine d'un niveau de configuration, `None` si ce niveau n'existe pas.
    #[must_use]
    pub fn root(&self, level: Level) -> Option<&Path> {
        match level {
            Level::User => Some(self.user()),
            Level::Shared => self.shared(),
        }
    }

    /// Répertoire d'une catégorie sous un niveau, `None` si ce niveau n'existe pas.
    #[must_use]
    pub fn dir(&self, level: Level, category: Category) -> Option<PathBuf> {
        self.root(level)
            .map(|racine| racine.join(category.dir_name()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Category, Layers, Level};

    const HOME: &str = "/home/dev";

    #[test]
    fn repository_is_found_from_a_subdirectory() {
        let racine = Path::new("/work/projet");
        let cwd = racine.join("src").join("a").join("b");
        let git = racine.join(".git");

        let layers = Layers::discover(Path::new(HOME), &cwd, None, |chemin| chemin == git);

        assert_eq!(
            layers.shared(),
            Some(racine.join(".vanyline").as_path()),
            "the shared level must be the .vanyline directory at the root of the repository"
        );
        assert_eq!(
            layers.user(),
            Path::new("/home/dev/.config/vanyline"),
            "the user level must stay under $HOME/.config/vanyline"
        );
        assert_eq!(
            layers.state(),
            Path::new("/home/dev/.local/state/vanyline"),
            "the state directory must stay under $HOME/.local/state/vanyline"
        );
    }

    #[test]
    fn repository_option_takes_precedence_over_the_search() {
        let depot_a = Path::new("/work/a");
        let depot_b = Path::new("/work/b");
        let cwd = depot_a.join("src");
        let git_a = depot_a.join(".git");

        let layers = Layers::discover(Path::new(HOME), &cwd, Some(depot_b), |chemin| {
            chemin == git_a
        });

        assert_eq!(
            layers.shared(),
            Some(depot_b.join(".vanyline").as_path()),
            "the repository given by --repo must win over the repository found from the current directory"
        );
    }

    #[test]
    fn without_repository_only_the_user_level_exists() {
        let layers = Layers::discover(Path::new(HOME), Path::new("/tmp/hors-depot"), None, |_| {
            false
        });

        assert_eq!(
            layers.shared(),
            None,
            "outside any repository the shared level must be None"
        );
        assert_eq!(
            layers.dir(Level::Shared, Category::Agents),
            None,
            "a directory of a missing level must be None"
        );
        assert_eq!(
            layers.dir(Level::User, Category::Agents),
            Some(PathBuf::from("/home/dev/.config/vanyline/agents")),
            "the user level must be unchanged without a repository"
        );
    }

    #[test]
    fn category_directories_have_their_names() {
        let layers = Layers::discover(Path::new(HOME), Path::new("/work/projet"), None, |chemin| {
            chemin == Path::new("/work/projet/.git")
        });
        let racine = Path::new("/work/projet/.vanyline");
        let attendus = [
            (Category::Agents, "agents"),
            (Category::Profiles, "profiles"),
            (Category::Providers, "providers"),
            (Category::Toolsets, "toolsets"),
            (Category::ToolsetDrafts, "toolsets-drafts"),
            (Category::ContextTypes, "context-types"),
            (Category::Contexts, "contexts"),
            (Category::Memories, "memories"),
        ];

        for (categorie, nom) in attendus {
            assert_eq!(
                layers.dir(Level::Shared, categorie),
                Some(racine.join(nom)),
                "the shared directory of the category must be named `{nom}`"
            );
            assert_eq!(
                layers.dir(Level::User, categorie),
                Some(Path::new("/home/dev/.config/vanyline").join(nom)),
                "the user directory of the category must be named `{nom}`"
            );
        }
    }
}
