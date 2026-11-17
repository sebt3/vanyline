//! Registre des types de widgets (`FR-4`, `DEC-1`) — contrat : `./registry.sdd`.
//! Table statique des douze types livrés (`FR-6`) : nom, schéma JSON déclaratif de `config`,
//! besoins de contexte, source de données et indicateur `scriptable`. Le registre ne rend rien et
//! ne valide pas la `config` contre son schéma : seule la forme objet est exigée à l'écriture
//! (`../db/entities/dashboards.rs`).

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

/// Source de données du widget (`FR-10` en est la documentation, pas un champ).
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
    /// Données portées par `config` (markdown, mermaid, bar, pie).
    Inline,
    /// Donnée produite par un script de la définition (`../widgets/run.sdd`).
    Scripted,
}

/// Entrée du registre : pour un type de widget, son nom verbatim, le schéma JSON déclaratif
/// de sa `config` (pour le frontend — l'écriture n'exige qu'un objet), ses besoins, sa source et
/// son aptitude à être produit par un script.
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
    /// Vrai pour un widget graphique qu'un script peut produire : `bar` et `pie`, et eux seuls
    /// (`../widgets/output.sdd`, `dsd/app/widgets.md#DEC-1`).
    pub scriptable: bool,
}

/// Schémas `config` des douze types, adressés de façon stable par la table statique (une
/// seule initialisation paresseuse, `OnceLock`).
struct Schemas {
    bar: Value,
    chat: Value,
    code_health: Value,
    dashboards: Value,
    markdown: Value,
    mermaid: Value,
    pie: Value,
    projects: Value,
    runs: Value,
    sandboxes: Value,
    scripted: Value,
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
        mermaid: json!({
            "type": "object",
            "properties": { "title": { "type": "string" }, "source": { "type": "string" } },
            "required": ["source"]
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
        scripted: json!({
            "type": "object",
            "properties": {
                "script": { "type": "string" },
                "ttl_secs": { "type": "integer", "minimum": 0 }
            },
            "required": ["script"]
        }),
        workflows: json!({}),
    })
}

/// Le registre complet (`FR-6` — douze types verbatims), ordre alphabétique de `name`.
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
    // Entrée standard : aucun besoin, non scriptable.
    let simple = |name, config_schema, needs, source| WidgetType {
        name,
        config_schema,
        needs,
        source,
        scriptable: false,
    };
    ALL.get_or_init(|| {
        vec![
            WidgetType {
                scriptable: true,
                ..simple("bar", &s.bar, needs_none, DataSource::Inline)
            },
            simple("chat", &s.chat, needs_none, DataSource::Chat),
            simple(
                "code-health",
                &s.code_health,
                needs_sandbox,
                DataSource::CodeHealth,
            ),
            simple(
                "dashboards",
                &s.dashboards,
                needs_project,
                DataSource::Dashboards,
            ),
            simple("markdown", &s.markdown, needs_none, DataSource::Inline),
            simple("mermaid", &s.mermaid, needs_none, DataSource::Inline),
            WidgetType {
                scriptable: true,
                ..simple("pie", &s.pie, needs_none, DataSource::Inline)
            },
            simple("projects", &s.projects, needs_none, DataSource::Projects),
            simple("runs", &s.runs, needs_none, DataSource::Runs),
            simple(
                "sandboxes",
                &s.sandboxes,
                needs_project,
                DataSource::Sandboxes,
            ),
            simple("scripted", &s.scripted, needs_none, DataSource::Scripted),
            simple(
                "workflows",
                &s.workflows,
                needs_project,
                DataSource::Workflows,
            ),
        ]
    })
}

/// Recherche par nom exact (sensible à la casse) — `None` ne distingue pas « futur type »
/// de « faute de frappe » (`AC-4`).
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

    /// Scenario: `les douze types standard sont connus` (`FR-6`, `scripted` compris).
    #[test]
    fn les_douze_types_standard_sont_connus() {
        let expected: &[(&str, bool, bool, DataSource)] = &[
            ("bar", false, false, DataSource::Inline),
            ("chat", false, false, DataSource::Chat),
            ("code-health", false, true, DataSource::CodeHealth),
            ("dashboards", true, false, DataSource::Dashboards),
            ("markdown", false, false, DataSource::Inline),
            ("mermaid", false, false, DataSource::Inline),
            ("pie", false, false, DataSource::Inline),
            ("projects", false, false, DataSource::Projects),
            ("runs", false, false, DataSource::Runs),
            ("sandboxes", true, false, DataSource::Sandboxes),
            ("scripted", false, false, DataSource::Scripted),
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
        assert_eq!(table.len(), 12, "exactement douze entrées");
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

    /// Scenario: `seuls bar et pie sont scriptables`.
    #[test]
    fn seuls_bar_et_pie_sont_scriptables() {
        for widget in all() {
            assert_eq!(
                widget.scriptable,
                matches!(widget.name, "bar" | "pie"),
                "scriptable de {}",
                widget.name
            );
        }
    }

    /// Scenario: `un nom inconnu rend None sans distinction`.
    #[test]
    fn un_nom_inconnu_rend_none_sans_distinction() {
        for name in ["Markdown", "", "widget/chat", "MARKDOWN", "futur-type"] {
            assert!(known(name).is_none(), "{name:?} ne doit rien rendre");
        }
    }
}
