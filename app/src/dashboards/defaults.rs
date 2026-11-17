//! Tableaux par défaut livrés avec l'application (`FR-2`, `DEC-2`) — contrat :
//! `./defaults.sdd`. Versions CODÉES de l'app, jamais en base : la base ne porte que
//! personnalisations et tableaux complémentaires. Consommateur unique : `./resolve.sdd`.

use serde_json::json;
use std::sync::OnceLock;

use super::model::{DashboardDef, Layout, Scope};

/// Définition par défaut de l'écran : `Some` pour les trois scopes livrés (tableau Structure
/// de `./defaults.sdd`) ; `None` est la voie d'erreur « écran sans défaut »
/// (`VNL-APP-DSH-008` chez le consommateur) et n'arrive qu'avec un futur scope sans défaut.
/// Le `id` rendu est `None` (un défaut n'est pas une ligne) ; l'origine est annotée par
/// `./resolve.sdd`, pas ici.
#[must_use]
#[allow(clippy::unnecessary_wraps)] // `None` = voie « écran sans défaut » du contrat — defaults.sdd
pub fn default_for(scope: Scope) -> Option<&'static DashboardDef> {
    match scope {
        Scope::Home => Some(home()),
        Scope::Project => Some(project()),
        Scope::Sandbox => Some(sandbox()),
    }
}

/// Construction contrôlée d'un défaut à partir de sa forme JSON : une construction invalide
/// (erreur de programmation, jamais une donnée utilisateur) est JOURNALISÉE et retombe sur
/// une définition vide — la détection de l'invalidité est faite par TEST (`Scenario` « chaque
/// défaut respecte les bornes d'écriture »), jamais par un `expect` en production.
fn construire(value: serde_json::Value) -> DashboardDef {
    serde_json::from_value(value).unwrap_or_else(|erreur| {
        tracing::error!("VNL-APP-DSH-008: définition par défaut invalide: {erreur}");
        DashboardDef {
            id: None,
            title: String::new(),
            layout: Layout::default(),
            widgets: Vec::new(),
        }
    })
}

/// Défaut accueil — une seule construction paresseuse (`OnceLock`), tableau Structure de
/// `./defaults.sdd` : rangée 0 deux colonnes larges, `runs` pleine largeur dessous, `chat`
/// dernier.
fn home() -> &'static DashboardDef {
    static DEF: OnceLock<DashboardDef> = OnceLock::new();
    DEF.get_or_init(|| {
        construire(json!({
            "title": "Accueil",
            "layout": { "columns": 12 },
            "widgets": [
                { "id": "w-projects",  "type": "projects",  "config": {}, "x": 0, "y": 0, "w": 6,  "h": 8 },
                { "id": "w-sandboxes", "type": "sandboxes", "config": {}, "x": 6, "y": 0, "w": 6,  "h": 8 },
                { "id": "w-runs",      "type": "runs",      "config": {}, "x": 0, "y": 8, "w": 12, "h": 6 },
                { "id": "w-chat",      "type": "chat",      "config": {}, "x": 0, "y": 14, "w": 12, "h": 10 }
            ]
        }))
    })
}

fn project() -> &'static DashboardDef {
    static DEF: OnceLock<DashboardDef> = OnceLock::new();
    DEF.get_or_init(|| {
        construire(json!({
            "title": "Projet",
            "layout": { "columns": 12 },
            "widgets": [
                { "id": "w-sandboxes",  "type": "sandboxes",  "config": {}, "x": 0, "y": 0, "w": 6,  "h": 8 },
                { "id": "w-workflows",  "type": "workflows",  "config": {}, "x": 6, "y": 0, "w": 6,  "h": 8 },
                { "id": "w-runs",       "type": "runs",       "config": {}, "x": 0, "y": 8, "w": 6,  "h": 6 },
                { "id": "w-dashboards", "type": "dashboards", "config": {}, "x": 6, "y": 8, "w": 6,  "h": 6 },
                { "id": "w-chat",       "type": "chat",       "config": {}, "x": 0, "y": 14, "w": 12, "h": 10 }
            ]
        }))
    })
}

