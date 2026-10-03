//! 移动云盘 **App 端签到（AI豆中心）** —— H5 链路的上游接口层。
//!
//! 对应 Go 参考的 `internal/upstream/app_signin*.go` 一族。
//!
//! # 为什么这一层看起来"不像一个 App 接口"
//!
//! 因为**签到根本不是一个原生接口，是一个 H5 页面**（`newsignin`）。
//! 原生 App 只做两件事：换一个 SSO token、把 WebView 打开到
//! `m.mcloud.139.com`。真正的签到动作**全由 H5 发出** —— 所以这一族
//! 用的是 H5 那套头（`jwttoken` / `activityid` / `referer`），
//! 而不是原生那套（`Authorization` + 设备头）。
//!
//! 建会话那三枪（`querySpecTokenV2` → `timestamp` → `tyrzLogin`）在
//! [`crate::session`]，由 [`crate::h5_client::H5Client`] 自动完成。
//! **本模块只管业务请求的构造与解析**，一个请求都不发。
//!
//! # 端点全部按实拍，不按旧 TS
//!
//! 旧项目写的是 `/market/signin/page/...`，实测是 `/ycloud/signin/page/...`
//! （前缀不同）。这一条在参考实现的 `../mcloud/docs/business-feature-inventory.md` §5.1
//! 有专门一节，而"照着旧代码写就错"的代价是任务**每天都失败**却看不出原因。
//!
//! # 本模块的证据等级
//!
//! | 端点 | 证据 |
//! |---|---|
//! | `infoV3` / `startSignIn` / `receiveV3` / `receiveRevivalReward` | **实拍**（`../mcloud/test/fixtures/http/*.json`、`test/golden/app-signin-vectors.json`）|
//! | `multiple` / `receiveTaskExpansion` | 旧 TS + H5 源码逐字，**缺成功样本**（两个测试号本月都已领过）|
//! | `record`（奖品） | **实拍**（`prizeRecords` 黄金向量）|
//! | `cloudRecordV2`（流水） | H5 源码逐字，**无实拍** |

use mcloud_core::{Error, HttpRequest, Result};
use serde_json::Value;

/// 签到活动的市场名，同时是请求头 `activityid` 的取值。
///
/// 由 `test/golden/app-signin-vectors.json` 的 `constants` 段钉住 ——
/// 改错一个不会报错，只会让整条链路变成"另一个客户端"。
pub const MARKET_NAME: &str = "sign_in_3";

/// 签到页的来源 id。
pub const SOURCE_ID: &str = "1002";

/// H5 请求头 `appversion`。
///
/// ⚠ 与原生 App 的 `12.6.0` **不是一个值**：这里带第 4 段（`12.6.0.0`）。
pub const H5_APP_VERSION: &str = "12.6.0.0";

/// AI豆中心的基址。
///
/// 复用 `session` 那一份 —— 同一个常量两处定义，改一处忘一处就会
/// 出现"建会话打 A 主机、业务请求打 B 主机"，而那看起来像凭据问题。
pub use crate::session::BASE_MCLOUD;

// ══════════════════════════ 端点 ══════════════════════════

/// 签到状态查询 —— **权威判据**。
///
/// # ⚠ 判成败一律回查它，不信 `startSignIn` 的响应
///
/// 实测两次：`startSignIn` 说没签，但紧接着 `infoV3` 已经是 `s:true` 了。
/// 只信 `startSignIn` 的实现会把一次**成功**的签到报成失败 ——
/// 而用户看到失败又点一次，就是重复请求。
pub const INFO_V3: &str = "https://m.mcloud.139.com/ycloud/signin/page/infoV3";

/// 执行签到。
pub const START_SIGN_IN: &str = "https://m.mcloud.139.com/ycloud/signin/page/startSignIn";

/// 领AI豆。
pub const RECEIVE_V3: &str = "https://m.mcloud.139.com/ycloud/signin/page/receiveV3";
/// 任务拓展的**读口**（只读）。⚠ 裸 URL，**没有查询串**。
pub const TASK_EXPANSION: &str = "https://m.mcloud.139.com/ycloud/signin/page/taskExpansion";

/// 用复活卡 / 回馈卡。
pub const RECEIVE_REVIVAL_REWARD: &str =
    "https://m.mcloud.139.com/ycloud/signin/page/receiveRevivalReward";

/// 签到翻倍（每月一次机会）。
pub const MULTIPLE: &str = "https://m.mcloud.139.com/ycloud/signin/page/multiple";

/// 延期领取（`cloudType == 1`）。
pub const RECEIVE_TASK_EXPANSION: &str =
    "https://m.mcloud.139.com/ycloud/signin/page/receiveTaskExpansion";

/// 奖品记录（默认页签是「待领取」）。
pub const PRIZE_RECORD: &str = "https://m.mcloud.139.com/ycloud/signin/public/record";

/// 豆的收支流水（审计线）。
pub const CLOUD_RECORD_V2: &str = "https://m.mcloud.139.com/ycloud/signin/public/cloudRecordV2";

/// 备份好礼的**状态**。
///
/// # ⚠ 主机与这一族其余接口**不同**
///
/// 它不在新 H5 的签到页里 —— 新 H5 里 `backupGift` 只出现一次，
/// 是一个指向**独立活动页**的 `popLink`，所以它没有"新接口"可换，
/// 沿用旧项目那条。
///
/// 换主机有实测依据：裸请求 `https://caiyun.feixin.10086.cn/market/backupgift/info`
/// 回 `{"code":90001,"msg":"未登录"}` —— 与 `m.mcloud.139.com` 上的
/// `record` / `liveRoomFeedback` **回同一个信封**，说明两者在同一道鉴权网关后面。
///
/// ⚠ 若哪天它回 `404` 而不是 `90001`，那是**换了主机**，别以为接口下线了。
pub const BACKUP_GIFT_INFO: &str = "https://caiyun.feixin.10086.cn/market/backupgift/info";

