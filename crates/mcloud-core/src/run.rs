//! 运行语义：`RunId`、运行结果四态、以及"需人工"的结构化出口。
//!
//! # 为什么单独成模块
//!
//! 这四条语义（`FieldSpec` 参数 / 三类错误 / `runId` 幂等 / `ManualRequired`）
//! 是所有 19 项能力的**公共判据**。旧项目把它们各自散在 6+ 个文件里，
//! 结果是同一个概念有 3 份实现、且互相矛盾（详见 `docs/business-feature-inventory.md` §1）。
//! 这里钉成唯一来源，能力层只引用、不重定义。
//!
//! # 与 [`crate::model::RunStatus`] 的分工
//!
//! [`RunStatus`] 是**存储层的生命周期**：`Running` → `Done` / `Failed`，
//! 它回答"这一行还在跑吗"。而 [`RunOutcome`] 是**业务层的终局判定**，
//! 它回答"这一趟到底算成没成、要不要叫人"。
//!
//! 两者**不是**一对一的：`Done` 可以对应三种结果
//! （全成 / 部分成 / 需人工），把它们压成"成功"正是旧项目最要命的失真 ——
//! `logger.info("需手动完成: …")` 之后仍然 `return true`，
//! 于是前端与推送都以为一切正常（`docs/business-feature-inventory.md` §1.4）。
//!
//! ```text
//! RunStatus::Running ──┬── Done   ── RunOutcome::{Success, PartialSuccess, ManualRequired}
//!                      └── Failed ── RunOutcome::Failed
//! ```
//!
//! # 事件模型（[`Seq`] / [`RunEvent`] / [`Sequenced`]）
//!
//! 模块后半是**运行过程中的事件流**。它与上面的"终局判定"是两条互补的线：
//! `RunOutcome` 回答"这一趟算成没成"，事件流回答"这一趟**是怎么跑的**"。
//!
//! 旧项目把它放在内存里（`api/src/runner.ts` 的 `AsyncRunState`），
//! 于是有三个洞（进程重启即丢、晚连的人看不到前面、断线无法续传）。
//! 这里的 [`Seq`] 就是为"续传"设计的：客户端把最后收到的序号带回来，
//! 服务端接着发 —— 序号一旦重复或跳号，前端就会显示一个**错的进度**，
//! 而且是静默的。所以序号语义被钉成一个独立的类型，而不是随手一个 `u64`。

use serde::{Deserialize, Serialize};

use crate::crypto::Random;

