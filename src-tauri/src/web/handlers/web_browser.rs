// Copyright (C) 2026 taliszhou
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Phase 4a / 内置浏览器: 反代任意 URL 到 iframe, 重写 HTML/CSS 里所有链接 让 iframe 一直
// 留在 codeg-server 域 (绕过 X-Frame-Options / 单源策略)。
//
// 入口: GET /api/browse?url=<encoded>&via=<device_id?>
//   - 经本机出口 (via 缺省) 直接 fetch <url>
//   - 经远端 device (via=N) 透过 C1 链路调远端的 /api/_internal_browse?url=...
//
// 响应:
//   - Content-Type: text/html  → lol_html 重写, 改 <a/img/script/link 等所有 url
//   - Content-Type: text/css   → 正则重写 url(...)
//   - 其他 (image/js/font/...) → 透传
// 头处理:
//   - 剥 X-Frame-Options / Content-Security-Policy (允许嵌入 iframe)
//   - 剥 Set-Cookie 的 Secure / SameSite (因为我们不是 https)

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Extension, Query, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use lol_html::{element, HtmlRewriter, Settings};
use serde::Deserialize;

use crate::app_error::{AppCommandError, AppErrorCode};
use crate::app_state::AppState;
use crate::db::service::remote_device_service;

#[derive(Deserialize)]
pub struct BrowseParams {
    /// 目标 URL (绝对 URL, 含 scheme)
    pub url: String,
    /// 可选: 经过哪台远程 device 出口。None = 本机
    pub via: Option<i32>,
}

pub async fn browse(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<BrowseParams>,
    req: Request,
) -> Result<Response, AppCommandError> {
    let target_url = params.url.clone();
    if !target_url.starts_with("http://") && !target_url.starts_with("https://") {
        return Err(AppCommandError::invalid_input(
            "browse url must include http:// or https://",
        ));
    }

    // 从 query 拿当前 token, 用于 propagate 到 rewrite 后的子资源 URL.
    // 浏览器自动 fetch <link>/<img>/<script> 时不会带 Authorization header,
    // 必须把 token 写进 query string 让 auth middleware 能识别.
    let req_token = req
        .uri()
        .query()
        .and_then(|q| extract_query_token(q))
        .unwrap_or_default();

    // 1. fetch upstream — 经本机 or 经远端 device
    let (status, upstream_headers, body) = match params.via {
        None => fetch_local(&target_url).await?,
        Some(device_id) => fetch_via_device(&state, device_id, &target_url).await?,
    };

    // 2. content type
    let ct = upstream_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    // 3. 决定怎么处理 body
    let rewritten = if ct.starts_with("text/html") {
        rewrite_html(&body, &target_url, params.via, &req_token)
    } else if ct.starts_with("text/css") {
        rewrite_css(&body, &target_url, params.via, &req_token).into_bytes()
    } else {
        body
    };

    // 4. 构建 axum Response 剥 frame headers + 透传安全 headers
    let mut response_builder = Response::builder().status(
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
    );
    for (k, v) in upstream_headers.iter() {
        let name = k.as_str().to_ascii_lowercase();
        // 剥所有 frame / CSP 类头
        if matches!(
            name.as_str(),
            "x-frame-options"
                | "content-security-policy"
                | "content-security-policy-report-only"
                | "cross-origin-opener-policy"
                | "cross-origin-embedder-policy"
                | "content-length"
                | "content-encoding"  // body 已被 reqwest 解压
                | "transfer-encoding"
                | "connection"
        ) {
            continue;
        }
        if let Ok(value) = axum::http::HeaderValue::from_bytes(v.as_bytes()) {
            response_builder = response_builder.header(k.as_str(), value);
        }
    }
    // 显式设置 content-type 防止丢失
    response_builder = response_builder.header(axum::http::header::CONTENT_TYPE, &ct);
    // 显式 frame 友好
    response_builder = response_builder.header("X-Frame-Options-Stripped", "1");

    Ok(response_builder
        .body(Body::from(rewritten))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .unwrap()
        }))
}

// ============================================================================
// fetch
// ============================================================================

async fn fetch_local(
    url: &str,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, Vec<u8>), AppCommandError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppCommandError::network(format!("client: {e}")))?;
    let resp = client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (codeg-browser/0.1)",
        )
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("fetch {url}: {e}")))?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.unwrap_or_default().to_vec();
    Ok((status, headers, body))
}

