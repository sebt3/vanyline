use uuid::Uuid;
use vanyline_crds::OwnerSpec;

use crate::{db::models::User, error::AppError, AppState};

/// Détecte d'un raw (email ou oidc_sub) une étiquette DNS `RFC1123` :
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

/// Lit `users.k8s_owner_name` pour `user_id`. `Some(...)` si l'Owner a déjà
/// été résolu ; `None` sinon (routes de lecture : « aucun Owner »).
pub async fn resolve_owner_name(
    state: &AppState,
    user_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(
        sqlx::query_scalar::<_, Option<String>>("SELECT k8s_owner_name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::DatabaseError)?
            .flatten(),
    )
}

/// Crée l'Owner si absent et persiste `users.k8s_owner_name` pour `db_user`.
/// Réservé au `POST /api/projects` (décision développeur : lazy provisioning
/// restreint). Retourne le nom d'Owner.
pub async fn ensure_owner(state: &AppState, db_user: &User) -> Result<String, AppError> {
    if let Some(name) = &db_user.k8s_owner_name {
        return Ok(name.clone());
    }
    let local = db_user.email.split('@').next().unwrap_or_default();
    let raw = if local.trim().is_empty() {
        &db_user.oidc_sub
    } else {
        &db_user.email
    };
    let name = sanitize_owner_name(raw);

    let k8s = crate::k8s::client(state).await?;
    match k8s.get_owner(&name).await {
        Ok(_) => {}
        Err(_) => {
            k8s.create_owner(
                &name,
                OwnerSpec {
                    existing_pvc: None,
                    home_size: None,
                    home_storage_class: None,
                    project_defaults: None,
                    application_ref: None,
                    egress: Vec::new(),
                },
            )
            .await?;
        }
    }
    sqlx::query("UPDATE users SET k8s_owner_name = $1 WHERE id = $2")
        .bind(&name)
        .bind(db_user.id)
        .execute(&state.pool)
        .await?;
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
