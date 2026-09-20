//! 移动云盘 **App 端签到（云朵中心）** —— H5 链路的上游接口层。
//!
//! # 规格来源
//!
//! `docs/capture/app-signin-samples.txt`（2026-09-19 实测抓包 + 账号 B 纯协议端到端复现）。
//! 黄金向量：`test/golden/app-signin-vectors.json`。
//!
//! # 为什么这一层看起来"不像一个 App 接口"
//!
//! 因为**签到根本不是一个原生接口，是一个 H5 页面**（`newsignin`）。
//! 原生 App 只做两件事：换一个 SSO token、把 WebView 打开到 `m.mcloud.139.com`。
//! 真正的签到动作全由 H5 发出 —— 所以本模块要同时说两种"客户端语言"：
//!
//! ```text
//! 原生（A 族头，Authorization + 设备头）        H5（自有头，jwttoken）
//! ───────────────────────────────────────────   ─────────────────────────────────────
//! ① POST /user/querySpecTokenV2  ──→ SSO token
//! ② GET  /ycloud/api/timestamp   ──→ 服务端毫秒时间戳（签名要用）
//! ③ POST /ycloud/auth-service/auth/tyrzLogin ──→ jwtToken（≈905 字符，2 小时）
//! ④ GET  /ycloud/signin/page/infoV3?client=app        ← 签到状态，**权威判据**
//! ⑤ GET  /ycloud/signin/page/startSignIn?client=app   ← 执行签到
//! ⑥ POST /ycloud/signin/page/receiveV3                ← 领云朵（**必须带 deviceId**）
//! ⑦ POST /ycloud/signin/page/receiveRevivalReward     ← 用复活卡/回馈卡（**不带 deviceId**）
//! ```
//!
//! ①②③ 是"建会话"，④⑤⑥⑦ 是"签到 + 领取"。**没有 ③ 就没有 ④⑤⑥⑦** ——
//! 少了 `jwttoken` 头，它们一律回 `90001 未登录`。
//!
//! # 三条容易踩的坑（都实测过）
//!
//! 1. **签名不是 B 族那套。** 这里既不是 `signature`（SHA256 + `clientKey`）
//!    也不是 `encodeData`，而是 `md5(盐 + request-id + 时间戳 + nonce + 空串 + 盐)`，
//!    盐是 `sekaMdYYLIZfbCfm` —— 与 PC 端签名**同一个盐**，所以直接复用
//!    [`mcloud_core::generate_signature`]（`salt_key = 2`）。见 [`sign_h5_request`]。
//! 2. **`x-timestamp` 必须取自服务端**（`/ycloud/api/timestamp`），不能用本地时间；
//!    签名里用的是同一个值，两者对不上就是 `401 请求签名校验失败`。
//! 3. **`todaySignIn` 不是成败判据。** 实测 `startSignIn` 的响应是**调用前**的快照，
//!    `todaySignIn:false` 恰恰表示"这次把今天签上了"。判成败一律回查 [`parse_sign_in_status`]。
//!
//! # `deviceId`：**逐接口**判断，绝不按"签到链路"整体外推
//!
//! 签到页里有个 `deviceId`，值是 `"B"` + 88 字符 base64。它**不是"这一族接口都要"**：
//! H5 是**逐接口**用 `isDeviceId: true` 声明的，实测结果与声明一致。
//!
//! ## 它的来源：**就是 [`crate::smid`]，不需要浏览器**
//!
//! 页面里的 `window.SMSdk` 是 `fp.min.js`（数美设备指纹）**自己定义**的
//! （所以 App 二进制里搜不到这个名字），`getDeviceId()` 返回
//! `'B' + localStorage['SMID']`；而 `SMID` 正是它往
//! `slw.h5cmpassport.com:9090/deviceprofile/v4` 上报特征后拿回的 `detail.deviceId`。
//!
//! 那套配置（organization / appId / RSA 公钥 / host）写在签到页 HTML 的
//! `window._smConf` 里，与本项目 [`crate::smid`] 的常量**逐字相同** ——
//! 两边是同一个客户端，所以纯 Rust 上报就能拿到同一个 id。
//! 守卫：`smid::tests::the_signin_page_fingerprint_sdk_is_this_same_client`。
//!
//! ⇒ 曾经那条"先用官方 App 打开一次签到页、再从 WebView 的 `localStorage` 里读"
//! 的路子**已废弃**；`scripts/h5-device-id.py` 只留作取证工具。
//!
//! ## 各接口的签名
//!
//! - [`start_sign_in_request`] 收 `Option<&str>` —— 允许省略（实测不带也能签上），
//!   但调用点必须写出来；
//! - [`do_task_post_request`] 与 [`receive_v3_request`] 收 `&str` —— **必填**，
//!   让"漏发"变成编译期错误，而不是运行时的上游报错（`receiveV3` 不带实测回 `615`）；
//! - [`receive_revival_reward_request`] 收 `Option<&str>` —— H5 定义里**没有**
//!   `isDeviceId`，所以缺省就是不发；给了才补一个 `deviceid` **头**（老抓包如此）。
//!   两个分支都有证据，取哪个都行，见该函数的说明。
//!
//! ⚠ 同一个值在三个接口上落点**各不相同**：`startSignIn` 在 **query**、
//! `receiveV3` 在 **body**、`receiveRevivalReward` 在 **头**。别照着前一个抄。

