//! cron 表达式：解析 + "下次什么时候触发"。
//!
//! # 为什么自己写，而不是引一个库
//!
//! - **语义要跟旧项目对齐**：它是 6 段（含秒），`hms_to_cron` 生成的就是 6 段；
//! - **需要的子集很小**：`*` / 数字 / 范围 / 步长 / 列表，没有名字（`JAN`/`MON`）也没有 `@daily`；
//! - 这是个纯函数，测试能把它钉死。而它算错一次的后果很具体：**漏跑或重跑** ——
//!   前者让人以为任务已经跑过了，后者会重复领奖。
//!
//! # 段数：5 段或 6 段
//!
//! - 6 段：秒 分 时 日 月 周（旧项目与 `hms_to_cron` 用的形态）
//! - 5 段：分 时 日 月 周（POSIX 形态），秒按 `0` 补齐
//!
//! 段数不同**不算错**，但必须知道 `0 30 7 * * *` 与 `0 30 7 * *` 不是一回事：
//! 前者是"07:30:00"，后者是"07:30:00"（秒补 0）—— 真正的差别在 `30 7 * * *`（5 段，07:30）
//! 与 `30 7 * * * *`（6 段，每秒的第 30、7 秒…根本不同）。所以解析时按段数明确分流。
//!
//! # 两条容易踩的语义
//!
//! 1. **日与周同时被限制时是 OR**（Vixie cron）：`0 0 0 1 * 1` = 每月 1 号**或**每周一。
//!    反直觉但必须照抄 —— 用户与旧项目都按这个语义写表达式。
//! 2. **周日的写法有两种**：`0` 与 `7` 都表示周日（POSIX 如此）。解析时归一化，
//!    否则 `0 0 9 * * 7` 会永远不触发。
//!
//! # 时区
//!
//! 计算在**调用方指定的时区**里进行（`Schedule.timezone`，默认 `Asia/Shanghai`）。
//! 把默认值写死在算法里，将来多时区就要改核心逻辑；把它留给调用方，多时区只是多传一个参数。

use chrono::{DateTime, Datelike as _, NaiveDate, TimeZone as _, Utc};
use chrono_tz::Tz;

use crate::Error;

/// 搜索"下次触发"的时间上界（天）。
///
/// `0 0 0 30 2 *`（2 月 30 日）这类表达式**永远不成立**：没有上界就是死循环。
/// 4 年覆盖闰年周期，过了就返回 `None` —— 那表示"这个表达式不会触发"，
/// 而不是"我算不出来"。
const SEARCH_LIMIT_DAYS: i64 = 366 * 4;

/// 一个 cron 字段（位图）。
///
/// 用位图而不是 `Vec`：判断"这个值在不在集合里"是每次推进都要做的事，
/// 而位图让它是**一次按位与**；字段上界最大 59，`u64` 装得下。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Field {
    bits: u64,
    min: u32,
    max: u32,
}

impl Field {
    const fn new(min: u32, max: u32) -> Self {
        Self { bits: 0, min, max }
    }

    fn set(&mut self, value: u32, name: &str) -> Result<(), Error> {
        if value < self.min || value > self.max {
            return Err(Error::BadRequest(format!(
                "cron 的{name}字段取值 {value} 超出范围 {}~{}",
                self.min, self.max
            )));
        }
        self.bits |= 1u64 << value;
        Ok(())
    }

    const fn contains(&self, value: u32) -> bool {
        self.bits & (1u64 << value) != 0
    }

    const fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// 是否覆盖了整段（`*` 或等价的写法）。
    ///
    /// 日/周的 OR 语义要靠它判断"这一维到底被限定了没有"。
    fn covers_all(&self) -> bool {
        (self.min..=self.max).all(|v| self.contains(v))
    }

