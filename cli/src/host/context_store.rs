//! Types de contexte et contextes sur des fichiers YAML, en deux niveaux. Contrat : `host/context_store.sdd`.

use std::future::{self, Future};

use lymaille::{
    ContextStore, ContextType, ContextTypeStore, DefaultSecretDetector, ToolsetContext,
    ToolsetsError, ToolsetsResult,
};
use serde::Serialize;

use super::HostError;
use super::catalog_store::{Entree, check_name, ecrire, encode, lire, lister, supprimer};
use super::layers::{Category, Layers, Level};

/// Extension des types et des contextes (YAML).
const YAML_EXT: &str = "yaml";
/// Nom du type de contexte livré avec la CLI (`context_derive.sdd`) : il n'est jamais écrit sur disque.
const SHIPPED_TYPE: &str = "vanyline-cli";

/// Marqueur d'un type de contexte livré avec la CLI, refusé à l'écriture.
const SHIPPED: &str = "VNL-CLI-HST-028";
/// Marqueur d'une valeur de contexte qui ressemble à un secret.
const SECRET: &str = "VNL-CLI-HST-029";
/// Marqueur d'un nom de contexte absent de la forme lisible.
const NAME: &str = "VNL-CLI-HST-030";

/// Stockage des types de contexte et des contextes. Ne crée rien à la construction.
#[derive(Debug, Clone)]
pub struct FileContexts {
    layers: Layers,
}

impl FileContexts {
    /// Stockage rangé sous les niveaux de `layers`.
    #[must_use]
    pub fn new(layers: &Layers) -> Self {
        Self {
            layers: layers.clone(),
        }
    }

    /// Écrit un type de contexte au niveau `niveau`.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name or the type shipped with the CLI, [`HostError::Io`] for the
    /// write.
    pub fn put_type_at(&self, niveau: Level, context_type: &ContextType) -> Result<(), HostError> {
        let nom = nom_de(context_type)?;
        check_name(&nom)?;
        if nom == SHIPPED_TYPE {
            return Err(HostError::Invalid {
                cause: format!("{SHIPPED}: the type shipped with the CLI is never written"),
            });
        }
        let texte = encode(context_type)?;
        ecrire(
            &self.layers,
            niveau,
            Category::ContextTypes,
            YAML_EXT,
            &nom,
            &texte,
        )
    }

    /// Écrit un contexte au niveau `niveau`. Refuse une valeur qui ressemble à un secret.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name or a secret-like value, [`HostError::Io`] for the write.
    pub fn put_context_at(&self, niveau: Level, context: &ToolsetContext) -> Result<(), HostError> {
        let nom = nom_de(context)?;
        check_name(&nom)?;
        if !context.check(&DefaultSecretDetector::default()) {
            return Err(HostError::Invalid {
                cause: format!(
                    "{SECRET}: a value of context `{nom}` looks like a secret; name the secret instead"
                ),
            });
        }
        let texte = encode(context)?;
        ecrire(
            &self.layers,
            niveau,
            Category::Contexts,
            YAML_EXT,
            &nom,
            &texte,
        )
    }

    /// Liste les types de contexte de tous les niveaux.
    #[must_use]
    pub fn list_types(&self) -> Vec<Result<Entree<ContextType>, HostError>> {
        lister(&self.layers, Category::ContextTypes, YAML_EXT, decode_type)
    }

    /// Liste les contextes de tous les niveaux.
    #[must_use]
    pub fn list_contexts(&self) -> Vec<Result<Entree<ToolsetContext>, HostError>> {
        lister(&self.layers, Category::Contexts, YAML_EXT, decode_context)
    }

    /// Supprime un type de contexte d'un niveau. `true` s'il existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name, [`HostError::Io`] for the removal.
    pub fn delete_type(&self, niveau: Level, nom: &str) -> Result<bool, HostError> {
        supprimer(&self.layers, niveau, Category::ContextTypes, YAML_EXT, nom)
    }

