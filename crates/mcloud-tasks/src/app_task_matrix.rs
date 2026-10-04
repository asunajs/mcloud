//! App 任务矩阵（`app-task-matrix`）—— **只看那一半**：把矩阵表拉回来、聚合、报读数。
//!
//! 16 次只读请求 = 2 个市场名 × 8 个 group。请求与解析在
//! [`mcloud_upstream::app_task_list_v2`]（纯函数、无 IO）。
//!
//! # ⚠ 为什么这条能力的名字**不叫** `app-task`
//!
//! 参考实现里 `app-task` 的标题是「App 任务（**做任务**）」、`Kind = Action`，
//! 含义是**逐条 `task/click` 上报**（会写用户的账、不可撤销），
//! 而且它自己写着**「今天一次真打都没有」**、`RunInBatch = false`，
//! 并指明"手工点一次是拿到第一个实拍的最快路径"。
//!
//! ⇒ 拿同一个 code 实现成只读，会让"`app-task`"这个名字在两边指不同的东西
//! （将来真要做那条写链时还得先改回来）。所以这里另起一个名，
//! 把**写的那一半原样留给 `app-task`**。
//!
//! # 这一条链**一个写请求都不发**
//!
//! 上游那一族里另有两口是**写**：
//!
//! * `task/click`（上报"这条我做完了"）—— 会写用户的账，没有撤销那一口。
//! * 上传那三枪（`file/create` → `PUT` → `file/complete`）—— 真的往用户网盘写
//!   `asign-*.png`，而**它们与收尾那一枪 `batchTrash` 都零实拍**。
//!
//! 所以这一条只做"把表拉回来看"。用户能照着任务名自己去 App 里做 ——
//! 与「拍拍盘点」同一取态。
//!
//! # 失败策略：**一次失败不算失败，全挂才算**
//!
//! 某一组挂了只**登记**它、继续拉下一组（照旧实现）；十六次**全**挂才交回错误。
//! ⚠ 这**不是**参考实现的宽容档 —— 它刻意与旧 TS 不同：旧 TS 十六次全失败仍回一张**空表**，
//! 而那会让"全没读到"看起来像"这个号没任务"。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_task_list_v2 as proto;
use mcloud_upstream::app_task_list_v2::{TaskListV2, TaskV2Item};

/// 这一口要拉的两个市场名（**顺序是行为的一部分**：先云盘、后邮箱）。
pub const APP_TASK_MARKETS: [&str; 2] = ["sign_in_3", proto::TASK_LIST_V2_EMAIL_MARKET];

/// 这一趟要发几次请求 = 市场数 × group 数。
pub fn app_task_matrix_fetches() -> usize {
    APP_TASK_MARKETS.len() * proto::TASK_LIST_V2_GROUPS.len()
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

/// 只读盘点的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatrixVerdict {
    /// 至少有一次拉取成功（部分失败也落这一档，失败会写进小结与明细）。
    Read,
    /// **十六次全挂** —— 没有结论可报。
    ///
    /// ⚠ 这个变体**刻意是零值**：零值若是 `Read`，一句"矩阵 0 条"会在
    /// **一次都没拉成**的那趟上被念出来，而它看起来像"这个号没任务"。
    #[default]
    Failed,
}

/// 一个账号这一趟的产出。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppTaskMatrixResult {
    pub verdict: MatrixVerdict,
    pub matrix: AppTaskMatrix,
    pub error: String,
}

impl AppTaskMatrixResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// ⚠ **不念任务名**：那是上游文案，进小结等于把上游文案写进我们对外承诺的那一列
    /// （同「拍拍盘点」那条理由）。⚠ 也**不念** `description`（那里面是奖励文案，
    /// 实拍带「领38云朵」这种会随改名漂移的串）。
    pub fn summary_line(&self) -> String {
        const HEAD: &str = "App 任务矩阵";
        if self.verdict == MatrixVerdict::Failed {
            return if self.error.is_empty() {
                format!("{HEAD}没读成")
            } else {
                format!("{HEAD}没读成：{}", self.error)
            };
        }
        let m = &self.matrix;
        let (done, waiting, unknown) = m.state_counts();
        let mut line = format!(
            "{HEAD}：拉到 {}/{} 次、{} 条去重后 {} 条（未完成 {}、已完成 {}",
            m.succeeded,
            app_task_matrix_fetches(),
            m.raw_count,
            m.items.len(),
            waiting,
            done
        );
        if unknown > 0 {
            line.push_str(&format!("、状态读不出 {unknown}"));
        }
        line.push('）');
        if !m.failures.is_empty() {
            // 部分失败必须说出来：不然"少了几组"会被读成"那几组没任务"。
            line.push_str(&format!("；{} 组没读到", m.failures.len()));
        }
        if let Some(disabled) = match m.disabled_count() {
            0 => None,
            n => Some(n),
        } {
            line.push_str(&format!("；其中 {disabled} 条 `enable != 1`"));
        }
        line.push_str("；本工具只读：不打卡、不上传");
        line
    }

    /// 映射到 [`TaskOutcome`]。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            MatrixVerdict::Read => TaskOutcome::success(summary),
            MatrixVerdict::Failed => TaskOutcome::failed(summary),
        }
    }
}

