// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// 代理流量 ring buffer。每条 request 进/出记一条,UI 可拿最近 N 条调试。

use std::collections::VecDeque;
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::Serialize;

const CAPACITY: usize = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceEntry {
    pub ts_ms: u64,
    pub label: &'static str,   // "remote-device" | "forward-proxy" | "service-proxy" | "tunnel"
    pub method: String,
    pub upstream: String,
    pub status: Option<u16>,
    pub elapsed_ms: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub error: Option<String>,
}

static TRACES: Lazy<RwLock<VecDeque<TraceEntry>>> =
    Lazy::new(|| RwLock::new(VecDeque::with_capacity(CAPACITY)));

pub fn record(entry: TraceEntry) {
    if let Ok(mut q) = TRACES.write() {
        if q.len() == CAPACITY {
            q.pop_front();
        }
        q.push_back(entry);
    }
}

pub fn snapshot(limit: usize) -> Vec<TraceEntry> {
    TRACES
        .read()
        .map(|q| q.iter().rev().take(limit).cloned().collect())
        .unwrap_or_default()
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

/// 测试用: 清空 buffer
#[cfg(test)]
pub fn clear() {
    if let Ok(mut q) = TRACES.write() {
        q.clear();
    }
}
