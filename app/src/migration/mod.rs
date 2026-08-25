mod m20260825_000001_create_vanyline_skills;
mod m20260825_000002_create_vanyline_toolsets;
mod m20260825_000003_create_vanyline_llm_providers;
mod m20260825_000004_create_vanyline_mcp_servers;
mod m20260825_000005_create_vanyline_model_profiles;

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
        ]
    }

    fn migration_table_name() -> DynIden {
        Alias::new("seaql_migrations_app").into_iden()
    }
}