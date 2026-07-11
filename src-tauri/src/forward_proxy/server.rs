// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// HTTP forward proxy 监听器 (Phase 3 / C3 形态 A)。
//
// 协议:
//   - 普通 HTTP: client 发 `GET http://example.com/path HTTP/1.1` (absolute-URI),
//     proxy 用 reqwest 转发, 写回完整 response。
//   - HTTPS / 任意 TCP: client 发 `CONNECT example.com:443 HTTP/1.1`,
//     proxy 连 upstream, 回 `200 Connection Established`, 之后 raw TCP 双向 pipe。
//   - 鉴权: Proxy-Authorization: Bearer <token>。错则 407 + Proxy-Authenticate。

use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::proxy::trace::{now_ms, record, TraceEntry};

pub struct ForwardProxyConfig {
    pub listen_port: u16,
    pub token: String,
    /// Phase 4b: 跳板 device (base_url + token), None = 本机直接出口
    pub upstream_device: Option<UpstreamDevice>,
}

#[derive(Clone)]
pub struct UpstreamDevice {
    pub base_url: String,
    pub token: String,
}

pub async fn run_forward_proxy(
    config: ForwardProxyConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let addr = format!("0.0.0.0:{}", config.listen_port);
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[FWD-PROXY] bind {addr} failed: {e}");
            return;
        }
    };
    eprintln!(
        "[FWD-PROXY] listening on {addr} (upstream: {})",
        config
            .upstream_device
            .as_ref()
            .map(|u| u.base_url.as_str())
            .unwrap_or("local")
    );
    let token = std::sync::Arc::new(config.token);
    let upstream = std::sync::Arc::new(config.upstream_device);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    eprintln!("[FWD-PROXY] shutdown");
                    return;
                }
            }
            accept = listener.accept() => {
                let (sock, peer) = match accept {
                    Ok(v) => v,
                    Err(e) => { eprintln!("[FWD-PROXY] accept: {e}"); continue; }
                };
                let tok = token.clone();
                let up = upstream.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_client(sock, peer.to_string(), tok, up).await {
                        eprintln!("[FWD-PROXY] {peer} error: {e}");
                    }
                });
            }
        }
    }
}

#[derive(Debug)]
struct RequestHead {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    /// head 之后已经从 socket 读到 buffer 但属于 body 的字节
    body_prefix: Vec<u8>,
}

/// 从 sock 读 HTTP request head, 用 4KB chunk 累积, 找到 `\r\n\r\n` 分界。
/// head 里 hold 着剩余 body bytes (普通 HTTP 时和 Content-Length 配合; CONNECT 时一般为空)
async fn read_request_head(sock: &mut TcpStream) -> std::io::Result<RequestHead> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let head_end_idx;
    loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "client closed before complete request",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_double_crlf(&buf) {
            head_end_idx = pos + 4;
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(std::io::Error::other("request head too large"));
        }
    }
    let (head_bytes, rest) = buf.split_at(head_end_idx);
    let head_str = std::str::from_utf8(head_bytes)
        .map_err(|e| std::io::Error::other(format!("non-utf8 head: {e}")))?;
    let mut lines = head_str.split("\r\n");
    let start = lines.next().unwrap_or("");
    let mut parts = start.splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let _version = parts.next().unwrap_or("");
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(RequestHead {
        method,
        target,
        headers,
        body_prefix: rest.to_vec(),
    })
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn check_auth(headers: &[(String, String)], expected: &str) -> bool {
    let Some(raw) = header_value(headers, "Proxy-Authorization") else {
        return false;
    };
    let raw = raw.trim();
    if let Some(t) = raw.strip_prefix("Bearer ") {
        return t.trim() == expected;
    }
    if let Some(b64) = raw.strip_prefix("Basic ") {
        use base64::Engine;
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
            return false;
        };
        let Ok(s) = std::str::from_utf8(&decoded) else {
            return false;
        };
        if let Some((_, pass)) = s.split_once(':') {
            return pass == expected;
        }
    }
    false
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}

