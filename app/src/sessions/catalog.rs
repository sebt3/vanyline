//! Catalogue publié au démarrage — contrat : `catalog.sdd`.
//!
//! Deux contextes stockés `G_home` / `G_config` (types du même nom, nature et agent par
//! défaut portés par le type) et les agents `G_vanyline_*`, publiés create-only idempotents
//! (`DEC-7`, `FR-6`), plus la table compilée écran → agent pour les contextes `H_` (`DEC-6`).

use lymaille::{
    AgentDefinition, AgentKind, ContextLevel, ContextType, SessionKind, ToolsetContext,
};
use miryad_core::lymaille::host::ScriptGrants;
use miryad_core::lymaille::store::publish::{
    Provenance, ensure_agent, ensure_catalog_context, ensure_context_type,
};
use sea_orm::DatabaseConnection;

use super::host::{parse_project_id, parse_sandbox_id};
use crate::error::AppError;

/// Source déclarée de la publication : la marque `managed_by` des lignes publiées.
const SOURCE_CATALOGUE: &str = "vanyline";
/// Version du schéma du type de contexte (champ `version` du `ContextType` lymaille), sans
/// rapport avec la version de provenance de la publication.
const VERSION_TYPE: &str = "1.0.0";

/// Publie le catalogue par défaut, à la version de l'app (`CARGO_PKG_VERSION`) : relancé, c'est
/// un no-op ; à une version supérieure, les lignes gérées non modifiées sont mises à jour et
/// les lignes d'un administrateur — ou modifiées par lui — restent intactes (`FR-6`, sémantique
/// de `miryad_core::lymaille::store::publish`). Ouverture par tout utilisateur authentifié
/// (`DEC-7`) : les lignes du catalogue sont `Public` en lecture chez miryad-core et le contexte
/// ne porte pas de groupes. Branché au démarrage en B9 (`mod.sdd` Tasks).
pub(crate) async fn ensure_catalog(db: &DatabaseConnection) -> Result<(), AppError> {
    ensure_catalog_version(db, env!("CARGO_PKG_VERSION")).await
}

/// [`ensure_catalog`] à une version de provenance explicite — seam des tests de montée de
/// version.
pub(crate) async fn ensure_catalog_version(
    db: &DatabaseConnection,
    version: &str,
) -> Result<(), AppError> {
    let provenance = Provenance {
        source: SOURCE_CATALOGUE.to_string(),
        version: version.to_string(),
    };
    publier_type(db, &provenance, G_HOME, G_VANYLINE_HOME, SessionKind::Work).await?;
    publier_contexte(db, &provenance, G_HOME).await?;
    publier_type(
        db,
        &provenance,
        G_CONFIG,
        G_VANYLINE_CONFIG,
        SessionKind::Config,
    )
    .await?;
    publier_contexte(db, &provenance, G_CONFIG).await?;
    // Types d'écran sans contexte stocké : ils portent les `script_grants` des contextes `H_…`
    // (`host.sdd`, ticket #70).
    publier_type(
        db,
        &provenance,
        G_PROJECT,
        G_VANYLINE_PROJECT,
        SessionKind::Work,
    )
    .await?;
    publier_type(
        db,
        &provenance,
        G_SANDBOX,
        G_VANYLINE_SANDBOX,
        SessionKind::Work,
    )
    .await?;
    for (id, nature, prompt) in [
        (
            G_VANYLINE_HOME,
            AgentKind::Work,
            "Agent productif par défaut de l'accueil de vanyline.",
        ),
        (
            G_VANYLINE_CONFIG,
            AgentKind::Config,
            "Agent de configuration par défaut de vanyline.",
        ),
        (
            G_VANYLINE_PROJECT,
            AgentKind::Work,
            "Agent productif par défaut des projets de vanyline.",
        ),
        (
            G_VANYLINE_SANDBOX,
            AgentKind::Work,
            "Agent productif par défaut des sandboxes de vanyline.",
        ),
    ] {
        publier_agent(db, &provenance, id, nature, prompt).await?;
    }
    Ok(())
}

/// Correspondance compilée écran → agent pour les contextes `H_` (`DEC-6`) ; `None` pour un
/// contexte stocké, dont la nature et l'agent viennent du type lui-même (`Exposes`). La table
/// n'est ni administrable ni lisible par une session `Config` : constante compilée, isolée ici.
/// Appelant de production = l'ouverture par écran (tranche DEC-8, frontend + relais) ; B9 la
/// branche côté tests — tooling.sdd
#[allow(dead_code)]
#[must_use]
pub(crate) fn agent_pour_contexte(context: &str) -> Option<&'static str> {
    if parse_project_id(context).is_some() {
        return Some(G_VANYLINE_PROJECT);
    }
    if parse_sandbox_id(context).is_some() {
        return Some(G_VANYLINE_SANDBOX);
    }
    None
}

