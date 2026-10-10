use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ProjectWorkflow::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ProjectWorkflow::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ProjectWorkflow::OwnerId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectWorkflow::ProjectName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectWorkflow::DefinitionId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ProjectWorkflow::Scope).string().not_null())
                    .col(ColumnDef::new(ProjectWorkflow::Description).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from(ProjectWorkflow::Table, ProjectWorkflow::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ProjectWorkflow::Table, ProjectWorkflow::DefinitionId)
                            .to(WorkflowDefinition::Table, WorkflowDefinition::Id)
                            // Supprimer une définition supprime ses rattachements.
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // Contrainte de table (seul `.unique()` est valide chaîné ici — un index
                    // non-UNIQUE chaîné génère un fragment orphelin invalide, cf. la
                    // régression Phase 3 commentée dans ./mod.rs).
                    .index(
                        Index::create()
                            .unique()
                            .col(ProjectWorkflow::OwnerId)
                            .col(ProjectWorkflow::ProjectName)
                            .col(ProjectWorkflow::DefinitionId),
                    )
                    .to_owned(),
            )
            .await?;

        // Index de lecture « les rattachements d'un projet » : instruction séparée (règle
        // régression Phase 3, cf. ./mod.rs).
        manager
            .create_index(
                Index::create()
                    .name("idx_vanyline_project_workflows_owner_project")
                    .table(ProjectWorkflow::Table)
                    .col(ProjectWorkflow::OwnerId)
                    .col(ProjectWorkflow::ProjectName)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ProjectWorkflow::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ProjectWorkflow {
    #[sea_orm(iden = "vanyline_project_workflows")]
    Table,
    Id,
    OwnerId,
    ProjectName,
    DefinitionId,
    Scope,
    Description,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum WorkflowDefinition {
    #[sea_orm(iden = "miryad_workflow_definitions")]
    Table,
    Id,
}
