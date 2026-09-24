package app

// 「待领取奖品」跑一个账号那一段 —— **只读**，对应旧项目的 `printPendingPrizes`。
//
// # 为什么要把它做成一条能力
//
// 奖品躺在「待领取」里是**会过期**的（实拍那条是中奖后 14 天）。旧项目把它做成
// 签到结束时顺带打印的一行，于是只有"跑了签到且那天有奖品"才看得见 ——
// 而真正需要提醒的是"好几天没签到、奖品静静过期"那种情况。做成独立能力之后
// 它可以单独挂一行定时任务。
//
// # 这一条不领奖品
//
// 「去领取」要拿 `prizeId` 再打奖品详情与领取那两口，**尚无抓包样本**
// （`docs/business-feature-inventory.md` §2.1 `prizes`）。所以这里只列不领，
// 界面上也只承诺"看有没有"。

import (
	"context"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// PrizeListPageSize 是那一页拉多少条。
//
// 10 = 旧项目 `printPendingPrizes` 用的数，**不是**我们调出来的最优值。
// ⚠ 这条查询**只取一页**，多于 10 条会静默截断（不足与补法登记在
// [PendingPrizes] 的注释与 §2.1）。
const PrizeListPageSize int64 = 10

// PrizesRun 查一次待领取奖品。
type PrizesRun struct {
	SignInWorld
}

// PrizesResult 是一个账号这一趟的产出。
//
// ⚠ 这里**没有** `Failure` 字段：查询失败一律作为 `*core.Error` 交回编排层 ——
// 编排层那条路会多发一条 `errored` 事件、并把错误原文写进 `runs.message`
// （见 `run.go` 里 `taskErr` 那一支）。自己折成"失败结论"反而会少那一条事件。
type PrizesResult struct {
	Account core.AccountID
	Records []upstream.PrizeRecordItem
}

func (r PrizesRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (PrizesResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return PrizesResult{Account: account}, perr
	}
	records, perr := PendingPrizes(ctx, src, PrizeListPageSize)
	if perr != nil {
		return PrizesResult{Account: account}, perr
	}
	result := PrizesResult{Account: account, Records: records.Records}
	r.progress(core.LevelInfo, fmt.Sprintf("待领取奖品 %d 条", len(result.Records)))
	return result, nil
}

// SummaryLine 列出**至多三个**奖品名。
//
// 为什么要截：这一句会进 `runs.message` 与推送标题，而"待领取"是可以有十几条的；
// 全列会把一条通知变成一段清单。要看得全，走界面那条查询。
func (res PrizesResult) SummaryLine() string {
	if len(res.Records) == 0 {
		return "没有待领取的奖品"
	}
	names := make([]string, 0, 3)
	for _, item := range res.Records {
		if len(names) == 3 {
			break
		}
		name := "未命名奖品"
		if item.PrizeName != nil && *item.PrizeName != "" {
			name = *item.PrizeName
		}
		names = append(names, name)
	}
	text := fmt.Sprintf("待领取奖品 %d 条：%s", len(res.Records), strings.Join(names, "、"))
	if len(res.Records) > len(names) {
		text += "…"
	}
	return text
}

// RunReport：**只读，所以只有成功这一态**。
//
// 空列表算成功 —— "来看一眼，没有要过期的奖品"正是这一趟想要的答复。
// 失败走不到这里：查询失败已经作为错误交回编排层（见 [PrizesResult] 那条注释）。
func (res PrizesResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	return core.RunReport{
		Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
	}
}
