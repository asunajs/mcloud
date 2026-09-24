package upstream

// 那三家"结论不是一个请求的成败"的：企业微信两步、customPost 多条、email 一封。
//
// 与 `notify_channels_test.go` 的分工：那一个钉**单个请求的形状**（纯函数、可逐字节断言），
// 这一个钉**多次发送的编排** —— 顺序、只打几枪、多条怎么合成一条。
// 假客户端 [notifyClient] 在脚本打光后直接 panic，所以"少打一枪"与"多打一枪"都会当场红。

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

func weixinChannels() core.NotifyChannels {
	return core.NotifyChannels{WorkWeixin: &core.WorkWeixinConfig{
		CorpID: core.NewSecretValue("mycorp"), CorpSecret: core.NewSecretValue("mysecret"),
	}}
}

func TestWorkWeixinSendsTokenExchangeThenMessage(t *testing.T) {
	client := &notifyClient{script: []replyScript{
		{status: 200, body: `{"errcode":0,"access_token":"TOK-1","expires_in":7200}`},
		{status: 200, body: `{"errcode":0,"errmsg":"ok"}`},
	}}
	report := NewNotifySender(client, nil, nil).
		SendAll(context.Background(), weixinChannels(), "标题", "正文")

	if len(client.requests) != 2 {
		t.Fatalf("两步恰好两枪，实得 %d 枪：%v", len(client.requests), client.urls())
	}
	first := client.requests[0]
	if !strings.HasSuffix(first.URL, "/gettoken") {
		t.Errorf("第一枪应是换票：%s", first.URL)
	}
	tokenBody := requestBody(t, first)
	if tokenBody["corpid"] != "mycorp" || tokenBody["corpsecret"] != "mysecret" {
		t.Errorf("换票体实得 %v", tokenBody)
	}

	second := client.requests[1]
	if !strings.Contains(second.URL, "access_token=TOK-1") {
		t.Errorf("第二枪要带换到的票，实得 %s", second.URL)
	}
	sendBody := requestBody(t, second)
	if sendBody["touser"] != "@all" {
		t.Errorf("不填收件人应是全员（旧实现的解构默认值），实得 %v", sendBody["touser"])
	}
	if sendBody["msgtype"] != "text" {
		t.Errorf("msgtype 默认 text，实得 %v", sendBody["msgtype"])
	}
	textBlock, ok := sendBody["text"].(map[string]any)
	if !ok || textBlock["content"] != "标题\n\n正文" {
		t.Errorf("正文挂在 msgtype 同名键下，实得 %v", sendBody["text"])
	}
	if _, present := sendBody["agentid"]; present {
		t.Errorf("没配 agentid 就不该有这个键：%v", sendBody)
	}
	if report[0].Outcome != NotifySent {
		t.Fatalf("两家结论实得 %+v", report)
	}
}

func TestWorkWeixinTokenExchangeFailureSendsNoSecondShot(t *testing.T) {
	// 五种"没换到票"的样子，全都不许打第二枪：拿着空 access_token 去发消息，
	// 上游回的那句与"corpid 错了"长得一样，会把排障的人引到错的方向。
	cases := []struct {
		name       string
		reply      replyScript
		wantReason string
	}{
		{"响应里没有 access_token", replyScript{status: 200, body: `{"errcode":0}`}, "没有 access_token"},
		{"换票回了非 0 errcode", replyScript{status: 200, body: `{"errcode":40013,"errmsg":"invalid corpid"}`}, "invalid corpid"},
		{"换票响应不是 JSON", replyScript{status: 200, body: `<html>`}, "不是 JSON"},
		{"换票 HTTP 非 2xx", replyScript{status: 502, body: `{}`}, "换票 HTTP 502"},
		{"换票没发出去", replyScript{err: fmt.Errorf("dial tcp: connection refused")}, "请求未发出"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			client := &notifyClient{script: []replyScript{c.reply}}
			report := NewNotifySender(client, nil, nil).
				SendAll(context.Background(), weixinChannels(), "标题", "正文")
			if len(client.requests) != 1 {
				t.Fatalf("换票失败后不得打第二枪，实得 %d 枪：%v", len(client.requests), client.urls())
			}
			one := report[0]
			if one.Outcome != NotifyFailed {
				t.Fatalf("这一家要判失败，实得 %+v", one)
			}
			if !strings.Contains(one.Reason, c.wantReason) {
				t.Errorf("原因实得 %q，期望含 %q", one.Reason, c.wantReason)
			}
		})
	}
}

