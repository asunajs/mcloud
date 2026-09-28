//! 公告任务族（`notice`，界面那句是「开启通知领AI豆」）—— **协议层**。
//!
//! 查两个槽的状态 + 领这一族的奖励。判定（什么时候该领）在 `mcloud-tasks::notice`。
//!
//! # 证据分两半，别混着读
//!
//! ## 有实拍的一半
//!
//! 两份进页抓包（`mcloud/test/fixtures/http/开启通知领云朵.json` 第 0/3/6/7 条、
//! `完成通知领云朵.json` 第 8 条）加一份 Android WebView 流量
//! （`mobile-events-…json` 第 71/87 条）逐字证实了这一族的五条请求：
//!
//! ```text
//! GET  https://caiyun.feixin.10086.cn:7071/ycloud/signin/task/click?key=task&id=1022
//!      → {"code":0,"msg":"success"}
//! GET  https://caiyun.feixin.10086.cn:7071/ycloud/openemailsms-service/openEmailsms/getTaskInfo
//!      → {"code":0,"msg":"success","result":{
//!            "taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},
//!            "taskTwoInfo":{"cloudStatus":1,"progress":31,"sum":100,"taskStatus":3}}}
//! GET  https://caiyun.feixin.10086.cn:7071/ycloud/openemailsms-service/openEmailsms/reward
//!      → {"code":0,"msg":"success"}                       ← **连 `result` 都没有**
//! GET  https://caiyun.feixin.10086.cn:7071/ycloud/signin/page/receiveTask?taskId=1021
//!      → {"code":0,"msg":"success","result":100}           ← `result` 是**裸数字**
//! GET  https://caiyun.feixin.10086.cn:7071/ycloud/signin/page/reportAppNoticeStatus?status=1
//!      → {"code":0,"msg":"success"}
//! ```
//!
//! ⚠ 主机的**端口**与路径的**两处大小写**都不能"顺手改对"：主机段是
//! `openemailsms-service`（全小写），末段是 `openEmailsms`（**驼峰**），
//! 端口 `:7071` 是这个老域的组成部分。改任何一处的表现都是 `404`，
//! 而 404 看起来像"接口下线了"。
//!
//! ## 零样本的一半
//!
//! * **没有任何一条非 0 码的响应** —— 五条实拍全是 `code:0`。
//! * **没有任何一条"通知未开启"态的响应** ⇒ `mcloud-tasks::notice` 里"需人工"
//!   那一档因此不可达（判据恒 `false`，见那边的 `notice_needs_manual`）。
//! * [`report_app_notice_status_request`] 那一条来自**另一个客户端**
//!   （Android WebView，实拍带 `x-requested-with: cn.cj.pe`），而
//!   `openEmailsms/*` 那几条来自 iOS WebView ⇒
//!   "iOS 那套流程要不要上报这一口"**未验证**。
//! * `day` / `canReceive` 两格：旧 TS 据说是按
//!   `Number(day) > 0 && canReceive === 1` 判要不要上报状态，而这两格
//!   **一条实拍都没有**（连它们在哪个响应里都不确定）⇒ 本层不实现这条判据。
//! * ⚠ `getTaskInfo` 到底**吃不吃查询参数**未验证：它的完整 URL 被脱敏器压成了
//!   `https://caiyun.feixin.10086.cn:<REDACTED>`，`match.key` 里只有路径、没有 `?`
//!   ⇒ 本文件按**不带参数**实现（与旧项目一致）。这是选择，不是读数。
//! * `1021` 与 `1022` **各自是什么任务、为什么差 1 没有成文依据**：两个数分别来自
//!   两份不同的抓包，别把它们读成同一个任务的两个阶段。
//!
//! # ⚠ 三方口径不一致（本层不自行裁决）
//!
//! 旧 TS 记的链路是 `getTaskInfo → reward → taskListV2(clientVersion 11.2.1)
//! 找 id 1021 → reportAppNoticeStatus?status=1`，而**实拍里没有 `taskListV2`
//! 这一环**，且 `reward` 与 `receiveTask?taskId=1021` 分别出现在**两份**抓包里
//! （顺序是"先 `reward`、后 `receiveTask`"）。
//!
//! ⇒ 本层把**两条都实现**，判定按**实拍最强的那一条**（`getTaskInfo` 的四个字段）裁：
//! [`notice_reward_request`] 进链，而 [`receive_task_request`] /
//! [`report_app_notice_status_request`]（以及「开启通知」那一步的
//! [`signin_task_click_request`]）**只实现、不接链** —— 用哪一口入账、要不要上报
//! 由编排层拍板，本层两条都给。
//!
//! # 票：这一族走**公共来源**
//!
//! 五条实拍都带 `jwttoken`，但**换到那张票的链路没被抓到**（抓包里 `referer` /
//! `jwttoken` 全被脱敏，也没有前置的 `tyrzLogin`）⇒ 无法确定这一页特有的取法时
//! 复用既有建会话入口：本族的请求由 `H5Client` 带上**签到页那张票**
//! （`SessionRecipe::SignInPage`）。
//!
//! ⚠ 连带未验证：那张票的 `referer` 指向 `m.mcloud.139.com`，而这里的请求打在
//! `caiyun.feixin.10086.cn:7071` 上 —— 跨主机带 referer 上游认不认，**没有样本**
//! （实拍那条的 referer 被脱敏，看不出它当时带的是哪个）。这是**临时态**：
//! 哪天抓到了这一页自己的换票三枪，就改用特有取法。
//!
//! # 头：这五条实拍都带 `deviceid`，本仓仍不发
//!
//! 拦截器只在 H5 显式标 `isDeviceId` 时注入设备串，而这一族的 H5 调用点我们**没有**
//! （只有 WebView 的流量原文）⇒ 按既有约定**不带** `isdeviceid`、也不带 `deviceid`。
//! 服务端是否真的认这一条 ⇒ **未验证**（怎么测：拿一条不带 `deviceid` 的请求打
//! `getTaskInfo`，看是不是照样回 `code:0`）。

