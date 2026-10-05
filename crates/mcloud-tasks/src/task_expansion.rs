//! 「任务拓展」（`task-expansion`）：读一次状态 →（本月没开备份就**传一个文件凑**）
//! → 等一会儿 → 重读一次 →（这个月攒下记录就）领掉 → **把自己传的那个文件删掉**。
//!
//! # 这一条是旧项目里唯一"为了改变上游状态而写用户网盘"的任务
//!
//! `cur_month_backup == Some(false)` 时它不是提示用户去开备份，而是**自己传一个文件**
//! 把这一格刷成 true。本仓其余写网盘的链（`ai-avatar`、`app-task` 的 106/522）
//! 传文件是**任务要求**，而这里的传文件是**冒充任务要求**。
//!
//! # ⚠ 与参考实现的一处刻意不同：**本仓跑完会删掉自己传的那个文件**
//!
//! 参考实现这一条链**不删**（它把那件事留在 P1 缺口里），跑一次你的网盘里就多一个
//! 没人清理的文件。本仓有 [`mcloud_upstream::cloud_trash`]（`batchTrash`，
//! 2026-09-26 起有真跑读数），所以链尾**自己收尾**。
//!
//! ⚠⚠ 但**撞同名那一档绝不删**：`exist == true` 时那个 `fileId` 是
//! **用户本来就有的文件**（上游按 `refuse` 语义把已有那枚交回来了），
//! 把它放进清理名单就是**删用户的东西**。所以那一档**不交 id**（`upload_file_id` 留空）。
//!
//! ⚠ 删除是**移进回收站**不是彻底删 —— 真删错了还能捞回来。
//!
//! # 判据（纯函数，不碰网络）
//!
//! * 凑备份：`cur_month_backup == Some(false)` —— **严格判**，不是"没读到 true"。
//!   `None`（上游没给这一格）走"不凑"：把"上游没说"读成"上游说了没开"
//!   就是拿一次写用户网盘去补一个空洞。
//! * 领：`cur_month_task_record_count > 0`。`None` 也**不领**。
//! * 两格都没读到 ⇒ 判不了：**一个请求都不发**。
//!
//! # 两处"刻意"与参考实现不同（都有注释）
//!
//! 1. **上传一枪都没发出去时不空等**：旧实现的 `backFile` 返回值在调用点被丢掉，
//!    于是"备份失败"之后**照样 sleep**。那一等是"给服务端时间把这次上传记进当月备份"，
//!    而**没有上传就没有东西要记** ⇒ 只在真的发出过 `file/create` 之后等。
//!    ⚠ 少等的那一支**仍然重读**（重读是只读、无害，而结论要用最新那一份）。
//! 2. **领取成功但上游没报数，仍然报"领了"**：旧实现读的是
//!    `!receiveErr && receiveData?.result?.cloudCount` —— 一个 truthy 判据，
//!    于是"`code:0` 但 `result` 空"那一趟它**一句都不念**，看起来像什么都没发生。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_cloud_upload as upload;
use mcloud_upstream::app_cloud_upload::{CloudChannel, CloudFileCreateInput, CloudOpType};
use mcloud_upstream::app_signin as proto;
use mcloud_upstream::app_signin::{SignInEnvelope, TaskExpansion};
use mcloud_upstream::cloud_trash;

/// 账号配置的 `backup_wait_secs` → 毫秒（旧 TS：`(ctx.config.backupWaitTime || 20) * 1000`）。
///
/// ⚠ 本仓**刻意**把 `0` 当「不等待」，而不是像 TS 那句 `|| 20` 回落到 20 秒：
/// 用户把等待调成 0 的意思就是"别等"（测试与活体探针也靠它不真睡 20 秒）。
///
/// 等的是"服务端把刚发出的一次备份事件记进当月"，所以等待只发生在上传
/// **真的发过枪**之后（见 [`BackupUpload::needs_wait`]）。
pub fn backup_wait_ms(secs: u64) -> i64 {
    // `u64 → i64` 用饱和：一个手改坏的值不该在转换时绕回负数。
    i64::try_from(secs).unwrap_or(i64::MAX).saturating_mul(1000)
}

/// 最多凑几次备份 = 1。
///
/// ⚠ 参考实现把这一格单独列成常量并由一条"最多传一个文件"的测试钉住，
/// 理由照抄：谁把这里改成循环，必须让那条上限**真的**参与判断。
pub const TASK_EXPANSION_MAX_UPLOADS: usize = 1;

// ══════════════════════════ 请求 ══════════════════════════

