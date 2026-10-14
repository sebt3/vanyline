mod m20260825_000001_create_vanyline_skills;
mod m20260825_000002_create_vanyline_toolsets;
mod m20260825_000003_create_vanyline_llm_providers;
mod m20260825_000004_create_vanyline_mcp_servers;
mod m20260825_000005_create_vanyline_model_profiles;
mod m20260825_000006_create_vanyline_agents;
mod m20260825_000007_create_vanyline_chat_contexts;
mod m20260825_000008_create_vanyline_conversations;
mod m20260825_000009_create_vanyline_messages;
mod m20260825_000010_create_vanyline_owner_links;
mod m20261005_000001_create_vanyline_project_workflows;
mod m20261005_000002_create_vanyline_workflow_runs;
mod m20261005_000003_alter_vanyline_agents_system;
mod m20261005_000004_create_vanyline_app_settings;

use sea_orm::sea_query::{Alias, DynIden, IntoIden};
use sea_orm_migration::{MigrationTrait, MigratorTrait};

pub struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260825_000001_create_vanyline_skills::Migration),
            Box::new(m20260825_000002_create_vanyline_toolsets::Migration),
            Box::new(m20260825_000003_create_vanyline_llm_providers::Migration),
            Box::new(m20260825_000004_create_vanyline_mcp_servers::Migration),
            Box::new(m20260825_000005_create_vanyline_model_profiles::Migration),
            Box::new(m20260825_000006_create_vanyline_agents::Migration),
            Box::new(m20260825_000007_create_vanyline_chat_contexts::Migration),
            Box::new(m20260825_000008_create_vanyline_conversations::Migration),
            Box::new(m20260825_000009_create_vanyline_messages::Migration),
            Box::new(m20260825_000010_create_vanyline_owner_links::Migration),
            Box::new(m20261005_000001_create_vanyline_project_workflows::Migration),
            Box::new(m20261005_000002_create_vanyline_workflow_runs::Migration),
            Box::new(m20261005_000003_alter_vanyline_agents_system::Migration),
            Box::new(m20261005_000004_create_vanyline_app_settings::Migration),
        ]
    }

    fn migration_table_name() -> DynIden {
        Alias::new("seaql_migrations_app").into_iden()
    }
}

/// Régression Phase 3 (miryad-core-integration) : plusieurs migrations chaînaient un index
/// non-UNIQUE en `.index(...)` directement sur `Table::create()` — sea-query génère alors un
/// fragment `(...)` orphelin dans le `CREATE TABLE`, syntaxiquement invalide (confirmé contre
/// Postgres ET SQLite, pas une limite sqlite-only). Seul un `.unique()` produit une contrainte de
/// table valide (`UNIQUE (...)`) ; un index simple doit passer par `manager.create_index(...)`,
/// une instruction séparée. Ce test applique la paire complète de migrations (miryad-core + app)
/// contre une base réelle (sqlite en mémoire) — le seul moyen de détecter ce type d'erreur SQL,
/// invisible aux tests unitaires qui ne touchent jamais une vraie connexion.
#[cfg(test)]
mod migrations_apply_cleanly {

    #[tokio::test]
    async fn full_migration_set_applies_against_a_real_schema() {
        use sea_orm::EntityTrait;

        let db = crate::db::test_support::real_db().await;
        // `real_db()` applique déjà les deux migrateurs ; un échec y aurait paniqué avant
        // d'atteindre cette ligne. Une requête réelle contre une table créée confirme un schéma
        // effectivement utilisable, pas seulement "la migration n'a pas paniqué".
        let count = crate::db::entities::skills::Entity::find()
            .all(&db)
            .await
            .expect("querying a freshly migrated table succeeds");
        assert!(count.is_empty());
    }
}

/// Tests des deux migrations workflow `m20261005_000001` / `m20261005_000002`, dérivés des
/// Scenario de leurs specs (`ddl_applique_sur_une_vraie_base`, `unicite_du_rattachement`,
/// `unicite`, `down_supprime_la_table`, et les Scenario de FK), dans le harnais `real_db()`
/// (chaîne complète miryad-core + app sur sqlite en mémoire).
///
/// Note harness : sqlx-sqlite ≥ 0.9 pose `PRAGMA foreign_keys = ON` par défaut à la
/// connexion (cf. `sqlx-sqlite::ConnectOptions::default`, pragmas initiaux) — les cascades et
/// `SET NULL` contractés sont donc RÉELLEMENT appliqués par `real_db()`, contrairement à la
/// dette `[!]` documentée sous l'ancien harnais
/// (`m20260825_000001_create_vanyline_skills.sdd`, `m20260825_000010_create_vanyline_owner_links.sdd`).
/// Les Scenario de FK sont dérivés en tests sur cette base ; l'insertion avec FK pendante est
/// refusée (vérifié), les seeds du fichier posent donc de vraies lignes `miryad_users` /
/// `miryad_workflow_definitions`.
#[cfg(test)]
mod workflow_migrations {