/// 领备份好礼。
///
/// 门槛**不在这里判**：先读 [`BACKUP_GIFT_INFO`] 的 `state`，
/// 只有 `state == 0`（可领）才发这一条。
pub const BACKUP_GIFT_RECEIVE: &str = "https://caiyun.feixin.10086.cn/market/backupgift/receive";

// ══════════════════════════ 请求构造 ══════════════════════════

/// 查询串值的百分号编码。
///
/// `deviceId` 是 base64，含 `+` `/` `=`。**`+` 在查询串里表示空格** ——
/// 不编码就会被服务端解成另一个值，而且看起来"没报错"，只是行为不对。
/// 抓包里这三个字符正是 `%2B` / `%2F` / `%3D`。
///
/// 只编码"会破坏查询串语义"的字符，其余原样保留 —— 与抓包一致
/// （抓包里字母数字都没被编码）。
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 签到页的 `referer`。
///
/// # 它复用 `session` 里那一份，不另写一个
///
/// `referer` 与票是同一条会话的两半，它必须**逐字**等于建会话时用的那个
/// 页面 URL（`RecipeParams::page_url`）。两处各拼一遍的后果是"票是对的、
/// referer 差一个参数"，而表现是 **90001 未登录** —— 那个码通常被读成
/// "票过期了"，排查方向整个跑偏。
///
/// ⚠ `targetSourceId` 必须带，且排在 `token` **之后**：参考实现在这里
/// 踩过一次坑（初版漏了它，因为是"照着实现写"而不是照着抓包写）。
pub fn signin_referer(sso_token: &str) -> String {
    crate::session::SessionRecipe::SignInPage
        .params()
        .page_url(sso_token)
}

/// `infoV3` 请求。
///
/// ⚠ `deviceId` **可以不带**（签到这一族里它只在 `receiveV3` 上是硬要求）。
/// 带上更贴近真实客户端。
pub fn info_v3_request(device_id: Option<&str>) -> HttpRequest {
    let mut url = format!("{INFO_V3}?client=app");
    if let Some(dev) = device_id {
        url.push_str(&format!("&deviceId={}", percent_encode(dev)));
    }
    h5(device_id.is_some(), HttpRequest::get(url), "true")
}

/// `startSignIn` 请求。
pub fn start_sign_in_request(device_id: Option<&str>) -> HttpRequest {
    let mut url = format!("{START_SIGN_IN}?client=app");
    if let Some(dev) = device_id {
        url.push_str(&format!("&deviceId={}", percent_encode(dev)));
    }
    h5(device_id.is_some(), HttpRequest::get(url), "true")
}

/// `receiveV3` 请求 —— **领AI豆**。
///
/// # ⚠ `deviceId` 在这里是**必填**
///
/// 它是这一族里**已知唯一一个"不带就失败"**的接口（H5 标了
/// `isDeviceId: true`）。不带会回 `615`。
///
/// ⚠ 别把"签到能不带"外推到这一族：`infoV3` / `startSignIn` 不带没事，
/// 这一个不行。参数是必填的（不是 `Option`），漏发在**编译期**就暴露。
///
/// # `cloudType` 为什么是可选且缺省是**空串**
///
/// H5 是 `e.cloudType ?? ""` —— 缺省给的是**空串**，不是 `0`。
/// 照抄，别"顺手改成 0"：`0` 是合法取值（当前可领取）。
pub fn receive_v3_request(device_id: &str, cloud_id: &str, cloud_type: Option<i64>) -> HttpRequest {
    let cloud_type: Value = match cloud_type {
        Some(n) => Value::from(n),
        None => Value::from(""),
    };
    let body = serde_json::json!({
        "client": "app",
        "cloudId": cloud_id,
        "cloudType": cloud_type,
        "deviceId": device_id,
    });
    h5(true, HttpRequest::post(RECEIVE_V3).json_body(&body), "true")
}

/// `receiveRevivalReward` 请求 —— 用掉一张卡。
///
/// body 是**空对象** `{}`（与抓包的 `content-length: 2` 对齐）——
/// 不是"没有 body"。
///
/// `showLoading` 是 `"false"`：同页其余接口几乎都是 `true`，
/// 只有它是 `false`（H5 要自己画那张卡片弹窗，不想被全局 loading 遮住）。
///
/// # `deviceId`：两个证据不一致，做成可选
///
/// - H5 源码（iOS 12.6.0）→ 头里**没有** `isDeviceId` ⇒ 拦截器不注入
/// - 老抓包（Android WebView）→ 带一个 `deviceid` **头**
///
/// **哪个是服务端真正要求的，未验证** —— 这正是留成可选的意义：
/// 真出问题时能一次 A/B 试出来。
pub fn receive_revival_reward_request(device_id: Option<&str>) -> HttpRequest {
    // ⚠ 这里补的是 `deviceid` **头**（老抓包那一档），
    // 与 `isdeviceid: true`（H5 拦截器那一档）是两件事 ——
    // 所以 `h5` 的第一个参数是 `false`，而头单独补。
    let req = h5(
        false,
        HttpRequest::post(RECEIVE_REVIVAL_REWARD).json_body(&serde_json::json!({})),
        "false",
    );
    match device_id {
        Some(dev) => req.header("deviceid", dev),
        None => req,
    }
}

