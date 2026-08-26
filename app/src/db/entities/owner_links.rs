use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Lien utilisateur miryad-core → Owner CRD K8s (`k8s_owner_name`).
/// Table séparée côté `app` : le schéma `miryad_users` est fixe (aucun
/// mécanisme d'extension), le lien vit ici, FK vers `miryad_users.id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "vanyline_owner_links")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub user_id: i32,
    pub k8s_owner_name: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
