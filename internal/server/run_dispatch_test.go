package server

// 执行分派层（`run_dispatch.go`）的用例。
//
// 这一层只有三件事值得钉：
//
//  1. **分派表的键必须是注册表认识的 code** —— 拼错不报错，只表现成"这条能力回 501"；
//  2. **接上了就必须真的受理并留下结局**：202 + 配对 + 事件流里有终局那一条；
//  3. **通用那两栏对每条能力都同样成立**（`accountMode` 只有串行、参数键名以声明为准）。
//
// ⚠ 链本身（签到打哪些接口、刷新判到哪一档）**不在这里测** —— 判据在 `internal/app`
// （`signin_run_test.go`、`auth_refresh_test.go`）。这里刻意让第一枪就失败：
// 要证的是"分派出去的那一步会跑到终局"，不是它跑对了。

import (
	"context"
	"net/http"
	"slices"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 分派表本身
// ─────────────────────────────────────────────────────────────────────────────

// TestEveryDispatchKeyIsARegisteredCapability：⭐ 分派表的键名自检。
//
// 这条用例挡的是本层**唯一一种"不会红的错"**：`runBuilders` 里把
// [capabilities.CodeSign] 写成 `"sign "` 或 `"signin"` 时，编译通过、
// 注册表照常、签到链本身也全绿 —— 只有用户点"开始"时拿到一句 501。
// 拼错的这一处没有任何别的测试会红，所以自检必须存在。
func TestEveryDispatchKeyIsARegisteredCapability(t *testing.T) {
	for code := range runBuilders {
		if _, found := capabilities.Get(code); !found {
			t.Errorf("分派表里有 %q，注册表里没有这个能力：这一条永远不会被命中", code)
		}
	}
	// 反方向也要看一眼，但它**不是**失败条件：注册表里本来就有"登记了还没接上"
	// 的能力（那正是 501 那条用例负责的）。这里只把实况打出来，让人看见差集。
	for _, capability := range capabilities.All() {
		if _, wired := runBuilders[capability.Code]; !wired {
			t.Logf("还没接上执行：%s（%s）", capability.Code, capability.Title)
		}
	}
}

// TestUnwiredCapabilitiesAreExactlyTheKnownTwo：接上几条 = 分派表几条。
//
// 与 [TestRegisteredButUnwiredCapabilityIsNotImplemented] 是一对：那边钉"没接的确实回 501"，
// 这一条钉"表里没有偷偷多出第四行"。多出来的那一行要么没有用例（不该存在），
// 要么就该同时出现在受理用例里。
func TestUnwiredCapabilitiesAreExactlyTheKnownTwo(t *testing.T) {
	var unwired []string
	for _, capability := range capabilities.All() {
		if _, wired := runBuilders[capability.Code]; !wired {
			unwired = append(unwired, capability.Code)
		}
	}
	slices.Sort(unwired)
	if want := []string{"cloud-snapshot", "exchange"}; !slices.Equal(unwired, want) {
		t.Fatalf("未接上执行的能力应是 %v，实得 %v", want, unwired)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 接上了的两条新链：受理 → 终局
// ─────────────────────────────────────────────────────────────────────────────

// TestSignInIsAcceptedAndReachesTerminalEvent：`sign` 走的是同一条 202 + 事件流。
//
// 它**不**需要 `codes` 那一栏（参数面只有 `accounts` / `accountMode`），
// 所以空参数就该受理 —— 这条同时钉住了"`sign` 的缺省 = 全部启用账号"。
func TestSignInIsAcceptedAndReachesTerminalEvent(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs", `{"capability":"sign","params":{}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("签到应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != 1 {
		t.Fatalf("应给出一个账号的配对，实得 %v", rows)
	}
	if asText(t, rows[0]["accountId"]) == "" {
		t.Errorf("配对里没有账号：%v", rows[0])
	}
	runID, ok := core.ParseRunID(asText(t, rows[0]["runId"]))
	if !ok {
		t.Fatalf("配对的 runId 不合法：%v", rows[0])
	}
	events := waitForRunFinished(t, h, runID)
	// ⚠ 签到整条链都是 HTTP，**不该**开 WS。默认那份 [refusingDialer] 不会自己让
	// 用例变红（它的报错只会成为事件流里的一条），所以要显式往事件里看一眼：
	// 装错世界（把直播那套 driver 塞进签到）的表现是这里出现"拒绝开 WS"那句话。
	for _, item := range events {
		if strings.Contains(item.Event.Message, "拒绝开 WS") {
			t.Errorf("签到这一趟去开了一条 WS，事件里写着：%s", item.Event.Message)
		}
	}
	assertRunCapability(t, h, rows, "sign")
}

// TestAuthRefreshIsAcceptedAndReachesTerminalEvent：`auth-refresh` 也接上了同一条编排。
//
// 这一条存在的意义是"到点换凭据**有**一个可寻址的入口"：它此前只是注册表里
// 一个名字（`TestRegisteredButUnwiredCapabilityIsNotImplemented` 里它回 501），
// 而调度要指的就是这一个 code。
//
// ⚠ 不判它落在四态的哪一格：那由库里那串的到期时刻决定，判据在能力层
// （`internal/capabilities/auth_refresh_test.go`）。这里只看到"它跑到了终局"。
func TestAuthRefreshIsAcceptedAndReachesTerminalEvent(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs", `{"capability":"auth-refresh","params":{}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("刷新凭据应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != 1 {
		t.Fatalf("应给出一个账号的配对，实得 %v", rows)
	}
	waitRuns(t, h, rows)
	assertRunCapability(t, h, rows, "auth-refresh")
}

// assertRunCapability 回查库里那一条：能力名与配对里的 runId 对得上。
//
// `runs.capability` 那一列是历史列表分组与"这次跑的是什么"的唯一依据，
// 而它由编排层从 [app.RunPlan.Capability] 写下来 —— 分派表传错的话，
// 界面上会把签到显示成别的任务，且没有任何一处报错。
func assertRunCapability(t *testing.T, h *harness, rows []map[string]any, want string) {
	t.Helper()
	runs, err := h.store.ListRuns(context.Background(), core.ScopeUser(userIDOf(t, h)), 200)
	if err != nil {
		t.Fatalf("列 run 失败: %v", err)
	}
	for _, row := range rows {
		id := asText(t, row["runId"])
		for _, run := range runs {
			if run.ID != id {
				continue
			}
			if run.Capability != want {
				t.Errorf("运行记录里写的是 %q，应为 %q（id %s）", run.Capability, want, id)
			}
			return
		}
	}
	t.Errorf("配对里的 runId 在库里没有对应记录：%v（库里 %d 条）", rows, len(runs))
}

// ─────────────────────────────────────────────────────────────────────────────
// 通用参数：对每条接上的能力都同样成立
// ─────────────────────────────────────────────────────────────────────────────

// TestParallelModeIsRefusedOnEveryWiredCapability：串行那条闸门**不分能力**。
//
// 判据共用一处（[sequentialOnly]），所以用例也共用一条：只测 `live-room` 的话，
// 新接的 `sign` 完全可以漏掉这一拦而没人发现 —— 而界面上那个下拉对每条能力都渲染。
func TestParallelModeIsRefusedOnEveryWiredCapability(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	for _, code := range []string{"sign", "auth-refresh", "live-room"} {
		rec := h.do(t, "POST", "/api/runs",
			`{"capability":`+quoteJSON(code)+`,"params":{"accountMode":"parallel"}}`, nil)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("%s 的并行应 400，实得 %d：%s",
				code, rec.Code, brief(rec.Body.String()))
			continue
		}
		if message, _ := decode(t, rec)["message"].(string); !strings.Contains(message, "串行") {
			t.Errorf("%s 的 400 该说清只有串行，实得 %q", code, message)
		}
	}
	if ids := storedRunIDs(t, h); len(ids) != 0 {
		t.Errorf("被拒的请求留下了运行记录：%v", ids)
	}
}

// TestSignInDoesNotAcceptAReceiveSwitch：`sign` 的参数面**不许**长出新一栏。
//
// ⚠ 这一条是对拍面守卫，不是洁癖：`sign` 的声明与 Rust 侧逐字对齐
// （`docs/go-migration-plan.md` 的冻结基准），而"领取"在这份实现里是
// 这条链的固有部分（`SignInRun.Receive` 由调用点写死 `true`），不是参数。
// 什么时候这一栏该出现？先改声明、再对基准、并让前端表单跟着长出来 ——
// 而不是后端偷偷接一个前端给不出的开关。
func TestSignInDoesNotAcceptAReceiveSwitch(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs",
		`{"capability":"sign","params":{"receive":true}}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("签到不该有 receive 这一栏，实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	message, _ := decode(t, rec)["message"].(string)
	if !strings.Contains(message, "receive") {
		t.Errorf("400 该点出是哪一栏不认识，实得 %q", message)
	}
	if !strings.Contains(message, "accountMode") {
		t.Errorf("400 该把能填的键列出来，实得 %q", message)
	}
}
