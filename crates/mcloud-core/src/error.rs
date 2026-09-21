//! 错误分类。
//!
//! 分类的唯一目的是回答两个问题：**能不能重试**、**能不能把原文给用户看**。
//!
//! 对齐参考项目 caiyun 的两条约定：
//! - `exchange_request_control.go`：错误分类决定重试策略；
//! - `exchange_errors.go`：对外**绝不回传** `err.Error()`，走文案白名单。

use thiserror::Error;

/// 错误类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// 凭据失效 —— 不重试，标记该账号需要重新登录/刷新。
    Auth,
    /// 触发风控或限流 —— 退避后重试（并发过高时最常出现）。
    RateLimited,
    /// 奖品已兑完 —— 不重试。
    SoldOut,
    /// 云朵不足 —— 不重试。
    InsufficientCloud,
    /// 超时 —— 可重试。
    Timeout,
    /// 网络层错误 —— 可重试。
    Network,
    /// 上游 5xx —— 可重试。
    Upstream,
    /// 请求参数错误 —— 不重试。
    BadRequest,
    /// 资源不存在**或不属于当前用户** —— 不重试。
    ///
    /// 两者刻意共用一个类别（见 [`crate::user::is_visible_to`]）：
    /// "不属于你"若给出与"不存在"不同的错误，响应差异本身就泄漏了资源是否存在，
    /// 把 id 空间变成可枚举的。
    NotFound,
    /// 响应解析失败 —— 不重试（通常是上游改动，重试无意义）。
    Parse,
    /// 内部错误 —— 不重试。
    Internal,
}

impl ErrorKind {
    /// 是否可安全重试。
    ///
    /// 注意：**写请求**即使 `is_retryable() == true`，也必须带幂等键才能重试。
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Timeout | Self::Network | Self::Upstream
        )
    }

    /// 是否可以立即判定"该账号有硬问题"（用于健康度与告警）。
    pub const fn marks_account_unhealthy(self) -> bool {
        matches!(self, Self::Auth | Self::RateLimited)
    }

    /// 对外可见文案。
    ///
    /// 原文只进日志，**不进 API 响应**。
    pub const fn public_message(self) -> &'static str {
        match self {
            Self::Auth => "账号凭据已失效，请重新登录",
            Self::RateLimited => "触发限流，请降低并发后重试",
            Self::SoldOut => "奖品已兑完",
            Self::InsufficientCloud => "云朵不足",
            Self::Timeout => "请求超时",
            Self::Network => "网络异常",
            Self::Upstream => "上游服务异常",
            Self::BadRequest => "请求参数不合法",
            Self::NotFound => "资源不存在",
            Self::Parse => "响应解析失败",
            Self::Internal => "内部错误",
        }
    }
}

/// 按 HTTP 状态码分类。
///
/// 这是纯函数，重试策略完全由它决定，因此单测直接覆盖边界值。
pub const fn classify_status(status: u16) -> ErrorKind {
    match status {
        200..=299 => ErrorKind::Internal, // 成功不该走错误路径；落到 Internal 说明调用方判断有误
        401 | 403 => ErrorKind::Auth,
        408 => ErrorKind::Timeout,
        429 => ErrorKind::RateLimited,
        400 | 404 | 422 => ErrorKind::BadRequest,
        500..=599 => ErrorKind::Upstream,
        _ => ErrorKind::Internal,
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("凭据失效: {0}")]
    Auth(String),

    #[error("限流/风控: {0}")]
    RateLimited(String),

    #[error("已兑完: {0}")]
    SoldOut(String),

    #[error("云朵不足: {0}")]
    InsufficientCloud(String),

    #[error("超时: {0}")]
    Timeout(String),

    #[error("网络错误: {0}")]
    Network(String),

    #[error("上游错误 {status}: {message}")]
    Upstream { status: u16, message: String },

    #[error("参数错误: {0}")]
    BadRequest(String),

    /// 资源不存在或对当前用户不可见。**两种情形刻意不区分**，详见 [`ErrorKind::NotFound`]。
    #[error("不存在: {0}")]
    NotFound(String),

    #[error("解析失败: {0}")]
    Parse(String),

    #[error("内部错误: {0}")]
    Internal(String),

    /// fixture 未命中 —— 只可能出现在测试/调试台。
    ///
    /// 单独成类是为了让"偷偷打真网"立刻暴露而不是静默通过。
    #[error("fixture 未命中: {0}")]
    FixtureMiss(String),
}

