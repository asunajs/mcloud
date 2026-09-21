//! 移动云盘登录：扫码与短信两条链路的**状态机**。
//!
//! # 为什么状态机在这里，而不是在 `mcloud-upstream::login`
//!
//! 上游那层只做两件事：**请求怎么拼**（逐字段对齐实测）与**响应怎么读**（业务码 → 语义）。
//! 本层负责"接下来怎么办"：什么时候再问一次、什么时候必须停、拿到 `userInfo` 之后落成什么。
//!
//! 这么切分是为了让 CLI 与 WebUI **共用同一份状态机**（方案 P2.5 的验收点）：
//! 两个入口的差别只是"谁来等这 1000 毫秒"和"谁来渲染二维码"，
//! 而不是各写一套轮询循环 —— 客户端源码里那句"登录成功后必须 `clearInterval`"
//! 说明这套循环很容易写错一次（停了不该停的、或者忘了停）。
//!
//! # 两个刻意的设计
//!
//! - **本层不 sleep**：`poll()` 只发一次请求并推进状态，等多久交给调用方
//!   （见 [`QrLogin::next_delay_ms`]）。于是"轮询节奏"能被单测直接断言，
//!   "每次等 1 秒"不需要靠真的等 1 秒来验证。
//! - **终态不可逆**：一旦出结果（成功或失败），后续 `poll()` 直接返回同一结论且**不发请求**。
//!   `clearInterval` 那条教训因此从"记得写"变成"写不错"。

use chrono::{DateTime, Duration, Utc};
use mcloud_core::{Account, AccountId, AccountState, Error, HttpClient, Random, Scope, StateStore};
use mcloud_upstream::client::ClientProfile;
use mcloud_upstream::login::{
    self, LoginPayloadDecryptor, PcPlatform, PollOutcome, SendSmsOutcome,
};
use mcloud_upstream::{Envelope, Upstream};
use serde_json::Value;

/// 扫码轮询间隔。实测客户端是 `setInterval` 1000ms（见 `docs/upstream-login-protocol.md` §3）。
pub const QR_POLL_INTERVAL_MS: u64 = 1000;

/// 二维码在我们这一侧的有效期。
///
/// # 这是一个**产品取值**，不是实测值
///
/// 归档里 `200059542 二维码已失效` 只出现过 1 次，推不出时长。这里取 120 秒：
/// 宁可早一点失效让用户重新出码（代价是一次点击），
/// 也不要让用户对着一个已经无效的码干等（代价是"这软件坏了"）。
pub const QR_TTL_SECONDS: i64 = 120;

/// 扫码登录的推进结果。
#[derive(Debug, Clone, PartialEq)]
pub enum QrProgress {
    /// 码还没被扫 —— 轮询期的绝大多数情况（实测 121/192），不是错误。
    NotScanned,
    /// 已扫码、等手机上确认 —— 唯一值得对外显示进度的中间态。
    Scanned,
    /// 成功，且 `user_info` 已解密。
    LoggedIn { user_info: Value },
    /// 终态失败。
    Failed { reason: QrFailure },
}

impl QrProgress {
    /// 是否已到终态（调用方据此停止轮询）。
    ///
    /// # 限流为什么**不是**终态
    ///
    /// `200059545 扫码登录过于频繁` 的语义是"慢一点再问"，会话本身仍然有效。
    /// 把它当终态会逼用户回去重开一张码，而重开只会更深地撞进风控 ——
    /// 所以这里返回 `false`，由调用方退避后继续问（总时长仍受 TTL 约束）。
    pub const fn is_terminal(&self) -> bool {
        match self {
            Self::LoggedIn { .. } => true,
            Self::Failed {
                reason: QrFailure::TooFrequent,
            } => false,
            Self::Failed { .. } => true,
            Self::NotScanned | Self::Scanned => false,
        }
    }
}

/// 扫码失败的几种终态。
#[derive(Debug, Clone, PartialEq)]
pub enum QrFailure {
    /// 我们自己的 TTL 到了（码可能还有效，但不再等）。
    TtlElapsed { ttl_seconds: i64 },
    /// 上游说码已失效（`200059542`）。
    ExpiredUpstream,
    /// `sID` 不匹配（`200059544`）：换码之后还在用旧 `sID` 轮询，属于调用方的状态错误。
    SessionMismatch,
    /// 触发限流（`200059545 扫码登录过于频繁`）—— **该退避**，不是换张码立刻重试。
    TooFrequent,
    /// 上游回了别的码。**原样带回**：上游会在不通知的情况下加新码，
    /// 硬编码成"未知错误"会让新码永远查不出来。
    Rejected { code: Option<i64>, message: String },
}

