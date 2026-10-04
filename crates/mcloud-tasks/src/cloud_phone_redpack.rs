//! 红包派对（cloud-phone-redpack）—— 任务层编排。
//!
//! # 这一层管什么
//!
//! 协议层（mcloud_upstream::cloud_phone_redpack）只负责"造请求 / 解响应"，
//! 它不认识"该不该发"。本模块补上那一步：先用 /user/tokenValidate 把 SSO 票
//! 换成这一族自己的 token，再读任务表，然后逐个任务按 userStatus 决定
//! 要不要打那几枪写请求。
//!
//! 依据只有旧 TS（优先级 ①，那份能跑）：
//! packages/core/src/tasks/cloud-phone-redpack/{task.ts,api.ts}。
//! 每处行为都在注释里标了 TS 的函数名与行号，便于对拍。
//!
//! # 读写分界与"最多写一次"
//!
//! 只读：configTaskLoginList / userAccountInfo / userToastInfo /
//! configTopicList / thirdQueryUserRelationshipV2 / configAppList /
//! gethwToken / instanceApkInstallStatus / appInstallTaskStatus。
//! 写：userSign / userCompleteTask / userBrowse / userTopicAnswer /
//! apkInstall / userInstallApp，外加有副作用的 getRcsToken。
//!
//! 每个 taskCode 在一次运行内最多写一次（HashSet 记已写过的码）。
//! 这不是照着 TS 抄的（TS 的任务表里一个码只出现一次，它没有这个问题），
//! 而是本仓的硬要求：上游若把同一个码列两遍，重复写就是重复领。
//!
//! # 状态码怎么判：只有 0 才写，未知码单独一档
//!
//! 旧 TS 的 handleTask 是 Number(task.userStatus ?? 0)（task.ts:440）：
//! 0 未完成 / 1 已完成 / 3 奖品已兑完 / 4 明天再来，其余走"去完成"。
//! 本模块照抄那四个码的语义，但**不**照抄"其余走去做"：读到这一族没见过的码
//! （2、5、99 …）时归进 Unknown 档、**不发**写请求，并单独计数。
//! 这正是协议层 redpack_status_text 对未知码回 None 的用途 ——
//! 把未知折进"未完成"就等于对一条可能已经领过的任务再打一枪。
//!
//! ⚠ 缺 userStatus 这一格按 0（未完成）处理，与未知**码**是两回事：
//! 旧 TS 的 ?? 0 就是这么定的（协议层 RedpackTask.user_status 的注释也写明
//! "缺失按 0（未完成）"）。所以"缺格"会去写，而"读到 99"不会。
//!
//! # 会话 token 只在本次运行内复用
//!
//! 旧 TS 把换来的 token 写进 ctx.state.cloudPhoneRedpackToken，下一轮直接复用
//! （task.ts:39-42、130）。本仓的 TaskContext 没有 state 槽；能落盘的
//! Store::account_state / save_account_state 存的是整账号共享的一份 JSON
//! （crates/mcloud-core/src/traits.rs:173），本仓别的任务也往里写设备指纹之类。
//! 做整份读-改-写既要把别人的键原样搬回去（一次搬错就是别的任务状态损坏），
//! 又没有任何一条实拍说明这一族 token 的有效期 —— 所以这里只在本趟内复用，
//! 不落盘。少省一次换票，不会写坏别人的状态。
//!
//! # 没有开关
//!
//! ⚠ 这里**没有**账号级开关：`redpack_enabled` 已于 2026-10-03 按用户口径
//! 「账号不应该有这个配置」删除（它默认 `true`，是本仓自己加的、旧 TS 没有的
//! 一位）。任务**是用户显式点起来的**，"跑不跑"这一层由那个动作表达。
//!
//! # 等待时间做成字段
//!
//! 旧 TS 里有三处 sleep：浏览确认前 15 秒（task.ts:244）、答题重取题库每轮
//! 150ms（task.ts:413）、装机轮询每轮 1 秒（task.ts:350）。它们都装进
//! [RedpackPlan]，测试设 0 ⇒ 真 sleep 只出现在生产路径（与
//! task_expansion.rs 同一套做法）。
//!
//! # 凭据不进明细
//!
//! report_details 里只有"读到了什么 / 发了哪几枪"，**没有** token 与 SSO 票
//! （那是凭据，会落进运行台账与界面）。

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use mcloud_core::{Error, HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskGroup, TaskResult};
use mcloud_upstream::cloud_phone_redpack as proto;
use mcloud_upstream::cloud_phone_redpack::{
    RedpackAccount, RedpackCall, RedpackEnvelope, RedpackPhoneStatus, RedpackTask, RedpackTaskList,
    RedpackTopic,
};

/// 任务名（注册表、测试、/api/capabilities 都认它）。
pub const REDPACK_TASK_NAME: &str = "cloud-phone-redpack";
/// 任务标题。
pub const REDPACK_TASK_TITLE: &str = "红包派对";
/// 任务说明。
pub const REDPACK_TASK_DESCRIPTION: &str =
    "移动云手机红包派对：签到、做新手/每日/每月任务并领奖；按 userStatus 决定该不该写";

// ══════════════════════ 旧 TS 的三张任务码表 ══════════════════════

/// finishTask(..., browse=true)：先登记再等一会儿报"浏览过了"（task.ts:19）。
const BROWSE_TASKS: [&str; 3] = ["NOVICE_2", "NOVICE_3", "MONTHLY_1"];
/// finishTask(..., browse=false)：只登记，没有浏览确认（task.ts:20）。
const DIRECT_TASKS: [&str; 2] = ["MONTHLY_4", "MONTHLY_5"];
/// 规则上只能人工做的码（task.ts:21）。
const MANUAL_TASKS: [(&str, &str); 1] = [("NOVICE_1", "需跳转领取定向流量")];

/// 旧 TS 写死的题库答案（task.ts:22-38，逐条照抄）。
///
/// ⚠ 这是上游题面与答案的对照表，不是我们推导出来的东西；题干哪天改了，
/// 这里就查不到 ⇒ 走"未知题目 ⇒ 需手动完成"，而不是瞎选一个字母。
const KNOWN_ANSWERS: [(&str, &str); 15] = [
    (
        "如何查看并更新移动云手机客户端最新版本？",
        "进入'我的'-点击'关于云手机'-点击'检查新版本'",
    ),
    (
        "移动云手机可领取定向流量，每月赠送的定向流量是（  ）。",
        "30GB",
    ),
    (
        "移动云手机端内订购的专业版分辨率已升级到1080P，该说法是否正确？",
        "正确",
    ),
    ("移动云手机支持视频录制，该说法是否正确？", "正确"),
    (
        "云手机支持通过手机、平板、电脑等多种终端设备登录使用，该说法是否正确？",
        "正确",
    ),
    (
        "使用中国移动号码登录移动云手机，是否支持手机号一键登录？",
        "支持",
    ),
    ("只有中国移动运营商号码能使用移动云手机？", "不正确"),
    ("移动云手机是否需要充电使用？", "不需要"),
    ("移动云手机支持截图，该说法是否正确？", "正确"),
    ("移动云手机AI灵犀助手已接入DeepSeek，是否正确？", "正确"),
    ("移动云手机内支持画面清晰度切换，该说法是否正确？", "正确"),
    ("移动云手机支持连接蓝牙使用吗？", "不支持"),
    (
        "在云手机内安装游戏应用是否占本地手机存储空间？",
        "否，不占本地空间",
    ),
    (
        "如何更换云机内的桌面主题或壁纸？",
        "云机内-【设置】-壁纸/个性主题",
    ),
    (
        "如何将云手机里的应用添加至本地手机桌面？",
        "云手机桌面-长按应用-发送图标到本地",
    ),
];

