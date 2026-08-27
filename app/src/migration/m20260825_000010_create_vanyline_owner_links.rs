use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OwnerLink::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OwnerLink::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OwnerLink::UserId)
                            .integer()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(OwnerLink::K8sOwnerName).string())
                    .foreign_key(
                        ForeignKey::create()
                            .from(OwnerLink::Table, OwnerLink::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(OwnerLink::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum OwnerLink {
    #[sea_orm(iden = "vanyline_owner_links")]
    Table,
    Id,
    UserId,
    K8sOwnerName,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}
