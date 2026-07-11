// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M7: 本地 NPU LLM provider — 通过 Unix socket 调 Kotlin 侧 MediaPipeLlmBridge。
//
// 见 docs/implementation/interfaces/npu-llm-protocol.md。

pub mod client;
pub mod provider;
pub mod types;

pub use provider::LocalNpuProvider;
pub use types::{ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ModelInfo};
