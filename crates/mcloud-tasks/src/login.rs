//! 账号登录（扫码 / 短信 / 账密）—— **不是** `Task`，是交互式流程。
//!
//! # 为什么不做成 `Task`
//!
//! `Task` 的契约是"给我一个账号，我跑完报个结果"。登录**没有账号可给**
//! （它就是要造出账号），而且要等人扫码、等人输验证码 —— 把人的交互
//! 塞进为无人值守设计的 `Task` 里，两边都会别扭。
//!
//! 所以这里是**一步一步**的显式状态机：调用方（CLI / WebUI）负责
//! 展示二维码、等时间、收验证码，本模块负责"这一步该发什么、回了什么、
//! 接下来能不能继续"。
//!
//! # 与 `mcloud-upstream::login` 的分工
//!
//! 那一层是**纯函数**：请求形状、响应语义、密码学。这一层才碰 `Clock` 与
//! `HttpClient`。分开的收益是登录协议可以离线逐字段对拍，而这里只需要
//! 测"节奏与终态"。
//!
//! # 平台形态从头贯穿到尾
//!
//! 登录请求的平台（`clientType` / `cpid` / UA / 渠道）与**落盘的那一列**
//! 是同一个值：`pc_platform`。这样"用 mac 登录、之后用 win 续期"这种
//! 自相矛盾的状态在类型上就构造不出来。

use mcloud_core::{
    Account, AccountId, AccountSettings, AccountStore, DeviceId, Error, HttpClient, PcPlatform,
    Result,
};
use mcloud_crypto::Random;
use mcloud_upstream::app_login;
use mcloud_upstream::auth::{format_auth, parse_auth, APP_AUTH_PREFIX, PC_AUTH_PREFIX};
use mcloud_upstream::login::{
    self, LoginDataDecryptor, LoginEnvelope, LoginPayloadDecryptor, PcClient, PollOutcome,
};

/// 重新导出：它是登录 API 的一部分（调用方要 match 它的分支）。
pub use mcloud_upstream::login::SendSmsOutcome;
use mcloud_upstream::session::ensure_http_ok;
use serde_json::{json, Value};

/// 扫码轮询间隔。实测客户端是 `setInterval` 1000ms。
pub const QR_POLL_INTERVAL_MS: u64 = 1000;

/// 二维码在我们这一侧的有效期。
///
/// # 这是一个**产品取值**，不是实测值
///
/// 归档里 `200059542 二维码已失效` 只出现过 1 次，推不出时长。取 120 秒：
/// 宁可早一点失效让用户重新出码（代价是一次点击），也不要让用户对着一个
/// 已经无效的码干等（代价是"这软件坏了"）。
pub const QR_TTL_SECONDS: i64 = 120;

/// 扫码登录的推进结果。
#[derive(Debug, Clone, PartialEq)]
pub enum QrProgress {
    /// 码还没被扫 —— 轮询期的绝大多数情况，**不是错误**。
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

/// 扫码失败的终态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrFailure {
    /// 我们自己的 TTL 到了（码可能还有效，但不再等）。
    TtlElapsed { ttl_seconds: i64 },
    /// 上游说码已失效。
    ExpiredUpstream,
    /// `sID` 不匹配：换了码却还在用旧值轮询，属于调用方的状态错误。
    SessionMismatch,
    /// 触发限流 —— **该退避**，不是换张码立刻重试。
    TooFrequent,
    /// 上游回了别的码。**原样带回**：上游会在不通知的情况下加新码，
    /// 硬编码成"未知错误"会让新码永远查不出来。
    Rejected { code: Option<i64>, message: String },
}

/// 扫码登录会话。
pub struct QrLogin {
    /// 端档案：平台 + 设备指纹（请求头与二维码都要用）。
    client: PcClient,
    /// `sID`，同时就是 `thirdlogin` 的 `dycpwd`。
    session_id: String,
    started_at_ms: i64,
    ttl_seconds: i64,
    /// 终态缓存。`None` = 还在进行中。
    finished: Option<QrProgress>,
}

