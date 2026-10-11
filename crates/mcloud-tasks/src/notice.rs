//! 公告任务（`notice`，界面那页叫「开启通知领AI豆」）：
//! 查两个槽的状态 → **该领才领** → **复查一次**拿到账读数。
//!
//! 请求构造与解析在 [`mcloud_upstream::app_notice`]（纯函数、无 IO）；
//! 本模块只负责**编排**：查状态、判分支、把结论说成一句人话。
//!
//! # 为什么它是独立一条链，而不是挂在签到后面顺手跑
//!
//! 它是**另一条链**：主机带端口（`:7071`）、路径前缀是 `openemailsms-service`
//! 而不是 `signin`，判据字段一个都不在 `infoV3` 里。挂在签到后面跑，
//! 签到那一条链就多两个出网请求，而这两条的失败互不相干。
//!
//! # 证据分成两半，别混着读
//!
//! * **`getTaskInfo` 有实拍**：路径、两个槽的四个字段名、实拍那两条读数
//!   都是逐字看到的（见协议层文件头）。
//! * **`reward` 只实拍到"成功那一枪的形状"**：响应是 `{"code":0,"msg":"success"}`，
//!   **连 `result` 都没有** ⇒ 它报不出到账数，"到底领没领上"只能靠**复查**。
//! * **零样本**：非 0 码的响应、**"通知未开启"态的响应**、`day` / `canReceive`
//!   两格、`taskListV2` 那一环 —— 一个样本都没有。
//!
//! ⇒ 于是本模块的判据分两种语气：**判据字段的存在**是实拍，
//! 而"**什么值该做什么**"这一层**仍是旧项目的口径**（它说 `cloudStatus == 1`
//! 且 `taskStatus == 3` 才发）—— 实拍那天**恰好**命中了那个值（紧接着就发了
//! `reward`），但方向没有被证实，见 [`plan_notice`] 那一大段。
//!
//! # ⚠ 判定的口径写在协议层文件头，这里只留一句话版本
//!
//! 旧 TS 说链路是 `getTaskInfo → reward → taskListV2(clientVersion 11.2.1)
//! 找 id 1021 → reportAppNoticeStatus?status=1`，而**实拍里没有 `taskListV2`**：
//! 看到的是 `getTaskInfo ×2 → reward`，另一份抓包里另有 `receiveTask?taskId=1021`。
//!
//! 处置：**按实拍最强的那条裁** —— 判据只用 `getTaskInfo` 的四个字段，
//! 入账口用 `reward`。另一条入账口（`receive_task_request`）与打卡口
//! （`signin_task_click_request`）已在协议层实现并测过，**本模块不选它们**
//!（选哪一个由编排层/接线拍板，这里不替它选）。
//!
//! # ✅ 2026-10-06：**邮箱那一半**接上了（用户口径「跟参考」）
//!
//! 这一族其实有**两半**：短信那一半（上面那条链）与**邮箱那一半**
//!（旧 TS `packages/core/src/tasks/notice.ts:32-54`，任务 **1021**
//!「连续31天开启邮箱APP通知」）。后者是：查邮箱月表 → 找到 1021 →
//! `day > 0 && canReceive === 1` 才 `reportAppNoticeStatus?status=1`。
//! ⇒ [`claim_email_notice`] 逐字照它落码，`run` 里两半**各自 best-effort**
//!（一半挂了不改另一半的结论 ✓）。
//!
//! ⚠ 上报口（`report_app_notice_status_request`）**从"不接链"变成"接链"**；
//! `receive_task_request` 仍不接（旧 TS 没发过它，实拍那一枪在另一份抓包里）。
//!
//! # ⚠⚠ 那个"哪个 `cloudStatus` 是未领"的映射是**猜的**
//!
//! 见 [`plan_notice`] —— 这一条最要紧，别读成"从数据里读出来的"。
//!
//! # ⚠ 票来自**另一页**
//!
//! 这一页自己的换票链路**没被抓到**（抓包里 `referer` / `jwttoken` 全被脱敏，
//! 也没有前置的 `tyrzLogin`）⇒ 走**公共来源**：本模块经 `H5Client` 拿的是
//! **签到页那张票**（`SessionRecipe::SignInPage`）。
//!
//! ⚠ "跨主机带 referer（票的 referer 指向 `m.mcloud.139.com`，请求打在
//! `caiyun.feixin.10086.cn:7071`）上游认不认"**未测试**。
//! 这是**临时态**：哪天抓到这一页自己的换票三枪，就改用特有取法。
//!
//! # ⚠ 已知结构差之一：`TaskOutcome` 没有 `ManualRequired`
//!
//! 参考实现把"得有人去 App 里开通知"那一档记成 `OutcomeManualRequired` ——
//! 一条带**具体动作**的人工出口（推送与历史列表都能看见"要人去做什么"）。
//! 本仓 [`TaskOutcome`] 没有这一档 ⇒ 照 `mcloud_tasks::exchange` 的既有处置：
//! 那句说明**逐字**放进小结（[`NOTICE_MANUAL_INSTRUCTION`]），档位落
//! [`TaskOutcome::Skipped`]。**这是已知的结构差，不是等价物** ——
//! `Skipped` 不会把"要人去做"这件事推给用户，只能在小结里读到。
//!
//! ⚠ 而这一档**今天根本不可达**：[`notice_needs_manual`] 恒 `false`
//! （"通知未开启"态的响应**零样本**）。保留它是因为判据补上时
//! **只该改那一个函数**，映射本身不该还得先测一遍。
//!
//! # ⚠ 已知结构差之二：「已领 + 错误同时交回」的落点
//!
//! 参考实现的 `ClaimNotice` 返回 `(NoticeOutcome, *core.Error)` **一对**：
//! "`reward` 发成了、复查挂了"那一趟，结论是**已领**，而那条错误**同时**交回
//! 编排层（那一趟的实况是"没走完"，不是"没领到"）。
//!
//! 本仓 [`TaskResult`] 只有一个 `Result<TaskOutcome>` 槽，装不下这一对 ⇒
//! 两件事在**同一个值**里交回：[`claim_notice`] 的 `Ok(NoticeOutcome)` 里
//! `kind == Claimed`（已领）**且** `recheck_failed` / `error` 带上那条错误；
//! `run` 把它写进小结、结构化明细，并单发一条 `warn` 日志。
//!
//! ⚠ **不许**在这一档 `return Err`：runner 会把整趟记成 `Failed`，
//! 用户看到"失败"就会**再去点一次** —— 而 `reward` 幂等与否**零样本**。
//! 这一档永远走 `Ok(Success)`。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_notice as proto;
use mcloud_upstream::app_notice::{NoticeEmailSlot, NoticeTaskInfo, NoticeTaskSlot};
use mcloud_upstream::app_signin::SignInEnvelope;

