//! 消息推送族（App「开启APP通知领好礼」页 = `msgPushOn/*`）—— **协议层**。
//!
//! 查状态 + 领第二次奖励。判定（什么时候该领）在 `mcloud-tasks::msg_push`。
//!
//! # 证据分两半，别混着读
//!
//! ## 有实拍的一半
//!
//! 2026-09-25 那一次进页抓包（取证面是 App 内 WebView，不是 App 本体）逐字证实了：
//!
//! ```text
//! GET  https://m.mcloud.139.com/ycloud/msgPushOn/task/status     ← 连 `?` 都没有
//!      → {"code":0,"msg":"success","result":{"pushOn":1,"firstTaskStatus":3,
//!         "secondTaskStatus":1,"pushTaskStatus":0,"onDuaration":16,"total":31}}
//! POST https://m.mcloud.139.com/ycloud/msgPushOn/task/list       ← body 是字面量 `null`
//!      → {"code":0,"msg":"success","result":[ 五笔 ]}
//! ```
//!
//! ⚠ 实拍把这一族的**入口换掉了**：旧描述记的是
//! `caiyun.feixin.10086.cn/market/msgPushOn/task/…`，而实拍这页走
//! `m.mcloud.139.com/ycloud/msgPushOn/task/…`。
//!
//! **但不要把它读成"旧那条是错的"** —— 同一天另一次真跑正是从 `/market/…`
//! 那一支拿到过可解析的信封（三个读数都有值）。两个入口的关系（同一个网关的两个
//! 门？还是一个转发？）**未测试**。选 `/ycloud/` 的理由是"本仓唯一看过原文的那一页
//! 走的是它"，不是"另一条打不通"。
//!
//! ## 零样本的一半
//!
//! * **`task/obtain` 整条**：实拍那天 `secondTaskStatus = 1`（进度 16/31 天），
//!   页面上根本没有领取入口 ⇒ 这一枪**结构性拍不到**。所以它的**方法**（GET 还是 POST）、
//!   非 0 时的例文，两格全空。唯一有实拍支持的是**路径前缀**，最后那段 `/obtain`
//!   仍只是旧描述。
//! * **`pushOn` 在关闭态下的值**：实拍只见过 `1`。判定层照旧描述写 `0`，
//!   而"关了的时候上游回什么"**未测试**。
//! * **六个读数的语义**：实拍只有一条读数 ⇒ 本文件**不解释**任何字段的含义，
//!   只把读数原样交出去。
//!
//! # 票：这一页有**自己的**取法，但本文件不负责建会话
//!
//! 实拍里这一页自己换过票，参数与签到页**不同**：
//!
//! ```text
//! POST /ycloud/auth-service/auth/tyrzLogin
//!   {"marketName":"msgpushon","openAccount":0,"sourceId":"1062","token":<SSO>}
//!   referer = …/portal/hcyview/index.html?path=openNoticeGift&sourceid=1062&…
//! ```
//!
//! 而 [`SessionRecipe`](crate::session::SessionRecipe) 今天只有签到页那一个取值，
//! 所以这一族**走公共来源**（签到页那张票）。
//!
//! ⇒ 真跑结论都要带着"票来自另一页"这个前提读。
//! ⚠ 那一问**没有答案**：实拍抓到的流量用的就是这一页自己的票，
//! "借签到页的票打这一页的接口行不行"**未测试**。
//!
//! # 头：不是逐字复刻浏览器
//!
//! [`h5`](crate::app_signin::h5) 会多带 `activityid` 与 `appversion` 两个头，
//! 而实拍那两条**没有**（它带的是 `sec-fetch-*` / `cookie` 那一族，是 WebView 自己加的）。
//! 这是全仓库统一的既有选择（一套头只维护一处），不是这一族特例 ——
//! "上游认不认这两个多出来的头"同样**未测试**。

use serde_json::Value;

use mcloud_core::{HttpRequest, Result};

use crate::app_signin::{h5, SignInEnvelope};
use crate::endpoints::MCLOUD_HOST;

/// 这一族的路径前缀 —— **实拍逐字**。
///
/// ⚠ 三段都不能"顺手改"：主机是 [`MCLOUD_HOST`]（**不带端口**），
/// 路径里 `msgPushOn` 是**驼峰**。
pub const MSG_PUSH_TASK_PATH: &str = "/ycloud/msgPushOn/task";

