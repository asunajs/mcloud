//! `app-task` 写的那一半：「把 App 任务矩阵里能做的做掉」。
//!
//! # ⚠ 这条能力**会写**
//!
//! 它每一发 `task/click` 都在告诉上游"这条我做完了"，而且**顺带往用户网盘传文件**
//! （玩法 106 / 522 / beiyong1 的名字门）。`task/click` 不可回滚；
//! 传上去的文件本仓**会自己收尾**（移进回收站 —— 与 `task-expansion` 同一取向，
//! 那里是对参考实现的刻意改进，理由见 `task_expansion.rs` 文件头）。
//!
//! # 为什么和 `app-task-matrix` **不是同一个 code**
//!
//! 本仓 `app-task-matrix` 是「只看」的只读那一半。写的那一半用**另一个 code**
//! （就是本模块）——拿一个 code 做两件事会让它在两处指不同的东西
//! ⇒ 只读那一半叫 `app-task-matrix`，写的那一半叫 `app-task`。
//!
//! # 判据做成**纯函数**
//!
//! 「这一条要不要传、传几个」全部落在 [`upload_shape_for`] 这个纯函数里，
//! 于是它能**在没有网络的情况下被钉住**。真发的枪在后面的执行层。
//!
//! 依据是 **`mcloud-sign`**（那份 TS 是**能跑**的，行为以它为准）：
//! `packages/core/src/tasks/app/runners.ts` 的 `106` / `522` 两个 runner，
//! 以及 `tasks.ts:68-77` 那个 `switch (task.groupid)` 的 `beiyong1` 分支。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskResult};
use mcloud_upstream::app_cloud_upload as upload;
use mcloud_upstream::app_cloud_upload::{CloudChannel, CloudFileCreateInput, CloudOpType};
use mcloud_upstream::app_task_click;
use mcloud_upstream::app_task_list_v2::TaskV2Item;
use mcloud_upstream::cloud_trash;

use crate::app_task_matrix::{self, APP_TASK_MARKETS};

/// 「这一条要往网盘传文件」的参数集。
///
/// ⚠ **零值不成一档** ⇒ "要不要传"由 `Option` 表达，结构体内部不设 `None` 那样的变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadShape {
    /// **最多**传几个。
    pub rounds: u32,
    /// `> 0` ⇒ 每传一个之前先看条目自己的 `process`，达到就停。
    ///
    /// ⚠ 这一格读的 `process` **语义未验证**（实拍里三条恒 0，而同批条目全 `FINISH`）。
    /// 用它的理由不是"它靠谱"，是"**旧实现读它**"；而真正兜底的是 [`Self::rounds`] ——
    /// 就算 `process` 永远读到 0，这一趟也最多传 5 个，不会变成"连传 100 个"。
    pub stop_at_process: i64,
    /// **每传完一个**等一拍（旧 522 循环体末尾那句 `sleep(500)`，**包括最后一轮**）。
    ///
    /// ⚠ 它是 522 与 106 **唯一的形状差别**（旧实现 106 的 runner 体里没有这一句）。
    /// 把两者混成一个的后果是：要么 522 变成"连传 5 个零间隔"，要么 106 白等一拍
    /// —— 两种都**不会报错**，只在"哪天被判成脚本"那一类最难查的差异上显形。
    pub pace_each_round: bool,
}

/// 「就传一个」那一档（玩法 106 与 beiyong1 的名字门共用）。
///
/// 单独一个常量是为了让"106 与 beiyong1 走的是**同一档**形状"这件事可断言 ——
/// 旧 TS 两处都是裸的 `uploadRandomFile(ctx)`，没有任何参数差别。
pub const UPLOAD_ONE: UploadShape = UploadShape {
    rounds: 1,
    stop_at_process: 0,
    pace_each_round: false,
};

/// 522 那一趟最多传几个（旧实现的 `每月上传任务单日数量 ?? 5` 里那个默认值）。
///
/// ⚠ 本仓**没有**"每月上传任务单日数量"这一格配置（旧实现是可配的）⇒ 取旧实现那个
/// **默认值 5**。要开成参数面得连 CLI 与 WebUI 一起改，不在这一层单方面决定。
pub const ROUNDS_PER_RUN: u32 = 5;

/// 玩法 522 的 `process` 门槛（旧实现那个 `>= 100`）。
pub const STOP_AT_PROCESS: i64 = 100;

