//! 纯函数层 —— 全部可被 `cargo test -p mcloud-core <名字>` 定向单跑。
//!
//! 这些是原 TS 版本里"藏在任务函数内部、只能靠真机试跑验证"的逻辑，
//! 现在被提出来成为可验证的最小单元：日快照增量、开抢时刻、时间纠偏。

use crate::model::{AccountId, CloudStats};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Timelike, Utc};

/// 按 `Asia/Shanghai` 取"今天"，用作日快照的日期键。
///
/// 跨零点必须按本地时区切分，否则会把昨天的余额算进今天。
pub fn daily_key(now: DateTime<Utc>) -> String {
    now.with_timezone(&crate::local_tz())
        .format("%Y-%m-%d")
        .to_string()
}

/// 计算今天的云朵快照（含日增量与周增量）。
///
/// `history` 必须是**过去**的快照，按日期升序；不含 `date` 当天。
///
/// 增量语义（对齐参考项目 caiyun 的 `CloudStats`）：
/// - `cloud_diff`：与上一个**有记录**之日之差；
/// - `cloud_diff_week`：与 7 天前（或更早）最近一条之差；若历史不足 7 天，退化为最早一条之差；
/// - 完全无历史时两者均为 0（首次采集不虚报增量）。
pub fn next_cloud_stats(
    account_id: &AccountId,
    date: &str,
    count: i64,
    history: &[CloudStats],
) -> CloudStats {
    let prev = history.last();

    let today = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok();
    let cutoff = today.map(|d| d - Duration::days(7));

    // 优先取"7 天前或更早"里最近的一条；没有则退化为最早一条。
    let week_base = cutoff
        .and_then(|cut| {
            history
                .iter()
                .filter(|s| {
                    NaiveDate::parse_from_str(&s.date, "%Y-%m-%d")
                        .map(|d| d <= cut)
                        .unwrap_or(false)
                })
                .next_back()
        })
        .or_else(|| history.first());

    let cloud_diff = prev.map_or(0, |p| count - p.cloud_count);
    let cloud_diff_week = week_base.map_or(0, |p| count - p.cloud_count);

    CloudStats {
        account_id: account_id.clone(),
        date: date.to_string(),
        cloud_count: count,
        cloud_diff,
        cloud_diff_week,
    }
}

/// 多源余额取值：优先取第一个可信的非空来源。
///
/// 原 TS 版本 `UserState.totalCloud` 的写入被注释掉，余额从未被采集；
/// 上游其实有 4 处能拿到余额（signInPoints / cloudStatus / totalClouds / hc1t 记录），
/// 这里按优先级收敛成一个确定结果，而不是散落在各任务里。
pub fn pick_cloud_value(sources: &[Option<i64>]) -> Option<i64> {
    sources.iter().copied().flatten().next()
}

/// 服务器时间与本地时间的偏差（毫秒，正数表示服务器更快）。
///
/// 抢兑需要它：客户端时钟慢 2 秒就可能整点抢不到。
pub fn server_offset_ms(local_before_ms: i64, server_ms: i64) -> i64 {
    server_ms - local_before_ms
}

/// 距离下一次 `h:m:s` 还有多少毫秒。
///
/// 若今天该时刻已过（含正好等于），则指向明天。纯函数，不读系统时间。
pub fn ms_until_next_hms<Tz: TimeZone>(now: DateTime<Tz>, h: u32, m: u32, s: u32) -> i64 {
    let nanos_now = now.timestamp_nanos_opt().expect("时间戳超出纳秒表示范围");

    let base = now
        .with_hour(h)
        .and_then(|d| d.with_minute(m))
        .and_then(|d| d.with_second(s))
        .and_then(|d| d.with_nanosecond(0));

    let Some(target) = base else {
        // 非法时刻（如 24:00:00）—— 交给调用方校验，这里返回 0 表示"立刻"。
        return 0;
    };

    let mut target_nanos = target
        .timestamp_nanos_opt()
        .expect("时间戳超出纳秒表示范围");
    if target_nanos <= nanos_now {
        target_nanos += Duration::days(1).num_nanoseconds().expect("一天可纳秒表示");
    }

    (target_nanos - nanos_now) / 1_000_000
}