/// 「得有人去 App 里点一下」那句说明 —— **参考实现的原文**（一处用词差异见下）。
///
/// 参考实现把它写成 `core.NewRunReportManualRequired(...)` 的 `instruction`，
/// 而本仓 [`TaskOutcome`] 没有那一档（见文件头）⇒ 这句说明唯一的落点是小结。
///
/// ⚠ 与参考实现的**一处刻意差异（只此一处，别的字一字未动）**：原文那句写的是
/// "公告任务的**云朵**不会入账"，而 `docs/conventions.md` §5.5 规定
/// "我们自己写给用户看的文本一律用 AI豆"（云朵只留在**引用**上游/旧项目原文的地方）。
/// 这句是**我们写给用户看的**，所以这里用 AI豆。
pub const NOTICE_MANUAL_INSTRUCTION: &str = "要去移动云盘 App 里开启「邮箱通知 / 短信通知」，\
这两条没开时公告任务的 AI豆不会入账；本工具代劳不了这一步";

/// 发一个请求并解成信封。
async fn call(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<SignInEnvelope> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    SignInEnvelope::parse(body)
}

/// 只查两个槽的读数、不领（给"看一眼"那种用法留着，也是复查那一枪）。
pub async fn query_notice_task_info(ctx: &TaskContext) -> Result<NoticeTaskInfo> {
    let env = call(ctx, proto::notice_task_info_request(), "查询公告任务状态").await?;
    proto::parse_notice_task_info(&env)
}

/// 只读一次 `getTaskInfo`（**不发** `reward`、**不**上报状态 ✓）—— 给界面的只读路由用 ✓。
///
/// ⚠ 与 [`query_notice_task_info`] 的差别**只在入口**：那条收 `TaskContext`（任务链里 ✓），
/// 这条收一个裸 `HttpClient`（WebUI 路由里 ✓）。判定与副作用一概不碰 ✓
///（这一族**唯一**会产生副作用的两个口是 `reward` 与 `reportAppNoticeStatus` ✓，都不在这里 ✓）。
///
/// ⚠ 它**给不出**"进行到第几天"：实拍两个槽的四个字段是
/// `cloudStatus` / `sum` / `taskStatus`（+ 邮箱那个槽多一个 `progress` = **分母 31** ✓）
/// ⇒ 分子（`day`）只能来自任务条目 ✓（见 `app_task_list_v2` 的 `button_out_day` ✓）。
pub async fn read_task_info(http: &dyn mcloud_core::HttpClient) -> Result<NoticeTaskInfo> {
    let resp = http.request(proto::notice_task_info_request()).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, "查询通知任务状态")?;
    proto::parse_notice_task_info(&SignInEnvelope::parse(body)?)
}

// ══════════════════════════ 判定 ══════════════════════════

/// [`plan_notice`] 的结论档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticePlan {
    /// **该领**：至少一个槽的 `cloudStatus` 读成"未领"、而它的 `taskStatus`
    /// 读成"已完成" ⇒ 发 `reward`。
    ///
    /// ⚠ 这条映射是**待验证**的，集中在一处的实现见 [`notice_slot_claimable`]。
    Reward,
    /// **得有人去 App 里点一下**（开邮箱 / 短信通知）才会有的那一档。
    ///
    /// ⚠⚠ 这一档**今天到不了**（[`notice_needs_manual`] 恒 `false`）。
    /// 保留它是因为结果层的映射必须一直通着，抓到"未开启"那条例文之后
    /// **只改一个函数**。
    Manual,
    /// **没得领**：能判的槽里没有一个读成可领。
    ///
    /// 它算**成功**：这一趟的目标是"该领的时候领掉"，来一趟确认"现在不该领"
    /// 就是达成了。
    Nothing,
    /// **判不了**：没有任何一个槽把两个判据字段都给全。
    ///
    /// 处置是**一个请求都不发**：当成"该领"会白发一次写请求（而 `reward`
    /// 幂等与否**零样本**），当成"不该领"会漏领。两边代价不对称，
    /// 但都不如"什么都不发、把判不了说清楚"。
    Unknown,
}

