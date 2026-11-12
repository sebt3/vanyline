//! Catalogue publié au démarrage — contrat : `catalog.sdd`.
//!
//! Deux contextes stockés `G_home` / `G_config` (types du même nom, nature et agent par
//! défaut portés par le type) et les agents `G_vanyline_*`, publiés create-only idempotents
//! (`DEC-7`, `FR-6`), plus la table compilée écran → agent pour les contextes `H_` (`DEC-6`).

use chrono::Utc;
use lymaille::{
    AgentDefinition, AgentKind, ContextLevel, ContextType, SessionKind, ToolsetContext,
};
use sea_orm::ConnectionTrait;
use sea_orm::DatabaseConnection;
use sea_orm::sea_query::{Alias, Expr, OnConflict, Query};

use super::host::{parse_project_id, parse_sandbox_id};
use crate::error::AppError;

/// Tables du catalogue posées par le `Migrator` de miryad-core (`sessions#CON-2`) — noms
/// complets, motif des stores de miryad-core (`src/lymaille/store/`).
const CATALOG_AGENTS_TABLE: &str = "miryad_lymaille_catalog_agents";
const CATALOG_CONTEXT_TYPES_TABLE: &str = "miryad_lymaille_catalog_context_types";
const CATALOG_CONTEXTS_TABLE: &str = "miryad_lymaille_catalog_contexts";

/// Version sémantique des entrées publiées ; relancée, la publication est un `no-op`, aucune
/// « variante datée » n'est ajoutée (`AC-4`).
const VERSION_CATALOGUE: &str = "1.0.0";
/// Nom du profil de modèle porté par les agents publiés. Par convention l'app amorce ce nom
/// depuis le réglage global `default_model` (`app-settings`), comme le faisait
/// `system_agents::resolve_or_bootstrap_profile` ; sans profil de ce nom en base, c'est
/// `lymaille` qui refuse le tour (`UnknownProfile`) — aucun modèle codé en dur ici. La
/// correspondance du réglage vers un profil miryad-core est `[?]` (Tasks de `catalog.sdd`).
const PROFIL_MODELE: &str = "default";

/// Publie le catalogue par défaut, create-only : chaque ligne est insérée avec
/// `INSERT … ON CONFLICT DO NOTHING` sur sa clé `UNIQUE`, donc aucune ligne existante — par
/// exemple redéfinie par un administrateur sous le même nom — n'est lue ni patchée (`FR-6`).
/// Ouverture par tout utilisateur authentifié (`DEC-7`) : les lignes du catalogue sont
/// `Public` en lecture chez miryad-core et le contexte ne porte pas de groupes
/// (`allowed_groups` n'existe que sur les serveurs MCP) — rien à régler ici. Branché au
/// démarrage en B9 (`mod.sdd` Tasks).
pub(crate) async fn ensure_catalog(db: &DatabaseConnection) -> Result<(), AppError> {
    publier_type(db, G_HOME, G_VANYLINE_HOME, SessionKind::Work).await?;
    publier_contexte(db, G_HOME).await?;
    publier_type(db, G_CONFIG, G_VANYLINE_CONFIG, SessionKind::Config).await?;
    publier_contexte(db, G_CONFIG).await?;
    publier_agent(
        db,
        G_VANYLINE_HOME,
        AgentKind::Work,
        "Agent productif par défaut de l'accueil de vanyline.",
    )
    .await?;
    publier_agent(
        db,
        G_VANYLINE_CONFIG,
        AgentKind::Config,
        "Agent de configuration par défaut de vanyline.",
    )
    .await?;
    publier_agent(
        db,
        G_VANYLINE_PROJECT,
        AgentKind::Work,
        "Agent productif par défaut des projets de vanyline.",
    )
    .await?;
    publier_agent(
        db,
        G_VANYLINE_SANDBOX,
        AgentKind::Work,
        "Agent productif par défaut des sandboxes de vanyline.",
    )
    .await?;
    Ok(())
}

/// Correspondance compilée écran → agent pour les contextes `H_` (`DEC-6`) ; `None` pour un
/// contexte stocké, dont la nature et l'agent viennent du type lui-même (`Exposes`). La table
/// n'est ni administrable ni lisible par une session `Config` : constante compilée, isolée ici.
#[must_use]
pub(crate) fn agent_pour_contexte(context: &str) -> Option<&'static str> {
    if parse_project_id(context).is_some() {
        return Some(G_VANYLINE_PROJECT);
    }
    if !parse_sandbox_id(context).is_empty() {
        return Some(G_VANYLINE_SANDBOX);
    }
    None
}

/// Publie la ligne de `catalog_context_types` du type `nom` : `active` est la forme
/// sérialisée d'un `ContextType` construit par `lymaille` elle-même — schéma minimal sans
/// paramètre, nature et agent par défaut portés par le type (`FR-6`) —, et `allowed_agents`
/// contient l'agent par défaut, condition de l'ouverture par défaut chez miryad-core. La
/// colonne `context_type` des contextes référence le type par ce nom `G_` : c'est ce que
/// résout `interactive/manager.rs` de miryad-core (`[?]` dans `catalog.sdd`).
async fn publier_type(
    db: &DatabaseConnection,
    nom: &str,
    agent: &str,
    nature: SessionKind,
) -> Result<(), AppError> {
    let actif = serde_json::to_value(
        serde_json::from_value::<ContextType>(serde_json::json!({
            "name": nom,
            "version": VERSION_CATALOGUE,
            "schema": { "type": "object" },
            "roles": [],
            "includes": [],
            "default_agent": agent,
            "session_kind": nature,
        }))
        .map_err(panne_crate)?,
    )
    .map_err(panne_crate)?;
    inserer_si_absente(
        db,
        CATALOG_CONTEXT_TYPES_TABLE,
        "name",
        &[
            ("name", Expr::val(nom)),
            ("version", Expr::val(VERSION_CATALOGUE)),
            ("allowed_agents", Expr::val(serde_json::json!([agent]))),
            (
                "script_grants",
                Expr::val(serde_json::json!({"toolsets": [], "modules": []})),
            ),
            ("active", Expr::val(actif)),
            ("draft", Expr::null()),
            ("created_at", Expr::val(Utc::now())),
        ],
    )
    .await
}

