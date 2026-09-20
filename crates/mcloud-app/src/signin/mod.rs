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

// `pub use` 同时也把名字引进本模块 —— 所以上面那行**不要**再列一次它们。
pub use mcloud_upstream::app_signin::{CloudRecords, PrizeRecordItem, PrizeRecords};

// 账本与筛选的取值**从上游层转发出来**，免得调用方（CLI）为了几个整数
// 再去依赖 `mcloud_upstream`。它们本来就是协议常量，转一道不产生新语义。
// （`CloudRecords` / `PrizeRecords` 不在这里再导一次 —— 上面那行已经引进了。）
pub use mcloud_upstream::app_signin::{
    PRIZE_MARKET_NAME, PRIZE_STATUS_EXPIRED, PRIZE_STATUS_PENDING, PRIZE_STATUS_RECEIVED,
    RECORD_BOOK_BEAN, RECORD_BOOK_CLOUD, RECORD_FILTER_ALL, RECORD_FILTER_EARNED,
    RECORD_FILTER_SPENT,
};

mod backup_gift;
mod credential;
mod multiple;
mod query;
mod receive;
mod receive_plan;
mod report;
mod session;
mod sign_in;

pub use backup_gift::{BackupGiftOutcome, BackupGiftPlan, claim_backup_gift, plan_backup_gift};
pub use credential::SignInCredential;
pub use multiple::{MultipleOutcome, MultiplePlan, claim_multiple, plan_multiple};
pub use query::{RecordsQuery, cloud_records, pending_prizes, status, status_raw};
pub use receive::{ReceiveTarget, receive_cloud, receive_revival_reward};
pub use receive_plan::{ReceiveAction, ReceiveOutcome, plan_receive, receive_all};
pub use report::{SignInOptions, SignInReport};
pub use sign_in::sign_in;

#[cfg(test)]
mod tests;
