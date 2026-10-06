//! mcloud-server —— HTTP 服务。
//!
//! 路由 + SSE 事件流 + 前端产物内嵌。
//!
//! 生产形态是**同源**：后端同时发 API 与前端产物。
//! 开发态走 Vite dev server，用 `vite.config.ts` 的 `/api` 代理抹平差异
//! （见 webui-code-guide §7.4：刻意不用 CORS，避免"开发能登录、打包上去就登录不了"）。

pub mod auth_gate;
pub mod cli;
pub mod error;
pub mod logbuf;
pub mod qr;
pub mod ratelimit;
pub mod routes;
pub mod state;
pub mod static_files;

pub use state::ServerState;

use axum::middleware;
use axum::Router;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

/// **同时**最多处理多少个请求（2026-10-06 安全审计 `SEC-01` 的一部分）。
///
/// # 它挡的是什么
///
/// 审计里那条"30 个并发登录 = 30 × 19 MiB argon2 ≈ 570 MiB 瞬时内存"就是靠这层钉住的：
/// 第 129 个请求不是被拒，而是**排队**（`ConcurrencyLimit` 在 `poll_ready` 里等信号量），
/// 于是 CPU 与内存的峰值都变成有界的 ✓。
///
/// # 为什么是 128
///
/// 本产品是**单管理员、本机/LAN** 的工具，正常使用是个位数并发；
/// 而这一层钉的是**同时执行中的 handler 数**（信号量在 `poll_ready` 里取、
/// 在响应 future 完成时释放 —— 也就是 handler **返回响应**那一刻，
/// 见 `tower-0.5.3/src/limit/concurrency/service.rs`），所以 SSE 那种"长连接"
/// 并不占名额 ✓（它返回响应后名额就还回去了，真正长的是响应体那条流）。
/// 128 足够"几个页面 + 若干条事件流 + 一次多账号运行"，又远小于"能把机器打死"的量级 ✓。
const MAX_IN_FLIGHT: usize = 128;

/// 构建路由。
pub fn app(state: ServerState) -> Router {
    Router::new()
        .merge(routes::router())
        // 兜底：非 /api 的路径交给内嵌前端（SPA）。放在最后，
        // 保证接口路由优先匹配 —— 顺序反了会把 API 也吞进前端回落。
        .fallback(static_files::serve)
        // `CompressionLayer` 按响应头里的 `Content-Type` 判断可压性，
        // 对 `text/*` / JSON 等文本类型自动 gzip（客户端没带
        // `Accept-Encoding: gzip` 时原样放过，不会压坏）。
        //
        // **放在 TraceLayer 之内**：这样日志里记的是压缩后的字节数，
        // 与客户端实际收到的量一致 —— 否则排"首屏为什么慢"时，
        // 日志会显示 132KB、而浏览器只收了 19KB，两边对不上。
        //
        // SSE（`/api/events`）不受影响：`text/event-stream` 不出现在
        // 该层的可压类型表里，且流式响应不会被缓冲 —— 这是刻意的，
        // 事件流一旦被攒着等压缩就不会"实时"了。
        .layer(CompressionLayer::new())
        // ⚠ 登录闸必须在 `with_state` **之后**加：
        // `from_fn_with_state` 要拿到 state 才能查会话。
        .layer(middleware::from_fn_with_state(state.clone(), auth_gate::gate))
        .layer(TraceLayer::new_for_http())
        // ⚠ 并发上限放**最外层**（最后 `.layer()` = 最外层）：它要罩住包括登录闸
        // 与静态资源在内的**一切**，否则"打满并发"还是能从闸外面打进来。
        .layer(ConcurrencyLimitLayer::new(MAX_IN_FLIGHT))
        .with_state(state)
}
