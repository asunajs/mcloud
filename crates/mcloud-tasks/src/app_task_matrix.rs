//! App 任务矩阵的**数据层** —— 把那张表拉回来、聚合、交给调用方。
//!
//! 16 次只读请求 = 2 个市场名 × 8 个 group。请求与解析在
//! [`mcloud_upstream::app_task_list_v2`]（纯函数、无 IO）。
//!
//! # ⚠ 这里**没有**注册成一条任务（2026-10-07 用户口径）
//!
//! 原话：「**app-task-matrix 这个任务本身就没有意义**」✓。
//!
//! 他说得对：真正干活的那条 `app-task` **本来就要拉这张表**（它得先知道有哪些条目
//! 才谈得上判"该做哪条" ✓，见 `app_task.rs` 第 ① 步），而这里又单独开了一条只读任务
//! ⇒ 同一张表被拉了两遍，用户还得为了"看一眼清单"专门去跑一条什么都不做的任务 ✗。
//!
//! ⇒ 现在这份表跟着 **`app-task` 的明细**一起出（`details.reported.items` ✓），
//! 界面上在**任何一次真跑**的记录里都能看 ✓。本模块只剩数据层：
//! [`fetch_one`] / [`merge_pages`] / [`items_json`] 与那几个常量 ✓。
//!
//! # 这一族里**一个写请求都不发**
//!
//! 上游那一族里另有两口是**写**：
//!
//! * `task/click`（上报"这条我做完了"）—— 会写用户的账，没有撤销那一口。
//! * 上传那三枪（`file/create` → `PUT` → `file/complete`）—— 真的往用户网盘写
//!   `asign-*.png`，而**它们与收尾那一枪 `batchTrash` 都零实拍**。
//!
//! 本模块只做"把表拉回来看"（写的那一半全在 `app_task.rs` ✓）。
//!
//! # 失败策略：**一次失败不算失败，全挂才算**
//!
//! 某一组挂了只**登记**它、继续拉下一组（照旧实现）；十六次**全**挂才交回错误。
//! ⚠ 这**不是**参考实现的宽容档 —— 它刻意与旧 TS 不同：旧 TS 十六次全失败仍回一张**空表**，
//! 而那会让"全没读到"看起来像"这个号没任务"。

use serde_json::Value;

use mcloud_core::{Error, HttpClient, HttpRequest, Result};
use mcloud_engine::TaskContext;
use mcloud_upstream::app_task_list_v2 as proto;
use mcloud_upstream::app_task_list_v2::{TaskListV2, TaskV2Item};

/// 这一口要拉的两个市场名（**顺序是行为的一部分**：先云盘、后邮箱）。
pub const APP_TASK_MARKETS: [&str; 2] = ["sign_in_3", proto::TASK_LIST_V2_EMAIL_MARKET];

/// 这一趟要发几次请求 = 市场数 × group 数。
/// 其中 **V2 那 16 发**（2 市场 × 8 组 ✓）—— 想数"整表拉了几趟"就用它 ✓
///（`app_task_matrix_fetches()` 现在含 2 发 V3 ✓，直接比会差 2 ✓）。
pub fn app_task_matrix_v2_fetches() -> usize {
    APP_TASK_MARKETS.len() * proto::TASK_LIST_V2_GROUPS.len()
}

pub fn app_task_matrix_fetches() -> usize {
    // 16 发 V2（2 市场 × 8 组 ✓）+ **2 发 V3**（每个市场一发、空 `group` 一次拿全 ✓）。
    // ⚠ 加上 V3 之后这一口**不再是"只读 16 发"** ✗ —— 界面上那句"拉到 N/N 次"也跟着变 ✓。
    APP_TASK_MARKETS.len() * proto::TASK_LIST_V2_GROUPS.len() + APP_TASK_MARKETS.len()
}

/// 一次拉取的失败记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchFailure {
    pub market_name: String,
    pub group: String,
    pub error: String,
}

