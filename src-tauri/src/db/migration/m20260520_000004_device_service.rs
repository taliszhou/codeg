use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DeviceService::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeviceService::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DeviceService::DeviceId).integer().not_null())
                    .col(ColumnDef::new(DeviceService::Name).string().not_null())
                    .col(ColumnDef::new(DeviceService::Url).string().not_null())
                    .col(
                        ColumnDef::new(DeviceService::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(DeviceService::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(DeviceService::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeviceService::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(DeviceService::Table, DeviceService::DeviceId)
                            .to(
                                Alias::new("remote_device"),
                                Alias::new("id"),
                            )
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_device_service_device_id")
                    .table(DeviceService::Table)
                    .col(DeviceService::DeviceId)
                    .col(DeviceService::SortOrder)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeviceService::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum DeviceService {
    Table,
    Id,
    DeviceId,
    Name,
    Url,
    SortOrder,
    Enabled,
    CreatedAt,
    UpdatedAt,
}