/// Publie la ligne de `catalog_contexts` du contexte `nom`, de niveau `Shared`, référencé par
/// son propre nom comme type (`DEC-7`) : aucun paramètre, aucun rôle pour les deux contextes
/// d'écran (`contenu minimal`).
async fn publier_contexte(db: &DatabaseConnection, nom: &str) -> Result<(), AppError> {
    let brut = serde_json::to_value(
        ToolsetContext::new(nom.to_string(), ContextLevel::Shared).map_err(panne_crate)?,
    )
    .map_err(panne_crate)?;
    inserer_si_absente(
        db,
        CATALOG_CONTEXTS_TABLE,
        "name",
        &[
            ("name", Expr::val(nom)),
            ("context", Expr::val(brut)),
            ("context_type", Expr::val(nom)),
            ("created_at", Expr::val(Utc::now())),
        ],
    )
    .await
}

/// Publie la ligne de `catalog_agents` de l'agent `id` : définition minimale — nom, nature,
/// prompt d'une phrase, profil `default` (`[?]` de `catalog.sdd`). Les colonnes dérivées
/// `kind` et `profile` sont posées comme le font les hooks de miryad-core, la forme étant
/// produite par `lymaille` elle-même.
async fn publier_agent(
    db: &DatabaseConnection,
    id: &str,
    nature: AgentKind,
    prompt: &str,
) -> Result<(), AppError> {
    let definition = AgentDefinition::new(
        id.to_string(),
        nature,
        prompt.to_string(),
        PROFIL_MODELE.to_string(),
    );
    definition.validate().map_err(panne_crate)?;
    let brut = serde_json::to_value(&definition).map_err(panne_crate)?;
    let nature_texte = serde_json::to_value(definition.kind())
        .ok()
        .and_then(|valeur| valeur.as_str().map(str::to_owned))
        .ok_or_else(|| panne_crate("agent kind is not a text"))?;
    inserer_si_absente(
        db,
        CATALOG_AGENTS_TABLE,
        "agent_id",
        &[
            ("agent_id", Expr::val(id)),
            ("kind", Expr::val(nature_texte)),
            ("profile", Expr::val(PROFIL_MODELE)),
            ("definition", Expr::val(brut)),
            ("created_at", Expr::val(Utc::now())),
        ],
    )
    .await
}

/// `INSERT … ON CONFLICT (<clé UNIQUE>) DO NOTHING` : la création create-only qui ne lit et
/// ne patche jamais la ligne existante (`FR-6`), motif des upserts des stores de miryad-core
/// sans la branche `update`.
async fn inserer_si_absente(
    db: &DatabaseConnection,
    table: &str,
    cle: &str,
    colonnes: &[(&str, Expr)],
) -> Result<(), AppError> {
    let statement = Query::insert()
        .into_table(Alias::new(table))
        .columns(colonnes.iter().map(|(nom, _)| Alias::new(*nom)))
        .values(colonnes.iter().map(|(_, valeur)| valeur.clone()))
        .map_err(|cause| AppError::InternalError(format!("VNL-APP-SES-108: {cause}")))?
        .on_conflict(
            OnConflict::columns([Alias::new(cle)])
                .do_nothing()
                .to_owned(),
        )
        .to_owned();
    db.execute(&statement)
        .await
        .map(|_| ())
        .map_err(|cause| AppError::InternalError(format!("VNL-APP-SES-108: {cause}")))
}

/// Panne de construction d'une définition par `lymaille` ou de (dé)sérialisation de leur forme
/// stockée — implémentation interne à l'app, jamais un refus d'appel utilisateur.
fn panne_crate(cause: impl std::fmt::Display) -> AppError {
    AppError::InternalError(format!(
        "VNL-APP-SES-108: catalogue — définition refusée par la crate lymaille: {cause}"
    ))
}

/// Identifiant du contexte de l'accueil (`DEC-7`).
pub(crate) const G_HOME: &str = "G_home";
/// Identifiant du contexte de configuration (`DEC-7`).
pub(crate) const G_CONFIG: &str = "G_config";
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

    /// Identifiants attendus au catalogue : deux types, deux contextes, quatre agents.
    const TYPES_ATTENDUS: [&str; 2] = [G_HOME, G_CONFIG];
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
            "exactement deux types de contexte"
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
        let definition_admin = serde_json::to_value(AgentDefinition::new(
            G_VANYLINE_CONFIG.to_string(),
            AgentKind::Config,
            "prompt administrateur".to_string(),
            "profil-admin".to_string(),
        ))
        .expect("la définition de l'administrateur se sérialise");
        agent::Entity::insert(agent::ActiveModel {
            id: NotSet,
            agent_id: Set(G_VANYLINE_CONFIG.to_string()),
            kind: Set("config".to_string()),
            profile: Set("profil-admin".to_string()),
            definition: Set(definition_admin.clone()),
            created_at: Set(Utc::now()),
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
