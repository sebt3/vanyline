use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Identité d'une Sandbox : clé, propriétaire, projet, nom affiché (unique par projet), nom
/// technique de la CR `Sandbox` (unique par propriétaire) et première absence constatée.
/// Ni branche ni toolchains ni status : la CR reste la golden source (`/vanyline.sdd`).
/// Schéma : `../../migration/m20261023_000002_create_vanyline_sandboxes.sdd`.
// Consommateurs de production à venir : réconciliateur et routes sandboxes
// (`./sandboxes.sdd`, lots ultérieurs) — permis temporaire documenté (tooling.sdd).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_sandboxes")]
pub struct Model {
    /// Clé primaire, auto-incrémentée.
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Utilisateur propriétaire (dénormalisé depuis le projet).
    pub owner_id: i32,
    /// Projet auquel la sandbox appartient (`vanyline_projects.id`).
    pub project_id: i32,
    /// Nom affiché, unique dans le projet.
    pub name: String,
    /// `metadata.name` de la CR `Sandbox` (`<projet>-<nom>`), unique par propriétaire.
    pub cr_name: String,
    /// Première absence constatée de la CR ; `None` tant qu'elle existe.
    pub missing_since: Option<DateTimeUtc>,
    /// Horodatage de création, renseigné par le défaut de la base.
    pub created_at: DateTimeUtc,
}

/// Aucune relation `SeaORM` déclarée : les accès sont des filtres explicites.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
#[allow(dead_code)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use crate::db::test_support::real_db;
    use sea_orm::{ActiveValue::NotSet, ColumnTrait, QueryFilter, Set};

    async fn user(db: &DatabaseConnection, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    /// Insère un projet brut (identité seule) et rend son identifiant.
    async fn project(db: &DatabaseConnection, owner: i32, name: &str) -> i32 {
        use crate::db::entities::projects;
        projects::Entity::insert(projects::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("project inserts")
        .last_insert_id
    }

    async fn insert(
        db: &DatabaseConnection,
        owner: i32,
        project_id: i32,
        name: &str,
        cr_name: &str,
    ) -> Result<i32, DbErr> {
        Entity::insert(ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            project_id: Set(project_id),
            name: Set(name.to_string()),
            cr_name: Set(cr_name.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .map(|res| res.last_insert_id)
    }

    /// Scenario: `insertion_et_relecture`.
    #[tokio::test]
    async fn insertion_et_relecture() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let a = project(&db, alice, "a").await;
        let id = insert(&db, alice, a, "main", "a-main")
            .await
            .expect("sandbox inserts");

        let row = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query succeeds")
            .expect("row exists");
        assert_eq!(row.owner_id, alice);
        assert_eq!(row.project_id, a);
        assert_eq!(row.name, "main");
        assert_eq!(row.cr_name, "a-main");
        assert_eq!(row.missing_since, None);
        let floor = chrono::Utc::now()
            .checked_sub_signed(chrono::TimeDelta::try_minutes(5).expect("5 minutes is valid"))
            .expect("representable");
        assert!(
            row.created_at > floor && row.created_at <= chrono::Utc::now(),
            "created_at must be the database default (now), got {}",
            row.created_at
        );
    }

    /// Scenario: `meme_nom_affiche_dans_deux_projets`.
    #[tokio::test]
    async fn meme_nom_affiche_dans_deux_projets() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let a = project(&db, alice, "a").await;
        let b = project(&db, alice, "b").await;
        let in_a = insert(&db, alice, a, "main", "a-main")
            .await
            .expect("main in a");
        let in_b = insert(&db, alice, b, "main", "b-main")
            .await
            .expect("main in b");

        for (project_id, expected_id, expected_cr) in [(a, in_a, "a-main"), (b, in_b, "b-main")] {
            let rows = Entity::find()
                .filter(Column::ProjectId.eq(project_id))
                .filter(Column::Name.eq("main"))
                .all(&db)
                .await
                .expect("query succeeds");
            assert_eq!(rows.len(), 1, "one 'main' per project");
            assert_eq!(rows[0].id, expected_id);
            assert_eq!(rows[0].cr_name, expected_cr);
        }
    }

    /// Scenario: `collision_de_cr_name_rejetee`.
    #[tokio::test]
    async fn collision_de_cr_name_rejetee() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let a = project(&db, alice, "a").await;
        let b = project(&db, alice, "b").await;
        insert(&db, alice, a, "main", "a-main")
            .await
            .expect("first sandbox inserts");

        let err = insert(&db, alice, b, "main", "a-main")
            .await
            .expect_err("same (owner_id, cr_name) must be rejected");
        assert!(
            err.to_string().contains("UNIQUE"),
            "rejection must be the UNIQUE constraint, got: {err}"
        );
    }
}
