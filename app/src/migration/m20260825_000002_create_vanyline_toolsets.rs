use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Toolset::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Toolset::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Toolset::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Toolset::Name).string().not_null())
                    .col(ColumnDef::new(Toolset::Description).string())
                    .col(ColumnDef::new(Toolset::Prompt).string())
                    .col(
                        ColumnDef::new(Toolset::LocalTools)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'[]'")),
                    )
                    .col(
                        ColumnDef::new(Toolset::Mcp)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'[]'")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Toolset::Table, Toolset::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(Toolset::OwnerId)
                            .col(Toolset::Name),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Toolset::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Toolset {
    #[sea_orm(iden = "vanyline_toolsets")]
    Table,
    Id,
    OwnerId,
    Name,
    Description,
    Prompt,
    LocalTools,
    Mcp,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}