    /// Supprime un contexte d'un niveau. `true` s'il existait.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Invalid`] for a refused name, [`HostError::Io`] for the removal.
    pub fn delete_context(&self, niveau: Level, nom: &str) -> Result<bool, HostError> {
        supprimer(&self.layers, niveau, Category::Contexts, YAML_EXT, nom)
    }
}

/// Nom d'une valeur, lu de sa forme sérialisée : lymaille ne l'expose pas hors de sa crate.
pub(super) fn nom_de<T: Serialize>(valeur: &T) -> Result<String, HostError> {
    let forme = serde_json::to_value(valeur).map_err(|erreur| HostError::Invalid {
        cause: format!("{NAME}: {erreur}"),
    })?;
    forme
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| HostError::Invalid {
            cause: format!("{NAME}: the object has no name"),
        })
}

fn decode_type(texte: &str) -> Result<ContextType, String> {
    yaml_serde::from_str(texte).map_err(|erreur| erreur.to_string())
}

fn decode_context(texte: &str) -> Result<ToolsetContext, String> {
    yaml_serde::from_str(texte).map_err(|erreur| erreur.to_string())
}

/// Erreur de store de la bibliothèque, pour un refus de l'hôte.
fn lymaille_store(erreur: &HostError) -> ToolsetsError {
    ToolsetsError::Store(erreur.to_string())
}

impl ContextTypeStore for FileContexts {
    fn get_type(
        &self,
        name: &str,
    ) -> impl Future<Output = ToolsetsResult<Option<ContextType>>> + Send {
        future::ready(
            lire(
                &self.layers,
                Category::ContextTypes,
                YAML_EXT,
                name,
                decode_type,
            )
            .map_err(|erreur| lymaille_store(&erreur)),
        )
    }

    fn put_type(
        &self,
        context_type: &ContextType,
    ) -> impl Future<Output = ToolsetsResult<()>> + Send {
        future::ready(
            self.put_type_at(Level::User, context_type)
                .map_err(|erreur| lymaille_store(&erreur)),
        )
    }
}

impl ContextStore for FileContexts {
    fn get_context(
        &self,
        name: &str,
    ) -> impl Future<Output = ToolsetsResult<Option<ToolsetContext>>> + Send {
        future::ready(
            lire(
                &self.layers,
                Category::Contexts,
                YAML_EXT,
                name,
                decode_context,
            )
            .map_err(|erreur| lymaille_store(&erreur)),
        )
    }

