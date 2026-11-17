//! Validation de la sortie d'un script de widget — contrat : `./output.sdd`
//! (`dsd/app/widgets.md` `FR-4`, `DEC-1`). Une map `{type, config}` dont le type est un
//! graphique scriptable du registre et dont la `config` respecte la grammaire fermée du type.

use serde_json::{Map, Value};

use super::model::WidgetOutput;
use crate::dashboards::registry;

/// Refus de validation : message fixe, jamais le contenu de la valeur (`output.sdd`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputError {
    /// Message fixe décrivant la règle violée.
    pub message: &'static str,
}

const NOT_AN_OBJECT: &str = "la sortie doit être un objet à exactement les clés `type` et `config`";
const UNKNOWN_TYPE: &str = "type de widget inconnu du registre";
const NOT_SCRIPTABLE: &str = "type de widget non scriptable";
const NO_GRAMMAR: &str = "aucune grammaire déclarée pour ce type scriptable";
const BAD_CONFIG: &str = "config hors grammaire fermée du type";

/// Valide une sortie de script : `Ok` avec la sortie inchangée, ou un refus fixe (`FR-4`).
pub fn validate_output(value: Value) -> Result<WidgetOutput, OutputError> {
    let Value::Object(mut map) = value else {
        return Err(refus(NOT_AN_OBJECT));
    };
    if map.len() != 2 || !map.contains_key("type") || !map.contains_key("config") {
        return Err(refus(NOT_AN_OBJECT));
    }
    let (Some(Value::String(name)), Some(config)) = (map.remove("type"), map.remove("config"))
    else {
        return Err(refus(NOT_AN_OBJECT));
    };
    let entry = registry::known(&name).ok_or(refus(UNKNOWN_TYPE))?;
    if !entry.scriptable {
        return Err(refus(NOT_SCRIPTABLE));
    }
    match name.as_str() {
        "bar" | "pie" => check_data_chart(&config)?,
        _ => return Err(refus(NO_GRAMMAR)),
    }
    Ok(WidgetOutput {
        widget_type: name,
        config,
    })
}

fn refus(message: &'static str) -> OutputError {
    OutputError { message }
}

/// Grammaire fermée de `bar` et `pie` : `{title?: chaîne, data: [{label: chaîne, value: nombre}]}`.
fn check_data_chart(config: &Value) -> Result<(), OutputError> {
    let Value::Object(map) = config else {
        return Err(refus(BAD_CONFIG));
    };
    if map.get("title").is_some_and(|title| !title.is_string()) {
        return Err(refus(BAD_CONFIG));
    }
    if map.keys().any(|key| key != "title" && key != "data") {
        return Err(refus(BAD_CONFIG));
    }
    let Some(Value::Array(points)) = map.get("data") else {
        return Err(refus(BAD_CONFIG));
    };
    if points.iter().all(is_point) {
        Ok(())
    } else {
        Err(refus(BAD_CONFIG))
    }
}

fn is_point(point: &Value) -> bool {
    match point {
        Value::Object(fields) => point_fields_ok(fields),
        _ => false,
    }
}

fn point_fields_ok(fields: &Map<String, Value>) -> bool {
    fields.len() == 2
        && fields.get("label").is_some_and(Value::is_string)
        && fields
            .get("value")
            .is_some_and(|value| value.as_f64().is_some_and(f64::is_finite))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Scenario: un diagramme en barres conforme passe — la sortie est identique à l'entrée.
    #[test]
    fn un_diagramme_en_barres_conforme_passe() {
        let valeur = json!({"type": "bar", "config": {"title": "t", "data": [{"label": "ok", "value": 0.93}]}});

        let sortie = validate_output(valeur.clone()).expect("la sortie conforme passe");

        assert_eq!(sortie.widget_type, "bar");
        assert_eq!(sortie.config, valeur["config"]);
    }

    /// Scenario: les sorties hors contrat sont refusées, chacune au message fixe.
    #[test]
    fn les_sorties_hors_contrat_sont_refusees() {
        let refusees = [
            json!("une chaîne"),
            json!({"type": "markdown", "config": {"text": "x"}}),
            json!({"type": "mermaid", "config": {"source": "graph TD"}}),
            json!({"type": "scripted", "config": {"script": "1"}}),
            json!({"type": "bar", "config": {"data": 3}}),
            json!({"type": "pie", "config": {"data": [{"label": "a", "value": "b"}]}}),
            json!({"type": "inconnu", "config": {}}),
            json!({"type": "bar", "config": {"data": []}, "extra": 1}),
        ];
        for valeur in refusees {
            let err = validate_output(valeur.clone())
                .expect_err(&format!("doit être refusée : {valeur}"));
            assert!(!err.message.is_empty());
        }
    }

    /// `Must` : seules les clés `type` et `config` sont admises au niveau supérieur, et
    /// `config` est une grammaire fermée (clé inconnue refusée).
    #[test]
    fn une_clé_inconnue_refusee_au_niveau_superieur_ou_dans_config() {
        assert!(validate_output(json!({"config": {"data": []}})).is_err());
        assert!(validate_output(json!({"type": "bar"})).is_err());
        assert!(
            validate_output(json!({"type": "bar", "config": {"data": [], "couleur": "red"}}))
                .is_err()
        );
    }
}
