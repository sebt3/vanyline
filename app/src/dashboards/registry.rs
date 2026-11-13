//! Registre des types de widgets (`FR-4`, `DEC-1`) — contrat : `./registry.sdd`.
//! Table statique des dix types livrés (`FR-6`) : nom, schéma JSON déclaratif de `config`,
//! besoins de contexte et source de données. Le registre ne rend rien et ne valide pas la
//! `config` contre son schéma : seule la forme objet est exigée à l'écriture
//! (`../db/entities/dashboards.sdd`).

use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;

/// Contexte qu'un widget exige pour être monté (`AC-7`) : le moteur frontend ne fournit
/// jamais plus, jamais moins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// Le widget exige le contexte projet.
    pub project: bool,
    /// Le widget exige le contexte sandbox.
    pub sandbox: bool,
}

/// Source de données du widget (`FR-10` en est la documentation, pas un champ). `Scripted`
/// n'existe PAS ici : `app/widgets` n'est pas ouverte (`FR-6`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    /// Liste des projets de l'appelant.
    Projects,
    /// Liste des sandboxes (du projet courant).
    Sandboxes,
    /// Liste des workflows (du projet courant).
    Workflows,
    /// Runs de workflows (rythme de sondage à la main du frontend).
    Runs,
    /// Santé du code de la sandbox courante.
    CodeHealth,
    /// Chat de l'écran courant.
    Chat,
    /// Tableaux complémentaires du projet (widget `dashboards`, `FR-5`).
    Dashboards,
    /// Données portées par `config` (markdown, bar, pie).
    Inline,
}

/// Entrée du registre : pour un type de widget, son nom verbatim, le schéma JSON déclaratif
/// de sa `config` (pour le frontend — l'écriture n'exige qu'un objet), ses besoins et sa
/// source.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetType {
    /// Nom verbatim (clé du registre, sensible à la casse).
    pub name: &'static str,
    /// Schéma JSON déclaratif de `config` (les clés inconnues sont TOÉRÉES à l'écriture).
    pub config_schema: &'static Value,
    /// Contexte exigé pour le montage.
    pub needs: Needs,
    /// Source de données du widget.
    pub source: DataSource,
}

/// Schémas `config` des dix types, adressés de façon stable par la table statique (une
/// seule initialisation paresseuse, `OnceLock`).
struct Schemas {
    bar: Value,
    chat: Value,
    code_health: Value,
    dashboards: Value,
    markdown: Value,
    pie: Value,
    projects: Value,
    runs: Value,
    sandboxes: Value,
    workflows: Value,
}

fn schemas() -> &'static Schemas {
    static SCHEMAS: OnceLock<Schemas> = OnceLock::new();
    SCHEMAS.get_or_init(|| Schemas {
        bar: json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "data": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "label": { "type": "string" }, "value": {} }
                    }
                }
            },
            "required": ["data"]
        }),
        chat: json!({}),
        code_health: json!({}),
        dashboards: json!({}),
        markdown: json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        }),
        pie: json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "data": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "label": { "type": "string" }, "value": {} }
                    }
                }
            },
            "required": ["data"]
        }),
        projects: json!({}),
        runs: json!({
            "type": "object",
            "properties": {
                "days": { "type": "integer" },
                "max": { "type": "integer" }
            }
        }),
        sandboxes: json!({}),
        workflows: json!({}),
    })
}

