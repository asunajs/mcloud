//! mcloud-upstream —— 上游接口封装层。
//!
//! # 状态
//!
//! - [`login`]：**已实现**。依据 `docs/upstream-login-protocol.md` 与
//!   `docs/upstream-logout-sms-login.md`（新抓包 + 客户端源码双向确认）——
//!   这是"按新抓包重写"落地的第一块，也是产物能真正跑起来的起点（一切接口都要凭据）。
//! - 其余云盘业务接口：**等重新抓包后再写**，不搬旧 TS（旧 TS 与线上请求有多处不一致，
//!   详见 `README.md` 的「现状须知」）。
//!
//! 骨架部分保留三样东西：
//!
//! 1. [`Upstream`]：注入 `HttpClient` 的调用上下文（本层禁止自己造 HTTP 客户端）；
//! 2. [`Envelope`]：上游响应信封（`code` / `msg` / `data|result` 的取值与业务码判定）；
//! 3. [`base`]：上游基址常量。
//!
//! # 写接口时的两条硬规矩（上一轮对拍换来的教训）
//!
//! - **以真实流量为准，而不是以 TS 代码为准**。旧 TS 与线上实际请求有多处不一致；
//!   新抓包进来后同样要先对拍再落笔。
//! - 每个接口函数都要能被 `cargo test -p mcloud-upstream` 离线回放（L2）+ 逐字段断言 ——
//!   只断言"没报错"等于没测。`login` 模块的 body 构造函数全是纯函数，
//!   所以在拿到登录流量的夹具之前，也能按文档逐字段钉住。

#![forbid(unsafe_code)]

pub mod client;
pub mod login;

use mcloud_core::{Error, ErrorKind, HttpClient, HttpRequest, classify_status};
use serde_json::Value;

/// 上游接口基址（自旧实现整理，供重写时对照；本层不发起真实请求）。
pub mod base {
    /// 主站：签到、兑换、用户域、认证。
    pub const MCLOUD_SIGNIN: &str = "https://m.mcloud.139.com/ycloud/signin";
    /// 主站根。
    pub const MCLOUD: &str = "https://m.mcloud.139.com";
    /// 彩云主入口（消息推送、备份礼物、老签到入口）。
    pub const CAIYUN: &str = "https://caiyun.feixin.10086.cn";
    /// 彩云 `ycloud/*` 入口。
    ///
    /// **端口不能省**：真实流量里 `ycloud/signin/...` 走的是 `:7071`，
    /// 而 TS 里同一批接口写的是不带端口的 `/market/signin/...`。两者是不同入口，
    /// 混用会打到错误的地址上。
    pub const CAIYUN_YCLOUD: &str = "https://caiyun.feixin.10086.cn:7071";
    /// 活动资源（任务扩展）。
    pub const MRP_MCLOUD: &str = "https://mrp.mcloud.139.com";
    /// 用户域（token 刷新、spec token）。
    pub const USER_YUN: &str = "https://user-njs.yun.139.com";
    /// 笔记服务。
    pub const NOTE_MCLOUD: &str = "https://note.mcloud.139.com";
    /// 认证服务。
    pub const AAS_CAIYUN: &str = "https://aas.caiyun.feixin.10086.cn";
    /// 139 邮箱。
    pub const MAIL_139: &str = "https://mail.10086.cn";
}

/// 客户端版本号，对齐 `@asunajs/shared/constants.ts` 的 `CAIYUN_APP_VERSION`。
///
/// 它是**业务参数**而不是展示信息：`exchangeList` / `exchangeV2` 都会带上它，
/// 改这个值等于把自己伪装成另一个版本的客户端（上游的开关与灰度常常按它分流）。
pub const CAIYUN_APP_VERSION: &str = "12.6.0";

/// 上游统一响应信封。
///
/// # 为什么不做成"万能结构"
///
/// 各接口的外层字段名并不统一：`{code, msg, result}`、`{code, msg, data}`、
/// `{success, code, message, data}`（`auth.ts` 的 `RefreshTokenResponse`）都有，
/// 有些接口（`taskListV2`）还带回 `groupState` 这样的平级业务字段。
/// 硬造一个统一结构只会让"实际上没有的字段"看起来存在，
/// 所以这里只收敛**两个**跨接口成立的动作：取业务码、取载荷。
#[derive(Debug, Clone)]
pub struct Envelope {
    raw: Value,
}