/// beiyong1 分组的名字门（旧实现逐字）：
/// `name.includes("上传") && (name.includes("图") || name.includes("照"))`。
///
/// ⚠ 旧实现同一支里**还有一句**看 `button.app.link` 含 `uploadView` ⇒ 走"补满 10 张"
/// 那条（`ai-tasks.ts` 的 `upload10PictureTask`）。**本模块不实现那一支**：
/// 我们的 `TaskV2Item` 里没有 `button` 那一格（解析器没读它），而"10 张"那条玩法
/// 的产物链（`aiRecognize` / `chat/v2/add` SSE）本仓**零实拍**。
/// ⇒ 留空并注明，不编一个"看起来合理"的入口。
pub fn beiyong1_name_gate(name: &str) -> bool {
    name.contains("上传") && (name.contains("图") || name.contains("照"))
}

/// 这一条要不要传、按哪一档传。判不了（不传）⇒ `None`。
///
/// 三个来源逐条对着旧实现：
///
/// | 命中 | 档 | 旧实现出处 |
/// |---|---|---|
/// | `task_id == 106` | [`upload_one`] | `runners.ts:41-44` 的 `106` runner |
/// | `task_id == 522` | `{5, 100, pace}` | `runners.ts:117-125` 的循环 |
/// | `group_id == "beiyong1"` 且过名字门 | [`upload_one`] | `tasks.ts:68-77` |
///
/// ⚠ 三条是**并列**的，不是"先按 id 再看分组"：旧实现里 106 / 522 走 runner 表、
/// beiyong1 走 `switch (task.groupid)`，两条路互不遮罩 —— 一个 beiyong1 分组里
/// 的条目也可能正好 `task_id == 106`，那时两边给的**是同一档**（都是 `upload_one`），
/// 所以先判哪一条都不改结果。这一点由测试钉住，免得将来有人"顺手"给 beiyong1 换一档。
pub fn upload_shape_for(item: &TaskV2Item) -> Option<UploadShape> {
    match item.task_id {
        Some(106) => return Some(UPLOAD_ONE),
        Some(522) => {
            return Some(UploadShape {
                rounds: ROUNDS_PER_RUN,
                stop_at_process: STOP_AT_PROCESS,
                pace_each_round: true,
            })
        }
        _ => {}
    }
    if item.group_id.as_deref() == Some("beiyong1") {
        if let Some(name) = item.name.as_deref() {
            if beiyong1_name_gate(name) {
                return Some(UPLOAD_ONE);
            }
        }
    }
    None
}

/// 这一趟还要不要再传一个（每轮开始前问一次）。
///
/// 两条停止条件：
/// * 已经传满 `rounds`
/// * `stop_at_process > 0` 且条目当前 `process >= stop_at_process`
///
/// ⚠ `process` 缺失（上游没给这一格）**当 0**，与旧实现读 `task.process` 拿到
/// `undefined` 的行为差别在于：JS 里 `undefined >= 100` 是 `false`（继续传），
/// 我们当 0 也是继续传 ⇒ **同向**。这一格由测试钉住。
pub fn should_upload_another(shape: UploadShape, done: u32, process: Option<i64>) -> bool {
    if done >= shape.rounds {
        return false;
    }
    if shape.stop_at_process > 0 && process.unwrap_or(0) >= shape.stop_at_process {
        return false;
    }
    true
}

// ══════════════════════════ 执行层 ══════════════════════════

/// 每一轮上传之间等多久（旧 TS 522 循环体末尾那句 `sleep(500)`）。
pub const PACE_MS: i64 = 500;

/// 一条待做任务的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskStep {
    pub task_id: Option<i64>,
    pub name: String,
    /// 真传上去的个数。
    pub uploaded: u32,
    /// 打卡那一枪的结局；`None` = 没打（形状是"不传"的那种，或上传全挂）。
    pub clicked: bool,
    /// 逐条的原因。
    pub problems: Vec<String>,
}

/// 这一趟的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppTaskResult {
    /// **真的发过写请求**（传过文件或打过卡）的条目数。
    pub acted: usize,
    /// 判出来"要传"的条目数。
    pub planned: usize,
    pub steps: Vec<TaskStep>,
    /// 我们造的文件 id（**`exist` 那一档不进来** —— 那是用户自己的文件）。
    pub file_ids: Vec<String>,
    pub cleaned_up: usize,
    pub cleanup_error: String,
    pub failures: Vec<String>,
}

