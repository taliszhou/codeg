// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M7: OpenAI 兼容请求/响应类型，跟 Kotlin 端协议对齐。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: i32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunkChoice {
    pub index: i32,
    pub delta: ChatDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
    pub loaded: bool,
    pub file_path: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsListResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLoadRequest {
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLoadResponse {
    pub loaded: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUnloadResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unloaded: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum LocalNpuError {
    #[error("socket connect failed: {0}")]
    SocketConnect(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("malformed http response: {0}")]
    BadResponse(String),

    #[error("model not loaded")]
    ModelNotLoaded,

    #[error("model file missing: {0}")]
    ModelFileMissing(String),

    #[error("npu busy")]
    NpuBusy,

    #[error("server returned {status}: {message}")]
    Server { status: u16, message: String },

    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
}