impl QrLogin {
    /// 出码。
    ///
    /// **零网络请求**：二维码是本地拼的（客户端源码 `createLoginQRcode()`
    /// 就是字符串拼接）。
    pub fn start(
        platform: PcPlatform,
        device_id: impl Into<String>,
        rng: &mut impl Random,
        now_ms: i64,
    ) -> Self {
        Self {
            client: PcClient::new(platform, device_id),
            session_id: login::new_session_id(rng),
            started_at_ms: now_ms,
            ttl_seconds: QR_TTL_SECONDS,
            finished: None,
        }
    }

    /// 二维码载荷（交给前端渲染成二维码）。
    pub fn qr_payload(&self) -> String {
        login::qr_payload(&self.client, &self.session_id)
    }

    /// `sID`。换码时要一起换掉，否则会得到 [`QrFailure::SessionMismatch`]。
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 端档案（落盘时要用它的平台与设备号）。
    pub fn client(&self) -> &PcClient {
        &self.client
    }

    /// 覆盖默认有效期（"把码发给别人扫"时 120 秒不够）。
    pub fn with_ttl(mut self, seconds: i64) -> Self {
        self.ttl_seconds = seconds;
        self
    }

    /// 下次该隔多久再问；`None` = 别再问了。
    ///
    /// 终态与超时都返回 `None` —— 调用方只认这一个判断。
    pub fn next_delay_ms(&self, now_ms: i64) -> Option<u64> {
        if self.finished.is_some() || self.is_expired(now_ms) {
            return None;
        }
        Some(QR_POLL_INTERVAL_MS)
    }

    /// 是否已超过我们这一侧的 TTL。
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.started_at_ms + self.ttl_seconds * 1000
    }

    /// 轮询一次并推进状态。
    ///
    /// 已到终态时**不发请求**、直接返回上次的结论（对应客户端的 `clearInterval`）。
    pub async fn poll<H: HttpClient + ?Sized, D: LoginPayloadDecryptor + ?Sized>(
        &mut self,
        http: &H,
        tid: &str,
        decryptor: &D,
        now_ms: i64,
    ) -> Result<QrProgress> {
        if let Some(done) = &self.finished {
            return Ok(done.clone());
        }
        if self.is_expired(now_ms) {
            let done = QrProgress::Failed {
                reason: QrFailure::TtlElapsed {
                    ttl_seconds: self.ttl_seconds,
                },
            };
            self.finished = Some(done.clone());
            return Ok(done);
        }

        let body = login::qr_thirdlogin_body(&self.client, &self.session_id);
        let req = login::thirdlogin_request(&self.client, tid, &body);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "登录")?)?;

        let progress = match login::classify_poll(&env) {
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
/// 链路（含降级）：
///
/// ```text
/// 发码 ──成功──▶ 等用户输验证码 ──▶ thirdlogin(pintype=5)
///  │
///  └─200059554──▶ 取滑块图 ──▶ 带滑块再发码
///                   │
///                   └─200059553──▶ **必须重新取图**（旧挑战作废），不能复用
/// ```
///
/// 本类型只提供"一步一步"的入口，把"要不要重试、取哪张图"留给调用方 ——
/// 滑块图是 IO（要渲染给人看），本层不碰。
pub struct SmsLogin {
    client: PcClient,
    phone: String,
    random: String,
}

impl SmsLogin {
    /// 开始一次短信登录（生成发码用的 `random`）。
    pub fn start(
        platform: PcPlatform,
        device_id: impl Into<String>,
        phone: impl Into<String>,
        rng: &mut impl Random,
    ) -> Self {
        Self {
            client: PcClient::new(platform, device_id),
            phone: phone.into(),
            random: login::new_sms_random(rng),
        }
    }

    pub fn phone(&self) -> &str {
        &self.phone
    }

    pub fn client(&self) -> &PcClient {
        &self.client
    }

    /// 发给用户的短信里的 `random`（排障用；换了就要重新发码）。
    pub fn random(&self) -> &str {
        &self.random
    }

    /// 重掷 `random`。
    ///
    /// 发码被限流之后要换一个 —— 服务端按它去重，复用同一个会被直接拒。
    pub fn reroll(&mut self, rng: &mut impl Random) {
        self.random = login::new_sms_random(rng);
    }

    /// 发送验证码。
    pub async fn send_code<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        tid: &str,
    ) -> Result<SendSmsOutcome> {
        let body = login::send_sms_code_body(&self.client, &self.phone, &self.random);
        self.send_body(http, tid, body).await
    }

    /// 带滑块答案发送验证码。
    pub async fn send_code_with_slider<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        tid: &str,
        slider_answer: &str,
    ) -> Result<SendSmsOutcome> {
        let body = login::send_sms_code_body_with_slider(
            &self.client,
            &self.phone,
            &self.random,
            slider_answer,
        )?;
        self.send_body(http, tid, body).await
    }

    async fn send_body<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        tid: &str,
        body: Value,
    ) -> Result<SendSmsOutcome> {
        let req = login::send_sms_code_request(&self.client, tid, &body);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "登录")?)?;
        Ok(login::classify_send_sms(&env))
    }

    /// 取滑块挑战（需要把图给用户看时用）。
    pub async fn slider_puzzle<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        tid: &str,
    ) -> Result<login::SliderPuzzle> {
        let body = login::slider_puzzle_body(&self.phone);
        let req = login::slider_puzzle_request(&self.client, tid, &body);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "登录")?)?;
        login::parse_slider_puzzle(&env)
    }

    /// 用 6 位验证码完成登录。
    pub async fn login<H: HttpClient + ?Sized, D: LoginPayloadDecryptor + ?Sized>(
        &self,
        http: &H,
        tid: &str,
        sms_code: &str,
        decryptor: &D,
    ) -> Result<Value> {
        let body = login::sms_thirdlogin_body(&self.client, &self.phone, sms_code);
        let req = login::thirdlogin_request(&self.client, tid, &body);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "登录")?)?;
        match login::classify_poll(&env) {
            PollOutcome::Success { encrypted } => decryptor.decrypt_user_info(&encrypted),
            other => Err(Error::Upstream {
                code: env.raw_code.clone(),
                message: format!("短信登录未成功：{other:?} {}", env.message),
            }),
        }
    }
}