async fn get_body(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<String> {
    let resp = ctx.http.request(req).await?;
    Ok(mcloud_upstream::session::ensure_http_ok(&resp, what)?.to_string())
}

/// 读一次（只读，可重复调）。
async fn read_once(ctx: &TaskContext, device_id: Option<&str>) -> Result<TaskExpansion> {
    let body = get_body(
        ctx,
        proto::task_expansion_request(device_id),
        "读取任务拓展",
    )
    .await?;
    proto::parse_task_expansion(&body)
}

// ══════════════════════════ 判据（纯函数） ══════════════════════════

/// 这一份读数要求**去凑备份**。
///
/// ⚠ 判据是 `cur_month_backup` **读到 `false`**，不是"没读到 `true`"。
/// `None` 走"不凑" —— 这一条不是抠字：凑备份会**往用户网盘写一个文件**。
pub fn should_upload(read: &TaskExpansion) -> bool {
    read.cur_month_backup == Some(false)
}

/// 这一份读数要求**去领**。
///
/// ⚠ `None` 也**不领**：领是写请求，而"记录数为 0"与"上游没说记录数"
/// 在念出来时是两句话，在发不发这一枪上却是同一个处置。
pub fn should_claim(read: &TaskExpansion) -> bool {
    matches!(read.cur_month_task_record_count, Some(n) if n > 0)
}

/// 这一份读数**至少给出一格判据**。
///
/// 两格都没有 ⇒ 一个请求都不发，并把"上游回了 `code:0` 但什么都没读出来"说清楚。
pub fn judged(read: &TaskExpansion) -> bool {
    read.cur_month_backup.is_some() || read.cur_month_task_record_count.is_some()
}

// ══════════════════════════ 档位 ══════════════════════════

/// 这一趟的结论档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExpansionVerdict {
    /// **没走到能报结论的那一步**（读第一遍就挂了）。
    ///
    /// ⚠ 刻意占零值：[`run_expansion`] 有好几条提前 `return` 错误的路径，
    /// 那些路径上结构体是零值。零值若是"领到了"，一句"领到 N 颗"就会在
    /// **一个请求都没发**的那趟上被念出来。
    #[default]
    NotAttempted,
    /// 领取那一发**发出去且上游认了**。
    Claimed,
    /// **没得领**（结论那次读到的记录数是 0 或没读到）。算**成功**。
    Nothing,
    /// **凑过一次备份之后仍未开**，且这一趟没领到东西 ⇒ 要人去 App 里开自动备份。
    ///
    /// ⚠ 与"公告任务"那一档不同，这一条**今天可达**：判据就是重读那一次的
    /// `cur_month_backup == Some(false)`。
    BackupManual,
    /// **判不了**：两格判据都没读到 ⇒ 一枪不发。
    Unreadable,
    /// 领取那一发**发了但失败**。
    ///
    /// 不返回错误：请求发过、没成，这是实况；报成错误会把"发过"这件事抹掉。
    ClaimFailed,
}

/// "凑备份那一发"的结局档位。
///
/// ⚠ 它与 [`ExpansionVerdict`] 是**两个轴**：上传成没成 ≠ 这一趟领没领到
/// （旧实现里这两件事在同一条链上但**互不门控** —— 上传失败它仍会重读、仍会去领）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackupUpload {
    /// **没凑**：备份本来就开着，或那一格没读到。
    #[default]
    NotNeeded,
    /// 三枪走完、网盘里多了一个**我们造的**文件。
    Done,
    /// **秒传**（`rapidUpload:true`）：后两枪没发，但上游收到了这一次备份类事件。
    ///
    /// ⚠ 这一档本仓**零样本**。
    Rapid,
    /// **撞上用户已有同名文件**（`exist:true`）。
    ///
    /// ⚠⚠ 那一个**不是我们造的** ⇒ id **绝不**进清理名单（删它就是删用户的东西）。
    Exists,
    /// `exist` 那一格**读不出来**（缺席 / `null` / 数字 —— 形状变了）。
    ///
    /// 处置与 [`Self::Exists`] 同向（**不交 id** ⇒ 绝不删），单独一档只为让
    /// "上游改了响应形状"这件事在明细里看得见；2026-10-04 复审 `up-1`：
    /// 旧写法把这一格兜成 `false` ⇒ 形状一变就会删**用户自己的文件**。
    ExistUnknown,
    /// **一枪都没发**：前置缺（取不到 deviceId，或随机源没接）。
    ///
    /// 单独一档是因为它与"发了但失败"要分开：前者是本工具侧缺接线，
    /// 后者是上游侧的事，处置完全不同。
    Blocked,
    /// 上传**发过、没成**。
    Failed,
}