impl Envelope {
    pub const fn new(raw: Value) -> Self {
        Self { raw }
    }

    /// 原始响应体。接口自己有额外字段（`groupState` 之类）时从这里取。
    pub const fn raw(&self) -> &Value {
        &self.raw
    }

    /// 业务码。
    ///
    /// `code` 既有数字也有**字符串**（`RefreshTokenResponse.code` 就是 string），
    /// 两种都要认；没有 `code` 字段时返回 `None`
    /// —— 有些接口直接回数组或裸值，本来就没有信封。
    pub fn code(&self) -> Option<i64> {
        match self.raw.get("code") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        }
    }

    /// `success` 布尔位。有些接口（`RefreshTokenResponse`）用它表达成败。
    ///
    /// 与 [`Envelope::code`] **刻意分开**：把 `success` 折进 `code`
    /// （`false` → 0）会让 `{"success": false}` 被 [`Envelope::ensure_ok`] 判成成功 ——
    /// 见那里的说明。
    pub fn success(&self) -> Option<bool> {
        self.raw.get("success").and_then(Value::as_bool)
    }

    /// 人类可读的上游消息。**只进日志**，不要塞进 API 响应（见 `Error::public_message`）。
    pub fn message(&self) -> Option<&str> {
        ["msg", "message"]
            .iter()
            .find_map(|key| self.raw.get(*key).and_then(Value::as_str))
    }

    /// 业务载荷：`data` 或 `result`。
    pub fn payload(&self) -> Option<&Value> {
        self.raw.get("data").or_else(|| self.raw.get("result"))
    }

    /// 业务失败时返回错误，成功时给出载荷。
    ///
    /// 只做到"非 0 即失败"这一层：各期活动的"已抢完 / 云朵不足"码并不统一，
    /// 硬编码只会把某一期的码当成通用语义。细分交给调用方
    /// （见 `mcloud_core::classify_business_code` 的说明）。
    pub fn ensure_ok(&self) -> Result<Option<&Value>, Error> {
        // `success: false` 必须判失败，而且**优先于** `code`。
        //
        // 这里踩过一次：最初把 `success` 折进 `code`（`false` → `0`），
        // 于是 `{"success": false}` 被判成功。两类错误的代价并不对称 ——
        // 多报一次失败只是噪声，把失败当成功会让调用方拿着一张空 token 继续往下跑。
        if self.success() == Some(false) {
            return Err(Error::BadRequest(match self.message() {
                Some(msg) => format!("上游返回 success=false: {msg}"),
                None => "上游返回 success=false".to_string(),
            }));
        }
        match self.code() {
            Some(0) | None => Ok(self.payload()),
            Some(code) => Err(Error::BadRequest(match self.message() {
                Some(msg) => format!("上游返回 code={code}: {msg}"),
                None => format!("上游返回 code={code}"),
            })),
        }
    }
}

/// 把 HTTP 状态码映射成错误。
///
/// **不带响应体原文**：上游的错误体里常常直接回显凭据，
/// 而 `Error` 的 detail 会进日志、`public_message()` 才是对外文案 ——
/// 两头都不该出现上游原文，所以这里只保留状态码。
fn error_from_status(status: u16) -> Error {
    let detail = format!("HTTP {status}");
    match classify_status(status) {
        ErrorKind::Auth => Error::Auth(detail),
        ErrorKind::RateLimited => Error::RateLimited(detail),
        ErrorKind::Timeout => Error::Timeout(detail),
        ErrorKind::BadRequest => Error::BadRequest(detail),
        _ => Error::Upstream {
            status,
            message: detail,
        },
    }
}