func TestWorkWeixinAccessTokenIsEmptyStringTreatedAsMissing(t *testing.T) {
	// `{"access_token":""}` 是这一族最阴的一格：键在、值是空串。
	client := &notifyClient{script: []replyScript{
		{status: 200, body: `{"errcode":0,"access_token":""}`},
		{status: 200, body: `{"errcode":0}`},
	}}
	report := NewNotifySender(client, nil, nil).
		SendAll(context.Background(), weixinChannels(), "标题", "正文")
	if len(client.requests) != 1 {
		t.Fatalf("空票不能拿去发消息，实得 %d 枪：%v", len(client.requests), client.urls())
	}
	if report[0].Outcome != NotifyFailed {
		t.Fatalf("实得 %+v", report[0])
	}
}

func TestWorkWeixinTouserMsgTypeAndAgentIDFollowTheConfig(t *testing.T) {
	agentID := int64(1000002)
	channels := core.NotifyChannels{WorkWeixin: &core.WorkWeixinConfig{
		CorpID: core.NewSecretValue("c"), CorpSecret: core.NewSecretValue("s"),
		ToUser: "userA|userB", MsgType: "markdown", AgentID: &agentID,
	}}
	client := &notifyClient{script: []replyScript{
		{status: 200, body: `{"errcode":0,"access_token":"T"}`},
		{status: 200, body: `{"errcode":0}`},
	}}
	report := NewNotifySender(client, nil, nil).SendAll(context.Background(), channels, "标题", "正文")
	if report[0].Outcome != NotifySent {
		t.Fatalf("实得 %+v", report[0])
	}
	body := requestBody(t, client.requests[1])
	if body["touser"] != "userA|userB" {
		t.Errorf("touser 实得 %v", body["touser"])
	}
	if body["agentid"] != float64(agentID) {
		t.Errorf("agentid 实得 %v", body["agentid"])
	}
	// 配 markdown 时正文挂在 `markdown` 下面（旧实现那一句"msgtype 决定键名"保留）。
	block, ok := body["markdown"].(map[string]any)
	if !ok || block["content"] != "标题\n\n正文" {
		t.Errorf("非 text 的 msgtype 键名实得 %v", body)
	}
	if _, present := body["text"]; present {
		t.Errorf("配了 markdown 就不该再有 text 键：%v", body)
	}
}

