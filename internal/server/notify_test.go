package server

// 试发接口（`POST /api/notify/test`）的用例。
//
// ⚠⚠ 这一组**每一条**都必须显式注入假 HTTP 客户端。理由与别的接口不同：
// 漏注入在别处表现为"用例变红"，在这里表现为**用户手机上多一条通知**
// （[Deps.notify] 留 nil 时是 500，但一旦有人为了"方便"把默认装配改成
// 现拼真客户端，用例就会全部变绿而消息真发出去）。所以：
//   - [newHarness] 默认**不**装发送器，本文件的 [withNotifySender] 是唯一入口；
//   - 断言里反复用 `client.requests` 的**条数**，因为"多打一家"与"少打一家"
//     在这条接口上是同一类失效（用户收到的是第三方那一条消息，不是错误码）。
//
// 配置的读写（脱敏视图、逐字段合并、未知键 400）在 `notify_config_test.go`。

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

// notifyClient 记录每一个出网请求，并按可配的函数给响应。
//
// 默认响应是 `{"code":200}`：**必须**是个 JSON 对象而不是空体 ——
// 判据把"响应不是 JSON"算失败，用空体会让所有"应当 sent"的用例都因为
// 一个与本接口无关的原因变红。
type notifyClient struct {
	mu       sync.Mutex
	requests []core.HTTPRequest
	reply    func(core.HTTPRequest) (core.HTTPResponse, error)
}

var _ core.HTTPClient = (*notifyClient)(nil)

func (c *notifyClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.mu.Lock()
	c.requests = append(c.requests, request)
	reply := c.reply
	c.mu.Unlock()
	if reply != nil {
		return reply(request)
	}
	return core.HTTPResponse{
		Status: http.StatusOK,
		Body:   core.Body(`{"code":200,"msg":"ok"}`),
	}, nil
}

func (c *notifyClient) urls() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]string, 0, len(c.requests))
	for _, request := range c.requests {
		out = append(out, request.URL)
	}
	return out
}

// hostHits 把"各家各被打了几次"折成一张表（键用基址域名，见 [notifyHostOfChannel]）。
func (c *notifyClient) hostHits() map[string]int {
	hits := map[string]int{}
	for _, url := range c.urls() {
		for name, host := range notifyHostOfChannel {
			if strings.Contains(url, host) {
				hits[name]++
			}
		}
	}
	return hits
}

// notifyHostOfChannel 用基址域名认"这一枪打的是哪一家"。
//
// 为什么不看请求体里的字段：URL 由**上一轮**（`internal/upstream`）那批用例逐字钉住，
// 这一族要断言的是"这一个接口有没有多打一家"，用域名认最省事且不重复钉 URL。
var notifyHostOfChannel = map[string]string{
	"pushplus":   "pushplus.plus",
	"serverChan": "sctapi.ftqq.com",
	"workWeixin": "qyapi.weixin.qq.com",
	"tgBot":      "api.telegram.org",
	"bark":       "api.day.app",
	"twoIm":      "api.2im.cn",
	"dingTalk":   "oapi.dingtalk.com",
	"customPost": "example.test",
}

// withNotifySender 把推送装配换成"带这台客户端"的那一份。
//
// `mail` 传 nil 是刻意的：email 那一家的缺法本身有用例（见
// [TestNotifyTestSkipsEmailWhenNoSMTPSenderIsWired]），而"漏接 SMTP"在测试里的
// 失效形态是真连一台外部服务器。
func (h *harness) withNotifySender(t *testing.T, client core.HTTPClient) *harness {
	t.Helper()
	h.server.notify = NewNotifyWorld(h.store, client, nil, newClock())
	return h
}

func testSend(t *testing.T, h *harness, body string) *httptest.ResponseRecorder {
	t.Helper()
	return h.do(t, "POST", "/api/notify/test", body, nil)
}

// seedNotifyConfig 直接写库，**绕开** PUT 那一条路的校验。
//
// 只给两类用例用：① 必填缺半（"配了但没填全"正是这一族最常见的现场，
// 而写接口会先 400 拦掉）；② email 那一家（要 port 指针，用 JSON 写也行，
// 但这里连"读回的形状"一起控住）。
func seedNotifyConfig(t *testing.T, h *harness, config core.NotifyConfig) {
	t.Helper()
	config.UpdatedAt = &fixedNow
	if err := h.store.PutNotifyConfig(context.Background(), &config); err != nil {
		t.Fatalf("写推送配置失败: %v", err)
	}
}

