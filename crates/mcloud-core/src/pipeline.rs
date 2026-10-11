//! 任务编排（pipeline）模型。
//!
//! 一个 pipeline 是**有序的步骤列表**，步骤之间共享一组账号。
//! 刻意不做"步骤间传值/依赖图"：那会把编排器变成一个需要调试的语言。
//! 步骤之间唯一的耦合是**失败语义**（继续或中止）。

use serde::{Deserialize, Serialize};

use crate::push::{PushEvent, PushRuleId};
use crate::schedule::ScheduleId;
use crate::user::UserId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PipelineId(pub String);

impl PipelineId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PipelineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 多账号的执行方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccountMode {
    /// 逐个账号跑完整个 pipeline。默认 —— 上游常有限流，
    /// 并发打过去容易触发风控，而串行的代价用户可以接受。
    #[default]
    Sequential,
    /// 所有账号并行。
    Parallel,
}

/// 一个步骤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStep {
    /// 任务名。
    pub task: String,
    /// 该步骤的参数。
    #[serde(default)]
    pub body: serde_json::Value,
    /// 该步骤失败时是否继续后续步骤。
    ///
    /// 默认 `false`：**多数步骤是有依赖的**（先签到才能领AI豆），
    /// 默认继续会让后面的步骤在错误状态下空跑，产出误导性的"成功"。
    #[serde(default)]
    pub continue_on_error: bool,
}

/// 一个编排。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pipeline {
    pub id: PipelineId,
    pub name: String,
    /// 目标账号。为空表示"所有已启用账号"。
    #[serde(default)]
    pub accounts: Vec<String>,
    #[serde(default)]
    pub account_mode: AccountMode,
    /// 步骤失败时是否中止整个 pipeline。
    ///
    /// 与步骤自己的 `continue_on_error` 是**两层**：
    /// 步骤级说"这一步失败不算数"，编排级说"整个流程还要不要走"。
    /// 两层都有才够用 —— 只有一层时，要么无法跳过非关键步骤，
    /// 要么无法在关键步骤失败时止损。
    #[serde(default = "default_true")]
    pub stop_on_error: bool,
    pub steps: Vec<PipelineStep>,
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// **归属**：这条编排属于哪个系统用户。
    ///
    /// # 为什么是 Option
    ///
    /// 与 `Account::owner` 同一条理由：这一列后加，老 `pipelines.json` 里没有。
    /// 读不到 ⇒ `None`，加载时回填给唯一的管理员（`owner::legacy_owner`），
    /// 新建时由路由写成当前登录用户。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,

    /// **这条编排选中的推送规则**（**可多选** ✓）。
    ///
    /// 与 `Schedule::push_rules` 同一口径（用户口径 2026-10-08：「**不是规则去命中
    /// 行为，而是行为选择规则，一个行为可以选择多个规则**」✓）——
    /// 编排也是"被触发的那一头" ✓（手动跑编排时按这里的规则推 ✓）。
    ///
    /// 空 = **不推** ✓：旧字段 `push_rule: Option<…>` 那个"沿用用户级渠道配置"的
    /// 语义，随那份配置一起删掉了 ✓（规则自带凭据 ✓）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push_rules: Vec<PushRuleId>,

    /// **只在哪些运行结局时推**（空 = 跑完就推 ✓）。语义同 `Schedule::push_events` ✓。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push_events: Vec<PushEvent>,

    /// 上次执行时间（毫秒）。
    #[serde(default)]
    pub last_run_ms: Option<i64>,
}

fn default_true() -> bool {
    true
}

impl Pipeline {
    /// 校验：步骤非空、任务名非空。
    ///
    /// 空 steps 的 pipeline 是"点了运行什么也没发生"，
    /// 必须创建时就拒绝，而不是运行时静默成功。
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("编排至少要有一个步骤".into());
        }
        for (i, s) in self.steps.iter().enumerate() {
            if s.task.trim().is_empty() {
                return Err(format!("第 {} 步没有指定任务名", i + 1));
            }
        }
        Ok(())
    }

    /// 某个步骤失败后是否应中止整个编排。
    ///
    /// 判定顺序：步骤说"继续"就继续（它比编排级更具体）；
    /// 否则看编排级的 `stop_on_error`。
    pub fn should_abort_after(&self, step: &PipelineStep, step_failed: bool) -> bool {
        if !step_failed {
            return false;
        }
        if step.continue_on_error {
            return false;
        }
        self.stop_on_error
    }
}