/// 待验证的映射**只写在这两个常量与那一个函数里**。
///
/// # ⚠⚠ 这是**猜的对应关系**
///
/// 本仓库关于它的实拍只有**两条**读数，而且两条互相分不清方向：
///
/// ```text
/// taskOneInfo{cloudStatus: 2, sum: 200,              taskStatus: 3}
/// taskTwoInfo{cloudStatus: 1, sum: 100, progress: 31, taskStatus: 3}
/// ```
///
/// 两条的 `taskStatus` **相同**而 `cloudStatus` **不同** ⇒ `taskStatus` 不是
/// "这个槽领没领"的读数，`cloudStatus` 才是。但**哪个值是"未领"**实拍定不出来。
///
/// 唯一能加分的一条旁证：`taskTwoInfo.sum == 100`，而另一场抓包里
/// `receiveTask?taskId=1021` 回的正是 `result: 100` ⇒ "槽二当时还没入账"
/// 与"`cloudStatus == 1` 是未领"自洽。**旁证不是判据**（两件事跨了两份抓包、
/// 两次会话）。
///
/// ⚠ 参考实现另外记了两趟真账号读数（登记成 R1）：四槽读到
/// `cloudStatus ∈ {0, 2}` 配 `taskStatus ∈ {0, 1, 2, 3}`（`sum` 两槽恒 200 / 100），
/// **没有一槽给过 1** ⇒ "哪个值是未领"今天仍**没有正面样本**。
/// 而 `0` 既不是 1 也不是 2，语义**未验证** ⇒ 它照旧落 [`NoticePlan::Nothing`]
/// （不发写请求），但小结**不许**把这一档念成"已入账"。
///
/// ⚠ **反向映射也照样让实拍那一条走 `Reward`**：两个值里总有一个被读成"未领"，
/// 而实拍里真实客户端**紧接着就发了 `reward`** ⇒ "该不该发"这一半是稳的，
/// 不稳的是"两个槽都已领"那一半：方向猜反的表现是**该领时说不领**（漏领），
/// 而不是"多发一次写请求"。
///
/// **怎么定**：拿一个从没开过通知的号看两槽读数 → 开启 → 再读一次；
/// 或开一次、`reward` 一次、再读一次。**两个方向只能这样分开**。
pub const NOTICE_CLOUD_STATUS_UNCLAIMED: i64 = 1;

/// 读作"这一槽**已入账**"。⚠ 同 [`NOTICE_CLOUD_STATUS_UNCLAIMED`]：**猜的**。
pub const NOTICE_CLOUD_STATUS_CLAIMED: i64 = 2;

/// 读作"任务本身**已完成**"（实拍两槽都是 `3`）。
pub const NOTICE_TASK_STATUS_DONE: i64 = 3;

/// 报告"这一槽该领"。
///
/// ⚠ 两个判据字段**缺任何一个**都回 `false`（= 这一槽不参与"该领"的判断），
/// 而不是参与判断后落成"不该领"：`None` 与 `Some(0)` 不是一回事，
/// 把"上游没说"当成"说了没完成"会让这条链**静默漏领**。
pub fn notice_slot_claimable(slot: Option<&NoticeTaskSlot>) -> bool {
    let Some(slot) = slot else {
        return false;
    };
    let (Some(cloud), Some(status)) = (slot.cloud_status, slot.task_status) else {
        return false;
    };
    cloud == NOTICE_CLOUD_STATUS_UNCLAIMED && status == NOTICE_TASK_STATUS_DONE
}

/// 报告"这一槽**能不能判**"（两个判据字段都给全了）。
pub fn notice_slot_judged(slot: Option<&NoticeTaskSlot>) -> bool {
    let Some(slot) = slot else {
        return false;
    };
    slot.cloud_status.is_some() && slot.task_status.is_some()
}

/// "得有人去 App 里开通知"的**唯一**入口。
///
/// # ⚠⚠ 待验证，且今天恒回 `false`
///
/// 这一档因此**不可达**，判据本身**未被断言** —— 造不出反例的分支要就地声明，
/// 不许因"没红过"当作不重要。
///
/// 为什么不硬凑一个条件：
///
/// * 实拍的两个字段读不出"通知开没开"：`cloud_status` 实拍只见过 `1` / `2`
///   （另有两趟真账号读到 `0`），而 `1` / `2` 与"领没领"已经绑在一起用了
///   （见 [`notice_slot_claimable`]）—— 同一个字段同时当"开没开"与"领没领"用
///   是自相矛盾的；
/// * 旧 TS 的判据说是 `day` / `canReceive` 两个字段，**一条实拍都没有**，
///   连它们在哪个响应里都不确定；
/// * **"未开启"态的响应零样本** —— 上游很可能是回一个**非 0 码**，
///   而那种形态今天会走 `ensure_ok` 直接报错那条路，**压根到不了判定层**。
///   要核它得抓一条"从没开过通知的账号打 `getTaskInfo`"的原文。
///
/// 拿到那条例文之后：**只改这一个函数**，结果层不用动。
pub fn notice_needs_manual(_info: &NoticeTaskInfo) -> bool {
    false
}

/// 按 `getTaskInfo` 的两个槽判 [`NoticePlan`]。**纯函数**，不碰网络。
///
/// ⚠ 顺序有意义：`Manual` 排在 `Reward` 前面（真开着通知但已领过，
/// 也不该再发一次写请求），而 `Unknown` 是兜底 —— "什么都没给"必须落 `Unknown`，
/// 不能落 `Nothing`（落 `Nothing` 会让小结说成"现在没得领"，
/// 那是一句**没根据的肯定话**）。
pub fn plan_notice(info: &NoticeTaskInfo) -> NoticePlan {
    if notice_needs_manual(info) {
        return NoticePlan::Manual;
    }
    let slots = [info.task_one_info.as_ref(), info.task_two_info.as_ref()];
    let mut judged = 0;
    for slot in slots {
        if notice_slot_claimable(slot) {
            return NoticePlan::Reward;
        }
        if notice_slot_judged(slot) {
            judged += 1;
        }
    }
    if judged > 0 {
        return NoticePlan::Nothing;
    }
    NoticePlan::Unknown
}

// ══════════════════════════ 链 ══════════════════════════

