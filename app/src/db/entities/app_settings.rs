//! Réglages globaux clé/valeur JSON de l'application — contrat : `./app_settings.sdd`.
//! Lecture ouverte aux utilisateurs authentifiés, écriture réservée aux administrateurs ;
//! seule clé connue en v1 : `default_profile` (nom du profil de modèle par défaut des agents,
//! cf. `../../sessions/host.sdd`).

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

/// Clé connue en v1 : le nom du profil de modèle lymaille par défaut, celui que reçoivent les
/// agents du catalogue (`../../sessions/catalog.sdd`) par `LymailleHost::default_profile`
/// (`../../sessions/host.sdd`).
pub const KEY_DEFAULT_PROFILE: &str = "default_profile";

/// Nom du profil désigné quand la ligne `default_profile` est absente (`./app_settings.sdd`
/// `Handles`).
pub const PROFIL_PAR_DEFAUT: &str = "default";

/// Extrait le nom de profil d'une valeur `default_profile` : `{ "profile": <chaîne non vide> }`.
/// `None` si la forme n'est pas celle-là. Partagé par la validation d'écriture et la lecture de
/// l'hôte, pour qu'une valeur acceptée à l'écriture soit toujours relue.
#[must_use]
pub fn profil_de_la_valeur(value: &serde_json::Value) -> Option<&str> {
    value
        .as_object()?
        .get("profile")?
        .as_str()
        .filter(|nom| !nom.is_empty())
}

/// Validation de la valeur d'un réglage POUR SA CLÉ (`app_settings.sdd` `Exposes`) :
/// `default_profile` = `{ "profile": <chaîne non vide> }`. L'existence du profil n'est pas
/// vérifiée : lymaille résout un profil par son nom à l'exécution, sans clé étrangère.
fn validate_value(key: &str, value: &serde_json::Value) -> Result<(), HookError> {
    if key != KEY_DEFAULT_PROFILE {
        return Err(HookError::with_code(
            "VNL-SET-001",
            format!("unknown app setting key: {key}"),
        ));
    }
    if profil_de_la_valeur(value).is_none() {
        return Err(HookError::with_code(
            "VNL-SET-002",
            format!(
                "app setting {key} requires an object with a non-empty string 'profile', got: {value}"
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

    /// Forme async à connexion générique : miryad-core 0.3.1 (hooks.sdd, DEC-2) — logique
    /// sans attente rendue par [`std::future::ready`] (harnais : `manual_async_fn` et `unused_async_trait_impl` se contredisent sur `async fn` sans `.await`, cf. `tooling.sdd`).
    fn before_create<C>(
        _db: &C,
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> impl std::future::Future<Output = Result<Self::ActiveModel, HookError>> + Send
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        std::future::ready(valider_creation(active))
    }

    /// La paire effective (clé, valeur) après update est revalidée : les champs non posés
    /// viennent de la ligne relue `existing` (`before_update` la prête en lecture seule).
    /// Forme async à connexion générique : miryad-core 0.3.1 (hooks.sdd, DEC-2), comportement
    /// inchangé.
    fn before_update<C>(
        _db: &C,
        active: Self::ActiveModel,
        existing: &Self::Model,
        _principal: &AuthPrincipal,
    ) -> impl std::future::Future<Output = Result<Self::ActiveModel, HookError>> + Send
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        std::future::ready(valider_mise_a_jour(active, existing))
    }
}

/// Logique de `before_create` (sans attente) : validation de la paire (`key`, `value`) du corps.
fn valider_creation(active: ActiveModel) -> Result<ActiveModel, HookError> {
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

/// Logique de `before_update` (sans attente) : validation de la paire effective, les champs
/// non posés venant de la ligne relue `existing`.
fn valider_mise_a_jour(active: ActiveModel, existing: &Model) -> Result<ActiveModel, HookError> {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use miryad_core::auth::PrincipalSource;
    use sea_orm::{ActiveValue::NotSet, Set};

    /// Connexion factice pour la forme async des hooks (miryad-core 0.3.1, hooks.sdd DEC-2) —
    /// même motif que `mock_db` des tests de `miryad-core/src/resource.rs` : les hooks testés
    /// ici n'effectuent aucune requête, la connexion n'est pas consultée.
    fn mock_db() -> sea_orm::DatabaseConnection {
        sea_orm::MockDatabase::new(sea_orm::DbBackend::Sqlite).into_connection()
    }

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

    /// Scenario : `cle_inconnue` — une clé hors `default_profile` est refusée (dont l'ancienne
    /// clé `default_model`).
    #[tokio::test]
    async fn cle_inconnue() {
        for cle in ["autre", "default_model"] {
            let err = Entity::before_create(
                &mock_db(),
                active_for(cle, serde_json::json!({})),
                &test_principal(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code.as_deref(), Some("VNL-SET-001"), "clé {cle}");
        }
    }

    /// Scenario : `default_profile_valide` — la forme contractée est acceptée, sans vérifier
    /// l'existence du profil.
    #[tokio::test]
    async fn default_profile_valide() {
        assert!(
            Entity::before_create(
                &mock_db(),
                active_for("default_profile", serde_json::json!({"profile": "default"})),
                &test_principal()
            )
            .await
            .is_ok()
        );
    }

    /// Scenario : `default_profile_invalide` — sans `profile`, `profile` vide, `profile` non
    /// chaîne : `VNL-SET-002` dans les trois cas.
    #[tokio::test]
    async fn default_profile_invalide() {
        for valeur in [
            serde_json::json!({}),
            serde_json::json!({"profile": ""}),
            serde_json::json!({"profile": 3}),
        ] {
            let err = Entity::before_create(
                &mock_db(),
                active_for("default_profile", valeur.clone()),
                &test_principal(),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code.as_deref(), Some("VNL-SET-002"), "valeur {valeur}");
        }
    }

    /// `before_update` revalide la paire effective : passer la valeur à une forme invalide
    /// est refusé, une nouvelle valeur valide passe.
    #[tokio::test]
    async fn before_update_revalide_la_paire_effective() {
        let existing = Model {
            id: 1,
            key: "default_profile".to_string(),
            value: serde_json::json!({"profile": "default"}),
        };

        let mut active = ActiveModel {
            id: NotSet,
            key: NotSet,
            value: NotSet,
        };
        active.value = Set(serde_json::json!({"profile": ""}));
        let err = Entity::before_update(&mock_db(), active, &existing, &test_principal())
            .await
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-SET-002"));

        let active = ActiveModel {
            id: NotSet,
            key: NotSet,
            value: Set(serde_json::json!({"profile": "autre"})),
        };
        assert!(
            Entity::before_update(&mock_db(), active, &existing, &test_principal())
                .await
                .is_ok()
        );
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
                        serde_json::json!({"key": "default_profile", "value": {"profile": "m"}})
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
