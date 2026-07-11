// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M7: Local NPU model market API.
//
//   GET    /api/local_npu_list_models       — 列出可下载 + 已下载的模型
//   POST   /api/local_npu_download_model    — 启动后台下载（task_id 形式）
//   GET    /api/local_npu_download_status   — 查下载进度（轮询）
//   POST   /api/local_npu_delete_model      — 删除本地模型文件
//   POST   /api/local_npu_load_model        — load 指定模型到 NPU
//   POST   /api/local_npu_unload_model      — 卸载当前 active model
//   GET    /api/local_npu_active            — 当前 active model
//   GET    /api/local_npu_health            — Bridge 健康检查

use std::collections::HashMap;
use std::sync::Mutex;

use axum::Json;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::llm::LocalNpuProvider;

// ----------------------------------------------------------------------
// 推荐模型列表（v0.1 hardcoded）
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RecommendedModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub size_bytes: u64,
    pub url: &'static str,
    pub sha256: Option<&'static str>,
    pub estimated_tok_per_sec: u32,
}

// 注意：实际可用模型/URL 在 M7 验收时确认。下面是占位 + 文档化预期。
const RECOMMENDED: &[RecommendedModel] = &[
    RecommendedModel {
        id: "gemma-4-e2b",
        display_name: "Gemma 4 E2B (LiteRT-LM, GPU 后端)",
        description: "2B 参数 .litertlm，CPU+GPU 后端，骁龙 8 Gen 3 GPU ~30-50 tok/s。\
                       HF gated 需 token + accept license。",
        size_bytes: 2_780_000_000, // ~2.59GB
        url: "https://huggingface.co/litert-community/gemma-4-E2B-it-litert-lm/resolve/main/gemma-4-E2B-it.litertlm",
        sha256: None,
        estimated_tok_per_sec: 40,
    },
    RecommendedModel {
        id: "gemma-4-e4b",
        display_name: "Gemma 4 E4B (LiteRT-LM)",
        description: "4B 参数 .litertlm，~5GB，需要旗舰机 + 8GB+ RAM。HF gated。",
        size_bytes: 5_368_709_120,
        url: "https://huggingface.co/litert-community/gemma-4-E4B-it-litert-lm/resolve/main/gemma-4-E4B-it.litertlm",
        sha256: None,
        estimated_tok_per_sec: 30,
    },
    RecommendedModel {
        id: "gemma-3-1b",
        display_name: "Gemma 3 1B IT (轻量测试)",
        description: "1B 参数 .task，~600MB，最快上手测试用。HF gated。",
        size_bytes: 645_000_000,
        url: "https://huggingface.co/litert-community/Gemma3-1B-IT/resolve/main/Gemma3-1B-IT_multi-prefill-seq_q8_ekv1280.task",
        sha256: None,
        estimated_tok_per_sec: 60,
    },
];

// ----------------------------------------------------------------------
// 下载任务状态（in-memory）
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DownloadTaskState {
    pub task_id: String,
    pub model_id: String,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub status: String, // "downloading" / "completed" / "error" / "cancelled"
    pub error: Option<String>,
}

