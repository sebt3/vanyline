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
    // tooling.sdd D5 — m20260825_000006_create_vanyline_agents.sdd (Exposes, DDL contractuelle) :
    // `model_profile_id` — integer().not_null(), sans DEFAULT ; wire snake_case contracté
    // par ./agents.sdd (JSON REST identique aux colonnes, corps de création du frontend).
    #[allow(clippy::struct_field_names)]
    pub model_profile_id: i32,
    #[serde(default = "default_json_array")]
    pub toolsets: serde_json::Value,
    #[serde(default = "default_auto")]
    pub skills: serde_json::Value,
    #[serde(default)]
    pub system_prompt: String,
    // agents.sdd `Must` AGENTS SYSTÈME (2026-10-04, migration m20261005_000003) : clé stable
    // d'un agent préconfiguré, NULL pour un agent ordinaire.
    #[serde(default)]
    pub system_key: Option<String>,
    // Version du prompt système embarqué dans le code (`crate::system_agents::SYSTEM_VERSION`),
    // NULL pour un agent ordinaire.
    #[serde(default)]
    pub system_version: Option<i32>,
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
        verifier_mode(mode)?;
        // AGENTS SYSTÈME (`Must` agents.sdd l.191-197, system_agents.sdd) : `system_key` et
        // `system_version` sont des colonnes serveur — un corps de création client qui en
        // porte une est refusé. Le CRUD générique ne crée que des agents ordinaires.
        let carries_system_column = matches!(&active.system_key, ActiveValue::Set(Some(_)))
            || matches!(&active.system_version, ActiveValue::Set(Some(_)));
        if carries_system_column {
            return Err(HookError::with_code(
                "VNL-AGENT-004",
                "system_key and system_version are server-owned columns and cannot be set by a client body",
            ));
        }
        Ok(active)
    }

    /// AGENTS SYSTÈME (`Must` agents.sdd l.191-197) : sur une ligne portant `system_key`,
    /// seul `model_profile_id` est modifiable — tout autre champ qui diverge de la ligne
    /// relue est refusé (`VNL-AGENT-002`, rendu 422 par miryad-core). Les lignes système
    /// sont réécrites par `crate::system_agents::ensure_system_agents` directement en
    /// base, hors de cette voie. `id`/`owner_id` ne sont pas comparés : `rest::core::update`
    /// les force APRÈS ce hook (ils ne sont donc jamais un changement observable).
    fn before_update(
        active: Self::ActiveModel,
        existing: &Self::Model,
        _principal: &AuthPrincipal,
    ) -> Result<Self::ActiveModel, HookError> {
        if existing.system_key.is_some() {
            let mut refused: Vec<&str> = Vec::new();
            if is_changed(&active.name, &existing.name) {
                refused.push("name");
            }
            if is_changed(&active.description, &existing.description) {
                refused.push("description");
            }
            if is_changed(&active.mode, &existing.mode) {
                refused.push("mode");
            }
            if is_changed(&active.toolsets, &existing.toolsets) {
                refused.push("toolsets");
            }
            if is_changed(&active.skills, &existing.skills) {
                refused.push("skills");
            }
            if is_changed(&active.system_prompt, &existing.system_prompt) {
                refused.push("system_prompt");
            }
            if is_changed(&active.system_key, &existing.system_key) {
                refused.push("system_key");
            }
            if is_changed(&active.system_version, &existing.system_version) {
                refused.push("system_version");
            }
            if !refused.is_empty() {
                return Err(HookError::with_code(
                    "VNL-AGENT-002",
                    format!(
                        "system agents are code-piloted: only model_profile_id is editable, refused change on: {}",
                        refused.join(", ")
                    ),
                ));
            }
        }
        // `agents.sdd` (DÉCIDÉ 2026-10-18) : un `mode` posé par le PUT est revalidé comme à la
        // création. Après la garde système : sur une ligne système, `VNL-AGENT-002` prime.
        if let ActiveValue::Set(mode) = &active.mode {
            verifier_mode(mode)?;
        }
        Ok(active)
    }

    /// AGENTS SYSTÈME (`Must` agents.sdd l.191-197) : un agent système ne se supprime pas
    /// (`VNL-AGENT-003`) — il est piloté par le code et sa version, pas par le CRUD.
    fn before_delete(existing: &Self::Model, _principal: &AuthPrincipal) -> Result<(), HookError> {
        if existing.system_key.is_some() {
            return Err(HookError::with_code(
                "VNL-AGENT-003",
                "system agents cannot be deleted",
            ));
        }
        Ok(())
    }
}