// ─────────────────────────────────────────────────────────────────────────────
// 常量（全部来自抓包，不是猜的）
// ─────────────────────────────────────────────────────────────────────────────

/// 签到活动的市场名。它同时是请求头 `activityid` 的取值。
pub const MARKET_NAME: &str = "sign_in_3";

/// 签到页的来源 id（URL 的 `sourceid` / `tyrzLogin` 体的 `sourceId`）。
pub const SOURCE_ID: &str = "1002";

/// H5 页面的 `appID`（页面 URL 上那个固定串，不是客户端 id）。
pub const H5_APP_ID: &str = "cy62d5a5e8373a272aeca467accad61722c51cfd48";

/// H5 请求头 `appversion`。
///
/// 注意它与 [`crate::app_client::ios::APP_VERSION`]（`12.6.0`）**不是一个值**：
/// 这里带第 4 段（`12.6.0.0`）。
pub const H5_APP_VERSION: &str = "12.6.0.0";

/// `taskListV3` 请求体里的 `clientVersion`（不带第 4 段）。
pub const TASK_CLIENT_VERSION: &str = "12.6.0";

/// `startSignIn` / `infoV3` 的查询参数 `client`。
///
/// 抓包实测是字面量 `app`（**不是数字**）—— 别照 `taskListV3` 的 `client:1` 抄。
pub const CLIENT: &str = "app";

/// `querySpecTokenV2` 请求体里 `toSourceId` 的取值（**本功能选的**，不是"App 端的"）。
///
/// ⚠ **它不是端常量，是调用方按场景选的参数。** 同一个 App 进程里就出现过三个值
/// （`001003` / `001005` / `010001`），按发起方不同而变。本模块取 `001005`，
/// 依据是**签到页 URL 自带的 `targetSourceId=001005`**（App 实拍，见
/// `docs/capture/app-signin-samples.txt` §2.1）。
///
/// 别拿别处的值来换它：
/// - `apiServer.js` 里 `getCloudTicket` 的默认值是 `001003` —— 那是 **PC 客户端**的包；
/// - 签到会话里那条 **WebKit** 发起的 `querySpecTokenV2` 也确实用了 `001003`，
///   但发起方不是签到页本身（同一条接口，原生与 H5 两个发起方的头、参数都不同）。
///
/// "两边实测都能过"只说明**服务端在这条路径上不校验**，不等于可以互换 ——
/// 上游不校验的字段一旦哪天开始校验，错的那个是**静默失效**，不会报错。
/// 跨端（PC ↔ App）、跨发起方（原生 ↔ H5）的常量一律不互搬。
///
/// 反过来说，**同一个文件里出现多个值也不矛盾**：PC 客户端 `apiServer.js` 里
/// 同一条接口三处调用分别传 `001003`（默认）/ `001005` / `001248`。
/// 完整的取值谱、分类判据与"哪些码会整套换头"见 `docs/device-and-source-codes.md`。
pub const TO_SOURCE_ID: &str = "001005";

/// H5 签名用的盐值编号 —— [`mcloud_core::sign_salt`] 里的 `sekaMdYYLIZfbCfm`。
///
/// 单列成常量而不是在调用点写 `2`：`SIGN_SALT_MAP` 哪天被改动，
/// 这里会有一条测试直接失败（见 `h5_salt_is_the_pc_signing_salt`），
/// 而不是变成"签到忽然回 401"。
pub const H5_SIGN_SALT_KEY: u8 = 2;

/// 业务成功码。
pub const CODE_OK: i64 = 0;

/// `90001 未登录` —— `jwttoken` 没带上，或会话没建起来。
pub const CODE_NOT_LOGGED_IN: i64 = 90001;

/// `200050409 token 已失效` —— SSO token 过期。
///
/// 它**很短命**：实测 14:38:54 换到的 token，14:42:09 重放就被拒。
/// 换完立刻用，不要缓存。
pub const CODE_SSO_EXPIRED: i64 = 200050409;

/// `401 请求签名校验失败` —— 签名算式/盐/时间戳任一处不对。
pub const CODE_SIGNATURE_REJECTED: i64 = 401;

/// `11029` —— 上游"活动太火爆"。H5 收到它就 toast「活动太火爆了，请稍后再试」。
///
/// 它**不是**"参数错了"：请求被正常处理了，只是这次没给。可以退避后重试。
pub const CODE_RECEIVE_TOO_BUSY: i64 = 11029;

