//! 「凭据自动刷新」的**纯逻辑**：那条定时叫什么 id、预设刷新时间怎么解析成 cron。
//!
//! # 这件事是什么（用户口径 2026-10-11）
//!
//! 用户原话：「**刷新auth，1. 可手动一键刷新 2. 自动设置定时，明天检查是否到达预设刷新时间，到了就刷**」✓
//!
//! # ⚠ 「预设刷新时间」是**钟点**，不是「剩余天数」（口径先钉死）
//!
//! 这句话里的「**时间**」读作**墙钟钟点**（例如「每天 03:00」✓）——
//! 依据是「**明天检查是否到达**…**到了就刷**」：到没到是**一个时刻**的事，
//! 而 [`crate::AccountSettings::refresh_token_days`] 那套是「剩余不足 N 天」的**阈值**
//! （没有"到没到"可言，任何时刻都在那个判据之下 ✓）。
//!
//! ⚠⚠ **两套判据并存、不互相取代** ✗：到点之后**先走既有判据**
//! （`refresh_token_days` / `--force`，在 `mcloud_app::auth_refresh` 里 ✓）——
//! **不需要刷就跳过**，并把"跳过的原因"写进日志 ✓。
//! 本模块只管"那条定时什么时候响"，**一个字都不改**既有判据 ✓
//!（见 [`AUTH_REFRESH_TASK`] 那段说明 ✓）。
//!
//! # 三个纯问题（都能单测、都不碰 IO）
//!
//! 1. [`auth_refresh_schedule_id`]：那条定时叫什么 id（确定性 ⇒ upsert 幂等 ✓）；
//! 2. [`parse_clock`] / [`format_clock`]：`HH:MM` 与人话之间来回；
//! 3. [`daily_expr`]：把 `HH:MM` 写成一条**每天那一刻**的 cron ✓。
//!
//! 组装与落盘在 `mcloud_app::auth_refresh_auto`，接口在
//! `mcloud_server::routes::auth`，命令行在 `mcloud auth auto-schedule` ✓。

use crate::schedule::ScheduleId;
use crate::traits::{beijing_offset, parse_beijing_time};
use crate::user::UserId;

/// 系统维护标记：`Schedule::managed_by == Some(这个值)` ⇒ 那条定时是
/// 「凭据自动刷新定时」，系统**只动它**；`None` ⇒ 用户手建的，系统**绝不碰** ✗。
///
/// # 为什么用"标记"而不是"按 id 前缀认"
///
/// 与 [`crate::live_auto::MANAGED_BY_LIVE_AUTO`] **逐字同一条理由**：
/// `auto-auth-refresh-<owner>` 这个 id 是**确定性**的（⇒ upsert 幂等 ✓），
/// 但 id 是**用户也能写**的东西 —— 他完全可以手建一条恰好叫这个名字的定时 ✗。
/// 按 id 认就会去改它 ✗ —— 那是"系统动了用户手建的东西"，正是要避免的那件事 ✓。
pub const MANAGED_BY_AUTH_REFRESH: &str = "auth-refresh";

/// 这条自动定时跑的任务名。
///
/// # ⚠ 它**不是**注册表里的任务（`Task::name`）✗
///
/// 与 [`crate::live_auto::LIVE_ROOM_TASK`] 那种"任务名"长得一样，但语义不同：
/// 凭据刷新**不是一条任务** —— 它没有 `Task` 实现，跑起来也不产生 `RunRecord`
/// （它的产物是"库里那串凭据换了"✓，那由 `AuthRefreshService` 自己落盘 ✓）。
///
/// 它的处置在调度器里**单开一支**（`mcloud_engine::scheduler` 的 `fire_auth_refresh` ✓），
/// 与编排（`PIPELINE_TASK` ✓）同一条路子：任务名在注册表里查不到 ⇒ 普通任务分支会回
/// `UnknownTask` ✗，所以必须**在查注册表之前**认出来 ✓。
///
/// ⚠ 判据、上游调用、窄写落库**全部**在 `mcloud_app::auth_refresh::AuthRefreshService`
/// 里（那是既有正确逻辑 ✓）—— 本模块与调度器都**不重写**它们 ✗。
pub const AUTH_REFRESH_TASK: &str = "auth-refresh";

