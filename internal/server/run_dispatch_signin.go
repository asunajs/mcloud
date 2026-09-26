package server

// 执行分派的**签到页那一族**：十四个能力共用同一份世界装配。
//
// # 为什么单独一个文件
//
// `run_dispatch.go` 承载的是"分派表 + 起跑 + 互斥键"这套骨架；本文件全是
// **同一形状的叶子**：取账号 → 装世界 → 交给 `internal/app` 那条链。
// 十四个各写一份 builder 的话，真正会漂移的是那段共用装配
// （尤其"必须接票池"那一句），所以装配只留 [Server.signInWorld] 一处，
// 外壳只留 [buildSignInChain] 一处。
//
// ⚠ "共用这一份装配"的**前提**是"共用这一张票"。2026-09-24 新接的两条里，
// `play-ai-special` 满足它（今天借签到页那张 `jwttoken`，见 `play_ai_run.go` 文件头那条
// AGENTS §1.12 的临时态），而 `cloud-phone-redpack` **不**满足（活动站自己的票）
// ⇒ 那一条在 `run_dispatch_cloud_phone.go`，两份池共用会互相顶票。
//
// # 这一层**不写**上游调用
//
// 判据、请求与解析全在 `internal/app`：签到页那七条用的是
// `ClaimMultiple` / `ClaimBackupGift` / `ReceiveAll` / `ReceiveRevivalReward` /
// `PendingPrizes` / `RecordCloud`；2026-09-23 接上的四条（`task-list` / `notice` /
// `msg-push` / `fun-ai`）各自带来一条**新**上游接口，那四口的证据与"哪些仍待验证"
// 写在 `internal/upstream/app_*.go` 的文件头。
// 这里只把它们挂上能力 code，让界面上的"开始"与定时任务的 `target.code` 指得到。

