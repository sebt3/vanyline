//! Agrégation du module dashboards — contrat : `./mod.sdd`. PORTEUR de la fonction de
//! validation FR-3 partagée entre l'écriture (hook de `../db/entities/dashboards.rs`) et la
//! lecture (`./resolve.sdd`). Sans état : aucune fonction ne capture de connection, de
//! config ou de registre mutable (`DEC-1`).

pub mod defaults;
pub mod model;
pub mod registry;
pub mod resolve;

use model::DASHBOARD_GRID_COLUMNS;

/// Nombre maximal de widgets par définition (`OI-2` du DSD, borne `VNL-APP-DSH-001`).
pub const DASHBOARD_MAX_WIDGETS: usize = 24;

/// Taille sérialisée maximale de la partie stockée `{layout, widgets}` (`OI-2` du DSD,
/// borne `VNL-APP-DSH-002`).
pub const DASHBOARD_MAX_BYTES: usize = 65_536;

/// Garde-fou FR-3 partagé (`./mod.sdd`) : écriture (hook de l'entité) et lecture
/// (`./resolve.sdd`). Évalue les règles DANS L'ORDRE contractuel — taille (`002`), nombre
/// (`001`), ids (`005`), types (`003`), config objet (`006`), coordonnées (`004`) — et rend
/// le PREMIER manquement seul, jamais deux marqueurs pour une définition. Ne panique pas et
/// ne lit ni DB ni registre dynamique.
///
/// # Errors
///
/// Le couple `(code, message)` du premier manquement, codes `VNL-APP-DSH-001..006`.
pub fn validate_definition(def: &model::DashboardDef) -> Result<(), (&'static str, &'static str)> {
    // Règle 1 — taille sérialisée `{layout, widgets}` (`model::serializes_within_limit`,
    // source unique de la mesure).
    if !model::serializes_within_limit(def, DASHBOARD_MAX_BYTES) {
        return Err((
            "VNL-APP-DSH-002",
            "définition de tableau de bord sérialisée au-delà de 64 Kio (VNL-APP-DSH-002)",
        ));
    }
    // Règle 2 — nombre de widgets.
    if def.widgets.len() > DASHBOARD_MAX_WIDGETS {
        return Err((
            "VNL-APP-DSH-001",
            "définition de tableau de bord au-delà de 24 widgets (VNL-APP-DSH-001)",
        ));
    }
    // Règle 3 — ids uniques.
    let mut seen: std::collections::HashSet<&str> =
        std::collections::HashSet::with_capacity(def.widgets.len());
    for widget in &def.widgets {
        if !seen.insert(widget.id.as_str()) {
            return Err((
                "VNL-APP-DSH-005",
                "identifiants de widgets non uniques dans le tableau de bord (VNL-APP-DSH-005)",
            ));
        }
    }
    // Règle 4 — types connus du registre statique (`./registry.rs`).
    for widget in &def.widgets {
        if registry::known(&widget.widget_type).is_none() {
            return Err((
                "VNL-APP-DSH-003",
                "type de widget inconnu du registre (VNL-APP-DSH-003)",
            ));
        }
    }
    // Règle 5 — config objet.
    for widget in &def.widgets {
        if !widget.config.is_object() {
            return Err((
                "VNL-APP-DSH-006",
                "config de widget non-objet (VNL-APP-DSH-006)",
            ));
        }
    }
    // Règle 6 — coordonnées dans la grille FIXE de 12 colonnes (`FR-1`) : `columns` vaut 12,
    // `w ≥ 1`, `x + w ≤ 12` (donc `w ≤ 12`), `h ∈ [1, 64]`, `y < 4096`.
    let columns = def.layout.columns;
    if columns != DASHBOARD_GRID_COLUMNS {
        return Err((
            "VNL-APP-DSH-004",
            "grille différente de 12 colonnes (VNL-APP-DSH-004)",
        ));
    }
    for widget in &def.widgets {
        let within_row = widget.w >= 1
            && widget.x < columns
            && widget
                .x
                .checked_add(widget.w)
                .is_some_and(|end| end <= columns);
        let within_bounds = widget.h >= 1 && widget.h <= 64 && widget.y < 4096;
        if !(within_row && within_bounds) {
            return Err((
                "VNL-APP-DSH-004",
                "coordonnées de widget hors grille (VNL-APP-DSH-004)",
            ));
        }
    }
    // Règle 7 — `config` d'un widget `scripted` : `script` chaîne non vide, `ttl_secs` (s'il est
    // présent) entier positif ou nul (`dsd/app/widgets.md#FR-1`).
    for widget in &def.widgets {
        if widget.widget_type == "scripted" && !scripted_config_valid(&widget.config) {
            return Err((
                "VNL-APP-DSH-011",
                "config de widget scripté invalide (VNL-APP-DSH-011)",
            ));
        }
    }
    Ok(())
}