/// 账号密码登录。
///
/// 口令经 `SHA1("fetion.com.cn:" + 口令)` 大写 hex 后放进 `dycpwd` ——
/// 见 `mcloud_upstream::login::password_hash`。
///
/// 安全登录分支（服务端下发 `smsCheckRandom` 时改用 `secinfo`）**未实现**：
/// 实测没走到那条分支，且它依赖 `safeVerifyCodeEncrypt`。不先写猜测性代码。
pub struct PasswordLogin {
    client: PcClient,
}

impl PasswordLogin {
    pub fn new(platform: PcPlatform, device_id: impl Into<String>) -> Self {
        Self {
            client: PcClient::new(platform, device_id),
        }
    }

    pub fn client(&self) -> &PcClient {
        &self.client
    }

    pub async fn login<H: HttpClient + ?Sized, D: LoginPayloadDecryptor + ?Sized>(
        &self,
        http: &H,
        tid: &str,
        username: &str,
        password: &str,
        decryptor: &D,
    ) -> Result<Value> {
        let body = login::password_thirdlogin_body(&self.client, username, password);
        let req = login::thirdlogin_request(&self.client, tid, &body);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "登录")?)?;
        match login::classify_poll(&env) {
            PollOutcome::Success { encrypted } => decryptor.decrypt_user_info(&encrypted),
            other => Err(Error::Upstream {
                code: env.raw_code.clone(),
                message: format!("账密登录未成功：{other:?} {}", env.message),
            }),
        }
    }
}

/// 退出登录。
///
/// # 失败要**吞掉**
///
/// 实测客户端是"先清本地 token，再发 logout"，服务端因此常回
/// `01000001 用户token不能为空`。那不是失败，是正常顺序的结果。
/// 所以这里**不发请求**时也不算错：调用方清完本地凭据就算退出成功。
pub async fn logout<H: HttpClient + ?Sized>(
    http: &H,
    client: &PcClient,
    tid: &str,
    phone: &str,
) -> Result<()> {
    let body = login::logout_body(phone);
    let req = login::logout_request(client, tid, &body);
    // 尽力而为：网络错误也不上报（本地凭据已经清了）
    let _ = http.request(req).await;
    Ok(())
}