/// 链路跑完的结局档位。
///
/// # ⚠ 零值刻意是 `None`（"没走到能报结论的那一步"）
///
/// 查判据就挂了那一趟，`run` 交回的是 `Err`、**不构造**这个结构体 ——
/// 但 `NoticeOutcome::default()` 仍需要一个**不说假话**的零值：
/// 它若读成 `Claimed`，一句"已领"就会在**一个请求都没发**的那趟上被念出来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeOutcomeKind {
    /// "没走到能报结论的那一步"（查判据就挂了）—— 只在**零值**上出现。
    #[default]
    None,
    /// `reward` 发出去且上游认了。
    ///
    /// 到账读数在 [`NoticeOutcome::recheck`] 里 —— ⚠ **不在** `gained` 里，
    /// 因为实拍那条 `reward` 响应**连 `result` 都没有**。
    Claimed,
    /// 没得领 —— **没发写请求**。
    Nothing,
    /// 得有人去 App 里开通知 —— **没发写请求**。⚠ 今天不可达
    /// （[`notice_needs_manual`] 恒 `false`）。
    Manual,
    /// 判不了 —— **一个请求都不发**。
    Unknown,
    /// `reward` 发了但失败（含"发了、上游没回可信结论"）。
    ///
    /// ⚠ 它**不是** `Err`：这一趟的实况是"发过、没成"，
    /// 报成错误会把"发过"这件事抹掉。
    Failed,
}

/// [`claim_notice`] 的产出。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticeOutcome {
    /// 档位。
    pub kind: NoticeOutcomeKind,
    /// **发写请求之前**那份 `getTaskInfo`（判据用它算的）。
    pub before: NoticeTaskInfo,
    /// **发完 `reward` 之后**再查一次的那份；`None` = 没复查（没发）
    /// 或复查挂了（那时看 [`Self::recheck_failed`]）。
    ///
    /// ⚠ 它是这一条**唯一的"到账读数"**：`reward` 报不出数。
    pub recheck: Option<NoticeTaskInfo>,
    /// `reward` 报出的数。实拍**恒 `None`**（响应里没有 `result`）。
    /// 留这一格是"哪天上游给了别丢"，不是"这里应该有数"。
    pub gained: Option<i64>,
    /// ⚠ 标的是"`reward` 发成了、但**复查挂了**"这一种叠加态：
    /// 结论是**已领**（实况），而那条错误**同时**在 [`Self::error`] 里交回。
    ///
    /// 不留这一位，两种"没数"就分不开：一种是真的没读到、一种本来就不该有数。
    pub recheck_failed: bool,
    /// [`NoticeOutcomeKind::Failed`] 的失败原因，或复查那条错误原话。
    pub error: String,
}

/// 跑这一族的链：`getTaskInfo` → 判 → （该领才）`reward` → **复查** `getTaskInfo`。
///
/// # 为什么领完还要再查一次
///
/// `reward` 的实拍响应是 `{"code":0,"msg":"success"}` —— **一个数都没有**。
/// 只看它，"到底领没领上"就只剩"上游说了句 success"。复查那一份里
/// `cloudStatus` 会不会从 1 变 2、什么时候变，**未验证**；但它是眼下**唯一**
/// 能对拍的读数，也是复查存在的全部理由。
///
/// # 三种失败的走法不同（这一处最容易写错）
///
/// * **查判据失败** ⇒ `Err`：整趟**没有结论可报**（零值那个 `None` 档就是它）。
/// * **`reward` 失败** ⇒ `Ok` + [`NoticeOutcomeKind::Failed`]，**不**交错误：
///   请求发过，这是实况。
/// * **复查失败** ⇒ `Ok` + [`NoticeOutcomeKind::Claimed`] **且**
///   `recheck_failed = true`、`error` 带上原话 —— "领成功了但复查挂了"
///   **不许**写成失败（那会让人以为没领上、再去点一次，而 `reward` 幂等与否
///   **零样本**）。⚠ 本仓没有第二个返回值装这条错误（见文件头"结构差之二"），
///   所以它与"已领"装在**同一个值**里交回。
pub async fn claim_notice(ctx: &TaskContext) -> Result<NoticeOutcome> {
    ctx.progress(1, 3, "查询公告任务状态");
    let before = query_notice_task_info(ctx).await?;

    match plan_notice(&before) {
        NoticePlan::Manual => {
            return Ok(NoticeOutcome {
                kind: NoticeOutcomeKind::Manual,
                before,
                ..Default::default()
            })
        }
        NoticePlan::Nothing => {
            return Ok(NoticeOutcome {
                kind: NoticeOutcomeKind::Nothing,
                before,
                ..Default::default()
            })
        }
        NoticePlan::Unknown => {
            return Ok(NoticeOutcome {
                kind: NoticeOutcomeKind::Unknown,
                before,
                ..Default::default()
            })
        }
        NoticePlan::Reward => {}
    }

    ctx.check_cancelled()?;
    ctx.progress(2, 3, "领取公告任务奖励");
    // ⚠ 这一枪是**写**（GET 但会记账），且**不重试** ——
    //    "上游幂等与否零样本"，重试等于赌。
    let reward = match call(ctx, proto::notice_reward_request(), "领取公告任务奖励").await {
        Ok(env) => match proto::parse_notice_reward(&env) {
            Ok(r) => r,
            Err(e) => {
                // 写请求发出去了、但没读到可信结论 ⇒ 报失败，**不报跳过**。
                return Ok(NoticeOutcome {
                    kind: NoticeOutcomeKind::Failed,
                    before,
                    error: e.to_string(),
                    ..Default::default()
                });
            }
        },
        Err(e) => {
            return Ok(NoticeOutcome {
                kind: NoticeOutcomeKind::Failed,
                before,
                error: e.to_string(),
                ..Default::default()
            })
        }
    };

    ctx.progress(3, 3, "复查公告任务状态");
    match query_notice_task_info(ctx).await {
        Ok(after) => Ok(NoticeOutcome {
            kind: NoticeOutcomeKind::Claimed,
            before,
            recheck: Some(after),
            gained: reward.cloud_count,
            ..Default::default()
        }),
        // ⚠ 交回 `Ok`（结论是"已领"）**同时**把那条例外带上，见函数头第三条。
        Err(e) => Ok(NoticeOutcome {
            kind: NoticeOutcomeKind::Claimed,
            before,
            // 复查没跑成 ⇒ 这一格刻意留 `None`（`recheck_failed` 说明为什么）。
            recheck: None,
            gained: reward.cloud_count,
            recheck_failed: true,
            error: e.to_string(),
        }),
    }
}