/// 答题时选项下标到字母的映射（TS 的 "ABCD"[index]，task.ts:419）。
const ANSWER_LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];

// ══════════════════════ 计划（等待时间） ══════════════════════

/// 这条链上那三处等待的时长。
///
/// ⚠ 分开成结构体而不是写死常量，是为了让测试把整条链跑到"零等待"——
/// 真睡 15 秒 + 60 秒轮询的测试既慢，也会把"等"这件事从断言里挤出去。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedpackPlan {
    /// 登记完成后、发浏览确认前等多久（TS 15000ms，task.ts:244）。
    pub browse_wait_ms: u64,
    /// 重取题库最多试几轮（TS 15，task.ts:393）。
    pub topic_attempts: u32,
    /// 每轮重取题库之间等多久（TS 150ms，task.ts:413）。
    pub topic_interval_ms: u64,
    /// 装机进度最多轮询几次（TS 60，task.ts:336）。
    pub install_poll_attempts: u32,
    /// 每次轮询之间等多久（TS 1000ms，task.ts:350）。
    pub install_poll_interval_ms: u64,
}

impl Default for RedpackPlan {
    fn default() -> Self {
        Self {
            browse_wait_ms: 15_000,
            topic_attempts: 15,
            topic_interval_ms: 150,
            install_poll_attempts: 60,
            install_poll_interval_ms: 1_000,
        }
    }
}

// ══════════════════════ 归类与计数 ══════════════════════

/// 一条任务在本趟判定里的归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedpackTaskState {
    /// userStatus == 0（或缺这一格，照 TS 的 ?? 0）：这一趟该去把它做掉。
    Todo,
    /// userStatus == 1：上游说已经完成。
    Done,
    /// userStatus == 3 / 4：奖品已兑完 / 明天再来，本轮不做。
    Blocked,
    /// 上游给了这一族没见过的码。
    ///
    /// ⚠ 单独一档是刻意的：把它折进 Todo 就等于"对一条可能已经完成的任务
    /// 再发一次写请求"。零值也故意是它 —— 一个没填过 state 的表项
    /// 绝不能被念成"该去做"。
    #[default]
    Unknown,
}

impl RedpackTaskState {
    /// 明细里那个稳定的小写名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }

    /// 把 userStatus 归进四档。
    ///
    /// 缺格按 Todo（旧 TS Number(userStatus ?? 0)，task.ts:440、203）；
    /// 没见过的码落 Unknown、不发写请求（见模块头）。
    fn classify(status: Option<i64>) -> Self {
        match status {
            None | Some(proto::REDPACK_STATUS_TODO) => Self::Todo,
            Some(proto::REDPACK_STATUS_DONE) => Self::Done,
            Some(proto::REDPACK_STATUS_SOLD_OUT) | Some(proto::REDPACK_STATUS_TOMORROW) => {
                Self::Blocked
            }
            Some(_) => Self::Unknown,
        }
    }
}

/// 任务表按四档数出来的计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RedpackCounts {
    pub total: usize,
    pub todo: usize,
    pub done: usize,
    pub blocked: usize,
    pub unknown: usize,
}

/// 一条任务在本次运行里的处置记录。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedpackTaskEntry {
    /// 上游的 id，原样留着（发 userCompleteTask 时也原样发回）。
    pub id: Option<Value>,
    pub task_code: String,
    pub task_name: Option<String>,
    /// 原始 userStatus；None = 没读到。
    pub status: Option<i64>,
    pub state: RedpackTaskState,
    /// 本趟是否真的为它发过写请求。
    pub handled: bool,
    /// 人话：为什么没做 / 做到哪一步 / 失败在哪一步。
    pub note: String,
}

/// 每日签到那一格的处置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedpackSignRecord {
    pub status: Option<i64>,
    pub done: bool,
    pub wrote: bool,
    pub note: String,
}

/// 某条任务这一趟实际拿到的奖（userToastInfo 的读数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedpackReward {
    pub task_code: String,
    pub prize_name: Option<String>,
    pub prize_amount: Option<i64>,
}

/// 这一趟的完整读数与处置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedpackBoard {
    pub tasks: Vec<RedpackTaskEntry>,
    pub sign: Option<RedpackSignRecord>,
    pub balance_before: RedpackAccount,
    pub balance_after: Option<RedpackAccount>,
    /// 本趟发出去过的写请求（标签），只读请求不进这里。
    pub writes: Vec<String>,
    /// 需人工去 App 里点一下的项。
    pub manual: Vec<String>,
    /// 明确失败的步骤（任一非空 ⇒ 整趟判 Failed）。
    pub failures: Vec<String>,
    /// 非致命、但"没读到"要如实说一声的项。
    pub notes: Vec<String>,
    pub rewards: Vec<RedpackReward>,
}

impl RedpackBoard {
    /// 按四档数任务表。
    pub fn counts(&self) -> RedpackCounts {
        let mut counts = RedpackCounts {
            total: self.tasks.len(),
            ..Default::default()
        };
        for task in &self.tasks {
            match task.state {
                RedpackTaskState::Todo => counts.todo += 1,
                RedpackTaskState::Done => counts.done += 1,
                RedpackTaskState::Blocked => counts.blocked += 1,
                RedpackTaskState::Unknown => counts.unknown += 1,
            }
        }
        counts
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.failures.push(message.into());
    }

    fn note(&mut self, message: impl Into<String>) {
        self.notes.push(message.into());
    }

    fn manual(&mut self, message: impl Into<String>) {
        self.manual.push(message.into());
    }
}

/// 整趟的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedpackVerdict {
    /// 走到了该走的每一步，没有硬失败。
    Done,
    /// 至少有一个明确失败的步骤。
    ///
    /// ⚠ 零值是它（与 play_ai 同款的保守选择）：一个没填过 verdict 的结果
    /// 绝不能被念成"一切正常"。
    #[default]
    Failed,
}

/// 一个账号这一趟的产出。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RedpackResult {
    pub verdict: RedpackVerdict,
    pub board: RedpackBoard,
    /// 失败时的说明；成功时空串。
    pub error: String,
}