use serde_json::Value;

use mcloud_core::{HttpRequest, Result};

use crate::app_signin::{h5, SignInEnvelope};

/// 这一族的主机 —— **带端口 `:7071`**，实拍逐字。
///
/// 它与签到族那台 `m.mcloud.139.com` **不是同一台**（老域 + 老端口），
/// 而这一族的请求之所以也能拿到票，是因为 `caiyun.feixin.10086.cn`
/// 在 `endpoints::JWT_HOSTS` 里（判据按**主机名**匹配、不含端口）。
pub const NOTICE_HOST: &str = "https://caiyun.feixin.10086.cn:7071";

/// `openEmailsms/*` 这一族的路径前缀 —— 实拍逐字。
///
/// ⚠ 两处大小写都不能"顺手改对"：主机段 `openemailsms-service` 全小写，
/// 末段 `openEmailsms` 是驼峰（各请求函数自己补）。
pub const NOTICE_TASK_PATH: &str = "/ycloud/openemailsms-service/openEmailsms";

/// `signin/page/*` 那一族的前缀。
///
/// ⚠ 与 [`NOTICE_SIGNIN_TASK_CLICK`] 的 `task/click` **不是一个前缀** ——
/// 同一条链上两种前缀都出现过，照抄实拍，不"统一"。
pub const NOTICE_SIGNIN_PAGE_PATH: &str = "/ycloud/signin/page";

/// 「开启通知」那一步的打卡路径。
///
/// ⚠ 是 `signin/task/click` 而**不是** `signin/page/…`（实拍逐字）。
pub const NOTICE_SIGNIN_TASK_CLICK: &str = "/ycloud/signin/task/click";

/// `task/click` 的 `key` 参数 —— 实拍逐字 `task`。
pub const NOTICE_CLICK_KEY: &str = "task";

/// `reportAppNoticeStatus` 的**唯一实拍**取值（`status=1`）。
///
/// ⚠ 其它取值的语义**未验证** —— 别把"1 = 已开启通知"当结论写进文案，
/// 只能说"上游在这个值上回过 success"。
pub const APP_NOTICE_STATUS_ON: i64 = 1;

// ══════════════════════════ 请求 ══════════════════════════

/// `GET …/openEmailsms/getTaskInfo` —— 查两个槽的状态。**只读**。
///
/// # 查询参数：按"不带"实现，且这是一个未验证的选择
///
/// 见文件头「零样本的一半」那一格：实拍的完整 URL 被脱敏，只有
/// `match.key` 里的路径（没有 `?`）。真要出问题（例如回了另一个活动的状态），
/// 改这一处加参数即可。
pub fn notice_task_info_request() -> HttpRequest {
    h5(false, HttpRequest::get(task_info_url()), "true")
}

