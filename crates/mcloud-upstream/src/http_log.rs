//! 第三方请求日志层 —— 排查上游问题时用。
//!
//! # 为什么需要它
//!
//! 上游（移动云盘的各接口、风控换 id、直播长连、webhook 推送）出问题时，
//! 现在能看到的往往只有一句 `Error::Network(...)` ✗ —— 「发了什么、回来什么、
//! 花了多久」一概看不见，于是只能靠猜 ✗。这一层把那三件事按请求记下来 ✓。
//!
//! # 为什么放在**最内层**（紧贴传输层）
//!
//! 本仓每个账号的链子是：
//!
//! ```text
//! SmidClient → H5Client → ReqwestClient
//! ```
//!
//! 到这一层时，`deviceid` / `jwttoken` / `authorization` **都已经注入完毕** ✓
//! ⇒ 记下来的就是**真正上网的那一发** ✓（查第三方问题时，要看的就是这个 ✓，
//! 而不是"我们打算发什么"）。
//!
//! 顺带解释了它**不**放在 `builder.rs` 更外层的理由：那两层是**装饰器**，
//! 各有各的重写（补头、建会话、90001 刷新重放 ✓）—— 在它们外面记，看到的是
//! 重写**前**的形状 ✗。
//!
//! # ⚠⚠ 脱敏是硬要求，不是可选项
//!
//! 这一层的每一条请求都带着整套凭证：`authorization`、`jwttoken`、`cookie`、
//! 以及各种 `?token=…` 查询串。而日志会被**贴进 issue、留在文件里、被 agent 读走** ✗
//! ⇒ URL、请求头、请求体、响应体、错误文本**一律先过**
//! [`mcloud_core::event::redact`] ✓。
//!
//! 少过一处就等于把凭证送出去 ✗ —— 所以本文件的每个格式化出口都走 [`safe()`]，
//! 有单元测试钉着它 ✓（见文件尾）。
//!
//! # 档位（与全仓 tracing 口径一致，不另加开关）
//!
//! * `debug`：一行"请求 → 响应"（方法、脱敏 URL、状态、耗时、字节数）
//! * `trace`：再加请求头、请求体、响应体的**截断预览**（同样脱敏）
//!
//! 本仓服务端默认过滤器是 `info,mcloud=debug` ⇒ **debug 那档开箱可见** ✓；
//! 要看正文就 `RUST_LOG=mcloud=trace`（或 `mcloud_upstream=trace`）✓。
//! 不引入新环境变量的理由：日志档位已经是 `RUST_LOG` 的事，再加一个开关
//! 只会让"为什么没日志"多一种可能 ✗。

use std::time::Instant;

use async_trait::async_trait;
use mcloud_core::{HttpClient, HttpRequest, HttpResponse, Result};

/// 一条日志里最多放多少字符（正文预览）。
const PREVIEW_CHARS: usize = 512;

/// 出错响应（≥400）的预览更短：那多半是上游的错误 JSON，够看出 code/message 就行 ✓。
const ERROR_PREVIEW_CHARS: usize = 256;

/// 请求头值的预览上限（头都很短，`authorization` 那种长的只需看出"是不是空"）。
const HEADER_CHARS: usize = 96;

/// 把"发出去的每一枪"记进 tracing 的 [`HttpClient`] 装饰器。
///
/// 泛型而不是 `Arc<dyn HttpClient>`：内层是具体类型时不必多一层间接 ✓；
/// 需要放进 `Arc<dyn HttpClient>` 的地方照样能装进去 ✓（builder 就是这么用的）。
pub struct LoggingClient<T> {
    inner: T,
    /// 记进日志，用来把并发跑的多账号请求分开 —— 不写的话多条日志交叉着看不懂 ✗。
    account: String,
}

impl<T> LoggingClient<T> {
    pub fn new(inner: T, account: impl Into<String>) -> Self {
        Self {
            inner,
            account: account.into(),
        }
    }
}

