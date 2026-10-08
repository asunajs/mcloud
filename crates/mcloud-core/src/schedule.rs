//! 定时任务模型。

use serde::{Deserialize, Serialize};

use crate::push::{PushEvent, PushRuleId};
use crate::user::UserId;

/// 定时任务标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScheduleId(pub String);

impl ScheduleId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一条定时任务。
///
/// `expr` 是 5 或 6 段 cron 表达式（6 段时首段是秒）。
/// 时区**固定存字符串**而不是 `chrono::Tz`：它要落盘、要跨机复现，
/// 而进程重启后不一定能再次解析出同一个时区对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: ScheduleId,
    /// 显示名。
    pub name: String,
    /// 要跑的任务名（对应注册表里的 `Task::name`）。
    pub task: String,
    /// 目标账号。为空表示"所有已启用账号"。
    #[serde(default)]
    pub accounts: Vec<String>,
    /// cron 表达式。
    pub expr: String,
    /// 时区名，例如 `Asia/Shanghai`。
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// 任务参数。
    #[serde(default)]
    pub body: serde_json::Value,
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// **谁在维护这条定时**（用户口径 2026-10-08）。
    ///
    /// * `None` = **用户手建的** ⇒ 系统**绝不碰它** ✗（一个字节都不改 ✓）；
    /// * `Some(crate::live_auto::MANAGED_BY_LIVE_AUTO)` = 系统为「直播自动定时」
    ///   维护的那**一条** ✓ ⇒ 系统每次重算都会改它的**时刻**（见下）✓。
    ///
    /// # 为什么要有这一格（而不是"按 id 前缀认"）
    ///
    /// 那条自动定时的 id 是 `auto-live-<owner>`（确定性 ⇒ upsert 幂等 ✓），
    /// 但 id 是**用户也能写**的东西 ✗：他完全可以手建一条恰好叫 `auto-live-xxx` 的
    /// 定时。按 id 认就会去改它 ⇒ 系统动了用户手建的东西 ✗。
    ///
    /// 标记则**用户改不了**：`routes::schedules::ScheduleInput` 里**没有**这一格
    /// （接口刻意不接 ✓），编辑保存时服务端从**老行**继承 ✓ ⇒
    /// 用户既伪造不出、也抹不掉"这是系统在维护的"这个事实 ✓
    /// （他**能**删掉整条 —— 那是另一回事：下次重算会再建一条 ✓，
    /// 见 `mcloud-app::live_auto` 的模块头 ✓）。
    ///
    /// # ⚠ 用户手改时刻 ⇒ 下次重算**以系统算出的为准**（覆盖 ✗）
    ///
    /// 这不是 bug，是这一格存在的**代价**，说清楚免得后人当缺陷去"修"：
    /// 这条定时的时刻是**算出来的**（这批账号里最早的「下一场」+ 1 小时 ✓），
    /// 而"下一场"每次直播任务跑完都会变（延期 / 改期 / 换场次 ✓）。
    /// 允许用户改时刻的话，两种意图会打架：用户写的是**这一次**的想法，
    /// 而系统每次都在按最新的日程重算 ⇒ 谁后写谁赢，用户看到的是
    /// "为什么我改了又被改回去" ✗。
    ///
    /// 所以口径是：**时刻归系统**（用户改了也会被覆盖 ✓），
    /// **开关与账号归用户**（`managed_wanted` 与 `accounts` 每次都原样保留 ✓）。
    /// 用户真想固定一个时刻，就**手建一条自己的**定时 ✓ —— 那条 `managed_by` 是
    /// `None`，系统一个字节都不会动它 ✓。
    ///
    /// ⚠ 后加的一格 ⇒ `serde(default)` 保证老 `schedules.json` 读得回来 ✓
    /// （读不到 = `None` = 手建的 ✓，正是老数据的真实含义 ✓）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,

    /// **用户意愿**：他还想不想要这条自动定时（`None` = 还没表态 ⇒ 按"要"算 ✓）。
    ///
    /// # 为什么不能只靠 `enabled`（这一格是**必须**的，别合并掉 ✗）
    ///
    /// `enabled` 是**算出来的**：有下一场 ⇒ `true`、一个都没有 ⇒ `false`
    /// （用户口径：「查不到『下一场』时 ⇒ 停用（条目留着，下次算出来自动启用）」✓）。
    /// 而"用户想不想要"是**他选的**。两件事合成一格的话：
    ///
    /// * 用户关掉它（`enabled=false`）⇒ 下一次重算算出了下一场 ⇒ 系统把它当成
    ///   "自己上次停用的"再打开 ✗ —— 用户会发现"我关了的开关自己又开了" ✗；
    /// * 反过来，系统因为"没有下一场"停用它之后，用户再点开 ⇒ 系统分不清
    ///   "用户想要"与"系统算出来可以跑" ✗。
    ///
    /// ⇒ 一格记**事实**（`enabled`，系统写）、一格记**意愿**（这一格，用户写）✓。
    /// 最终能不能跑 = 两者都为真 ✓（`mcloud-app::live_auto::reschedule_live_auto` 里那一行）。
    ///
    /// ⚠ 只对 `managed_by` 非空的那条有意义；手建的定时这一格恒为 `None` ✓。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_wanted: Option<bool>,

    /// **归属**：这条定时属于哪个系统用户。
    ///
    /// # 为什么是 Option
    ///
    /// 与 `Account::owner` 同一条理由：这一列后加，老 `schedules.json` 里没有。
    /// 读不到 ⇒ `None`，加载时回填给唯一的管理员（`owner::legacy_owner`），
    /// 新建时由路由写成当前登录用户。`skip_serializing_if` 保证未归属时
    /// 不落一个显式 `null`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,

    /// **这个定时选中的推送规则**（**可多选** ✓）。
    ///
    /// # 为什么是数组、为什么在行为这一侧（2026-10-08 用户口径）
    ///
    /// 用户原话：「**不是规则去命中行为，而是行为选择规则，一个行为可以选择
    /// 多个规则**」✓ ⇒ 绑定存在**行为**上（这条定时 ✓），而不是规则里挂一串
    /// "我绑了谁" ✗。
    ///
    /// 空 = **不推** ✓ —— ⚠ 与旧字段 `push_rule: Option<…>` 的语义**不一样** ✗：
    /// 那个 `None` 是"沿用这个登录用户的渠道配置"，而那份用户级配置这一版
    /// **已经整个删掉** ✓（规则自带凭据 ✓）⇒ "没绑规则"就只剩"不推"一个意思 ✓
    /// （§6 验收第 3 条：没绑规则 ⇒ **0 个请求** ✓）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push_rules: Vec<PushRuleId>,

    /// **只在哪些运行结局时推**（空 = **跑完就推** ✓）。
    ///
    /// ⚠ 只对**运行类**事件有意义 ✓（`runFailed` / `runPartial` / `runOk` /
    /// `runCancelled` ✓）。登录 / 性能那几个变体归 [`crate::push::PanelBindings`] ✓
    /// —— 它们没有定时可绑，用户在设置里单独给它们选规则 ✓。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub push_events: Vec<PushEvent>,

    /// **水位线**：已经处理过的那个触发时刻（毫秒）。
    ///
    /// ⚠ 这不是"上次运行时间"。它必须在**打上游之前**就推进 ——
    /// 反过来的话，进程在跑任务时被杀、重启后同一个点会再跑一次，
    /// 而签到、用口令领小红花在上游那侧不是幂等的。**宁可漏跑一次，不可重跑一次。**
    ///
    /// 也不能用"上次运行时间"代替它：错过的点被弃掉时根本不产生运行记录，
    /// 于是下一次 tick 会把同一个点重新捞出来 —— 表现成"每次刷新多一条 skipped"。
    #[serde(default, rename = "lastFireMs")]
    pub last_fire_ms: Option<i64>,

    /// 上次真正跑起来的时间（毫秒）。纯展示用，与水位线无关。
    #[serde(default)]
    pub last_run_ms: Option<i64>,

    /// 上次结果：`ok` / `error` / `skipped`。
    ///
    /// 用 `Option<String>` 而不是枚举：未知文本必须能**原样读回并报错**，
    /// 而不是被静默当成某个默认值。一个来路不明的状态会让界面显示错误的结论。
    #[serde(default)]
    pub last_status: Option<String>,

    /// 上次错误。成功时必须被**显式清空**（写 null），
    /// 否则"ok 配一句旧错误"会在界面上并排出现，正是查不出原因的形态。
    #[serde(default)]
    pub last_error: Option<String>,

    /// 累计触发次数。
    #[serde(default)]
    pub run_count: u64,
}