/// 扫码登录会话。
pub struct QrLogin {
    /// 端档案：平台、设备指纹、设备信息串（请求头与二维码都要用）。
    profile: ClientProfile,
    /// `sID`，同时就是 `thirdlogin` 的 `dycpwd`。
    session_id: String,
    started_at: DateTime<Utc>,
    /// 我们这一侧的有效期（默认 [`QR_TTL_SECONDS`]，可用 [`QrLogin::with_ttl`] 覆盖）。
    ttl_seconds: i64,
    /// 终态缓存。`None` = 还在进行中。
    finished: Option<QrProgress>,
}

impl QrLogin {
    /// 出码。
    ///
    /// **零网络请求**：二维码是本地拼的（源码 `createLoginQRcode()` 就是字符串拼接），
    /// 这是本次抓包推翻的直觉假设里最重要的一个。
    pub fn start(
        platform: PcPlatform,
        device_id: impl Into<String>,
        rng: &mut impl Random,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            profile: ClientProfile::new(platform, device_id),
            session_id: login::new_session_id(rng),
            started_at: now,
            ttl_seconds: QR_TTL_SECONDS,
            finished: None,
        }
    }

    /// 二维码载荷（交给前端渲染成二维码）。
    pub fn qr_payload(&self) -> String {
        login::qr_payload(
            self.profile.platform(),
            self.profile.device_id(),
            &self.session_id,
        )
    }

    /// 端档案（调用方需要它来发别的请求时用）。
    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }

    /// `sID`。换码时要一起换掉，否则会得到 [`QrFailure::SessionMismatch`]。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 覆盖默认有效期。
    ///
    /// 用于"把二维码发给别人扫"的场景（比如终端里打印出来、或者远程协助）——
    /// 那时 120 秒不够，而重出一次码的代价只是重新扫。
    pub fn with_ttl(mut self, seconds: i64) -> Self {
        self.ttl_seconds = seconds;
        self
    }

    /// 下次该隔多久再问；`None` = 别再问了。
    ///
    /// 终态与超时都返回 `None` —— 调用方只认这一个判断，
    /// 不需要自己再记一套"成功了吗 / 超时了吗"。
    pub fn next_delay_ms(&self, now: DateTime<Utc>) -> Option<u64> {
        if self.finished.is_some() {
            return None;
        }
        if self.is_expired(now) {
            return None;
        }
        Some(QR_POLL_INTERVAL_MS)
    }

    /// 是否已超过我们这一侧的 TTL。
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.started_at + Duration::seconds(self.ttl_seconds)
    }

    /// 轮询一次并推进状态。
    ///
    /// 已到终态时**不发请求**、直接返回上次的结论（对应客户端的 `clearInterval`）。
    pub async fn poll<C: HttpClient, D: LoginPayloadDecryptor>(
        &mut self,
        upstream: &Upstream<'_, C>,
        tid: &str,
        decryptor: &D,
        now: DateTime<Utc>,
    ) -> Result<QrProgress, Error> {
        if let Some(done) = &self.finished {
            return Ok(done.clone());
        }

        if self.is_expired(now) {
            let done = QrProgress::Failed {
                reason: QrFailure::TtlElapsed {
                    ttl_seconds: self.ttl_seconds,
                },
            };
            self.finished = Some(done.clone());
            return Ok(done);
        }

        let body = login::qr_thirdlogin_body(self.profile.platform(), &self.session_id);
        let envelope = upstream.thirdlogin(&self.profile, tid, &body).await?;

        let progress = match login::classify_poll(&envelope) {
            PollOutcome::NotScanned => QrProgress::NotScanned,
            PollOutcome::Scanned => QrProgress::Scanned,
            PollOutcome::Expired => QrProgress::Failed {
                reason: QrFailure::ExpiredUpstream,
            },
            PollOutcome::SessionMismatch => QrProgress::Failed {
                reason: QrFailure::SessionMismatch,
            },
            PollOutcome::TooFrequent => QrProgress::Failed {
                reason: QrFailure::TooFrequent,
            },
            // 成功：`data` 是密文，必须解密才能拿到 userInfo
            PollOutcome::Success { encrypted } => QrProgress::LoggedIn {
                user_info: decryptor.decrypt_user_info(&encrypted)?,
            },
            PollOutcome::Other { code, message } => QrProgress::Failed {
                reason: QrFailure::Rejected { code, message },
            },
        };

        if progress.is_terminal() {
            self.finished = Some(progress.clone());
        }
        Ok(progress)
    }
}