#[async_trait]
impl<T: HttpClient> HttpClient for LoggingClient<T> {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.log_outgoing(&req, "请求");
        let started = Instant::now();
        match self.inner.request(req).await {
            Ok(resp) => {
                let ms = started.elapsed().as_millis() as u64;
                // ≥400 提到 warn：那正是"第三方出问题"时最该在默认档位里看见的东西 ✓
                // （默认过滤器是 `info,mcloud=debug` ⇒ warn 一定出得来 ✓）。
                if resp.status >= 400 {
                    tracing::warn!(
                        account = %self.account,
                        status = resp.status,
                        ms,
                        bytes = resp.body.len(),
                        // ⚠ **网页不当正文贴**（2026-10-08 用户实测）：网关在路径不存在时
                        // 回一整页 HTML ✗，原样进日志等于往日志里倒标记，把真正的原因淹掉 ✗。
                        // 与 `ensure_http_ok` 用同一个判据 ✓（JSON 原样、HTML 说清是网页 ✓）。
                        preview = %crate::session::describe_body(&safe(&resp.body, ERROR_PREVIEW_CHARS)),
                        "上游应答（非 2xx）"
                    );
                } else {
                    tracing::debug!(
                        account = %self.account,
                        status = resp.status,
                        ms,
                        bytes = resp.body.len(),
                        "上游应答"
                    );
                }
                Ok(resp)
            }
            Err(e) => {
                // ⚠ 错误文本也要脱敏：`Error::Network` 里带的是**上游原文**，
                // 而某些网关会把请求头原样回显 ✗（复核意见 §1.14 就是这一类）。
                tracing::warn!(
                    account = %self.account,
                    ms = started.elapsed().as_millis() as u64,
                    error = %safe(&e.to_string(), ERROR_PREVIEW_CHARS),
                    "上游请求失败"
                );
                Err(e)
            }
        }
    }

    /// ⚠⚠ **必须转发，不能吃默认实现** —— `traits.rs` 里那句警告说的就是这里：
    /// 默认实现走的是 `self.request()`，而叶子层回的是**有损文本**
    /// ⇒ 二进制（头像下载那条链）的字节会在这一跳被毁掉，而且**不报错** ✗。
    ///
    /// 这条通路只记大小、不记正文：它运的是图片字节，记下来也没法看 ✓
    /// （真要排查，看"字节数对不对"通常就够了 ✓）。
    async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
        self.log_outgoing(&req, "请求（原始字节）");
        let started = Instant::now();
        let out = self.inner.request_raw(req).await;
        match &out {
            Ok(bytes) => tracing::debug!(
                account = %self.account,
                ms = started.elapsed().as_millis() as u64,
                bytes = bytes.len(),
                "上游应答（原始字节）"
            ),
            Err(e) => tracing::warn!(
                account = %self.account,
                ms = started.elapsed().as_millis() as u64,
                error = %safe(&e.to_string(), ERROR_PREVIEW_CHARS),
                "上游请求失败（原始字节）"
            ),
        }
        out
    }
}

impl<T> LoggingClient<T> {
    /// 出门前的那一行。`debug` 只格式化便宜的几个字段 ✓；
    /// 头与正文只在 `trace` 档才拼串（默认档位下**一次分配都不做** ✓）。
    fn log_outgoing(&self, req: &HttpRequest, what: &str) {
        tracing::debug!(
            account = %self.account,
            method = %req.method,
            url = %safe(&req.url, PREVIEW_CHARS),
            body_bytes = req.body.as_ref().map(|b| b.len()).unwrap_or(0),
            "{what}"
        );

        if tracing::enabled!(tracing::Level::TRACE) {
            let headers = req
                .headers
                .iter()
                .map(|(k, v)| format!("{k}={}", safe(v, HEADER_CHARS)))
                .collect::<Vec<_>>()
                .join(" ");
            let body = req
                .body
                .as_deref()
                .map(|b| safe(&String::from_utf8_lossy(b), PREVIEW_CHARS))
                .unwrap_or_default();
            tracing::trace!(account = %self.account, %headers, %body, "请求详情");
        }
    }
}

