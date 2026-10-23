use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Identité d'un Project : clé, propriétaire, nom de la CR `Project` et première absence
/// constatée. Ni `spec` ni `status` : la CR Kubernetes reste la seule golden source
/// (`/vanyline.sdd`, « Modèle de données Project/Sandbox »). Schéma :
/// `../../migration/m20261023_000001_create_vanyline_projects.sdd`.
// Consommateurs de production à venir : réconciliateur et routes projets
// (`./projects.sdd`, lots ultérieurs) — permis temporaire documenté (tooling.sdd).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_projects")]
pub struct Model {
    /// Clé primaire, auto-incrémentée.
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Utilisateur miryad-core propriétaire du Project.
    pub owner_id: i32,
    /// `metadata.name` de la CR `Project`.
    pub name: String,
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

    /// Insère un projet via l'`ActiveModel` avec `missing_since` et `created_at` non fournis.
    async fn insert(db: &DatabaseConnection, owner: i32, name: &str) -> i32 {
        let res = Entity::insert(ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("project inserts");
        res.last_insert_id
    }

    async fn user(db: &DatabaseConnection, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    /// Scenario: `insertion_et_relecture`.
    #[tokio::test]
    async fn insertion_et_relecture() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let id = insert(&db, alice, "demo").await;

        let row = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query succeeds")
            .expect("row exists");
        assert_eq!(row.name, "demo");
        assert_eq!(row.owner_id, alice);
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

    /// Scenario: `marquage_et_effacement_de_l_absence`.
    #[tokio::test]
    async fn marquage_et_effacement_de_l_absence() {
        use sea_orm::ActiveModelTrait;

        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let id = insert(&db, alice, "demo").await;
        let instant = chrono::DateTime::from_timestamp(1_700_000_000, 0).expect("valid instant");

        let marked = ActiveModel {
            id: Set(id),
            missing_since: Set(Some(instant)),
            ..Default::default()
        }
        .update(&db)
        .await
        .expect("mark update");
        assert_eq!(marked.missing_since, Some(instant));
        let reread = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query succeeds")
            .expect("row exists");
        assert_eq!(reread.missing_since, Some(instant));

        ActiveModel {
            id: Set(id),
            missing_since: Set(None),
            ..Default::default()
        }
        .update(&db)
        .await
        .expect("clear update");
        let cleared = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query succeeds")
            .expect("row exists");
        assert_eq!(cleared.missing_since, None);
    }

    /// Scenario: `filtre_par_proprietaire_et_nom`.
    #[tokio::test]
    async fn filtre_par_proprietaire_et_nom() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let bob = user(&db, "bob").await;
        insert(&db, alice, "demo").await;
        insert(&db, bob, "demo").await;

        let rows = Entity::find()
            .filter(Column::OwnerId.eq(bob))
            .filter(Column::Name.eq("demo"))
            .all(&db)
            .await
            .expect("query succeeds");
        assert_eq!(rows.len(), 1, "exactly one row for (owner, name)");
        assert_eq!(rows[0].owner_id, bob);
    }
}