// ══════════════════════════ 拉取 ══════════════════════════

pub(crate) async fn fetch_one(ctx: &TaskContext, market: &str, group: &str) -> Result<TaskListV2> {
    let req: HttpRequest = proto::task_list_v2_request(market, group);
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "读取 App 任务矩阵")?;
    proto::parse_task_list_v2(body, group)
}

/// 拉整张矩阵：2 个市场 × 8 个 group，逐次容错。
pub struct AppTaskMatrixTask;

#[async_trait]
impl Task for AppTaskMatrixTask {
    fn name(&self) -> &'static str {
        "app-task-matrix"
    }
    fn title(&self) -> &'static str {
        "App 任务矩阵（只看）"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::App
    }
    fn description(&self) -> &'static str {
        "把 App 任务矩阵那张表拉回来看（2 个市场 × 8 个 group；只读，不打卡、不上传网盘）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        let total = app_task_matrix_fetches();
        let mut pages: Vec<TaskListV2> = Vec::new();
        let mut failures: Vec<FetchFailure> = Vec::new();
        let mut groups: Vec<GroupReading> = Vec::new();
        let mut done = 0usize;

        for market in APP_TASK_MARKETS {
            for group in proto::TASK_LIST_V2_GROUPS {
                done += 1;
                ctx.check_cancelled()?;
                ctx.progress(done as u32, total as u32, format!("读取 {market}/{group}"));
                match fetch_one(ctx, market, group).await {
                    Ok(page) => {
                        // 组完成度**只存不判**：这一组那枚键就是请求的组名。
                        let finish_state = page
                            .group_finish_states
                            .iter()
                            .find(|(name, _)| name == group)
                            .and_then(|(_, v)| *v);
                        groups.push(GroupReading {
                            market_name: market.to_string(),
                            group: group.to_string(),
                            count: page.items.len(),
                            finish_state,
                        });
                        pages.push(page);
                    }
                    Err(e) => {
                        // 一次失败不算失败 —— 登记它、继续拉下一组。
                        failures.push(FetchFailure {
                            market_name: market.to_string(),
                            group: group.to_string(),
                            error: e.to_string(),
                        });
                    }
                }
            }
        }

        // 十六次**全**挂 ⇒ 没有结论可报（旧 TS 那种"回一张空表"会让全没读到
        // 看起来像"这个号没任务"）。
        if pages.is_empty() {
            let first = failures
                .first()
                .map(|f| format!("{}/{}：{}", f.market_name, f.group, f.error))
                .unwrap_or_else(|| "原因未知".into());
            return Err(mcloud_core::Error::Other(format!(
                "App 任务矩阵 {total} 次拉取全部失败（首个：{first}）"
            )));
        }

        let succeeded = pages.len();
        let (items, raw_count) = merge_pages(&pages);
        let result = AppTaskMatrixResult {
            verdict: MatrixVerdict::Read,
            matrix: AppTaskMatrix {
                items,
                raw_count,
                succeeded,
                failures,
                groups,
            },
            error: String::new(),
        };
        report_details(ctx, &result);
        Ok(result.task_outcome())
    }
}

/// 把这一趟的读数交给明细通道。
///
/// 小结刻意不念任务名与奖励文案，所以**逐条**那份只能走这里 ——
/// 而"这张表里到底有什么"正是跑这条链的人要看的东西。
fn report_details(ctx: &TaskContext, result: &AppTaskMatrixResult) {
    let m = &result.matrix;
    let (done, waiting, unknown) = m.state_counts();
    let items: Vec<Value> = m
        .items
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
            })
        })
        .collect();
    let groups: Vec<Value> = m
        .groups
        .iter()
        .map(|g| {
            serde_json::json!({
                "marketName": g.market_name,
                "group": g.group,
                "count": g.count,
                "taskFinishState": g.finish_state,
            })
        })
        .collect();
    let failures: Vec<Value> = m
        .failures
        .iter()
        .map(|f| {
            serde_json::json!({
                "marketName": f.market_name,
                "group": f.group,
                "error": f.error,
            })
        })
        .collect();
    ctx.details(serde_json::json!({
        "kind": "app-task-matrix",
        "verdict": match result.verdict {
            MatrixVerdict::Read => "read",
            MatrixVerdict::Failed => "failed",
        },
        "error": result.error,
        "fetches": app_task_matrix_fetches(),
        "succeeded": m.succeeded,
        "rawCount": m.raw_count,
        "dedupedCount": m.items.len(),
        "disabledCount": m.disabled_count(),
        "stateCounts": {"done": done, "waiting": waiting, "unknown": unknown},
        "groups": groups,
        "failures": failures,
        "items": items,
    }));
}

#[cfg(test)]
mod tests;
