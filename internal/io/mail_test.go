package mcloudio

// SMTP 实现的协议对拍 —— 打给本包那台进程内假服务器（`mail_fakeserver_test.go`）。
//
// # 这一组用例钉的是什么
//
// 全部是"**删掉这行代码，哪一枪会不对**"能答出来的那种：
// 端口与 TLS 模式的耦合有没有传到连接、机制怎么挑、没有 TLS 时到底登不登录、
// 信封与头部各自的字节、点填充、错误文本里有没有授权码。
//
// # 这一组用例**不**钉什么
//
// 真服务商的行为。假服务器是照 RFC 的最小形状写的，它说"PLAIN 优先"只证明
// 我们的分支照声明走，不证明 QQ / 163 真那么声明 —— 那一格仍缺实拍，
// 补法写在 `mail.go` 抬头。

import (
	"context"
	"encoding/base64"
	"io"
	"mime/quotedprintable"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

const (
	mailTestUser = "me@smtp.mcloud.test"
	mailTestPass = "授权码-secret-abc"
)

// mailTestMessage 是一封"形状齐"的信：中文标题、中文正文、两个收件人。
func mailTestMessage() core.MailMessage {
	return core.MailMessage{
		Host:     mailTestHost,
		Port:     465,
		Username: core.NewSecretValue(mailTestUser),
		Password: core.NewSecretValue(mailTestPass),
		From:     "mcloud 运行推送 <" + mailTestUser + ">",
		To:       "a@x.test, b@y.test",
		Subject:  "签到结果",
		Text:     "第一行\n第二行 带空格\n中文正文",
	}
}

// plainWorkingServer 是"什么都正常"那台：465 隐式 TLS、PLAIN+LOGIN 都收。
func plainWorkingServer(t *testing.T) *fakeSMTP {
	t.Helper()
	return startFakeSMTP(t, fakeSMTPOptions{
		implicit:   true,
		authMechs:  []string{"PLAIN", "LOGIN"},
		acceptAuth: true,
	})
}

// stubMailClock 钉住 `Date:` 那一行。
type stubMailClock struct{ at time.Time }

func (c stubMailClock) Now() time.Time { return c.at }

// ─────────────────────────────────────────────────────────────────────────────
// 两条 TLS 通路
// ─────────────────────────────────────────────────────────────────────────────

func TestImplicitTLS465SendsNoStartTLS(t *testing.T) {
	srv := plainWorkingServer(t)

	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("465 那一趟应该发出去：%v", err)
	}

	if got, want := srv.commandOrder(), "EHLO AUTH MAIL RCPT RCPT DATA QUIT"; got != want {
		t.Errorf("命令序列不符：\n实得 %s\n期望 %s", got, want)
	}
	if srv.seen("STARTTLS") {
		t.Error("隐式 TLS 的连接上不该再发 STARTTLS（对面会回 503，看起来像配置错了）")
	}
	// 端口 → 模式的耦合要**传到连接**那一层才算数（core 那边另有一组纯函数用例）。
	dials := srv.dialsSeen()
	if len(dials) != 1 || !dials[0].implicit || dials[0].port != 465 {
		t.Errorf("拨号模式应是 {implicit:true port:465}，实得 %+v", dials)
	}
	if user, pass := srv.creds(); user != mailTestUser || pass != mailTestPass {
		t.Errorf("AUTH PLAIN 解出来的凭据不符：%q / %q", user, pass)
	}
}