/// 编排与定时的绑定（可选）。
///
/// 单独一张表而不是把 `schedule` 塞进 `Pipeline`：一个编排可以被多条
/// 定时引用（早班/晚班），把定时写进编排就锁死成一对一了。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSchedule {
    pub pipeline: PipelineId,
    pub schedule: ScheduleId,
}

/// **默认编排 / 默认定时**的 id（两者同 id，各在一张表里 ✓）。
///
/// # 为什么带用户
///
/// 用户口径（2026-10-07）：
/// 「默认编排应该是給所有人」→「**我不要这种所有人可动的，别人给你改了不就跑不了了？
/// 不能登录时給好建一个？**」✓
///
/// ⇒ 正确答案是**每人一份自己的**，不是一条共享的 ✗：
/// * 共享那版（我做过、被否掉 ✗）：谁都能改、能删 —— 别人改坏了你就跑不了了 ✗；
/// * 每人一份：各自独立、各自可改、互不可见 ✓，**登录/建号时给好** ✓。
///
/// 所以 id 里带上归属：`daily-<用户id>` ✓（`owner` 为 `None` 时退回 `daily` ——
/// 那是 **CLI 的临时预设**用的，它不落库 ✓）。
pub fn default_pipeline_id(owner: Option<&UserId>) -> String {
    match owner {
        Some(u) => format!("daily-{}", u.0),
        None => "daily".to_string(),
    }
}

/// **内置默认定时**：每天 08:00/// **内置默认定时**：每天 08:00（`Asia/Shanghai`）跑 [`default_pipeline`]。
///
/// 用户口径（2026-10-07）：「实现一个默认的编排任务，让本项目能和 ts 版一样一键运行，
/// **cli 一条命令，webui 一个定时**」✓ —— 这一条就是"webui 那个定时" ✓。
///
/// # ⚠ 它是**一条普通定时**
///
/// 表达方式与用户手建的一条**逐格相同**：`task: "pipeline"` + `body.pipelineId`
/// （调度器认这个约定，见 `mcloud_engine::scheduler::PIPELINE_TASK` ✓）、
/// 账号留空 = 全部已启用账号 ✓。没有"内置"这个字段 ✗ —— 所以它在定时页里能被
/// 停用/改时间/删除，与别的定时没有区别 ✓（用户口径：「**默认编排也要和普通的
/// 自定义编排一个逻辑**」✓）。
///
/// # 08:00 这个点
///
/// 取自本机既有那条用户自建定时（`data/schedules.json` 里 `0 8 * * *` ✓）——
/// 用户已经在那个点跑，默认就跟着它，不另立一个时间 ✓。
///
/// # `enabled: true`
///
/// "一键运行"要真的会跑 ✓。用户随时可以停用/删除 ✓。
pub fn default_schedule(owner: Option<crate::user::UserId>) -> crate::schedule::Schedule {
    // 定时与它要跑的编排**同名**（两张表各自 id 空间 ✓）⇒ 这里只算一次 ✓。
    let id = default_pipeline_id(owner.as_ref());
    crate::schedule::Schedule {
        id: crate::schedule::ScheduleId::new(id.clone()),
        name: "每日一键（默认）".into(),
        // 与 `mcloud_engine::scheduler::PIPELINE_TASK` 同一个串（引擎层不能依赖本层，
        // 所以这个字面量在两处各写一次；`mcloud-server` 的测试会断言两边一致 ✓）。
        task: "pipeline".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::json!({ "pipelineId": id }),
        enabled: true,
        // 内置的那条是**手建同款**（用户口径：「默认编排也要和普通的自定义编排
        // 一个逻辑」✓）⇒ 系统维护标记留空 ✓ —— 只有「直播自动定时」那条带标记 ✓。
        managed_by: None,
        managed_wanted: None,
        // ⚠ 归属**必须由调用方当场给**（见 `default_pipeline` 那段说明）：两个用户的实例里
        // `owner: None` 谁都看不见 ✗（`owner::visible_to` 的单用户规则 ✓）。
        owner,
        // 内置的那两条**不绑推送规则**：空数组 = **不推** ✓
        //（绑一条具体规则会让"默认"多一个别人没配过的依赖 ✗；
        //  `push_events` 也留空 = 跑完就推 —— 但没绑规则本来就一条都不发 ✓）。
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        last_note: None,
        run_count: 0,
    }
}