/// Publie le type de contexte `nom` : schéma minimal sans paramètre, nature et agent par
/// défaut portés par le type (`FR-6`), `allowed_agents` = l'agent par défaut (condition de
/// l'ouverture par défaut chez miryad-core), `script_grants` vides (aucun droit de script par
/// défaut, `widgets#OI-3`). La colonne `context_type` des contextes référence le type par ce
/// nom `G_` : c'est ce que résout l'ouverture de session de miryad-core.
async fn publier_type(
    db: &DatabaseConnection,
    provenance: &Provenance,
    nom: &str,
    agent: &str,
    nature: SessionKind,
) -> Result<(), AppError> {
    let definition = serde_json::from_value::<ContextType>(serde_json::json!({
        "name": nom,
        "version": VERSION_TYPE,
        "schema": { "type": "object" },
        "roles": [],
        "includes": [],
        "default_agent": agent,
        "session_kind": nature,
    }))
    .map_err(panne_crate)?;
    ensure_context_type(
        db,
        provenance,
        &definition,
        &[agent.to_string()],
        &ScriptGrants::default(),
    )
    .await
    .map(|_| ())
    .map_err(panne_crate)
}

/// Publie le contexte `nom`, de niveau `Shared`, référencé par son propre nom comme type
/// (`DEC-7`) : aucun paramètre, aucun rôle pour les deux contextes d'écran.
async fn publier_contexte(
    db: &DatabaseConnection,
    provenance: &Provenance,
    nom: &str,
) -> Result<(), AppError> {
    let contexte =
        ToolsetContext::new(nom.to_string(), ContextLevel::Shared).map_err(panne_crate)?;
    ensure_catalog_context(db, provenance, &contexte, Some(nom))
        .await
        .map(|_| ())
        .map_err(panne_crate)
}

/// Publie l'agent `id` : définition minimale — nom, nature, prompt d'une phrase, **sans
/// profil** (le profil vient de `default_profile` de l'hôte, `host.sdd`).
async fn publier_agent(
    db: &DatabaseConnection,
    provenance: &Provenance,
    id: &str,
    nature: AgentKind,
    prompt: &str,
) -> Result<(), AppError> {
    let definition = AgentDefinition::new(id.to_string(), nature, prompt.to_string());
    ensure_agent(db, provenance, &definition)
        .await
        .map(|_| ())
        .map_err(panne_crate)
}

/// Panne de publication ou de construction d'une définition — implémentation interne à l'app,
/// jamais un refus d'appel utilisateur.
fn panne_crate(cause: impl std::fmt::Display) -> AppError {
    AppError::InternalError(format!(
        "VNL-APP-SES-108: catalogue — publication refusée: {cause}"
    ))
}