/// `multiple` 请求 —— **签到翻倍**。
///
/// # 它领什么
///
/// 「自动备份」每月给**一次**翻倍机会：本月开着备份、且这次机会还没用掉时，
/// 领一次能把**今日签到奖励**翻 2–50 倍。
///
/// # ⚠ 门槛必须调用方自己判
///
/// 旧项目的判据逐字：`if (curMonthBackup && !curMonthBackupSignAccept)` ——
/// 两个值都在 `infoV3` 里。**两个都成立才去领**：每月只有一次机会，
/// 条件不成立还发，是**白白消耗**。
///
/// ⚠ 与 `receiveTaskExpansion` 同一个坑：H5 传的 `params` 是**空对象**，
/// axios 默认序列化器**一个查询参数都不产生** ⇒ 发的是**裸 GET**，不是 `?`。
pub fn multiple_request() -> HttpRequest {
    h5(false, HttpRequest::get(MULTIPLE), "true")
}

/// `receiveTaskExpansion` 请求 —— **延期领取**（`cloudType == 1`）。
///
/// **只有查询参数 `acceptDate`**、**没有 body**、**没有 `isDeviceId`**
/// （H5 没标 ⇒ 拦截器不注入）。别照 `receiveV3` 抄那套。
///
/// # `acceptDate` 为什么是可选（这条被实拍推翻过两次）
///
/// - H5 的调用点是 `receiveTaskExpansion({acceptDate: t})`，取自**同一条条目**；
/// - 而**实测两条账号的 `receiveList` 里都没有 `acceptDate` 这个字段**
///   （原始响应里只有 `cloudNum` / `cloudType`）⇒ 那个值是 `undefined`；
/// - axios 的默认序列化器对 `undefined` 是**整个键丢掉**（不是发
///   `acceptDate=`）⇒ 真实客户端发出去的是**裸 GET，一个查询参数都没有**。
///
/// 所以 `None` → 不带查询串（**与真实客户端一致，是缺省路径**）。
///
/// ⚠ 别把 `None` 写成空串 —— 那会发出 `?acceptDate=`，是**另一个请求**。
pub fn receive_task_expansion_request(accept_date: Option<&str>) -> HttpRequest {
    let url = match accept_date {
        Some(d) => format!("{RECEIVE_TASK_EXPANSION}?acceptDate={}", percent_encode(d)),
        None => RECEIVE_TASK_EXPANSION.to_string(),
    };
    h5(false, HttpRequest::get(url), "true")
}

/// `record` 请求 —— **奖品记录**（默认页签是「待领取」）。
///
/// # 分页判据与流水那条**不同**
///
/// H5 逐字写的是 **`records.length < pageSize` ⇒ 到底了** ——
/// 它**不读 `total`**。所以别照搬流水那边的字段名。
pub fn prize_record_request(
    market_name: &str,
    status: i64,
    page_number: i64,
    page_size: i64,
) -> HttpRequest {
    let url = format!(
        "{PRIZE_RECORD}?marketname={}&type={status}&pageNumber={page_number}&pageSize={page_size}",
        percent_encode(market_name)
    );
    h5(false, HttpRequest::get(url), "true")
}

/// `backupgift/info` 请求 —— 备份好礼的**状态**（只读）。
///
/// ⚠ 它走的是**另一个主机**（`caiyun.feixin.10086.cn`），
/// 不在 `m.mcloud.139.com` 那一族里 —— 别照 [`info_v3_request`] 抄。
pub fn backup_gift_info_request() -> HttpRequest {
    h5(false, HttpRequest::get(BACKUP_GIFT_INFO), "true")
}

/// `backupgift/receive` 请求 —— **领**备份好礼。
///
/// 门槛在调用方判（先看 `state`）。见 [`BackupGift::claimable`]。
pub fn backup_gift_receive_request() -> HttpRequest {
    h5(false, HttpRequest::get(BACKUP_GIFT_RECEIVE), "true")
}

/// `backupgift/info` 的 `state` —— **三态**。
///
/// 逐字来自旧项目的 `switch (state)`：三个值都可能出现，
/// 而"没开备份"与"本月已领过"要用**不同的话**说给用户听
/// （一个要他动手去开，一个什么都不用做）。
pub mod backup_gift_state {
    /// **未开启备份** ⇒ 不领，提示去 App 开。
    pub const OFF: i64 = -1;
    /// 本月**已领过** ⇒ 不领。
    pub const CLAIMED: i64 = 1;
    /// **可领** ⇒ 发 `receive`。
    pub const CLAIMABLE: i64 = 0;
}

/// `backupgift/info` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupGift {
    /// 主判据。**必填** —— 缺了它就无法回答"该不该领"。
    pub state: i64,
    /// 本月**有没有**备份好礼。
    ///
    /// 旧项目先判它：`if (!curMonth) { "无备份礼物可领"; return; }` ——
    /// 即它比 `state` **更靠前**，`false` 时连 `state` 都不用看。
    pub cur_month: bool,
}

impl BackupGift {
    /// 本月没有备份礼 ⇒ 无论如何都不用往下走。
    pub fn no_gift_this_month(&self) -> bool {
        !self.cur_month
    }

    /// 可以领吗。
    pub fn claimable(&self) -> bool {
        self.cur_month && self.state == backup_gift_state::CLAIMABLE
    }
}

/// 解析 `backupgift/info`。
pub fn parse_backup_gift(env: &SignInEnvelope) -> Result<BackupGift> {
    let r = env.ok_result("查询备份好礼状态")?;
    let state = int_at(r, "state")
        .ok_or_else(|| Error::Parse("backupgift/info 响应缺少 result.state（主判据）".into()))?;
    Ok(BackupGift {
        state,
        cur_month: bool_at(r, "curMonth"),
    })
}

