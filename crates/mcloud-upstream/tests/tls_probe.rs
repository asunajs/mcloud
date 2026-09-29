//! TLS 兼容性探针。
//!
//! # 为什么需要它
//!
//! 本仓的 TLS 取向是**兼容性优先**：只走 `native-tls`（各平台系统实现），
//! 因为 rustls 出于安全考虑**不实现任何 CBC 套件**，而直播侧主机
//! `v.cmcc-cs.cn` 只给 `TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA`（0xC013）
//! ⇒ 双方零交集、`HandshakeFailure`。详见 `live_consts.rs` 与 `client.rs`。
//!
//! 这条取向有两个容易被改错的地方，本探针就是拿来钉住它们的：
//!
//! 1. **老主机连得上**（换回 rustls 就红）；
//! 2. **现代主机也连得上** —— 这才是"兼容性优先"真正的代价所在：
//!    全局换后端之后，原本靠 rustls 好好的 `m.mcloud.139.com` 必须照样通。
//!
//! # 怎么跑
//!
//! 需要外网，所以全部 `#[ignore]`，CI 不跑（CI 不该依赖上游可达性）：
//!
//! ```text
//! cargo test -p mcloud-upstream --test tls_probe -- --ignored --nocapture
//! ```
//!
//! ⚠ 只做 TLS 握手与读响应，**不发任何业务请求**：不点小红花、不发弹幕、不登录。

use std::time::Duration;

/// 老主机：只有 CBC 套件（换错后端时这里第一个红）。
const CBC_HOST: &str = "https://v.cmcc-cs.cn/cmlive";

/// 现代主机：原先靠 rustls 就是通的 —— 全局换 native-tls 后必须照样通。
const MODERN_HOST: &str = "https://m.mcloud.139.com/ycloud";

/// 直播弹幕 WS（`live_ws_client` 生产用的基址）。
const WS_BASE: &str = "wss://videoim.cmcc-cs.cn:20102";

/// 判断一个错误是不是**TLS 层**的失败。
///
/// 区分这个很关键：HTTP 401/403/404 都说明**TLS 已经握手成功**了，
/// 那正是我们要的证据；只有握手阶段失败才算探针失败。
fn looks_like_tls_failure(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("handshake")
        || m.contains("tls")
        || m.contains("certificate")
        || m.contains("unknown ca")
        || m.contains("alert")
}

#[tokio::test]
#[ignore = "需要外网；用 --ignored 手动跑"]
async fn cbc_only_host_completes_tls() {
    // reqwest 现在只编了 native-tls，所以默认后端就是系统 TLS
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("构造 client");
    match client.get(CBC_HOST).send().await {
        // 任何 HTTP 状态都算通过：TLS 已经谈成了
        Ok(resp) => eprintln!("{CBC_HOST} → HTTP {}（TLS 握手成功）", resp.status()),
        Err(e) => panic!("{CBC_HOST} 失败：{e}\n（CBC 主机连不上 = TLS 后端选错了）"),
    }
}

#[tokio::test]
#[ignore = "需要外网；用 --ignored 手动跑"]
async fn modern_host_still_completes_tls() {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("构造 client");
    match client.get(MODERN_HOST).send().await {
        Ok(resp) => eprintln!("{MODERN_HOST} → HTTP {}（TLS 握手成功）", resp.status()),
        Err(e) => panic!("{MODERN_HOST} 失败：{e}\n（换成 native-tls 把原本能通的主机弄红了）"),
    }
}

#[tokio::test]
#[ignore = "需要外网；用 --ignored 手动跑"]
async fn live_ws_handshake_reaches_http_layer() {
    // 固定 serverId/sessionId 只为拼一个合法形状的 URL —— 不参与任何业务，
    // 连上就断，不发消息。
    let url = mcloud_upstream::live_ws_client::live_ws_url(WS_BASE, 1, "tls-probe");
    match tokio_tungstenite::connect_async(&url).await {
        // 101 说明连 WS 升级都成了
        Ok((_, resp)) => eprintln!("{url} → WS {}（TLS 握手成功）", resp.status()),
        Err(e) => {
            let msg = e.to_string();
            // 401/403/404 之类同样说明 TLS 已经谈成 —— 探针只关心这一点
            assert!(
                !looks_like_tls_failure(&msg),
                "{url} 在 **TLS 层**失败：{msg}\n（WS 侧仍需 native-tls）"
            );
            eprintln!("{url} → 非 TLS 层错误（TLS 握手成功）：{msg}");
        }
    }
}