/// 短信登录会话。
///
/// 链路的完整形状（含降级）：
///
/// ```text
/// 发码 ──成功──▶ 等用户输验证码 ──▶ thirdlogin(pintype=5)
///  │
///  └─200059554──▶ 取滑块图 ──▶ 带滑块再发码
///                   │
///                   └─200059553──▶ **必须重新取图**（旧挑战作废），不能复用
/// ```
///
/// 所以本类型只提供"一步一步"的入口，把"要不要重试、取哪张图"留给调用方 ——
/// 滑块图是 IO（要渲染给人看），本层不碰。
pub struct SmsLogin {
    /// 端档案：同样是请求头必须带的那一套。
    profile: ClientProfile,
    phone: String,
    /// 发码请求里的 `random`：**24 位**十六进制（不是 UUID）。
    random: String,
    /// 服务端给的配额余量。跟踪它是为了不把配额无谓耗光。
    remain_get_times: Option<u32>,
}

impl SmsLogin {
    pub fn start(
        platform: PcPlatform,
        device_id: impl Into<String>,
        phone: impl Into<String>,
        rng: &mut impl Random,
    ) -> Self {
        Self {
            profile: ClientProfile::new(platform, device_id),
            phone: phone.into(),
            random: login::new_sms_random(rng),
            remain_get_times: None,
        }
    }

    pub fn phone(&self) -> &str {
        &self.phone
    }

