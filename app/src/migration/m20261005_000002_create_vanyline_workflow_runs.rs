use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkflowRun::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkflowRun::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WorkflowRun::OwnerId).integer().not_null())
                    .col(ColumnDef::new(WorkflowRun::ProjectName).string().not_null())
                    .col(ColumnDef::new(WorkflowRun::SandboxName).string())
                    .col(ColumnDef::new(WorkflowRun::DefinitionId).integer())
                    .col(
                        ColumnDef::new(WorkflowRun::DefinitionName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkflowRun::RunId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(WorkflowRun::RunKey).string().not_null())
                    .col(
                        ColumnDef::new(WorkflowRun::InvocationId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(WorkflowRun::Scope).string().not_null())
                    .col(ColumnDef::new(WorkflowRun::TriggeredBy).string().not_null())
                    .col(
                        ColumnDef::new(WorkflowRun::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkflowRun::Table, WorkflowRun::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(WorkflowRun::Table, WorkflowRun::DefinitionId)
                            .to(WorkflowDefinition::Table, WorkflowDefinition::Id)
                            // L'historique d'un run survit à sa définition : la FK met
                            // `definition_id` à NULL, elle ne supprime jamais le run.
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        // Fenêtre d'affichage par projet (`owner_id, project_name, started_at`) : instruction
        // séparée (règle régression Phase 3, cf. ./mod.rs — un index non-UNIQUE chaîné est un
        // fragment orphelin invalide).
        manager
            .create_index(
                Index::create()
                    .name("idx_vanyline_workflow_runs_project")
                    .table(WorkflowRun::Table)
                    .col(WorkflowRun::OwnerId)
                    .col(WorkflowRun::ProjectName)
                    .col(WorkflowRun::StartedAt)
                    .to_owned(),
            )
            .await?;
        // Fenêtre d'affichage par sandbox.
        manager
            .create_index(
                Index::create()
                    .name("idx_vanyline_workflow_runs_sandbox")
                    .table(WorkflowRun::Table)
                    .col(WorkflowRun::OwnerId)
                    .col(WorkflowRun::SandboxName)
                    .col(WorkflowRun::StartedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WorkflowRun::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum WorkflowRun {
    #[sea_orm(iden = "vanyline_workflow_runs")]
    Table,
    Id,
    OwnerId,
    ProjectName,
    SandboxName,
    DefinitionId,
    DefinitionName,
    RunId,
    RunKey,
    InvocationId,
    Scope,
    TriggeredBy,
    StartedAt,
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