/// 一次运行的幂等标识（UUID v4）。
///
/// # 为什么用 newtype 而不是裸 `String`
///
/// `runId` 是要**落库当主键**、还要进 URL（`GET /api/runs/:id/events`）的东西。
/// 用 newtype 挡住两种很容易犯的错：把「账号 id」或「能力 code」误传进需要 runId 的位置 ——
/// 这三者在旧项目里都是字符串，编译器分不出来。
///
/// 构造只走 [`RunId::new`]（从注入的 [`Random`] 取），不提供 `From<String>`：
/// 从外部字符串进来的东西应当走 [`RunId::parse`] 做形状校验，
/// 否则一个手写的 `"abc"` 也能当 runId 用，幂等闸就形同虚设。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    /// 生成一个新的运行标识。
    ///
    /// 随机源从 [`Random`] 注入 —— 与 [`crate::user::new_session_token`] 同一个理由：
    /// 测试里要能钉住"同一个 runId 重放"这条语义，直接取全局随机源就做不到。
    pub fn new(rng: &mut impl Random) -> Self {
        Self(rng.uuid_v4())
    }

    /// 从已存库的字符串还原。
    ///
    /// **只做形状校验，不做语义校验** —— 是否真的存在这个 run 由存储层回答。
    /// 形状按 UUID v4 的规范形式（36 字符、4 个连字符），
    /// 这样 `parse` 失败就意味着"这串东西不可能是我们生成的"，
    /// 调用方可以直接判 404，不必再去查库。
    pub fn parse(raw: &str) -> Option<Self> {
        if !is_uuid_v4_shape(raw) {
            return None;
        }
        Some(Self(raw.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 校验 UUID v4 的规范字符串形状：`8-4-4-4-12` 十六进制，且版本位为 `4`。
///
/// 只认小写与大写混排的十六进制（比较时统一转小写），
/// **不接受** braced / urn / 无连字符等变体 —— 那些我们从不生成，
/// 放宽只会让"这不是我们的 id"这类判断失效。
fn is_uuid_v4_shape(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        let expect_dash = matches!(i, 8 | 13 | 18 | 23);
        if expect_dash {
            if *c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    // 版本位固定为 4（与 `Random::uuid_v4` 里 `b[6] = ..| 0x40` 一致）
    b[14] == b'4'
}

/// 运行的终局判定。
///
/// 四态是**业务结论**，不是错误分类：`ManualRequired` 与 `PartialSuccess` 都不是失败。
/// 它们在旧项目里被压成"成功 + 一条 info 日志"，于是没人知道该去 App 里点一下。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// 全部目标达成。
    Success,
    /// 有目标没达成，但**没有需要人做的事** —— 重跑或等下一轮即可。
    ///
    /// 典型：一个账号成功、另一个风控退避。
    PartialSuccess,
    /// 上游要求**人去客户端操作**才能继续，程序无法代劳。
    ///
    /// 典型：`pushOn === 0`（要先去 App 打开消息通知）、
    /// `未开启备份`（要先去 App 打开备份开关）。详见 [`ManualStep`]。
    ///
    /// 这一态**必须**能进推送与历史 —— 否则"需要你动一下手"这件事就永远传不到人。
    ManualRequired,
    /// 彻底失败。与 [`RunOutcome::PartialSuccess`] 的区别是"一个都没成"。
    Failed,
}

impl RunOutcome {
    /// 是否算"没白跑"（部分成功也算有产出）。
    ///
    /// 注意 [`RunOutcome::ManualRequired`] **不算成功** ——
    /// 它意味着目标尚未达成，只是再跑一遍也没用。把它算成功正是旧项目的失真点。
    pub const fn is_success_like(self) -> bool {
        matches!(self, Self::Success | Self::PartialSuccess)
    }

    /// 是否需要通知到人（进推送队列）。
    ///
    /// `PartialSuccess` 也返回 `true`：部分失败同样值得人知道，
    /// 但**不需要人动手** —— 用 [`Self::needs_human_action`] 区分这两件事。
    pub const fn needs_notification(self) -> bool {
        !matches!(self, Self::Success)
    }

    /// 是否必须由人去客户端操作才能推进。
    pub const fn needs_human_action(self) -> bool {
        matches!(self, Self::ManualRequired)
    }

    /// 从 [`crate::model::RunStatus`] 与本态的映射回推 —— 供存储层在只有
    /// status 一列时的**退路**使用。
    ///
    /// 这是有损的：`Done` 无法区分三种成功态，所以只用于读**旧数据**
    /// （旧库没有 outcome 列）。新写入一律带完整 outcome，不要走这里。
    pub const fn from_legacy_status(status: crate::model::RunStatus) -> Self {
        match status {
            crate::model::RunStatus::Running | crate::model::RunStatus::Done => Self::Success,
            crate::model::RunStatus::Failed => Self::Failed,
        }
    }

    /// 写回 [`crate::model::RunStatus`] —— 存储层的生命周期列。
    ///
    /// `Done` 表示"跑完了"，**不表示"成了"**，所以只有 [`Self::Failed`]
    /// 映射到 `Failed`：需人工介入的那一趟确实跑完了，只是目标没达成 ——
    /// 把它写成 `Failed` 会让历史里出现一堆"失败"，而它们其实只是在等用户。
    pub const fn to_legacy_status(self) -> crate::model::RunStatus {
        match self {
            Self::Failed => crate::model::RunStatus::Failed,
            Self::Success | Self::PartialSuccess | Self::ManualRequired => {
                crate::model::RunStatus::Done
            }
        }
    }
}

/// 一条"需要人去客户端做的事"。
///
/// # 为什么是一等类型而不是一句日志
///
/// 旧项目用 `logger.info("需手动完成: …")` / `logger.error("…请前往 APP 手动打开")`
/// 表达这件事，返回值仍是成功。后果有三层：
/// ① 前端看不到；② 推送看不到（而推送正是用来喊人的）；③ 历史记录里查不出
/// "我到底积了多少件没去做的事"。
///
/// 所以这里把**结构化**的部分与**给人看**的部分分开：
/// 前者供程序判断（[`Self::capability`] 可去重、[`Self::account`] 可归属），
/// 后者供渲染（[`Self::instruction`] 是完整句子，[`Self::target`] 是"去哪做"）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualStep {
    /// 能力 code（如 `msg-push`）—— 用于按能力去重与聚合。
    pub capability: String,
    /// 账号 id（脱敏标识，**不是**凭据）。无账号上下文的任务为 `None`。
    pub account: Option<crate::model::AccountId>,
    /// 一句话说明**要人去做什么**。是完整句子，可直接进推送正文。
    pub instruction: String,
    /// 去哪做。旧文案里散落的"请前往 APP"归到这里，便于以后换端。
    pub target: ManualTarget,
}

impl ManualStep {
    pub fn new(
        capability: impl Into<String>,
        account: Option<crate::model::AccountId>,
        instruction: impl Into<String>,
        target: ManualTarget,
    ) -> Self {
        Self {
            capability: capability.into(),
            account,
            instruction: instruction.into(),
            target,
        }
    }
}

/// 需要人工介入的地方。
///
/// 单独成枚举而不是塞进文案：以后 WebUI 要按这个分出"去 App 的引导"与
/// "去网页的引导"，靠 `instruction` 里的字符串做前缀匹配太脆。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualTarget {
    /// 移动云盘 App。
    MobileApp,
    /// 移动云盘网页端。
    Web,
    /// 本系统的 WebUI（如"去设置里补手机号"）。
    Webui,
    /// 本系统的 CLI。
    Cli,
    /// 上游未指明，或我们没归类出来。
    ///
    /// **刻意保留**：抓包与旧文案里总会有归不进去的情形，
    /// 强行归类会把"我们没搞清"伪装成"我们知道"。
    Unspecified,
}

/// 一次运行的完整结果。
///
/// 把 [`RunOutcome`] 与它携带的细节打包，让"结论"和"依据"一起流动 ——
/// 否则调用方拿到一个 `ManualRequired` 却不知道要人去做什么（旧项目的日志就是这个下场）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub outcome: RunOutcome,
    /// 需要人做的事（可能多件）。非 `ManualRequired` 时通常为空，
    /// 但**允许非空** —— `PartialSuccess` 也可能夹带一件小事，
    /// 强制清空会丢掉信息。
    pub manual_steps: Vec<ManualStep>,
    /// 给人看的一句话小结。可为 `None`（没有值得说的话）。
    pub summary: Option<String>,
}