/// `HH:mm:ss` → 6 段 cron（秒 分 时 日 月 周）。
///
/// 原 TS 版本用 `ss mm hh * * *` 生成定时任务（如 09:59:40 → `40 59 9 * * *`）。
pub fn hms_to_cron(h: u32, m: u32, s: u32) -> String {
    format!("{s} {m} {h} * * *")
}

/// 校验时刻是否合法。抢兑时刻是用户输入，必须挡在入口。
pub const fn is_valid_hms(h: u32, m: u32, s: u32) -> bool {
    h < 24 && m < 60 && s < 60
}

/// 把脱敏手机号按"前两位 + **** + 后两位"格式化。
///
/// 原 CLI 的账号展示约定；非 11 位手机号（如已是昵称）原样返回。
pub fn mask_phone(phone: &str) -> String {
    let chars: Vec<char> = phone.chars().collect();
    if chars.len() < 7 || !chars.iter().all(|c| c.is_ascii_digit()) {
        return phone.to_string();
    }
    let head: String = chars[..2].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}****{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn stats(date: &str, count: i64) -> CloudStats {
        CloudStats {
            account_id: AccountId::new("acc-1"),
            date: date.to_string(),
            cloud_count: count,
            cloud_diff: 0,
            cloud_diff_week: 0,
        }
    }

    #[test]
    fn daily_key_uses_shanghai_timezone() {
        // UTC 2026-09-16T16:30Z == 上海 2026-09-17T00:30 → 属于"17 号"
        let now = Utc.with_ymd_and_hms(2026, 9, 16, 16, 30, 0).unwrap();
        assert_eq!(daily_key(now), "2026-09-17");

        // UTC 2026-09-16T15:30Z == 上海 2026-09-16T23:30 → 仍属于"16 号"
        let now = Utc.with_ymd_and_hms(2026, 9, 16, 15, 30, 0).unwrap();
        assert_eq!(daily_key(now), "2026-09-16");
    }

    #[test]
    fn no_history_does_not_fake_a_diff() {
        let s = next_cloud_stats(&AccountId::new("acc-1"), "2026-09-16", 1286, &[]);
        assert_eq!(s.cloud_count, 1286);
        assert_eq!(s.cloud_diff, 0, "首次采集不应虚报增量");
        assert_eq!(s.cloud_diff_week, 0);
    }

    #[test]
    fn diff_is_against_previous_recorded_day() {
        let history = vec![stats("2026-09-14", 1274), stats("2026-09-15", 1278)];
        let s = next_cloud_stats(&AccountId::new("acc-1"), "2026-09-16", 1286, &history);
        assert_eq!(s.cloud_diff, 8, "应取最近一条 1278 作为基准");
    }

    #[test]
    fn diff_can_be_negative_after_spending_clouds() {
        let history = vec![stats("2026-09-15", 1500)];
        let s = next_cloud_stats(&AccountId::new("acc-1"), "2026-09-16", 1286, &history);
        assert_eq!(s.cloud_diff, -214, "兑换消耗后余额下降应记为负数");
    }

    #[test]
    fn week_diff_prefers_record_at_least_seven_days_old() {
        let history = vec![
            stats("2026-09-08", 1200), // 距 09-16 第 8 天 → 命中 7 天线
            stats("2026-09-10", 1250),
            stats("2026-09-15", 1278),
        ];
        let s = next_cloud_stats(&AccountId::new("acc-1"), "2026-09-16", 1286, &history);
        assert_eq!(s.cloud_diff, 8);
        assert_eq!(s.cloud_diff_week, 86, "周增量基准应为 09-08 的 1200");
    }

    #[test]
    fn week_diff_degrades_to_earliest_when_history_is_short() {
        let history = vec![stats("2026-09-14", 1274), stats("2026-09-15", 1278)];
        let s = next_cloud_stats(&AccountId::new("acc-1"), "2026-09-16", 1286, &history);
        assert_eq!(s.cloud_diff_week, 12, "历史不足 7 天时退化为最早一条之差");
    }

    #[test]
    fn exact_seven_days_old_counts_as_week_base() {
        let history = vec![stats("2026-09-09", 1000), stats("2026-09-15", 1278)];
        let s = next_cloud_stats(&AccountId::new("acc-1"), "2026-09-16", 1100, &history);
        assert_eq!(
            s.cloud_diff_week, 100,
            "09-09 恰好是 7 天前，应作为周增量基准"
        );
    }

    #[test]
    fn pick_cloud_value_skips_empty_sources() {
        assert_eq!(pick_cloud_value(&[None, Some(1286), Some(1)]), Some(1286));
        assert_eq!(pick_cloud_value(&[None, None]), None);
        assert_eq!(pick_cloud_value(&[]), None);
        assert_eq!(
            pick_cloud_value(&[Some(0)]),
            Some(0),
            "0 是合法余额，不能被当成空"
        );
    }

    #[test]
    fn server_offset_positive_when_server_ahead() {
        assert_eq!(server_offset_ms(1_000, 3_500), 2_500);
        assert_eq!(server_offset_ms(1_000, 1_000), 0);
        assert_eq!(server_offset_ms(3_500, 1_000), -2_500);
    }

    #[test]
    fn cron_mapping_matches_existing_schedule_semantics() {
        assert_eq!(hms_to_cron(9, 59, 40), "40 59 9 * * *");
        assert_eq!(hms_to_cron(11, 59, 40), "40 59 11 * * *");
        assert_eq!(hms_to_cron(0, 0, 0), "0 0 0 * * *");
    }

    #[test]
    fn invalid_hms_is_rejected() {
        assert!(is_valid_hms(23, 59, 59));
        assert!(!is_valid_hms(24, 0, 0));
        assert!(!is_valid_hms(0, 60, 0));
        assert!(!is_valid_hms(0, 0, 60));
    }

    #[test]
    fn ms_until_rolls_to_tomorrow_when_time_has_passed() {
        let tz = crate::local_tz();
        // 上海 09:00:00，目标 09:59:40 → 今天还有 59 分 40 秒
        let now = tz.with_ymd_and_hms(2026, 9, 16, 9, 0, 0).unwrap();
        assert_eq!(ms_until_next_hms(now, 9, 59, 40), (59 * 60 + 40) * 1000);

        // 上海 10:00:00，目标 09:59:40 → 已过，指向明天（23h59m40s）
        let now = tz.with_ymd_and_hms(2026, 9, 16, 10, 0, 0).unwrap();
        let expected = ((24 * 60 - 1) * 60 + 40) * 1000;
        assert_eq!(ms_until_next_hms(now, 9, 59, 40), expected);
    }

    #[test]
    fn ms_until_exact_time_points_to_tomorrow() {
        let tz = crate::local_tz();
        let now = tz.with_ymd_and_hms(2026, 9, 16, 9, 59, 40).unwrap();
        let one_day_ms = 24 * 60 * 60 * 1000;
        assert_eq!(
            ms_until_next_hms(now, 9, 59, 40),
            one_day_ms,
            "恰好等于目标时刻应指向明天，避免立即触发"
        );
    }

    #[test]
    fn mask_phone_follows_cli_convention() {
        assert_eq!(mask_phone("19900000001"), "19****01");
        assert_eq!(mask_phone("账号2"), "账号2", "非纯数字应原样返回");
        assert_eq!(mask_phone("123456"), "123456", "过短不脱敏");
    }

    #[test]
    fn mask_phone_never_reveals_the_middle() {
        let masked = mask_phone("13812345678");
        assert!(!masked.contains("81234"), "中间段必须被遮蔽");
        assert_eq!(masked, "13****78");
    }
}