func configureBark(t *testing.T, h *harness) {
	t.Helper()
	rec := putNotify(t, h, `{"channels":{"bark":{"key":"`+barkKey+`"}}}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("配 bark 应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
}

// testResultRows 取逐家结果那张表（按渠道名索引）。
func testResultRows(t *testing.T, rec *httptest.ResponseRecorder) []map[string]any {
	t.Helper()
	body := decode(t, rec)
	raw, _ := body["results"].([]any)
	if raw == nil {
		t.Fatalf("响应里没有 results 那张表：%v", body)
	}
	rows := make([]map[string]any, 0, len(raw))
	for _, item := range raw {
		row, ok := item.(map[string]any)
		if !ok {
			t.Fatalf("results 里有一项不是对象：%v", item)
		}
		rows = append(rows, row)
	}
	return rows
}

func testRowOrder(t *testing.T, rows []map[string]any) []string {
	t.Helper()
	out := make([]string, 0, len(rows))
	for _, row := range rows {
		out = append(out, mustText(t, row, "channel"))
	}
	return out
}

func testRow(t *testing.T, rows []map[string]any, channel string) map[string]any {
	t.Helper()
	for _, row := range rows {
		if mustText(t, row, "channel") == channel {
			return row
		}
	}
	t.Fatalf("结果里没有 %s 这一家：%v", channel, testRowOrder(t, rows))
	return nil
}

func testBodyText(t *testing.T, rec *httptest.ResponseRecorder) string {
	t.Helper()
	return mustText(t, decode(t, rec), "body")
}

const (
	warnLineText  = "这一条是 warn 档"
	errorLineText = "这一条是 error 档"
	infoLineText  = "这是一条测试推送。"
)

// ─────────────────────────────────────────────────────────────────────────────
// 没接发送器时怎么说
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyTestFailsLoudlyWhenTheSenderIsNotWired(t *testing.T) {
	h := newHarness(t).initialized(t)
	configureBark(t, h)

	rec := testSend(t, h, `{}`)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("装配漏了应当 500，实得 %d：%s", rec.Code, rec.Body.String())
	}
	// 对外只有 `code:"internal"` 那一句通用话（"本进程没接推送发送器"进不了响应体 ——
	// 那是 [core.KindInternal] 既成的"不外泄内部细节"口径）。这一条用例真正要钉的是
	// **状态码不是 200**：漏装配若被写成 200 + 空结果，界面上就是"试发成功"。
	if code := mustText(t, decode(t, rec), "code"); code != "internal" {
		t.Errorf("错误码应是 internal（这是装配问题，不是用户配错），实得 %q", code)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 哪些闸绕过、哪些不绕
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyTestWorksWhileTheAutoPushSwitchIsOff(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	// enabled 保持默认 false：这一条就是"先测通再开"那一步的守卫。
	configureBark(t, h)
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("总开关关着也该允许试发，实得 %d：%s", rec.Code, rec.Body.String())
	}
	row := testRow(t, testResultRows(t, rec), "bark")
	if got := mustText(t, row, "outcome"); got != "sent" {
		t.Errorf("bark 应算已发，实得 %q（reason %q）", got, row["reason"])
	}
	if hits := client.hostHits()["bark"]; hits != 1 {
		t.Errorf("bark 应被打一次，实得 %d：%v", hits, client.urls())
	}
	// `gates` 那一块是**给人看的**"这一条没经过哪些闸"，不是判定结果。
	gates, _ := decode(t, rec)["gates"].(map[string]any)
	if gates == nil {
		t.Fatal("响应里没有 gates 那一块")
	}
	if mustBool(t, gates, "enabled") {
		t.Error("gates.enabled 应如实报false（试发成功≠自动推会成功）")
	}
}

func TestNotifyTestIgnoresOnlyErrorForTheRequestButStillCarriesAnErrorLine(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	if rec := putNotify(t, h, `{"onlyError":true,"channels":{"bark":{"key":"`+barkKey+`"}}}`); rec.Code != http.StatusOK {
		t.Fatalf("配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("只推错误也不该挡住试发，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if client.hostHits()["bark"] != 1 {
		t.Errorf("应打一次 bark，实得请求 %v", client.urls())
	}
	// 试发正文里**固定**带一条 error 行：`onlyError` 的判据是"这次有没有 error 档"，
	// 没有这一行的话"只推错误"的人一点试发就永远收不到，而那正好是他要验的那一档。
	if text := testBodyText(t, rec); !strings.Contains(text, errorLineText) {
		t.Errorf("正文应含 error 那一行，实得 %q", text)
	}
}

func TestNotifyTestAppliesTheSameBodyLevelGate(t *testing.T) {
	// 门槛：info 收三行，warn 收两行，error 只收一行。**不**绕过（抬头写了理由）。
	cases := []struct {
		level string
		want  []string
		deny  []string
	}{
		{level: "info", want: []string{infoLineText, warnLineText, errorLineText}},
		{level: "warn", want: []string{warnLineText, errorLineText}, deny: []string{infoLineText}},
		{level: "error", want: []string{errorLineText}, deny: []string{infoLineText, warnLineText}},
		{level: "debug", want: []string{infoLineText, warnLineText, errorLineText}},
	}
	for _, testCase := range cases {
		t.Run(testCase.level, func(t *testing.T) {
			h := newHarness(t).initialized(t)
			client := &notifyClient{}
			if rec := putNotify(t, h, `{"minLevel":"`+testCase.level+
				`","channels":{"bark":{"key":"`+barkKey+`"}}}`); rec.Code != http.StatusOK {
				t.Fatalf("配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
			}
			h.withNotifySender(t, client)

			rec := testSend(t, h, `{}`)
			if rec.Code != http.StatusOK {
				t.Fatalf("应 200，实得 %d：%s", rec.Code, rec.Body.String())
			}
			text := testBodyText(t, rec)
			for _, want := range testCase.want {
				if !strings.Contains(text, want) {
					t.Errorf("门槛 %s 下正文应含 %q，实得 %q", testCase.level, want, text)
				}
			}
			for _, deny := range testCase.deny {
				if strings.Contains(text, deny) {
					t.Errorf("门槛 %s 下正文不该含 %q，实得 %q", testCase.level, deny, text)
				}
			}
			// 出网的那一条**与响应里这一份同文**：响应只是回执，真发出去的内容
			// 若另走一份组装，界面上"看着对、手机上不对"就查不出来。
			got := string(client.requests[0].Body)
			for _, want := range testCase.want {
				if !strings.Contains(got, jsonEscaped(want)) {
					t.Errorf("真发出的请求体应含 %q，实得 %q", want, got)
				}
			}
		})
	}
}

