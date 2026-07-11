// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// CRUD for the remote_device table (Phase 2 / C1 of the proxy suite).
// 跟 codeg PC 端 remote_workspace_connection_service 同形,只是改了 entity 名 + 字段名。

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, EntityTrait, IntoActiveModel,
    QueryOrder, Set, TransactionTrait,
};

use crate::app_error::AppCommandError;
use crate::db::entities::remote_device;
use crate::db::error::DbError;
use crate::models::remote_device::RemoteDeviceInfo;

fn to_info(model: remote_device::Model) -> RemoteDeviceInfo {
    RemoteDeviceInfo {
        id: model.id,
        name: model.name,
        base_url: model.base_url,
        token: model.token,
        sort_order: model.sort_order,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

/// 标准化 base_url: trim + 去尾部 /, 校验 scheme 是 http/https。
pub fn normalize_base_url(raw: &str) -> Result<String, AppCommandError> {
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    let parsed = reqwest::Url::parse(&trimmed)
        .map_err(|e| AppCommandError::invalid_input("Remote device URL is invalid").with_detail(e.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => Ok(trimmed),
        _ => Err(AppCommandError::invalid_input(
            "Remote device URL must use http or https",
        )),
    }
}

fn validate_name(name: &str) -> Result<String, AppCommandError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppCommandError::invalid_input(
            "Remote device name is required",
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_token(token: &str) -> Result<String, AppCommandError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(AppCommandError::invalid_input(
            "Remote device token is required",
        ));
    }
    Ok(trimmed.to_string())
}

pub async fn list(conn: &DatabaseConnection) -> Result<Vec<RemoteDeviceInfo>, DbError> {
    let rows = remote_device::Entity::find()
        .order_by_asc(remote_device::Column::SortOrder)
        .order_by_asc(remote_device::Column::Name)
        .all(conn)
        .await?;
    Ok(rows.into_iter().map(to_info).collect())
}

pub async fn get(
    conn: &DatabaseConnection,
    id: i32,
) -> Result<Option<RemoteDeviceInfo>, DbError> {
    let row = remote_device::Entity::find_by_id(id).one(conn).await?;
    Ok(row.map(to_info))
}

pub async fn create(
    conn: &DatabaseConnection,
    name: &str,
    base_url: &str,
    token: &str,
) -> Result<RemoteDeviceInfo, AppCommandError> {
    let now = Utc::now();
    let max_order = remote_device::Entity::find()
        .order_by_desc(remote_device::Column::SortOrder)
        .one(conn)
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?
        .map(|m| m.sort_order)
        .unwrap_or(-1);
    let active = remote_device::ActiveModel {
        id: NotSet,
        name: Set(validate_name(name)?),
        base_url: Set(normalize_base_url(base_url)?),
        token: Set(validate_token(token)?),
        sort_order: Set(max_order + 1),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let model = active
        .insert(conn)
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?;
    Ok(to_info(model))
}

pub async fn update(
    conn: &DatabaseConnection,
    id: i32,
    name: &str,
    base_url: &str,
    token: &str,
) -> Result<RemoteDeviceInfo, AppCommandError> {
    let row = remote_device::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found(format!("Remote device {id} not found")))?;

    let mut active = row.into_active_model();
    active.name = Set(validate_name(name)?);
    active.base_url = Set(normalize_base_url(base_url)?);
    active.token = Set(validate_token(token)?);
    active.updated_at = Set(Utc::now());
    let model = active
        .update(conn)
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?;
    Ok(to_info(model))
}

pub async fn delete(conn: &DatabaseConnection, id: i32) -> Result<(), DbError> {
    remote_device::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}

pub async fn reorder(conn: &DatabaseConnection, ids: Vec<i32>) -> Result<(), AppCommandError> {
    if ids.is_empty() {
        return Ok(());
    }

    let unique_ids = ids.iter().copied().collect::<HashSet<_>>();
    if unique_ids.len() != ids.len() {
        return Err(AppCommandError::invalid_input(
            "Remote device order contains duplicate ids",
        ));
    }

    let rows = remote_device::Entity::find()
        .all(conn)
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?;
    let existing_ids = rows.iter().map(|row| row.id).collect::<HashSet<_>>();
    if existing_ids != unique_ids {
        return Err(AppCommandError::invalid_input(
            "Remote device order must include every device exactly once",
        ));
    }

    let now = Utc::now();
    let mut rows_by_id = rows
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let txn = conn
        .begin()
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?;
    for (idx, id) in ids.into_iter().enumerate() {
        let Some(row) = rows_by_id.remove(&id) else {
            return Err(AppCommandError::invalid_input(
                "Remote device order contains an unknown id",
            ));
        };
        let mut active = row.into_active_model();
        active.sort_order = Set(idx as i32);
        active.updated_at = Set(now);
        active
            .update(&txn)
            .await
            .map_err(DbError::from)
            .map_err(AppCommandError::db)?;
    }
    txn.commit()
        .await
        .map_err(DbError::from)
        .map_err(AppCommandError::db)?;

    Ok(())
}
