use axum::{Json, extract::State};
use serde::Serialize;

use vanyline_tools::mcp::{command_tools, filesystem_tools, search_tools};

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;

use crate::{AppState, error::AppError};

#[derive(Serialize)]
pub struct LocalTool {
    pub name: String,
    pub description: String,
}

/// Aplati le registre statique `tools::mcp` — pure, testable sans routeur.
fn flatten_local_tools() -> Vec<LocalTool> {
    let mut out = Vec::new();
    for tool in filesystem_tools()
        .into_iter()
        .chain(search_tools())
        .chain(command_tools())
    {
        if let (Some(name), Some(description)) =
            (tool["name"].as_str(), tool["description"].as_str())
        {
            out.push(LocalTool {
                name: name.to_string(),
                description: description.to_string(),
            });
        }
    }
    out
}

pub async fn list_local_tools(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<LocalTool>>, AppError> {
    resolve_user(&state.auth.db, &user.subject, user.email.as_deref())
        .await
        .map_err(AppError::from)?;
    Ok(Json(flatten_local_tools()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn test_key() -> cookie::Key {
        cookie::Key::from(&[0u8; 64])
    }

    fn make_app() -> Router {
        make_app_with_auth(crate::auth::test_support::test_auth_state())
    }

    fn make_app_with_auth(auth: miryad_core::auth::MiryadAuthState) -> Router {
        let config = crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
            oidc_connect_timeout: std::time::Duration::from_secs(5),
            oidc_timeout: std::time::Duration::from_secs(15),
            cookie_secret: "0".repeat(64),
            database_url: "postgres://localhost/test".to_string(),

            listen_addr: "0.0.0.0:8080".to_string(),
            static_dir: "./static".to_string(),
            k8s_namespace: None,
            application_name: None,
            default_home_storage_class: None,
            default_home_access_mode: None,
            default_project_storage_class: None,
            default_project_access_mode: None,
            restate_admin_url: None,
            restate_ingress_url: None,
            workflow_listen: "0.0.0.0:9080".to_string(),
            workflow_deployment_url: None,
        };

        let state = AppState {
            config,
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth,
            shared_sandbox_state: crate::ws::sandbox_state::SharedState::new(),
            workflow: None,
        };

        Router::new()
            .route("/local-tools", axum::routing::get(list_local_tools))
            .with_state(state)
    }

    #[test]
    fn flatten_returns_8_tools() {
        let tools = flatten_local_tools();
        assert_eq!(tools.len(), 8);
    }

    #[test]
    fn flatten_contains_all_expected_names() {
        let tools = flatten_local_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"delete_file"));
        assert!(names.contains(&"list_directory"));
        assert!(names.contains(&"find_files"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"execute_command"));
    }

    #[test]
    fn flatten_all_descriptions_non_empty() {
        let tools = flatten_local_tools();
        for tool in &tools {
            assert!(
                !tool.description.is_empty(),
                "tool '{}' has empty description",
                tool.name
            );
        }
    }

    #[test]
    fn flatten_ordre_filesystem_puis_search_puis_commande() {
        let names: Vec<String> = flatten_local_tools().into_iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "list_directory",
                "find_files",
                "search",
                "execute_command",
            ]
        );
    }

    #[test]
    fn local_tool_serialize_en_name_et_description() {
        let tool = LocalTool {
            name: "read_file".to_string(),
            description: "Lit un fichier".to_string(),
        };
        let json = serde_json::to_value(&tool).expect("serialize LocalTool");
        assert_eq!(
            json,
            serde_json::json!({"name": "read_file", "description": "Lit un fichier"})
        );
    }

    #[tokio::test]
    async fn list_local_tools_authentifie_repond_200_et_tableau_de_paires() {
        // Cookie de session scellé avec la même clé que `make_app` (extracteur `AuthUser`
        // = cookie seul). `exp` 2100-01-01 : le token factice n'est pas vérifié, seul l'exp l'est.
        let db = crate::db::test_support::real_db().await;
        let identity = miryad_core::auth::OidcIdentity {
            id_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.signature".to_string(),
            subject: "alice".to_string(),
            email: None,
            preferred_username: None,
        };
        let set_cookie = miryad_core::auth::cookie::build_set_cookie(&identity, &test_key(), false);
        let cookie_pair = set_cookie
            .split(';')
            .next()
            .expect("cookie has a name=value pair")
            .to_string();
        let app = make_app_with_auth(crate::auth::test_support::test_auth_state_with_db(db));
        let req = Request::builder()
            .uri("/local-tools")
            .header("Cookie", cookie_pair)
            .body(Body::empty())
            .expect("valid request");
        let resp = app.oneshot(req).await.expect("router does not fail");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        let items = json.as_array().expect("body is an array");
        assert_eq!(items.len(), 8);
        for item in items {
            let obj = item.as_object().expect("entry is an object");
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(keys, ["description", "name"]);
            assert!(item["name"].as_str().is_some_and(|v| !v.is_empty()));
            assert!(item["description"].as_str().is_some_and(|v| !v.is_empty()));
        }
    }

    #[tokio::test]
    async fn list_local_tools_without_cookie_returns_401() {
        let app = make_app();
        let req = Request::builder()
            .uri("/local-tools")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
