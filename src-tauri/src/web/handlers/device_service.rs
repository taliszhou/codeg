// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Phase 5: 设备服务清单 handlers。

use std::sync::Arc;

use axum::extract::Extension;
use axum::Json;
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::db::service::device_service_service::{self, DeviceServiceInfo};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdParams {
    pub device_id: i32,
}

pub async fn list_device_services(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeviceIdParams>,
) -> Result<Json<Vec<DeviceServiceInfo>>, AppCommandError> {
    let rows = device_service_service::list_by_device(&state.db.conn, params.device_id)
        .await
        .map_err(AppCommandError::db)?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeviceServiceParams {
    pub device_id: i32,
    pub name: String,
    pub url: String,
}

pub async fn create_device_service(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<CreateDeviceServiceParams>,
) -> Result<Json<DeviceServiceInfo>, AppCommandError> {
    if params.name.trim().is_empty() || params.url.trim().is_empty() {
        return Err(AppCommandError::invalid_input("name 和 url 都不能为空"));
    }
    let info = device_service_service::create(
        &state.db.conn,
        params.device_id,
        &params.name,
        &params.url,
    )
    .await
    .map_err(AppCommandError::db)?;
    Ok(Json(info))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDeviceServiceParams {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

pub async fn update_device_service(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateDeviceServiceParams>,
) -> Result<Json<DeviceServiceInfo>, AppCommandError> {
    let info = device_service_service::update(
        &state.db.conn,
        params.id,
        params.name.as_deref(),
        params.url.as_deref(),
        params.enabled,
    )
    .await
    .map_err(AppCommandError::db)?;
    Ok(Json(info))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceServiceIdParams {
    pub id: i32,
}

pub async fn delete_device_service(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeviceServiceIdParams>,
) -> Result<Json<()>, AppCommandError> {
    device_service_service::delete(&state.db.conn, params.id)
        .await
        .map_err(AppCommandError::db)?;
    Ok(Json(()))
}