// ══════════════════════════ 登录结果 → 落库 ══════════════════════════

/// 一次登录成功后要落盘的两样东西。
#[derive(Debug, Clone)]
pub struct LoginResult {
    pub account: Account,
}

/// 把登录响应里的 `userInfo` 转成要落盘的 [`Account`]。
///
/// # 凭据是**原样拼出来的整串**
///
/// `Authorization` 的形状是 `Basic base64("pc:<手机号>:<authToken>")`，
/// 本函数用 [`format_auth`] 拼它，而不是让调用方自己拼 —— 前缀只有一个来源。
///
/// # 手机号落两处，且**不是重复**
///
/// | 落哪 | 值 | 谁看 |
/// |---|---|---|
/// | `Account::id` / `nickname` | 掩码 `19****01` | 直接吐给前端（列表页） |
/// | `AuthInfo::phone` | **完整号码** | 拼 `Authorization` 的中间那段 |
///
/// # `pc_platform` 必须**同时**落
///
/// 平台决定后续每一次请求的 UA 与头（见 [`PcClient`]）。凭据本身区分不出来
/// （两种平台的 auth 前缀都是 `pc`），所以不存它，"刷新凭据"就只能靠人再猜一次。
pub fn account_from_login(
    user_info: &Value,
    platform: PcPlatform,
    device_id: impl Into<String>,
) -> Result<Account> {
    let auth = auth_from_user_info(user_info, PC_AUTH_PREFIX)?;
    account_from_auth(
        &auth,
        Some(platform),
        device_id,
        user_domain_id_from(user_info),
    )
}

/// 同上，但显式指定 auth 前缀（App 端传 `mobile`）。
///
/// 前缀**不给默认值**：默认值会让"给 App 的结果套 PC 前缀"静默通过。
pub fn account_from_user_info_with_prefix(
    user_info: &Value,
    prefix: &str,
    platform: Option<PcPlatform>,
    device_id: impl Into<String>,
) -> Result<Account> {
    let auth = auth_from_user_info(user_info, prefix)?;
    account_from_auth(&auth, platform, device_id, user_domain_id_from(user_info))
}

/// 从登录响应里取 `userDomainId`。
///
/// # 为什么在这里取而不是等到刷新时
///
/// 刷新凭据（`POST /user/auth/refreshToken`）的请求体**必须有**它，
/// 而它**无法从凭据串里反推**。登录响应里正好有（实拍：19 位数字串，
/// 与请求头 `x-yun-uni` 同值）—— 现在不取，以后就再也拿不到，
/// 只能让用户重新登录一次。
///
/// 缺失**不算登录失败**：字段存在之前的库、或上游某天不发它，
/// 都还能正常跑任务，只是刷新会明确报"缺用户域"。
fn user_domain_id_from(user_info: &Value) -> Option<String> {
    user_info
        .get("userDomainId")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// 从已拼好的 auth 串构造账号。
fn account_from_auth(
    auth: &str,
    platform: Option<PcPlatform>,
    device_id: impl Into<String>,
    user_domain_id: Option<String>,
) -> Result<Account> {
    // 复用既有解析：它已被 22 条黄金向量钉住，没必要在这里重写分段逻辑。
    let parsed = parse_auth(auth)
        .ok_or_else(|| Error::Parse("拼出来的 auth 解析失败（格式与 parse_auth 不一致）".into()))?;

    let account_id = AccountId::new(mcloud_upstream::account_id_for(&parsed.phone));

    Ok(Account {
        id: account_id,
        // 手机号即为账号主键（掩码形态），昵称留空由用户后续设置
        nickname: None,
        auth: parsed,
        device_id: DeviceId(device_id.into()),
        pc_platform: platform,
        user_domain_id,
        // 归属由 server 层在拿到当前会话后补写（见 routes/login.rs 的 claim_account），
        // 这里不知道是谁在登录。
        owner: None,
        settings: AccountSettings::default(),
    })
}

/// 从 `userInfo` 里取 `account` 与 `authToken`，拼出整串 auth。
///
/// ⚠ 两个字段都必须是**非空字符串**：上游的 `account` / `authToken` 从来都是
/// 字符串，容忍数字类型只会让"上游改了类型"这件事静默通过。
pub fn auth_from_user_info(user_info: &Value, prefix: &str) -> Result<String> {
    let field = |name: &str| {
        user_info
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Error::Parse(format!("登录结果缺少字段 {name}")))
    };
    let account = field("account")?;
    let auth_token = field("authToken")?;
    format_auth(prefix, account, auth_token)
}

