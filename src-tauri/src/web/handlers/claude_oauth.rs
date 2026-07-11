// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 把 host 端 macOS Keychain 的 Claude Code OAuth 凭证写到容器内 ~/.claude/.credentials.json,
// 让 claude-agent-acp / Claude Code CLI 自动通过 OAuth refresh flow 用上你的 Max 订阅。
// 同时在 model_provider 表 ensure 一行,让 UI "模型提供商" 能看到 "My Claude Code"。

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::app_state::AppState;
use crate::db::service::model_provider_service;

fn home_dir_or_default() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCredsParams {
    pub credentials_json: String,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCredsResult {
    pub written: String,
    pub provider_id: i32,
    pub provider_name: String,
    pub subscription_type: Option<String>,
    pub expires_at: Option<i64>,
}

pub async fn import_creds(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ImportCredsParams>,
) -> Result<Json<ImportCredsResult>, AppCommandError> {
    // 1. 验证 JSON 合法且包含 OAuth blob
    let parsed: serde_json::Value =
        serde_json::from_str(&params.credentials_json).map_err(|e| {
            AppCommandError::new(
                AppErrorCode::InvalidInput,
                format!("credentialsJson 不是合法 JSON: {e}"),
            )
        })?;

    let oauth = parsed.get("claudeAiOauth").ok_or_else(|| {
        AppCommandError::new(
            AppErrorCode::InvalidInput,
            "缺少 claudeAiOauth 字段 (期望 macOS Keychain 导出的 JSON)".to_string(),
        )
    })?;

    let access_token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            AppCommandError::new(
                AppErrorCode::InvalidInput,
                "claudeAiOauth.accessToken 缺失".to_string(),
            )
        })?
        .to_string();

    let subscription_type = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .map(String::from);
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64());

    // 2. 写 ~/.claude/.credentials.json (Claude SDK 标准读取位置)
    let claude_dir = home_dir_or_default().join(".claude");
    fs::create_dir_all(&claude_dir).map_err(|e| {
        AppCommandError::new(
            AppErrorCode::IoError,
            format!("mkdir {}: {e}", claude_dir.display()),
        )
    })?;
    let creds_path = claude_dir.join(".credentials.json");
    fs::write(&creds_path, &params.credentials_json).map_err(|e| {
        AppCommandError::new(
            AppErrorCode::IoError,
            format!("write {}: {e}", creds_path.display()),
        )
    })?;
    // 限制为 0600 (UNIX 友好)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&creds_path, fs::Permissions::from_mode(0o600));
    }

    // 3. 在 model_provider DB ensure 一行,让 UI "模型提供商" 能看到
    let provider_name = params
        .provider_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "My Claude Code (OAuth)".to_string());
    let api_url = params
        .api_url
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://api.anthropic.com".to_string());

    let provider = model_provider_service::ensure_by_name(
        &state.db.conn,
        &provider_name,
        &api_url,
        &access_token,
        "claude_code",
        "",
    )
    .await
    .map_err(|e| {
        AppCommandError::new(
            AppErrorCode::ExternalCommandFailed,
            format!("ensure provider failed: {e}"),
        )
    })?;

    Ok(Json(ImportCredsResult {
        written: creds_path.to_string_lossy().to_string(),
        provider_id: provider.id,
        provider_name: provider.name,
        subscription_type,
        expires_at,
    }))
}
