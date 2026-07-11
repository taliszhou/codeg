// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M7: Local LLM module. 当前只有 local_npu (MediaPipe via Unix socket)。
// 后续若加其他 local LLM (llama.cpp, vllm-android, etc.) 都挂这下面。

pub mod local_npu;

pub use local_npu::LocalNpuProvider;