/// 把登录结果**落盘**：账号进 `AccountStore`，设备号进 `account_state`。
///
/// # 为什么要单独一个函数
///
/// 落盘这一步横跨两个 store 方法（账号 + 状态），而"先写哪个、失败了怎么办"
/// 是有讲究的：先写账号（它含凭据，是主要产物），再补状态。
/// 状态晚一步写不会让账号不可用，反过来会留下一个"有设备号但没凭据"的孤儿。
pub async fn persist_login<S: AccountStore + ?Sized>(
    store: &S,
    result: &LoginResult,
) -> Result<()> {
    store.save_account(&result.account).await?;

    // 设备号同时进账号（`Account::device_id`）与运行期状态。
    //
    // 后者是给 `SmidClient` 那类"按账号查设备"的路径用的；两处值必须一致，
    // 否则会出现"创建会话用 A 设备、轮询用 B"这种定位不到会话的怪现象。
    let state = json!({
        "device_id": result.account.device_id.as_str(),
        "pc_platform": result.account.pc_platform.map(|p| p.as_str()),
    });
    store.save_account_state(&result.account.id, &state).await?;
    Ok(())
}

/// 生产用的登录数据解密器。
pub fn production_decryptor() -> LoginDataDecryptor {
    LoginDataDecryptor::production()
}

// ══════════════════════════ App 端（手机）登录 ══════════════════════════

/// App 端短信登录的状态机。
///
/// # 与 PC 端 [`SmsLogin`] 的关系：**同名的动作，不同的协议**
///
/// 两者的方法名很像（发码、取滑块、登录），但每一枪的
/// 端点头、请求体形态、加解密都不同（见 [`mcloud_upstream::app_login`]）。
/// 所以它们是**两个类型**，不共用一个泛型开关 ——
/// 那样"传错端"会变成一个能静默通过的选项。
pub struct AppSmsLogin {
    device_id: String,
    phone: String,
    /// `thirdlogin` 用的一次性 `random`（与发码用的 `qwert` 不是一回事）。
    random: String,
}

