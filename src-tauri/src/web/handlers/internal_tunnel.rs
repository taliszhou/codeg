// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Phase 4b 远端 endpoint: 接收源 codeg-server forward proxy 经 C1 链路过来的
// raw TCP 流量, 用本机网络连真正 upstream 后双向 binary frame pipe。

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Query;
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::proxy::trace::{now_ms, record, TraceEntry};

#[derive(Deserialize)]
pub struct TunnelParams {
    pub host: String,
    pub port: u16,
}

pub async fn internal_tunnel(
    Query(params): Query<TunnelParams>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| async move {
        handle_tunnel(socket, params.host, params.port).await;
    })
}

async fn handle_tunnel(mut ws: WebSocket, host: String, port: u16) {
    let started = std::time::Instant::now();
    let upstream_repr = format!("{host}:{port}");

    let upstream = match TcpStream::connect((host.as_str(), port)).await {
        Ok(s) => s,
        Err(e) => {
            record(TraceEntry {
                ts_ms: now_ms(),
                label: "tunnel",
                method: "TUNNEL".into(),
                upstream: upstream_repr,
                status: Some(502),
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in: 0,
                bytes_out: 0,
                error: Some(e.to_string()),
            });
            let _ = ws.send(Message::Close(None)).await;
            return;
        }
    };

    let (mut up_r, mut up_w) = upstream.into_split();
    // axum ws split — 用 futures::StreamExt::split
    let (mut ws_sink, mut ws_stream) = ws.split();

    let ws_to_up = async move {
        let mut total = 0u64;
        while let Some(msg) = ws_stream.next().await {
            let Ok(msg) = msg else { break };
            match msg {
                Message::Binary(b) => {
                    if up_w.write_all(&b).await.is_err() { break; }
                    total += b.len() as u64;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = up_w.shutdown().await;
        total
    };
    let up_to_ws = async move {
        let mut buf = [0u8; 16 * 1024];
        let mut total = 0u64;
        loop {
            let n = up_r.read(&mut buf).await.unwrap_or(0);
            if n == 0 { break; }
            if ws_sink.send(Message::Binary(buf[..n].to_vec().into())).await.is_err() { break; }
            total += n as u64;
        }
        let _ = ws_sink.send(Message::Close(None)).await;
        total
    };

    let (bytes_in, bytes_out) = tokio::join!(ws_to_up, up_to_ws);

    record(TraceEntry {
        ts_ms: now_ms(),
        label: "tunnel",
        method: "TUNNEL".into(),
        upstream: upstream_repr,
        status: Some(200),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in,
        bytes_out,
        error: None,
    });
}

// HTTP 模式: 接收上游 codeg 转发的 absolute-URI 请求, 在本机出口 reqwest fetch。
use axum::body::Body;
use axum::http::StatusCode;

#[derive(Deserialize)]
pub struct HttpProxyParams {
    pub method: String,
    pub url: String,
}

pub async fn internal_http_proxy(
    Query(params): Query<HttpProxyParams>,
    body: axum::body::Bytes,
) -> Response {
    let method = match reqwest::Method::from_bytes(params.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "bad method"),
    };
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "client"),
    };
    let resp = match client.request(method, &params.url).body(body.to_vec()).send().await {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &format!("{e}")),
    };
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.unwrap_or_default();
    let mut builder = Response::builder().status(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY));
    for (k, v) in headers.iter() {
        let name = k.as_str().to_ascii_lowercase();
        if matches!(name.as_str(), "content-length" | "content-encoding" | "transfer-encoding" | "connection") {
            continue;
        }
        if let Ok(value) = axum::http::HeaderValue::from_bytes(v.as_bytes()) {
            builder = builder.header(k.as_str(), value);
        }
    }
    builder.body(Body::from(body)).unwrap_or_else(|_| {
        Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR).body(Body::empty()).unwrap()
    })
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    Response::builder().status(status).body(Body::from(msg.to_string())).unwrap()
}
