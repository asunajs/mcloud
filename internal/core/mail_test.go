package core

// 邮件契约的用例 —— 只钉"判据"，不钉协议（协议在 `internal/io`，
// 那一半照进程内假服务器逐枪对拍：`internal/io/mail_test.go`）。
//
// 值得写的只有一件事：**端口与 TLS 模式绑在一起**。
// 旧实现是 `secure: port === 465`，本仓把它写成两个能被断言的函数。
// 一旦有人"顺手"把默认端口改成 587，行为会连着变（隐式 TLS 变 STARTTLS），
// 而这两件事分开看都"合理"，合起来是一次没人预期的连接方式变更。

import (
	"context"
	"testing"
)

func TestMailDefaultPortIsTheOneOldCodeUsed(t *testing.T) {
	if MailDefaultPort != 465 {
		t.Fatalf("默认端口应是 465（旧 `Number(port) || 465`），实得 %d", MailDefaultPort)
	}
	blank := MailMessage{Host: "smtp.example.invalid"}
	if got := blank.EffectivePort(); got != 465 {
		t.Errorf("没填端口应落到 %d，实得 %d", MailDefaultPort, got)
	}
	if !blank.ImplicitTLS() {
		t.Error("465 应是隐式 TLS —— 这一格与上面的默认端口是同一条判据")
	}
	submitted := MailMessage{Port: 25}
	if submitted.EffectivePort() != 25 || submitted.ImplicitTLS() {
		t.Errorf("25 端口不该按隐式 TLS 连：%d / %v", submitted.EffectivePort(), submitted.ImplicitTLS())
	}
	// 显式填 465 与"没填"必须同一条路（否则"默认"就成了第二种连接模式）。
	explicit := MailMessage{Port: 465}
	if explicit.ImplicitTLS() != blank.ImplicitTLS() {
		t.Error("显式 465 与默认端口走了两条路")
	}
}

// TestMailRecipientsCutsEverySeparatorWeHaveSeen 分隔符表：半角 / 全角 / 换行。
//
// ⚠ 全角那两个是本仓**新增**的宽容（旧实现原样交给 nodemailer 去拆）。
// 理由写在 mail.go：这份配置常是从聊天窗口粘过来的，
// 而"少发一个人"的表现是**静默的**。反向例：把全角分号从表里删掉，
// `a@b.c；c@d.e` 会被当成**一个**收件人交出去 —— 一封谁都收不到的信。
func TestMailRecipientsCutsEverySeparatorWeHaveSeen(t *testing.T) {
	cases := []struct {
		in   string
		want []string
	}{
		{"a@b.c", []string{"a@b.c"}},
		{"a@b.c,c@d.e", []string{"a@b.c", "c@d.e"}},
		{"a@b.c; c@d.e", []string{"a@b.c", "c@d.e"}},
		{"a@b.c；c@d.e", []string{"a@b.c", "c@d.e"}},
		{"a@b.c，c@d.e", []string{"a@b.c", "c@d.e"}},
		{"a@b.c\nc@d.e", []string{"a@b.c", "c@d.e"}},
		{"  a@b.c  ,,  ;", []string{"a@b.c"}},
		{"", nil},
		{",,;", nil},
	}
	for _, c := range cases {
		got := MailRecipients(c.in)
		if len(got) != len(c.want) {
			t.Errorf("%q 应拆成 %d 个，实得 %d：%q", c.in, len(c.want), len(got), got)
			continue
		}
		for i := range c.want {
			if got[i] != c.want[i] {
				t.Errorf("%q 第 %d 个应是 %q，实得 %q", c.in, i+1, c.want[i], got[i])
			}
		}
	}
}

// TestMailSenderIsSatisfiableWithoutNet 钉"契约能被假实现满足"——
// 这是渠道侧测试不联网的前提，也是这一对接口存在的全部理由。
func TestMailSenderIsSatisfiableWithoutNet(t *testing.T) {
	fake := &recordingMailSender{}
	var sender MailSender = fake
	if err := sender.SendMail(context.Background(), MailMessage{Subject: "hi"}); err != nil {
		t.Fatalf("假实现不该报错：%v", err)
	}
	if len(fake.sent) != 1 || fake.sent[0].Subject != "hi" {
		t.Errorf("假实现没收到那封信：%+v", fake.sent)
	}
}

// recordingMailSender 是本包测试用的假发送器；`internal/upstream` 的渠道测试
// 各有一份同形的（跨包不共享测试工具，除非将来抽成 `internal/golden` 那样的显式测试包）。
type recordingMailSender struct {
	sent []MailMessage
}

func (s *recordingMailSender) SendMail(_ context.Context, message MailMessage) error {
	s.sent = append(s.sent, message)
	return nil
}