impl AppSmsLogin {
    /// 开始一次 App 短信登录。
    pub fn start(
        device_id: impl Into<String>,
        phone: impl Into<String>,
        rng: &mut impl Random,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            phone: phone.into(),
            random: app_login::new_login_random(rng),
        }
    }

    pub fn phone(&self) -> &str {
        &self.phone
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// 发送验证码（首次不带滑块答案）。
    pub async fn send_code<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        timestamp: i64,
    ) -> Result<app_login::AppSendSmsOutcome> {
        let body = app_login::send_sms_code_body(&self.phone, "");
        self.send_body(http, timestamp, body).await
    }

    /// 带滑块答案重发验证码。
    ///
    /// 这是 `200059554` 之后的**正常**下一步。答案取 [`Self::slider_puzzle`]
    /// 解出来的坐标。
    pub async fn send_code_with_slider<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        timestamp: i64,
        slider_answer: &str,
    ) -> Result<app_login::AppSendSmsOutcome> {
        let body = app_login::send_sms_code_body_with_slider(&self.phone, slider_answer)?;
        self.send_body(http, timestamp, body).await
    }

    async fn send_body<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        timestamp: i64,
        body: serde_json::Value,
    ) -> Result<app_login::AppSendSmsOutcome> {
        let req = app_login::send_sms_code_request(&self.device_id, timestamp, &body);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "App 登录")?)?;
        Ok(app_login::classify_send_sms(&env))
    }

    /// 取滑块挑战（第一方接口，与 PC 端是同一个）。
    pub async fn slider_puzzle<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        timestamp: i64,
    ) -> Result<login::SliderPuzzle> {
        let req = app_login::slider_puzzle_request(&self.device_id, timestamp, &self.phone);
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "App 登录")?)?;
        login::parse_slider_puzzle(&env)
    }

    /// 用 6 位验证码完成登录，返回解密后的会话载荷。
    ///
    /// # 错误信息要把 `9103` / `9441` 的区别说出来
    ///
    /// 这两个码的**含义完全不同**：`9441` 是"体解开了、码不对"（用户输错，
    /// 重输即可），`9103` 是"体没解开"（**我们这边的 bug**：密钥、错位方向
    /// 或大小写写错了）。混成一句"登录失败"会让一个实现缺陷看起来像用户错误。
    pub async fn login<H: HttpClient + ?Sized>(
        &self,
        http: &H,
        timestamp: i64,
        sms_code: &str,
    ) -> Result<Value> {
        let body = app_login::sms_thirdlogin_body(&self.phone, sms_code, &self.random);
        let crypto = app_login::AppLoginCrypto::production();
        let req = app_login::thirdlogin_request(&self.device_id, timestamp, &crypto, &body)?;
        let resp = http.request(req).await?;
        let env = LoginEnvelope::parse(ensure_http_ok(&resp, "App 登录")?)?;

        match app_login::classify_login(&env) {
            app_login::AppLoginOutcome::Success { encrypted } => {
                crypto.decrypt_user_info(&encrypted)
            }
            // ⚠ 用 `Error::Config` 而不是 `Error::Upstream`：验证码错是
            // **用户输入问题**，应当映射成 400 让前端就地在表单上显示。
            // 用 Upstream 会变成 500，前端只能弹一句"服务器错误"，
            // 而用户真正需要知道的是"这个码不对，重发一个"。
            app_login::AppLoginOutcome::SmsCodeWrong => {
                Err(Error::Config("验证码不正确或已过期，请重新获取".into()))
            }
            // 这个反过来：**是我们的 bug**，不是用户的错。
            // 保持 Upstream（500），并在文案里说清责任方。
            app_login::AppLoginOutcome::Undecryptable => Err(Error::Upstream {
                code: env.raw_code.clone(),
                message: "上游无法解开登录体（9103）—— 这是本客户端的加密实现问题，不是验证码问题"
                    .into(),
            }),
            app_login::AppLoginOutcome::LoginFailed => Err(Error::Upstream {
                code: env.raw_code.clone(),
                message: format!("App 登录被拒（9999），多半是设备头不完整：{}", env.message),
            }),
            // 上游的业务拒绝（手机号格式、风控……）都是**用户可修**的，
            // 归到 Config（400）而不是 500。
            //
            // ⚠ 这里刻意**不打印枚举的 Debug**：那会把
            // `Other { code: Some(5010047) }` 这种 Rust 语法糊到用户脸上。
            // 上游的 message 本身已经是人话，直接用。
            app_login::AppLoginOutcome::Other { code, .. } => {
                let detail = if env.message.is_empty() {
                    "上游未给出原因".to_string()
                } else {
                    env.message.clone()
                };
                match code {
                    Some(c) => Err(Error::Config(format!("登录失败（上游码 {c}）：{detail}"))),
                    None => Err(Error::Config(format!("登录失败：{detail}"))),
                }
            }
        }
    }
}

/// 从 App 登录结果构造账号（`auth` 前缀是 **`mobile`**）。
///
/// # 为什么 `pc_platform` 是 `None`
///
/// App 登录的账号**不是 PC 形态**：它用 `mobile:` 前缀、走 App 的设备头与 UA。
/// 给它随便填一个 PC 平台，会让后续每次请求都用错的 UA 发出去 ——
/// 而那种错误在服务端表现为业务码，不表现为"平台不对"。
///
/// `None` 也正是 [`mcloud_server::routes::accounts::AccountSummary`] 里
/// "非 PC 端登录（App / 历史数据）"那一档的判据。
pub fn account_from_app_login(user_info: &Value, device_id: impl Into<String>) -> Result<Account> {
    account_from_user_info_with_prefix(user_info, APP_AUTH_PREFIX, None, device_id)
}

/// 生产用的 App 端登录体解密器。
pub fn production_app_decryptor() -> app_login::AppLoginCrypto {
    app_login::AppLoginCrypto::production()
}

#[cfg(test)]
mod tests;
