//! Réglages globaux clé/valeur JSON de l'application — contrat : `./app_settings.sdd`.
//! Lecture ouverte aux utilisateurs authentifiés, écriture réservée aux administrateurs ;
//! seule clé connue en v1 : `default_model` (amorçage des agents système,
//! cf. `../../system_agents.sdd`).

use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = AppSetting)]
#[sea_orm(table_name = "vanyline_app_settings")]
pub struct Model {
    // Corps « tel quel » de `resource_router` : `id` absent du body → 0 (écrasé par le
    // routeur), même contrat que les entités sœurs de `./mod.rs`.
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Clé connue en v1 : le modèle par défaut utilisé pour amorcer les agents système d'un
/// utilisateur sans aucun profil (`../../system_agents.sdd`, comportement `NoModelProfile`).
pub const KEY_DEFAULT_MODEL: &str = "default_model";

/// Validation de la valeur d'un réglage POUR SA CLÉ (`app_settings.sdd` `Exposes`) :
/// `default_model` = `{ "provider_id": <entier>, "model": <chaîne non vide>, "options"?: <objet> }`.
/// La forme seule est vérifiée ici : `before_create`/`before_update` n'ont aucun accès DB
/// (limite miryad-core déjà actée pour `toolsets`/`skills` sous `./agents.sdd`) —
/// `provider_id` désigne un fournisseur LLM existant par contrat d'usage.
fn validate_value(key: &str, value: &serde_json::Value) -> Result<(), HookError> {
    if key != KEY_DEFAULT_MODEL {
        return Err(HookError::with_code(
            "VNL-SET-001",
            format!("unknown app setting key: {key}"),
        ));
    }
    let object = value.as_object().ok_or_else(|| {
        HookError::with_code(
            "VNL-SET-002",
            format!("app setting {key} must be a JSON object, got: {value}"),
        )
    })?;
    if object
        .get("provider_id")
        .is_none_or(|v| v.as_i64().is_none())
    {
        return Err(HookError::with_code(
            "VNL-SET-002",
            format!("app setting {key} requires a numeric 'provider_id', got: {value}"),
        ));
    }
    if object
        .get("model")
        .is_none_or(|v| v.as_str().is_none_or(str::is_empty))
    {
        return Err(HookError::with_code(
            "VNL-SET-002",
            format!("app setting {key} requires a non-empty string 'model', got: {value}"),
        ));
    }
    if object.contains_key("options") && !object["options"].is_object() {
        return Err(HookError::with_code(
            "VNL-SET-002",
            format!(
                "app setting {key} 'options' must be a JSON object, got: {}",
                object["options"]
            ),
        ));
    }
    Ok(())
}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "app-settings"
    }
    fn read_policy() -> AccessPolicy {
        // Les valeurs ne sont pas secrètes (identifiant de fournisseur global, nom de
        // modèle) : lecture ouverte aux utilisateurs authentifiés.
        AccessPolicy::Public
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::AdminOnly
    }
    fn owner_column() -> Option<Column> {
        // Réglages globaux : aucune colonne propriétaire.
        None
    }

    fn before_create(
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let key = match &active.key {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
            ActiveValue::NotSet => "",
        };
        let value = match &active.value {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.clone(),
            ActiveValue::NotSet => serde_json::Value::Null,
        };
        validate_value(key, &value)?;
        Ok(active)
    }

    /// La paire effective (clé, valeur) après update est revalidée : les champs non posés
    /// viennent de la ligne relue `existing` (`before_update` la prête en lecture seule).
    fn before_update(
        active: Self::ActiveModel,
        existing: &Self::Model,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let key = match &active.key {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.clone(),
            ActiveValue::NotSet => existing.key.clone(),
        };
        let value = match &active.value {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.clone(),
            ActiveValue::NotSet => existing.value.clone(),
        };
        validate_value(&key, &value)?;
        Ok(active)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use miryad_core::auth::PrincipalSource;
    use sea_orm::{ActiveValue::NotSet, Set};

    fn test_principal() -> AuthPrincipal {
        AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            preferred_username: None,
            source: PrincipalSource::ApiToken { token_id: 1 },
        }
    }

    fn active_for(key: &str, value: serde_json::Value) -> ActiveModel {
        ActiveModel {
            id: NotSet,
            key: Set(key.to_string()),
            value: Set(value),
        }
    }

    /// Scenario : `contrat_de_ressource` — `app-settings`, lecture `Public`, écriture
    /// `AdminOnly`, aucune colonne propriétaire.
    #[test]
    fn contrat_de_ressource() {
        assert_eq!(Entity::resource_name(), "app-settings");
        assert_eq!(Entity::read_policy(), AccessPolicy::Public);
        assert_eq!(Entity::write_policy(), AccessPolicy::AdminOnly);
        assert!(
            Entity::owner_column().is_none(),
            "réglage global sans colonne propriétaire"
        );
    }

    /// Scenario : `cle_inconnue` — une clé hors `default_model` est refusée.
    #[test]
    fn cle_inconnue() {
        let err = Entity::before_create(
            active_for("autre", serde_json::json!({})),
            &test_principal(),
        )
        .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-SET-001"));
    }

    /// Scenario : `default_model_valide` — la forme contractée est acceptée, avec et sans
    /// `options`.
    #[test]
    fn default_model_valide() {
        assert!(
            Entity::before_create(
                active_for(
                    "default_model",
                    serde_json::json!({"provider_id": 1, "model": "qwen3.6:35b-a3b"})
                ),
                &test_principal()
            )
            .is_ok()
        );
        assert!(
            Entity::before_create(
                active_for(
                    "default_model",
                    serde_json::json!({
                        "provider_id": 1,
                        "model": "qwen3.6:35b-a3b",
                        "options": {"temperature": 0.2}
                    })
                ),
                &test_principal()
            )
            .is_ok()
        );
    }

    /// Scenario : `default_model_invalide` — sans `model`, puis `provider_id` non
    /// numérique : `HookError` dans les deux cas.
    #[test]
    fn default_model_invalide() {
        let err = Entity::before_create(
            active_for("default_model", serde_json::json!({"provider_id": 1})),
            &test_principal(),
        )
        .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-SET-002"));

        let err = Entity::before_create(
            active_for(
                "default_model",
                serde_json::json!({"provider_id": "un", "model": "m"}),
            ),
            &test_principal(),
        )
        .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-SET-002"));
    }

    /// `before_update` revalide la paire effective : passer la valeur à une forme invalide
    /// est refusé, une nouvelle valeur valide passe.
    #[test]
    fn before_update_revalide_la_paire_effective() {
        let existing = Model {
            id: 1,
            key: "default_model".to_string(),
            value: serde_json::json!({"provider_id": 1, "model": "m"}),
        };

        let mut active = ActiveModel {
            id: NotSet,
            key: NotSet,
            value: NotSet,
        };
        active.value = Set(serde_json::json!({"provider_id": "un", "model": "m"}));
        let err = Entity::before_update(active, &existing, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-SET-002"));

        let active = ActiveModel {
            id: NotSet,
            key: NotSet,
            value: Set(serde_json::json!({"provider_id": 2, "model": "autre-modele"})),
        };
        assert!(Entity::before_update(active, &existing, &test_principal()).is_ok());
    }

    /// Scenario : `ecriture_reservee_aux_admins` — un utilisateur ordinaire (hors groupe
    /// admin) est refusé en écriture (403) par le RBAC miryad-core derrière
    /// `AccessPolicy::AdminOnly`, et la lecture réussit sous `AccessPolicy::Public`.
    #[tokio::test]
    async fn ecriture_reservee_aux_admins() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use miryad_core::rest::resource_router;
        use tower::ServiceExt;

        use crate::db::test_support::real_db;

        let db = real_db().await;
        let state = crate::auth::test_support::test_auth_state_with_db(db);
        let token = miryad_core::auth::issue_token(
            &state.db,
            "alice",
            "test",
            None,
            crate::auth::test_support::TEST_TOKEN_PEPPER,
        )
        .await
        .expect("issuing succeeds")
        .token;
        let app = Router::new()
            .merge(resource_router::<Entity, _>())
            .with_state(state);

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/app-settings")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(
            read.status(),
            StatusCode::OK,
            "lecture ouverte aux authentifiés"
        );

        let write = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/app-settings")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"key": "default_model", "value": {"provider_id": 1, "model": "m"}})
                            .to_string(),
                    ))
                    .expect("valid request"),
            )
            .await
            .expect("router does not fail");
        assert_eq!(
            write.status(),
            StatusCode::FORBIDDEN,
            "écriture réservée aux admins (RBAC miryad-core sur AdminOnly)"
        );
    }
}
