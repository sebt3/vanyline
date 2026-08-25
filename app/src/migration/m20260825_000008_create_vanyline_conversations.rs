use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Conversation::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Conversation::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Conversation::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Conversation::AgentId).integer())
                    .col(ColumnDef::new(Conversation::ContextId).integer().not_null())
                    .col(ColumnDef::new(Conversation::Title).string())
                    .col(ColumnDef::new(Conversation::Todo).string())
                    .col(
                        ColumnDef::new(Conversation::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("NOW()")),
                    )
                    .col(
                        ColumnDef::new(Conversation::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("NOW()")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Conversation::Table, Conversation::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Conversation::Table, Conversation::AgentId)
                            .to(Agent::Table, Agent::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Conversation::Table, Conversation::ContextId)
                            .to(ChatContext::Table, ChatContext::Id)
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .index(
                        Index::create()
                            .col(Conversation::OwnerId),
                    )
                    .index(
                        Index::create()
                            .col(Conversation::ContextId),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Conversation::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Conversation {
    #[sea_orm(iden = "vanyline_conversations")]
    Table,
    Id,
    OwnerId,
    AgentId,
    ContextId,
    Title,
    Todo,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Agent {
    #[sea_orm(iden = "vanyline_agents")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum ChatContext {
    #[sea_orm(iden = "vanyline_chat_contexts")]
    Table,
    Id,
}