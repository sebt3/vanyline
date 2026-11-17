//! Types de la requête et de la réponse d'exécution d'un widget scripté — contrat :
//! `./model.sdd` (`dsd/app/widgets.md` `FR-2`, `FR-4`, `FR-7`, `FR-10`). Données pures
//! uniquement : aucune exécution, aucune validation, aucun accès base (`Must` de `./model.sdd`).

use serde::{Deserialize, Serialize};

/// Requête d'exécution d'un widget (`FR-2`, `DEC-6`) — le script n'y figure jamais : il est
/// relu dans la définition du tableau. Un champ inconnu est ignoré.
#[derive(Debug, Clone, Deserialize)]
pub struct RunRequest {
    /// Identifiant de ligne du tableau (celui que la résolution rend en `id`).
    pub dashboard: i32,
    /// Identifiant du widget dans ce tableau.
    pub widget: String,
    /// Relance malgré un résultat encore valide (`FR-6`, « actualiser »).
    #[serde(default)]
    pub refresh: bool,
    /// Nom de la sandbox affichée — exigé pour un tableau d'écran `sandbox`.
    #[serde(default)]
    pub sandbox: Option<String>,
}

/// Déclaration d'un widget graphique rendue par un script, déjà validée (`FR-4`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct WidgetOutput {
    /// Type du widget (`bar`, `pie`) ; clé JSON `type`.
    #[serde(rename = "type")]
    pub widget_type: String,
    /// Données et configuration d'affichage du graphique (grammaire de `./output.sdd`).
    /// `any` côté TypeScript, comme `dashboards` : aucun `JsonValue` à importer.
    #[cfg_attr(feature = "ts-rs", ts(type = "any"))]
    pub config: serde_json::Value,
}

/// Erreur structurée d'un widget (`FR-7`) — message borné, sans source ni jeton.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct RunError {
    /// Identifiant `VNL-APP-WDG-NNN`.
    pub code: String,
    /// Message borné et sans contenu interne.
    pub message: String,
}

/// Réponse d'exécution : `status` vaut `"ready"` ou `"failed"` (`FR-10`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum RunResponse {
    /// Résultat prêt : sortie, fraîcheur et durée du dernier calcul.
    Ready {
        output: WidgetOutput,
        /// Instant du calcul, RFC 3339.
        computed_at: String,
        duration_ms: u64,
        cached: bool,
    },
    /// Échec de script (`FR-7`).
    Failed { error: RunError },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Scenario: la réponse prête porte la sortie et sa fraîcheur.
    #[test]
    fn la_reponse_prete_porte_la_sortie_et_sa_fraicheur() {
        let reponse = RunResponse::Ready {
            output: WidgetOutput {
                widget_type: "bar".to_string(),
                config: json!({"data": [{"label": "ok", "value": 0.93}]}),
            },
            computed_at: "2026-11-17T10:00:00Z".to_string(),
            duration_ms: 120,
            cached: false,
        };

        let v = serde_json::to_value(&reponse).expect("la réponse est sérialisable");

        assert_eq!(v["status"], json!("ready"));
        assert_eq!(v["output"]["type"], json!("bar"));
        assert_eq!(
            v["output"]["config"],
            json!({"data": [{"label": "ok", "value": 0.93}]})
        );
        assert_eq!(v["computed_at"], json!("2026-11-17T10:00:00Z"));
        assert_eq!(v["duration_ms"], json!(120));
        assert_eq!(v["cached"], json!(false));
        assert!(
            v["output"].get("widget_type").is_none(),
            "la clé du type de widget est `type`, jamais `widget_type`"
        );
    }

    /// Scenario: la réponse en échec porte un code et un message sans secret.
    #[test]
    fn la_reponse_en_echec_porte_un_code_et_un_message() {
        let reponse = RunResponse::Failed {
            error: RunError {
                code: "VNL-APP-WDG-004".to_string(),
                message: "plafond de durée atteint".to_string(),
            },
        };

        let v = serde_json::to_value(&reponse).expect("la réponse est sérialisable");

        assert_eq!(v["status"], json!("failed"));
        assert_eq!(v["error"]["code"], json!("VNL-APP-WDG-004"));
        assert_eq!(v["error"]["message"], json!("plafond de durée atteint"));
    }

    /// `FR-2`, `DEC-6` : le script n'est jamais un champ de la requête. Un champ inconnu,
    /// dont `script`, est ignoré ; `refresh` et `sandbox` ont leurs défauts.
    #[test]
    fn la_requete_ignore_un_script_fourni_par_le_client() {
        let requete: RunRequest = serde_json::from_value(json!({
            "dashboard": 7,
            "widget": "tests",
            "script": "loop {}"
        }))
        .expect("la requête minimale se désérialise");

        assert_eq!(requete.dashboard, 7);
        assert_eq!(requete.widget, "tests");
        assert!(!requete.refresh);
        assert_eq!(requete.sandbox, None);
    }

    /// Scenario: export ts-rs des formes publiées (feature `ts-rs`). Régénère
    /// `packages/protocol/src/generated/widget-domain.ts` : un seul fichier agrégé, sans
    /// `import type`, déclarant les trois types publiés.
    #[cfg(feature = "ts-rs")]
    #[test]
    fn export_widget_types_ts() {
        use ts_rs::{Config, TS};
        let cfg = Config::from_env();
        // Les trois types sont agrégés dans CE fichier : les `import type` ciblant des
        // fichiers frères sont retirés (même motif que `dashboards/model.rs`).
        let aggregate = |decl: String| -> String {
            let mut kept = decl
                .lines()
                .filter(|line| !line.starts_with("import type"))
                .collect::<Vec<_>>()
                .join("\n");
            kept.push('\n');
            kept
        };
        let output = aggregate(
            <WidgetOutput as TS>::export_to_string(&cfg).expect("WidgetOutput exportable"),
        );
        let error =
            aggregate(<RunError as TS>::export_to_string(&cfg).expect("RunError exportable"));
        let response =
            aggregate(<RunResponse as TS>::export_to_string(&cfg).expect("RunResponse exportable"));
        let out = format!(
            "// Generated by vanyline-app (ts-rs) from app/src/widgets/model.rs.\n\
             // DO NOT EDIT — regenerate: cargo test -p vanyline-app --features ts-rs\n\n\
             {output}\n{error}\n{response}\n"
        );
        let target = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../packages/protocol/src/generated/widget-domain.ts"
        );
        std::fs::write(target, out).expect("l'export s'écrit");

        let content = std::fs::read_to_string(target).expect("le fichier généré est relisible");
        for name in ["RunResponse", "RunError", "WidgetOutput"] {
            assert!(
                content.contains(&format!("export type {name} ")),
                "{name} doit être déclaré en export type : {content}"
            );
        }
        assert!(
            content.contains("type: string") && !content.contains("widget_type"),
            "WidgetOutput doit porter `type` comme clé du type de widget : {content}"
        );
        assert!(
            !content.contains("import type"),
            "le fichier agrégé ne doit porter aucun import : {content}"
        );
    }
}
