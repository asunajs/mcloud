//! 签到（云朵中心）的编排：**换 SSO token → 建会话 → 查信息 → 签到**。
//!
//! # 这一层在补什么
//!
//! `mcloud-upstream::app_signin` 只给"一个请求怎么拼、一个响应怎么读"；
//! 它不知道"先做哪一步、什么时候该停、凭什么算成功"。那些判断都在这里。
//!
//! 整条链路是四段（详见 `docs/capture/app-signin-samples.txt`）：
//!
//! ```text
//! ① querySpecTokenV2  原生 A 族头 + Authorization  → SSO token（很短命）
//! ② api/timestamp     取服务端毫秒 → 算 x-signature
//! ③ tyrzLogin         带签名 → jwtToken（905 字符，2 小时）
//! ④ infoV3 / startSignIn   带 jwttoken 头 —— 真正的签到
//! ```
//!
//! # 三条判据（都是踩过的坑，不是设计洁癖）
//!
//! 1. **成败一律回查 `infoV3`**，不看 `startSignIn` 的 `todaySignIn`。
//!    那个字段是**调用前**的快照：实测 `todaySignIn:false` 的那次恰恰把今天签上了。
//!    见 [`sign_in`] 末尾的回查与 [`SignInReport::signed_now`]。
//! 2. **`x-timestamp` 必须取自服务端**（第 ② 步），不能用本地时钟 ——
//!    本地偏一点就是 `401`，而 `401` 在日志里和"盐抄错了"长得一模一样。
//! 3. **`Authorization` 拼不出来就不要猜**。见 [`SignInCredential`]。
//!
//! # 不做什么
//!
//! - **不把 H5 的 `jwtToken` 落库**。它 2 小时就废，而
//!   `account_state.jwt_token` 装的是**登录凭据**（App 的 `authToken`）——
//!   两者同名不同物，写进去等于把可长期复用的凭据覆盖成一张两小时的票。
//! - **不写云朵余额**。`infoV3` 的 `beforeTotal` 只读出来给人看；
//!   落库走 [`crate::cloud::record_cloud`]（当前值 + 日快照），由**调用方**显式调。
//!   不在这里顺手写，是因为余额有四个上游来源（见那个模块的说明），
//!   签到只是最早跑通的那一个 —— 写在这里，第二个来源出现时就只能再写一次。

use mcloud_core::{AccountId, Clock, Error, Random, Scope, StateStore, account_label};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use mcloud_upstream::app_signin::{
    BackupGift, BackupGiftReceive, MultipleResult, ReceiveItem, ReceiveResult,
    ReceiveTaskExpansion, RevivalReward, SignInResult, SignInStatus, TO_SOURCE_ID, sign_h5_request,
    signin_page_url,
};
// `pub use` 同时也把名字引进本模块 —— 所以上面那行**不要**再列一次它们。
pub use mcloud_upstream::app_signin::{CloudRecords, PrizeRecordItem, PrizeRecords};

use crate::app_login::user_timestamp;

// 账本与筛选的取值**从上游层转发出来**，免得调用方（CLI）为了几个整数
// 再去依赖 `mcloud_upstream`。它们本来就是协议常量，转一道不产生新语义。
// （`CloudRecords` / `PrizeRecords` 不在这里再导一次 —— 上面那行已经引进了。）
pub use mcloud_upstream::app_signin::{
    PRIZE_MARKET_NAME, PRIZE_STATUS_EXPIRED, PRIZE_STATUS_PENDING, PRIZE_STATUS_RECEIVED,
    RECORD_BOOK_BEAN, RECORD_BOOK_CLOUD, RECORD_FILTER_ALL, RECORD_FILTER_EARNED,
    RECORD_FILTER_SPENT,
};

/// 一次签到的凭据。
///
/// # 为什么单独成结构，而不是两个 `&str` 参数
///
/// 与 [`crate::app_login::AppLoginContext`] 同一个理由：这三样凑齐才是"以谁的身份签到"，
/// 分开传容易在调用点被摆错顺序，而它们都是字符串，摆错了编译器不会吭声。
///
/// # 手机号还需要吗（**v7 起不需要了**）
///
/// `Authorization` 的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`。
/// 早先的写法是把它**拆开存**（只留第三段 token），用的时候再拼回去 —— 于是必须凑齐
/// 前缀与手机号，而这两样都推不出来（`accounts.id` 是掩码、`jwt_token` 里没有号码），
/// 只能由调用方每次在命令行上补（`--phone`）。那是缺口不是设计。
///
/// **v7 起登录把整串原文存进 `account_state.authorization`**，用的时候原样发出去 ——
/// 一段都不用凑，`phone` 列从此只是**标识**（打印、排障），不参与拼头。
/// 旧项目 `mcloud-sign` 一直就是这么做的（它的 `asign.json` 里 `auth` 存的就是整串），
/// 没有重新发明的必要。
///
/// 老行（v7 之前登录、库里没有整串的）退回"用 `phone` + `jwt_token` 现拼"，
/// 那条路的前缀由 [`AppProfile::authorization`] 硬编码成 `mobile` —— 见 [`Self::load`]。
///
/// ⚠ 无论走哪条路，**都不要**拿掩码值去凑这个头 —— 服务端认不认掩码是个未知数，
/// 而这类"能过但不该过"的东西一旦过了一次，就会变成一条没人敢动的隐性依赖。
pub struct SignInCredential {
    /// 完整手机号。**不参与拼头**（v7 起），只用于日志里认出"签的是谁"。
    phone: String,
    /// `Authorization` 头。含凭据，[`Debug`] 里隐去。
    authorization: String,
    /// 账号 id（掩码），用于定位库里的行。
    account_id: AccountId,
}

impl SignInCredential {
    /// 从库里已存的登录态取签到凭据（**推荐路径**，也是唯一不需要用户补参数的路）。
    ///
    /// v7 起库里存的是整串 `Authorization`，所以只要一个掩码 id 就够了 ——
    /// 手机号、前缀、token 全在里面。老行（v7 之前登录的）会退回现拼，
    /// 那时才需要 `phone` 列；该列也是 `NULL` 就报错并给出两条出路，
    /// **不会**退化成用掩码去凑 —— 见本结构的文档。
    pub async fn from_account<S: StateStore>(
        store: &S,
        scope: Scope<'_>,
        account_id: &AccountId,
    ) -> Result<Self, Error> {
        Self::load(store, scope, account_id, None).await
    }

    /// 同上，但**强制**用调用方给的手机号现拼 —— 给 `phone` 列还是 `NULL` 的老账号用。
    ///
    /// `phone` 必须是**完整手机号**。账号定位方式与登录落库时一致
    /// （同一个 [`account_label`]），所以"登录时存进去的"和"签到时要读的"必然是同一行。
    ///
    /// ⚠ 这条会**跳过**整串优先（哪怕库里存着整串），拼出来的前缀固定是 `mobile`。
    /// 所以它只是老行的逃生口，新调用方一律走 [`Self::from_account`]。
    ///
    /// ⚠⚠ **它的 `authorization()` 是拼出来的，不是拿到的** —— 调用方可以把
    /// `phone` 落库（那是用户给的事实），但**绝不要**把这个串写进
    /// `account_state.authorization`。那一列的口径是"服务端给的原文"，写进去等于
    /// 给一个本地合成物伪造来源：它会走整串优先路径、**绕过上面那道前缀闸**，
    /// 于是此后每个请求都带着一个我们自己安上去的前缀，且再也没人核对。
    /// 只记 `phone` 就够了 —— 下次照旧现拼，闸照旧生效。
    pub async fn from_phone<S: StateStore>(
        store: &S,
        scope: Scope<'_>,
        phone: &str,
    ) -> Result<Self, Error> {
        let account_id = AccountId::new(account_label(None, phone));
        Self::load(store, scope, &account_id, Some(phone)).await
    }