impl BackupUpload {
    /// 这一档之后要不要等一会儿再重读。
    ///
    /// 等的是"服务端把刚刚这一次备份事件记进当月" ⇒ 前提是**事件真的发出去了**。
    /// [`BackupUpload::Failed`] 里也含"`create` 发出去了但 PUT / complete 挂了"那种
    /// （错误路径上也可能已经占了坑）⇒ 仍等。只有"一枪没发"不等。
    pub fn needs_wait(self) -> bool {
        !matches!(self, BackupUpload::NotNeeded | BackupUpload::Blocked)
    }

    /// 这一档的人话名（播报与小结共用一份，免得两处漂移）。
    pub fn word(self) -> &'static str {
        match self {
            BackupUpload::Done => "传了一个文件，已删",
            BackupUpload::Rapid => "秒传（后两枪没发），已删",
            BackupUpload::Exists => "撞上同名（那个文件不是我们造的，不删）",
            BackupUpload::ExistUnknown => "exist 读不出来（形状变了）⇒ 按不是我们造的算，不删",
            BackupUpload::Blocked => "一枪没发",
            BackupUpload::Failed => "没传成",
            BackupUpload::NotNeeded => "没凑",
        }
    }
}

/// 链尾清理那一枪的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CleanupVerdict {
    /// 没有我们造的文件要清（没凑 / 撞同名 / 一枪没发）。
    #[default]
    NotNeeded,
    /// 移进回收站了。
    Trashed,
    /// 删那一枪发了但失败 —— **要把原文带出来**，否则用户网盘里会留一个不知名的文件。
    Failed,
}

// ══════════════════════════ 结果 ══════════════════════════

/// 这一趟要凑备份用的那两份东西。
///
/// 单独成参数（而不是在 `run_expansion` 里现取随机）是为了让编排可测：
/// 测试给固定的名字与内容，就能对请求体逐字断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpansionPlan {
    /// 文件名（旧实现是随机名 + `.png` 一类后缀）。
    pub name: String,
    /// 内容字节。
    pub content: Vec<u8>,
    /// 凑完之后等多久（`0` = 不等；测试用）。
    pub backup_wait_ms: i64,
}

/// 一个账号这一趟的产出。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExpansionResult {
    pub verdict: ExpansionVerdict,
    pub upload: BackupUpload,
    /// 我们造的那个文件名（`Exists` 档也念，但 id 不交）。
    pub upload_name: String,
    /// **我们造的**那个文件的 id；`Exists` / `Blocked` 档为空。
    ///
    /// ⚠ 空 = "没有该由我们清的东西"，**不是**"清过了"。
    pub upload_file_id: Option<String>,
    /// 领取那一发报回来的数。⚠ 实拍**没有**成功样本 ⇒ `None` 是常见路径。
    pub cloud_count: Option<i64>,
    /// 没领 / 判不了的人话依据。
    pub reason: String,
    /// 这一趟**计划**等多久（毫秒）—— 直接来自 [`ExpansionPlan::backup_wait_ms`]，
    /// 也就是账号配置 `backup_wait_secs` 的换算值。
    ///
    /// 进明细（`backupWaitMs`）：这是"账号配置到底有没有生效"唯一不必真等
    /// 20 秒就能读到的读数。
    pub backup_wait_ms: i64,
    /// 失败原文（`ClaimFailed` 或清理失败）。
    pub error: String,
    /// 第一次读到的（判据用它）。
    pub first: TaskExpansion,
    /// 凑完之后重读的那一次；`None` = 没重读。
    pub second: Option<TaskExpansion>,
    /// 结论：凑过一次之后 `cur_month_backup` **仍然是** `Some(false)`。
    pub backup_still_off: bool,
    /// 链尾清理的结局。
    pub cleanup: CleanupVerdict,
    /// 清理失败的原因（单独一格：它与领取失败是**两件事**）。
    pub cleanup_error: String,
}

