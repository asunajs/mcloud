package server

// /api/status 的测试。
//
// # 为什么不塞进 routes_test.go
//
// routes_test 测的是**路由表与冻结基准逐条对位**，期望值抄自 Rust 侧。
// 这条接口是 Go 侧后加的，**不在**那 15 条里 —— 混进去会让"哪些是冻结契约"
// 和"哪些是我们自己扩的"分不开，而这两类断言的失效方式完全不同：
// 前者坏了说明移植漏了，后者坏了只是我们自己改坏了。
//
// # 这条接口存在的意义就是"不说谎"，所以断言都盯着值本身
//
// 界面那一行拿它当"后端状态"用，报错一次的成本是**用户按一个不存在的状态做
// 决定**。所以逐字段钉死：版本必须与 `/api/version` 同源（两份真相早晚分叉）、
// `lastRun` 必须是库里**最近**那条、没有 run 时必须是 null 而不是 `{}`。

import (
	"context"
	"net/http"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
)

// TestStatusAgreesWithVersionEndpointAboutIdentity：
// 两处报的名字/版本必须同源。
func TestStatusAgreesWithVersionEndpointAboutIdentity(t *testing.T) {
	h := newHarness(t).initialized(t)

	status := decode(t, h.do(t, "GET", "/api/status", "", nil))
	version := decode(t, h.do(t, "GET", "/api/version", "", nil))

	// 期望值取自 app 层那份唯一来源，而不是拿 status 自己回显比：
	// "两个接口互相一致"是空断言 —— 两边同时错也过。
	want := app.VersionPayloadOf()
	if status["name"] != want.Name || version["name"] != want.Name {
		t.Errorf("name 应为 %q，实得 %v / %v", want.Name, status["name"], version["name"])
	}
	if status["version"] != want.Version || version["version"] != want.Version {
		t.Errorf("version 应为 %q，实得 %v / %v", want.Version, status["version"], version["version"])
	}
}

// TestStatusReportsTheLatestRunOnly：库里塞两条 run，接口必须报**最近**那条。
//
// "最近"这条不能省：ListRuns 的排序一旦反过来，界面就会显示一条半年前的
// 执行而毫看不出异样。
func TestStatusReportsTheLatestRunOnly(t *testing.T) {
	h := newHarness(t).initialized(t)
	older := seedRunAt(t, h, fixedNow.Add(-48*time.Hour), 11)
	latest := seedRunAt(t, h, fixedNow.Add(-1*time.Hour), 23)
	// 两条必须真是两条，否则下面的"报最近那条"是空断言（见 [seedRunAt] 里那条周期警告）。
	if older == latest {
		t.Fatalf("两条 run 的 id 撞了（%s）—— 换 seed", older)
	}

	status := decode(t, h.do(t, "GET", "/api/status", "", nil))
	last, ok := status["lastRun"].(map[string]any)
	if !ok {
		t.Fatalf("lastRun 应是对象，实得 %T（%v）", status["lastRun"], status["lastRun"])
	}
	if last["id"] != string(latest) {
		t.Errorf("lastRun.id 应为最近那条 %s，实得 %v（早的那条是 %s）—— 排序反了",
			latest, last["id"], older)
	}
	if last["status"] != "running" {
		t.Errorf("lastRun.status 应为 running，实得 %v", last["status"])
	}
	if last["startedAt"] != fixedNow.Add(-time.Hour).UTC().Format(time.RFC3339) {
		t.Errorf("lastRun.startedAt 应为 -1h 的 RFC3339，实得 %v", last["startedAt"])
	}
}

// TestStatusHasNoRunYet：一条 run 都没有时 lastRun 必须是 null。
//
// 落成 `{}` 的话前端读到的是 undefined，界面表现为"那一行空白" ——
// 而空白和"后端坏了"长得一模一样。
func TestStatusHasNoRunYet(t *testing.T) {
	h := newHarness(t).initialized(t)

	status := decode(t, h.do(t, "GET", "/api/status", "", nil))
	value, present := status["lastRun"]
	if !present {
		t.Fatal("lastRun 这个键必须在 —— 缺键与「没有 run」是两回事，前端区分不了")
	}
	if value != nil {
		t.Errorf("没有 run 时 lastRun 应为 null，实得 %T（%v）", value, value)
	}
}

