use miryad_core::auth::AuthPrincipal;
use sea_orm::{ActiveValue::NotSet, ColumnTrait, EntityTrait, QueryFilter, Set};
use vanyline_crds::{OwnerSpec, ProjectDefaults};

use crate::{AppState, error::AppError};

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
        .map_err(AppError::from)?;
    Ok(link.and_then(|l| l.k8s_owner_name))
}

/// Choisit le `raw` alimentant `sanitize_owner_name` dans `ensure_owner` (owners.sdd
/// `Must`, décision Sébastien 2026-10-07) : `preferred_username` quand il est
/// `Some` non vide après `trim()`, sinon `subject` (garanti unique par le
/// fournisseur). L'email n'entre JAMAIS dans le calcul.
fn owner_raw_source(principal: &AuthPrincipal) -> &str {
    match principal.preferred_username.as_deref() {
        Some(preferred) if !preferred.trim().is_empty() => preferred,
        _ => principal.subject.as_str(),
    }
}

/// Crée l'Owner si absent et persiste `vanyline_owner_links.k8s_owner_name` pour
/// `user_id`. Réservé au `POST /api/projects` (décision développeur : lazy
/// provisioning restreint). Retourne le nom d'Owner.
pub async fn ensure_owner(
    state: &AppState,
    user_id: i32,
    principal: &AuthPrincipal,
) -> Result<String, AppError> {
    if let Some(name) = resolve_owner_name(state, user_id).await? {
        return Ok(name);
    }
    // Choix du raw (owners.sdd `Must`, 2026-10-07) : preferred_username non vide,
    // sinon subject — l'email est retiré de la chaîne de nommage.
    let name = sanitize_owner_name(owner_raw_source(principal));

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

    // Upsert atomique (INSERT ... ON CONFLICT (user_id) DO UPDATE) : élimine la fenêtre de
    // course du find-then-insert précédent, où deux requêtes concurrentes passant toutes les
    // deux le `resolve_owner_name` en tête de fonction pouvaient percuter la contrainte unique
    // sur `user_id` (la seconde échouant en 500 brut au lieu d'être idempotente).
    let db = &state.auth.db;
    let active = crate::db::entities::owner_links::ActiveModel {
        id: NotSet,
        user_id: Set(user_id),
        k8s_owner_name: Set(Some(name.clone())),
    };
    OwnerLinkEntity::insert(active)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(Column::UserId)
                .update_column(Column::K8sOwnerName)
                .to_owned(),
        )
        .exec(db)
        .await
        .map_err(AppError::from)?;

    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::{owner_raw_source, sanitize_owner_name};
    use miryad_core::auth::{AuthPrincipal, PrincipalSource};

    fn principal(email: Option<&str>, preferred: Option<&str>, subject: &str) -> AuthPrincipal {
        AuthPrincipal {
            subject: subject.to_string(),
            email: email.map(str::to_string),
            preferred_username: preferred.map(str::to_string),
            source: PrincipalSource::Session {
                id_token: String::new(),
            },
        }
    }

    // Scenario: ensure_owner nomme d'après preferred_username et ignore l'email
    // (choix du `raw` — décision Sébastien 2026-10-07, owners.sdd `Must`).
    #[test]
    fn owner_raw_source_prefers_preferred_username_over_email() {
        let p = principal(
            Some("Alice.Doe@corp.example"),
            Some("Alice.Doe"),
            "6d4f3a-oidc-sub",
        );
        assert_eq!(owner_raw_source(&p), "Alice.Doe");
    }

    // Scenario: ensure_owner retombe sur le subject sans preferred_username
    // (chemin token API ou fournisseur sans le claim — owners.sdd `Must`).
    #[test]
    fn owner_raw_source_none_preferred_retombe_sur_subject() {
        let p = principal(Some("alice@x.com"), None, "sub-42");
        assert_eq!(owner_raw_source(&p), "sub-42");
    }

    // Scenario: ensure_owner retombe sur le subject sans preferred_username
    // (`Some("")` compte comme absent : « Some non vide » exige un contenu).
    #[test]
    fn owner_raw_source_empty_preferred_retombe_sur_subject() {
        let p = principal(Some("alice@x.com"), Some(""), "sub-42");
        assert_eq!(owner_raw_source(&p), "sub-42");
    }

    // Scenario: ensure_owner retombe sur le subject sans preferred_username
    // (contrat amendé owners.sdd `Must`, validator 2026-10-07 : « `Some` non vide
    // APRÈS `trim()` » — un `preferred_username` uniquement whitespace compte absent).
    #[test]
    fn owner_raw_source_whitespace_only_retombe_sur_subject() {
        let p = principal(Some("alice@x.com"), Some("   "), "sub-42");
        assert_eq!(
            owner_raw_source(&p),
            "sub-42",
            "un preferred_username whitespace-only doit retomber sur le subject"
        );
    }

    // Contrat gelé owners.sdd `Must` (2026-10-07) : l'EMAIL EST RETIRÉ de la
    // chaîne de nommage — avec preferred_username et email présents, le choix
    // n'est jamais l'email (ni brut, ni local-part).
    #[test]
    fn owner_raw_source_ne_retient_jamais_l_email() {
        let p = principal(Some("Alice.Doe@corp.example"), Some("Alice.Doe"), "sub-42");
        let raw = owner_raw_source(&p);
        assert!(!raw.contains('@'), "jamais l'email brut : {raw:?}");
        assert_ne!(raw, "Alice.Doe@corp.example");
        let q = principal(Some("bob@y.com"), None, "sub-99");
        assert_eq!(
            owner_raw_source(&q),
            "sub-99",
            "repli = subject, jamais l'email"
        );
    }

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