impl ExpansionResult {
    /// 结论那次用的读数（重读过就用重读的）。
    fn conclusion(&self) -> &TaskExpansion {
        self.second.as_ref().unwrap_or(&self.first)
    }

    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// ⚠ 只有**我们造的**那个文件名进这一句，`authorization` 与预签名 URL 一概不出。
    pub fn summary_line(&self) -> String {
        const HEAD: &str = "任务拓展";
        let body = match self.verdict {
            ExpansionVerdict::Claimed => {
                let mut s = match self.cloud_count {
                    Some(n) => format!("{HEAD}领到 {n} 颗 AI豆"),
                    // ⚠ 上游没报数时**仍然报"领了"** —— 写请求发出去了且上游认了，
                    // 这是实况。旧实现那个 truthy 判据在这里会一句都不念。
                    None => format!("{HEAD}已领（上游没报数）"),
                };
                if self.backup_still_off {
                    s.push_str("；本月备份仍读成 false，翻倍的前置没开");
                }
                s
            }
            ExpansionVerdict::Nothing => format!("{HEAD}没领：{}", self.reason),
            ExpansionVerdict::BackupManual => format!("{HEAD}没领：{}", self.reason),
            ExpansionVerdict::Unreadable => format!("{HEAD}判不了：{}", self.reason),
            ExpansionVerdict::ClaimFailed => {
                format!("{HEAD}失败：领取那一发没成 —— {}", self.error)
            }
            ExpansionVerdict::NotAttempted => format!("{HEAD}没办成"),
        };
        body + &self.readings_tail() + &self.upload_tail()
    }

    /// 读数那一段（判据原值）。**只照抄、不解释**。
    fn readings_tail(&self) -> String {
        let read = self.conclusion();
        let mut parts = Vec::new();
        parts.push(match read.cur_month_backup {
            Some(v) => format!("curMonthBackup={v}"),
            None => "curMonthBackup=缺".to_string(),
        });
        parts.push(match read.cur_month_task_record_count {
            Some(v) => format!("curMonthTaskRecordCount={v}"),
            None => "curMonthTaskRecordCount=缺".to_string(),
        });
        format!("（读数：{}）", parts.join(" "))
    }

    /// 上传那一段。/ 领到了就不念读数（那是领**之前**的数）。
    fn upload_tail(&self) -> String {
        if self.upload == BackupUpload::NotNeeded {
            return String::new();
        }
        let mut parts = vec![self.upload.word().to_string()];
        if !self.upload_name.is_empty() {
            parts.push(self.upload_name.clone());
        }
        if let Some(id) = &self.upload_file_id {
            parts.push(format!("fileId {id}"));
        }
        match self.cleanup {
            CleanupVerdict::Trashed => parts.push("已移进回收站".into()),
            CleanupVerdict::Failed => parts.push(format!("清理失败：{}", self.cleanup_error)),
            CleanupVerdict::NotNeeded => {}
        }
        format!("（{}）", parts.join("，"))
    }

    /// 映射到 [`TaskOutcome`]。
    ///
    /// ⚠ "要人去 App 里开备份"那一档本仓 [`TaskOutcome`] **没有** `ManualRequired`，
    /// 照 `exchange` 的既有处置：落 `Skipped`，把那句话**逐字**放进小结。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            ExpansionVerdict::Claimed => TaskOutcome::success(summary),
            ExpansionVerdict::ClaimFailed => TaskOutcome::failed(summary),
            _ => TaskOutcome::skipped(summary),
        }
    }
}

// ══════════════════════════ 链 ══════════════════════════