import (
	"context"
	"encoding/json"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// signInWorld 装配"以签到页身份跑一个账号"那一份世界，顺带解析目标账号。
//
// 两件事必须**同源**：账号是按参数与可见范围筛出来的，而世界里的 `Scope` 决定
// 这些账号的凭据读不读得到 —— 分开取的话，"筛到别人的账号"这一类越权不会报错。
//
// ⚠ `Sessions` 接的是 [RunWorld] 上那一份**跨请求**票池：漏接的表现不是报错，
// 而是每次点击重打三枪建会话（用户 2026-09-23 点名不许，见 `signin_session_pool.go` 头注）。
// ⚠ `Progress` **不在**这里给：它是每个账号一份，由 [signInTaskFor] 在闭包里现装。
func (s *Server) signInWorld(
	ctx context.Context, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (app.SignInWorld, []core.AccountID, *core.Error) {
	targets, perr := s.selectedAccounts(newRunParams(capability, raw), scope, ctx)
	if perr != nil {
		return app.SignInWorld{}, nil, perr
	}
	world, perr := s.signInWorldBase(scope)
	if perr != nil {
		return app.SignInWorld{}, nil, perr
	}
	return world, targets, nil
}

// signInWorldBase 只装"以签到页身份跑这个 scope"那一份世界，**不**碰参数与账号筛选。
//
// 拆出来只为一件事：只读探针（`GET /api/accounts/{id}/exchange-prizes`）也要同一份装配
// —— 同一份上游客户端、同一份进程级票池。两处各写一遍的话，最先漂移的一定是
// `Sessions` 那一句（漏了它就是"每次点按钮都重新建会话、多打三枪"，
// 而它不会报错，只会让界面上那一枪变慢）。
//
// 设备档案在这里就解析（不是延到用账号时）：失败时调用点直接回 500，
// 而不是留下 N 条永远 `running` 的记录（理由同 [buildLiveRoomRun]）。
func (s *Server) signInWorldBase(scope core.Scope) (app.SignInWorld, *core.Error) {
	device, perr := s.runs.appDevice()
	if perr != nil {
		return app.SignInWorld{}, perr
	}
	return app.SignInWorld{
		Store:    s.store,
		Scope:    scope,
		Upstream: s.runs.upstream,
		Device:   device,
		Clock:    s.clock,
		Random:   s.rng,
		Sessions: s.runs.sessions,
	}, nil
}

// signInTaskFor 把"以这份世界跑一个账号"接成编排层的 [app.RunTask]。
//
// 每个账号一份世界拷贝：[app.SignInWorld.Progress] 落的是**这一次运行**的事件流，
// 共享一份会让第二个账号的过程话进第一个账号的事件里（[liveRoomTask] 同一条理由）。
func signInTaskFor[T runConclusion](
	base app.SignInWorld,
	call func(context.Context, app.SignInWorld, core.AccountID) (T, *core.Error),
) app.RunTask {
	return accountRunTask(func(
		ctx context.Context, account core.AccountID, progress app.RunProgress,
	) (T, *core.Error) {
		world := base
		world.Progress = progress
		return call(ctx, world, account)
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// builder
// ─────────────────────────────────────────────────────────────────────────────

// buildSignInChain 是这一族 builder 的**共用外壳**：装世界 → 组 plan → 接任务。
//
// 为什么要抽出这一层：这一族有十一条能力走这一个外壳，而它们之间**唯一**的差别
// 就是最后那一行调用。
// 各写一份的话，"必须接票池""`Progress` 每账号一份""`ParamsDigest` 留空"这三条
// 就会在十处各自漂移 —— 而这类漂移不报错，只在界面上表现为"第二次点击又建了一条会话"。
//
// ⚠ 只有 [buildSignInRun] 不走这一层：它的参数面是对拍面，且 plan 里要带
// `signInDigest()` 那一份指纹（见它的注释）。
func buildSignInChain[T runConclusion](
	call func(
		context.Context, app.SignInWorld, core.AccountID,
	) (T, *core.Error),
) runBuilder {
	return func(
		ctx context.Context, s *Server, scope core.Scope,
		capability capabilities.Capability, raw map[string]json.RawMessage,
	) (runSpec, *core.Error) {
		world, targets, perr := s.signInWorld(ctx, scope, capability, raw)
		if perr != nil {
			return runSpec{}, perr
		}
		return runSpec{
			scope: scope,
			plan: app.RunPlan{
				Capability: capability.Code, Accounts: targets, ParamsDigest: "",
			},
			task: signInTaskFor(world, call),
		}, nil
	}
}

// buildSignInRun 是"每日签到 + 顺手领取"这一条。
//
// 参数面**只有** `accounts` 与 `accountMode` 两栏（[capabilities.accountSelector]），
// 而且这**是**对拍面：`sign` 的声明与 Rust 侧逐字对齐，所以这里不能顺手加一栏
// "要不要领取"。领取是这条链的固有部分 —— 声明里那半句"并领取奖励"就是它的承诺，
// 于是 `Receive` 在这个调用点上显式写 `true`（理由见 [app.SignInRun.Receive]）。
func buildSignInRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	world, targets, perr := s.signInWorld(ctx, scope, capability, raw)
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability:   capability.Code,
			Accounts:     targets,
			ParamsDigest: signInDigest(),
		},
		task: signInTaskFor(world, func(
			ctx context.Context, w app.SignInWorld, account core.AccountID,
		) (app.SignInAccountResult, *core.Error) {
			return app.SignInRunFrom(w, true).RunAccount(ctx, account)
		}),
	}, nil
}

// buildCloudSnapshotRun 把登记了很久却没接上的 `cloud-snapshot` 接上执行。
//
// 它此前正是"注册表里有名字、分派表里没有"那一类（点"开始"回 501），
// 而领域层（[app.RecordCloud]）早就写好了 —— 缺的只有这一行登记。
var buildCloudSnapshotRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.CloudSnapshotResult, *core.Error) {
	return app.CloudSnapshotRun{SignInWorld: w}.RunAccount(ctx, account)
})

var buildReceiveRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.ReceiveResult, *core.Error) {
	return app.ReceiveRun{SignInWorld: w}.RunAccount(ctx, account)
})

// buildClaimRun 接三条"月度 / 一次性开关"：翻倍、备份好礼、复活卡。
//
// 三条只差一个 [app.ClaimKind]，所以只差这一处 —— 结论映射（含"未开备份 ⇒ 要人动手"
// 那一档）在领域层共用一份（`signin_claims_run.go` 头注写了为什么共用）。
//
// ⚠ 返回的是 [runBuilder]，所以它出现在分派表的**字面量**里；
// 传错的 `kind` 不会编译报错（三档都是同一个类型），
// 由 `run_dispatch_signin_test.go` 按"每条 code 跑哪条链"钉住。
func buildClaimRun(kind app.ClaimKind) runBuilder {
	return buildSignInChain(func(
		ctx context.Context, w app.SignInWorld, account core.AccountID,
	) (app.ClaimResult, *core.Error) {
		return app.ClaimRun{SignInWorld: w, Kind: kind}.RunAccount(ctx, account)
	})
}