/// 预设刷新时间的**默认值**（东八区墙钟 `HH:MM`）。
///
/// 取 **03:00**：凭据续期是一次**真轮换**（旧串立刻作废 ✓），
/// 挑一个用户大概率不在用云盘的钟点最不容易撞上"正在跑的任务" ✓。
///
/// ⚠ 它必须与 [`crate::settings::AppSettings`] 那格的 serde 默认值**同一个值** ——
/// 抽成常量、两处共用（各写一遍的话，"从没改过设置"的实例行为会与默认值分叉 ✗）。
pub const DEFAULT_AUTH_REFRESH_AT: &str = "03:00";

/// 某个**登录用户**那条「凭据自动刷新定时」的 id：`auto-auth-refresh-<owner>`。
///
/// # 为什么每个用户一条
///
/// 与 [`crate::live_auto::live_auto_schedule_id`] 同一条理由：多用户实例里
/// 两个用户各一条，id 撞了就会互相覆盖 ✗（`ScheduleId` 是全局唯一的 ✓）。
///
/// ⚠ id 里带 owner 之后，它天然是**确定性**的 ⇒ upsert 幂等 ✓，
/// 不需要"先查有没有再决定建不建"那种两步写法 ✓。
pub fn auth_refresh_schedule_id(owner: &UserId) -> ScheduleId {
    ScheduleId::new(format!("auto-auth-refresh-{owner}"))
}

/// 把 `HH:MM`（东八区墙钟）解析成"当天的毫秒偏移"。
///
/// 接受 `H:MM` / `HH:MM`（`3:05` 与 `03:05` 等价 ✓）。
/// 解不出来回 `None` —— **不兜默认值** ✗：静默把一个打错的时刻换成 03:00，
/// 表现是"我明明设了 22:30，它却在半夜刷" ✗（调用方必须把 `None` 变成 400 / 用法错 ✓）。
///
/// # 边界
///
/// * `24:00` **不接受**（一天没有第 25 个小时 ⇒ 钟点是 `00:00`..=`23:59` ✓）；
/// * 前后空白先 `trim`（用户从别处粘过来常带空格 ✓）；
/// * 秒一律为 `0`（本功能只到分钟 —— 用户口径里给的就是钟点 ✓）。
pub fn parse_clock(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let (h, m) = trimmed.split_once(':')?;
    // 只认纯数字：`3a:00` / `+3:00` / 空段都判不合法（`parse` 自己会拒）。
    let hour: i64 = h.trim().parse().ok()?;
    let minute: i64 = m.trim().parse().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    Some(hour * 3_600_000 + minute * 60_000)
}

/// [`parse_clock`] 的逆：把"当天的毫秒偏移"写成 `HH:MM`（**零填充**，`03:05` 形状 ✓）。
///
/// 越界（负数 / 超过一天）回 `None` —— 与 [`parse_clock`] 一样，不静默钳到边界 ✓。
pub fn format_clock(offset_ms: i64) -> Option<String> {
    if !(0..86_400_000).contains(&offset_ms) {
        return None;
    }
    let minute_of_day = offset_ms / 60_000;
    Some(format!(
        "{:02}:{:02}",
        minute_of_day / 60,
        minute_of_day % 60
    ))
}

/// 把预设刷新时间（`HH:MM`）写成一条**每天那一刻**触发的 cron（6 段 ✓）。
///
/// `"03:00"` ⇒ `"0 0 3 * * *"`（秒 分 时 日 月 周 ✓）。
///
/// # 为什么日 / 月 / 周都是 `*`
///
/// 用户口径是「**每天**到预设刷新时间跑一次」（自动设置定时 ✓）——
/// 所以日与月不设限；而周几必须是 `*`：`cron` crate 里"日"与"周"是 **OR** 语义
///（见 `mcloud_core::live_auto::one_shot_expr` 那段说明 ✓），
/// 写死一个周几会让 OR 的另一半**每天**都成立 ⇒ 变成"每天跑 N 次" ✗。
///
/// 解析不出来回 `None`（**不兜默认值** ✓，理由同 [`parse_clock`]）。
pub fn daily_expr(at: &str) -> Option<String> {
    let offset = parse_clock(at)?;
    let minute_of_day = offset / 60_000;
    Some(format!(
        "0 {} {} * * *",
        minute_of_day % 60,
        minute_of_day / 60
    ))
}