    /// 两条公开路径的共同实现。`phone_override` 为 `None` 时从库里读。
    async fn load<S: StateStore>(
        store: &S,
        scope: Scope<'_>,
        account_id: &AccountId,
        phone_override: Option<&str>,
    ) -> Result<Self, Error> {
        let state = store
            .get_account_state(scope, account_id)
            .await?
            .ok_or_else(|| {
                Error::BadRequest(format!(
                    "本地库里没有账号 {}。先跑一次 App 端登录：\
                     `mcloud login sms --client app`",
                    account_id.as_str()
                ))
            })?;

        // ── 首选：整串 auth（v7 起）────────────────────────────────────────
        //
        // 库里存的就是 `Basic base64("<前缀>:<手机号>:<authToken>")` 的**原文**，
        // 前缀在里面、手机号在里面、token 在里面 —— **一段都不用凑**。
        // 所以走这条路时既不需要 `phone` 列，也不需要猜前缀。
        //
        // 这也是旧项目 `mcloud-sign` 一直以来的做法（它的 `asign.json` 里
        // `auth` 存的就是这个整串），照搬即可，没有重新发明的必要。
        //
        // # 为什么它排在下面那两道闸**前面**
        //
        // 那两道闸的前提在整串这条路上**都不成立**：
        //   - **前缀闸**拦的是"我们硬编码 `mobile`、配上一个 PC 发的 token"。整串自带
        //     前缀，不存在配错的可能 —— 发出去的就是登录时拿到的那个字符串。旧项目
        //     同样不做这个区分（`initAccount` 里是 `setAuth({type:"basic", token})`，
        //     整串原样用），`pc:` 前缀的账号与 `mobile:` 的走同一条签到路径。
        //   - **token 闸**拦的是"拼不出凭据"。整串里就带着 token，不依赖 `jwt_token` 列。
        // 把它们留在前面，就会出现"库里明明有完整凭据、却被判成没有"的假阴性 ——
        // 而 v7 的迁移回填恰恰会造出这种"只有整串"的行。
        if phone_override.is_none()
            && let Some(raw) = state
                .authorization
                .as_deref()
                .filter(|value| !value.is_empty())
        {
            return Ok(Self {
                // 手机号只用于打印与排障，**不参与拼头**。整串里就带着，
                // 顺手解出来 —— 免得"库里 `phone` 是 NULL"这种纯展示问题
                // 又变成一条要用户去补数据的报错。
                phone: state
                    .phone
                    .filter(|phone| !phone.is_empty())
                    .or_else(|| {
                        mcloud_core::auth::parse_auth(raw)
                            .and_then(|info| info.phone)
                            .filter(|phone| !phone.is_empty())
                    })
                    .unwrap_or_default(),
                authorization: raw.to_string(),
                account_id: account_id.clone(),
            });
        }

        // ── 退路：老行（v7 之前）没有整串，用 `phone` + `jwt_token` 现拼 ──
        //
        // 这条路的两个代价都写在下面，**不要再往里加新调用方**：
        //   ① 必须凑齐手机号，缺了就只能让用户在命令行上补（`--phone`）；
        //   ② 前缀由 [`AppProfile::authorization`] **硬编码**成 `mobile`，
        //      它只对 App 端的 token 成立 —— 下面那道 `platform` 闸拦的就是这个。
        //
        // 新登录一律写整串，所以这条路只会随老行自然消亡。

        // 空串与 `None` 同等对待：`Some("")` 拼出来的头服务端只会回 90001，
        // 那时错误信息指向"未登录"，而真正的原因是"库里存了个空值"。
        let auth_token = state
            .jwt_token
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                Error::BadRequest(format!(
                    "账号 {} 没有可用的登录凭据（authToken 为空），重新登录一次",
                    account_id.as_str()
                ))
            })?;

        // 这道闸**只会新增拒绝，不会改变既有行为**：
        // 只有"能证明是另一个端登录的"才拦；`platform` 缺失（老数据）时照旧放行 ——
        // 那正是 `--phone` 这条老路径一直以来的行为，不在这里悄悄收紧。
        //
        // 拦的理由与 `app_client` 不提供 `Android` 变体同源：**这是我们没有实测过的组合**，
        // 宁可让调用方显式去重登一次，也不要拿一个"PC 发的 token 配 App 的前缀"去试探上游 ——
        // 那种失败长得像"未登录"，会把人带到凭据坏了的错误方向上。
        let platform = store
            .get_account(scope, account_id)
            .await?
            .and_then(|account| account.platform)
            .filter(|p| !p.is_empty());
        if let Some(platform) = platform
            && platform != mcloud_upstream::app_client::ios::AUTH_PREFIX
        {
            return Err(Error::BadRequest(format!(
                "账号 {} 是用 `--client {platform}` 登录的，而签到走的是 App 端凭据\
                 （`Authorization` 前缀 `{}`）。重新登录一次：`mcloud login sms --client app`",
                account_id.as_str(),
                mcloud_upstream::app_client::ios::AUTH_PREFIX
            )));
        }

        let phone = match phone_override {
            Some(phone) => phone.to_string(),
            None => state
                .phone
                .filter(|phone| !phone.is_empty())
                .ok_or_else(|| {
                    Error::BadRequest(format!(
                        "账号 {} 库里既没有整串 `Authorization`，也没有完整手机号，\
                         拼不出签到要用的凭据。两条出路：\
                         重新登录一次 `mcloud login sms --client app`（推荐，会存下整串）；\
                         或者用 `--phone <完整手机号>` 补一次（补完会记进库里，不必再输）。\
                         ⚠ 不要拿掩码 id 去凑 —— 服务端认不认是未知数。",
                        account_id.as_str()
                    ))
                })?,
        };

        Ok(Self {
            authorization: AppProfile::authorization(&phone, &auth_token),
            account_id: account_id.clone(),
            phone,
        })
    }

    /// 账号 id（掩码），可直接打印。
    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    /// 完整手机号。**含隐私**，打印前先掩码（[`account_label`]）。
    ///
    /// v7 起它不参与拼头，只是"签的是谁"的标识 —— 日志与排障用。
    pub fn phone(&self) -> &str {
        &self.phone
    }

    /// `Authorization` 头。**含凭据**，只在发请求时用。
    pub fn authorization(&self) -> &str {
        &self.authorization
    }
}

/// 手写 [`Debug`]：这个结构里有一整条可冒充登录的凭据，
/// 而 `{:?}` 是最容易在排障时顺手打出来的东西。手机号也一并掩码 ——
/// 它虽然不算凭据，但打日志时没有任何理由用完整的。
impl std::fmt::Debug for SignInCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInCredential")
            .field("account_id", &self.account_id)
            .field("phone", &account_label(None, &self.phone))
            .field("authorization", &"<已隐去>")
            .finish()
    }
}

/// 一次签到调用的结果。
///
/// `before` / `after` 都是**服务端给的**状态，不是从 `startSignIn` 的响应推的 ——
/// 见 [`SignInReport::signed_now`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInReport {
    /// 签到**前**的状态（`infoV3`）。
    pub before: SignInStatus,
    /// 签到**后**的状态。今天本来就签过时**不会**再查一次，两者相同。
    pub after: SignInStatus,
    /// `startSignIn` 的响应。`None` = 这次没发（查的时候今天已经签过了）。
    pub result: Option<SignInResult>,
}

impl SignInReport {
    /// 今天（调用之后）是否已签。**这是唯一的成败判据。**
    pub fn signed_today(&self) -> bool {
        self.after.signed_today() == Some(true)
    }

    /// 来的时候今天**就已经**签过了。
    ///
    /// 与 [`Self::signed_now`] 是互斥的两件事，而它们在日志里长得很像：
    /// "已签"既可能是"这次刚签上"，也可能是"本来就已经签过"。
    /// 定时任务连跑一个月，只有分开报才看得出它到底有没有在干活。
    pub fn was_already_signed(&self) -> bool {
        self.before.signed_today() == Some(true)
    }

    /// 这次调用**是否真的把今天从「未签」变成了「已签」**。
    ///
    /// 注意 `before` 那一项：`--force`（协议自检）会在已签的情况下也发一次
    /// `startSignIn`，那次的 `result` 是 `Some`、`after` 也是已签，
    /// 但**不是**"这次签上的"。
    pub fn signed_now(&self) -> bool {
        self.result.is_some() && !self.was_already_signed() && self.signed_today()
    }

    /// 本次签到可得云朵数（`startSignIn` 里那个）。没发请求时为 `None`。
    pub fn sign_in_points(&self) -> Option<i64> {
        self.result.as_ref().map(|r| r.sign_in_points)
    }

    /// 服务端报的本月已签天数。
    pub fn month_days(&self) -> i64 {
        self.after.month_days
    }
}

/// [`sign_in`] 的两个可选开关。
///
/// 做成结构而不是两个裸参数：两个都是 `Option`/`bool`，位置一错就是
/// "把 deviceId 当成了 force"，而编译器不会吭声。
#[derive(Debug, Clone, Copy, Default)]
pub struct SignInOptions<'a> {
    /// H5 那个 `deviceId`（`"B"` + 88 字符 base64 → 64 字节）。见 [`sign_in`] 的说明。
    pub device_id: Option<&'a str>,
    /// 今天已签也发一次 `startSignIn`。
    ///
    /// **唯一的用途是协议自检**：确认我们拼出来的请求被上游接受
    /// （而不是回 `401` / `90001`）。服务端对同一天的重复签到是幂等的 ——
    /// 实拍里真实客户端在已签状态下也调了它，回 `success` + `todaySignIn:true`。
    ///
    /// 日常签到**不要**开它：那会让"今天已经签过"这条捷径失效，多打一次写请求。
    pub force: bool,
}

/// 只查状态，不签到（CLI 的 `--status-only`）。
pub async fn status<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<SignInStatus, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream.app_sign_in_status(&jwt, &referer).await
}

/// 同上，但返回 **`infoV3` 的原始响应体**（不解析）。
///
/// 排障用：见 [`mcloud_upstream::app_signin::Upstream::app_sign_in_status_raw`]。
pub async fn status_raw<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<serde_json::Value, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream.app_sign_in_status_raw(&jwt, &referer).await
}

/// `cloudRecordV2` 的查询条件。
///
/// 做成结构而不是四个裸参数，理由与 [`SignInOptions`] 同源：`ai_dou` 与 `filter`
/// 都是 `i64`，`page_number` 与 `page_size` 也都是 —— 四个裸 `i64` 连排时，
/// 传反了编译期一点提示都没有（实测这种错会静默变成"查错账本、取错页"）。
pub struct RecordsQuery {
    /// 账本：取 [`mcloud_upstream::app_signin::RECORD_BOOK_CLOUD`]（改名前的云朵）
    /// 或 [`mcloud_upstream::app_signin::RECORD_BOOK_BEAN`]（改名后的 AI 豆）。
    pub ai_dou: i64,
    /// 收支方向过滤。全部用 [`RECORD_FILTER_ALL`]。
    pub filter: i64,
    /// 页码，**从 1 开始**。
    pub page_number: i64,
    /// 每页条数。
    pub page_size: i64,
}

/// 查一档**豆的收支流水**（`cloudRecordV2`）。**只读**，审计 / 排障用。
///
/// # 它跟 `--status-only` 的分工
///
/// `--status-only` 回答"**现在有多少**"（`infoV3.total`）；这一条回答"**这些是怎么来的**"。
/// 余额对不上时，只有流水能指出差在哪一笔 —— 光看两个余额是永远对不出来的。
///
/// 查询条件走 [`RecordsQuery`]，见那里的说明。
pub async fn cloud_records<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    query: &RecordsQuery,
    clock: &K,
    rng: &mut R,
) -> Result<CloudRecords, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_cloud_records(
            &jwt,
            &referer,
            query.ai_dou,
            query.filter,
            query.page_number,
            query.page_size,
        )
        .await
}