/// **所有**出日志的文本都走这里：先脱敏，再截断。
///
/// 顺序不能反 ✗：先截断再脱敏的话，一枚被从中截断的 token 就凑不出完整形状，
/// `redact()` 认不出来 ⇒ 剩下半截照样泄漏 ✗。
///
/// 截断按**字符**（不是字节）算：按字节切会把多字节字符切坏 ⇒ 日志里出现乱码 ✗。
fn safe(text: &str, limit: usize) -> String {
    let redacted = mcloud_core::event::redact(text);
    let mut out: String = redacted.chars().take(limit).collect();
    if out.len() < redacted.len() {
        out.push_str(&format!("…（共 {} 字符）", redacted.chars().count()));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 内层替身：把收到的请求原样记下来，应答由构造时给死。
    ///
    /// ⚠ 直接给 `Fake` 实现 `HttpClient`（**不能**写成 `impl ... for Arc<Fake>` ✗）：
    /// `Arc` 与 `HttpClient` 都是外部类型 ⇒ 孤儿规则不让 ✗。要还想在测试里看记录，
    /// 就把 `seen` 做成共享句柄 ✓（[`Fake::seen`]）。
    struct Fake {
        seen: Arc<Mutex<Vec<HttpRequest>>>,
        status: u16,
        body: String,
        raw: Vec<u8>,
    }

    impl Fake {
        fn new(status: u16, body: &str, raw: Vec<u8>) -> Self {
            Self {
                seen: Arc::new(Mutex::new(Vec::new())),
                status,
                body: body.to_string(),
                raw,
            }
        }
        fn seen(&self) -> Arc<Mutex<Vec<HttpRequest>>> {
            Arc::clone(&self.seen)
        }
    }

    #[async_trait]
    impl HttpClient for Fake {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().expect("锁中毒").push(req);
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
                headers: Vec::new(),
            })
        }
        async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
            self.seen.lock().expect("锁中毒").push(req);
            Ok(self.raw.clone())
        }
    }

    /// ⚠⚠ **这个模块里每条会打日志的测试都要装一个订阅器**（哪怕是丢掉的 ✗）。
    ///
    /// 成因：`tracing` 的 callsite interest 是**全局缓存**的 —— 一次"**没有**订阅器"的命中
    /// 会把它**永久**缓存成 `never` ✗，此后那条 callsite 压根不再问订阅器 ⇒
    /// [`emits_debug_line_and_warns_on_4xx`] 就**偶发**取不到日志 ✗
    /// （2026-10-06 实测：21 次里红 1 次，报「没打出应答行」，而缓冲区里**只有出站那行** ✓
    /// —— 出站与应答是两个 callsite，被缓存掉的是后者 ✓）。
    ///
    /// ⚠ 实测过一条**不管用**的修法，别照抄：在捕获那条里调
    /// `tracing::callsite::rebuild_interest_cache()` —— 它按**全局**订阅器重建，而
    /// `set_default` 是**线程局部**的 ⇒ 带着它照样复现 ✗（当时第一跑就红了 ✓）。
    /// 从**源头**堵才确定：让每个会命中那些 callsite 的测试都装上订阅器 ✓。
    fn quiet() -> tracing::subscriber::DefaultGuard {
        let sub = tracing_subscriber::fmt()
            .with_writer(std::io::sink)
            .finish();
        tracing::subscriber::set_default(sub)
    }

    #[tokio::test]
    async fn request_reaches_inner_and_comes_back_untouched() {
        let _quiet = quiet();
        let fake = Fake::new(200, r#"{"code":0}"#, Vec::new());
        let seen = fake.seen();
        let client = LoggingClient::new(fake, "acct-1");

        let resp = client
            .request(HttpRequest::post("https://host/api?token=abc"))
            .await
            .expect("不该失败");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, r#"{"code":0}"#);
        // 装饰器不许改请求内容（URL 原样到达内层 ✓）
        let got = seen.lock().expect("锁中毒");
        assert_eq!(
            got.last().expect("内层应收到请求").url,
            "https://host/api?token=abc"
        );
    }

    /// ⚠⚠ 这条钉的是 `traits.rs` 里那句警告 —— **装饰器吃默认实现会把二进制毁掉**。
    /// 用非 UTF-8 字节：走默认实现的话它们会变成 U+FFFD（有损文本），而且**不报错** ✗。
    #[tokio::test]
    async fn request_raw_is_forwarded_byte_for_byte() {
        let _quiet = quiet();
        let bytes = vec![0xff, 0xfe, 0x00, 0x80, 0x41, 0x42];
        let fake = Fake::new(200, "", bytes.clone());
        let client = LoggingClient::new(fake, "acct-1");

        let got = client
            .request_raw(HttpRequest::post("https://host/upload"))
            .await
            .expect("不该失败");
        assert_eq!(got, bytes, "原始字节被有损转换毁掉了");
    }

    #[test]
    fn safe_redacts_before_truncating() {
        // 凭证整体消失
        let out = safe("登录失败 password=hunter2，token=deadbeef", PREVIEW_CHARS);
        assert!(!out.contains("hunter2"), "{out}");
        assert!(!out.contains("deadbeef"), "{out}");
        // 裸 JWT 也要拦住（没有键名跟着的那种）
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert!(
            !safe(jwt, PREVIEW_CHARS).contains("eyJhbGciOi"),
            "裸 JWT 没被脱敏"
        );
        // 正常内容留着（脱敏不是"什么都藏起来"）
        assert!(safe("一切正常", PREVIEW_CHARS).contains("一切正常"));
    }

    #[test]
    fn safe_truncates_by_chars_not_bytes() {
        // 全中文：按字节切会把字符切坏（这里只断言"没切坏 + 带省略说明"）
        let long = "中".repeat(600);
        let out = safe(&long, 10);
        assert!(out.starts_with(&"中".repeat(10)), "{out}");
        assert!(out.contains("共 600 字符"), "该说明原文有多长：{out}");
        assert_eq!(out.chars().filter(|c| *c == '中').count(), 10);
    }

    /// 捕获 tracing 输出的 writer（`fmt()` 要一个 `MakeWriter` ⇒ 给可克隆的句柄）。
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("锁中毒").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// ⚠ 这条是"**它真的会打日志**"的钉子 —— 前面几条只测了纯函数与转发 ✓，
    /// 少了解释器的话它们照样全绿 ✗（复核那批改动的规矩：每个改动配会红的钉子 ✓）。
    ///
    /// 断言两件事：① 2xx 走 `debug` 且 **URL 已脱敏**；② ≥400 升到 `warn` 并带正文预览。
    /// 服务端默认过滤器是 `info,mcloud=debug` ⇒ ① 这一档开箱可见 ✓（不用改环境变量 ✓）。
    #[tokio::test]
    async fn emits_debug_line_and_warns_on_4xx() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let capture = Capture(Arc::clone(&buf));
        let sub = tracing_subscriber::fmt()
            .with_writer(move || capture.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish();
        // ⚠ `set_default` 是**线程局部** ⇒ 需要当前线程跑完（tokio::test 默认
        // current_thread ✓）。用 multi_thread 的话 await 可能换线程、日志就漏采 ✗。
        let _guard = tracing::subscriber::set_default(sub);

        // ① 2xx：debug 档，URL 脱敏
        let client = LoggingClient::new(Fake::new(200, "ok", Vec::new()), "acct-1");
        client
            .request(HttpRequest::post("https://host/api?token=deadbeef"))
            .await
            .expect("不该失败");
        let text = String::from_utf8(buf.lock().expect("锁中毒").clone()).expect("日志应是 UTF-8");
        assert!(text.contains("上游应答"), "没打出应答行：{text}");
        assert!(!text.contains("deadbeef"), "日志里带了 token：{text}");
        assert!(
            text.contains("https://host/api"),
            "该留下可定位的 URL 骨架：{text}"
        );

        // ② 403：升到 warn 并带正文预览（第三方出问题时最该看见的那一行 ✓）
        buf.lock().expect("锁中毒").clear();
        let client = LoggingClient::new(
            Fake::new(403, r#"{"code":403,"msg":"forbidden"}"#, Vec::new()),
            "acct-2",
        );
        client
            .request(HttpRequest::post("https://host/x"))
            .await
            .expect("不该失败");
        let text = String::from_utf8(buf.lock().expect("锁中毒").clone()).expect("日志应是 UTF-8");
        assert!(text.contains("WARN"), "4xx 该升到 warn：{text}");
        assert!(text.contains("forbidden"), "该带错误正文预览：{text}");
    }
}