impl RedpackResult {
    /// 给人看的一句话小结（进 runs.message 与终局事件，会出网）。
    ///
    /// 要求三样都写清：未完成 N / 已完成 M / 未知 K，以及失败在哪一步。
    pub fn summary_line(&self) -> String {
        let counts = self.board.counts();
        let tally = if counts.blocked > 0 {
            format!(
                "未完成 {}、已完成 {}、未知 {}、暂不可做 {}",
                counts.todo, counts.done, counts.unknown, counts.blocked
            )
        } else {
            format!(
                "未完成 {}、已完成 {}、未知 {}",
                counts.todo, counts.done, counts.unknown
            )
        };

        if self.verdict == RedpackVerdict::Failed {
            return if self.error.is_empty() {
                format!("红包派对失败（{tally}）")
            } else {
                format!("红包派对失败：{}（{tally}）", self.error)
            };
        }

        let mut line = format!("红包派对：{tally}");
        match &self.board.sign {
            Some(sign) if sign.done => line.push_str("；每日签到已完成"),
            Some(sign) if sign.wrote => line.push_str("；每日签到已提交"),
            _ => {}
        }
        if let Some(can) = self.board.balance_after.as_ref().and_then(|a| a.can_amount) {
            line.push_str(&format!("；余额可用 {}", yuan(can)));
        }
        if !self.board.manual.is_empty() {
            line.push_str(&format!("；需手动完成 {} 项", self.board.manual.len()));
        }
        if !self.board.writes.is_empty() {
            line.push_str(&format!("；本趟写请求 {} 条", self.board.writes.len()));
        }
        line
    }

    /// 映射到 [TaskOutcome]。
    pub fn task_outcome(&self) -> TaskOutcome {
        let summary = self.summary_line();
        match self.verdict {
            RedpackVerdict::Done => TaskOutcome::success(summary),
            RedpackVerdict::Failed => TaskOutcome::failed(summary),
        }
    }
}

/// 分 → 元。
///
/// **整数算术**：1234 分经 f64 除 100 再乘回去会出现 12.339999 这种尾数，
/// 而这一格是要念给用户看的钱数。
fn yuan(cents: i64) -> String {
    let (sign, value) = if cents < 0 {
        ("-", cents.unsigned_abs())
    } else {
        ("", cents as u64)
    };
    format!("{sign}{}.{:02} 元", value / 100, value % 100)
}

// ══════════════════════ 任务 ══════════════════════

/// 红包派对（会写：签到、登记任务完成、报浏览、交答案、装机、上报装机）。
#[derive(Default)]
pub struct CloudPhoneRedpackTask {
    /// 三处等待的时长，测试把它设成 0。
    pub plan: RedpackPlan,
}

impl CloudPhoneRedpackTask {
    /// 生产形态：真等待。
    pub fn production() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Task for CloudPhoneRedpackTask {
    fn name(&self) -> &'static str {
        REDPACK_TASK_NAME
    }

    fn title(&self) -> &'static str {
        REDPACK_TASK_TITLE
    }
    fn group(&self) -> TaskGroup {
        TaskGroup::Device
    }

    fn description(&self) -> &'static str {
        REDPACK_TASK_DESCRIPTION
    }

    async fn run(&self, ctx: &TaskContext, _body: &Value) -> TaskResult {
        // ⚠ 这里**曾经**有一道账号级总闸（`redpack_enabled`，默认 `true`；
        // 旧 TS 没有这一位，是本仓自己加的设置项）。2026-10-03 用户口径
        // 「**账号不应该有这个配置**」⇒ 字段已删。
        // 任务**是用户显式点起来的**，"跑不跑"这一层由那个动作表达。

        // ① 建会话（两段）：querySpecToken ⇒ tokenValidate。
        ctx.progress(1, 4, "建立红包派对会话");
        ctx.check_cancelled()?;
        let token = match login(ctx).await {
            Ok(token) => token,
            Err(err) => {
                report_fatal(ctx, &err.to_string());
                return Err(err);
            }
        };

        // ② 任务表是判据：读不到就不该继续（否则"没任务"会被误读成"没事做"）。
        ctx.progress(2, 4, "读取红包派对任务列表");
        ctx.check_cancelled()?;
        let task_list = match get_task_list(ctx, &token).await {
            Ok(list) => list,
            Err(err) => {
                report_fatal(ctx, &err.to_string());
                return Err(err);
            }
        };

        let mut board = RedpackBoard::default();
        // 余额是只读回执，读不到只记一句（TS logBalance 也吞错误，task.ts:136）。
        match account_info(ctx, &token).await {
            Ok(account) => board.balance_before = account,
            Err(err) => board.note(format!("余额没读到：{err}")),
        }

        // ③ 签到 + 逐个任务处置。
        ctx.progress(3, 4, "处理签到与任务");
        ctx.check_cancelled()?;
        let mut written: HashSet<String> = HashSet::new();
        handle_sign(ctx, &token, &task_list, &mut board, &mut written).await?;
        for task in flatten_tasks(&task_list) {
            ctx.check_cancelled()?;
            handle_task(ctx, &self.plan, &token, &task, &mut board, &mut written).await?;
        }

        // ④ 复读余额（TS 也是开始/结束各一次，task.ts:485、497）。
        ctx.progress(4, 4, "复读余额");
        ctx.check_cancelled()?;
        match account_info(ctx, &token).await {
            Ok(account) => board.balance_after = Some(account),
            Err(err) => board.note(format!("复读余额失败：{err}")),
        }

        let mut result = RedpackResult {
            verdict: RedpackVerdict::Done,
            board,
            error: String::new(),
        };
        if !result.board.failures.is_empty() {
            result.verdict = RedpackVerdict::Failed;
            result.error = result.board.failures.join("；");
        }
        report_details(ctx, &result);
        Ok(result.task_outcome())
    }
}

// ══════════════════════ 会话 ══════════════════════

/// 会话第 ①、② 段：SSO 票 ⇒ 这一族自己的 token。
///
/// 两段都失败即 Err —— 没有 token 时后面任何一枪都会回一个含义模糊的业务码，
/// 在这里断掉，错误才指向真原因（协议层 parse_token_validate 也是为此判空）。
async fn login(ctx: &TaskContext) -> Result<String> {
    let sso = call(
        ctx,
        proto::spec_token_request(ctx.clock.now_ms(), proto::REDPACK_SSO_SOURCE_ID),
        "红包派对换取 SSO 票",
        proto::parse_spec_token,
    )
    .await?;

    // ⚠ 换票那一枪的 token 头是空串、签名也按空串算（旧 TS getHeaders("")，
    // task.ts:123）。make_call(ctx, "") 同时喂头和签名，不可能出现两处不一致。
    let device_id = ctx.account.device_id.as_str().to_string();
    call(
        ctx,
        proto::token_validate_request(&make_call(ctx, ""), &sso, &device_id),
        "红包派对换取 token",
        proto::parse_token_validate,
    )
    .await
}

/// 造一次调用要带的四样"每次现算"的东西。
///
/// 时间只来自 ctx.clock.now_ms()（旧 TS 是 Date.now()，task.ts:58）；
/// requestId 的形状照旧 TS 的 yyyyMMddHHmmss + Date.now() + 8 位随机
/// （generateRequestId，task.ts:44-55）。
///
/// ⚠ 旧 TS 里 generateRequestId 的 Date.now() 与 getHeaders 的 timestamp 是
/// 两次调用，理论上能差 1 毫秒。这里两边用同一个 ts —— 签名里含 requestId，
/// 让"签名用的时间"与"头里的时间"永远是同一个值，比复刻那 1 毫秒的抖动更值得。
fn make_call(ctx: &TaskContext, token: &str) -> RedpackCall {
    let timestamp_ms = ctx.clock.now_ms();
    let mut rng = mcloud_crypto::OsRandom;
    let request_id = format!(
        "{}{timestamp_ms}{}",
        mcloud_core::beijing_time_digits(&*ctx.clock),
        base36(&mut rng, 8)
    );
    RedpackCall::new(token, timestamp_ms, request_id, base36(&mut rng, 16))
}