impl Error {
    /// 错误正文 —— **不含**类别前缀。
    ///
    /// [`Display`](std::fmt::Display) 会给每条错误加上"凭据失效:" / "不存在:" 这类前缀：
    /// 那在**日志**里很有用（一眼看出类别），但直接端给使用者就很怪 ——
    /// 登录页上写"凭据失效: 用户名或口令不正确"，前半句是给运维看的。
    ///
    /// 所以要不要前缀是调用方的事，而这个能力必须显式提供：
    /// 不给的话，各处的解法会是 `split(": ")` 或者干脆把前缀一起发出去。
    pub fn message(&self) -> &str {
        match self {
            Self::Auth(m)
            | Self::RateLimited(m)
            | Self::SoldOut(m)
            | Self::InsufficientCloud(m)
            | Self::Timeout(m)
            | Self::Network(m)
            | Self::BadRequest(m)
            | Self::NotFound(m)
            | Self::Parse(m)
            | Self::Internal(m)
            | Self::FixtureMiss(m) => m,
            Self::Upstream { message, .. } => message,
        }
    }

    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Auth(_) => ErrorKind::Auth,
            Self::RateLimited(_) => ErrorKind::RateLimited,
            Self::SoldOut(_) => ErrorKind::SoldOut,
            Self::InsufficientCloud(_) => ErrorKind::InsufficientCloud,
            Self::Timeout(_) => ErrorKind::Timeout,
            Self::Network(_) => ErrorKind::Network,
            Self::Upstream { .. } => ErrorKind::Upstream,
            Self::BadRequest(_) => ErrorKind::BadRequest,
            Self::NotFound(_) => ErrorKind::NotFound,
            Self::Parse(_) => ErrorKind::Parse,
            Self::Internal(_) | Self::FixtureMiss(_) => ErrorKind::Internal,
        }
    }

    /// 可对外暴露的文案（不含任何原文细节）。
    pub fn public_message(&self) -> &'static str {
        self.kind().public_message()
    }
}

