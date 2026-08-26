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