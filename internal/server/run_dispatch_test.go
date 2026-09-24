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
	// 反方向（"注册表里有、表里没有"）由 [TestEveryRegisteredCapabilityIsWired] 判，
	// 这里不重复一遍：两处各判一次的话，将来放宽时只会改到一处。
}

// TestEveryRegisteredCapabilityIsWired：接上几条 = 注册表几条，差集为空。
//
// 与 [TestRegisteredButUnwiredCapabilityIsNotImplemented] 是一对：那边钉"没接的确实回 501"
// （现在只剩临时摘掉一条这一种验法），这一条钉"注册表里不会多出一行没人接"。
// 多出来那一行的表现不是 501 就是"界面上点得到、后端什么都不做"。
//
// 2026-09-23：`exchange` 接上之后，注册表十四条**全部**有 builder（此前长期是
// "只剩 exchange 没接"，因为它不可逆、要真扣云朵）。2026-09-24 又登记两条
// （`cloud-phone-redpack` 与 `play-ai-special`，都只接了**只读那一半**），2026-09-24 再接
// 一条 `ai-avatar`（写那一半）：十四条 → 十七条。
// ⚠ 这条用例因此从"名单比对"变成"必须为空" —— 将来登记新能力时必须同时接执行，
// 否则这里红；那是**对的**红（`docs/business-feature-inventory.md` §2.2 那条口径）。
func TestEveryRegisteredCapabilityIsWired(t *testing.T) {
	var unwired []string
	for _, capability := range capabilities.All() {
		if _, wired := runBuilders[capability.Code]; !wired {
			unwired = append(unwired, capability.Code)
		}
	}
	slices.Sort(unwired)
	if len(unwired) != 0 {
		t.Fatalf("注册表里有 %d 条还没接上执行：%v", len(unwired), unwired)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 接上了的链：受理 → 终局（签到与刷新凭据这两条最早接上的）
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
// 新接的能力完全可以漏掉这一拦而没人发现 —— 而界面上那个下拉对每条能力都渲染。
//
// 名单**取自分派表**而不是手写：2026-09-23 签到页那一族一次接上五条，
// 手写的清单在那一天就已经少了一半 —— 遍历表才是"每条接上的都测到"。
//
// ⚠ 参数也不能手写一份 code→参数 的映射，理由与上面同族、而且更隐蔽：
// 2026-09-23 接上 `exchange` 当天这一条就红了，红成的不是"并行没拦住"，
// 而是"「奖品」不能为空" —— builder 里**便宜的参数校验排在账号解析之前**
// （那条顺序本身是对的，见 [buildExchangeRun]），于是一条只给
// `accountMode` 的通用请求根本走不到串行闸门。现在按**声明**补齐必填栏。
func TestParallelModeIsRefusedOnEveryWiredCapability(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	var codes []string
	for code := range runBuilders {
		codes = append(codes, code)
	}
	slices.Sort(codes)
	if len(codes) < 2 {
		t.Fatalf("分派表空了？实得 %v", codes)
	}
	for _, code := range codes {
		body := `{"capability":` + quoteJSON(code) + `,"params":` +
			parallelParams(t, code) + `}`
		rec := h.do(t, "POST", "/api/runs", body, nil)
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

// parallelParams 造一份"账号执行方式=并行"的参数体，并按声明补上必填栏。
//
// 只补 `Required && Default == nil` 的栏：有默认值的那一栏后端自己会落到默认
// （[runParams.take]），补了反而把"缺省走默认"这条路径从本用例里挪走。
// 值是合成的（一串 `1`）—— 本用例在起跑前就返回 400，一个上游请求也不会发出去，
// 所以这里不需要"真的存在的那件奖品"。
func parallelParams(t *testing.T, code string) string {
	t.Helper()
	capability, found := capabilities.Get(code)
	if !found {
		t.Fatalf("分派表里的 %q 在注册表里没有：先看 [TestEveryDispatchKeyIsARegisteredCapability]", code)
	}
	items := []string{`"accountMode":"parallel"`}
	for _, field := range capability.Params() {
		if !field.Required || len(field.Default) > 0 {
			continue
		}
		items = append(items, quoteJSON(field.Key)+":"+syntheticValue(field))
	}
	return "{" + strings.Join(items, ",") + "}"
}

// syntheticValue 给一栏造一个"类型对得上"的读数：判的是闸门，不是取值。
func syntheticValue(field core.FieldSpec) string {
	switch field.Kind {
	case core.FieldNumber:
		return "1"
	case core.FieldBool:
		return "true"
	case core.FieldMultiSelect:
		return `["1"]`
	case core.FieldJSON:
		return `{}`
	case core.FieldSelect:
		// 下拉的合法值只有声明里那几个：优先取第一个，别造一个选项里没有的串。
		if len(field.Options) > 0 {
			return string(field.Options[0].Value)
		}
		return `"1"`
	default: // FieldText
		return `"1"`
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