/// 跑一条链。
///
/// # 错误与结论的分界
///
/// 只有"**没走到能报结论的那一步**"才回 `Err`：第一次读挂了、入参本身不成立。
/// 凡已经判过 / 已经发过枪的，一律折进 [`ExpansionResult`]。
pub async fn run_expansion(
    ctx: &TaskContext,
    device_id: Option<&str>,
    plan: &ExpansionPlan,
) -> Result<ExpansionResult> {
    // 把"计划等多久"记进结果：它决定下面真睡多久，也是"账号配置有没有生效"
    // 唯一不必真等就能读到的读数。
    let mut result = ExpansionResult {
        backup_wait_ms: plan.backup_wait_ms,
        ..Default::default()
    };

    // ① 读第一遍。它挂了 ⇒ 整趟没有结论可报。
    let first = read_once(ctx, device_id).await?;
    result.first = first.clone();

    // 两格判据都没读到 ⇒ 判不了，**一个请求都不发**。
    if !judged(&first) {
        result.verdict = ExpansionVerdict::Unreadable;
        result.reason = "上游回了 code:0，但 curMonthBackup 与记录数两格都没读到".into();
        report_details(ctx, &result);
        return Ok(result);
    }

    // ② 凑备份（只在**严格**读到 false 时）。
    let mut conclusion = first.clone();
    if should_upload(&first) {
        if device_id.is_none() {
            // 旧实现那一族每一枪都带 `deviceid` ⇒ 缺它就不发
            // （不赌"少一个头上游理不理"，这一族零样本）。
            result.upload = BackupUpload::Blocked;
            result.reason = "换不到 H5 deviceId，而这一族的每一枪都带 deviceid 头 ⇒ 不发".into();
        } else {
            run_upload(ctx, plan, &mut result).await;
        }

        // ⚠ **仍然重读**（连"一枪没发"那一支也一样）：重读是只读、无害，
        // 而结论要用最新那一份 ⇒ 那处偏差只压在"等"上，没有压"读"。
        if result.upload.needs_wait() {
            ctx.check_cancelled()?;
            tokio::time::sleep(std::time::Duration::from_millis(
                plan.backup_wait_ms.max(0) as u64
            ))
            .await;
        }
        let second = read_once(ctx, device_id).await?;
        result.backup_still_off = second.cur_month_backup == Some(false);
        conclusion = second.clone();
        result.second = Some(second);
    }

    // ③ 领（只在结论那次读到记录数 > 0 时）。
    if should_claim(&conclusion) {
        match claim(ctx, &conclusion, &mut result).await {
            Ok(()) => result.verdict = ExpansionVerdict::Claimed,
            Err(e) => {
                result.verdict = ExpansionVerdict::ClaimFailed;
                result.error = e;
            }
        }
    } else {
        result.verdict = if result.backup_still_off {
            ExpansionVerdict::BackupManual
        } else {
            ExpansionVerdict::Nothing
        };
        // ⚠ **追加**而不是覆盖：上传那一支可能已经写下了"为什么一枪没发"
        // （换不到 deviceId / 随机源没接），那一句对用户比这里这句更有用。
        let mut reason = match conclusion.cur_month_task_record_count {
            Some(0) => "本月攒下的任务记录数是 0".to_string(),
            None => "上游没给本月任务记录数，不据此发领取请求".to_string(),
            Some(n) => format!("本月记录数是 {n}"),
        };
        if result.backup_still_off {
            reason += "；凑传过一次备份之后仍未开启，去 App 里开自动备份才能拿到翻倍奖励";
        }
        result.reason = match result.reason.is_empty() {
            true => reason,
            false => format!("{}；{reason}", result.reason),
        };
    }

    // ④ 清理：**只有我们自己造的文件才删**。
    cleanup_uploaded(ctx, &mut result).await;
    // ⑤ 把"本账号本月有没有备份"落盘 —— 供 `app-task` 的月份门读取
    //    （旧 TS `task-expansion.ts:50` 的 `ctx.state.curMonthBackup = false`）。
    persist_month_backup(ctx, &result).await;
    // 明细在这一层报（不是 `Task::run`）：`run_expansion` 才是链本身，
    // 直接调它的调用方（活体探针、单测）也该拿到同一份结构化读数。
    report_details(ctx, &result);
    Ok(result)
}

/// 把本账号**当月**的备份读数落进 `account_state`（旧 TS `task-expansion.ts:50`）。
///
/// # 它写什么
///
/// 写**结论那次**读到的 `curMonthBackup`（`Some(true)` / `Some(false)` 都写，
/// `None` 不写）。旧 TS 只在"凑过一次仍未开"那一支里赋 `false`
/// （`task-expansion.ts:48-51`），从不赋 `true` —— 那是因为 `ctx.state` 是
/// **一次运行内的内存态**，跨运行即丢。本仓的落盘态**跨运行还在**：
/// 只写 `false` 会让"5 号没开备份、6 号用户开了"之后 `app-task` 仍拿着旧的
/// `false` 一直跳到 20 号 ⇒ **必须把 `true` 也写回**，才等价于旧 TS 那种
/// "每趟重新读"。
///
/// # 为什么是"尽力而为"
///
/// 它只影响**下一趟** `app-task` 的月份门：写失败时那道门按"不知道"（不跳）
/// 处理，比让整条 task-expansion 链因一次落盘失败而报错要好。失败打一句 warn。
///
/// # 账号维度
///
/// 落在 `ctx.store.account_state(&ctx.account.id)` 上 —— **按账号 id** 存的一份
/// 状态（`mcloud-core::traits::AccountStore`），键里再带**月份**；绝不做成
/// 进程级全局，那会让 A 账号的"没备份"把 B 账号的 month 任务一起跳掉。
async fn persist_month_backup(ctx: &TaskContext, result: &ExpansionResult) {
    let Some(backup) = result.conclusion().cur_month_backup else {
        return;
    };
    let Some((month, _)) = crate::app_task::beijing_month_day(&*ctx.clock) else {
        return;
    };
    if let Err(e) = crate::app_task::save_month_backup(ctx, &month, backup).await {
        ctx.warn(format!(
            "本月备份读数没能落盘（app-task 的月份门将按'不知道'处理）：{e}"
        ));
    }
}

