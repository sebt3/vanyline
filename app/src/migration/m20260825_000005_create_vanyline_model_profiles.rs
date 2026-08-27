use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ModelProfile::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ModelProfile::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ModelProfile::OwnerId).integer().not_null())
                    .col(ColumnDef::new(ModelProfile::Name).string().not_null())
                    .col(
                        ColumnDef::new(ModelProfile::ProviderId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ModelProfile::Model).string().not_null())
                    .col(ColumnDef::new(ModelProfile::Temperature).double())
                    .col(ColumnDef::new(ModelProfile::MaxTokens).big_integer())
                    .col(
                        ColumnDef::new(ModelProfile::Options)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'{}'")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ModelProfile::Table, ModelProfile::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(ModelProfile::Table, ModelProfile::ProviderId)
                            .to(LlmProvider::Table, LlmProvider::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(ModelProfile::OwnerId)
                            .col(ModelProfile::Name),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ModelProfile::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ModelProfile {
    #[sea_orm(iden = "vanyline_model_profiles")]
    Table,
    Id,
    OwnerId,
    Name,
    ProviderId,
    Model,
    Temperature,
    MaxTokens,
    Options,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum LlmProvider {
    #[sea_orm(iden = "vanyline_llm_providers")]
    Table,
    Id,
}