// jsonEscaped 把一句中文按 JSON 字符串的字面量形状转出来（请求体里的正文是 JSON 值，
// 而 [core.MarshalJSON] 不转义非 ASCII，所以中文原样在，只有引号/反斜杠要处理）。
func jsonEscaped(text string) string {
	encoded, err := core.MarshalJSON(text)
	if err != nil {
		panic(err)
	}
	return string(encoded[1 : len(encoded)-1])
}

// ─────────────────────────────────────────────────────────────────────────────
// 打给哪几家
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyTestSendsOnlyTheRequestedChannels(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	if rec := putNotify(t, h, `{"channels":{"bark":{"key":"`+barkKey+`"},`+
		`"serverChan":{"token":"SCT-synthetic-token"}}}`); rec.Code != http.StatusOK {
		t.Fatalf("配两家应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	h.withNotifySender(t, client)

	if rec := testSend(t, h, `{"channels":["bark"]}`); rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	hits := client.hostHits()
	if hits["bark"] != 1 {
		t.Errorf("点名 bark 却打了几次：%v", hits)
	}
	if hits["serverChan"] != 0 {
		t.Errorf("没点名的那一家不该被打：%v", hits)
	}
	// 不点名 = 全部已配的那几家。
	if rec := testSend(t, h, `{}`); rec.Code != http.StatusOK {
		t.Fatalf("空体那次应 200，实得 %d", rec.Code)
	}
	hits = client.hostHits()
	if hits["bark"] != 2 || hits["serverChan"] != 1 {
		t.Errorf("不点名时应把两家都打到，实得 %v", hits)
	}
}

func TestNotifyTestDeduplicatesAndOrdersChannels(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	if rec := putNotify(t, h, `{"channels":{"bark":{"key":"`+barkKey+`"},`+
		`"serverChan":{"token":"SCT-synthetic-token"},"tgBot":{"token":"`+tgToken+
		`","chat_id":"-100123"}}}`); rec.Code != http.StatusOK {
		t.Fatalf("配三家应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	h.withNotifySender(t, client)

	// 重复点名一次就够；顺序按固定序（serverChan 在 bark 前，与请求里写的先后无关）。
	rec := testSend(t, h, `{"channels":["bark","bark","serverChan"]}`)
	order := testRowOrder(t, testResultRows(t, rec))
	if len(order) != 2 || order[0] != "serverChan" || order[1] != "bark" {
		t.Errorf("应按固定序去重，实得 %v", order)
	}
	if hits := client.hostHits(); hits["bark"] != 1 || hits["serverChan"] != 1 || hits["tgBot"] != 0 {
		t.Errorf("请求次数不对：%v（%v）", hits, client.urls())
	}
}

func TestNotifyTestRejectsAnUnknownChannelName(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	configureBark(t, h)
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{"channels":["barks"]}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("打错的渠道名应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if !strings.Contains(errorMessage(t, rec), "barks") {
		t.Errorf("那一句该带上打错的名字：%q", errorMessage(t, rec))
	}
	if len(client.urls()) != 0 {
		t.Errorf("报错那次一家都不该被打：%v", client.urls())
	}
}

func TestNotifyTestRefusesToReachAnUnconfiguredChannel(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	configureBark(t, h)
	h.withNotifySender(t, client)

	// 这一条是 SSRF 形状的闸：一个输入框若能凭空指定一家，
	// 就等于让这台机器往任意 URL 发一条带正文的请求。
	rec := testSend(t, h, `{"channels":["tgBot"]}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("没配的一家应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	message := errorMessage(t, rec)
	if !strings.Contains(message, "没有配置") {
		t.Errorf("那一句要说清是**没配**（而不是名字打错）：%q", message)
	}
	if len(client.urls()) != 0 {
		t.Errorf("一次都不该出门：%v", client.urls())
	}
}

func TestNotifyTestSaysSoWhenNothingIsConfigured(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("一家没配时应 400 说清楚，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if !strings.Contains(errorMessage(t, rec), "一家推送渠道都没配") {
		t.Errorf("那一句该说'一家都没配'：%q", errorMessage(t, rec))
	}
	if len(client.urls()) != 0 {
		t.Errorf("一次都不该出门：%v", client.urls())
	}
}

func TestNotifyTestReportsAMissingRequiredFieldAsSkipped(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	// 绕开写接口直接落一行"配了但 key 空着"的记录：这正是"我以为配上了"那一格，
	// 而它在界面上的表现只能是"点了试发，然后有一句说缺什么"。
	seedNotifyConfig(t, h, core.NotifyConfig{
		MinLevel: core.NotifyLevelInfo,
		Channels: core.NotifyChannels{Bark: &core.BarkConfig{}},
	})
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200 并把逐家结论回出来，实得 %d：%s", rec.Code, rec.Body.String())
	}
	row := testRow(t, testResultRows(t, rec), "bark")
	if got := mustText(t, row, "outcome"); got != "skipped" {
		t.Errorf("缺必填应算 skipped，实得 %q", got)
	}
	if reason := mustText(t, row, "reason"); !strings.Contains(reason, "缺少必填项") {
		t.Errorf("那一句该指出缺必填：%q", reason)
	}
	if len(client.urls()) != 0 {
		t.Errorf("缺必填的那一家不该真出门：%v", client.urls())
	}
}

func TestNotifyTestSkipsEmailWhenNoSMTPSenderIsWired(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	port := 465
	seedNotifyConfig(t, h, core.NotifyConfig{
		MinLevel: core.NotifyLevelInfo,
		Channels: core.NotifyChannels{Email: &core.EmailConfig{
			Host: "smtp.test", Port: &port,
			From: core.NewSecretValue("me@smtp.test"),
			Pass: core.NewSecretValue(smtpPass),
			To:   "you@smtp.test",
		}},
	})
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	row := testRow(t, testResultRows(t, rec), "email")
	if got := mustText(t, row, "outcome"); got != "skipped" {
		t.Errorf("没接 SMTP 应算 skipped 而不是 failed，实得 %q", got)
	}
	if reason := mustText(t, row, "reason"); !strings.Contains(reason, "SMTP") {
		t.Errorf("那一句要说清是没接 SMTP 发送器：%q", reason)
	}
	if strings.Contains(rec.Body.String(), smtpPass) {
		t.Error("授权码不该出现在响应里")
	}
	// email 走的是 SMTP 出口，不占 HTTP 客户端 —— 这一条为零也说明它没被"顺手"改成 HTTP。
	if len(client.urls()) != 0 {
		t.Errorf("email 不该走 HTTP 出口：%v", client.urls())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 回执的形状
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyTestAcceptsAnEmptyBody(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	configureBark(t, h)
	h.withNotifySender(t, client)

	// 界面上那枚按钮什么也不填：`do` 在 body 为空时给的是 ContentLength 0。
	rec := testSend(t, h, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("空请求体应是合法用法，实得 %d：%s", rec.Code, rec.Body.String())
	}
	text := testBodyText(t, rec)
	if !strings.Contains(text, infoLineText) {
		t.Errorf("默认正文该含那句占位的话，实得 %q", text)
	}
	if got := mustText(t, decode(t, rec), "title"); got != core.DefaultNotifyTitle {
		t.Errorf("没配标题时应回默认标题，实得 %q", got)
	}
}

func TestNotifyTestTitleAndMessageOverrides(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	if rec := putNotify(t, h, `{"title":"配置里的标题","channels":{"bark":{"key":"`+barkKey+`"}}}`); rec.Code != http.StatusOK {
		t.Fatalf("配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{"title":"这一次用这个","message":"自定义的那一句"}`)
	body := decode(t, rec)
	if got := mustText(t, body, "title"); got != "这一次用这个" {
		t.Errorf("标题覆盖没生效，实得 %q", got)
	}
	text := testBodyText(t, rec)
	if !strings.Contains(text, "自定义的那一句") {
		t.Errorf("正文应含自定义那句，实得 %q", text)
	}
	if strings.Contains(text, infoLineText) {
		t.Errorf("覆盖了还留着默认那句：两个形状同时出现在一条消息里：%q", text)
	}
	if strings.Contains(text, "配置里的标题") {
		t.Errorf("标题被覆盖后不该还出现在正文里：%q", text)
	}
	// 空白 message 等于"没填"（与配置写入那一族同一条判据）。
	if rec = testSend(t, h, `{"message":"   "}`); strings.Contains(testBodyText(t, rec), "   ") {
		t.Errorf("一串空白不该被当成正文")
	}
}

func TestNotifyTestSummarizesThreeBuckets(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	// bark 成功、serverChan 回 500、tgBot 缺 chat_id ⇒ 三档各一家。
	seedNotifyConfig(t, h, core.NotifyConfig{
		MinLevel: core.NotifyLevelInfo,
		Channels: core.NotifyChannels{
			Bark:       &core.BarkConfig{Key: core.NewSecretValue(barkKey)},
			ServerChan: &core.ServerChanConfig{Token: core.NewSecretValue("SCT-synthetic-token")},
			TGBot:      &core.TGBotConfig{Token: core.NewSecretValue(tgToken)},
		},
	})
	client.reply = func(request core.HTTPRequest) (core.HTTPResponse, error) {
		if strings.Contains(request.URL, "sctapi") {
			return core.HTTPResponse{Status: http.StatusInternalServerError,
				Body: core.Body(`{"code":500}`)}, nil
		}
		return core.HTTPResponse{Status: http.StatusOK, Body: core.Body(`{"code":200}`)}, nil
	}
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	rows := testResultRows(t, rec)
	for channel, want := range map[string]string{
		"bark": "sent", "serverChan": "failed", "tgBot": "skipped",
	} {
		if got := mustText(t, testRow(t, rows, channel), "outcome"); got != want {
			t.Errorf("%s 应 %s，实得 %s", channel, want, got)
		}
	}
	if reason := mustText(t, testRow(t, rows, "serverChan"), "reason"); reason != "HTTP 500" {
		t.Errorf("失败那一句该是状态码，实得 %q", reason)
	}
	summary := mustText(t, decode(t, rec), "summary")
	for _, want := range []string{"已发 1", "失败 1", "跳过 1", "serverChan"} {
		if !strings.Contains(summary, want) {
			t.Errorf("小结该含 %q，实得 %q", want, summary)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 凭据不外泄
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyTestResponseCarriesNoCredential(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	if rec := putNotify(t, h, `{"channels":{`+
		`"bark":{"key":"`+barkKey+`"},`+
		`"tgBot":{"token":"`+tgToken+`","chat_id":"-100123"},`+
		`"workWeixinBot":{"url":"`+webhookURL+`"},`+
		`"twoIm":{"key":"2im-key-synthetic","sid":"2im-sid-synthetic"},`+
		`"customPost":[{"url":"`+customURL+`","headers":{"Authorization":"`+customHeader+`"}}]`+
		`}}`); rec.Code != http.StatusOK {
		t.Fatalf("配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	// 模拟 `io.Client` 的口径：请求发不出去时，错误串里**带着完整 URL**
	// （`Get "https://api.day.app/<key>": ...`），而四家的密钥就在 URL 里。
	client.reply = func(request core.HTTPRequest) (core.HTTPResponse, error) {
		return core.HTTPResponse{}, fmt.Errorf("Get %q: dial tcp: i/o timeout", request.URL)
	}
	h.withNotifySender(t, client)

	rec := testSend(t, h, `{}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200 并逐家记账，实得 %d：%s", rec.Code, rec.Body.String())
	}
	// 先证明"这些凭据今天确实在请求里"，否则下面那半段是空转。
	urls := strings.Join(client.urls(), "\n")
	for _, want := range []string{barkKey, tgToken} {
		if !strings.Contains(urls, want) {
			t.Errorf("前提不成立：请求 URL 里没有 %q，这一条就没在防泄漏（%v）", want, client.urls())
		}
	}
	text := rec.Body.String()
	for _, secret := range []string{
		barkKey, tgToken, corpSecretValue, smtpPass, webhookURL, customURL, customHeader,
		"synthetic-abc", "synthetic-xyz", "2im-key-synthetic", "2im-sid-synthetic",
	} {
		if strings.Contains(text, secret) {
			t.Errorf("响应里出现了凭据 %q", secret)
		}
	}
	// 而且**每**一家都要有一句可看的话：只把凭据抹掉、留一句空话，等于把失败也抹掉了。
	for _, row := range testResultRows(t, rec) {
		if mustText(t, row, "outcome") != "failed" {
			t.Errorf("传输超时应算 failed，实得 %v", row)
		}
		if reason := mustText(t, row, "reason"); reason == "" {
			t.Errorf("一家没有原因：%v", row)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 闸门
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyTestRequiresAnIdentity(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	configureBark(t, h)
	h.withNotifySender(t, client)
	h.cookie = nil

	if rec := testSend(t, h, `{}`); rec.Code != http.StatusUnauthorized {
		t.Errorf("没带会话应 401，实得 %d", rec.Code)
	}
	if len(client.urls()) != 0 {
		t.Errorf("未授权的请求不该触发任何外发：%v", client.urls())
	}
}

func TestNotifyTestRejectsAMalformedBody(t *testing.T) {
	h := newHarness(t).initialized(t)
	client := &notifyClient{}
	configureBark(t, h)
	h.withNotifySender(t, client)

	// `channels` 写成字符串：与"没有这个键"不同形，必须报错而不是当成"没点名"
	// —— 后者会把"我只想发 bark"变成"十家全发"。
	for _, raw := range []string{`{"channels":"bark"}`, `not json`, `[1,2,3]`} {
		rec := testSend(t, h, raw)
		if rec.Code != http.StatusBadRequest {
			t.Errorf("请求体 %q 应 400，实得 %d：%s", raw, rec.Code, rec.Body.String())
		}
	}
	if len(client.urls()) != 0 {
		t.Errorf("报错的三次一次都不该出门：%v", client.urls())
	}
}