/// 走上传那三枪（`file/create` → `PUT` → `file/complete`）。
async fn run_upload(ctx: &TaskContext, plan: &ExpansionPlan, result: &mut ExpansionResult) {
    let hash = sha256_hex(&plan.content);
    let input = CloudFileCreateInput {
        // 旧实现逐字 `parentFileId: "/"` —— 传进**根目录**。
        parent_file_id: "/".into(),
        name: plan.name.clone(),
        size: plan.content.len() as i64,
        content_hash: hash.clone(),
        // ⚠ 旧 TS 的默认档就是安卓端（`getFileOpHeader` 的缺省值）。
        channel: CloudChannel::Android,
        op: CloudOpType::Backup,
    };
    let req = match upload::file_create_request(input) {
        Ok(r) => r,
        Err(e) => {
            result.upload = BackupUpload::Blocked;
            result.reason = format!("构造 file/create 失败：{e}");
            return;
        }
    };
    let resp = match ctx.http.request(req).await {
        Ok(r) => r,
        Err(e) => {
            // ⚠ 一枪都没发出去（传输层就挂了）—— 这一档**不等**。
            result.upload = BackupUpload::Failed;
            result.reason = format!("file/create 没发出去：{e}");
            return;
        }
    };
    let created = match upload::parse_file_create(&resp.body) {
        Ok(c) => c,
        Err(e) => {
            // ⚠ create **发出去了**（拿到了响应，只是解不动）⇒ 网盘里可能已经占坑
            // ⇒ 这一档要等（`Failed.needs_wait()` 为 true）。
            result.upload = BackupUpload::Failed;
            result.reason = format!("file/create 响应解不动：{e}");
            return;
        }
    };
    result.upload_name = plan.name.clone();
    match created.owned_by_us() {
        // ⚠⚠ 那个文件**不是我们造的** ⇒ **不交 id**，链尾绝不删它。
        Some(false) => {
            result.upload = BackupUpload::Exists;
            return;
        }
        // ⚠ 判不了（形状变了）⇒ **同样不交 id**。两个方向都错的时候，
        // 只有"漏删一个我们自己传的"是可接受的：删了用户的文件就回不来了
        //（2026-10-04 复审 up-1；判据见 `CloudFileCreated::owned_by_us` ——
        // 2026-10-05 起上游把"没撞同名"回成 `null`，只看 `exist` 会把每一发都拒掉 ✗）。
        None => {
            result.upload = BackupUpload::ExistUnknown;
            result.reason =
                "file/create 的 exist 判不了（响应形状变了）⇒ 判不了那个文件是不是我们造的，按不清理处理"
                    .into();
            return;
        }
        Some(true) => {}
    }
    // 从这一格起，这个 id 是**我们造的** ⇒ 链尾要删。
    result.upload_file_id = created.file_id.clone();
    if created.rapid_upload {
        result.upload = BackupUpload::Rapid;
        return;
    }
    let (Some(file_id), Some(url)) = (created.file_id.clone(), created.upload_url.clone()) else {
        result.upload = BackupUpload::Failed;
        result.reason = "file/create 没给出 fileId 或预签名 uploadUrl".into();
        return;
    };
    // ② PUT 原始字节。
    match ctx
        .http
        .request(upload::upload_put_request(&url, &plan.content))
        .await
    {
        Ok(resp) => {
            if let Err(e) = upload::ensure_put_ok(&resp) {
                result.upload = BackupUpload::Failed;
                result.reason = format!("PUT 没成：{e}");
                return;
            }
        }
        Err(e) => {
            result.upload = BackupUpload::Failed;
            result.reason = format!("PUT 没发出去：{e}");
            return;
        }
    }
    // ③ complete：`uploadId` 为空**照样发**（旧实现 `stringOf(null)` 就是空串）。
    let complete = match upload::file_complete_request(
        &file_id,
        created.upload_id.as_deref().unwrap_or(""),
        &hash,
        CloudChannel::Android,
        CloudOpType::Backup,
    ) {
        Ok(r) => r,
        Err(e) => {
            result.upload = BackupUpload::Failed;
            result.reason = format!("构造 file/complete 失败：{e}");
            return;
        }
    };
    match ctx.http.request(complete).await {
        Ok(resp) => match upload::parse_file_complete(&resp.body) {
            Ok(()) => result.upload = BackupUpload::Done,
            Err(e) => {
                result.upload = BackupUpload::Failed;
                result.reason = format!("file/complete 没成：{e}");
            }
        },
        Err(e) => {
            result.upload = BackupUpload::Failed;
            result.reason = format!("file/complete 没发出去：{e}");
        }
    }
}