    use sea_orm::{ConnectionTrait, QueryResult, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::test_support::real_db;

    /// Valeur d'une colonne texte d'un `QueryResult` (nullable : `None` sur SQL NULL).
    fn cell_text(res: &QueryResult, column: usize) -> Option<String> {
        res.try_get_by_index_nullable::<Option<String>>(column)
            .expect("text column decodes")
    }

    /// SQL de création d'une table (`sqlite_master`) — `None` si la table n'existe pas.
    async fn table_sql(db: &impl ConnectionTrait, table: &str) -> Option<String> {
        let backend = db.get_database_backend();
        let stmt = Statement::from_sql_and_values(
            backend,
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table.into()],
        );
        let res = db
            .query_one_raw(stmt)
            .await
            .expect("sqlite_master is queryable");
        res.and_then(|res| cell_text(&res, 0))
    }

    /// Noms des index d'une table (`PRAGMA index_list`).
    async fn index_names(db: &impl ConnectionTrait, table: &str) -> Vec<String> {
        let backend = db.get_database_backend();
        let stmt =
            Statement::from_sql_and_values(backend, format!("PRAGMA index_list({table})"), []);
        db.query_all_raw(stmt)
            .await
            .expect("index_list is queryable")
            .iter()
            // Colonne 0 = `seq`, colonne 1 = `name`.
            .filter_map(|res| cell_text(res, 1))
            .collect()
    }

    /// Seed : une définition `miryad_workflow_definitions` (la table de miryad-core, posée
    /// par la feature `workflow`) — colonnes contractées, `steps` = DAG vide, sans
    /// propriétaire (la colonne n'est pas une FK).
    async fn seed_definition(db: &impl ConnectionTrait, name: &str) {
        let backend = db.get_database_backend();
        let stmt = Statement::from_sql_and_values(
            backend,
            "INSERT INTO miryad_workflow_definitions (name, steps, owner_id, created_at) \
             VALUES (?, '{}', NULL, CURRENT_TIMESTAMP)",
            [name.into()],
        );
        db.execute_raw(stmt).await.expect("definition seeds");
    }

    /// Seed : un utilisateur via le chemin officiel miryad-core.
    async fn seed_user(db: &impl ConnectionTrait, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    /// Identifiant d'une définition posée par `seed_definition` (insert brut, id auto).
    async fn definition_id(db: &impl ConnectionTrait, name: &str) -> i32 {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT id FROM miryad_workflow_definitions WHERE name = ?",
            [name.into()],
        );
        let res = db
            .query_one_raw(stmt)
            .await
            .expect("definition is selectable")
            .expect("definition exists");
        res.try_get_by_index::<i32>(0).expect("id decodes")
    }