// TestStatusReportsTheSchedulerState：`schedulerRunning` 报的是**运行时实况**。
//
// 这一条以前钉的是"恒 false"（后端确实没有循环），那句谎话有个正当用途：
// 让界面别把"存得下来"显示成"在自动跑"。循环真接上之后（`scheduler.go`），
// 常量这一档就成了谎报 —— 于是这里改钉**两个方向**：
//   - 没起循环（本夹具就是这种服务端：只构造 `Server`，不起后台）⇒ false；
//   - 起了循环 ⇒ true。
//
// 只测第一个方向等于什么都没测：把字段写回常量 `false` 依然通过，
// 而那时侧栏会在**每台正常工作的服务**上说"定时调度未启用"。
func TestStatusReportsTheSchedulerState(t *testing.T) {
	h := newHarness(t).initialized(t)
	ask := func() map[string]any {
		return decode(t, h.do(t, "GET", "/api/status", "", nil))
	}

	if status := ask(); status["schedulerRunning"] != false {
		t.Errorf("没起循环时该报 false，实得 %v", status["schedulerRunning"])
	}

	ctx, cancel := context.WithCancel(context.Background())
	stopped := make(chan struct{})
	go func() {
		defer close(stopped)
		h.server.startScheduler(ctx, func(string) {})
	}()
	if !waitSchedulerFlag(t, h, true) {
		t.Fatal("起了循环后开关位没变真（等 5 秒）")
	}
	if status := ask(); status["schedulerRunning"] != true {
		t.Errorf("起了循环该报 true，实得 %v", status["schedulerRunning"])
	}
	cancel()
	select {
	case <-stopped:
	case <-time.After(5 * time.Second):
		t.Fatal("ctx 结束后循环没退，不能再关库")
	}
	if status := ask(); status["schedulerRunning"] != false {
		t.Errorf("循环退了该回到 false，实得 %v", status["schedulerRunning"])
	}
}

// TestStatusRequiresASession：这条接口不在免登录名单里。
//
// ⚠ 它报的是"这个人最近执行过什么"，属于用户自己的数据。
func TestStatusRequiresASession(t *testing.T) {
	h := newHarness(t).initialized(t)
	bare := &harness{server: h.server, store: h.store, rng: h.rng}

	rec := bare.do(t, "GET", "/api/status", "", nil)
	if rec.Code != http.StatusUnauthorized {
		t.Errorf("未登录应 401，实得 %d：%s", rec.Code, rec.Body.String())
	}
}

// seedRunAt 塞一条只指定开始时间的 run，`seed` 决定它的 id。
//
// 不复用 server_test.go 的 [seedRun]：它绑死 `fixedNow` 且顺带塞事件流，
// 而这里要的恰恰是**两条不同时间**的 run，事件这条接口根本不读。
//
// ⚠ 为什么 id 要外面传进来：夹具 [fixedRandom] 是单字节 LCG，而种子 3 那条
// 轨道的**周期正好是 16** —— 连取两次 16 字节会得**同一个 UUID**，
// 第二条 run 于是被 `ON CONFLICT(id) DO NOTHING` 静默吞掉，
// "报最近那条"的 id 断言变成空断言（实测：只有 startedAt 那条把它揪出来）。
// 换个种子就是另一条轨道，两次取回的 id 才真的不同。
//
// ⚠ 账号那一步不是冗余：`ListRuns` 的可见性条件是
// `account_id IN (SELECT id FROM accounts WHERE owner = ?)`，
// `account_id` 为 NULL 的行在 SQL 里落进"既不真也不假"，于是**查不出来** ——
// 少了这条账号，测试会以"lastRun 是 null"通过，而那是个假绿。
func seedRunAt(t *testing.T, h *harness, at time.Time, seed byte) core.RunID {
	t.Helper()
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))
	account := core.NewAccount("A1")
	account.Owner = scope.Owner()
	if err := h.store.UpsertAccount(ctx, scope, &account); err != nil {
		t.Fatalf("塞账号失败: %v", err)
	}
	accountID := core.NewAccountID("A1")
	runID := core.NewRunID(&fixedRandom{next: seed})
	record := core.RunRecord{
		ID:         string(runID),
		AccountID:  &accountID,
		Capability: "sign",
		Status:     core.StatusRunning,
		StartedAt:  at,
	}
	if err := h.store.InsertRun(ctx, scope, &record); err != nil {
		t.Fatalf("塞 run 失败: %v", err)
	}
	return runID
}
