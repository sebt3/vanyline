use miryad_core::auth::AuthPrincipal;
use miryad_core::resource::{AccessPolicy, HookError, MiryadResource};
use sea_orm::ActiveValue;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

fn default_json_object() -> serde_json::Value {
    serde_json::json!({})
}

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = McpServer)]
#[sea_orm(table_name = "vanyline_mcp_servers")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` évite un 400
    // sur les payloads de création du frontend, qui n'envoie pas `id` (écrasé de toute façon
    // par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    pub name: String,
    pub server_type: String,
    pub url: String,
    // Écriture seule (décision Sébastien, 2026-09-27) : `Authorization` et autres en-têtes ne
    // sont jamais sérialisés dans une réponse REST ; la désérialisation en entrée reste inchangée.
    #[serde(default = "default_json_object", skip_serializing)]
    pub headers: serde_json::Value,
    #[serde(default = "default_json_array")]
    pub available_tools: serde_json::Value,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "mcp-servers"
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

    /// Validation perdue lors de la bascule miryad-core (CHECK en base + `validate_server_type`
    /// sur chaque create/update, cf. `docs/features/miryad-core-integration.md`) — restaurée ici
    /// pour la création (seule surface que `before_create` couvre). Forme async à connexion
    /// générique : miryad-core 0.3.1 (hooks.sdd, DEC-2), comportement inchangé — logique sans
    /// attente rendue par [`std::future::ready`] (harnais : `manual_async_fn` et `unused_async_trait_impl` se contredisent sur `async fn` sans `.await`, cf. `tooling.sdd`).
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

    /// Mise à jour. (1) `server_type` posé par le PUT est revalidé comme à la création
    /// (`mcp_servers.sdd`, DÉCIDÉ 2026-10-18). (2) Secrets (décision 2026-10-18, Tasks « MISE À
    /// JOUR DES SECRETS ») : `headers` n'est jamais relu par un client, donc un `PUT` qui ne le
    /// porte pas ne doit pas l'effacer. `headers` absent (`{}` par défaut serde) ou objet vide
    /// ⇒ valeur existante conservée ; objet non vide ⇒ remplacé ; `null` explicite ⇒ effacé.
    /// Forme async à connexion générique : miryad-core 0.3.1 (hooks.sdd, DEC-2), comportement
    /// inchangé.
    fn before_update<C>(
        _db: &C,
        active: Self::ActiveModel,
        existing: &Model,
        _principal: &AuthPrincipal,
    ) -> impl std::future::Future<Output = Result<Self::ActiveModel, HookError>> + Send
    where
        C: sea_orm::ConnectionTrait,
        Self::ActiveModel: Send,
    {
        std::future::ready(valider_mise_a_jour(active, existing))
    }
}

/// Logique de `before_create` (sans attente) : valeurs admises de `server_type`.
fn valider_creation(active: ActiveModel) -> Result<ActiveModel, HookError> {
    let server_type = match &active.server_type {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.as_str(),
        ActiveValue::NotSet => "",
    };
    verifier_server_type(server_type)?;
    Ok(active)
}