/// 内置默认编排：**一条命令跑完"每天该跑的那些"**。
///
/// 用户口径（2026-10-07）：「实现一个默认的编排任务，让本项目能和 ts 版一样一键运行，
/// cli 一条命令，webui 一个定时」✓。顺序照 TS 版
/// `packages/cli/src/index.ts` 的 `executeAccount`（那份 TS **能跑** ⇒ 它是依据 ✓），
/// 按本仓的任务名落下来。
///
/// # ⚠ 刻意**不**编进来的两条（用户同一条口径的下一句）
///
/// 原话：「比如**兑换，直播**这种不能固定运行的就不要编排进去，ts 那个是把口令错误编排
/// 但是**一直没开过配置**」✓
///
/// - `exchange`（兑换）：**要花豆**、兑哪一件要人判断 ⇒ 不能固定跑 ✗；
/// - `live-room`（直播口令）：要**外部给口令**（口令池 / 人工输入）⇒ 没有输入跑不了 ✗。
///   ⚠ TS 那条虽然写在序列里，但它被 `if (config.直播口令?.开启)` 挡着、**从来没开过** ⇒
///   "TS 编了它"不构成依据 ✓（照抄它就是把一条**永远跑不动**的步骤塞进每日编排 ✗）。
///
/// # ⚠⚠ 顺序是**契约**，不是随手排的（用户 2026-10-07 口径）
///
/// 原话：「还有注意顺序，比如**任务签到 / app Task 等做完了才能领取 ai 豆**」✓。
/// 三条硬约束（`default_pipeline` 的测试里各有一条哨兵 ✓）：
///
/// 1. **`receive`（领取AI豆）必须收在最后** —— 它领的是**前面那些任务攒出来的豆**：
///    `sign` 签到给的、`app-task` 做完 App 任务给的、`backup-gift` / `revival` 领的……
///    先领后做 ⇒ 那一趟做的全白做（要等下一次定时才领得到）✗。
/// 2. **`task-expansion` 必须在 `app-task` 之前** —— `app-task` 有一道"本月已备份"的
///    月份门（`app_task.rs`），而把那一格刷开的正是 `task-expansion`（它凑一次备份 ✓）。
///    反过来的话，`app-task` 那一趟会**跳过**依赖备份的任务 ✗。
/// 3. **`multiple`（签到翻倍）紧跟 `sign`** —— 它用的是"本月那一次签到翻倍机会"，
///    得先有签到这件事 ✓。
///
/// 另外 `backup-gift`（备份好礼）也排在 `task-expansion` 之后：它先查备份状态再领，
/// 而状态是 `task-expansion` 刷的 ✓。
///
/// # 为什么每步 `continue_on_error: true` 且 `stop_on_error: false`
///
/// 这是"每天跑一遍"的驱动器：**一条任务失败不该把当天其余的都掐掉** ✗ ——
/// 每条任务自己会报结论（`TaskOutcome`），而 `stop_on_error` 留给需要止损的编排用 ✓。
/// 这也是本机那条用户自建编排的选择（`data/pipelines.json` 里三步全 true ✓）。
///
/// # 账号
///
/// `accounts` 留空 = **所有已启用账号** ✓（与 CLI 的 `pipeline run` 同一条口径：
/// 空就取全部 ✓）。
pub fn default_pipeline(owner: Option<UserId>) -> Pipeline {
    // 顺序与理由逐条对齐 TS（`executeAccount`）：
    //   signIn → sign；revivalRewardTask → revival；taskExpansionTask → task-expansion；
    //   [AI新头像] aiAvatarTask → ai-avatar；[春日拍拍大作战] playAiSpecialTask → play-ai-special；
    //   playAiLotteryTask → fun-ai；[红包派对] cloudPhoneRedpackTask → cloud-phone-redpack；
    //   ⚠ [直播口令] liveRoomKoulingTask → **刻意不编**（见上）；
    //   msgPushOnTask → msg-push；noticeTask → notice；backupGiftTask → backup-gift；
    //   appTask → app-task；printPendingPrizes → prizes；receiveTask → receive。
    //
    // ⚠⚠ **[AI新头像] `ai-avatar` 也不编了**（2026-10-08 用户实测 + 口径「删除才对」✓）。
    //
    // 它与 `exchange` / `live-room` **不是**同一类理由：那两条是"能跑但不该自动跑" ✓，
    // 而这一条是**上游活动已经结束、接口整族都没了** ✗ —— 只读探测（2026-10-08）：
    //
    // | 口 | 结果 |
    // |---|---|
    // | `GET /market/rabactive/image/catalogFullPath?marketName=rabbityearpic` | 404 |
    // | `GET /market/rabactive/image/getWorkList?type=1` | 404 |
    // | `GET /market/rabactive/image/getHashLeft` | 404 |
    // | 同主机 `/` | 301（⇒ 不是域名挂了） |
    //
    // `rabbityearpic` 是**兔年**（2023 那期）的活动 ✓，到期下线合理 ✓；而本仓、Go 参考
    // （它自己写着「rabactive 那六口**整族零实拍**」✓）、抓包仓里**都没有别的 `marketName`** ✓
    // ⇒ **没有据实可换的替代** ✓，编进去只会每天准点红一条 ✗。
    //
    // ⚠ 任务本身**仍然注册着** ✓（`mcloud-tasks` 的 `ai-avatar` ✓）：它受**账号级开关**控制
    // （默认关 ✓），万一哪天活动回来了，开关一开就能用 ✓ —— 删掉的是"**默认编排里的这一步**" ✓，
    // 不是这个任务 ✓。想手动跑照旧 `mcloud run ai-avatar <账号>` ✓。
    //
    // 本仓独有的两条插在它们该在的位置：
    //   `multiple`（签到翻倍）紧跟 `sign`（它要用本月那次签到翻倍机会）；
    //   `task-expansion` 在 `app-task` **之前** —— `app-task` 有一道"本月备份"的月份门
    //   （见 `app_task.rs`），而把那一格刷开的正是 `task-expansion` ✓。
    // 顺序上"先攒后领"：会产出AI豆的都在前，`receive`（领取）收在最后 ✓。
    //
    // ⚠ 带 `[方括号]` 的那几条是**账号级开关**控着的（默认关）：它们**照样编进来**，
    //   因为任务自己会读开关并跳过 ✓ —— 与 TS 那个 `if (config.X.开启)` 等价，
    //   而放在编排里能让"开关一开就自动跑"，不用回来改编排 ✓。
    let steps = [
        "sign",
        "multiple",
        "revival",
        "task-expansion",
        "play-ai-special",
        "fun-ai",
        "cloud-phone-redpack",
        "msg-push",
        "notice",
        "backup-gift",
        "app-task",
        "prizes",
        "receive",
    ]
    .iter()
    .map(|task| PipelineStep {
        task: (*task).to_string(),
        body: serde_json::json!({}),
        continue_on_error: true,
    })
    .collect();

    Pipeline {
        id: PipelineId::new(default_pipeline_id(owner.as_ref())),
        name: "每日一键（默认）".into(),
        accounts: vec![],
        account_mode: AccountMode::Sequential,
        stop_on_error: false,
        steps,
        enabled: true,
        owner,
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_run_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(task: &str, cont: bool) -> PipelineStep {
        PipelineStep {
            task: task.into(),
            body: serde_json::Value::Null,
            continue_on_error: cont,
        }
    }

    fn pipeline(steps: Vec<PipelineStep>, stop_on_error: bool) -> Pipeline {
        Pipeline {
            id: PipelineId::new("p1"),
            name: "测试".into(),
            accounts: vec![],
            account_mode: AccountMode::default(),
            stop_on_error,
            steps,
            enabled: true,
            owner: None,
            push_rules: Vec::new(),
            push_events: Vec::new(),
            last_run_ms: None,
        }
    }

    /// ⭐ **每人一份自己的**（用户口径：「我不要这种所有人可动的…不能登录时給好建一个？」✓）。
    ///
    /// 哨兵就一条：两个用户的默认编排 **id 不同、内容相同、归属各是各的** ✓ ——
    /// 这样"别人改了"影响不到你 ✓，而"默认"仍然是**开箱就有** ✓。
    #[test]
    fn defaults_are_per_user_not_shared() {
        let a = UserId::new("u-a");
        let b = UserId::new("u-b");
        let pa = default_pipeline(Some(a.clone()));
        let pb = default_pipeline(Some(b.clone()));
        assert_ne!(pa.id, pb.id, "两个人的默认编排必须是两条，不是共享一条");
        assert_eq!(pa.id.as_str(), default_pipeline_id(Some(&a)));
        assert_eq!(
            pa.owner,
            Some(a.clone()),
            "各归各的 ⇒ 只有他自己看得见/改得动"
        );
        assert_eq!(pb.owner, Some(b));
        // 内容（步骤）相同：都是那份"每天该跑的" ✓
        assert_eq!(
            pa.steps.iter().map(|s| s.task.clone()).collect::<Vec<_>>(),
            pb.steps.iter().map(|s| s.task.clone()).collect::<Vec<_>>()
        );
        // 定时同理：id 跟着走，`body.pipelineId` 指向**自己那条** ✓
        let sa = default_schedule(Some(UserId::new("u-a")));
        assert_eq!(sa.id.as_str(), pa.id.as_str());
        assert_eq!(
            sa.body.get("pipelineId").and_then(|v| v.as_str()),
            Some(pa.id.as_str()),
            "别指向别人那条"
        );
    }

    /// 默认定时：**一条普通定时**（约定任务名 + `body.pipelineId`），不是特殊形状。
    #[test]
    fn default_schedule_is_an_ordinary_pipeline_schedule() {
        let s = default_schedule(None);
        assert_eq!(s.task, "pipeline", "调度器认的是这个约定名");
        assert_eq!(
            s.body.get("pipelineId").and_then(|v| v.as_str()),
            Some(default_pipeline_id(None).as_str()),
            "指向的必须是那条默认编排"
        );
        assert_eq!(s.id.as_str(), default_pipeline_id(None));
        assert_eq!(s.expr, "0 8 * * *");
        assert_eq!(s.timezone, "Asia/Shanghai");
        assert!(s.enabled, "一键运行要真的会跑");
        assert!(s.accounts.is_empty(), "空 = 所有已启用账号");
        assert_eq!(s.owner, None, "模板不带归属，保存时按当前用户写");
    }

    /// 内置默认编排：**能固定跑的那些都在、不能固定跑的那两条不在**。
    ///
    /// ⚠ 这条用例就是用户那条口径的哨兵：`exchange` / `live-room` 一旦被谁"顺手补进来"
    /// （最可能的来路是照 TS 的序列抄 ✗），它会红 ✓。
    #[test]
    fn default_pipeline_excludes_the_two_that_cannot_run_on_a_schedule() {
        let p = default_pipeline(None);
        let names: Vec<&str> = p.steps.iter().map(|s| s.task.as_str()).collect();
        assert!(
            !names.contains(&"exchange"),
            "兑换要花豆、要人判断 ⇒ 不进默认编排"
        );
        assert!(
            !names.contains(&"live-room"),
            "直播口令要外部输入 ⇒ 不进默认编排"
        );
        // ⭐ 用户口径（2026-10-08「**删除才对**」✓）：`ai-avatar` 的上游活动已经结束
        // —— `/market/rabactive/image/*` 三个口实测全 404 ✓（理由与探测记录见
        // `default_pipeline` 的注释 ✓）。它**不是**"能跑但不该自动跑"那一类 ✗，
        // 是**根本跑不了** ⇒ 编进去只会每天准点红一条 ✗。
        //
        // ⚠ 这条哨兵防的是"以后有人照 TS 的序列顺手把它补回来" ✗（与上面两条同一个用法 ✓）。
        assert!(
            !names.contains(&"ai-avatar"),
            "AI 头像活动已在上游下线（三个口全 404）⇒ 不进默认编排；\
             任务本身仍注册着，要跑就手动 `mcloud run ai-avatar <账号>`"
        );
        // 能固定跑的核心几条必须在，且**顺序**是"先攒后领"。
        for must in [
            "sign",
            "multiple",
            "task-expansion",
            "backup-gift",
            "app-task",
            "receive",
        ] {
            assert!(names.contains(&must), "{must} 应当在默认编排里：{names:?}");
        }
        let pos = |t: &str| names.iter().position(|n| *n == t).expect("刚断言过存在");
        // ⭐ 用户点名的那条：**签到 / app-task 做完了才能领AI豆**（2026-10-07 原话）✓
        assert!(pos("sign") < pos("receive"), "先签到后领取");
        assert!(
            pos("app-task") < pos("receive"),
            "app-task 做完才能领它攒下的豆 —— 先领后做的话那一趟白做 ✗"
        );
        assert!(
            pos("backup-gift") < pos("receive"),
            "备份好礼领的也是豆，同样要在领取之前"
        );
        assert!(
            pos("task-expansion") < pos("backup-gift"),
            "backup-gift 先查备份状态再领，而那一格是 task-expansion 刷开的"
        );
        assert!(pos("sign") < pos("multiple"), "翻倍用的是本月那次签到机会");
        assert!(
            pos("task-expansion") < pos("app-task"),
            "task-expansion 要把「本月已备份」刷开，app-task 的月份门才放行"
        );
        assert_eq!(pos("receive"), names.len() - 1, "领取收在最后");
        assert!(!p.stop_on_error, "每日驱动器不该因一条失败掐掉其余");
        assert!(p.steps.iter().all(|s| s.continue_on_error));
        assert!(p.accounts.is_empty(), "空 = 所有已启用账号");
        assert!(p.validate().is_ok(), "内置的这条自己必须合法");
        assert_eq!(p.id.as_str(), default_pipeline_id(None));
    }

    #[test]
    fn empty_steps_is_rejected() {
        let p = pipeline(vec![], true);
        assert!(p.validate().is_err(), "空编排必须被拒绝");
    }

    #[test]
    fn blank_task_name_is_rejected() {
        let p = pipeline(vec![step("  ", false)], true);
        let e = p.validate().unwrap_err();
        assert!(e.contains("第 1 步"), "错误应指出是第几步：{e}");
    }

    #[test]
    fn valid_pipeline_passes() {
        let p = pipeline(vec![step("sign", false), step("receive", false)], true);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn step_continue_overrides_pipeline_stop() {
        let p = pipeline(vec![step("optional", true)], true);
        // 步骤自己说继续 ⇒ 即使编排级 stop_on_error=true 也不中止
        assert!(!p.should_abort_after(&step("optional", true), true));
    }

    #[test]
    fn pipeline_stop_applies_when_step_does_not_continue() {
        let p = pipeline(vec![step("critical", false)], true);
        assert!(p.should_abort_after(&step("critical", false), true));
    }

    #[test]
    fn success_never_aborts() {
        let p = pipeline(vec![step("critical", false)], true);
        assert!(!p.should_abort_after(&step("critical", false), false));
    }

    #[test]
    fn lenient_pipeline_continues_on_any_failure() {
        let p = pipeline(vec![step("a", false)], false);
        assert!(
            !p.should_abort_after(&step("a", false), true),
            "stop_on_error=false 时任何失败都不中止"
        );
    }

    #[test]
    fn account_mode_defaults_to_sequential() {
        assert_eq!(AccountMode::default(), AccountMode::Sequential);
    }
}