impl RunReport {
    /// 单纯的成功，没有附带信息。
    pub fn success() -> Self {
        Self {
            outcome: RunOutcome::Success,
            manual_steps: Vec::new(),
            summary: None,
        }
    }

    pub fn failed(summary: impl Into<String>) -> Self {
        Self {
            outcome: RunOutcome::Failed,
            manual_steps: Vec::new(),
            summary: Some(summary.into()),
        }
    }

    /// 造一个"需人工"的结果。
    ///
    /// `manual_steps` 为空时**降级为 `PartialSuccess`**：一个"需人工"却不说什么事的
    /// 结果和旧项目那句没人看的日志是同一个东西，不能让它进库。
    /// 这是构造函数里的守卫，不是运行期断言 —— 把不可能的状态挡在类型外。
    pub fn manual_required(steps: Vec<ManualStep>, summary: impl Into<String>) -> Self {
        if steps.is_empty() {
            return Self {
                outcome: RunOutcome::PartialSuccess,
                manual_steps: Vec::new(),
                summary: Some(summary.into()),
            };
        }
        Self {
            outcome: RunOutcome::ManualRequired,
            manual_steps: steps,
            summary: Some(summary.into()),
        }
    }

    /// 只要有一条人工步骤，结论就不能是"成功"。
    ///
    /// 这个不变式由 [`Self::manual_required`] 保证，这里再做一次组装后的复核 ——
    /// 因为 `RunReport` 的字段是 `pub` 的，可以被字面量直接构造绕过构造函数。
    /// 存储层与推送层在发送前调它，避免"自造的 `Success` + 人工步骤"漏出去。
    pub fn normalize(mut self) -> Self {
        if !self.manual_steps.is_empty() && self.outcome == RunOutcome::Success {
            self.outcome = RunOutcome::ManualRequired;
        }
        self
    }
}

// ── 事件流 ──────────────────────────────────────────────────────────────────

/// 运行事件的序号。
///
/// 从 **1** 起单调递增，**0 是哨兵**：表示"这个 run 还没有发过任何事件"。
/// 因此 0 永远不会作为一条事件的序号出现，这带来两个便利：
/// 落库侧可以拿 `0` 当"没有下界"（不用为 `None` 单写一条 SQL），
/// SSE 的 `Last-Event-ID` 缺失时也可以安心地按 0 处理。
///
/// # 为什么要 newtype，而不是直接用 `u64`
///
/// 因为**"已看到"与"下一个要取"是两个不同的值**，而它们都是 `u64`，
/// 编译器分不出来。这个仓库里真的踩过：客户端报 `3`，
/// 服务端照着"下一个要取"去查，于是从 5 开始发，**4 号被静默跳过**。
///
/// 所以两个构造器的名字就是为区分这件事而存在的：
///
/// | 想表达 | 用哪个 | 内部值 |
/// |---|---|---|
/// | "我从头开始" | [`Seq::start`] | 0 |
/// | "我已经看到 3 了"（要 4、5、…） | [`Seq::seen(3)`](Seq::seen) | 3 |
/// | "我要取 4 之后的"（**要 5、6、…**） | [`Seq::resume_after(3)`](Seq::resume_after) | 4 |
///
/// 注意后两行：`seen(3)` 与 `resume_after(3)` 的**参数一样、结果差 1**。
/// 凡是准备把 `Seq` 传给"从某处开始取"的接口，先想清楚手里这个是
/// "客户端已经看到的值"还是"我们已经发到的值"，再选构造器。
///
/// # 饱和而不是回绕
///
/// [`Seq::next`] 在 `u64::MAX` 处停住（`saturating_add`）而不是绕回 0。
/// 绕回会让序号**变小**，于是续传的 `seq > after` 条件永远为假，
/// 这个 run 从此再也发不出任何事件 —— 一个到不了、但代价是彻底静默的分支。
/// 饱和的代价是"到了上限之后序号不再递增"（本地库里到不了），
/// 但至少事件还在走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(u64);

impl Seq {
    /// "还没发过任何事件" —— 内部值 0。
    ///
    /// 传它等于传 `None`：两者都表示"从头来"。
    pub const fn start() -> Self {
        Self(0)
    }

    /// 客户端说"**我已经看到** `seen` 了"，下一个要给它的是 `seen + 1`。
    ///
    /// 这是 SSE 的 `Last-Event-ID` 那种语义，也是 `run_events_after` 的
    /// `after` 参数那种语义（SQL 里就是 `seq > seen`）。
    pub const fn seen(seen: u64) -> Self {
        Self(seen)
    }