static DOWNLOAD_TASKS: Lazy<Mutex<HashMap<String, DownloadTaskState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// ----------------------------------------------------------------------
// 公共响应类型
// ----------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ModelCatalogEntry {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub size_bytes: u64,
    pub estimated_tok_per_sec: u32,
    pub url: String,
    pub downloaded: bool,
    pub file_path: Option<String>,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct ListModelsResponse {
    pub models: Vec<ModelCatalogEntry>,
    pub active_model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelIdParams {
    pub model_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStartedResponse {
    pub task_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatusParams {
    pub task_id: String,
}

// ----------------------------------------------------------------------
// 模型文件根目录 (host-side)
// ----------------------------------------------------------------------

/// 模型存放优先级 (从高到低)：
///   1. /root/models/npu/    (容器内 = app 内部存储 /data/data/.../containers/0/root/models/npu/)
///                            优势：无需 MANAGE_EXTERNAL_STORAGE 权限，最可靠
///   2. /media/sd/mobilega/models/npu/  (sd 卡 via proot --bind=/storage/self/primary)
///                                       需要用户授权外存访问。
/// list/load 时按顺序探测；download 默认写到 (1)。
fn models_dirs() -> Vec<std::path::PathBuf> {
    vec![
        std::path::PathBuf::from("/root/models/npu"),
        std::path::PathBuf::from("/media/sd/mobilega/models/npu"),
    ]
}

fn models_dir() -> std::path::PathBuf {
    models_dirs()[0].clone()
}

// ----------------------------------------------------------------------
// Handlers
// ----------------------------------------------------------------------

pub async fn list_models() -> Result<Json<ListModelsResponse>, AppCommandError> {
    let provider = LocalNpuProvider::new();
    let bridge_models = provider.list_models().await.ok();
    let active_id = bridge_models
        .as_ref()
        .and_then(|m| m.data.iter().find(|x| x.loaded).map(|x| x.id.clone()));

    let downloaded_ids: std::collections::HashSet<String> = bridge_models
        .map(|m| m.data.iter().map(|x| x.id.clone()).collect())
        .unwrap_or_default();

    let dirs = models_dirs();
    let mut models = Vec::new();
    for r in RECOMMENDED {
        // 多目录 × 多扩展名扫
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        for d in &dirs {
            candidates.push(d.join(format!("{}.litertlm", r.id)));
            candidates.push(d.join(format!("{}.task", r.id)));
        }
        let found_path = candidates.iter().find(|p| p.exists()).cloned();
        let downloaded = found_path.is_some() || downloaded_ids.contains(r.id);
        models.push(ModelCatalogEntry {
            id: r.id.into(),
            display_name: r.display_name.into(),
            description: r.description.into(),
            size_bytes: r.size_bytes,
            estimated_tok_per_sec: r.estimated_tok_per_sec,
            url: r.url.into(),
            downloaded,
            file_path: found_path.map(|p| p.to_string_lossy().to_string()),
            active: active_id.as_deref() == Some(r.id),
        });
    }

    Ok(Json(ListModelsResponse {
        models,
        active_model_id: active_id,
    }))
}

pub async fn download_model(
    Json(params): Json<ModelIdParams>,
) -> Result<Json<DownloadStartedResponse>, AppCommandError> {
    let recommended = RECOMMENDED
        .iter()
        .find(|r| r.id == params.model_id)
        .cloned()
        .ok_or_else(|| {
            AppCommandError::new(AppErrorCode::NotFound, format!("unknown model: {}", params.model_id))
        })?;

    let task_id = Uuid::new_v4().to_string();
    let state = DownloadTaskState {
        task_id: task_id.clone(),
        model_id: recommended.id.into(),
        total_bytes: recommended.size_bytes,
        downloaded_bytes: 0,
        status: "downloading".into(),
        error: None,
    };
    DOWNLOAD_TASKS
        .lock()
        .unwrap()
        .insert(task_id.clone(), state);

    let tid = task_id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_download(tid.clone(), recommended).await {
            tracing::warn!("download {} failed: {}", tid, e);
            if let Ok(mut m) = DOWNLOAD_TASKS.lock() {
                if let Some(s) = m.get_mut(&tid) {
                    s.status = "error".into();
                    s.error = Some(e.to_string());
                }
            }
        }
    });

    Ok(Json(DownloadStartedResponse { task_id }))
}

async fn run_download(task_id: String, model: RecommendedModel) -> Result<(), String> {
    use std::fs;
    let dir = models_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir failed: {e}"))?;
    let partial = dir.join(format!("{}.task.downloading", model.id));
    let final_path = dir.join(format!("{}.task", model.id));

    let resp = reqwest::get(model.url).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(model.size_bytes);
    // 更新 total
    if let Ok(mut m) = DOWNLOAD_TASKS.lock() {
        if let Some(s) = m.get_mut(&task_id) {
            s.total_bytes = total;
        }
    }

    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("create file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        if downloaded % (4 * 1024 * 1024) < chunk.len() as u64 {
            // 大约每 4MB 一次进度更新
            if let Ok(mut m) = DOWNLOAD_TASKS.lock() {
                if let Some(s) = m.get_mut(&task_id) {
                    s.downloaded_bytes = downloaded;
                }
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    if let Some(expected) = model.sha256 {
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("sha256 mismatch: expected {expected}, got {actual}"));
        }
    }

    std::fs::rename(&partial, &final_path).map_err(|e| format!("rename: {e}"))?;

    // 更新 manifest.json
    update_manifest(&model, &final_path)?;

    if let Ok(mut m) = DOWNLOAD_TASKS.lock() {
        if let Some(s) = m.get_mut(&task_id) {
            s.status = "completed".into();
            s.downloaded_bytes = downloaded;
        }
    }
    Ok(())
}

fn update_manifest(model: &RecommendedModel, file: &std::path::Path) -> Result<(), String> {
    let manifest_path = models_dir().join("manifest.json");
    let mut manifest: serde_json::Value = if manifest_path.exists() {
        let raw = std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({"models": []}))
    } else {
        serde_json::json!({"models": []})
    };
    let models = manifest
        .get_mut("models")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| "bad manifest".to_string())?;
    models.retain(|m| m.get("id").and_then(|x| x.as_str()) != Some(model.id));
    models.push(serde_json::json!({
        "id": model.id,
        "file": file.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        "size_bytes": model.size_bytes,
        "downloaded_at": chrono::Utc::now().to_rfc3339(),
        "source_url": model.url,
    }));
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn download_status(
    Json(params): Json<DownloadStatusParams>,
) -> Result<Json<DownloadTaskState>, AppCommandError> {
    let state = DOWNLOAD_TASKS
        .lock()
        .unwrap()
        .get(&params.task_id)
        .cloned()
        .ok_or_else(|| {
            AppCommandError::new(AppErrorCode::NotFound, format!("task {}", params.task_id))
        })?;
    Ok(Json(state))
}