async fn handle_client(
    mut sock: TcpStream,
    peer: String,
    token: std::sync::Arc<String>,
    upstream: std::sync::Arc<Option<UpstreamDevice>>,
) -> std::io::Result<()> {
    let started = Instant::now();
    let head = match read_request_head(&mut sock).await {
        Ok(h) => h,
        Err(e) => {
            let _ = sock
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Err(e);
        }
    };
    eprintln!("[FWD-PROXY] {peer} → {} {}", head.method, head.target);

    if !check_auth(&head.headers, token.as_str()) {
        record(TraceEntry {
            ts_ms: now_ms(),
            label: "forward-proxy",
            method: head.method.clone(),
            upstream: head.target.clone(),
            status: Some(407),
            elapsed_ms: started.elapsed().as_millis() as u64,
            bytes_in: 0,
            bytes_out: 0,
            error: Some("auth invalid".into()),
        });
        let _ = sock
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                 Proxy-Authenticate: Bearer\r\n\
                 Content-Length: 0\r\n\
                 Connection: close\r\n\r\n",
            )
            .await;
        return Ok(());
    }

    if head.method.eq_ignore_ascii_case("CONNECT") {
        match upstream.as_ref() {
            Some(up) => handle_connect_via_device(sock, head, started, up).await,
            None => handle_connect(sock, head, started).await,
        }
    } else {
        match upstream.as_ref() {
            Some(up) => handle_http_via_device(sock, head, started, up).await,
            None => handle_http_proxy(sock, head, started).await,
        }
    }
}

async fn handle_connect(
    mut sock: TcpStream,
    head: RequestHead,
    started: Instant,
) -> std::io::Result<()> {
    let Some((host, port)) = head.target.rsplit_once(':') else {
        let _ = sock
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            .await;
        return Ok(());
    };
    let port: u16 = match port.parse() {
        Ok(p) => p,
        Err(_) => {
            let _ = sock
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Ok(());
        }
    };

    let upstream = match TcpStream::connect((host, port)).await {
        Ok(s) => s,
        Err(e) => {
            record(TraceEntry {
                ts_ms: now_ms(),
                label: "forward-proxy",
                method: "CONNECT".into(),
                upstream: head.target.clone(),
                status: Some(502),
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in: 0,
                bytes_out: 0,
                error: Some(e.to_string()),
            });
            let _ = sock
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Ok(());
        }
    };

    // 通知 client tunnel 已建立
    sock.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    sock.flush().await?;

    let (mut client_r, mut client_w) = sock.into_split();
    let (mut up_r, mut up_w) = upstream.into_split();

    // 如果 read_request_head 多读了一些字节(理论上 CONNECT 不该有),先发出去
    if !head.body_prefix.is_empty() {
        up_w.write_all(&head.body_prefix).await?;
    }

    let c2u = async move {
        let r = tokio::io::copy(&mut client_r, &mut up_w).await;
        let _ = up_w.shutdown().await;
        r.unwrap_or(0)
    };
    let u2c = async move {
        let r = tokio::io::copy(&mut up_r, &mut client_w).await;
        let _ = client_w.shutdown().await;
        r.unwrap_or(0)
    };
    let (bytes_in, bytes_out) = tokio::join!(c2u, u2c);

    record(TraceEntry {
        ts_ms: now_ms(),
        label: "forward-proxy",
        method: "CONNECT".into(),
        upstream: head.target,
        status: Some(200),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in,
        bytes_out,
        error: None,
    });
    Ok(())
}

// ============================================================================
// Phase 4b: 经远端 device 跳板
// ============================================================================