    /// 我们已经发到 `last`，**下一个要取的**是 `last + 1`。
    ///
    /// ⚠ 与 [`Seq::seen`] 差 1。传错会**静默跳过一条事件** ——
    /// 前端不会报错，只是进度条少一步；排查时几乎没人会怀疑到这里。
    /// 见测试 `resume_after_and_seen_are_off_by_one`。
    ///
    /// 内部值饱和在 `u64::MAX`：`resume_after(u64::MAX)` 与 `seen(u64::MAX)`
    /// 得到同一个值，也就是"已经到头了"。
    pub const fn resume_after(last: u64) -> Self {
        Self(last.saturating_add(1))
    }

    /// 从库里存着的整数原样还原。
    ///
    /// 与 [`Seq::resume_after`] 的区别就体现在这里：**存的是序号本身**，
    /// 不是"上一条"或"下一条"。所以直接按原值还原，
    /// 用 `resume_after(seq - 1)` 也能得到同一个数，但那是在绕圈子表达
    /// "这就是 seq"，读的人得先算一遍才知道对不对。
    ///
    /// 负数按 0 处理（SQLite 的 INTEGER 是有符号的，理论上能存进负数）——
    /// 落到 0 意味着"从头来"，是安全的退路；断言 panic 会让一条脏数据
    /// 把整次回放打断。
    pub const fn from_stored(stored: i64) -> Self {
        if stored <= 0 {
            Self(0)
        } else {
            Self(stored as u64)
        }
    }

    /// 取下一个序号并前进。见类型文档的"饱和而不是回绕"。
    pub const fn next(&mut self) -> Self {
        self.0 = self.0.saturating_add(1);
        *self
    }

    /// 内部值。落库、比较、拼 SQL 都用它。
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// 手写而不是 `derive(Default)`。
///
/// 两者结果一样（都是 0），但 `#[derive]` 的默认值是"类型零值"，
/// 而这里的 0 是**有含义的哨兵**（"还没发过"）。手写一遍是为了让
/// 读代码的人看到我们确实想过这件事 —— 而不是"编译器替他决定了一个 0"。
impl Default for Seq {
    fn default() -> Self {
        Self::start()
    }
}

/// 日志级别。
///
/// 只有三档，且**不含** `Debug`/`Trace`：这套日志是给**用户**看的
/// （前端面板、推送正文），不是给开发者调试的。多出来的档位只会
/// 诱使调用方把内部细节写进去，而那些东西既不该进前端也不该进推送。
///
/// 变体顺序即严重程度，`Ord` 依赖它在声明里保持一致 ——
/// 与 `ErrorKind` 的排列规则相同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// 正常进展，用户看着安心用的。
    Info,
    /// 没成功但不算失败，以及"要人去做点什么"的提示。
    Warn,
    /// 这一条出错了。注意"某一条出错"未必让整个 run 失败（见 [`RunOutcome`]）。
    Error,
}

impl LogLevel {
    /// 稳定的小写名字。落库、进 JSON、进前端 class 名都用它。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 运行过程中的一条事件。
///
/// # 五态，且是 `tag` 式而不是结构体 + 可选字段
///
/// 五类事件的字段差别很大：`Progress` 有 done/total，`Finished` 有
/// outcome/manual_steps/summary，`Log` 有 level/message。
/// 用一个结构体塞全部字段的话，大部分字段对大部分行是 `None` ——
/// 于是"`Progress` 一定有 total"这种不变式就没人保证了。
///
/// `#[serde(tag = "type")]` 让 JSON 长这样：
///
/// ```json
/// {"type":"progress","runId":"…","done":3,"total":20}
/// {"type":"log","runId":"…","level":"warn","message":"…"}
/// ```
///
/// 这个形状是**外部契约**：进 `run_events.payload`、进 SSE 的 `data:`、
/// 前端直接 `switch (ev.type)`。所以 `rename_all = "snake_case"` 不能随手改。
///
/// # 字段名 `runId` 为什么是驼峰
///
/// 其余字段是 snake_case，`runId` 保持旧项目的驼峰 —— 因为它已经出现在
/// 前端的 API 契约与库里的历史数据里，改名会让"新旧数据混读"多一条分支。
/// 这一条**不受**仓库的文件命名规则影响（那条只约束文件名，见
/// `docs/repo-layout.md`），也不受 Rust 的 `snake_case` 风格约束 ——
/// 它是序列化后的外部名字。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// 开跑。
    Started {
        #[serde(rename = "runId")]
        run_id: RunId,
        /// 能力 code（如 `sign` / `msg-push`）。
        capability: String,
    },

    /// 一条日志。
    Log {
        #[serde(rename = "runId")]
        run_id: RunId,
        level: LogLevel,
        message: String,
    },

    /// 进度。`done` / `total` 都用 `u32` —— 一个 run 的步数量级是"账号数"，
    /// 现实里到不了四十亿，用 `u64` 只是让前端的 JSON 数字看起来更吓人。
    Progress {
        #[serde(rename = "runId")]
        run_id: RunId,
        done: u32,
        total: u32,
    },