/// Logique de `before_update` (sans attente) : revalidation de `server_type` puis règle des
/// secrets sur `headers` (absent/objet vide ⇒ conservé, non vide ⇒ remplacé, `null` ⇒ effacé).
fn valider_mise_a_jour(
    mut active: ActiveModel,
    existing: &Model,
) -> Result<ActiveModel, HookError> {
    if let ActiveValue::Set(server_type) = &active.server_type {
        verifier_server_type(server_type)?;
    }
    let incoming = match &active.headers {
        ActiveValue::Set(v) | ActiveValue::Unchanged(v) => v.clone(),
        ActiveValue::NotSet => default_json_object(),
    };
    let effective = match incoming {
        serde_json::Value::Null => default_json_object(),
        serde_json::Value::Object(ref map) if map.is_empty() => existing.headers.clone(),
        other => other,
    };
    active.headers = ActiveValue::Set(effective);
    Ok(active)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Valeurs admises de `server_type` (création et mise à jour) : `VNL-MCP-003` sinon.
fn verifier_server_type(server_type: &str) -> Result<(), HookError> {
    if server_type != "sse" && server_type != "http-streamable" {
        return Err(HookError::with_code(
            "VNL-MCP-003",
            format!("server_type must be 'sse' or 'http-streamable', got: {server_type}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::IntoActiveModel;

    /// Connexion factice pour la forme async des hooks (miryad-core 0.3.1, hooks.sdd DEC-2) —
    /// même motif que `mock_db` des tests de `miryad-core/src/resource.rs` : les hooks testés
    /// ici n'effectuent aucune requête, la connexion n'est pas consultée.
    fn mock_db() -> sea_orm::DatabaseConnection {
        sea_orm::MockDatabase::new(sea_orm::DbBackend::Sqlite).into_connection()
    }

    #[test]
    fn mcp_server_resource_contract() {
        assert_eq!(Entity::resource_name(), "mcp-servers");
        assert_eq!(Entity::read_policy(), AccessPolicy::Public);
        assert_eq!(Entity::write_policy(), AccessPolicy::AdminOnly);
        assert!(Entity::owner_column().is_none());
    }

    fn sample(server_type: &str) -> ActiveModel {
        Model {
            id: 0,
            name: "test".to_string(),
            server_type: server_type.to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({}),
            available_tools: serde_json::json!([]),
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

    #[tokio::test]
    async fn before_create_accepts_known_server_types() {
        assert!(
            Entity::before_create(&mock_db(), sample("sse"), &test_principal())
                .await
                .is_ok()
        );
        assert!(
            Entity::before_create(&mock_db(), sample("http-streamable"), &test_principal())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn before_create_rejects_unknown_server_type() {
        let err = Entity::before_create(&mock_db(), sample("bogus"), &test_principal())
            .await
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-MCP-003"));
    }

    /// `mcp_servers.sdd` (tâche DÉCIDÉ 2026-10-18) : `before_update` valide `server_type`
    /// comme `before_create` (mêmes valeurs, même code `VNL-MCP-003`).
    #[tokio::test]
    async fn before_update_accepte_les_server_types_connus() {
        let existing = sample_with_headers();
        for server_type in ["sse", "http-streamable"] {
            let mut active = existing.clone().into_active_model();
            active.server_type = sea_orm::ActiveValue::Set(server_type.to_string());
            assert!(
                Entity::before_update(&mock_db(), active, &existing, &test_principal())
                    .await
                    .is_ok(),
                "server_type {server_type} doit passer"
            );
        }
    }

    #[tokio::test]
    async fn before_update_rejette_server_type_inconnu_avec_vnl_mcp_003() {
        let existing = sample_with_headers();
        let mut active = existing.clone().into_active_model();
        active.server_type = sea_orm::ActiveValue::Set("bogus".to_string());
        let err = Entity::before_update(&mock_db(), active, &existing, &test_principal())
            .await
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-MCP-003"));
    }

    fn sample_with_headers() -> Model {
        Model {
            id: 9,
            name: "secret-mcp".to_string(),
            server_type: "http-streamable".to_string(),
            url: "http://localhost:3000/mcp".to_string(),
            headers: serde_json::json!({"Authorization": "Bearer sk-hdr-123"}),
            available_tools: serde_json::json!(["fs.read"]),
        }
    }

    /// Sécurité (décision Sébastien, 2026-09-27) : `headers` (porte souvent `Authorization`)
    /// est écriture seule. La ligne seule (GET par id) ne la rend jamais.
    #[test]
    fn headers_are_absent_from_single_row_json() {
        let v = serde_json::to_value(sample_with_headers()).expect("serializes");
        assert!(
            v.get("headers").is_none(),
            "headers must not be serialized in REST responses, got: {v}"
        );
        assert_eq!(v["name"], "secret-mcp", "les autres clés restent lisibles");
    }

    /// Sécurité (2026-09-27) : la page de liste (GET collection) ne rend pas `headers`.
    #[test]
    fn headers_are_absent_from_list_page_json() {
        let page = miryad_core::query::PagedResult {
            items: vec![sample_with_headers()],
            page: 1,
            per_page: 20,
            total_items: 1,
            total_pages: 1,
        };
        let v = serde_json::to_value(page).expect("serializes");
        assert!(v["items"][0].get("headers").is_none());
        assert!(
            !v.to_string().contains("sk-hdr-123"),
            "secret leaked in list page: {v}"
        );
    }

    /// Écriture inchangée : `headers` reste acceptée en entrée (désérialisation).
    #[test]
    fn headers_are_still_accepted_on_input() {
        let m: Model = serde_json::from_value(serde_json::json!({
            "name": "p",
            "server_type": "sse",
            "url": "http://localhost:3000/mcp",
            "headers": {"Authorization": "Bearer sk-in"}
        }))
        .expect("deserializes");
        assert_eq!(
            m.headers,
            serde_json::json!({"Authorization": "Bearer sk-in"})
        );
    }

    /// Écriture inchangée + lecture interne : `headers` reçus en entrée sont persistés et
    /// relus telle quelle par le chemin `SeaORM`, rotation comprise.
    #[tokio::test]
    async fn headers_written_on_input_are_persisted_and_read_internally() {
        let db = crate::db::test_support::real_db().await;
        let body: Model = serde_json::from_value(serde_json::json!({
            "name": "stored",
            "server_type": "http-streamable",
            "url": "http://localhost:3000/mcp",
            "headers": {"Authorization": "Bearer sk-in"}
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
        assert_eq!(
            row.headers,
            serde_json::json!({"Authorization": "Bearer sk-in"})
        );

        let mut active = row.into_active_model();
        active.headers =
            sea_orm::ActiveValue::Set(serde_json::json!({"Authorization": "Bearer sk-rotated"}));
        Entity::update(active).exec(&db).await.expect("update");
        let row = Entity::find_by_id(id)
            .one(&db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(
            row.headers,
            serde_json::json!({"Authorization": "Bearer sk-rotated"})
        );
    }

    /// `ActiveModel` tel que `rest::core::update` le passe au hook (`mark_all_set`) avec le
    /// `headers` donné dans le body.
    fn put_active(headers: serde_json::Value) -> ActiveModel {
        let mut a = sample_with_headers().into_active_model();
        a.headers = sea_orm::ActiveValue::Set(headers);
        a
    }

    fn headers_of(a: &ActiveModel) -> serde_json::Value {
        match &a.headers {
            sea_orm::ActiveValue::Set(v) | sea_orm::ActiveValue::Unchanged(v) => v.clone(),
            sea_orm::ActiveValue::NotSet => panic!("headers must be Set after before_update"),
        }
    }

    /// Décision 2026-10-18 (`mcp_servers.sdd`, Tasks « MISE À JOUR DES SECRETS ») : `headers`
    /// absent (donc `{}` par le défaut serde) ou objet vide ⇒ la valeur existante est conservée.
    #[tokio::test]
    async fn put_sans_headers_conserve_les_headers() {
        let out = Entity::before_update(
            &mock_db(),
            put_active(serde_json::json!({})),
            &sample_with_headers(),
            &test_principal(),
        )
        .await
        .expect("hook accepts");
        assert_eq!(
            headers_of(&out),
            serde_json::json!({"Authorization": "Bearer sk-hdr-123"})
        );
    }

    /// Objet non vide ⇒ remplacé.
    #[tokio::test]
    async fn put_headers_non_vide_remplace() {
        let out = Entity::before_update(
            &mock_db(),
            put_active(serde_json::json!({"X-Api-Key": "k2"})),
            &sample_with_headers(),
            &test_principal(),
        )
        .await
        .expect("hook accepts");
        assert_eq!(headers_of(&out), serde_json::json!({"X-Api-Key": "k2"}));
    }

    /// `null` EXPLICITE ⇒ efface (`{}` en base).
    #[tokio::test]
    async fn put_headers_null_efface() {
        let out = Entity::before_update(
            &mock_db(),
            put_active(serde_json::Value::Null),
            &sample_with_headers(),
            &test_principal(),
        )
        .await
        .expect("hook accepts");
        assert_eq!(headers_of(&out), serde_json::json!({}));
    }
}
