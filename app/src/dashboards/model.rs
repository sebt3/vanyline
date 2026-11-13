//! Types purement datés de la Solution tableaux de bord — contrat : `./model.sdd`.
//! Données pures uniquement : aucun accès DB, aucune résolution, aucune validation de
//! registre ici (`Must not` de `./model.sdd`). Source unique de la forme TypeScript
//! `dashboard-domain` (vague G5) et du contenu de la colonne JSON de la ressource
//! `../db/entities/dashboards.rs`.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Borne haute structurelle de la grille (`FR-1` : « grille de 12 colonnes ») — la
/// constante utilisée par le hook de validation et les défauts ; `Layout::columns` peut la
/// porter autrement (définition importée) mais aucun défaut livré ne le fait.
pub const DASHBOARD_GRID_COLUMNS: u16 = 12;

/// Écran porteur d'un tableau de bord (`FR-1`) — exactement trois, dans cet ordre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum Scope {
    /// Accueil de l'utilisateur.
    Home,
    /// Écran d'un projet (`project` non vide chez le consommateur).
    Project,
    /// Vue d'ensemble d'une sandbox (`sandbox` non vide chez le consommateur).
    Sandbox,
}

impl Scope {
    /// Forme `snake_case` verbatim, identique à `Serialize` : `"home"`, `"project"`, `"sandbox"`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Project => "project",
            Self::Sandbox => "sandbox",
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse total et sensible à la casse de la forme `snake_case` exacte (`Scenario` « scope et
/// kind en `snake_case` verbatim ») : tout le reste rend un `Err` sans contenu d'erreur (« pas
/// d'erreur » du contrat — l'appelant décide du marqueur).
impl FromStr for Scope {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "home" => Ok(Self::Home),
            "project" => Ok(Self::Project),
            "sandbox" => Ok(Self::Sandbox),
            _ => Err(()),
        }
    }
}

/// Origine d'une définition (`FR-1`, `FR-9`) — quatre variétés. Ne voyage JAMAIS dans le
/// JSON stocké : c'est une colonne de la ressource (`../db/entities/dashboards.sdd`) ou une
/// annotation de la réponse de résolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum Kind {
    /// Défaut livré avec l'app (jamais en base, `DEC-2`).
    Default,
    /// Personnalisation d'un écran par son utilisateur.
    Custom,
    /// Tableau complémentaire d'un projet (`FR-5`).
    Complementary,
    /// Définition éphémère sans ligne ni route (`FR-9`).
    Ephemeral,
}

/// Grille de la définition (`FR-1`) ; `Default` = la grille standard de 12 colonnes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Layout {
    /// Nombre de colonnes de la grille (donnée de la définition, bornée [4, 24] à
    /// l'écriture par le hook de `../db/entities/dashboards.rs`).
    pub columns: u16,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            columns: DASHBOARD_GRID_COLUMNS,
        }
    }
}

/// Widget d'un tableau : type connu du registre `./registry.rs`, config dont la grammaire
/// appartient au type (CE fichier ne la valide pas), placement dans la grille.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[allow(clippy::struct_field_names)] // nom verbatim porté par `./model.sdd` (clé JSON `type` par `rename`)
pub struct Widget {
    /// Identifiant stable dans la définition (les défauts livrent `"w-<nom>"`,
    /// `./defaults.sdd`).
    pub id: String,
    /// Nom du type (clé JSON littérale `type`, pas `widget_type`).
    #[serde(rename = "type")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "type"))]
    pub widget_type: String,
    /// Config objet dont la grammaire appartient au type déclaré (`./registry.sdd`).
    #[cfg_attr(feature = "ts-rs", ts(type = "any"))]
    pub config: serde_json::Value,
    /// Colonne de départ (0-based).
    pub x: u16,
    /// Rangée de départ (0-based).
    pub y: u16,
    /// Largeur en colonnes (≥ 1 à l'écriture).
    pub w: u16,
    /// Hauteur en rangées (bornée [1, 64] à l'écriture).
    pub h: u16,
}

