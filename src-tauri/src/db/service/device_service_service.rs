// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 设备绑定的内网服务清单 CRUD (Phase 5)。
// 每个 remote_device 可以挂多个 service (name + url), 给浏览器模式快捷访问用。

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};

use crate::db::entities::device_service;
use crate::db::error::DbError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceServiceInfo {
    pub id: i32,
    pub device_id: i32,
    pub name: String,
    pub url: String,
    pub sort_order: i32,
    pub enabled: bool,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

fn to_info(m: device_service::Model) -> DeviceServiceInfo {
    DeviceServiceInfo {
        id: m.id,
        device_id: m.device_id,
        name: m.name,
        url: m.url,
        sort_order: m.sort_order,
        enabled: m.enabled,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

pub async fn list_by_device(
    conn: &DatabaseConnection,
    device_id: i32,
) -> Result<Vec<DeviceServiceInfo>, DbError> {
    let rows = device_service::Entity::find()
        .filter(device_service::Column::DeviceId.eq(device_id))
        .order_by_asc(device_service::Column::SortOrder)
        .order_by_asc(device_service::Column::Name)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(to_info).collect())
}

pub async fn create(
    conn: &DatabaseConnection,
    device_id: i32,
    name: &str,
    url: &str,
) -> Result<DeviceServiceInfo, DbError> {
    let now = Utc::now();
    let max_order = device_service::Entity::find()
        .filter(device_service::Column::DeviceId.eq(device_id))
        .order_by_desc(device_service::Column::SortOrder)
        .one(conn)
        .await?
        .map(|m| m.sort_order)
        .unwrap_or(-1);
    let active = device_service::ActiveModel {
        id: NotSet,
        device_id: Set(device_id),
        name: Set(name.trim().to_string()),
        url: Set(url.trim().to_string()),
        sort_order: Set(max_order + 1),
        enabled: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
    };
    Ok(to_info(active.insert(conn).await?))
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    name: Option<&str>,
    url: Option<&str>,
    enabled: Option<bool>,
) -> Result<DeviceServiceInfo, DbError> {
    let row = device_service::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration(format!("device_service {id} not found")))?;
    let mut active = row.into_active_model();
    if let Some(n) = name {
        active.name = Set(n.trim().to_string());
    }
    if let Some(u) = url {
        active.url = Set(u.trim().to_string());
    }
    if let Some(e) = enabled {
        active.enabled = Set(e);
    }
    active.updated_at = Set(Utc::now());
    Ok(to_info(active.update(conn).await?))
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    device_service::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}
