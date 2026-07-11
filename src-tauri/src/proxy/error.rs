// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Proxy 模块的统一错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("upstream connect failed: {0}")]
    UpstreamConnect(String),

    #[error("upstream returned {status}: {message}")]
    UpstreamStatus { status: u16, message: String },

    #[error("invalid upstream url: {0}")]
    InvalidUpstream(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("ws upgrade error: {0}")]
    WsUpgrade(String),

    #[error("ws closed unexpectedly")]
    WsClosed,

    #[error("device not found: {0}")]
    DeviceNotFound(i32),

    #[error("auth required")]
    AuthRequired,
}

impl ProxyError {
    pub fn upstream_status(status: u16, body: impl Into<String>) -> Self {
        Self::UpstreamStatus {
            status,
            message: body.into(),
        }
    }
}
