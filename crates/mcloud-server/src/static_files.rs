//! 前端产物托管。
//!
//! **为什么内嵌而不是读目录**：
//!
//! 1. 与后端**同源** —— 生产形态下浏览器只访问一个源，不需要 CORS，
//!    也不会出现"开发能用、打包上去就不行"（webui-code-guide §7.4）。
//! 2. 单二进制交付：拷一个可执行文件就能跑，不必同时带 `web/dist`。
//!
//! **构建顺序有依赖**：`rust-embed` 在**编译期**把 `web/dist` 读进来。
//! 所以改前端后必须先 `npm run build` 再 `cargo build`，否则二进制里还是旧产物。
//! 目录不存在时（例如只跑 `cargo test`）会得到一个空资产集，
//! 此时所有静态路径都落到 SPA fallback —— 不 panic，只是没有页面可发。

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// 内嵌的前端产物。
///
/// `folder = "../../web/dist"` 是相对**本 crate 的 `Cargo.toml`**。
/// 用 `interpolate-folder-path` 特性以支持相对路径展开。
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// 从内嵌资产取文件。
///
/// SPA 路由用的是 hash（`#/tasks`），真实路径永远只有 `/`，
/// 因此**不需要**为任意路径回落 `index.html`；这里只在
/// 资产不存在且请求的不是 `/api/*` 时回落，保证刷新页面不 404。
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = WebAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref())],
            content.data,
        )
            .into_response();
    }

    // SPA 回落：认不出的一律给 index.html，让前端按 hash 自己路由。
    // 但**不能**把 API 路径也吞掉 —— 那会让打错字的接口返回一张 HTML，
    // 前端 `JSON.parse` 报一个与真实原因无关的错。
    if uri.path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "接口不存在").into_response();
    }

    match WebAssets::get("index.html") {
        Some(content) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            content.data,
        )
            .into_response(),
        // 前端还没构建：明确说出来，别给一个空白 404 让人以为是后端坏了
        None => (
            StatusCode::NOT_FOUND,
            "前端产物未内嵌；先执行 `cd web && npm run build`，再重新编译本二进制",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assets_compile_in() {
        // 只断言"能取到 index.html 或目录为空"两种情况之一，
        // 不把"前端一定已构建"写成测试前提 —— 否则 `cargo test` 会依赖 npm。
        let has_index = WebAssets::get("index.html").is_some();
        let empty = WebAssets::iter().next().is_none();
        assert!(
            has_index || empty,
            "要么内嵌了 index.html，要么整个资产集为空；不该出现有资产但缺 index.html"
        );
    }
}
