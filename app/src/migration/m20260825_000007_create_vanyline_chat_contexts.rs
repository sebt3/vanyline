use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ChatContext::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ChatContext::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ChatContext::Kind).string().not_null())
                    .col(
                        ColumnDef::new(ChatContext::Data)
                            .custom("JSONB")
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChatContext::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("NOW()")),
                    )
                    .index(
                        Index::create()
                            .col(ChatContext::Kind),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ChatContext::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ChatContext {
    #[sea_orm(iden = "vanyline_chat_contexts")]
    Table,
    Id,
    Kind,
    Data,
    CreatedAt,
}