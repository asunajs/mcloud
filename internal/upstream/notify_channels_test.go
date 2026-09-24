package upstream

// 七家"一次请求"渠道的**形状**用例。
//
// ⚠ 先说清这一组测的是哪一半 truth：本仓库**没有**任何一条第三方推送的实拍，
// 所以这些断言钉的是"我们发出去的就是我们想发的那一条"（与旧 TS 字面量对齐），
// **不**钉"那一家认这一条"。后者要靠真发一次补（方法写在 `notify.go` 抬头）。
//
// 每一例都挑"错了不会报错、只会静默变形"的那一格：
// URL 里的 token、正文放错了键名、默认值方向反过来、编码少一步。

import (
	"encoding/json"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// requestBody 把请求体解成 map（这些 body 全是我们自己拼的 JSON）。
func requestBody(t *testing.T, request core.HTTPRequest) map[string]any {
	t.Helper()
	var body map[string]any
	if err := json.Unmarshal(request.Body, &body); err != nil {
		t.Fatalf("请求体不是 JSON 对象：%v（原文 %q）", err, request.Body)
	}
	return body
}

// noError 是"构造不该失败"这一句的展开。写成小助手是为了让每个用例里
// 少两行样板，而不是把 `t.Fatalf` 藏进断言函数里。
func noError(t *testing.T, err *core.Error) {
	t.Helper()
	if err != nil {
		t.Fatalf("构造失败：%v", err)
	}
}

func TestPushplusRequestCarriesTokenInBodyNotURL(t *testing.T) {
	request, err := PushplusRequest(
		core.PushplusConfig{Token: core.NewSecretValue("PP-TOKEN")}, "标题", "正文一\n正文二")
	noError(t, err)
	if request.URL != "https://www.pushplus.plus/send" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	body := requestBody(t, request)
	if body["token"] != "PP-TOKEN" {
		t.Errorf("token 应在请求体里，实得 %v", body["token"])
	}
	if body["title"] != "标题" || body["content"] != "正文一\n正文二" {
		t.Errorf("标题/正文字段名应是 title/content，实得 %v / %v", body["title"], body["content"])
	}
	// 这一家的正文键叫 `content`（不是 `text`）。拼错的表现是
	// pushplus 回 200、消息却只有标题 —— 判据完全看不出来。
	if _, exists := body["text"]; exists {
		t.Error("pushplus 没有 text 这个键")
	}
	if got := headerValue(request.Headers, "content-type"); got != "application/json" {
		t.Errorf("应显式带 content-type，实得 %q", got)
	}
}

// TestExtraOverridesTheSameKeyLikeTheOldSpread 钉"Extra 在最后展开"这个方向。
//
// 旧实现每家都是 `{我们拼的..., ...config}`。反过来（我们先展开）会让用户
// 配的 `topic` / `sound` 生效、但**永远改不动**正文 —— 那是另一种行为，
// 而且界面上一句"你配的 content 被忽略了"都没写。
func TestExtraOverridesTheSameKeyLikeTheOldSpread(t *testing.T) {
	cfg := core.PushplusConfig{
		Token: "PP",
		Extra: map[string]any{"content": "用户自己写的正文", "topic": "group-a"},
	}
	request, err := PushplusRequest(cfg, "标题", "我们拼的正文")
	noError(t, err)
	body := requestBody(t, request)
	if body["content"] != "用户自己写的正文" {
		t.Errorf("Extra 应覆盖同名键，实得 %v", body["content"])
	}
	if body["topic"] != "group-a" {
		t.Errorf("未知的自由键应原样并进请求体，实得 %v", body["topic"])
	}
}

// TestMergeExtraReportsOverriddenKeysSorted 钉两件事：
// 报了哪些键、以及**顺序稳定**（`Extra` 是 map，不排序的话同一份配置
// 两次跑出的结论文案不一样，那是最难复现的一类偶发）。
func TestMergeExtraReportsOverriddenKeysSorted(t *testing.T) {
	body := map[string]any{"token": "A", "title": "t", "content": "c"}
	overridden := mergeExtra(body, map[string]any{
		"token": "B", "content": "C2", "title": "T2", "topic": "new",
	})
	if strings.Join(overridden, ",") != "content,title" {
		t.Errorf("应按键名排序报出被覆盖的三个中的两个（token 同键例外），实得 %v", overridden)
	}
	if body["token"] != "B" || body["topic"] != "new" {
		t.Errorf("报不报都要照样并进去，实得 %v / %v", body["token"], body["topic"])
	}
	// token 那一格例外只免"报"，不免"并"：漏并会让用户换 token 换不掉。
}

func TestServerChanRequestPutsTokenInThePathAndDoublesNewlines(t *testing.T) {
	request, err := ServerChanRequest(
		core.ServerChanConfig{Token: core.NewSecretValue("SCT123")}, "标题", "一行\n两行")
	noError(t, err)
	if request.URL != "https://sctapi.ftqq.com/SCT123.send" {
		t.Errorf("token 应在路径上，实得 %q", request.URL)
	}
	body := requestBody(t, request)
	// Server酱的两个键名与直觉相反：`text` 才是标题。旧实现两个键都给标题。
	if body["text"] != "标题" || body["title"] != "标题" {
		t.Errorf("text 与 title 都应是标题，实得 %v / %v", body["text"], body["title"])
	}
	if body["desp"] != "一行\n\n两行" {
		t.Errorf("正文换行要翻倍（markdown 渲染），实得 %q", body["desp"])
	}
}

func TestWorkWeixinBotRequestUsesMsgTypeAsTheContentKey(t *testing.T) {
	request, err := WorkWeixinBotRequest(
		core.WorkWeixinBotConfig{URL: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=K"},
		"标题", "正文")
	noError(t, err)
	if request.URL != "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=K" {
		t.Errorf("这一家的地址整个由用户给，实得 %q", request.URL)
	}
	body := requestBody(t, request)
	if body["msgtype"] != "text" {
		t.Errorf("缺省 msgtype 应是 text，实得 %v", body["msgtype"])
	}
	textBlock, ok := body["text"].(map[string]any)
	if !ok {
		t.Fatalf("正文应挂在 msgtype 同名键下，实得 %T", body["text"])
	}
	if textBlock["content"] != "标题\n\n正文" {
		t.Errorf("这一家把标题与正文合成一条 content，实得 %q", textBlock["content"])
	}

	markdownRequest, err := WorkWeixinBotRequest(
		core.WorkWeixinBotConfig{URL: "https://x.invalid/h", MsgType: "markdown"}, "T", "B")
	noError(t, err)
	markdown := requestBody(t, markdownRequest)
	if _, isText := markdown["text"]; isText {
		t.Error("msgtype=markdown 时正文不该还挂在 text 下面")
	}
	block, ok := markdown["markdown"].(map[string]any)
	if !ok || block["content"] != "T\n\nB" {
		t.Errorf("markdown 那一档的键名要跟着 msgtype 走，实得 %v", markdown["markdown"])
	}
}

func TestTGBotRequestDefaultsPreviewDisabledAndChatIDInBody(t *testing.T) {
	request, err := TGBotRequest(
		core.TGBotConfig{Token: "123:ABC", ChatID: "-1001"}, "标题", "正文")
	noError(t, err)
	if request.URL != "https://api.telegram.org/bot123:ABC/sendMessage" {
		t.Errorf("默认主机与路径实得 %q", request.URL)
	}
	body := requestBody(t, request)
	// ⚠ 默认值方向：旧实现是 `disable_web_page_preview = true`。
	// 写成 Go 习惯的"没填就 false"，用户会突然开始收到带大图的推送。
	if body["disable_web_page_preview"] != true {
		t.Errorf("缺省应禁掉链接预览，实得 %v", body["disable_web_page_preview"])
	}
	if body["chat_id"] != "-1001" {
		t.Errorf("chat_id 应在请求体里，实得 %v", body["chat_id"])
	}
	if body["text"] != "标题\n\n正文" {
		t.Errorf("这一家也是合成一条 text，实得 %q", body["text"])
	}

	explicit, err := TGBotRequest(
		core.TGBotConfig{Token: "t", ChatID: "1", DisableWebPagePreview: boolPtr(false)}, "T", "B")
	noError(t, err)
	if requestBody(t, explicit)["disable_web_page_preview"] != false {
		t.Error("显式填 false 必须真的发 false（不能被默认值吃掉）")
	}

	custom, err := TGBotRequest(
		core.TGBotConfig{APIHost: "tg.example.invalid:8443", Token: "t", ChatID: "1"}, "T", "B")
	noError(t, err)
	if custom.URL != "https://tg.example.invalid:8443/bott/sendMessage" {
		t.Errorf("自建反代主机应被用上，实得 %q", custom.URL)
	}

	// 空白主机 = 没填，回落到默认（不留下 `https:///bot…` 这种半截 URL）。
	blank, err := TGBotRequest(
		core.TGBotConfig{APIHost: "   ", Token: "t", ChatID: "1"}, "T", "B")
	noError(t, err)
	if !strings.HasPrefix(blank.URL, "https://api.telegram.org/bot") {
		t.Errorf("空白主机应回落默认，实得 %q", blank.URL)
	}
}

func TestBarkRequestPutsTextUnderBodyKeyNotContent(t *testing.T) {
	request, err := BarkRequest(core.BarkConfig{Key: "KK"}, "标题", "正文")
	noError(t, err)
	if request.URL != "https://api.day.app/KK" {
		t.Errorf("key 应在路径上，实得 %q", request.URL)
	}
	body := requestBody(t, request)
	if body["body"] != "正文" {
		t.Errorf("Bark 的正文键叫 body，实得 %v", body["body"])
	}
	if _, exists := body["content"]; exists {
		t.Error("Bark 没有 content 这个键")
	}
	if body["level"] != "passive" {
		t.Errorf("缺省 level 应是 passive（旧实现的默认），实得 %v", body["level"])
	}
	raised, err := BarkRequest(core.BarkConfig{Key: "K", Level: "active"}, "T", "B")
	noError(t, err)
	if requestBody(t, raised)["level"] != "active" {
		t.Error("显式 level 要被用上")
	}
}

func TestTwoIMRequestSplitsQueryAndBody(t *testing.T) {
	cfg := core.TwoIMConfig{
		Key:   core.NewSecretValue("K"),
		SID:   core.NewSecretValue("S"),
		Query: map[string]any{"plat": "server", "group": "g1"},
	}
	request, err := TwoIMRequest(cfg, "标题", "正文")
	noError(t, err)
	if !strings.HasPrefix(request.URL, "https://api.2im.cn/push?") {
		t.Fatalf("基址实得 %q", request.URL)
	}
	query := strings.SplitN(request.URL, "?", 2)[1]
	// key 与 sid 必须在查询串上（这一家不认请求体里的那两个）。
	for _, want := range []string{"key=K", "sid=S", "group=g1", "plat=server"} {
		if !strings.Contains(query, want) {
			t.Errorf("查询串里应有 %s，实得 %q", want, query)
		}
	}
	body := requestBody(t, request)
	if body["content"] != "正文" || body["title"] != "标题" {
		t.Errorf("正文应进请求体的 content（这一家不合成），实得 %v / %v", body["title"], body["content"])
	}
	if body["msgtype"] != "text" {
		t.Errorf("缺省 msgtype 应是 text，实得 %v", body["msgtype"])
	}
	// 键序必须稳定：`url.Values.Encode()` 按键名排序，而旧版 URLSearchParams 是
	// append 序 —— 差异只在字符串形态，钉住它是为了让"每次都不一样"这一类
	// 偶发不会从这一家冒出来。
	for round := 0; round < 3; round++ {
		again, err := TwoIMRequest(cfg, "标题", "正文")
		noError(t, err)
		if again.URL != request.URL {
			t.Fatalf("同一份配置两次的 URL 不一样：\n%q\n%q", request.URL, again.URL)
		}
	}
}

// TestDingTalkSignedURLMatchesTheReferenceVector ⭐ 本文件最硬的一例。
//
// 期望值**不是**从我们代码里跑出来的，是独立用 python 的 hmac 算出来的：
//
//	base64(hmac_sha256(key=secret, msg=f"{timestamp}\\n{secret}"))
//
// 反向改坏它的样子：把 [core.HmacSHA256Base64] 的两个参数顺序照 crypto-js 的
// 写法 `(message, key)` 传 —— 编译过、本仓其余测试全绿、钉钉回 `sign not match`。
// 这一家是本族唯一有签名的一步，也是唯一能靠"参数顺序"这一格悄悄坏掉的一步。
func TestDingTalkSignedURLMatchesTheReferenceVector(t *testing.T) {
	cases := []struct {
		timestamp int64
		secret    string
		wantSign  string // urlencode 之后的形态
	}{
		{1700000000000, "SECabc123", "N5P09a4%2Bp1AMJIJWnIvQd2Yxw9%2Bfu%2FoEBnPrjCcsLXk%3D"},
		{1, "s3cr3t", "xtCOFtiHl6HB8oNfoqF%2BCj94zOsZbnmuoxOBCO2Yfk8%3D"},
	}
	for _, c := range cases {
		got := DingTalkSignedURL(core.NewSecretValue("TOKEN"), core.NewSecretValue(c.secret), c.timestamp)
		want := "https://oapi.dingtalk.com/robot/send?access_token=TOKEN" +
			"&timestamp=" + strconv.FormatInt(c.timestamp, 10) +
			"&sign=" + c.wantSign
		if got != want {
			t.Errorf("timestamp=%d\n应得 %q\n实得 %q", c.timestamp, want, got)
		}
	}
}

// TestDingTalkWithoutSecretSendsNoSign 钉"没配 secret 就不加签"那一支：
// 查询串里不许留下空的 `sign=`（把"不匹配"与"没配"混成同一种形态，
// 界面上就再也分不出该去补 secret 还是该去查时间戳）。
func TestDingTalkWithoutSecretSendsNoSign(t *testing.T) {
	got := DingTalkSignedURL(core.NewSecretValue("T"), core.NewSecretValue("  "), 1700000000000)
	if strings.Contains(got, "sign=") || strings.Contains(got, "timestamp=") {
		t.Errorf("空白 secret 不该加签，实得 %q", got)
	}
	if !strings.Contains(got, "access_token=T") {
		t.Errorf("token 仍要在查询串上，实得 %q", got)
	}

	request, err := DingTalkRequest(
		core.DingTalkConfig{Token: "T"}, "标题", "正文", 1700000000000)
	noError(t, err)
	body := requestBody(t, request)
	if body["msgtype"] != "text" {
		t.Errorf("这一家钉死 msgtype=text（旧实现如此），实得 %v", body["msgtype"])
	}
	textBlock, ok := body["text"].(map[string]any)
	if !ok {
		t.Fatalf("正文应在 text.content，实得 %T", body["text"])
	}
	// ⚠ 与旧实现差一个前导空格：它是笔误，会出现在用户收到的每条消息开头。
	if textBlock["content"] != "标题\n\n正文" {
		t.Errorf("正文不该带前导空格，实得 %q", textBlock["content"])
	}
}

func TestCustomPostRequestEscapesPlaceholdersInURLButNotInBody(t *testing.T) {
	cfg := core.CustomPostConfig{
		URL:  "https://hook.example.invalid/{title}?t={text}",
		Body: map[string]any{"note": "{title}", "n": 1},
	}
	request, err := CustomPostRequest(cfg, "A&B", "x y")
	noError(t, err)
	// URL 里的占位符要编码：不编码时正文里一个 `&` 就会新增一个查询参数（注入）。
	if request.URL != "https://hook.example.invalid/A%26B?t=x%20y" {
		t.Errorf("URL 侧应做 percent 编码，实得 %q", request.URL)
	}
	body := requestBody(t, request)
	// 请求体里的占位符是"文本上替换后重新解析"（旧实现如此）⇒ 值原样进字符串。
	if body["note"] != "A&B" {
		t.Errorf("请求体里的值不该被编码，实得 %v", body["note"])
	}
	if body["n"] != float64(1) {
		t.Errorf("非字符串字段要原样保留，实得 %v", body["n"])
	}
}

func TestCustomPostRequestRejectsUnparseableBodyEmptyURLAndUnknownMethod(t *testing.T) {
	// 正文里带裸引号 ⇒ 替换后的 JSON 文本坏了。旧实现让 JSON.parse 抛，
	// 这一格必须**报出来**：静默跳过等于"用户配了一条永远不发的 webhook"。
	_, err := CustomPostRequest(
		core.CustomPostConfig{URL: "https://x.invalid", Body: map[string]any{"k": "{text}"}},
		"t", "带\"引号\"的正文")
	if err == nil {
		t.Fatal("正文里的裸引号应让这一条构造失败")
	}
	if _, err := CustomPostRequest(core.CustomPostConfig{URL: "  "}, "t", "b"); err == nil {
		t.Error("空 URL 该被拒（旧实现是静默 return，那是这一族最难查的静默）")
	}
	if _, err := CustomPostRequest(
		core.CustomPostConfig{URL: "https://x.invalid", Method: "PATCH"}, "t", "b"); err == nil {
		t.Error("第五种方法不该被兜成 POST")
	}
}

func TestCustomPostRequestAppliesMethodHeadersAndTimeout(t *testing.T) {
	timeout := int64(3000)
	request, err := CustomPostRequest(core.CustomPostConfig{
		Method: "get", URL: "https://x.invalid/h",
		Headers: map[string]string{"x-token": "T"}, Timeout: &timeout,
	}, "t", "b")
	noError(t, err)
	if request.Method != core.MethodGet {
		t.Errorf("方法应大小写不敏感地认 GET，实得 %v", request.Method)
	}
	if headerValue(request.Headers, "x-token") != "T" {
		t.Errorf("用户配的头要带上，实得 %v", request.Headers)
	}
	if request.TimeoutMS == nil || *request.TimeoutMS != 3000 {
		t.Errorf("超时应是 3000ms，实得 %v", request.TimeoutMS)
	}
	if len(request.Body) != 0 {
		t.Errorf("GET 且没有 data/body 时不该有请求体，实得 %q", request.Body)
	}
}

// TestCustomPostMethodRejectsFifthForm 单列一支：`""` 与 `PATCH` 是两种东西 ——
// 前者是"没填"（旧实现兜成 POST），后者是"填了我们不认的"（必须报）。
// 把两者都兜成 POST 的表现：一条本来要 PATCH 的自建 webhook 静默变成 POST。
func TestCustomPostMethodRejectsFifthForm(t *testing.T) {
	if _, ok := customPostMethod(""); !ok {
		t.Error("空方法应落到 POST")
	}
	if kind, ok := customPostMethod(" put "); !ok || kind != core.MethodPut {
		t.Errorf("PUT 该被认出来，实得 %v / %v", kind, ok)
	}
	if _, ok := customPostMethod("PATCH"); ok {
		t.Error("PATCH 不在旧实现的四种里，不该被接受")
	}
	if _, ok := customPostMethod("post"); !ok {
		t.Error("小写方法名要被接受（旧实现 toUpperCase 过）")
	}
}

func TestCustomPostDataWinsOverBody(t *testing.T) {
	request, err := CustomPostRequest(core.CustomPostConfig{
		URL:  "https://x.invalid",
		Data: map[string]any{"from": "data"},
		Body: map[string]any{"from": "body"},
	}, "t", "b")
	noError(t, err)
	if got := requestBody(t, request)["from"]; got != "data" {
		t.Errorf("data 应优先于 body，实得 %v", got)
	}
}

func boolPtr(v bool) *bool { return &v }