// ══════════════════════════ 跑一个账号 ══════════════════════════

/// 一次公告任务的结局档位（[`NoticeOutcomeKind`] 在编排层的投影）。
///
/// ⚠ 名字不叫 `NoticeOutcome`：那是链路层那个**带读数**的结构，
/// 两件事共用一个名字会让人读 [`NoticeResult`] 时分不清"档位"还是"链路产出"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeVerdict {
    /// 领到了。
    Done,
    /// 没得领 —— 算成功。
    Nothing,
    /// 得有人去 App 里开通知。⚠ 今天不可达。
    Manual,
    /// 判不了（**没发请求**）。与 [`Nothing`](Self::Nothing) 分开是因为**说法**不同：
    /// 一句是有根据的"现在不该领"，另一句是"我们没根据"。
    Unknown,
    /// `reward` 发了但失败；或查判据就挂了（那时 `run` 交回的是 `Err`，
    /// 这一档只是**零值**的落点，见下）。
    ///
    /// ⚠ 这个字段**刻意是零值**：`NoticeResult` 有多条提前 `return` 的路径，
    /// 零值若是 [`Done`](Self::Done)，一句"已领"就会在**一个请求都没发**的
    /// 那趟上被念出来。参考实现那一侧正好有这个坑（它的 `NoticeStatus` 零值
    /// 就是"已领"），别照抄。
    #[default]
    Failed,
}

/// 一个账号这一趟的产出。
///
/// # 为什么这里只放**读数**、不放"领到多少颗"
///
/// 上游那一口 `reward` 什么都不报（实拍没有 `result`），而两个槽的
/// `cloudStatus` / `sum` 语义**未验证**（见 [`plan_notice`] 那一大段）。
/// 于是"进账 N 颗 AI豆"这句话眼下**没有来源** —— 与其编一个（比如拿 `sum` 顶 N），
/// 不如把两份读数原样交出去，让人自己看得见。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticeResult {
    /// 档位。
    pub verdict: NoticeVerdict,
    /// `reward` 报出的数，实拍**恒 `None`**（≠ 0，也 ≠"领到 0 颗"）。
    pub gained: Option<i64>,
    /// **结论对应那一次**的槽读数：走过 `reward` 的那一趟（[`Done`](NoticeVerdict::Done)）
    /// 是**复查之后**的；没发写请求的那几档是判据那一次的。
    ///
    /// `None` = 上游没给这一槽，或**复查没跑成**（见 [`Self::recheck_failed`]）。
    pub task_one: Option<NoticeTaskSlot>,
    pub task_two: Option<NoticeTaskSlot>,
    /// ⚠ 标的是"`reward` 发成了、但**复查挂了**"这一种叠加态：
    /// 结论是**已领**（实况），那条错误同时交回（在本结构里就是 [`Self::error`]）。
    ///
    /// 不留这一位，两种"没数"就分不开：一种是真的没读到、一种本来就不该有数。
    pub recheck_failed: bool,
    /// [`Nothing`](NoticeVerdict::Nothing) / [`Manual`](NoticeVerdict::Manual)
    /// / [`Unknown`](NoticeVerdict::Unknown) 的人话依据。
    pub reason: String,
    /// [`Failed`](NoticeVerdict::Failed) 的失败原因，或复查那句失败的原话。
    pub error: String,
}

/// 把链路结局投影成跑一趟的结论。
///
/// ⚠ 单独成一个函数，是因为 `run` 有**两条**出口（错误那条也要先投影一次），
/// 内联的话两条出口各写一遍字段，改一档只会改到一半。
pub fn project(outcome: &NoticeOutcome) -> NoticeResult {
    // 没发写请求的那几档（含零值那个 `None`）：读数就是判据那一次的。
    let from_before = |kind, reason: &str| NoticeResult {
        verdict: kind,
        gained: outcome.gained,
        task_one: outcome.before.task_one_info.clone(),
        task_two: outcome.before.task_two_info.clone(),
        recheck_failed: false,
        reason: reason.to_string(),
        error: String::new(),
    };

    match outcome.kind {
        NoticeOutcomeKind::Claimed => NoticeResult {
            verdict: NoticeVerdict::Done,
            gained: outcome.gained,
            // ⚠ 只认 [`NoticeOutcome::recheck`]：**不许**回落到 `before`。
            // 拿"领之前"那份读数去填"复查之后"这两个字段，表现是小结念出
            // "复查：cloudStatus=1"，而它其实是领之前的数 —— 那正是复查要看的东西，
            // 念错了比念不出来更糟（会把"到底变没变"这个唯一的问题答成假的）。
            task_one: outcome
                .recheck
                .as_ref()
                .and_then(|r| r.task_one_info.clone()),
            task_two: outcome
                .recheck
                .as_ref()
                .and_then(|r| r.task_two_info.clone()),
            recheck_failed: outcome.recheck_failed,
            reason: String::new(),
            error: outcome.error.clone(),
        },
        NoticeOutcomeKind::Nothing => {
            // ⚠ 这句**不许**写成"能判的槽都读成已入账"（参考实现的原话如此，
            // 后来被一趟真跑推翻）：这一档只说明"没有一槽落在可领那一格"，
            // 而"已入账"是 `cloudStatus == 2` 的读法 —— 真账号读到过 `0`，
            // 它既不是可领（1）也不是已入账（2），念成"已入账"就是一句
            // **没根据的肯定话**。读数本来就跟着这句一起播（[`NoticeResult::readings`]）。
            from_before(NoticeVerdict::Nothing, "能判的槽里没有一个读成可领")
        }
        NoticeOutcomeKind::Manual => from_before(NoticeVerdict::Manual, "有槽读成通知未开启"),
        NoticeOutcomeKind::Unknown => from_before(
            NoticeVerdict::Unknown,
            "上游没给判据字段（cloudStatus / taskStatus），判不了，本次没发请求",
        ),
        NoticeOutcomeKind::Failed => {
            let mut r = from_before(NoticeVerdict::Failed, "");
            r.error = outcome.error.clone();
            r
        }
        // 查判据就挂了那一趟走的是 `Err`，**不会**带着这个档位到这里 ——
        // 这一支只接零值。`Error` 由 `run` 用交回的那条错误原文填。
        NoticeOutcomeKind::None => from_before(NoticeVerdict::Failed, ""),
    }
}