// buildPrizesRun 接"待领取奖品"这一条**只读**查询。
//
// 它只列不领：领奖品那两口（详情 + 领取）**尚无抓包样本**，
// 所以声明里那句"本工具只列不领"不是谦虚，是边界
// （见 `docs/business-feature-inventory.md` §2.1 `prizes`）。
var buildPrizesRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.PrizesResult, *core.Error) {
	return app.PrizesRun{SignInWorld: w}.RunAccount(ctx, account)
})

// ── 新链路批（2026-09-23）：四条各带一条新上游接口 ──────────────────────────
//
// 这四条**只是接线**：判据、请求与解析全在 `internal/app` 那四份文件里，
// 连"哪一格仍是猜的"也写在那边（`notice_run.go` 的两个状态常量、
// `msg_push_run.go` 的零样本声明、`funai_run.go` 的"最多抽一次"）。
// 这里重复一遍就会漂移。

// buildTaskListRun 接"任务清单"这一条**只查不做**的查询。
//
// ⚠ 它不打 `doTaskPost`：做任务的口子（`task/click`、`receiveTask`）在领域层
// 刻意没接，判据在 `task_list_run.go` 的文件头。
var buildTaskListRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.TaskListResult, *core.Error) {
	return app.TaskListRun{SignInWorld: w}.RunAccount(ctx, account)
})

// buildNoticeRun 接"开启通知领云朵"：查两槽 → 该领时领一次 → 领完复查。
//
// 这一条**会**发进账请求，所以它出现在分派表里就意味着定时任务也能触发它 ——
// 会不会触发由注册表那条的 `RunInBatch` 决定，不在这里判。
var buildNoticeRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.NoticeResult, *core.Error) {
	return app.NoticeRun{SignInWorld: w}.RunAccount(ctx, account)
})

// buildMsgPushRun 接"消息推送任务"。
//
// ⚠ 这一族的接口层**零样本**（口径见 `internal/app/msg_push_run.go` 文件头）：
// 接上它是为了让界面上点得到那一下 —— 手工点一次是拿到第一个样本的最快路径。
// 它不进编排，所以自动跑不会替一个可能已下线的端点每天产生一条失败历史。
var buildMsgPushRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.MsgPushResult, *core.Error) {
	return app.MsgPushRun{SignInWorld: w}.RunAccount(ctx, account)
})

// buildFunaiRun 接"趣玩AI抽奖"：读一次判据 → **最多**抽一次。
//
// 每日 1 次的上限由领域层钉住，不在这一层重复；参数面只有账号两栏。
var buildFunaiRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.FunaiResult, *core.Error) {
	return app.FunaiRun{SignInWorld: w}.RunAccount(ctx, account)
})

// buildPlayAISpecialRun 接"拍拍盘点"：查活动侧状态 + 查任务表，**只读**。
//
// 它走 [buildSignInChain]（也就是复用 [Server.signInWorld] 那一份装配与**签到页那份票池**），
// 这不是偷懒而是**今天的实情**：这一条链借的就是签到页那张 `jwttoken`
// （AGENTS §1.12 的临时态，留痕与"哪天改成特有取法要动哪一处"写在 `play_ai_run.go` 文件头）。
// ⚠ 与它同族的红包派对那一条**不能**共用这一份装配：那一条用的是活动站自己的票，
// 池也得分开（`run_dispatch_cloud_phone.go` 文件头写的就是那件事）。
//
// ⚠ 这一条**不发** ③ 上报、④ 抽奖、⑤ 图片识别：三口各自的代价与卡点
// 记在 `play_ai_run.go` 文件头与 `docs/pending-decisions.md`（P1 / P4 / P5 / P6）。
var buildPlayAISpecialRun = buildSignInChain(func(
	ctx context.Context, w app.SignInWorld, account core.AccountID,
) (app.PlayAIResult, *core.Error) {
	return app.PlayAIRun{SignInWorld: w}.RunAccount(ctx, account)
})

