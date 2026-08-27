mod m20260825_000001_create_vanyline_skills;
mod m20260825_000002_create_vanyline_toolsets;
mod m20260825_000003_create_vanyline_llm_providers;
mod m20260825_000004_create_vanyline_mcp_servers;
mod m20260825_000005_create_vanyline_model_profiles;
mod m20260825_000006_create_vanyline_agents;
mod m20260825_000007_create_vanyline_chat_contexts;
mod m20260825_000008_create_vanyline_conversations;
mod m20260825_000009_create_vanyline_messages;
mod m20260825_000010_create_vanyline_owner_links;

use sea_orm::sea_query::{Alias, DynIden, IntoIden};
use sea_orm_migration::{MigrationTrait, MigratorTrait};

pub struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260825_000001_create_vanyline_skills::Migration),
            Box::new(m20260825_000002_create_vanyline_toolsets::Migration),
            Box::new(m20260825_000003_create_vanyline_llm_providers::Migration),
            Box::new(m20260825_000004_create_vanyline_mcp_servers::Migration),
            Box::new(m20260825_000005_create_vanyline_model_profiles::Migration),
            Box::new(m20260825_000006_create_vanyline_agents::Migration),
            Box::new(m20260825_000007_create_vanyline_chat_contexts::Migration),
            Box::new(m20260825_000008_create_vanyline_conversations::Migration),
            Box::new(m20260825_000009_create_vanyline_messages::Migration),
            Box::new(m20260825_000010_create_vanyline_owner_links::Migration),
        ]
    }

    fn migration_table_name() -> DynIden {
        Alias::new("seaql_migrations_app").into_iden()
    }
}

/// Régression Phase 3 (miryad-core-integration) : plusieurs migrations chaînaient un index
/// non-UNIQUE en `.index(...)` directement sur `Table::create()` — sea-query génère alors un
/// fragment `(...)` orphelin dans le `CREATE TABLE`, syntaxiquement invalide (confirmé contre
/// Postgres ET SQLite, pas une limite sqlite-only). Seul un `.unique()` produit une contrainte de
/// table valide (`UNIQUE (...)`) ; un index simple doit passer par `manager.create_index(...)`,
/// une instruction séparée. Ce test applique la paire complète de migrations (miryad-core + app)
/// contre une base réelle (sqlite en mémoire) — le seul moyen de détecter ce type d'erreur SQL,
/// invisible aux tests unitaires qui ne touchent jamais une vraie connexion.
#[cfg(test)]
mod migrations_apply_cleanly {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[tokio::test]
    async fn full_migration_set_applies_against_a_real_schema() {
        let db = crate::db::test_support::real_db().await;
        // `real_db()` applique déjà les deux migrateurs ; un échec y aurait paniqué avant
        // d'atteindre cette ligne. Une requête réelle contre une table créée confirme un schéma
        // effectivement utilisable, pas seulement "la migration n'a pas paniqué".
        use sea_orm::EntityTrait;
        let count = crate::db::entities::skills::Entity::find()
            .all(&db)
            .await
            .expect("querying a freshly migrated table succeeds");
        assert!(count.is_empty());
    }
}