    /// 升序列出命中的值（升序很重要：`next_after` 靠"第一个大于 after 的候选"来保证最早）。
    fn values(&self) -> impl Iterator<Item = u32> + '_ {
        (self.min..=self.max).filter(move |v| self.contains(*v))
    }

    fn parse(&mut self, text: &str, name: &str) -> Result<(), Error> {
        if text.is_empty() {
            return Err(Error::BadRequest(format!("cron 的{name}字段为空")));
        }
        for term in text.split(',') {
            self.parse_term(term, name)?;
        }
        if self.is_empty() {
            return Err(Error::BadRequest(format!(
                "cron 的{name}字段没有任何有效取值: {text:?}"
            )));
        }
        Ok(())
    }

    /// 单个项：`*` / `a` / `a-b` / `*/n` / `a-b/n` / `a/n`。
    fn parse_term(&mut self, term: &str, name: &str) -> Result<(), Error> {
        let (range, step) = match term.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().map_err(|_| {
                    Error::BadRequest(format!("cron 的{name}字段步长不是数字: {term:?}"))
                })?;
                if step == 0 {
                    return Err(Error::BadRequest(format!(
                        "cron 的{name}字段步长不能是 0: {term:?}"
                    )));
                }
                (range, step)
            }
            None => (term, 1),
        };

        let (start, end) = if range == "*" {
            (self.min, self.max)
        } else if let Some((from, to)) = range.split_once('-') {
            if to.contains('-') {
                return Err(Error::BadRequest(format!(
                    "cron 的{name}字段范围写法不合法: {term:?}"
                )));
            }
            (self.number(from, name)?, self.number(to, name)?)
        } else {
            let single = self.number(range, name)?;
            // `a/n` = 从 a 起按步长取到上界；单独的 `a` = 只取 a 这一个值
            if step == 1 && !term.contains('/') {
                (single, single)
            } else {
                (single, self.max)
            }
        };

        if start > end {
            return Err(Error::BadRequest(format!(
                "cron 的{name}字段范围起点大于终点: {term:?}"
            )));
        }

        let mut value = start;
        while value <= end {
            self.set(value, name)?;
            value += step;
        }
        Ok(())
    }

    fn number(&self, text: &str, name: &str) -> Result<u32, Error> {
        text.trim().parse::<u32>().map_err(|_| {
            Error::BadRequest(format!(
                "cron 的{name}字段不是数字: {text:?}（本实现不支持 JAN/MON 这类名字）"
            ))
        })
    }
}

/// 解析好的 cron 表达式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    seconds: Field,
    minutes: Field,
    hours: Field,
    days: Field,
    months: Field,
    weekdays: Field,
}

impl CronSchedule {
    /// 解析 5 段或 6 段表达式。
    pub fn parse(expr: &str) -> Result<Self, Error> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        let (sec, min, hour, day, month, weekday) = match parts[..] {
            [s, mi, h, d, mo, w] => (s, mi, h, d, mo, w),
            // 5 段：秒补齐为 `0`（而不是"每秒都行"）—— POSIX 里没有秒这一维，
            // 补 `*` 会让"每天 7:30"变成"7:30 那一分钟里跑 60 次"
            [mi, h, d, mo, w] => ("0", mi, h, d, mo, w),
            _ => {
                return Err(Error::BadRequest(format!(
                    "cron 必须是 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周），\
                     实际 {} 段: {expr:?}",
                    parts.len()
                )));
            }
        };

        let mut schedule = Self {
            seconds: Field::new(0, 59),
            minutes: Field::new(0, 59),
            hours: Field::new(0, 23),
            days: Field::new(1, 31),
            months: Field::new(1, 12),
            // 周日有 0 与 7 两种写法（POSIX），所以上界放到 7，解析后归一化
            weekdays: Field::new(0, 7),
        };
        schedule.seconds.parse(sec, "秒")?;
        schedule.minutes.parse(min, "分")?;
        schedule.hours.parse(hour, "时")?;
        schedule.days.parse(day, "日")?;
        schedule.months.parse(month, "月")?;
        schedule.weekdays.parse(weekday, "周")?;
        // 归一化：`7` 也当周日，否则 `0 0 9 * * 7` 永远不触发
        if schedule.weekdays.contains(7) {
            schedule.weekdays.set(0, "周")?;
        }

