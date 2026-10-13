use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Une instruction ALTER par colonne : SQLite n'accepte qu'une action par
        // `ALTER TABLE` — deux `alter_table` séparés rendent le même DDL sur les
        // deux moteurs (spec : DDL identique PostgreSQL/SQLite).
        manager
            .alter_table(
                Table::alter()
                    .table(Agent::Table)
                    .add_column(ColumnDef::new(Agent::SystemKey).string())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Agent::Table)
                    .add_column(ColumnDef::new(Agent::SystemVersion).integer())
                    .to_owned(),
            )
            .await?;
        // Unicité `(owner_id, system_key)` : index UNIQUE composite, une instruction
        // séparée (règle régression Phase 3, cf. ./mod.rs). Les lignes à `system_key`
        // NULL n'entrent jamais en conflit (NULL ≠ NULL en contrainte d'unicité,
        // PostgreSQL comme SQLite) : un utilisateur a au plus un agent par clé système
        // et autant d'agents ordinaires qu'il veut.
        manager
            .create_index(
                Index::create()
                    .unique()
                    .name("uq_vanyline_agents_owner_system_key")
                    .table(Agent::Table)
                    .col(Agent::OwnerId)
                    .col(Agent::SystemKey)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Ordre symétrique : l'index d'abord (il dépend des colonnes), puis les colonnes.
        manager
            .drop_index(
                Index::drop()
                    .name("uq_vanyline_agents_owner_system_key")
                    .table(Agent::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Agent::Table)
                    .drop_column(Agent::SystemKey)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Agent::Table)
                    .drop_column(Agent::SystemVersion)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Agent {
    #[sea_orm(iden = "vanyline_agents")]
    Table,
    OwnerId,
    SystemKey,
    SystemVersion,
}