async fn fetch_via_device(
    state: &Arc<AppState>,
    device_id: i32,
    url: &str,
) -> Result<(reqwest::StatusCode, reqwest::header::HeaderMap, Vec<u8>), AppCommandError> {
    let device = remote_device_service::get(&state.db.conn, device_id)
        .await
        .map_err(AppCommandError::db)?
        .ok_or_else(|| AppCommandError::not_found(format!("device {device_id}")))?;
    let upstream = format!(
        "{}/api/_internal_browse?url={}",
        device.base_url.trim_end_matches('/'),
        urlencoding::encode(url)
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| AppCommandError::network(format!("client: {e}")))?;
    let resp = client
        .get(&upstream)
        .bearer_auth(&device.token)
        .send()
        .await
        .map_err(|e| AppCommandError::network(format!("fetch via dev {device_id}: {e}")))?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.unwrap_or_default().to_vec();
    Ok((status, headers, body))
}

/// 远端 server 的"内部浏览"端点 — 由 fetch_via_device 调,在远端用本机网络出口。
/// 返回的 response 透过 C1 回传到发起的 codeg-server 再做 rewrite。
pub async fn internal_browse(
    Query(params): Query<BrowseParams>,
) -> Result<Response, AppCommandError> {
    let target_url = params.url.clone();
    if !target_url.starts_with("http://") && !target_url.starts_with("https://") {
        return Err(AppCommandError::new(
            AppErrorCode::InvalidInput,
            "browse url must include http:// or https://",
        ));
    }
    let (status, headers, body) = fetch_local(&target_url).await?;

    let mut builder = Response::builder().status(
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
    );
    for (k, v) in headers.iter() {
        let name = k.as_str().to_ascii_lowercase();
        // 跳过 content-encoding (body 已解压) + transfer-encoding
        if matches!(
            name.as_str(),
            "content-encoding" | "transfer-encoding" | "content-length" | "connection"
        ) {
            continue;
        }
        if let Ok(value) = axum::http::HeaderValue::from_bytes(v.as_bytes()) {
            builder = builder.header(k.as_str(), value);
        }
    }
    Ok(builder.body(Body::from(body)).unwrap_or_else(|_| {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::empty())
            .unwrap()
    }))
}

// ============================================================================
// HTML rewrite — 用 lol_html 流式处理, 改 a/img/script/link/iframe/form 等
// ============================================================================

fn rewrite_html(body: &[u8], base_url: &str, via: Option<i32>, token: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(body.len() + 1024);
    let url_attrs: Vec<(&str, &str)> = vec![
        ("a", "href"),
        ("link", "href"),
        ("area", "href"),
        ("base", "href"),
        ("img", "src"),
        ("script", "src"),
        ("iframe", "src"),
        ("source", "src"),
        ("video", "src"),
        ("audio", "src"),
        ("track", "src"),
        ("embed", "src"),
        ("form", "action"),
    ];

    let mut element_handlers: Vec<_> = url_attrs
        .iter()
        .map(|(tag, attr)| {
            let attr = attr.to_string();
            let base = base_url.to_string();
            let via_clone = via;
            let token = token.to_string();
            element!(format!("{}[{}]", tag, &attr), move |el| {
                if let Some(orig) = el.get_attribute(&attr) {
                    let rewritten = rewrite_url(&orig, &base, via_clone, &token);
                    let _ = el.set_attribute(&attr, &rewritten);
                }
                Ok(())
            })
        })
        .collect();

    // 把所有 <a target=...> 和 <form target=...> 非 _self 的统一改成 _blank,
    // client 端 click listener 会拦截 _blank 在内部新 tab 打开,不脱出 iframe。
    element_handlers.push(element!("a[target]", |el| {
        if let Some(t) = el.get_attribute("target") {
            if t != "_self" {
                let _ = el.set_attribute("target", "_blank");
            }
        }
        Ok(())
    }));
    element_handlers.push(element!("form[target]", |el| {
        if let Some(t) = el.get_attribute("target") {
            if t != "_self" {
                let _ = el.set_attribute("target", "_blank");
            }
        }
        Ok(())
    }));

    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: element_handlers,
            ..Settings::default()
        },
        |c: &[u8]| output.extend_from_slice(c),
    );
    if rewriter.write(body).is_ok() {
        let _ = rewriter.end();
    }
    output
}