/// `GET …/openEmailsms/reward` —— **领公告任务的奖励**。
///
/// ⚠ 这是一次**写**动作（会真的入账），门槛由调用方判：
/// 见 `mcloud_tasks::notice::plan_notice`。
///
/// ⚠ 它**报不出到账数**：实拍响应是 `{"code":0,"msg":"success"}`，连 `result`
/// 都没有 ⇒ "到底领没领上"只能靠调用方**复查一次** [`notice_task_info_request`]
/// （链路的实现见 `mcloud-tasks::notice`）。
pub fn notice_reward_request() -> HttpRequest {
    h5(false, HttpRequest::get(reward_url()), "true")
}

/// `GET …/signin/page/receiveTask?taskId=<n>` —— **只实现、不接链**。
///
/// # 为什么它值得单独存在
///
/// 实拍（`完成通知领云朵.json` 第 8 条，`taskId=1021`）回的是
/// `{"code":0,"msg":"success","result":100}` —— `result` 是**裸数字**，
/// 也就是"**这一口报了到账数**"。而链路里那口 `reward` 什么都不报。
///
/// ⇒ "`receiveTask?taskId=1021` 是本族**唯一留下过到账读数**的入账口"这句话
/// 是有分量的。用哪一口入账交编排层判，本层两条都给 ——
/// 但**今天**编排层选的是 `reward`（理由：它是实拍那条链里紧跟着
/// `getTaskInfo` 的那一枪，而 `receiveTask` 出现在另一份抓包里）。
///
/// ⚠ `task_id` 只见过 `1021` 这一个值，而它与打卡那一步的 `1022` **不是一回事**
/// （见文件头最后一格）。所以这里**没有缺省值**：要发就得把那个数写出来。
pub fn receive_task_request(task_id: i64) -> HttpRequest {
    h5(false, HttpRequest::get(receive_task_url(task_id)), "true")
}

/// `GET …/signin/page/reportAppNoticeStatus?status=<n>` —— **只实现、不接链**。
///
/// # 它来自**另一个客户端**
///
/// 实拍在 `mobile-events-…json`（Android WebView，`x-requested-with: cn.cj.pe`），
/// 与 `openEmailsms/*` 那几条（iOS WebView）**不是同一个客户端** ⇒
/// "iOS 那套流程要不要上报这一口"**未验证**，所以它不接链。
///
/// 旧 TS 那侧的条件是 `Number(day) > 0 && canReceive === 1`，而 `day` /
/// `canReceive` **零样本**（连在哪个响应里都不知道）。
pub fn report_app_notice_status_request(status: i64) -> HttpRequest {
    h5(false, HttpRequest::get(report_status_url(status)), "true")
}

/// `GET …/signin/task/click?key=task&id=<n>` —— **只实现、不接链**。
///
/// 实拍只有 `id=1022` 那一条（`开启通知领云朵.json` 第 0 条，就是「开启通知」
/// 这一步的打卡）。⚠ 它与入账那条的 `taskId=1021` **是两个不同的 id**。
///
/// # 为什么它不接链
///
/// 参考实现的链路里**一枪都不发它**（`ClaimNotice` 只打 `getTaskInfo` 与
/// `reward`）—— 它是"用户去 App 里开启通知导航到这一页"时客户端自己发的
/// 那一步，而本工具代劳不了"去 App 里点那一下"（那一档今天连判据都没有，
/// 见 `mcloud-tasks::notice` 的 `notice_needs_manual`）。
/// 把它留在本层是因为**形状有实拍**，哪天那条链接上了它就是现成的。
pub fn signin_task_click_request(task_id: i64) -> HttpRequest {
    h5(false, HttpRequest::get(click_url(task_id)), "true")
}

fn task_info_url() -> String {
    format!("{NOTICE_HOST}{NOTICE_TASK_PATH}/getTaskInfo")
}

fn reward_url() -> String {
    format!("{NOTICE_HOST}{NOTICE_TASK_PATH}/reward")
}

fn receive_task_url(task_id: i64) -> String {
    format!("{NOTICE_HOST}{NOTICE_SIGNIN_PAGE_PATH}/receiveTask?taskId={task_id}")
}

