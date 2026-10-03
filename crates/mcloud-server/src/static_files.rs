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
//! 「覆写 `target/debug/mcloud` ⇒ SIGKILL ⇒ launchd 重拉 ⇒ TCC 弹框」那条链。
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
//! ## 2026-09-30：改成**优先发构建期的 `.gz` 边车**
//!
//! `CompressionLayer` 是**每请求现压**，而这一步在 debug 构建下贵得离谱。
//! 实测同一个 130,575 B 的 JS 产物（本机）：
//!
//! | 路径 | 耗时 |
//! |---|---|
//! | 不压（identity） | 4.8 ms |
//! | 每请求现压 gzip（串行） | 45.9–60.6 ms |
//! | 每请求现压 gzip（并发 10） | **86 ms p50 / 114 RPS** |
//! | 离线压一次（同机 zlib） | 3.4–4.9 ms，**只花一次** |
//!
//! 所以现在：`npm run build` 会顺带产出 `<产物>.gz`（`web/scripts/gzip-sidecars.mjs`），
//! 这里在客户端声明 `accept-encoding: gzip` 时**直接发那份边车** —— 一次压缩都不做。
//!
//! ⚠ 两条路都要留着：边车缺一份（旧的 `dist`、或将来某类产物没被脚本覆盖）时
//! 仍然退回 `CompressionLayer` 现压。否则"没边车 = 不压缩"，等于把缺陷还回去。
//!
//! ⚠ 边车必须与产物**同批**生成（所以挂在 `npm run build` 后面而不是一个独立命令）：
//! debug 构建下服务端是运行时读盘的，`dist` 里"新 JS + 旧边车"会立刻发出过期字节 ——
//! 而 `assets/*` 带内容哈希，这种不一致不会报错，只会让人看到旧页面。
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

use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
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
///
/// `headers` 只用来判一件事：客户端要不要 gzip（要就发 `.gz` 边车）。
pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(content) = WebAssets::get(path) {
        // 有边车就发边车：一次压缩都不做（见模块头那张实测表）。
        // 没有就发原文，交给 `CompressionLayer` 现压 —— 行为与改造前一致。
        if accepts_gzip(&headers) {
            if let Some(gz) = WebAssets::get(&format!("{path}.gz")) {
                return asset_response(path, gz.data.into_owned(), Some("gzip"));
            }
        }
        return asset_response(path, content.data.into_owned(), None);
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

    // SPA 回落：认不出的一律给 index.html，让前端自己按**路径**路由
    // （`web/src/ui/router.ts`；2026-10-03 起去掉了 `#`）。
    //
    // ⚠ 这条回落与上面那条"末段带点 ⇒ 404"是配套的：干净路径路由下，
    // 路由段里**不能出现 `.`**（会被当静态资源）。今天全部安全（任务名是 Rust
    // 标识符、账号 id 是十六进制），新增路由时要过一遍这条。
    // 但**不能**把 API 路径也吞掉 —— 那会让打错字的接口返回一张 HTML，
    // 前端 `JSON.parse` 报一个与真实原因无关的错。
    if uri.path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "接口不存在").into_response();
    }

    match WebAssets::get("index.html") {
        // ⚠ 回落也必须发 `no-cache`：`/#/tasks` 刷新走的就是这条路径，
        // 漏掉它等于"首页不缓存、深层路由缓存"，问题只会藏得更深。
        // 入口 HTML 同样吃边车：它每次都要回源校验，而这 4 KB 的压缩
        // 在现压路径上也要 1.8 ms（identity 只要 0.56 ms）。
        Some(content) => {
            if accepts_gzip(&headers) {
                if let Some(gz) = WebAssets::get("index.html.gz") {
                    return asset_response("index.html", gz.data.into_owned(), Some("gzip"));
                }
            }
            asset_response("index.html", content.data.into_owned(), None)
        }
        // 前端还没构建：明确说出来，别给一个空白 404 让人以为是后端坏了
        None => (
            StatusCode::NOT_FOUND,
            "前端产物未内嵌；先执行 `cd web && npm run build`，再重新编译本二进制",
        )
            .into_response(),
    }
}