/// 领那一发。
async fn claim(
    ctx: &TaskContext,
    read: &TaskExpansion,
    result: &mut ExpansionResult,
) -> std::result::Result<(), String> {
    // `acceptDate` 来自**同一条**响应；没给就发裸 GET（与真实客户端一致）。
    let req = proto::receive_task_expansion_request(read.accept_date.as_deref());
    let body = get_body(ctx, req, "领取任务拓展奖励")
        .await
        .map_err(|e| e.to_string())?;
    let env = SignInEnvelope::parse(&body).map_err(|e| e.to_string())?;
    env.ensure_ok("领取任务拓展奖励")
        .map_err(|e| e.to_string())?;
    // ⚠ `result` 是**裸数字**（同族实拍 `result: 100`），也可能整个缺席或是个对象。
    // 只存不判：**没报数时仍然报"领了"**。
    result.cloud_count = env.result.as_ref().and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::Object(o) => o.get("cloudCount").and_then(Value::as_i64),
        _ => None,
    });
    Ok(())
}

/// 链尾：把我们自己传上去的那个文件移进回收站。
///
/// ⚠ **只删 `upload_file_id` 有值的那些** —— 撞同名那一档留空，
/// 那是用户本来就有的文件。
async fn cleanup_uploaded(ctx: &TaskContext, result: &mut ExpansionResult) {
    let Some(file_id) = result.upload_file_id.clone() else {
        result.cleanup = CleanupVerdict::NotNeeded;
        return;
    };
    let req = match cloud_trash::batch_trash_request(std::slice::from_ref(&file_id)) {
        Ok(r) => r,
        Err(e) => {
            result.cleanup = CleanupVerdict::Failed;
            result.cleanup_error = e.to_string();
            return;
        }
    };
    match ctx.http.request(req).await {
        Ok(resp) => match cloud_trash::parse_batch_trash(&resp.body) {
            Ok(()) => result.cleanup = CleanupVerdict::Trashed,
            Err(e) => {
                result.cleanup = CleanupVerdict::Failed;
                result.cleanup_error = e.to_string();
            }
        },
        Err(e) => {
            result.cleanup = CleanupVerdict::Failed;
            result.cleanup_error = e.to_string();
        }
    }
}

/// 内容哈希（`create` 与 `complete` 必须同一个串）。
fn sha256_hex(content: &[u8]) -> String {
    mcloud_crypto::sha256_hex_bytes(content)
}

// ══════════════════════════ 任务 ══════════════════════════

/// 跑一次任务拓展（**会写**：往用户网盘传一个文件并刷备份状态）。
///
/// # 等待时长**不写死在这里**
///
/// 它来自**账号配置** `backup_wait_secs`（默认 20 秒）—— 换算见
/// [`backup_wait_ms`]。原先那个写死 20 秒的字段已删：配置里那个旋钮此前
/// 声明了却没人读，等于摆设。
pub struct TaskExpansionTask;

impl Default for TaskExpansionTask {
    fn default() -> Self {
        Self
    }
}

impl TaskExpansionTask {
    pub fn new() -> Self {
        Self
    }
}

/// 造这一趟要传的那个文件（随机名 + 随机内容）。
///
/// ⚠ 名字与内容都随机：撞同名走 `Exists`（不删）、撞内容哈希走秒传，
/// 两者都会让链路少发几枪 —— 而这条链要验的正是"三枪都发出去"。
pub(crate) fn random_plan(
    rng: &mut impl mcloud_crypto::Random,
    backup_wait_ms: i64,
) -> ExpansionPlan {
    let mut suffix = [0u8; 8];
    rng.fill_bytes(&mut suffix);
    let mut hex = String::with_capacity(16);
    for b in suffix {
        hex.push_str(&format!("{b:02x}"));
    }
    // 内容是随机字节 —— 别用全 0：那会撞上"文件内容哈希已知"的秒传档。
    let mut content = vec![0u8; 256];
    rng.fill_bytes(&mut content);
    ExpansionPlan {
        name: format!("mcloud-task-expansion-{hex}.bin"),
        content,
        backup_wait_ms,
    }
}