func TestPort25UpgradesWithStartTLSBeforeAuth(t *testing.T) {
	srv := startFakeSMTP(t, fakeSMTPOptions{
		advertiseStartTLS: true,
		authMechs:         []string{"PLAIN", "LOGIN"},
		acceptAuth:        true,
	})
	message := mailTestMessage()
	message.Port = 25

	if err := srv.senderFor(t, nil).SendMail(context.Background(), message); err != nil {
		t.Fatalf("587/25 那一趟应该发出去：%v", err)
	}

	// ⚠ AUTH 必须排在**第二个** EHLO 之后：STARTTLS 会把扩展表清空，
	// 拿升级前那份能力去发 AUTH 是"看着对、其实发了个对面已经不认的命令"。
	if got, want := srv.commandOrder(),
		"EHLO STARTTLS EHLO AUTH MAIL RCPT RCPT DATA QUIT"; got != want {
		t.Errorf("命令序列不符：\n实得 %s\n期望 %s", got, want)
	}
	if dials := srv.dialsSeen(); len(dials) != 1 || dials[0].implicit || dials[0].port != 25 {
		t.Errorf("端口 25 应该走明文起步，实得 %+v", dials)
	}
	if user, pass := srv.creds(); user != mailTestUser || pass != mailTestPass {
		t.Errorf("升级后的凭据不符：%q / %q", user, pass)
	}
}

func TestPlainServerWithPasswordRefusesToSendCredentials(t *testing.T) {
	// 这台**不**声明 STARTTLS：明文连接上发授权码等于把码上网。
	srv := startFakeSMTP(t, fakeSMTPOptions{authMechs: []string{"PLAIN"}, acceptAuth: true})

	err := srv.senderFor(t, nil).SendMail(context.Background(), func() core.MailMessage {
		m := mailTestMessage()
		m.Port = 25
		return m
	}())
	if err == nil {
		t.Fatal("没有 TLS 还照登：这一条必须失败")
	}
	if kind, _ := core.KindOf(err); kind != core.KindBadRequest {
		t.Errorf("这是配置不合法，不是网络问题，应该是 BadRequest，实得 %v", kind)
	}
	if !strings.Contains(err.Error(), "拒绝明文送出授权码") {
		t.Errorf("错误文本要说明**为什么**不发，实得 %q", err.Error())
	}
	if srv.seen("AUTH") || srv.seen("MAIL") {
		t.Errorf("拒绝=一枪都不发，实得命令序列 %q", srv.commandOrder())
	}
	if strings.Contains(err.Error(), mailTestPass) {
		t.Error("错误文本里带着授权码原文")
	}
}

func TestNoPasswordSendsNoAuthAtAll(t *testing.T) {
	// 内网免认证中继的形状：服务器声明了 AUTH，但我们没口令 ⇒ 不猜、不发。
	srv := startFakeSMTP(t, fakeSMTPOptions{implicit: true, authMechs: []string{"PLAIN", "LOGIN"}})
	message := mailTestMessage()
	message.Password = core.NewSecretValue("")

	if err := srv.senderFor(t, nil).SendMail(context.Background(), message); err != nil {
		t.Fatalf("免认证中继应该发得出去：%v", err)
	}
	if srv.seen("AUTH") {
		t.Error("没填口令还去发 AUTH：对面会回 502 语法错误，比\"这趟没认证\"难查得多")
	}
	if !srv.seen("DATA") {
		t.Errorf("应该继续把信发完，实得 %q", srv.commandOrder())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// AUTH 机制挑选（按服务器声明的那一份）
// ─────────────────────────────────────────────────────────────────────────────

func TestAuthMechanismFollowsWhatTheServerAdvertises(t *testing.T) {
	cases := []struct {
		name     string
		mechs    []string
		wantMech string // "" 表示"这一枪根本不该发"
	}{
		{"两种都给 ⇒ PLAIN（一句发完）", []string{"PLAIN", "LOGIN"}, "PLAIN"},
		{"只给 LOGIN（国内邮箱常见）", []string{"LOGIN"}, "LOGIN"},
		{"只给 PLAIN", []string{"PLAIN"}, "PLAIN"},
		{"声明了没实现的机制 ⇒ 不发", []string{"XOAUTH2", "CRAM-MD5"}, ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			srv := startFakeSMTP(t, fakeSMTPOptions{
				implicit: true, authMechs: tc.mechs, acceptAuth: true,
			})
			err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage())
			if tc.wantMech == "" {
				if err == nil || !strings.Contains(err.Error(), "没有实现那一种登录机制") {
					t.Fatalf("认不出的机制该明确报错，实得 %v", err)
				}
				if srv.seen("AUTH") {
					t.Error("不该发 AUTH")
				}
				return
			}
			if err != nil {
				t.Fatalf("应该发出去：%v", err)
			}
			if got := srv.authMechSeen(); got != tc.wantMech {
				t.Errorf("机制挑错了：实得 %q，期望 %q", got, tc.wantMech)
			}
			if user, pass := srv.creds(); user != mailTestUser || pass != mailTestPass {
				t.Errorf("%s 的凭据不符：%q / %q", tc.wantMech, user, pass)
			}
		})
	}
}