/// Valeurs admises de `mode` (création et mise à jour) : `VNL-AGENT-001` sinon.
fn verifier_mode(mode: &str) -> Result<(), HookError> {
    if mode != "primary" && mode != "subagent" && mode != "all" {
        return Err(HookError::with_code(
            "VNL-AGENT-001",
            format!("mode must be 'primary', 'subagent' or 'all', got: {mode}"),
        ));
    }
    Ok(())
}

/// Vrai quand `current` porte une valeur explicitement posée (`Set`) différente de la
/// valeur relue `previous`. `NotSet` (champ absent du body) et `Unchanged` (valeur de la
/// ligne) ne sont jamais un changement.
fn is_changed<T>(current: &ActiveValue<T>, previous: &T) -> bool
where
    T: PartialEq + Into<sea_orm::Value>,
{
    match current {
        ActiveValue::Set(value) => value != previous,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    use super::*;
    use miryad_core::resource::AccessPolicy;
    use sea_orm::{IntoActiveModel, Set};

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

    fn sample_model(mode: &str) -> Model {
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
            system_key: None,
            system_version: None,
        }
    }

    fn sample(mode: &str) -> ActiveModel {
        sample_model(mode).into_active_model()
    }

    /// Ligne système existante telle que `before_update`/`before_delete` la relèvent :
    /// `system_key` posée, version embarquée (litéral volontaire — pas de dépendance des
    /// tests d'entité à la valeur de `SYSTEM_VERSION`).
    fn existing_system_row() -> Model {
        Model {
            id: 5,
            owner_id: 1,
            name: "config".to_string(),
            description: Some("agent de configuration".to_string()),
            mode: "primary".to_string(),
            model_profile_id: 2,
            toolsets: serde_json::json!([]),
            skills: serde_json::json!([]),
            system_prompt: "prompt embarque".to_string(),
            system_key: Some("config".to_string()),
            system_version: Some(1),
        }
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

    /// `agents.sdd` (tâche DÉCIDÉ 2026-10-18) : `before_update` valide `mode` comme
    /// `before_create` (mêmes valeurs, même code `VNL-AGENT-001`).
    #[test]
    fn before_update_accepte_les_modes_connus() {
        let existing = sample_model("primary");
        for mode in ["primary", "subagent", "all"] {
            let mut active = existing.clone().into_active_model();
            active.mode = Set(mode.to_string());
            assert!(
                Entity::before_update(active, &existing, &test_principal()).is_ok(),
                "mode {mode} doit passer"
            );
        }
    }

    #[test]
    fn before_update_rejette_mode_inconnu_avec_vnl_agent_001() {
        let existing = sample_model("primary");
        let mut active = existing.clone().into_active_model();
        active.mode = Set("bogus".to_string());
        let err = Entity::before_update(active, &existing, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-001"));
    }

    /// Un update qui ne touche pas `mode` (`NotSet`/`Unchanged`) n'est pas revalidé : la
    /// valeur relue reste la responsabilité de la ligne existante.
    #[test]
    fn before_update_sans_mode_pose_ne_revalide_pas() {
        let existing = sample_model("primary");
        let mut active = existing.clone().into_active_model();
        active.mode = sea_orm::ActiveValue::NotSet;
        active.system_prompt = Set("autre".to_string());
        assert!(Entity::before_update(active, &existing, &test_principal()).is_ok());
    }

    /// La garde des agents SYSTÈME (VNL-AGENT-002) coexiste avec la validation du mode et lui
    /// est PRIORITAIRE : sur une ligne système, tout changement de `mode` (même invalide) est
    /// refusé par la garde, la validation du mode n'est jamais atteinte.
    #[test]
    fn before_update_garde_systeme_reste_prioritaire_sur_le_mode() {
        let existing = existing_system_row();
        let mut active = existing.clone().into_active_model();
        active.mode = Set("bogus".to_string());
        let err = Entity::before_update(active, &existing, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-002"));
    }

    // ── AGENTS SYSTÈME (agents.sdd `Must` l.191-197 ; Scenario de system_agents.sdd) ──

    /// Scenario (`system_agents.sdd`) : `creation_client_avec_cle_systeme` — un corps de
    /// création portant `system_key` est refusé (`VNL-AGENT-004`).
    #[test]
    fn before_create_refuse_corps_portant_system_key() {
        let mut active = sample("primary");
        active.system_key = Set(Some("config".to_string()));
        let err = Entity::before_create(active, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-004"));
    }

    /// Même Scenario, branche `system_version` : un corps portant la version du prompt
    /// embarqué est refusé aussi.
    #[test]
    fn before_create_refuse_corps_portant_system_version() {
        let mut active = sample("primary");
        active.system_version = Set(Some(1));
        let err = Entity::before_create(active, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-004"));
    }

    /// Le défaut serde des deux champs (clé absente du corps → `Set(None)`) passe
    /// `before_create` : les agents ordinaires ne sont pas affectés.
    #[test]
    fn before_create_accepte_ligne_ordinaire_sans_cle_systeme() {
        assert!(Entity::before_create(sample("primary"), &test_principal()).is_ok());
    }

    /// Scenario (`system_agents.sdd`) : `seul_le_modele_est_modifiable` — modifier
    /// `system_prompt` d'une ligne portant `system_key` est refusé (`VNL-AGENT-002`, 422).
    #[test]
    fn before_update_refuse_changement_de_prompt_sur_ligne_systeme() {
        let existing = existing_system_row();
        let mut active = existing.clone().into_active_model();
        active.model_profile_id = Set(9);
        active.system_prompt = Set("prompt detourne".to_string());
        let err = Entity::before_update(active, &existing, &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-002"));
    }

    /// Même Scenario, branche succès : un update qui ne change QUE `model_profile_id` passe.
    #[test]
    fn before_update_accepte_changement_du_seul_modele() {
        let existing = existing_system_row();
        let mut active = existing.clone().into_active_model();
        active.model_profile_id = Set(9);
        assert!(Entity::before_update(active, &existing, &test_principal()).is_ok());
    }

    /// Un update d'une ligne ORDINAIRE (pas de `system_key`) garde son comportement actuel :
    /// tout changement passe.
    #[test]
    fn before_update_accepte_ligne_ordinaire() {
        let existing = sample_model("primary");
        let mut active = existing.clone().into_active_model();
        active.system_prompt = Set("modifie".to_string());
        assert!(Entity::before_update(active, &existing, &test_principal()).is_ok());
    }

    /// Scenario (`system_agents.sdd`) : `suppression_refusee` — `before_delete` refuse la
    /// suppression d'une ligne portant `system_key` (`VNL-AGENT-003`).
    #[test]
    fn before_delete_refuse_ligne_systeme() {
        let err = Entity::before_delete(&existing_system_row(), &test_principal()).unwrap_err();
        assert_eq!(err.code.as_deref(), Some("VNL-AGENT-003"));
    }

    /// La suppression d'un agent ordinaire reste autorisée par les hooks de ce fichier.
    #[test]
    fn before_delete_accepte_ligne_ordinaire() {
        assert!(Entity::before_delete(&sample_model("primary"), &test_principal()).is_ok());
    }

    /// Scenario : `Sérialisation REST expose la forme ligne complète` — depuis
    /// `Must` AGENTS SYSTÈME, la forme ligne compte ONZE clés : les neuf d'origine plus
    /// `system_key` et `system_version` (présentes même à `None`).
    #[test]
    fn serialisation_expose_la_forme_ligne_complete() {
        let row = Model {
            id: 3,
            owner_id: 7,
            name: "x".to_string(),
            description: None,
            mode: "primary".to_string(),
            model_profile_id: 2,
            toolsets: serde_json::json!(["fs"]),
            skills: serde_json::json!("auto"),
            system_prompt: String::new(),
            system_key: None,
            system_version: None,
        };
        let value = serde_json::to_value(&row).expect("serializes");
        let object = value.as_object().expect("object");
        for key in [
            "id",
            "owner_id",
            "name",
            "description",
            "mode",
            "model_profile_id",
            "toolsets",
            "skills",
            "system_prompt",
            "system_key",
            "system_version",
        ] {
            assert!(object.contains_key(key), "missing key {key}");
        }
    }
}

/// Régression Phase 3 (miryad-core-integration) : le corps de création envoyé par
/// `AgentsScreen.vue` (`CreateAgent` — ni `id` ni `owner_id`) doit désérialiser et créer une
/// ressource via `resource_router`, et deux créations successives doivent obtenir des ids
/// distincts (bug trouvé : `#[serde(default)]` manquant sur `id`/`owner_id`, cf.
/// `docs/features/miryad-core-integration.md`).
#[cfg(test)]
mod resource_router_regression {

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
        issue_token(
            db,
            subject,
            "test",
            None,
            crate::auth::test_support::TEST_TOKEN_PEPPER,
        )
        .await
        .expect("issuing succeeds")
        .token
    }

    fn json_request(uri: &str, token: &str, body: &serde_json::Value) -> Request<Body> {
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
            .oneshot(json_request("/api/v1/agents", &token, &body("coder")))
            .await
            .expect("router does not fail");
        assert_eq!(
            first.status(),
            StatusCode::CREATED,
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
            .oneshot(json_request("/api/v1/agents", &token, &body("reviewer")))
            .await
            .expect("router does not fail");
        assert_eq!(
            second.status(),
            StatusCode::CREATED,
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

    /// Ligne d'agent ordinaire d'`owner` insérée directement ; rend son id.
    async fn seed_agent_row(db: &DatabaseConnection, owner: i32, profile: i32) -> i32 {
        crate::db::entities::agents::Entity::insert(crate::db::entities::agents::ActiveModel {
            id: NotSet,
            owner_id: Set(owner),
            name: Set("reviewer".to_string()),
            description: Set(None),
            mode: Set("primary".to_string()),
            model_profile_id: Set(profile),
            toolsets: Set(serde_json::json!([])),
            skills: Set(serde_json::json!("auto")),
            system_prompt: Set(String::new()),
            system_key: Set(None),
            system_version: Set(None),
        })
        .exec(db)
        .await
        .expect("agent row inserts")
        .last_insert_id
    }

    /// `PUT /api/v1/agents/{id}` via `resource_router` ; rend le statut et le corps.
    async fn put_json(
        app: Router,
        uri: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> (StatusCode, String) {
        let req = Request::builder()
            .method("PUT")
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("valid request");
        let resp = app.oneshot(req).await.expect("router does not fail");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("readable body");
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// Scenario `put_mode_invalide_rejete_ligne_intacte` (agents.sdd) : un PUT au `mode`
    /// inconnu est rejeté avec `VNL-AGENT-001` et la ligne en base reste INCHANGÉE ; un PUT
    /// à valeur valide réussit.
    #[tokio::test]
    async fn put_mode_invalide_rejete_ligne_intacte() {
        let db = real_db().await;
        let alice = resolve_user(&db, "alice", None)
            .await
            .expect("alice resolves");
        let profile_id = seed_model_profile(&db, alice.id).await;
        let row_id = seed_agent_row(&db, alice.id, profile_id).await;
        let state = test_auth_state_with_db(db);
        let token = bearer_for(&state.db, "alice").await;
        let app = Router::new()
            .merge(resource_router::<AgentEntity, _>())
            .with_state(state.clone());
        let uri = format!("/api/v1/agents/{row_id}");
        let body = |mode: &str| {
            serde_json::json!({
                "name": "reviewer",
                "mode": mode,
                "model_profile_id": profile_id,
                "toolsets": [],
                "skills": "auto",
                "system_prompt": "",
            })
        };

        let (status, text) = put_json(app.clone(), &uri, &token, &body("bogus")).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "PUT au mode inconnu rejeté : {text}"
        );
        assert!(text.contains("VNL-AGENT-001"), "code rendu : {text}");
        let stored = AgentEntity::find_by_id(row_id)
            .one(&state.db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(stored.mode, "primary", "la ligne est restée intacte");

        let (status, text) = put_json(app, &uri, &token, &body("subagent")).await;
        assert_eq!(status, StatusCode::OK, "PUT valide réussit : {text}");
        let stored = AgentEntity::find_by_id(row_id)
            .one(&state.db)
            .await
            .expect("query")
            .expect("row exists");
        assert_eq!(stored.mode, "subagent");
    }
}