impl NoticeResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// ⚠ 里面只有档位与上游给的两个读数，没有任何凭据形态的东西。
    /// 念读数而**不解释**它 —— "槽一 cloudStatus=2"是**照抄**，
    /// 翻成"槽一已领"就等于把 [`plan_notice`] 那份**未验证的映射**写成结论。
    pub fn summary_line(&self) -> String {
        let head = "公告任务";
        match self.verdict {
            NoticeVerdict::Done => {
                let mut text = match self.gained {
                    // ⚠ 量词是「颗」（AI豆论颗）；`gained` 实拍恒 `None`，
                    // 这一支是"哪天上游给了别丢"。
                    Some(n) => format!("{head}领到 {n} 颗 AI豆"),
                    None => format!("{head}已领"),
                };
                if self.recheck_failed {
                    // ⚠ 这里**不**念 `task_one` / `task_two`：这一档它们刻意是 `None`
                    // （见 [`project`]），念出来只会得到两个"无槽"，
                    // 而"无槽"在上游真没给那一槽时也是一句真话 —— 两种情况就混了。
                    text.push_str("（复查没跑成，拿不到到账读数）");
                    if !self.error.is_empty() {
                        // 结构差（见文件头）：本仓没有第二个返回值装这条错误，
                        // 所以它跟"已领"这个结论一起进小结。
                        text.push('：');
                        text.push_str(&self.error);
                    }
                } else {
                    text.push_str(&format!(
                        "（复查：{}，{}）",
                        slot_label(self.task_one.as_ref()),
                        slot_label(self.task_two.as_ref())
                    ));
                }
                text
            }
            NoticeVerdict::Manual => format!(
                "{head}没领：{}，{NOTICE_MANUAL_INSTRUCTION}{}",
                self.reason,
                self.readings()
            ),
            NoticeVerdict::Nothing | NoticeVerdict::Unknown => {
                format!("{head}没领：{}{}", self.reason, self.readings())
            }
            NoticeVerdict::Failed => {
                if self.error.is_empty() {
                    format!("{head}失败")
                } else {
                    format!("{head}失败：{}", self.error)
                }
            }
        }
    }

    /// 把两个槽的读数念成一句"（读数：…）"。
    ///
    /// 为什么"没领"那一档也要念：判据那一问（"`cloudStatus` 哪个值是未领"）
    /// 唯一可能被答出来的地方就是这两趟读数。读数本身仍**不解释**（照抄）。
    ///
    /// 两个槽都没读到时返回**空串**：那时念出来的是"无槽，无槽"，
    /// 一句没信息量的话占掉小结的一半。
    pub fn readings(&self) -> String {
        if self.task_one.is_none() && self.task_two.is_none() {
            return String::new();
        }
        format!(
            "（读数：{}，{}）",
            slot_label(self.task_one.as_ref()),
            slot_label(self.task_two.as_ref())
        )
    }

    /// 把档位映射到 [`TaskOutcome`]。
    ///
    /// # 分流只有一条判据：**写请求发出去了没有**
    ///
    /// - 没发（`Nothing` / `Manual` / `Unknown`）⇒ **[`TaskOutcome::Skipped`]**。
    ///   目标没达成，但**没有**一次上游出错 —— `Skipped` 正是本仓"条件不成立、
    ///   不是失败"的表达。⚠ `Unknown` 也落这里：它是"上游没给判据"，
    ///   不是"接口打挂了"，报成失败会把它混进错误率。
    /// - 发了且成了（`Done`）⇒ **[`TaskOutcome::Success`]**。
    ///   ⚠ 含"领成功但复查挂了"那一档：实况是**已领**，
    ///   报失败会让用户再去点一次（见文件头"结构差之二"）。
    /// - 发了但没成（`Failed`）⇒ **[`TaskOutcome::Failed`]**：
    ///   那一枪**已经出去了**，报成跳过会把"发过"这件事抹掉。
    ///
    /// # 关于参考实现里的 `ManualRequired`（**已知的结构差**）
    ///
    /// 参考实现把 [`Manual`](NoticeVerdict::Manual) 记成
    /// `OutcomeManualRequired` + 一条 `ManualStep`（界面上挂一个**具体动作**）。
    /// 本仓 [`TaskOutcome`] 没有那一档 ⇒ 那句说明**逐字**进小结
    /// （[`NOTICE_MANUAL_INSTRUCTION`]）、档位落 [`TaskOutcome::Skipped`]。
    /// ⚠ 这是**已知的结构差，不是等价物**：`Skipped` 不触发告警，
    /// 但也**不会**把"要人去做什么"推到界面上；等那一档补上，这一支应当改挂它，
    /// 那句说明就是现成的 `instruction`。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            NoticeVerdict::Done => TaskOutcome::success(summary),
            NoticeVerdict::Nothing | NoticeVerdict::Manual | NoticeVerdict::Unknown => {
                TaskOutcome::skipped(summary)
            }
            NoticeVerdict::Failed => TaskOutcome::failed(summary),
        }
    }
}

// ══════════════════════════ 邮箱那一半（任务 1021） ══════════════════════════

/// 邮箱那一半的结论。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmailNoticeOutcome {
    /// 月表里**有没有** 1021 这一条（旧 TS 的 `if (task1021)` ✓）。
    pub found: bool,
    /// 那一格的 `button.email`；`None` = 1021 不在表里
    ///（与"在、但 `button.email` 缺席"不是一回事 ✓）。
    pub slot: Option<NoticeEmailSlot>,
    /// 旧 TS 的判据算出来**该不该上报**（`day > 0 && canReceive === 1`）——
    /// 记下来是为了让"为什么没发那一枪"可复核 ✓。
    pub report_should: bool,
    /// `reportAppNoticeStatus?status=1` 发出去了、且上游认了。
    pub reported: bool,
    /// 这一半自己的失败原话。**不改**短信那一半的结论（TS 两半各自 best-effort ✓）。
    pub error: String,
}

