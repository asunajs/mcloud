//! 消息推送任务（`msg-push`）：查状态 → **只有 `secondTaskStatus == 2` 才领**。
//!
//! 请求构造与解析在 [`mcloud_upstream::app_msg_push`]（纯函数、无 IO）；
//! 本模块只负责**编排**：查状态、判分支、把结论说成一句人话。
//!
//! # 证据分成两半，别混着读
//!
//! * **`task/status` 有实拍**：URL（裸 GET、零查询参数）/ 方法 / 六个字段名
//!   都是 2026-09-25 抓包逐字看到的，不再是照着旧描述写的。
//! * **`task/obtain` 仍零样本**：那次实拍落在 `secondTaskStatus = 1`（进度 16/31 天），
//!   页面上根本没有领取入口 ⇒ 这一枪**结构性拍不到**。方法、非 0 例文两格全空。
//!
//! ⇒ 于是本模块的判据分两种语气：**判据字段的存在**是实拍，
//! 而"**什么值该做什么**"这一层**仍是旧项目的口径**（它说 `== 2` 才发、
//! 说 `== 0` 是通知关了）—— 实拍那天恰好**没有**命中任何一个被判据的值，
//! 所以那些判据今天**一条都没被上游证实过**。
//!
//! # 为什么这里**不**发第二个请求去复查（与公告任务那条相反）
//!
//! `notice` 领完要再查一次，是因为它至少**有**一个可查的读数口。
//! 这一条复查要看的是同一个 `task/status`，而 `obtain` 之后它会不会变**零样本** ——
//! 为一个零样本接口多打一次请求，换来的读数解释不了自己，只会把
//! "两次读数不同"变成新的疑点。要加，等抓包说明 `obtain` 前后状态怎么变之后再加。
//!
//! # 票来自**另一页**
//!
//! 这一页**有自己换票的参数**且**已经用上了**：
//! [`SessionRecipe::MsgPushOn`](mcloud_upstream::session::SessionRecipe::MsgPushOn)
//! （`marketName=msgpushon` / `sourceId=1062`、入口 `portal/hcyview/…?path=openNoticeGift`）。
//!
//! 依据是 `docs/capture/app-msg-push-on-samples.txt` §2 的**三张票核对表**：
//! 那一趟三张 `jwttoken` 互不相等，这一页那张被用在 **24 个接口**上（含本模块打的
//! `msgPushOn/task/status`、`msgPushOn/task/list`），而签到页那张用在另外 16 个上。
//!
//! 路由方式见 `H5Client::recipe_for`：本模块**整条链**都在 `/ycloud/msgPushOn/` 下
//! ⇒ 一个配方到底，不会一条链里混两张票。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::app_msg_push as proto;
use mcloud_upstream::app_msg_push::MsgPushStatus;
use mcloud_upstream::app_signin::SignInEnvelope;

/// 发一个请求并解成信封。
async fn call(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<SignInEnvelope> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    SignInEnvelope::parse(body)
}

/// 只查状态、不领（给"看一眼"那种用法留着）。
pub async fn query_status(ctx: &TaskContext) -> Result<MsgPushStatus> {
    let env = call(
        ctx,
        proto::msg_push_status_request(),
        "查询消息推送任务状态",
    )
    .await?;
    proto::parse_msg_push_status(&env)
}

// ══════════════════════════ 判定 ══════════════════════════

/// [`plan_msg_push`] 的结论档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgPushPlan {
    /// 该领第二次奖励：`secondTaskStatus == 2`。
    Obtain,
    /// `pushOn == 0` —— 通知本身没开，得有人去 App 里打开。
    Manual,
    /// 判据给了、但不是该领的那个值 ⇒ 现在不该领（算成功）。
    Nothing,
    /// **判据字段缺失** ⇒ 一个写请求都不发。
    Unknown,
}

/// `pushOn == 0` 读作"消息通知已关闭"（旧项目：`pushOn === 0` ⇒ 报错并 return）。
///
/// ⚠ 关闭态**没有样本**：实拍只见过 `1`。
pub const PUSH_ON_OFF: i64 = 0;

