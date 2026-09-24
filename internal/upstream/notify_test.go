// 发送层的分派与记账：[classifyChannelReply] 的判定表 + [NotifySender.SendAll] 的
// "一家一枪、失败不断后续"。
//
// ⚠ 这一族**十家一家真响应体都没有**（本仓所有归档里没有任何第三方推送响应），
// 所以这些用例钉的是"与旧实现同口径 + 明显判反的那一格修掉 + 我们自己定的三档记账"，
// **不**钉"某家真回什么"。补样方法见 `notify.go` 抬头。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 两件假物件：按脚本回放的 HTTP 客户端、记录收到的邮件发送器
// ─────────────────────────────────────────────────────────────────────────────

// replyScript 是"下一枪回这个"的一格脚本。
type replyScript struct {
	status uint16
	body   string
	err    error
}

// notifyClient 按脚本逐枪回放，并留下收到的请求。
//
// 与 `upstream_test.go` 的 `scriptedClient` 区分开：那一个只会"每次都回同一份"，
// 而这一族要断的正是"一家失败后后面几家还发不发"、"企业微信那两枪的顺序"，
// 脚本必须是**逐枪**的。
type notifyClient struct {
	script   []replyScript
	requests []core.HTTPRequest
}

func (c *notifyClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	index := len(c.requests) - 1
	if index >= len(c.script) {
		// 脚本打光还继续发 ⇒ 立刻失败：这一族的"多发一枪"（企业微信换票失败后
		// 还去发消息、customPost 漏算一条）正是要钉住的东西，静默回 200 会把它放过去。
		panic(fmt.Sprintf("推送脚本只有 %d 枪，第 %d 枪无脚本可回放", len(c.script), index+1))
	}
	one := c.script[index]
	if one.err != nil {
		return core.HTTPResponse{}, one.err
	}
	return core.NewResponse(one.status, []byte(one.body)), nil
}

func (c *notifyClient) urls() []string {
	var out []string
	for _, request := range c.requests {
		out = append(out, request.URL)
	}
	return out
}

type sentMail struct {
	message core.MailMessage
	err     error
}

type mailScript struct {
	mails []sentMail
	err   error
}

func (m *mailScript) SendMail(_ context.Context, message core.MailMessage) error {
	m.mails = append(m.mails, sentMail{message: message, err: m.err})
	return m.err
}

// ─────────────────────────────────────────────────────────────────────────────
// 公共判据
// ─────────────────────────────────────────────────────────────────────────────

