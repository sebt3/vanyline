// Tests structurels sur la migration `0006_chat_contexts.sql`.
// Aucun serveur de base de données n'est requis : la migration est chargée
// comme texte binaire via `include_str!` et les assertions vérifient sa
// structure.

const MIGRATION: &str = include_str!("../migrations/0006_chat_contexts.sql");

#[test]
fn chat_contexts_table_shape() {
    assert!(MIGRATION.contains("CREATE TABLE chat_contexts"));
    assert!(
        MIGRATION.contains("kind TEXT NOT NULL"),
        "chat_contexts should have a NOT NULL kind column"
    );
    assert!(
        MIGRATION.contains("data JSONB NOT NULL"),
        "chat_contexts should have a NOT NULL data column"
    );
}

#[test]
fn conversations_reference_chat_contexts() {
    assert!(
        MIGRATION.contains("ADD COLUMN context_id UUID NOT NULL REFERENCES chat_contexts(id)"),
        "conversations should gain a NOT NULL context_id FK to chat_contexts"
    );
}

#[test]
fn indexes_present() {
    assert!(
        MIGRATION.contains("CREATE INDEX idx_chat_contexts_kind ON chat_contexts(kind)"),
        "idx_chat_contexts_kind should exist"
    );
    assert!(
        MIGRATION
            .contains("CREATE INDEX idx_conversations_context_id ON conversations(context_id)"),
        "idx_conversations_context_id should exist"
    );
}