/// `secondTaskStatus == 2` 读作"第二次奖励该领"（旧项目：`=== 2` 才发 `obtain`）。
///
/// ⚠ `2` 那一档**没有样本**（实拍那次正好是 `1`）。
pub const SECOND_TASK_CLAIMABLE: i64 = 2;

/// `firstTaskStatus == 3` 读作"首次奖励已领"（旧项目：`!== 3` ⇒ 只打一句 info）。
///
/// ⚠ 这一条**不影响成功**，所以它不参与 [`plan_msg_push`]，只影响小结里那句话。
pub const FIRST_TASK_DONE: i64 = 3;

/// 按 `task/status` 的读数判 [`MsgPushPlan`]。**纯函数**，不碰网络。
///
/// # 顺序就是旧项目的顺序
///
/// 旧描述的逐字顺序是：先 `if (pushOn === 0) { …; return; }` ——
/// 即它是**前置开关**，关着的时候连 `secondTaskStatus` 都不必看。
/// 之后才是 `secondTaskStatus === 2` 才发。
///
/// # 为什么 `pushOn` 缺失**不**判 `Manual`
///
/// 旧项目用的是 `=== 0`（严格相等）：`undefined === 0` 是 `false` ⇒ 它往下走。
/// 我们照这个口径，而且它本来也是更诚实的那个：
/// `pushOn` **没给**不等于**给了 0**，把"上游没说"落成"要人去开通知"
/// 是**凭空造一条待办** —— 用户会为了一个可能根本没坏的东西去翻 App 设置。
///
/// 反向后果（少提示一次）只在"真的关了、但上游没报"时发生，而那一形态**零样本**。
pub fn plan_msg_push(status: &MsgPushStatus) -> MsgPushPlan {
    if let Some(push_on) = status.push_on {
        if push_on == PUSH_ON_OFF {
            return MsgPushPlan::Manual;
        }
    }
    match status.second_task_status {
        // 判据字段缺失 ⇒ **一个写请求都不发**。
        // ⚠ 不能落 Nothing：那会让小结说成"现在没得领"，
        // 而那是一句**没根据的肯定话**（"判不了"与"判了不该领"是两件事）。
        None => MsgPushPlan::Unknown,
        Some(v) if v == SECOND_TASK_CLAIMABLE => MsgPushPlan::Obtain,
        Some(_) => MsgPushPlan::Nothing,
    }
}

/// "首次奖励还没领"这句话要不要说（旧项目在这里只打一句 info）。
///
/// ⚠ 判据字段缺失时回 `false`：旧项目写的是 `!== 3`，而 `undefined !== 3`
/// 在 JS 里是 `true` ⇒ 它**会**打那句 info。
/// 我们反过来选"不提示"，因为这句提示的落点不是日志而是**界面上的一句话**，
/// 而"上游没给 `firstTaskStatus`"不该被念成"首次奖励没领"。
///
/// ⚠ 这一处是**主动偏离**旧项目，理由是那句提示在旧项目里没有对应后果
/// （不影响成功），而我们这句话会被当事实读。
pub fn first_reward_pending(status: &MsgPushStatus) -> bool {
    matches!(status.first_task_status, Some(v) if v != FIRST_TASK_DONE)
}

// ══════════════════════════ 跑一个账号 ══════════════════════════

/// 一次消息推送任务的结局档位。
///
/// ⚠ 名字不叫 `MsgPushStatus`：那个名字已经是上游那个**读数结构**
/// （[`proto::MsgPushStatus`]），两件事共用一个名字会让人读
/// [`MsgPushResult`] 时分不清"档位"还是"读数"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MsgPushVerdict {
    /// 领到了。
    Done,
    /// 不该领 —— 算成功。
    Nothing,
    /// 得有人去 App 里打开消息通知。
    Manual,
    /// 判据缺失（**没发请求**）。与 [`Nothing`](Self::Nothing) 分开是因为
    /// 一句有根据、一句没根据。
    Unknown,
    /// `obtain` 发了但没成（含"发了、上游没回 `obtainCode`"）。
    ///
    /// ⚠ 这个字段**刻意是零值**：`MsgPushResult` 有多条提前 `return` 的路径，
    /// 零值若是 `Done`，一句"领到了"就会在**一个请求都没发**的那趟上被念出来。
    #[default]
    Failed,
}

