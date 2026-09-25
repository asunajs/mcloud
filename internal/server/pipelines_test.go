package server

// 任务组合的 HTTP 面（`pipelines.go`）的用例。
//
// 三层各管一件事，别把判据写串：
//   - `pipeline_run_test.go`：清单**逐步跑**得对不对（顺序、只推一条、失败继续…）。
//   - `pipeline_scheduler_test.go`：调度到点**去不去**、那两行账落没落。
//   - 本文件：对外那一层的四件事 ——
//     1. **界面读的那份形状**：外层驼峰、`steps` 内部蛇形（serde 冻结），少一个键
//        或改一个键名，表现是页面上那片空白，而不是报错。
//     2. **写入口径**：哪些清单不许存进库（空清单、没登记的能力、拼错的参数键、
//        未知执行方式、别人的账号、占用保留 id）。
//     3. **归属**：别人的组合一律 404 而不是 403（403 等于确认"这个 id 存在"）。
//     4. **受理那一层**：202 的响应形状、坏清单在受理前就 400、没装执行面时是 500，
//        以及"受理之后不许因为请求结束而腰斩"（见
//        [TestPipelineChainSurvivesTheEndOfItsOwnRequest] 那条）。
//
// ⚠ 上游响应全是合成夹具，号码一律 199 段合成值（见 login_test.go 的夹具说明）。

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// pipelineHTTPHarness 起一台"有一个能跑成的账号"的服务端（HTTP 面用）。
//
// 与 [pipelineHarness] 的区别只有一件事：这里不预置清单，用例各自走
// `PUT /api/pipelines/...` —— 存不存得进去本身就是被测对象。
func pipelineHTTPHarness(t *testing.T, routes ...probeRoute) (*harness, string, *probeScript) {
	t.Helper()
	h := newHarness(t).initialized(t)
	accountID := seedProbeAccount(t, h)
	client := newProbeScript(append(signInSessionRoutes(), routes...)...)
	h.replaceRunWorld(t, client)
	distinctRunIDs(h)
	return h, accountID, client
}

