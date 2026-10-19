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

/// Lit `vanyline_owner_links.k8s_owner_name` pour `user_id` (id `miryad_users`).
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

/// Décision de nommage/création d'un Owner (owners.sdd « Lien périmé », 2026-10-19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerPlan {
    /// Le lien existe et son Owner est présent : on réutilise son nom.
    Reuse(String),
    /// Owner à créer sous ce nom (nom du lien s'il existait, sinon nom calculé).
    Create(String),
}

/// Fonction pure : choisit le nom d'Owner à utiliser. Un lien n'est pris en compte que
/// si son Owner existe ; sinon le nom du lien est conservé (jamais recalculé).
#[must_use]
pub fn plan_owner(link: Option<String>, computed: String, exists: bool) -> OwnerPlan {
    match link {
        Some(name) if exists => OwnerPlan::Reuse(name),
        Some(name) => OwnerPlan::Create(name),
        None => OwnerPlan::Create(computed),
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
    use crate::db::entities::owner_links::Column;
    use crate::db::entities::owner_links::Entity as OwnerLinkEntity;

    // (1) Lien résolu — jamais d'early-return : son Owner est re-vérifié côté k8s.
    let link = resolve_owner_name(state, user_id).await?;
    // (2) Nom calculé (owners.sdd `Must`, 2026-10-07) : preferred_username non vide,
    // sinon subject — l'email est retiré de la chaîne de nommage.
    let computed = sanitize_owner_name(owner_raw_source(principal));

    // (3)-(4) Existence de l'Owner visé (le lien s'il existe, sinon le nom calculé).
    // Toute erreur de get_owner vaut « absent » (cf. Tasks `[!]` de owners.sdd).
    let k8s = crate::k8s::client(state).await?;
    let probe = link.clone().unwrap_or_else(|| computed.clone());
    let exists = k8s.get_owner(&probe).await.is_ok();
    let (OwnerPlan::Reuse(name) | OwnerPlan::Create(name)) = plan_owner(link, computed, exists);
    if !exists {
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
    use super::{OwnerPlan, owner_raw_source, plan_owner, sanitize_owner_name};
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
    fn local_part_d_email_est_assaini_en_etiquette_minuscule() {
        assert_eq!(sanitize_owner_name("John.Doe@example.com"), "john-doe");
        assert_eq!(sanitize_owner_name("valid-owner@host"), "valid-owner");
    }

    #[test]
    fn email_a_local_part_vide_retombe_sur_le_repli_owner() {
        assert_eq!(sanitize_owner_name("@example.com"), "owner");
    }

    #[test]
    fn local_part_plus_long_que_63_caracteres_est_coupe_a_63() {
        let input = format!("{}@x", "a".repeat(70));
        assert_eq!(sanitize_owner_name(&input), "a".repeat(63));
    }

    #[test]
    fn la_coupe_a_63_caracteres_ne_finit_jamais_par_un_tiret() {
        // Le '-' en position 62 (0-indexed) tombe exactement sur la coupe à 63.
        let input = format!("{}-{}@x", "a".repeat(62), "b".repeat(10));
        let result = sanitize_owner_name(&input);
        assert!(
            !result.ends_with('-'),
            "label should never end with '-' after truncation, got {result:?}"
        );
        assert_eq!(result, "a".repeat(62));
    }

    /// Scenario : `premier caractère invalide est remplacé par la lettre a`.
    #[test]
    fn premier_caractere_invalide_est_remplace_par_la_lettre_a() {
        assert_eq!(sanitize_owner_name("_alice@x"), "aalice");
        assert_eq!(sanitize_owner_name("-bob-"), "abob");
    }

    /// Scenario : `chaine sans arobase passe au filtre caractère par caractère`.
    #[test]
    fn chaine_sans_arobase_passe_au_filtre_caractere_par_caractere() {
        assert_eq!(sanitize_owner_name("sub-42"), "sub-42");
        assert_eq!(sanitize_owner_name("keycloak|abc.1"), "keycloak-abc-1");
    }

    /// Scenario : `seule la partie avant le premier arobase est conservée`.
    #[test]
    fn seule_la_partie_avant_le_premier_arobase_est_conservee() {
        assert_eq!(sanitize_owner_name("a@b@c"), "a");
    }

    /// Scenario : `caractères non ASCII deviennent des séparateurs découpés`.
    #[test]
    fn caracteres_non_ascii_deviennent_des_separateurs_decoupes() {
        assert_eq!(sanitize_owner_name("josé@x"), "jos");
    }

    /// Scenario : `chaine vide et tout-tirets aux cas limites`.
    #[test]
    fn chaine_vide_et_tout_tirets_aux_cas_limites() {
        assert_eq!(sanitize_owner_name(""), "owner");
        assert_eq!(sanitize_owner_name("----"), "a");
    }

    // Scenario: plan_owner_reutilise_un_lien_dont_l_owner_existe
    #[test]
    fn plan_owner_reutilise_un_lien_dont_l_owner_existe() {
        assert_eq!(
            plan_owner(Some("alice".into()), "bob".into(), true),
            OwnerPlan::Reuse("alice".into())
        );
    }

    // Scenario: plan_owner_recree_un_owner_disparu_sous_le_nom_du_lien
    #[test]
    fn plan_owner_recree_un_owner_disparu_sous_le_nom_du_lien() {
        assert_eq!(
            plan_owner(Some("alice".into()), "bob".into(), false),
            OwnerPlan::Create("alice".into())
        );
    }

    // Scenario: plan_owner_sans_lien_cree_sous_le_nom_calcule
    #[test]
    fn plan_owner_sans_lien_cree_sous_le_nom_calcule() {
        assert_eq!(
            plan_owner(None, "bob".into(), false),
            OwnerPlan::Create("bob".into())
        );
    }
}