/// 聚合后的那张表。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppTaskMatrix {
    /// 聚合 + 排序 + 去重之后的条目。
    pub items: Vec<TaskV2Item>,
    /// **去重之前**的条目数（十六次拉取加起来的全部条目）。
    ///
    /// 它只为播报那一行存在（旧实现那句「获取到 32 个任务，去重后 24 个」）。
    pub raw_count: usize,
    /// 实际成功的请求数。
    pub succeeded: usize,
    /// 失败的组（登记，不中断）。
    pub failures: Vec<FetchFailure>,
    /// 每个 (market, group) 的条目数与组完成度读数。
    ///
    /// ⚠ 组完成度**只存不判**：它在实拍里一次是空、一次是 `2`（而那一组三条全是 `FINISH`）
    /// ⇒ 不是"完成/未完成"那种二值口径。
    pub groups: Vec<GroupReading>,
}

/// 一个 (market, group) 的读数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupReading {
    pub market_name: String,
    pub group: String,
    /// 这一组回来的条目数（**去重之前**）。
    pub count: usize,
    /// 这一组的 `groupState.<group>.taskFinishState`（**只存不判**）。
    pub finish_state: Option<i64>,
}

impl AppTaskMatrix {
    /// `state` 的分布：已完成 / 未完成 / 读不出。
    ///
    /// ⚠ "读不出"**单独一档**，**不许**折进"未完成" ——
    /// 折进去的表现是将来有人接写链时，一条读不出状态的任务会被当成"还没做"而**重复上报**。
    pub fn state_counts(&self) -> (usize, usize, usize) {
        let (mut done, mut waiting, mut unknown) = (0, 0, 0);
        for item in &self.items {
            match item.state.as_deref() {
                Some(STATE_DONE) => done += 1,
                Some(STATE_WAITING) => waiting += 1,
                _ => unknown += 1,
            }
        }
        (done, waiting, unknown)
    }

    /// `enable != 1` 的条目数（旧实现的门：不等于 1 就跳过）。
    ///
    /// ⚠ `None`（上游没给这一格）**算进来**：它既不是 1，也就过不了那道门。
    /// 本链只播报、不用它做任何决定。
    pub fn disabled_count(&self) -> usize {
        self.items.iter().filter(|t| t.enable != Some(1)).count()
    }
}

/// ⚠ `state` 的两个字面值。**不**引用拍拍那一族那份常量（那个在
/// `mcloud_upstream::app_play_ai`）：两族字面相同，但各自的取值集各自只由各自的样本支撑。
pub(crate) const STATE_DONE: &str = "FINISH";
const STATE_WAITING: &str = "WAIT";

/// 把十六份响应合成一张表。
///
/// # 三步，逐字照旧实现（`tasks.ts:12-14`）
///
/// 1. 按 `id` **升序**排（`id` 缺席的排最后）；
/// 2. 按 `name` 去重，**保留先来的那条**；
/// 3. `name` **缺席**的条目**各自保留**（旧 TS 会把它们全去重成一条 —— 那是错的：
///    "上游没给名字"与"两条同名"是两回事，合并会把条目数报少）。
///
/// `sort_by_key` 是**稳定排序**，所以 `id` 相同（含都缺席）的条目保持原顺序。
pub fn merge_pages(pages: &[TaskListV2]) -> (Vec<TaskV2Item>, usize) {
    let mut all: Vec<TaskV2Item> = pages.iter().flat_map(|p| p.items.iter().cloned()).collect();
    let raw = all.len();
    // 第 1 步：按 id 升序；缺席的给 i64::MAX（排最后，且彼此保持原顺序）
    all.sort_by_key(|t| t.task_id.unwrap_or(i64::MAX));
    // 第 2、3 步：按 name 去重，name 缺席的各自保留
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<TaskV2Item> = Vec::with_capacity(all.len());
    for item in all {
        // 先取出名字（`String`），别在移动 `item` 的同时还借着它
        let Some(name) = item.name.clone() else {
            // 名字缺席 ⇒ 各自保留
            out.push(item);
            continue;
        };
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        out.push(item);
    }
    (out, raw)
}

