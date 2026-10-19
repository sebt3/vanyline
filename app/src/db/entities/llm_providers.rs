use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = LlmProvider)]
#[sea_orm(table_name = "vanyline_llm_providers")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` évite un 400
    // sur les payloads de création du frontend, qui n'envoie pas `id` (écrasé de toute façon
    // par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    pub name: String,
    pub provider_type: String,
    pub endpoint: String,
    // Écriture seule (décision Sébastien, 2026-09-27) : jamais sérialisée dans une réponse
    // REST ; la désérialisation en entrée reste inchangée.
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    #[serde(default = "default_json_array")]
    pub available_models: serde_json::Value,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "llm-providers"
    }
    fn read_policy() -> AccessPolicy {
        AccessPolicy::Public
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::AdminOnly
    }
    fn owner_column() -> Option<Column> {
        None
    }

    /// Validation perdue lors de la bascule miryad-core (CHECK en base + `validate_provider_type`
    /// sur chaque create/update, cf. `docs/features/miryad-core-integration.md`) — restaurée ici
    /// pour la création (seule surface que `before_create` couvre ; pas d'équivalent
    /// `before_update` dans le trait, limite du framework, pas de cette entité).
    fn before_create(
        active: Self::ActiveModel,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        let provider_type = match &active.provider_type {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
            ActiveValue::NotSet => "",
        };
        verifier_provider_type(provider_type)?;
        Ok(active)
    }

    /// Mise à jour. (1) `provider_type` posé par le PUT est revalidé comme à la création
    /// (`llm_providers.sdd`, DÉCIDÉ 2026-10-18). (2) Secrets (décision 2026-10-18, Tasks « MISE
    /// À JOUR DES SECRETS ») : `api_key` n'est jamais relue par un client, donc un `PUT` qui ne
    /// la porte pas ne doit pas l'effacer. `api_key` absente ou `null` (indiscernables, @None)
    /// ⇒ valeur existante conservée ; chaîne vide ⇒ clé effacée ; chaîne non vide ⇒ remplacée.
    fn before_update(
        mut active: Self::ActiveModel,
        existing: &Model,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        if let ActiveValue::Set(provider_type) = &active.provider_type {
            verifier_provider_type(provider_type)?;
        }
        let incoming = match &active.api_key {
            ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.clone(),
            ActiveValue::NotSet => None,
        };
        active.api_key = ActiveValue::Set(match incoming {
            None => existing.api_key.clone(),
            Some(key) if key.is_empty() => None,
            Some(key) => Some(key),
        });
        Ok(active)
    }
}

