use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

fn default_auto() -> serde_json::Value {
    serde_json::json!("auto")
}

fn default_mode() -> String {
    "primary".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = Agent)]
#[sea_orm(table_name = "vanyline_agents")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` sur
    // `id`/`owner_id` évite un 400 sur les payloads de création du frontend, qui n'envoie
    // ni l'un ni l'autre (écrasés de toute façon par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub owner_id: i32,
    pub name: String,
    pub description: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    pub model_profile_id: i32,
    #[serde(default = "default_json_array")]
    pub toolsets: serde_json::Value,
    #[serde(default = "default_auto")]
    pub skills: serde_json::Value,
    #[serde(default)]
    pub system_prompt: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "agents"
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

    /// Validation perdue lors de la bascule miryad-core (CHECK en base + `AgentMode` typé côté
    /// handler, cf. `docs/features/miryad-core-integration.md`) — restaurée ici pour la création
    /// (seule surface que `before_create` couvre). Le gap sur les noms `toolsets`/`skills`
    /// (existence côté DB, `validate_toolsets`/`validate_skills`) reste accepté : `before_create`
    /// n'a pas accès DB, même limite que la validation croisée par id déjà actée en Phase 1.
    fn before_create(
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let mode = match &active.mode {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
            ActiveValue::NotSet => "",
        };
        if mode != "primary" && mode != "subagent" && mode != "all" {
            return Err(HookError::with_code(
                "VNL-AGENT-001",
                format!("mode must be 'primary', 'subagent' or 'all', got: {mode}"),
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::IntoActiveModel;

    #[test]
    fn agent_record_resource_contract() {
        assert_eq!(Entity::resource_name(), "agents");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        assert!(
            matches!(Entity::owner_column(), Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {:?}",
            Entity::owner_column()
        );
    }

    fn sample(mode: &str) -> ActiveModel {
        Model {
            id: 0,
            owner_id: 0,
            name: "test".to_string(),
            description: None,
            mode: mode.to_string(),
            model_profile_id: 1,
            toolsets: serde_json::json!([]),
            skills: serde_json::json!("auto"),
            system_prompt: String::new(),
        }
        .into_active_model()
    }

    fn test_principal() -> AuthPrincipal {
        AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        }
    }

    #[test]
    fn before_create_accepts_known_modes() {
        for mode in ["primary", "subagent", "all"] {
            assert!(Entity::before_create(sample(mode), &test_principal()).is_ok());
        }
    }

    #[test]
    fn before_create_rejects_unknown_mode() {
        let err = Entity::before_create(sample("bogus"), &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-001"));
    }
}

/// Régression Phase 3 (miryad-core-integration) : le corps de création envoyé par
/// `AgentsScreen.vue` (`CreateAgent` — ni `id` ni `owner_id`) doit désérialiser et créer une
/// ressource via `resource_router`, et deux créations successives doivent obtenir des ids
/// distincts (bug trouvé : `#[serde(default)]` manquant sur `id`/`owner_id`, cf.
/// `docs/features/miryad-core-integration.md`).
#[cfg(test)]
mod resource_router_regression {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use miryad_core::auth::issue_token;
    use miryad_core::rest::resource_router;
    use miryad_core::users::resolve_user;
    use sea_orm::{ActiveValue::NotSet, DatabaseConnection, EntityTrait, Set};
    use tower::ServiceExt;

    use crate::auth::test_support::test_auth_state_with_db;
    use crate::db::entities::{agents::Entity as AgentEntity, llm_providers, model_profiles};
    use crate::db::test_support::real_db;

    /// Provider + profil valides pour `owner_id` — nécessaires pour satisfaire les FK
    /// `model_profiles.owner_id -> miryad_users.id` et
    /// `agents.model_profile_id -> vanyline_model_profiles.id`.
    async fn seed_model_profile(db: &DatabaseConnection, owner_id: i32) -> i32 {
        let provider = llm_providers::ActiveModel {
            id: NotSet,
            name: Set("ollama-local".to_string()),
            provider_type: Set("ollama".to_string()),
            endpoint: Set("http://localhost:11434".to_string()),
            api_key: Set(None),
            available_models: Set(serde_json::json!([])),
            is_default: Set(true),
        };
        let provider = llm_providers::Entity::insert(provider)
            .exec(db)
            .await
            .expect("provider inserts");

        let profile = model_profiles::ActiveModel {
            id: NotSet,
            owner_id: Set(owner_id),
            name: Set("qwen".to_string()),
            provider_id: Set(provider.last_insert_id),
            model: Set("qwen2.5".to_string()),
            temperature: Set(None),
            max_tokens: Set(None),
            options: Set(serde_json::json!({})),
        };
        model_profiles::Entity::insert(profile)
            .exec(db)
            .await
            .expect("profile inserts")
            .last_insert_id
    }

    async fn bearer_for(db: &DatabaseConnection, subject: &str) -> String {
        issue_token(db, subject, "test", None)
            .await
            .expect("issuing succeeds")
            .token
    }

    fn json_request(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid request")
    }

    #[tokio::test]
    async fn frontend_create_body_round_trips_with_distinct_ids() {
        let db = real_db().await;
        let alice = resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves");
        let profile_id = seed_model_profile(&db, alice.id).await;
        let state = test_auth_state_with_db(db);
        let token = bearer_for(&state.db, "alice").await;
        let app = Router::new()
            .merge(resource_router::<AgentEntity, _>())
            .with_state(state);

        // Corps exact envoyé par `AgentsScreen.vue::createAgent()` — ni `id` ni `owner_id`.
        let body = |name: &str| {
            serde_json::json!({
                "name": name,
                "mode": "primary",
                "model_profile_id": profile_id,
                "toolsets": ["fs"],
                "skills": "auto",
                "system_prompt": "you are a coder",
            })
        };

        let first = app
            .clone()
            .oneshot(json_request("/api/v1/agents", &token, body("coder")))
            .await
            .expect("router does not fail");
        assert_eq!(
            first.status(),
            StatusCode::OK,
            "create should not 400 on a missing id/owner_id"
        );
        let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .expect("readable body");
        let first_json: serde_json::Value =
            serde_json::from_slice(&first_body).expect("valid JSON body");

        // Nom différent : `(owner_id, name)` est unique côté schéma (comportement voulu, pas
        // lié au bug testé ici) — un même nom sur le même owner collide légitimement.
        let second = app
            .oneshot(json_request("/api/v1/agents", &token, body("reviewer")))
            .await
            .expect("router does not fail");
        assert_eq!(
            second.status(),
            StatusCode::OK,
            "second create should not collide"
        );
        let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .expect("readable body");
        let second_json: serde_json::Value =
            serde_json::from_slice(&second_body).expect("valid JSON body");

        assert_ne!(
            first_json["id"], second_json["id"],
            "two creates must not collide on the same auto-increment id"
        );
    }
}