// ══════════════════════════ 拉取 ══════════════════════════

pub async fn fetch_one(http: &dyn HttpClient, market: &str, group: &str) -> Result<TaskListV2> {
    let req: HttpRequest = proto::task_list_v2_request(market, group);
    let resp = http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "读取 App 任务矩阵")?;
    proto::parse_task_list_v2(body, group)
}

/// 把这张表转成明细 JSON。
///
/// ⚠ 字段名是**对外契约**（界面上那份清单就按它画 ✓）⇒ 两处调用（`app-task` 的明细、
/// 以前那条只读任务）必须**同一份**，别各写一遍 ✗。
pub fn items_json(items: &[TaskV2Item]) -> Vec<Value> {
    items
        .iter()
        .map(|t| {
            serde_json::json!({
                "taskId": t.task_id,
                "name": t.name,
                "description": t.description,
                "groupId": t.group_id,
                "state": t.state,
                "limitType": t.limit_type,
                "enable": t.enable,
                "currStep": t.curr_step,
                "process": t.process,
                "marketName": t.market_name,
                // ⚠ 只读这一格是为了**判据可复核**：玩法 615 那一族的**门**就在
                // `button.app.link` 里（`beiyong1_button_gate`）⇒ 不报出来就没法在界面上
                // 核"它到底该不该传 10 张" ✓（2026-10-04 加，接 615 时 ✓）。
                "buttonAppLink": t.button_app_link,
                // 上游自己给这条任务、这个状态写的那句话（`buttonProperty.out.text` ✓）——
                // 界面照抄它，读不出才回落状态词（2026-10-11 用户口径 ✓）。
                // 条目上那句奖励（`content_display2` ✓，实拍 550 是 `"+200豆"` ✓）。
                "rewardText": t.reward_text,
                "buttonOutText": t.button_out_text,
                // 进度与"能不能领"（`button.out.day` / `canReceive` ✓）—— 界面此前只读
                // `process`，于是"App 里有进度、这里没有" ✗（2026-10-11 用户口径 ✓）。
                "buttonOutDay": t.button_out_day,
                // 上游**直接给**的分母（V3 那一口 ✓，实拍 551 是 `31` ✓）。
                "buttonOutTotal": t.button_out_total,
                // 上游那格是**三档**文案（`text` / `textMiddle` / `textFinish` ✓）——
                // 界面按阶段挑，别只读第一档 ✗（2026-10-11 用户口径 ✓）。
                "buttonTextMiddle": t.button_text_middle,
                "buttonTextFinish": t.button_text_finish,
                "buttonActionMiddle": t.button_action_middle,
                "buttonCanReceive": t.button_can_receive,
            })
        })
        .collect()
}

/// 这张表的状态分布：`(已完成, 未完成, 读不出)`。
///
/// ⚠ 与 [`AppTaskMatrix::state_counts`] 同一份口径 —— 界面上的"未完成 N"就是它 ✓。
pub fn state_counts_json(items: &[TaskV2Item]) -> Value {
    let mut done = 0usize;
    let mut waiting = 0usize;
    let mut unknown = 0usize;
    for t in items {
        match t.state.as_deref() {
            Some(STATE_DONE) => done += 1,
            Some(_) => waiting += 1,
            None => unknown += 1,
        }
    }
    serde_json::json!({"done": done, "waiting": waiting, "unknown": unknown})
}

