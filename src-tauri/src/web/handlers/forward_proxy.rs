// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Phase 3 / C3: HTTP forward proxy 配置 + 启停 handler。

use std::sync::Arc;

use axum::extract::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::db::service::{forward_proxy_service, remote_device_service};
use crate::forward_proxy::{ForwardProxyConfig};
use crate::forward_proxy::server::UpstreamDevice;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardProxyStatus {
    pub enabled: bool,
    pub running: bool,
    pub listen_port: u16,
    pub running_port: Option<u16>,
    /// 完整 token (前端展示时也要看到, 用户复制配置 client)
    pub token: String,
    /// Phase 4b: 经此 device 跳板; None 表示本机出口
    pub upstream_device_id: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
}

async fn resolve_upstream(
    db: &sea_orm::DatabaseConnection,
    device_id: Option<i32>,
) -> Result<Option<UpstreamDevice>, AppCommandError> {
    let Some(id) = device_id else { return Ok(None) };
    let dev = remote_device_service::get(db, id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found(format!("device {id}")))?;
    Ok(Some(UpstreamDevice {
        base_url: dev.base_url,
        token: dev.token,
    }))
}

pub async fn get_forward_proxy_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ForwardProxyStatus>, AppCommandError> {
    let cfg = forward_proxy_service::get_or_default(&state.db.conn)
        .await
        .map_err(AppCommandError::db)?;
    let running_port = state.forward_proxy.status().await;
    Ok(Json(ForwardProxyStatus {
        enabled: cfg.enabled,
        running: running_port.is_some(),
        listen_port: cfg.listen_port,
        running_port,
        token: cfg.token,
        upstream_device_id: cfg.upstream_device_id,
        created_at: cfg.created_at.to_rfc3339(),
        updated_at: cfg.updated_at.to_rfc3339(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateForwardProxyParams {
    pub enabled: bool,
    pub listen_port: u16,
    /// Phase 4b: 选哪台 device 当跳板 (null 表示本机出口)。可选, 不传则保留原值。
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub upstream_device_id: Option<Option<i32>>,
}

// 区分 "key 缺失" (保留原值) 和 "key 显式为 null" (清空)
fn deserialize_optional_field<'de, D>(de: D) -> Result<Option<Option<i32>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Option::<Option<i32>>::deserialize(de).map(|v| match v {
        Some(inner) => Some(inner),
        None => Some(None),
    })
}

pub async fn update_forward_proxy(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateForwardProxyParams>,
) -> Result<Json<ForwardProxyStatus>, AppCommandError> {
    if !(1..=65535).contains(&params.listen_port) {
        return Err(AppCommandError::invalid_input(
            "listen_port must be 1..65535",
        ));
    }
    let cfg = forward_proxy_service::update(
        &state.db.conn,
        params.enabled,
        params.listen_port,
        None,
        params.upstream_device_id,
    )
    .await
    .map_err(AppCommandError::db)?;

    // 同步 manager: 启用就 start (会重启如端口换了), 禁用就 stop
    if cfg.enabled {
        let upstream = resolve_upstream(&state.db.conn, cfg.upstream_device_id).await?;
        state
            .forward_proxy
            .start(ForwardProxyConfig {
                listen_port: cfg.listen_port,
                token: cfg.token.clone(),
                upstream_device: upstream,
            })
            .await;
    } else {
        state.forward_proxy.stop().await;
    }
    let running_port = state.forward_proxy.status().await;
    Ok(Json(ForwardProxyStatus {
        enabled: cfg.enabled,
        running: running_port.is_some(),
        listen_port: cfg.listen_port,
        running_port,
        token: cfg.token,
        upstream_device_id: cfg.upstream_device_id,
        created_at: cfg.created_at.to_rfc3339(),
        updated_at: cfg.updated_at.to_rfc3339(),
    }))
}

pub async fn regenerate_forward_proxy_token(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ForwardProxyStatus>, AppCommandError> {
    let cfg = forward_proxy_service::regenerate_token(&state.db.conn)
        .await
        .map_err(AppCommandError::db)?;
    // 如果当前 running, 必须重启 task 让新 token 生效
    if cfg.enabled {
        let upstream = resolve_upstream(&state.db.conn, cfg.upstream_device_id).await?;
        state
            .forward_proxy
            .start(ForwardProxyConfig {
                listen_port: cfg.listen_port,
                token: cfg.token.clone(),
                upstream_device: upstream,
            })
            .await;
    }
    let running_port = state.forward_proxy.status().await;
    Ok(Json(ForwardProxyStatus {
        enabled: cfg.enabled,
        running: running_port.is_some(),
        listen_port: cfg.listen_port,
        running_port,
        token: cfg.token,
        upstream_device_id: cfg.upstream_device_id,
        created_at: cfg.created_at.to_rfc3339(),
        updated_at: cfg.updated_at.to_rfc3339(),
    }))
}
