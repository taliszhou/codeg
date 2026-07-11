use axum::{
    body::Body,
    extract::Request,
    http::{header::CONTENT_TYPE, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
};
use include_dir::{include_dir, Dir};

/// Static web assets compiled into the binary.
///
/// The `web/out/` directory is produced by `pnpm build` in the `web/`
/// workspace.  CI must build the frontend **before** compiling the
/// server so that this macro can embed the files.
static WEB_DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/../web/out");

/// Guess MIME type from file extension.
fn guess_mime(path: &str) -> &'static str {
    if path.ends_with(".html") || path == "" {
        "text/html"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".txt") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

/// Serve a single static file from the embedded directory.
///
/// Returns `None` only when the file truly does not exist (and there is
/// no `index.html` to fall back to).  SPA routes should rely on the
/// caller checking `index.html` fallback.
pub fn serve_static(path: &str) -> Option<(Vec<u8>, &'static str)> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // Dev mode: CODEG_STATIC_DIR 指向磁盘 web/out,优先读盘 (实时反映前端 rebuild)
    if let Ok(dev_dir) = std::env::var("CODEG_STATIC_DIR") {
        let base = std::path::PathBuf::from(&dev_dir);
        if base.exists() {
            for candidate in [
                base.join(path),
                base.join(format!("{}.html", path)),
                base.join(format!("{}/index.html", path.trim_end_matches('/'))),
                base.join("index.html"),
            ] {
                if let Ok(bytes) = std::fs::read(&candidate) {
                    let mime_path = candidate.to_string_lossy().to_string();
                    return Some((bytes, guess_mime(&mime_path)));
                }
            }
            return None;
        }
    }

    // 1) 直接命中文件 (eg. /_next/static/foo.js)
    if let Some(file) = WEB_DIST.get_file(path) {
        return Some((file.contents().to_vec(), guess_mime(path)));
    }

    // 2) Next.js static export 路由：/settings/appearance 实际是
    //    web/out/settings/appearance.html。尝试加 .html 后缀。
    let html_candidate = format!("{}.html", path);
    if let Some(file) = WEB_DIST.get_file(&html_candidate) {
        return Some((file.contents().to_vec(), "text/html"));
    }

    // 3) 或者文件夹形式 settings/appearance/index.html
    let dir_index = format!("{}/index.html", path.trim_end_matches('/'));
    if let Some(file) = WEB_DIST.get_file(&dir_index) {
        return Some((file.contents().to_vec(), "text/html"));
    }

    // 4) SPA fallback — 都没命中就给 index.html，让客户端 React Router 自己处理
    if let Some(file) = WEB_DIST.get_file("index.html") {
        return Some((file.contents().to_vec(), "text/html"));
    }

    None
}

/// Axum handler for static assets.
///
/// Mounted as the router `.fallback()` so every unmatched path is
/// served from the embedded directory.
pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    match serve_static(uri.path()) {
        Some((content, mime)) => {
            // HTML 永远 no-cache (避免 dev 时浏览器拿老 HTML 引用过期 chunk)。
            // 静态资源 (.js / .css with hash) 安全长缓存。
            let cache_header = if mime == "text/html" {
                "no-store, no-cache, must-revalidate"
            } else {
                "public, max-age=31536000, immutable"
            };
            Response::builder()
                .header(CONTENT_TYPE, mime)
                .header("Cache-Control", cache_header)
                .body(Body::from(content))
                .unwrap()
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap(),
    }
}

/// Middleware that rewrites `/folder` → `/folder.html` before the
/// static handler runs, matching Next.js static-export behaviour.
///
/// This is kept for backward compatibility with the original codeg
/// ServeDir-based rewrite layer.
pub async fn html_rewrite_middleware(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if path != "/"
        && !path.contains('.')
        && !path.starts_with("/api")
        && !path.starts_with("/ws")
        && path != "/healthz"
    {
        let html_path = format!("{}.html", path.trim_end_matches('/'));
        if WEB_DIST.get_file(&html_path).is_some() {
            let new_path = if let Some(q) = req.uri().query() {
                format!("{}?{}", html_path, q)
            } else {
                html_path
            };
            if let Ok(new_uri) = new_path.parse::<Uri>() {
                let (mut parts, body) = req.into_parts();
                parts.uri = new_uri;
                let req = Request::from_parts(parts, body);
                return next.run(req).await;
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serve_index_html() {
        let (content, mime) = serve_static("/").expect("index.html should exist");
        assert_eq!(mime, "text/html");
        let text = String::from_utf8(content).expect("valid UTF-8");
        assert!(
            text.contains("<html") || text.contains("<!DOCTYPE"),
            "index.html should contain html tag"
        );
    }

    #[test]
    fn test_serve_static_js() {
        // Find any JS file in the embedded dist
        let js_file = WEB_DIST
            .files()
            .find(|f| f.path().to_str().unwrap_or("").ends_with(".js"));
        if let Some(file) = js_file {
            let path = file.path().to_str().unwrap();
            let (content, mime) = serve_static(path).expect("JS file should be served");
            assert_eq!(mime, "application/javascript");
            assert!(!content.is_empty());
        }
    }

    #[test]
    fn test_spa_fallback() {
        let (content, mime) = serve_static("/some/random/react/route")
            .expect("SPA fallback should return index.html");
        assert_eq!(mime, "text/html");
        let text = String::from_utf8(content).expect("valid UTF-8");
        assert!(
            text.contains("<html") || text.contains("<!DOCTYPE"),
            "fallback should be index.html"
        );
    }

    #[test]
    fn test_guess_mime() {
        assert_eq!(guess_mime("index.html"), "text/html");
        assert_eq!(guess_mime("app.js"), "application/javascript");
        assert_eq!(guess_mime("style.css"), "text/css");
        assert_eq!(guess_mime("data.json"), "application/json");
        assert_eq!(guess_mime("icon.svg"), "image/svg+xml");
        assert_eq!(guess_mime("img.png"), "image/png");
        assert_eq!(guess_mime("font.woff2"), "font/woff2");
        assert_eq!(guess_mime("unknown.xyz"), "application/octet-stream");
    }
}
