// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// HTTP 反向代理。把 axum 收到的 Request 转发到 upstream URL,流式回 Response。

use std::time::Instant;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use reqwest::Client;

use super::error::ProxyError;
use super::trace::{self, TraceEntry};

#[derive(Default)]
pub struct ForwardOptions<'a> {
    pub bearer_token: Option<&'a str>,
    pub trace_label: &'static str,
}

/// hop-by-hop headers that MUST NOT be forwarded (RFC 7230 §6.1) +
/// `authorization` 因为我们要用 upstream-specific 的 bearer token 覆盖
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
    // 不要把 client 的 Bearer (本机 token) 透传, 用 ForwardOptions.bearer_token 替换
    "authorization",
    "cookie",
];

fn is_hop_by_hop(name: &HeaderName) -> bool {
    let s = name.as_str().to_ascii_lowercase();
    HOP_BY_HOP.iter().any(|h| *h == s)
}

fn filter_request_headers(orig: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in orig {
        if !is_hop_by_hop(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn filter_response_headers(orig: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in orig {
        let name = match HeaderName::from_bytes(k.as_str().as_bytes()) {
            Ok(n) if !is_hop_by_hop(&n) => n,
            _ => continue,
        };
        if let Ok(value) = HeaderValue::from_bytes(v.as_bytes()) {
            out.append(name, value);
        }
    }
    out
}

/// 把 axum 收到的 Request 转发到 upstream。
/// - 复制 method/headers/body
/// - 如设 bearer_token, 覆盖 Authorization 头
/// - 流式返回 Response
pub async fn forward_http(
    req: Request,
    upstream: &str,
    opts: &ForwardOptions<'_>,
) -> Result<Response, ProxyError> {
    let started = Instant::now();
    let method = req.method().clone();
    let method_clone = method.to_string();

    // 1. 取出 headers 和 body
    let headers = filter_request_headers(req.headers());
    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| ProxyError::UpstreamConnect(format!("read body: {e}")))?;
    let bytes_in = body_bytes.len() as u64;

    // 2. 构造 reqwest Client + Request
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ProxyError::UpstreamConnect(format!("client build: {e}")))?;

    let method_for_reqwest = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|e| ProxyError::InvalidUpstream(format!("bad method: {e}")))?;
    let mut builder = client.request(method_for_reqwest, upstream).body(body_bytes);

    // 3. 转发 headers (过滤 hop-by-hop)
    for (k, v) in &headers {
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()) {
            if let Ok(value) = reqwest::header::HeaderValue::from_bytes(v.as_bytes()) {
                builder = builder.header(name, value);
            }
        }
    }

    // 4. 注入 Bearer token (覆盖 client 原有的 Authorization)
    if let Some(token) = opts.bearer_token {
        builder = builder.bearer_auth(token);
    }

    // 5. 发请求
    let trace_label = opts.trace_label;
    let upstream_str = upstream.to_string();
    let send_result = builder.send().await;

    let resp = match send_result {
        Ok(r) => r,
        Err(e) => {
            trace::record(TraceEntry {
                ts_ms: trace::now_ms(),
                label: trace_label,
                method: method_clone,
                upstream: upstream_str,
                status: None,
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in,
                bytes_out: 0,
                error: Some(e.to_string()),
            });
            return Err(ProxyError::UpstreamConnect(e.to_string()));
        }
    };

    let status_code = resp.status().as_u16();
    let resp_headers = filter_response_headers(resp.headers());

    // 6. 流式 body 回客户端 — 用 reqwest::Response::bytes_stream() → axum::body::Body::from_stream
    let stream = resp.bytes_stream();
    let body = Body::from_stream(stream);

    let mut builder = Response::builder().status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
    for (k, v) in &resp_headers {
        builder = builder.header(k, v);
    }
    let response = builder.body(body).map_err(|e| ProxyError::UpstreamConnect(format!("response build: {e}")))?;

    trace::record(TraceEntry {
        ts_ms: trace::now_ms(),
        label: trace_label,
        method: method_clone,
        upstream: upstream_str,
        status: Some(status_code),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in,
        bytes_out: 0, // 流式 body 时无法预知,留 0
        error: None,
    });

    Ok(response)
}

/// 判断 axum Request 是否为 WebSocket upgrade。调用方据此决定走 forward_http 还是 pipe_websocket。
pub fn is_websocket_upgrade(req: &Request) -> bool {
    if req.method() != Method::GET {
        return false;
    }
    let upgrade = req
        .headers()
        .get(axum::http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    upgrade.eq_ignore_ascii_case("websocket")
}
