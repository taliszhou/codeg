// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// WebSocket pipe: 本机 axum WebSocket ↔ 远端 tokio-tungstenite client。
//
// 用法: 在 axum handler 里用 WebSocketUpgrade::on_upgrade 拿到 WebSocket,然后
// 调 pipe_websocket(socket, upstream_url, token) 让两边双向流。

use std::time::Instant;

use axum::extract::ws::{Message as AxumMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        protocol::Message as TgMessage,
    },
};

use super::error::ProxyError;
use super::trace::{self, TraceEntry};

/// 双向 pipe: axum 端的 WebSocket ↔ 远端 ws://... 的 client。
/// `auth_header` 如果非 None,会作为 Authorization 头加到 upgrade 请求。
pub async fn pipe_websocket(
    client_ws: WebSocket,
    upstream_url: String,
    auth_header: Option<String>,
    trace_label: &'static str,
) -> Result<(), ProxyError> {
    let started = Instant::now();
    let upstream_for_trace = upstream_url.clone();

    // 1. 连远端
    let mut req = upstream_url
        .clone()
        .into_client_request()
        .map_err(|e| ProxyError::WsUpgrade(format!("upstream url: {e}")))?;
    if let Some(auth) = auth_header.as_ref() {
        req.headers_mut().insert(
            "Authorization",
            auth.parse()
                .map_err(|e| ProxyError::WsUpgrade(format!("bad auth header: {e}")))?,
        );
    }

    let (upstream_ws, _resp) = connect_async(req).await.map_err(|e| {
        let err = ProxyError::WsUpgrade(format!("connect upstream: {e}"));
        trace::record(TraceEntry {
            ts_ms: trace::now_ms(),
            label: trace_label,
            method: "WS".into(),
            upstream: upstream_for_trace.clone(),
            status: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            bytes_in: 0,
            bytes_out: 0,
            error: Some(err.to_string()),
        });
        err
    })?;

    let (mut up_sink, mut up_stream) = upstream_ws.split();
    let (mut cl_sink, mut cl_stream) = client_ws.split();

    let (close_tx, mut close_rx) = mpsc::channel::<()>(2);

    // 2. client → upstream
    let close_tx_a = close_tx.clone();
    let c2u = tokio::spawn(async move {
        while let Some(msg) = cl_stream.next().await {
            let Ok(msg) = msg else { break };
            let tg = match msg {
                AxumMessage::Text(t) => TgMessage::Text(t.to_string().into()),
                AxumMessage::Binary(b) => TgMessage::Binary(b),
                AxumMessage::Ping(p) => TgMessage::Ping(p),
                AxumMessage::Pong(p) => TgMessage::Pong(p),
                AxumMessage::Close(_) => {
                    let _ = up_sink.send(TgMessage::Close(None)).await;
                    break;
                }
            };
            if up_sink.send(tg).await.is_err() {
                break;
            }
        }
        let _ = close_tx_a.send(()).await;
    });

    // 3. upstream → client
    let close_tx_b = close_tx.clone();
    let u2c = tokio::spawn(async move {
        while let Some(msg) = up_stream.next().await {
            let Ok(msg) = msg else { break };
            let ax = match msg {
                TgMessage::Text(t) => AxumMessage::Text(t.to_string().into()),
                TgMessage::Binary(b) => AxumMessage::Binary(b),
                TgMessage::Ping(p) => AxumMessage::Ping(p),
                TgMessage::Pong(p) => AxumMessage::Pong(p),
                TgMessage::Close(_) => {
                    let _ = cl_sink.send(AxumMessage::Close(None)).await;
                    break;
                }
                TgMessage::Frame(_) => continue,
            };
            if cl_sink.send(ax).await.is_err() {
                break;
            }
        }
        let _ = close_tx_b.send(()).await;
    });

    let _ = close_rx.recv().await; // 任一方向关闭就一起退
    c2u.abort();
    u2c.abort();

    trace::record(TraceEntry {
        ts_ms: trace::now_ms(),
        label: trace_label,
        method: "WS".into(),
        upstream: upstream_for_trace,
        status: Some(101),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in: 0,
        bytes_out: 0,
        error: None,
    });

    Ok(())
}
