use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "device_service")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub device_id: i32,
    pub name: String,
    pub url: String,
    pub sort_order: i32,
    pub enabled: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
