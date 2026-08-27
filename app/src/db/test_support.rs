#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

/// Base sqlite en mémoire avec les migrations miryad-core ET app appliquées — même paire
/// que `main.rs` au démarrage. Nécessaire pour les tests qui vérifient un vrai aller-retour
/// DB (désérialisation du body + insert réel) : `MockDatabase`
/// (`auth::test_support::test_auth_state`) ne pose aucun vrai schéma, seulement des résultats
/// de requête pré-programmés — insuffisant pour reproduire des bugs d'insertion réels (cf.
/// régression `id: Set(0)` trouvée en revue Phase 3 de miryad-core-integration).
pub(crate) async fn real_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite connects");
    miryad_core::migration::Migrator::up(&db, None)
        .await
        .expect("miryad-core migrations apply cleanly");
    crate::migration::Migrator::up(&db, None)
        .await
        .expect("app migrations apply cleanly");
    db
}