/// 小写 base36 随机片段（旧 TS：Math.random().toString(36).substring(2, 2+len)）。
///
/// ⚠ 分布不与 JS 一致：JS 那个小数尾数的位数分布并不均匀，这里按 36 取模逐字符抽。
/// 上游看到的只是"一串定长小写字母数字"，形状一致即可 —— 这一格不参与任何判据，
/// 而逐字节复刻浮点尾数的成本远大于收益。
fn base36(rng: &mut impl mcloud_crypto::Random, len: usize) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = vec![0u8; len];
    rng.fill_bytes(&mut buf);
    buf.iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect()
}

// ══════════════════════ HTTP 小助手 ══════════════════════

/// 发一枪并交给协议层的解析器。
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

/// 发一枪，只要"信封判成功"，不要 payload。
///
/// 那是旧 TS 对 userBrowse / getRcsToken / userInstallApp 的判据
/// （只 isOk(data)），这几条响应里没有本层要读的格子。
async fn call_ok(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<()> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    let envelope = RedpackEnvelope::parse(body)?;
    if !envelope.is_ok() {
        return Err(Error::Upstream {
            code: envelope_status_code(&envelope),
            message: envelope.error_text(what),
        });
    }
    Ok(())
}

/// 发一枪，交回原始 payload（协议层没给这一格专用解析器的那些端点用）。
///
/// 典型的两个是 instanceApkInstallStatus 与 apkInstall：它们的响应形状是
/// "嵌套数组里挑一格"，旧 TS 在编排层现读，所以本层也只在这里按需读。
/// 信封判据仍走协议层的 [RedpackEnvelope]，失败一律 Error::Upstream——
/// 不映射 AuthExpired（这一族票过期回什么码没有样本）。
async fn call_payload(ctx: &TaskContext, req: HttpRequest, what: &str) -> Result<Value> {
    let resp = ctx.http.request(req).await?;
    let body = mcloud_upstream::session::ensure_http_ok(&resp, what)?;
    let envelope = RedpackEnvelope::parse(body)?;
    if !envelope.is_ok() {
        return Err(Error::Upstream {
            code: envelope_status_code(&envelope),
            message: envelope.error_text(what),
        });
    }
    envelope
        .payload
        .ok_or_else(|| Error::Parse(format!("{what}：响应里没有 data")))
}

/// 把信封里的 status 按 JS String() 的语义转成文本（报错文案用）。
fn envelope_status_code(envelope: &RedpackEnvelope) -> String {
    match &envelope.status {
        Some(Value::String(s)) => s.clone(),
        Some(value) => value.to_string(),
        None => "-".to_string(),
    }
}

// ══════════════════════ 各条只读 ══════════════════════

async fn get_task_list(ctx: &TaskContext, token: &str) -> Result<RedpackTaskList> {
    call(
        ctx,
        proto::task_login_list_request(&make_call(ctx, token)),
        "红包派对任务列表",
        proto::parse_task_login_list,
    )
    .await
}

async fn account_info(ctx: &TaskContext, token: &str) -> Result<RedpackAccount> {
    call(
        ctx,
        proto::account_info_request(&make_call(ctx, token)),
        "红包派对余额",
        proto::parse_account_info,
    )
    .await
}

async fn topic_list(ctx: &TaskContext, token: &str) -> Result<Vec<RedpackTopic>> {
    call(
        ctx,
        proto::topic_list_request(&make_call(ctx, token)),
        "红包派对题库",
        proto::parse_topic_list,
    )
    .await
}

/// 拉两份云机列表（ACTIVE + PENDING）。
///
/// ⚠ 单份失败不中断：旧 TS 的 getCloudPhones 对每一份都是 continue
/// （task.ts:192）。这里同样继续，但把失败记进 notes —— 不静默。
async fn cloud_phones(ctx: &TaskContext, token: &str, board: &mut RedpackBoard) -> Vec<Value> {
    let mut phones = Vec::new();
    for status in [RedpackPhoneStatus::Active, RedpackPhoneStatus::Pending] {
        match call(
            ctx,
            proto::cloud_phones_request(&make_call(ctx, token), status),
            "红包派对云机列表",
            proto::parse_cloud_phones,
        )
        .await
        {
            Ok(mut items) => phones.append(&mut items),
            Err(err) => board.note(format!("云机列表（{}）没读到：{err}", status.as_str())),
        }
    }
    phones
}

// ══════════════════════ 签到 ══════════════════════

/// 每日签到那一格（旧 TS handleSign，task.ts:200-226）。
async fn handle_sign(
    ctx: &TaskContext,
    token: &str,
    task_list: &RedpackTaskList,
    board: &mut RedpackBoard,
    written: &mut HashSet<String>,
) -> Result<()> {
    let Some(today) = task_list.sign.iter().find(|item| item.is_today == Some(1)) else {
        return Ok(());
    };
    let mut record = RedpackSignRecord {
        status: today.status,
        ..Default::default()
    };

    match RedpackTaskState::classify(today.status) {
        RedpackTaskState::Done => record.note = "上游说已完成".to_string(),
        RedpackTaskState::Blocked => {
            record.note = format!("暂不可做（{}）", status_text_or(today.status));
        }
        RedpackTaskState::Unknown => {
            record.note = unknown_status_note(today.status);
        }
        RedpackTaskState::Todo => {
            if !written.insert("SIGN_1".to_string()) {
                record.note = "本趟同一个签到已经写过一次，跳过".to_string();
            } else {
                record.wrote = true;
                board.writes.push("userSign".to_string());
                ctx.check_cancelled()?;
                match call(
                    ctx,
                    proto::user_sign_request(&make_call(ctx, token)),
                    "红包派对每日签到",
                    proto::parse_write_status,
                )
                .await
                {
                    // 旧 TS 对签到是严格判据：必须 data.status === 1
                    // （task.ts:220），缺失不算成功。
                    Ok(Some(1)) => {
                        record.done = true;
                        record.note = "已完成".to_string();
                        log_reward(ctx, token, "SIGN_1", "每日签到", board).await;
                    }
                    Ok(other) => {
                        record.note = "签到没成功".to_string();
                        board.fail(format!("每日签到失败：上游 status={}", opt_i64_text(other)));
                    }
                    Err(err) => {
                        record.note = "签到没成功".to_string();
                        board.fail(format!("每日签到失败：{err}"));
                    }
                }
            }
        }
    }

    board.sign = Some(record);
    Ok(())
}

// ══════════════════════ 逐条任务 ══════════════════════

/// 把三组任务按旧 TS 的顺序摊平（新手 → 每日 → 每月，task.ts:487-491）。
fn flatten_tasks(task_list: &RedpackTaskList) -> Vec<RedpackTask> {
    task_list
        .novice
        .iter()
        .chain(task_list.daily.iter())
        .chain(task_list.monthly.iter())
        .cloned()
        .collect()
}

