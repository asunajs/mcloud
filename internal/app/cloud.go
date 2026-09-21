package app

// 云朵余额采集：把"服务端报的余额"落成 `accounts.cloud_current` + `cloud_stats` 日快照。
//
// # 为什么单独一条链路，而不是在签到里顺手写
//
// 余额有**四个**上游来源（`infoV3.beforeTotal`、AI 工作台的 `cloudStatus`、
// 领取类返回的 `totalClouds`、`hc1t` 的云朵记录），签到只是其中最早跑通的那一个。
// 把写库塞进 `signin`，第二个来源出现时就只能再塞一次 ——
// "余额怎么算"于是散落在各个任务里，正是原 TS 版本 `UserState.totalCloud`
// 的写入被注释掉之后、再没人敢打开的那种局面（规划 §3.4 的 D1）。
//
// 所以这里只做一件事：**给一个余额数字，把它记对**。
// 增量语义（日增 / 周增）是纯函数 [core.NextCloudStats]，可单测；
// 本模块只负责"取哪段历史、按什么顺序写"。
//
// # 两处写入，不是重复
//
// | 写到哪 | 是什么 | 谁来读 |
// | --- | --- | --- |
// | `accounts.cloud_current` | **当前值**，每次采集覆盖 | 账号列表页 |
// | `cloud_stats`（PK `account_id + date`）| **历史**，每天一行、当天 UPSERT | 趋势曲线与日/周增量 |
//
// # 失败由调用方决定
//
// 规划 §3.4 的要求是"多源取可信值，失败降级，不阻塞主流程" ——
// 那是**调用方的策略**（CLI 是"打一句警告、签到照样算成功"），
// 本模块不替所有调用方做掉：WebUI 将来可能想把它当成一次真实失败。

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
)

// HistoryFrom 是历史查询的下界（[core.StateStore.CloudStatsSince] 的语义是 `date >= since`）。
//
// # 为什么不取"近 7 天"
//
// 两个增量各有各的基准：`CloudDiff` 要**上一个有记录之日**（可能很久以前），
// `CloudDiffWeek` 要"7 天前或更早的最近一条"。窗口取 7 天，
// 一个两周没采集过的账号两条基准都会掉到窗口外 ——
// `core.NextCloudStats` 拿不到 prev，按"首次采集"记 0，
// **把"这段时间涨了 300"静默写成"没变化"**。
//
// 每账号每天一行、一年 365 行，取全量的代价可以忽略；
// 而"静默少算"是那种要几个月后看趋势图才发现的错。
const HistoryFrom = "0000-01-01"

// RecordCloud 记录一次余额采集：写当前值 + UPSERT 当天快照，返回算好的那条快照。
//
// `count` 必须是**服务端给的余额原值**（`infoV3.total`），
// 不是本地推算的 —— 这里不做任何"猜一个差不多的数"的兜底。
//
// ⚠ **是 `total`，不是 `beforeTotal`。** 这里记错过一次，说明白：
//
// | 字段 | 是什么 | 实拍（同一天，领取前后）|
// | --- | --- | --- |
// | `total` | **现行余额**（H5 原话「现拥有 N 豆」）| `10496` → **`10502`**（领了 6）✓ |
// | `beforeTotal` | 云朵明细（历史）页口径的那个数 | `8027` → `8027`（**不动**）|
//
// 另有一条独立旁证：`12000 - total == lackCloudNum`（兑换门槛减去余额 = 上游自己报的"还差多少"），
// 两次实拍都成立（`1504` / `1498`）。拿 `beforeTotal` 去减就对不上。
//
// 记 `beforeTotal` 的后果不是报错，而是**趋势图变成一条直线** ——
// 那个数不随领取变化，静默地什么都不显示。
func RecordCloud(
	ctx context.Context,
	store core.StateStore,
	scope core.Scope,
	accountID core.AccountID,
	count int64,
	clock core.Clock,
) (core.CloudStats, *core.Error) {
	now := clock.Now()
	today := core.DailyKey(now)

	// 同一天重复采集（定时任务重跑 / 手工补跑 / `--status-only` 之后再正式签一次）
	// **不能把今天的行当成基准** —— 那样 `cloud_diff` 恒为 0。
	// 所以先滤掉 `date >= today`，让增量始终相对"今天之前"算：**同日幂等**。
	all, err := store.CloudStatsSince(ctx, scope, accountID, HistoryFrom)
	if err != nil {
		return core.CloudStats{}, asAppError(err, "读云朵历史失败")
	}
	history := make([]core.CloudStats, 0, len(all))
	for _, snapshot := range all {
		if snapshot.Date < today {
			history = append(history, snapshot)
		}
	}

	stats := core.NextCloudStats(accountID, today, count, history)

	// 顺序有讲究：**先写历史，再写当前值**。
	//
	// 两者不可能在同一个事务里（一个走 `UpsertCloudStats`、一个走 `SetAccountCloud`），
	// 所以一定会有一个"只完成一半"的窗口。按这个顺序失败，留下的是
	// "趋势图完整、列表页的余额旧了一天"；反过来则是
	// "列表页新、趋势图缺了今天这一天"—— 而缺的那一格**下次采集不会补**
	// （补的是新的一天），累积性数据丢一格就是永久少一格。
	if err := store.UpsertCloudStats(ctx, scope, &stats); err != nil {
		return core.CloudStats{}, asAppError(err, "写云朵日快照失败")
	}
	if err := store.SetAccountCloud(ctx, scope, accountID, count, now); err != nil {
		// ⚠ 日快照**已经写进去了**，所以这里失败也要把 stats 交回去：
		// 返回空快照会让调用方以为"什么都没记"，于是再采一次 ——
		// 而第二次的基准变成了刚才那条今天的行，`cloud_diff` 就此恒为 0。
		return stats, asAppError(err, "写账号当前余额失败（日快照已写入）")
	}

	return stats, nil
}