    /// 端档案（取滑块图等额外请求要用）。
    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }

    /// 配额余量（服务端每次成功发码时返回）。
    pub const fn remain_get_times(&self) -> Option<u32> {
        self.remain_get_times
    }

    /// 本轮发码用的 `random`。换一次发码要换一个值，所以失败重试时用 [`SmsLogin::reroll`]。
    pub fn random(&self) -> &str {
        &self.random
    }

    /// 换一个新的 `random`（重试发码前调用）。
    pub fn reroll(&mut self, rng: &mut impl Random) {
        self.random = login::new_sms_random(rng);
    }

    /// 不带滑块的发码。
    pub async fn send_code<C: HttpClient>(
        &mut self,
        upstream: &Upstream<'_, C>,
        tid: &str,
    ) -> Result<SendSmsOutcome, Error> {
        let body = login::send_sms_code_body(self.profile.platform(), &self.phone, &self.random);
        self.finish_send(upstream.send_sms_code(&self.profile, tid, &body).await?)
    }

    /// 带滑块的发码。
    ///
    /// `slider_answer` 是**拖动像素偏移的十进制字符串**（实测两条真实请求解出来是 `461` / `391`）。
    /// 走这条路的前提是先调 `/user/verfycode` 拿到图 —— 那是调用方的事。
    pub async fn send_code_with_slider<C: HttpClient>(
        &mut self,
        upstream: &Upstream<'_, C>,
        tid: &str,
        slider_answer: &str,
    ) -> Result<SendSmsOutcome, Error> {
        let body = login::send_sms_code_body_with_slider(
            self.profile.platform(),
            &self.phone,
            &self.random,
            slider_answer,
        )?;
        self.finish_send(upstream.send_sms_code(&self.profile, tid, &body).await?)
    }

    /// 取滑块挑战并**自动解出**拖动答案。
    ///
    /// # 为什么不需要图像识别
    ///
    /// 服务端把缺口横坐标加密后直接给了出来（`data.puzzleLeft`，密钥是客户端源码里
    /// 那个公开常量），见 `mcloud_upstream::login::SliderPuzzle::answer`。
    /// 2026-09-18 实测解出 `370`，而同一响应的 `picWidth` 是 `680`。
    pub async fn solve_slider<C: HttpClient>(
        &mut self,
        upstream: &Upstream<'_, C>,
        tid: &str,
    ) -> Result<String, Error> {
        let puzzle = upstream
            .fetch_slider_puzzle(&self.profile, tid, &self.phone)
            .await?;
        puzzle.answer()
    }

    /// 只取滑块挑战（需要把图给用户看时用；正常路径走 [`SmsLogin::solve_slider`]）。
    ///
    /// 留着它是为了"自动解不出来"时仍有退路：拿到图 → 人工量缺口 →
    /// [`SmsLogin::send_code_with_slider`]。那条路不需要新代码，
    /// 但接口得先在这里，不然退路等于不存在。
    pub async fn slider_puzzle<C: HttpClient>(
        &self,
        upstream: &Upstream<'_, C>,
        tid: &str,
    ) -> Result<mcloud_upstream::login::SliderPuzzle, Error> {
        upstream
            .fetch_slider_puzzle(&self.profile, tid, &self.phone)
            .await
    }

    fn finish_send(&mut self, envelope: Envelope) -> Result<SendSmsOutcome, Error> {
        let outcome = login::classify_send_sms(&envelope);
        if let SendSmsOutcome::Sent { remain_get_times } = &outcome {
            self.remain_get_times = *remain_get_times;
        }
        Ok(outcome)
    }

    /// 用验证码登录，返回**已解密**的 `userInfo`。
    ///
    /// `sms_code` 是 6 位明文（`thirdlogin` 的 `dycpwd` 在短信方式下就是明文，不是哈希）。
    pub async fn login<C: HttpClient, D: LoginPayloadDecryptor>(
        &self,
        upstream: &Upstream<'_, C>,
        tid: &str,
        sms_code: &str,
        decryptor: &D,
    ) -> Result<Value, Error> {
        let body = login::sms_thirdlogin_body(self.profile.platform(), &self.phone, sms_code);
        let envelope = upstream.thirdlogin(&self.profile, tid, &body).await?;

        match login::classify_poll(&envelope) {
            PollOutcome::Success { encrypted } => decryptor.decrypt_user_info(&encrypted),
            // 短信登录不会经过二维码那几个中间态；走到这里说明是这个码不对之类的业务失败。
            // 原样带回上游结论，不臆造"验证码错误"这种结论。
            other => Err(Error::BadRequest(match other {
                PollOutcome::Other { code, message } => match code {
                    Some(code) => format!("登录失败 code={code}: {message}"),
                    None => format!("登录失败: {message}"),
                },
                other => format!("登录未成功: {other:?}"),
            })),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 登录结果落库
// ═══════════════════════════════════════════════════════════════════════════

/// 用登录得到的 `userInfo` 拼出与客户端**逐字一致**的 `auth`。
///
/// # 格式的证据
///
/// 客户端 `common/utils.js: sendRequest` L230 写得很直接：
///
/// ```js
/// xhr.setRequestHeader("Authorization", "Basic "+btoa("pc:"+window.UserInfo.account+":"+window.UserInfo.authToken));
/// ```
///
/// 也就是 `base64("pc:<account>:<authToken>")` —— 注意**第一段是 `pc` 而不是平台号**。
/// 这个形状正好是 `mcloud_core::parse_auth` 认得的 `platform:phone:token`，
/// 所以登录结果能直接被既有的账号/登录态模型消费，**不需要第二套解析**。
///
/// # 第一段是**按端**变的
///
/// PC 端是 `pc`；**App 端是 `mobile`**（实测：App 日志里 1076 处 `Authorization`
/// 解码后全是 `mobile:<手机号>:<authToken>`，见 `docs/client-profiles.md` §3.3）。
/// 两端的 `authToken` 形状完全一样，只有这一段字面量不同。
/// 所以前缀由调用方给出（[`auth_from_user_info_with_prefix`]），
/// 本函数只是 PC 端的便利包装 —— **不要**把 `pc` 当成"默认值"用到别的端上。
pub fn auth_from_user_info(user_info: &Value) -> Result<String, Error> {
    auth_from_user_info_with_prefix(user_info, PC_AUTH_PREFIX)
}

/// PC 端 `Authorization` 载荷的第一段。
///
/// 客户端源码里是字面量 `pc`（不是平台号，Windows 与 macOS 都用它）。
pub const PC_AUTH_PREFIX: &str = "pc";

/// 同 [`auth_from_user_info`]，但显式指定载荷的第一段。
///
/// 之所以不把前缀做成 `Option` 默认 `pc`：默认值会让"给 App 的登录结果套了 PC 的前缀"
/// 变成一个静默通过的调用，而那正是最难在事后发现的一类错误。
pub fn auth_from_user_info_with_prefix(user_info: &Value, prefix: &str) -> Result<String, Error> {
    let field = |name: &str| {
        user_info
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Error::Parse(format!("登录结果缺少字段 {name}")))
    };

    let account = field("account")?;
    let auth_token = field("authToken")?;
    Ok(format!(
        "Basic {}",
        b64(&format!("{prefix}:{account}:{auth_token}"))
    ))
}

fn b64(raw: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
}

/// 登录结果 → 领域对象（**纯函数**，便于逐字段断言）。
///
/// 返回 `(账号, 登录态)`：前者是"能直接给前端看的信息"，后者是"能直接冒充登录的凭据"，
/// 两者在存储里本来就分表（见 `mcloud-io` 的 schema 说明）。
///
/// # 凭据落位的口径
///
/// `authToken` 存进 [`AccountState::jwt_token`]：移动端把它叫 `jwtToken`、
/// PC 端叫 `authToken`，但**在各自客户端里都是放进 Authorization 头的那一个值**，
/// 职责完全相同，所以不新开一列。
///
/// # 手机号落两处，且**不是重复**
///
/// | 落哪 | 值 | 谁看 |
/// |---|---|---|
/// | `accounts.phone_masked` | 掩码 `19****16` | 直接吐给前端（列表页） |
/// | `account_state.phone` | **完整号码**（v6 起） | 拼 `Authorization` 用 |
///
/// 完整号码是 `Authorization` 的中间那段，而它**推不出来**（掩码、`jwt_token`、
/// `user_domain_id` 都不含它）。不存的话，每个要用这个头的能力都得让调用方
/// 在命令行上重输一遍 —— 那正是 `mcloud signin --phone` 存在的原因（缺口，非设计）。
pub fn account_and_state_from_login(
    scope: Scope<'_>,
    device_id: &str,
    user_info: &Value,
    now: DateTime<Utc>,
) -> Result<(Account, AccountState), Error> {
    account_and_state_from_login_with_prefix(scope, device_id, user_info, PC_AUTH_PREFIX, now)
}

/// 同 [`account_and_state_from_login`]，但显式指定 `Authorization` 载荷的第一段。
///
/// App 端传 `"mobile"`（见 [`auth_from_user_info_with_prefix`] 的说明）。
///
/// # 为什么这里能容忍两种用户域字段名
///
/// 用户域标识的**响应字段名是 `userDomainId`**（形如 `1990000000000000001`）——
/// 这条已由一次真机登录打印出的字段清单证实：
/// `account, …, userAssetFlag, userDomainId, userExtInfo, userid`，
/// 里面**没有 `userId`**。`userId` 是**请求体**里常见的键名，由 `userDomainId` 赋过去
/// （如 `userId: window.userDomainId || window.UserInfo.userDomainId || ''`）。
///
/// 事前就已经从 PC 客户端源码反推出来了：`UserInfo.userDomainId` 被读了 11 次，
/// 而 `UserInfo.userId` **一次都没有**；事后实测与这个统计完全吻合。
/// 而且 App 与 PC **都调 `POST /user/thirdlogin`**，所以两端共用这套字段名。
///
/// 既然 `userId` 一次都没在响应里出现过，这里为什么还留着它？
/// 因为**别的端的响应/别的流程可能用它**，认两个名字的成本是零，
/// 而认错名字的代价是"用户域标识静默为空"。所以保留兜底，
/// 而不是让调用方先去伪造一个 `userId`。
///
/// 注意**别把 `userid`（全小写）并进这条兜底链**：它在同一份响应里与
/// `userDomainId` **同时出现、值不同**（12 字符 vs 19 位），不是一回事。
pub fn account_and_state_from_login_with_prefix(
    scope: Scope<'_>,
    device_id: &str,
    user_info: &Value,
    prefix: &str,
    now: DateTime<Utc>,
) -> Result<(Account, AccountState), Error> {
    let auth = auth_from_user_info_with_prefix(user_info, prefix)?;

    // 复用既有解析：它已经有黄金向量对拍，没必要在这里重写一遍分段逻辑。
    let parsed = mcloud_core::parse_auth(&auth)
        .ok_or_else(|| Error::Parse("拼出来的 auth 解析失败（格式与 parse_auth 不一致）".into()))?;

    let phone = parsed.phone.clone().unwrap_or_default();
    let account_id = AccountId::new(mcloud_core::account_label(None, &phone));

    let mut account = Account::new(account_id.as_str());
    // `Scope::Local` 时是 `None`（无主）：本地运维不替谁认领账号，
    // 之后由首个管理员初始化时的认领流程接手（见 StateStore::claim_unowned_accounts）。
    account.owner = scope.owner().cloned();
    // 手机号非空时 AccountId 本身就是掩码，直接复用，避免两处掩码逻辑跑偏
    account.phone_masked = (!phone.is_empty()).then(|| account_id.as_str().to_string());
    account.platform = parsed.platform.clone();
    // 旧语义里 `0` 是"没有过期信息"，不是"1970 年过期"
    account.auth_expire = (parsed.expire_millis() != 0).then(|| parsed.expire_millis());

    let state = AccountState {
        jwt_token: parsed.token.clone(),
        device_id: (!device_id.is_empty()).then(|| device_id.to_string()),
        // `userDomainId` 是**两端的响应字段名**（实测字段清单里就是它）；
        // `userId` 只是**请求体**里的键名，值是同一个。详见本函数的文档。
        // `userid`（全小写）**不在此列**，它是 12 字符的另一个值。
        user_domain_id: user_info
            .get("userId")
            .or_else(|| user_info.get("userDomainId"))
            .and_then(Value::as_str)
            .map(str::to_string),
        // **完整手机号**（v6 起落库）。这里它是**给人看的标识**（日志里认出
        // "签的是谁"），**不是**拼凭据的原料 —— 凭据见下面的 `authorization`。
        //
        // 用 `phone_or_none` 而不是上面那个 `phone`（它把空串折成了 `""`）：
        // 库里要区分"有号码"与"上游没给"，`Some("")` 会让下游以为有值。
        phone: parsed.phone_or_none().map(str::to_string),
        // **整串 auth 原样落库**（v7 起）。
        //
        // 注意上面那两行 `auth_from_user_info_with_prefix` → `parse_auth` 的写法：
        // 拼出整串、紧接着又把它拆回三段 —— 那只是**为了拿 `phone` / `platform` /
        // `expire` 三个派生值**（`account` 表要它们）。凭据本身不再走"拆开再拼"，
        // 所以哪怕将来这个拼装规则变了、或者某天 `phone` 解不出来，
        // 发出去的 `Authorization` 仍然是登录那一刻原样的那一串。
        //
        // 旧项目 `mcloud-sign` 的 `asign.json` 里 `auth` 存的就是这个值，
        // 两边从此同构，迁移时可以直接搬。
        authorization: Some(auth.clone()),
        updated_at: Some(now),
        ..AccountState::new(account_id.as_str())
    };

    Ok((account, state))
}

/// 把登录结果落库，返回账号 id。
///
/// 顺序是**先账号、后登录态**：`account_state` 的写入会校验账号归属
/// （见 `StateStore` 的隔离契约），账号不在就先写凭据会被正确地拒掉。
pub async fn persist_login<S: StateStore>(
    store: &S,
    scope: Scope<'_>,
    device_id: &str,
    user_info: &Value,
    now: DateTime<Utc>,
) -> Result<AccountId, Error> {
    persist_login_with_prefix(store, scope, device_id, user_info, PC_AUTH_PREFIX, now).await
}

/// 同 [`persist_login`]，但显式指定 `Authorization` 载荷的第一段（App 端传 `"mobile"`）。
pub async fn persist_login_with_prefix<S: StateStore>(
    store: &S,
    scope: Scope<'_>,
    device_id: &str,
    user_info: &Value,
    prefix: &str,
    now: DateTime<Utc>,
) -> Result<AccountId, Error> {
    let (account, state) =
        account_and_state_from_login_with_prefix(scope, device_id, user_info, prefix, now)?;
    store.upsert_account(scope, &account).await?;
    store.upsert_account_state(scope, &state).await?;
    Ok(account.id)
}

#[cfg(test)]
mod tests;