func TestClassifyChannelReplyFollowsTheOldThreeKeys(t *testing.T) {
	cases := []struct {
		name       string
		body       string
		wantOK     bool
		wantReason string // 子串；空串表示"只要判对档，不钉那句"
	}{
		{"errcode 为 0 算成功", `{"errcode":0,"errmsg":"ok"}`, true, "ok"},
		{"errcode 非 0 算失败", `{"errcode":40001,"errmsg":"invalid secret"}`, false, "errcode=40001"},
		{"code 0 算成功", `{"code":0,"msg":"success"}`, true, "success"},
		{"code 200 算成功", `{"code":200}`, true, ""},
		{"三个键都没有算成功（旧口径 undefined）", `{}`, true, ""},
		{"只有 message 键算成功", `{"message":"queued"}`, true, "queued"},
		{"code 非 0 非 200 算失败", `{"code":500,"msg":"gateway"}`, false, "gateway"},
		{"code 显式 null 算失败", `{"code":null}`, false, ""},
		{"err 有值算失败", `{"err":"boom"}`, false, "boom"},
		{"err 为 null 算成功", `{"err":null}`, true, ""},
		{"err 为空串算成功", `{"err":""}`, true, ""},
		{"err 为 0 算成功", `{"err":0}`, true, ""},
		// 这一支是"刻意不同于旧实现"的那一格：Bot API 失败回 ok=false + error_code，
		// 旧实现那三键都命不中 ⇒ 一次 401 会被念成"已发送"。
		{"ok=false 带 error_code 算失败", `{"ok":false,"error_code":401,"description":"Unauthorized"}`, false, "Unauthorized"},
		{"ok=true 算成功", `{"ok":true,"result":{"message_id":1}}`, true, ""},
		// ok 优先于三键：tg 的失败体里常常同时有 error_code 而没有 code。
		{"ok=false 优先于 code", `{"ok":false,"code":200}`, false, ""},
		{"非 JSON 判失败而不是判成功", `<html>502 Bad Gateway</html>`, false, "响应不是 JSON"},
		{"数组判失败", `["ok"]`, false, "响应不是对象"},
		{"空体判失败", ``, false, "响应不是 JSON"},
		// ⚠ 比旧实现松的一格（见 classifyChannelReply 头注）：旧版 `code === c` 严格相等，
		// 字符串 "200" 判失败；这里按数字认。方向是"少报一次失败"，登记在待拍板表里。
		{"字符串码按数字认（与旧实现不同，松）", `{"code":"200"}`, true, ""},
		{"字符串码非 0 仍判失败", `{"code":"500"}`, false, ""},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			reason, ok := classifyChannelReply([]byte(c.body))
			if ok != c.wantOK {
				t.Fatalf("判档实得 ok=%v（原因 %q），期望 %v", ok, reason, c.wantOK)
			}
			if c.wantReason != "" && !strings.Contains(reason, c.wantReason) {
				t.Errorf("原因实得 %q，期望含 %q", reason, c.wantReason)
			}
		})
	}
}

func TestClassifyChannelReplyPicksTheFirstHumanMessageKey(t *testing.T) {
	// 键序是钉死的：某些失败体同时带 `errmsg` 与 `msg`，只该念前面那一句。
	cases := []struct {
		body string
		want string
	}{
		{`{"errcode":1,"errmsg":"第一","msg":"第二","message":"第三"}`, "第一"},
		{`{"ok":false,"msg":"第二","message":"第三"}`, "第二"},
		{`{"ok":false,"message":"第三","description":"第四"}`, "第三"},
		{`{"ok":false,"description":"第四","error":"第五"}`, "第四"},
		{`{"ok":false,"error":"第五"}`, "第五"},
		{`{"ok":false,"errmsg":"   ","msg":"空白跳过"}`, "空白跳过"},
		{`{"ok":false}`, ""},
	}
	for _, c := range cases {
		reason, _ := classifyChannelReply([]byte(c.body))
		if !strings.Contains(reason, c.want) {
			t.Errorf("体 %s ⇒ 原因 %q，期望含 %q", c.body, reason, c.want)
		}
	}
}

func TestClassifyChannelReplyClipsTheMessageByRunesNotBytes(t *testing.T) {
	// 中文网关会把整页 HTML 塞进 msg；不截断会撑爆 run_events 那一列。
	// 按**字符**截：按字节切会把汉字切成乱码尾巴。
	long := strings.Repeat("云", 300)
	body, err := json.Marshal(map[string]any{"ok": false, "description": long})
	if err != nil {
		t.Fatal(err)
	}
	_, ok := classifyChannelReply(body)
	if ok {
		t.Fatal("ok=false 必须判失败")
	}
	reason, _ := classifyChannelReply(body)
	// 前缀"渠道回了 ok=false："是中文，尾巴省略号一个；正文只留 120 个字符。
	if !strings.HasSuffix(reason, "…") {
		t.Errorf("期望以省略号收尾，实得 %q", reason)
	}
	if strings.Contains(reason, strings.Repeat("云", 121)) {
		t.Errorf("没有截到 120 个字符：%q", reason)
	}
	if strings.ContainsAny(reason, "é") || strings.Contains(reason, "\ufffd") {
		t.Errorf("出现被切开的字节：%q", reason)
	}
}

