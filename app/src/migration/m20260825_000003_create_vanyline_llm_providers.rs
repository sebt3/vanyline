use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LlmProvider::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LlmProvider::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LlmProvider::Name).string().not_null().unique_key())
                    .col(ColumnDef::new(LlmProvider::ProviderType).string().not_null())
                    .col(ColumnDef::new(LlmProvider::Endpoint).string().not_null())
                    .col(ColumnDef::new(LlmProvider::ApiKey).string())
                    .col(
                        ColumnDef::new(LlmProvider::AvailableModels)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'[]'")),
                    )
                    .col(
                        ColumnDef::new(LlmProvider::IsDefault)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LlmProvider::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum LlmProvider {
    #[sea_orm(iden = "vanyline_llm_providers")]
    Table,
    Id,
    Name,
    ProviderType,
    Endpoint,
    ApiKey,
    AvailableModels,
    IsDefault,
}