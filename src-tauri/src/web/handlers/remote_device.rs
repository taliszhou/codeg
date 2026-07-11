// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Phase 2 / C1: 远程设备 CRUD + 透传代理 handler。

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Extension, Path, Request};
use axum::response::Response;
use axum::Json;
use reqwest::StatusCode as ReqStatus;
use serde::{Deserialize, Serialize};

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::app_state::AppState;
use crate::db::service::remote_device_service;
use crate::models::remote_device::{RemoteDeviceInfo, RemoteDeviceMasked};
use crate::proxy::{forward_http, pipe_websocket, ForwardOptions};

// ============================================================================
// CRUD
// ============================================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRemoteDeviceParams {
    pub name: String,
    pub base_url: String,
    pub token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRemoteDeviceParams {
    pub id: i32,
    pub name: String,
    pub base_url: String,
    pub token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDeviceIdParams {
    pub id: i32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderRemoteDevicesParams {
    pub ids: Vec<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRemoteDeviceParams {
    pub base_url: String,
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRemoteDeviceResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub message: Option<String>,
}

pub async fn list_remote_devices(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<RemoteDeviceMasked>>, AppCommandError> {
    let rows = remote_device_service::list(&state.db.conn)
        .await
        .map_err(AppCommandError::db)?;
    Ok(Json(rows.iter().map(RemoteDeviceInfo::masked).collect()))
}

pub async fn get_remote_device(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RemoteDeviceIdParams>,
) -> Result<Json<RemoteDeviceMasked>, AppCommandError> {
    let row = remote_device_service::get(&state.db.conn, params.id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found(format!("Remote device {} not found", params.id)))?;
    Ok(Json(row.masked()))
}

pub async fn create_remote_device(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateRemoteDeviceParams>,
) -> Result<Json<RemoteDeviceMasked>, AppCommandError> {
    validate_remote_health(&params.base_url, &params.token).await?;
    let row = remote_device_service::create(&state.db.conn, &params.name, &params.base_url, &params.token).await?;
    Ok(Json(row.masked()))
}

pub async fn update_remote_device(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateRemoteDeviceParams>,
) -> Result<Json<RemoteDeviceMasked>, AppCommandError> {
    validate_remote_health(&params.base_url, &params.token).await?;
    let row = remote_device_service::update(&state.db.conn, params.id, &params.name, &params.base_url, &params.token)
        .await?;
    Ok(Json(row.masked()))
}

pub async fn delete_remote_device(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<RemoteDeviceIdParams>,
) -> Result<Json<()>, AppCommandError> {
    remote_device_service::delete(&state.db.conn, params.id)
        .await
        .map_err(AppCommandError::db)?;
    Ok(Json(()))
}

pub async fn reorder_remote_devices(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ReorderRemoteDevicesParams>,
) -> Result<Json<()>, AppCommandError> {
    remote_device_service::reorder(&state.db.conn, params.ids).await?;
    Ok(Json(()))
}

pub async fn test_remote_device(
    Json(params): Json<TestRemoteDeviceParams>,
) -> Json<TestRemoteDeviceResult> {
    match validate_remote_health(&params.base_url, &params.token).await {
        Ok(()) => Json(TestRemoteDeviceResult {
            ok: true,
            status: Some(200),
            message: None,
        }),
        Err(e) => Json(TestRemoteDeviceResult {
            ok: false,
            status: None,
            message: Some(e.message),
        }),
    }
}

/// 拨打远端 `<base_url>/api/health` 验证连通性 + token 合法性。
async fn validate_remote_health(base_url: &str, token: &str) -> Result<(), AppCommandError> {
    let normalized = remote_device_service::normalize_base_url(base_url)?;
    let url = format!("{normalized}/api/health");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| AppCommandError::network(format!("http client: {e}")))?;
    let resp = client
        .post(&url)
        .bearer_auth(token.trim())
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| {
            AppCommandError::network(format!("Cannot reach {url}")).with_detail(e.to_string())
        })?;
    let status = resp.status();
    if status == ReqStatus::UNAUTHORIZED {
        return Err(AppCommandError::authentication_failed(
            "Remote device token is invalid (401)",
        ));
    }
    if !status.is_success() {
        return Err(AppCommandError::network(format!(
            "Remote responded {status}"
        )));
    }
    Ok(())
}

// ============================================================================
// 透传代理 — 通配 /api/remote/:device_id/api/*path → device.base_url/api/<path>
// ============================================================================

pub async fn http_proxy(
    Extension(state): Extension<Arc<AppState>>,
    Path((device_id, path)): Path<(i32, String)>,
    req: Request,
) -> Result<Response, AppCommandError> {
    let device = remote_device_service::get(&state.db.conn, device_id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| {
            AppCommandError::not_found(format!("Remote device {device_id} not found"))
        })?;

    let query = req.uri().query().unwrap_or("");
    let upstream = if query.is_empty() {
        format!("{}/api/{}", device.base_url.trim_end_matches('/'), path)
    } else {
        format!(
            "{}/api/{}?{}",
            device.base_url.trim_end_matches('/'),
            path,
            query
        )
    };

    let opts = ForwardOptions {
        bearer_token: Some(&device.token),
        trace_label: "remote-device",
    };
    forward_http(req, &upstream, &opts).await.map_err(|e| {
        AppCommandError::new(AppErrorCode::NetworkError, e.to_string())
    })
}

/// 透传 WS 到远端 `/ws/events?token=<remote_token>`。
pub async fn ws_proxy(
    Extension(state): Extension<Arc<AppState>>,
    Path(device_id): Path<i32>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppCommandError> {
    let device = remote_device_service::get(&state.db.conn, device_id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| {
            AppCommandError::not_found(format!("Remote device {device_id} not found"))
        })?;
    let token = device.token.clone();
    let base = device.base_url.trim_end_matches('/').to_string();
    let upstream_ws = base
        .replacen("http://", "ws://", 1)
        .replacen("https://", "wss://", 1)
        + "/ws/events?token="
        + &urlencoding::encode(&token);

    let response = ws.on_upgrade(move |socket| async move {
        if let Err(e) = pipe_websocket(socket, upstream_ws.clone(), None, "remote-device").await {
            eprintln!("[PROXY:remote-device] ws pipe error: {e}");
        }
    });
    Ok(response)
}