impl EmailNoticeOutcome {
    /// 一句话小结（三句文案逐字照旧 TS，只有成功那句我们多打了一个 ✓）。
    ///
    /// ⚠ 返回**空串**是一种正常结果：旧 TS 在"表里没有 1021"时**一句都不念** ✓。
    pub fn summary_line(&self) -> String {
        if !self.found {
            return if self.error.is_empty() {
                String::new()
            } else {
                format!("邮箱通知：这一半没跑成：{}", self.error)
            };
        }
        let day = self.slot.as_ref().and_then(|s| s.day).unwrap_or(0);
        if day <= 0 {
            // 旧 TS：「邮箱通知未开启，请手动开启」—— 与短信那一半同一句口径 ✓。
            return "邮箱通知未开启，请手动开启".to_string();
        }
        if !self.report_should {
            return format!("邮箱通知已开启 {day} 天");
        }
        if self.reported {
            return format!("邮箱通知已开启 {day} 天，上报通知状态 ✓");
        }
        if self.error.is_empty() {
            format!("邮箱通知已开启 {day} 天，上报通知状态**没打上** ✗")
        } else {
            format!(
                "邮箱通知已开启 {day} 天，上报通知状态**没打上** ✗：{}",
                self.error
            )
        }
    }
}

/// 跑**邮箱那一半**：查月表 → 找 1021 → 判 `day` / `canReceive` → 该报才报。
///
/// # 逐字照旧 TS `packages/core/src/tasks/notice.ts:32-54`
///
/// ```ts
/// const [emailErr, emailData] = await tryRequest(getEmailTaskList, ctx.http);
/// if (emailData?.result?.month) {
///   const task1021 = emailData.result.month.find((t) => t.id === 1021);
///   if (task1021) {
///     const day = task1021.button?.email?.day || "0";
///     const canReceive = task1021.button?.email?.canReceive || 0;
///     if (Number(day) > 0) {
///       if (canReceive === 1) { await tryRequest(reportAppNoticeStatus, ctx.http, 1); }
///       else { ctx.logger.info(`邮箱通知已开启 ${day} 天`); }
///     } else { ctx.logger.info("邮箱通知未开启，请手动开启"); }
///   }
/// }
/// ```
///
/// # 为什么**不返回 `Result`**
///
/// 旧 TS 两半各自 `tryRequest` + 只 `logger.debug` ⇒ 一半挂掉**不影响**另一半 ✓。
/// 所以失败装进 [`EmailNoticeOutcome::error`] 交回，由调用方念出来
///（本仓口径是"别静默"，所以失败那条走 `warn`，见 `NoticeTask::run`）。
///
/// ⚠ 上报那一枪是**写**（告诉上游"这个状态"），只发**一次**、不重试
///（与 [`claim_notice`] 的 `reward` 同一口径：幂等与否零样本 ⇒ 不赌）。
pub async fn claim_email_notice(ctx: &TaskContext) -> EmailNoticeOutcome {
    let env = match call(ctx, proto::email_task_list_request(), "查询邮箱任务月表").await {
        Ok(env) => env,
        Err(e) => {
            return EmailNoticeOutcome {
                error: e.to_string(),
                ..Default::default()
            }
        }
    };
    let slot = match proto::parse_email_notice_slot(&env) {
        Ok(slot) => slot,
        Err(e) => {
            return EmailNoticeOutcome {
                error: e.to_string(),
                ..Default::default()
            }
        }
    };
    // 表里没有 1021 ⇒ 什么都不做（旧 TS 那句 `if (task1021)` ✓）。
    let Some(slot) = slot else {
        return EmailNoticeOutcome::default();
    };
    let day = slot.day.unwrap_or(0);
    let can_receive = slot.can_receive.unwrap_or(0);
    let mut outcome = EmailNoticeOutcome {
        found: true,
        slot: Some(slot),
        report_should: day > 0 && can_receive == 1,
        ..Default::default()
    };
    if !outcome.report_should {
        return outcome;
    }
    match call(
        ctx,
        proto::report_app_notice_status_request(proto::APP_NOTICE_STATUS_ON),
        "上报 App 通知状态",
    )
    .await
    {
        Ok(env) => match proto::parse_report_app_notice_status(&env) {
            Ok(()) => outcome.reported = true,
            Err(e) => outcome.error = e.to_string(),
        },
        Err(e) => outcome.error = e.to_string(),
    }
    outcome
}

/// 把邮箱那一半的结论交给明细通道（进 `record.details.reported`）。
///
/// 为什么值得报：`day` / `canReceive` **零实拍**，而"到底发没发那一枪"
/// 只有这里能回答（旧 TS 连日志都只有 `debug` ✗）。
fn report_email_details(ctx: &TaskContext, o: &EmailNoticeOutcome) {
    ctx.details(serde_json::json!({
        "kind": "noticeEmail",
        "taskId": proto::NOTICE_EMAIL_TASK_ID,
        "found": o.found,
        "day": o.slot.as_ref().and_then(|s| s.day),
        "canReceive": o.slot.as_ref().and_then(|s| s.can_receive),
        "reportShould": o.report_should,
        "reported": o.reported,
        "error": o.error,
    }));
}

// ══════════════════════════ 任务 ══════════════════════════

/// 公告任务。
///
/// # 这一条链**不会**自己跑起来
///
/// 只有编排层或用户显式点名 `notice` 才会走到这里；本模块没有任何后台任务
/// ⇒ 不存在"没接线就打到真账号上"的路径。
pub struct NoticeTask;

