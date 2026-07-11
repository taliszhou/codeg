use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ForwardProxyConfig::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ForwardProxyConfig::Id)
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ForwardProxyConfig::Enabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ForwardProxyConfig::ListenPort)
                            .integer()
                            .not_null()
                            .default(8118),
                    )
                    .col(
                        ColumnDef::new(ForwardProxyConfig::Token)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ForwardProxyConfig::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ForwardProxyConfig::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ForwardProxyConfig::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ForwardProxyConfig {
    Table,
    Id,
    Enabled,
    ListenPort,
    Token,
    CreatedAt,
    UpdatedAt,
}
