//! 拍拍（`play-ai-special`）—— **只读盘点**那一半的编排。
//!
//! 两枪：`user/info`（次数读数）→ `task/list`（任务表）。请求构造与解析在
//! [`mcloud_upstream::app_play_ai`]（纯函数、无 IO）。
//!
//! # 为什么这一条只做只读（另外三口各自卡在不一样的地方）
//!
//! 上游那一族有五个口子，这里只接前两个，因为剩下三口的"该不该发"**今天都答不出来**：
//!
//! * **③ `task/step/click`（写）**：实拍序列是"查 → 上报一步 → 回查"，也就是说上游
//!   确实**认**这一枪。但旧实现把 `visitlog/journaling` 埋点当**门槛**用（上报失败就连
//!   `step/click` 都不发），而那两条埋点**零实拍** ⇒ 接不接门槛是待拍板的事。
//! * **④ `blindbox/lottery`（写）**：判据本该是 ① 那个次数读数，而这一格**语义未验证**
//!   （实拍那条 `1` 出现在**抽奖之前**，所以它至少含"还没花掉"的意思，但一次观测
//!   说不出花掉会不会减回去）。拿一个读不懂的数当判据去抽 = 猜，而且抽奖**不可回滚**。
//! * **⑤ `aiRecognize`（写云盘）**：它 `uploadToCloud` 实拍恒 `true` ⇒ 真的往用户网盘
//!   写一张图，而"识别出来的 `fileId` 谁来删"那一族今天接口层冻结 ⇒ 不发。
//!
//! 于是这里交"看得见的一层"：这个号在这个活动里有什么任务、做到哪一步、次数那一格
//! 读到多少。它单独成立（用户能照着任务名去 App 里做）。
//!
//! # 这一层**不解释任何档位语义**
//!
//! 只念读数：任务表里 `state` 只有 `WAIT` / `FINISH` 两个实拍值，其余（含缺键）
//! 一律算"状态读不出"并**单独计数**，**不**折进"未完成"（将来接 ③ 时那一格会
//! 直接决定发不发请求）。次数那一格同理：只念原值、不据此判断。
//!
//! # ⚠ `jwttoken` 使用**公共来源**（临时态不是终判）
//!
//! 这一页的特有换票链**有出处、但零实拍**（整份夹具里连一次换票都没拍到）
//! ⇒ 今天走仓库现成的建会话入口。切过去要多发两枪本仓零实拍的请求，
//! 而"上游认不认这一页自己那张票"与"签到页那张票打这一族的接口认不认"**两个都没有样本**。
//!
//! ⚠ 但 **Referer 不借**：用 [`proto::play_ai_page_referer`]（旧 TS 逐字，B 档），
//! **不用**会话里那份签到页 referer。"票借、页面身份不借"这个组合**零实拍** ——
//! 真打时若回非零码，第一个要试的就是把 referer 换成会话自带的那一份。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_play_ai as proto;
use mcloud_upstream::app_play_ai::{PlayAiUserStatus, PlayAiUserTask};

/// 次数那一格"没解释"的原话。
///
/// 单独成常量是为了让它**只有一处**：这句要抄进小结与测试断言，
/// 抄成两处就早晚只改一处。
const LOTTERY_COUNT_HEDGE: &str = "是累计获得还是还剩，未验证";

/// 发一个请求并解成信封，再交给调用方解析。
async fn call<T>(
    ctx: &TaskContext,
    req: HttpRequest,
    what: &str,
    parse: fn(&str) -> Result<T>,
) -> Result<T> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    parse(body)
}

// ══════════════════════════ 盘点 ══════════════════════════

/// 这一屏的两份读数（①与②，**按实拍顺序**：夹具里先查状态再查任务表）。
///
/// ⚠ `tasks` 为**空数组**不等于"这个号没事做"：上游给 `result: []` 与
/// "这一期没配任务"是同一种回法 ⇒ 念的时候只数条数，**不下**"没有可做的"那个结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayAiBoard {
    /// ① `user/info`。
    pub status: PlayAiUserStatus,
    /// ② `task/list`。
    pub tasks: Vec<PlayAiUserTask>,
}

