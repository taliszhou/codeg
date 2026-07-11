// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M7: LocalNpuProvider - 高层封装，集成到 codeg 现有 model provider 体系。
// 实际请求通过 LocalNpuClient 走 Unix socket。

use std::sync::Arc;

use super::client::LocalNpuClient;
use super::types::{
    AdminLoadResponse, AdminUnloadResponse, ChatCompletionChunk, ChatCompletionRequest,
    ChatCompletionResponse, LocalNpuError, ModelsListResponse,
};

// Kotlin MediaPipeLlmBridge 监听 127.0.0.1:11434 (TCP)。
// container 跟 app 共享 network namespace，所以直接连。
const DEFAULT_ADDR: &str = "127.0.0.1:11434";

pub struct LocalNpuProvider {
    client: Arc<LocalNpuClient>,
}

impl LocalNpuProvider {
    /// 创建一个 provider，自动用默认 TCP 地址 127.0.0.1:11434
    pub fn new() -> Self {
        Self::with_addr(DEFAULT_ADDR)
    }

    pub fn with_addr(addr: impl Into<String>) -> Self {
        Self {
            client: Arc::new(LocalNpuClient::new(addr)),
        }
    }

    /// codeg model provider 体系里用的 name (UI / DB 里看到)
    pub fn name(&self) -> &'static str {
        "local-npu"
    }

    pub fn display_name(&self) -> &'static str {
        "Local NPU (MediaPipe)"
    }

    pub fn addr(&self) -> &str {
        self.client.addr()
    }

    // ------------------------------------------------------------------
    // Endpoints
    // ------------------------------------------------------------------

    pub async fn list_models(&self) -> Result<ModelsListResponse, LocalNpuError> {
        self.client.list_models().await
    }

    pub async fn load(&self, model_id: &str) -> Result<AdminLoadResponse, LocalNpuError> {
        self.client.admin_load(model_id).await
    }

    pub async fn unload(&self) -> Result<AdminUnloadResponse, LocalNpuError> {
        self.client.admin_unload().await
    }

    pub async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, LocalNpuError> {
        self.client.chat_completion(req).await
    }

    pub async fn chat_completion_stream(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<
        tokio::sync::mpsc::Receiver<Result<Option<ChatCompletionChunk>, LocalNpuError>>,
        LocalNpuError,
    > {
        self.client.chat_completion_stream(req).await
    }

    /// 健康检查 — Bridge 在线则返回 true，否则 false。
    pub async fn is_available(&self) -> bool {
        self.list_models().await.is_ok()
    }
}

impl Default for LocalNpuProvider {
    fn default() -> Self {
        Self::new()
    }
}
