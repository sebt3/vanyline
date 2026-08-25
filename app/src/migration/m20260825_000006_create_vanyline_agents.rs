use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Agent::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Agent::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Agent::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Agent::Name).string().not_null())
                    .col(ColumnDef::new(Agent::Description).string())
                    .col(
                        ColumnDef::new(Agent::Mode)
                            .string()
                            .not_null()
                            .default("primary"),
                    )
                    .col(ColumnDef::new(Agent::ModelProfileId).integer().not_null())
                    .col(
                        ColumnDef::new(Agent::Toolsets)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'[]'")),
                    )
                    .col(
                        ColumnDef::new(Agent::Skills)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'\"auto\"'")),
                    )
                    .col(
                        ColumnDef::new(Agent::SystemPrompt)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Agent::Table, Agent::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Agent::Table, Agent::ModelProfileId)
                            .to(ModelProfile::Table, ModelProfile::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(Agent::OwnerId)
                            .col(Agent::Name),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Agent::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Agent {
    #[sea_orm(iden = "vanyline_agents")]
    Table,
    Id,
    OwnerId,
    Name,
    Description,
    Mode,
    ModelProfileId,
    Toolsets,
    Skills,
    SystemPrompt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ModelProfile {
    #[sea_orm(iden = "vanyline_model_profiles")]
    Table,
    Id,
}