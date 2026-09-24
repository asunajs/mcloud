package server

// 执行分派：能力 code → "这一趟到底跑什么"。
//
// # 为什么这一层要独立成文件、并且不碰 HTTP
//
// 同一个能力有**两个触发者**：界面上那枚"开始"（`POST /api/runs`）和服务进程内的
// 调度循环（到点自己跑，见 `docs/status.md` 的调度那一节）。两者要跑的链、要的互斥键、
// 给的 runId 配对**必须是同一份实现** —— 各写一遍的代价不是重复几十行，
// 而是"界面上跑对了、定时跑的那一版漏了一步"，那种偏差没有任何测试会红。
//
// 所以本文件的入口 [launchRun] 只收 `context.Context` 与 [runSpec]，
// **不收** `*http.Request`、也不收 [AuthUser]：调度那边只有从 `schedule.owner`
// 反解出来的 [core.Scope]。HTTP 面剩下的只有"参数怎么翻、错回什么状态码"。
//
// # 分派表就是"会不会跑"的唯一真相
//
// [runBuilders] 里没有这一条能力 ⇒ [ErrNotImplemented]（501），**不**回 202。
// 判据见 `runs_test.go` 的 `TestRegisteredButUnwiredCapabilityIsNotImplemented`：
// 回 202 的后果不是错一个状态码，而是前端拿着那对 runId 去订阅事件流，
// 而那条流永远不会有事件 —— 界面上的表现与"任务还在跑"一模一样。

