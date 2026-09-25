//! 定时任务模型。

use serde::{Deserialize, Serialize};

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

    /// **水位线**：已经处理过的那个触发时刻（毫秒）。
    ///
    /// ⚠ 这不是"上次运行时间"。它必须在**打上游之前**就推进 ——
    /// 反过来的话，进程在跑任务时被杀、重启后同一个点会再跑一次，
    /// 而签到、投口令在上游那侧不是幂等的。**宁可漏跑一次，不可重跑一次。**
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

    #[test]
    fn id_is_hashable_and_ordered() {
        use std::collections::HashSet;
        let mut s = HashSet::new();
        s.insert(ScheduleId::new("a"));
        s.insert(ScheduleId::new("a"));
        assert_eq!(s.len(), 1);
    }
}
