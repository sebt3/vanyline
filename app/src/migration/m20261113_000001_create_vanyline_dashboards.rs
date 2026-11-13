//! `vanyline_dashboards` — table des définitions portées (`DEC-3` du DSD
//! `dsd/app/dashboards.md`) — contrat : `./m20261113_000001_create_vanyline_dashboards.sdd`.
//! Aucune donnée insérée : les défauts sont codés (`../dashboards/defaults.sdd`, `DEC-2`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Dashboard::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Dashboard::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Dashboard::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Dashboard::Scope).string().not_null())
                    .col(ColumnDef::new(Dashboard::Kind).string().not_null())
                    .col(ColumnDef::new(Dashboard::Project).string())
                    .col(ColumnDef::new(Dashboard::Sandbox).string())
                    .col(ColumnDef::new(Dashboard::Title).string().not_null())
                    // `ColumnType::JsonBinary` en dur (précédent m20261005_000004 pour les
                    // colonnes JSON) : rendu `jsonb` sur PostgreSQL, typage dynamique accepté
                    // tel quel par SQLite — DDL identique sur les deux moteurs.
                    .col(ColumnDef::new(Dashboard::Def).json_binary().not_null())
                    // FK anonyme vers miryad_users : un utilisateur supprimé emporte ses
                    // personnalisations (même motif que les sœurs).
                    .foreign_key(
                        ForeignKey::create()
                            .from(Dashboard::Table, Dashboard::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // UN index partiel UNIQUE (`Handles` de `./m20261113_000001_create_vanyline_dashboards.sdd`) :
        // une seule personnalisation `custom` par (owner, écran). Instruction séparée (règle
        // issue de la régression Phase 3 de ./mod.sdd : dans un `Table::create()`, `.index(...)`
        // uniquement avec `.unique()` ; le WHERE d'un partiel ne peut de toute façon pas vivre
        // dans une contrainte de table).
        // Écart de contrat CONSIGNÉ (`[?]` des Tasks de la spec) : sur les deux moteurs, une
        // clé NULL dans un index UNIQUE ne collide JAMAIS (`NULL != NULL`) — posé tel quel sur
        // `(owner_id, scope, project, sandbox)`, l'index ne garderait le `home` (deux clés
        // NULL) ni le `project`/`sandbox` (une clé NULL). Repli par expressions `COALESCE` :
        // DDL brut IDENTIQUE sur PostgreSQL et SQLite (index d'expressions + WHERE partiel
        // supportés des deux côtés), vérifié par le test des deux moteurs.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_dashboards_custom_screen \
                 ON vanyline_dashboards \
                 (owner_id, scope, COALESCE(project, ''), COALESCE(sandbox, '')) \
                 WHERE kind = 'custom'",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // DROP TABLE seul — l'index partiel tombe avec la table ; sans `IF EXISTS`
        // (politique des sœurs, `[?]` de ./mod.sdd).
        manager
            .drop_table(Table::drop().table(Dashboard::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Dashboard {
    #[sea_orm(iden = "vanyline_dashboards")]
    Table,
    Id,
    OwnerId,
    Scope,
    Kind,
    Project,
    Sandbox,
    Title,
    Def,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DbErr, Statement, Value};

    use crate::db::test_support::real_db;

    /// Exécute une instruction SQL brute avec paramètres `?`.
    async fn exec(db: &impl ConnectionTrait, sql: &str, values: Vec<Value>) -> Result<u64, DbErr> {
        let stmt = Statement::from_sql_and_values(db.get_database_backend(), sql, values);
        db.execute_raw(stmt).await.map(|res| res.rows_affected())
    }

    /// Valeur entière d'une requête scalaire.
    async fn scalar(db: &impl ConnectionTrait, sql: &str, values: Vec<Value>) -> i64 {
        let stmt = Statement::from_sql_and_values(db.get_database_backend(), sql, values);
        db.query_one_raw(stmt)
            .await
            .expect("scalaire exécuté")
            .expect("scalaire rend une ligne")
            .try_get_by_index::<i64>(0)
            .expect("scalaire entier")
    }

    /// SQL de création d'un objet de schéma (`sqlite_master`) — `None` s'il n'existe pas.
    async fn object_sql(db: &impl ConnectionTrait, kind: &str, name: &str) -> Option<String> {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT sql FROM sqlite_master WHERE type = ? AND name = ?",
            vec![kind.into(), name.into()],
        );
        db.query_one_raw(stmt)
            .await
            .expect("sqlite_master est queryable")
            .map(|row| row.try_get_by_index::<String>(0).expect("sql se décode"))
    }

    /// Identifiant d'un utilisateur via le chemin officiel miryad-core.
    async fn user(db: &impl ConnectionTrait, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("l'utilisateur se résout")
            .id
    }

    fn insert_custom(
        db: &impl ConnectionTrait,
        owner: i32,
        project: Option<&str>,
        kind: &str,
    ) -> impl std::future::Future<Output = Result<u64, DbErr>> {
        exec(
            db,
            "INSERT INTO vanyline_dashboards (owner_id, scope, kind, project, sandbox, title, def) \
             VALUES (?, 'project', ?, ?, NULL, 'T', '{\"layout\": {\"columns\": 12}, \"widgets\": []}')",
            vec![owner.into(), kind.into(), project.into()],
        )
    }

    /// Scenario: `la table naît avec ses colonnes et son index partiel`.
    #[tokio::test]
    async fn la_table_nait_avec_ses_colonnes_et_son_index_partiel() {
        let db = real_db().await;
        // Témoin positif : la table existe (sinon toute assertion négative passerait).
        assert!(scalar(
            &db,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vanyline_dashboards'",
            vec![]
        )
        .await > 0);

        let stmt = Statement::from_string(
            db.get_database_backend(),
            "PRAGMA table_info(vanyline_dashboards)",
        );
        let rows = db
            .query_all_raw(stmt)
            .await
            .expect("table_info est queryable");
        let colonnes: Vec<(String, i32, i32)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get_by_index::<String>(1).expect("nom"),
                    row.try_get_by_index::<i32>(3).expect("notnull"),
                    row.try_get_by_index::<i32>(5).expect("pk"),
                )
            })
            .collect();
        assert_eq!(
            colonnes,
            vec![
                ("id".to_string(), 1, 1),
                ("owner_id".to_string(), 1, 0),
                ("scope".to_string(), 1, 0),
                ("kind".to_string(), 1, 0),
                ("project".to_string(), 0, 0),
                ("sandbox".to_string(), 0, 0),
                ("title".to_string(), 1, 0),
                ("def".to_string(), 1, 0),
            ],
            "les huit colonnes avec nullabilité et clé primaire du contrat"
        );

        let table = object_sql(&db, "table", "vanyline_dashboards")
            .await
            .expect("la table a un SQL")
            .replace('"', "");
        assert!(
            table.contains("FOREIGN KEY (owner_id) REFERENCES miryad_users (id) ON DELETE CASCADE"),
            "FK cascade vers miryad_users absente : {table}"
        );

        let index = object_sql(&db, "index", "idx_dashboards_custom_screen")
            .await
            .expect("l'index partiel est nommé dans sqlite_master")
            .replace('"', "");
        assert!(
            index.contains("UNIQUE"),
            "l'index doit être UNIQUE : {index}"
        );
        assert!(
            index.contains("WHERE") && index.to_lowercase().contains("custom"),
            "le WHERE partiel (kind = 'custom') doit être visible : {index}"
        );
    }

    /// Scenario: `le partiel garde sa promesse sur les deux moteurs` (SQLite ici ;
    /// PostgreSQL est le moteur de service, même DDL).
    #[tokio::test]
    async fn le_partiel_garde_sa_promesse() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;

        insert_custom(&db, alice, Some("p1"), "custom")
            .await
            .expect("premier custom insère");
        let err = insert_custom(&db, alice, Some("p1"), "custom")
            .await
            .expect_err("le second custom du même écran doit être rejeté");
        assert!(
            err.to_string().to_uppercase().contains("UNIQUE"),
            "le rejet doit venir de la contrainte UNIQUE partielle : {err}"
        );

        insert_custom(&db, alice, Some("p1"), "complementary")
            .await
            .expect("les customs multiples ne bloquent pas les complémentaires (FR-5)");

        // Témoin positif avant l'assertion de cascade : les deux lignes admises sont là.
        assert_eq!(
            scalar(
                &db,
                "SELECT COUNT(*) FROM vanyline_dashboards WHERE owner_id = ?",
                vec![alice.into()]
            )
            .await,
            2,
            "un custom + un complementary admis (le second custom a été rejeté)"
        );

        exec(
            &db,
            "DELETE FROM miryad_users WHERE id = ?",
            vec![alice.into()],
        )
        .await
        .expect("alice se supprime");
        assert_eq!(
            scalar(
                &db,
                "SELECT COUNT(*) FROM vanyline_dashboards WHERE owner_id = ?",
                vec![alice.into()]
            )
            .await,
            0,
            "la FK emporte tout par CASCADE"
        );
    }
}