/// 上游**自己拼错**的那个键名（正确拼法应是 `onDuration`）。
///
/// # ⚠⚠ 不许"顺手改对"
///
/// 旧项目记着反序列化**必须按错的键名**收，而 2026-09-25 的实拍**独立证实**了它 ——
/// 响应里就是 `onDuaration`。
///
/// 改成正确拼法的表现是"这个字段永远是 0"，而**不报错**：它只是没读到。
/// 所以这个常量是**唯一的读入口**，测试里用"两个拼法都给"的用例钉住。
pub const MSG_PUSH_DURATION_KEY: &str = "onDuaration";

/// `task/obtain` body 里那个 `type` —— 实拍**零条**，旧项目逐字是 `{type: 2}`。
///
/// 做成常量是因为它同时出现在两处：**发出去的那个数**与**触发发送的判据**
/// （`secondTaskStatus == 2`）。两处必须是同一个数 —— 抄成两个不同的数字，
/// 这条链就**永远不发**，而且不会有任何报错。
pub const MSG_PUSH_OBTAIN_BODY_TYPE: i64 = 2;

// ══════════════════════════ 请求 ══════════════════════════

/// `GET /ycloud/msgPushOn/task/status` —— 查推送任务状态。
///
/// 查询参数**一个都不带** —— 这一条**有实拍**（实拍 URL 里连 `?` 都没有）。
pub fn msg_push_status_request() -> HttpRequest {
    h5(false, HttpRequest::get(status_url()), "true")
}

/// `POST /ycloud/msgPushOn/task/list` —— 领取记录。
///
/// # body 是字面量 `null`（4 字节）
///
/// 实拍逐字如此：`content-length: 4`、`content-type: application/json`（**不带 charset**）。
/// 用 `json_body(&Value::Null)` 得到的正是这四个字节 —— 别"顺手"改成 `{}`
/// 或干脆不带 body。
///
/// ⚠ **`content-type` 要显式写**：客户端在调用方没声明时会补
/// [`CONTENT_TYPE_JSON`](crate::client::CONTENT_TYPE_JSON)，而那是
/// `application/json;charset=UTF-8` —— 实拍这一条**没有 charset**。
/// 两个多半都被接受，但本仓的规矩是"实拍长什么样就发什么样"：
/// 多出来的参数是我们自己加的，不是上游要的。
///
/// ⚠ 这一口**没有接进任务链**：`obtain` 之后列表会不会立刻多一条**零样本**，
/// 拿它去对账等于用一个没人验证过的读数解释另一个。
/// 它进本文件是因为**形状有实拍**，将来要核 `obtain` 时它是唯一能追的那条线。
pub fn msg_push_list_request() -> HttpRequest {
    h5(false, HttpRequest::post(list_url()), "true")
        .header("content-type", MSG_PUSH_LIST_CONTENT_TYPE)
        .json_body(&Value::Null)
}

/// `task/list` 的 `content-type` —— **实拍逐字：不带 charset**。
pub const MSG_PUSH_LIST_CONTENT_TYPE: &str = "application/json";

/// `task/obtain` 用哪个方法 —— **仍未定，A/B 只改这一处**。
///
/// 2026-09-25 那次实拍**没打到这一枪**（`secondTaskStatus = 1` ⇒ 页面上没有领取入口），
/// 所以"GET 还是 POST"**没有被推进**。
///
/// ⚠ "两种方法都发一遍"是**绝对不行**的：那等于向一个真实账号发两次同一个写请求，
/// 而上游是否幂等**零样本**。
/// ⇒ 钉成**一个**常量，缺省 `GET`（`json_body` 会把 body 与 `content-type` 一起带上，
/// 所以翻成 `POST` 时不需要动别的代码）。
///
/// **怎么定**：等一次 `secondTaskStatus == 2` 的窗口（连开满 31 天那一档），
/// 或单独抓一次「点领取」的流量 —— 方法就在 `:method` 那一行。
pub const MSG_PUSH_OBTAIN_IS_POST: bool = false;