/// CONNECT 模式: 在远端 device 上代为建立 TCP 到 upstream, raw 双向 pipe via WS。
async fn handle_connect_via_device(
    mut sock: TcpStream,
    head: RequestHead,
    started: Instant,
    upstream: &UpstreamDevice,
) -> std::io::Result<()> {
    let Some((host, port_str)) = head.target.rsplit_once(':') else {
        let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
        return Ok(());
    };
    let Ok(port): Result<u16, _> = port_str.parse() else {
        let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
        return Ok(());
    };

    // 建到 device 的 WS: ws[s]://device-base/api/_internal_tunnel?host=...&port=...&token=...
    let ws_url = build_tunnel_url(&upstream.base_url, host, port, &upstream.token);
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TgMsg;
    let connect_result = tokio_tungstenite::connect_async(&ws_url).await;
    let ws_stream = match connect_result {
        Ok((s, _)) => s,
        Err(e) => {
            record(TraceEntry {
                ts_ms: now_ms(),
                label: "forward-proxy",
                method: "CONNECT".into(),
                upstream: format!("{} (via {})", head.target, upstream.base_url),
                status: Some(502),
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in: 0,
                bytes_out: 0,
                error: Some(format!("tunnel dial: {e}")),
            });
            let _ = sock.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
            return Ok(());
        }
    };

    sock.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    sock.flush().await?;

    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    let (mut client_r, mut client_w) = sock.into_split();

    // 如果 read_request_head 多读了 body, prepend
    if !head.body_prefix.is_empty() {
        if ws_sink.send(TgMsg::Binary(head.body_prefix.clone().into())).await.is_err() {
            return Ok(());
        }
    }

    let target_repr = format!("{} (via {})", head.target, upstream.base_url);
    let target_for_trace = target_repr.clone();

    let c2u = async move {
        let mut buf = [0u8; 16 * 1024];
        let mut total = 0u64;
        loop {
            let n = client_r.read(&mut buf).await.unwrap_or(0);
            if n == 0 { break; }
            if ws_sink.send(TgMsg::Binary(buf[..n].to_vec().into())).await.is_err() { break; }
            total += n as u64;
        }
        let _ = ws_sink.send(TgMsg::Close(None)).await;
        total
    };
    let u2c = async move {
        let mut total = 0u64;
        while let Some(msg) = ws_stream.next().await {
            let Ok(msg) = msg else { break };
            match msg {
                TgMsg::Binary(b) => {
                    if client_w.write_all(&b).await.is_err() { break; }
                    total += b.len() as u64;
                }
                TgMsg::Close(_) => break,
                _ => {}
            }
        }
        let _ = client_w.shutdown().await;
        total
    };

    let (bytes_in, bytes_out) = tokio::join!(c2u, u2c);
    record(TraceEntry {
        ts_ms: now_ms(),
        label: "forward-proxy",
        method: "CONNECT".into(),
        upstream: target_for_trace,
        status: Some(200),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in,
        bytes_out,
        error: None,
    });
    Ok(())
}

/// 普通 HTTP 模式: 走 reqwest 经远端 device 的 _internal_http_proxy
async fn handle_http_via_device(
    mut sock: TcpStream,
    head: RequestHead,
    started: Instant,
    upstream: &UpstreamDevice,
) -> std::io::Result<()> {
    if !head.target.starts_with("http://") && !head.target.starts_with("https://") {
        let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n").await;
        return Ok(());
    }
    let content_length: usize = header_value(&head.headers, "Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = head.body_prefix.clone();
    while body.len() < content_length {
        let mut tmp = [0u8; 4096];
        let need = content_length - body.len();
        let n = sock.read(&mut tmp[..need.min(4096)]).await?;
        if n == 0 { break; }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    let endpoint = format!(
        "{}/api/_internal_http_proxy?token={}&method={}&url={}",
        upstream.base_url.trim_end_matches('/'),
        urlencoding::encode(&upstream.token),
        urlencoding::encode(&head.method),
        urlencoding::encode(&head.target)
    );

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = sock.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
            return Err(std::io::Error::other(e.to_string()));
        }
    };
    let mut builder = client.post(&endpoint).body(body);
    for (k, v) in &head.headers {
        if is_hop_by_hop(k) || k.eq_ignore_ascii_case("authorization") { continue; }
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(format!("x-fwd-{}", k).as_bytes()) {
            if let Ok(value) = reqwest::header::HeaderValue::from_bytes(v.as_bytes()) {
                builder = builder.header(name, value);
            }
        }
    }
    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            record(TraceEntry {
                ts_ms: now_ms(),
                label: "forward-proxy",
                method: head.method.clone(),
                upstream: format!("{} (via {})", head.target, upstream.base_url),
                status: None,
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in: content_length as u64,
                bytes_out: 0,
                error: Some(e.to_string()),
            });
            let _ = sock.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await;
            return Ok(());
        }
    };
    let status = resp.status();
    let resp_headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(k, _)| !is_hop_by_hop(k.as_str()))
        .filter(|(k, _)| !k.as_str().starts_with("x-fwd-"))
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = resp.bytes().await.unwrap_or_default();
    sock.write_all(format!("HTTP/1.1 {} {}\r\n", status.as_u16(), status.canonical_reason().unwrap_or("")).as_bytes()).await?;
    for (k, v) in &resp_headers {
        sock.write_all(format!("{k}: {v}\r\n").as_bytes()).await?;
    }
    sock.write_all(format!("Content-Length: {}\r\n", body.len()).as_bytes()).await?;
    sock.write_all(b"Connection: close\r\n\r\n").await?;
    sock.write_all(&body).await?;
    sock.flush().await?;

    record(TraceEntry {
        ts_ms: now_ms(),
        label: "forward-proxy",
        method: head.method,
        upstream: format!("{} (via {})", head.target, upstream.base_url),
        status: Some(status.as_u16()),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in: content_length as u64,
        bytes_out: body.len() as u64,
        error: None,
    });
    Ok(())
}

