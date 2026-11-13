//! Agrégation du module dashboards — contrat : `./mod.sdd`. PORTEUR de la fonction de
//! validation FR-3 partagée entre l'écriture (hook de `../db/entities/dashboards.rs`) et la
//! lecture (`./resolve.sdd`). Sans état : aucune fonction ne capture de connection, de
//! config ou de registre mutable (`DEC-1`).

pub mod defaults;
pub mod model;
pub mod registry;
pub mod resolve;

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
    // Règle 6 — coordonnées dans la grille. `colonnes` hors `[4, 24]` est un manquement 004
    // avant même de regarder les widgets.
    let columns = def.layout.columns;
    if !(4..=24).contains(&columns) {
        return Err((
            "VNL-APP-DSH-004",
            "grille hors bornes [4, 24] colonnes (VNL-APP-DSH-004)",
        ));
    }
    for widget in &def.widgets {
        let within_row = widget.w >= 1
            && widget.w <= 64
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

    /// Scenario: `une définition standard passe` — le défaut accueil, même appel que le test
    /// de `./defaults.sdd` (source unique de la vérification).
    #[test]
    fn une_definition_standard_passe() {
        let def = defaults::default_for(model::Scope::Home).expect("le défaut accueil existe");
        validate_definition(def).expect("le défaut accueil passe les bornes");
    }
}