    fn put_context(
        &self,
        context: &ToolsetContext,
    ) -> impl Future<Output = ToolsetsResult<()>> + Send {
        future::ready(
            self.put_context_at(Level::User, context)
                .map_err(|erreur| lymaille_store(&erreur)),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use lymaille::{
        ContextLevel, ContextStore as _, ContextType, ContextTypeStore as _, SecretDetector,
        ToolsetContext,
    };
    use serde_json::json;

    use super::{FileContexts, HostError};
    use crate::host::layers::{Layers, Level};

    /// Détecteur de l'hôte qui ne connaît que le préfixe `tok_` : `sk-abc` y passe.
    struct TokDetector;

    impl SecretDetector for TokDetector {
        fn looks_like_secret(&self, text: &str) -> bool {
            text.starts_with("tok_")
        }
    }

    fn sans_depot(home: &Path) -> Layers {
        Layers::discover(home, home, None, |_| false)
    }

    fn avec_depot(home: &Path, racine: &Path) -> Layers {
        let git = racine.join(".git");
        Layers::discover(home, racine, None, |chemin| chemin == git)
    }

    /// Type de contexte valide, construit par son format lisible.
    fn type_nomme(nom: &str) -> ContextType {
        serde_json::from_value(json!({
            "name": nom,
            "version": "1.0.0",
            "schema": {"type": "object", "properties": {"langue": {"type": "string"}}},
            "roles": [],
            "includes": []
        }))
        .expect("a valid context type must read from its form")
    }

    /// Contexte utilisateur avec un paramètre `langue`.
    fn contexte_nomme(nom: &str, langue: &str) -> ToolsetContext {
        ToolsetContext::new(nom.to_owned(), ContextLevel::User)
            .expect("a named context must be built")
            .with_parameter("langue".to_owned(), json!(langue))
            .expect("a plain parameter must be accepted")
    }

    #[tokio::test]
    async fn type_and_context_round_trip_as_readable_yaml() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileContexts::new(&sans_depot(home.path()));
        let type_original = type_nomme("doc-type");
        let contexte = contexte_nomme("ctx-a", "fr");

        stock
            .put_type(&type_original)
            .await
            .expect("a valid type must be stored");
        stock
            .put_context(&contexte)
            .await
            .expect("a valid context must be stored");

        assert_eq!(
            stock
                .get_type("doc-type")
                .await
                .expect("the type lookup must succeed"),
            Some(type_original),
            "the type read back must equal the one written"
        );
        assert_eq!(
            stock
                .get_context("ctx-a")
                .await
                .expect("the context lookup must succeed"),
            Some(contexte),
            "the context read back must equal the one written"
        );
        let racine = home.path().join(".config/vanyline");
        let texte = fs::read_to_string(racine.join("contexts/ctx-a.yaml"))
            .expect("the context must be a YAML file named after it");
        assert!(
            texte.contains("name: ctx-a"),
            "the file must be readable by hand: {texte}"
        );
        assert!(
            racine.join("context-types/doc-type.yaml").is_file(),
            "a type must be a YAML file in context-types/"
        );
    }

    #[tokio::test]
    async fn shared_context_wins_then_user_shows_after_removal() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let depot = tempfile::tempdir().expect("a temporary repository must be created");
        let stock = FileContexts::new(&avec_depot(home.path(), depot.path()));
        stock
            .put_context(&contexte_nomme("c", "utilisateur"))
            .await
            .expect("the user context must be stored");
        let partage = contexte_nomme("c", "partage");
        stock
            .put_context_at(Level::Shared, &partage)
            .expect("the shared context must be stored");

        assert_eq!(
            stock
                .get_context("c")
                .await
                .expect("the lookup must succeed"),
            Some(partage),
            "the shared context must answer first"
        );
        assert!(
            stock
                .delete_context(Level::Shared, "c")
                .expect("the shared removal must succeed"),
            "the shared file must have existed"
        );
        assert_eq!(
            stock
                .get_context("c")
                .await
                .expect("the lookup must succeed"),
            Some(contexte_nomme("c", "utilisateur")),
            "without the shared context, the user context must answer"
        );
    }

    #[tokio::test]
    async fn context_holding_a_secret_is_refused_and_nothing_is_written() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileContexts::new(&sans_depot(home.path()));
        let mut contexte = ToolsetContext::new("cle".to_owned(), ContextLevel::User)
            .expect("the context must be built")
            .with_secret_detector(Arc::new(TokDetector));
        contexte = contexte
            .with_parameter("cle".to_owned(), json!("sk-abc123"))
            .expect("the permissive host detector must accept the parameter");

        let resultat = stock.put_context_at(Level::User, &contexte);

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "a context whose value looks like a secret must be refused, got {resultat:?}"
        );
        assert!(
            !home.path().join(".config/vanyline/contexts").exists(),
            "a refused context must not create any file"
        );
    }

    #[test]
    fn the_type_shipped_with_the_cli_is_never_written() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileContexts::new(&sans_depot(home.path()));

        let resultat = stock.put_type_at(Level::User, &type_nomme("vanyline-cli"));

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "the shipped type must never be written to a configuration level, got {resultat:?}"
        );
        assert!(
            !home.path().join(".config/vanyline/context-types").exists(),
            "no file must be written for the shipped type"
        );
    }

    #[test]
    fn a_context_name_that_leaves_its_directory_is_refused() {
        let home = tempfile::tempdir().expect("a temporary home must be created");
        let stock = FileContexts::new(&sans_depot(home.path()));

        let resultat = stock.put_context_at(Level::User, &contexte_nomme("../x", "fr"));

        assert!(
            matches!(resultat, Err(HostError::Invalid { .. })),
            "a name that leaves its directory must be refused, got {resultat:?}"
        );
        assert!(
            !home.path().join(".config/vanyline/x.yaml").exists(),
            "no file must be written outside the contexts directory"
        );
    }
}