import (
	"context"
	"encoding/json"
	"strconv"
	"strings"
	"sync"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	mcloudexec "github.com/catlair/mcloud/internal/exec"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 执行面（进程级常驻）
// ─────────────────────────────────────────────────────────────────────────────

// RunWorld 是"跑任务"那一份常驻装配。
//
// 它住在 Server 上而不是在处理器里现拼，理由与 [Server.hub] 同一条，且更硬：
// 它持有 [mcloudexec.RunRegistry]，而**登记表的整个意义就是跨请求** ——
// 每次请求新建一份的话，"同一账号不许并发跑"这一条守卫一句都不成立。
//
// ⚠ 出站 HTTP 客户端与 WebSocket 拨号器**必须**由调用方注入（见 [newRunWorld]）：
// 本包自己不造真的，否则"测试里漏注入"的表现是真打上游、真开一条 WS，
// 而不是变红。
type RunWorld struct {
	upstream   *upstream.Upstream
	dialer     core.WebSocketDialer
	registry   *mcloudexec.RunRegistry
	deviceFile string
	rng        core.Random

	// sessions 是**跨请求**的会话票池，与上面那张登记表同一个意义：
	// 界面上连点三次签到是三次请求，而它们应当共用同一张 `jwttoken`
	// （用户 2026-09-23 的口径；判据与"为什么不设 TTL"见 `internal/app/signin_session_pool.go`）。
	// 每次请求新建一份 ⇒ 它形同不存在，表现就是每次都重打三枪。
	sessions *app.SessionPool

	// cloudPhoneSessions 是**红包派对活动站**那张票的跨请求池。
	//
	// 它与上面那份**不是同一个池、也不能共用**：签到那一条交的是 `jwttoken`，
	// 这一族每个请求要的是活动站自己的 `token`（签名算式都不一样，见
	// `internal/app/cloud_phone_session.go` 文件头）。
	// 共用一份池的表现是两条链互相顶票 ⇒ 每次都重建。
	cloudPhoneSessions *app.CloudPhoneSessionPool

	// waiter 是能力链里"等一会"的注入点（今天只有 AI 头像那条用到：合成轮询 8×3 秒）。
	//
	// 默认是 [app.RealWaiter]（真的等），只有测试夹具顶掉它（`newTestRunWorld`）。
	// ⚠ 这里**不**开一个 `Deps` / 命令行选项：那条链的三处等待是**判据的一部分**
	// （上游要 3 秒才合成得完），不是一个"用户可配置的时长"，开一栏等于承诺一个不存在的配置面。
	// 与登录那条的 [Deps.QRWaiter] 同族但不同处：那一个是"轮询扫码"的用户可见节奏。
	waiter app.Waiter

	// deviceShort 在**构造时**定一次：一次运行（乃至一个进程）对应一种设备形态。
	// 放进账号循环会让每个账号拿到不同的 `device_short`（CLI 那侧的同名注释同义）。
	deviceShort string

	// 设备指纹文件是**惰性**读/建的，所以那三件配成一次 Memo。
	//
	// 不在构造时读：`mcloud-server` 一启动就往 `data/` 写一份指纹文件，
	// 而管理员可能一次都不跑这条链。
	// ⚠ 也不能"每次请求读一遍"：文件被人换掉时那意味着两趟运行是两台机器，
	// 而上游是按设备指纹定位扫码会话与计"扫码登录过于频繁"的。
	deviceOnce  sync.Once
	deviceID    string
	deviceError *core.Error
}

// newRunWorld 装配执行面。
//
// 非导出与 [Server.hub] 一样由 `[Deps]` 那一条路构造（见 server.go）。
func newRunWorld(
	client core.HTTPClient, dialer core.WebSocketDialer,
	rng core.Random, deviceFile string,
) *RunWorld {
	return &RunWorld{
		upstream:           upstream.NewUpstream(client),
		dialer:             dialer,
		registry:           mcloudexec.NewRunRegistry(),
		deviceFile:         deviceFile,
		rng:                rng,
		sessions:           app.NewSessionPool(),
		cloudPhoneSessions: app.NewCloudPhoneSessionPool(),
		waiter:             app.RealWaiter{},
		deviceShort:        app.NewAppDeviceShort(rng),
	}
}

// appDevice 取那份"整个安装共用"的设备档案。
//
// 失败只可能是"文件读不出也写不进"（权限、路径是目录），
// 那是一台跑不了这条链的服务端 ⇒ 报 500，不去猜"要不要用一份临时的顶上"。
func (w *RunWorld) appDevice() (upstream.AppDevice, *core.Error) {
	w.deviceOnce.Do(func() {
		w.deviceID, w.deviceError = app.LoadOrCreateAppDeviceID(w.deviceFile, w.rng)
	})
	if w.deviceError != nil {
		return upstream.AppDevice{}, w.deviceError
	}
	return upstream.NewIPadAppDevice(w.deviceID, w.deviceShort), nil
}

// runSpec 是一条能力在**起跑之前**的全部产物。
//
// 刻意不含 runId：那一样由 [launchRun] 统一预分配（它是"账号↔runId 一一对应"
// 这条不变量的唯一持有者）。builder 自己填一份的话，多出来的账号就会拿到
// 一个空 runId 去落 `runs` 主键。
type runSpec struct {
	scope core.Scope
	plan  app.RunPlan
	task  app.RunTask
}

// runBuilder 把（已过键名校验的）参数翻成一份 [runSpec]。
//
// `scope` 由调用方给而不是这里反查：HTTP 来自登录态、调度来自 `schedule.owner`，
// 两处的来源不同而**含义必须相同**（"这一趟只看得见这些账号"）。
type runBuilder func(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error)

// runBuilders 是分派表：code → 怎么跑。
//
// ⚠ 键用的是 [capabilities] 里那些常量而不是字面量：同一个 code 有三处使用者
// （注册表、本表、`runs.capability` 那一列），拼错**不会编译报错也不会红**，
// 只会让一条已经接好的能力安静地回 501。
//
// 表里**没有**空位了：注册表那十八条现在全部接上了执行（2026-09-24 这一天接上四条：
// `cloud-phone-redpack` / `play-ai-special` 只接了**只读那一半**，`ai-avatar` 与
// `app-task` 接的是写那一半 —— 代价与"为什么不进编排"写在 `registry.go` 各那一条的注释里）。
// 于是 501
// 那一支只剩"注册表里根本没这个 code"的邻近情形（`runs_test.go` 的
// [TestRegisteredButUnwiredCapabilityIsNotImplemented]）—— 别把"全接上了"写成
// "这条用例不再需要"：那是唯一一处能证明"没接时不会假装受理 202"的地方。
//
// ⚠ `exchange` 接上 ≠ 真跑过：它是**不可逆消耗**（真扣云朵），本仓的领域层用例
// 是夹具/回放，"链跑成真扣豆"那一格仍标待验证（怎么验见
// `internal/server/run_dispatch_exchange.go` 文件头与 `docs/capture/app-exchange-samples.txt` §5.4）。
var runBuilders = map[string]runBuilder{
	capabilities.CodeLiveRoom:      buildLiveRoomRun,
	capabilities.CodeSign:          buildSignInRun,
	capabilities.CodeAuthRefresh:   buildAuthRefreshRun,
	capabilities.CodeCloudSnapshot: buildCloudSnapshotRun,
	capabilities.CodeReceive:       buildReceiveRun,
	capabilities.CodeMultiple:      buildClaimRun(app.ClaimKindSignInMultiple),
	capabilities.CodeBackupGift:    buildClaimRun(app.ClaimKindBackupGift),
	capabilities.CodeRevival:       buildClaimRun(app.ClaimKindRevival),
	capabilities.CodePrizes:        buildPrizesRun,
	capabilities.CodeTaskList:      buildTaskListRun,
	capabilities.CodeNotice:        buildNoticeRun,
	capabilities.CodeMsgPush:       buildMsgPushRun,
	capabilities.CodeFunAI:         buildFunaiRun,
	capabilities.CodeExchange:      buildExchangeRun,
	// 红包派对这一条**不在**上面那一族的世界里：它用的是活动站自己的票，
	// 所以走 [Server.cloudPhoneWorld] 那份装配（为什么不共用写在
	// `run_dispatch_cloud_phone.go` 文件头）。
	capabilities.CodeCloudPhoneRedpack: buildCloudPhoneRedpackRun,
	// 拍拍这一条**在**上面那一族的世界里：它今天借的就是签到页那张 `jwttoken`
	// （§1.12 的临时态，留痕与"哪天改"写在 `internal/app/play_ai_run.go` 文件头），
	// 所以它复用 [Server.signInWorld] 与那一份票池，不另起一份装配。
	capabilities.CodePlayAISpecial: buildPlayAISpecialRun,
	// AI 头像也借签到页那张票（§1.12 的临时态），但它**不走**共用外壳：那条链要
	// 多带一份"等一会"的注入点（理由见 `buildAiAvatarRun` 的注释）。
	capabilities.CodeAiAvatar: buildAiAvatarRun,
	// App 任务矩阵的**写侧**（每做一条任务一发 `task/click`）。同 AI 头像那一条：
	// 它也要那份 `Waiter` 注入点，所以不走共用外壳（见 `buildAppTaskRun` 的注释）。
	capabilities.CodeAppTask: buildAppTaskRun,
}

// builderFor 查这一条能力接上了执行没有。
func builderFor(code string) (runBuilder, bool) {
	builder, found := runBuilders[code]
	return builder, found
}

// ─────────────────────────────────────────────────────────────────────────────
// 起跑
// ─────────────────────────────────────────────────────────────────────────────

// acceptedRun 是"这一趟受理了某个账号，它的 runId 是这一个"。
//
// 它是 202 响应里那一条的领域形态，也是调度那边写 `schedule_runs` 要的东西 ——
// 两边都需要**同一对**（账号, runId），因为订阅事件流与回填历史记录都按 runId 找。
type acceptedRun struct {
	Account core.AccountID
	RunID   core.RunID
}

// launchRun 预分配 runId、后台起跑，返回账号↔runId 的配对。
//
// 它**不**写响应也不回 HTTP 状态：那两样是调用方的事（HTTP 回 202，调度记日志）。
//
// ⚠ 返回的配对与 [runSpec.plan] 里的 Accounts **同序**，而那个顺序就是执行顺序
// （串行）。前端按它渲染"第几个账号"，调度按它逐条登记。
//
// # `done` 是跑完之后的回调，HTTP 那一路传 nil
//
// 调度要把"这一趟跑成没成"写回 `schedules.last_status`（见 `scheduler.go`），
// 而那份结论只有编排层知道。它**必须**是回调而不是返回值：本函数在响应已经走了
// 之后还要占着调用方的栈等一分钟弹幕是不可接受的（202 的全部意义就是不等）。
//
// ⚠ 回调在**那个后台 goroutine 里**执行，且拿的是脱离调用方的上下文（下面那条
// `WithoutCancel` 的同一份），所以它写库时不能假设"请求还活着"。传进去的 error
// 是**编排自身**的错误（写库失败那一类），与"账号跑失败了"是两件事 ——
// 后者已经合并在 `summary.Outcome` 里。
func (s *Server) launchRun(
	ctx context.Context, spec runSpec, done func(app.RunSummary, error),
) ([]acceptedRun, *core.Error) {
	plan := spec.plan
	// 一一对应在这里就验掉，不留给 [app.Runner.Run]：那里已经起了 goroutine，
	// 长度不等会让多出来的账号拿着空 runId 落库，而那是一条谁也查不到的记录。
	if len(plan.RunIDs) != 0 && len(plan.RunIDs) != len(plan.Accounts) {
		return nil, core.NewInternal("预分配的 runId 数量与账号不等：" +
			strconv.Itoa(len(plan.RunIDs)) + " 对 " + strconv.Itoa(len(plan.Accounts)))
	}
	if len(plan.RunIDs) == 0 {
		plan.RunIDs = make([]core.RunID, 0, len(plan.Accounts))
		for range plan.Accounts {
			plan.RunIDs = append(plan.RunIDs, core.NewRunID(s.rng))
		}
	}
	out := make([]acceptedRun, 0, len(plan.Accounts))
	for i, account := range plan.Accounts {
		out = append(out, acceptedRun{Account: account, RunID: plan.RunIDs[i]})
	}

	// 推送接在这里，而不是接在某个具体能力上：**任何**一条走这条路的能力，收尾都该
	// 按同一份配置推一次 —— 各能力自己推就会各写一份门槛判断（那正是旧项目
	// "有的能力推了、有的没推"的成因）。
	//
	// `s.notify` 为 nil 时 [app.WithRunNotify] 等于不接：运行器不攒过程话、
	// 收尾一句都不推（见 `internal/app/run.go` 的 `RunSummary.Notify`）。
	runner := app.NewRunner(s.store, spec.scope, s.clock, s.runs.registry, s.bus,
		app.WithRunNotify(s.notify.RunNotify()))
	// ⚠ 脱开调用方的上下文：HTTP 那条路上客户端关掉页面，不该把一趟**已经往上游
	// 投了口令**的运行半途掐掉（那会留下一条永远 `running` 的记录，而真实动作
	// 已经发生了）；调度那条路上进程收尾时同样让它跑完而不是腰斩。
	// 与 [Server.hub] 那轮后台轮询同一条理由。
	go func() {
		// 编排自身的错误（写库失败那一类）在 HTTP 那一路**只**表现为历史里少几条记录：
		// 响应早就走了，它没有可回的地方，也不该 panic。调度那一路会把它写进
		// `schedules.last_error`（`done` 的第二个参数就是它）。
		summary, err := runner.Run(context.WithoutCancel(ctx), plan, s.rng, spec.task)
		if done != nil {
			done(summary, err)
		}
	}()
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 各条能力的 builder
// ─────────────────────────────────────────────────────────────────────────────

func buildLiveRoomRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	params, targets, perr := s.liveRoomRequest(newRunParams(capability, raw), scope, ctx)
	if perr != nil {
		return runSpec{}, perr
	}
	// 设备指纹在**起跑之前**解析：失败时这一次点"开始"直接回 500，
	// 而不是留下 N 条永远 `running` 的记录（那种记录的界面表现是"卡在开门中"）。
	device, perr := s.runs.appDevice()
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability:   capability.Code,
			Accounts:     targets,
			ParamsDigest: liveRoomDigest(params),
		},
		task: liveRoomTask(s.store, scope, s.runs, s.clock, s.rng, device, params),
	}, nil
}

