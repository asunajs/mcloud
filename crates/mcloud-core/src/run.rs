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
}