    /// 终局之一：正常结算（**不等于成功**）。
    ///
    /// 带 `outcome` 与完整 `manual_steps`：丢掉待办的话，
    /// 回看历史就只剩一句"需人工"，而"要人去做什么"恰恰是这条结论的全部价值。
    Finished {
        #[serde(rename = "runId")]
        run_id: RunId,
        outcome: RunOutcome,
        manual_steps: Vec<ManualStep>,
        summary: Option<String>,
    },

    /// 终局之二：执行器自己崩了（还没到能力层做判定的时候）。
    ///
    /// 与 [`RunEvent::Finished`] 的区别是**没有 outcome** ——
    /// 上游错误、panic 恢复、超时这些都没跑到"这一步成没成"的判断，
    /// 硬塞一个 `RunOutcome::Failed` 会假装我们知道得比实际多。
    Errored {
        #[serde(rename = "runId")]
        run_id: RunId,
        message: String,
    },
}

impl RunEvent {
    /// 开跑事件。
    pub fn started(run_id: RunId, capability: impl Into<String>) -> Self {
        Self::Started {
            run_id,
            capability: capability.into(),
        }
    }

    /// 日志事件。
    pub fn log(run_id: RunId, level: LogLevel, message: impl Into<String>) -> Self {
        Self::Log {
            run_id,
            level,
            message: message.into(),
        }
    }

    /// 进度事件。
    ///
    /// 不做 `done > total` 的校验：调用方可能是"总步数后来才确定"的场景
    /// （先报到 3 再到 20）。真正要守的是 [`RunEvent::percent`] 里的除零。
    pub const fn progress(run_id: RunId, done: u32, total: u32) -> Self {
        Self::Progress {
            run_id,
            done,
            total,
        }
    }

    /// 终局之一：正常结算。
    pub fn finished(
        run_id: RunId,
        outcome: RunOutcome,
        manual_steps: &[ManualStep],
        summary: Option<String>,
    ) -> Self {
        Self::Finished {
            run_id,
            outcome,
            manual_steps: manual_steps.to_vec(),
            summary,
        }
    }

    /// 终局之二：执行器出错。
    pub fn errored(run_id: RunId, message: impl Into<String>) -> Self {
        Self::Errored {
            run_id,
            message: message.into(),
        }
    }

    pub const fn run_id(&self) -> &RunId {
        match self {
            Self::Started { run_id, .. }
            | Self::Log { run_id, .. }
            | Self::Progress { run_id, .. }
            | Self::Finished { run_id, .. }
            | Self::Errored { run_id, .. } => run_id,
        }
    }

    /// 稳定的小写名字，与 JSON 里的 `type` 标签、库里的 `kind` 列**同源**。
    ///
    /// 库里的 `kind` 列由它决定（见 `mcloud-io` 的 `event_kind`），
    /// 两者冗余但不漂移。单独存一列是为了让"只看 `errored`"能走 SQL 的
    /// `WHERE kind = ?`，不必把每个 payload 都解一遍。
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Log { .. } => "log",
            Self::Progress { .. } => "progress",
            Self::Finished { .. } => "finished",
            Self::Errored { .. } => "errored",
        }
    }

    /// 是不是"这条 run 已经结束了"的事件。
    ///
    /// [`crate::stage::RunExecution`] 靠它在发布侧守住"终局事件至多且最后一条"。
    /// `Errored` 也算终局：执行器崩了，这个 run 不会再有后续事件。
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished { .. } | Self::Errored { .. })
    }

    /// 是不是开场那条。
    ///
    /// 单独给一个方法而不是让调用方去 `matches!`：前端第一次连上时
    /// 要区分"这个 run 还没开始"与"开始了但还没发日志"，两处都用它。
    pub const fn is_started(&self) -> bool {
        matches!(self, Self::Started { .. })
    }

    /// 终局事件的业务判定。只有 [`RunEvent::Finished`] 有 ——
    /// `Errored` 返回 `None`，理由见该变体的文档。
    pub const fn outcome(&self) -> Option<RunOutcome> {
        match self {
            Self::Finished { outcome, .. } => Some(*outcome),
            _ => None,
        }
    }

    /// 进度百分比（0–100），只对 [`RunEvent::Progress`] 有值。
    ///
    /// `total == 0` 返回 `None` 而不是 0 或 100：
    /// 一个"总共 0 步"的进度条画出来是**满的**（任何比例都无意义，
    /// 前端多半会除零拿到 `NaN`，然后渲染成一个空条或全条）。
    /// 返回 `None` 让前端有机会写"暂无法估算"，比给它一个假数字好。
    ///
    /// 用整数算而不是浮点：百分比要进 JSON，`33.333333333333336` 这种值
    /// 在前端显示出来很难看，而且同样的输入在两端可能因为浮点细节不一致。
    pub const fn percent(&self) -> Option<u32> {
        match self {
            Self::Progress { done, total, .. } if *total > 0 => {
                // 先乘后除会溢出吗：done 最大 u32::MAX，乘 100 后仍在 u64 里，
                // 所以这里显式转 u64 再算，结果落回 u32（0..=100 * 100 / 1 也在范围内）
                Some(((*done as u64 * 100) / (*total as u64)) as u32)
            }
            _ => None,
        }
    }
}