/// `…/task/obtain` —— 领第二次奖励。**写请求，且整条零样本**。
///
/// 门槛（只有 `secondTaskStatus == 2` 才该发）由调用方判：
/// 见 `mcloud_tasks::msg_push::plan_msg_push`。
///
/// ⚠ "GET 带 JSON body"本身是个不常见形态（有的网关会把它吞掉）。
/// 要改就改 [`MSG_PUSH_OBTAIN_IS_POST`] 那一处，别在这里"顺手去掉 body"。
pub fn msg_push_obtain_request() -> HttpRequest {
    let body = serde_json::json!({ "type": MSG_PUSH_OBTAIN_BODY_TYPE });
    let req = if MSG_PUSH_OBTAIN_IS_POST {
        HttpRequest::post(obtain_url())
    } else {
        HttpRequest::get(obtain_url())
    };
    h5(false, req.json_body(&body), "true")
}

fn status_url() -> String {
    format!("{MCLOUD_HOST}{MSG_PUSH_TASK_PATH}/status")
}

fn list_url() -> String {
    format!("{MCLOUD_HOST}{MSG_PUSH_TASK_PATH}/list")
}

fn obtain_url() -> String {
    format!("{MCLOUD_HOST}{MSG_PUSH_TASK_PATH}/obtain")
}

// ══════════════════════════ 解析 ══════════════════════════

/// `task/status` 的解析结果。
///
/// # 六个字段**全部**来自实拍，但**仍然全部可选**
///
/// 实拍只有一条读数 ⇒ "上游哪天不给某个键"没有任何样本可依据。
///
/// "上游没给"必须落成判定层的 **`Unknown`** 档（**一个写请求都不发**），
/// 不能落成"给了 0"。尤其 `push_on`：把它缺省成 `0` 等于**凭空造一条
/// "通知已关闭"的待办**，用户会为一个可能根本没坏的东西去翻 App 设置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MsgPushStatus {
    /// `pushOn` —— 实拍 `1`。
    ///
    /// 旧描述：`0` ⇒ 消息通知已关闭（要人去 App 里打开）。
    /// ⚠ **关闭态没有样本**，非 0 有几种取值也没有样本。
    pub push_on: Option<i64>,
    /// `onDuaration`（上游拼错的键名，见 [`MSG_PUSH_DURATION_KEY`]）—— 实拍 `16`。
    ///
    /// 旧描述完全没说它是干什么的，本仓**不解释**，只照抄读数。
    /// ⚠ 页面上那个"已开启 16 天"与这一格**同时**出现，所以"它像天数"
    /// 是一个**观察**，不是结论。
    pub on_duaration: Option<i64>,
    /// `firstTaskStatus` —— 实拍 `3`。
    ///
    /// 旧描述：`!= 3` ⇒ 首次奖励未领（旧项目**只打一句 info、不影响成功**）。
    pub first_task_status: Option<i64>,
    /// `secondTaskStatus` —— 实拍 `1`。
    ///
    /// 旧描述：**只有 `2` 才该发 `obtain`**。
    /// ⚠ `2` 那一档**没有样本**（实拍那次正好是 `1`），所以"`3` 是什么"仍没答案。
    pub second_task_status: Option<i64>,
    /// `total` —— 实拍 `31`。
    ///
    /// **新键**：旧那份描述里**没有**这一格，是实拍读到的。
    /// ⚠ 语义**未验证**（看着像周期总天数），本仓不解释、只照抄。
    pub total: Option<i64>,
    /// `pushTaskStatus` —— 实拍 `0`。**同样是旧描述里没有的新键**。
    ///
    /// ⚠ 语义**未验证**，而且它**不参与任何判定**（判据仍是旧描述那三格）。
    pub push_task_status: Option<i64>,
}

/// 解析 `task/status`。
///
/// # 缺 `result` **不报错** —— 所以这里用 `ensure_ok` 而不是 `ok_result`
///
/// `ok_result` 在缺 `result` 时会报 `响应没有 result`，那是**协议层**的判据。
/// 而这一条链要的是：判据读不出来 ⇒ 落 `Unknown` ⇒ **一个写请求都不发**。
/// 把"上游没给判据"报成"接口故障"，会让这一趟变成一次**失败**，
/// 而它其实什么都没做 —— 那是两句不同的话。
///
/// ⚠ 外层 `code` 非 0 仍然是错（这一枪没打上）：那是 [`ensure_ok`] 管的，
/// 与"判据缺没缺"是两件事。
pub fn parse_msg_push_status(env: &SignInEnvelope) -> Result<MsgPushStatus> {
    env.ensure_ok("查询消息推送任务状态")?;
    let obj = match env.result.as_ref() {
        Some(v @ Value::Object(_)) => v,
        // `result` 缺席、或不是对象 ⇒ 六格全缺，交给判定层落 Unknown。
        _ => return Ok(MsgPushStatus::default()),
    };
    Ok(MsgPushStatus {
        push_on: opt_int_at(obj, "pushOn"),
        on_duaration: opt_int_at(obj, MSG_PUSH_DURATION_KEY),
        first_task_status: opt_int_at(obj, "firstTaskStatus"),
        second_task_status: opt_int_at(obj, "secondTaskStatus"),
        total: opt_int_at(obj, "total"),
        push_task_status: opt_int_at(obj, "pushTaskStatus"),
    })
}