fn rewrite_css(body: &[u8], base_url: &str, via: Option<i32>, token: &str) -> String {
    let text = String::from_utf8_lossy(body).to_string();
    let re = regex::Regex::new(r#"url\(\s*['"]?([^'")]+)['"]?\s*\)"#).unwrap();
    re.replace_all(&text, |caps: &regex::Captures| {
        let orig = &caps[1];
        let rewritten = rewrite_url(orig, base_url, via, token);
        format!("url({})", rewritten)
    })
    .into_owned()
}

// ============================================================================
// URL rewrite — 任何"绝对 / 协议相对 / 站点相对 / 普通相对"路径都改成本机 /api/browse
// ============================================================================

fn extract_query_token(q: &str) -> Option<String> {
    for kv in q.split('&') {
        let mut parts = kv.splitn(2, '=');
        let k = parts.next()?;
        if k == "token" {
            let raw = parts.next().unwrap_or("");
            return urlencoding::decode(raw).ok().map(|c| c.into_owned());
        }
    }
    None
}

fn rewrite_url(orig: &str, base_url: &str, via: Option<i32>, token: &str) -> String {
    let trimmed = orig.trim();
    // data: / blob: / javascript: / mailto: / tel: / # 锚 — 都不改
    if trimmed.starts_with("data:")
        || trimmed.starts_with("blob:")
        || trimmed.starts_with("javascript:")
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with("tel:")
        || trimmed.starts_with("about:")
        || trimmed.starts_with('#')
        || trimmed.is_empty()
    {
        return orig.to_string();
    }

    let absolute = resolve_to_absolute(trimmed, base_url);
    let mut wrapped = format!("/api/browse?url={}", urlencoding::encode(&absolute));
    if let Some(d) = via {
        wrapped.push_str(&format!("&via={}", d));
    }
    if !token.is_empty() {
        wrapped.push_str(&format!("&token={}", urlencoding::encode(token)));
    }
    wrapped
}

/// 把任意路径解成绝对 URL (相对 base_url)。
fn resolve_to_absolute(href: &str, base_url: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    if href.starts_with("//") {
        // 协议相对 — 用 base 的 scheme
        let scheme = if base_url.starts_with("https") {
            "https:"
        } else {
            "http:"
        };
        return format!("{}{}", scheme, href);
    }
    // 用 url crate? 没引入。手写 resolve:
    let Some(scheme_end) = base_url.find("://") else {
        return href.to_string();
    };
    let after_scheme = &base_url[scheme_end + 3..];
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let host_part = &base_url[..scheme_end + 3 + path_start];

    if href.starts_with('/') {
        return format!("{}{}", host_part, href);
    }
    // 普通相对: 用 base path 同级目录
    let base_path = &base_url[scheme_end + 3 + path_start..];
    let base_dir = match base_path.rfind('/') {
        Some(p) => &base_path[..=p],
        None => "/",
    };
    let base_dir = if base_dir.is_empty() { "/" } else { base_dir };
    let resolved_path = format!("{}{}", base_dir, href);
    format!("{}{}", host_part, resolved_path)
}

// 让 axum 接受空 body 的 POST — 这个 endpoint 用 GET, 但 router 习惯 POST
#[allow(dead_code)]
fn _hint(_: HeaderMap) {}

// ============================================================================
// Bookmark handlers
// ============================================================================

use crate::db::service::bookmark_service::{self, BookmarkInfo};

pub async fn list_bookmarks(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<axum::Json<Vec<BookmarkInfo>>, AppCommandError> {
    let rows = bookmark_service::list(&state.db.conn)
        .await
        .map_err(AppCommandError::db)?;
    Ok(axum::Json(rows))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertBookmarkParams {
    pub id: Option<i32>,
    pub title: String,
    pub url: String,
}

pub async fn upsert_bookmark(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(params): axum::Json<UpsertBookmarkParams>,
) -> Result<axum::Json<BookmarkInfo>, AppCommandError> {
    let info = match params.id {
        Some(id) => bookmark_service::update(&state.db.conn, id, &params.title, &params.url).await,
        None => bookmark_service::create(&state.db.conn, &params.title, &params.url).await,
    }
    .map_err(AppCommandError::db)?;
    Ok(axum::Json(info))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkIdParams {
    pub id: i32,
}

pub async fn delete_bookmark(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(params): axum::Json<BookmarkIdParams>,
) -> Result<axum::Json<()>, AppCommandError> {
    bookmark_service::delete(&state.db.conn, params.id)
        .await
        .map_err(AppCommandError::db)?;
    Ok(axum::Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderBookmarksParams {
    pub ids: Vec<i32>,
}

pub async fn reorder_bookmarks(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(params): axum::Json<ReorderBookmarksParams>,
) -> Result<axum::Json<()>, AppCommandError> {
    bookmark_service::reorder(&state.db.conn, params.ids)
        .await
        .map_err(AppCommandError::db)?;
    Ok(axum::Json(()))
}