/// 带序号的事件 —— 落库与投递的最小单位。
///
/// # 为什么把 `seq` 与 `event` 绑在一起
///
/// 因为它们必须**同批次确定**：序号由发布侧（`EventBus`）分配，
/// 而分配之后到真正写库之间，进程可能崩。重启后重放同一批时，
/// 如果序号是"写的时候再算"，重放就会得到**新的**序号 ——
/// 于是同一个事件在库里出现两次、序号不同，客户端会看到重复进度。
///
/// 绑在一起之后，"重放同一批"是幂等的：`(run_id, seq)` 是主键，
/// 落库侧 `ON CONFLICT DO NOTHING` 就把它挡住了。
/// 这不是纸上谈兵 —— 崩在"写完库、还没告诉总线"确实是**正常路径**。
///
/// # 字段是 `pub` 的
///
/// 这是个纯数据载体，没有需要守的不变式（序号合法性由 [`Seq`] 自己保证），
/// 加 getter 只会让 `Sequenced { seq, event }` 的字面量构造变得啰嗦。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sequenced {
    pub seq: Seq,
    pub event: RunEvent,
}

impl Sequenced {
    pub const fn new(seq: Seq, event: RunEvent) -> Self {
        Self { seq, event }
    }

    /// 序号 + 事件体的便捷解构（`for (seq, event) in batch` 那种写法用得上）。
    pub fn into_parts(self) -> (Seq, RunEvent) {
        (self.seq, self.event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定序列的随机源：同 seed 出同结果，这样"同 runId 重放"能被钉住。
    struct SeededRng(u8);

    impl Random for SeededRng {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for b in dst.iter_mut() {
                self.0 = self.0.wrapping_mul(31).wrapping_add(7);
                *b = self.0;
            }
        }
    }

    fn rng() -> SeededRng {
        SeededRng(3)
    }

    #[test]
    fn run_id_is_uuid_v4_shaped() {
        let id = RunId::new(&mut rng());
        assert_eq!(id.as_str().len(), 36);
        assert!(is_uuid_v4_shape(id.as_str()));
        // 版本位必须是 4
        assert_eq!(id.as_str().as_bytes()[14], b'4');
    }

    #[test]
    fn same_seed_same_run_id() {
        assert_eq!(RunId::new(&mut rng()), RunId::new(&mut rng()));
    }

    #[test]
    fn parse_accepts_generated_and_rejects_junk() {
        let id = RunId::new(&mut rng());
        assert_eq!(RunId::parse(id.as_str()), Some(id.clone()));

        // 形状不对的一律拒掉 —— 这些串都不可能是我们生成的
        for bad in [
            "",
            "abc",
            "not-a-uuid-at-all-but-right-length!",  // 38 字符
            "00000000-0000-0000-0000-000000000000", // 版本位不是 4
            "550e8400e29b41d4a716446655440000",     // 无连字符
        ] {
            assert!(RunId::parse(bad).is_none(), "不该接受: {bad}");
        }
    }

    #[test]
    fn parse_normalizes_case() {
        let upper = "550E8400-E29B-41D4-A716-446655440000";
        let parsed = RunId::parse(upper).expect("大写应被接受");
        assert_eq!(parsed.as_str(), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn manual_required_without_steps_degrades_to_partial() {
        // 不说"要人做什么"的 ManualRequired 等于旧项目那句没人看的日志
        let r = RunReport::manual_required(vec![], "有事要做");
        assert_eq!(r.outcome, RunOutcome::PartialSuccess);
        assert!(r.manual_steps.is_empty());
    }

    #[test]
    fn manual_required_with_steps_keeps_outcome() {
        let step = ManualStep::new(
            "msg-push",
            None,
            "请前往 App 打开消息通知",
            ManualTarget::MobileApp,
        );
        let r = RunReport::manual_required(vec![step.clone()], "需要你动一下手");
        assert_eq!(r.outcome, RunOutcome::ManualRequired);
        assert_eq!(r.manual_steps, vec![step]);
        // 「需人工」在存储层仍是 Done（跑完了），业务结论另存 —— 别把两者混成一个字段
        assert_eq!(r.outcome.to_legacy_status(), crate::model::RunStatus::Done);
    }

    #[test]
    fn normalize_catches_hand_built_success_with_steps() {
        // 字段是 pub 的，字面量能绕过构造函数 —— 这条守住"成功却有活要干"
        let hand_built = RunReport {
            outcome: RunOutcome::Success,
            manual_steps: vec![ManualStep::new(
                "notice",
                None,
                "短信通知未开启",
                ManualTarget::Web,
            )],
            summary: None,
        };
        let fixed = hand_built.normalize();
        assert_eq!(fixed.outcome, RunOutcome::ManualRequired);
    }

    #[test]
    fn normalize_leaves_clean_success_alone() {
        let r = RunReport::success().normalize();
        assert_eq!(r.outcome, RunOutcome::Success);
    }

    #[test]
    fn success_like_excludes_manual_required() {
        assert!(RunOutcome::Success.is_success_like());
        assert!(RunOutcome::PartialSuccess.is_success_like());
        // 关键：需人工**不算**成功 —— 目标尚未达成
        assert!(!RunOutcome::ManualRequired.is_success_like());
        assert!(!RunOutcome::Failed.is_success_like());
    }

    #[test]
    fn only_success_skips_notification() {
        assert!(!RunOutcome::Success.needs_notification());
        for outcome in [
            RunOutcome::PartialSuccess,
            RunOutcome::ManualRequired,
            RunOutcome::Failed,
        ] {
            assert!(outcome.needs_notification(), "{outcome:?} 应当通知");
        }
    }

    #[test]
    fn only_manual_required_needs_human_action() {
        assert!(RunOutcome::ManualRequired.needs_human_action());
        // 部分失败值得通知，但不需要人动手 —— 重跑就行
        assert!(!RunOutcome::PartialSuccess.needs_human_action());
        assert!(!RunOutcome::Failed.needs_human_action());
    }

    #[test]
    fn only_failed_maps_to_failed_status() {
        use crate::model::RunStatus;
        assert_eq!(RunOutcome::Failed.to_legacy_status(), RunStatus::Failed);
        for outcome in [
            RunOutcome::Success,
            RunOutcome::PartialSuccess,
            RunOutcome::ManualRequired,
        ] {
            assert_eq!(outcome.to_legacy_status(), RunStatus::Done);
        }
    }

    #[test]
    fn legacy_status_fallback_is_lossy_but_safe() {
        use crate::model::RunStatus;
        assert_eq!(
            RunOutcome::from_legacy_status(RunStatus::Failed),
            RunOutcome::Failed
        );
        // Done 无法区分三种成功态，退路一律给 Success —— 这是有损的，只用于读旧数据
        assert_eq!(
            RunOutcome::from_legacy_status(RunStatus::Done),
            RunOutcome::Success
        );
        assert_eq!(
            RunOutcome::from_legacy_status(RunStatus::Running),
            RunOutcome::Success
        );
    }

    #[test]
    fn run_report_round_trips_through_json() {
        let step = ManualStep::new(
            "backup-gift",
            Some(crate::model::AccountId("15****42".into())),
            "未开启备份，请前往 App 手动开启",
            ManualTarget::MobileApp,
        );
        let r = RunReport::manual_required(vec![step], "有一件待办");
        let json = serde_json::to_string(&r).expect("应能序列化");
        let back: RunReport = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(back, r);
        // snake_case 是外部契约（进 JSON 与推送正文）
        assert!(json.contains("\"manual_required\""));
        assert!(json.contains("\"mobile_app\""));
    }

    #[test]
    fn run_id_serializes_transparently() {
        let id = RunId::new(&mut rng());
        let json = serde_json::to_string(&id).expect("应能序列化");
        // 透明包装：JSON 里就是裸字符串，不是 {"0": "..."}
        assert_eq!(json, format!("\"{}\"", id.as_str()));
    }

    // ── 事件流 ──────────────────────────────────────────────────────────────

    #[test]
    fn seq_starts_at_one_not_zero() {
        let mut seq = Seq::start();
        // 0 是"还没发过"的哨兵，所以第一条事件必须是 1
        assert_eq!(seq.value(), 0);
        assert_eq!(seq.next().value(), 1);
        assert_eq!(seq.next().value(), 2);
        assert_eq!(seq.value(), 2, "next 要推进自身，不是只返回");
    }

    /// 这条测试存在的唯一理由：`seen(n)` 与 `resume_after(n)` **差 1**，
    /// 而它们是同一个参数。传错会静默跳过一条事件，排查时没人会想到这里。
    #[test]
    fn resume_after_and_seen_are_off_by_one() {
        // 「我已经看到 3 了」→ 下一个要给它的是 4
        assert_eq!(Seq::seen(3).value(), 3);
        // 「我们发到 3 了」→ 下一个要取的是 4
        assert_eq!(Seq::resume_after(3).value(), 4);
        // 于是直接拿 seen(3) 当"下一个要取"就对了；
        // 拿 resume_after(3) 当"已看到"就会跳过 4 号
        assert_ne!(Seq::seen(3), Seq::resume_after(3));
        // 从头是 0，与 seen(0) 等价
        assert_eq!(Seq::start(), Seq::seen(0));
    }

    #[test]
    fn seq_saturates_instead_of_wrapping() {
        let mut seq = Seq::seen(u64::MAX);
        // 绕回 0 会让"seq > after"永远为假，这个 run 从此再也发不出事件
        assert_eq!(seq.next().value(), u64::MAX);
        assert_eq!(Seq::resume_after(u64::MAX), Seq::seen(u64::MAX));
    }

    #[test]
    fn seq_restores_from_stored_values() {
        assert_eq!(Seq::from_stored(7), Seq::seen(7));
        // 脏数据退到"从头来"，而不是 panic 打断整次回放
        assert_eq!(Seq::from_stored(0), Seq::start());
        assert_eq!(Seq::from_stored(-3), Seq::start());
    }

    #[test]
    fn seq_serializes_as_a_bare_number() {
        let json = serde_json::to_string(&Seq::seen(12)).expect("应能序列化");
        // 透明包装：不是 {"0": 12}
        assert_eq!(json, "12");
    }

    #[test]
    fn event_json_shape_is_the_external_contract() {
        let id = RunId::new(&mut rng());

        let json = serde_json::to_string(&RunEvent::progress(id.clone(), 3, 20)).expect("ok");
        assert!(json.contains("\"type\":\"progress\""));
        assert!(json.contains("\"runId\""), "runId 保持驼峰：这是外部契约");
        assert!(json.contains("\"done\":3") && json.contains("\"total\":20"));

        let json = serde_json::to_string(&RunEvent::log(id.clone(), LogLevel::Warn, "风控退避"))
            .expect("ok");
        assert!(json.contains("\"type\":\"log\""));
        assert!(json.contains("\"level\":\"warn\""), "级别是小写蛇形");
        assert!(json.contains("\"message\":\"风控退避\""));

        let json = serde_json::to_string(&RunEvent::errored(id, "又挂了")).expect("ok");
        assert!(json.contains("\"type\":\"errored\""));
    }

    #[test]
    fn event_kind_matches_the_json_tag() {
        let id = RunId::new(&mut rng());
        for ev in [
            RunEvent::started(id.clone(), "sign"),
            RunEvent::log(id.clone(), LogLevel::Info, "hi"),
            RunEvent::progress(id.clone(), 1, 2),
            RunEvent::finished(id.clone(), RunOutcome::Success, &[], None),
            RunEvent::errored(id.clone(), "boom"),
        ] {
            let json = serde_json::to_string(&ev).expect("ok");
            // 库里的 kind 列与 JSON 里的 type 必须同源 —— 漂移了就没法按 kind 查
            let expected = format!("\"type\":\"{}\"", ev.kind());
            assert!(json.contains(&expected), "{json} 里应当有 {expected}");
        }
    }

    /// 终局事件至多两类，且 `Errored` **不带** outcome ——
    /// 它表示"还没跑到能力层判定"，硬塞一个 Failed 是假装知道得比实际多。
    #[test]
    fn only_finished_carries_an_outcome() {
        let id = RunId::new(&mut rng());

        let fin = RunEvent::finished(id.clone(), RunOutcome::ManualRequired, &[], None);
        assert_eq!(fin.outcome(), Some(RunOutcome::ManualRequired));
        assert!(fin.is_terminal());
        assert!(!fin.is_started());

        let err = RunEvent::errored(id.clone(), "又挂了");
        assert_eq!(err.outcome(), None, "执行器出错时还没做业务判定");
        assert!(err.is_terminal());

        let started = RunEvent::started(id, "sign");
        assert!(started.is_started());
        assert!(!started.is_terminal());
        assert_eq!(started.outcome(), None);
    }

    #[test]
    fn percent_is_integer_and_guards_zero_total() {
        let id = RunId::new(&mut rng());
        assert_eq!(RunEvent::progress(id.clone(), 3, 10).percent(), Some(30));
        assert_eq!(RunEvent::progress(id.clone(), 0, 10).percent(), Some(0));
        assert_eq!(RunEvent::progress(id.clone(), 10, 10).percent(), Some(100));
        // 除零：给 None 而不是 0/100 —— 一个假数字会让前端画出一条错的进度条
        assert_eq!(RunEvent::progress(id.clone(), 0, 0).percent(), None);
        // 非进度事件没有百分比
        assert_eq!(RunEvent::started(id.clone(), "sign").percent(), None);
        // 三分之一：整数除法，不进 JSON 里塞 33.333333333333336
        assert_eq!(RunEvent::progress(id, 1, 3).percent(), Some(33));
    }

    #[test]
    fn finished_event_round_trips_with_manual_steps() {
        let step = ManualStep::new(
            "msg-push",
            None,
            "请前往 App 手动打开消息通知",
            ManualTarget::MobileApp,
        );
        let ev = RunEvent::finished(
            RunId::new(&mut rng()),
            RunOutcome::ManualRequired,
            std::slice::from_ref(&step),
            Some("3 个账号已签到，1 个需手动".into()),
        );

        let json = serde_json::to_string(&ev).expect("应能序列化");
        let back: RunEvent = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(back, ev);
        // 待办要一字不差地回来 —— 丢掉它，回看历史就只剩一句"需人工"
        match back {
            RunEvent::Finished { manual_steps, .. } => assert_eq!(manual_steps, vec![step]),
            other => panic!("应当是 Finished，实得 {other:?}"),
        }
    }

    #[test]
    fn sequenced_binds_seq_and_event_together() {
        let id = RunId::new(&mut rng());
        let item = Sequenced::new(Seq::seen(4), RunEvent::started(id, "sign"));
        assert_eq!(item.seq.value(), 4);
        assert!(item.event.is_started());

        let (seq, ev) = item.into_parts();
        assert_eq!(seq.value(), 4);
        assert!(ev.is_started());
    }

    #[test]
    fn log_level_order_matches_declaration() {
        // Ord 依赖声明顺序，谁把 Error 挪到 Info 前面就该挂
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.to_string(), "error");
    }
}
