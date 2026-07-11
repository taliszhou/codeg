// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// CONNECT tunnel: 接受 HTTP CONNECT method, hijack TCP, 双向 pipe 到 upstream:port。
// 用于 C3 forward proxy 形态 A。
//
// 注: 这个 module 在 Phase 3 完成,Phase 1 只放骨架。
//
// 实施时关键点:
// 1. 在独立的 :8118 listener (不是主 :8080 axum server) 上处理
// 2. 收到 "CONNECT host:port HTTP/1.1\r\n..." 后 read headers, 检查 Proxy-Authorization
// 3. 回 "HTTP/1.1 200 Connection Established\r\n\r\n"
// 4. tokio::io::copy_bidirectional(client, upstream)

use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::error::ProxyError;
use super::trace::{self, TraceEntry};

/// 接管一个已收到 CONNECT 行 + headers 的 client TCP socket,连 upstream host:port,双向 pipe。
/// 调用前必须已校验过 Proxy-Authorization,本函数不做鉴权。
pub async fn tunnel_connect<S>(
    mut client: S,
    host: &str,
    port: u16,
    trace_label: &'static str,
) -> Result<(), ProxyError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let started = Instant::now();
    let upstream_repr = format!("{host}:{port}");

    // 连 upstream
    let upstream = match TcpStream::connect((host, port)).await {
        Ok(s) => s,
        Err(e) => {
            let err = ProxyError::UpstreamConnect(format!("{host}:{port}: {e}"));
            // Send 502 to client
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            trace::record(TraceEntry {
                ts_ms: trace::now_ms(),
                label: trace_label,
                method: "CONNECT".into(),
                upstream: upstream_repr,
                status: Some(502),
                elapsed_ms: started.elapsed().as_millis() as u64,
                bytes_in: 0,
                bytes_out: 0,
                error: Some(err.to_string()),
            });
            return Err(err);
        }
    };

    // Tell client tunnel is established
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    client.flush().await?;

    // Bidirectional pipe
    let (mut client_r, mut client_w) = tokio::io::split(client);
    let (mut up_r, mut up_w) = tokio::io::split(upstream);

    let c2u = async {
        let mut buf = [0u8; 16 * 1024];
        let mut total = 0u64;
        loop {
            let n = client_r.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            if up_w.write_all(&buf[..n]).await.is_err() {
                break;
            }
            total += n as u64;
        }
        let _ = up_w.shutdown().await;
        total
    };
    let u2c = async {
        let mut buf = [0u8; 16 * 1024];
        let mut total = 0u64;
        loop {
            let n = up_r.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            if client_w.write_all(&buf[..n]).await.is_err() {
                break;
            }
            total += n as u64;
        }
        let _ = client_w.shutdown().await;
        total
    };

    let (bytes_in, bytes_out) = tokio::join!(c2u, u2c);

    trace::record(TraceEntry {
        ts_ms: trace::now_ms(),
        label: trace_label,
        method: "CONNECT".into(),
        upstream: upstream_repr,
        status: Some(200),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_in,
        bytes_out,
        error: None,
    });
    Ok(())
}