/// 处置一条任务（旧 TS handleTask，task.ts:437-473）。
async fn handle_task(
    ctx: &TaskContext,
    plan: &RedpackPlan,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
    written: &mut HashSet<String>,
) -> Result<()> {
    let code = task.task_code.clone().unwrap_or_default();
    let state = RedpackTaskState::classify(task.user_status);
    let mut entry = RedpackTaskEntry {
        id: task.id.clone(),
        task_code: code.clone(),
        task_name: task.task_name.clone(),
        status: task.user_status,
        state,
        handled: false,
        note: String::new(),
    };

    match state {
        RedpackTaskState::Done => entry.note = "上游说已完成".to_string(),
        RedpackTaskState::Blocked => {
            entry.note = format!("暂不可做（{}）", status_text_or(task.user_status));
        }
        RedpackTaskState::Unknown => entry.note = unknown_status_note(task.user_status),
        RedpackTaskState::Todo => {
            // ⚠ 判据就是"这一格是不是 0"。缺 taskCode 的条目没有分派键 ⇒ 不发。
            if code.is_empty() {
                entry.note = "上游没给 taskCode ⇒ 不写".to_string();
            } else if !written.insert(code.clone()) {
                entry.note = "本趟同一个 taskCode 已经写过一次，跳过".to_string();
            } else {
                let writes_before = board.writes.len();
                dispatch(ctx, plan, token, task, board, &mut entry).await?;
                entry.handled = board.writes.len() > writes_before;
            }
        }
    }

    board.tasks.push(entry);
    Ok(())
}

/// 按 taskCode 分派到具体那条链（旧 TS handleTask 的 if 串，task.ts:451-472）。
async fn dispatch(
    ctx: &TaskContext,
    plan: &RedpackPlan,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
    entry: &mut RedpackTaskEntry,
) -> Result<()> {
    let code = entry.task_code.clone();
    if BROWSE_TASKS.contains(&code.as_str()) {
        finish_task(ctx, plan, token, task, true, board, entry).await
    } else if DIRECT_TASKS.contains(&code.as_str()) {
        finish_task(ctx, plan, token, task, false, board, entry).await
    } else if code == "DAILY_1" {
        complete_cloud_use(ctx, token, task, board, entry).await
    } else if code == "MONTHLY_2" {
        install_app(ctx, plan, token, task, board, entry).await
    } else if code == "MONTHLY_3" {
        answer_topic(ctx, plan, token, task, board, entry).await
    } else {
        let reason = MANUAL_TASKS
            .iter()
            .find(|(manual_code, _)| *manual_code == code.as_str())
            .map(|(_, reason)| *reason)
            .unwrap_or("未知任务");
        entry.note = format!("需手动完成（{reason}）");
        board.manual(format!("{}：{reason}", task_title(task)));
        Ok(())
    }
}

/// userCompleteTask 那一枪（旧 TS completeAction，task.ts:165-181 / finishTask 前半）。
///
/// ⚠ 判据是宽松的：!isNil(status) && status !== 1 才算失败
/// （task.ts:176、239），即"读到且不是 1"才失败、缺失放行。与签到的严格判据不同，
/// 两处都是照抄。
///
/// 成功回 true；失败已记进 board.failures，调用方不必再记一次。
async fn complete_task(
    ctx: &TaskContext,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
) -> Result<bool> {
    let title = task_title(task);
    // ⚠ 旧 TS 原样把 task.id 发回上游（可能是数也可能是串，task.ts:169）；
    // 但"没有 id"时它发的是一个不存在的键。我们不改成 null —— 那会被
    // 读成另一种意思 —— 而是直接不发并记一档失败。
    let Some(task_id) = task.id.clone() else {
        board.fail(format!("{title}：上游没给 task.id，不发 userCompleteTask"));
        return Ok(false);
    };
    board.writes.push(format!(
        "userCompleteTask {}",
        task.task_code.clone().unwrap_or_default()
    ));
    match call(
        ctx,
        proto::complete_task_request(&make_call(ctx, token), &task_id),
        "红包派对登记任务完成",
        proto::parse_write_status,
    )
    .await
    {
        Ok(None) | Ok(Some(proto::REDPACK_STATUS_DONE)) => Ok(true),
        Ok(Some(status)) => {
            board.fail(format!("{title}：userCompleteTask 回 status={status}"));
            Ok(false)
        }
        Err(err) => {
            board.fail(format!("{title}：userCompleteTask 没成功（{err}）"));
            Ok(false)
        }
    }
}

/// 登记完 +（可选）浏览确认，再回查最终状态（旧 TS finishTask，task.ts:228-265）。
async fn finish_task(
    ctx: &TaskContext,
    plan: &RedpackPlan,
    token: &str,
    task: &RedpackTask,
    browse: bool,
    board: &mut RedpackBoard,
    entry: &mut RedpackTaskEntry,
) -> Result<()> {
    if !complete_task(ctx, token, task, board).await? {
        entry.note = "userCompleteTask 没成功".to_string();
        return Ok(());
    }

    if browse {
        // 旧 TS 先等 15 秒再报浏览（task.ts:244）—— 服务端要时间把那笔记上。
        if plan.browse_wait_ms > 0 {
            ctx.check_cancelled()?;
            tokio::time::sleep(Duration::from_millis(plan.browse_wait_ms)).await;
        }
        ctx.check_cancelled()?;
        board.writes.push(format!("userBrowse {}", entry.task_code));
        if let Err(err) = call_ok(
            ctx,
            proto::browse_request(&make_call(ctx, token), &entry.task_code),
            "红包派对浏览确认",
        )
        .await
        {
            board.fail(format!("浏览确认失败（{}）：{err}", task_title(task)));
            entry.note = "浏览确认失败".to_string();
            return Ok(());
        }
    }

    settle(ctx, token, task, board, entry).await
}

/// 写完之后回查一次任务表，把最终状态落到 entry / board。
///
/// 旧 TS 的 finishTask / completeCloudUse 都以"回查后 userStatus==1"作为
/// "真的成功"的判据（task.ts:256-262、377-384）；回查失败它退回原任务，
/// 这里同样不把回查失败当硬失败，只记一句。
async fn settle(
    ctx: &TaskContext,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
    entry: &mut RedpackTaskEntry,
) -> Result<()> {
    let refreshed = refresh_status(ctx, token, &entry.task_code, board).await;
    if refreshed == Some(proto::REDPACK_STATUS_DONE) {
        entry.note = "已完成".to_string();
        let title = task_title(task);
        log_reward(ctx, token, &entry.task_code, &title, board).await;
    } else {
        entry.note = format!("已登记，回查状态 {}", opt_i64_text(refreshed));
    }
    Ok(())
}

/// 重取任务表并挑出某个码的 userStatus；失败只记一句、回 None。
async fn refresh_status(
    ctx: &TaskContext,
    token: &str,
    task_code: &str,
    board: &mut RedpackBoard,
) -> Option<i64> {
    match get_task_list(ctx, token).await {
        Ok(list) => list.find(task_code).and_then(|task| task.user_status),
        Err(err) => {
            board.note(format!("回查任务列表失败（{task_code}）：{err}"));
            None
        }
    }
}