#[async_trait]
impl Task for NoticeTask {
    fn name(&self) -> &'static str {
        "notice"
    }
    fn title(&self) -> &'static str {
        // 界面/注册表上的名字用**页名**（"开启通知领AI豆"是这一页我们要用户
        // 认识的那句话），而小结里的头用参考实现那句"公告任务" ——
        // 两处说的是同一件事，不改。
        "开启通知领AI豆"
    }
    fn group(&self) -> TaskGroup {
        TaskGroup::Device
    }
    fn description(&self) -> &'static str {
        "领取公告任务奖励（先查两个槽；cloudStatus=1 且 taskStatus=3 才领，领完复查一次）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        // ⚠⚠ **两半**（旧 TS `tasks/notice.ts:12-54`）：短信那一半 + 邮箱那一半
        //（任务 1021）。两半**各自 best-effort** ⇒ 先各跑各的，再按短信那半交回。
        // ⚠ 顺序也照 TS：短信在前、邮箱在后 ✓。
        let sms = claim_notice(ctx).await;
        let email = claim_email_notice(ctx).await;
        let email_line = email.summary_line();
        if !email_line.is_empty() {
            if email.error.is_empty() {
                ctx.info(email_line);
            } else {
                // ⚠ 旧 TS 这一半**静默**（只 `logger.debug`）—— 本仓不照抄这份静默：
                // "上报失败却静默"正是本仓反复批评过的那一档 ⇒ 走 `warn` ✓。
                ctx.warn(email_line);
            }
        }
        // ⚠ 明细的**先后**是行为的一部分：短信那一半在前（既有明细的下标不动 ✓），
        // 邮箱那一半在后。短信那半挂掉时也要把邮箱的读数报出去（两半互不影响 ✓），
        // 所以那条早退路径自己补一次。
        let outcome = match sms {
            Ok(o) => o,
            Err(e) => {
                report_email_details(ctx, &email);
                return Err(e);
            }
        };
        let result = project(&outcome);
        report_details(ctx, &outcome, &result);
        report_email_details(ctx, &email);

        if result.recheck_failed {
            // ⚠ 它是**警告**不是失败（见文件头"结构差之二"）：
            // 结论仍是"已领"，这条日志只是把"这一趟没走完"如实说出来。
            ctx.warn(format!("公告任务已领，但复查没跑成：{}", result.error));
        }
        ctx.info(result.summary_line());
        Ok(result.task_outcome())
    }
}

/// 把这一趟的结论与**两份读数**交给明细通道（进 `record.details.reported`）。
///
/// # 为什么这一条值得报明细
///
/// 两个槽的 `cloudStatus` 语义**未验证**，而判据那一问（"哪个值是未领"）
/// 只能靠真跑读数回答；`reward` 又报不出到账数 ⇒ 复查那一份是**唯一**的
/// 到账读数。只把小结点进 `runs.message`，那些数就跟着历史一起被压成一句话了。
///
/// ⚠ `before` 与 `recheck` **分开放**：把复查那份回落到 `before`（或反之）
/// 会让"领之前 / 领之后"这个唯一的问题答成假的。
fn report_details(ctx: &TaskContext, outcome: &NoticeOutcome, result: &NoticeResult) {
    ctx.details(serde_json::json!({
        "kind": "notice",
        "verdict": verdict_slug(result.verdict),
        "reason": result.reason,
        "error": result.error,
        "recheckFailed": result.recheck_failed,
        "gained": result.gained,
        // 读数照抄，键名保留上游的拼写。
        "before": info_json(&outcome.before),
        "recheck": outcome.recheck.as_ref().map(info_json),
    }));
}

/// 一份 `getTaskInfo` 读数的 JSON 形状（两个槽）。
fn info_json(info: &NoticeTaskInfo) -> Value {
    serde_json::json!({
        "taskOneInfo": slot_json(info.task_one_info.as_ref()),
        "taskTwoInfo": slot_json(info.task_two_info.as_ref()),
    })
}

/// 一个槽的 JSON 形状。
///
/// ⚠ "上游没给这一槽"念成 `null`，而"给了但字段全缺"是四个 `null` 的**对象** ——
/// 合成一件事之后，排查时就分不清"槽没给"与"槽给了、读数没读到"。
fn slot_json(slot: Option<&NoticeTaskSlot>) -> Value {
    match slot {
        None => Value::Null,
        Some(s) => serde_json::json!({
            "cloudStatus": s.cloud_status,
            "taskStatus": s.task_status,
            "sum": s.sum,
            "progress": s.progress,
        }),
    }
}

/// 把一个槽念成一句短语。**缺的键念成「缺」，不念成 0** ——
/// "上游没给 `progress`"与"上游给了 0"是两件事，
/// 混起来会让人觉得任务进度是零。
///
/// ⚠ 四格的语义**一个都没被上游证实**，所以只照抄键名与数，**不翻成中文**。
fn slot_label(slot: Option<&NoticeTaskSlot>) -> String {
    let Some(slot) = slot else {
        return "无槽".to_string();
    };
    [
        ("cloudStatus", slot.cloud_status),
        ("taskStatus", slot.task_status),
        ("sum", slot.sum),
        ("progress", slot.progress),
    ]
    .iter()
    .map(|(key, value)| match value {
        Some(n) => format!("{key}={n}"),
        None => format!("{key}=缺"),
    })
    .collect::<Vec<_>>()
    .join(" ")
}

/// 档位的稳定标识（落进 JSON，供界面/脚本按它分流）。
///
/// ⚠ 不用 `Debug` 输出：那是给人看的，改个枚举名就会悄悄改掉接口。
fn verdict_slug(v: NoticeVerdict) -> &'static str {
    match v {
        NoticeVerdict::Done => "done",
        NoticeVerdict::Nothing => "nothing",
        NoticeVerdict::Manual => "manual",
        NoticeVerdict::Unknown => "unknown",
        NoticeVerdict::Failed => "failed",
    }
}

#[cfg(test)]
mod tests;
