// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Proxy 套件公共底层。共享给 C1 远程设备 / C2 服务清单 / C3 网络跳板。
// 见 docs/PROXY_SUITE.md。

pub mod connect_tunnel;
pub mod error;
pub mod http_forwarder;
pub mod trace;
pub mod ws_forwarder;

pub use error::ProxyError;
pub use http_forwarder::{forward_http, is_websocket_upgrade, ForwardOptions};
pub use trace::{snapshot as trace_snapshot, TraceEntry};
pub use ws_forwarder::pipe_websocket;