/// DAILY_1：登记完成 ⇒ 摸一次云机（换 RCS 票）⇒ 回查（旧 TS completeCloudUse，task.ts:373-385）。
async fn complete_cloud_use(
    ctx: &TaskContext,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
    entry: &mut RedpackTaskEntry,
) -> Result<()> {
    if !complete_task(ctx, token, task, board).await? {
        entry.note = "userCompleteTask 没成功".to_string();
        return Ok(());
    }
    let phones = cloud_phones(ctx, token, board).await;
    if !phones.is_empty() {
        // ⚠ 有云机才换票（旧 TS if (phones.length)，task.ts:376）——
        // 没有云机时换来的票没有对应主体。
        board.writes.push("getRcsToken 0".to_string());
        if let Err(err) = call_ok(
            ctx,
            proto::rcs_token_request(&make_call(ctx, token), "0"),
            "红包派对换取 RCS 票",
        )
        .await
        {
            board.note(format!("getRcsToken 没成功（不影响判定）：{err}"));
        }
    }

    let refreshed = refresh_status(ctx, token, &entry.task_code, board).await;
    if refreshed == Some(proto::REDPACK_STATUS_DONE) {
        entry.note = "已完成".to_string();
        let title = task_title(task);
        log_reward(ctx, token, &entry.task_code, &title, board).await;
    } else {
        entry.note = format!(
            "需手动完成（需云机内实际使用；回查状态 {}）",
            opt_i64_text(refreshed)
        );
        board.manual(format!("{}：需云机内实际使用", task_title(task)));
    }
    Ok(())
}

/// MONTHLY_3：登记 ⇒ 取题库 ⇒ 交答案（旧 TS answerTopic，task.ts:387-435）。
async fn answer_topic(
    ctx: &TaskContext,
    plan: &RedpackPlan,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
    entry: &mut RedpackTaskEntry,
) -> Result<()> {
    if !complete_task(ctx, token, task, board).await? {
        entry.note = "userCompleteTask 没成功".to_string();
        return Ok(());
    }

    let mut topic: Option<RedpackTopic> = None;
    let mut options: Vec<String> = Vec::new();
    let mut answer_text = String::new();
    let mut question = String::new();
    for _ in 0..plan.topic_attempts {
        ctx.check_cancelled()?;
        let topics = match topic_list(ctx, token).await {
            Ok(topics) => topics,
            Err(err) => {
                board.fail(format!("{}：题库没读到（{err}）", task_title(task)));
                entry.note = "题库没读到".to_string();
                return Ok(());
            }
        };
        let Some(first) = topics.into_iter().next() else {
            board.fail(format!("{}：题库为空", task_title(task)));
            entry.note = "题库为空".to_string();
            return Ok(());
        };
        question = first.topic_content.clone().unwrap_or_default();
        answer_text = KNOWN_ANSWERS
            .iter()
            .find(|(known, _)| *known == question.as_str())
            .map(|(_, answer)| (*answer).to_string())
            .unwrap_or_default();
        options = first
            .topic_option
            .as_deref()
            .map(proto::parse_topic_options)
            .unwrap_or_default();
        topic = Some(first);
        if !answer_text.is_empty() && options.contains(&answer_text) {
            break;
        }
        // 旧 TS 每轮都等 150ms 再重取（task.ts:413），最后一轮也不例外。
        if plan.topic_interval_ms > 0 {
            ctx.check_cancelled()?;
            tokio::time::sleep(Duration::from_millis(plan.topic_interval_ms)).await;
        }
    }

    let Some(topic) = topic else {
        entry.note = "题库为空".to_string();
        board.manual(format!("{}：未知题目（题库为空）", task_title(task)));
        return Ok(());
    };
    if answer_text.is_empty() || !options.contains(&answer_text) {
        // ⚠ 查不到答案 / 答案不在选项里 ⇒ 不猜，交人工（旧 TS 同一分支，task.ts:415）。
        entry.note = format!("需手动完成（未知题目: {question}）");
        board.manual(format!("{}：未知题目（{question}）", task_title(task)));
        return Ok(());
    }
    let index = options
        .iter()
        .position(|option| option == &answer_text)
        .unwrap_or(0);
    let Some(letter) = ANSWER_LETTERS.get(index) else {
        board.fail(format!(
            "{}：答案在第 {} 个选项，超出 ABCD 能表达的范围",
            task_title(task),
            index + 1
        ));
        entry.note = "选项下标超出 ABCD".to_string();
        return Ok(());
    };

    // ⚠ 这两个 id 是旧 TS 里唯二被取整的地方（Number(task.id) /
    // Number(topic.id)，task.ts:423-424）—— 与 userCompleteTask 的原样透传
    // 刻意不同，不要"统一一下"。
    let (Some(task_id), Some(topic_id)) = (
        as_number_i64(task.id.as_ref()),
        as_number_i64(topic.id.as_ref()),
    ) else {
        board.fail(format!(
            "{}：答题需要整数 taskId / topicId，上游给的不是数",
            task_title(task)
        ));
        entry.note = "taskId / topicId 不是整数".to_string();
        return Ok(());
    };

    board
        .writes
        .push(format!("userTopicAnswer {}", entry.task_code));
    match call(
        ctx,
        proto::answer_topic_request(
            &make_call(ctx, token),
            task_id,
            topic_id,
            &letter.to_string(),
        ),
        "红包派对交答案",
        proto::parse_write_status,
    )
    .await
    {
        // 旧 TS 对答题也是严格判据（data.status === 1，task.ts:428）。
        Ok(Some(proto::REDPACK_STATUS_DONE)) => {
            entry.note = "已完成".to_string();
            let title = task_title(task);
            log_reward(ctx, token, &entry.task_code, &title, board).await;
        }
        Ok(other) => {
            board.fail(format!(
                "{}：答题没成功（status={}）",
                task_title(task),
                opt_i64_text(other)
            ));
            entry.note = "答题失败".to_string();
        }
        Err(err) => {
            board.fail(format!("{}：答题失败（{err}）", task_title(task)));
            entry.note = "答题失败".to_string();
        }
    }
    Ok(())
}

