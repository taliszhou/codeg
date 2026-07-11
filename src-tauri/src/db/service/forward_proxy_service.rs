// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// CRUD for the single-row forward_proxy_config table (Phase 3 / C3).
// 表设计是 single row (id = 1)。get_or_default 取/造默认; upsert 更新整行。

use chrono::Utc;
use rand::distributions::Alphanumeric;
use rand::Rng;
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, EntityTrait, IntoActiveModel,
    Set,
};

use crate::db::entities::forward_proxy_config;
use crate::db::error::DbError;

pub const SINGLETON_ID: i32 = 1;

#[derive(Debug, Clone)]
pub struct ForwardProxyConfigInfo {
    pub enabled: bool,
    pub listen_port: u16,
    pub token: String,
    /// Phase 4b: 经此 device 跳板, None = 本机直接出口
    pub upstream_device_id: Option<i32>,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

fn to_info(model: forward_proxy_config::Model) -> ForwardProxyConfigInfo {
    ForwardProxyConfigInfo {
        enabled: model.enabled,
        listen_port: model.listen_port as u16,
        token: model.token,
        upstream_device_id: model.upstream_device_id,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn gen_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// 拿当前 config, 不存在则按默认创建 (enabled=false, port=8118, token=random)。
pub async fn get_or_default(
    conn: &DatabaseConnection,
) -> Result<ForwardProxyConfigInfo, DbError> {
    if let Some(model) = forward_proxy_config::Entity::find_by_id(SINGLETON_ID)
        .one(conn)
        .await?
    {
        return Ok(to_info(model));
    }
    let now = Utc::now();
    let token = gen_token();
    let active = forward_proxy_config::ActiveModel {
        id: Set(SINGLETON_ID),
        enabled: Set(false),
        listen_port: Set(8118),
        token: Set(token),
        upstream_device_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let model = active.insert(conn).await?;
    Ok(to_info(model))
}

pub async fn update(
    conn: &DatabaseConnection,
    enabled: bool,
    listen_port: u16,
    token: Option<String>,
    upstream_device_id: Option<Option<i32>>,
) -> Result<ForwardProxyConfigInfo, DbError> {
    // 确保已存在
    let _ = get_or_default(conn).await?;
    let existing = forward_proxy_config::Entity::find_by_id(SINGLETON_ID)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration("forward_proxy_config singleton missing".into()))?;

    let mut active = existing.into_active_model();
    active.enabled = Set(enabled);
    active.listen_port = Set(listen_port as i32);
    if let Some(t) = token {
        active.token = Set(t);
    }
    if let Some(d) = upstream_device_id {
        active.upstream_device_id = Set(d);
    }
    active.updated_at = Set(Utc::now());
    let model = active.update(conn).await?;
    Ok(to_info(model))
}

/// 重新生成 token (用户点 "Regenerate token" 时)。
pub async fn set_upstream_device(
    conn: &DatabaseConnection,
    upstream_device_id: Option<i32>,
) -> Result<ForwardProxyConfigInfo, DbError> {
    let _ = get_or_default(conn).await?;
    let existing = forward_proxy_config::Entity::find_by_id(SINGLETON_ID)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration("forward_proxy_config singleton missing".into()))?;
    let mut active = existing.into_active_model();
    active.upstream_device_id = Set(upstream_device_id);
    active.updated_at = Set(Utc::now());
    Ok(to_info(active.update(conn).await?))
}

pub async fn regenerate_token(
    conn: &DatabaseConnection,
) -> Result<ForwardProxyConfigInfo, DbError> {
    let _ = get_or_default(conn).await?;
    let existing = forward_proxy_config::Entity::find_by_id(SINGLETON_ID)
        .one(conn)
        .await?
        .ok_or_else(|| DbError::Migration("forward_proxy_config singleton missing".into()))?;
    let mut active = existing.into_active_model();
    active.token = Set(gen_token());
    active.updated_at = Set(Utc::now());
    let model = active.update(conn).await?;
    Ok(to_info(model))
}

#[allow(dead_code)]
fn _silence_unused_notset() {
    let _ = forward_proxy_config::ActiveModel {
        id: NotSet,
        ..Default::default()
    };
}