/// `task/obtain` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MsgPushObtain {
    /// `result.obtainCode` —— 旧描述：成功判据是它**等于 0**。
    ///
    /// # ⚠ 为什么是 `Option` 而不是"缺了当 0"
    ///
    /// 缺了**不等于成功**，而这一条已经把写请求发出去了 ——
    /// 把"判不了"伪造成"成功"，用户就再也不会去查这一笔。
    ///
    /// ⚠ 整条 `obtain` 至今**零样本**，所以这一格连"上游会不会用别的名字"
    /// 都没核过。
    pub obtain_code: Option<i64>,
}

/// 解析 `task/obtain`。
///
/// # `code == 0` **不是**这条的成功判据
///
/// 旧描述点名的是 `result.obtainCode == 0` ⇒ 本函数**不**替调用方判成败，
/// 只把读数交出去。
///
/// 但外层 `code` 非 0 仍走 [`SignInEnvelope::ensure_ok`] 报成错误 ——
/// 那一层回答的是"这一枪打没打上"，与"领没领到"是两件事。
pub fn parse_msg_push_obtain(env: &SignInEnvelope) -> Result<MsgPushObtain> {
    env.ensure_ok("领取消息推送奖励")?;
    Ok(MsgPushObtain {
        obtain_code: env
            .result
            .as_ref()
            .and_then(|r| opt_int_at(r, "obtainCode")),
    })
}

/// `task/list` 里的一笔领取记录。
///
/// 实拍五笔除日期外**逐字相同**（`200` / `type:2` / `obtainStatus:1`）。
///
/// ⚠ 只登记**形状**：`obtainStatus` 只见过 `1`、`type` 只见过 `2`
/// ⇒ "`1` = 已领取""`2` = 第二次任务"都是**看着像**，不是结论。
/// 所以这里全部可选、且**没有任何 `is_xxx()` 之类的语义包装**。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MsgPushRecord {
    /// `name` —— 实拍是**字符串** `"200"`（而 `num` 是**数字** `200`，两种类型都实拍到过）。
    pub name: Option<String>,
    pub num: Option<i64>,
    /// ⚠ 字段名是 `type`，是 Rust 关键字 ⇒ 只能叫 `type_`（serde 那边不用管，
    /// 这里不走 derive 反序列化）。
    pub type_: Option<i64>,
    pub obtain_status: Option<i64>,
    pub obtain_time: Option<String>,
}

/// 解析 `task/list`。
pub fn parse_msg_push_list(env: &SignInEnvelope) -> Result<Vec<MsgPushRecord>> {
    env.ensure_ok("查询消息推送领取记录")?;
    let arr = match env.result.as_ref() {
        Some(Value::Array(a)) => a,
        // 不是数组（含缺席）⇒ 没有记录（而不是"解析失败"）：这一口是**只读**
        // 且没有任何判定依赖它，为它判失败只会让一次真跑白跑。
        _ => return Ok(Vec::new()),
    };
    Ok(arr
        .iter()
        .map(|item| MsgPushRecord {
            name: item.get("name").and_then(Value::as_str).map(str::to_string),
            num: opt_int_at(item, "num"),
            type_: opt_int_at(item, "type"),
            obtain_status: opt_int_at(item, "obtainStatus"),
            obtain_time: item
                .get("obtainTime")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
        .collect())
}

// ══════════════════════════ 小工具 ══════════════════════════

/// 取一个**可选**整数：缺键与"给了非数字"都回 `None`。
///
/// 与 `app_signin::int_at` 同一判据，但那个是私有的（`fn int_at`）且
/// 语义不同 —— 它给的是「有没有」的 `Option`，调用方自己决定缺了算不算错。
/// 这里保持同样的"不伪造成 0"。
fn opt_int_at(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        // 上游有的字段是字符串形式的数字
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