/// Valeurs admises de `provider_type` (création et mise à jour) : `VNL-LLM-005` sinon.
fn verifier_provider_type(provider_type: &str) -> Result<(), HookError> {
    if provider_type != "ollama" && provider_type != "openai-compatible" {
        return Err(HookError::with_code(
            "VNL-LLM-005",
            format!("provider_type must be 'ollama' or 'openai-compatible', got: {provider_type}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::IntoActiveModel;

    #[test]
    fn llm_provider_resource_contract() {
        assert_eq!(Entity::resource_name(), "llm-providers");
        assert_eq!(Entity::read_policy(), AccessPolicy::Public);
        assert_eq!(Entity::write_policy(), AccessPolicy::AdminOnly);
        assert!(Entity::owner_column().is_none());
    }

    fn sample(provider_type: &str) -> ActiveModel {
        Model {
            id: 0,
            name: "test".to_string(),
            provider_type: provider_type.to_string(),
            endpoint: "http://localhost:11434".to_string(),
            api_key: None,
            available_models: serde_json::json!([]),
            is_default: false,
        }
        .into_active_model()
    }

    fn test_principal() -> AuthPrincipal {
        AuthPrincipal {
            subject: "alice".to_string(),
            email: None,
            // Chemin token API : miryad-core pose `preferred_username` à `None` sur ce chemin.
            preferred_username: None,
            source: miryad_core::auth::PrincipalSource::ApiToken { token_id: 1 },
        }
    }

    #[test]
    fn before_create_accepts_known_provider_types() {
        assert!(Entity::before_create(sample("ollama"), &test_principal()).is_ok());
        assert!(Entity::before_create(sample("openai-compatible"), &test_principal()).is_ok());
    }

    #[test]
    fn before_create_rejects_unknown_provider_type() {
        let err = Entity::before_create(sample("bogus"), &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-LLM-005"));
    }

    /// `llm_providers.sdd` (tâche DÉCIDÉ 2026-10-18) : `before_update` valide `provider_type`
    /// comme `before_create` (mêmes valeurs, même code `VNL-LLM-005`).
    #[test]
    fn before_update_accepte_les_provider_types_connus() {
        let existing = sample_with_key();
        for provider_type in ["ollama", "openai-compatible"] {
            let mut active = existing.clone().into_active_model();
            active.provider_type = sea_orm::ActiveValue::Set(provider_type.to_string());
            assert!(
                Entity::before_update(active, &existing, &test_principal()).is_ok(),
                "provider_type {provider_type} doit passer"
            );
        }
    }

    #[test]
    fn before_update_rejette_provider_type_inconnu_avec_vnl_llm_005() {
        let existing = sample_with_key();
        let mut active = existing.clone().into_active_model();
        active.provider_type = sea_orm::ActiveValue::Set("bogus".to_string());
        let err = Entity::before_update(active, &existing, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-LLM-005"));
    }

    fn sample_with_key() -> Model {
        Model {
            id: 7,
            name: "secret-provider".to_string(),
            provider_type: "openai-compatible".to_string(),
            endpoint: "https://llm.example.com".to_string(),
            api_key: Some("sk-secret-123".to_string()),
            available_models: serde_json::json!(["m1"]),
            is_default: false,
        }
    }

    /// Sécurité (décision Sébastien, 2026-09-27) : `api_key` est écriture seule. La ligne
    /// seule (GET par id) ne la rend jamais.
    #[test]
    fn api_key_is_absent_from_single_row_json() {
        let v = serde_json::to_value(sample_with_key()).expect("serializes");
        assert!(
            v.get("api_key").is_none(),
            "api_key must not be serialized in REST responses, got: {v}"
        );
    }

    /// Sécurité (2026-09-27) : la page de liste (GET collection) ne la rend pas non plus.
    #[test]
    fn api_key_is_absent_from_list_page_json() {
        let page = miryad_core::query::PagedResult {
            items: vec![sample_with_key()],
            page: 1,
            per_page: 20,
            total_items: 1,
            total_pages: 1,
        };
        let v = serde_json::to_value(page).expect("serializes");
        assert!(v["items"][0].get("api_key").is_none());
        assert!(
            !v.to_string().contains("sk-secret-123"),
            "secret leaked in list page: {v}"
        );
    }

    /// Écriture inchangée : la clé reste acceptée en entrée (désérialisation).
    #[test]
    fn api_key_is_still_accepted_on_input() {
        let m: Model = serde_json::from_value(serde_json::json!({
            "name": "p",
            "provider_type": "ollama",
            "endpoint": "http://localhost:11434",
            "api_key": "sk-in"
        }))
        .expect("deserializes");
        assert_eq!(m.api_key.as_deref(), Some("sk-in"));
    }

    /// Écriture inchangée + lecture interne : la clé reçue en entrée est persistée, relue
    /// telle quelle par le chemin `SeaORM` (sans sérialisation JSON), et une rotation est
    /// persistée de la même façon.
    #[tokio::test]
    async fn api_key_written_on_input_is_persisted_and_read_internally() {
        let db = crate::db::test_support::real_db().await;
        let body: Model = serde_json::from_value(serde_json::json!({
            "name": "stored",
            "provider_type": "openai-compatible",
            "endpoint": "https://llm.example.com",
            "api_key": "sk-in"
        }))
        .expect("deserializes");
        let mut active = body.into_active_model();
        active.id = sea_orm::ActiveValue::NotSet;
        let id = Entity::insert(active)
            .exec(&db)
            .await
            .expect("insert")
            .last_insert_id;

        let row = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(row.api_key.as_deref(), Some("sk-in"));

        let mut active = row.into_active_model();
        active.api_key = sea_orm::ActiveValue::Set(Some("sk-rotated".to_string()));
        Entity::update(active).exec(&db).await.expect("update");
        let row = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(row.api_key.as_deref(), Some("sk-rotated"));
    }

    /// Valeur `api_key` d'un `ActiveModel` produit par le hook de mise à jour.
    fn key_of(a: &ActiveModel) -> Option<String> {
        match &a.api_key {
            sea_orm::ActiveValue::Set(v) | sea_orm::ActiveValue::Unchanged(v) => v.clone(),
            sea_orm::ActiveValue::NotSet => panic!("api_key must be Set after before_update"),
        }
    }

    /// `ActiveModel` tel que `rest::core::update` le passe au hook (`mark_all_set`) avec le
    /// `api_key` donné dans le body.
    fn put_active(api_key: Option<String>) -> ActiveModel {
        let mut a = sample_with_key().into_active_model();
        a.api_key = sea_orm::ActiveValue::Set(api_key);
        a
    }

    /// Décision 2026-10-18 (`llm_providers.sdd`, Tasks « MISE À JOUR DES SECRETS ») :
    /// `api_key` absente ou `null` (indiscernables, @None) ⇒ la valeur existante est conservée.
    #[test]
    fn put_sans_api_key_conserve_la_cle() {
        let out = Entity::before_update(put_active(None), &sample_with_key(), &test_principal())
            .expect("hook accepts");
        assert_eq!(key_of(&out).as_deref(), Some("sk-secret-123"));
    }

    /// Chaîne vide ⇒ la clé est effacée (@None en base).
    #[test]
    fn put_api_key_vide_efface_la_cle() {
        let out = Entity::before_update(
            put_active(Some(String::new())),
            &sample_with_key(),
            &test_principal(),
        )
        .expect("hook accepts");
        assert_eq!(key_of(&out), None);
    }

    /// Chaîne non vide ⇒ la clé existante est remplacée.
    #[test]
    fn put_api_key_non_vide_remplace_la_cle() {
        let out = Entity::before_update(
            put_active(Some("sk-new".to_string())),
            &sample_with_key(),
            &test_principal(),
        )
        .expect("hook accepts");
        assert_eq!(key_of(&out).as_deref(), Some("sk-new"));
    }
}