/// `backupgift/receive` 的结果 —— 领到了多少。
///
/// ⚠ `result` 是**裸数字**（份数），不是对象；且可能整个缺失。
/// 所以它是可选的：缺了**不判失败**（领取这个动作本身已经成功了）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackupGiftReceive {
    pub received: Option<i64>,
}

/// 解析 `backupgift/receive`。
pub fn parse_backup_gift_receive(env: &SignInEnvelope) -> Result<BackupGiftReceive> {
    env.ensure_ok("领取备份好礼")?;
    Ok(BackupGiftReceive {
        // 裸数字与对象两种形态都收（H5 只读一个数）。
        received: env.result.as_ref().and_then(|r| match r {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => int_at(r, "receive").or_else(|| int_at(r, "cloudCount")),
        }),
    })
}

/// `cloudRecordV2` 请求 —— **豆的收支流水**（审计线）。
///
/// # `infoV3.total` 与它的关系
///
/// `total` 给的是**当前余额**，但不告诉你这个数是怎么来的 ——
/// 于是"余额对不上"只能靠猜。这条给的是**流水**。
///
/// 实测自洽：`aiDou=1` 那一档的**全部条目求和，逐字等于 `total`**。
///
/// ⚠ 函数名里那个 `AIBean` 是**误导**：同一个接口被两个页面共用，
/// 靠 `aiDou` 切换账本（`pointRecord` 传 `0`、`AIBeanRecord` 传 `1`）。
pub fn cloud_record_v2_request(
    ai_dou: i64,
    filter: i64,
    page_number: i64,
    page_size: i64,
) -> HttpRequest {
    let url = format!(
        "{CLOUD_RECORD_V2}?aiDou={ai_dou}&type={filter}&pageNumber={page_number}&pageSize={page_size}"
    );
    h5(false, HttpRequest::get(url), "true")
}

/// 给请求补上这一族共用的那几片头。
///
/// # ⚠ `jwttoken` 与 `Referer` **不在**这里
///
/// 它们由 [`crate::h5_client::H5Client`] 自动注入 —— 那是**会话的两半**，
/// 必须来自同一次建会话，不能由调用方各拼一遍。
///
/// 这一层一度把 `jwt` / `referer` 做成入参，那是错的：任务手里的客户端
/// 就是 `H5Client` 包出来的，它会**丢掉**调用方给的 `referer`、换成会话里
/// 那一条，并用会话的票覆盖 `jwttoken`。做成入参只会让人以为"传了就生效"，
/// 而实际传什么都被覆盖 —— 更糟的是有人会传一个自己拼的 referer，
/// 于是拿到"票是对的、referer 差一个参数 ⇒ 90001"那种最难查的组合。
///
/// 所以这里只管**与请求内容有关**的那几片：`activityid`（市场名）、
/// `appversion`（H5 版本）、`showloading`、`Accept`。
///
/// `is_device_id` 为真时补 `isdeviceid: true` —— **只有标了它的接口才补**。
/// 这一族里两条证据在 `receiveRevivalReward` 上打架，所以它是逐接口的入参，
/// 不是一刀切。
///
/// # 为什么提升到 `pub(crate)`
///
/// 兑换那一族（`crate::exchange`）用的是**同一套 H5 头**（同一个 `activityid`、
/// 同一个 `appversion`）。各写一遍的后果是两边各自漂移，而漂移的表现是
/// "签到能用、兑换回 90001"这种最难查的组合。
pub(crate) fn h5(is_device_id: bool, req: HttpRequest, show_loading: &str) -> HttpRequest {
    let req = req
        .header("Accept", "*/*")
        .header("activityid", MARKET_NAME)
        .header("appversion", H5_APP_VERSION)
        .header("showloading", show_loading);
    if is_device_id {
        req.header("isdeviceid", "true")
    } else {
        req
    }
}

// ══════════════════════════ 响应信封 ══════════════════════════

/// 签到族的响应信封。
///
/// # 这一族的 code 是**数字**
///
/// 与登录族（`"0000"` 字符串）不同：签到族实拍是 `{"code":0,"msg":"success"}`。
/// 所以两个信封**不能互换** —— 拿登录那个去解签到会永远判失败
/// （它把 `0` 当成"不是 0000"）。
#[derive(Debug, Clone)]
pub struct SignInEnvelope {
    pub code: i64,
    pub message: String,
    /// `result` 原样保留。`None` = 响应里没有这个键。
    pub result: Option<Value>,
}

impl SignInEnvelope {
    pub fn parse(body: &str) -> Result<Self> {
        let v: Value = serde_json::from_str(body)
            .map_err(|e| Error::Parse(format!("签到响应不是 JSON：{e}")))?;

        // code 既可能是数字也可能是数字字符串 —— 两种都收。
        let code = match v.get("code") {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(-1),
            Some(Value::String(s)) => s.parse().unwrap_or(-1),
            _ => -1,
        };
        // 上游用过 `msg` 与 `message` 两个键名。
        let message = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(Self {
            code,
            message,
            result: v.get("result").cloned(),
        })
    }

    /// 成功时取 `result`（对象）。
    ///
    /// # ⚠ 判据是 `code == 0`
    ///
    /// 这一族**没有一个统一的成功码**：`receiveTaskExpansion` / `notice`
    /// 那些接口的 `result` 甚至是**裸数字**（`result: 100`）。
    /// 所以成功判定只看 `code`，形状校验交给各自的解析函数。
    pub fn ok_result(&self, what: &str) -> Result<&Value> {
        self.ensure_ok(what)?;
        self.result
            .as_ref()
            .ok_or_else(|| Error::Parse(format!("{what} 响应没有 result")))
    }