// pipelineList 打 `GET /api/pipelines` 并返回那一排行（含内置那一版）。
func pipelineList(t *testing.T, h *harness) []map[string]any {
	t.Helper()
	rec := h.do(t, "GET", "/api/pipelines", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("清单应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	raw, ok := decode(t, rec)["pipelines"].([]any)
	if !ok {
		t.Fatalf("响应里没有 pipelines 数组：%s", brief(rec.Body.String()))
	}
	rows := make([]map[string]any, 0, len(raw))
	for _, item := range raw {
		row, ok := item.(map[string]any)
		if !ok {
			t.Fatalf("pipelines 里有一项不是对象：%v", item)
		}
		rows = append(rows, row)
	}
	return rows
}

// pipelineRow 取清单里指定 id 那一行（找不到返回 nil）。
func pipelineRow(t *testing.T, h *harness, id string) map[string]any {
	t.Helper()
	for _, row := range pipelineList(t, h) {
		if row["id"] == id {
			return row
		}
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 清单：内置那一版排在最前，且它没有行
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelineListStartsWithTheBuiltInChainThatHasNoRow 空库也要看得见那枚按钮。
//
// 判点是"库里没有行、界面上却必须有那一条"：内置全跑**故意**不落库
// （落库的表现是第一个改它的人替所有人改了默认值）。所以这一行只能由
// [Server.listPipelines] 合成 —— 忘了合成，界面就是一片空白加一个跑不了的按钮。
func TestPipelineListStartsWithTheBuiltInChainThatHasNoRow(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)

	rows := pipelineList(t, h)
	if len(rows) != 1 {
		t.Fatalf("空库里也该看得见内置那一版，实得 %d 行：%v", len(rows), rows)
	}
	builtIn := rows[0]
	if builtIn["id"] != PipelineRunAllID || builtIn["builtIn"] != true {
		t.Fatalf("第一行该是内置全跑（%q / builtIn=true），实得 %v / %v",
			PipelineRunAllID, builtIn["id"], builtIn["builtIn"])
	}
	if steps, ok := builtIn["steps"].([]any); !ok || len(steps) != 14 {
		t.Fatalf("内置那一版该带出 14 步（界面上要能数出来跑哪些），实得 %v", builtIn["steps"])
	}
	// 没有行 ⇒ 记账那几格必须是"没人跑过"，不能是猜出来的默认值。
	for _, key := range []string{"createdAt", "updatedAt", "lastRunAt", "lastStatus",
		"lastError", "lastDurationMs"} {
		if builtIn[key] != nil {
			t.Errorf("内置全跑没有行，%s 该是 null，实得 %v", key, builtIn[key])
		}
	}

	// 用户自建那一条排在**它后面**，且 builtIn=false（界面据此决定能不能删）。
	rec := h.do(t, "PUT", "/api/pipelines/job",
		`{"name":"我的清单","steps":[{"task":"`+capabilities.CodePrizes+`"}]}`, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("保存应成功，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows = pipelineList(t, h)
	if len(rows) != 2 || rows[0]["id"] != PipelineRunAllID || rows[1]["id"] != "job" {
		t.Fatalf("顺序应是「内置、自建」，实得 %v", rows)
	}
	if rows[1]["builtIn"] != false {
		t.Errorf("自建那一行 builtIn 该是 false（删掉它得有个按钮），实得 %v", rows[1]["builtIn"])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 形状：界面读的那一份
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelinePutRoundTripsEveryFieldTheWebReads 存进去的每一项都要读得回来。
//
// ⚠ 外层键是**驼峰**（本仓 HTTP 面的惯例，与 [schedulePayload] 同一份），而
// `steps` 里面是**蛇形**（`task` / `body` / `continue_on_error`，serde 冻结形状）。
// 这个不一致是**存量数据的读法**决定的，刻意保留（理由见 `pipelinePayload` 抬头）。
// 所以这一条同时钉住两件事：写入口认蛇形的 `account_ids`，读出口给驼峰的 `accountIds`。
func TestPipelinePutRoundTripsEveryFieldTheWebReads(t *testing.T) {
	h, accountID, _ := pipelineHTTPHarness(t)

	body := `{"name":"每日一把","description":"签到+领奖",` +
		`"steps":[{"task":"` + capabilities.CodeLiveRoom + `","body":{"skipKouling":true},"continue_on_error":true},` +
		`{"task":"` + capabilities.CodeCloudSnapshot + `"}],` +
		`"account_ids":[` + quoteJSON(accountID) + `],"account_mode":"sequential","stop_on_error":true}`
	rec := h.do(t, "PUT", "/api/pipelines/daily", body, nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("保存应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	row, _ := decode(t, rec)["pipeline"].(map[string]any)
	if row == nil {
		t.Fatalf("响应里没有 pipeline：%s", brief(rec.Body.String()))
	}

	if row["id"] != "daily" || row["name"] != "每日一把" || row["description"] != "签到+领奖" {
		t.Errorf("id / name / description 没原样回来：%v / %v / %v",
			row["id"], row["name"], row["description"])
	}
	if row["stopOnError"] != true || row["accountMode"] != "sequential" || row["builtIn"] != false {
		t.Errorf("组合级三栏不对：stopOnError=%v accountMode=%v builtIn=%v",
			row["stopOnError"], row["accountMode"], row["builtIn"])
	}
	ids, _ := row["accountIds"].([]any)
	if len(ids) != 1 || ids[0] != accountID {
		t.Errorf("accountIds 该带出那一个账号，实得 %v", row["accountIds"])
	}
	steps, _ := row["steps"].([]any)
	if len(steps) != 2 {
		t.Fatalf("该有 2 步，实得 %v", row["steps"])
	}
	first, _ := steps[0].(map[string]any)
	// 步骤里只有这三个键：多一个（比如把内部的 `Body` 以驼峰漏出来）就是改了线格式。
	if len(first) != 3 || first["task"] != capabilities.CodeLiveRoom ||
		first["continue_on_error"] != true {
		t.Errorf("第一步的形状不对：%v", first)
	}
	// 参数对象逐键原样回来（这里挑一个**不是** `accounts` 的键：那两个键会被
	// 组合级账号覆盖，看不出"存进去的东西有没有被改写"）。
	if skipped, _ := first["body"].(map[string]any)["skipKouling"].(bool); !skipped {
		t.Errorf("第一步的 body 参数没原样回来：%v", first["body"])
	}
	second, _ := steps[1].(map[string]any)
	// 没给 body ⇒ 存成 `{}` 而**不是** null：模型层对 null 的往返有 `== nil` 反转的坑
	// （[core.PipelineStep.UnmarshalJSON] 第 3 条），空对象没有两面性。
	if object, ok := second["body"].(map[string]any); !ok || len(object) != 0 {
		t.Errorf("第二步的 body 该是空对象，实得 %v", second["body"])
	}
	if second["continue_on_error"] != nil {
		t.Errorf("没填三态那一栏就该是 null（跟随组合级），实得 %v", second["continue_on_error"])
	}
	// 新行没跑过：四列记账是 null，不是 0 / "ok"。
	if row["lastRunAt"] != nil || row["lastStatus"] != nil || row["lastError"] != nil ||
		row["lastDurationMs"] != nil {
		t.Errorf("新建的组合不该带执行记账：%v / %v / %v / %v",
			row["lastRunAt"], row["lastStatus"], row["lastError"], row["lastDurationMs"])
	}
	created, _ := row["createdAt"].(string)
	if _, err := time.Parse(time.RFC3339, created); err != nil {
		t.Errorf("createdAt 该是 RFC3339 文本，实得 %q（%v）", created, err)
	}

	// 读回来必须是同一份（存库→出库这一圈不改形状）。
	back := pipelineRow(t, h, "daily")
	if back == nil {
		t.Fatalf("PUT 之后清单里读不到这一行")
	}
	for _, key := range []string{"name", "description", "stopOnError", "accountMode"} {
		if asText(t, back[key]) != asText(t, row[key]) {
			t.Errorf("往返之后 %s 变了：写入 %v，读回 %v", key, row[key], back[key])
		}
	}
	backSteps, _ := back["steps"].([]any)
	if len(backSteps) != 2 {
		t.Fatalf("往返之后步数变了：%v", back["steps"])
	}

	// ⚠ 这一格钉的是**不对称本身**：请求体只认蛇形 `account_ids`，驼峰那一份会被
	// 静默丢掉。它是既有惯例（`scheduleBody` 同一形状），钉住不是为了说它好，
	// 是因为"哪天有人把一侧改成驼峰"会让另一侧**静默失效** —— 表现是界面选了账号、
	// 实际跑全部，而这种差异不会有任何地方报错。
	camel := h.do(t, "PUT", "/api/pipelines/camel",
		`{"name":"驼峰试探","steps":[{"task":"`+capabilities.CodePrizes+`"}],`+
			`"accountIds":[`+quoteJSON(accountID)+`]}`, nil)
	if camel.Code != http.StatusOK {
		t.Fatalf("保存应 200，实得 %d：%s", camel.Code, brief(camel.Body.String()))
	}
	if ids, _ := decode(t, camel)["pipeline"].(map[string]any)["accountIds"].([]any); len(ids) != 0 {
		t.Errorf("请求体的驼峰 accountIds 竟被吃下了（实得 %v）—— 形状已改，本文件上面那条判据要一起重定", ids)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 写入口径：什么存得进去
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelinePutRefusesToShadowTheBuiltInChain 保留 id 不许建同名行。
func TestPipelinePutRefusesToShadowTheBuiltInChain(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)
	rec := h.do(t, "PUT", "/api/pipelines/"+PipelineRunAllID,
		`{"name":"我的全跑","steps":[{"task":"`+capabilities.CodePrizes+`"}]}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("占用中的 id 该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "内置") {
		t.Errorf("那句错该说清是内置占用的 id（否则用户只会看到'存不进去'），实得 %q", message)
	}
	// 挡下之后库里仍然不能有这一行（否则"400 但也存了"会让那枚按钮静默换清单）。
	if row := pipelineRow(t, h, PipelineRunAllID); row == nil || row["builtIn"] != true {
		t.Errorf("内置那一版不该被顶掉，实得 %v", row)
	}
	if getPipelineNow(t, h, PipelineRunAllID) != nil {
		t.Errorf("被拒的请求不许在库里留下行")
	}
}

// TestPipelinePutValidationTable 八种"这份清单不该存进去"各自指名。
//
// 每一格都配"库里没有行"：PUT 是全量覆盖，**校验没过却写了一半**是最难查的形态
// （界面上看得到那一条，点执行永远 400）。
func TestPipelinePutValidationTable(t *testing.T) {
	const prizes = `{"task":"` + capabilities.CodePrizes + `"}`
	cases := []struct {
		name    string
		id      string
		body    string
		want    string
		preset  func(t *testing.T, h *harness) // 需要账号/他人身份那一格用
		wantErr string                         // 错误码（bad_request / 契约不合）
	}{
		{name: "一步都没有", id: "job", body: `{"name":"空的","steps":[]}`, want: "至少要有一步"},
		{name: "步骤没有任务名", id: "job", body: `{"name":"缺名","steps":[{"body":{}}]}`,
			want: "第 1 步没有任务名"},
		{name: "能力表里查不到", id: "job",
			body: `{"name":"坏能力","steps":[` + prizes + `,{"task":"no-such-task"}]}`,
			want: "第 2 步没有这个能力：no-such-task"},
		{name: "参数键声明里没有", id: "job",
			body: `{"name":"拼错键","steps":[{"task":"` + capabilities.CodeSign + `","body":{"receive":true}}]}`,
			want: "第 1 步"},
		{name: "参数不是对象", id: "job",
			body: `{"name":"参数是数组","steps":[{"task":"` + capabilities.CodePrizes + `","body":[1,2]}]}`,
			want: "不合契约"},
		{name: "执行方式写了个没见过的", id: "job",
			body: `{"name":"乱填","steps":[` + prizes + `],"account_mode":"fast"}`,
			want: "执行方式只认"},
		{name: "组合名是空白", id: "job", body: `{"name":"   ","steps":[` + prizes + `]}`,
			want: "组合名不能为空"},
		{name: "组合名超长", id: "job",
			body: `{"name":"` + strings.Repeat("字", maxNameLen+1) + `","steps":[` + prizes + `]}`,
			want: "组合名最多"},
		{name: "id 里有斜杠", id: "bad%20id", body: `{"name":"坏 id","steps":[` + prizes + `]}`,
			want: "任务 id 只能用"},
		{name: "账号不存在", id: "job",
			body: `{"name":"别人的账号","steps":[` + prizes +
				`],"account_ids":["19900000000"]}`, want: "云盘账号不存在或不属于你"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			h, _, _ := pipelineHTTPHarness(t)
			rec := h.do(t, "PUT", "/api/pipelines/"+tc.id, tc.body, nil)
			if rec.Code != http.StatusBadRequest {
				t.Fatalf("该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
			}
			if got := decode(t, rec)["code"]; got != "bad_request" {
				t.Errorf("code 该是 bad_request（是使用者的清单有问题），实得 %v", got)
			}
			if message := errorMessage(t, rec); !strings.Contains(message, tc.want) {
				t.Errorf("那句话该含 %q（指到哪一栏才有处置），实得 %q", tc.want, message)
			}
			if row := getPipelineNow(t, h, tc.id); row != nil {
				t.Errorf("校验没过却存进了库：%+v", row)
			}
		})
	}
}

// TestPipelinePutKeepsTheColumnsTheExecutionOwns 改一次定义不许洗掉执行那几列。
//
// 与 [TestScheduleEditKeepsWhatOnlyTheSchedulerOwns] 同一条判据，另一份实现：
// `PUT` 是全量覆盖，但请求体里没有 `created_at` 与那四列记账 —— 拿整行覆盖会把
// "上次跑成什么样"抹掉，而那正是用户**改完清单之后最想看**的一格。
func TestPipelinePutKeepsTheColumnsTheExecutionOwns(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))

	put := func(name string) {
		t.Helper()
		rec := h.do(t, "PUT", "/api/pipelines/job",
			`{"name":`+quoteJSON(name)+`,"steps":[{"task":"`+capabilities.CodePrizes+`"}]}`, nil)
		if rec.Code != http.StatusOK {
			t.Fatalf("保存应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
		}
	}
	put("原名字")
	first := pipelineRow(t, h, "job")
	created, _ := first["createdAt"].(string)

	ranAt := fixedNow.Add(-time.Hour)
	duration := int64(4321)
	note := "第 2 步失败：上游 502"
	written, err := h.store.RecordPipelineRun(ctx, scope, "job",
		&ranAt, core.PipelineError, &note, &duration)
	if err != nil || !written {
		t.Fatalf("预置记账失败: written=%v err=%v", written, err)
	}

	put("改了名")
	got := pipelineRow(t, h, "job")
	if got["name"] != "改了名" {
		t.Fatalf("改名没生效，实得 %v", got["name"])
	}
	if got["createdAt"] != created {
		t.Errorf("一次编辑不该改 created_at（界面上那是'建的时刻'），实得 %v，应为 %v",
			got["createdAt"], created)
	}
	if got["lastRunAt"] != ranAt.UTC().Format(time.RFC3339) {
		t.Errorf("一次编辑不该洗掉上次执行时刻，实得 %v", got["lastRunAt"])
	}
	if got["lastStatus"] != "error" || asText(t, got["lastError"]) != note {
		t.Errorf("一次编辑不该洗掉执行结论，实得 %v / %v", got["lastStatus"], got["lastError"])
	}
	// ⚠ 数字比的是**文本**：本包的 JSON 解码留着精确数值（`json.Number`），
	// 拿 `float64(4321)` 去比会永远不等（那不是形状问题，是夹具的读法）。
	if asText(t, got["lastDurationMs"]) != "4321" {
		t.Errorf("一次编辑不该洗掉耗时，实得 %v", got["lastDurationMs"])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 归属：读、写、删、跑，四条路都只认自己的
// ─────────────────────────────────────────────────────────────────────────────

// TestPipelinesAreScopedToTheirOwner 别人的组合在四条路上都只是"不存在"。
//
// 404 而不是 403：报 403 等于替人确认"这个 id 确实有一行"，那是白送一张清单。
// 与 [TestEventsOfSomeoneElsesRunAreNotFoundNotForbidden] 同一口径。
func TestPipelinesAreScopedToTheirOwner(t *testing.T) {
	h, accountID, _ := pipelineHTTPHarness(t)
	adminCookie := h.cookie
	adminScope := core.ScopeUser(userIDOf(t, h))
	ownedByAdmin := func(id string) *core.Pipeline {
		t.Helper()
		row, err := h.store.GetPipeline(context.Background(), adminScope, id)
		if err != nil {
			t.Fatalf("读管理员那一行失败: %v", err)
		}
		return row
	}
	if rec := h.do(t, "PUT", "/api/pipelines/job",
		`{"name":"我的清单","steps":[{"task":"`+capabilities.CodePrizes+`"}]}`, nil); rec.Code != http.StatusOK {
		t.Fatalf("管理员建组合应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	h.cookie = secondUser(t, h, "bob")

	// 读：别人的那一行不在他的清单里（只有内置那一版，它本来就不属于谁）。
	rows := pipelineList(t, h)
	if len(rows) != 1 || rows[0]["id"] != PipelineRunAllID {
		t.Errorf("bob 的清单该只剩内置那一版，实得 %v", rows)
	}

	// 跑：别人的组合 ⇒ 404，且**没有**替谁跑起来。
	rec := h.do(t, "POST", "/api/pipelines/job/run", "", nil)
	if rec.Code != http.StatusNotFound {
		t.Errorf("跑别人的组合该 404（403 会确认 id 存在），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	if ownedByAdmin("job") == nil {
		t.Fatalf("前提：管理员那一行没建上（夹具坏了）")
	}

	// 删：同样 404，且那一行没被删掉（`LastStatus` 仍为空也说明"跑"那一步没替他跑）。
	if rec := h.do(t, "DELETE", "/api/pipelines/job", "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("删别人的组合该 404，实得 %d", rec.Code)
	}
	if still := ownedByAdmin("job"); still == nil || still.LastStatus != nil {
		t.Errorf("越权的读/跑/删三条路都不许动到管理员那一行，实得 %+v", still)
	}

	// 同一个 id **全局唯一**：bob 拿管理员用过的 id 建自己那份会被挡下。
	// 这是存储层那一道 `ensureNotOwnedByOther`（与定时任务同一实现，见
	// `internal/io/configstore.go`）：`pipelines.id` 是整表主键，不是 (owner,id) 复合键。
	// 这里把它钉住，是因为界面上"各建各的组合"很容易被读成"各有一套 id"。
	rec = h.do(t, "PUT", "/api/pipelines/job",
		`{"name":"bob 的清单","steps":[{"task":"`+capabilities.CodeCloudSnapshot+`"}]}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("顶别人的 id 建同名组合该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "已被其他用户创建") {
		t.Errorf("那句错该说清是 id 撞了别人，实得 %q", message)
	}
	if ownedByAdmin("job") == nil || ownedByAdmin("job").Name != "我的清单" {
		t.Errorf("被挡下的写入不许动到管理员那一行，实得 %+v", ownedByAdmin("job"))
	}
	// 换个 id 就是各存各的：bob 那一行不出现在管理员的清单里，反之亦然。
	if rec := h.do(t, "PUT", "/api/pipelines/bob-job",
		`{"name":"bob 的清单","steps":[{"task":"`+capabilities.CodeCloudSnapshot+`"}]}`, nil); rec.Code != http.StatusOK {
		t.Fatalf("bob 建自己的组合应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if row := pipelineRow(t, h, "bob-job"); row == nil || row["name"] != "bob 的清单" {
		t.Errorf("bob 该看得见自己那一行，实得 %v", row)
	}
	// 顺带钉住"账号归属"这一格：**bob 名下**没有管理员那个账号，
	// 拿它的 id 填进 `account_ids` 存不进去（否则 A 的组合会去打 B 的账号）。
	rec = h.do(t, "PUT", "/api/pipelines/steal",
		`{"name":"别人的账号","steps":[{"task":"`+capabilities.CodePrizes+`"}],`+
			`"account_ids":[`+quoteJSON(accountID)+`]}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("拿别人的账号建组合该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "云盘账号不存在或不属于你") {
		t.Errorf("那句错该指名是账号归属问题，实得 %q", message)
	}
	if getPipelineNow(t, h, "steal") != nil {
		t.Errorf("被拒的请求不许留下行")
	}

	// 回到管理员这一侧：他看不见 bob 那一行，自己的仍原样在。
	h.cookie = adminCookie
	if row := pipelineRow(t, h, "bob-job"); row != nil {
		t.Errorf("bob 的组合不该出现在管理员清单里：%v", row)
	}
	if ownedByAdmin("bob-job") != nil {
		t.Errorf("bob 那一行不该被插进管理员名下")
	}
	if row := pipelineRow(t, h, "job"); row == nil || row["name"] != "我的清单" {
		t.Errorf("管理员看到的仍该是自己那一行，实得 %v", row)
	}
}

// TestDeletePipelineRefusesWhileASchedulePointsAtIt 被引用时拒绝删，且指名是哪几条。
//
// 不级联删定时任务：那会静默带走用户另一处配好的"每天 08:10 跑它"。
// 顺带钉住另外三条：内置那个 id 不需要删、没删到东西是 404、删完清单少一行。
func TestDeletePipelineRefusesWhileASchedulePointsAtIt(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)
	if rec := h.do(t, "PUT", "/api/pipelines/job",
		`{"name":"每日一把","steps":[{"task":"`+capabilities.CodePrizes+`"}]}`, nil); rec.Code != http.StatusOK {
		t.Fatalf("建组合应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if rec := h.do(t, "PUT", "/api/schedules/morning",
		`{"name":"早晨那一趟","cron":"0 8 * * *","target":{"kind":"pipeline","id":"job"}}`, nil); rec.Code != http.StatusOK &&
		rec.Code != http.StatusCreated {
		t.Fatalf("建定时任务应成功，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	rec := h.do(t, "DELETE", "/api/pipelines/job", "", nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("被引用时该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	message := errorMessage(t, rec)
	if !strings.Contains(message, "早晨那一趟") {
		t.Errorf("那句话该指名是哪条定时任务在用（否则用户得自己翻），实得 %q", message)
	}
	if getPipelineNow(t, h, "job") == nil {
		t.Fatalf("被拒的删除不该真删掉")
	}

	if rec := h.do(t, "DELETE", "/api/schedules/morning", "", nil); rec.Code != http.StatusNoContent {
		t.Fatalf("删定时任务应 204，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if rec := h.do(t, "DELETE", "/api/pipelines/job", "", nil); rec.Code != http.StatusNoContent {
		t.Fatalf("没人引用之后该删掉（204），实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if getPipelineNow(t, h, "job") != nil {
		t.Errorf("删过之后库里不该还有这一行")
	}
	// 再删一次：不存在 ⇒ 404（幂等删除的第二次不是"成功"，那会掩盖打错的 id）。
	if rec := h.do(t, "DELETE", "/api/pipelines/job", "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("删已经不存在的组合该 404，实得 %d", rec.Code)
	}
	// 内置那一版没有行，删它是一句"不需要删"，不是 404（404 会让人以为按钮坏了）。
	if rec := h.do(t, "DELETE", "/api/pipelines/"+PipelineRunAllID, "", nil); rec.Code != http.StatusBadRequest {
		t.Errorf("删内置全跑该 400（那枚按钮没有行），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
}

// TestScheduleMayTargetTheBuiltInPipelineButNotAmissingOne 调度目标的"存在"判据。
//
// 内置那一版在 `pipelines` 表里**没有行**（`allRunPipeline` 每次现造），所以按查库判存在
// 会得到 400「没有这个组合: all」—— 表现成"界面上那枚按钮能跑的东西，定时却选不了"。
// 到点执行那一路本来就认这个保留 id（`pipelineToRun` 的第一行），受理面放行只是与它对齐。
//
// ⚠ 反向那一半同样要钉：放行**只**给这一枚保留 id，别的错 id 仍旧在入库前拒掉。
// 否则"随便写一个不存在的组合"会存成一条每天到点报 error 的死任务。
func TestScheduleMayTargetTheBuiltInPipelineButNotAmissingOne(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)

	rec := h.do(t, "PUT", "/api/schedules/everyday",
		`{"name":"每天一把","cron":"0 8 * * *","target":{"kind":"pipeline","id":"`+PipelineRunAllID+`"}}`, nil)
	if rec.Code != http.StatusOK && rec.Code != http.StatusCreated {
		t.Fatalf("调度该选得了内置那一版，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	rec = h.do(t, "PUT", "/api/schedules/ghost-pipeline",
		`{"name":"不存在的那份","cron":"0 8 * * *","target":{"kind":"pipeline","id":"no-such"}}`, nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("打错的组合 id 该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "no-such") {
		t.Errorf("那句错该指名是哪个 id（否则人去翻库），实得 %q", message)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 那枚按钮：受理、拒绝、以及"受理之后不许腰斩"
// ─────────────────────────────────────────────────────────────────────────────

// TestRunPipelineRouteAcceptsAndRunsTheChain 202 的响应形状，且那一趟真的跑了。
//
// 响应里**没有** runId 配对，是刻意的（判据见 [Server.runPipelineRoute]）：这条钉的是
// "至少要说清受理了哪一份、共几步"，界面上那句"开始跑了"就靠它。
func TestRunPipelineRouteAcceptsAndRunsTheChain(t *testing.T) {
	h, accountID, client := pipelineHTTPHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}},
		probeRoute{match: pipelineSnapshotRoute, bodies: []string{signInStatusClaimable}},
	)
	if rec := h.do(t, "PUT", "/api/pipelines/job",
		`{"name":"每日一把","steps":[{"task":"`+capabilities.CodePrizes+`"},`+
			`{"task":"`+capabilities.CodeCloudSnapshot+`"}]}`, nil); rec.Code != http.StatusOK {
		t.Fatalf("建组合应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	rec := h.do(t, "POST", "/api/pipelines/job/run", "", nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("受理该 202，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	body := decode(t, rec)
	if body["pipelineId"] != "job" || body["name"] != "每日一把" {
		t.Errorf("响应该说出受理了哪一份，实得 %v", body)
	}
	if asText(t, body["steps"]) != "2" {
		t.Errorf("steps 该是清单步数 2，实得 %v", body["steps"])
	}
	if _, paired := body["runs"]; paired {
		t.Errorf("组合**不该**回 runId 配对：后面的 runId 要等前面的跑完才分配得出来，"+
			"给了就是假配对（判据见 runPipelineRoute 抬头）：%v", body["runs"])
	}

	// 202 之后确实跑了那两步（记账落库 = 整条链已到终局）。
	pipeline := waitForPipelineRecorded(t, h, "job")
	if pipeline.LastStatus == nil || *pipeline.LastStatus != core.PipelineOK {
		t.Errorf("两步都该跑成，实得 %v（%v）", pipeline.LastStatus, pipeline.LastError)
	}
	for _, route := range []string{pipelinePrizesRoute, pipelineSnapshotRoute} {
		if got := client.count(route); got != 1 {
			t.Errorf("%s 该被打 1 次，实得 %d（全部 URL：%v）", route, got, urlsOf(client))
		}
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 2 {
		t.Errorf("两步该留 2 行运行记录，实得 %d 行：%v", len(ids), ids)
	}
}

// TestRunPipelineRouteRejectsAChainThatCannotStart 坏清单在受理之前就 400。
//
// 判点是"没有半条链在后台跑"：预检一旦挪到起跑之后，响应里那句 400 照样回得出来，
// 而库里会留下一行"受理了但一步没跑"的记录 —— 界面上那一条与"还在跑"长得一模一样。
// 这里用**直接落库**造坏数据（`PUT` 自己的预检已经挡掉这一种），因为存量导入的
// `data/pipelines.json` 里完全可能有这种行：能力后来被摘表。
func TestRunPipelineRouteRejectsAChainThatCannotStart(t *testing.T) {
	h, accountID, client := pipelineHTTPHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}})
	seedStoredPipeline(t, h, "job")

	builder, wired := runBuilders[capabilities.CodePrizes]
	if !wired {
		t.Fatalf("前提是 %q 已接上执行（否则不需要摘表）", capabilities.CodePrizes)
	}
	delete(runBuilders, capabilities.CodePrizes)
	defer func() { runBuilders[capabilities.CodePrizes] = builder }()

	rec := h.do(t, "POST", "/api/pipelines/job/run", "", nil)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("清单跑不了该 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "还没有接上执行") {
		t.Errorf("那句话该说清是'这一步没接线'，实得 %q", message)
	}
	if got := client.sent(); got != 0 {
		t.Errorf("预检没过就该一枪不发，实得 %d 次（URL：%v）", got, urlsOf(client))
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 0 {
		t.Errorf("被拒的这一次不该留下运行记录：%v", ids)
	}
	if pipeline := getPipelineNow(t, h, "job"); pipeline.LastStatus != nil {
		t.Errorf("被拒的这一次不该给组合记执行账（界面上会是'刚跑过一次'）：%v %v",
			pipeline.LastStatus, pipeline.LastError)
	}
}

// TestRunPipelineRouteWithoutTheExecutionFaceIsAnInternalError 没装执行面 ⇒ 500。
//
// 这一条钉的是"不许静默"：`s.runs` 为 nil 时如果照回 202，界面上会得到一条
// 永远不动的运行记录，而真正的原因（这个服务端没装配执行面）在装配那一刻就该响。
func TestRunPipelineRouteWithoutTheExecutionFaceIsAnInternalError(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)
	seedStoredPipeline(t, h, "job")
	world := h.server.runs
	h.server.runs = nil

	rec := h.do(t, "POST", "/api/pipelines/job/run", "", nil)
	h.server.runs = world
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("没装执行面该 500，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	if got := decode(t, rec)["code"]; got != "internal" {
		t.Errorf("code 该是 internal（这是装配问题，不是用户的清单问题），实得 %v", got)
	}
	if pipeline := getPipelineNow(t, h, "job"); pipeline.LastStatus != nil {
		t.Errorf("没起跑就不该记账，实得 %v", pipeline.LastStatus)
	}
}

// TestBuiltInChainNeedsNoRowAndPassesItsOwnPrecheck 那枚按钮指的东西**现在**就能受理。
//
// 全部走同步函数，不打 `POST /api/pipelines/all/run`：内置那一版**故意不落账**
// （见 [Server.recordPipelineRun]），所以它在 HTTP 面上没有可等的终局 —— 拿它跑一次
// 就是在后台留 14 步在打的链，用例结束时库已经被 `t.Cleanup` 关掉。
// "受理之后确实会跑"这一格由 [TestRunPipelineRouteAcceptsAndRunsTheChain] 与
// [TestPipelineChainSurvivesTheEndOfItsOwnRequest] 用自建清单钉。
func TestBuiltInChainNeedsNoRowAndPassesItsOwnPrecheck(t *testing.T) {
	h, _, _ := pipelineHTTPHarness(t)
	ctx := context.Background()
	scope := core.ScopeUser(userIDOf(t, h))

	if getPipelineNow(t, h, PipelineRunAllID) != nil {
		t.Fatalf("前提：库里不该有 id 为 %q 的行（有的话内置那一版就白设计了）", PipelineRunAllID)
	}
	pipeline, perr := h.server.pipelineToRun(ctx, scope, PipelineRunAllID)
	if perr != nil {
		t.Fatalf("内置那一版该**不查库**就能解析出来：%s", perr.Message())
	}
	if pipeline.ID != PipelineRunAllID || len(pipeline.Steps) != 14 {
		t.Errorf("解析出来的不是内置那份：%s / %d 步", pipeline.ID, len(pipeline.Steps))
	}
	requests, perr := pipelineRequests(pipeline)
	if perr != nil {
		t.Fatalf("内置清单过不了自己的预检（那枚按钮一点就整体 400）：%s", perr.Message())
	}
	if len(requests) != len(pipeline.Steps) {
		t.Errorf("预检结果与步数对不上：%d / %d", len(requests), len(pipeline.Steps))
	}

	// 对照组：其他 id 都要查库，查不到就是 404。
	if _, perr := h.server.pipelineToRun(ctx, scope, "ghost"); perr == nil ||
		perr.Kind() != core.KindNotFound {
		t.Errorf("不存在的组合该报 not_found，实得 %v", perr)
	}
}

// TestPipelineChainSurvivesTheEndOfItsOwnRequest 202 之后不许因为请求结束而腰斩。
//
// ⭐ 这一条只有打**真服务器**才测得出来，所以它不走 `h.do`：
// `httptest.ResponseRecorder` 那一路请求上下文**永远不被取消**，而 net/http 的
// `Request.Context` 文档写明"or when the ServeHTTP method returns"—— 组合一跑就是几分钟，
// 拿请求上下文跑它 = 每次点按钮都会在响应走后的那一刻被取消。
// 在 recorder 上这表现为"全绿"，在真服务器上表现为"只跑了第一步"。
func TestPipelineChainSurvivesTheEndOfItsOwnRequest(t *testing.T) {
	h, accountID, client := pipelineHTTPHarness(t,
		probeRoute{match: pipelinePrizesRoute, bodies: []string{prizeRecordBody}},
		probeRoute{match: pipelineSnapshotRoute, bodies: []string{signInStatusClaimable}},
	)
	if rec := h.do(t, "PUT", "/api/pipelines/job",
		`{"name":"每日一把","steps":[{"task":"`+capabilities.CodePrizes+`"},`+
			`{"task":"`+capabilities.CodeCloudSnapshot+`"}]}`, nil); rec.Code != http.StatusOK {
		t.Fatalf("建组合应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}

	ts := httptest.NewServer(h.server.Handler())
	t.Cleanup(ts.Close)
	req, err := http.NewRequest("POST", ts.URL+"/api/pipelines/job/run", nil)
	if err != nil {
		t.Fatal(err)
	}
	req.AddCookie(h.cookie)
	resp, err := ts.Client().Do(req)
	if err != nil {
		t.Fatalf("请求失败: %v", err)
	}
	defer func() { _ = resp.Body.Close() }()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusAccepted {
		t.Fatalf("受理该 202，实得 %d：%s", resp.StatusCode, brief(string(raw)))
	}

	// 响应已经走了 ⇒ 请求上下文此刻已被取消，链必须照跑不误。
	pipeline := waitForPipelineRecorded(t, h, "job")
	if pipeline.LastStatus == nil || *pipeline.LastStatus != core.PipelineOK {
		t.Errorf("202 之后整条链该跑完（两步都成），实得 %v：%v",
			pipeline.LastStatus, pipeline.LastError)
	}
	for _, route := range []string{pipelinePrizesRoute, pipelineSnapshotRoute} {
		if got := client.count(route); got != 1 {
			t.Errorf("请求结束之后 %s 仍该被打到（被打 %d 次说明链被腰斩了），URL：%v",
				route, got, urlsOf(client))
		}
	}
	if ids := runIDsForAccount(t, h, accountID); len(ids) != 2 {
		t.Errorf("两步该留 2 行运行记录，实得 %d 行：%v", len(ids), ids)
	}
}

// TestWaitingStepGivesUpWhenTheServerIsClosing 上下文真被取消时那一步不干等。
//
// 它是 [Server.launchRunWaiting] 里 `ctx.Done()` 那一档的**唯一**判据 —— 上面那条
// 把"请求结束"脱开之后，日常路径已经没有可取消的上下文了，剩下的是**进程收尾**
// （Ctrl-C 之后的关服）。那一档不许变成没人测的兜底：它的判据是
// "不等、但也不猜结果"（那一趟会在后台跑完，见 `launchRun` 的 `WithoutCancel`）。
//
// 夹具是 [blockingClient]：第一枪卡住 ⇒ 这一步永远不会自己结束，取消该是**唯一**
// 能让它结束的东西。放开之后等它走完，免得用例收了后台还在写已关闭的库。
func TestWaitingStepGivesUpWhenTheServerIsClosing(t *testing.T) {
	h := newHarness(t).initialized(t)
	accountID := seedProbeAccount(t, h)
	blocker := newBlockingClient()
	h.replaceRunWorld(t, blocker)
	distinctRunIDs(h)

	pipeline := pPipeline(pStep(capabilities.CodePrizes, ""))
	pipeline.Owner = userIDOf(t, h)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go func() {
		<-blocker.entered // 第一枪已经发出去 ⇒ 这一步确实卡在等结果
		cancel()
	}()

	result, perr := h.server.runPipeline(ctx, core.ScopeUser(userIDOf(t, h)), pipeline)
	if perr != nil {
		t.Fatalf("跑起来之后的失败不进返回值：%s", perr.Message())
	}
	if len(result.Steps) != 1 {
		t.Fatalf("该有一条结论，实得 %+v", result.Steps)
	}
	if result.Steps[0].Status != core.PipelineError {
		t.Errorf("没等完的那一步该记 error，实得 %s", result.Steps[0].Status)
	}
	if !strings.Contains(result.Note, "没等完") {
		t.Errorf("那句话该说清是'没等完'而不是'跑失败了'（后者会让人以为清单坏了），实得 %q",
			result.Note)
	}
	if result.Steps[0].Ran != 0 {
		t.Errorf("没等完时不许猜结果（那一趟可能仍在后台跑），实得 ran=%d", result.Steps[0].Ran)
	}

	// 收尾：放开阻塞，等那一趟真的结束（它跑的是脱离请求的上下文，会跑到终局）。
	close(blocker.release)
	deadline := time.Now().Add(10 * time.Second)
	for h.server.runs.registry.Running() > 0 {
		if time.Now().After(deadline) {
			t.Fatalf("等不到那一趟结束（账号 %s）", accountID)
		}
		time.Sleep(2 * time.Millisecond)
	}
}
