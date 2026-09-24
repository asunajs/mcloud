package app

// 「只领取、不签到」跑一个账号那一段。
//
// # 为什么单独一条能力
//
// [SignInRun] 那条链把领取做成了签到的**后半段**（`Receive` 由调用点写死 `true`），
// 于是"今天已经签过了、只想把账上挂着的领掉"没有入口 —— 而那是这条链最常见的补漏场景：
// 领取失败多半是**当时**取不到 `deviceId` 或上游临时回 `615`，签上的那一步不用重来，
// 只需要把领取再跑一遍。
//
// # 与 [ReceiveAll] 的分工
//
// 判据（`receiveList` 里哪条该走哪个接口）全在 [PlanReceive]，本文件只做三件事：
// 拿票、换 `deviceId`、把逐条实况合成四态结论。

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// ReceiveRun 跑一次"按上游账目自己判断该领哪些、把它们领掉"。
type ReceiveRun struct {
	SignInWorld
}

// ReceiveResult 是一个账号这一趟的实况。
//
// ⚠ 这里**没有**"整批失败"那一格：读 `infoV3` 失败会作为 `*core.Error` 交回编排层，
// 那条路多发一条 `errored` 事件、错误原文直接进 `runs.message`
// （见 `run.go` 里 `taskErr` 那一支），比在这里折成失败结论说得更清楚。
type ReceiveResult struct {
	Account core.AccountID
	// Outcomes 是**逐条**结果（含"下月才可领"那种没发请求的）。
	Outcomes []ReceiveOutcome
	// DeviceFailure 是 H5 `deviceId` 没换到的原因 —— 领取**硬需要**它（不带回 `615`）。
	//
	// ⚠ 有值时仍然**照发**：那是风控侧的问题，预先判死会让一次本可以成的领取
	// 变成"什么都没做"，而 `615` 本身就是明确答案。
	DeviceFailure string
}

func (r ReceiveRun) RunAccount(
	ctx context.Context, account core.AccountID,
) (ReceiveResult, *core.Error) {
	src, perr := r.Source(ctx, account)
	if perr != nil {
		return ReceiveResult{Account: account}, perr
	}
	result := ReceiveResult{Account: account}

	deviceID, derr := r.DeviceIDFor(ctx, account)
	if derr != nil {
		result.DeviceFailure = derr.Error()
		r.progress(core.LevelWarn, "⚠ 取 H5 deviceId 失败，本次领取大概率回 615："+derr.Error())
	}

	outcomes, perr := ReceiveAll(ctx, src, deviceID)
	if perr != nil {
		return ReceiveResult{Account: account, DeviceFailure: result.DeviceFailure}, perr
	}
	result.Outcomes = outcomes
	return result, nil
}

// SummaryLine 是给人看的一句话小结。
func (res ReceiveResult) SummaryLine() string {
	var gained int64
	var failed, nextMonth, claimed int
	for _, item := range res.Outcomes {
		switch item.Kind {
		case ReceiveOutcomeImmediate:
			claimed++
			gained += item.Received
		case ReceiveOutcomeMonthlyExpansion:
			claimed++
			if item.CloudCount != nil {
				gained += *item.CloudCount
			}
		case ReceiveOutcomeNextMonth:
			nextMonth++
		case ReceiveOutcomeFailed:
			failed++
		}
	}
	if len(res.Outcomes) == 0 {
		return "账上没有可领的云朵"
	}
	text := fmt.Sprintf("领到 %d 豆（%d 笔）", gained, claimed)
	if nextMonth > 0 {
		text += fmt.Sprintf("；%d 笔下月才可领", nextMonth)
	}
	if failed > 0 {
		text += fmt.Sprintf("；%d 笔失败", failed)
	}
	if res.DeviceFailure != "" {
		text += "；deviceId 没换到"
	}
	return text
}

// RunReport 把逐条实况合成四态结论。
//
//   - **全部可领的都领到了** ⇒ 成功；
//   - **账上没有可领的** ⇒ 也是成功（"来看一眼、没东西可领"就是这一趟的正常答案）；
//   - **领到一部分、另一部分失败** ⇒ 部分成功：已经进账的不会因重跑退回去，
//     所以它既不是失败也不该报成功（与 [SignInAccountResult.RunReport] 同一判据）；
//   - **一笔都没成**（发了请求全回错误）⇒ 失败。
//
// ⚠ "读 `infoV3` 就挂了"走不到这里 —— 那是错误，编排层直接报失败。
func (res ReceiveResult) RunReport() core.RunReport {
	summary := res.SummaryLine()
	var claimed, failed int
	for _, item := range res.Outcomes {
		switch item.Kind {
		case ReceiveOutcomeImmediate, ReceiveOutcomeMonthlyExpansion:
			claimed++
		case ReceiveOutcomeFailed:
			failed++
		}
	}
	switch {
	case failed == 0:
		return core.RunReport{
			Outcome: core.OutcomeSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	case claimed > 0:
		return core.RunReport{
			Outcome: core.OutcomePartialSuccess, ManualSteps: []core.ManualStep{}, Summary: &summary,
		}
	default:
		return core.NewRunReportFailed(summary)
	}
}