    /// 成功判定但**不要求** `result`（有些接口成功时没有 result）。
    pub fn ensure_ok(&self, what: &str) -> Result<()> {
        if self.code == 0 {
            return Ok(());
        }
        Err(self.error(what))
    }

    /// 把非 0 业务码翻成一条**能指导排查**的错误。
    ///
    /// # ⚠ 会话失效要单独说清楚（一处从旧实现保留下来的行为）
    ///
    /// `90001` 不是"任务失败"，是**票的问题** —— 票过期了，或者这张票
    /// 不属于当前页面（`referer` 与会话对不上）。
    ///
    /// 把它混进一句干巴巴的"失败 code=90001"，排查方向会整个跑偏：
    /// 人会去查账号、查权限、查重试，而真正该做的是**重建会话**。
    /// 所以这里换成会话层那句人话。
    fn error(&self, what: &str) -> Error {
        if self.code == crate::session_source::CODE_SESSION_EXPIRED {
            return Error::Other(format!(
                "{what} 回会话失效（code={}）—— {}",
                self.code,
                crate::session_source::describe(crate::session_source::CODE_SESSION_EXPIRED)
            ));
        }
        Error::Upstream {
            code: self.code.to_string(),
            message: format!("{what}：{}", self.message),
        }
    }
}

// ══════════════════════════ 解析 ══════════════════════════

fn int_at(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        // 上游有的字段是字符串形式的数字
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn bool_at(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// 日历里的一格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInDay {
    pub day: u8,
    pub signed: bool,
    /// 是否属于**本月** —— 非本月的格子是上下月的补位，**不算今天**。
    pub current_month: bool,
    pub points: i64,
    /// 是否**今天**。
    pub today: bool,
}

/// `infoV3` 里我们关心的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInStatus {
    /// **现行余额（豆）** —— 上游字段 `total`。
    ///
    /// ⚠ 不是 `beforeTotal`。那个是"改名那一刻的结转额"，是一张
    /// **冻结的历史快照**，永远不变 —— 曾把它当余额记进趋势图，
    /// 结果是一条直线。
    pub total: i64,
    /// 本次签到可得数（`signInPoints`）—— 是"能拿多少"，不是"已到手"。
    pub sign_in_points: i64,
    pub sign_count: i64,
    pub month_days: i64,
    pub days: Vec<SignInDay>,
    pub receive_list: Vec<ReceiveItem>,
    /// 待领取AI豆数。
    pub to_receive: Option<i64>,
    /// 本月是否开着自动备份。
    pub cur_month_backup: Option<bool>,
    /// 本月备份礼那一次机会**是否已经用掉**。
    ///
    /// 与 `cur_month_backup` 一起构成 `multiple` 的门槛：
    /// **两个都成立才去领**（`cur_month_backup && !cur_month_backup_sign_accept`）。
    pub cur_month_backup_sign_accept: Option<bool>,
}

impl SignInStatus {
    /// 今日是否已签到 —— **唯一权威判据**。
    ///
    /// 只在"本月的格子"里找 `today`：
    /// 上下月的补位格带上 `t` 是上游的实现细节，信它会把"今天"认错。
    pub fn signed_today(&self) -> bool {
        self.days
            .iter()
            .any(|d| d.today && d.current_month && d.signed)
    }

    /// 今天那一格的读数（排障用；用 [`Self::signed_today`] 判成败）。
    pub fn today_cell(&self) -> Option<&SignInDay> {
        self.days.iter().find(|d| d.today && d.current_month)
    }

    /// 是否应当去领 `multiple`（签到翻倍）。
    ///
    /// 旧项目的判据逐字：`curMonthBackup && !curMonthBackupSignAccept`。
    /// 两个值上游都可能不给 —— 缺任何一个都**不去领**：
    /// 每月只有一次机会，条件不成立还发是**白白消耗**。
    pub fn should_claim_multiple(&self) -> bool {
        self.cur_month_backup == Some(true) && self.cur_month_backup_sign_accept == Some(false)
    }
}

/// `receiveList` 里的一笔。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveItem {
    /// 决定这一笔现在能不能领。
    ///
    /// | 值 | 意义 | 动作 |
    /// |---|---|---|
    /// | `0` | 现在可领 | `receiveV3` |
    /// | `1` | 已到期、现在能领 | `receiveTaskExpansion`（要 `acceptDate`）|
    /// | `2` | 下月可领取 | **什么都不发**（H5 只弹一句 toast）|
    pub cloud_type: i64,
    pub cloud_num: i64,
    /// `receiveV3` 的 `cloudId` 来源。
    ///
    /// ⚠ 实测抓到的条目里**没有这个字段**（只有 `cloudType` / `cloudNum`），
    /// 于是 H5 的 `e.recordId ?? ""` 实际给的就是空串。
    /// 这里如实做成可选，**不替上游编一个默认值**。
    pub record_id: Option<String>,
    /// 只有 `cloud_type == 1` 才用得上。
    pub accept_date: Option<String>,
}

/// AI豆账目的三种类型。
pub mod cloud_type {
    /// **当前可领取**。
    pub const NOW: i64 = 0;
    /// 已到期、现在能领 ⇒ 走 `receiveTaskExpansion`。
    pub const EXPANSION: i64 = 1;
    /// **下月可领取** ⇒ 什么都不发。
    pub const NEXT_MONTH: i64 = 2;
}

