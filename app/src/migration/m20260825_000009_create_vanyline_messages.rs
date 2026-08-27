use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Message::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Message::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Message::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Message::ConversationId).integer().not_null())
                    .col(ColumnDef::new(Message::Role).string().not_null())
                    .col(ColumnDef::new(Message::Payload).custom("JSONB").not_null())
                    .col(
                        ColumnDef::new(Message::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Message::Table, Message::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Message::Table, Message::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Composite (pas juste `conversation_id`) : `get_messages` filtre par `conversation_id`
        // ET trie par `created_at` — index perdu lors de la bascule miryad-core (trouvé en revue
        // Phase 3), restauré ici. Instruction séparée, pas chaînée sur `Table::create()` : un
        // index non-UNIQUE chaîné en `.index(...)` génère un fragment orphelin syntaxiquement
        // invalide côté sea-query (Postgres et SQLite compris), cf. commentaire équivalent dans
        // la migration chat_contexts.
        manager
            .create_index(
                Index::create()
                    .name("idx_vanyline_messages_conversation_id_created_at")
                    .table(Message::Table)
                    .col(Message::ConversationId)
                    .col(Message::CreatedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Message::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Message {
    #[sea_orm(iden = "vanyline_messages")]
    Table,
    Id,
    OwnerId,
    ConversationId,
    Role,
    Payload,
    CreatedAt,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Conversation {
    #[sea_orm(iden = "vanyline_conversations")]
    Table,
    Id,
}
