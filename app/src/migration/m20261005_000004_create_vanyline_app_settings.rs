use sea_orm_migration::prelude::*;

#[derive(DeriveIden)]
enum AppSetting {
    #[sea_orm(iden = "vanyline_app_settings")]
    Table,
    Id,
    Key,
    Value,
}

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AppSetting::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AppSetting::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AppSetting::Key)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    // `JSONB` en dur (même motif que m20260825_000002 pour les colonnes
                    // JSON) : rendu `JSONB` sur PostgreSQL, accepté tel quel par SQLite
                    // (typage dynamique) — DDL identique sur les deux moteurs, aucune FK
                    // (la validité de la valeur est contrôlée par le hook de l'entité).
                    .col(ColumnDef::new(AppSetting::Value).custom("JSONB").not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AppSetting::Table).to_owned())
            .await
    }
}
