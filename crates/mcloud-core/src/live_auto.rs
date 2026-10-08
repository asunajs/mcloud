//! 「直播自动定时」的**纯逻辑**：时刻怎么算、cron 怎么写、id 怎么拼。
//!
//! # 这件事是什么（用户口径 2026-10-08）
//!
//! 用户原话：「**为直播增加一个自动设置定时的功能，设置为下次直播开始时间的一个小时后**」
//! —— 接着逐条定死了细节：
//!
//! * 「**每个用户一条**，所有**指定的一个多个云盘账号**到点跑**直播任务检测口令，领取**」；
//! * 「查不到『下一场』时 ⇒ **停用**（条目留着，下次算出来自动启用）」。
//!
//! 于是这一层只回答三个纯问题（都能单测、都不碰 IO）：
//!
//! 1. [`live_auto_schedule_id`]：那条定时叫什么 id；
//! 2. [`earliest_plus_one_hour`]：一串「下一场」里取**最早**的，再 +1 小时；
//! 3. [`one_shot_expr`]：把那一刻写成一条 cron。
//!
//! 组装与落盘在 `mcloud-app::live_auto`，接口在 `mcloud-server::routes::live_room`。
//!
//! # ⚠ 「下一场」这个值**从哪来**（动手前务必读完）
//!
//! 它**不是**这里算出来的 —— 这里只做两件纯事：[`earliest_plus_one_hour`]
//!（一串候选里取最早 + 1 小时）与 [`one_shot_expr`]（写成 cron）。
//! **取值**在 `mcloud_app::live_auto::reschedule_live_auto`，链条是：
//!
//! ```text
//! ① 该用户选中的每个账号**各去上游现取一次**「下一场」（用那个账号自己的票）
//! ② 取不到（网络错 / 未登录 / 上游没给这一格）⇒ 用**共享口令池**里那份兜底
//! ③ 两个都没有 ⇒ 停用（条目留着，下次算出来自动启用）
//! ```
//!
//! # ⚠ 为什么改成"向上游现取"（用户口径 2026-10-08）
//!
//! 用户原话：「**下一场应该从上游获取啊**」✓。上一版读的是**口令池里缓存的那一份**
//!（`data/kouling-pool.json` 的 `schedule`，由直播任务探测时写进去 ✓）——
//! 而池是**实例级共享**的（跨 mcloud 用户、跨重启 ✓）⇒ 全仓**没有**"按云盘账号的
//! 下一场"这种东西，于是"每个账号各取一次"在上一版根本无从谈起 ✗。
//!
//! # ⚠⚠ 上游那份日程是**按主播的**（= 平台级、所有账号同一份）—— 事实
//!
//! 取它要打**门的三步**（请求构造在 `mcloud_upstream::session` /
//! `mcloud_upstream::live_room`，编排在 `mcloud_tasks::live_room::gate_jwt` /
//! `gate_rooms` ✓）：① `querySpecTokenV2(017027)` 换 SSO；② `cmpassLoginByCmToken`
//! 换**直播侧** JWT；③ `tabContentList?channelId=134` 拉两个页签的房间。
//! 第 ③ 步的入参只有 `tabId` 与"我是谁"（JWT ✓），内容按**固定频道**给，
//! 真正干活的判据是 `anchor_nickname == KOULING_ANCHOR` ✓
//! ⇒ 上游这份日程是**按主播的**（平台级、**不是**按账号分的 ✓）：
//! 本仓逐账号取回来的是**同一份** ✓ —— 凭据只决定"这个账号**能不能**取到" ✓。
//! 这既不是"我们按账号分了数据"，也不是"每个账号各不相同" ✗。
//!
//! ⇒ 仍然**逐账号各取一次**（用户口径 ✓）：票是**每个账号自己的**
//!（凭据坏了 / 未登录时只有那个账号取不到 ✓），而"上游哪天按账号分时"时
//! 这一层**不需要改结构** ✓。"取最早"也就保住了它的依据 ✓。
//!
//! ⚠ 判据（按唯一有效主播筛 → 只认预告场 → 跳过已过去的场次 → 取最早）只有一份：
//! `mcloud_core::kouling::next_room_at` ✓ —— 池那条路与上游那条路**都调它** ✓
//!（上一版判据分叉的后果用户见过：「16:31 还显示 15:00」✗）。
//!
//! # ⚠⚠ 池仍然是**实例级**的（这一点没变）
//!
//! 口令池的文档头写得很清楚：它跨 mcloud 用户、跨重启共享
//!（一场直播的日程与"谁去看"无关 ✓）。它现在的位置是**兜底**
//!（上游取不到时用的"最近一次探测的结论" ✓），不再是唯一出处 ✓。

