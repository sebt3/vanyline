// Tests structurels sur la migration `0004_owner_k8s_name.sql`.
// Aucun serveur de base de données n'est requis : la migration est chargée
// comme texte binaire via `include_str!` et les assertions vérifient sa
// structure.

const MIGRATION: &str = include_str!("../migrations/0004_owner_k8s_name.sql");

#[test]
fn users_adds_k8s_owner_name_column() {
    assert!(
        MIGRATION.contains("ALTER TABLE users ADD COLUMN k8s_owner_name TEXT"),
        "0004 should add a nullable k8s_owner_name TEXT column to users"
    );
}

#[test]
fn k8s_owner_name_column_is_nullable_no_default() {
    assert!(
        !MIGRATION.contains("k8s_owner_name TEXT NOT NULL"),
        "k8s_owner_name column should be nullable (no NOT NULL)"
    );
    assert!(
        !MIGRATION.contains("DEFAULT"),
        "k8s_owner_name column should have no default value"
    );
}
