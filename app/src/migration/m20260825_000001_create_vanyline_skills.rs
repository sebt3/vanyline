use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Skill::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Skill::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Skill::OwnerId).integer().not_null())
                    .col(ColumnDef::new(Skill::Name).string().not_null())
                    .col(ColumnDef::new(Skill::Description).string().not_null().default(""))
                    .col(ColumnDef::new(Skill::Body).string().not_null().default(""))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Skill::Table, Skill::OwnerId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(Skill::OwnerId)
                            .col(Skill::Name),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Skill::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Skill {
    #[sea_orm(iden = "vanyline_skills")]
    Table,
    Id,
    OwnerId,
    Name,
    Description,
    Body,
}

#[derive(DeriveIden)]
enum User {
    #[sea_orm(iden = "miryad_users")]
    Table,
    Id,
}