/// 一个账号这一趟的产出。
///
/// # 为什么把状态读数**全部**带出来（六个数，不只是判据那些）
///
/// 这一族的 `obtain` 仍零样本：将来排查时唯一能比对的就是
/// "上游当时到底给了什么数"。只留一个档位，等于把第一批真跑读数扔掉、
/// 再靠猜走第二遍。
///
/// ⇒ `total` 与 `push_task_status` 是实拍里读到、**旧描述根本没提**的两格，
/// 这里一并带出来：它们的语义**没人说过**，所以只照抄、**不进判据、不解释**。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MsgPushResult {
    /// `result.obtainCode` 读数；`None` = 上游没给（≠ 0，≠ 成功）。
    pub obtain_code: Option<i64>,
    /// 判据读数。
    pub push_on: Option<i64>,
    pub first_task_status: Option<i64>,
    pub second_task_status: Option<i64>,
    /// 同一份响应里另外三格（实拍 `31` / `16` / `0`）。
    ///
    /// ⚠ 语义**未验证**，不参与任何判定；`on_duaration` 与 `total` 只进小结
    /// （它们在 App 页面上对得上号），`push_task_status` 连小结都不进。
    pub total: Option<i64>,
    pub on_duaration: Option<i64>,
    pub push_task_status: Option<i64>,
    /// "首次奖励看着也没领"那句话要不要说。
    pub first_reward_notice: bool,
    /// 档位。
    pub verdict: MsgPushVerdict,
    /// [`Nothing`](MsgPushVerdict::Nothing) / [`Manual`](MsgPushVerdict::Manual)
    /// / [`Unknown`](MsgPushVerdict::Unknown) 的人话依据。
    pub reason: String,
    /// [`Failed`](MsgPushVerdict::Failed) 的失败原因。
    pub error: String,
}

/// 建一份"还没判"的结果，只填读数。
fn result_of(status: &MsgPushStatus) -> MsgPushResult {
    MsgPushResult {
        // 写请求还没发 ⇒ 这一格理应是 `None`（它只在 `obtain` 之后才有值）。
        // ⚠ 别把它当成"上游没给"：那两种 `None` 由 `verdict` 分开 ——
        // 走到过写请求的才是后者。
        obtain_code: None,
        push_on: status.push_on,
        first_task_status: status.first_task_status,
        second_task_status: status.second_task_status,
        total: status.total,
        on_duaration: status.on_duaration,
        push_task_status: status.push_task_status,
        first_reward_notice: false,
        verdict: MsgPushVerdict::Failed,
        reason: String::new(),
        error: String::new(),
    }
}

impl MsgPushResult {
    /// 给人看的一句话小结（进 `runs.message` 与终局事件，**出网**）。
    ///
    /// ⚠ 只有档位与照抄的读数，没有任何凭据形态的东西；读数**不解释** ——
    /// 这一族连"`pushOn` 有几种取值"都没样本，翻成中文就是编。
    pub fn summary_line(&self) -> String {
        let head = "消息推送任务";
        let note = if self.first_reward_notice {
            // 旧项目这一句是 info 级、**不影响成功**。我们照这个口径：
            // 它只进小结，**不**升成失败。
            "；另外首次奖励看着也没领"
        } else {
            ""
        };
        match self.verdict {
            MsgPushVerdict::Done => {
                format!("{head}已领（{}）{note}", self.readings())
            }
            MsgPushVerdict::Nothing | MsgPushVerdict::Manual | MsgPushVerdict::Unknown => {
                let mut text = format!("{head}没领：{}（{}）{note}", self.reason, self.readings());
                if self.verdict == MsgPushVerdict::Manual {
                    text.push_str("，要去 App 里打开消息通知");
                }
                text
            }
            MsgPushVerdict::Failed => {
                if self.error.is_empty() {
                    format!("{head}失败")
                } else {
                    format!("{head}失败：{}", self.error)
                }
            }
        }
    }