func TestWorkWeixinBotIsOneShotAndNeedsNoToken(t *testing.T) {
	// 群机器人（workWeixinBot）与上面那一家是**两条不同的路**：一个 key 一枪，不换票。
	// 这一格容易写串，因为两家的名字只差一个后缀。
	client := &notifyClient{script: []replyScript{{status: 200, body: `{"errcode":0,"errmsg":"ok"}`}}}
	report := NewNotifySender(client, nil, nil).SendAll(context.Background(), core.NotifyChannels{
		WorkWeixinBot: &core.WorkWeixinBotConfig{URL: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=botkey"},
	}, "标题", "正文")
	if len(client.requests) != 1 {
		t.Fatalf("群机器人只有一枪，实得 %d", len(client.requests))
	}
	if report[0].Channel != core.NotifyWorkWeixinBot || report[0].Outcome != NotifySent {
		t.Fatalf("实得 %+v", report[0])
	}
	if strings.Contains(client.requests[0].URL, "gettoken") {
		t.Errorf("群机器人不换票：%s", client.requests[0].URL)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// customPost：一家多条合成一条
// ─────────────────────────────────────────────────────────────────────────────

func TestCustomPostsMergeIntoOneResultPerChannel(t *testing.T) {
	client := &notifyClient{script: []replyScript{
		{status: 200, body: `{"ok":true}`},
		{status: 500, body: `boom`},
		{status: 200, body: `{}`},
	}}
	channels := core.NotifyChannels{CustomPost: []core.CustomPostConfig{
		{URL: "https://a.test/hook"}, {URL: "https://b.test/hook"}, {URL: "https://c.test/hook"},
	}}
	report := NewNotifySender(client, nil, nil).SendAll(context.Background(), channels, "标题", "正文")

	if len(report) != 1 {
		t.Fatalf("一家只出一条结论（三条详情并进 Reason），实得 %d 条：%+v", len(report), report)
	}
	one := report[0]
	if one.Outcome != NotifyFailed {
		t.Fatalf("有一条失败这一家就判失败，实得 %+v", one)
	}
	want := "第 1 条已发；第 2 条 HTTP 500；第 3 条已发"
	if one.Reason != want {
		t.Errorf("详情实得 %q\n期望 %q", one.Reason, want)
	}
}

func TestCustomPostOnlyLooksAtTheStatusNotTheThreeKeys(t *testing.T) {
	// 通用 webhook 的响应形状由用户自己定：回 HTML、回 `{"code":"OK"}` 的都在这一家。
	// 套那三键会把"其实发成功了"判成失败，所以这一家刻意不套。
	client := &notifyClient{script: []replyScript{{status: 200, body: `<html>queued</html>`}}}
	report := NewNotifySender(client, nil, nil).SendAll(context.Background(), core.NotifyChannels{
		CustomPost: []core.CustomPostConfig{{URL: "https://hook.test/x"}},
	}, "标题", "正文")
	if report[0].Outcome != NotifySent {
		t.Fatalf("只看状态码 ⇒ 这一条应算已发，实得 %+v", report[0])
	}
}

func TestCustomPostConstructionFailureIsCountedPerItem(t *testing.T) {
	client := &notifyClient{script: []replyScript{{status: 200, body: `{}`}}}
	channels := core.NotifyChannels{CustomPost: []core.CustomPostConfig{
		{URL: "https://a.test/x", Method: "PATCH"}, // 四种方法之外，构造即失败
		{URL: "https://b.test/x"},
	}}
	report := NewNotifySender(client, nil, nil).SendAll(context.Background(), channels, "标题", "正文")
	if report[0].Outcome != NotifyFailed {
		t.Fatalf("实得 %+v", report[0])
	}
	if !strings.Contains(report[0].Reason, "第 1 条构造失败") {
		t.Errorf("实得 %q", report[0].Reason)
	}
	if len(client.requests) != 1 {
		t.Fatalf("构造失败那条不该发出去，实得 %d 枪：%v", len(client.requests), client.urls())
	}
	if !strings.Contains(client.requests[0].URL, "b.test") {
		t.Errorf("后面的条仍要打出去，实得 %v", client.urls())
	}
}

func TestCustomPostsReasonIsClippedNotUnbounded(t *testing.T) {
	// 一家配二十条时 Reason 会撑爆 run_events 那一列。
	var items []core.CustomPostConfig
	var script []replyScript
	for i := 0; i < 20; i++ {
		items = append(items, core.CustomPostConfig{URL: fmt.Sprintf("https://a%d.test/x", i)})
		script = append(script, replyScript{status: 599, body: `{}`})
	}
	client := &notifyClient{script: script}
	report := NewNotifySender(client, nil, nil).SendAll(context.Background(),
		core.NotifyChannels{CustomPost: items}, "标题", "正文")
	if len([]rune(report[0].Reason)) > 201 {
		t.Errorf("Reason 没截断：%d 个字符", len([]rune(report[0].Reason)))
	}
	if !strings.HasSuffix(report[0].Reason, "…") {
		t.Errorf("截断处要留省略号，实得结尾 %q", report[0].Reason[len(report[0].Reason)-3:])
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// email：没有 HTTP 请求可看，只能把那一封本身导出来断言
// ─────────────────────────────────────────────────────────────────────────────

func TestEmailMessageShape(t *testing.T) {
	port := 587
	cfg := core.EmailConfig{
		Host: "smtp.example.test", Port: &port,
		From: core.NewSecretValue("me@example.test"), Pass: core.NewSecretValue("authcode"),
		To: "a@example.test；b@example.test",
	}
	message := EmailMessage(cfg, "标题", "正文")
	if message.Host != "smtp.example.test" || message.Port != 587 {
		t.Errorf("地址与端口实得 %s:%d", message.Host, message.Port)
	}
	if message.Username.Value() != "me@example.test" || message.Password.Value() != "authcode" {
		t.Errorf("登录名与授权码要原样传给发送器：%+v", message)
	}
	// 打印出来必须是脱敏的：这一句会进日志（三条路 %v / %+v / %#v 都要堵）。
	for _, printed := range []string{fmt.Sprintf("%v", message), fmt.Sprintf("%+v", message), fmt.Sprintf("%#v", message)} {
		if strings.Contains(printed, "authcode") {
			t.Errorf("打印泄漏授权码：%s", printed)
		}
	}
	if !strings.Contains(message.String(), "redacted") {
		t.Errorf("打印里要看得出\"凭据在、只是不给看\"，实得 %s", message)
	}
	if got := EmailMessage(cfg, "标题", "正文里那段运行数据"); strings.Contains(fmt.Sprintf("%v", got), "运行数据") {
		t.Errorf("正文只该报长度，实得 %s", got)
	}
	if !strings.Contains(message.String(), "Text:<2 字符>") {
		t.Errorf("正文按字符数报长度（两个汉字不能成 6 字节），实得 %s", message.String())
	}
	if message.From != "标题 <me@example.test>" {
		t.Errorf("From 的显示名拼法照旧实现，实得 %q", message.From)
	}
	if message.Subject != "标题" || message.Text != "正文" {
		t.Errorf("主题/正文实得 %q / %q", message.Subject, message.Text)
	}
	if got := core.MailRecipients(message.To); len(got) != 2 {
		t.Errorf("收件人应拆成两个，实得 %v", got)
	}

	// 不填端口 ⇒ 走隐式 TLS 的 465（那一对是绑在一起的，见 core/mail.go）。
	if zero := EmailMessage(core.EmailConfig{Host: "h"}, "标题", "正文"); zero.Port != 0 {
		t.Errorf("没填端口时结构体里留 0，由 EffectivePort 换算，实得 %d", zero.Port)
	} else if zero.EffectivePort() != core.MailDefaultPort || !zero.ImplicitTLS() {
		t.Errorf("465 / 隐式 TLS 应同时成立，实得 %d / %v", zero.EffectivePort(), zero.ImplicitTLS())
	}
}

func TestEmailSenderReceivesExactlyOneMessagePerRun(t *testing.T) {
	mails := &mailScript{}
	channels := core.NotifyChannels{Email: allTenChannelsConfigured().Email}
	report := NewNotifySender(&notifyClient{}, mails, nil).
		SendAll(context.Background(), channels, "标题", "正文")
	if len(mails.mails) != 1 || len(report) != 1 {
		t.Fatalf("只配 email 时：一封一条，实得 %d 封 / %d 条", len(mails.mails), len(report))
	}
	if mails.mails[0].message.Text != "正文" {
		t.Errorf("正文实得 %q", mails.mails[0].message.Text)
	}
}
