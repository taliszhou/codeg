use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Connection::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Connection::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Connection::Name).string().not_null())
                    .col(ColumnDef::new(Connection::Kind).string().not_null())
                    .col(ColumnDef::new(Connection::SshHost).string())
                    .col(ColumnDef::new(Connection::SshUser).string())
                    .col(ColumnDef::new(Connection::SshPort).integer())
                    .col(ColumnDef::new(Connection::SshAlias).string())
                    .col(ColumnDef::new(Connection::SshKeyPath).string())
                    .col(
                        ColumnDef::new(Connection::SshAuthMethod)
                            .string()
                            .not_null()
                            .default("key"),
                    )
                    .col(ColumnDef::new(Connection::ProxyJump).string())
                    .col(
                        ColumnDef::new(Connection::DaemonPath)
                            .string()
                            .not_null()
                            .default("~/.codeg-remote"),
                    )
                    .col(ColumnDef::new(Connection::DaemonVersion).string())
                    .col(
                        ColumnDef::new(Connection::AutoConnect)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Connection::LastConnectedAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Connection::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Connection::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Connection::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Connection {
    Table,
    Id,
    Name,
    Kind,
    SshHost,
    SshUser,
    SshPort,
    SshAlias,
    SshKeyPath,
    SshAuthMethod,
    ProxyJump,
    DaemonPath,
    DaemonVersion,
    AutoConnect,
    LastConnectedAt,
    CreatedAt,
    UpdatedAt,
}
