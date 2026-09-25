package app

// 「云朵余额快照」跑一个账号那一段 —— 把 [RecordCloud] 接成一条可寻址的能力。
//
// # 为什么它能独立跑
//
// 余额有**四个**上游来源（见 `cloud.go` 头注），签到只是最早跑通的那一个。
// 这一条能力取的是 `infoV3.total` —— 与签到链上记的**同一个数**，所以两条链
// 在同一天各跑一次不会打架（[RecordCloud] 同日 UPSERT，且基准始终取"今天之前"那条）。
//
// # 与签到那条链的**唯一**语义差别
//
// 签到里余额落库失败是**顺带**的副产品（不因此判失败）；在这一条能力里
// **写库就是目标本身** —— 写不进去就把错误交回编排层（它报失败、多发一条 `errored`
// 事件、原文进 `runs.message`），而 [SignInRun.finish] 那一段刻意相反。

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// CloudSnapshotRun 采集一次余额并写日快照。
type CloudSnapshotRun struct {
	SignInWorld
}

// CloudSnapshotResult 是一个账号这一趟的产出。
type CloudSnapshotResult struct {
	Account core.AccountID
	// Count 是**服务端报的余额原值**（`infoV3.total`）。
	Count int64
	// Stats 是算好并写进去的那条日快照。
	Stats *core.CloudStats
}

func (r CloudSnapshotRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (CloudSnapshotResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return CloudSnapshotResult{Account: account}, perr
	}
	status, perr := SignInStatus(ctx, src)
	if perr != nil {
		// 连数都没取到 ⇒ 这一趟确实什么都没办成，交回错误让历史说得清。
		return CloudSnapshotResult{Account: account}, perr
	}
	result := CloudSnapshotResult{Account: account, Count: status.BeanTotal}

	stats, cerr := RecordCloud(ctx, r.Store, r.Scope, account, status.BeanTotal, r.Clock)
	if cerr != nil {
		// ⚠ 与签到那条链**相反**：那里余额是顺带采集的副产品（写不进去不判失败），
		// 这里写库就是目标本身，所以直接交回错误（编排层会报失败并把原文写进历史）。
		return result, cerr
	}
	result.Stats = &stats
	r.progress(core.LevelInfo, fmt.Sprintf("余额 %d 豆", stats.CloudCount))
	return result, nil
}

// SummaryLine 是给人看的一句话小结。
//
// ⚠ `Stats` 为 nil 时（走不到：那时上一步已经返回错误了）只报余额原值，
// 不编出"日增 0"—— 没写进库却把增量念一遍，表现是"界面说今天涨了、趋势图上却没有这一格"。
//
// ⚠ 增量本身为 nil（库里**没有可比的往日记录**：第一天采，或同一天多次采）时
// 同样不能念成 0：
// 那句话的读法是"今天没涨"，而事实是"今天才有第一条数"。（判据同 [core.NextCloudStats] 的字段注。）
// 这一档分流住在 [core.CloudStats.DiffClause] 里，**不在这里** —— CLI 的"余额已记录"
// 念的是同一句话，两处各写一遍的话就会有一处漏掉 nil（那一处漏掉的实测后果见那里的注释）。
func (res CloudSnapshotResult) SummaryLine() string {
	if res.Stats == nil {
		return fmt.Sprintf("余额 %d 豆", res.Count)
	}
	return fmt.Sprintf("余额 %d 豆；%s", res.Stats.CloudCount, res.Stats.DiffClause())
}

func (res CloudSnapshotResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