fn report_status_url(status: i64) -> String {
    format!("{NOTICE_HOST}{NOTICE_SIGNIN_PAGE_PATH}/reportAppNoticeStatus?status={status}")
}

fn click_url(task_id: i64) -> String {
    format!("{NOTICE_HOST}{NOTICE_SIGNIN_TASK_CLICK}?key={NOTICE_CLICK_KEY}&id={task_id}")
}

// ══════════════════════════ 解析 ══════════════════════════

/// `getTaskInfo` 里的一个槽（`taskOneInfo` / `taskTwoInfo`）。
///
/// # 四个字段**全部可选**，而且这不是偷懒
///
/// 实拍只有**两条**读数：
///
/// ```text
/// taskOneInfo{cloudStatus:2, sum:200,             taskStatus:3}
/// taskTwoInfo{cloudStatus:1, sum:100, progress:31, taskStatus:3}
/// ```
///
/// ⇒ 两槽的 `taskStatus` **相同**而 `cloudStatus` **不同**，这本身就说明
/// `cloudStatus` 不是"任务做没做完"，但我们**不知道**它是什么
/// （"这个槽的通知开没开"？"这个槽的奖励领没领"？两种读法都自洽）。
///
/// 所以：① 字段用 `Option`，"上游没给"与"给了 0"必须分得开（判据缺了就什么都不发）；
/// ② 本层**不给** `cloudStatus == 1 / 2`、`taskStatus == 3` 编语义，
/// 只把读数**照抄进结论**（见 `mcloud_tasks::notice` 的 `NoticeResult`）。
///
/// ⚠ `progress` 只在 `taskTwoInfo` 上出现过（`taskOneInfo` **没有**这个键）
/// ⇒ 它是可选的，不是 0。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticeTaskSlot {
    /// `cloudStatus` —— 实拍读数 `1` / `2`，语义**未验证**。
    ///
    /// ⚠ 参考实现另外记了两趟真账号读数（登记成 R1）：四槽读到
    /// `cloudStatus ∈ {0, 2}` 配 `taskStatus ∈ {0, 1, 2, 3}`，
    /// **没有一槽给过 1** ⇒ "哪个值是未领"至今**没有正面样本**。
    /// 而 `0` 既不是 1 也不是 2，语义同样**未验证**。
    pub cloud_status: Option<i64>,
    /// `sum` —— 实拍 `200` / `100`。看着像"这个槽值多少 AI豆"，
    /// ⚠ 但**没有任何证据**说领到手的数等于它：`reward` 的实拍响应里连
    /// `result` 都没有。
    pub sum: Option<i64>,
    /// `progress` ——实拍只在第二个槽上见过 `31`。
    pub progress: Option<i64>,
    /// `taskStatus` —— 实拍两槽都是 `3`。
    pub task_status: Option<i64>,
}

/// `getTaskInfo` 的解析结果。
///
/// 槽本身也是 `Option`：上游**可以整槽不给**（旧 TS 的 `?.` 就是这么防的），
/// 而"没给这一槽"与"给了但四个字段全缺"在判定上是两件事 ——
/// 后者说明"这一槽存在、但读数我们一个都没拿到"。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticeTaskInfo {
    /// `result.taskOneInfo`。
    pub task_one_info: Option<NoticeTaskSlot>,
    /// `result.taskTwoInfo`。
    pub task_two_info: Option<NoticeTaskSlot>,
}

/// 解析 `getTaskInfo`。
///
/// # 缺字段**不报错** —— 所以这里用 `ensure_ok` 而不是 `ok_result`
///
/// `ok_result` 在缺 `result` 时会报 `响应没有 result`，那是**协议层**的判据。
/// 而这一条链要的是：判据读不出来 ⇒ 落"判不了"那一档 ⇒ **一个写请求都不发**。
/// 把"上游没给判据"报成"接口故障"，会让这一趟变成一次**失败**，
/// 而它其实什么都没做 —— 那是两句不同的话。
///
/// ⚠ 外层 `code` 非 0 仍然是错（这一枪没打上）：那是 [`SignInEnvelope::ensure_ok`]
/// 管的，与"判据缺没缺"是两件事。
pub fn parse_notice_task_info(env: &SignInEnvelope) -> Result<NoticeTaskInfo> {
    env.ensure_ok("查询公告任务状态")?;
    let result = env.result.as_ref();
    Ok(NoticeTaskInfo {
        task_one_info: notice_slot_at(result, "taskOneInfo"),
        task_two_info: notice_slot_at(result, "taskTwoInfo"),
    })
}