/// Identifiant du contexte de l'accueil (`DEC-7`).
pub(crate) const G_HOME: &str = "G_home";
/// Identifiant du contexte de configuration (`DEC-7`).
pub(crate) const G_CONFIG: &str = "G_config";
/// Type de contexte porteur des `script_grants` des écrans projet (`host.sdd`, #70).
pub(crate) const G_PROJECT: &str = "G_project";
/// Type de contexte porteur des `script_grants` des écrans sandbox (`host.sdd`, #70).
pub(crate) const G_SANDBOX: &str = "G_sandbox";
/// Agent productif par défaut de l'accueil (`DEC-6`, `DEC-7`).
pub(crate) const G_VANYLINE_HOME: &str = "G_vanyline_home";
/// Agent de configuration par défaut (`DEC-7`).
pub(crate) const G_VANYLINE_CONFIG: &str = "G_vanyline_config";
/// Agent productif par défaut des projets — désigné par la table écran → agent (`DEC-6`).
pub(crate) const G_VANYLINE_PROJECT: &str = "G_vanyline_project";
/// Agent productif par défaut des sandboxes — désigné par la table écran → agent (`DEC-6`).
pub(crate) const G_VANYLINE_SANDBOX: &str = "G_vanyline_sandbox";

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::test_support::real_db;
    use crate::sessions::host::{h_project_id, h_sandbox_id};
    use chrono::{DateTime, Utc};
    use lymaille::{AgentDefinition, AgentKind, ContextType, SessionKind, ToolsetContext};
    use miryad_core::lymaille::definitions::{agent, catalog_context, context_type};
    use sea_orm::EntityTrait;
    use sea_orm::QueryFilter;
    use sea_orm::{ActiveValue::NotSet, ActiveValue::Set, ColumnTrait};

    /// Identifiants attendus au catalogue : quatre types (dont `G_project`/`G_sandbox`, sans
    /// contexte stocké), deux contextes, quatre agents.
    const TYPES_ATTENDUS: [&str; 4] = [G_HOME, G_CONFIG, G_PROJECT, G_SANDBOX];
    const CONTEXTES_ATTENDUS: [&str; 2] = [G_HOME, G_CONFIG];
    const AGENTS_ATTENDUS: [&str; 4] = [
        G_VANYLINE_CONFIG,
        G_VANYLINE_HOME,
        G_VANYLINE_PROJECT,
        G_VANYLINE_SANDBOX,
    ];

    /// Snapshot du catalogue : `(table, identifiant, created_at)` de chaque ligne, trié — la
    /// preuve d'idempotence compare les lignes elles-mêmes, pas seulement les comptes.
    async fn snapshot(db: &DatabaseConnection) -> Vec<(String, String, DateTime<Utc>)> {
        let mut lignes = Vec::new();
        for ligne in agent::Entity::find().all(db).await.unwrap() {
            lignes.push(("agents".to_string(), ligne.agent_id, ligne.created_at));
        }
        for ligne in context_type::Entity::find().all(db).await.unwrap() {
            lignes.push(("types".to_string(), ligne.name, ligne.created_at));
        }
        for ligne in catalog_context::Entity::find().all(db).await.unwrap() {
            lignes.push(("contextes".to_string(), ligne.name, ligne.created_at));
        }
        lignes.sort();
        lignes
    }

    /// Scenario « démarrage répété idempotent (`AC-4`) » : trois démarrages sur une base vide
    /// laissent exactement les entrées attendues, sans doublon ni variante datée.
    #[tokio::test]
    async fn demarrage_repete_idempotent() {
        let db = real_db().await;
        ensure_catalog(&db)
            .await
            .expect("premier démarrage publie le catalogue");
        let premier = snapshot(&db).await;
        ensure_catalog(&db).await.expect("second démarrage");
        ensure_catalog(&db).await.expect("troisième démarrage");
        let troisieme = snapshot(&db).await;
        assert_eq!(
            premier, troisieme,
            "relancer n'ajoute ni ne duplique rien, aucune variante datée"
        );

        let noms = |mut noms: Vec<String>| {
            noms.sort();
            noms
        };
        let attendus = |noms_attendus: &[&str]| {
            noms(noms_attendus.iter().map(|nom| (*nom).to_string()).collect())
        };
        assert_eq!(
            noms(
                context_type::Entity::find()
                    .all(&db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|ligne| ligne.name)
                    .collect(),
            ),
            attendus(&TYPES_ATTENDUS),
            "exactement quatre types de contexte"
        );
        assert_eq!(
            noms(
                catalog_context::Entity::find()
                    .all(&db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|ligne| ligne.name)
                    .collect(),
            ),
            attendus(&CONTEXTES_ATTENDUS),
            "exactement deux contextes stockés"
        );
        assert_eq!(
            noms(
                agent::Entity::find()
                    .all(&db)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|ligne| ligne.agent_id)
                    .collect(),
            ),
            attendus(&AGENTS_ATTENDUS),
            "exactement quatre agents"
        );
    }

    /// Scenario « définition administrateur intacte » : une ligne `G_vanyline_config` posée
    /// avant le démarrage traverse `ensure_catalog` inchangée (`FR-6` : create-only, jamais de
    /// patch), le reste du catalogue étant publié.
    #[tokio::test]
    async fn definition_administrateur_intacte() {
        let db = real_db().await;
        let definition_admin = serde_json::to_value(
            AgentDefinition::new(
                G_VANYLINE_CONFIG.to_string(),
                AgentKind::Config,
                "prompt administrateur".to_string(),
            )
            .with_profile("profil-admin".to_string()),
        )
        .expect("la définition de l'administrateur se sérialise");
        agent::Entity::insert(agent::ActiveModel {
            id: NotSet,
            agent_id: Set(G_VANYLINE_CONFIG.to_string()),
            kind: Set("config".to_string()),
            profile: Set("profil-admin".to_string()),
            definition: Set(definition_admin.clone()),
            created_at: Set(Utc::now()),
            managed_by: NotSet,
            managed_version: NotSet,
            managed_digest: NotSet,
        })
        .exec(&db)
        .await
        .expect("l'administrateur publie sa définition avant le démarrage");

        ensure_catalog(&db).await.expect("démarrage");

        let apres = agent::Entity::find()
            .filter(agent::Column::AgentId.eq(G_VANYLINE_CONFIG))
            .one(&db)
            .await
            .unwrap()
            .expect("la ligne de l'administrateur est toujours là");
        assert_eq!(
            apres.definition, definition_admin,
            "la publication ne patche jamais une ligne existante"
        );
        assert_eq!(
            apres.profile, "profil-admin",
            "le profil de l'administrateur est intact"
        );
        assert_eq!(
            agent::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .into_iter()
                .map(|ligne| ligne.agent_id)
                .collect::<Vec<_>>()
                .len(),
            4,
            "les trois autres agents sont publiés, sans doublon"
        );
    }

    /// Scenario « ouverture selon l'écran (`AC-4`) » : accueil et configuration ouvrent les
    /// contextes stockés typés ; projet et sandbox ouvrent leur `H_` avec l'agent de la table
    /// compilée, sans que l'appelant nomme d'agent.
    #[tokio::test]
    async fn ouverture_selon_ecran() {
        let db = real_db().await;
        ensure_catalog(&db).await.expect("démarrage");

        // Table écran → agent (`DEC-6`) : les `H_` portent l'agent, les stockés rendent `None`.
        assert_eq!(
            agent_pour_contexte(&h_project_id("mon-projet")),
            Some(G_VANYLINE_PROJECT),
            "l'écran projet ouvre sur son `H_project_` avec `G_vanyline_project`"
        );
        assert_eq!(
            agent_pour_contexte(&h_sandbox_id("alice", "mon-pod")),
            Some(G_VANYLINE_SANDBOX),
            "l'écran sandbox ouvre sur son `H_sandbox_` avec `G_vanyline_sandbox`"
        );
        assert_eq!(
            agent_pour_contexte(G_HOME),
            None,
            "un contexte stocké n'a pas d'agent dans la table"
        );
        assert_eq!(agent_pour_contexte(G_CONFIG), None, "idem configuration");

        // Accueil : `G_home` de type `G_home`, nature `Work`, agent `G_vanyline_home`.
        verifier_contexte_typ(&db, G_HOME, G_VANYLINE_HOME, SessionKind::Work).await;
        // Configuration : `G_config` de type `G_config`, nature `Config`, agent `G_vanyline_config`.
        verifier_contexte_typ(&db, G_CONFIG, G_VANYLINE_CONFIG, SessionKind::Config).await;

        // Les agents désignés par la table existent au catalogue, productifs (`FR-6`).
        for identifiant in [G_VANYLINE_PROJECT, G_VANYLINE_SANDBOX] {
            let ligne = agent::Entity::find()
                .filter(agent::Column::AgentId.eq(identifiant))
                .one(&db)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{identifiant} est publié au catalogue"));
            assert_eq!(ligne.kind, "work", "{identifiant} est un agent productif");
        }
    }

    /// Scenario « démarrage répété idempotent » (suite) : toute ligne publiée porte la marque
    /// `vanyline` et la version courante de l'app.
    #[tokio::test]
    async fn lignes_publiees_marquees_vanyline() {
        let db = real_db().await;
        ensure_catalog(&db).await.expect("démarrage");
        for ligne in agent::Entity::find().all(&db).await.unwrap() {
            assert_eq!(ligne.managed_by.as_deref(), Some("vanyline"));
            assert_eq!(
                ligne.managed_version.as_deref(),
                Some(env!("CARGO_PKG_VERSION"))
            );
        }
        for ligne in context_type::Entity::find().all(&db).await.unwrap() {
            assert_eq!(ligne.managed_by.as_deref(), Some("vanyline"));
        }
        for ligne in catalog_context::Entity::find().all(&db).await.unwrap() {
            assert_eq!(ligne.managed_by.as_deref(), Some("vanyline"));
        }
    }

    /// Scenario « les agents publiés ne désignent aucun profil » (ticket #75) : le profil vient
    /// de `default_profile` de l'hôte, jamais d'une ligne d'agent.
    #[tokio::test]
    async fn agents_publies_sans_profil() {
        let db = real_db().await;
        ensure_catalog(&db).await.expect("démarrage");
        let lignes = agent::Entity::find().all(&db).await.unwrap();
        assert_eq!(lignes.len(), AGENTS_ATTENDUS.len());
        for ligne in lignes {
            let definition = serde_json::from_value::<AgentDefinition>(ligne.definition)
                .expect("la colonne definition relit un `AgentDefinition`");
            assert_eq!(definition.profile(), None, "{} sans profil", ligne.agent_id);
            assert_eq!(
                ligne.profile, "",
                "colonne dérivée vide pour {}",
                ligne.agent_id
            );
        }
    }

    /// Scenario « les types publiés portent les droits de script attendus ».
    #[tokio::test]
    async fn types_publies_avec_script_grants_vides() {
        let db = real_db().await;
        ensure_catalog(&db).await.expect("démarrage");
        for ligne in context_type::Entity::find().all(&db).await.unwrap() {
            assert_eq!(
                ligne.script_grants,
                serde_json::json!({"toolsets": [], "modules": []}),
                "type {}",
                ligne.name
            );
        }
    }

    /// Scenario « le catalogue suit la montée de version » : une ligne gérée non modifiée est
    /// mise à jour, une ligne modifiée par un administrateur depuis sa publication reste intacte.
    #[tokio::test]
    async fn catalogue_suit_la_montee_de_version() {
        let db = real_db().await;
        ensure_catalog_version(&db, "0.0.1")
            .await
            .expect("ancienne version");

        // Un administrateur retouche l'agent projet après la publication.
        let projet = agent::Entity::find()
            .filter(agent::Column::AgentId.eq(G_VANYLINE_PROJECT))
            .one(&db)
            .await
            .unwrap()
            .expect("agent projet publié");
        let mut definition: serde_json::Value = projet.definition.clone();
        definition["prompt"] = serde_json::json!("prompt retouché par un administrateur");
        let mut active: agent::ActiveModel = projet.into();
        active.definition = Set(definition.clone());
        sea_orm::ActiveModelTrait::update(active, &db)
            .await
            .expect("retouche directe");

        ensure_catalog(&db).await.expect("version courante");

        let sandbox = agent::Entity::find()
            .filter(agent::Column::AgentId.eq(G_VANYLINE_SANDBOX))
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sandbox.managed_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION")),
            "ligne non modifiée : mise à jour"
        );
        let projet = agent::Entity::find()
            .filter(agent::Column::AgentId.eq(G_VANYLINE_PROJECT))
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(projet.definition, definition, "ligne modifiée : intacte");
        assert_eq!(projet.managed_version.as_deref(), Some("0.0.1"));
    }

    /// Vérifie le contexte stocké `nom` : sa référence de type, la nature et l'agent par défaut
    /// portés par la colonne `active` du type, l'agent membre de `allowed_agents`, et la nature
    /// de la ligne d'agent.
    async fn verifier_contexte_typ(
        db: &DatabaseConnection,
        nom: &str,
        agent_attendu: &str,
        nature: SessionKind,
    ) {
        let contexte = catalog_context::Entity::find()
            .filter(catalog_context::Column::Name.eq(nom))
            .one(db)
            .await
            .unwrap()
            .expect("le contexte est publié");
        assert_eq!(
            contexte.context_type.as_deref(),
            Some(nom),
            "le contexte {nom} référence son type"
        );
        serde_json::from_value::<ToolsetContext>(contexte.context)
            .expect("la colonne context relit un `ToolsetContext`");

        let type_ligne = context_type::Entity::find()
            .filter(context_type::Column::Name.eq(nom))
            .one(db)
            .await
            .unwrap()
            .expect("le type de contexte est publié");
        let typ = serde_json::from_value::<ContextType>(
            type_ligne.active.expect("la version active est publiée"),
        )
        .expect("la colonne active relit un `ContextType`");
        assert_eq!(
            typ.default_agent(),
            Some(agent_attendu),
            "l'agent par défaut du type {nom}"
        );
        assert_eq!(typ.session_kind(), Some(nature), "la nature du type {nom}");
        assert!(
            type_ligne
                .allowed_agents
                .as_array()
                .is_some_and(|agents| agents.iter().any(|a| a.as_str() == Some(agent_attendu))),
            "l'agent par défaut est membre de `allowed_agents` : {:?}",
            type_ligne.allowed_agents
        );

        let agent_ligne = agent::Entity::find()
            .filter(agent::Column::AgentId.eq(agent_attendu))
            .one(db)
            .await
            .unwrap()
            .expect("l'agent par défaut est publié");
        let nature_attendue = match nature {
            SessionKind::Config => "config",
            _ => "work",
        };
        assert_eq!(
            agent_ligne.kind, nature_attendue,
            "{agent_attendu} porte la nature de {nom}"
        );
    }
}