fn build_tunnel_url(base_url: &str, host: &str, port: u16, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = base
        .replacen("http://", "ws://", 1)
        .replacen("https://", "wss://", 1);
    format!(
        "{}/api/_internal_tunnel?host={}&port={}&token={}",
        ws_base,
        urlencoding::encode(host),
        port,
        urlencoding::encode(token)
    )
}

async fn handle_http_proxy(
    mut sock: TcpStream,
    head: RequestHead,
    started: Instant,
) -> std::io::Result<()> {
    if !head.target.starts_with("http://") && !head.target.starts_with("https://") {
        let _ = sock
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            .await;
        return Ok(());
    }

    let content_length: usize = header_value(&head.headers, "Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = head.body_prefix.clone();
    while body.len() < content_length {
        let mut tmp = [0u8; 4096];
        let need = content_length - body.len();
        let n = sock.read(&mut tmp[..need.min(4096)]).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = sock
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Err(std::io::Error::other(e.to_string()));
        }
    };
    let method = reqwest::Method::from_bytes(head.method.as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let mut builder = client.request(method, &head.target).body(body);
    for (k, v) in &head.headers {
        if is_hop_by_hop(k) {
            continue;
        }
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(k.as_bytes()) {
            if let Ok(value) = reqwest::header::HeaderValue::from_bytes(v.as_bytes()) {
                builder = builder.header(name, value);
            }
        }
    }
    let resp = match builder.send().await {
        Ok(r) => r,
        Err(e) => {
            record(TraceEntry {
                ts_ms: now_ms(),
                label: "forward-proxy",
                method: head.method.clone(),
                upstream: head.target.clone(),
                status: None,
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in: content_length as u64,
                bytes_out: 0,
                error: Some(e.to_string()),
            });
            let _ = sock
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return Ok(());
        }
    };

    let status = resp.status();
    let resp_headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(k, _)| !is_hop_by_hop(k.as_str()))
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = resp.bytes().await.unwrap_or_default();

    let status_line = format!(
        "HTTP/1.1 {} {}\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );
    sock.write_all(status_line.as_bytes()).await?;
    for (k, v) in &resp_headers {
        sock.write_all(format!("{k}: {v}\r\n").as_bytes()).await?;
    }
    sock.write_all(format!("Content-Length: {}\r\n", body.len()).as_bytes())
        .await?;
    sock.write_all(b"Connection: close\r\n\r\n").await?;
    sock.write_all(&body).await?;
    sock.flush().await?;

    record(TraceEntry {
        ts_ms: now_ms(),
        label: "forward-proxy",
        method: head.method,
        upstream: head.target,
        status: Some(status.as_u16()),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in: content_length as u64,
        bytes_out: body.len() as u64,
        error: None,
    });
    Ok(())
}
