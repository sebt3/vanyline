use miryad_core::resource::{AccessPolicy, MiryadResource};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema, DeriveEntityModel)]
#[schema(as = Skill)]
#[sea_orm(table_name = "vanyline_skills")]
pub struct Model {
    // Cf. commentaire équivalent dans `model_profiles.rs` : `#[serde(default)]` sur
    // `id`/`owner_id` évite un 400 sur les payloads de création du frontend, qui n'envoie
    // ni l'un ni l'autre (écrasés de toute façon par `resource_router`).
    #[sea_orm(primary_key)]
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub owner_id: i32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl MiryadResource for Entity {
    fn resource_name() -> &'static str {
        "skills"
    }
    fn read_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn write_policy() -> AccessPolicy {
        AccessPolicy::OwnerOnly
    }
    fn owner_column() -> Option<Column> {
        Some(Column::OwnerId)
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use miryad_core::resource::AccessPolicy;

    #[test]
    fn skill_resource_contract() {
        assert_eq!(Entity::resource_name(), "skills");
        assert_eq!(Entity::read_policy(), AccessPolicy::OwnerOnly);
        assert_eq!(Entity::write_policy(), AccessPolicy::OwnerOnly);
        // `Column` enum ne derive pas `PartialEq` (SeaORM 2) — on teste
        // la structure via `matches!` au lieu d'`assert_eq!`.
        let oc = Entity::owner_column();
        assert!(
            matches!(oc, Some(Column::OwnerId)),
            "owner_column should be Some(Column::OwnerId), got {oc:?}"
        );
    }

    #[test]
    fn skill_payload_minimal_deserialise_avec_defauts() {
        let model: Model =
            serde_json::from_str(r#"{"name": "pdf"}"#).expect("payload minimal deserialises");
        assert_eq!(model.name, "pdf");
        assert_eq!(model.id, 0);
        assert_eq!(model.owner_id, 0);
        assert_eq!(model.description, "");
        assert_eq!(model.body, "");
    }

    #[test]
    fn skill_payload_sans_name_rejete() {
        let err = serde_json::from_str::<Model>(r#"{"description": "x", "body": "y"}"#)
            .expect_err("payload without name must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("missing field") && msg.contains("name"),
            "expected `missing field` on `name`, got: {msg}"
        );
    }

    #[test]
    fn skill_round_trip_serialisation_fidele() {
        let original = Model {
            id: 12,
            owner_id: 34,
            name: "rust-review".to_string(),
            description: "Revue de code Rust : clippy, erreurs, perf".to_string(),
            body: "# Étapes\n- lire le diff\n- \"vérifier\" les unwrap".to_string(),
        };
        let json = serde_json::to_value(&original).expect("serialize skill");
        let obj = json.as_object().expect("skill serialises as an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["body", "description", "id", "name", "owner_id"]);

        let back: Model = serde_json::from_value(json).expect("deserialize skill");
        assert_eq!(back, original);
    }
}