    /// Suppression brute d'une définition — déclenche l'action d'FK contractée (cascade ou
    /// `SET NULL`) sur les tables de l'app.
    async fn delete_definition(db: &impl ConnectionTrait, id: i32) {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            "DELETE FROM miryad_workflow_definitions WHERE id = ?",
            [id.into()],
        );
        db.execute_raw(stmt).await.expect("definition deletes");
    }

    /// Scenario: `ddl_applique_sur_une_vraie_base` (`m20261005_000001`) — la table existe avec
    /// ses colonnes contractées, l'unicité `(owner_id, project_name, definition_id)` et
    /// l'index `idx_vanyline_project_workflows_owner_project`.
    #[tokio::test]
    async fn ddl_project_workflows_appliquee() {
        let db = real_db().await;
        // Comparaison insensible aux guillemets d'identifiants (rendu SQLite `"col"`).
        let sql = table_sql(&db, "vanyline_project_workflows")
            .await
            .expect("table exists after the full migration chain")
            .replace('"', "");
        for column in [
            "id",
            "owner_id",
            "project_name",
            "definition_id",
            "scope",
            "description",
        ] {
            assert!(
                sql.contains(column),
                "column {column} missing from DDL: {sql}"
            );
        }
        assert!(
            sql.contains("UNIQUE (owner_id, project_name, definition_id)"),
            "composite UNIQUE missing from DDL: {sql}"
        );
        assert!(
            sql.contains("FOREIGN KEY (owner_id) REFERENCES miryad_users (id) ON DELETE CASCADE"),
            "cascade FK to miryad_users missing from DDL: {sql}"
        );
        assert!(
            sql.contains(
                "FOREIGN KEY (definition_id) REFERENCES miryad_workflow_definitions (id) ON DELETE CASCADE"
            ),
            "cascade FK to miryad_workflow_definitions missing from DDL: {sql}"
        );
        let indexes = index_names(&db, "vanyline_project_workflows").await;
        assert!(
            indexes
                .iter()
                .any(|name| name == "idx_vanyline_project_workflows_owner_project"),
            "named index missing, got: {indexes:?}"
        );
    }

    /// Scenario: `unicite_du_rattachement` — le second rattachement identique est refusé, un
    /// rattachement qui change l'un des trois composants de la clé passe.
    #[tokio::test]
    async fn unicite_du_rattachement() {
        use crate::db::entities::project_workflows::{ActiveModel, Entity};
        use sea_orm::{ActiveValue::NotSet, EntityTrait, Set};

        let db = real_db().await;
        let owner = seed_user(&db, "alice").await;
        seed_definition(&db, "dep-1").await;
        seed_definition(&db, "dep-2").await;
        let attach = |definition_id: i32, scope: &str| ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            project_name: Set("mon-projet".to_string()),
            definition_id: Set(definition_id),
            scope: Set(scope.to_string()),
            description: Set(None),
        };
        Entity::insert(attach(1, "project"))
            .exec(&db)
            .await
            .expect("first attachment inserts");

        let dup = Entity::insert(attach(1, "sandbox")).exec(&db).await;
        assert!(
            dup.is_err(),
            "same (owner_id, project_name, definition_id) must violate UNIQUE"
        );

        Entity::insert(attach(2, "project"))
            .exec(&db)
            .await
            .expect("a different definition is a different attachment");
    }

    /// Scenario: `cascade_a_la_suppression_de_la_definition` — supprimer la définition
    /// supprime ses rattachements (`ON DELETE CASCADE` réellement appliqué par la base).
    #[tokio::test]
    async fn cascade_a_la_suppression_de_la_definition() {
        use crate::db::entities::project_workflows::{Entity, Model};
        use sea_orm::EntityTrait;

        let db = real_db().await;
        let owner = seed_user(&db, "alice").await;
        seed_definition(&db, "dep-1").await;
        let definition_id = definition_id(&db, "dep-1").await;
        let attach = crate::db::entities::project_workflows::ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            owner_id: sea_orm::Set(owner),
            project_name: sea_orm::Set("mon-projet".to_string()),
            definition_id: sea_orm::Set(definition_id),
            scope: sea_orm::Set("project".to_string()),
            description: sea_orm::Set(None),
        };
        Entity::insert(attach)
            .exec(&db)
            .await
            .expect("attach inserts");

        delete_definition(&db, definition_id).await;

        let left = Entity::find().all(&db).await.expect("query succeeds");
        assert_eq!(left, [] as [Model; 0], "cascade must drop the attachment");
    }

    /// Scenario: `ddl_applique_sur_une_vraie_base` (`m20261005_000002`) — colonnes contractées,
    /// unicités `run_id` / `invocation_id`, deux index nommés.
    #[tokio::test]
    async fn ddl_workflow_runs_appliquee() {
        let db = real_db().await;
        let sql = table_sql(&db, "vanyline_workflow_runs")
            .await
            .expect("table exists after the full migration chain")
            .replace('"', "");
        for column in [
            "id",
            "owner_id",
            "project_name",
            "sandbox_name",
            "definition_id",
            "definition_name",
            "run_id",
            "run_key",
            "invocation_id",
            "scope",
            "triggered_by",
            "started_at",
        ] {
            assert!(sql.contains(column), "column {column} missing: {sql}");
        }
        assert!(
            sql.contains("run_id") && sql.contains("UNIQUE"),
            "UNIQUE run_id missing from DDL: {sql}"
        );
        assert!(
            sql.contains("invocation_id") && sql.contains("UNIQUE"),
            "UNIQUE invocation_id missing from DDL: {sql}"
        );
        assert!(
            sql.contains(
                "FOREIGN KEY (definition_id) REFERENCES miryad_workflow_definitions (id) ON DELETE SET NULL"
            ),
            "SET NULL FK to miryad_workflow_definitions missing from DDL: {sql}"
        );
        let indexes = index_names(&db, "vanyline_workflow_runs").await;
        for name in [
            "idx_vanyline_workflow_runs_project",
            "idx_vanyline_workflow_runs_sandbox",
        ] {
            assert!(
                indexes.iter().any(|n| n == name),
                "named index {name} missing, got: {indexes:?}"
            );
        }
    }

    /// Scenario: `unicite` (`m20261005_000002`, côté base) — insertion dupliquée refusée sur
    /// `run_id` puis sur `invocation_id`. (Le test homonyme de l'entité
    /// `workflow_runs::tests::unicite` couvre le même contrat côté `SeaORM`.)
    #[tokio::test]
    async fn unicite_des_identifiants_de_run() {
        use crate::db::entities::workflow_runs::{ActiveModel, Entity};
        use sea_orm::{ActiveValue::NotSet, EntityTrait, Set};

        let db = real_db().await;
        let owner = seed_user(&db, "alice").await;
        let row = |run_id: &str, invocation_id: &str| ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            project_name: Set("p".to_string()),
            sandbox_name: Set(None),
            definition_id: Set(None),
            definition_name: Set("d".to_string()),
            run_id: Set(run_id.to_string()),
            run_key: Set("k".to_string()),
            invocation_id: Set(invocation_id.to_string()),
            scope: Set("project".to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(chrono::Utc::now()),
        };
        Entity::insert(row("r1", "i1"))
            .exec(&db)
            .await
            .expect("first run inserts");

        let dup_run = Entity::insert(row("r1", "i2")).exec(&db).await;
        assert!(dup_run.is_err(), "duplicate run_id must violate UNIQUE");

        let dup_inv = Entity::insert(row("r2", "i1")).exec(&db).await;
        assert!(
            dup_inv.is_err(),
            "duplicate invocation_id must violate UNIQUE"
        );
    }

    /// Scenario: `definition_supprimee_garde_le_run` — supprimer la définition passe
    /// `definition_id` à NULL (`ON DELETE SET NULL`) sans toucher au run ni à son
    /// `definition_name`.
    #[tokio::test]
    async fn definition_supprimee_garde_le_run() {
        use crate::db::entities::workflow_runs::{Entity, Model};
        use sea_orm::{ActiveValue::NotSet, EntityTrait, Set};

        let db = real_db().await;
        let owner = seed_user(&db, "alice").await;
        seed_definition(&db, "dep-1").await;
        let definition_id = definition_id(&db, "dep-1").await;
        Entity::insert(crate::db::entities::workflow_runs::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            project_name: Set("p".to_string()),
            sandbox_name: Set(None),
            definition_id: Set(Some(definition_id)),
            definition_name: Set("instantane-du-nom".to_string()),
            run_id: Set("r1".to_string()),
            run_key: Set("k1".to_string()),
            invocation_id: Set("i1".to_string()),
            scope: Set("project".to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(chrono::Utc::now()),
        })
        .exec(&db)
        .await
        .expect("run inserts");

        delete_definition(&db, definition_id).await;

        let runs = Entity::find().all(&db).await.expect("query succeeds");
        assert_eq!(runs.len(), 1, "the run must survive its definition");
        assert_eq!(runs[0].definition_id, None, "SET NULL must clear the link");
        assert_eq!(
            runs[0].definition_name, "instantane-du-nom",
            "the name snapshot must survive"
        );
        let _: &Model = &runs[0];
    }

    /// Scenario: `utilisateur_supprime_supprime_ses_runs` — supprimer l'utilisateur emporte
    /// ses runs (`ON DELETE CASCADE` sur `owner_id`).
    #[tokio::test]
    async fn utilisateur_supprime_supprime_ses_runs() {
        use crate::db::entities::workflow_runs::{Entity, Model};
        use sea_orm::{ActiveValue::NotSet, ConnectionTrait as _, EntityTrait, Set, Statement};

        let db = real_db().await;
        let owner = seed_user(&db, "alice").await;
        Entity::insert(crate::db::entities::workflow_runs::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            project_name: Set("p".to_string()),
            sandbox_name: Set(None),
            definition_id: Set(None),
            definition_name: Set("d".to_string()),
            run_id: Set("r1".to_string()),
            run_key: Set("k1".to_string()),
            invocation_id: Set("i1".to_string()),
            scope: Set("project".to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(chrono::Utc::now()),
        })
        .exec(&db)
        .await
        .expect("run inserts");

        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            "DELETE FROM miryad_users WHERE id = ?",
            [owner.into()],
        );
        db.execute_raw(stmt).await.expect("user deletes");

        let left = Entity::find().all(&db).await.expect("query succeeds");
        assert_eq!(left, [] as [Model; 0], "cascade must drop the run");
    }

    /// Scenario: `down_supprime_la_table` (les deux specs) — `Migrator::down` annule la fin de
    /// chaîne : depuis la chaîne à QUATORZE migrations, trois pas (000004 `app_settings`,
    /// 000003 `agents_system`, puis 000002) annulent `vanyline_workflow_runs` (disparaît,
    /// `vanyline_project_workflows` reste) ; un pas de plus annule la première. Les deux
    /// migrations du bout sont exercées par `mod system_migrations` ci-dessous.
    #[tokio::test]
    async fn down_supprime_les_tables() {
        let db = real_db().await;
        assert!(table_sql(&db, "vanyline_workflow_runs").await.is_some());

        crate::migration::Migrator::down(&db, Some(3))
            .await
            .expect("three-step down succeeds (000004, 000003, then 000002)");
        assert!(
            table_sql(&db, "vanyline_workflow_runs").await.is_none(),
            "vanyline_workflow_runs must be dropped by its down()"
        );
        assert!(
            table_sql(&db, "vanyline_project_workflows").await.is_some(),
            "the previous migration must remain applied"
        );

        crate::migration::Migrator::down(&db, Some(1))
            .await
            .expect("one more down succeeds");
        assert!(
            table_sql(&db, "vanyline_project_workflows").await.is_none(),
            "vanyline_project_workflows must be dropped by its down()"
        );
    }
}