// buildAuthRefreshRun 把"到点换凭据"接进同一条编排。
//
// # 与账号页那枚按钮的**唯一**区别是 force
//
// 那一枚是"人刚拍板现在就换"，恒 `force = true`；这一条路上**没有人**，
// 所以 [app.RefreshRun.RunAccount] 固定不越提前量那道闸。表现上的差别是：
// 这一条大多数时候判到 `not-yet` 就什么都不做（那正是它该做的，也算成功）。
//
// `LeadDays` 这里给 **0**，让能力层落到 [capabilities.RefreshDefaultLeadDays]：
// 与 CLI 的 `--lead-days` 缺省同一个来源，服务端不另记一份数字
// （另记一份的话，界面上说的"提前 7 天"与后端实际用的天数会各自漂移）。
func buildAuthRefreshRun(
	ctx context.Context, s *Server, scope core.Scope,
	capability capabilities.Capability, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	targets, perr := s.selectedAccounts(newRunParams(capability, raw), scope, ctx)
	if perr != nil {
		return runSpec{}, perr
	}
	device, perr := s.runs.appDevice()
	if perr != nil {
		return runSpec{}, perr
	}
	return runSpec{
		scope: scope,
		plan: app.RunPlan{
			Capability:   capability.Code,
			Accounts:     targets,
			ParamsDigest: authRefreshDigest(0),
		},
		task: authRefreshTask(s.store, scope, s.runs, s.clock, s.rng, device, 0),
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 每个账号那一步
// ─────────────────────────────────────────────────────────────────────────────

// runConclusion 是"跑完一个账号，能给四态结论"。
type runConclusion interface {
	RunReport() core.RunReport
}

// accountRunTask 把领域层的"跑一个账号"接成编排层的 [app.RunTask]。
//
// ⚠ 所有链共用它的唯一理由就是那个 nil 陷阱：`perr` 是 `*core.Error`，
// 一个 nil 的它装进 `error` 接口之后是**非空**接口，于是"这个账号跑成功了"
// 会被编排层记成一次失败（而且错误串还是空的）。各写一遍的话，
// 漏掉那一处的调用点不会报错，只会让历史列表里多出一堆没来由的红色。
func accountRunTask[T runConclusion](
	call func(context.Context, core.AccountID, app.RunProgress) (T, *core.Error),
) app.RunTask {
	return func(
		ctx context.Context, account core.AccountID, progress app.RunProgress,
	) (core.RunReport, error) {
		result, perr := call(ctx, account, progress)
		if perr != nil {
			return core.RunReport{}, perr
		}
		return result.RunReport(), nil
	}
}

// liveRoomTask 造出"跑一个账号"那一步。
//
// 它是**每个账号一次**被调用的闭包，但 `Progress` 由编排层每次现给，
// 所以这里不在闭包外共享 [app.LiveRoomRun]：那个结构体带着 Progress，
// 共享一份会让第二个账号的进度落进第一个账号的事件流里。
func liveRoomTask(
	store core.StateStore, scope core.Scope, world *RunWorld,
	clock core.Clock, rng core.Random, device upstream.AppDevice,
	params app.LiveRoomParams,
) app.RunTask {
	return accountRunTask(func(
		ctx context.Context, account core.AccountID, progress app.RunProgress,
	) (app.LiveRoomAccountResult, *core.Error) {
		return app.LiveRoomRun{
			Store: store,
			Scope: scope,
			Driver: app.LiveRoomDriver{
				Upstream: world.upstream,
				Dialer:   world.dialer,
				Random:   rng,
				Clock:    clock,
			},
			Device:   device,
			Params:   params,
			Progress: progress,
			// 票池是**跨请求**那一份：连着点三次"领取小红花"只建一条会话。
			Sessions: world.sessions,
		}.RunAccount(ctx, account)
	})
}

// authRefreshTask 同 [liveRoomTask]，链换成"到期才换凭据"。
func authRefreshTask(
	store core.StateStore, scope core.Scope, world *RunWorld,
	clock core.Clock, rng core.Random, device upstream.AppDevice, leadDays float64,
) app.RunTask {
	return accountRunTask(func(
		ctx context.Context, account core.AccountID, progress app.RunProgress,
	) (app.RefreshReport, *core.Error) {
		return app.RefreshRun{
			Store:    store,
			Scope:    scope,
			Upstream: world.upstream,
			Clock:    clock,
			Random:   rng,
			Device:   device,
			// PC 端那一支的平台：库里没记是 windows 还是 macos，只能在这里拍板。
			// 取 Windows —— 与原项目 PC 客户端的抓包基准同一形态（见 `docs/auth-refresh.md`）。
			PCPlatform: upstream.PlatformWindows,
			LeadDays:   leadDays,
			Progress:   progress,
		}.RunAccount(ctx, account)
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 互斥键的参数指纹
// ─────────────────────────────────────────────────────────────────────────────

// liveRoomDigest 算参数指纹（互斥键的一部分）。
//
// 取的是**会影响上游动作**的那几件：口令列表、两个开关、听多久。
// 不含账号（[mcloudexec.NewMutexKey] 的键里已经有账号了）。
//
// ⚠ 为什么要带参数：同一账号"只听不投"与"投一轮"是两件事，
// 不带参数的话前者会把后者挡掉，而界面上只写得下"被跳过"。
func liveRoomDigest(params app.LiveRoomParams) string {
	return mcloudexec.ParamsDigest(map[string]string{
		"codes":         strings.Join(params.Codes, ","),
		"koulingOnly":   strconv.FormatBool(params.KoulingOnly),
		"skipKouling":   strconv.FormatBool(params.SkipKouling),
		"listenSeconds": strconv.Itoa(params.ListenSeconds),
	})
}

// signInDigest 是"每日签到"这一条的参数指纹。
//
// 它的参数面（`accounts` / `accountMode`）里**没有**任何会影响上游动作的东西 ——
// 账号已经在互斥键里，执行方式只有串行。所以这一份指纹实际是**一个常量**，
// 而它仍然是对的：键的作用域是"同一账号 + 同一能力 + 同一参数"，
// 签到没有第二种参数形态，两趟签到之间**就该**互斥（同一账号签两次不产生第二次的收益，
// 只多记一次风控数）。
//
// 留 `"receive"` 这一格而不是给个空 map：将来领取变成可调开关时，
// 指纹会自然跟着分叉，不需要谁回来补这一步。
func signInDigest() string {
	return mcloudexec.ParamsDigest(map[string]string{"receive": "true"})
}

// authRefreshDigest 把提前量算进指纹。
//
// ⚠ 为什么只有它：`leadDays` 是唯一会让两趟刷新**打得不一样**的输入
// （"提前 7 天"与"提前 1 天"是两件不同的事，同时跑会各自判到不同档位）。
// 它与 [app.RefreshRun.LeadDays] 同源，所以这里的数字与真正生效的那个不会分家。
func authRefreshDigest(leadDays float64) string {
	return mcloudexec.ParamsDigest(map[string]string{
		"leadDays": strconv.FormatFloat(leadDays, 'f', -1, 64),
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 通用参数：账号与执行方式
// ─────────────────────────────────────────────────────────────────────────────

// selectedAccounts 读每条能力都有的那两栏（`accountMode` + `accounts`）。
//
// 顺序是**先方式后账号**：方式不对时连"要跑谁"都还没定，
// 先去查一遍账号表只是白打一次库。
func (s *Server) selectedAccounts(
	p runParams, scope core.Scope, ctx context.Context,
) ([]core.AccountID, *core.Error) {
	mode, perr := p.text("accountMode")
	if perr != nil {
		return nil, perr
	}
	if perr := sequentialOnly(mode); perr != nil {
		return nil, perr
	}
	ids, perr := p.texts("accounts")
	if perr != nil {
		return nil, perr
	}
	return s.resolveTargets(ctx, scope, ids)
}

// sequentialOnly 拒掉"并行"。
//
// ⚠ 执行方式**不**翻成并行：编排层只有串行，而那是原项目防风控的要求
// （见 [app.Runner.Run]）。在这里静默接受 `parallel` 等于界面上留一个
// 后端做不到的开关 —— 后果不是错，是**慢**：用户以为并行完了才去查历史，
// 而它其实还在排第一个账号。
func sequentialOnly(mode string) *core.Error {
	if mode == core.ModeSequential.String() {
		return nil
	}
	return core.NewBadRequest(
		"执行方式目前只有串行（并行会放宽对上游的节奏、容易触发风控，尚未接入）")
}
