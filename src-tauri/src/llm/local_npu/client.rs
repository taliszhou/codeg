// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// M7: 用 tokio::net::UnixStream 直接读写 HTTP/1.1 over Unix socket。
// 不引入 hyperlocal 等专门库 (协议简单，手写更轻量 + 没多余依赖)。

use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::types::{
    AdminLoadRequest, AdminLoadResponse, AdminUnloadResponse, ChatCompletionChunk,
    ChatCompletionRequest, ChatCompletionResponse, LocalNpuError, ModelsListResponse,
};

pub struct LocalNpuClient {
    addr: String,
}

impl LocalNpuClient {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
        }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    async fn connect(&self) -> Result<TcpStream, LocalNpuError> {
        TcpStream::connect(&self.addr)
            .await
            .map_err(|e| LocalNpuError::SocketConnect(format!("{}: {e}", self.addr)))
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    pub async fn list_models(&self) -> Result<ModelsListResponse, LocalNpuError> {
        let stream = self.connect().await?;
        let raw = http_request(stream, "GET", "/v1/models", None).await?;
        parse_json_body(raw)
    }

    pub async fn admin_load(&self, model: &str) -> Result<AdminLoadResponse, LocalNpuError> {
        let stream = self.connect().await?;
        let body = serde_json::to_vec(&AdminLoadRequest { model: model.into() })?;
        let raw = http_request(stream, "POST", "/v1/admin/load", Some(&body)).await?;
        parse_json_body(raw)
    }

    pub async fn admin_unload(&self) -> Result<AdminUnloadResponse, LocalNpuError> {
        let stream = self.connect().await?;
        let raw = http_request(stream, "POST", "/v1/admin/unload", Some(b"{}")).await?;
        parse_json_body(raw)
    }

    pub async fn chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, LocalNpuError> {
        let mut req2 = req.clone();
        req2.stream = Some(false);
        let stream = self.connect().await?;
        let body = serde_json::to_vec(&req2)?;
        let raw = http_request(stream, "POST", "/v1/chat/completions", Some(&body)).await?;
        parse_json_body(raw)
    }

    /// 流式 chat：返回一个 mpsc Receiver，每个 chunk 是一个 ChatCompletionChunk
    /// (最后跟着一个 None 表示 [DONE])。
    pub async fn chat_completion_stream(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<mpsc::Receiver<Result<Option<ChatCompletionChunk>, LocalNpuError>>, LocalNpuError>
    {
        let mut req2 = req.clone();
        req2.stream = Some(true);
        let stream = self.connect().await?;
        let body = serde_json::to_vec(&req2)?;
        let (tx, rx) = mpsc::channel(32);
        tokio::spawn(async move {
            let result = stream_sse(stream, "POST", "/v1/chat/completions", Some(&body), tx.clone()).await;
            if let Err(e) = result {
                let _ = tx.send(Err(e)).await;
            }
        });
        Ok(rx)
    }
}

// ----------------------------------------------------------------------
// Internal: minimal HTTP/1.1 client over given stream
// ----------------------------------------------------------------------

struct HttpResponseRaw {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn http_request(
    stream: TcpStream,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<HttpResponseRaw, LocalNpuError> {
    let (rh, mut wh) = stream.into_split();

    // Build request
    let body_bytes = body.unwrap_or(b"");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {len}\r\nConnection: close\r\n",
        method = method,
        path = path,
        len = body_bytes.len()
    );
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str("\r\n");

    wh.write_all(req.as_bytes()).await?;
    if !body_bytes.is_empty() {
        wh.write_all(body_bytes).await?;
    }
    wh.flush().await?;
    drop(wh);

    // Read response
    let mut reader = BufReader::new(rh);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).await?;
    let status = parse_status_line(&status_line)?;

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(colon) = line.find(':') {
            let k = line[..colon].trim().to_lowercase();
            let v = line[colon + 1..].trim().to_string();
            headers.push((k, v));
        }
    }

    let content_length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok());

    let body = match content_length {
        Some(n) => {
            let mut buf = vec![0u8; n];
            reader.read_exact(&mut buf).await?;
            buf
        }
        None => {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).await?;
            buf
        }
    };

    Ok(HttpResponseRaw { status, headers, body })
}

async fn stream_sse(
    stream: TcpStream,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    tx: mpsc::Sender<Result<Option<ChatCompletionChunk>, LocalNpuError>>,
) -> Result<(), LocalNpuError> {
    let (rh, mut wh) = stream.into_split();
    let body_bytes = body.unwrap_or(b"");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\nContent-Length: {len}\r\nConnection: close\r\n",
        method = method, path = path, len = body_bytes.len()
    );
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str("\r\n");
    wh.write_all(req.as_bytes()).await?;
    if !body_bytes.is_empty() {
        wh.write_all(body_bytes).await?;
    }
    wh.flush().await?;
    drop(wh);

    let mut reader = BufReader::new(rh);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).await?;
    let status = parse_status_line(&status_line)?;

    // 吞 headers
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    if !(200..300).contains(&status) {
        // 非 200 直接读 body 当错误返回
        let mut err = String::new();
        let _ = reader.read_to_string(&mut err).await;
        return Err(LocalNpuError::Server {
            status,
            message: err,
        });
    }

    // SSE body: 每个 event 是 `data: <line>\n\n`
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(|c| c == '\n' || c == '\r');
        if trimmed.is_empty() {
            continue;
        }
        let payload = trimmed.strip_prefix("data: ").unwrap_or(trimmed);
        if payload == "[DONE]" {
            let _ = tx.send(Ok(None)).await;
            break;
        }
        match serde_json::from_str::<ChatCompletionChunk>(payload) {
            Ok(chunk) => {
                if tx.send(Ok(Some(chunk))).await.is_err() {
                    // receiver dropped
                    break;
                }
            }
            Err(e) => {
                // 跳过解析失败的帧但记录
                let _ = tx.send(Err(LocalNpuError::Json(e))).await;
            }
        }
    }
    Ok(())
}

fn parse_status_line(line: &str) -> Result<u16, LocalNpuError> {
    // Expected: "HTTP/1.1 200 OK\r\n"
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(LocalNpuError::BadResponse(format!("bad status line: {line:?}")));
    }
    parts[1]
        .parse::<u16>()
        .map_err(|_| LocalNpuError::BadResponse(format!("bad status code: {}", parts[1])))
}

fn parse_json_body<T: DeserializeOwned>(raw: HttpResponseRaw) -> Result<T, LocalNpuError> {
    if !(200..300).contains(&raw.status) {
        let msg = String::from_utf8_lossy(&raw.body).to_string();
        let mapped = if raw.status == 503 && msg.contains("model_not_loaded") {
            LocalNpuError::ModelNotLoaded
        } else if raw.status == 503 && msg.contains("npu_busy") {
            LocalNpuError::NpuBusy
        } else if raw.status == 404 && msg.contains("model_file_missing") {
            LocalNpuError::ModelFileMissing(msg)
        } else {
            LocalNpuError::Server {
                status: raw.status,
                message: msg,
            }
        };
        return Err(mapped);
    }
    Ok(serde_json::from_slice(&raw.body)?)
}