    /// 把判据读数念成一句短语。缺的键念"缺"、**不念 0**
    /// （把"没说"写成"说了 0"是最难发现的一类假话）。
    ///
    /// # 为什么念这五个、不念 `pushTaskStatus`
    ///
    /// 判据那三个（`pushOn` / `firstTaskStatus` / `secondTaskStatus`）是结论依据；
    /// `onDuaration` 与 `total` 是**人在 App 界面上能核对**的那两个数
    /// （页面上写着 16/31），带上它们，小结才具备"对着手机看一眼就知道有没有读错"的能力。
    /// `pushTaskStatus` 只在结构里留着、**不进小结**：它既不是判据，
    /// 也没有任何界面读数能与它对上号 —— 那句话进历史列表只是噪声。
    ///
    /// ⚠ 这五格的语义**一个都没被上游证实**（`total` / `pushTaskStatus`
    /// 连旧项目的描述都没有），所以只照抄键名与数，不翻成中文。
    pub fn readings(&self) -> String {
        let pairs = [
            ("pushOn", self.push_on),
            ("firstTaskStatus", self.first_task_status),
            ("secondTaskStatus", self.second_task_status),
            // ⚠ 用上游那个**拼错**的键名：小结里念的是"我们读的是哪一个键"。
            (proto::MSG_PUSH_DURATION_KEY, self.on_duaration),
            ("total", self.total),
        ];
        pairs
            .iter()
            .map(|(k, v)| match v {
                Some(n) => format!("{k}={n}"),
                None => format!("{k}=缺"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 把档位映射到 [`TaskOutcome`]。
    ///
    /// # 分流只有一条判据：**写请求发出去了没有**
    ///
    /// - 没发（`Nothing` / `Manual` / `Unknown`）⇒ **[`TaskOutcome::Skipped`]**。
    ///   目标没达成，但**没有**一件"必须人去做、程序代劳不了"的事，
    ///   也没有一次上游出错 —— `Skipped` 正是本仓"条件不成立、不是失败"的表达。
    ///   ⚠ `Unknown` 也落这里：它是"上游没给判据"，不是"接口打挂了"，
    ///   报成失败会把它混进错误率，而错误率一旦被这种"什么都没做"的趟占住，
    ///   告警就失去意义。**但它不是静默的** —— 理由那句话会进 `runs.message`。
    /// - 发了但没成（`Failed`）⇒ **[`TaskOutcome::Failed`]**：
    ///   那一枪**已经出去了**，这一趟的实况是"发过、判不出成"，
    ///   报成跳过会把"发过"这件事抹掉（与 `backup-gift` 的领取那一枪同一口径）。
    ///
    /// ⚠ 本仓 [`TaskOutcome`] 没有参考实现那档 `ManualRequired`，
    /// 所以"要去 App 里打开通知"那句说明**逐字放进小结**
    /// （见 [`Self::summary_line`]）。等那一档补上，这一支应当改挂它。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            MsgPushVerdict::Failed => TaskOutcome::failed(summary),
            _ => {
                if self.verdict == MsgPushVerdict::Done {
                    TaskOutcome::success(summary)
                } else {
                    TaskOutcome::skipped(summary)
                }
            }
        }
    }
}

/// 跑一次消息推送任务。
///
/// # 这一条链**不会**自己跑起来
///
/// 只有编排层或用户显式点名 `msg-push` 才会走到这里；本模块没有任何后台任务
/// ⇒ 不存在"没接线就打到真账号上"的路径。
pub struct MsgPushTask;

#[async_trait]
impl Task for MsgPushTask {
    fn name(&self) -> &'static str {
        "msg-push"
    }
    fn title(&self) -> &'static str {
        "消息推送"
    }

    fn group(&self) -> TaskGroup {
        TaskGroup::Device
    }
    fn description(&self) -> &'static str {
        "开启 App 消息通知后领第二次奖励（先查状态；只有 secondTaskStatus=2 才领）"
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        ctx.progress(1, 1, "查询消息推送任务状态");
        let status = query_status(ctx).await?;
        let mut result = result_of(&status);