/// `reward` 的解析结果。
///
/// ⚠ 实拍响应**只有** `{"code":0,"msg":"success"}` —— 连 `result` 都没有，
/// 所以这条**报不出"领到多少"**。到账读数只能靠**复查一次**
/// [`notice_task_info_request`]。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticeReward {
    /// `result` 里的数。**实拍恒 `None`**，做成 `Option` 不是"这里应该有数"，
    /// 而是"哪天上游补上这个字段，别把它丢了"。
    ///
    /// ⚠ 取值口径与 [`ReceiveTask`] 一致（`result` 是**裸数字**时收下来），
    /// 但 `reward` 回裸数字这件事**没有样本**。
    pub cloud_count: Option<i64>,
}

/// 解析 `reward`。
///
/// 没有 `result` **不算失败** —— 实拍就没有，成败判据只有外层 `code`
/// （由 [`SignInEnvelope::ensure_ok`] 判，非 0 才报错）。
pub fn parse_notice_reward(env: &SignInEnvelope) -> Result<NoticeReward> {
    env.ensure_ok("领取公告任务奖励")?;
    Ok(NoticeReward {
        cloud_count: env.result.as_ref().and_then(as_int),
    })
}

/// `receiveTask` 的解析结果。**只实现、不接链**。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReceiveTask {
    /// `result` —— 这次入账的数（实拍 `100`）。
    ///
    /// `None` 表示上游没给数字（**不是 0**）：0 是"进账 0"，`None` 是"没说"。
    pub cloud_count: Option<i64>,
}

/// 解析 `receiveTask`：`result` 是**裸数字**，不是对象。
///
/// ⚠ 别按对象去取字段（`result.cloudCount` 之类）—— 那会让这条**永远报 `None`**
/// 且不报错，与"上游真的没发"看着一模一样。
pub fn parse_receive_task(env: &SignInEnvelope) -> Result<ReceiveTask> {
    env.ensure_ok("领取公告任务奖励")?;
    Ok(ReceiveTask {
        cloud_count: env.result.as_ref().and_then(as_int),
    })
}

/// 解析 `reportAppNoticeStatus`：实拍响应**没有 `result`** ⇒ 只判 `code`。
///
/// 回 `Ok(())` 说的是"这一枪打上了、上游认了"，**不是**"通知已经开了"——
/// 后者这一口答不了（见 [`APP_NOTICE_STATUS_ON`]）。
pub fn parse_report_app_notice_status(env: &SignInEnvelope) -> Result<()> {
    env.ensure_ok("上报 App 通知状态")
}

/// 解析 `signin/task/click`：实拍响应**没有 `result`** ⇒ 只判 `code`。
pub fn parse_signin_task_click(env: &SignInEnvelope) -> Result<()> {
    env.ensure_ok("公告任务打卡")
}

/// 取一个槽对象；缺失、非对象一律 `None`（= "上游没给这一槽"）。
fn notice_slot_at(result: Option<&Value>, key: &str) -> Option<NoticeTaskSlot> {
    let raw = result?.get(key)?;
    if !raw.is_object() {
        return None;
    }
    Some(NoticeTaskSlot {
        cloud_status: opt_int_at(raw, "cloudStatus"),
        sum: opt_int_at(raw, "sum"),
        progress: opt_int_at(raw, "progress"),
        task_status: opt_int_at(raw, "taskStatus"),
    })
}

// ══════════════════════════ 小工具 ══════════════════════════

/// 取一个**可选**整数：缺键与"给了非数字"都回 `None`。
///
/// 与 `app_msg_push::opt_int_at` 同一判据（那边是私有函数，本 crate 里
/// 两个模块各持一份），同样保持"不伪造成 0"。
fn opt_int_at(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(as_int)
}

/// 把一个 `Value` 读成整数：数字与**字符串形式的数字**都收，其余 `None`。
///
/// 上游有的字段是字符串形式的数字（参考实现踩过"严格相等在真实读数上永假"
/// 那一档），所以两种形态都要收 —— 但**不**做"非数字就编个 0"。
fn as_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
