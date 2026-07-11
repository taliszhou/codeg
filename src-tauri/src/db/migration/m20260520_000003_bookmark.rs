use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Bookmark::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Bookmark::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Bookmark::Title).string().not_null())
                    .col(ColumnDef::new(Bookmark::Url).string().not_null())
                    .col(
                        ColumnDef::new(Bookmark::SortOrder)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Bookmark::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Bookmark::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_bookmark_sort_order")
                    .table(Bookmark::Table)
                    .col(Bookmark::SortOrder)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Bookmark::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Bookmark {
    Table,
    Id,
    Title,
    Url,
    SortOrder,
    CreatedAt,
    UpdatedAt,
}