/// 从任务表**算出来**的四个计数。
///
/// ⚠ 单独成一个类型（而不是在小结里就地算）是为了让"未知那一档存在"这件事在测试里
/// 可读 —— 把 `2` 那种没见过的值折进"未完成"是最容易顺手写错的归并。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayAiTaskCounts {
    pub total: usize,
    pub done: usize,
    pub waiting: usize,
    pub unknown: usize,
}

impl PlayAiBoard {
    /// 数任务表的三档 + 总数。
    pub fn counts(&self) -> PlayAiTaskCounts {
        let mut counts = PlayAiTaskCounts {
            total: self.tasks.len(),
            ..Default::default()
        };
        for task in &self.tasks {
            match task.state.as_deref() {
                Some(proto::PLAY_AI_TASK_STATE_DONE) => counts.done += 1,
                Some(proto::PLAY_AI_TASK_STATE_WAITING) => counts.waiting += 1,
                // 缺键、空串、以及这一族没见过的任何值都在这里。
                _ => counts.unknown += 1,
            }
        }
        counts
    }
}

/// 只读盘点的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayAiVerdict {
    /// 两枪都拿到了合法信封。
    ///
    /// ⚠ 它**不**要求次数那一格非 `None`，也不要求任务表非空：
    /// 那些是"上游给了几个读数"，不是"这一趟成没成"。
    Read,
    /// 没走到能报结论的那一步（建会话挂 / 任一枪挂）。
    ///
    /// ⚠ 这个变体**刻意是零值**：零值若是 `Read`，一句"拍拍任务 0 件"
    /// 就会在**一枪都没发成**的那趟上被念出来 —— 而"0 件"看起来像一份正常读数。
    #[default]
    Failed,
}

/// 一个账号这一趟的产出。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayAiResult {
    pub verdict: PlayAiVerdict,
    pub board: PlayAiBoard,
    pub error: String,
}

impl PlayAiResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// # 三个"不念"
    ///
    /// * **不念"还能抽 N 次"**：那一格语义未验证，只写"读到 N"并标明没解释。
    /// * **不念任务名**：`name` / `flag` 是上游文案（实拍「拍夏日关键词」这种带期次
    ///   色彩的串），进小结等于把上游文案写进我们对外承诺的那一列。
    /// * **不念手机号**：这一族的响应里不带账号标识，但这一句会被抄进日志，
    ///   所以将来加字段时也别把上游可能给的掩码号塞进来。
    pub fn summary_line(&self) -> String {
        const HEAD: &str = "拍拍";
        if self.verdict == PlayAiVerdict::Failed {
            return if self.error.is_empty() {
                format!("{HEAD}没读成")
            } else {
                format!("{HEAD}没读成：{}", self.error)
            };
        }
        let counts = self.board.counts();
        let mut line = format!(
            "{HEAD}任务 {} 件（未完成 {}、已完成 {}",
            counts.total, counts.waiting, counts.done
        );
        if counts.unknown > 0 {
            // 单独念出来：这一档存在就说明"上游给了这一族没见过的值"，
            // 藏进总数里的话，将来接写请求时这一格会被当成两档之一。
            line.push_str(&format!("、状态读不出 {}", counts.unknown));
        }
        line.push_str("）；");
        line.push_str(&lottery_count_line(self.board.status.total_lottery_count));
        line.push_str("；本工具只读：不做任务、不抽奖、不写网盘");
        line
    }

    /// 映射到 [`TaskOutcome`]。
    ///
    /// 只读那一半**不产 `ManualSteps`**：那档的语义是"得有人去 App 里点一下才继续"，
    /// 而这一条今天的用途就是"看一眼" —— 它不"继续"任何流程。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            PlayAiVerdict::Read => TaskOutcome::success(summary),
            PlayAiVerdict::Failed => TaskOutcome::failed(summary),
        }
    }
}