/// Forme de la `config` d'un widget `scripted` (règle 7) : `script` non vide, `ttl_secs` absent
/// ou entier positif ou nul.
fn scripted_config_valid(config: &serde_json::Value) -> bool {
    let script_ok = config
        .get("script")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|script| !script.is_empty());
    let ttl_ok = config
        .get("ttl_secs")
        .is_none_or(|ttl| ttl.as_u64().is_some());
    script_ok && ttl_ok
}

/// Garde-fou des définitions ÉPHÉMÈRES (`dsd/app/dashboards.md#FR-9`, `dsd/app/widgets.md#FR-9`) :
/// `validate_definition` puis refus de tout widget `scripted` (`VNL-APP-DSH-012`). Jamais appelée
/// par le hook d'écriture (un éphémère n'est pas stocké).
///
/// # Errors
///
/// Le couple `(code, message)` du premier manquement, codes `VNL-APP-DSH-001..006, 011, 012`.
// Consommateur à venir : la réception d'un éphémère (`FR-9`, vague G6/H1). Aucun appelant
// d'écriture — un éphémère n'est jamais stocké (`./mod.sdd`).
#[allow(dead_code)]
pub fn validate_ephemeral(def: &model::DashboardDef) -> Result<(), (&'static str, &'static str)> {
    validate_definition(def)?;
    if def
        .widgets
        .iter()
        .any(|widget| widget.widget_type == "scripted")
    {
        return Err((
            "VNL-APP-DSH-012",
            "widget scripté refusé dans un tableau éphémère (VNL-APP-DSH-012)",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboards::model::{DashboardDef, Layout, Widget};
    use serde_json::{Value, json};

    fn widget(id: &str, widget_type: &str, config: Value, x: u16, w: u16) -> Widget {
        Widget {
            id: id.to_string(),
            widget_type: widget_type.to_string(),
            config,
            x,
            y: 0,
            w,
            h: 4,
        }
    }

    fn def_with(widgets: Vec<Widget>) -> DashboardDef {
        DashboardDef {
            id: None,
            title: String::new(),
            layout: Layout { columns: 12 },
            widgets,
        }
    }

    /// Scenario: `la première borne touchée est celle rendue`.
    #[test]
    fn la_premiere_borne_touch_est_celle_rendue() {
        // (1) 25 widgets de type connu (taille OK) → 001.
        let many: Vec<Widget> = (0..25)
            .map(|i| widget(&format!("w-{i}"), "chat", json!({}), 0, 1))
            .collect();
        let err =
            validate_definition(&def_with(many)).expect_err("25 widgets doivent être refusés");
        assert_eq!(err.0, "VNL-APP-DSH-001");

        // (2) 25 widgets dont un `scripted` → la borne de NOMBRE prime sur le type inconnu.
        let mut mixed: Vec<Widget> = (0..24)
            .map(|i| widget(&format!("w-{i}"), "chat", json!({}), 0, 1))
            .collect();
        mixed.push(widget("w-24", "scripted", json!({}), 0, 1));
        let err = validate_definition(&def_with(mixed))
            .expect_err("25 widgets dont un scripted doivent être refusés");
        assert_eq!(
            err.0, "VNL-APP-DSH-001",
            "le nombre prime sur le type inconnu"
        );

        // (3) un widget x=11 w=4 sur grille 12 → 004.
        let err = validate_definition(&def_with(vec![widget("w-a", "chat", json!({}), 11, 4)]))
            .expect_err("x+w hors grille doit être refusé");
        assert_eq!(err.0, "VNL-APP-DSH-004");

        // (4) un widget à config scalaire `3` → 006.
        let err = validate_definition(&def_with(vec![widget("w-a", "markdown", json!(3), 0, 6)]))
            .expect_err("une config non-objet doit être refusée");
        assert_eq!(err.0, "VNL-APP-DSH-006");
    }

    fn def_grille(columns: u16, widgets: Vec<Widget>) -> DashboardDef {
        DashboardDef {
            id: None,
            title: String::new(),
            layout: Layout { columns },
            widgets,
        }
    }

    fn scripted(config: Value) -> Widget {
        widget("w-s", "scripted", config, 0, 6)
    }

    /// Scenario: `la config d'un widget scripté est contrôlée` (règle 7, `VNL-APP-DSH-011`).
    #[test]
    fn la_config_d_un_widget_scripte_est_controlee() {
        validate_definition(&def_with(vec![scripted(json!({"script": "1"}))]))
            .expect("un script non vide passe");
        for config in [
            json!({}),
            json!({"script": "", "ttl_secs": 5}),
            json!({"script": "1", "ttl_secs": -1}),
        ] {
            let err = validate_definition(&def_with(vec![scripted(config.clone())]))
                .expect_err("la config scriptée invalide doit être refusée");
            assert_eq!(err.0, "VNL-APP-DSH-011", "config {config}");
        }
    }

    /// Scenario: `un tableau éphémère refuse un widget scripté` (`VNL-APP-DSH-012`).
    #[test]
    fn un_tableau_ephemere_refuse_un_widget_scripte() {
        let def = def_with(vec![scripted(json!({"script": "1", "ttl_secs": 0}))]);
        validate_definition(&def).expect("la définition standard passe");
        let err = validate_ephemeral(&def).expect_err("un éphémère refuse le scripté");
        assert_eq!(err.0, "VNL-APP-DSH-012");
    }

    /// Règle 6 (`./mod.sdd`) : la grille est fixe à 12 colonnes et `w ≤ 12`.
    #[test]
    fn la_grille_est_fixe_a_douze_colonnes() {
        let widgets = vec![widget("w-a", "chat", json!({}), 0, 6)];
        for columns in [4, 11, 24] {
            let err = validate_definition(&def_grille(columns, widgets.clone()))
                .expect_err("une grille autre que 12 colonnes est refusée");
            assert_eq!(err.0, "VNL-APP-DSH-004", "colonnes {columns}");
        }
        validate_definition(&def_grille(
            12,
            vec![widget("w-b", "chat", json!({}), 0, 12)],
        ))
        .expect("un widget pleine largeur (w=12) passe");
        let err = validate_definition(&def_grille(
            12,
            vec![widget("w-c", "chat", json!({}), 0, 13)],
        ))
        .expect_err("w=13 déborde la grille");
        assert_eq!(err.0, "VNL-APP-DSH-004");
    }

    /// Scenario: `une définition standard passe` — le défaut accueil, même appel que le test
    /// de `./defaults.sdd` (source unique de la vérification).
    #[test]
    fn une_definition_standard_passe() {
        let def = defaults::default_for(model::Scope::Home).expect("le défaut accueil existe");
        validate_definition(def).expect("le défaut accueil passe les bornes");
    }
}