/// 客户端是否接受 `gzip`（也认 `x-gzip`，历史上有的客户端只发这个别名）。
///
/// 判据是 RFC 9110 的那套：`Accept-Encoding: gzip;q=0` 表示**明确不要** ——
/// 只看"出现没出现 gzip"会把它当成要，于是给一个不能解压的客户端发了压缩体。
/// 这里只解析到 q 值，不实现通配符 `*` 的完整协商：本服务只提供 gzip 与 identity
/// 两种，而 `*` 在浏览器里从不是"只接受 gzip"的表达方式。
fn accepts_gzip(headers: &HeaderMap) -> bool {
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for part in value.split(',') {
            let mut segments = part.split(';');
            let name = segments.next().unwrap_or("").trim();
            if !name.eq_ignore_ascii_case("gzip") && !name.eq_ignore_ascii_case("x-gzip") {
                continue;
            }
            let refused = segments.any(|param| {
                let param = param.trim();
                match param.split_once('=') {
                    // ⚠ 必须按**数值**判 q，不能看首字符是不是 '0'：
                    // `q=0.5` 是"能接受、次优先"，按首字符判会把它当成拒绝。
                    // （这条是写测试时才发现的：`br;q=1, gzip;q=0.5` 一开始被判成 no。）
                    Some((key, q)) => {
                        key.trim().eq_ignore_ascii_case("q")
                            && q.trim().parse::<f32>().map(|v| v <= 0.0).unwrap_or(false)
                    }
                    None => false,
                }
            });
            return !refused;
        }
    }
    false
}

/// 组装一个静态资产的响应：准确的 `Content-Type` + 对应的缓存策略。
///
/// `Content-Type` 除了给浏览器，还是**压缩层的开关**
/// （`CompressionLayer` 按类型判断可压性）—— 这里刻意走 `mime_guess`，
/// 不硬编码，免得新增产物类型时被静默跳过压缩。
///
/// `encoding` 是我们要发的那份边车的编码（`Some("gzip")`）。⚠ 传进来的是**原始
/// 路径**（`assets/x.js`）而不是边车路径，所以 `Content-Type` 仍是 `text/javascript`：
/// 若按 `x.js.gz` 去猜类型会得到 `application/gzip`，浏览器会拒绝执行它。
fn asset_response(path: &str, body: Vec<u8>, encoding: Option<&'static str>) -> Response {
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

    let mut response = (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&content_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
            // 同一个 URL 会因 `accept-encoding` 而不同 ⇒ 必须声明 vary，
            // 否则中间缓存（或浏览器自己的缓存）可能把压缩体发给不支持它的客户端。
            (header::VARY, HeaderValue::from_static("accept-encoding")),
        ],
        body,
    )
        .into_response();

    if let Some(encoding) = encoding {
        response
            .headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
    }
    response
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

