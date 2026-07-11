// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Forward proxy 子系统 (Phase 3 / C3)。
// 见 docs/PROXY_SUITE.md §6 (C3 网络跳板)。

pub mod manager;
pub mod server;

pub use manager::ForwardProxyManager;
pub use server::{run_forward_proxy, ForwardProxyConfig, UpstreamDevice};