fn default_timezone() -> String {
    "Asia/Shanghai".to_string()
}

fn default_true() -> bool {
    true
}

impl Schedule {
    /// 这条定时是不是**某个系统维护者**的（见 [`Self::managed_by`]）。
    ///
    /// 抽成方法而不是让调用方到处写 `s.managed_by.as_deref() == Some(..)`：
    /// 判据只有一处，改了不会漏掉某一处比较。
    pub fn is_managed_by(&self, marker: &str) -> bool {
        self.managed_by.as_deref() == Some(marker)
    }

    /// 把 cron 表达式规范成 `cron` crate 能解析的 6 段形式。
    ///
    /// 用户习惯写 POSIX 的 5 段（`0 8 * * *`），而 `cron` crate **只吃 6 段**。
    /// 补秒必须补 `0` 而不是 `*`：补 `*` 会把「每天 7:30」变成「7:30 那一分钟里
    /// 每秒跑一次」——60 倍请求量，而且完全不报错。
    ///
    /// 规范化与解析放在一起，是为了让**校验**与**触发**用同一条路径。
    /// 之前两边各写一份，结果 `validate_expr` 按段数放行 5 段、随后又用
    /// `from_str` 直接解析而失败——「创建时通过、跑的时候报错」正是注释里
    /// 说要避免的那种情况。
    pub fn normalize_expr(expr: &str) -> Result<String, String> {
        let trimmed = expr.trim();
        match trimmed.split_whitespace().count() {
            5 => Ok(format!("0 {trimmed}")),
            6 => Ok(trimmed.to_string()),
            n => Err(format!(
                "cron 表达式需要 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周），当前 {n} 段"
            )),
        }
    }