/// 测试用的简写：**不声明 `accept-encoding`**（即走"原文"那条路）。
///
/// 绝大多数老测试关心的是缓存头 / 状态码 / Content-Type，与压缩无关；
/// 让它们继续一眼能读懂，比每处都手写一个空 `HeaderMap` 好。
#[cfg(test)]
async fn serve_plain(uri: Uri) -> Response {
    serve(uri, HeaderMap::new()).await
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
                serve_plain(axum::http::Uri::from_maybe_shared(format!("/{asset}")).unwrap()).await;
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
            let resp = serve_plain(axum::http::Uri::from_maybe_shared(uri).unwrap()).await;
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
            let resp =
                serve_plain(axum::http::Uri::from_maybe_shared(format!("/{js}")).unwrap()).await;
            let ct = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert_eq!(ct, "text/javascript", "JS 的 Content-Type 不对：{ct}");
        }
        let resp = serve_plain(axum::http::Uri::from_maybe_shared("/").unwrap()).await;
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "text/html; charset=utf-8");
    }

    /// ⭐ 有边车时必须**直接发边车**，一次压缩都不做。
    ///
    /// 这条测试**没有挂 `CompressionLayer`** —— 所以能拿到 `content-encoding: gzip`，
    /// 只可能是"边车被发出来了"。顺带钉住三件容易写错的事：
    /// 类型仍是 `text/javascript`（按 `x.js.gz` 去猜会得到 `application/gzip`，
    /// 浏览器会拒绝执行）、缓存头不变、`vary` 在位。
    #[tokio::test]
    async fn precompressed_sidecar_is_served_as_is() {
        use http_body_util::BodyExt as _;

        if !has_built_assets() {
            return;
        }
        let Some(js) = find_hashed_asset(".js") else {
            return;
        };
        let Some(gz) = WebAssets::get(&format!("{js}.gz")) else {
            eprintln!("跳过：{js}.gz 不存在（这份 dist 不是 `npm run build` 产出的）");
            return;
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br, zstd"),
        );
        let resp = serve(Uri::from_maybe_shared(format!("/{js}")).unwrap(), headers).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_ENCODING)
                .map(|v| v.as_bytes()),
            Some(b"gzip".as_slice()),
            "有边车就该发边车"
        );
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/javascript"),
            "Content-Type 必须按原始路径判断，不能变成 application/gzip"
        );
        assert!(
            resp.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .contains("immutable"),
            "缓存策略不该被这次改造影响"
        );
        assert_eq!(
            resp.headers()
                .get(header::VARY)
                .and_then(|v| v.to_str().ok()),
            Some("accept-encoding")
        );

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &body[..2],
            &[0x1f, 0x8b],
            "边车应当是 gzip 流（magic 1f 8b）"
        );
        assert_eq!(
            body.len(),
            gz.data.len(),
            "发的应当是边车本身的字节，而不是现压的结果"
        );
    }

    /// 不声明 gzip 的客户端拿到**原文**：不能把边车塞给不会解压的客户端。
    #[tokio::test]
    async fn client_without_gzip_gets_the_plain_bytes() {
        use http_body_util::BodyExt as _;

        if !has_built_assets() {
            return;
        }
        let Some(js) = find_hashed_asset(".js") else {
            return;
        };

        let resp = serve_plain(Uri::from_maybe_shared(format!("/{js}")).unwrap()).await;
        assert!(resp.headers().get(header::CONTENT_ENCODING).is_none());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let raw = WebAssets::get(&js).expect("刚找到的资产").data;
        assert_eq!(body.as_ref(), raw.as_ref(), "应当是逐字节的原文");
    }

    /// `accept-encoding` 的判定：要 / 不要 / **明确拒绝（q=0）**。
    ///
    /// `q=0` 这条是真会咬人的：只看"出现没出现 gzip"，就会把压缩体发给一个
    /// 明确说不要的客户端，而它解不开 —— 表现为"页面白屏，控制台语法错误"。
    #[test]
    fn accept_encoding_parsing_handles_the_q_zero_trap() {
        let hv = |s: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(s).unwrap());
            h
        };
        assert!(
            accepts_gzip(&hv("gzip, deflate, br, zstd")),
            "浏览器默认就长这样"
        );
        assert!(accepts_gzip(&hv("x-gzip")), "老客户端只发这个别名");
        assert!(accepts_gzip(&hv("br;q=1, gzip;q=0.5")));
        assert!(!accepts_gzip(&hv("gzip;q=0")), "q=0 是明确不要");
        assert!(!accepts_gzip(&hv("gzip; q=0.0")));
        assert!(!accepts_gzip(&hv("deflate, br")), "没声明 gzip 就不发边车");
        assert!(!accepts_gzip(&HeaderMap::new()), "连头都没有");
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
            let res = serve_plain(Uri::from_static(uri)).await;
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri} 应当 404");
        }
    }

    /// 看起来是文件的路径（末段带点）同样 404：把 HTML 当图标/脚本发出去属于误导。
    #[tokio::test]
    async fn file_like_miss_is_404() {
        let res = serve_plain(Uri::from_static("/favicon.ico")).await;
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
        let res = serve_plain(Uri::from_static("/some/deep/route")).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
    }
}