/// 这条定时**下一次**会在东八区墙钟的哪一刻响 —— `HH:MM` 形态。
///
/// # 为什么不用 `mcloud_engine::next_fire_at` 现算
///
/// 那条路要解析 cron、要处理 5/6 段与水位线（`decide` 那一套 ✓），
/// 而本功能的表达式**就是** [`daily_expr`] 造出来的"每天 HH:MM" ——
/// 现算一遍只会多一个失败点，答案却与 `HH:MM` 本身**逐字相同** ✓
/// （每天同一个钟点 ⇒ "下一次"的那个钟点恒等于预设时间 ✓）。
///
/// ⚠ 于是界面上要显示"下一次触发"时，读的正是**这一格**；
/// 它与调度器实际会响的那一刻一致，因为表达式就是它写出来的 ✓。
pub fn next_clock(at: &str) -> Option<String> {
    let offset = parse_clock(at)?;
    format_clock(offset)
}

/// 某一刻的东八区墙钟串（`YYYY-MM-DD HH:MM:SS` 形态）。
///
/// ⚠ 偏移只认 [`beijing_offset`]（东八区在本项目的**唯一**定义 ✓）——
/// 用机器本地时区的话，机器设在别处就会整体错几小时 ✗
///（与 `mcloud_core::live_auto::beijing_stamp` 同一条口径 ✓）。
pub fn beijing_stamp(ms: i64) -> String {
    use chrono::TimeZone as _;
    match beijing_offset().timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        // 东八区没有夏令时 ⇒ 这个分支不可达；真到了说明偏移常量被改坏了。
        _ => String::new(),
    }
}

