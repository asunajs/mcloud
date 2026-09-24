package server

// 推送配置读写接口的用例。
//
// 三件事各有一组，因为它们都是"改了不会响"的那一类设计决定：
//   - 出网视图**不泄漏**凭据（改了字段表就可能泄漏，而编译期没人拦）；
//   - 写入是**逐字段合并**（改成整块覆盖的话，界面上只改一个输入框就会洗掉看不见的凭据）；
//   - 不认识的键**必须 400**（标准库不拒绝未知字段，打错键名会变成"配了却不响"）。

import (
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

func putNotify(t *testing.T, h *harness, body string) *httptest.ResponseRecorder {
	t.Helper()
	return h.do(t, "PUT", "/api/notify/config", body, nil)
}

func notifyView(t *testing.T, h *harness) map[string]any {
	t.Helper()
	rec := h.do(t, "GET", "/api/notify/config", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("读推送配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	return viewOf(t, decode(t, rec))
}

func viewOf(t *testing.T, body map[string]any) map[string]any {
	t.Helper()
	view, _ := body["config"].(map[string]any)
	if view == nil {
		t.Fatalf("响应里没有 config 那一块：%v", body)
	}
	return view
}

// channelView 取一家的视图（十家**一家都不该少**：界面要靠它渲染整张表单）。
func channelView(t *testing.T, view map[string]any, name string) map[string]any {
	t.Helper()
	channels, _ := view["channels"].(map[string]any)
	if channels == nil {
		t.Fatalf("视图里没有 channels：%v", view)
	}
	one, _ := channels[name].(map[string]any)
	if one == nil {
		t.Fatalf("视图里缺了这一家 %s（十家必须全部出现）：%v", name, keysOf(channels))
	}
	return one
}

// fieldsOf 取一家的字段清单（**数组**，序 = 后端那张字段表的声明序）。
func fieldsOf(t *testing.T, one map[string]any) []map[string]any {
	t.Helper()
	raw, _ := one["fields"].([]any)
	if raw == nil {
		t.Fatalf("这一家的视图里 fields 不是数组：%v", one["fields"])
	}
	out := make([]map[string]any, 0, len(raw))
	for _, item := range raw {
		row, _ := item.(map[string]any)
		if row == nil {
			t.Fatalf("字段清单里出现了一个不是对象的格子：%v", item)
		}
		out = append(out, row)
	}
	return out
}

// fieldNames 取一家的字段名（按清单顺序）。
func fieldNames(t *testing.T, one map[string]any) []string {
	t.Helper()
	rows := fieldsOf(t, one)
	out := make([]string, 0, len(rows))
	for _, row := range rows {
		name, _ := row["name"].(string)
		out = append(out, name)
	}
	return out
}

// fieldView 取一家里一个字段的视图。
func fieldView(t *testing.T, one map[string]any, field string) map[string]any {
	t.Helper()
	for _, row := range fieldsOf(t, one) {
		if row["name"] == field {
			return row
		}
	}
	t.Fatalf("这一家的视图里没有字段 %s：%v", field, one)
	return nil
}

func mustBool(t *testing.T, view map[string]any, key string) bool {
	t.Helper()
	value, ok := view[key].(bool)
	if !ok {
		t.Fatalf("%s 应是布尔，实得 %T（%v）", key, view[key], view[key])
	}
	return value
}

func mustText(t *testing.T, view map[string]any, key string) string {
	t.Helper()
	if view[key] == nil {
		return ""
	}
	value, ok := view[key].(string)
	if !ok {
		t.Fatalf("%s 应是字符串，实得 %T（%v）", key, view[key], view[key])
	}
	return value
}

// names 读一张清单（`configured` / `sendable`）。
//
// ⚠ "这一格根本不存在或不是数组"必须**报错**，不能当成空清单：
// Go 的 nil slice marshal 出来是 `null`，而 `null` 到了界面里不是"一家都没配"
// 而是"这格坏了"（实测：设置页三张卡整片不出现）。上一版的写法是
// `raw, _ := view[key].([]any)`，它把 `null` 悄悄读成长度 0 ——
// 于是后端带着这个缺陷也照样"通过"了这条测试。
func names(t *testing.T, view map[string]any, key string) []string {
	t.Helper()
	value, ok := view[key]
	if !ok {
		t.Fatalf("视图里没有 %q 这一格：%v", key, keysOf(view))
	}
	raw, ok := value.([]any)
	if !ok {
		t.Fatalf("%q 应是数组，实得 %T（%v）—— nil 清单要在后端裁成 []，不能出网成 null", key, value, value)
	}
	out := make([]string, 0, len(raw))
	for _, item := range raw {
		text, _ := item.(string)
		out = append(out, text)
	}
	return out
}

// shownLength 读"这一格有几个字符"那一栏（数字经 [asText] 归一后再比，
// 免得 `int64` 与 `json.Number` 两种形态在断言里各对一半）。
func shownLength(t *testing.T, field map[string]any) int {
	t.Helper()
	raw := asText(t, field["length"])
	value, err := strconv.Atoi(raw)
	if err != nil {
		t.Fatalf("length 不是个数字：%v", field)
	}
	return value
}

func errorMessage(t *testing.T, rec *httptest.ResponseRecorder) string {
	t.Helper()
	return mustText(t, decode(t, rec), "message")
}

func keysOf(m map[string]any) []string {
	out := make([]string, 0, len(m))
	for key := range m {
		out = append(out, key)
	}
	return out
}

// 下面这些都是**合成**值（不属于任何真服务）。它们长到足以被 `strings.Contains`
// 认出来，短到能塞进断言。
const (
	barkKey         = "bK-synthetic-key-0123456789"
	barkKeySecond   = "bK-second-synthetic-key"
	tgToken         = "123456:AA-synthetic-token"
	corpSecretValue = "corpsecret-synthetic-value"
	smtpPass        = "smtp-pass-synthetic-value"
	webhookURL      = "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=synthetic-abc"
	customURL       = "https://example.test/hook?token=synthetic-xyz"
	customHeader    = "Bearer synthetic-header-token"
)

// ─────────────────────────────────────────────────────────────────────────────
// 默认视图
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyConfigDefaultsToOffWithNothingConfigured(t *testing.T) {
	h := newHarness(t).initialized(t)
	view := notifyView(t, h)

	if mustBool(t, view, "enabled") {
		t.Error("默认应关闭：开了就等于配置一迁过来就往外发消息")
	}
	if mustBool(t, view, "onlyError") {
		t.Error("默认不该只推错误")
	}
	if got := mustText(t, view, "minLevel"); got != string(core.NotifyLevelInfo) {
		t.Errorf("默认门槛应是 info，实得 %q", got)
	}
	if view["title"] != nil {
		t.Errorf("默认没有自定义标题，实得 %v", view["title"])
	}
	if view["updatedAt"] != nil {
		t.Errorf("没写过就不该有时间戳，实得 %v", view["updatedAt"])
	}
	if got := names(t, view, "configured"); len(got) != 0 {
		t.Errorf("默认一家都没配，实得 %v", got)
	}
	if got := names(t, view, "sendable"); len(got) != 0 {
		t.Errorf("默认可发清单应空，实得 %v", got)
	}
	// 十家**全部**列出（含未配置的），未配置的那几家不该把"缺少必填项"念出来。
	channels, _ := view["channels"].(map[string]any)
	if len(channels) != len(core.NotifyChannelOrder()) {
		t.Fatalf("视图里应有十家，实得 %d：%v", len(channels), keysOf(channels))
	}
	for _, name := range core.NotifyChannelOrder() {
		one := channelView(t, view, string(name))
		if mustBool(t, one, "configured") {
			t.Errorf("%s 默认不该算配了", name)
		}
		if got := mustText(t, one, "missing"); got != "" {
			t.Errorf("%s 未配置时报 %q —— 没配与配错了是两句话，不能混", name, got)
		}
	}
}

// 一张都没配时，那两张清单在**线格式**上必须是 `[]` 而不是 `null`。
//
// 为什么单独立一条、而不靠上面那条 `names(...)==0`：界面读的是解码后的数组，
// `null` 与 `[]` 在"长度为 0"这一层看起来一样，坏的是下一层 ——
// `configured.includes(name)` 在 `null` 上直接抛 TypeError，设置页三张卡整片不出现。
// 反向改坏它：把 [notifyPayloadOf] 里那两处 nil→[] 的裁剪删掉，这条会红。
func TestNotifyConfigEmptyListsLeaveAsArraysNotNull(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := h.do(t, "GET", "/api/notify/config", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("读推送配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	body := rec.Body.String()
	for _, want := range []string{`"configured":[]`, `"sendable":[]`, `"order":[`} {
		if !strings.Contains(body, want) {
			t.Errorf("出网体里应出现 %s（nil 清单要裁成空数组）：实得 %s", want, body)
		}
	}
	for _, bad := range []string{`"configured":null`, `"sendable":null`} {
		if strings.Contains(body, bad) {
			t.Errorf("出网体里出现了 %s —— 前端按数组读它，null 会让整页渲染断掉", bad)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 往返 + 出网形状
// ─────────────────────────────────────────────────────────────────────────────
func TestNotifyConfigPutRoundTripsScalars(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := putNotify(t, h, `{"enabled":true,"title":"我的标题","onlyError":true,`+
		`"minLevel":"warn","channels":{"bark":{"key":"`+barkKey+`"}}}`)
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	view := viewOf(t, decode(t, rec))
	if !mustBool(t, view, "enabled") || !mustBool(t, view, "onlyError") {
		t.Errorf("两个开关没存住：%v", view)
	}
	if got := mustText(t, view, "title"); got != "我的标题" {
		t.Errorf("标题实得 %q", got)
	}
	if got := mustText(t, view, "minLevel"); got != "warn" {
		t.Errorf("门槛实得 %q", got)
	}
	if view["updatedAt"] == nil {
		t.Error("写过一次就该有时间戳（界面上那一格“上次改动”靠它）")
	}
	one := channelView(t, view, "bark")
	if !mustBool(t, one, "configured") {
		t.Fatal("bark 应算配好了")
	}
	field := fieldView(t, one, "key")
	if !mustBool(t, field, "present") {
		t.Error("填了 key 却显示没填")
	}
	if got, want := shownLength(t, field), utf8.RuneCountInString(barkKey); got != want {
		t.Errorf("长度实得 %d，期望 %d", got, want)
	}
	if got := mustText(t, field, "kind"); got != "secret" {
		t.Errorf("凭据那一格应标成 secret，实得 %q", got)
	}
	if got := names(t, view, "sendable"); len(got) != 1 || got[0] != "bark" {
		t.Errorf("可发清单实得 %v", got)
	}
}

func TestNotifyConfigResponseNeverCarriesAnyCredential(t *testing.T) {
	h := newHarness(t).initialized(t)
	body := `{"enabled":true,"channels":{` +
		`"bark":{"key":"` + barkKey + `"},` +
		`"tgBot":{"token":"` + tgToken + `","chat_id":"-100123"},` +
		`"serverChan":{"token":"SCT-synthetic-token"},` +
		`"dingTalk":{"token":"ding-token-synthetic","secret":"ding-secret-synthetic"},` +
		`"pushplus":{"token":"pp-token-synthetic","extra":{"topic":"synthetic-topic"}},` +
		`"workWeixin":{"corpid":"cid-synthetic","corpsecret":"` + corpSecretValue + `"},` +
		`"workWeixinBot":{"url":"` + webhookURL + `"},` +
		`"twoIm":{"key":"2im-key-synthetic","sid":"2im-sid-synthetic"},` +
		`"email":{"host":"smtp.test","port":465,"from":"me@smtp.test","pass":"` + smtpPass + `","to":"you@smtp.test"},` +
		`"customPost":[{"url":"` + customURL + `","method":"POST","headers":{"Authorization":"` + customHeader + `"},"data":{"note":"synthetic-data"}}]` +
		`}}`
	if rec := putNotify(t, h, body); rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}

	// 两份响应都查：PUT 自己回的那一份、与另一次 GET。
	recs := map[string]*httptest.ResponseRecorder{
		"PUT": putNotify(t, h, `{"enabled":true}`),
		"GET": h.do(t, "GET", "/api/notify/config", "", nil),
	}
	for _, secret := range []string{
		barkKey, tgToken, "SCT-synthetic-token", "ding-token-synthetic",
		"ding-secret-synthetic", "pp-token-synthetic", "cid-synthetic", corpSecretValue,
		webhookURL, "2im-key-synthetic", "2im-sid-synthetic", smtpPass,
		customURL, customHeader, "synthetic-abc", "synthetic-xyz", "synthetic-token",
	} {
		for label, rec := range recs {
			if strings.Contains(rec.Body.String(), secret) {
				t.Errorf("%s 响应里出现了凭据原文 %q", label, secret)
			}
		}
	}
	for label, rec := range recs {
		// 自由键的**值**也不出（只出键名）：`extra` 里可能装着第二个凭据。
		if strings.Contains(rec.Body.String(), "synthetic-topic") ||
			strings.Contains(rec.Body.String(), "synthetic-data") {
			t.Errorf("%s 响应里出现了自由键的值", label)
		}
	}

	view := viewOf(t, decode(t, recs["GET"]))
	// 反面：非凭据的配置值**必须**出，否则界面没法编辑（掩掉一切等于这一栏只能删了重填）。
	if got := fieldView(t, channelView(t, view, "tgBot"), "chat_id")["value"]; got != "-100123" {
		t.Errorf("chat_id 应原样出，实得 %v", got)
	}
	mail := channelView(t, view, "email")
	if got := fieldView(t, mail, "host")["value"]; got != "smtp.test" {
		t.Errorf("host 应原样出，实得 %v", got)
	}
	if got := asText(t, fieldView(t, mail, "port")["value"]); got != "465" {
		t.Errorf("port 应原样出，实得 %v", got)
	}
	if got := asText(t, fieldView(t, mail, "to")["value"]); got != "you@smtp.test" {
		t.Errorf("to 应原样出，实得 %v", got)
	}
	// `from` / `pass` 跟随 core 的分类（都是 SecretValue），这里一致地掩掉。
	if got := mustText(t, fieldView(t, mail, "from"), "kind"); got != "secret" {
		t.Errorf("email.from 应标成 secret（core 就是这么分类的），实得 %q", got)
	}
	// 整条就是凭据的 webhook 地址：core 里它是普通字符串，这里仍按凭据处理。
	if got := mustText(t, fieldView(t, channelView(t, view, "workWeixinBot"), "url"), "kind"); got != "secret" {
		t.Error("workWeixinBot.url 应按凭据处理：整条地址就是 key")
	}
	custom := channelView(t, view, "customPost")
	items, _ := custom["items"].([]any)
	if len(items) != 1 {
		t.Fatalf("customPost 应有 1 条，实得 %d：%v", len(items), custom)
	}
	first, _ := items[0].(map[string]any)
	urlField := fieldView(t, first, "url")
	if urlField["kind"] != "secret" {
		t.Errorf("customPost 的 url 应掩掉，实得 %v", urlField)
	}
	headerField := fieldView(t, first, "headers")
	if got, _ := headerField["keys"].([]any); len(got) != 1 || got[0] != "Authorization" {
		t.Errorf("headers 只出键名，实得 %v", headerField)
	}
	if got := mustText(t, fieldView(t, first, "method"), "kind"); got != "plain" {
		t.Errorf("method 是普通配置值，实得 %v", fieldView(t, first, "method"))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 不认识的键 ⇒ 400
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyConfigRejectsAnUnknownTopLevelKey(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := putNotify(t, h, `{"enabled":true,"onlyerror":true}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("打错的顶层键应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	message := errorMessage(t, rec)
	if !strings.Contains(message, "onlyerror") || !strings.Contains(message, "onlyError") {
		t.Errorf("那句错该指出**打错的键**并列出**认识的键**：%q", message)
	}
	// 且**一个字段都不落**：整次请求失败，不是"部分保存"。
	if mustBool(t, notifyView(t, h), "enabled") {
		t.Error("报错那次不该把 enabled 写进去")
	}
}

func TestNotifyConfigRejectsATypoInAChannelFieldName(t *testing.T) {
	h := newHarness(t).initialized(t)
	// `chatId` 而不是 `chat_id` —— 旧实现那一族"配了却不响"的经典成因。
	rec := putNotify(t, h, `{"channels":{"tgBot":{"token":"`+tgToken+`","chatId":"-1"}}}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("渠道里打错的字段应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	message := errorMessage(t, rec)
	if !strings.Contains(message, "chatId") || !strings.Contains(message, "chat_id") {
		t.Errorf("错话里要同时有打错的键与正确的键名：%q", message)
	}
	if got := names(t, notifyView(t, h), "sendable"); len(got) != 0 {
		t.Errorf("那次失败的写入不该留下 tgBot：%v", got)
	}
}

func TestNotifyConfigRejectsAnUnknownChannelName(t *testing.T) {
	h := newHarness(t).initialized(t)
	// 库里被人手改 JSON、或界面传了个旧名字：静默跳过的表现是"我以为配上了"。
	rec := putNotify(t, h, `{"channels":{"smtp":{"key":"x"}}}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("不认识的渠道名应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "smtp") {
		t.Errorf("那句错该带上打错的名字：%q", message)
	}
}

func TestNotifyConfigRejectsAWrongShapedChannelEntry(t *testing.T) {
	h := newHarness(t).initialized(t)
	cases := map[string]string{
		"对象型写成字符串": `{"channels":{"bark":"` + barkKey + `"}}`,
		"数组型写成对象":  `{"channels":{"customPost":{"url":"https://example.test/x"}}}`,
	}
	for label, body := range cases {
		if got := putNotify(t, h, body).Code; got != http.StatusBadRequest {
			t.Errorf("%s 应 400，实得 %d", label, got)
		}
	}
}

func TestNotifyConfigRejectsABodyThatIsNotAnObject(t *testing.T) {
	h := newHarness(t).initialized(t)
	for _, raw := range []string{"", "not json", "[1,2,3]", `"x"`, "5"} {
		if got := putNotify(t, h, raw).Code; got != http.StatusBadRequest {
			t.Errorf("请求体 %q 应 400，实得 %d", raw, got)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 校验与必填
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyConfigRejectsAChannelWithoutItsRequiredField(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := putNotify(t, h, `{"channels":{"bark":{}}}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("配了 bark 却没填 key 应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "key") {
		t.Errorf("那句错该指出缺 **哪个字段**：%q", message)
	}
}

func TestNotifyConfigRejectsAnUnknownMinLevel(t *testing.T) {
	h := newHarness(t).initialized(t)
	rec := putNotify(t, h, `{"minLevel":"verbose"}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("不认识的门槛应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	message := errorMessage(t, rec)
	for _, level := range []string{"error", "warn", "info", "debug"} {
		if !strings.Contains(message, level) {
			t.Errorf("那句错该列出四个合法门槛：%q", message)
			break
		}
	}
}

func TestNotifyConfigAllowsAnEmptyMinLevelAsFollowDefault(t *testing.T) {
	h := newHarness(t).initialized(t)
	if got := putNotify(t, h, `{"minLevel":""}`).Code; got != http.StatusOK {
		t.Fatalf("空门槛是有效信息（跟随默认），应 200，实得 %d", got)
	}
	if got := mustText(t, notifyView(t, h), "minLevel"); got != "" {
		t.Errorf("空门槛要**原样**回读（替它填成 info 会让往返对不上），实得 %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 逐字段合并（这一组是"没填 = 保持"那条设计的守卫）
// ─────────────────────────────────────────────────────────────────────────────

func TestNotifyConfigPutMergesFieldByField(t *testing.T) {
	h := newHarness(t).initialized(t)
	first := `{"title":"第一轮","channels":{` +
		`"bark":{"key":"` + barkKey + `","level":"time-sensitive","extra":{"sound":"minuet"}},` +
		`"tgBot":{"token":"` + tgToken + `","chat_id":"-100123"}}}`
	if got := putNotify(t, h, first).Code; got != http.StatusOK {
		t.Fatalf("第一轮应 200，实得 %d", got)
	}
	// 第二轮：只动 bark 的 key —— 界面上那**一个**输入框就是它的形状。
	second := `{"channels":{"bark":{"key":"` + barkKeySecond + `"}}}`
	if got := putNotify(t, h, second).Code; got != http.StatusOK {
		t.Fatalf("第二轮应 200，实得 %d", got)
	}
	view := notifyView(t, h)

	if got := mustText(t, view, "title"); got != "第一轮" {
		t.Errorf("没提交 title 就被洗掉了：实得 %q", got)
	}
	bark := channelView(t, view, "bark")
	if got, want := shownLength(t, fieldView(t, bark, "key")), utf8.RuneCountInString(barkKeySecond); got != want {
		t.Errorf("新 key 没写进去：实得 %d，期望 %d", got, want)
	}
	if got := fieldView(t, bark, "level")["value"]; got != "time-sensitive" {
		t.Errorf("没提交的 level 被洗掉了：实得 %v", got)
	}
	if got := names(t, fieldView(t, bark, "extra"), "keys"); len(got) != 1 || got[0] != "sound" {
		t.Errorf("看不见的自由键被洗掉了：实得 %v", got)
	}
	tg := channelView(t, view, "tgBot")
	if !mustBool(t, tg, "configured") {
		t.Fatal("这一轮没提交 tgBot，它却被删了")
	}
	if got := fieldView(t, tg, "chat_id")["value"]; got != "-100123" {
		t.Errorf("tgBot.chat_id 被洗掉了：实得 %v", got)
	}
	if got, want := shownLength(t, fieldView(t, tg, "token")), utf8.RuneCountInString(tgToken); got != want {
		t.Errorf("tgBot.token 被洗掉了：实得 %d，期望 %d", got, want)
	}
}

func TestNotifyConfigBlankSecretKeepsTheStoredOne(t *testing.T) {
	h := newHarness(t).initialized(t)
	if got := putNotify(t, h, `{"channels":{"bark":{"key":"`+barkKey+`"}}}`).Code; got != http.StatusOK {
		t.Fatalf("第一轮应 200，实得 %d", got)
	}
	// 一串空白与"忘了填"在请求体里同形 ⇒ 只能选"保住已配好的那一份"。
	if got := putNotify(t, h, `{"channels":{"bark":{"key":"   "}}}`).Code; got != http.StatusOK {
		t.Fatalf("空白 key 应 200（视作没填），实得 %d", got)
	}
	field := fieldView(t, channelView(t, notifyView(t, h), "bark"), "key")
	if !mustBool(t, field, "present") ||
		shownLength(t, field) != utf8.RuneCountInString(barkKey) {
		t.Errorf("空白提交把已配好的 key 洗掉了：%v", field)
	}
}

func TestNotifyConfigNullRemovesAChannelAndKeepsAScalar(t *testing.T) {
	h := newHarness(t).initialized(t)
	if got := putNotify(t, h, `{"title":"留着","channels":{`+
		`"bark":{"key":"`+barkKey+`"},"tgBot":{"token":"`+tgToken+`","chat_id":"1"}}}`).Code; got != http.StatusOK {
		t.Fatalf("第一轮应 200，实得 %d", got)
	}
	if got := putNotify(t, h, `{"title":null,"minLevel":null,"channels":{"bark":null}}`).Code; got != http.StatusOK {
		t.Fatalf("第二轮应 200，实得 %d", got)
	}
	view := notifyView(t, h)
	if mustBool(t, channelView(t, view, "bark"), "configured") {
		t.Error("置 null 的一家应被删除")
	}
	if !mustBool(t, channelView(t, view, "tgBot"), "configured") {
		t.Error("没提交的一家不该被删")
	}
	if got := mustText(t, view, "title"); got != "留着" {
		t.Errorf("title 置 null 应是保持，实得 %q", got)
	}
	if got := mustText(t, view, "minLevel"); got != string(core.NotifyLevelInfo) {
		t.Errorf("minLevel 置 null 应是保持，实得 %q", got)
	}
}

func TestNotifyConfigTitleEmptyClearsItAndDefaultStaysVisible(t *testing.T) {
	h := newHarness(t).initialized(t)
	if got := putNotify(t, h, `{"title":"临时标题"}`).Code; got != http.StatusOK {
		t.Fatalf("第一轮应 200，实得 %d", got)
	}
	view := notifyView(t, h)
	if got := mustText(t, view, "title"); got != "临时标题" {
		t.Fatalf("实得 %q", got)
	}
	if got := mustText(t, view, "defaultTitle"); got != core.DefaultNotifyTitle {
		t.Errorf("视图要带上默认标题那一格（界面要靠它提示留空的后果）：%q", got)
	}
	if got := putNotify(t, h, `{"title":""}`).Code; got != http.StatusOK {
		t.Fatalf("清空那次应 200，实得 %d", got)
	}
	if view = notifyView(t, h); view["title"] != nil {
		t.Errorf("空串应是“清空”，实得 %v", view["title"])
	}
}

func TestNotifyConfigEmptyExtraObjectClearsTheFreeKeys(t *testing.T) {
	h := newHarness(t).initialized(t)
	if got := putNotify(t, h, `{"channels":{"bark":{"key":"`+barkKey+
		`","extra":{"sound":"minuet","icon":"http://x"}}}}`).Code; got != http.StatusOK {
		t.Fatalf("第一轮应 200，实得 %d", got)
	}
	bark := channelView(t, notifyView(t, h), "bark")
	if got := names(t, fieldView(t, bark, "extra"), "keys"); len(got) != 2 {
		t.Fatalf("应有两个自由键，实得 %v", got)
	}
	// 键名要**排序**出：不排的话同一份配置两次 GET 的键序不一样，界面上那行会自己换顺序。
	if got := names(t, fieldView(t, bark, "extra"), "keys"); got[0] != "icon" || got[1] != "sound" {
		t.Errorf("键名应按字典序，实得 %v", got)
	}
	if got := putNotify(t, h, `{"channels":{"bark":{"extra":{}}}}`).Code; got != http.StatusOK {
		t.Fatalf("清空那次应 200，实得 %d", got)
	}
	empty := fieldView(t, channelView(t, notifyView(t, h), "bark"), "extra")
	if got := names(t, empty, "keys"); len(got) != 0 {
		t.Errorf("extra 提交空对象应是清空，实得 %v", got)
	}
	if got := asText(t, empty["count"]); got != "0" {
		t.Errorf("count 应与键数一致：%v", empty)
	}
}

func TestNotifyConfigCustomPostIsReplacedWholesale(t *testing.T) {
	h := newHarness(t).initialized(t)
	two := `{"channels":{"customPost":[{"url":"` + customURL +
		`","method":"POST"},{"url":"https://example.test/second","method":"PUT"}]}}`
	if rec := putNotify(t, h, two); rec.Code != http.StatusOK {
		t.Fatalf("两条应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	custom := channelView(t, notifyView(t, h), "customPost")
	if got := asText(t, custom["count"]); got != "2" {
		t.Errorf("应有两条，实得 %v", custom["count"])
	}
	one := `{"channels":{"customPost":[{"url":"https://example.test/only"}]}}`
	if got := putNotify(t, h, one).Code; got != http.StatusOK {
		t.Fatalf("一条应 200，实得 %d", got)
	}
	custom = channelView(t, notifyView(t, h), "customPost")
	if got := asText(t, custom["count"]); got != "1" {
		t.Errorf("customPost 是整块替换，第二次提交后应只剩一条：%v", custom)
	}
	// 数组里的未知字段同样要拦住（与对象型渠道同一条纪律）。
	bad := `{"channels":{"customPost":[{"url":"https://example.test/x","timeoutMs":5}]}}`
	rec := putNotify(t, h, bad)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("条目里打错的键应 400，实得 %d：%s", rec.Code, rec.Body.String())
	}
	if message := errorMessage(t, rec); !strings.Contains(message, "timeoutMs") {
		t.Errorf("那句错该指出打错的键：%q", message)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 归属：这张表**无归属**
// ─────────────────────────────────────────────────────────────────────────────

// TestNotifyConfigIsOneRowSharedByEveryUser 钉的是**既成事实**，不是"应当如此"。
//
// `notify_config` 是 `CHECK (id = 1)` 的单行表（与 `exchange_config`、`webui_config`
// 同一族），所以它天然全安装共用一份。写成断言是为了：将来真要按用户分，
// 这一条一定会红，那时才看得见"这一族的行为要一起改"。
// 多用户部署下的后果（任何登录用户改得动整个安装的推送）登记在
// `docs/pending-decisions.md`。
func TestNotifyConfigIsOneRowSharedByEveryUser(t *testing.T) {
	h := newHarness(t).initialized(t)
	if got := putNotify(t, h, `{"enabled":true,"title":"管理员起的标题"}`).Code; got != http.StatusOK {
		t.Fatalf("第一轮应 200，实得 %d", got)
	}
	h.cookie = secondUser(t, h, "bob")
	view := notifyView(t, h)
	if !mustBool(t, view, "enabled") {
		t.Error("第二个身份看到的应是同一份（单行表、无归属）")
	}
	if got := mustText(t, view, "title"); got != "管理员起的标题" {
		t.Errorf("实得 %q", got)
	}
	if got := putNotify(t, h, `{"title":"bob 改的"}`).Code; got != http.StatusOK {
		t.Errorf("普通用户今天写这份配置应 200，实得 %d", got)
	}
}

// tableFieldNames 把后端那张字段表摊成名字清单（测试拿它当**唯一判据**：
// 视图里的清单必须与它逐格同序，而不是与前端另抄的一份同序）。
func tableFieldNames(fields []notifyField) []string {
	out := make([]string, 0, len(fields))
	for _, field := range fields {
		out = append(out, field.name)
	}
	return out
}

// TestNotifyViewCarriesFieldSpecsForEveryChannel 钉的是**界面**那一侧能不能只靠视图建表单：
//
//  1. 十家的顺序由 `order` 给（`channels` 是 JSON 对象，marshal 会把键名排成**字典序**，
//     而 Go 的 map 本身没有顺序）；
//  2. 每一家的 `fields` 是**数组**，格数与顺序**恰好**等于后端那张字段表 ——
//     前端因此不必抄一份表；抄了会漂移，而两种漂移都不报错（多的一格被 400 拦下、
//     少的一格永远配不到）；
//  3. **一家都没配时也要出整张清单**（按零值）：新装上用户点的就是"填写 pushplus"，
//     那时没有清单就等于这一家填不了；
//  4. 每格带 `kind` 与 `type`：同一个 `plain` 档背后是 string / number / boolean 三种控件，
//     而"值在不在"分不出它们（`port` 没填时 JSON 里根本没有这一格）。
func TestNotifyViewCarriesFieldSpecsForEveryChannel(t *testing.T) {
	h := newHarness(t).initialized(t)
	if rec := putNotify(t, h, `{"channels":{"bark":{"key":"`+barkKey+`"},`+
		`"customPost":[{"url":"`+customURL+`"}]}}`); rec.Code != http.StatusOK {
		t.Fatalf("配置应 200，实得 %d：%s", rec.Code, rec.Body.String())
	}
	view := notifyView(t, h)

	want := make([]string, 0, len(core.NotifyChannelOrder()))
	for _, name := range core.NotifyChannelOrder() {
		want = append(want, string(name))
	}
	if got := names(t, view, "order"); strings.Join(got, ",") != strings.Join(want, ",") {
		t.Errorf("order 应等于发送序，实得 %v", got)
	}

	for _, name := range want {
		one := channelView(t, view, name)
		table := notifyChannelFields[core.NotifyChannel(name)]
		if name == "customPost" {
			table = notifyCustomPostFields
		}
		got := fieldNames(t, one)
		if strings.Join(got, ",") != strings.Join(tableFieldNames(table), ",") {
			t.Errorf("%s 的字段清单与后端那张表不一致：界面 %v / 表 %v", name, got, tableFieldNames(table))
		}
		for index, field := range table {
			row := fieldsOf(t, one)[index]
			if kind := mustText(t, row, "kind"); kind != field.kind.String() {
				t.Errorf("%s.%s 档位实得 %q，期望 %q", name, field.name, kind, field.kind)
			}
			if mustText(t, row, "type") == "" {
				t.Errorf("%s.%s 没有值类型那一格（前端不知道该摆什么控件）：%v", name, field.name, row)
			}
		}
	}

	// 没配的三家（pushplus / email / tgBot）：清单齐、每格都念成"没填"。
	// 这一条是"未配置也出表单"的反面 —— 回到"只出已配置那几家"就会在这里红。
	for _, name := range []string{"pushplus", "email", "tgBot"} {
		one := channelView(t, view, name)
		if mustBool(t, one, "configured") {
			t.Fatalf("%s 本轮没配，视图却说配了", name)
		}
		rows := fieldsOf(t, one)
		if len(rows) == 0 {
			t.Fatalf("%s 没配却连字段清单都不出：%v", name, one)
		}
		for _, row := range rows {
			switch mustText(t, row, "kind") {
			case "secret":
				if mustBool(t, row, "present") || asText(t, row["length"]) != "0" {
					t.Errorf("%s 没配，%v 却显示了值", name, row)
				}
			case "keys":
				if asText(t, row["count"]) != "0" || len(names(t, row, "keys")) != 0 {
					t.Errorf("%s 没配，自由键却非空：%v", name, row)
				}
			default:
				// 零值结构 marshal 之后：没有 omitempty 的字符串格是 `""`，
				// 指针格（`port` / `disable_web_page_preview`）是 `null`。
				// 两者都念成"没填"，界面上摆一个空的输入框。
				switch value := row["value"]; value {
				case nil, "":
				default:
					t.Errorf("%s 没配，%v 却带了一个值", name, row)
				}
			}
		}
	}
	// 值类型三种控件各钉一格（这三格在 core 里分别是 *int / *bool / string）：
	// 判据只写"有没有值"就分不出它们，而界面要按它选数字框 / 开关 / 文本框。
	if got := mustText(t, fieldView(t, channelView(t, view, "tgBot"), "disable_web_page_preview"), "type"); got != "boolean" {
		t.Errorf("disable_web_page_preview 应是 boolean，实得 %q", got)
	}
	if got := mustText(t, fieldView(t, channelView(t, view, "email"), "port"), "type"); got != "number" {
		t.Errorf("port 应是 number，实得 %q", got)
	}
	if got := mustText(t, fieldView(t, channelView(t, view, "bark"), "level"), "type"); got != "string" {
		t.Errorf("level 应是 string，实得 %q", got)
	}

	// customPost：清单出在这一家这一层，条目里那一份与它同序同数（同一条表）。
	custom := channelView(t, view, "customPost")
	items, _ := custom["items"].([]any)
	if len(items) != 1 {
		t.Fatalf("customPost 应有一条，实得 %v", custom["items"])
	}
	row, _ := items[0].(map[string]any)
	if strings.Join(fieldNames(t, row), ",") != strings.Join(fieldNames(t, custom), ",") {
		t.Errorf("条目字段清单与这一家的不一致：%v / %v", fieldNames(t, row), fieldNames(t, custom))
	}
	// 反向：字段表里新增一格而视图没跟上，上面那条"清单 == 表"就会红（把表里删一项即红）。
	if got := fieldNames(t, channelView(t, view, "tgBot")); got[0] != "api_host" {
		t.Errorf("tgBot 应按字段表原序（api_host 在 token 前），实得 %v", got)
	}
}