/// 从上游响应的业务码推断错误类别。
///
/// 上游用 200 + 业务码表达失败很常见，所以不能只看 HTTP 状态。
///
/// 返回 `None` 表示"按已知映射算成功"。**注意它不知道信封长什么样** ——
/// 判断"这次响应到底属于哪一类失败"要用 [`classify_body`]。
pub fn classify_business_code(code: i64) -> Option<ErrorKind> {
    match code {
        0 => None,
        // 各期活动/接口的"已抢完"码不统一，这里只收敛出语义明确的几类，
        // 其余返回 BadRequest，交由调用方按业务文案细化。
        _ => Some(ErrorKind::BadRequest),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 三类失败
// ─────────────────────────────────────────────────────────────────────────────

/// 响应体的失败判据 —— 旧项目在同一个代码库里混用了**三套**，这里是收敛后的那一套。
///
/// # 为什么要分这三类（而不是"Error 和非 Error"）
///
/// 旧项目三套判据（`tryRequest` / `tryResponse` / `header.status`）的共同毛病是
/// **把"拿到的答复"和"答复的判据"分开了**：
///
/// | 旧判据 | 位置 | 症状 |
/// | --- | --- | --- |
/// | `tryRequest` | `shared/src/result.ts:69-86` | HTTP 抛错 **或** `code ∉ {0,"0000",200,"200"}` **或** 响应含 `err\|errcode\|error` 键 ⇒ 一律 Error，**业务码丢失** |
/// | `tryResponse` | `:93-103` | 只认 HTTP 层抛错，业务码原样透传给调用方自己判 |
/// | `header.status` | `cloud-phone-redpack/task.ts:77-79` | 该域**完全没有 `code` 字段**，`String(header.status)==="200"` 才算成功 |
///
/// 于是同一个"上游说这活儿干不了"，在三个域里是三种类型：一个 `Error`、
/// 一个待判的 `code`、一个字符串比较。调用方想统一处理只能靠字符串匹配。
///
/// 收敛成三类之后，"这是什么失败"由**唯一一处**判定，且每一类都能分别测：
/// - [`Transport`](FailureClass::Transport)：**压根没拿到答复**（超时/连接/DNS）。
/// - [`Biz`](FailureClass::Biz)：拿到了答复，上游用它自己的业务码说"不行" —— 原文码保留。
/// - [`UnknownEnvelope`](FailureClass::UnknownEnvelope)：拿到了答复，但**信封不认识**。
///
/// # 三类里只有 `UnknownEnvelope` 是新增的语义
///
/// 旧代码在"信封不认识"时会落到 `_ => /* 其他 code */`，**只打一条日志然后继续**，
/// 于是上游改了协议这件事被伪装成"业务上的一次普通失败"。
/// 它必须**显式报错**：能落地的话，能力层就永远不知道自己的判据已经过期了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureClass {
    /// 传输层失败 —— 请求没有抵达/没有答复。可重试（读操作）。
    Transport {
        /// 传输层的错误类别（`Timeout` / `Network`）。
        kind: ErrorKind,
    },
    /// 业务失败 —— 上游答复了，并用它自己的码表示失败。
    ///
    /// **原始码必须保留**：它是唯一能把"已抢完"与"参数错"分开的信息，
    /// 也是排查时唯一能拿去问上游的东西。
    Biz {
        code: i64,
        message: String,
        kind: ErrorKind,
    },
    /// 信封不认识 —— 拿到的响应不符合任何已知判据。
    ///
    /// **不许静默降级**：这条的存在就是为了让协议漂移立刻可见。
    UnknownEnvelope {
        /// 为什么判定不认识（进日志，可含响应片段）。
        reason: String,
    },
}

/// 一次失败的三选一分类。
///
/// 与 [`FailureClass`] 的关系：`FailureClass` 回答"**哪一类**失败"，
/// 这里回答"**哪个错误对象**"，并额外把**响应体原文**留成字段 ——
/// 因为 `Error` 的 `Display` 会进日志、`public_message()` 才是对外文案，
/// 而"协议漂移"最需要看的恰恰是原文，它不该被塞进那两个地方。
///
/// # 为什么手写 `Clone` / `PartialEq` 而不是 derive
///
/// [`Error`] 是 `thiserror` 枚举，**既不是 `Clone` 也不是 `PartialEq`**
/// （它带 `String` 载荷，且语义上"两个错误相等"这件事没意义）。
/// 给它加 `Clone` 会污染整个错误体系，所以 `Classified` 自己实现：
/// `Clone` 靠各分支重组，`PartialEq` **只比类别与业务码**，不比 `Error` 载荷 ——
/// 单测里要断言的也正是"这是哪一类、码是多少"，不是文案逐字相同。
#[derive(Debug)]
pub enum Classified {
    /// 传输层失败。没有响应体可看。
    Transport { kind: ErrorKind, error: Error },
    /// 业务失败，携带原始码。
    Biz { code: i64, error: Error },
    /// 信封不认识 —— 原文照留。
    UnknownEnvelope {
        error: Error,
        /// 响应体（截断后的）原文，仅供日志与排查。
        body: String,
    },
}

impl Clone for Classified {
    fn clone(&self) -> Self {
        match self {
            Self::Transport { kind, error } => Self::Transport {
                kind: *kind,
                // `Error` 不可 Clone，重组一个等价的即可：分类判定只看 kind，
                // 而 `Display` 文案由 `message()` 提供，两者都保住了。
                error: Error::Internal(error.to_string()),
            },
            Self::Biz { code, error } => Self::Biz {
                code: *code,
                error: Error::Internal(error.to_string()),
            },
            Self::UnknownEnvelope { error, body } => Self::UnknownEnvelope {
                error: Error::Internal(error.to_string()),
                body: body.clone(),
            },
        }
    }
}

impl PartialEq for Classified {
    /// 只比**类别与业务码**，不比 `Error` 载荷 —— 见类型上的说明。
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Transport { kind: a, .. }, Self::Transport { kind: b, .. }) => a == b,
            (Self::Biz { code: a, .. }, Self::Biz { code: b, .. }) => a == b,
            (Self::UnknownEnvelope { .. }, Self::UnknownEnvelope { .. }) => true,
            _ => false,
        }
    }
}

