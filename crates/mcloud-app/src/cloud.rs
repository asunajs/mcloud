//! 云朵余额采集：把"服务端报的余额"落成 `accounts.cloud_current` + `cloud_stats` 日快照。
//!
//! # 为什么单独一条链路，而不是在签到里顺手写
//!
//! 余额有**四个**上游来源（`infoV3.beforeTotal`、AI 工作台的 `cloudStatus`、
//! 领取类返回的 `totalClouds`、`hc1t` 的云朵记录），签到只是其中最早跑通的那一个。
//! 把写库塞进 [`crate::signin`]，第二个来源出现时就只能再塞一次 ——
//! "余额怎么算"于是散落在各个任务里，正是原 TS 版本 `UserState.totalCloud`
//! 的写入被注释掉之后、再没人敢打开的那种局面（规划 §3.4 的 D1）。
//!
//! 所以这里只做一件事：**给一个余额数字，把它记对**。
//! 增量语义（日增 / 周增）是纯函数 [`mcloud_core::pure::next_cloud_stats`]，可单测；
//! 本模块只负责"取哪段历史、按什么顺序写"。
//!
//! # 两处写入，不是重复
//!
//! | 写到哪 | 是什么 | 谁来读 |
//! | --- | --- | --- |
//! | `accounts.cloud_current` | **当前值**，每次采集覆盖 | 账号列表页 |
//! | `cloud_stats`（PK `account_id + date`）| **历史**，每天一行、当天 UPSERT | 趋势曲线与日/周增量 |
//!
//! # 失败由调用方决定
//!
//! 规划 §3.4 的要求是"多源取可信值，失败降级，不阻塞主流程" ——
//! 那是**调用方的策略**（CLI 是"打一句警告、签到照样算成功"），
//! 本模块不替所有调用方做掉：WebUI 将来可能想把它当成一次真实失败。

use mcloud_core::pure::{daily_key, next_cloud_stats};
use mcloud_core::{AccountId, Clock, CloudStats, Error, Scope, StateStore};

/// 历史查询的下界（[`StateStore::cloud_stats_since`] 的语义是 `date >= since`）。
///
/// # 为什么不取"近 7 天"
///
/// 两个增量各有各的基准：`cloud_diff` 要**上一个有记录之日**（可能很久以前），
/// `cloud_diff_week` 要"7 天前或更早的最近一条"。窗口取 7 天，
/// 一个两周没采集过的账号两条基准都会掉到窗口外 ——
/// `next_cloud_stats` 拿不到 `prev`，按"首次采集"记 0，
/// **把"这段时间涨了 300"静默写成"没变化"**。
///
/// 每账号每天一行、一年 365 行，取全量的代价可以忽略；
/// 而"静默少算"是那种要几个月后看趋势图才发现的错。
const HISTORY_FROM: &str = "0000-01-01";