/// 念 ① 那一格。
///
/// ⚠ `None` 与 `0` 分开：实拍**两条都有值**（`0` 与 `1`），所以"没给这一位"是一种
/// **没样本**的形态 ⇒ 只报"没给"，不猜它等于几。
fn lottery_count_line(count: Option<i64>) -> String {
    match count {
        None => "抽奖次数那一格上游没给（这一格语义本就未验证，不据此判断）".to_string(),
        Some(n) => format!("抽奖次数那一格读到 {n}（{LOTTERY_COUNT_HEDGE}）"),
    }
}

// ══════════════════════════ 任务 ══════════════════════════

/// 跑只读那两枪：`user/info` → `task/list`。
///
/// # 两枪**都**是致命读
///
/// ① 给的是这一屏唯一的"次数"读数，② 给的是"还有什么事可做"。
/// 缺任何一句都**不**只是"少一行" —— 缺 ② 会让用户以为这个号没事做，
/// 那是会改变行为的误读 ⇒ 两枪任一失败都交回错误，不折成"半份结论"。
pub struct PlayAiTask;

#[async_trait]
impl Task for PlayAiTask {
    fn name(&self) -> &'static str {
        "play-ai-special"
    }
    fn title(&self) -> &'static str {
        "拍拍盘点"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::App
    }
    fn description(&self) -> &'static str {
        "看这个号在拍拍活动里的任务表与抽奖次数读数（只盘点不代做：不做任务、不抽奖、不写网盘）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        // ⚠ 这里**曾经**有一道最外层的账号级总闸（`play_ai_enabled`，照 TS 的
        // `if (config.春日拍拍大作战?.开启)` 搬来的）：关 ⇒ 一个请求都不发。
        // 2026-10-03 用户口径「**账号不应该有这个配置**」⇒ 字段已删。
        // 任务**是用户显式点起来的**，"跑不跑"这一层由那个动作表达。

        ctx.progress(1, 2, "读取拍拍活动状态");
        // ⚠ 这一条**只发读请求**，两枪的 URL 里没有任何写路径。
        let status = call(
            ctx,
            proto::play_ai_status_request(),
            "读取拍拍活动状态",
            proto::parse_play_ai_status,
        )
        .await?;

        ctx.check_cancelled()?;
        ctx.progress(2, 2, "读取拍拍任务表");
        let tasks = call(
            ctx,
            proto::play_ai_task_list_request(),
            "读取拍拍任务表",
            proto::parse_play_ai_task_list,
        )
        .await?;

        let result = PlayAiResult {
            verdict: PlayAiVerdict::Read,
            board: PlayAiBoard { status, tasks },
            error: String::new(),
        };
        report_details(ctx, &result);
        Ok(result.task_outcome())
    }
}

/// 把这一趟的读数交给明细通道。
///
/// 这一条是**只读盘点**：跑它的人想看的就是"有哪些任务、各在哪一步、次数读到几"，
/// 而小结那一句被刻意压成了不解释的计数。任务级明细只能走这里，否则
/// `currstep` / `recordid` / `limitType` 这些上游真正给出来的读数会全丢，
/// 而**下一批收口"这一格到底是什么语义"的读数正是它们**。
fn report_details(ctx: &TaskContext, result: &PlayAiResult) {
    let counts = result.board.counts();
    let tasks: Vec<Value> = result
        .board
        .tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "taskId": t.task_id,
                "name": t.name,
                "flag": t.flag,
                "description": t.description,
                "groupId": t.group_id,
                "state": t.state,
                "limitType": t.limit_type,
                "recordId": t.record_id,
                "currStep": t.curr_step,
                "process": t.process,
            })
        })
        .collect();
    ctx.details(serde_json::json!({
        "kind": "play-ai-special",
        "verdict": match result.verdict {
            PlayAiVerdict::Read => "read",
            PlayAiVerdict::Failed => "failed",
        },
        "error": result.error,
        "totalLotteryCount": result.board.status.total_lottery_count,
        "lotteryCountHedge": LOTTERY_COUNT_HEDGE,
        "counts": {
            "total": counts.total,
            "done": counts.done,
            "waiting": counts.waiting,
            "unknown": counts.unknown,
        },
        "tasks": tasks,
    }));
}

#[cfg(test)]
mod tests;