/// MONTHLY_2：登记 ⇒ 挑云机 ⇒ 装应用 ⇒ 上报（旧 TS installApp，task.ts:267-371）。
async fn install_app(
    ctx: &TaskContext,
    plan: &RedpackPlan,
    token: &str,
    task: &RedpackTask,
    board: &mut RedpackBoard,
    entry: &mut RedpackTaskEntry,
) -> Result<()> {
    let title = task_title(task);
    if !complete_task(ctx, token, task, board).await? {
        entry.note = "userCompleteTask 没成功".to_string();
        return Ok(());
    }

    let phones = cloud_phones(ctx, token, board).await;
    if phones.is_empty() {
        entry.note = "需手动完成（无云手机）".to_string();
        board.manual(format!("{title}：无云手机"));
        return Ok(());
    }
    let Some((instance_id, locked)) = pick_phone(&phones) else {
        board.fail(format!("{title}：云机列表里读不出 instanceId"));
        entry.note = "云机列表里读不出 instanceId".to_string();
        return Ok(());
    };
    if locked {
        entry.note = "需手动完成（云机锁屏）".to_string();
        board.manual(format!("{title}：云机锁屏"));
        return Ok(());
    }

    let apps = match app_list(ctx, token).await {
        Ok(apps) => apps,
        Err(err) => {
            board.fail(format!("{title}：应用列表没读到（{err}）"));
            entry.note = "应用列表没读到".to_string();
            return Ok(());
        }
    };
    if apps.is_empty() {
        board.fail(format!("{title}：应用列表为空"));
        entry.note = "应用列表为空".to_string();
        return Ok(());
    }
    let hw_token = match hw_token(ctx, token).await {
        Ok(Some(hw_token)) if !hw_token.is_empty() => hw_token,
        Ok(_) => {
            board.fail(format!("{title}：hwToken 获取失败"));
            entry.note = "hwToken 获取失败".to_string();
            return Ok(());
        }
        Err(err) => {
            board.fail(format!("{title}：hwToken 没读到（{err}）"));
            entry.note = "hwToken 获取失败".to_string();
            return Ok(());
        }
    };

    for app in &apps {
        ctx.check_cancelled()?;
        let Some(package_name) = app
            .get("packageName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        // ⚠ 两个 id 来源不同：装机用 String(packageId)，上报用 id
        // （协议层 apk_install_request / report_install_app_request 的说明）。
        let package_id = app
            .get("packageId")
            .map(value_as_js_string)
            .filter(|id| !id.is_empty());
        let Some(package_id) = package_id else {
            board.note(format!("应用 {package_name} 没有 packageId，跳过"));
            continue;
        };
        let Some(app_id) = app.get("id").cloned() else {
            board.note(format!("应用 {package_name} 没有 id，跳过"));
            continue;
        };

        // 已经装好（installStatus == 2）⇒ 直接上报，不用再装。
        match installed_already(ctx, token, &hw_token, &instance_id, package_name).await {
            Ok(true) => {
                if report_install(ctx, token, task, &app_id, Some(1), board).await? {
                    entry.note = "已完成".to_string();
                    log_reward(ctx, token, &entry.task_code, &title, board).await;
                    return Ok(());
                }
                continue;
            }
            Ok(false) => {}
            // 旧 TS 在这里读不到就当"没装过"，继续走装机（task.ts:311-323）。
            Err(err) => board.note(format!("装机状态没读到（{package_name}）：{err}")),
        }

        board.writes.push(format!("apkInstall {package_name}"));
        let install = match call_payload(
            ctx,
            proto::apk_install_request(
                &make_call(ctx, token),
                &instance_id,
                &package_id,
                &hw_token,
            ),
            "红包派对云机装应用",
        )
        .await
        {
            Ok(payload) => payload,
            Err(err) => {
                board.note(format!("apkInstall 没成功（{package_name}）：{err}"));
                continue;
            }
        };
        if !js_truthy(install.get("success").unwrap_or(&Value::Null)) {
            continue;
        }
        let install_task_id = install
            .get("taskId")
            .map(value_as_js_string)
            .unwrap_or_default();
        if !poll_install(ctx, plan, token, &hw_token, &install_task_id).await? {
            continue;
        }
        if report_install(ctx, token, task, &app_id, None, board).await? {
            entry.note = "已完成".to_string();
            log_reward(ctx, token, &entry.task_code, &title, board).await;
            return Ok(());
        }
        let app_name = app
            .get("apkName")
            .and_then(Value::as_str)
            .unwrap_or(package_name);
        entry.note = format!("已安装应用 {app_name}");
        return Ok(());
    }

    board.fail(format!("{title}：未找到可安装应用"));
    entry.note = "未找到可安装应用".to_string();
    Ok(())
}

/// userInstallApp + 回查；回 true 表示回查后 userStatus == 1。
async fn report_install(
    ctx: &TaskContext,
    token: &str,
    task: &RedpackTask,
    app_id: &Value,
    existed: Option<i64>,
    board: &mut RedpackBoard,
) -> Result<bool> {
    let title = task_title(task);
    let Some(task_id) = task.id.clone() else {
        board.fail(format!("{title}：上游没给 task.id，不发 userInstallApp"));
        return Ok(false);
    };
    let task_code = task.task_code.clone().unwrap_or_default();
    board.writes.push(format!("userInstallApp {task_code}"));
    // ⚠ 旧 TS 不看这一枪的响应体（task.ts:315、353）；这里同样只判信封，
    // 判失败就记一档、不把它当"领到了"。
    if let Err(err) = call_ok(
        ctx,
        proto::report_install_app_request(&make_call(ctx, token), &task_id, app_id, existed),
        "红包派对上报装应用",
    )
    .await
    {
        board.fail(format!("{title}：userInstallApp 没成功（{err}）"));
        return Ok(false);
    }
    let refreshed = refresh_status(ctx, token, &task_code, board).await;
    Ok(refreshed == Some(proto::REDPACK_STATUS_DONE))
}

/// 轮询装机异步任务：taskStatus == 2 是完成、== 3 是失败（旧 TS task.ts:336-351）。
async fn poll_install(
    ctx: &TaskContext,
    plan: &RedpackPlan,
    token: &str,
    hw_token: &str,
    install_task_id: &str,
) -> Result<bool> {
    for attempt in 0..plan.install_poll_attempts {
        ctx.check_cancelled()?;
        match call_payload(
            ctx,
            proto::install_task_status_request(&make_call(ctx, token), install_task_id, hw_token),
            "红包派对装机进度",
        )
        .await
        {
            Ok(payload) => match payload.get("taskStatus").and_then(Value::as_i64) {
                Some(2) => return Ok(true),
                Some(3) => return Ok(false),
                _ => {}
            },
            // 旧 TS 对单次轮询失败也是继续（tryResponse 吞掉），照做。
            Err(err) => ctx.info(format!("装机进度没读到（第 {} 轮）：{err}", attempt + 1)),
        }
        if plan.install_poll_interval_ms > 0 {
            ctx.check_cancelled()?;
            tokio::time::sleep(Duration::from_millis(plan.install_poll_interval_ms)).await;
        }
    }
    Ok(false)
}

/// 这个包在这台云机上装没装（旧 TS task.ts:303-323 的那段取数）。
async fn installed_already(
    ctx: &TaskContext,
    token: &str,
    hw_token: &str,
    instance_id: &str,
    package_name: &str,
) -> Result<bool> {
    let payload = call_payload(
        ctx,
        proto::install_status_request(&make_call(ctx, token), hw_token, instance_id, package_name),
        "红包派对装机状态",
    )
    .await?;
    // 旧 TS：data 是数组就用它，否则用 data.list（task.ts:311-313）。
    let list: Vec<&Value> = match &payload {
        Value::Array(items) => items.iter().collect(),
        other => other
            .get("list")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default(),
    };
    let status = list
        .first()
        .and_then(|item| item.get("instanceList"))
        .and_then(Value::as_array)
        .and_then(|instances| instances.first())
        .and_then(|instance| instance.get("installStatus"));
    Ok(status.and_then(Value::as_i64) == Some(2))
}

/// 挑一台没锁屏的云机；全锁了就用第一台（旧 TS task.ts:274-284）。
///
/// 回 (instanceId, 是否锁屏)：instanceId 为空时也是失败，由调用方判。
fn pick_phone(phones: &[Value]) -> Option<(String, bool)> {
    let parse = |phone: &Value| -> (String, bool) {
        let instance_id = phone
            .get("instanceId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let locked = phone
            .get("subsProdInstances")
            .and_then(Value::as_array)
            .and_then(|instances| instances.first())
            .and_then(|instance| instance.get("lockStatus"))
            .and_then(Value::as_str)
            == Some("Y");
        (instance_id, locked)
    };
    phones
        .iter()
        .map(parse)
        .find(|(_, locked)| !*locked)
        .or_else(|| phones.first().map(parse))
}

/// userToastInfo：某条任务最近拿到的奖，只进明细/小结，不参与判定。
///
/// 旧 TS 在 !prize.prizeAmount 时什么都不打（task.ts:152）——
/// "0 或没给"都算"这格没奖可报"，这里同样只在真读到非 0 金额时记一条。
async fn log_reward(
    ctx: &TaskContext,
    token: &str,
    task_code: &str,
    default_name: &str,
    board: &mut RedpackBoard,
) {
    match call(
        ctx,
        proto::toast_info_request(&make_call(ctx, token), task_code),
        "红包派对奖励回执",
        proto::parse_toast_info,
    )
    .await
    {
        Ok(prize) => match prize.prize_amount {
            Some(amount) if amount != 0 => board.rewards.push(RedpackReward {
                task_code: task_code.to_string(),
                prize_name: prize.prize_name.or_else(|| Some(default_name.to_string())),
                prize_amount: Some(amount),
            }),
            _ => {}
        },
        Err(err) => board.note(format!("奖励回执没读到（{task_code}）：{err}")),
    }
}

async fn app_list(ctx: &TaskContext, token: &str) -> Result<Vec<Value>> {
    call(
        ctx,
        proto::app_list_request(&make_call(ctx, token)),
        "红包派对应用列表",
        proto::parse_app_list,
    )
    .await
}

async fn hw_token(ctx: &TaskContext, token: &str) -> Result<Option<String>> {
    call(
        ctx,
        proto::hw_token_request(&make_call(ctx, token)),
        "红包派对 hwToken",
        proto::parse_hw_token,
    )
    .await
}

// ══════════════════════ 小工具 ══════════════════════

/// 任务名（没有就退回任务码，再没有就一句占位）——只用于人话与错误文案。
fn task_title(task: &RedpackTask) -> String {
    task.task_name
        .clone()
        .filter(|name| !name.is_empty())
        .or_else(|| task.task_code.clone())
        .unwrap_or_else(|| "未命名任务".to_string())
}

/// 已认识的码翻文案；None 与没见过的码都说"读不出"。
fn status_text_or(status: Option<i64>) -> String {
    status
        .and_then(proto::redpack_status_text)
        .unwrap_or("状态读不出")
        .to_string()
}

/// 未知状态那一档的原话（单独一处，免得同一句话写两遍）。
///
/// 只有"读到没见过的码"才会走到这里（缺格按 0，见 classify）。
fn unknown_status_note(status: Option<i64>) -> String {
    match status {
        Some(code) => {
            format!("上游给了这一族没见过的状态码 {code}，既不当未完成也不当已完成 ⇒ 不写")
        }
        None => "上游没给 userStatus ⇒ 照旧 TS 的 ?? 0 按未完成处理".to_string(),
    }
}

fn opt_i64_text(status: Option<i64>) -> String {
    match status {
        Some(code) => code.to_string(),
        None => "读不出".to_string(),
    }
}

/// Number(x) 的整数版：数、或数字串都认，其余回 None。
///
/// 旧 TS 在答题那两个 id 上用的是 Number(...)（task.ts:423-424），
/// 数字串 "31" 要读成 31；非数字串在 JS 里是 NaN（发出去会变 null）——
/// 那种情况这里直接不发。
fn as_number_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    value
        .as_str()
        .and_then(|text| text.trim().parse::<i64>().ok())
}

/// String(x) 的近似：数与串都转文本，其余回空串（调用方按"没有"处理）。
///
/// ⚠ 与 JS 的 String() 有一处差别：null / 对象在 JS 里会得到
/// "null" / "[object Object]" 这种"看起来像个值"的垃圾串。这里回空串，
/// 让调用方跳过 —— 往云机上装一个叫 "undefined" 的包没有任何意义。
fn value_as_js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

/// JS 的真值判断（if (success)，task.ts:333）。
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|f| f != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

// ══════════════════════ 明细 ══════════════════════

/// 会话/任务表这种致命读失败时也交一份明细 —— 不留"什么都没有"的运行。
fn report_fatal(ctx: &TaskContext, error: &str) {
    ctx.details(json!({
        "kind": REDPACK_TASK_NAME,
        "verdict": "failed",
        "error": error,
    }));
}

/// 把这一趟的读数与处置交给明细通道。
///
/// 只读部分（余额、奖励、任务状态）与写入部分（发过哪些枪、哪一步失败、
/// 哪些要人工）都在这里，小结那一句刻意压缩，任务级明细只能走这里。
/// ⚠ 不含 token / SSO 票。
fn report_details(ctx: &TaskContext, result: &RedpackResult) {
    let counts = result.board.counts();
    let tasks: Vec<Value> = result
        .board
        .tasks
        .iter()
        .map(|task| {
            json!({
                "taskId": task.id,
                "taskCode": task.task_code,
                "taskName": task.task_name,
                "userStatus": task.status,
                "statusText": task.status.and_then(proto::redpack_status_text),
                "state": task.state.as_str(),
                "handled": task.handled,
                "note": task.note,
            })
        })
        .collect();
    let sign = result.board.sign.as_ref().map(|sign| {
        json!({
            "userStatus": sign.status,
            "statusText": sign.status.and_then(proto::redpack_status_text),
            "done": sign.done,
            "wrote": sign.wrote,
            "note": sign.note,
        })
    });
    let rewards: Vec<Value> = result
        .board
        .rewards
        .iter()
        .map(|reward| {
            json!({
                "taskCode": reward.task_code,
                "prizeName": reward.prize_name,
                "prizeAmount": reward.prize_amount,
            })
        })
        .collect();
    ctx.details(json!({
        "kind": REDPACK_TASK_NAME,
        "verdict": match result.verdict {
            RedpackVerdict::Done => "done",
            RedpackVerdict::Failed => "failed",
        },
        "error": result.error,
        "counts": {
            "total": counts.total,
            "todo": counts.todo,
            "done": counts.done,
            "blocked": counts.blocked,
            "unknown": counts.unknown,
        },
        "sign": sign,
        "balanceBefore": {
            "canAmount": result.board.balance_before.can_amount,
            "totalAmount": result.board.balance_before.total_amount,
        },
        "balanceAfter": result.board.balance_after.as_ref().map(|account| json!({
            "canAmount": account.can_amount,
            "totalAmount": account.total_amount,
        })),
        "writes": result.board.writes,
        "manual": result.board.manual,
        "failures": result.board.failures,
        "notes": result.board.notes,
        "rewards": rewards,
        "tasks": tasks,
    }));
}

#[cfg(test)]
mod tests;
