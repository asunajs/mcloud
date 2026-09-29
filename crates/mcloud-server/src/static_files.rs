//! 前端产物托管。
//!
//! **为什么内嵌而不是读目录**：
//!
//! 1. 与后端**同源** —— 生产形态下浏览器只访问一个源，不需要 CORS，
//!    也不会出现"开发能用、打包上去就不行"（webui-code-guide §7.4）。
//! 2. 单二进制交付：拷一个可执行文件就能跑，不必同时带 `web/dist`。
//!
//! **构建顺序有依赖（release 构建）**：`rust-embed` 在**编译期**把 `web/dist` 读进来。
//! 所以**交付构建**必须先 `npm run build` 再 `cargo build`，否则二进制里还是旧产物。
//!
//! ⚠ 但 **debug 构建是运行时读盘** —— 这条反直觉，2026-09-28 核实过：
//! rust-embed 8 的实现在 `#[cfg(all(debug_assertions, not(feature = "debug-embed")))]`
//! 下**每次 `get()` 都去 `web/dist` 读文件**，而本仓只开了 `interpolate-folder-path`。
//! 于是开发期改前端**只需 `npm run build`**：正在跑的服务下一次请求就是新产物，
//! 不必重新编译 —— 也就不必触发
//! 「覆写 `target/debug/mcloud-server` ⇒ SIGKILL ⇒ launchd 重拉 ⇒ TCC 弹框」那条链。
//!
//! 实测：服务进程 18:36:37 启动后**没有再重启**，把**次日 08:06** 新构建出的 JS 与
//! 服务端返回的字节逐字节 `cmp` —— 一致（CSS、`index.html` 同）。
//!
//! 目录不存在时（例如没跑过 `npm run build`）会得到一个空资产集，
//! 此时所有静态路径都落到 SPA fallback —— 不 panic，只是没有页面可发。
//!
//! ## 传输层：gzip 与缓存（首屏从 ~2.5s 降到目标区间的那一半）
//!
//! 前端产物是**大段重复文本**，而文本的压缩比高得离谱。实测：
//!
//! | 文件 | 原始 | gzip | 省 |
//! |---|---|---|---|
//! | `index-*.css` | 132969 | 19304 | 85% |
//! | `index-*.js` | 154433 | 48474 | 69% |
//! | `index.html` | 1444 | 940 | 35% |
//! | **合计** | **288846** | **68718** | **76%** |
//!
//! 也就是说"没压缩"这一个问题就多传了 **220KB**。这不是"优化项"，
//! 是**缺陷**：`Content-Encoding` 是服务端的责任，浏览器不会替你压。
//! （Edge 的性能面板会把这条直接标成问题 —— 因为它看到的是未压缩的
//! 传输字节，且 `Content-Length` 与实际载荷对不上。）
//!
//! 压缩由 `lib.rs` 挂的 `CompressionLayer` 统一做，**这里只负责发对头**：
//! 那层的判定依据是响应头里的 `Content-Type`，所以 `Content-Type` 必须准确，
//! 否则会被当成不可压类型（如 `application/octet-stream`）跳过。
//!
//! ## 缓存：`assets/` 与 `index.html` 必须**相反**地对待
//!
//! Vite 给产物文件名带**内容哈希**，于是同一 URL 的字节永不改变 ——
//! 这是 `immutable` 一年的充分条件，二次访问**一个字节都不下**。
//!
//! 但 `index.html` **绝不能**跟着一起长缓存：它正是那张"指向当前哈希"的表。
//! 一旦被缓存住，浏览器会拿着旧 HTML 去要**已经不存在的** `index-<旧哈希>.js`，
//! 于是"发版后白屏、硬刷新才好" —— 这类事故排查成本远高于省下的那 1.4KB。
//! 所以它是 `no-cache`（允许存，但每次**必须**回源校验）。
//!
//! 判据是**路径前缀**而不是文件名：`assets/` 是 Vite 的产物目录约定，
//! `index.html` 在根。这个区分写死在这里，比"猜哪些文件带哈希"可靠。

use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// 内嵌的前端产物。
///
/// `folder = "../../web/dist"` 是相对**本 crate 的 `Cargo.toml`**。
/// 用 `interpolate-folder-path` 特性以支持相对路径展开。
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// 带内容哈希的产物：一年、且 `immutable`。
///
/// `immutable` 是给浏览器"**别**在用户按刷新时还去校验"的承诺 ——
/// 没有它，刷新会带上 `If-None-Match` 白跑一趟 304 往返。
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";

/// 入口 HTML：每次都要回源校验。
///
/// 不用 `no-store` —— 那样连"304 复用"都没了。`no-cache` 的语义是
/// "可以存，但用之前必须问一次"，正好是这个文件要的。
const NO_CACHE: &str = "no-cache";