func TestClipRunesLeavesShortTextAlone(t *testing.T) {
	for _, in := range []string{"", "短", strings.Repeat("a", 120)} {
		if got := clipRunes(in, 120); got != in {
			t.Errorf("实得 %q，期望原样", got)
		}
	}
	if got := clipRunes(strings.Repeat("a", 121), 120); len([]rune(got)) != 121 {
		t.Errorf("截断后实得 %d 个字符（期望 120 + 省略号）", len([]rune(got)))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 分派：固定序、缺必填不发、一家失败不断后续
// ─────────────────────────────────────────────────────────────────────────────

func allTenChannelsConfigured() core.NotifyChannels {
	return core.NotifyChannels{
		Pushplus:      &core.PushplusConfig{Token: core.NewSecretValue("push-token")},
		ServerChan:    &core.ServerChanConfig{Token: core.NewSecretValue("scu-scrtoken")},
		WorkWeixin:    &core.WorkWeixinConfig{CorpID: core.NewSecretValue("corp"), CorpSecret: core.NewSecretValue("csecret")},
		WorkWeixinBot: &core.WorkWeixinBotConfig{URL: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=botkey"},
		TGBot:         &core.TGBotConfig{Token: core.NewSecretValue("123:abc"), ChatID: "42"},
		Bark:          &core.BarkConfig{Key: core.NewSecretValue("barkkey")},
		TwoIM:         &core.TwoIMConfig{Key: core.NewSecretValue("imkey"), SID: core.NewSecretValue("1")},
		DingTalk:      &core.DingTalkConfig{Token: core.NewSecretValue("dingtoken")},
		Email:         &core.EmailConfig{Host: "smtp.example.test", From: core.NewSecretValue("me@example.test"), Pass: core.NewSecretValue("authcode"), To: "you@example.test"},
		CustomPost:    []core.CustomPostConfig{{URL: "https://hook.example.test/x"}},
	}
}

func TestSendAllFollowsTheDeclaredChannelOrder(t *testing.T) {
	client := &notifyClient{}
	// 十家全配：email 不打 HTTP 枪、企业微信两枪 ⇒ 恰好 10 枪。脚本只给 10 格，
	// 多打一枪就 panic（那正是"换票失败还去发消息"这类错的表现）。
	for i := 0; i < 10; i++ {
		client.script = append(client.script, replyScript{status: 200, body: `{"errcode":0,"access_token":"tok"}`})
	}
	sender := NewNotifySender(client, &mailScript{}, core.NewFixedClock(time.UnixMilli(1700000000000)))
	report := sender.SendAll(context.Background(), allTenChannelsConfigured(), "标题", "正文")

	if len(client.requests) != 10 {
		t.Fatalf("十家应打 10 枪，实得 %d 枪", len(client.requests))
	}
	if len(report) != 10 {
		t.Fatalf("十家要十条结论，实得 %d", len(report))
	}
	for i, name := range core.NotifyChannelOrder() {
		if report[i].Channel != name {
			t.Errorf("第 %d 条实得 %s，期望 %s", i, report[i].Channel, name)
		}
	}
	// 声明序钉的是**配置字段的书写序**：这里故意按 NotifyChannelOrder 的顺序建配置，
	// 而真正的反向证据在下面那一条（打乱字面序，结论序不变）。
}

func TestSendAllOrderIsIndependentOfLiteralOrder(t *testing.T) {
	// 旧实现那一份是 `Object.entries(config)` 的书写序，Go 的 map 更无序。
	// 这里把配置**倒着**填，结论序必须仍是声明序。
	client := &notifyClient{}
	for i := 0; i < 3; i++ {
		client.script = append(client.script, replyScript{status: 200, body: `{"code":0}`})
	}
	sender := NewNotifySender(client, nil, nil)
	channels := core.NotifyChannels{
		Bark:       &core.BarkConfig{Key: core.NewSecretValue("k")},
		ServerChan: &core.ServerChanConfig{Token: core.NewSecretValue("t")},
		Pushplus:   &core.PushplusConfig{Token: core.NewSecretValue("p")},
	}
	report := sender.SendAll(context.Background(), channels, "标题", "正文")
	want := []core.NotifyChannel{core.NotifyPushplus, core.NotifyServerChan, core.NotifyBark}
	for i, one := range report {
		if one.Channel != want[i] {
			t.Fatalf("第 %d 条实得 %s，期望 %s（全表 %v）", i, one.Channel, want[i], report)
		}
	}
}

func TestSendAllSkipsMissingRequiredWithoutSending(t *testing.T) {
	client := &notifyClient{script: []replyScript{{status: 200, body: `{"code":200}`}}}
	sender := NewNotifySender(client, nil, nil)
	channels := core.NotifyChannels{
		Pushplus: &core.PushplusConfig{Token: core.NewSecretValue("")},
		Bark:     &core.BarkConfig{Key: core.NewSecretValue("k")},
	}
	report := sender.SendAll(context.Background(), channels, "标题", "正文")

	if len(report) != 2 {
		t.Fatalf("两家两条，实得 %d", len(report))
	}
	if report[0].Outcome != NotifySkipped {
		t.Errorf("空 token 应跳过，实得 %s", report[0].Outcome)
	}
	if !strings.Contains(report[0].Reason, "token") {
		t.Errorf("跳过原因要点名缺哪个字段，实得 %q", report[0].Reason)
	}
	if report[1].Outcome != NotifySent {
		t.Errorf("后一家仍要发，实得 %s（%q）", report[1].Outcome, report[1].Reason)
	}
	if len(client.requests) != 1 {
		t.Errorf("缺必填那家不得打一枪，实得 %d 枪", len(client.requests))
	}
}

func TestSendAllKeepsGoingAfterAFailure(t *testing.T) {
	// 第一家 500、第一家之后仍要发：这是"三家坏了表现成一家没收到"那条连带。
	client := &notifyClient{script: []replyScript{
		{status: 500, body: `server error`},
		{status: 200, body: `{"errcode":0,"errmsg":"ok"}`},
		{err: fmt.Errorf("dial tcp: no route to host")},
		{status: 200, body: `not json at all`},
	}}
	sender := NewNotifySender(client, nil, nil)
	channels := core.NotifyChannels{
		Pushplus:      &core.PushplusConfig{Token: core.NewSecretValue("p")},
		ServerChan:    &core.ServerChanConfig{Token: core.NewSecretValue("t")},
		WorkWeixinBot: &core.WorkWeixinBotConfig{URL: "https://hook.test/send?key=k"},
		Bark:          &core.BarkConfig{Key: core.NewSecretValue("k")},
	}
	report := sender.SendAll(context.Background(), channels, "标题", "正文")
	if len(report) != 4 || len(client.requests) != 4 {
		t.Fatalf("四家要四条结论四枪，实得 %d 条 / %d 枪", len(report), len(client.requests))
	}
	want := []NotifyOutcome{NotifyFailed, NotifySent, NotifyFailed, NotifyFailed}
	for i, outcome := range want {
		if report[i].Outcome != outcome {
			t.Errorf("第 %d 家(%s) 实得 %s，期望 %s（原因 %q）",
				i, report[i].Channel, report[i].Outcome, outcome, report[i].Reason)
		}
	}
	if !strings.Contains(report[0].Reason, "500") {
		t.Errorf("原因里要有状态码，实得 %q", report[0].Reason)
	}
	if !strings.Contains(report[3].Reason, "不是 JSON") {
		t.Errorf("非 JSON 响应要说明原因，实得 %q", report[3].Reason)
	}
}

func TestSendBuiltReportsFailureWhenTheRequestCannotBeBuilt(t *testing.T) {
	// 构造失败的一格用 Extra 复现：Extra 是用户自由键，并进请求体后由 `encoding/json`
	// 序列化，塞一个编不出来的值（chan）就是那条"这一家配置拼不出请求"的路。
	client := &notifyClient{}
	sender := NewNotifySender(client, nil, nil)
	channels := core.NotifyChannels{
		Pushplus: &core.PushplusConfig{
			Token: core.NewSecretValue("p"),
			Extra: map[string]any{"坏值": make(chan int)},
		},
	}
	report := sender.SendAll(context.Background(), channels, "标题", "正文")
	if len(report) != 1 || report[0].Outcome != NotifyFailed {
		t.Fatalf("构造失败要判这一家 failed，实得 %+v", report)
	}
	if len(client.requests) != 0 {
		t.Errorf("构造失败不该发出去，实得 %d 枪", len(client.requests))
	}
}

func TestSendAllDoesNotLeakTheURLIntoReason(t *testing.T) {
	// serverChan / bark / tgBot / twoIm 的密钥都在 URL 里，而 Reason 将来进 run_events。
	// 这一条钉的是"我们这边产生的那一句不含 URL 与密钥"。
	client := &notifyClient{script: []replyScript{{status: 200, body: `<html>oops</html>`}}}
	sender := NewNotifySender(client, nil, nil)
	channels := core.NotifyChannels{ServerChan: &core.ServerChanConfig{Token: core.NewSecretValue("scu-SECRETINURL")}}
	report := sender.SendAll(context.Background(), channels, "标题", "正文")
	line := fmt.Sprintf("%+v", report)
	if strings.Contains(line, "http") || strings.Contains(line, "scu-SECRETINURL") {
		t.Errorf("结论里出现了 URL 或密钥：%q", line)
	}
	// 传输层错误那一句只留类别：`io.Client` 的原文里带着完整 URL（含密钥），
	// 照抄就等于把密钥抄进 run_events 那张表。
	client2 := &notifyClient{script: []replyScript{{err: fmt.Errorf(`Get "https://api.day.app/SECRETKEY/标题": context deadline exceeded`)}}}
	report2 := NewNotifySender(client2, nil, nil).SendAll(context.Background(),
		core.NotifyChannels{Bark: &core.BarkConfig{Key: core.NewSecretValue("SECRETKEY")}}, "标题", "正文")
	if report2[0].Outcome != NotifyFailed {
		t.Fatalf("传输层错误要判失败，实得 %+v", report2[0])
	}
	if strings.Contains(report2[0].Reason, "SECRETKEY") || strings.Contains(report2[0].Reason, "api.day.app") {
		t.Errorf("原因里出现了密钥或主机：%q", report2[0].Reason)
	}
	if !strings.Contains(report2[0].Reason, "请求未发出") {
		t.Errorf("要看得出没发出去，实得 %q", report2[0].Reason)
	}

	// 本仓的 [core.Error] 走类别那一句。
	client3 := &notifyClient{script: []replyScript{{err: core.NewNetwork("dial tcp: no route")}}}
	report3 := NewNotifySender(client3, nil, nil).SendAll(context.Background(),
		core.NotifyChannels{ServerChan: &core.ServerChanConfig{Token: core.NewSecretValue("scu-KEY")}}, "标题", "正文")
	if report3[0].Reason != "请求未发出：网络异常" {
		t.Errorf("实得 %q", report3[0].Reason)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 时钟与钉钉
// ─────────────────────────────────────────────────────────────────────────────

func TestDingTalkWithoutSecretNeedsNoClock(t *testing.T) {
	client := &notifyClient{script: []replyScript{{status: 200, body: `{"errcode":0}`}}}
	sender := NewNotifySender(client, nil, nil) // 刻意不接时钟
	report := sender.SendAll(context.Background(), core.NotifyChannels{
		DingTalk: &core.DingTalkConfig{Token: core.NewSecretValue("dingtoken")},
	}, "标题", "正文")
	if report[0].Outcome != NotifySent {
		t.Fatalf("只填 webhook 地址不加签是常见配法，不能因没时钟判失败：%+v", report[0])
	}
	if strings.Contains(client.requests[0].URL, "sign=") || strings.Contains(client.requests[0].URL, "timestamp=") {
		t.Errorf("没配 secret 就不该加签：%s", client.requests[0].URL)
	}
}

func TestDingTalkWithSecretFailsWithoutAClock(t *testing.T) {
	client := &notifyClient{}
	sender := NewNotifySender(client, nil, nil)
	report := sender.SendAll(context.Background(), core.NotifyChannels{
		DingTalk: &core.DingTalkConfig{Token: core.NewSecretValue("dingtoken"), Secret: core.NewSecretValue("SECsecret")},
	}, "标题", "正文")
	if report[0].Outcome != NotifyFailed {
		t.Fatalf("带 secret 却没时钟 ⇒ 判 failed（不许偷偷拿系统时间），实得 %+v", report[0])
	}
	if len(client.requests) != 0 {
		t.Errorf("没有时刻就不该发这一枪")
	}
}

func TestDingTalkWithAClockSignsTheRequest(t *testing.T) {
	client := &notifyClient{script: []replyScript{{status: 200, body: `{"errcode":0}`}}}
	sender := NewNotifySender(client, nil, core.NewFixedClock(time.UnixMilli(1700000000000)))
	report := sender.SendAll(context.Background(), core.NotifyChannels{
		DingTalk: &core.DingTalkConfig{Token: core.NewSecretValue("dingtoken"), Secret: core.NewSecretValue("SECabc123")},
	}, "标题", "正文")
	if report[0].Outcome != NotifySent {
		t.Fatalf("实得 %+v", report[0])
	}
	target := client.requests[0].URL
	if !strings.Contains(target, "timestamp=1700000000000") {
		t.Errorf("时刻要来自注入的时钟：%s", target)
	}
	if !strings.Contains(target, "sign=N5P09a4%2Bp1AMJIJWnIvQd2Yxw9%2Bfu%2FoEBnPrjCcsLXk%3D") {
		t.Errorf("签名与参考向量不符：%s", target)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 邮件那一家：没接 SMTP 是 skipped，不是 failed
// ─────────────────────────────────────────────────────────────────────────────

// emailChannelsOf 每次**现造**一份配置：渠道结构体是值，但里面的 `*EmailConfig`
// 是指针 —— 一次 `complete := channels` 之后再改 `.Email.To`，改的是同一个对象，
// 上一个子用例的"清空收件人"会漏到下一个子用例里（表现成"该发的没发"）。
func emailChannelsOf(to string) core.NotifyChannels {
	return core.NotifyChannels{Email: &core.EmailConfig{
		Host: "smtp.example.test", From: core.NewSecretValue("me@example.test"),
		Pass: core.NewSecretValue("authcode"), To: to,
	}}
}

func TestEmailOutcomeBands(t *testing.T) {
	t.Run("没接发送器算跳过", func(t *testing.T) {
		report := NewNotifySender(&notifyClient{}, nil, nil).
			SendAll(context.Background(), emailChannelsOf("you@example.test"), "标题", "正文")
		if report[0].Outcome != NotifySkipped {
			t.Fatalf("实得 %+v", report[0])
		}
		if report[0].Reason != "本进程没接 SMTP 发送器" {
			t.Errorf("原因实得 %q", report[0].Reason)
		}
	})

	t.Run("没填收件人算跳过", func(t *testing.T) {
		report := NewNotifySender(&notifyClient{}, &mailScript{}, nil).
			SendAll(context.Background(), emailChannelsOf("  ; ， "), "标题", "正文")
		if report[0].Outcome != NotifySkipped || !strings.Contains(report[0].Reason, "收件人") {
			t.Fatalf("实得 %+v", report[0])
		}
	})

	t.Run("发了且没报错算成功", func(t *testing.T) {
		mails := &mailScript{}
		report := NewNotifySender(&notifyClient{}, mails, nil).
			SendAll(context.Background(), emailChannelsOf("you@example.test"), "标题", "正文")
		if report[0].Outcome != NotifySent {
			t.Fatalf("实得 %+v", report[0])
		}
		if len(mails.mails) != 1 {
			t.Fatalf("只发一封，实得 %d", len(mails.mails))
		}
		if mails.mails[0].message.Subject != "标题" {
			t.Errorf("主题实得 %q", mails.mails[0].message.Subject)
		}
	})

	t.Run("发送器报错算失败并带上那一句", func(t *testing.T) {
		mails := &mailScript{err: fmt.Errorf("auth failed: 535 invalid username or password")}
		report := NewNotifySender(&notifyClient{}, mails, nil).
			SendAll(context.Background(), emailChannelsOf("you@example.test"), "标题", "正文")
		if report[0].Outcome != NotifyFailed || !strings.Contains(report[0].Reason, "535") {
			t.Fatalf("实得 %+v", report[0])
		}
	})

	t.Run("授权码不出现在结论里", func(t *testing.T) {
		// 个别 SMTP 服务会把最后一条命令回显在错误应答里，而授权码就在那一条上。
		mails := &mailScript{err: fmt.Errorf("535 bad: AUTH LOGIN authcode PLAIN")}
		report := NewNotifySender(&notifyClient{}, mails, nil).
			SendAll(context.Background(), emailChannelsOf("you@example.test"), "标题", "正文")
		if strings.Contains(report[0].Reason, "authcode") {
			t.Errorf("授权码泄漏进结论：%q", report[0].Reason)
		}
		if !strings.Contains(report[0].Reason, "‹已脱敏›") {
			t.Errorf("应留脱敏占位，实得 %q", report[0].Reason)
		}
	})

	t.Run("必填不齐时连发送器都不碰", func(t *testing.T) {
		partial := core.NotifyChannels{Email: &core.EmailConfig{
			Host: "h", From: core.NewSecretValue("f"), Pass: core.NewSecretValue(""),
		}}
		mails := &mailScript{}
		report := NewNotifySender(&notifyClient{}, mails, nil).SendAll(context.Background(), partial, "标题", "正文")
		if report[0].Outcome != NotifySkipped || !strings.Contains(report[0].Reason, "pass") {
			t.Fatalf("实得 %+v", report[0])
		}
		if len(mails.mails) != 0 {
			t.Errorf("缺必填不该碰 SMTP，实得 %d 封", len(mails.mails))
		}
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 小结那一行
// ─────────────────────────────────────────────────────────────────────────────

func TestSummaryLineCountsEveryBandAndNamesFailures(t *testing.T) {
	report := NotifyReport{
		{Channel: core.NotifyPushplus, Outcome: NotifySent},
		{Channel: core.NotifyDingTalk, Outcome: NotifyFailed, Reason: "HTTP 502"},
		{Channel: core.NotifyBark, Outcome: NotifySkipped, Reason: "缺少必填项：key"},
		{Channel: core.NotifyTGBot, Outcome: NotifyFailed, Reason: "渠道回了 ok=false"},
	}
	got := report.SummaryLine()
	want := "已发 1 / 失败 2 / 跳过 1：dingTalk、tgBot"
	if got != want {
		t.Errorf("实得 %q\n期望 %q", got, want)
	}
}

func TestSummaryLineSaysSoWhenNothingIsConfigured(t *testing.T) {
	if got := (NotifyReport{}).SummaryLine(); got != "没配任何推送渠道" {
		t.Errorf("实得 %q", got)
	}
	// 全跳过也要报出三档：这一族最常见的现象就是"什么都没发出去"。
	only := NotifyReport{{Channel: core.NotifyEmail, Outcome: NotifySkipped}}
	if got := only.SummaryLine(); got != "已发 0 / 失败 0 / 跳过 1" {
		t.Errorf("实得 %q", got)
	}
}

func TestReportCountIsPerBand(t *testing.T) {
	report := NotifyReport{
		{Outcome: NotifySent}, {Outcome: NotifySent}, {Outcome: NotifyFailed},
	}
	if report.Count(NotifySent) != 2 || report.Count(NotifyFailed) != 1 || report.Count(NotifySkipped) != 0 {
		t.Errorf("分档计数实得 %+v", report)
	}
}
