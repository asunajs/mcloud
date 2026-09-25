package upstream

// 139 邮箱会话那一族的**协议层**用例（玩法 472 / 473 的前置）。
//
// # 这一族钉什么、不钉什么
//
// 四枪里只有 ①② 有实拍，而 ② **只有请求面**（响应 200 + gzip + `text/javascript`，
// 体未捕获）⇒ 本文件没有任何一条断言在钉"上游真的回 `var.sid` / `var.rmkey`"。
// ③④ 连请求面都是旧 TS 那一份（`autologin.action` 与 `taskClickV2` 在本仓 har/夹具里
// 逐份扫过，零命中）⇒ 钉的也是"发得出 + 形状与出处逐字一致"。
//
// 全部用例钉五类**可控**的事：
//
//  1. **实拍的请求面不许在翻译中被"顺手统一"** —— 六键的顺序、`eMode` / `clientIp`
//     这两枚旧 TS 没有的键、`?cguid=`、`application/xml; charset=utf-8` 那个**空格**与
//     小写 `utf-8`、以及**不带 authorization**。
//  2. **转义那一格不能是死代码** —— 票的形状由上游定，成文只记录过"hex"这一例；
//     带 `&` 的票必须解出**同样六键**而不是把结构打断。
//  3. **凭据只进不出** —— `ssoToken` / `sid` / `rmkey` 三枚都不许出现在错误文本里，
//     包括 ③ 那一发（凭据在 URL 里，而传输层错误原样嵌 URL）与"上游把票 echo 回 summary"
//     那一档。反向改坏：删掉 [redactError] ⇒ 那两条红。
//  4. **脱敏不能洗掉类别** —— 一次 `90001` 被重建后必须仍是"会话失效"，一次 5xx 必须
//     仍带回状态码。这一类是 [redactError] 存在的唯一理由：`notify` 那一族交回的是纯文本，
//     这里交回的是要按类别决定重放 / 重试的 [core.Error]。
//  5. **`activityid` 跟着 `marketname` 走，且同名只留一枚** —— 先剥后补，
//     不是再 append（两枚同名头会让"哪枚生效"变成传输层的事）。
//
// # 凭据边界
//
// 三枚凭据一律用合成哨兵串；成文与断言里都不出现真实票（`YZsidssolg…` 那两枚实拍值
// 在 `docs/capture/` 里也已按 §2.1 掩码）。

import (
	"context"
	"encoding/json"
	"sort"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	mailSynthSSO       = "YZsidssolgsynthsynthsynthsynthsynth1"
	mailSynthCguid     = "11112222-3333-4444-5555-666677778888"
	mailSynthSID       = "00synthSID000000000000000000ABCD"
	mailSynthRMKey     = "synth+rmkey/with=chars"
	mailSynthAuth      = "Basic U1lOVEhFVElDLUFVVEg="
	mailSynthJWT       = "synth-jwt-token"
	mailSynthReferer   = "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin"
	mailSynthTaskID    = int64(472)
	mailSynthErr       = "dial tcp 10.0.0.1:443: connect: connection refused"
	mailSynthByteSenti = "SYNTH-BODY-SENTINEL-绝不该出现在错误文本里"
)

func mailHeader(request core.HTTPRequest, name string) (string, bool) {
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1], true
		}
	}
	return "", false
}

func mailHeaderCount(request core.HTTPRequest, name string) int {
	n := 0
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			n++
		}
	}
	return n
}