use crate::schedule::ScheduleId;
use crate::traits::{beijing_format, beijing_offset, parse_beijing_time};
use crate::user::UserId;
use crate::Clock;

/// 系统维护标记：`Schedule::managed_by == Some(这个值)` ⇒ 那条定时是
/// 「直播自动定时」，系统**只动它**；`None` ⇒ 用户手建的，系统**绝不碰** ✗。
///
/// # 为什么用"标记"而不是"按 id 前缀认"
///
/// `auto-live-<owner>` 这个 id 是**确定性**的（⇒ upsert 幂等 ✓），但 id 是
/// **用户也能写**的东西：他完全可以手建一条恰好叫 `auto-live-xxx` 的定时 ✗。
/// 按 id 认就会去改它 ✗ —— 那是"系统动了用户手建的东西"，正是要避免的那件事。
/// 标记是**服务端自己写、接口里根本不接**的一格（`ScheduleInput` 没有它 ✓）
/// ⇒ 用户改不了、也伪造不了 ✓。
pub const MANAGED_BY_LIVE_AUTO: &str = "live-auto";

/// 这条自动定时跑的任务名。
///
/// ⚠ 与 `mcloud-tasks` 里 `LiveRoomTask::name()` 是**同一个串**，而引擎层与任务层
/// 都不该依赖对方 ⇒ 本仓的惯例是两边各写一次字面量，再用测试钉住一致
/// （与 `mcloud_engine::scheduler::PIPELINE_TASK` 同款 ✓）。
pub const LIVE_ROOM_TASK: &str = "live-room";

/// 「下一场」与上游一致的时间形态：`2026-10-08 10:00:00`（东八区墙钟、无时区）。
///
/// ⚠ 必须与 [`crate::kouling::ProbeRoom::expect_start_time`] **逐字同形** ——
/// 它来自上游，本仓**原样**存、原样比（定长 ⇒ 字典序等于时间序 ✓）。
pub const LOCAL_FMT: &str = "%Y-%m-%d %H:%M:%S";

/// 一个小时的毫秒数（"下一场 + 1 小时"里的那个 1 小时）。
pub const ONE_HOUR_MS: i64 = 60 * 60 * 1000;

/// 某个**登录用户**那条「直播自动定时」的 id：`auto-live-<owner>`。
///
/// # 为什么每个用户一条，而不是每个云盘账号一条（用户口径）
///
/// 用户原话：「**每个用户一条**，所有**指定的一个多个云盘账号**到点跑直播任务」✓。
/// 三条理由：
///
/// 1. **上游那份日程是按主播的**（平台级、与账号无关，见模块头的事实 ✓）
///    ⇒ 每个账号现取到的"下一场"是**同一个值**，按账号各建一条只会得到
///    N 条时刻完全相同的定时 ✗ —— 到点那一刻 N 个账号一起扑上游，
///    正是口令池那一层要消灭的事（`SCRAPE_MIN_INTERVAL_MS` 的注释写着
///    "抓取也是一个 mcloud 用户只需要开一个 ws/小红书抓取"✓）。
/// 2. **一个用户一条**才配得上"用户能在界面上改它"这件事：多选账号是**这一条**上的
///    一个字段 ✓（复用 `Schedule::accounts` ✓，不新造 ✓），用户改一次就够了 ✓。
/// 3. id **确定性**（`auto-live-<owner>`）⇒ upsert 天然幂等 ✓，
///    不需要"先查有没有再决定建不建"那种两步写法 ✓。
///
/// # 为什么 id 里带 owner
///
/// 与 `pipeline::default_pipeline_id` 同一条理由：多用户实例里两个用户各一条，
/// id 撞了就会互相覆盖 ✗。`ScheduleId` 是全局唯一的（`Store::schedule` 按 id 查 ✓）。
pub fn live_auto_schedule_id(owner: &UserId) -> ScheduleId {
    ScheduleId::new(format!("auto-live-{owner}"))
}