/// 签到。今天已签过就**不发** `startSignIn`，直接回报当前状态。
///
/// # `device_id`
///
/// 真实客户端会带 H5 那个 `deviceId`（`"B"` + 88 字符 base64），
/// **它就是 [`crate::smid::device_id`] 换回来的那个值** —— 签到页的 `fp.min.js`
/// 与本项目的 `mcloud-upstream::smid` 打的是同一个 `deviceprofile/v4`
/// （同 host / organization / appId / 公钥，逐字比对过），所以**纯 Rust 就能拿到**。
///
/// 但它是 `Option`：实测签到**不带也能签上**（2026-09-19 账号 B），
/// 所以调用点可以给 `None`；给 `Some(..)` 时是"更像真实客户端"的那一版。
/// 别让它变成"忘了传" —— 调用点必须显式写。
///
/// ⚠ 但**领取类接口（`receiveV3` 领云朵 / AI 豆）不带就报错**（回 `615`）——
/// 所以"签到能不带"**不能**外推到同族的别的接口，见 `app_signin` 的模块说明。
///
/// # 判据
///
/// 成败一律以**回查 `infoV3`** 为准，见 [`SignInReport`]。
pub async fn sign_in<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    options: &SignInOptions<'_>,
    clock: &K,
    rng: &mut R,
) -> Result<SignInReport, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;

    let before = upstream.app_sign_in_status(&jwt, &referer).await?;
    if before.signed_today() == Some(true) && !options.force {
        // 已经签过了：**不发** `startSignIn`。少一次写请求，也少一次风控记数。
        return Ok(SignInReport {
            before: before.clone(),
            after: before,
            result: None,
        });
    }

    let result = upstream
        .app_start_sign_in(&jwt, &referer, options.device_id)
        .await?;
    let after = upstream.app_sign_in_status(&jwt, &referer).await?;

    // ⚠ 这里**必须**回查，不能拿 `result.today_sign_in` 当结论。
    // 那个字段是调用**前**的快照 —— 实测 `todaySignIn:false` 的那次恰恰把今天签上了
    // （见 `docs/capture/app-signin-samples.txt` §3.2）。反过来，
    // 只看 `code == 0` 也不行：上游对"今天已经签过"同样回 `success`。
    // 所以唯一可信的判据是重新查一次 `infoV3`。
    if after.signed_today() != Some(true) {
        return Err(Error::Internal(format!(
            "startSignIn 回了成功，但回查 infoV3 显示今天仍未签（signed_today={:?}）。\
             以 infoV3 为准；`todaySignIn={}` 是调用前的快照，不是成败判据",
            after.signed_today(),
            result.today_sign_in
        )));
    }

    Ok(SignInReport {
        before,
        after,
        result: Some(result),
    })
}

/// [`receive_cloud`] 的输入：领哪一笔、以及拿什么设备标识去领。
///
/// 做成结构而不是三个裸参数，理由与 [`SignInOptions`] 同源：
/// 它们全是字符串 / `Option`，位置一错就是"把 cloudType 当成了 deviceId"，
/// 而编译器不会吭声。
#[derive(Debug, Clone, Copy, Default)]
pub struct ReceiveTarget<'a> {
    /// `receiveV3` 的 `cloudId` —— 来自 `infoV3` 的 `receiveList[].recordId`。
    ///
    /// 实拍里那个数组**没有 `recordId`**，所以 H5 的 `e.recordId ?? ""` 给的就是空串。
    pub cloud_id: &'a str,
    /// `receiveV3` 的 `cloudType` —— 同样来自 `receiveList[]`。
    ///
    /// `None` 表示"上游没给"，body 里会写成空串（照 H5 的 `?? ""`）。
    pub cloud_type: Option<i64>,
    /// H5 的 `deviceId` —— **就是 [`crate::smid::device_id`] 换回来的那个值**
    /// （`'B'` + 64 字节 base64）。
    ///
    /// 取值走 [`crate::smid::device_id`]（纯 Rust，不用浏览器）。
    ///
    /// ⚠ 上游**校验它的内容**（不带回 `615`、内容错回 `614`），所以不能给编的值 ——
    /// 但也不需要"用官方 App 打开一次签到页"：那个值我们自己就能换。
    pub device_id: &'a str,
}

/// 领云朵（`receiveV3`）。**这是写请求** —— 上游认为有可领的就会真的领走。
///
/// # 为什么要单独一个函数，而不是塞进 [`sign_in`]
///
/// 两者**不是一回事**：签到只是打卡，云朵要另外领（所以 `infoV3.beforeTotal`
/// 在签到前后不变）。日常流程里它们通常连着跑，但也有人只想签到、不想领。
/// 分开之后"领不领"由调用方决定，而不是被签到顺手决定掉。
///
/// # `device_id`
///
/// 这是**已知唯一一个不带就失败**的接口（H5 标了 `isDeviceId: true`），
/// 而且上游**校验它的内容** —— 见 [`ReceiveTarget::device_id`]。
///
/// ⚠ 会话在这里**又建了一次**（[`open_session`] 没暴露给外部）。
/// 自检/探测场景下多两次请求无所谓；真要做"签到 + 领取"一条龙时，
/// 应该把会话提出来复用，而不是让它建两遍。
pub async fn receive_cloud<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    target: &ReceiveTarget<'_>,
    clock: &K,
    rng: &mut R,
) -> Result<ReceiveResult, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_receive_v3(
            &jwt,
            &referer,
            target.cloud_id,
            target.cloud_type,
            target.device_id,
        )
        .await
}

// ─────────────────────────────────────────────────────────────────────────────
// 领取的分流：`receiveList[]` → 该发哪个请求
// ─────────────────────────────────────────────────────────────────────────────

/// `receiveList[]` 里一条**该怎么处理** —— 这就是 H5 的点击分流，只是不依赖 UI。
///
/// # 两条完全不同的线（这是最容易搞混的地方）
///
/// | | **月度任务奖励** | **立即领取** |
/// | --- | --- | --- |
/// | `cloudType` | `1`（**已到期、现在能领**）/ `2`（**还没到期**，下月才可领）| 其它值 |
/// | 来源 | 签到/做任务攒下的记录，**跨月结算** | 当期挂在账上的 |
/// | 接口 | `receiveTaskExpansion(acceptDate)` | `receiveV3(client, cloudId, cloudType)` |
/// | 门槛 | **要开启自动备份**（否则 `1302`）| 无 |
/// | 生效 | 按 `acceptDate` 结算，**不是立即** | **直接生效** |
///
/// ⇒ 所以「`receiveList` 里有个数」**推不出**「现在能领到」：
/// 那个数可能是**下个月**才作数的那一类（[`Self::NextMonth`]）。
///
/// ⚠ `1` 与 `2` 是**同一个特性的两个时点**，不是两种奖励：
/// `2` 是"本月攒的、下月才能领"的预告，等它到期就变成 `1`（那时才带 `acceptDate`）。
/// 这也解释了"为什么 `acceptDate` 只在 `1` 上出现" —— 只有到期的那条才需要结算日。
///
/// # 判据出处
///
/// H5 `newsignin.336f18c3.js` 里那三行（逐字见
/// [`mcloud_upstream::app_signin::ReceiveItem::cloud_type`]）：
///
/// ```js
/// 1 === t.cloudType ? A("2", t.acceptDate, e)   // receiveTaskExpansion
/// : 2 === t.cloudType ? s.$toast("下月可领取")  // 纯提示，不发请求
/// : n("receivePoints", t, "", …)                // receiveV3
/// ```
///
/// 旧项目 `mcloud-sign` 是同一套逻辑，只是拆成两个任务，而且它走的是**老路径**
/// （`mrp.mcloud.139.com/market/signin/page/taskExpansion`，新 H5 已经没有这个"查询"接口，
/// `acceptDate` 改由 `infoV3.receiveList[]` 直接带下来）：
///
/// | 老路径字段 | 对应 | 老项目怎么用 |
/// | --- | --- | --- |
/// | `curMonthTaskRecordCount` | `cloudType == 1` | `> 0` 就 `receiveTaskExpansion(acceptDate)` |
/// | `nextMonthTaskRecordCount` | `cloudType == 2` | **只打印**「下月可领取 N 个云朵」 |
/// | `curMonthBackup` | `infoV3.curMonthBackup` | `false` 就先上传个文件触发备份再重试 |
///
/// `receiveTask` 单独负责立即领取那一条（老路径是无参的
/// `GET …/market/signin/page/receiveV2?client=app`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveAction {
    /// **月度任务奖励已到期**（`cloudType == 1`）—— 走 `receiveTaskExpansion`。
    ///
    /// `accept_date` 是上游在**同一条条目**上给的结算日，不是我们编的日期。
    ///
    /// ⚠ `None` = 上游没给 —— 这时**照样发**，只是不带查询参数。理由：H5 的调用点
    /// 逐字是 `A("2", t.acceptDate, e)` → `receiveTaskExpansion({acceptDate: t})`，
    /// 而 `t.acceptDate` 缺席时是 `undefined`，axios 的默认序列化器会把
    /// `undefined` **整个键丢掉** ⇒ 真实客户端发出去的就是**裸 GET**。
    ///
    /// 这里曾经做成"缺了就跳过不发"，那会与真实客户端**行为分叉**：
    /// 上游明明受理，我们却什么都没发，还看不出为什么。要"可见"就该把
    /// `accept_date: None` 这件事**打进结果**（见 [`ReceiveOutcome::MonthlyExpansion`]），
    /// 而不是替上游决定不发。
    MonthlyExpansion {
        accept_date: Option<String>,
        cloud_num: i64,
    },
    /// **下月才能领**（`cloudType == 2`）—— 现在**什么都不发**。
    NextMonth { cloud_num: i64 },
    /// **立即领取、直接生效** —— 走 `receiveV3`。
    Immediate {
        cloud_id: String,
        cloud_type: i64,
        cloud_num: i64,
    },
}