/// 同 [`random_plan`]，但**后缀可指定** —— 旧实现的 `uploadRandomFile({ ext })` ✓。
///
/// ⚠ 为什么值这一格：615 那一族（`ai-tasks.ts:205` 的 `upload10PictureTask`）
/// 刻意传 **`.jpg`**，而别处都不带 `ext`（走缺省 `.bin` ✓）。上游到底看不看这个后缀
/// **未验证**，但旧实现是**分开写的** ⇒ 照抄分开，不替它统一 ✗。
pub(crate) fn random_plan_with_ext(
    rng: &mut impl mcloud_crypto::Random,
    backup_wait_ms: i64,
    ext: &str,
) -> ExpansionPlan {
    let mut plan = random_plan(rng, backup_wait_ms);
    // 名字形如 `mcloud-task-expansion-<16 hex>.bin`（hex 里没有 `.` ✓）
    // ⇒ 从**最后一个** `.` 截掉旧后缀、再把新的接上（不是再拼一个 ✗）。
    if let Some(dot) = plan.name.rfind('.') {
        plan.name.truncate(dot);
    }
    plan.name.push_str(ext);
    plan
}

#[async_trait]
impl Task for TaskExpansionTask {
    fn name(&self) -> &'static str {
        "task-expansion"
    }
    fn title(&self) -> &'static str {
        "任务拓展"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::App
    }
    fn description(&self) -> &'static str {
        "读本月备份状态与任务记录；未开备份会往网盘传一个文件凑数，攒够记录就领掉（传的那个文件跑完自己删）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "读取任务拓展状态");
        // ⚠ `deviceId` 这一族**每枪都要**：取不到就不发（不赌"少一个头上游理不理"）。
        //
        // ⚠ **显式传，不指望发送层注入**：`ReqwestClient` 会按 profile 注入一枚
        // `deviceid`，但任务那个 profile 的 `device_id` 是 `None`（见 `builder.rs`
        // 那条"由 SmidClient 懒加载"的注释）⇒ 不在这里传，这一族就是**少一个头**。
        // 同族既有写法见 `signin.rs` 那句 `ctx.account.device_id.as_str()`。
        let account_device = ctx.account.device_id.as_str();
        let device_id: Option<&str> = (!account_device.is_empty()).then_some(account_device);
        // ⚠ 等待时长随**账号配置**走，不是写死的 20 秒。
        let plan = random_plan(
            &mut mcloud_crypto::OsRandom,
            backup_wait_ms(ctx.account.settings.backup_wait_secs),
        );

        let result = run_expansion(ctx, device_id, &plan).await?;
        match result.verdict {
            ExpansionVerdict::Claimed => ctx.info(result.summary_line()),
            ExpansionVerdict::NotAttempted => {}
            _ => ctx.info(result.summary_line()),
        }
        Ok(result.task_outcome())
    }
}

/// 把这一趟的读数交给明细通道。
fn report_details(ctx: &TaskContext, result: &ExpansionResult) {
    let slug = match result.verdict {
        ExpansionVerdict::NotAttempted => "not-attempted",
        ExpansionVerdict::Claimed => "claimed",
        ExpansionVerdict::Nothing => "nothing",
        ExpansionVerdict::BackupManual => "backup-manual",
        ExpansionVerdict::Unreadable => "unreadable",
        ExpansionVerdict::ClaimFailed => "claim-failed",
    };
    let readings = |read: &TaskExpansion| {
        serde_json::json!({
            "curMonthBackup": read.cur_month_backup,
            "preMonthBackup": read.pre_month_backup,
            "curMonthBackupTaskAccept": read.cur_month_backup_task_accept,
            "curMonthTaskRecordCount": read.cur_month_task_record_count,
            "nextMonthTaskRecordCount": read.next_month_task_record_count,
            "acceptDate": read.accept_date,
        })
    };
    ctx.details(serde_json::json!({
        "kind": "task-expansion",
        "verdict": slug,
        "backupWaitMs": result.backup_wait_ms,
        "reason": result.reason,
        "error": result.error,
        "cloudCount": result.cloud_count,
        "upload": result.upload.word(),
        "uploadName": result.upload_name,
        "uploadFileId": result.upload_file_id,
        "backupStillOff": result.backup_still_off,
        "cleanup": match result.cleanup {
            CleanupVerdict::NotNeeded => "not-needed",
            CleanupVerdict::Trashed => "trashed",
            CleanupVerdict::Failed => "failed",
        },
        "cleanupError": result.cleanup_error,
        "first": readings(&result.first),
        "second": result.second.as_ref().map(readings),
    }));
}

#[cfg(test)]
mod tests;