func TestLoginAuthAnswersTheTwoChallengesInOrder(t *testing.T) {
	// 这一条单独存在是因为 `loginAuth` 是本实现里唯一"标准库没有、我们自写"的一段。
	// 它按**步序**回答而不是匹配挑战文本，所以要把"两步各回了什么"钉成原文：
	// 只断言最后解出来的 user/pass 不够 —— 那两个值对了也可能是顺序恰好撞对。
	srv := startFakeSMTP(t, fakeSMTPOptions{
		implicit: true, authMechs: []string{"LOGIN"}, acceptAuth: true,
	})

	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("LOGIN 那一趟应该发出去：%v", err)
	}
	steps := srv.steps()
	if len(steps) != 2 {
		t.Fatalf("AUTH LOGIN 应该是两步应答，实得 %d 步：%v", len(steps), steps)
	}
	if got := mustDecode(t, steps[0]); got != mailTestUser {
		t.Errorf("第一步应是登录名，实得 %q", got)
	}
	if got := mustDecode(t, steps[1]); got != mailTestPass {
		t.Errorf("第二步应是授权码，实得 %q", got)
	}
}

func TestPlainAuthSendsOneStepWithEmptyAuthorizationID(t *testing.T) {
	srv := startFakeSMTP(t, fakeSMTPOptions{
		implicit: true, authMechs: []string{"PLAIN"}, acceptAuth: true,
	})
	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("PLAIN 那一趟应该发出去：%v", err)
	}
	steps := srv.steps()
	if len(steps) != 1 {
		t.Fatalf("PLAIN 应该只有一句参数，实得 %d 句", len(steps))
	}
	// `\0user\0pass`：**授权方留空**是这一串的固定形状，写成 `user\0pass` 会被回 501。
	if got := mustDecode(t, steps[0]); got != "\x00"+mailTestUser+"\x00"+mailTestPass {
		t.Errorf("PLAIN 的参数不符：%q", got)
	}
}

func mustDecode(t *testing.T, text string) string {
	t.Helper()
	decoded, err := base64.StdEncoding.DecodeString(text)
	if err != nil {
		t.Fatalf("%q 不是 base64：%v", text, err)
	}
	return string(decoded)
}

// ─────────────────────────────────────────────────────────────────────────────
// 失败可见：服务器的每一句 5xx 都要落进错误文本，且不带出凭据
// ─────────────────────────────────────────────────────────────────────────────

func TestAuthFailureStopsBeforeTheEnvelope(t *testing.T) {
	srv := startFakeSMTP(t, fakeSMTPOptions{
		implicit: true, authMechs: []string{"PLAIN"},
		// 有的自建中继会把最后一条命令回显在错误应答里，而 AUTH PLAIN 的参数
		// 只是 base64 —— 这一句就是照那个形状造的。
		authRejectText: "535 login rejected for " + mailTestUser + " " +
			base64.StdEncoding.EncodeToString([]byte(mailTestPass)) + " (" + mailTestPass + ")",
	})

	err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage())
	if err == nil {
		t.Fatal("AUTH 被拒却返回了成功")
	}
	if kind, _ := core.KindOf(err); kind != core.KindAuth {
		t.Errorf("授权码错应该是 Auth 档，实得 %v", kind)
	}
	if strings.Contains(err.Error(), mailTestPass) {
		t.Errorf("错误文本里出现了授权码原文（脱敏没生效）：%q", err.Error())
	}
	if !strings.Contains(err.Error(), "‹已脱敏›") {
		t.Errorf("脱敏要留一个可见的记号，实得 %q", err.Error())
	}
	if srv.seen("MAIL") {
		t.Error("AUTH 失败之后不该继续发信封（那会在对面留下半截事务）")
	}
}

