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
                    .col(ColumnDef::new(Dashboard::ProjectId).integer())
                    .col(ColumnDef::new(Dashboard::SandboxId).integer())
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
                    // Un projet supprimé emporte ses tableaux.
                    .foreign_key(
                        ForeignKey::create()
                            .from(Dashboard::Table, Dashboard::ProjectId)
                            .to(Project::Table, Project::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // Une sandbox supprimée n'emporte rien : la personnalisation appartient au
                    // projet, seule l'information `sandbox_id` est vidée.
                    .foreign_key(
                        ForeignKey::create()
                            .from(Dashboard::Table, Dashboard::SandboxId)
                            .to(Sandbox::Table, Sandbox::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        // UN index partiel UNIQUE (`Handles` de `./m20261113_000001_create_vanyline_dashboards.sdd`) :
        // une seule personnalisation `custom` par (owner, écran). Instruction séparée (règle
        // issue de la régression Phase 3 de ./mod.sdd : dans un `Table::create()`, `.index(...)`
        // uniquement avec `.unique()` ; le WHERE d'un partiel ne peut de toute façon pas vivre
        // dans une contrainte de table).
        // Une clé NULL dans un index UNIQUE ne collide jamais (`NULL != NULL`) : l'écran
        // d'accueil (`project_id` NULL) passe donc par `COALESCE(project_id, 0)`, sûr car aucun
        // projet n'a l'identifiant 0. DDL identique sur PostgreSQL et SQLite (index d'expressions
        // et WHERE partiel supportés des deux côtés).
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_dashboards_custom_screen \
                 ON vanyline_dashboards \
                 (owner_id, scope, COALESCE(project_id, 0)) \
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
    ProjectId,
    SandboxId,
    Title,
    Def,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Project {
    #[sea_orm(iden = "vanyline_projects")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Sandbox {
    #[sea_orm(iden = "vanyline_sandboxes")]
    Table,
    Id,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use sea_orm::{
        ActiveValue::NotSet, ConnectionTrait, DbErr, EntityTrait, Set, Statement, Value,
    };

    use crate::db::entities::{projects, sandboxes};
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

    /// Projet de `owner` (via l'entité, `created_at` par défaut de la base).
    async fn projet(db: &impl ConnectionTrait, owner: i32, name: &str) -> i32 {
        projects::Entity::insert(projects::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set(name.to_string()),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("le projet s'insère")
        .last_insert_id
    }

    /// Sandbox de `project` pour `owner` (via l'entité).
    async fn sandbox(db: &impl ConnectionTrait, owner: i32, project: i32, name: &str) -> i32 {
        sandboxes::Entity::insert(sandboxes::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            project_id: Set(project),
            name: Set(name.to_string()),
            cr_name: Set(format!("cr-{name}")),
            missing_since: NotSet,
            created_at: NotSet,
        })
        .exec(db)
        .await
        .expect("la sandbox s'insère")
        .last_insert_id
    }

    /// Insertion SQL brute d'une ligne de définition (contourne le hook : c'est le DDL qui
    /// est testé ici). `project`/`sandbox` sont des identifiants ou `NULL`.
    async fn insert_row(
        db: &impl ConnectionTrait,
        owner: i32,
        scope: &str,
        kind: &str,
        project: Option<i32>,
        sandbox: Option<i32>,
    ) -> Result<u64, DbErr> {
        exec(
            db,
            "INSERT INTO vanyline_dashboards (owner_id, scope, kind, project_id, sandbox_id, title, def) \
             VALUES (?, ?, ?, ?, ?, 'T', '{\"layout\": {\"columns\": 12}, \"widgets\": []}')",
            vec![
                owner.into(),
                scope.into(),
                kind.into(),
                project.into(),
                sandbox.into(),
            ],
        )
        .await
    }

    /// Scenario: `la table naît avec ses colonnes et son index d'unicité`.
    #[tokio::test]
    async fn la_table_nait_avec_ses_colonnes_et_son_index_unique() {
        let db = real_db().await;
        // Témoin positif : la table existe (sinon toute assertion négative passerait).
        assert!(
            scalar(
                &db,
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'vanyline_dashboards'",
                vec![]
            )
            .await
                > 0
        );

        let stmt = Statement::from_string(
            db.get_database_backend(),
            "PRAGMA table_info(vanyline_dashboards)",
        );
        let rows = db
            .query_all_raw(stmt)
            .await
            .expect("table_info est queryable");
        let colonnes: Vec<(String, String, i32, i32)> = rows
            .iter()
            .map(|row| {
                (
                    row.try_get_by_index::<String>(1).expect("nom"),
                    row.try_get_by_index::<String>(2).expect("type"),
                    row.try_get_by_index::<i32>(3).expect("notnull"),
                    row.try_get_by_index::<i32>(5).expect("pk"),
                )
            })
            .collect();
        let attendu = [
            ("id", "integer", 1, 1),
            ("owner_id", "integer", 1, 0),
            ("scope", "varchar", 1, 0),
            ("kind", "varchar", 1, 0),
            ("project_id", "integer", 0, 0),
            ("sandbox_id", "integer", 0, 0),
            ("title", "varchar", 1, 0),
            ("def", "jsonb", 1, 0),
        ];
        let noms: Vec<(&str, i32, i32)> = colonnes
            .iter()
            .map(|(nom, _, notnull, pk)| (nom.as_str(), *notnull, *pk))
            .collect();
        assert_eq!(
            noms,
            attendu
                .iter()
                .map(|(nom, _, notnull, pk)| (*nom, *notnull, *pk))
                .collect::<Vec<_>>(),
            "les huit colonnes avec nullabilité et clé primaire du contrat"
        );
        for ((nom, type_sql, _, _), (_, attendu_type, _, _)) in colonnes.iter().zip(attendu.iter())
        {
            assert!(
                type_sql.to_lowercase().contains(attendu_type),
                "colonne {nom} : type {type_sql}, attendu {attendu_type}"
            );
        }

        let table = object_sql(&db, "table", "vanyline_dashboards")
            .await
            .expect("la table a un SQL")
            .replace('"', "");
        assert!(
            table.contains("FOREIGN KEY (owner_id) REFERENCES miryad_users (id) ON DELETE CASCADE"),
            "FK cascade vers miryad_users absente : {table}"
        );
        assert!(
            table.contains(
                "FOREIGN KEY (project_id) REFERENCES vanyline_projects (id) ON DELETE CASCADE"
            ),
            "FK project_id cascade absente : {table}"
        );
        assert!(
            table.contains(
                "FOREIGN KEY (sandbox_id) REFERENCES vanyline_sandboxes (id) ON DELETE SET NULL"
            ),
            "FK sandbox_id set null absente : {table}"
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
            index.contains("COALESCE(project_id, 0)"),
            "l'index porte l'expression COALESCE(project_id, 0) : {index}"
        );
        assert!(
            index.contains("WHERE") && index.to_lowercase().contains("kind = 'custom'"),
            "le WHERE partiel (kind = 'custom') doit être visible : {index}"
        );
    }

    /// Scenario: `l'unicité garde sa promesse sur les deux moteurs` (SQLite ici ; PostgreSQL
    /// est le moteur de service, même DDL).
    #[tokio::test]
    async fn l_unicite_garde_sa_promesse() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let p1 = projet(&db, alice, "p1").await;
        let p2 = projet(&db, alice, "p2").await;

        insert_row(&db, alice, "home", "custom", None, None)
            .await
            .expect("premier custom d'accueil insère");
        let err = insert_row(&db, alice, "home", "custom", None, None)
            .await
            .expect_err("le second custom d'accueil doit être rejeté (NULL ramené à 0)");
        assert!(
            err.to_string().to_uppercase().contains("UNIQUE"),
            "le rejet doit venir de la contrainte UNIQUE partielle : {err}"
        );

        insert_row(&db, alice, "project", "custom", Some(p1), None)
            .await
            .expect("un custom sur le projet p1 passe");
        insert_row(&db, alice, "project", "custom", Some(p2), None)
            .await
            .expect("un custom sur un AUTRE projet passe (pas de sur-restriction)");
        insert_row(&db, alice, "project", "complementary", Some(p1), None)
            .await
            .expect("le complémentaire de p1 passe");

        // Témoin positif avant l'assertion de cascade : les quatre lignes admises sont là.
        assert_eq!(
            scalar(
                &db,
                "SELECT COUNT(*) FROM vanyline_dashboards WHERE owner_id = ?",
                vec![alice.into()]
            )
            .await,
            4,
            "les quatre lignes admises sont présentes"
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

    /// Scenario: `les suppressions suivent les FK`.
    #[tokio::test]
    async fn les_suppressions_suivent_les_fk() {
        let db = real_db().await;
        let alice = user(&db, "alice").await;
        let p1 = projet(&db, alice, "p1").await;
        let s1 = sandbox(&db, alice, p1, "s1").await;
        insert_row(&db, alice, "sandbox", "custom", Some(p1), Some(s1))
            .await
            .expect("la personnalisation de sandbox s'insère");

        exec(
            &db,
            "DELETE FROM vanyline_sandboxes WHERE id = ?",
            vec![s1.into()],
        )
        .await
        .expect("la sandbox se supprime");
        let stmt = Statement::from_string(
            db.get_database_backend(),
            "SELECT sandbox_id IS NULL FROM vanyline_dashboards",
        );
        let row = db
            .query_one_raw(stmt)
            .await
            .expect("la ligne subsiste")
            .expect("une ligne");
        assert!(
            row.try_get_by_index::<bool>(0).expect("booléen"),
            "sandbox_id passe à NULL, la ligne est conservée"
        );

        exec(
            &db,
            "DELETE FROM vanyline_projects WHERE id = ?",
            vec![p1.into()],
        )
        .await
        .expect("le projet se supprime");
        assert_eq!(
            scalar(&db, "SELECT COUNT(*) FROM vanyline_dashboards", vec![]).await,
            0,
            "supprimer le projet emporte le tableau"
        );

        // Suppression de l'utilisateur : ses lignes disparaissent par CASCADE.
        let bob = user(&db, "bob").await;
        let p2 = projet(&db, bob, "p2").await;
        insert_row(&db, bob, "project", "complementary", Some(p2), None)
            .await
            .expect("le complémentaire de bob s'insère");
        exec(
            &db,
            "DELETE FROM miryad_users WHERE id = ?",
            vec![bob.into()],
        )
        .await
        .expect("bob se supprime");
        assert_eq!(
            scalar(&db, "SELECT COUNT(*) FROM vanyline_dashboards", vec![]).await,
            0,
            "les lignes de l'utilisateur disparaissent par CASCADE"
        );
    }
}
