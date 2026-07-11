// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// ForwardProxyManager — 管理 forward proxy task 的启停。
// 单例 (per AppState), 配置变更时 stop 旧 task + start 新 task。

use std::sync::Arc;

use tokio::sync::Mutex;

use super::server::{run_forward_proxy, ForwardProxyConfig};

#[derive(Default)]
pub struct ForwardProxyManager {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// 当前运行实例: 端口 + shutdown sender + JoinHandle
    running: Option<RunningInstance>,
}

struct RunningInstance {
    listen_port: u16,
    shutdown: tokio::sync::watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl ForwardProxyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前是否运行 + 端口。
    pub async fn status(&self) -> Option<u16> {
        self.inner.lock().await.running.as_ref().map(|r| r.listen_port)
    }

    /// 启动指定配置的 proxy。如果已有 instance,先 stop 旧的。
    pub async fn start(&self, config: ForwardProxyConfig) {
        let mut inner = self.inner.lock().await;
        if let Some(old) = inner.running.take() {
            let _ = old.shutdown.send(true);
            old.handle.abort();
        }
        let (tx, rx) = tokio::sync::watch::channel(false);
        let port = config.listen_port;
        let handle = tokio::spawn(async move {
            run_forward_proxy(config, rx).await;
        });
        inner.running = Some(RunningInstance {
            listen_port: port,
            shutdown: tx,
            handle,
        });
    }

    /// 停止当前 instance (如有)。
    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        if let Some(old) = inner.running.take() {
            let _ = old.shutdown.send(true);
            old.handle.abort();
        }
    }

    /// shutdown 全部 — 通常在 app 退出时调用。
    pub async fn shutdown_all(&self) {
        self.stop().await;
    }
}

pub type SharedForwardProxyManager = Arc<ForwardProxyManager>;