/// Définition complète d'un tableau (`FR-1`) — forme de la réponse de résolution. Le JSON
/// STOCKÉ (colonne `def` de la ressource) ne porte que `{layout, widgets}` : `id` et `title`
/// sont tolérants à la désérialisation (`#[serde(default)]`), reconstitués depuis les
/// colonnes de la ligne par `./resolve.sdd`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct DashboardDef {
    /// Identifiant de ligne : `Some` pour une définition persistée rendue par
    /// `./resolve.sdd`, `None` pour un défaut livré et pour tout éphémère (`FR-9`).
    #[serde(default)]
    pub id: Option<i32>,
    /// Libellé (colonne `title` de la ressource fait foi côté persistance).
    #[serde(default)]
    pub title: String,
    /// Grille de la définition.
    pub layout: Layout,
    /// Widgets dans l'ordre de rendu.
    pub widgets: Vec<Widget>,
}

/// Mesure la taille sérialisée de la partie STOCKÉE `{layout, widgets}` et la compare à
/// `max_bytes` (borne 64 Kio du hook sans dupliquer `serde_json` ailleurs — `Exposes` de
/// `./model.sdd`). Une panne de sérialisation compte comme hors limite.
pub(crate) fn serializes_within_limit(def: &DashboardDef, max_bytes: usize) -> bool {
    let stored = serde_json::json!({ "layout": &def.layout, "widgets": &def.widgets });
    serde_json::to_vec(&stored).is_ok_and(|bytes| bytes.len() <= max_bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Scenario: `aller-retour serde stable de la forme FR-1`.
    #[test]
    fn aller_retour_serde_stable_de_la_forme_fr_1() {
        let def = DashboardDef {
            id: Some(7),
            title: "Mon tableau".to_string(),
            layout: Layout { columns: 12 },
            widgets: vec![
                Widget {
                    id: "w-note".to_string(),
                    widget_type: "markdown".to_string(),
                    config: json!({"text": "bonjour"}),
                    x: 0,
                    y: 0,
                    w: 6,
                    h: 4,
                },
                Widget {
                    id: "w-runs".to_string(),
                    widget_type: "runs".to_string(),
                    config: json!({}),
                    x: 6,
                    y: 0,
                    w: 6,
                    h: 4,
                },
            ],
        };

        let value = serde_json::to_value(&def).expect("la définition est sérialisable");
        assert_eq!(value["id"], json!(7));
        assert_eq!(value["title"], json!("Mon tableau"));
        assert_eq!(value["layout"]["columns"], json!(12));
        let round: DashboardDef = serde_json::from_value(value.clone()).expect("la forme se relit");
        assert_eq!(round, def, "l'aller-retour doit être identique");

        // And : les clés JSON littérales — la clé du type est `type`, pas `widget_type`.
        for (index, widget) in value["widgets"]
            .as_array()
            .expect("widgets est un tableau")
            .iter()
            .enumerate()
        {
            let object = widget.as_object().expect("un widget est un objet");
            for key in ["id", "type", "config", "x", "y", "w", "h"] {
                assert!(object.contains_key(key), "widgets[{index}] sans clé {key}");
            }
            assert!(
                !object.contains_key("widget_type"),
                "widgets[{index}] doit porter la clé `type`, pas `widget_type`"
            );
        }
    }

    /// Scenario: `scope et kind en snake_case verbatim`.
    #[test]
    fn scope_et_kind_en_snake_case_verbatim() {
        for (scope, expected) in [
            (Scope::Home, "home"),
            (Scope::Project, "project"),
            (Scope::Sandbox, "sandbox"),
        ] {
            assert_eq!(
                serde_json::to_value(scope).expect("scope sérialisable"),
                json!(expected)
            );
            assert_eq!(scope.to_string(), expected);
            assert_eq!(Scope::from_str(expected), Ok(scope));
        }
        for (kind, expected) in [
            (Kind::Default, "default"),
            (Kind::Custom, "custom"),
            (Kind::Complementary, "complementary"),
            (Kind::Ephemeral, "ephemeral"),
        ] {
            assert_eq!(
                serde_json::to_value(kind).expect("kind sérialisable"),
                json!(expected)
            );
        }

        // Parse sensible à la casse : seule la forme exacte passe.
        assert_eq!(Scope::from_str("global"), Err(()));
        assert_eq!(Scope::from_str("Home"), Err(()));
        assert_eq!(Scope::from_str(""), Err(()));
    }

    /// Scenario: `export ts-rs des formes publiées` (feature `ts-rs` uniquement — la
    /// génération effective et le job CI appartiennent à G5).
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_ts_rs_des_formes_publiees() {
        use ts_rs::{Config, TS};
        let cfg = Config::from_env();
        assert_eq!(<DashboardDef as TS>::name(&cfg), "DashboardDef");
        assert_eq!(<Widget as TS>::name(&cfg), "Widget");
        assert_eq!(<Layout as TS>::name(&cfg), "Layout");
        assert_eq!(<Scope as TS>::name(&cfg), "Scope");
        assert_eq!(<Kind as TS>::name(&cfg), "Kind");
        let decl = <Widget as TS>::decl(&cfg);
        assert!(
            decl.contains("type") && !decl.contains("widget_type:"),
            "la déclaration de Widget doit porter `type` comme clé du type de widget : {decl}"
        );
    }

    /// Scenario: `export ts-rs des formes publiées` (G5) — régénère
    /// `packages/protocol/src/generated/dashboard-domain.ts` depuis les cinq types publiés.
    /// Fichier commité ; le job CI `tsrs` régénère et vérifie `git diff --exit-code`
    /// (vanyline-app couvert depuis B8). Motif `lib/src/event.rs::export_chat_event_ts`.
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_dashboard_types_ts() {
        use ts_rs::{Config, TS};
        let cfg = Config::from_env();
        // Les cinq types sont agrégés dans CE fichier : pour un export par type, ts-rs
        // ferait pointer des `import type` vers des fichiers frères inexistants — caducs
        // en fichier unique (les déclarations sont ci-dessous), on les retire.
        let aggregate = |decl: String| -> String {
            let mut kept = decl
                .lines()
                .filter(|line| !line.starts_with("import type {"))
                .collect::<Vec<_>>()
                .join("\n");
            kept.push('\n');
            kept
        };
        let scope = aggregate(<Scope as TS>::export_to_string(&cfg).expect("Scope exportable"));
        let kind = aggregate(<Kind as TS>::export_to_string(&cfg).expect("Kind exportable"));
        let layout = aggregate(<Layout as TS>::export_to_string(&cfg).expect("Layout exportable"));
        let widget = aggregate(<Widget as TS>::export_to_string(&cfg).expect("Widget exportable"));
        let def = aggregate(
            <DashboardDef as TS>::export_to_string(&cfg).expect("DashboardDef exportable"),
        );
        let out = format!(
            "// Generated by vanyline-app (ts-rs) from app/src/dashboards/model.rs.\n\
             // DO NOT EDIT — regenerate: cargo test -p vanyline-app --features ts-rs\n\n\
             {scope}\n{kind}\n{layout}\n{widget}\n{def}\n"
        );
        let target = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packages/protocol/src/generated/dashboard-domain.ts"
        );
        std::fs::write(target, out).expect("l'export s'écrit");

        // Then : le fichier généré déclare les cinq types et la déclaration de `Widget`
        // porte `type` comme clé du type de widget (pas `widget_type`).
        let content = std::fs::read_to_string(target).expect("le fichier généré est relisible");
        for name in ["DashboardDef", "Widget", "Layout", "Scope", "Kind"] {
            assert!(
                content.contains(&format!("export type {name} ")),
                "{name} doit être déclaré en export type : {content}"
            );
        }
        assert!(
            content.contains("type: string") && !content.contains("widget_type:"),
            "la déclaration de Widget doit porter `type` comme clé du type de widget"
        );
        assert!(
            !content.contains("import type"),
            "le fichier agrégé ne doit porter aucun import (tout est dans CE fichier)"
        );
        // Le fichier exporté porte la forme fil `snake_case` verbatim (serde-compat).
        for variant in [
            "\"home\"",
            "\"project\"",
            "\"sandbox\"",
            "\"default\"",
            "\"custom\"",
            "\"complementary\"",
            "\"ephemeral\"",
        ] {
            assert!(
                content.contains(variant),
                "la variante {variant} doit être exportée en snake_case"
            );
        }
    }
}
