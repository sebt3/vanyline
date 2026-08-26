use axum::{Json, extract::State};
use serde::Serialize;

use vanyline_tools::mcp::{command_tools, filesystem_tools, search_tools};

use miryad_core::auth::AuthUser;
use miryad_core::users::resolve_user;

use crate::{
    AppState, error::AppError,
};

fn db_err(e: sea_orm::DbErr) -> AppError {
    AppError::InternalError(format!("VNL-DB-006: {e}"))
}

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
        .map_err(db_err)?;
    Ok(Json(flatten_local_tools()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
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
        let config = crate::config::Config {
            oidc_issuer_url: "https://issuer.example.com".to_string(),
            oidc_client_id: "client-id".to_string(),
            oidc_client_secret: "client-secret".to_string(),
            oidc_redirect_url: "https://app.example.com/callback".to_string(),
            oidc_scopes: vec![],
            oidc_ca_cert: None,
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
        };

        let state = AppState {
            config,
            cookie_key: test_key(),
            busy: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            k8s: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            auth: crate::auth::test_support::test_auth_state(),
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
