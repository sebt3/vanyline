#![cfg(test)]

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

/// Tests du Scenario « tables lymaille posées et paire de migrations rejouable »
/// (`./test_support.sdd`) : les tables `miryad_lymaille_*` doivent être posées par le
/// `Migrator` de miryad-core sous sa feature `lymaille` (activée dans `../Cargo.toml`),
/// sans migration `vanyline_*` pour elles, et la paire de migrations doit être rejouable
/// sur la même connexion sans effet.
#[cfg(test)]
mod lymaille_migrations {

    use sea_orm::{ConnectionTrait, Statement};
    use sea_orm_migration::{MigratorTrait, SchemaManager};

    use super::real_db;

    /// Tables que le `Migrator` de miryad-core doit poser sous sa feature `lymaille`
    /// (`dsd/app/sessions.md#CON-2`).
    const LYMAILLE_TABLES: [&str; 2] = [
        "miryad_lymaille_catalog_agents",
        "miryad_lymaille_mcp_servers",
    ];

    /// Nombre de lignes d'une table de suivi des migrations — `seaql_migrations_miryad_core`
    /// (nom déclaré par `@miryad_core::migration::Migrator::migration_table_name`) et
    /// `seaql_migrations_app` (nom déclaré par `crate::migration::Migrator`).
    async fn tracked_count(db: &impl ConnectionTrait, table: &str) -> i64 {
        let stmt = Statement::from_string(
            db.get_database_backend(),
            format!("SELECT COUNT(*) FROM {table}"),
        );
        db.query_one_raw(stmt)
            .await
            .expect("tracking table is queryable")
            .expect("COUNT returns a row")
            .try_get_by_index::<i64>(0)
            .expect("count decodes as integer")
    }

    /// Première branche du Scenario : après `real_db()`, les deux tables lymaille existent.
    /// Vérification statique au passage : la chaîne de migrations de l'app ne doit rien
    /// référencer de `miryad_lymaille` — la fixture embarque la source du migrateur app à la
    /// compilation, un renvoi futur de ces tables côté app fait échouer ce test.
    #[tokio::test]
    async fn tables_lymaille_posees_par_le_migrator_miryad_core() {
        const APP_MIGRATOR_SOURCE: &str = include_str!("../migration/mod.rs");
        assert!(
            !APP_MIGRATOR_SOURCE.contains("miryad_lymaille"),
            "la chaîne de migrations de l'app ne doit référencer aucune table miryad_lymaille_*"
        );

        let db = real_db().await;
        for table in LYMAILLE_TABLES {
            let exists = SchemaManager::new(&db)
                .has_table(table)
                .await
                .expect("has_table probe runs");
            assert!(exists, "{table} doit exister apres real_db()");
        }
    }

    /// Seconde branche du Scenario : rejouer la PAIRE une seconde fois sur la même connexion
    /// réussit, laisse les mêmes tables et n'ajoute aucune ligne aux deux tables de suivi.
    #[tokio::test]
    async fn paire_de_migrations_rejouable() {
        let db = real_db().await;
        let core_before = tracked_count(&db, "seaql_migrations_miryad_core").await;
        let app_before = tracked_count(&db, "seaql_migrations_app").await;
        assert!(
            core_before > 0 && app_before > 0,
            "precondition: les deux tables de suivi sont alimentees ({core_before}, {app_before})"
        );

        miryad_core::migration::Migrator::up(&db, None)
            .await
            .expect("second miryad-core pass succeeds");
        crate::migration::Migrator::up(&db, None)
            .await
            .expect("second app pass succeeds");

        assert_eq!(
            tracked_count(&db, "seaql_migrations_miryad_core").await,
            core_before,
            "la seconde passe ne doit rien ajouter a seaql_migrations_miryad_core"
        );
        assert_eq!(
            tracked_count(&db, "seaql_migrations_app").await,
            app_before,
            "la seconde passe ne doit rien ajouter a seaql_migrations_app"
        );
        for table in LYMAILLE_TABLES {
            let exists = SchemaManager::new(&db)
                .has_table(table)
                .await
                .expect("has_table probe runs");
            assert!(exists, "{table} doit rester posee apres la seconde passe");
        }
    }
}