/// 把 `infoV3` 的 `receiveList` 按上面的规则分流。**纯函数**，不碰网络。
///
/// 抽成纯函数是为了能单测：这条分流逻辑是"领错东西"的唯一防线，
/// 而它一旦错，表现只是"领到 0 朵"——**看不出来**。
pub fn plan_receive(receive_list: &[ReceiveItem]) -> Vec<ReceiveAction> {
    receive_list
        .iter()
        .map(|item| match item.cloud_type {
            // 月度任务奖励：`acceptDate` 有就用，没有就发裸请求（见变体的说明）。
            // 空串按"没有"处理 —— H5 那边 `""` 也会被当成有值发出去，但一个空日期
            // 对服务端没有意义，**发 `?acceptDate=` 反而多一个空参数**，不如不发。
            1 => ReceiveAction::MonthlyExpansion {
                accept_date: item.accept_date.clone().filter(|date| !date.is_empty()),
                cloud_num: item.cloud_num,
            },
            // 下月可领取：H5 连请求都不发。
            2 => ReceiveAction::NextMonth {
                cloud_num: item.cloud_num,
            },
            // 其余：立即领取。`cloudId` 照 H5 的 `e.recordId ?? ""` —— 缺了就是空串。
            cloud_type => ReceiveAction::Immediate {
                cloud_id: item.record_id.clone().unwrap_or_default(),
                cloud_type,
                cloud_num: item.cloud_num,
            },
        })
        .collect()
}

/// 一条 [`ReceiveAction`] 跑完之后的实况。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// 月度任务奖励领成功，`cloud_count` 见
    /// [`ReceiveTaskExpansion::cloud_count`]（`None` = 上游没说领到多少）。
    ///
    /// `accept_date` 是**发出去的那个值**：`None` = 没带查询参数（裸 GET）。
    /// 如实记下来，是为了让"这条到底发了什么"可查 —— 它不影响成功与否。
    MonthlyExpansion {
        accept_date: Option<String>,
        cloud_count: Option<i64>,
    },
    /// 下月才能领 —— 没发请求。
    NextMonth { cloud_num: i64 },
    /// 立即领取成功，`received` 是上游回的 `result.receive`。
    Immediate { cloud_type: i64, received: i64 },
    /// 请求发了但**失败了** —— 不中断其余条目，如实记下来。
    Failed { cloud_type: i64, error: String },
}

/// 按 [`plan_receive`] 的分流**把该领的都领一遍**。
///
/// # 与 [`receive_cloud`] 的区别
///
/// [`receive_cloud`] 是"**我指定**领哪一笔"（`--cloud-type` 那种自检用法）；
/// 本函数是"**按上游的账目自己判断**该领哪些" —— 这才是日常该走的路。
///
/// 之所以要有它：把"该领哪个"交给用户，等于让用户去猜 `cloudType` 的含义，
/// 而猜错的表现是"领到 0 朵"，**看起来像上游没发**。
///
/// # 会话只建一次
///
/// `infoV3` 与随后每个领取请求共用同一条会话 —— 见 [`open_session`]。
///
/// # 单条失败不中断
///
/// 一条失败（例如月度那条回 `1302` 未开备份）不影响其余条目：它们本来就是
/// 互不相干的账。失败进 [`ReceiveOutcome::Failed`]，由调用方决定怎么报。
pub async fn receive_all<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    device_id: &str,
    clock: &K,
    rng: &mut R,
) -> Result<Vec<ReceiveOutcome>, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    let status = upstream.app_sign_in_status(&jwt, &referer).await?;

    let mut outcomes = Vec::new();
    for action in plan_receive(&status.receive_list) {
        let outcome = match action {
            ReceiveAction::NextMonth { cloud_num } => ReceiveOutcome::NextMonth { cloud_num },
            ReceiveAction::MonthlyExpansion {
                accept_date,
                cloud_num: _,
            } => match upstream
                .app_receive_task_expansion(&jwt, &referer, accept_date.as_deref())
                .await
            {
                Ok(ReceiveTaskExpansion { cloud_count }) => ReceiveOutcome::MonthlyExpansion {
                    accept_date,
                    cloud_count,
                },
                Err(err) => ReceiveOutcome::Failed {
                    cloud_type: 1,
                    error: err.to_string(),
                },
            },
            ReceiveAction::Immediate {
                cloud_id,
                cloud_type,
                cloud_num: _,
            } => match upstream
                .app_receive_v3(&jwt, &referer, &cloud_id, Some(cloud_type), device_id)
                .await
            {
                Ok(ReceiveResult { receive }) => ReceiveOutcome::Immediate {
                    cloud_type,
                    received: receive,
                },
                Err(err) => ReceiveOutcome::Failed {
                    cloud_type,
                    error: err.to_string(),
                },
            },
        };
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// 用复活卡 / 回馈卡（`receiveRevivalReward`）。**这是写请求** —— 卡用掉就没了。
///
/// # 它跟 [`receive_cloud`] 不是同一件事
///
/// `receiveV3` 领的是**当期可领的云朵**；这一条用的是**卡**：
/// H5 弹窗写得很清楚 ——「关闭弹窗视为放弃机会，**无法补发**」。
/// 所以调用点必须是有意为之，不能"顺手也跑一下"。
///
/// # `device_id` 为什么是 `Option`
///
/// 两个证据不一致：H5 定义里**没有** `isDeviceId`（⇒ 不发），
/// 老抓包里却带了一个 `deviceid` **头**。哪个是服务端真正要求的**未验证**，
/// 所以留成可选参数 —— 真出问题时能一次 A/B 试出来（`receiveV3` 就是这么定的）。
/// 要显式给的话，取值走 [`crate::smid::device_id`]（纯 Rust，见那里）。
///
/// ⚠ 与会话的取舍同 [`receive_cloud`]：这里**又建了一次会话**。
pub async fn receive_revival_reward<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    device_id: Option<&str>,
    clock: &K,
    rng: &mut R,
) -> Result<RevivalReward, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_receive_revival_reward(&jwt, &referer, device_id)
        .await
}

/// 建会话：换 SSO token → 取服务端时间戳 → 签名 → `tyrzLogin`。
///
/// 返回 `(referer, jwtToken)`。`referer` 是签到页地址（带本次的 SSO token），
/// 后面每个请求都要带同一个 —— 它同时也是"这条会话属于哪个页面"的凭据。
///
/// 抽出来是因为 [`status`] 与 [`sign_in`] 的前四步**逐字相同**；
/// 复制一份的话，"签名少了一个头"这种错只会修在其中一条路径上。
async fn open_session<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<(String, String), Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    // ① 换 SSO token。`x-yun-user-timestamp` 是**微秒**，由 `user_timestamp` 统一换算。
    let sso_token = upstream
        .app_query_spec_token(
            profile,
            user_timestamp(clock),
            credential.authorization(),
            TO_SOURCE_ID,
        )
        .await?;

    let referer = signin_page_url(&sso_token);

    // ② 服务端毫秒时间戳。**不能**用本地时钟替代：签名里用的就是它。
    let timestamp = upstream.app_h5_timestamp().await?;
    let sign = sign_h5_request(rng, &timestamp);

    // ③ 建会话。SSO token 很短命（换完几十秒内就要用），所以它到这一步之间没有别的 IO。
    let login = upstream.app_h5_login(&sign, &referer, &sso_token).await?;

    Ok((referer, login.jwt_token))
}

// ─────────────────────────────────────────────────────────────────────────────
// 签到翻倍（`multiple`）—— 旧项目 `mcloud-sign` 的 `executeSignInMultiple`
// ─────────────────────────────────────────────────────────────────────────────
/// 签到翻倍**能不能领** —— 判据逐字来自旧项目 `mcloud-sign`
/// （`packages/core/src/tasks/sign/task.ts`）：
///
/// ```javascript
/// if (curMonthBackup && !curMonthBackupSignAccept) { await executeSignInMultiple(ctx); }
/// ```
///
/// 两个值都在 `infoV3` 里，见 [`SignInStatus::cur_month_backup`] /
/// [`SignInStatus::cur_month_backup_sign_accept`]。
///
/// # 为什么要有这个枚举，而不是在调用点写个 `if`
///
/// **每月只有一次机会** —— 条件不成立还发，是白白消耗掉这个月的名额。
/// 而"条件不成立"有三种完全不同的原因（已领过 / 没开备份 / 上游没说），
/// 它们的**处置相同、说法不同**。做成枚举就能：① 单独测；② 原样报给用户，
/// 而不是笼统一句"跳过"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplePlan {
    /// **可以领**：本月开着备份，且这一次机会还没用掉。
    Claim,
    /// 本月**已经领过** —— 不再发请求。
    AlreadyAccepted,
    /// 本月**没开自动备份** —— 翻倍机会根本没产生。
    ///
    /// ⚠ 旧项目在这种情况下会**上传一个文件把备份打开**再重试
    /// （`backFile` + 等 `backupWaitTime`，默认 20 秒）。我们**没有网盘上传能力**，
    /// 所以这里只如实报告，**不假装做过**。要补的是"上传"那条线，不是这条判定。
    NoBackup,
    /// 上游**没给**这两个字段 —— 判不了。
    ///
    /// 单独成一档是为了**安全**：`None` 既不是"开了"也不是"没开"。
    /// 当成"开了"会白发一个写请求、把名额用掉；当成"没开"会漏领。
    /// 所以这一档的处置是**什么都不发**，并把这个"判不了"如实报出来。
    Unknown,
}

/// 按 `infoV3` 的两个门槛字段判 [`MultiplePlan`]。
///
/// ⚠ 匹配顺序有意义：`(true, true)` 要排在 `(false, _)` 前面 ——
/// 两个都"不领"，但"本月已领过"是更具体的说法，报给用户更有用。
pub fn plan_multiple(status: &SignInStatus) -> MultiplePlan {
    match (status.cur_month_backup, status.cur_month_backup_sign_accept) {
        (Some(true), Some(false)) => MultiplePlan::Claim,
        (Some(true), Some(true)) => MultiplePlan::AlreadyAccepted,
        (Some(false), _) => MultiplePlan::NoBackup,
        _ => MultiplePlan::Unknown,
    }
}

