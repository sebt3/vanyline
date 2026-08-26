use miryad_core::auth::AuthUser;
use sea_orm::{ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};
use vanyline_crds::{OwnerSpec, ProjectDefaults};

use crate::{AppState, error::AppError};

fn db_err(e: sea_orm::DbErr) -> AppError {
    AppError::InternalError(format!("VNL-DB-006: {e}"))
}

/// Détecte d'un raw (email ou `oidc_sub`) une étiquette DNS `RFC1123` :
/// minuscules/chiffres/tirets, début alphanumérique, ≤ 63 caractères.
/// Préfixe le local-part de l'email (`@`), remplace les caractères invalides
/// par `-`, coupe aux 63, et retombe sur `"owner"` si le résultat est vide.
pub fn sanitize_owner_name(raw: &str) -> String {
    let local = raw.split('@').next().unwrap_or_default();
    let mut out = String::with_capacity(local.len());
    for (i, ch) in local.to_lowercase().chars().enumerate() {
        let valid = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-';
        let c = if valid { ch } else { '-' };
        if i == 0 && !(c.is_ascii_lowercase() || c.is_ascii_digit()) {
            out.push('a');
        } else {
            out.push(c);
        }
    }
    let trimmed = out.trim_matches('-');
    let cut: String = trimmed.chars().take(63).collect();
    // Le cut à 63 caractères peut retomber pile sur un `-` (ex. un `-` en
    // position 62 d'un local-part plus long) — retrim pour ne jamais
    // renvoyer une étiquette qui se termine par un tiret (RFC1123 : début
    // ET fin alphanumériques).
    let mut label = cut.trim_end_matches('-').to_string();
    if label.is_empty() {
        label = "owner".to_string();
    }
    label
}

/// Lit `vanyline_owner_links.k8s_owner_name` pour `user_id` (id miryad_users).
/// `Some(...)` si l'Owner a déjà été résolu ; `None` sinon (routes de lecture :
/// « aucun Owner »).
pub async fn resolve_owner_name(
    state: &AppState,
    user_id: i32,
) -> Result<Option<String>, AppError> {
    use crate::db::entities::owner_links::Column;
    use crate::db::entities::owner_links::Entity as OwnerLinkEntity;

    let db = &state.auth.db;
    let link = OwnerLinkEntity::find()
        .filter(Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(db_err)?;
    Ok(link.and_then(|l| l.k8s_owner_name))
}

/// Crée l'Owner si absent et persiste `vanyline_owner_links.k8s_owner_name` pour
/// `user_id`. Réservé au `POST /api/projects` (décision développeur : lazy
/// provisioning restreint). Retourne le nom d'Owner.
pub async fn ensure_owner(
    state: &AppState,
    user_id: i32,
    principal: &AuthUser,
) -> Result<String, AppError> {
    if let Some(name) = resolve_owner_name(state, user_id).await? {
        return Ok(name);
    }
    let local = principal.email.as_deref().unwrap_or("").split('@').next().unwrap_or_default();
    let raw = if local.trim().is_empty() {
        principal.subject.as_str()
    } else {
        principal.email.as_deref().unwrap_or("")
    };
    let name = sanitize_owner_name(raw);

    let k8s = crate::k8s::client(state).await?;
    if k8s.get_owner(&name).await.is_err() {
        let project_defaults = if state.config.default_project_storage_class.is_some()
            || state.config.default_project_access_mode.is_some()
        {
            Some(ProjectDefaults {
                storage_size: None,
                storage_class: state.config.default_project_storage_class.clone(),
                storage_access_mode: state.config.default_project_access_mode.clone(),
            })
        } else {
            None
        };
        k8s.create_owner(
            &name,
            OwnerSpec {
                existing_pvc: None,
                home_size: None,
                home_storage_class: state.config.default_home_storage_class.clone(),
                home_access_mode: state.config.default_home_access_mode.clone(),
                project_defaults,
                application_ref: state.config.application_name.clone(),
                egress: Vec::new(),
            },
        )
        .await?;
    }

    use crate::db::entities::owner_links::Column;
    use crate::db::entities::owner_links::Entity as OwnerLinkEntity;

    let db = &state.auth.db;
    let existing = OwnerLinkEntity::find()
        .filter(Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(db_err)?;
    if let Some(existing) = existing {
        let mut active = existing.into_active_model();
        active.k8s_owner_name = Set(Some(name.clone()));
        OwnerLinkEntity::update(active).exec(db).await.map_err(db_err)?;
    } else {
        let active = crate::db::entities::owner_links::ActiveModel {
            id: Set(0),
            user_id: Set(user_id),
            k8s_owner_name: Set(Some(name.clone())),
        };
        OwnerLinkEntity::insert(active).exec(db).await.map_err(db_err)?;
    }

    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::sanitize_owner_name;

    #[test]
    fn sanitize_prefixed_email_local_part() {
        assert_eq!(sanitize_owner_name("John.Doe@example.com"), "john-doe");
        assert_eq!(sanitize_owner_name("valid-owner@host"), "valid-owner");
    }

    #[test]
    fn sanitize_owner_empty_local_falls_back() {
        assert_eq!(sanitize_owner_name("@example.com"), "owner");
    }

    #[test]
    fn sanitize_owner_truncates_to_63() {
        let input = format!("{}@x", "a".repeat(70));
        assert_eq!(sanitize_owner_name(&input), "a".repeat(63));
    }

    #[test]
    fn sanitize_owner_truncation_never_ends_in_hyphen() {
        // Le '-' en position 62 (0-indexed) tombe exactement sur la coupe à 63.
        let input = format!("{}-{}@x", "a".repeat(62), "b".repeat(10));
        let result = sanitize_owner_name(&input);
        assert!(
            !result.ends_with('-'),
            "label should never end with '-' after truncation, got {result:?}"
        );
        assert_eq!(result, "a".repeat(62));
    }
}
