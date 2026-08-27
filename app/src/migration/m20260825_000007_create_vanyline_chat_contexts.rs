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
                    .col(ColumnDef::new(ChatContext::Data).custom("JSONB").not_null())
                    .col(
                        ColumnDef::new(ChatContext::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // Un index non-UNIQUE ne peut pas être chaîné en `.index(...)` sur `Table::create()` —
        // seul un vrai `.unique()` produit une contrainte de table valide (`UNIQUE (...)`) ; sans
        // lui, sea-query génère un fragment `(...)` orphelin, syntaxiquement invalide (trouvé en
        // testant les migrations contre sqlite en revue Phase 3, confirmé également invalide
        // contre Postgres). Un index simple s'exprime via une instruction séparée.
        manager
            .create_index(
                Index::create()
                    .name("idx_vanyline_chat_contexts_kind")
                    .table(ChatContext::Table)
                    .col(ChatContext::Kind)
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
