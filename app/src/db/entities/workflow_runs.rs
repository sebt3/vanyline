use miryad_core::resource::{AccessPolicy, MiryadResource};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
/// Index des runs de workflow lancés par l'app : QUI a lancé QUOI, quand et sur quoi
/// (métadonnées de lancement, immuables, écrites par l'app au lancement — jamais par le
/// client). Le STATUT n'est jamais copié ici : il est lu dans Restate
/// (`../../workflow/status.sdd`). Une ligne survit à la suppression de sa définition
/// (`definition_id` passé à NULL par la FK, `definition_name` conservé).
// Consommateurs de production à venir : ../../api/workflows.sdd (lecture),
// ../../workflow/run_context.sdd (écriture au lancement) — permis temporaire documenté
// (tooling.sdd).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = WorkflowRun)]
#[sea_orm(table_name = "vanyline_workflow_runs")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub owner_id: i32,
    pub project_name: String,
    /// Absent pour un run de portée projet.
    pub sandbox_name: Option<String>,
    /// `NULL` si la définition miryad-core a été supprimée depuis le lancement.
    pub definition_id: Option<i32>,
    /// Instantané du nom de la définition au lancement (survit à sa suppression).
    pub definition_name: String,
    /// UUID de l'app, unique — sert aux `job_id` des steps shell.
    pub run_id: String,
    /// Clé du workflow Restate.
    pub run_key: String,
    /// Identifiant d'invocation Restate, unique.
    pub invocation_id: String,
    pub scope: String,
    /// `ui` | `agent:<nom>` | `api`.
    pub triggered_by: String,
    // `utoipa` 5 ne compose `chrono::DateTime` que derrière sa feature `chrono` (non ajoutée,
    // aucune dépendance neuve autorisée par la spec) : le schéma OpenAPI expose l'horodatage
    // en `String` date-time (même rendu fil qu'en base).
    #[schema(value_type = String, format = "date-time")]
    pub started_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
// Relations non déclarées (contracté) ; consommateurs : ../../api/workflows.sdd,
// ../../workflow/run_context.sdd.
#[allow(dead_code)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "workflow-runs"
    }
    fn read_policy() -> AccessPolicy {
        // Lecture propriétaire seul ; l'écriture est fermée au CRUD générique (aucun client
        // ne crée, modifie ni supprime une ligne) — la ligne est écrite par l'app au
        // lancement du run, hors ressource REST.
        AccessPolicy::OwnerOnly
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::AdminOnly
    }
    fn owner_column() -> Option<Column> {
        Some(Column::OwnerId)
    }
}

/// Supprime les lignes dont `started_at` date de plus de `days` jours (défaut d'usage : 30,
/// planifié une fois par jour par le runtime `../../workflow/runtime.sdd`) et rend le nombre
/// de lignes supprimées. La fenêtre d'affichage (14 jours / 50 lignes,
/// `../../api/workflows.sdd`) est plus courte que la rétention, pour ne pas dépendre d'une
/// purge fine.
///
/// # Errors
///
/// `DbErr` propagé de la requête `DELETE` (connexion ou schéma en erreur).
// Consommateur de production à venir : ../../api/workflows.sdd et
// ../../workflow/runtime.sdd (purge planifiée) — permis temporaire documenté, harnais
// « #[allow] justifié d'une ligne citant la spec » (AGENTS.md, tooling.sdd).
#[allow(dead_code)]
pub async fn purge_older_than(db: &DatabaseConnection, days: i64) -> Result<u64, DbErr> {
    // `checked_*`/`try_*` partout (harnais `arithmetic_side_effects`) ; une fenêtre aberrante
    // est une erreur comme une autre, propagée, jamais un panic en production.
    let out_of_range = || {
        DbErr::Custom(format!(
            "VNL-WFR-001: purge window out of range: {days} days"
        ))
    };
    let Some(delta) = chrono::TimeDelta::try_days(days) else {
        return Err(out_of_range());
    };
    let Some(cutoff) = chrono::Utc::now().checked_sub_signed(delta) else {
        return Err(out_of_range());
    };
    let res = Entity::delete_many()
        .filter(Column::StartedAt.lt(cutoff))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use crate::db::test_support::real_db;
    use chrono::Utc;
    use sea_orm::{ActiveValue::NotSet, EntityTrait, Set};

    /// Scenario: `contrat_de_ressource` — `workflow-runs`, lecture `OwnerOnly`, écriture
    /// `AdminOnly` (l'écriture client fermée par le CRUD générique est une dépendance
    /// miryad-core déjà vérifiée côté crate ; la déclaration est le contrat de ce fichier).
    #[test]
    fn contrat_de_ressource() {
        assert_eq!(Entity::resource_name(), "workflow-runs");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::AdminOnly);
        assert!(
            matches!(Entity::owner_column(), Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {:?}",
            Entity::owner_column()
        );
    }

    fn run(owner_id: i32, run_id: &str, invocation_id: &str, age_days: i64) -> ActiveModel {
        ActiveModel {
            id: NotSet,
            owner_id: Set(owner_id),
            project_name: Set("mon-projet".to_string()),
            sandbox_name: Set(None),
            definition_id: Set(None),
            definition_name: Set("définition".to_string()),
            run_id: Set(run_id.to_string()),
            run_key: Set(format!("key-{run_id}")),
            invocation_id: Set(invocation_id.to_string()),
            scope: Set("project".to_string()),
            triggered_by: Set("ui".to_string()),
            started_at: Set(Utc::now()
                .checked_sub_signed(chrono::TimeDelta::try_days(age_days).unwrap())
                .unwrap()),
        }
    }

    /// Scenario: `unicite` — une seconde ligne de même `run_id` (puis de même
    /// `invocation_id`) est rejetée par l'unicité de la base.
    #[tokio::test]
    async fn unicite() {
        let db = real_db().await;
        let owner = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        Entity::insert(run(owner, "r1", "i1", 1))
            .exec(&db)
            .await
            .expect("first row inserts");

        let dup_run = Entity::insert(run(owner, "r1", "i2", 1)).exec(&db).await;
        assert!(dup_run.is_err(), "duplicate run_id must violate UNIQUE");

        let dup_inv = Entity::insert(run(owner, "r2", "i1", 1)).exec(&db).await;
        assert!(
            dup_inv.is_err(),
            "duplicate invocation_id must violate UNIQUE"
        );

        assert_eq!(Entity::find().all(&db).await.unwrap().len(), 1);
    }

    /// Scenario: `purge` — des lignes de 5, 20 et 40 jours : `purge_older_than(db, 30)` ne
    /// supprime que la ligne de 40 jours et rend 1.
    #[tokio::test]
    async fn purge() {
        let db = real_db().await;
        let owner = miryad_core::users::resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves")
            .id;
        Entity::insert(run(owner, "r5", "i5", 5))
            .exec(&db)
            .await
            .expect("5-day row inserts");
        Entity::insert(run(owner, "r20", "i20", 20))
            .exec(&db)
            .await
            .expect("20-day row inserts");
        Entity::insert(run(owner, "r40", "i40", 40))
            .exec(&db)
            .await
            .expect("40-day row inserts");

        let deleted = purge_older_than(&db, 30).await.expect("purge succeeds");
        assert_eq!(deleted, 1);

        let kept: Vec<String> = Entity::find()
            .all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.run_id)
            .collect();
        assert_eq!(kept.len(), 2);
        assert!(kept.contains(&"r5".to_string()));
        assert!(kept.contains(&"r20".to_string()));
    }
}
