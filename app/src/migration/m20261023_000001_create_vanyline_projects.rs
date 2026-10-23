use sea_orm_migration::prelude::*;

/// Identité des Projects (`vanyline_projects`) : clé, propriétaire, nom de la CR, première
/// absence constatée. Contrat : `m20261023_000001_create_vanyline_projects.sdd`.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Project::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Project::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Project::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Project::Name).string().not_null())
                    .col(ColumnDef::new(Project::MissingSince).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Project::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Project::Table, Project::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // Unicité par propriétaire : arbitre une collision de création (409 côté API).
                    // `.unique()` obligatoire dans `Table::create()` (régression Phase 3, ./mod.rs).
                    .index(
                        Index::create()
                            .name("uq_vanyline_projects_owner_name")
                            .unique()
                            .col(Project::OwnerId)
                            .col(Project::Name),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Project::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Project {
    #[sea_orm(iden = "vanyline_projects")]
    Table,
    Id,
    OwnerId,
    Name,
    MissingSince,
    CreatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}