        match plan_msg_push(&status) {
            MsgPushPlan::Manual => {
                result.verdict = MsgPushVerdict::Manual;
                result.reason = "pushOn=0，消息通知是关着的".into();
                report_details(ctx, &result);
                return Ok(result.task_outcome());
            }
            MsgPushPlan::Nothing => {
                result.verdict = MsgPushVerdict::Nothing;
                result.reason = "secondTaskStatus 不是该领的那个值".into();
                report_details(ctx, &result);
                return Ok(result.task_outcome());
            }
            MsgPushPlan::Unknown => {
                result.verdict = MsgPushVerdict::Unknown;
                result.reason = "上游没给 secondTaskStatus，判不了，本次没发请求".into();
                report_details(ctx, &result);
                return Ok(result.task_outcome());
            }
            MsgPushPlan::Obtain => {}
        }

        result.first_reward_notice = first_reward_pending(&status);

        ctx.check_cancelled()?;
        ctx.progress(1, 1, "领取消息推送奖励");
        // ⚠ 这一枪是**写**，且整条零样本。它**不重试** ——
        //    "上游幂等与否零样本"，重试等于赌。
        let env = call(ctx, proto::msg_push_obtain_request(), "领取消息推送奖励").await;
        let obtained = match env {
            Ok(env) => proto::parse_msg_push_obtain(&env),
            Err(e) => Err(e),
        };
        match obtained {
            Err(e) => {
                // 写请求发出去了、但没读到可信结论 ⇒ 报失败，**不报跳过**。
                result.verdict = MsgPushVerdict::Failed;
                result.error = e.to_string();
            }
            Ok(o) => {
                result.obtain_code = o.obtain_code;
                match o.obtain_code {
                    // ⚠ 缺 `obtainCode` 也算**没成**而不是"成了"：
                    //    写请求已经发出去了，把"判不了"伪造成"成功"，
                    //    用户就再也不会去查这一笔。
                    None => {
                        result.verdict = MsgPushVerdict::Failed;
                        result.error = "上游没回 obtainCode，判不出这次到底成没成".into();
                    }
                    Some(0) => result.verdict = MsgPushVerdict::Done,
                    Some(code) => {
                        result.verdict = MsgPushVerdict::Failed;
                        result.error = format!("上游回 obtainCode={code}");
                    }
                }
            }
        }
        report_details(ctx, &result);
        Ok(result.task_outcome())
    }
}

/// 把这一趟的结论交给明细通道（进 `record.details.reported`）。
///
/// # 为什么这一条值得报明细
///
/// `obtain` 整条**零样本**，而这一族有**六格**读数、语义一个都没被上游证实。
/// 真跑时唯一能回答"上游当时到底给了什么数"的就是这份读数 ——
/// 只把小结点进 `runs.message`，那些数就跟着历史一起被压成一句话了。
fn report_details(ctx: &TaskContext, result: &MsgPushResult) {
    ctx.details(serde_json::json!({
        "kind": "msg-push",
        "verdict": verdict_slug(result.verdict),
        "reason": result.reason,
        "error": result.error,
        "obtainCode": result.obtain_code,
        // 读数照抄，键名保留上游的拼写（含它拼错的那个）。
        "readings": {
            "pushOn": result.push_on,
            "firstTaskStatus": result.first_task_status,
            "secondTaskStatus": result.second_task_status,
            proto::MSG_PUSH_DURATION_KEY: result.on_duaration,
            "total": result.total,
            "pushTaskStatus": result.push_task_status,
        },
        "firstRewardNotice": result.first_reward_notice,
    }));
}

/// 档位的稳定标识（落进 JSON，供界面/脚本按它分流）。
///
/// ⚠ 不用 `Debug` 输出：那是给人看的，改个枚举名就会悄悄改掉接口。
fn verdict_slug(v: MsgPushVerdict) -> &'static str {
    match v {
        MsgPushVerdict::Done => "done",
        MsgPushVerdict::Nothing => "nothing",
        MsgPushVerdict::Manual => "manual",
        MsgPushVerdict::Unknown => "unknown",
        MsgPushVerdict::Failed => "failed",
    }
}

#[cfg(test)]
mod tests;
