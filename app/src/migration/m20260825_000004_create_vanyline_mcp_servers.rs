use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(McpServer::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(McpServer::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(McpServer::Name).string().not_null().unique_key())
                    .col(ColumnDef::new(McpServer::ServerType).string().not_null())
                    .col(ColumnDef::new(McpServer::Url).string().not_null())
                    .col(
                        ColumnDef::new(McpServer::Headers)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'{}'")),
                    )
                    .col(
                        ColumnDef::new(McpServer::AvailableTools)
                            .custom("JSONB")
                            .not_null()
                            .default(Expr::cust("'[]'")),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(McpServer::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum McpServer {
    #[sea_orm(iden = "vanyline_mcp_servers")]
    Table,
    Id,
    Name,
    ServerType,
    Url,
    Headers,
    AvailableTools,
}