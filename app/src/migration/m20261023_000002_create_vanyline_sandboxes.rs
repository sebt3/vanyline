use sea_orm_migration::prelude::*;

/// Identité des Sandboxes (`vanyline_sandboxes`) : `name` unique par projet, `cr_name` unique
/// par propriétaire. Contrat : `m20261023_000002_create_vanyline_sandboxes.sdd`.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Sandbox::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Sandbox::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Sandbox::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Sandbox::ProjectId).integer().not_null())
                    .col(ColumnDef::new(Sandbox::Name).string().not_null())
                    .col(ColumnDef::new(Sandbox::CrName).string().not_null())
                    .col(ColumnDef::new(Sandbox::MissingSince).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Sandbox::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Sandbox::Table, Sandbox::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Sandbox::Table, Sandbox::ProjectId)
                            .to(Project::Table, Project::Id)
                            // Supprimer l'identité d'un projet emporte celles de ses sandboxes.
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // Unicités posées dans `Table::create()` : `.unique()` obligatoire (régression
                    // Phase 3, ./mod.rs).
                    .index(
                        Index::create()
                            .name("uq_vanyline_sandboxes_project_name")
                            .unique()
                            .col(Sandbox::ProjectId)
                            .col(Sandbox::Name),
                    )
                    .index(
                        Index::create()
                            .name("uq_vanyline_sandboxes_owner_cr_name")
                            .unique()
                            .col(Sandbox::OwnerId)
                            .col(Sandbox::CrName),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Sandbox::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Sandbox {
    #[sea_orm(iden = "vanyline_sandboxes")]
    Table,
    Id,
    OwnerId,
    ProjectId,
    Name,
    CrName,
    MissingSince,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Project {
    #[sea_orm(iden = "vanyline_projects")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}