/// Tests des migrations `m20261005_000003_alter_vanyline_agents_system` et
/// `m20261005_000004_create_vanyline_app_settings`, dérivés des Scenario de leurs specs,
/// dans le harnais `real_db()` (chaîne complète miryad-core + app sur sqlite en mémoire).
/// Tout passe par SQL brut (`Statement`) : ces tests verrouillent le DDL POSÉ PAR LA
/// MIGRATION, indépendamment des entités `SeaORM` qui n'existent pas encore à ce stade.
#[cfg(test)]
mod system_migrations {

    use sea_orm::{ConnectionTrait, QueryResult, Statement};
    use sea_orm_migration::MigratorTrait;

    use crate::db::test_support::real_db;

    /// Valeur d'une colonne texte d'un `QueryResult` (nullable : `None` sur SQL NULL).
    fn cell_text(res: &QueryResult, column: usize) -> Option<String> {
        res.try_get_by_index_nullable::<Option<String>>(column)
            .expect("text column decodes")
    }

    /// SQL de création d'une table (`sqlite_master`) — `None` si la table n'existe pas.
    async fn table_sql(db: &impl ConnectionTrait, table: &str) -> Option<String> {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table.into()],
        );
        db.query_one_raw(stmt)
            .await
            .expect("sqlite_master is queryable")
            .and_then(|res| cell_text(&res, 0))
    }

    /// Noms des index d'une table (`PRAGMA index_list`).
    async fn index_names(db: &impl ConnectionTrait, table: &str) -> Vec<String> {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            format!("PRAGMA index_list({table})"),
            [],
        );
        db.query_all_raw(stmt)
            .await
            .expect("index_list is queryable")
            .iter()
            // Colonne 0 = `seq`, colonne 1 = `name`.
            .filter_map(|res| cell_text(res, 1))
            .collect()
    }

    /// Noms des colonnes d'une table (`PRAGMA table_info`).
    async fn column_names(db: &impl ConnectionTrait, table: &str) -> Vec<String> {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            format!("PRAGMA table_info({table})"),
            [],
        );
        db.query_all_raw(stmt)
            .await
            .expect("table_info is queryable")
            .iter()
            // Colonne 1 = `name`.
            .filter_map(|res| cell_text(res, 1))
            .collect()
    }

    /// Seed : une utilisatrice (chemin officiel miryad-core) — retourne son id.
    async fn seed_user(db: &impl ConnectionTrait, subject: &str) -> i32 {
        miryad_core::users::resolve_user(db, subject, None)
            .await
            .expect("user resolves")
            .id
    }

    /// Seed : provider + profil de modèle valides pour `owner` — retourne l'id du profil
    /// (`vanyline_agents.model_profile_id` est NOT NULL avec FK RESTRICT).
    async fn seed_profile(db: &impl ConnectionTrait, owner: i32) -> i32 {
        let backend = db.get_database_backend();
        db.execute_raw(Statement::from_sql_and_values(
            backend,
            "INSERT INTO vanyline_llm_providers (name, provider_type, endpoint, available_models, is_default) \
             VALUES (?, 'ollama', 'http://x', '[]', 1)",
            [format!("p-{owner}").into()],
        ))
        .await
        .expect("provider seeds");
        let provider_id = {
            let res = db
                .query_one_raw(Statement::from_sql_and_values(
                    backend,
                    "SELECT id FROM vanyline_llm_providers WHERE name = ?",
                    [format!("p-{owner}").into()],
                ))
                .await
                .expect("provider selectable")
                .expect("provider exists");
            res.try_get_by_index::<i32>(0).expect("id decodes")
        };
        db.execute_raw(Statement::from_sql_and_values(
            backend,
            "INSERT INTO vanyline_model_profiles (owner_id, name, provider_id, model, options) \
             VALUES (?, 'prof', ?, 'm', '{}')",
            [owner.into(), provider_id.into()],
        ))
        .await
        .expect("profile seeds");
        let res = db
            .query_one_raw(Statement::from_sql_and_values(
                backend,
                "SELECT id FROM vanyline_model_profiles WHERE owner_id = ? AND name = 'prof'",
                [owner.into()],
            ))
            .await
            .expect("profile selectable")
            .expect("profile exists");
        res.try_get_by_index::<i32>(0).expect("id decodes")
    }

    /// Insertion brute d'un agent — `system_key` `None` = colonne à NULL (les colonnes JSON
    /// sont posées littéralement pour rester indépendante de la forme attendue par l'entité).
    async fn insert_agent(
        db: &impl ConnectionTrait,
        owner: i32,
        name: &str,
        profile_id: i32,
        system_key: Option<&str>,
    ) -> Result<(), sea_orm::DbErr> {
        let stmt = Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO vanyline_agents \
             (owner_id, name, mode, model_profile_id, toolsets, skills, system_prompt, system_key) \
             VALUES (?, ?, 'primary', ?, '[]', '\"auto\"', '', ?)",
            [
                owner.into(),
                name.into(),
                profile_id.into(),
                system_key.into(),
            ],
        );
        db.execute_raw(stmt).await.map(|_| ())
    }

    /// Scenario: `agents_existants_intacts` (`m20261005_000003`) — une ligne d'agent ordinaire
    /// (créée sans valeur pour les nouvelles colonnes, le cas de toute ligne antérieure à la
    /// migration) ressort avec `system_key` ET `system_version` à NULL.
    #[tokio::test]
    async fn agents_existants_intacts() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let profile = seed_profile(&db, alice).await;
        insert_agent(&db, alice, "coder", profile, None)
            .await
            .expect("ordinary agent inserts");

        let res = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                "SELECT system_key, system_version FROM vanyline_agents WHERE name = 'coder'",
                [],
            ))
            .await
            .expect("select succeeds")
            .expect("row exists");
        let key: Option<String> = res
            .try_get_by_index_nullable(0)
            .expect("system_key decodes");
        let version: Option<i32> = res
            .try_get_by_index_nullable(1)
            .expect("system_version decodes");
        assert_eq!(key, None, "system_key must default to NULL");
        assert_eq!(version, None, "system_version must default to NULL");
    }

    /// Scenario: `unicite_de_la_cle_systeme` (`m20261005_000003`) — second agent `config`
    /// chez la MÊME utilisatrice : violation ; chez une autre : succès.
    #[tokio::test]
    async fn unicite_de_la_cle_systeme() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let bob = seed_user(&db, "bob").await;
        let alice_profile = seed_profile(&db, alice).await;
        let bob_profile = seed_profile(&db, bob).await;
        insert_agent(&db, alice, "config", alice_profile, Some("config"))
            .await
            .expect("first system agent inserts");

        let dup = insert_agent(&db, alice, "config-2", alice_profile, Some("config")).await;
        assert!(
            dup.is_err(),
            "second (owner_id, system_key) = (alice, config) must violate UNIQUE"
        );

        insert_agent(&db, bob, "config", bob_profile, Some("config"))
            .await
            .expect("same key for another owner must pass");
    }

    /// Scenario: `plusieurs_agents_ordinaires` (`m20261005_000003`) — plusieurs lignes à
    /// `system_key = NULL` chez la même utilisatrice : aucun conflit.
    #[tokio::test]
    async fn plusieurs_agents_ordinaires() {
        let db = real_db().await;
        let alice = seed_user(&db, "alice").await;
        let profile = seed_profile(&db, alice).await;
        insert_agent(&db, alice, "a1", profile, None)
            .await
            .expect("first ordinary agent inserts");
        insert_agent(&db, alice, "a2", profile, None)
            .await
            .expect("NULL system keys must never collide");
    }

    /// Scenario: `ddl_applique_sur_une_vraie_base` (`m20261005_000004`) — la table
    /// `vanyline_app_settings` existe avec ses trois colonnes contractées et l'unicité de `key`.
    #[tokio::test]
    async fn ddl_app_settings_appliquee() {
        let db = real_db().await;
        let sql = table_sql(&db, "vanyline_app_settings")
            .await
            .expect("table exists after the full migration chain")
            .replace('"', "");
        for column in ["id", "key", "value"] {
            assert!(
                sql.contains(column),
                "column {column} missing from DDL: {sql}"
            );
        }
        assert!(
            sql.contains("key varchar NOT NULL UNIQUE"),
            "UNIQUE on key missing from DDL: {sql}"
        );
        assert!(
            sql.contains("value JSONB NOT NULL"),
            "NOT NULL JSON value missing from DDL: {sql}"
        );
    }

    /// Scenario: `cle_unique` (`m20261005_000004`) — une seconde ligne de même clé est refusée.
    #[tokio::test]
    async fn cle_unique() {
        let db = real_db().await;
        let insert = |key: &str| {
            Statement::from_sql_and_values(
                db.get_database_backend(),
                "INSERT INTO vanyline_app_settings (key, value) VALUES (?, '{}')",
                [key.into()],
            )
        };
        db.execute_raw(insert("default_model"))
            .await
            .expect("first key inserts");
        let dup = db.execute_raw(insert("default_model")).await;
        assert!(dup.is_err(), "duplicate key must violate UNIQUE");
    }

    /// Scenario: `down_supprime_la_table` (`m20261005_000004`) — un pas de `down` (la
    /// migration est dernière de chaîne) DROP la table, la chaîne workflow reste posée.
    #[tokio::test]
    async fn down_supprime_app_settings() {
        let db = real_db().await;
        assert!(table_sql(&db, "vanyline_app_settings").await.is_some());

        crate::migration::Migrator::down(&db, Some(1))
            .await
            .expect("one-step down succeeds");
        assert!(
            table_sql(&db, "vanyline_app_settings").await.is_none(),
            "vanyline_app_settings must be dropped by its down()"
        );
        assert!(
            table_sql(&db, "vanyline_workflow_runs").await.is_some(),
            "the previous migration must remain applied"
        );
    }

    /// Scenario: `down_restaure_le_schema` (`m20261005_000003`) — DEUX pas de `down`
    /// (000004 puis 000003) : les colonnes `system_key`/`system_version` et leur index unique
    /// n'existent plus, la table des agents et la chaîne workflow restent intactes.
    #[tokio::test]
    async fn down_restaure_le_schema() {
        let db = real_db().await;
        assert!(
            column_names(&db, "vanyline_agents")
                .await
                .contains(&"system_key".to_string())
        );

        crate::migration::Migrator::down(&db, Some(2))
            .await
            .expect("two-step down succeeds (000004 then 000003)");

        let columns = column_names(&db, "vanyline_agents").await;
        assert!(
            !columns.contains(&"system_key".to_string())
                && !columns.contains(&"system_version".to_string()),
            "system columns must be dropped by down(), got: {columns:?}"
        );
        let indexes = index_names(&db, "vanyline_agents").await;
        assert!(
            !indexes
                .iter()
                .any(|name| name == "uq_vanyline_agents_owner_system_key"),
            "the unique index must be dropped by down(), got: {indexes:?}"
        );
        assert!(
            table_sql(&db, "vanyline_agents").await.is_some(),
            "the agents table itself must survive"
        );
    }
}