func TestPerRecipientRejectionNamesThatAddress(t *testing.T) {
	srv := startFakeSMTP(t, fakeSMTPOptions{
		implicit: true, authMechs: []string{"PLAIN"}, acceptAuth: true,
		rcptReject: "550 no such user here b@y.test",
	})

	err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage())
	if err == nil {
		t.Fatal("第二个收件人被拒却返回了成功")
	}
	if !strings.Contains(err.Error(), "b@y.test") {
		t.Errorf("要说清是**哪个**地址被拒（第一个其实已经收了），实得 %q", err.Error())
	}
	if kind, _ := core.KindOf(err); kind != core.KindUpstream {
		t.Errorf("对面拒的是收件人，属于上游答复，实得 %v", kind)
	}
}

func TestDataRejectionIsNotSuccess(t *testing.T) {
	// ⚠ 这一格是整封邮件最容易"假绿"的地方：`DotWriter.Close()` 才是服务器
	// 对整封信表态的那一刻。漏判它的表现是"日志说已发、用户什么都没收到"。
	srv := startFakeSMTP(t, fakeSMTPOptions{
		implicit: true, authMechs: []string{"PLAIN"}, acceptAuth: true,
		dataReject: "552 mailbox full",
	})

	err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage())
	if err == nil {
		t.Fatal("DATA 被拒却返回了成功")
	}
	if !strings.Contains(err.Error(), "邮件体被拒") {
		t.Errorf("错误文本要指向 DATA 那一步，实得 %q", err.Error())
	}
}

