package server

// 执行分派的**兑换**那一条：它是这一族里唯一带参数、且参数进互斥键的那一叶。
//
// # 为什么不挂在 `buildSignInChain` 那个外壳上
//
// `run_dispatch_signin.go` 的共用外壳假设了三件事：参数面只有账号两栏、
// `ParamsDigest` 留空、每个账号跑哪条链由调用点决定。兑换三条**都不满足**：
// 它多两栏参数（`prizeIdOrName` / `skipWait`），指纹**必须**非空（见下面
// [exchangeDigest] 那一段），而它是指纹非空的唯一一条 —— 为了这一条去给外壳
// 加一个"指纹"形参，等于把另外十一条的 `ParamsDigest: ""` 变成"看不见的默认值"。
// 所以这里自己组一份 [runSpec]，装配仍走 [Server.signInWorld]（票池、设备档案、
// `Progress` 每账号一份那三条不因能力而异，不该在两处各写一遍）。
//
// # 这一条**会**真扣云朵
//
// 判据、"什么算成交"、"哪些情况一枪都不发"全在 `internal/app/exchange_run.go`
// 与 `exchange_plan.go` 那两份文件头（含"只有 `514` 允许再来一轮""写操作零自动重试"
// 这两条口径的**证据**）。这里重复一遍就会漂移。
// 接线层只多做两件事：**必填那栏空着就回 400**，以及把奖品放进互斥键。
//
// ⚠ 本轮**没有真打上游**：领域层的用例是夹具/回放（`exchange_run_test.go` 头注写了
// 为什么，以及"链跑成真扣豆"那一格要怎么验）。上线后第一次真跑要按抓包 §5.4
// 那三条判据人工对账。

import (
	"context"
	"encoding/json"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
)

// buildExchangeRun 接"云朵兑换奖品"这一条。
//
// # 校验顺序：先参数、后账号
//
// `prizeIdOrName` 空着是一趟**注定什么都没做**的运行：走到领域层会得到一条
// `core.NewBadRequest`，而那时 202 早就发出去了、`runs` 表里已经落了 N 条记录，
// 界面上的表现是"N 条红色的失败历史"而不是一句"你还没说要兑哪个"。
// 所以这一栏在**起跑之前**判，回 400。
// （注册表里那枚 `MarkRequired()` 的**读取方**是界面表单：它随 `Params` 出网
// （`internal/app/payload.go`），前端用它做提交前校验与那颗红星（`exchange-page.tsx`），
// 后端处理器**不**读它 —— 少这一道判据时，界面上拦住而 CLI / 调度那条路照样放行。）
//
// ⚠ 传给领域层的是 **TrimSpace 之后**的那一份，与 [exchangeDigest] 同源：
// 两头都裁，"甲 "与"甲"才是同一件奖品、同一把锁。
func buildExchangeRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	p := newRunParams(capability, raw)
	prize, perr := p.text("prizeIdOrName")
	if perr != nil {
		return runSpec{}, perr
	}
	prize = strings.TrimSpace(prize)
	if prize == "" {
		return runSpec{}, core.NewBadRequest(
			"「" + p.label("prizeIdOrName") + "」不能为空：兑换必须点名兑哪一件")
	}
	skipWait, perr := p.flag("skipWait")
	if perr != nil {
		return runSpec{}, perr
	}
	world, targets, perr := s.signInWorld(ctx, scope, capability, raw)
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability:   capability.Code,
			Accounts:     targets,
			ParamsDigest: exchangeDigest(prize, skipWait),
		},
		task: signInTaskFor(world, func(
			ctx context.Context, w app.SignInWorld, account core.AccountID,
		) (app.ExchangeResult, *core.Error) {
			return app.ExchangeRun{SignInWorld: w, Prize: prize, SkipWait: skipWait}.
				RunAccount(ctx, account)
		}),
	}, nil
}

// exchangeDigest 把"这一趟兑哪一件、等不等整点"算进互斥键。
//
// # 为什么这一条的指纹不能留空
//
// [mcloudexec.NewMutexKey] 的键是「能力 # 账号 # 指纹」，指纹为空时退化成前两段。
// 而"同一账号兑 A"与"同一账号兑 B"是**两件事**：旧实现正是栽在这里 ——
// 它的 lockKey 不含 prizeId，于是"手动触发第二条"拿回的是**第一条的 RunRecord**
// （现象与判据见 `docs/business-feature-inventory.md` §2.5 `exchange` 小节里那条
// 「幂等缺口」：同账号绑两个奖品时，旧实现的第二条要么被 `isRunning` 整个跳过，
// 要么手动触发后拿回第一条的运行记录。）
// 这里的表现差别是：不带奖品 ⇒ 第二件排不上，界面上只写得下"被跳过"三个字，
// 而用户看到的是一句"我明明要兑 B，它说我在跑 A"。
//
// `skipWait` 同 [authRefreshDigest] 里 `leadDays` 那条理由：它是唯一另一个会让两趟
// **打得不一样**的输入 —— 不等的那一趟立刻下单，等的那一趟要到下一个整点才判、才打。
//
// # 带奖品的**代价**（这一格是刻意的取舍，不是漏了）
//
// 两件不同奖品不再互斥 ⇒ 同一账号可以并发跑两条**写**链。这与 [liveRoomDigest]
// 早就认下的口径一致（口令列表不同 = 两件事，可并发），所以这里没有另立规矩。
// ⚠ 但兑换比直播间更贵：这一族的请求里**没有幂等键**（`oid` 由服务端每次新生成），
// 两条并发链同时给同一账号下单会回什么**没有样本**。要不要给兑换加一条
// "同一账号一次只跑一条"的账号级串行 ⇒ **待用户拍板**（登记在 `docs/status.md`）。
//
// ⚠ 布尔的写法与 [liveRoomDigest] 同源（`strconv.FormatBool`）：指纹只要**内部自洽**
// 就成立，但两条链对同一个 `true` 压出两个不同摘要，将来做"跨能力去重"时就是暗坑。
func exchangeDigest(prize string, skipWait bool) string {
	return mcloudexec.ParamsDigest(map[string]string{
		"prizeIdOrName": prize,
		"skipWait":      strconv.FormatBool(skipWait),
	})
}