/// `614` / `615` —— H5 把它们归为同一句「领取失败，请稍后再试」，**但它们是两件事**。
///
/// # 两个码的语义都已实证（2026-09-19，账号 B）
///
/// 同一份 `receiveV3` 请求（`client=app`、`cloudId=""`、`cloudType=2`），只换 `deviceId`：
///
/// | `deviceId` | `code` | 上游 msg |
/// | --- | --- | --- |
/// | 不带 | **`615`** | 活动太火爆啦，**兑换失败**，试试其他奖品吧 |
/// | 长度对、内容错一个字符 | **`614`** | 活动太火爆啦，**锁定失败**，试试其他奖品吧 |
/// | 真实值 | `0` | （`result.receive` 正常返回） |
///
/// ⇒ **`615` = 没带 `deviceId`；`614` = `deviceId` 内容无效。**
/// 两个码的 msg 里那个动词（"兑换" vs "锁定"）就是唯一的区别线索。
///
/// ⚠ **上游的 msg 会把人带偏**：它开口就是"活动太火爆"，读起来像限流、像该退避重试，
/// 而真实原因是**参数缺失或参数无效** —— 重试一万次也没用。
/// 所以别照着 msg 做退避逻辑，先查 `deviceId`。
///
/// ⚠ 这同时说明**服务端校验的是 `deviceId` 的内容，不只是"非空"**：
/// 一个长度合法、只错一个字符的 base64 照样被拒（拿到 `614`）。
/// 所以"随便给个非空值"这条路是堵死的 —— 必须拿到真值。
///
/// 真值来自 iOS `SMSdk.getDeviceId()`，**89 字符 base64 → 65 字节**
/// （实测解码后正好 65 字节，与 `docs/capture/app-signin-samples.txt` §4.1
/// 记的 `1 + 16 + 48` 吻合）。它与 [`crate::smid`] 产出的 `B<uuid>` **不是同一个东西**。
pub const CODE_RECEIVE_FAILED: i64 = 614;
/// 见 [`CODE_RECEIVE_FAILED`]。
pub const CODE_RECEIVE_FAILED_ALT: i64 = 615;

/// 已知业务码 → 一句话排查提示。返回 `None` 表示"没有已知提示"。
pub const fn diagnose(code: i64) -> Option<&'static str> {
    match code {
        CODE_NOT_LOGGED_IN => Some("jwttoken 没带上，或 tyrzLogin 还没跑（会话没建起来）"),
        CODE_SSO_EXPIRED => Some("SSO token 已失效 —— 它很短命，换完必须立刻用"),
        CODE_SIGNATURE_REJECTED => {
            Some("签名不对：盐 / 算式 / x-timestamp（必须取自 /ycloud/api/timestamp）")
        }
        CODE_RECEIVE_TOO_BUSY => Some("上游「活动太火爆」—— 不是参数错，退避后重试"),
        // 614 / 615 的 msg 与 11029 长得几乎一样（都在说"火爆"），但成因完全不同：
        // 前两者是**参数问题**（重试无用），后者才是真的忙。别按 msg 归类。
        CODE_RECEIVE_FAILED_ALT => Some(
            "领取失败 —— 实测这个码就是「没带 H5 的 deviceId」；上游 msg 说的「兑换失败」是误导",
        ),
        CODE_RECEIVE_FAILED => Some(
            "领取失败 —— 实测这个码就是「H5 的 deviceId 内容无效」；上游 msg 说的「锁定失败」是误导",
        ),
        _ => None,
    }
}

mod backup_gift;
mod common;
mod prize;
mod receive;
mod records;
mod revival;
mod send;
mod session;
mod sign_in;
mod status;

pub use backup_gift::{
    BackupGift, BackupGiftReceive, backup_gift_info_request, backup_gift_receive_request,
    parse_backup_gift, parse_backup_gift_receive,
};
pub(crate) use common::{ensure_ok, percent_encode, require_i64, require_str, with_h5_headers};
pub use prize::{
    PRIZE_MARKET_NAME, PRIZE_STATUS_EXPIRED, PRIZE_STATUS_PENDING, PRIZE_STATUS_RECEIVED,
    PrizeRecordItem, PrizeRecords, parse_prize_records, prize_record_request,
};
pub use receive::{ReceiveResult, parse_receive_v3, receive_v3_request};
pub use records::{
    CloudRecords, RECORD_BOOK_BEAN, RECORD_BOOK_CLOUD, RECORD_FILTER_ALL, RECORD_FILTER_EARNED,
    RECORD_FILTER_SPENT, cloud_record_v2_request, parse_cloud_records,
};
pub use revival::{
    MultipleResult, ReceiveTaskExpansion, RevivalReward, multiple_request, parse_multiple,
    parse_receive_task_expansion, parse_revival_reward, receive_revival_reward_request,
    receive_task_expansion_request,
};
pub use session::{
    H5Login, H5Signature, h5_timestamp_request, parse_jwt_token, parse_server_timestamp,
    parse_sso_token, query_spec_token_body, query_spec_token_request, sign_h5_request,
    sign_h5_request_with, signin_page_url, tyrz_login_body, tyrz_login_request,
};
pub use sign_in::{
    SignInResult, do_task_post_request, info_v3_request, parse_start_sign_in, parse_task_list,
    start_sign_in_request, task_list_v3_request,
};
pub use status::{ReceiveItem, SignInDay, SignInStatus, parse_sign_in_status};

#[cfg(test)]
mod tests;