/// 领签到翻倍的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultipleOutcome {
    /// 领到了。`cloud_count` 见 [`MultipleResult::cloud_count`]
    /// （`None` = 上游没说领到多少，**不是 0**）。
    ///
    /// `multiple` 是**倍数**（`N` 倍），与 `cloud_count`（合计）不是一个量 ——
    /// 见 [`MultipleResult::multiple`]。
    Claimed {
        cloud_count: Option<i64>,
        multiple: Option<i64>,
    },
    /// 本月已领过 —— 没发请求。
    AlreadyAccepted,
    /// 本月没开自动备份 —— 没发请求（见 [`MultiplePlan::NoBackup`]）。
    NoBackup,
    /// 门槛字段上游没给 —— 没发请求（见 [`MultiplePlan::Unknown`]）。
    Unknown,
    /// 门槛成立、请求也发了，但**失败**（例如回 `1302`）。
    ///
    /// 不抛 `Err`：这个开关的用途恰恰是"看上游到底回什么码"，
    /// 抛出去就只剩一句错误信息。与 [`receive_all`] 的 [`ReceiveOutcome::Failed`] 同策。
    Failed { error: String },
}

/// 领签到翻倍（`multiple`）—— **写请求**，每月一次。
///
/// 自己先取一次 `infoV3` 判门槛（[`plan_multiple`]），不成立就**一个请求都不发**。
///
/// # 它解决什么
///
/// 「自动备份」每月送一次翻倍机会，把**今日签到奖励**翻 2–50 倍。
/// 旧项目 `mcloud-sign` 把它放在 `signIn` 里顺手做掉（`executeSignInMultiple`），
/// 很容易被忽略 —— 而它是一条**独立的、每月一次的**收入。
pub async fn claim_multiple<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<MultipleOutcome, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    let status = upstream.app_sign_in_status(&jwt, &referer).await?;

    match plan_multiple(&status) {
        MultiplePlan::Claim => match upstream.app_claim_multiple(&jwt, &referer).await {
            Ok(MultipleResult {
                cloud_count,
                multiple,
            }) => Ok(MultipleOutcome::Claimed {
                cloud_count,
                multiple,
            }),
            Err(err) => Ok(MultipleOutcome::Failed {
                error: err.to_string(),
            }),
        },
        MultiplePlan::AlreadyAccepted => Ok(MultipleOutcome::AlreadyAccepted),
        MultiplePlan::NoBackup => Ok(MultipleOutcome::NoBackup),
        MultiplePlan::Unknown => Ok(MultipleOutcome::Unknown),
    }
}

/// 「备份好礼」的门槛判定（纯函数，可单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupGiftPlan {
    /// **可以领**：本月有礼、且还没领过。
    Claim,
    /// 本月**已领过** —— 不再发请求。
    AlreadyReceived,
    /// **未开启自动备份** —— 没有礼可领。
    ///
    /// 与 [`MultiplePlan::NoBackup`] 同源（旧项目这里也是提示"前往 APP 手动开启"），
    /// 同样**不上传文件去开备份** —— 我们没有网盘上传能力。
    NoBackup,
    /// 本月**没有**备份好礼（`curMonth` 为假）。
    ///
    /// 它比 [`Self::NoBackup`] 更靠前：旧项目先判 `if (!curMonth)` 就直接返回，
    /// 连 `state` 都不看。所以这一档**不是**"未开备份"，别把两者说成一回事。
    NothingThisMonth,
}

/// 按 `backupgift/info` 的两个字段判 [`BackupGiftPlan`]。
///
/// ⚠ 顺序有意义：`curMonth` 为假时**先**返回 [`BackupGiftPlan::NothingThisMonth`]，
/// 因为旧项目就是这么写的（`curMonth` 是前置开关，`state` 是它的细分类）。
pub fn plan_backup_gift(gift: &BackupGift) -> BackupGiftPlan {
    if !gift.cur_month {
        return BackupGiftPlan::NothingThisMonth;
    }
    match gift.state {
        // 旧项目的 `switch (state)`：`0` 可领、`1` 已领、`-1` 未开备份。
        0 => BackupGiftPlan::Claim,
        1 => BackupGiftPlan::AlreadyReceived,
        _ => BackupGiftPlan::NoBackup,
    }
}

/// 领「备份好礼」的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupGiftOutcome {
    /// 领到了。`received` 见 [`BackupGiftReceive::received`]（`None` = 上游没说）。
    Claimed { received: Option<i64> },
    /// 本月已领过 —— 没发请求。
    AlreadyReceived,
    /// 未开启自动备份 —— 没发请求。
    NoBackup,
    /// 本月没有备份好礼 —— 没发请求。
    NothingThisMonth,
    /// 门槛成立、请求也发了，但**失败**。不抛 `Err`，理由同
    /// [`MultipleOutcome::Failed`]。
    Failed { error: String },
}

/// 领「备份好礼」（`backupgift/receive`）—— **写请求**，每月一次。
///
/// 先查一次状态判门槛（[`plan_backup_gift`]），不成立就**一个请求都不发**。
///
/// # 它解决什么
///
/// 旧项目 `mcloud-sign` 的 `backupGiftTask`。它与 [`claim_multiple`] 是**两条独立的线**：
/// `multiple` 翻的是**今日签到奖励**，这条是「备份好礼」本身 ——
/// 两者都以「开了自动备份」为前提，所以 `state = -1` 时两条会一起拿不到，
/// 但**别因此把它们合并**：它们的接口、周期、门槛字段都不是同一个。
pub async fn claim_backup_gift<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
) -> Result<BackupGiftOutcome, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    let gift = upstream.app_backup_gift(&jwt, &referer).await?;

    match plan_backup_gift(&gift) {
        BackupGiftPlan::Claim => match upstream.app_receive_backup_gift(&jwt, &referer).await {
            Ok(BackupGiftReceive { received }) => Ok(BackupGiftOutcome::Claimed { received }),
            Err(err) => Ok(BackupGiftOutcome::Failed {
                error: err.to_string(),
            }),
        },
        BackupGiftPlan::AlreadyReceived => Ok(BackupGiftOutcome::AlreadyReceived),
        BackupGiftPlan::NoBackup => Ok(BackupGiftOutcome::NoBackup),
        BackupGiftPlan::NothingThisMonth => Ok(BackupGiftOutcome::NothingThisMonth),
    }
}