fn sandbox() -> &'static DashboardDef {
    static DEF: OnceLock<DashboardDef> = OnceLock::new();
    DEF.get_or_init(|| {
        construire(json!({
            "title": "Sandbox",
            "layout": { "columns": 12 },
            "widgets": [
                { "id": "w-code-health", "type": "code-health", "config": {}, "x": 0, "y": 0, "w": 6,  "h": 8 },
                { "id": "w-workflows",   "type": "workflows",   "config": {}, "x": 6, "y": 0, "w": 6,  "h": 8 },
                { "id": "w-runs",        "type": "runs",        "config": {}, "x": 0, "y": 8, "w": 12, "h": 6 },
                { "id": "w-chat",        "type": "chat",        "config": {}, "x": 0, "y": 14, "w": 12, "h": 10 }
            ]
        }))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboards::{registry, validate_definition};

    /// Scenario: `les trois écrans ont un défaut`.
    #[test]
    fn les_trois_ecrans_ont_un_defaut() {
        let expected: [(Scope, &[&str]); 3] = [
            (Scope::Home, &["projects", "sandboxes", "runs", "chat"]),
            (
                Scope::Project,
                &["sandboxes", "workflows", "runs", "dashboards", "chat"],
            ),
            (
                Scope::Sandbox,
                &["code-health", "workflows", "runs", "chat"],
            ),
        ];
        for (scope, widgets) in expected {
            let def = default_for(scope).unwrap_or_else(|| panic!("{scope} doit avoir un défaut"));
            assert_eq!(def.id, None, "un défaut n'est pas une ligne");
            assert_eq!(def.layout.columns, 12, "{scope} doit livrer la grille 12");
            let names: Vec<&str> = def.widgets.iter().map(|w| w.widget_type.as_str()).collect();
            assert_eq!(
                names,
                widgets.to_vec(),
                "widgets de {scope} (noms et ordre du tableau Structure)"
            );
            let ids: Vec<&str> = def.widgets.iter().map(|w| w.id.as_str()).collect();
            let expected_ids: Vec<String> = widgets.iter().map(|w| format!("w-{w}")).collect();
            assert_eq!(
                ids,
                expected_ids.iter().map(String::as_str).collect::<Vec<_>>(),
                "ids stables `w-<nom>` pour {scope}"
            );
        }
    }

    /// Scenario: `aucun couple de widgets d'un même défaut ne se chevauche` (`./defaults.sdd`).
    #[test]
    fn aucun_chevauchement_dans_un_defaut() {
        for scope in [Scope::Home, Scope::Project, Scope::Sandbox] {
            let def = default_for(scope).expect("un défaut livré existe");
            for (i, a) in def.widgets.iter().enumerate() {
                for b in def.widgets.iter().skip(i + 1) {
                    let disjoint = a.x + a.w <= b.x
                        || b.x + b.w <= a.x
                        || a.y + a.h <= b.y
                        || b.y + b.h <= a.y;
                    assert!(disjoint, "{scope} : {} chevauche {}", a.id, b.id);
                }
            }
        }
    }

    /// Scenario: `chaque défaut respecte les bornes d'écriture`.
    #[test]
    fn chaque_defaut_respecte_les_bornes_ecriture() {
        for scope in [Scope::Home, Scope::Project, Scope::Sandbox] {
            let def = default_for(scope).expect("un défaut livré existe");
            validate_definition(def).unwrap_or_else(|(code, _)| {
                panic!("le défaut {scope} doit passer les bornes FR-3, obtenu {code}")
            });
            for widget in &def.widgets {
                assert!(
                    registry::known(&widget.widget_type).is_some(),
                    "le défaut {scope} cite le type inconnu {}",
                    widget.widget_type
                );
            }
        }
    }
}
