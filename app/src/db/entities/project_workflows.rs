use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Rattachement d'une définition de workflow miryad-core (`miryad_workflow_definitions`) à un
/// projet, avec sa portée (`project` | `sandbox`). Les projets étant des CR Kubernetes, la clé
/// du projet est son NOM dans le namespace de l'Owner ; le DAG reste dans la définition —
/// cette entité ne porte que le lien, la portée et un libellé.
// Consommateurs de production à venir : ../../api/workflows.sdd (ressource + routes) —
// permis temporaire documenté, harnais « #[allow] justifié citant la spec » (tooling.sdd).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = ProjectWorkflow)]
#[sea_orm(table_name = "vanyline_project_workflows")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` sur
    // `id`/`owner_id` évite un 400 sur les payloads de création du frontend, qui n'envoie
    // ni l'un ni l'autre (écrasés de toute façon par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub owner_id: i32,
    pub project_name: String,
    pub definition_id: i32,
    /// Portée du rattachement : `"project"` (lié au seul projet) ou `"sandbox"` (lancé sur une
    /// sandbox du projet, seuls rattachements autorisés aux kinds `shell` et `gate`).
    pub scope: String,
    pub description: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
// Relations non déclarées (contracté) ; consommateur de production : ../../api/workflows.sdd.
#[allow(dead_code)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "project-workflows"
    }
    fn read_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn owner_column() -> Option<Column> {
        Some(Column::OwnerId)
    }

    /// Garde de portée du contrat `project_workflows.sdd` : `scope` doit être dans
    /// `{project, sandbox}` (sinon `HookError` → 422). Le hook ne charge pas la définition
    /// (il est synchrone et sans base) — la validation des kinds contre la portée est faite
    /// au rattachement par `../../api/workflows.sdd` et refaite à chaque lancement
    /// (`../../workflow/run_context.sdd`).
    fn before_create(
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let scope = match &active.scope {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
            ActiveValue::NotSet => "",
        };
        if scope != "project" && scope != "sandbox" {
            return Err(HookError::with_code(
                "VNL-WFA-001",
                format!("scope must be 'project' or 'sandbox', got: {scope}"),
            ));
        }
        Ok(active)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use sea_orm::IntoActiveModel;

    /// Scenario: `contrat_de_ressource`
    #[test]
    fn contrat_de_ressource() {
        assert_eq!(Entity::resource_name(), "project-workflows");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        assert!(
            matches!(Entity::owner_column(), Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {:?}",
            Entity::owner_column()
        );
    }

    fn sample(scope: &str) -> ActiveModel {
        Model {
            id: 0,
            owner_id: 0,
            project_name: "mon-projet".to_string(),
            definition_id: 1,
            scope: scope.to_string(),
            description: None,
        }
        .into_active_model()
    }

    fn test_principal() -> AuthPrincipal {
        AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            preferred_username: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        }
    }

    /// Scenario: `scope_invalide` (et son symétrique d'acceptation)
    #[test]
    fn before_create_accepte_scopes_connus() {
        for scope in ["project", "sandbox"] {
            assert!(Entity::before_create(sample(scope), &test_principal()).is_ok());
        }
    }

    /// Scenario: `scope_invalide` — `scope = "global"` ⇒ `HookError` (422).
    #[test]
    fn before_create_rejette_scope_inconnu() {
        let err = Entity::before_create(sample("global"), &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-WFA-001"));
    }

    /// Scenario: `payload_du_frontend_sans_id_ni_owner` — le corps
    /// `{project_name, definition_id, scope}` désérialise avec `id == 0` et `owner_id == 0`
    /// (écrasés par le serveur).
    #[test]
    fn payload_du_frontend_sans_id_ni_owner() {
        let body = serde_json::json!({
            "project_name": "mon-projet",
            "definition_id": 7,
            "scope": "sandbox",
        });
        let model: Model = serde_json::from_value(body).expect("create body deserializes");

        assert_eq!(model.id, 0);
        assert_eq!(model.owner_id, 0);
        assert_eq!(model.project_name, "mon-projet");
        assert_eq!(model.definition_id, 7);
        assert_eq!(model.scope, "sandbox");
        assert_eq!(model.description, None);
    }
}