func mailHeaderNames(request core.HTTPRequest) []string {
	names := make([]string, 0, len(request.Headers))
	for _, pair := range request.Headers {
		names = append(names, strings.ToLower(pair[0]))
	}
	sort.Strings(names)
	return names
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ② 枪：inlogin 的请求面（实拍那一份）
// ─────────────────────────────────────────────────────────────────────────────

// TestMailInLoginRequestMatchesCapture 钉 URL、六键单行体与那五枚头。
//
// 反向改坏：把体改回旧 TS 那份（四键 + 换行缩进）⇒ 这一条红；去掉 `?cguid=` ⇒ 红；
// 给 `content-type` 补 authorization ⇒ 红。
func TestMailInLoginRequestMatchesCapture(t *testing.T) {
	request, perr := MailInLoginRequest(mailSynthSSO, mailSynthCguid)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if request.Method != core.MethodPost {
		t.Errorf("方法应是 POST，实得 %q", request.Method)
	}
	wantURL := "https://mail.10086.cn/login/inlogin.action?cguid=" + mailSynthCguid
	if request.URL != wantURL {
		t.Errorf("URL 实得 %q，期望 %q", request.URL, wantURL)
	}
	wantBody := `<?xml version="1.0" encoding="utf-8"?><object>` +
		`<string name="clientId">10804</string>` +
		`<string name="version">9</string>` +
		`<string name="loginType">7</string>` +
		`<string name="eMode">1</string>` +
		`<string name="token">` + mailSynthSSO + `</string>` +
		`<string name="clientIp"></string></object>`
	if string(request.Body) != wantBody {
		t.Errorf("体应与实拍逐字一致\n实得 %s\n期望 %s", request.Body, wantBody)
	}
	// 实拍那一发**只有**这五枚应用层头（`sec-fetch-*` / `priority` / `accept-encoding`
	// 是浏览器与传输层自动加的，本层不手写）。
	if got := strings.Join(mailHeaderNames(request), ","); got !=
		"accept,accept-language,content-type,origin,user-agent" {
		t.Errorf("头集合实得 %s", got)
	}
	if ct, _ := mailHeader(request, "content-type"); ct != "application/xml; charset=utf-8" {
		t.Errorf("content-type 应保留实拍的空格与小写 utf-8，实得 %q", ct)
	}
	if origin, _ := mailHeader(request, "origin"); origin != "mcloudlocal://yun.139.com" {
		t.Errorf("origin 实得 %q", origin)
	}
	if _, has := mailHeader(request, "authorization"); has {
		t.Error("实拍那一发不带 authorization：带上它等于发一份上游没收过的请求")
	}
	if _, has := mailHeader(request, "cookie"); has {
		t.Error("不该有 cookie：这一发正是为了**换**会话，手里还没有")
	}
}

// TestMailInLoginEscapesToken 钉"票里出现 XML 元字符时结构仍然完好"。
//
// 今天的票是 `YZsidssolg` + hex，所以这一格看起来用不到；但它不是死代码：
// 票的字符集由上游定，而成文只记录过见过的两例。少了转义，一张带 `&` 的票会把体
// 拼成一份结构不同的 XML，表现是"②一直失败"而错误指不到源头。
func TestMailInLoginEscapesToken(t *testing.T) {
	request, perr := MailInLoginRequest("a&b<c>\"d'e", mailSynthCguid)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	body := string(request.Body)
	for _, want := range []string{"a&amp;b&lt;c&gt;&quot;d&apos;e"} {
		if !strings.Contains(body, want) {
			t.Errorf("体里应出现转义后的票，实得 %s", body)
		}
	}
	// 转义后的体里 `<string name="clientIp">` 必须还在：那一句是"结构没被打断"的读数。
	if strings.Count(body, "<string name=") != 6 {
		t.Errorf("应有六枚 <string name=>，实得 %d（体: %s）", strings.Count(body, "<string name="), body)
	}
}

// TestMailInLoginRequiresBothInputs 钉两枚入参缺失时**一枪不发**。
func TestMailInLoginRequiresBothInputs(t *testing.T) {
	for _, c := range []struct{ name, token, cguid string }{
		{"缺票", "", mailSynthCguid},
		{"缺 cguid", mailSynthSSO, ""},
	} {
		_, perr := MailInLoginRequest(c.token, c.cguid)
		if perr == nil {
			t.Errorf("%s：应报错而不是发出去", c.name)
			continue
		}
		// 错误文本里不许出现票本身（它是凭据）。
		if strings.Contains(perr.Error(), mailSynthSSO) {
			t.Errorf("%s：错误文本泄漏票：%s", c.name, perr.Error())
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ② 枪：响应解析（形状零样本 ⇒ 判据只有"两枚都在"）
// ─────────────────────────────────────────────────────────────────────────────

func mailResponse(status uint16, body string) core.HTTPResponse {
	return core.HTTPResponse{
		Status:  status,
		Headers: []core.HeaderPair{{"content-type", "text/javascript;charset=UTF-8"}},
		Body:    core.Body(body),
	}
}

func TestParseMailSessionHappy(t *testing.T) {
	session, perr := ParseMailSession(mailResponse(200,
		`{"code":"S_OK","var":{"sid":"`+mailSynthSID+`","rmkey":"`+mailSynthRMKey+`"}}`),
		mailSynthSSO)
	if perr != nil {
		t.Fatalf("应解出会话: %v", perr.Message())
	}
	if session.SID.Value() != mailSynthSID || session.RMKey.Value() != mailSynthRMKey {
		t.Errorf("两枚值实得 %s / %s", session.SID.Value(), session.RMKey.Value())
	}
	if !session.OK() {
		t.Error("凑齐两枚时 OK 应为 true")
	}
	// ⚠ 打印必须是掩码：这一句挡的是"将来有人把整块会话塞进日志"。
	if strings.Contains(session.SID.String(), mailSynthSID) ||
		strings.Contains(session.RMKey.String(), mailSynthRMKey) {
		t.Error("凭据打印未掩码")
	}
}

// TestParseMailSessionLooseQuotes 钉同族那一套单引号 JSON 的兜底解得开。
//
// ⚠ 这一格兜的是 `RmWeb` 那族（`appmail.mail.10086.cn` 回单引号 JSON）的**惯例外推**：
// 2026-09-26 探针真跑过 ② 之后，inlogin 那一口的真实响应已确认是**标准 JSON**
// （`docs/capture/app-mail139-samples.txt` §2 ⇒ 严解即成），所以这一支对**当前的唯一调用点**
// 是"用不上但不能没有"的兜底。它兜的仍是"严解失败"那一条路径 ⇒ 标成用例而不是删掉：
// 删了它，兜底那一遍就成了未被断言的死路。
func TestParseMailSessionLooseQuotes(t *testing.T) {
	session, perr := ParseMailSession(mailResponse(200,
		`{'code':'S_OK','var':{'sid':'`+mailSynthSID+`','rmkey':'`+mailSynthRMKey+`'}}`),
		mailSynthSSO)
	if perr != nil {
		t.Fatalf("单引号那一份应解得开: %v", perr.Message())
	}
	if session.SID.Value() != mailSynthSID {
		t.Errorf("实得 %s", session.SID.Value())
	}
}

// TestParseMailSessionFailureTextLeaksNothing 钉失败文本：说得出缺哪枚，但不 echo 凭据与体。
func TestParseMailSessionFailureTextLeaksNothing(t *testing.T) {
	cases := []struct {
		name      string
		body      string
		wantPiece string
	}{
		{"缺 rmkey", `{"code":"S_FAIL","summary":"no rmkey","var":{"sid":"` + mailSynthSID + `"}}`,
			"var.rmkey"},
		{"var 不是对象", `{"code":"S_FAIL","var":"x"}`, "var.sid 与 var.rmkey"},
		{"解不开", mailSynthByteSenti, "解不开"},
	}
	for _, c := range cases {
		_, perr := ParseMailSession(mailResponse(200, c.body), mailSynthSSO)
		if perr == nil {
			t.Errorf("%s：应报错", c.name)
			continue
		}
		text := perr.Error()
		if !strings.Contains(text, c.wantPiece) {
			t.Errorf("%s：错误文本应含 %q，实得 %s", c.name, c.wantPiece, text)
		}
		if strings.Contains(text, mailSynthSID) || strings.Contains(text, mailSynthRMKey) {
			t.Errorf("%s：错误文本泄漏凭据：%s", c.name, text)
		}
		if strings.Contains(text, mailSynthByteSenti) {
			t.Errorf("%s：错误文本 echo 了响应体：%s", c.name, text)
		}
		if c.name == "解不开" && !strings.Contains(text, "单引号") {
			t.Errorf("解不开那一句应说明两种都试过，实得 %s", text)
		}
	}
}

// TestParseMailSessionRedactsEchoedToken 钉"上游把提交过去的票 echo 进 summary"时被抹掉。
//
// 这一档有先例（§2.1.1：`200050409 token 已失效`），所以不是防御性代码。
func TestParseMailSessionRedactsEchoedToken(t *testing.T) {
	_, perr := ParseMailSession(mailResponse(200,
		`{"code":"S_FAIL","summary":"token 已失效 `+mailSynthSSO+`","var":{}}`),
		mailSynthSSO)
	if perr == nil {
		t.Fatal("应报错")
	}
	if strings.Contains(perr.Error(), mailSynthSSO) {
		t.Errorf("错误文本泄漏票：%s", perr.Error())
	}
	if !strings.Contains(perr.Error(), "S_FAIL") {
		t.Errorf("应带上游自己说的 code，实得 %s", perr.Error())
	}
}

// TestParseMailSessionListsTopLevelKeys 钉"解开了却没有 var"时交回键名。
//
// ⚠ 这一行**断的是升序**，不是"两个键各自在不在"：`topLevelKeys` 早先按 Go 的 map
// 遍历顺序 join，于是同一份响应会交出 `msg,data` 也会交出 `data,msg` —— 用例连着
// 全仓门禁都能"运气好时绿"，而**把暂存树单独跑一遍**就红了（那一趟整包 48.7s 的
// 全量 test 恰好没红）。这一串是要**进日志与错误文本**的，顺序不稳定等于同一件事
// 有两种写法 ⇒ 实现里排序（`sort.Strings`），这里把排序钉住。坑表 **#166**。
func TestParseMailSessionListsTopLevelKeys(t *testing.T) {
	_, perr := ParseMailSession(mailResponse(403, `{"msg":"blocked","data":null}`), mailSynthSSO)
	if perr == nil {
		t.Fatal("应报错")
	}
	if !strings.Contains(perr.Error(), "data,msg") {
		t.Errorf("应按键名升序列出顶层键名，实得 %s", perr.Error())
	}
	if !strings.Contains(perr.Error(), "403") {
		t.Errorf("应带 HTTP 状态，实得 %s", perr.Error())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ③ 枪：autologin（凭据在 URL 里）
// ─────────────────────────────────────────────────────────────────────────────

// TestMailAutoLoginRequestShape 钉编码后的查询串与 authorization 的可选。
func TestMailAutoLoginRequestShape(t *testing.T) {
	session := MailSession{SID: core.NewSecretValue(mailSynthSID), RMKey: core.NewSecretValue(mailSynthRMKey)}
	request, perr := MailAutoLoginRequest(session, mailSynthAuth)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if request.Method != core.MethodGet {
		t.Errorf("方法应是 GET，实得 %q", request.Method)
	}
	// `rmkey` 含 `+ / =`：不编码时 `+` 在查询串里表示空格 ⇒ 服务端解成另一个值且不报错。
	if !strings.Contains(request.URL, "rmkey=synth%2Brmkey%2Fwith%3Dchars") {
		t.Errorf("rmkey 应被百分号编码，实得 %s", request.URL)
	}
	if !strings.HasPrefix(request.URL, "https://mail.10086.cn/login/autologin.action?sid=") {
		t.Errorf("URL 前缀实得 %s", request.URL)
	}
	if auth, _ := mailHeader(request, "authorization"); auth != mailSynthAuth {
		t.Errorf("这一口零实拍，唯一出处（旧 TS 经主实例）带的就是 authorization ⇒ 默认要带上")
	}
	if _, has := mailHeader(request, "cookie"); has {
		t.Error("不该带 cookie：旧实现在这一发也是裸 GET，靠的是查询串")
	}
	// authorization 传空 ⇒ 不带那一枚（由调用方决定，本层不猜也不拦）。
	bare, perr := MailAutoLoginRequest(session, "")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if _, has := mailHeader(bare, "authorization"); has {
		t.Error("传空就不该带 authorization")
	}
	// 缺一枚 ⇒ 不发。
	if _, perr := MailAutoLoginRequest(MailSession{SID: session.SID}, mailSynthAuth); perr == nil {
		t.Error("缺 rmkey 时应拒绝发出")
	}
}

// mailFailingClient 是"回一个传输层错误"的假客户端。
//
// ⚠ 错误文本按 `internal/io` 的真实形状写成 `Get "<完整 URL>": …` —— 那一句里含 URL，
// 而这一族的 URL 里有两枚凭据。测试必须复刻这个形状，否则钉不住泄漏路径。
type mailFailingClient struct {
	err    error
	sent   []core.HTTPRequest
	result core.HTTPResponse
}

func (c *mailFailingClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.sent = append(c.sent, request)
	if c.err != nil {
		return core.HTTPResponse{}, c.err
	}
	return c.result, nil
}

// TestAppMailAutoLoginRedactsURLSecrets 钉 ③ 的错误文本里没有两枚凭据，**而类别还在**。
func TestAppMailAutoLoginRedactsURLSecrets(t *testing.T) {
	session := MailSession{SID: core.NewSecretValue(mailSynthSID), RMKey: core.NewSecretValue(mailSynthRMKey)}
	request, perr := MailAutoLoginRequest(session, mailSynthAuth)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	client := &mailFailingClient{err: core.NewNetwork(`Get "` + request.URL + `": ` + mailSynthErr)}
	up := NewUpstream(client)
	got := up.AppMailAutoLogin(context.Background(), session, mailSynthAuth)
	if got == nil {
		t.Fatal("传输层错误应交回")
	}
	text := got.Error()
	if strings.Contains(text, mailSynthSID) || strings.Contains(text, mailSynthRMKey) {
		t.Errorf("错误文本泄漏查询串里的凭据：%s", text)
	}
	if got.Kind() != core.KindNetwork {
		t.Errorf("类别必须保住（重试策略按它判），实得 %s", got.Kind())
	}
	if !strings.Contains(text, mailSynthErr) {
		t.Errorf("真正有用的那半句（传输层原因）不该被抹掉：%s", text)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第 ④ 枪：taskClickV2
// ─────────────────────────────────────────────────────────────────────────────

// TestMailMarketTaskClickRequestShape 钉 :7071 那口：体三键、头集合、activityid 只留一枚。
func TestMailMarketTaskClickRequestShape(t *testing.T) {
	request, perr := MailMarketTaskClickRequest(
		mailSynthJWT, mailSynthReferer, MarketName, mailSynthTaskID)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if request.URL != "https://caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskClickV2" {
		t.Errorf("URL 实得 %s（:7071 与路径都不能省/换）", request.URL)
	}
	var obj map[string]any
	if err := json.Unmarshal(request.Body, &obj); err != nil {
		t.Fatalf("体应是 JSON: %v", err)
	}
	keys := make([]string, 0, len(obj))
	for key := range obj {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	if got := strings.Join(keys, ","); got != "clientVersion,marketname,taskId" {
		t.Errorf("体里应只有那三键，实得 %s", got)
	}
	if obj["marketname"] != MarketName || obj["clientVersion"] != CaiyunAppVersion ||
		obj["taskId"] != float64(mailSynthTaskID) {
		t.Errorf("三键面值实得 %v", obj)
	}
	if jwt, _ := mailHeader(request, "jwttoken"); jwt != mailSynthJWT {
		t.Errorf("应带小写 jwttoken（公共来源，见函数头），实得 %q", jwt)
	}
	if id, _ := mailHeader(request, "activityid"); id != MarketName {
		t.Errorf("activityid 应跟着本次的 marketname，实得 %q", id)
	}
	if n := mailHeaderCount(request, "activityid"); n != 1 {
		t.Errorf("同名头只能有一枚，实得 %d（先剥后补，不是再 append）", n)
	}
	if ct, _ := mailHeader(request, "content-type"); !strings.HasPrefix(ct, "application/json") {
		t.Errorf("这一口发 JSON，content-type 实得 %q", ct)
	}
	// 邮箱市场名那一档也要能发（1004 用的就是它 ⇒ 不是端常量）。
	other, perr := MailMarketTaskClickRequest(
		mailSynthJWT, mailSynthReferer, TaskListV2EmailMarketName, 1004)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if id, _ := mailHeader(other, "activityid"); id != TaskListV2EmailMarketName {
		t.Errorf("第二个市场名也应跟着走，实得 %q", id)
	}
}

// TestMailMarketTaskClickRejectsBadInputs 钉 id<=0 与空市场名**不发试探请求**。
func TestMailMarketTaskClickRejectsBadInputs(t *testing.T) {
	if _, perr := MailMarketTaskClickRequest(mailSynthJWT, mailSynthReferer, MarketName, 0); perr == nil {
		t.Error("id=0 的打卡不该发出去")
	}
	if _, perr := MailMarketTaskClickRequest(mailSynthJWT, mailSynthReferer, "", 472); perr == nil {
		t.Error("marketname 没有默认值：两处调用点用的是两个不同值")
	}
}

// TestAppMailMarketTaskClickFailsOnNonZeroCode 钉"上报失败不静默"。
//
// 旧实现把这一发的响应整个吞掉（`tryRequest`）。本层看 `code`：非 0 即失败。
// ⚠ 别把这一条读成"非 0 = 这一条任务没做完"：同族四发打卡实拍只回 `code`/`msg`，
// 而兄弟口今天对 22 个 id 全回 `58999 任务不存在` ⇒ 非 0 码的语义仍未验证。
func TestAppMailMarketTaskClickFailsOnNonZeroCode(t *testing.T) {
	for _, c := range []struct {
		name      string
		body      string
		wantError bool
	}{{"成功", `{"code":0,"msg":"success"}`, false},
		{"非 0", `{"code":58999,"msg":"请尝试重新登录","detail":"任务不存在"}`, true},
	} {
		client := &mailFailingClient{result: core.HTTPResponse{
			Status: 200, Headers: []core.HeaderPair{{"content-type", "application/json"}},
			Body: core.Body(c.body)}}
		got := NewUpstream(client).AppMailMarketTaskClick(
			context.Background(), mailSynthJWT, mailSynthReferer, MarketName, mailSynthTaskID)
		if c.wantError && got == nil {
			t.Errorf("%s：应回错误", c.name)
		}
		if !c.wantError && got != nil {
			t.Errorf("%s：不该回错误: %v", c.name, got.Message())
		}
		if c.wantError && !strings.Contains(got.Error(), "任务不存在") {
			t.Errorf("失败文本应带上游 detail（坑表 #149），实得 %s", got.Error())
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 脱敏重建必须保住变体
// ─────────────────────────────────────────────────────────────────────────────

// TestRedactErrorKeepsVariant 钉四类错误重建后**类别与状态都还在**。
//
// 这一组是 [redactError] 与 `notify` 那套纯文本脱敏的分界线：调用链要按类别决定
// 重不重试、按"会话失效"决定刷不刷票。
func TestRedactErrorKeepsVariant(t *testing.T) {
	secret := core.NewSecretValue(mailSynthSID)
	cases := []struct {
		name     string
		in       *core.Error
		wantKind core.ErrorKind
		isSame   bool // 文本不含凭据 ⇒ 应原样交回同一个指针
		check    func(t *testing.T, got *core.Error)
	}{{
		name: "网络", in: core.NewNetwork("Get ?sid=" + mailSynthSID + ": refused"),
		wantKind: core.KindNetwork,
	}, {
		name: "会话失效", in: core.NewSessionExpired("90001 sid=" + mailSynthSID),
		wantKind: core.KindBadRequest,
		check: func(t *testing.T, got *core.Error) {
			t.Helper()
			if !got.IsSessionExpired() {
				t.Error("90001 重建后必须仍是会话失效，否则刷票那一半逻辑静默失效")
			}
		},
	}, {
		name: "上游 5xx", in: core.NewUpstream(502, "bad gateway sid="+mailSynthSID),
		wantKind: core.KindUpstream,
		check: func(t *testing.T, got *core.Error) {
			t.Helper()
			if got.Status() != 502 {
				t.Errorf("状态码应保住，实得 %d", got.Status())
			}
		},
	}, {
		name: "fixture 未命中", in: core.NewFixtureMiss("没这条夹具 sid=" + mailSynthSID),
		wantKind: core.KindInternal,
		check: func(t *testing.T, got *core.Error) {
			t.Helper()
			if !got.IsFixtureMiss() {
				t.Error("fixture 未命中必须仍认得出：那是挡'偷偷打真网'的那一条")
			}
		},
	}, {
		name: "无凭据时不动", in: core.NewParse("干净的错误"),
		wantKind: core.KindParse, isSame: true,
	}}
	for _, c := range cases {
		got := redactError(c.in, secret)
		if got == nil {
			t.Errorf("%s：不该交回 nil", c.name)
			continue
		}
		if c.isSame && got != c.in {
			t.Errorf("%s：文本里没有凭据时应原样交回同一个指针（不做无谓重建）", c.name)
		}
		if !c.isSame && strings.Contains(got.Error(), mailSynthSID) {
			t.Errorf("%s：泄漏凭据：%s", c.name, got.Error())
		}
		if got.Kind() != c.wantKind {
			t.Errorf("%s：类别实得 %s，期望 %s", c.name, got.Kind(), c.wantKind)
		}
		if c.check != nil {
			c.check(t, got)
		}
	}
	if redactError(nil, secret) != nil {
		t.Error("nil 应原样交回 nil")
	}
}

// TestMail139SourceIDIsItsOwn 钉这一族的 `toSourceId` **不是**签到族那一枚。
//
// 反向改坏：把它"统一"成 [ToSourceID] ⇒ 这一条红。成文依据是拍到的那条链
// （`{"toSourceId":"001003"}` 换到的票与 inlogin 体里的票逐字相同），
// 不是"看起来都能过"（`docs/capture/app-signin-samples.txt` §2.1 末尾那条纠错）。
func TestMail139SourceIDIsItsOwn(t *testing.T) {
	if Mail139SourceID != "001003" {
		t.Errorf("实得 %q", Mail139SourceID)
	}
	if Mail139SourceID == ToSourceID {
		t.Error("邮箱与签到是两个发起方，常量不许互搬")
	}
}