/// 解析 `infoV3`。
pub fn parse_status(env: &SignInEnvelope) -> Result<SignInStatus> {
    let r = env.ok_result("查询签到状态")?;

    let total = int_at(r, "total")
        .ok_or_else(|| Error::Parse("infoV3 响应缺少 result.total（余额）".into()))?;

    let cells = r
        .get("cal")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("infoV3 响应缺少 result.cal 数组".into()))?;

    let mut days = Vec::with_capacity(cells.len());
    for cell in cells {
        let day = int_at(cell, "d").ok_or_else(|| Error::Parse("infoV3 的 cal[] 缺少 d".into()))?;
        if !(0..=31).contains(&day) {
            return Err(Error::Parse(format!("infoV3 的 cal[].d 越界：{day}")));
        }
        days.push(SignInDay {
            day: day as u8,
            signed: bool_at(cell, "s"),
            current_month: int_at(cell, "currentMonth") == Some(1),
            points: int_at(cell, "n").unwrap_or(0),
            today: bool_at(cell, "t"),
        });
    }

    // `receiveList` 与主判据无关：上游不给时给空，而不是报错 ——
    // 那会让"今天签没签"这件确定的事被一个可选的附属字段拖垮。
    let mut receive_list = Vec::new();
    if let Some(items) = r.get("receiveList").and_then(Value::as_array) {
        for item in items {
            let Some(cloud_type) = int_at(item, "cloudType") else {
                continue;
            };
            receive_list.push(ReceiveItem {
                cloud_type,
                cloud_num: int_at(item, "cloudNum").unwrap_or(0),
                record_id: item
                    .get("recordId")
                    .and_then(Value::as_str)
                    .map(String::from),
                accept_date: item
                    .get("acceptDate")
                    .and_then(Value::as_str)
                    .map(String::from),
            });
        }
    }

    Ok(SignInStatus {
        total,
        sign_in_points: int_at(r, "signInPoints").unwrap_or(0),
        sign_count: int_at(r, "signCount").unwrap_or(0),
        month_days: int_at(r, "monthDays").unwrap_or(0),
        days,
        receive_list,
        to_receive: int_at(r, "toReceive"),
        cur_month_backup: r.get("curMonthBackup").and_then(Value::as_bool),
        cur_month_backup_sign_accept: r.get("curMonthBackupSignAccept").and_then(Value::as_bool),
    })
}

/// `receiveV3` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveResult {
    /// 本次领到的AI豆数（`result.receive`）。
    ///
    /// ⚠ 与 [`SignInStatus::sign_in_points`] **不是一回事**：
    /// 那个是"签到能拿多少"（还没到手），这个是"这次真进账多少"。
    pub receive: i64,
}

/// 解析 `receiveV3`。
pub fn parse_receive_v3(env: &SignInEnvelope) -> Result<ReceiveResult> {
    let r = env.ok_result("领取AI豆")?;
    Ok(ReceiveResult {
        receive: int_at(r, "receive")
            .ok_or_else(|| Error::Parse("receiveV3 响应缺少 result.receive".into()))?,
    })
}

/// `receiveRevivalReward` 的解析结果 —— **用掉一张卡，换回一批AI豆**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevivalReward {
    /// `1` = **复活卡**（随机复活上一年失效的AI豆），`2` = **回馈卡**。
    ///
    /// H5 用 `1 == cardType ? "复活卡弹窗" : "回馈卡弹窗"` 分流，
    /// 两张卡连累计数的**文案单位都不同**。所以它是判据不是装饰。
    pub card_type: i64,
    /// 本次随机复活到的AI豆数 —— 这就是"这次进账多少"。
    pub reward_clouds: i64,
    /// 累计数（复活卡口径下是"累计复活 AI 豆"）。
    ///
    /// ⚠ **别把它接进余额记录**：它到底是不是"AI豆余额"口径**没有验证**
    /// —— H5 文案说的是"复活卡把失效云朵**转换成 AI 豆**"。
    /// 现在接进去只会把两个口径混进同一条曲线。
    pub total_clouds: i64,
    /// 只影响一句文案，服务端不给就给 0，**不因此判整条响应失败**。
    pub little_first_time: i64,
}

/// 解析 `receiveRevivalReward`。
pub fn parse_revival_reward(env: &SignInEnvelope) -> Result<RevivalReward> {
    let r = env.ok_result("领取复活奖励")?;
    let need = |k: &str, what: &str| {
        int_at(r, k).ok_or_else(|| Error::Parse(format!("{what} 响应缺少 result.{k}")))
    };
    Ok(RevivalReward {
        card_type: need("cardType", "receiveRevivalReward")?,
        reward_clouds: need("rewardClouds", "receiveRevivalReward")?,
        total_clouds: need("totalClouds", "receiveRevivalReward")?,
        // 附属字段：缺了不判失败
        little_first_time: int_at(r, "littleFirstTime").unwrap_or(0),
    })
}