impl AppTaskResult {
    pub fn summary_line(&self) -> String {
        const HEAD: &str = "App 任务";
        if self.planned == 0 {
            return format!("{HEAD}没做：这张表里没有需要上传的条目");
        }
        let tail = match (self.file_ids.is_empty(), self.cleaned_up) {
            (true, _) => String::new(),
            (false, n) if n == self.file_ids.len() => {
                format!("（网盘新增 {} 个文件，已移进回收站）", self.file_ids.len())
            }
            (false, n) => format!("（网盘新增 {} 个文件，只收回 {n} 个）", self.file_ids.len()),
        };
        let mut s = format!(
            "{HEAD}做了 {} 条（判出 {} 条）{tail}",
            self.acted, self.planned
        );
        if !self.cleanup_error.is_empty() {
            s.push_str(&format!("；清理失败：{}", self.cleanup_error));
        }
        if !self.failures.is_empty() {
            s.push_str(&format!("；{} 条有失败", self.failures.len()));
        }
        s
    }

    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        // ⚠ 一条都没做成、又确实判出该做 ⇒ 报失败，别报成"没事发生"。
        if self.planned > 0 && self.acted == 0 {
            return TaskOutcome::failed(summary);
        }
        if self.planned == 0 {
            return TaskOutcome::skipped(summary);
        }
        TaskOutcome::success(summary)
    }
}

/// 跑一次 `app-task`：**写**。
///
/// # ⚠ 顺序是**先打卡、再上传**（不是我原先以为的反过来）
///
/// 旧 TS `tasks.ts:33-44` 的 `handleTaskClick` 逐字：
///
/// ```ts
/// if (await clickTaskWithStep(ctx, task.id, task.currstep)) {   // ← 先打卡
///   await taskRunner[task.id]?.(ctx, task);                     // ← 成功之后才跑玩法
/// }
/// ```
///
/// ⇒ **打卡是一道门**：先问上游"这一条记上了没有"，成了才去真的做那件事。
/// 参考实现（Go）写的是同一句："beiyong1 的'上传图/照'那一条旧实现是
/// **先打卡、再上传**"。
///
/// ⚠ 这一版最初把顺序写反了（先传再打卡），理由是我自己推的
/// "打卡是'我做完了'的声明，声明在动作之前就是撒谎" —— **两边参考都不支持那句**。
/// 从第一原理推出来的行为，只要与参考相反，就是错的。
pub async fn run_app_task(ctx: &TaskContext, pace_ms: i64) -> Result<AppTaskResult> {
    let mut result = AppTaskResult::default();

    // ① 读整张表（十六组，逐次容错）。
    let mut pages: Vec<mcloud_upstream::app_task_list_v2::TaskListV2> = Vec::new();
    for market in APP_TASK_MARKETS {
        for group in mcloud_upstream::app_task_list_v2::TASK_LIST_V2_GROUPS {
            ctx.check_cancelled()?;
            match app_task_matrix::fetch_one(ctx, market, group).await {
                Ok(p) => pages.push(p),
                // 一次失败不算失败 —— 登记它、继续拉下一组（与只读那一半同口径）。
                Err(e) => result.failures.push(format!("读取 {market}/{group}：{e}")),
            }
        }
    }
    let (items, _raw) = app_task_matrix::merge_pages(&pages);

    // ② 逐条判形状。判不出来的一律**不动**。
    let planned: Vec<(TaskV2Item, UploadShape)> = items
        .iter()
        .filter_map(|i| upload_shape_for(i).map(|s| (i.clone(), s)))
        .collect();
    result.planned = planned.len();

    // ③ 逐条做。
    for (item, shape) in &planned {
        ctx.check_cancelled()?;
        let name = item.name.clone().unwrap_or_default();
        let mut step = TaskStep {
            task_id: item.task_id,
            name: name.clone(),
            ..Default::default()
        };
        ctx.progress(
            result.steps.len() as u32 + 1,
            planned.len() as u32,
            format!("做任务 {name}"),
        );

        // ③-a **先打卡**：它是一道门，成了才去做那件事。
        match click(ctx, item).await {
            Ok(notice) => {
                step.clicked = true;
                if let Some(n) = notice {
                    step.problems.push(format!("上游附言：{n}"));
                }
            }
            Err(e) => {
                // 门没过 ⇒ **不传**（旧 `if (await clickTaskWithStep(...))` 那个形状）。
                step.problems.push(format!("打卡失败，这一条不传：{e}"));
                result.steps.push(step);
                continue;
            }
        }

        // ③-b 门过了 ⇒ 按档传。
        let mut done: u32 = 0;
        while should_upload_another(*shape, done, item.process) {
            ctx.check_cancelled()?;
            match upload_one_file(ctx).await {
                Ok(Some(id)) => {
                    result.file_ids.push(id);
                    step.uploaded += 1;
                }
                // `exist` 那一档：**不是我们造的** ⇒ 不记账（记了就是删用户的文件）。
                Ok(None) => step
                    .problems
                    .push("上游说同名文件已存在 ⇒ 不算我们造的，也不去删".into()),
                Err(e) => step.problems.push(format!("上传失败：{e}")),
            }
            done += 1;
            if shape.pace_each_round && pace_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pace_ms as u64)).await;
            }
        }

        if step.uploaded > 0 || step.clicked {
            result.acted += 1;
        }
        result.steps.push(step);
    }

    // ④ 清理：**只删我们自己造的**。
    cleanup(ctx, &mut result).await;
    Ok(result)
}