pub async fn delete_model(
    Json(params): Json<ModelIdParams>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    let provider = LocalNpuProvider::new();
    // 若 active，先 unload
    if let Ok(list) = provider.list_models().await {
        if list.data.iter().any(|m| m.id == params.model_id && m.loaded) {
            let _ = provider.unload().await;
        }
    }
    let file = models_dir().join(format!("{}.task", params.model_id));
    if file.exists() {
        std::fs::remove_file(&file).map_err(|e| {
            AppCommandError::new(AppErrorCode::IoError, format!("remove {}: {e}", file.display()))
        })?;
    }
    // 更新 manifest
    let manifest_path = models_dir().join("manifest.json");
    if manifest_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
            if let Ok(mut manifest) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(models) = manifest.get_mut("models").and_then(|v| v.as_array_mut()) {
                    models.retain(|m| m.get("id").and_then(|x| x.as_str()) != Some(&params.model_id));
                    let _ = std::fs::write(
                        &manifest_path,
                        serde_json::to_string_pretty(&manifest).unwrap(),
                    );
                }
            }
        }
    }
    Ok(Json(serde_json::json!({"deleted": params.model_id})))
}

pub async fn load_model(
    Json(params): Json<ModelIdParams>,
) -> Result<Json<serde_json::Value>, AppCommandError> {
    let provider = LocalNpuProvider::new();
    let result = provider
        .load(&params.model_id)
        .await
        .map_err(|e| AppCommandError::new(AppErrorCode::ExternalCommandFailed, e.to_string()))?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTestParams {
    pub model_id: String,
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTestResponse {
    pub model_id: String,
    pub served_model: String,
    pub prompt: String,
    pub reply: String,
    pub elapsed_ms: u64,
}

/// 激活后自检：向 bridge 发一条短 prompt，返回模型回复用于 UI 显示。
/// 用来证明"激活"真的 work，而不是只看 active_model_id 的写入。
pub async fn chat_test(
    Json(params): Json<ChatTestParams>,
) -> Result<Json<ChatTestResponse>, AppCommandError> {
    use crate::llm::local_npu::types::{ChatCompletionRequest, ChatMessage};

    let provider = LocalNpuProvider::new();
    let prompt = params
        .prompt
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "你好，用一句话介绍自己。".to_string());

    let req = ChatCompletionRequest {
        model: params.model_id.clone(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: prompt.clone(),
        }],
        stream: Some(false),
        temperature: Some(0.3),
        top_k: Some(64),
        top_p: Some(0.95),
        max_tokens: Some(96),
    };

    let started = std::time::Instant::now();
    let resp = provider.chat_completion(&req).await.map_err(|e| {
        AppCommandError::new(
            AppErrorCode::ExternalCommandFailed,
            format!("chat_test failed: {e}"),
        )
    })?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let reply = resp
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    Ok(Json(ChatTestResponse {
        model_id: params.model_id,
        served_model: resp.model,
        prompt,
        reply,
        elapsed_ms,
    }))
}

pub async fn unload_model() -> Result<Json<serde_json::Value>, AppCommandError> {
    let provider = LocalNpuProvider::new();
    let result = provider
        .unload()
        .await
        .map_err(|e| AppCommandError::new(AppErrorCode::ExternalCommandFailed, e.to_string()))?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

pub async fn health() -> Result<Json<serde_json::Value>, AppCommandError> {
    let provider = LocalNpuProvider::new();
    let ok = provider.is_available().await;
    Ok(Json(serde_json::json!({
        "ok": ok,
        "addr": provider.addr(),
    })))
}