        Ok(schedule)
    }

    /// 严格晚于 `after` 的下一个触发时刻（在 `tz` 时区里计算），以 UTC 返回。
    ///
    /// 返回 `None` 表示**这个表达式不会触发**（如 2 月 30 日），而不是算不出来。
    pub fn next_after(&self, after: DateTime<Utc>, tz: Tz) -> Option<DateTime<Utc>> {
        let mut date = after.with_timezone(&tz).date_naive();
        for _ in 0..SEARCH_LIMIT_DAYS {
            if self.day_matches(date)
                && let Some(hit) = self.time_on(date, tz, after)
            {
                return Some(hit);
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// 这一天的日期维度（月 + 日/周）是否命中。
    fn day_matches(&self, date: NaiveDate) -> bool {
        if !self.months.contains(date.month()) {
            return false;
        }
        let dom_restricted = !self.days.covers_all();
        let dow_restricted = !self.weekdays.covers_all();
        let dom = self.days.contains(date.day());
        let dow = self
            .weekdays
            .contains(date.weekday().num_days_from_sunday());

        // 见模块文档第 1 条：两者都被限制时是 **OR**
        match (dom_restricted, dow_restricted) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }

    /// 这一天里第一个晚于 `after` 的时刻。
    fn time_on(&self, date: NaiveDate, tz: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        for hour in self.hours.values() {
            for minute in self.minutes.values() {
                for second in self.seconds.values() {
                    let naive = date.and_hms_opt(hour, minute, second)?;
                    // 夏令时"跳过"的那一小时里没有这个本地时刻：`earliest()` 给 None，跳过即可
                    let Some(local) = tz.from_local_datetime(&naive).earliest() else {
                        continue;
                    };
                    let candidate = local.with_timezone(&Utc);
                    // 值都是升序枚举的，所以第一个大于 after 的就是最早的那个
                    if candidate > after {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }
}

/// `Schedule.cron` + `Schedule.timezone` 的便捷入口。
///
/// 单独抽出来是因为调度侧拿到的永远是**两个字符串**（都来自库里），
/// 让每个调用点各写一遍"解析表达式 + 解析时区"只会把这两步的顺序写歪。
pub fn next_fire(
    cron: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Error> {
    let schedule = CronSchedule::parse(cron)?;
    let tz: Tz = timezone
        .parse()
        .map_err(|_| Error::BadRequest(format!("未知时区: {timezone:?}")))?;
    Ok(schedule.next_after(after, tz))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pure::hms_to_cron;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn shanghai() -> Tz {
        crate::local_tz()
    }

    /// 6 段：秒 分 时 日 月 周。
    #[test]
    fn six_field_expression_fires_at_the_named_second() {
        let schedule = CronSchedule::parse("40 59 9 * * *").unwrap();
        // 2026-09-20 01:59:39 UTC = 09:59:39 +08:00 → 下一秒就是 09:59:40
        let next = schedule
            .next_after(utc("2026-09-20T01:59:39Z"), shanghai())
            .unwrap();
        assert_eq!(next, utc("2026-09-20T01:59:40Z"));
    }

    /// 5 段：秒补 `0`，**不是**"每秒都跑"。
    #[test]
    fn five_field_expression_means_second_zero_not_every_second() {
        let schedule = CronSchedule::parse("30 7 * * *").unwrap();
        let next = schedule
            .next_after(utc("2026-09-19T20:00:00Z"), shanghai())
            .unwrap();
        // 07:30:00 +08:00 = 前一日 23:30:00 UTC
        assert_eq!(next, utc("2026-09-19T23:30:00Z"));

        // 再往后一次必须是**次日**的 07:30，而不是同一分钟里的下一秒
        let again = schedule
            .next_after(utc("2026-09-19T23:30:00Z"), shanghai())
            .unwrap();
        assert_eq!(again, utc("2026-09-20T23:30:00Z"));
    }

    /// 两种段数都要能解析 —— 但也别把它们混为一谈。
    #[test]
    fn five_and_six_field_forms_are_both_accepted() {
        assert!(
            CronSchedule::parse("30 7 * * *").is_ok(),
            "5 段 = 分 时 日 月 周"
        );
        assert!(
            CronSchedule::parse("0 30 7 * * *").is_ok(),
            "6 段 = 秒 分 时 日 月 周"
        );
        // 5 段里"时"的位置写 30 就是非法的 —— 这条断言的价值在提醒两者顺序不同
        assert!(
            CronSchedule::parse("0 30 7 * *").is_err(),
            "5 段的时不能是 30"
        );
    }

    /// 与旧项目生成口径对齐：`hms_to_cron` 的输出必须能解析，且触发时刻就是那个时刻。
    #[test]
    fn hms_to_cron_round_trips() {
        let expr = hms_to_cron(7, 30, 0);
        assert_eq!(expr, "0 30 7 * * *");
        let next = next_fire(&expr, "Asia/Shanghai", utc("2026-09-19T23:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next, utc("2026-09-19T23:30:00Z"), "07:30 +08:00");
    }

    #[test]
    fn steps_ranges_and_lists_work() {
        // 每 15 秒
        let every_15s = CronSchedule::parse("*/15 * * * * *").unwrap();
        assert_eq!(
            every_15s
                .next_after(utc("2026-09-20T00:00:16Z"), shanghai())
                .unwrap(),
            utc("2026-09-20T00:00:30Z")
        );

        // 工作时间每 10 分钟（9-18 点）
        let work = CronSchedule::parse("0 */10 9-18 * * *").unwrap();
        assert_eq!(
            work.next_after(utc("2026-09-20T01:55:00Z"), shanghai())
                .unwrap(),
            utc("2026-09-20T02:00:00Z"),
            "09:55 +08:00 → 10:00"
        );
        assert_eq!(
            work.next_after(utc("2026-09-20T10:59:00Z"), shanghai())
                .unwrap(),
            utc("2026-09-21T01:00:00Z"),
            "18:59 之后跨到次日 09:00"
        );

        // 列表
        let listed = CronSchedule::parse("0 0 8,12,20 * * *").unwrap();
        assert_eq!(
            listed
                .next_after(utc("2026-09-20T04:30:00Z"), shanghai())
                .unwrap(),
            utc("2026-09-20T12:00:00Z"),
            "12:30 +08:00 → 20:00"
        );
    }

    /// 周日既能写 `0` 也能写 `7`。
    #[test]
    fn seventh_weekday_is_also_sunday() {
        let zero = CronSchedule::parse("0 0 9 * * 0").unwrap();
        let seven = CronSchedule::parse("0 0 9 * * 7").unwrap();
        let from = utc("2026-09-20T00:00:00Z"); // 2026-09-20 是周日
        assert_eq!(
            zero.next_after(from, shanghai()),
            seven.next_after(from, shanghai())
        );
    }

    /// 日与周同时被限制 → **OR**（那条反直觉但必须照抄的语义）。
    #[test]
    fn day_and_weekday_are_or_ed_when_both_restricted() {
        // 每月 1 号 **或** 每周一，10:00（+08:00）
        let schedule = CronSchedule::parse("0 0 10 1 * 1").unwrap();

        // 2026-09-21 是周一：应当命中
        let next = schedule
            .next_after(utc("2026-09-20T00:00:00Z"), shanghai())
            .unwrap();
        assert_eq!(next, utc("2026-09-21T02:00:00Z"), "周一的 10:00");

        // 2026-09-29 是周二：下一个命中的是 **10-01（1 号，周四）** ——
        // 注意别把这条写成"下一个周一的 10:00"：周一先到就会先命中（见上一条）
        let next = schedule
            .next_after(utc("2026-09-29T00:00:00Z"), shanghai())
            .unwrap();
        assert_eq!(next, utc("2026-10-01T02:00:00Z"), "1 号的 10:00");
    }

    /// 时区是**参数**，不是写死的默认值。
    #[test]
    fn the_timezone_actually_shifts_the_answer() {
        let expr = "0 0 9 * * *";
        let from = utc("2026-09-20T00:00:00Z");
        let in_shanghai = next_fire(expr, "Asia/Shanghai", from).unwrap().unwrap();
        let in_utc = next_fire(expr, "UTC", from).unwrap().unwrap();
        assert_eq!(in_shanghai, utc("2026-09-20T01:00:00Z"), "09:00 +08:00");
        assert_eq!(in_utc, utc("2026-09-20T09:00:00Z"), "09:00 UTC");
    }

    /// 不可能成立的表达式返回 `None`，而不是死循环。
    #[test]
    fn an_impossible_expression_returns_none_instead_of_looping() {
        let schedule = CronSchedule::parse("0 0 0 30 2 *").unwrap();
        assert_eq!(
            schedule.next_after(utc("2026-09-20T00:00:00Z"), shanghai()),
            None
        );
    }

    #[test]
    fn bad_expressions_are_rejected_with_actionable_messages() {
        let cases = [
            ("", "段"),
            ("* * * *", "段"),
            ("* * * * * * *", "段"),
            ("60 * * * * *", "超出范围"), // 秒
            ("* * 24 * * *", "超出范围"), // 时（6 段里第 3 个才是"时"）
            ("* * * 32 * *", "超出范围"),
            ("* * * * 13 *", "超出范围"),
            ("* * * * * 8", "超出范围"),
            ("*/0 * * * * *", "不能是 0"),
            ("a * * * * *", "不是数字"),
            ("5-1 * * * * *", "起点大于终点"),
            ("1--5 * * * * *", "不合法"),
            ("*/x * * * * *", "步长不是数字"),
        ];
        for (expr, want) in cases {
            let err = CronSchedule::parse(expr).unwrap_err();
            assert!(
                format!("{err}").contains(want),
                "{expr:?} 的报错应包含 {want:?}，实际: {err}"
            );
        }
    }

    /// 跨年要能算过去（4 年上界不是为了兜住"每年 12 月 31 日"这类正常需求）。
    #[test]
    fn yearly_expression_crosses_the_year_boundary() {
        let schedule = CronSchedule::parse("0 0 0 1 1 *").unwrap();
        // 从 2026-03-01 起，下一次是 2027-01-01 00:00 +08:00
        let next = schedule
            .next_after(utc("2026-03-01T00:00:00Z"), shanghai())
            .unwrap();
        assert_eq!(next, utc("2026-12-31T16:00:00Z"));
    }

    /// 2 月 29 日：闰年才有的那一天必须算得对。
    #[test]
    fn leap_day_is_handled() {
        let schedule = CronSchedule::parse("0 0 0 29 2 *").unwrap();
        let next = schedule
            .next_after(utc("2026-03-01T00:00:00Z"), shanghai())
            .unwrap();
        assert_eq!(next, utc("2028-02-28T16:00:00Z"), "2028 是闰年");
    }
}
