use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RemoteDevice::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RemoteDevice::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(RemoteDevice::Name).string().not_null())
                    .col(ColumnDef::new(RemoteDevice::BaseUrl).string().not_null())
                    .col(ColumnDef::new(RemoteDevice::Token).text().not_null())
                    .col(
                        ColumnDef::new(RemoteDevice::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(RemoteDevice::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RemoteDevice::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_remote_device_sort_order")
                    .table(RemoteDevice::Table)
                    .col(RemoteDevice::SortOrder)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_remote_device_base_url")
                    .table(RemoteDevice::Table)
                    .col(RemoteDevice::BaseUrl)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RemoteDevice::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum RemoteDevice {
    Table,
    Id,
    Name,
    BaseUrl,
    Token,
    SortOrder,
    CreatedAt,
    UpdatedAt,
}