/// ⭐ 判断这一发 `receiveRevivalReward` 上游其实在说"本月已经领过了"。
///
/// # 为什么需要它（真账号实拍）
///
/// 这一族**没有判活接口**（参考实现在 §5.8 记着"从未抓到"），所以
/// "本月还剩几张卡"只能靠发一次写请求去撞。真账号跑出来的答案是：
///
/// ```text
/// HTTP 200，{"code":500,"msg":"本月已领取复活卡奖励"}
/// ```
///
/// 它**不是故障**：本月这一档确实没得领，处置与"签到翻倍本月已领过"
/// 完全同档（报"没领 + 依据"，**不报失败**）。
///
/// 判错方向的代价是**不对称的**：判成失败会让历史页每天一条红、
/// 并触发 `onlyError` 推送，而它下个月之前都不会自己好。
///
/// # 匹配宽严
///
/// 双条件：先要出现"已领取 / 已领过"，再要出现这一族的主体词
/// （复活卡 / 回馈卡 / 奖励）。
///
/// ⚠ **裸一句"本月已领取"（不带卡型也不带"奖励"）刻意不在匹配面里**：
/// 这一族没有判活接口，把认不出的文案折成"没领"就等于把真故障洗白。
/// 宁可漏认一档（报失败，人来判）也不误判一片。
pub fn revival_already_claimed(env: &SignInEnvelope) -> bool {
    revival_message_already_claimed(&env.message)
}

/// 同上，但只吃文案（解析失败时也会走到这里）。
pub fn revival_message_already_claimed(message: &str) -> bool {
    if !message.contains("已领取") && !message.contains("已领过") {
        return false;
    }
    message.contains("复活卡") || message.contains("回馈卡") || message.contains("奖励")
}

/// `multiple` 的解析结果 —— 这一次额外领到多少豆。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MultipleResult {
    /// 本次额外获得的豆数（`result.cloudCount`）。
    ///
    /// # 为什么是可选
    ///
    /// **没有实拍**（两个测试号本月都已领过，复现不出来），且缺了不判失败 ——
    /// 硬要求它会把一次**成功**的领取变成解析错误。
    pub cloud_count: Option<i64>,
    /// **倍数**（N 倍），不是豆数。
    ///
    /// 旧项目逐字读了它并拼成一句日志：「成功获得 {multiple} 倍云朵，
    /// 共计 {cloudCount}」—— 即 **`multiple` 是倍数、`cloudCount` 是合计**，
    /// 两者不是一个量。只报 `cloudCount` 会让"翻了几倍"这个信息丢掉。
    pub multiple: Option<i64>,
}

/// 解析 `multiple`。
///
/// `result` 缺失**不判失败** —— 理由见 [`MultipleResult::cloud_count`]。
pub fn parse_multiple(env: &SignInEnvelope) -> Result<MultipleResult> {
    env.ensure_ok("签到翻倍")?;
    let Some(r) = env.result.as_ref() else {
        return Ok(MultipleResult::default());
    };
    Ok(MultipleResult {
        cloud_count: int_at(r, "cloudCount"),
        multiple: int_at(r, "multiple"),
    })
}

/// 奖品记录里的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrizeRecord {
    pub prize_name: String,
    /// `flag == 1` ⇒ 待领取。
    pub flag: i64,
    pub market: String,
    /// `oid` 实拍是**数字**（`1270932714`），而 H5 把它拼进 URL 查询串，
    /// 故这里统一成字符串。
    pub oid: String,
    pub prize_id: i64,
    pub expire_time: Option<String>,
    pub insert_time: Option<String>,
}

impl PrizeRecord {
    /// 是否待领取。
    pub fn pending(&self) -> bool {
        self.flag == 1
    }
}

/// 解析 `record`（奖品记录）。
pub fn parse_prize_records(env: &SignInEnvelope) -> Result<Vec<PrizeRecord>> {
    let r = env.ok_result("查询奖品记录")?;
    let records = r
        .get("records")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("record 响应缺少 result.records 数组".into()))?;

    Ok(records
        .iter()
        .map(|it| PrizeRecord {
            prize_name: it
                .get("prizeName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            flag: int_at(it, "flag").unwrap_or(0),
            market: it
                .get("market")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            // 数字与字符串两种形态都收
            oid: match it.get("oid") {
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            },
            prize_id: int_at(it, "prizeId").unwrap_or(0),
            expire_time: it
                .get("expireTime")
                .and_then(Value::as_str)
                .map(String::from),
            insert_time: it
                .get("insertTime")
                .and_then(Value::as_str)
                .map(String::from),
        })
        .collect())
}

/// 流水里的一笔。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudRecord {
    /// 正负号就是进出。
    pub num: i64,
    pub summary: String,
    pub mark: String,
    pub insert_time: Option<String>,
}

/// 解析 `cloudRecordV2`（豆的收支流水）。
pub fn parse_cloud_records(env: &SignInEnvelope) -> Result<Vec<CloudRecord>> {
    let r = env.ok_result("查询AI豆流水")?;
    let records = r
        .get("records")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Parse("cloudRecordV2 响应缺少 result.records 数组".into()))?;

    Ok(records
        .iter()
        .map(|it| CloudRecord {
            num: int_at(it, "num").unwrap_or(0),
            summary: it
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            mark: it
                .get("mark")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            insert_time: it
                .get("inserttime")
                .or_else(|| it.get("insertTime"))
                .and_then(Value::as_str)
                .map(String::from),
        })
        .collect())
}

// ══════════════════════════ 任务拓展（`taskExpansion`）══════════════════════════
//
// ⚠ 这一族的**领取**那一口（`receiveTaskExpansion`）**不在这里** —— 它早就随签到族落码了
// （见本文件里那份 `receive_task_expansion_request`）。把只读的那一口与写的那一口分开放，
// 是为了让"哪些口会改用户账上的东西"那份清单**一眼可判**，而不用逐条读函数名。

/// `GET /ycloud/signin/page/taskExpansion` —— 只读。
///
/// # 裸 URL，**没有查询串**
///
/// 实拍那两条的 URL **逐字不带** `?client=`（与同族的 `infoV3?client=app` 不同）。
/// ⚠ 别"顺手对齐同族"补一个 `client=app`：那是**另一个请求**，而实拍里没有它。
pub fn task_expansion_request(device_id: Option<&str>) -> HttpRequest {
    let req = h5(false, HttpRequest::get(TASK_EXPANSION), "true");
    match device_id {
        Some(id) => req.header("deviceid", id),
        None => req,
    }
}