/// Le registre complet (`FR-6` — dix types verbatims), ordre alphabétique de `name`.
#[must_use]
pub fn all() -> &'static [WidgetType] {
    static ALL: OnceLock<Vec<WidgetType>> = OnceLock::new();
    let s = schemas();
    let needs_none = Needs {
        project: false,
        sandbox: false,
    };
    let needs_project = Needs {
        project: true,
        sandbox: false,
    };
    let needs_sandbox = Needs {
        project: false,
        sandbox: true,
    };
    ALL.get_or_init(|| {
        vec![
            WidgetType {
                name: "bar",
                config_schema: &s.bar,
                needs: needs_none,
                source: DataSource::Inline,
            },
            WidgetType {
                name: "chat",
                config_schema: &s.chat,
                needs: needs_none,
                source: DataSource::Chat,
            },
            WidgetType {
                name: "code-health",
                config_schema: &s.code_health,
                needs: needs_sandbox,
                source: DataSource::CodeHealth,
            },
            WidgetType {
                name: "dashboards",
                config_schema: &s.dashboards,
                needs: needs_project,
                source: DataSource::Dashboards,
            },
            WidgetType {
                name: "markdown",
                config_schema: &s.markdown,
                needs: needs_none,
                source: DataSource::Inline,
            },
            WidgetType {
                name: "pie",
                config_schema: &s.pie,
                needs: needs_none,
                source: DataSource::Inline,
            },
            WidgetType {
                name: "projects",
                config_schema: &s.projects,
                needs: needs_none,
                source: DataSource::Projects,
            },
            WidgetType {
                name: "runs",
                config_schema: &s.runs,
                needs: needs_none,
                source: DataSource::Runs,
            },
            WidgetType {
                name: "sandboxes",
                config_schema: &s.sandboxes,
                needs: needs_project,
                source: DataSource::Sandboxes,
            },
            WidgetType {
                name: "workflows",
                config_schema: &s.workflows,
                needs: needs_project,
                source: DataSource::Workflows,
            },
        ]
    })
}

/// Recherche par nom exact (sensible à la casse) — `None` ne distingue pas « futur type »
/// de « faute de frappe » (`AC-4`) ; `known("scripted")` rend `None` (`FR-6`).
#[must_use]
pub fn known(name: &str) -> Option<&'static WidgetType> {
    all().iter().find(|widget| widget.name == name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: `les dix types standard sont connus`.
    #[test]
    fn les_dix_types_standard_sont_connus() {
        let expected: &[(&str, bool, bool, DataSource)] = &[
            ("bar", false, false, DataSource::Inline),
            ("chat", false, false, DataSource::Chat),
            ("code-health", false, true, DataSource::CodeHealth),
            ("dashboards", true, false, DataSource::Dashboards),
            ("markdown", false, false, DataSource::Inline),
            ("pie", false, false, DataSource::Inline),
            ("projects", false, false, DataSource::Projects),
            ("runs", false, false, DataSource::Runs),
            ("sandboxes", true, false, DataSource::Sandboxes),
            ("workflows", true, false, DataSource::Workflows),
        ];
        for (name, project, sandbox, source) in expected {
            let widget = known(name).unwrap_or_else(|| panic!("{name} doit être connu"));
            assert_eq!(
                widget.needs,
                Needs {
                    project: *project,
                    sandbox: *sandbox
                },
                "besoins de {name}"
            );
            assert_eq!(widget.source, *source, "source de {name}");
        }

        let table = all();
        assert_eq!(table.len(), 10, "exactement dix entrées");
        let mut names: Vec<&str> = table.iter().map(|w| w.name).collect();
        assert!(
            names.windows(2).all(|pair| pair[0] < pair[1]),
            "all() doit être trié alphabétiquement : {names:?}"
        );
        names.sort_unstable();
        assert_eq!(
            names,
            expected.iter().map(|(name, ..)| *name).collect::<Vec<_>>()
        );
    }

    /// Scenario: `scripted est réservé mais absent`.
    #[test]
    fn scripted_est_reserve_mais_absent() {
        assert!(known("scripted").is_none());
    }

    /// Scenario: `un nom inconnu rend None sans distinction`.
    #[test]
    fn un_nom_inconnu_rend_none_sans_distinction() {
        for name in ["Markdown", "", "widget/chat", "scripted", "MARKDOWN"] {
            assert!(known(name).is_none(), "{name:?} ne doit rien rendre");
        }
    }
}
