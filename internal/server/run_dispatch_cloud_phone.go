package server

// 执行分派的**红包派对**那一条（以及将来这一族的其余几条）。
//
// # 为什么不塞进 `run_dispatch_signin.go`
//
// 那一份文件的全部前提都是"这一族共用签到页那一条会话"：`buildSignInChain` 建的是
// [app.SignInWorld]，接的是 `s.runs.sessions` 那张 `jwttoken` 池。
// 红包派对要的是**活动站自己的票**（`token` + 每次重算的 `sign`），
// 两池共用一份的表现是两条链互相顶票、每次都重建（`cloudPhoneWorld` 那里再写一次）。
//
// 所以这里照 [buildSignInChain] 的形状另起一份外壳：装配只留 [Server.cloudPhoneWorld]
// 一处，`Progress` 仍然**每账号一份**（同一条理由：共享会让第二个账号的过程话
// 进第一个账号的事件流）。
//
// # 这一层**不写**上游调用
//
// 判据、请求与解析全在 `internal/app/cloud_phone_run.go` 与 `cloud_phone_session.go`，
// 连"哪一半没接"（浏览 / 答题 / 装 APK）也写在那份文件的头注里。这里只挂 code。

import (
	"context"
	"encoding/json"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// cloudPhoneWorld 装配"以红包派对活动站身份跑一个账号"那一份世界，顺带解析目标账号。
//
// 与 [Server.signInWorld] 同一条硬约束：**账号筛选与世界里的 `Scope` 必须同源**，
// 分开取的话"筛到别人的账号"这一类越权不会报错。
//
// ⚠ `Sessions` 接的是 [RunWorld] 上那一份**跨请求**活动站票池；漏接的表现与签到那条
// 一样 —— 每次点击多打两枪换票（换这一族的票是两枪：换 SSO token + `tokenValidate`）。
func (s *Server) cloudPhoneWorld(
	ctx context.Context, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (app.CloudPhoneWorld, []core.AccountID, *core.Error) {
	targets, perr := s.selectedAccounts(newRunParams(capability, raw), scope, ctx)
	if perr != nil {
		return app.CloudPhoneWorld{}, nil, perr
	}
	// 设备档案在**起跑之前**解析：失败时这一次点"开始"直接回 500，而不是留下
	// N 条永远 `running` 的记录（理由同 [buildSignInRun] 那处注释）。
	device, perr := s.runs.appDevice()
	if perr != nil {
		return app.CloudPhoneWorld{}, nil, perr
	}
	return app.CloudPhoneWorld{
		Store:    s.store,
		Scope:    scope,
		Upstream: s.runs.upstream,
		Device:   device,
		Clock:    s.clock,
		Random:   s.rng,
		Sessions: s.runs.cloudPhoneSessions,
	}, targets, nil
}

// buildCloudPhoneRedpackRun 接"红包派对盘点"这一条**只看不动**的查询。
//
// ⚠ 这一条 builder 与上面那十条的唯一实质差别是它换了**一份世界**，
// 而这不是"顺手抄一份"能省掉的：抄 [buildSignInChain] 会让这一族借用签到页那张票，
// 表现是请求头里 `token` 位是别人的会话串 —— 上游回什么**未测试**，
// 但"两条链互相顶票"这一条确定会发生（两池键不同，共用 map 就是互相清）。
func buildCloudPhoneRedpackRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	base, targets, perr := s.cloudPhoneWorld(ctx, scope, capability, raw)
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability: capability.Code, Accounts: targets, ParamsDigest: "",
		},
		task: cloudPhoneTaskFor(base),
	}, nil
}

// cloudPhoneTaskFor 把"以这份世界跑一个账号"接成编排层的 [app.RunTask]。
//
// 每个账号一份世界拷贝：[app.CloudPhoneWorld.Progress] 落的是**这一次运行**的事件流
// （与 [signInTaskFor] 同一条理由，也同一个坑）。
//
// 今天这一族只有一条能力，所以它**不**做成泛型外壳：那层抽象的价值来自"十条只差最后
// 一行"（`buildSignInChain` 的注释写的就是这件事），一条做成泛型只会让下一个读者
// 找不到"这条链到底跑了什么"。真接上答题 / 浏览那一类那天，再按同一形状抽。
func cloudPhoneTaskFor(base app.CloudPhoneWorld) app.RunTask {
	return accountRunTask(func(
		ctx context.Context, account core.AccountID, progress app.RunProgress,
	) (app.CloudPhoneResult, *core.Error) {
		world := base
		world.Progress = progress
		return app.CloudPhoneRun{CloudPhoneWorld: world}.RunAccount(ctx, account)
	})
}