// buildAiAvatarRun 接"AI 头像生成与抽奖"：生成 5 张 → 抽完当天次数 → 找回网盘产物。
//
// 它是这一族里**唯一不走 [buildSignInChain] 共用外壳**的一条（除 `sign` 之外），
// 因为要多塞一样东西：那条链的三处等待（合成轮询 8×3 秒、轮间 2 秒、抽奖间隔 1 秒）
// 走 [RunWorld.waiter] 那一份注入点。用共用外壳的话，接线用例得真睡两分钟 ——
// 而"真睡"从来不是这条链要断言的东西，要断言的是"等了多久、等了几次"（在
// `internal/app/ai_avatar_run_test.go` 那侧用记录型 waiter 钉）。
//
// ⚠ 这一条**会往用户网盘写文件**，且注册表里 `RunInBatch: false` ⇒ 调度不会替它跑，
// 只有界面上点那一下会真的发出去（为什么允许接写请求见 `registry.go` 那一条注释）。
func buildAiAvatarRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	world, targets, perr := s.signInWorld(ctx, scope, capability, raw)
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability: capability.Code, Accounts: targets, ParamsDigest: "",
		},
		task: signInTaskFor(world, func(
			ctx context.Context, w app.SignInWorld, account core.AccountID,
		) (app.AiAvatarResult, *core.Error) {
			return app.AiAvatarRun{SignInWorld: w, Wait: s.runs.waiter}.RunAccount(ctx, account)
		}),
	}, nil
}

// buildTaskExpansionRun 接「任务拓展」：读一次 →（该凑才）传一个文件凑备份 →
// 等一会儿 → 重读 →（该领才）领掉。
//
// 它与 [buildAiAvatarRun] 同样**不走** [buildSignInChain] 共用外壳，因为要多塞一份
// [app.Waiter]：那条链在凑完备份之后要等 20 秒才重读（旧实现那句
// `sleep((backupWaitTime||20)*1000)`）。用共用外壳的话，接线用例里凑备份那一支就得
// 真睡 20 秒 —— 而这一层要断言的不是"睡了"，是"等了几次、等了多久、什么情况下不等"
// （判据在 `internal/app/task_expansion_run_test.go`，那一支的"不等"是刻意差别之一）。
//
// ⚠ 这一条**会往用户网盘写一个文件且不删**（注册表里 `RunInBatch: false`，
// 代价与"为什么这条链不清理"写在 `internal/app/task_expansion_run.go` 文件头）。
func buildTaskExpansionRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	world, targets, perr := s.signInWorld(ctx, scope, capability, raw)
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability: capability.Code, Accounts: targets, ParamsDigest: "",
		},
		task: signInTaskFor(world, func(
			ctx context.Context, w app.SignInWorld, account core.AccountID,
		) (app.TaskExpansionResult, *core.Error) {
			return app.TaskExpansionRun{SignInWorld: w, Wait: s.runs.waiter}.RunAccount(ctx, account)
		}),
	}, nil
}

// buildAppTaskRun 接「App 任务矩阵」的**写侧**：读整张表 → 逐条判定 → 打卡 / 跑玩法 → 复查。
//
// 它与 [buildAiAvatarRun] 同样**不走** [buildSignInChain] 共用外壳，因为要多塞一份
// [app.Waiter]：这条链每发出一枪要等 500ms（旧实现那句 `sleep(500)`，为什么保留写在
// `internal/app/task_matrix_run.go` 的 `taskMatrixPace` 旁边）。用共用外壳的话，
// 十几条待做任务就是接线用例里真睡十几秒 —— 而这一层要断言的从来不是"睡了"，
// 是"该等的等了几次"（判据在 `internal/app/task_matrix_run_test.go`）。
//
// ⚠ 这一条**会写用户的账**（`task/click` 一发出去，云朵可能当场入账且没有撤销那一口），
// 且注册表里 `RunInBatch: false` ⇒ 调度不会替它跑，只有界面上点那一下会真发出去。
// "本仓缺端点族"的那些任务一律不补打卡 —— 那条取向与它的代价写在
// `internal/app/task_matrix_dispatch.go` 文件头。
func buildAppTaskRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	world, targets, perr := s.signInWorld(ctx, scope, capability, raw)
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability: capability.Code, Accounts: targets, ParamsDigest: "",
		},
		task: signInTaskFor(world, func(
			ctx context.Context, w app.SignInWorld, account core.AccountID,
		) (app.TaskMatrixRunResult, *core.Error) {
			return app.TaskMatrixRun{SignInWorld: w, Wait: s.runs.waiter}.RunAccount(ctx, account)
		}),
	}, nil
}