func TestWrongCertificateNameRefusesTheConnection(t *testing.T) {
	// 证书签给别的名字 ⇒ 必须握手失败。这一条是"我们**确实**在验根证书与服务器名"
	// 的唯一反向证据：本仓刻意没有 `InsecureSkipVerify` 的口子。
	srv := startFakeSMTP(t, fakeSMTPOptions{
		implicit: true, authMechs: []string{"PLAIN"}, acceptAuth: true,
		certHost: "other.mcloud.test",
	})

	err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage())
	if err == nil {
		t.Fatal("证书名不对还发了出去：那等于关掉了校验")
	}
	if kind, _ := core.KindOf(err); kind != core.KindNetwork {
		t.Errorf("连不上应该落在 Network，实得 %v", kind)
	}
	if !strings.Contains(err.Error(), "连不上") {
		t.Errorf("错误文本应该指出是哪一台连不上，实得 %q", err.Error())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 超时与取消
// ─────────────────────────────────────────────────────────────────────────────

func TestExpiredContextFailsBeforeDialing(t *testing.T) {
	srv := plainWorkingServer(t)
	ctx, cancel := context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
	defer cancel()

	err := srv.senderFor(t, nil).SendMail(ctx, mailTestMessage())
	if err == nil {
		t.Fatal("ctx 早就过期了还发出去")
	}
	if kind, _ := core.KindOf(err); kind != core.KindTimeout {
		t.Errorf("这一档应该是 Timeout，实得 %v", kind)
	}
	if len(srv.dialsSeen()) != 0 {
		t.Error("超时判据在拨号之前，不该碰网络")
	}
}

func TestCanceledContextStopsAStalledHandshake(t *testing.T) {
	// 假服务器打完招呼就拖着不回 —— 真实世界里这是"连上了但对面的 EHLO 卡住"。
	srv := startFakeSMTP(t, fakeSMTPOptions{implicit: true, hangAfterGreeting: true})
	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Millisecond)
	defer cancel()

	started := time.Now()
	err := srv.senderFor(t, nil).SendMail(ctx, mailTestMessage())
	if err == nil {
		t.Fatal("服务器拖着不回却返回了成功")
	}
	if elapsed := time.Since(started); elapsed > 5*time.Second {
		t.Errorf("应该在 ctx 到点时断开，实得等了 %v", elapsed)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 信封与正文的字节
// ─────────────────────────────────────────────────────────────────────────────

func TestEnvelopeUsesOnlyTheBareAddress(t *testing.T) {
	srv := plainWorkingServer(t)
	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	joined := strings.Join(srv.rawLines(), "\n")
	if !strings.Contains(joined, "MAIL FROM:<"+mailTestUser+">") {
		t.Errorf("信封里必须是**裸地址**（带显示名整串会被回 501），实得：\n%s", joined)
	}
	if strings.Contains(joined, "MAIL FROM:<mcloud") {
		t.Error("把 \"显示名 <地址>\" 整串塞进了 MAIL FROM")
	}
	for _, want := range []string{"RCPT TO:<a@x.test>", "RCPT TO:<b@y.test>"} {
		if !strings.Contains(joined, want) {
			t.Errorf("少了 %s", want)
		}
	}
}

func TestFromHeaderAndEnvelopeAgreeOnTheAddress(t *testing.T) {
	// 两处不一致的表现为"信封说 a、头里写 b"，而 SPF/DMARC 判定看的正是这两个。
	srv := plainWorkingServer(t)
	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	body := srv.bodyText()
	if !strings.Contains(body, " <"+mailTestUser+">") {
		t.Errorf("From 头里没有那个地址：\n%s", headOf(body))
	}
	if !strings.HasPrefix(lineWith(body, "From:"), "From: =?utf-8?B?") {
		t.Errorf("中文显示名必须走 RFC 2047，实得 %q", headOf(body))
	}
}

func TestHeadersAreASCIIOnly(t *testing.T) {
	srv := plainWorkingServer(t)
	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	for _, line := range strings.Split(headOf(srv.bodyText()), "\r\n") {
		if !isASCII(line) {
			t.Errorf("头部出现了非 ASCII 字节（收件侧会折坏它）：%q", line)
		}
	}
}

func TestNoCRLFedIntoAHeaderIsHeadInjection(t *testing.T) {
	srv := plainWorkingServer(t)
	message := mailTestMessage()
	message.Subject = "签到\r\nBcc: evil@x.test\n结果"
	message.From = "运行\r\nBcc: evil2@x.test 推送 <" + mailTestUser + ">"

	if err := srv.senderFor(t, nil).SendMail(context.Background(), message); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	body := srv.bodyText()
	if strings.Contains(strings.ToUpper(body), "\r\nBCC:") {
		t.Errorf("头部被换行顶开了一条 Bcc（这是**头注入**，不是显示乱掉）：\n%s", headOf(body))
	}
	// 结构判据：头部**行数**不许因为注入而变多。上面那句只挡了一种写法，
	// 而"压平"与"整行丢弃"都能让它通过 —— 所以两处一起看。
	if got, want := len(strings.Split(headOf(body), "\r\n")), 6; got != want {
		t.Errorf("头部应是 %d 行（From/To/Subject/MIME-Version/Content-Type/CTE），实得 %d 行：\n%s",
			want, got, headOf(body))
	}
	// 压成空格而不是删掉：标题里带换行本来就非法，压平不丢信息。
	if decoded := decodeHeaderWord(t, lineWith(body, "Subject:")); !strings.Contains(decoded, "Bcc:") ||
		strings.ContainsAny(decoded, "\r\n") {
		t.Errorf("注入串应被压平而不是丢弃，实得 %q", decoded)
	}
}

func TestBodyIsQuotedPrintableAndRoundTrips(t *testing.T) {
	srv := plainWorkingServer(t)
	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	_, text, found := strings.Cut(srv.bodyText(), "\r\n\r\n")
	if !found {
		t.Fatalf("头部与正文之间没有空行：\n%q", srv.bodyText())
	}
	if decoded := decodeQuotedPrintable(t, text); decoded != crlf(mailTestMessage().Text) {
		t.Errorf("正文往返不一致：\n实得 %q\n期望 %q", decoded, crlf(mailTestMessage().Text))
	}
	for _, line := range strings.Split(text, "\r\n") {
		if len(line) > 76 {
			t.Errorf("QP 行超过 76 字符（老链路会截）：%d 字符 %q", len(line), line)
		}
	}
}

func TestLeadingDotIsStuffedAndDoesNotEndTheBody(t *testing.T) {
	// 正文里以 `.` 开头的那一行：不点填充的话，服务器会在**那一行**就结束 DATA，
	// 表现成"信发出去了但只有前半段"。
	srv := plainWorkingServer(t)
	message := mailTestMessage()
	message.Text = "上半\n. 这行以点开头\n下半"

	if err := srv.senderFor(t, nil).SendMail(context.Background(), message); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	_, text, _ := strings.Cut(srv.bodyText(), "\r\n\r\n")
	if decoded := decodeQuotedPrintable(t, text); decoded != crlf(message.Text) {
		t.Errorf("点填充没做对：实得 %q", decoded)
	}
}

func TestEveryLineEndsCRLF(t *testing.T) {
	// QP 写入器与点填充写入器**都**会把 `\n` 归一成 `\r\n`。两处都归一不会叠成
	// `\r\r\n` —— 这一条就是那一格的证据：删掉任一处都不会红，但把两处都改成
	// "自己补 \\r\\n" 会红。
	srv := plainWorkingServer(t)
	if err := srv.senderFor(t, nil).SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	body := srv.bodyText()
	if strings.Contains(body, "\r\r\n") {
		t.Errorf("换行被归一了两次：\n%q", body)
	}
	// 最后一行**不该**有 CRLF：假服务器在解 DATA 时会去掉终结符前那一个换行，
	// 而写入侧本来也不补（`TestEveryLineEndsCRLF` 后半段钉的就是它）。
	lines := strings.Split(body, "\n")
	for i, line := range lines[:max(0, len(lines)-1)] {
		if !strings.HasSuffix(line, "\r") {
			t.Errorf("第 %d 行不是 CRLF 结尾：%q", i, line)
		}
	}
	if strings.HasSuffix(body, "\r\n") {
		t.Error("末尾多补了一个空行（终结符前那一个由 DotWriter 负责）")
	}
}

func TestDateHeaderOnlyWhenAClockWasInjected(t *testing.T) {
	at := time.Date(2026, 9, 21, 8, 30, 0, 0, time.UTC)

	srvWith := plainWorkingServer(t)
	if err := srvWith.senderFor(t, stubMailClock{at: at}).
		SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("带时钟那一趟失败：%v", err)
	}
	if !strings.Contains(srvWith.bodyText(), "Date: Mon, 21 Sep 2026 08:30:00 +0000") {
		t.Errorf("注入了时钟就该有可断言的 Date：\n%s", headOf(srvWith.bodyText()))
	}

	srvWithout := plainWorkingServer(t)
	if err := srvWithout.senderFor(t, nil).
		SendMail(context.Background(), mailTestMessage()); err != nil {
		t.Fatalf("没时钟那一趟失败：%v", err)
	}
	if strings.Contains(srvWithout.bodyText(), "Date:") {
		t.Error("没注入时钟时**不写**那一行，而不是就地取系统时间")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送前的三道结构闸（不该碰网络）
// ─────────────────────────────────────────────────────────────────────────────

func TestPreFlightGuardsSendNothing(t *testing.T) {
	cases := []struct {
		name  string
		fix   func(m *core.MailMessage)
		wantS string
	}{
		{"没有主机名", func(m *core.MailMessage) { m.Host = "  " }, "SMTP 主机名为空"},
		{"没有收件人", func(m *core.MailMessage) { m.To = " ， ； " }, "没有收件人"},
		{"有授权码没登录名", func(m *core.MailMessage) { m.Username = core.NewSecretValue("") },
			"填了授权码但没填登录名"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			srv := plainWorkingServer(t)
			message := mailTestMessage()
			tc.fix(&message)

			err := srv.senderFor(t, nil).SendMail(context.Background(), message)
			if err == nil || !strings.Contains(err.Error(), tc.wantS) {
				t.Fatalf("应该报 %q，实得 %v", tc.wantS, err)
			}
			if len(srv.dialsSeen()) != 0 {
				t.Error("结构闸在拨号之前")
			}
		})
	}
}

func TestEnvelopeFallsBackToTheLoginName(t *testing.T) {
	// `From` 只填了个不像地址的东西时，信封退回登录名：对面认的是地址，
	// 而"发不出去"在这里比"显示名好看"贵。
	srv := startFakeSMTP(t, fakeSMTPOptions{implicit: true, acceptAuth: true})
	message := mailTestMessage()
	message.From = "只有显示名 没有地址"

	if err := srv.senderFor(t, nil).SendMail(context.Background(), message); err != nil {
		t.Fatalf("发信失败：%v", err)
	}
	if joined := strings.Join(srv.rawLines(), "\n"); !strings.Contains(
		joined, "MAIL FROM:<"+mailTestUser+">") {
		t.Errorf("信封应退回登录名：\n%s", joined)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具
// ─────────────────────────────────────────────────────────────────────────────

func headOf(body string) string {
	head, _, _ := strings.Cut(body, "\r\n\r\n")
	return head
}

func lineWith(body, prefix string) string {
	for _, line := range strings.Split(headOf(body), "\r\n") {
		if strings.HasPrefix(line, prefix) {
			return line
		}
	}
	return ""
}

// decodeHeaderWord 取出某个头的值，并把 `=?utf-8?B?…?=` 解回原文。
//
// 测试**不**复用 `mime.DecodeWord`：那样等于拿实现去验实现。这里手工切三段，
// 钉的是"收件方按 RFC 2047 能读出这句话"。
func decodeHeaderWord(t *testing.T, headerLine string) string {
	t.Helper()
	_, value, _ := strings.Cut(headerLine, ":")
	value = strings.TrimSpace(value)
	if !strings.HasPrefix(value, "=?") {
		return value
	}
	parts := strings.Split(value, "?") // = | utf-8 | B | base64 | =
	if len(parts) != 5 {
		t.Fatalf("不是合法的 encoded-word：%q", value)
	}
	if parts[1] != "utf-8" || parts[2] != "B" {
		t.Fatalf("期望 utf-8 / B 两段，实得 %q", value)
	}
	decoded, err := base64.StdEncoding.DecodeString(parts[3])
	if err != nil {
		t.Fatalf("%q 的 base64 段解不开：%v", value, err)
	}
	return string(decoded)
}

// decodeQuotedPrintable 解正文。换行**原样保留**（stdlib 的 Reader 会留住 CRLF），
// 所以调用侧要拿 CRLF 形态去比 —— 见 [crlf]。
func decodeQuotedPrintable(t *testing.T, text string) string {
	t.Helper()
	out, err := io.ReadAll(quotedprintable.NewReader(strings.NewReader(text)))
	if err != nil {
		t.Fatalf("正文不是合法 QP：%v", err)
	}
	return string(out)
}

// crlf 把"我们交给写入器的那份正文"换算成"线路上该有的那份"。
//
// 需要这么一函数本身就是结论：调用方给 `\n`，而信上走的是 `\r\n` ——
// 归一由 QP 与点填充两处完成（`TestEveryLineEndsCRLF` 钉的就是那一格）。
func crlf(text string) string {
	return strings.ReplaceAll(text, "\n", "\r\n")
}