/// 某一刻的东八区墙钟串（[`LOCAL_FMT`] 形态）。
///
/// ⚠ 偏移只认 [`beijing_offset`]（东八区在本项目的**唯一**定义 ✓）——
/// 用机器本地时区的话，机器设在别处就会整体错几小时，而上游那份日程是东八区的 ✗。
pub fn beijing_stamp(ms: i64) -> String {
    use chrono::TimeZone as _;
    match beijing_offset().timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) => t.format(LOCAL_FMT).to_string(),
        // 东八区没有夏令时 ⇒ 这个分支不可达；真到了说明偏移常量被改坏了。
        _ => String::new(),
    }
}

/// "现在"的东八区墙钟串 —— 与上游那份 `expect_start_time` **同格式**，
/// 用来筛掉已经过去的场次（两条路都拿它当 `not_before`：
/// 池那条是 `KoulingPool::next_start_at`、上游那条是
/// [`crate::kouling::next_room_at`] ✓）。
pub fn beijing_now(clock: &dyn Clock) -> String {
    beijing_format(clock, LOCAL_FMT)
}

/// 从一串「下一场」里取**最早**的那一场，再往后推 **1 小时**。
///
/// # 为什么取最早（用户口径：「设置为下次直播开始时间的一个小时后」）
///
/// 一条定时只有**一个**触发时刻 ✓，而这批账号可能各有一场（今天同值，见模块头）。
/// 取**最早**那一场：到点就跑一次直播任务，**不放过**最先开始的那一场 ✓。
/// 取最晚的话，早开始的那几场会在"定时还没到"的时候白白错过 ✗
/// —— 而直播口令只在**本次直播**有效（`live_room.rs` 的产品事实第 2 条 ✓），
/// 错过就是真的领不到 ✓。
///
/// # 为什么是 +1 小时（而不是开播那一刻）
///
/// 用户口径就是"+ 1 小时"。这一小时是**留给主播把口令发出来**的：
/// 直播刚开始时弹幕里还没有口令，那一刻去跑等于空手而归 ✓。
///
/// # 比较为什么只比字符串
///
/// 入参是 [`LOCAL_FMT`] 形态的定长串 ⇒ **字典序等于时间序** ✓
/// （与池里 `earliest_preview` 同一条口径 ✓）。**不要**先解析再比 ——
/// 解析只是下面"加一小时"需要，比较本身不需要，多一步就多一个失败点 ✗。
///
/// 空串与解析不出来的都**跳过**：宁可少算一场，也不写一个凭空造出来的时刻 ✗。
pub fn earliest_plus_one_hour<'a>(nexts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let earliest = nexts.into_iter().filter(|s| !s.is_empty()).min()?;
    let ms = parse_beijing_time(earliest, LOCAL_FMT)?;
    let at = beijing_stamp(ms.saturating_add(ONE_HOUR_MS));
    (!at.is_empty()).then_some(at)
}