/// 接口调用上下文：只携带"能发请求"的能力，不携带存储/时间。
///
/// 这样写的好处是 —— 上游层永远无法偷偷读文件或写数据库，
/// 它的可测性由类型系统保证而不是靠约定。
pub struct Upstream<'a, C: HttpClient> {
    pub http: &'a C,
}

impl<'a, C: HttpClient> Upstream<'a, C> {
    pub const fn new(http: &'a C) -> Self {
        Self { http }
    }

    /// 发请求并解析成信封。
    ///
    /// **业务失败不算错误**：非 2xx 是错误（这时响应体通常不是业务 JSON），
    /// 2xx 一律解析出信封交回调用方 —— 原实现也是这个语义，
    /// 调用方需要自己看 `code`（用 [`Envelope::ensure_ok`]）。
    pub async fn call(&self, request: HttpRequest) -> Result<Envelope, Error> {
        let response = self.http.send(request).await?;
        if !response.is_success() {
            return Err(error_from_status(response.status));
        }
        Ok(Envelope::new(response.json()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upstream_can_be_constructed_without_io() {
        // 这个断言的意义在于：`Upstream` 只要求一个 `HttpClient` 实现，
        // 不要求任何真实网络对象 —— 测试注入的前提在类型层面就成立了。
        struct NullClient;
        impl HttpClient for NullClient {
            async fn send(
                &self,
                _request: mcloud_core::HttpRequest,
            ) -> Result<mcloud_core::HttpResponse, mcloud_core::Error> {
                Err(mcloud_core::Error::Internal("null client".into()))
            }
        }

        let client = NullClient;
        let _upstream = Upstream::new(&client);
        assert!(base::MCLOUD_SIGNIN.starts_with("https://"));
    }

    #[test]
    fn envelope_reads_numeric_code_and_payload() {
        let env = Envelope::new(json!({"code": 0, "msg": "success", "result": {"a": 1}}));
        assert_eq!(env.code(), Some(0));
        assert_eq!(env.message(), Some("success"));
        assert_eq!(env.payload().unwrap()["a"], 1);
        assert!(env.ensure_ok().is_ok());
    }

    #[test]
    fn envelope_reads_string_code_and_success_flag() {
        // `RefreshTokenResponse` 的 code 是字符串 —— 只认数字会把它当成"没有信封"
        let string_code = Envelope::new(json!({"success": true, "code": "0", "message": "ok"}));
        assert_eq!(string_code.code(), Some(0));
        assert_eq!(string_code.success(), Some(true));
        assert!(string_code.ensure_ok().is_ok());

        // `success: false` 必须判失败，且**优先于** code。
        // 折进 code 会得到 0，于是失败被当成成功 —— 这是踩过的坑，用这条钉住。
        let failed = Envelope::new(json!({"success": false, "code": "0", "message": "凭据失效"}));
        assert!(failed.ensure_ok().is_err(), "success=false 不能被判成功");
    }

    #[test]
    fn envelope_without_any_code_is_treated_as_ok() {
        // 有些接口直接回数组（如 `taskListV2` 的 result），没有信封字段
        let bare = Envelope::new(json!([{"a": 1}]));
        assert_eq!(bare.code(), None);
        assert!(bare.ensure_ok().is_ok());
    }

    #[test]
    fn business_failure_carries_code_and_message_but_not_upstream_credentials() {
        let env = Envelope::new(json!({"code": 514, "msg": "验证失败"}));
        let err = env.ensure_ok().unwrap_err();
        let detail = format!("{err}");
        assert!(detail.contains("514"), "{detail}");
        assert!(detail.contains("验证失败"), "{detail}");
        // 对外文案走白名单，不夹带上游原文
        assert_eq!(err.public_message(), "请求参数不合法");
    }

    #[test]
    fn http_status_maps_to_error_kind() {
        assert!(matches!(error_from_status(401), Error::Auth(_)));
        assert!(matches!(error_from_status(429), Error::RateLimited(_)));
        assert!(matches!(error_from_status(408), Error::Timeout(_)));
        assert!(matches!(error_from_status(400), Error::BadRequest(_)));
        assert!(matches!(error_from_status(503), Error::Upstream { .. }));
    }
}