/// 记录一次余额采集：写当前值 + UPSERT 当天快照，返回算好的那条快照。
///
/// `count` 必须是**服务端给的余额原值**（`infoV3.total`），
/// 不是本地推算的 —— 这里不做任何"猜一个差不多的数"的兜底。
///
/// ⚠ **是 `total`，不是 `beforeTotal`。** 这里记错过一次，说明白：
///
/// | 字段 | 是什么 | 实拍（同一天，领取前后）|
/// | --- | --- | --- |
/// | `total` | **现行余额**（H5 原话「现拥有 N 豆」）| `10496` → **`10502`**（领了 6）✓ |
/// | `beforeTotal` | 云朵明细（历史）页口径的那个数 | `8027` → `8027`（**不动**）|
///
/// 另有一条独立旁证：`12000 - total == lackCloudNum`（兑换门槛减去余额 = 上游自己报的"还差多少"），
/// 两次实拍都成立（`1504` / `1498`）。拿 `beforeTotal` 去减就对不上。
///
/// 记 `beforeTotal` 的后果不是报错，而是**趋势图变成一条直线** ——
/// 那个数不随领取变化，静默地什么都不显示。
pub async fn record_cloud<S, K>(
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    count: i64,
    clock: &K,
) -> Result<CloudStats, Error>
where
    S: StateStore,
    K: Clock,
{
    let now = clock.now();
    let today = daily_key(now);

    // 同一天重复采集（定时任务重跑 / 手工补跑 / `--status-only` 之后再正式签一次）
    // **不能把今天的行当成基准** —— 那样 `cloud_diff` 恒为 0。
    // 所以先滤掉 `date >= today`，让增量始终相对"今天之前"算：**同日幂等**。
    let history: Vec<CloudStats> = store
        .cloud_stats_since(scope, account_id, HISTORY_FROM)
        .await?
        .into_iter()
        .filter(|snapshot| snapshot.date < today)
        .collect();

    let stats = next_cloud_stats(account_id, &today, count, &history);

    // 顺序有讲究：**先写历史，再写当前值**。
    //
    // 两者不可能在同一个事务里（一个走 `upsert_cloud_stats`、一个走 `set_account_cloud`），
    // 所以一定会有一个"只完成一半"的窗口。按这个顺序失败，留下的是
    // "趋势图完整、列表页的余额旧了一天"；反过来则是
    // "列表页新、趋势图缺了今天这一天"—— 而缺的那一格**下次采集不会补**
    // （补的是新的一天），累积性数据丢一格就是永久少一格。
    store.upsert_cloud_stats(scope, &stats).await?;
    store
        .set_account_cloud(scope, account_id, count, now)
        .await?;

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Account, ErrorKind, FixedClock, User, UserId, UserStore};
    use mcloud_io::SqliteStore;

    const PHONE: &str = "19900000001";

    /// 建一个内存库 + 一个属于 `alice` 的账号。
    async fn store_with_account() -> (SqliteStore, UserId, AccountId) {
        let store = SqliteStore::open_in_memory().unwrap();
        let user = User::new(
            "u1",
            "alice",
            "!",
            mcloud_core::Role::Admin,
            FixedClock::parse("2026-09-19T00:00:00Z").now(),
        );
        store.upsert_user(&user).await.unwrap();

        let account_id = AccountId::new(mcloud_core::account_label(None, PHONE));
        let mut account = Account::new(account_id.as_str());
        account.owner = Some(user.id.clone());
        store.upsert_account(Scope::Local, &account).await.unwrap();

        (store, user.id, account_id)
    }

    /// 直接塞一条历史快照（绕过 `record_cloud`，用来造"过去某天"）。
    async fn seed_snapshot(
        store: &SqliteStore,
        owner: &UserId,
        id: &AccountId,
        date: &str,
        count: i64,
    ) {
        store
            .upsert_cloud_stats(
                Scope::user(owner),
                &CloudStats {
                    account_id: id.clone(),
                    date: date.to_string(),
                    cloud_count: count,
                    cloud_diff: 0,
                    cloud_diff_week: 0,
                },
            )
            .await
            .unwrap();
    }

    /// 北京时间 2026-09-19 14:00 —— `daily_key` 给 `2026-09-19`。
    fn clock() -> FixedClock {
        FixedClock::parse("2026-09-19T06:00:00Z")
    }

    #[tokio::test]
    async fn first_collection_records_the_value_and_reports_no_delta() {
        let (store, owner, id) = store_with_account().await;

        let stats = record_cloud(&store, Scope::user(&owner), &id, 5392, &clock())
            .await
            .unwrap();

        assert_eq!(stats.date, "2026-09-19");
        assert_eq!(stats.cloud_count, 5392);
        assert_eq!(stats.cloud_diff, 0, "首次采集不该虚报增量");
        assert_eq!(stats.cloud_diff_week, 0);

        let account = store
            .get_account(Scope::user(&owner), &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(account.cloud_current, Some(5392));
        assert_eq!(
            account.cloud_updated_at,
            Some(clock().now()),
            "采集时间应取自注入的时钟，而不是实现自己取的系统时间"
        );
    }

    #[tokio::test]
    async fn daily_delta_is_measured_against_the_previous_snapshot() {
        let (store, owner, id) = store_with_account().await;
        seed_snapshot(&store, &owner, &id, "2026-09-18", 4200).await;

        let stats = record_cloud(&store, Scope::user(&owner), &id, 5392, &clock())
            .await
            .unwrap();

        assert_eq!(stats.cloud_diff, 1192);
        assert_eq!(
            stats.cloud_diff_week, 1192,
            "历史不足 7 天时周增量退化为最早一条之差"
        );
    }

    #[tokio::test]
    async fn a_second_collection_on_the_same_day_does_not_use_today_as_its_own_baseline() {
        let (store, owner, id) = store_with_account().await;
        seed_snapshot(&store, &owner, &id, "2026-09-18", 4200).await;

        record_cloud(&store, Scope::user(&owner), &id, 5000, &clock())
            .await
            .unwrap();
        let second = record_cloud(&store, Scope::user(&owner), &id, 5392, &clock())
            .await
            .unwrap();

        assert_eq!(
            second.cloud_diff, 1192,
            "基准仍应是昨天那条 4200，而不是今天早些时候那条 5000"
        );

        let all = store
            .cloud_stats_since(Scope::user(&owner), &id, HISTORY_FROM)
            .await
            .unwrap();
        assert_eq!(
            all.len(),
            2,
            "09-18 与 09-19 各一行，当天是 UPSERT 不是追加"
        );
        assert_eq!(all.last().unwrap().cloud_count, 5392);
    }

    #[tokio::test]
    async fn a_long_gap_does_not_silently_report_zero_growth() {
        // 一个两周没采集过的账号：两条基准都在"近 7 天"之外。
        // 窗口取窄了这里会算成 0 —— 把"涨了 300"写成"没变化"。
        let (store, owner, id) = store_with_account().await;
        seed_snapshot(&store, &owner, &id, "2026-09-01", 5092).await;

        let stats = record_cloud(&store, Scope::user(&owner), &id, 5392, &clock())
            .await
            .unwrap();

        assert_eq!(stats.cloud_diff, 300);
        assert_eq!(stats.cloud_diff_week, 300);
    }

    #[tokio::test]
    async fn a_foreign_account_is_refused_before_anything_is_written() {
        let (store, owner, id) = store_with_account().await;
        let other = UserId::new("u2");

        let err = record_cloud(&store, Scope::user(&other), &id, 5392, &clock())
            .await
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::NotFound);
        let all = store
            .cloud_stats_since(Scope::user(&owner), &id, HISTORY_FROM)
            .await
            .unwrap();
        assert!(all.is_empty(), "被拒的采集不该留下日快照");
    }
}