/// 查**待领取奖品**（`record` 的「待领取」页签）。只读。
///
/// # 它解决什么
///
/// 旧项目 `mcloud-sign` 的 `printPendingPrizes`：把"中了但还没领"的奖品列出来，
/// 连过期时间一起 —— 免得奖品躺着过期。
///
/// ⚠ 当前版 H5 的列表项**只显示「中奖时间」**（`insertTime`），
/// **没有 `expireTime`** —— 旧项目那个字段名在新接口上不存在，别照抄。
///
/// `page_size` 只取一页：这条的用途是"看有没有待领的"，不是把历史全拉下来。
pub async fn pending_prizes<C, K, R>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    credential: &SignInCredential,
    clock: &K,
    rng: &mut R,
    page_size: i64,
) -> Result<PrizeRecords, Error>
where
    C: mcloud_core::HttpClient,
    K: Clock,
    R: Random,
{
    let (referer, jwt) = open_session(upstream, profile, credential, clock, rng).await?;
    upstream
        .app_prize_records(
            &jwt,
            &referer,
            PRIZE_MARKET_NAME,
            PRIZE_STATUS_PENDING,
            1,
            page_size,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{HttpClient, HttpRequest, HttpResponse, User, UserId, UserStore};
    use mcloud_io::SqliteStore;
    use serde_json::{Value, json};
    use std::cell::RefCell;

    /// 确定性随机源：把给定字节循环填出去。
    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    /// 逐次返回脚本响应的假客户端，并记下发出去的请求。
    struct ScriptedClient {
        responses: RefCell<Vec<Value>>,
        seen: RefCell<Vec<HttpRequest>>,
    }

    impl ScriptedClient {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: RefCell::new(responses),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<HttpRequest> {
            self.seen.borrow().clone()
        }

        fn url_of(&self, index: usize) -> String {
            self.seen.borrow()[index].url.clone()
        }

        fn header_of(&self, index: usize, name: &str) -> Option<String> {
            self.seen.borrow()[index]
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        }
    }

    impl HttpClient for ScriptedClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
            self.seen.borrow_mut().push(request);
            let mut queue = self.responses.borrow_mut();
            if queue.is_empty() {
                return Err(Error::Internal("脚本已用完".into()));
            }
            Ok(HttpResponse::new(
                200,
                serde_json::to_vec(&queue.remove(0)).unwrap(),
            ))
        }
    }

    /// 黄金向量（真实抓包 + 真实响应）。
    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../test/golden/app-signin-vectors.json"))
            .expect("黄金向量文件必须是合法 JSON")
    }

    /// 拿真实 `infoV3` 响应，把**服务端标为今天**的那一格改成指定状态。
    ///
    /// 用真实响应而不是手搓一个最小 JSON：`cal` 里混着上/下月的补位格，
    /// 手搓的样本会把"按 `d` 裸查会命中错的月份"这个坑绕过去，
    /// 于是测试通过而实现在真实数据上出错。
    fn info_v3_with_today_signed(signed: bool) -> Value {
        let mut raw = vectors()["infoV3"]["raw"].clone();
        let cal = raw["result"]["cal"]
            .as_array_mut()
            .expect("黄金向量的 cal 必须是数组");
        let mut touched = 0;
        for cell in cal.iter_mut() {
            if cell["t"] == json!(true) {
                cell["s"] = json!(signed);
                touched += 1;
            }
        }
        assert_eq!(touched, 1, "真实响应里应当恰好有一格被标为今天");
        raw
    }

    fn app_device() -> mcloud_upstream::app_client::AppDevice {
        mcloud_upstream::app_client::AppDevice::ipad(
            "AAA00000000000000000000000000000",
            "AAA000000000",
        )
    }

    /// 库里放一个"已登录"的 App 账号（**v6 之后的形态**：`account_state.phone` 有值）。
    async fn store_with_login() -> (SqliteStore, Scope<'static>, UserId) {
        let s = SqliteStore::open_in_memory().unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-19T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let user = User::new("u1", "alice", "!", mcloud_core::Role::Admin, now);
        s.upsert_user(&user).await.unwrap();

        let account_id = AccountId::new(account_label(None, "19900000001"));
        let mut account = mcloud_core::Account::new(account_id.as_str());
        account.owner = Some(user.id.clone());
        // `platform` 是登录端（`parse_auth` 的第 1 段）—— App 端是 `mobile`。
        account.platform = Some("mobile".into());
        s.upsert_account(Scope::Local, &account).await.unwrap();
        s.upsert_account_state(
            Scope::Local,
            &mcloud_core::AccountState {
                account_id,
                jwt_token: Some("AUTHTOKEN".into()),
                user_domain_id: Some("1990000000000000002".into()),
                // v6 起登录会把完整手机号写进来 —— 这就是 `--account` 能省掉号码的依据。
                phone: Some("19900000001".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let owner = user.id.clone();
        (s, Scope::Local, owner)
    }

    /// 同上，但 `phone` 列是 `NULL` —— **v6 之前登录的老账号**。
    ///
    /// 刻意不另写一份建库代码：这里用一次"整体覆盖"把 `phone` 清成 `NULL`，
    /// 顺带钉住"覆盖写真的能清掉"这条语义（否则下面的用例可能只是"没写进去"的假象）。
    async fn store_with_legacy_login() -> (SqliteStore, Scope<'static>, UserId) {
        let (store, scope, owner) = store_with_login().await;
        let id = AccountId::new(account_label(None, "19900000001"));
        let mut state = store.get_account_state(scope, &id).await.unwrap().unwrap();
        state.phone = None;
        store.upsert_account_state(scope, &state).await.unwrap();
        (store, scope, owner)
    }

    /// 把账号的登录端改成别的（用于验证"端不匹配就拒绝"那道闸）。
    async fn set_platform(store: &SqliteStore, scope: Scope<'_>, platform: Option<&str>) {
        let id = AccountId::new(account_label(None, "19900000001"));
        let mut account = store.get_account(scope, &id).await.unwrap().unwrap();
        account.platform = platform.map(str::to_string);
        store.upsert_account(scope, &account).await.unwrap();
    }

    fn fixed_clock() -> mcloud_core::FixedClock {
        mcloud_core::FixedClock::new(
            chrono::DateTime::parse_from_rfc3339("2026-09-19T06:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
    }

    // ───────────────────────── 凭据 ─────────────────────────

    #[tokio::test]
    async fn credential_uses_the_stored_auth_token_and_the_full_phone() {
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        // `Authorization` 的形状必须与真机一致：`mobile:<完整手机号>:<authToken>`
        // —— 第一段是 `mobile`（不是 PC 的 `pc`），中间是**完整**手机号。
        assert_eq!(
            credential.authorization(),
            AppProfile::authorization("19900000001", "AUTHTOKEN")
        );
        let decoded = mcloud_core::parse_auth(credential.authorization()).unwrap();
        assert_eq!(decoded.platform.as_deref(), Some("mobile"));
        assert_eq!(decoded.phone.as_deref(), Some("19900000001"));
        assert_eq!(decoded.token.as_deref(), Some("AUTHTOKEN"));
        assert_eq!(credential.account_id().as_str(), "19****01");
    }

    #[tokio::test]
    async fn credential_debug_never_prints_the_token_or_the_full_phone() {
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        let shown = format!("{credential:?}");
        assert!(!shown.contains("AUTHTOKEN"), "{shown}");
        assert!(!shown.contains("19900000001"), "{shown}");
        assert!(shown.contains("19****01"), "{shown}");
    }

    #[tokio::test]
    async fn credential_refuses_an_account_that_is_not_in_the_store() {
        let (store, scope, _) = store_with_login().await;
        let err = SignInCredential::from_phone(&store, scope, "19900000002")
            .await
            .expect_err("库里没有这个账号就该早失败");
        assert!(format!("{err}").contains("没有账号"), "{err}");
    }

    #[tokio::test]
    async fn credential_refuses_an_empty_auth_token() {
        let (store, scope, _) = store_with_login().await;
        let account_id = AccountId::new(account_label(None, "19900000003"));
        let mut account = mcloud_core::Account::new(account_id.as_str());
        account.owner = Some(UserId::new("u1"));
        store.upsert_account(scope, &account).await.unwrap();
        store
            .upsert_account_state(
                scope,
                &mcloud_core::AccountState {
                    account_id,
                    // 空串与 None 必须同等对待：拼出来的头只会换来 90001，
                    // 而那个错误指向"未登录"，与真实原因（库里存了个空值）不符。
                    jwt_token: Some(String::new()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let err = SignInCredential::from_phone(&store, scope, "19900000003")
            .await
            .expect_err("空 authToken 必须被拒");
        assert!(format!("{err}").contains("凭据"), "{err}");
    }

    /// **v7 的核心**：库里有整串 `Authorization` 时**原样用它** —— 不解析、不重拼。
    ///
    /// 这条同时钉住三件事：
    ///   ① 整串优先于"`phone` + `jwt_token` 现拼"（两者故意给成不同的值）；
    ///   ② 不需要 `phone` 列 —— `asign.json` 导入的历史行没有它；
    ///   ③ 不需要 `jwt_token` 列 —— token 在整串里，所以"只有凭据、没有登录态"的行也能用。
    /// ③ 正是迁移回填造出来的形态；闸门顺序一旦排错，它会被误判成"没有凭据"。
    ///
    /// `platform` 故意写成 `2`（不是 `mobile`）：整串自带前缀，那道"端不匹配"的闸
    /// 拦的是"我们硬编码 `mobile`"，对这条路径不成立。旧项目同样不做这个区分
    /// （`initAccount` 里是 `setAuth({type:"basic", token})`，整串原样用）。
    #[tokio::test]
    async fn credential_prefers_the_whole_stored_authorization() {
        /// `Basic MjoxOTkwMDAwMDAwMTpTVE9SRUQtVE9LRU4=` = `2:19900000001:STORED-TOKEN`
        const STORED: &str = "Basic MjoxOTkwMDAwMDAwMTpTVE9SRUQtVE9LRU4=";

        let s = SqliteStore::open_in_memory().unwrap();
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-19T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let user = User::new("u1", "alice", "!", mcloud_core::Role::Admin, now);
        s.upsert_user(&user).await.unwrap();

        let account_id = AccountId::new(account_label(None, "19900000001"));
        let mut account = mcloud_core::Account::new(account_id.as_str());
        account.owner = Some(user.id.clone());
        account.platform = Some("2".into());
        s.upsert_account(Scope::Local, &account).await.unwrap();

        s.upsert_account_state(
            Scope::Local,
            &mcloud_core::AccountState {
                account_id: account_id.clone(),
                // 故意给一个**不同**的 token：整串优先时它不该被用上
                jwt_token: Some("REBUILT".into()),
                // `phone` 故意留空：整串里带着号码，这条路径不该需要它
                phone: None,
                authorization: Some(STORED.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let cred = SignInCredential::from_account(&s, Scope::Local, &account_id)
            .await
            .expect("整串在，就该能用 —— 不需要手机号也不需要 jwt_token");

        assert_eq!(cred.authorization(), STORED, "必须原样用整串，逐字节相同");
        assert_ne!(
            cred.authorization(),
            AppProfile::authorization("19900000001", "REBUILT"),
            "绝不能退回现拼 —— 那正是 v7 要消灭的路径"
        );
        assert_eq!(
            cred.phone(),
            "19900000001",
            "手机号只用于展示，从整串里顺手解出来"
        );
    }

    /// **缺口收口那条**：只要一个掩码 id，手机号从库里读。
    ///
    /// 与 `from_phone` 走同一条实现，所以拼出来的头必须**逐字节相同** ——
    /// 否则"补了个便捷入口"就变成了"多了一条会走偏的路"。
    #[tokio::test]
    async fn credential_reads_the_phone_from_the_store() {
        let (store, scope, _) = store_with_login().await;
        let id = AccountId::new(account_label(None, "19900000001"));

        let from_account = SignInCredential::from_account(&store, scope, &id)
            .await
            .unwrap();
        let from_phone = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        assert_eq!(
            from_account.authorization(),
            from_phone.authorization(),
            "两条路径拼出来的 Authorization 必须一模一样"
        );
        assert_eq!(
            from_account.authorization(),
            AppProfile::authorization("19900000001", "AUTHTOKEN")
        );
        assert_eq!(from_account.account_id().as_str(), "19****01");
    }

    /// `phone` 为 `NULL` 时（v6 之前登录的老账号）报错，且错误里给出**两条出路**。
    ///
    /// 这条刻意断言错误文案里同时出现"重新登录"与 `--phone`：
    /// 只报"没有手机号"会把用户晾在那儿 —— 他并不知道库里为什么会缺这个值。
    #[tokio::test]
    async fn credential_refuses_a_row_without_a_stored_phone() {
        let (store, scope, _) = store_with_legacy_login().await;
        let id = AccountId::new(account_label(None, "19900000001"));

        let err = SignInCredential::from_account(&store, scope, &id)
            .await
            .expect_err("库里没有完整手机号就该早失败，而不是拿掩码去凑");
        let text = format!("{err}");
        assert!(text.contains("重新登录"), "{text}");
        assert!(text.contains("--phone"), "{text}");
        // 关键：不能悄悄退化成用掩码拼头
        assert!(text.contains("掩码"), "要点明「别拿掩码凑」: {text}");

        // 同一个账号走 `--phone` 那条老路径仍然可用（这就是它保留的原因）
        let ok = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .expect("显式给号码时应能拼出来");
        assert_eq!(
            ok.authorization(),
            AppProfile::authorization("19900000001", "AUTHTOKEN")
        );
    }

    /// **PC 端登录的账号要拒掉。**
    ///
    /// 签到走的是 App 端凭据（`Authorization` 前缀 `mobile`），而 PC 登录存下来的
    /// 是 PC 端的 token 与 `pc` 前缀。这个组合**我们没有实测过**，
    /// 所以宁可让调用方显式去重登一次，也不要发出去试探 ——
    /// 那种失败长得像"未登录"，会把人带到"凭据坏了"的错误方向上。
    #[tokio::test]
    async fn credential_refuses_a_pc_logged_in_account() {
        let (store, scope, _) = store_with_login().await;
        set_platform(&store, scope, Some("pc")).await;
        let id = AccountId::new(account_label(None, "19900000001"));

        let err = SignInCredential::from_account(&store, scope, &id)
            .await
            .expect_err("PC 登录的凭据不该被拿来跑 App 链路");
        let text = format!("{err}");
        assert!(text.contains("--client pc"), "{text}");
        assert!(text.contains("--client app"), "要给出改法: {text}");
    }

    /// 反向确认那道闸**只新增拒绝、不改变既有行为**：
    /// `platform` 缺失（老数据、导入数据）时照旧放行。
    ///
    /// 不收紧的理由：`--phone` 这条老路径一直就是这个行为，
    /// 在这里顺手收紧会让一批能用的账号突然用不了，而收益只是"更一致"。
    #[tokio::test]
    async fn credential_allows_an_account_whose_platform_is_unknown() {
        let (store, scope, _) = store_with_login().await;
        set_platform(&store, scope, None).await;
        let id = AccountId::new(account_label(None, "19900000001"));

        let credential = SignInCredential::from_account(&store, scope, &id)
            .await
            .expect("登录端未知时不该拦");
        assert_eq!(
            credential.authorization(),
            AppProfile::authorization("19900000001", "AUTHTOKEN")
        );
    }

    // ───────────────────────── 主流程 ─────────────────────────

    /// 四步都成功、今天还没签 → 应当发满 6 个请求，并以 infoV3 的**回查**为准。
    #[tokio::test]
    async fn signs_when_today_is_not_signed_yet() {
        let client = ScriptedClient::new(vec![
            // ① querySpecTokenV2
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            // ② api/timestamp
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            // ③ tyrzLogin
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            // ④ infoV3（签到前）：今天未签
            info_v3_with_today_signed(false),
            // ⑤ startSignIn
            json!({ "code": 0, "msg": "success", "result": {
                "todaySignIn": false, "signInPoints": 3, "signCount": 2,
                "time": 1789799929696i64 } }),
            // ⑥ infoV3（回查）：今天已签
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        let report = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        assert!(report.signed_now(), "今天从「未签」变成了「已签」");
        assert!(report.signed_today());
        assert_eq!(report.sign_in_points(), Some(3));
        assert_eq!(client.sent().len(), 6, "四段链路一共 6 个请求");

        // 顺序：三段鉴权在前，两次 infoV3 夹着一次 startSignIn
        assert!(client.url_of(0).contains("/user/querySpecTokenV2"));
        assert!(client.url_of(1).ends_with("/ycloud/api/timestamp"));
        assert!(
            client
                .url_of(2)
                .contains("/ycloud/auth-service/auth/tyrzLogin")
        );
        assert!(client.url_of(3).contains("/ycloud/signin/page/infoV3"));
        assert!(client.url_of(4).contains("/ycloud/signin/page/startSignIn"));
        assert!(client.url_of(5).contains("/ycloud/signin/page/infoV3"));

        // `todaySignIn:false` 出现在**成功的**那一次 —— 它正是"调用前快照"的证据。
        // 如果哪天有人把它改成成败判据，这条断言会连同上面 `signed_now()` 一起炸。
        assert!(!report.result.as_ref().unwrap().today_sign_in);
    }

    /// ⚠ 这条锁住 `todaySignIn` 的反直觉语义：它**不能**当成败判据。
    ///
    /// 构造"`startSignIn` 回 `todaySignIn:true`（= 来的时候就已经签过了），
    /// 但回查 `infoV3` 说今天没签" —— 这种矛盾下必须**报错**而不是报成功。
    /// 真实世界里它对应"上游把我们的签到吞了"。
    #[tokio::test]
    async fn refuses_success_when_info_v3_still_says_unsigned() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(false),
            // 上游回了成功，而且 `todaySignIn:true`
            json!({ "code": 0, "msg": "success", "result": {
                "todaySignIn": true, "signInPoints": 3, "signCount": 2,
                "time": 1789799929696i64 } }),
            // 回查说**还是没签** —— 以这个为准
            info_v3_with_today_signed(false),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        let err = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .expect_err("回查说没签上就不能报成功");

        assert_eq!(err.kind(), mcloud_core::ErrorKind::Internal, "{err}");
        assert!(format!("{err}").contains("infoV3"), "{err}");
    }

    /// 今天已经签过 → **不发** `startSignIn`（少一次写请求、少一次风控记数）。
    #[tokio::test]
    async fn skips_start_sign_in_when_already_signed() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        let report = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        assert_eq!(client.sent().len(), 4, "已签过就只走鉴权 + 一次 infoV3");
        assert!(report.signed_today(), "今天确实已签");
        assert!(!report.signed_now(), "但**不是这次**签上的");
        assert!(report.result.is_none());
        assert_eq!(report.before, report.after, "没再查第二次");
    }

    /// `--force`（协议自检）：已签也发一次 `startSignIn`，
    /// 但**不能**把这次报成"刚签上的" —— 否则日志里会凭空多出一次签到成功。
    #[tokio::test]
    async fn force_resends_start_sign_in_without_claiming_a_fresh_sign_in() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            // 今天已经签过
            info_v3_with_today_signed(true),
            // 上游对"同一天重复签到"回成功 —— 实拍里真实客户端就是这么被回的
            // （黄金向量 `startSignIn` 那条：`todaySignIn:true` + `success`）
            json!({ "code": 0, "msg": "success", "result": {
                "todaySignIn": true, "signInPoints": 3, "signCount": 2,
                "time": 1789799929696i64 } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        let report = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions {
                force: true,
                ..Default::default()
            },
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        assert_eq!(client.sent().len(), 6, "force 下已签也要发 startSignIn");
        assert!(report.was_already_signed(), "来的时候就已经签过");
        assert!(report.signed_today());
        assert!(!report.signed_now(), "本来就是已签，不能说成「这次签上的」");
    }

    #[tokio::test]
    async fn status_only_never_signs() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(false),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();

        let s = status(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        assert_eq!(client.sent().len(), 4, "只查不签");
        assert_eq!(s.signed_today(), Some(false));
        assert!(!client.sent().iter().any(|r| r.url.contains("startSignIn")));
    }

    // ───────────────────────── 链路上的细节 ─────────────────────────

    /// `referer` 必须带上本次的 SSO token，且**逐字**是抓包里那个形状。
    #[tokio::test]
    async fn referer_carries_the_sso_token_and_the_signin_page_shape() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();
        let _ = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        let expected = mcloud_upstream::app_signin::signin_page_url("SSOTOKEN");
        // ③ 之后的每个请求都带同一个 referer
        for index in 2..4 {
            assert_eq!(
                client.header_of(index, "Referer").as_deref(),
                Some(expected.as_str()),
                "第 {index} 个请求的 referer 不对"
            );
        }
        assert!(expected.contains("targetSourceId=001005"), "{expected}");
    }

    /// 签名头必须与 `api/timestamp` 取回的那个值**逐字相同**。
    ///
    /// 这是"不能用本地时钟"那条规则的可执行形式：本地时间是 `2026-09-19T06:00:00Z`，
    /// 而服务端给的是 `1789800128813` —— 两者差着好几年，一旦有人改成读本地时钟，
    /// 这条断言立刻失败。
    #[tokio::test]
    async fn signature_uses_the_server_timestamp_not_the_local_clock() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();
        let _ = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        assert_eq!(
            client.header_of(2, "x-timestamp").as_deref(),
            Some("1789800128813")
        );
        let local = fixed_clock().now().timestamp_millis().to_string();
        assert_ne!(
            client.header_of(2, "x-timestamp").as_deref(),
            Some(local.as_str()),
            "用本地时钟就是 401"
        );
    }

    /// `querySpecTokenV2` 必须带完整手机号拼出来的 `Authorization`，
    /// 而**不是**掩码。掩码能过与否是未知数，所以不许出现。
    #[tokio::test]
    async fn query_spec_token_uses_the_full_phone_in_authorization() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();
        let _ = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        let auth = client.header_of(0, "Authorization").unwrap();
        assert_eq!(auth, AppProfile::authorization("19900000001", "AUTHTOKEN"));
        let decoded = mcloud_core::parse_auth(&auth).unwrap();
        assert_eq!(decoded.phone.as_deref(), Some("19900000001"));
        assert_ne!(decoded.phone.as_deref(), Some("19****01"));
    }

    /// 换来的 SSO token 必须原样进 `tyrzLogin` 的请求体，不许截断/转义。
    #[tokio::test]
    async fn sso_token_is_passed_through_verbatim() {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "YZsidssolgSYNTH000000000000000000000000000",
                              "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();
        let _ = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions::default(),
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        let body: Value =
            serde_json::from_slice(client.sent()[2].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body["token"],
            json!("YZsidssolgSYNTH000000000000000000000000000")
        );
        assert_eq!(body["marketName"], json!("sign_in_3"));
        assert_eq!(body["sourceId"], json!("1002"));
    }

    /// `device_id` 是 `Option`，但**传与不传必须在 URL 上看得出来**。
    #[tokio::test]
    async fn device_id_is_optional_but_visible_when_given() {
        async fn run(device_id: Option<&str>) -> String {
            let client = ScriptedClient::new(vec![
                json!({ "success": true, "code": "0000", "message": "请求成功",
                        "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
                json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
                json!({ "code": 0, "msg": "success",
                        "result": { "token": "JWT", "mobile": "19900000001" } }),
                info_v3_with_today_signed(false),
                json!({ "code": 0, "msg": "success", "result": {
                    "todaySignIn": false, "signInPoints": 3, "signCount": 2,
                    "time": 1789799929696i64 } }),
                info_v3_with_today_signed(true),
            ]);
            let upstream = Upstream::new(&client);
            let (store, scope, _) = store_with_login().await;
            let credential = SignInCredential::from_phone(&store, scope, "19900000001")
                .await
                .unwrap();
            let _ = sign_in(
                &upstream,
                &AppProfile::new(app_device()),
                &credential,
                &SignInOptions {
                    device_id,
                    ..Default::default()
                },
                &fixed_clock(),
                &mut SeqRandom(7),
            )
            .await
            .unwrap();
            client.url_of(4)
        }

        let without = run(None).await;
        assert_eq!(
            without,
            "https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app"
        );

        // 传了就要百分号编码：base64 里的 `+` 在查询串里表示空格，
        // 不编码服务端会解成另一个值**而且不报错**。
        let with = run(Some("a+b/c=")).await;
        assert!(with.ends_with("&deviceId=a%2Bb%2Fc%3D"), "{with}");
    }

    // ── 领取分流（`plan_receive`）────────────────────────────────────────────
    //
    // 这一组测的是"**该领哪个**"——它错了的表现是"领到 0 朵"，
    // 看起来像上游没发，所以必须有断言钉住。

    fn item(
        cloud_type: i64,
        cloud_num: i64,
        record_id: Option<&str>,
        accept: Option<&str>,
    ) -> ReceiveItem {
        ReceiveItem {
            cloud_type,
            cloud_num,
            record_id: record_id.map(str::to_string),
            accept_date: accept.map(str::to_string),
        }
    }

    /// **本次的核心结论**：`cloudType == 2` 是「下月可领取」—— 什么都不发。
    ///
    /// 判据是 H5 的点击分流（`2 === cloudType → $toast("下月可领取")`，
    /// 同一份源码里 `"is-next-month": 2 === cloudType` 与
    /// `2 === cloudType ? "" : "可领取"` 两处独立旁证）。
    ///
    /// 实测抓包里那条账号的 `receiveList` 只有一条 `{cloudType:2, cloudNum:234}`，
    /// 而整个 HAR 里 `receiveV3` 的请求数是 **0** —— 官方客户端正因为它是 `2`
    /// 而**从未领过**。所以"`receiveList` 里有个数"**推不出**"现在能领到"。
    #[test]
    fn cloud_type_2_is_next_month_and_never_claimed() {
        let plan = plan_receive(&[item(2, 154, None, None)]);
        assert_eq!(plan, vec![ReceiveAction::NextMonth { cloud_num: 154 }]);
    }

    /// `cloudType == 1` 走**月度**那条线：`acceptDate` 有就用，没有就发**裸 GET**。
    ///
    /// ⚠ 这条断言改过一次，原因值得记下来：原先写的是"缺 `acceptDate` 就跳过不发"。
    /// 那**与真实客户端分叉** —— H5 是 `receiveTaskExpansion({acceptDate: t.acceptDate})`，
    /// `t.acceptDate` 缺席时是 `undefined`，axios 把 `undefined` 整个键丢掉，
    /// 于是它照样发、只是不带参数。所以这里也发，把"没带日期"如实记进结果。
    #[test]
    fn cloud_type_1_is_monthly_and_omits_a_missing_accept_date() {
        let plan = plan_receive(&[item(1, 88, None, Some("2026-09-01"))]);
        assert_eq!(
            plan,
            vec![ReceiveAction::MonthlyExpansion {
                accept_date: Some("2026-09-01".to_string()),
                cloud_num: 88,
            }]
        );

        // 缺 `acceptDate` → 仍然是**要发**的那一条，只是不带参数。
        let plan = plan_receive(&[item(1, 88, None, None)]);
        assert_eq!(
            plan,
            vec![ReceiveAction::MonthlyExpansion {
                accept_date: None,
                cloud_num: 88,
            }],
            "缺日期不等于不发 —— 真实客户端会发裸 GET"
        );

        // 空串按"没有"处理：`?acceptDate=` 只是多一个空参数，不如不发。
        let plan = plan_receive(&[item(1, 88, None, Some(""))]);
        assert_eq!(
            plan,
            vec![ReceiveAction::MonthlyExpansion {
                accept_date: None,
                cloud_num: 88,
            }]
        );
    }

    /// 其它 `cloudType` 才是**立即领取**；`cloudId` 照 H5 的 `recordId ?? ""`。
    #[test]
    fn other_cloud_types_are_immediate_and_default_cloud_id_to_empty() {
        let plan = plan_receive(&[item(3, 20, None, None)]);
        assert_eq!(
            plan,
            vec![ReceiveAction::Immediate {
                cloud_id: String::new(),
                cloud_type: 3,
                cloud_num: 20,
            }],
            "实拍里 `receiveList[]` 没有 recordId，H5 给的就是空串"
        );

        let plan = plan_receive(&[item(3, 20, Some("REC-1"), None)]);
        assert_eq!(
            plan,
            vec![ReceiveAction::Immediate {
                cloud_id: "REC-1".to_string(),
                cloud_type: 3,
                cloud_num: 20,
            }]
        );
    }

    // ── 签到翻倍的门槛（`plan_multiple`）──────────────────────────────────────
    //
    // 判据逐字来自旧项目：`if (curMonthBackup && !curMonthBackupSignAccept)`。
    // 这一组错了的表现是**白白用掉每月一次的名额**，而名额是消耗品 ——
    // 错了不会报错，只会"这个月没领到"，所以必须有断言钉住。

    /// 只带两个门槛字段的状态 —— 其余字段与判定无关，给最小可编译的值。
    fn status_with_gates(backup: Option<bool>, accepted: Option<bool>) -> SignInStatus {
        SignInStatus {
            bean_total: 0,
            cloud_total: None,
            sign_in_points: 0,
            sign_count: 0,
            month_days: 0,
            days: Vec::new(),
            receive_list: Vec::new(),
            to_receive: None,
            cur_month_backup: backup,
            cur_month_backup_sign_accept: accepted,
        }
    }

    /// 两个都成立才领 —— 这是旧项目那行 `&&` 的逐字翻译。
    #[test]
    fn multiple_is_claimed_only_when_backup_is_on_and_not_yet_accepted() {
        assert_eq!(
            plan_multiple(&status_with_gates(Some(true), Some(false))),
            MultiplePlan::Claim
        );
        assert_eq!(
            plan_multiple(&status_with_gates(Some(true), Some(true))),
            MultiplePlan::AlreadyAccepted
        );
        assert_eq!(
            plan_multiple(&status_with_gates(Some(false), Some(false))),
            MultiplePlan::NoBackup
        );
        // 没开备份时 `SignAccept` 本该也是 `false`；万一上游给了矛盾的一对，
        // **照样不领**（`NoBackup` 优先于 `AlreadyAccepted` 的前提是它排在后面 ——
        // 这里钉住实际行为：先判 `(true,true)`，所以 `(false,true)` 落到 `NoBackup`）。
        assert_eq!(
            plan_multiple(&status_with_gates(Some(false), Some(true))),
            MultiplePlan::NoBackup
        );
    }

    /// ⚠ `None`（上游没说）**既不是"开了"也不是"没开"** —— 判不了就一个请求都不发。
    ///
    /// 这一条是**安全方向**的断言：当成"开了"会白发一个写请求、把每月一次的名额用掉；
    /// 当成"没开"会漏领。两个都不对，所以只能不动作 + 如实报告。
    #[test]
    fn multiple_does_nothing_when_a_gate_field_is_missing() {
        assert_eq!(
            plan_multiple(&status_with_gates(None, Some(false))),
            MultiplePlan::Unknown
        );
        assert_eq!(
            plan_multiple(&status_with_gates(Some(true), None)),
            MultiplePlan::Unknown
        );
        assert_eq!(
            plan_multiple(&status_with_gates(None, None)),
            MultiplePlan::Unknown
        );
    }

    // ── 备份好礼的门槛（`plan_backup_gift`）────────────────────────────────
    //
    // 这一组错了的表现是**白白用掉每月一次的名额**，与 `plan_multiple` 同源风险。

    /// 只带两个判据字段的备份好礼状态。
    fn gift(state: i64, cur_month: bool) -> BackupGift {
        BackupGift { state, cur_month }
    }

    /// 旧项目 `switch (state)` 的逐字翻译：`0` 领、`1` 已领、其余（实测 `-1`）未开备份。
    #[test]
    fn backup_gift_is_claimed_only_when_state_is_zero() {
        assert_eq!(plan_backup_gift(&gift(0, true)), BackupGiftPlan::Claim);
        assert_eq!(
            plan_backup_gift(&gift(1, true)),
            BackupGiftPlan::AlreadyReceived
        );
        assert_eq!(plan_backup_gift(&gift(-1, true)), BackupGiftPlan::NoBackup);
        // 认不出来的 `state` **落到不领**（`switch` 的 `default` 旧项目是"未知状态"告警）。
        // 这里不乐观：宁可漏领也不能把每月一次的名额乱花。
        assert_eq!(plan_backup_gift(&gift(7, true)), BackupGiftPlan::NoBackup);
    }

    /// `curMonth` 是**前置开关**，比 `state` 更靠前 —— 为假时连 `state` 都不看。
    ///
    /// 判据是旧项目那行 `if (!curMonth) { "无备份礼物可领"; return; }`。
    /// 所以这一档**不是**"未开备份"（[`BackupGiftPlan::NoBackup`]）——
    /// 把两者混起来会让用户跑去开一个已经开着的备份。
    #[test]
    fn backup_gift_cur_month_short_circuits_the_state() {
        assert_eq!(
            plan_backup_gift(&gift(0, false)),
            BackupGiftPlan::NothingThisMonth,
            "可领的 state 也救不了 curMonth 为假"
        );
        assert_eq!(
            plan_backup_gift(&gift(-1, false)),
            BackupGiftPlan::NothingThisMonth,
            "别把它说成 NoBackup"
        );
    }

    /// 一条 `receiveList` 里混着三种时，**顺序与逐条语义都要原样保住**。
    #[test]
    fn mixed_list_keeps_each_items_own_verdict() {
        let plan = plan_receive(&[
            item(2, 154, None, None),
            item(1, 88, None, Some("2026-09-01")),
            item(4, 7, Some("R-9"), None),
        ]);
        assert_eq!(
            plan,
            vec![
                ReceiveAction::NextMonth { cloud_num: 154 },
                ReceiveAction::MonthlyExpansion {
                    accept_date: Some("2026-09-01".to_string()),
                    cloud_num: 88,
                },
                ReceiveAction::Immediate {
                    cloud_id: "R-9".to_string(),
                    cloud_type: 4,
                    cloud_num: 7,
                },
            ]
        );
    }
}
