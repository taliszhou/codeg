use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "forward_proxy_config")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i32,
    pub enabled: bool,
    pub listen_port: i32,
    pub token: String,
    /// Phase 4b: 如果设了, 所有 forward proxy 流量经此 device 跳板出口
    pub upstream_device_id: Option<i32>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