/// 传一个随机文件，交回**我们造的**那个 id（`exist` 档交 `None`）。
async fn upload_one_file(ctx: &TaskContext) -> Result<Option<String>> {
    let mut rng = mcloud_crypto::OsRandom;
    let plan = crate::task_expansion::random_plan(&mut rng, 0);
    let hash = mcloud_crypto::sha256_hex_bytes(&plan.content);
    let created = {
        let req = upload::file_create_request(CloudFileCreateInput {
            parent_file_id: "/".into(),
            name: plan.name.clone(),
            size: plan.content.len() as i64,
            content_hash: hash.clone(),
            channel: CloudChannel::Android,
            op: CloudOpType::Upload,
        })?;
        let resp = ctx.http.request(req).await?;
        upload::parse_file_create(&resp.body)?
    };
    // ⚠ 撞同名 ⇒ 那个文件**不是我们造的**，交回 `None`。
    if created.exist {
        return Ok(None);
    }
    let Some(file_id) = created.file_id.clone() else {
        return Err(mcloud_core::Error::Parse("create 没给 fileId".into()));
    };
    if !created.rapid_upload {
        let Some(url) = created.upload_url.clone() else {
            return Err(mcloud_core::Error::Parse("create 没给 uploadUrl".into()));
        };
        let resp = ctx
            .http
            .request(upload::upload_put_request(&url, &plan.content))
            .await?;
        upload::ensure_put_ok(&resp)?;
        let req = upload::file_complete_request(
            &file_id,
            created.upload_id.as_deref().unwrap_or(""),
            &hash,
            CloudChannel::Android,
            CloudOpType::Upload,
        )?;
        let resp = ctx.http.request(req).await?;
        upload::parse_file_complete(&resp.body)?;
    }
    Ok(Some(file_id))
}

/// 打一发「这条我做完了」。
async fn click(ctx: &TaskContext, item: &TaskV2Item) -> Result<Option<String>> {
    let Some(id) = item.task_id else {
        return Err(mcloud_core::Error::Config("这一条没有 id，不打卡".into()));
    };
    let key = item.group_id.as_deref().unwrap_or("");
    let req: HttpRequest = app_task_click::task_click_request(key, id)?;
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "打卡（task/click）")?;
    let env = mcloud_upstream::app_signin::SignInEnvelope::parse(body)?;
    Ok(app_task_click::parse_task_click(&env)?.notice)
}

/// 把我们自己传上去的那些移进回收站。
async fn cleanup(ctx: &TaskContext, result: &mut AppTaskResult) {
    if result.file_ids.is_empty() {
        return;
    }
    let req = match cloud_trash::batch_trash_request(&result.file_ids) {
        Ok(r) => r,
        Err(e) => {
            result.cleanup_error = e.to_string();
            return;
        }
    };
    match ctx.http.request(req).await {
        Ok(resp) => match cloud_trash::parse_batch_trash(&resp.body) {
            Ok(()) => result.cleaned_up = result.file_ids.len(),
            Err(e) => result.cleanup_error = e.to_string(),
        },
        Err(e) => result.cleanup_error = e.to_string(),
    }
}

/// 跑一次 `app-task`（**会写**：往用户网盘传文件并打卡）。
pub struct AppTaskTask {
    /// 每轮之间的等待。做成字段是为了让测试设 0。
    pub pace_ms: i64,
}

impl Default for AppTaskTask {
    fn default() -> Self {
        Self { pace_ms: PACE_MS }
    }
}

impl AppTaskTask {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Task for AppTaskTask {
    fn name(&self) -> &'static str {
        "app-task"
    }
    fn title(&self) -> &'static str {
        "App 任务（做任务）"
    }
    fn description(&self) -> &'static str {
        "把 App 任务矩阵里能做的做掉：按玩法传文件、逐条打卡（传上去的文件跑完自己删）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.check_cancelled()?;
        let result = run_app_task(ctx, self.pace_ms).await?;
        ctx.details(serde_json::json!({
            "kind": "app-task",
            "planned": result.planned,
            "acted": result.acted,
            "uploadedFiles": result.file_ids.len(),
            "cleanedUp": result.cleaned_up,
            "cleanupError": result.cleanup_error,
            "failures": result.failures,
            "steps": result.steps.iter().map(|s| serde_json::json!({
                "taskId": s.task_id,
                "name": s.name,
                "uploaded": s.uploaded,
                "clicked": s.clicked,
                "problems": s.problems,
            })).collect::<Vec<_>>(),
        }));
        ctx.info(result.summary_line());
        Ok(result.task_outcome())
    }
}

#[cfg(test)]
mod tests;