/// 从内嵌资产取文件。
///
/// SPA 路由用的是 hash（`#/tasks`），真实路径永远只有 `/`，
/// 因此**不需要**为任意路径回落 `index.html`；这里只在
/// 资产不存在且请求的不是 `/api/*` 时回落，保证刷新页面不 404。
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = WebAssets::get(path) {
        return asset_response(path, content.data.into_owned());
    }

    // ⚠⚠ **`assets/` 下的未命中必须是 404，不能回落 SPA**（2026-09-29 实测的缺陷）。
    //
    // 那是 Vite 的**带内容哈希**产物目录：文件名一变，旧 URL 就不存在了 —— 而
    // 拿到旧 `index.html` 的浏览器**正是要**去要那个旧 URL，它必须收到 404 才会
    // 回头重新取 HTML。回落会让它拿到一张 `200 text/html` 并当 JS 解析 ⇒
    // 「发版后白屏 + 一个与真实原因无关的 MIME 报错」，排查成本远高于 404。
    //
    // 顺手也把"看起来是文件"的路径（末段带点，如 `/favicon.ico`）一起 404：
    // 同样是把 HTML 当别的类型发出去，收益为零、只有误导。
    // 注意本应用是 **hash 路由**（`#/tasks`），真实路径只有 `/`，
    // 所以这条收紧不会伤到任何正常入口。
    let last = path.rsplit('/').next().unwrap_or(path);
    if path.starts_with("assets/") || last.contains('.') {
        return (StatusCode::NOT_FOUND, "静态资源不存在").into_response();
    }

    // SPA 回落：认不出的一律给 index.html，让前端按 hash 自己路由。
    // 但**不能**把 API 路径也吞掉 —— 那会让打错字的接口返回一张 HTML，
    // 前端 `JSON.parse` 报一个与真实原因无关的错。
    if uri.path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "接口不存在").into_response();
    }

    match WebAssets::get("index.html") {
        // ⚠ 回落也必须发 `no-cache`：`/#/tasks` 刷新走的就是这条路径，
        // 漏掉它等于"首页不缓存、深层路由缓存"，问题只会藏得更深。
        Some(content) => asset_response("index.html", content.data.into_owned()),
        // 前端还没构建：明确说出来，别给一个空白 404 让人以为是后端坏了
        None => (
            StatusCode::NOT_FOUND,
            "前端产物未内嵌；先执行 `cd web && npm run build`，再重新编译本二进制",
        )
            .into_response(),
    }
}

/// 组装一个静态资产的响应：准确的 `Content-Type` + 对应的缓存策略。
///
/// `Content-Type` 除了给浏览器，还是**压缩层的开关**
/// （`CompressionLayer` 按类型判断可压性）—— 这里刻意走 `mime_guess`，
/// 不硬编码，免得新增产物类型时被静默跳过压缩。
fn asset_response(path: &str, body: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    // 用 `essence_str` 比较：`mime_guess` 对 `.html` 给的是不带参数的
    // `text/html`，但比较时忽略参数更稳（将来若带上 charset 也不会漏判）。
    // HTML 必须补 charset，否则中文会被按 latin-1 解析成乱码。
    let mut content_type = mime.to_string();
    if mime.essence_str() == "text/html" {
        content_type = "text/html; charset=utf-8".to_string();
    }

    let cache = if path.starts_with("assets/") {
        IMMUTABLE_CACHE
    } else {
        NO_CACHE
    };

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        body,
    )
        .into_response()
}

/// 内嵌资产集是否非空（即前端是否已构建）。
///
/// 给测试用：`cargo test` 不该反过来依赖 `npm run build`，
/// 所以测试遇到"没构建"时要**跳过**而不是失败，由这个函数给出判据。
pub fn has_built_assets() -> bool {
    WebAssets::iter().next().is_some()
}

