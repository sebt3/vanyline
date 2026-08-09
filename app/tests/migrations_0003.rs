// Tests structurels sur la migration `0003_conversation_todo.sql`.
// Aucun serveur de base de données n'est requis : la migration est chargée
// comme texte binaire via `include_str!` et les assertions vérifient sa
// structure.

const MIGRATION: &str = include_str!("../migrations/0003_conversation_todo.sql");

#[test]
fn conversations_adds_todo_column() {
    assert!(
        MIGRATION.contains("ALTER TABLE conversations ADD COLUMN todo TEXT"),
        "0003 should add a nullable todo TEXT column to conversations"
    );
}

#[test]
fn todo_column_is_nullable_no_default() {
    assert!(
        !MIGRATION.contains("todo TEXT NOT NULL"),
        "todo column should be nullable (no NOT NULL)"
    );
    assert!(
        !MIGRATION.contains("DEFAULT"),
        "todo column should have no default value"
    );
}