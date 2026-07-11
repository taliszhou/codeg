use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("forward_proxy_config"))
                    .add_column(
                        ColumnDef::new(Alias::new("upstream_device_id"))
                            .integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("forward_proxy_config"))
                    .drop_column(Alias::new("upstream_device_id"))
                    .to_owned(),
            )
            .await
    }
}
