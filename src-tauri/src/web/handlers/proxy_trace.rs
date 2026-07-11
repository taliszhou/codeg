// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Phase 6 / 调试: 取最近的 N 条代理流量 trace 给 UI 显示。
// 数据来自 server/src/proxy/trace.rs 的全局 ring buffer。

use axum::Json;
use serde::Deserialize;

use crate::app_error::AppCommandError;
use crate::proxy::trace::{snapshot, TraceEntry};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListTracesParams {
    /// 最多返回多少条 (默认 100, 上限 200 — buffer 容量)
    #[serde(default)]
    pub limit: Option<usize>,
}

pub async fn list_proxy_traces(
    Json(params): Json<ListTracesParams>,
) -> Result<Json<Vec<TraceEntry>>, AppCommandError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    Ok(Json(snapshot(limit)))
}