/// 把"某一刻"写成一条**只在这一刻触发**的 cron（6 段，[`crate::schedule::Schedule`] 用的形态）。
///
/// `"2026-10-08 11:00:00"` ⇒ `"0 0 11 8 10 *"`（秒 分 时 日 月 周）。
///
/// # ⚠⚠ cron **没有年字段**，所以这是一条"每年今天"的表达式
///
/// 这是本仓定时模型（cron 表达式）与"一次性时刻"之间**唯一的**那道缝，
/// 必须说清代价：**只要这条定时长期不再被重算，同一个「月-日 时:分」明年会再触发一次** ✗。
///
/// 兜底正是那条重算链本身：每次直播任务跑完就重算并 upsert ✓
/// （有下一场 ⇒ 改成新的那一刻 ✓；没有 ⇒ `enabled=false` ✓）。
/// 而"长期不再重算"意味着直播任务再也没跑过 —— 那时这条定时最多空跑一次，
/// 代价与"它压根没被维护"相比可以接受 ✓。
///
/// ⚠ 也不许把"年"塞进别的字段去补（例如拿 `body` 存年份）：那样这条定时就
/// **不再是一条普通定时**了 ✗ —— 用户口径要的是"它和手建的逐格相同"。
///
/// # 为什么周几写 `*`
///
/// `cron` crate 里"日"与"周"是 **OR** 语义（本仓 `schedules.rs` 的注释记着这条）⇒
/// 周几必须留 `*`（= 不限制 ✓），只靠"日 + 月"定出那一天 ✓。
/// 写死一个周几（例如拿那一刻的星期几）会让 OR 的另一半**每天**都成立 ✗。
pub fn one_shot_expr(local: &str) -> Option<String> {
    use chrono::{Datelike as _, Timelike as _};
    let naive = chrono::NaiveDateTime::parse_from_str(local.trim(), LOCAL_FMT).ok()?;
    Some(format!(
        "{} {} {} {} {} *",
        naive.second(),
        naive.minute(),
        naive.hour(),
        naive.day(),
        naive.month()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⭐ §3.1 验收：三个账号各有下一场（10:00 / 12:00 / 14:00）⇒ 定时时刻 = **11:00**。
    ///
    /// ⚠ 这是"取最早 + 1 小时"唯一能被区分出来的地方：线上池是**实例级**的
    /// （见模块头）⇒ 三个账号拿到的是同一个值，"最早"那一层在集成测试里退化了。
    /// 所以这条纯函数用例不是"顺手补的"，它是**唯一**钉住这条判据的地方 ✓。
    #[test]
    fn earliest_of_three_then_plus_one_hour() {
        let nexts = [
            "2026-10-08 12:00:00",
            "2026-10-08 14:00:00",
            "2026-10-08 10:00:00",
        ];
        assert_eq!(
            earliest_plus_one_hour(nexts).as_deref(),
            Some("2026-10-08 11:00:00"),
            "最早那场（10:00）+ 1 小时"
        );
    }

    /// §3.2 验收：只有一个账号有下一场 ⇒ 就用它（其余是空串 = 没有）。
    #[test]
    fn only_one_candidate_is_used() {
        let nexts = ["", "2026-10-08 20:30:00", ""];
        assert_eq!(
            earliest_plus_one_hour(nexts).as_deref(),
            Some("2026-10-08 21:30:00")
        );
    }

    /// §3.3 验收的一半：**一个都没有 ⇒ `None`**（调用方据此 `enabled=false` ✓）。
    ///
    /// 空清单与"全空串"都必须回 `None` —— 后者是真实的形态：
    /// 池里没探测过时每个账号那一格都是空的 ✓。
    #[test]
    fn no_candidate_means_none() {
        assert_eq!(earliest_plus_one_hour([] as [&str; 0]), None);
        assert_eq!(earliest_plus_one_hour(["", "", ""]), None);
    }

    /// 跨日 +1 小时必须真的跨到第二天（历法由 `chrono` 算，本函数只加毫秒）。
    #[test]
    fn plus_one_hour_crosses_midnight() {
        assert_eq!(
            earliest_plus_one_hour(["2026-10-08 23:30:00"]).as_deref(),
            Some("2026-10-09 00:30:00"),
            "23:30 + 1h 是第二天 00:30，不是 24:30"
        );
    }

    /// 解析不出来的串**跳过**，不让它变成"最早"。
    #[test]
    fn unparsable_candidate_is_skipped() {
        assert_eq!(earliest_plus_one_hour(["垃圾"]), None);
        assert_eq!(
            earliest_plus_one_hour(["垃圾", "2026-10-08 10:00:00"]).as_deref(),
            Some("2026-10-08 11:00:00"),
            "坏串不能把好串挤掉"
        );
    }

    /// `2026-10-08 11:00:00` ⇒ `0 0 11 8 10 *`（秒 分 时 日 月 周）。
    ///
    /// ⚠ 周几必须是 `*`：cron 里"日"与"周"是 **OR** 语义 ⇒ 写死一个周几会让
    /// 那一刻**每天**都成立 ✗（这条断言就是钉它的）。
    #[test]
    fn one_shot_expr_has_wildcard_weekday() {
        assert_eq!(
            one_shot_expr("2026-10-08 11:00:00").as_deref(),
            Some("0 0 11 8 10 *")
        );
        assert_eq!(
            one_shot_expr("2026-01-02 03:04:05").as_deref(),
            Some("5 4 3 2 1 *")
        );
        assert!(one_shot_expr("不是时间").is_none());
    }

    /// ⭐ 生成的表达式**真的**只在那一天那一刻触发（不是"看起来像"）。
    ///
    /// 这是把"cron 没有年字段"这条缝的**代价**量出来：同一天同一时刻明年会再来一次。
    /// 断言两件事：① 那天那一刻确实触发 ✓；② 次日同一时刻**不**触发 ✓
    ///（否则就成了"每天 11:00"，那是完全不同的行为 ✗）。
    ///
    /// ⚠⚠ **cron 的字段按传进去的那个时区解释**（`cron::Schedule::after` 用入参
    /// `DateTime` 的时区 ✓）⇒ 这里必须用**东八区**喂它，否则 `0 0 11 …` 会被当成
    /// 11:00 **UTC**（= 北京 19:00 ✗）。调度器那边是同一个道理：
    /// `decide()` 先把水位 `with_timezone(&parse_timezone(&schedule.timezone))` ✓
    /// —— 这就是 `reschedule_live_auto` 必须把 `timezone` 写成 `Asia/Shanghai` 的原因 ✓。
    #[test]
    fn generated_expr_fires_on_that_day_and_not_the_next() {
        use chrono::TimeZone as _;
        use std::str::FromStr as _;
        let expr = one_shot_expr("2026-10-08 11:00:00").unwrap();
        // ⚠ `one_shot_expr` 给的就是**6 段**（本仓调度器吃的那种）⇒ 直接解析 ✓，
        // 再补一个秒段会变成 7 段而解析失败 ✗。
        let cron = cron::Schedule::from_str(&expr).expect("必须能解析");

        let at =
            |s: &str| crate::traits::parse_beijing_time(s, LOCAL_FMT).expect("测试时刻必须能解析");
        let tz = crate::traits::beijing_offset();
        let hit: Vec<String> = cron
            .after(&tz.timestamp_millis_opt(at("2026-10-08 00:00:00")).unwrap())
            .take(2)
            .map(|t| beijing_stamp(t.timestamp_millis()))
            .collect();
        assert_eq!(
            hit,
            vec!["2026-10-08 11:00:00", "2027-10-08 11:00:00"],
            "第一次是今年那天 ✓，第二次是**明年**同一刻 —— 这正是 cron 没有年字段的代价"
        );
        // 次日同一时刻**不**触发 ⇒ 不是"每天 11:00" ✓
        let next_day = at("2026-10-09 10:59:00");
        let first = cron
            .after(&tz.timestamp_millis_opt(next_day).unwrap())
            .next()
            .map(|t| beijing_stamp(t.timestamp_millis()));
        assert_eq!(first.as_deref(), Some("2027-10-08 11:00:00"));
    }

    /// id 是**确定性**的 ⇒ 同一个用户两次算出来必须一模一样（upsert 幂等的前提 ✓）。
    #[test]
    fn schedule_id_is_deterministic_and_per_user() {
        let a = UserId::new("u-1");
        let b = UserId::new("u-2");
        assert_eq!(live_auto_schedule_id(&a).as_str(), "auto-live-u-1");
        assert_eq!(live_auto_schedule_id(&a), live_auto_schedule_id(&a));
        assert_ne!(live_auto_schedule_id(&a), live_auto_schedule_id(&b));
    }
}