/// 拉整张表：2 个市场 × 8 个 group，逐次容错。
///
/// 返回 `(拉到的页, 每组的失败说明)` ✓。
///
/// ⚠ **这份循环只此一处**（2026-10-07 抽出来）：同一段"两个市场 × 八个组"原先在
/// `app_task.rs` 里抄了三遍（开头那一趟、收尾复查那一趟、只读任务那一趟）——
/// 改一处忘两处是迟早的事 ✗。
///
/// ⚠ `cancelled` 是**逐次**问的：这一趟要发 16 枪，全发完才认取消会让"中止"按钮
/// 看起来没反应 ✗。服务器那条按需取数的路**没有**取消这个概念 ⇒ 传 `|| false` ✓。
///
/// ⚠ 某一组失败**不算失败**，只登记（照旧实现 ✓）；"十六组全挂算不算错"由**调用方**
/// 判 —— 那取决于它拿这张表干什么 ✓（任务侧只是少几页证据；页面那边是"读不到"，
/// 必须回一个错误码 ✗ 而不是一张空表）。
pub async fn fetch_all_with(
    http: &dyn HttpClient,
    cancelled: impl Fn() -> bool,
) -> Result<(Vec<TaskListV2>, Vec<String>)> {
    let mut pages: Vec<TaskListV2> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for market in APP_TASK_MARKETS {
        for group in proto::TASK_LIST_V2_GROUPS {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            match fetch_one(http, market, group).await {
                Ok(p) => pages.push(p),
                Err(e) => failures.push(format!("读取 {market}/{group}：{e}")),
            }
        }
    }
    // ⚠⚠ **再拉 V3 那一口**（每个市场一发、空 body ✓）—— 它给的字段 V2 **没有** ✗：
    //   `day` / `total`（App 那个「完成进度 (1/31)」✓）、以及**当前阶段**那一档 `text`
    //   （V3 的 551 是「开启中」✓，V2 同一条是「去开启」+`textMiddle`「开启中」✗）。
    //   ⇒ 这不是"换个版本号"，是**两个口各给一半** ✓（实拍见 `app_task_list_v2` 的
    //   `V3Overlay` 注释 ✓）。best-effort：V3 挂了不挡 V2 那份 ✓。
    for market in APP_TASK_MARKETS {
        match fetch_v3(http, market).await {
            Ok(map) => overlay_v3(&mut pages, &map),
            Err(e) => failures.push(format!("读取 {market}/taskListV3：{e}")),
        }
    }
    Ok((pages, failures))
}

/// 拉一个市场的 `taskListV3` 并摊成 `id → overlay` ✓。
async fn fetch_v3(
    http: &dyn HttpClient,
    market: &str,
) -> Result<std::collections::HashMap<i64, proto::V3Overlay>> {
    let resp = http.request(proto::task_list_v3_request(market)).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "读取 App 任务矩阵（V3）")?;
    proto::parse_v3_overlay(body)
}

/// 把 V3 的字段并进 V2 的条目 ✓（**V3 有值就覆盖** ✓ —— 它就是 App 显示的那一份 ✓）。
///
/// ⚠ 按 `id` 对 ✓（不是按名字 ✗ —— 名字里有 HTML、还可能重名 ✓）。
/// ⚠ V2 里没有的 id **不新建** ✗：主条目以 V2 那份为准 ✓，V3 只补它缺的那几格 ✓。
pub fn overlay_v3(
    pages: &mut [TaskListV2],
    map: &std::collections::HashMap<i64, proto::V3Overlay>,
) {
    for page in pages.iter_mut() {
        for item in page.items.iter_mut() {
            let Some(id) = item.task_id else { continue };
            let Some(v) = map.get(&id) else { continue };
            if v.text.is_some() {
                item.button_out_text = v.text.clone();
            }
            if v.text_middle.is_some() {
                item.button_text_middle = v.text_middle.clone();
            }
            if v.text_finish.is_some() {
                item.button_text_finish = v.text_finish.clone();
            }
            if v.action_middle.is_some() {
                item.button_action_middle = v.action_middle.clone();
            }
            if v.day.is_some() {
                item.button_out_day = v.day.clone();
            }
            if v.total.is_some() {
                item.button_out_total = v.total;
            }
            if v.can_receive.is_some() {
                item.button_can_receive = v.can_receive;
            }
        }
    }
}

/// 任务侧那一份：把 [`TaskContext`] 的取消信号接进去 ✓。
pub(crate) async fn fetch_all(ctx: &TaskContext) -> Result<(Vec<TaskListV2>, Vec<String>)> {
    fetch_all_with(ctx.http.as_ref(), || ctx.check_cancelled().is_err()).await
}

#[cfg(test)]
mod tests;