/// 找一个**带内容哈希**的产物路径（相对根，如 `assets/index-abc.css`）。
///
/// 同样是给测试用的：测试不该把哈希写死 —— 每次 `npm run build` 都会变，
/// 写死等于给前端加一道"改个样式就测试红"的枷锁。
///
/// `ext` 接受带点或不带点（`.css` / `css`），免得调用点记错。
pub fn find_hashed_asset(ext: &str) -> Option<String> {
    let ext = if ext.starts_with('.') {
        ext.to_string()
    } else {
        format!(".{ext}")
    };
    WebAssets::iter()
        .find(|p| p.starts_with("assets/") && p.ends_with(ext.as_str()))
        .map(|p| p.to_string())
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

    /// `asset_response` 发对了头，`CompressionLayer` 就会照预期压。
    ///
    /// # ⚠ 这条**测不到**"忘了在 `lib.rs` 挂层"
    ///
    /// 因为它在测试里**自己挂了一层** —— 摘掉 `lib.rs` 里那道真实装配，
    /// 这条照样绿（已用变异测试确认）。它验的是"层的语义如我们所想"：
    /// 声明 gzip 的客户端拿到 `content-encoding: gzip`、没声明的拿到原文。
    ///
    /// **装配那件事由集成测试盯**：
    /// `tests/routes.rs` 的 `hashed_assets_are_compressed_and_immutable`
    /// 打的是 `app(...)` 真实路由 —— 摘掉层它立刻红（同样验过）。
    ///
    /// 两条分工写在这里，免得后人看到这条"route 级"就以为装配也被覆盖了。
    #[tokio::test]
    async fn gzip_layer_compresses_when_the_client_asks() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt as _;
        use tower::ServiceExt as _;

        if !has_built_assets() {
            return; // 前端未构建：跳过，不让 cargo test 依赖 npm
        }

        // 只挂 CompressionLayer，避免把登录闸/存储也拉进来。
        let app = axum::Router::new()
            .fallback(serve)
            .layer(tower_http::compression::CompressionLayer::new());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("accept-encoding", "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.headers().get("content-encoding").unwrap(), "gzip");
        let _ = resp.into_body().collect().await.unwrap();

        // 不声明 gzip 的客户端不能被压坏
        let app = axum::Router::new()
            .fallback(serve)
            .layer(tower_http::compression::CompressionLayer::new());
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(resp.headers().get("content-encoding").is_none());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("<!doctype html"));
    }

    /// 缓存策略：`assets/*` 长缓存、其余（含 SPA 回落）一律 `no-cache`。
    #[tokio::test]
    async fn cache_policy_splits_hashed_assets_from_entry_html() {
        if !has_built_assets() {
            return;
        }
        // assets/ 下取一个真实文件
        if let Some(asset) = find_hashed_asset(".css").or_else(|| find_hashed_asset(".js")) {
            let resp =
                serve(axum::http::Uri::from_maybe_shared(format!("/{asset}")).unwrap()).await;
            let cc = resp
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(
                cc.contains("immutable"),
                "{asset} 应为 immutable，实际 {cc}"
            );
        }
        for uri in ["/", "/deep/route"] {
            let resp = serve(axum::http::Uri::from_maybe_shared(uri).unwrap()).await;
            let cc = resp
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert_eq!(cc, "no-cache", "{uri} 必须 no-cache，实际 {cc}");
        }
    }

    /// CSS/JS 必须带正确的 `Content-Type`：它同时是**压缩层的开关**。
    /// 若退化成 `application/octet-stream`，压缩会被静默跳过。
    #[tokio::test]
    async fn content_type_is_accurate_enough_to_trigger_compression() {
        if !has_built_assets() {
            return;
        }
        if let Some(js) = find_hashed_asset(".js") {
            let resp = serve(axum::http::Uri::from_maybe_shared(format!("/{js}")).unwrap()).await;
            let ct = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert_eq!(ct, "text/javascript", "JS 的 Content-Type 不对：{ct}");
        }
        let resp = serve(axum::http::Uri::from_maybe_shared("/").unwrap()).await;
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "text/html; charset=utf-8");
    }
}

#[cfg(test)]
mod assets_404_tests {
    use super::*;

    /// 未命中的哈希产物必须 404（而不是被 SPA 回落成 200 HTML）。
    /// 这条不需要前端构建过 —— 判据在"路径形状"上，与资产集无关。
    #[tokio::test]
    async fn hashed_asset_miss_is_404() {
        for uri in [
            "/assets/index-DOESNOTEXIST.js",
            "/assets/index-DOESNOTEXIST.css",
        ] {
            let res = serve(Uri::from_static(uri)).await;
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri} 应当 404");
        }
    }

    /// 看起来是文件的路径（末段带点）同样 404：把 HTML 当图标/脚本发出去属于误导。
    #[tokio::test]
    async fn file_like_miss_is_404() {
        let res = serve(Uri::from_static("/favicon.ico")).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    /// 反过来：普通路径仍要回落 SPA（直接输 `/whatever` 也能进应用）。
    /// 没构建过前端时跳过 —— 测试不该反过来依赖 `npm run build`。
    #[tokio::test]
    async fn plain_path_still_falls_back_to_spa() {
        if !has_built_assets() {
            eprintln!("跳过：web/dist 未构建");
            return;
        }
        let res = serve(Uri::from_static("/some/deep/route")).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
    }
}