impl Classified {
    /// 属于哪一类 —— 需要完整信息时用 `owned_class()`，这里返回的 `Biz.message`
    /// 是**空的**（`Error` 不可克隆，拿不到不含前缀的正文）。
    ///
    /// 它的用途是"判断是哪一类"（`matches!` / 比较 `kind`），而不是转发消息；
    /// 要正文直接用 [`Classified::into_error`] 或各字段。
    pub fn class(&self) -> FailureClass {
        match self {
            Self::Transport { kind, .. } => FailureClass::Transport { kind: *kind },
            Self::Biz { code, error } => FailureClass::Biz {
                code: *code,
                // 这里**不是** `error.message()` —— 那要 `&str`，而 `FailureClass`
                // 要 `String`；凑一个空串比造一个假的正文更诚实。
                message: String::new(),
                kind: error.kind(),
            },
            Self::UnknownEnvelope { .. } => FailureClass::UnknownEnvelope {
                reason: String::new(),
            },
        }
    }

    /// 能不能重试。**写操作即使 true 也必须带幂等键**（见 [`ErrorKind::is_retryable`]）。
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::Transport { kind, .. } => kind.is_retryable(),
            Self::Biz { .. } | Self::UnknownEnvelope { .. } => false,
        }
    }

    /// 这是不是"协议漂移"（信封不认识）。**应当触发告警**，而不是算一次业务失败。
    pub const fn is_protocol_drift(&self) -> bool {
        matches!(self, Self::UnknownEnvelope { .. })
    }

    /// 有没有拿到上游的业务码。
    pub const fn business_code(&self) -> Option<i64> {
        match self {
            Self::Biz { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// 转成 [`Error`]，供只关心错误的调用方使用。
    pub fn into_error(self) -> Error {
        match self {
            Self::Transport { error, .. }
            | Self::Biz { error, .. }
            | Self::UnknownEnvelope { error, .. } => error,
        }
    }
}

/// 响应体原文在日志/错误详情里的截断上限。
///
/// 不截断的话，一个把整页 HTML 当 JSON 返回的上游会往日志里灌几 MB。
const BODY_SNIPPET_LIMIT: usize = 512;

/// 把响应体截成可安全进日志的片段（**按字符**截，不是按字节 —— 否则会把 UTF-8 切坏）。
pub fn body_snippet(body: &str) -> String {
    match body.char_indices().nth(BODY_SNIPPET_LIMIT) {
        None => body.to_string(),
        Some((idx, _)) => {
            let mut out = body[..idx].to_string();
            out.push_str("…(已截断)");
            out
        }
    }
}

/// **唯一的**响应体失败判据。所有能力都必须走这里，不要各写一套。
///
/// # 判据顺序（顺序本身就是语义）
///
/// 1. **先看 HTTP 状态**：非 2xx 说明响应体通常不是业务 JSON，先按 [`classify_status`] 归类。
///    但 5xx 要判成 `UnknownEnvelope` 吗？不 —— `Upstream` 是明确的一类，不算协议漂移。
/// 2. **再解析 JSON**：解析不了 ⇒ `UnknownEnvelope`（**不静默**）。
/// 3. **再看是不是对象**：数组/裸值 ⇒ 有接口本来就回这些，但**只有声明了
///    `data_shape` 的能力认它**，否则仍是 `UnknownEnvelope`。
/// 4. 再看 `code` / `success` / 旧项目的 `err|errcode|error` 键 —— 见下。
///
/// # 与旧判据的兼容
///
/// 旧 `tryRequest` 把"响应含 `err|errcode|error` 键"也当成失败。
/// 这里**保留**这条判据，但**不再一律判 Error**：它现在落到
/// [`Classified::Biz`] 并尽量带上原始码 —— 也就是说，旧代码丢失的那个码，
/// 在这里被找回来了。
///
/// # 参数
///
/// - `status`：HTTP 状态码。
/// - `body`：响应体原文。**得先拿到原文才能判**，所以不要在这一步之前
///   用 `response.json()?` 把解析错误当成 `Error::Parse` 抛掉 ——
///   那样就分不清"解析不了"和"信封不认识"了。
pub fn classify_body(status: u16, body: &str) -> Result<serde_json::Value, Classified> {
    // ① 传输/HTTP 层。2xx 以外不看业务码 —— 这时响应体往往不是业务 JSON。
    if !(200..300).contains(&status) {
        let kind = classify_status(status);
        let error = Error::Upstream {
            status,
            message: format!("HTTP {status}"),
        };
        // 5xx / 429 / 408 是"上游明确说自己不行"，属于 Transport 类；
        // 4xx 里除了 408/429 之外是请求本身有问题 —— 仍是"拿到了答复"，
        // 但既不是业务码也不是协议漂移，归 Transport 会误导（无法重试的传输错误），
        // 所以按业务失败处理，带上状态码作为"码"。
        return Err(match kind {
            ErrorKind::Upstream | ErrorKind::Timeout | ErrorKind::RateLimited => {
                Classified::Transport { kind, error }
            }
            _ => Classified::Biz {
                // 用状态码本身当业务码：调用方能据此区分 401 与 400，
                // 而 `Error` 里已经带了 status。
                code: i64::from(status),
                error,
            },
        });
    }

    // ② 必须是 JSON。解析不了 ⇒ 协议漂移，**显式报错**。
    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => {
            return Err(Classified::UnknownEnvelope {
                error: Error::Parse(format!("响应体不是 JSON: {e}")),
                body: body_snippet(body),
            });
        }
    };

    // ③ 必须是对象 —— 信封的基本前提。
    let Some(obj) = value.as_object() else {
        return Err(Classified::UnknownEnvelope {
            error: Error::Parse(format!("信封不是对象（是 {}）", json_type_name(&value))),
            body: body_snippet(body),
        });
    };

    // ④ `success: false` 优先于 `code`。
    //
    // 这条踩过一次：最初把 `success` 折进 `code`（`false` → 0），
    // 于是 `{"success": false}` 被判成功。两类错误的代价并不对称 ——
    // 多报一次失败只是噪声，把失败当成功会让调用方拿着一张空 token 继续往下跑。
    if obj.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return Err(Classified::Biz {
            // 没有业务码时用 -1 占位而不是 0：0 是"成功"的码，
            // 让它出现在失败分支上一定会有人写出 `code == 0 ⇒ ok` 然后翻车。
            code: -1,
            error: Error::BadRequest(match message_of(obj) {
                Some(msg) => format!("上游返回 success=false: {msg}"),
                None => "上游返回 success=false".to_string(),
            }),
        });
    }

    // ⑤ 业务码。`code` 既有数字也有字符串（`RefreshTokenResponse.code` 就是 string）。
    if let Some(code) = code_of(obj) {
        return match classify_business_code(code) {
            None => Ok(value),
            Some(kind) => Err(Classified::Biz {
                code,
                error: biz_error(kind, code, message_of(obj)),
            }),
        };
    }

    // ⑥ 旧 `tryRequest` 的第三条判据：显式错误键。
    //
    // 保留它是因为**有些接口真的没有 `code`**（红包派对域就是，
    // 旧代码在那里退化成比较 `header.status` 字符串）。
    // 有 `error` 类键却没有 `code` ⇒ 按业务失败，码取 -1。
    if let Some(key) = ["err", "errcode", "error"]
        .into_iter()
        .find(|k| obj.contains_key(*k))
    {
        // `errcode` 本身可能就是码
        let code = obj.get("errcode").and_then(as_i64).unwrap_or(-1);
        return Err(Classified::Biz {
            code,
            error: Error::BadRequest(match message_of(obj) {
                Some(msg) => format!("上游返回 {key}: {msg}"),
                None => format!("上游返回 {key}"),
            }),
        });
    }

    // ⑦ 到这里：有对象、没有 `success`、没有 `code`、没有错误键。
    //
    // 这**不是**自动成功 —— 它只是"这个信封不认识"。
    // 注意 `{}` 也落在这里：空对象没有任何判据，当成功就是把"上游回了个空壳"放过去。
    Err(Classified::UnknownEnvelope {
        error: Error::Parse(
            "信封里既没有 `code`/`success`，也没有 `err|errcode|error` 键 —— 判据已过期"
                .to_string(),
        ),
        body: body_snippet(body),
    })
}