    /// 校验 cron 表达式是否可用。
    ///
    /// 校验放在**创建时**而不是触发时：触发时才报错的话，用户要等到
    /// 那个时间点才知道自己写错了，而错误信息还淹没在无人看的日志里。
    pub fn validate_expr(expr: &str) -> Result<(), String> {
        let normalized = Self::normalize_expr(expr)?;
        cron::Schedule::from_str(&normalized)
            .map(|_| ())
            .map_err(|e| format!("cron 表达式无法解析：{e}"))
    }
}

use std::str::FromStr;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_five_and_six_field_exprs() {
        assert!(
            Schedule::validate_expr("0 8 * * *").is_ok(),
            "5 段：每天 8:00"
        );
        assert!(Schedule::validate_expr("0 0 8 * * *").is_ok(), "6 段：带秒");
    }

    /// 5 段补的秒必须是 `0`，不能是 `*`。
    ///
    /// 补 `*` 会把「每天 8:00 跑一次」变成「8:00 那一分钟的每一秒都跑」——
    /// 60 倍请求量，而且**不会报任何错**。所以这里锁死补出来的具体形态，
    /// 而不只是断言"能解析"。
    #[test]
    fn five_field_expr_pads_zero_second_not_wildcard() {
        assert_eq!(
            Schedule::normalize_expr("0 8 * * *").unwrap(),
            "0 0 8 * * *",
            "补的必须是 0 秒"
        );
        assert_ne!(
            Schedule::normalize_expr("0 8 * * *").unwrap(),
            "* 0 8 * * *",
            "补 * 秒会让任务在一分钟内重复触发 60 次"
        );
        // 6 段原样保留，不要重复补
        assert_eq!(
            Schedule::normalize_expr("30 0 8 * * *").unwrap(),
            "30 0 8 * * *"
        );
    }

    /// 规范化与校验必须走同一条路径。
    ///
    /// 之前 `validate_expr` 按段数放行 5 段、随后直接 `from_str` 解析，
    /// 而 `cron` crate 只吃 6 段 —— 结果「创建时通过、跑的时候报错」。
    #[test]
    fn validate_and_normalize_agree_on_five_field() {
        let expr = "0 8 * * *";
        assert!(Schedule::validate_expr(expr).is_ok());
        assert!(
            cron::Schedule::from_str(&Schedule::normalize_expr(expr).unwrap()).is_ok(),
            "校验通过就必须能被真正解析，否则是创建时放行、运行时炸"
        );
    }

    #[test]
    fn rejects_wrong_field_count() {
        let e = Schedule::validate_expr("0 8 * *").unwrap_err();
        assert!(e.contains("4 段") || e.contains("段"), "应说明段数：{e}");
    }

    #[test]
    fn rejects_unparsable_expr() {
        // 段数对但语义非法
        assert!(Schedule::validate_expr("99 99 * * *").is_err());
    }

    /// ⭐ 两个新字段必须能读**老** `schedules.json`（后加的列，老文件里没有 ✓）。
    ///
    /// # 为什么这条非钉不可
    ///
    /// 读不回来 = **整个文件解析失败** = 启动就崩（`JsonStore::load_all` 是
    /// `read_json` 一整个文件 ✓）⇒ 升级即炸 ✗。而"读不到"在老数据上**正是**
    /// 正确的语义：那时候还没有"系统维护的定时"这回事 ⇒ 全是手建的 ✓
    ///（`managed_by: None` ✓）。
    #[test]
    fn new_fields_default_to_handmade_when_absent() {
        // 老形态（没有那两格）—— 抄的是 `data/schedules.json` 里真实的键名 ✓
        let old = r#"{
            "id": "s1", "name": "签到", "task": "sign", "accounts": [],
            "expr": "0 8 * * *", "timezone": "Asia/Shanghai",
            "enabled": true, "runCount": 3
        }"#;
        let s: Schedule = serde_json::from_str(old).expect("老文件必须读得回来");
        assert_eq!(s.managed_by, None, "老数据都是手建的");
        assert_eq!(s.managed_wanted, None, "手建的没有「用户意愿」这一层");
        assert!(!s.is_managed_by("live-auto"));

        // 新形态：两格都写进去 ⇒ 读回来还是它们 ✓
        let mut managed = s.clone();
        managed.managed_by = Some(crate::live_auto::MANAGED_BY_LIVE_AUTO.to_string());
        managed.managed_wanted = Some(false);
        let json = serde_json::to_string(&managed).expect("应能序列化");
        let back: Schedule = serde_json::from_str(&json).expect("应能读回");
        assert_eq!(back, managed);
        assert!(back.is_managed_by(crate::live_auto::MANAGED_BY_LIVE_AUTO));
        assert_eq!(back.managed_wanted, Some(false));

        // 手建的那条**不该**落一个显式的 `null`（与 `owner` 同一条口径：
        // "没归属"与"归属被清空"在 JSON 里长得一样的话，回填逻辑就没法判了 ✓）
        assert!(
            !serde_json::to_string(&s).unwrap().contains("managedBy"),
            "手建的定时不该落 managedBy 这一格"
        );
    }

    #[test]
    fn id_is_hashable_and_ordered() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(ScheduleId::new("a"));
        s.insert(ScheduleId::new("a"));
        assert_eq!(s.len(), 1);
    }
}