/// `taskExpansion` 的解析结果（**六格全部可选**）。
///
/// # 这一族在做什么
///
/// * `cur_month_backup == Some(false)` ⇒ 先往网盘**传一个 backup 档的文件**去"凑备份"，
///   等一会儿之后**再读一次**；重试**只有一次**，第二次还是 false 就只念一句
///   "本月未开启备份，将无法获取翻倍奖励"。
/// * `cur_month_task_record_count > 0` ⇒ 拿**同一条响应里的** `accept_date` 去领。
/// * `next_month_task_record_count` **只打印**，不发任何请求。
///
/// # 为什么六格全是指针
///
/// 实拍那两条是**同一份读数**：`curMonthBackup:true`、`curMonthBackupTaskAccept:false`、
/// `curMonthTaskRecordCount:0`、`nextMonthTaskRecordCount:358`、`preMonthBackup:false` ——
/// 给了**五格**、**没给 `accept_date`** ⇒ 六格里至少有一格是"上游可能就是不给"。
/// 而这一口是**只读**的：把"缺字段"判成解析失败，表现是换一种上游措辞就整条能力报红。
///
/// ⚠ `None` 与"读到 `false` / `0`"在两处判据上**不等价**：编排层必须能分清
/// "上游说这个号没开备份"（⇒ 要凑）与"上游没说"（⇒ **不凑、也不领**）。
/// 把"没读到"当"没开"的实际后果是**往用户网盘写一个文件**。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskExpansion {
    /// `curMonthBackup` —— 本月**是否已开启自动备份**。旧实现唯一的用法就是它。
    ///
    /// ⚠ **只认 JSON 真正的 `true` / `false`**。万一上游改成 `1` / `0`，
    /// 这里得到的是 `None` ⇒ 调用方判"未知"而**不发那一发写**。
    /// 这与本文件里那两个"要不要动用户东西"的门槛字段**同一档**，不是巧合。
    ///
    /// ⚠ 实拍那两条读到的都是 `true` ⇒ **"未开备份的号回什么形状"仍是待验证的**。
    pub cur_month_backup: Option<bool>,
    /// `preMonthBackup` —— 上月是否开着备份。
    ///
    /// ⚠ 旧实现**一个字都不读**它 ⇒ 带出来只为排障时能看见上游说了什么，
    /// 不参与任何判定，也不进对外文案。
    pub pre_month_backup: Option<bool>,
    /// `curMonthBackupTaskAccept` —— 本月的备份任务**是否已经领过**。
    ///
    /// 实拍给过这一格（`false`），但旧实现**一个字都不读**它 ⇒
    /// 带出来不等于本层据此分支。
    pub cur_month_backup_task_accept: Option<bool>,
    /// `curMonthTaskRecordCount` —— 本月攒下的任务记录数（旧实现判 `> 0` 才去领）。
    pub cur_month_task_record_count: Option<i64>,
    /// `nextMonthTaskRecordCount` —— 下月可领的朵数（旧实现只打印，不发请求）。
    pub next_month_task_record_count: Option<i64>,
    /// `acceptDate` —— 领取那一口要回带的日期串（来自**同一条**响应）。
    ///
    /// ⚠ 实拍那两条**都没有这一格** ⇒ 读到 `None` 是**缺省路径**，不是异常。
    /// `None` 时领取那一口发**裸 GET**，与真实客户端一致。
    ///
    /// 空串在这里一律收成 `None`：`?acceptDate=` 与不带参数是**两个不同的请求**，
    /// 而"上游给了一个空串"更接近"没给"。
    pub accept_date: Option<String>,
}

/// 解析 `taskExpansion` 的响应。
///
/// ⚠ **没有 `result` 不算失败**（理由见 [`TaskExpansion`] 那段"为什么六格全是指针"）。
/// 失败只有一种：信封不 OK（`90001` 那条"票的问题"在 [`SignInEnvelope`] 里已翻成人话）。
///
/// # 布尔与数字只认**自己的类型**
///
/// 布尔走 `Value::as_bool()`（`1` / `0` 得到 `None`）、数字走 `Value::as_i64()`。
/// 一律**不**"顺手兼容"字符串或数字形态 —— 这一格的门后面是一次**写用户网盘**。
pub fn parse_task_expansion(body: &str) -> Result<TaskExpansion> {
    let env = SignInEnvelope::parse(body)?;
    env.ensure_ok("读取任务拓展")?;
    let Some(Value::Object(obj)) = env.result.as_ref() else {
        // 没有 result（或它不是对象）⇒ 交回一份"什么都没读到"，**不报错**。
        return Ok(TaskExpansion::default());
    };
    Ok(TaskExpansion {
        cur_month_backup: obj.get("curMonthBackup").and_then(Value::as_bool),
        pre_month_backup: obj.get("preMonthBackup").and_then(Value::as_bool),
        cur_month_backup_task_accept: obj.get("curMonthBackupTaskAccept").and_then(Value::as_bool),
        cur_month_task_record_count: obj.get("curMonthTaskRecordCount").and_then(Value::as_i64),
        next_month_task_record_count: obj.get("nextMonthTaskRecordCount").and_then(Value::as_i64),
        // 空串收成 None：`?acceptDate=` 与不带参数是两个不同的请求
        accept_date: obj
            .get("acceptDate")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests;