/// 把"某天某个钟点"（`YYYY-MM-DD` + `HH:MM`）解成 epoch 毫秒。
///
/// 供测试与 CLI 展示用；解不出来回 `None`（不兜默认值 ✓）。
pub fn parse_beijing_clock(day: &str, at: &str) -> Option<i64> {
    parse_beijing_time(
        &format!("{} {}:00", day.trim(), at.trim()),
        "%Y-%m-%d %H:%M:%S",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ 口径：`HH:MM` 解析的**边界**（越界一律 `None`，不静默钳 ✓）。
    #[test]
    fn parse_clock_accepts_only_a_real_wall_clock() {
        assert_eq!(parse_clock("00:00"), Some(0));
        assert_eq!(parse_clock("03:00"), Some(3 * 3_600_000));
        assert_eq!(
            parse_clock("3:05"),
            Some(3 * 3_600_000 + 5 * 60_000),
            "一位小时也认"
        );
        assert_eq!(
            parse_clock(" 23:59 "),
            Some(23 * 3_600_000 + 59 * 60_000),
            "带空白先 trim"
        );
        assert_eq!(parse_clock("24:00"), None, "一天没有第 25 个小时");
        assert_eq!(parse_clock("03:60"), None);
        assert_eq!(parse_clock("-1:00"), None, "负数不是钟点");
        assert_eq!(parse_clock("3a:00"), None);
        assert_eq!(parse_clock(""), None);
        assert_eq!(parse_clock("0300"), None, "没有冒号就不算钟点");
    }

    /// `format_clock` 零填充，且与 `parse_clock` 往返闭合。
    #[test]
    fn format_clock_zero_pads_and_round_trips() {
        assert_eq!(format_clock(3 * 3_600_000).as_deref(), Some("03:00"));
        assert_eq!(
            format_clock(3 * 3_600_000 + 5 * 60_000).as_deref(),
            Some("03:05")
        );
        assert_eq!(
            format_clock(23 * 3_600_000 + 59 * 60_000).as_deref(),
            Some("23:59")
        );
        assert_eq!(format_clock(-1), None);
        assert_eq!(
            format_clock(86_400_000),
            None,
            "正好一天也不算（钟点只能到 23:59）"
        );
        for raw in ["00:00", "03:00", "07:30", "12:34", "23:59"] {
            let ms = parse_clock(raw).expect("合法钟点");
            assert_eq!(format_clock(ms).as_deref(), Some(raw), "{raw} 往返必须一致");
        }
    }

    /// ⭐ `HH:MM` ⇒ 每天那一刻的 cron（秒 分 时 日 月 周 ✓），且**周几是 `*`**。
    #[test]
    fn daily_expr_is_daily_and_has_wildcard_weekday() {
        assert_eq!(daily_expr("03:00").as_deref(), Some("0 0 3 * * *"));
        assert_eq!(daily_expr("07:30").as_deref(), Some("0 30 7 * * *"));
        assert_eq!(daily_expr("00:00").as_deref(), Some("0 0 0 * * *"));
        assert_eq!(daily_expr("23:59").as_deref(), Some("0 59 23 * * *"));
        assert!(daily_expr("25:00").is_none(), "越界不兜默认值");
        assert!(daily_expr("乱写").is_none());
    }

    /// ⭐ 生成的表达式**真的**每天那一刻触发，且**一天只一次**
    ///（周几写死会让"日 OR 周"两支各触发一次 ⇒ 这条断言就是钉它的 ✓）。
    ///
    /// ⚠⚠ cron 的字段按**传进去那个时区**解释 ⇒ 必须用**东八区**喂它，
    /// 否则 `0 0 3 * * *` 会被当成 03:00 **UTC**（= 北京 11:00 ✗）。
    /// 调度器那边同理：`decide()` 先把水位 `with_timezone(&parse_timezone(&schedule.timezone))`
    /// ⇒ 这就是那条定时必须把 `timezone` 写成 `Asia/Shanghai` 的原因 ✓。
    #[test]
    fn generated_expr_fires_daily_at_that_clock() {
        use chrono::TimeZone as _;
        use std::str::FromStr as _;
        let expr = daily_expr("03:00").expect("合法");
        let cron = cron::Schedule::from_str(&expr).expect("必须能解析");

        let tz = beijing_offset();
        let start = tz
            .timestamp_millis_opt(parse_beijing_clock("2026-10-12", "00:00").expect("起点"))
            .unwrap();
        let hits: Vec<String> = cron
            .after(&start)
            .take(3)
            .map(|t| beijing_stamp(t.timestamp_millis()))
            .collect();
        assert_eq!(
            hits,
            vec![
                "2026-10-12 03:00:00",
                "2026-10-13 03:00:00",
                "2026-10-14 03:00:00",
            ],
            "每天那一刻一次，不是每天 N 次"
        );
    }

    /// id 是**确定性**的 ⇒ 同一个用户两次算出来必须一模一样（upsert 幂等的前提 ✓），
    /// 且**两个用户各一条**（id 撞了会互相覆盖 ✗）。
    #[test]
    fn schedule_id_is_deterministic_and_per_user() {
        let a = UserId::new("u-1");
        let b = UserId::new("u-2");
        assert_eq!(
            auth_refresh_schedule_id(&a).as_str(),
            "auto-auth-refresh-u-1"
        );
        assert_eq!(auth_refresh_schedule_id(&a), auth_refresh_schedule_id(&a));
        assert_ne!(auth_refresh_schedule_id(&a), auth_refresh_schedule_id(&b));
    }

    /// `next_clock` 与预设时间**逐字相同**（"每天同一个钟点" ⇒ 下一次就是它 ✓）。
    #[test]
    fn next_clock_is_the_preset_itself() {
        assert_eq!(next_clock("03:00").as_deref(), Some("03:00"));
        assert_eq!(next_clock("7:5").as_deref(), Some("07:05"), "补零后同形");
        assert_eq!(next_clock("乱写"), None);
    }
}