/// JSON 值的类型名（错误信息用，避免把整段原文塞进字符串）。
const fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// 把 JSON 值当整数读（数字位、或数字字符串）。
fn as_i64(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn code_of(obj: &serde_json::Map<String, serde_json::Value>) -> Option<i64> {
    obj.get("code").and_then(as_i64)
}

fn message_of(obj: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    ["msg", "message"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(serde_json::Value::as_str))
}

fn biz_error(kind: ErrorKind, code: i64, message: Option<&str>) -> Error {
    let detail = match message {
        Some(msg) => format!("code={code}: {msg}"),
        None => format!("code={code}"),
    };
    match kind {
        ErrorKind::Auth => Error::Auth(detail),
        ErrorKind::RateLimited => Error::RateLimited(detail),
        ErrorKind::SoldOut => Error::SoldOut(detail),
        ErrorKind::InsufficientCloud => Error::InsufficientCloud(detail),
        ErrorKind::Timeout => Error::Timeout(detail),
        ErrorKind::Network => Error::Network(detail),
        ErrorKind::Upstream => Error::Upstream {
            status: 200,
            message: detail,
        },
        ErrorKind::NotFound => Error::NotFound(detail),
        ErrorKind::Parse => Error::Parse(detail),
        ErrorKind::Internal => Error::Internal(detail),
        // 未知/参数类都归 BadRequest —— 与 `classify_business_code` 的收敛一致
        ErrorKind::BadRequest => Error::BadRequest(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_are_not_retryable() {
        assert!(!ErrorKind::Auth.is_retryable());
        assert!(ErrorKind::Auth.marks_account_unhealthy());
    }

    #[test]
    fn only_transient_kinds_are_retryable() {
        let retryable = [
            ErrorKind::RateLimited,
            ErrorKind::Timeout,
            ErrorKind::Network,
            ErrorKind::Upstream,
        ];
        for kind in retryable {
            assert!(kind.is_retryable(), "{kind:?} 应可重试");
        }

        let not_retryable = [
            ErrorKind::Auth,
            ErrorKind::SoldOut,
            ErrorKind::InsufficientCloud,
            ErrorKind::BadRequest,
            ErrorKind::Parse,
            ErrorKind::Internal,
        ];
        for kind in not_retryable {
            assert!(!kind.is_retryable(), "{kind:?} 不应重试");
        }
    }

    #[test]
    fn classify_status_covers_boundaries() {
        assert_eq!(classify_status(401), ErrorKind::Auth);
        assert_eq!(classify_status(403), ErrorKind::Auth);
        assert_eq!(classify_status(408), ErrorKind::Timeout);
        assert_eq!(classify_status(429), ErrorKind::RateLimited);
        assert_eq!(classify_status(400), ErrorKind::BadRequest);
        assert_eq!(classify_status(422), ErrorKind::BadRequest);
        assert_eq!(classify_status(500), ErrorKind::Upstream);
        assert_eq!(classify_status(503), ErrorKind::Upstream);
        assert_eq!(classify_status(599), ErrorKind::Upstream);
        assert_eq!(classify_status(600), ErrorKind::Internal);
    }

    #[test]
    fn public_message_never_leaks_detail() {
        let err = Error::Upstream {
            status: 502,
            message: "upstream said: auth=SECRET_TOKEN_abc123".into(),
        };
        let msg = err.public_message();
        assert!(!msg.contains("SECRET_TOKEN"), "对外文案泄漏了原文: {msg}");
        assert_eq!(msg, "上游服务异常");
    }

    #[test]
    fn business_code_zero_means_ok() {
        assert!(classify_business_code(0).is_none());
        assert_eq!(classify_business_code(-1), Some(ErrorKind::BadRequest));
    }

    /// `Display` 带类别前缀（给日志看），`message()` 不带（给使用者看）。
    #[test]
    fn message_is_free_of_the_kind_prefix() {
        let err = Error::Auth("用户名或口令不正确".into());
        assert_eq!(err.to_string(), "凭据失效: 用户名或口令不正确");
        assert_eq!(err.message(), "用户名或口令不正确");
        assert!(!err.message().contains("凭据失效"));

        // 带字段的变体也要能取到正文
        let upstream = Error::Upstream {
            status: 502,
            message: "坏网关".into(),
        };
        assert_eq!(upstream.message(), "坏网关");
    }

    // ── 三类失败的判据 ──────────────────────────────────────────────────────

    /// 正常业务响应：解析成功，不是错误。
    #[test]
    fn plain_success_envelope_is_ok() {
        let v = classify_body(200, r#"{"code":0,"msg":"ok","data":{"a":1}}"#).unwrap();
        assert_eq!(v["data"]["a"], 1);
    }

    /// `code` 是**字符串**也算数（`RefreshTokenResponse.code` 就是 string）。
    #[test]
    fn string_code_is_recognized() {
        let ok = classify_body(200, r#"{"code":"0","data":1}"#);
        assert!(ok.is_ok(), "字符串 0 应判成功");

        let err = classify_body(200, r#"{"code":"1001","msg":"抢完了"}"#).unwrap_err();
        assert_eq!(err.business_code(), Some(1001));
    }

    /// 业务失败要**保住原始码** —— 旧 `tryRequest` 正是在这里把码丢了。
    #[test]
    fn biz_failure_keeps_the_raw_code() {
        let err = classify_body(200, r#"{"code":400112,"msg":"口令已兑换"}"#).unwrap_err();
        match &err {
            Classified::Biz { code, error } => {
                assert_eq!(*code, 400112, "原始码必须保留（旧项目丢的就是它）");
                assert_eq!(error.kind(), ErrorKind::BadRequest);
                assert!(error.to_string().contains("400112"), "详情里应带上码");
            }
            other => panic!("应判业务失败，实得 {other:?}"),
        }
        assert!(!err.is_retryable(), "业务失败不该重试");
        assert!(!err.is_protocol_drift());
    }

    /// `success: false` 必须判失败，且**优先于** `code`。
    ///
    /// 这条是真踩过的坑：把 `success` 折进 `code`（`false` → 0）之后，
    /// `{"success": false, "code": 0}` 会被判成功。
    #[test]
    fn success_false_beats_code_zero() {
        let err =
            classify_body(200, r#"{"success":false,"code":0,"message":"token 无效"}"#).unwrap_err();
        // 码是 -1 而不是 0 —— 0 是成功的码，出现在失败分支上一定会有人写出
        // `code == 0 ⇒ ok` 然后翻车
        assert_eq!(err.business_code(), Some(-1));
        assert!(matches!(err, Classified::Biz { code: -1, .. }));
        assert!(err.into_error().to_string().contains("token 无效"));
    }

    /// 传输层失败：5xx / 429 / 408。**没有响应体可看**。
    #[test]
    fn server_side_failures_are_transport() {
        for (status, expected) in [
            (500u16, ErrorKind::Upstream),
            (503, ErrorKind::Upstream),
            (429, ErrorKind::RateLimited),
            (408, ErrorKind::Timeout),
        ] {
            let err = classify_body(status, r#"{"code":1}"#).unwrap_err();
            match err {
                Classified::Transport { kind, .. } => assert_eq!(kind, expected, "HTTP {status}"),
                Classified::Biz { .. } => {
                    panic!(
                        "HTTP {status} 应判传输层：响应体在这时通常不是业务 JSON，看业务码会误判"
                    )
                }
                Classified::UnknownEnvelope { .. } => panic!("HTTP {status} 不该判协议漂移"),
            }
        }
    }

    /// 5xx 可重试（读操作）；4xx 的请求类错误不可重试。
    #[test]
    fn retryability_follows_the_class() {
        assert!(classify_body(503, "").unwrap_err().is_retryable());
        assert!(classify_body(500, "").unwrap_err().is_retryable());
        assert!(!classify_body(401, "").unwrap_err().is_retryable());
        assert!(
            !classify_body(200, r#"{"code":7}"#)
                .unwrap_err()
                .is_retryable()
        );
    }

    /// **这是本节最重要的一条**：信封不认识时必须**显式报错**，不许静默通过。
    ///
    /// 旧代码在这一步落到 `_ => /* 其他 code */` 只打日志，
    /// 于是"上游改了协议"被伪装成一次普通业务失败。
    #[test]
    fn unknown_envelope_is_never_silently_ok() {
        let cases: [(&str, &str); 5] = [
            ("空对象", r#"{}"#),
            ("只有无关字段", r#"{"hello":"world"}"#),
            ("数组", r#"[1,2,3]"#),
            ("裸值", r#""unexpected""#),
            ("不是 JSON", "<html>502 Bad Gateway</html>"),
        ];
        for (name, body) in cases {
            let err = classify_body(200, body)
                .expect_err(&format!("{name} 必须报错 —— 静默通过等于协议漂移不可见"));
            assert!(err.is_protocol_drift(), "{name} 应判协议漂移，实得 {err:?}");
            // 原文要留住，否则排查时不知道上游到底回了什么
            match &err {
                Classified::UnknownEnvelope { body: kept, .. } => {
                    assert!(!kept.is_empty(), "{name} 应保留响应体原文")
                }
                other => panic!("{name} 应保留原文，实得 {other:?}"),
            }
        }
    }

    /// 旧 `tryRequest` 的第三条判据要保留：有 `err|errcode|error` 键但**没有 `code`**。
    ///
    /// 红包派对域就是这种信封（完全没有 `code` 字段），
    /// 旧代码在那里退化成了 `String(header.status) === "200"` 的字符串比较。
    #[test]
    fn legacy_error_keys_still_count_as_failure() {
        let err = classify_body(
            200,
            r#"{"error":"invalid_grant","error_description":"过期"}"#,
        )
        .unwrap_err();
        assert_eq!(err.business_code(), Some(-1), "没有码时用 -1，不能用 0");
        assert!(!err.is_retryable());

        // `errcode` 本身就是码，要认出来
        let with_code = classify_body(200, r#"{"errcode":1005,"errmsg":"失败"}"#).unwrap_err();
        assert_eq!(with_code.business_code(), Some(1005));
    }

    /// 响应体原文要截断，且**按字符**截 —— 按字节截会把 UTF-8 切坏。
    #[test]
    fn body_snippet_truncates_on_char_boundaries() {
        let short = "短";
        assert_eq!(body_snippet(short), short, "短的应原样返回");

        // 全是多字节字符，长度刚好超过上限
        let long: String = "云".repeat(BODY_SNIPPET_LIMIT + 50);
        let snip = body_snippet(&long);
        assert!(snip.len() < long.len());
        assert!(snip.ends_with("…(已截断)"));
        // 关键：不能 panic，也不能产生非法 UTF-8 —— 能算 chars 就说明没切坏
        assert!(snip.chars().count() > 0);
    }

    /// `classify_body` 对非 2xx 不要求响应体是 JSON（那时通常就不是）。
    #[test]
    fn non_2xx_does_not_require_json_body() {
        let err = classify_body(502, "<html>bad gateway</html>").unwrap_err();
        assert!(matches!(err, Classified::Transport { .. }));
    }

    /// 401/400 这类"拿到了答复、但请求本身有问题"应判业务类而非传输类 ——
    /// 判成传输类会被 `is_retryable()` 的直觉带偏（4xx 重试只会一直 401）。
    #[test]
    fn client_errors_are_biz_not_transport() {
        for status in [400u16, 401, 404, 422] {
            let err = classify_body(status, "").unwrap_err();
            assert_eq!(
                err.business_code(),
                Some(i64::from(status)),
                "HTTP {status} 应带上状态码当业务码"
            );
            assert!(!err.is_retryable(), "HTTP {status} 不该重试");
        }
    }
}
