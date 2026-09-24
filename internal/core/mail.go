// 邮件发送的**契约**（纯层定义，实现在 `internal/io/mail.go`）。
//
// # 为什么要有这一对，而不是让推送渠道直接发 SMTP
//
// 与 [WebSocketDialer] 同源的理由：`net/smtp` 是 IO，而 `internal/upstream`
// 是纯层（依赖方向门禁里那条"不得出现 net/http、database/sql"的判据，
// 精神是"这一层不许自己开连接"）。把 SMTP 塞进渠道实现里，
// 十个渠道里有一个能测、九个测不了，而且"这封邮件发得出去"这件事
// 只能靠真连一台 SMTP 服务器来验 —— 那是本仓库最贵的验证方式。
//
// 拆开之后：渠道侧"该不该发、正文长什么样"用假发送器断言；
// 真正的 SMTP 协议步骤单独一处，那边用一台**进程内假服务器**逐枪对拍
// （`internal/io/mail_test.go`），两边都不需要联网就能跑。
//
// # ⚠ 这一族**零实拍**
//
// 旧实现的邮件走 nodemailer，本仓没有任何一份"我们自己的 SMTP 客户端被某个
// 服务商接受"的样本（QQ / 163 的邮箱推送要的是**授权码**而不是登录口令，
// 而这句话来自常识与旧文档，不是我们打出来的）。协议步骤已经在进程内假服务器上
// 对拍完（`internal/io/mail_test.go`），剩下的**是服务商侧的实拍缺口**，
// 登记在 `docs/pending-decisions.md` 的 R23；接上之后第一次真发要有人盯着，
// 验收口径见 `docs/status.md` 里对应那一节。
package core

import (
	"context"
	"fmt"
	"strings"
	"unicode/utf8"
)

// MailMessage 是一封要发出去的纯文本邮件。
//
// 字段少是有意的：旧实现也只发 `text/plain`（它甚至把 `Content-Type` 写死成
// `text/plain; charset=utf-8` 塞进 headers），不做 HTML、不做附件。
type MailMessage struct {
	// Host 是 SMTP 服务器地址。
	Host string
	// Port 是端口；`0` 表示"用户没填"，由实现按 [MailDefaultPort] 走。
	Port int
	// Username / Password 是登录凭据（多数国内邮箱这里要的是**授权码**）。
	Username SecretValue
	Password SecretValue
	// From 是信封发件人。旧实现把它同时放进 `From:` 头的显示名后面
	// （`` `${title} <${from}>` ``），那个写法保留在实现侧。
	From string
	// To 是收件人；逗号分隔的多收件人按 [MailRecipients] 拆。
	To string
	// Subject 是标题。
	Subject string
	// Text 是正文。实现负责把换行成 `CRLF`（旧实现就是这么做的：
	// `text.replace(/\n/g, "\r\n")`），调用方给的是 `\n`。
	Text string
}

// String 实现 fmt.Stringer —— 打印这一封时把凭据换成脱敏形式、正文只报长度。
//
// ⚠ 与 [SecretValue] 同一条边界：**只挡打印**，不挡 `encoding/json`。
// 这里仍要单独写一层，是因为日志里出现的往往是**整个结构体**（`%+v`），
// 而 `MailMessage` 一旦带上 `From`（含地址）与正文，一次"顺手把信打出来"
// 就能把整封运行数据抄进日志文件；正文按长度报，够判断"发没发、发多长"。
func (m MailMessage) String() string {
	return fmt.Sprintf(
		"MailMessage{Host:%s Port:%d Username:%s Password:%s From:%q To:%q Subject:%q Text:<%d 字符>}",
		m.Host, m.EffectivePort(), m.Username, m.Password, m.From, m.To, m.Subject,
		utf8.RuneCountInString(m.Text),
	)
}

// GoString 接住 `%#v` —— 那条路不走 Stringer（同 [SecretValue.GoString]）。
func (m MailMessage) GoString() string { return m.String() }

// MailDefaultPort 是没填端口时用的那一个（465）。
//
// 与旧实现同口径：`Number(config.port) || 465`，并且**只有 465 才按隐式 TLS 连**。
// 这条"端口决定 TLS 模式"的耦合是 SMTP 客户端的常见写法，不是本仓的发明，
// 但它意味着"填 25 却在 25 端口要求隐式 TLS"这类配置会被连错 —— 界面上要写清楚。
const MailDefaultPort = 465

// ImplicitTLS 判这一封要不要"连上就是 TLS"（465），还是先明文再升级（其余端口）。
//
// 旧 nodemailer 的 `secure: port === 465` 是同一个判据，所以这不是新增行为，
// 只是把那条判断写成一个能被断言的函数。两条通路**都**在进程内假服务器上对拍过
// （`internal/io/mail_test.go`：465 不发 STARTTLS、25 先发 STARTTLS 再发 AUTH）。
// 仍**未测试**的是真服务商那一边：这一格缺的是实拍而不是断言，见
// `docs/pending-decisions.md` 的 R23。
func (m MailMessage) ImplicitTLS() bool {
	return m.EffectivePort() == MailDefaultPort
}

// EffectivePort 返回实际端口（未填时给 [MailDefaultPort]）。
func (m MailMessage) EffectivePort() int {
	if m.Port == 0 {
		return MailDefaultPort
	}
	return m.Port
}

// MailRecipients 拆收件人串（逗号或分号分隔，空白项丢掉）。
//
// 旧实现是 `to: config.to` 原样交给 nodemailer，由它去拆；
// `net/smtp` 要的是**切片**，所以这一步必须自己做。
// 允许 `；`（全角）是因为这份配置常常是人从聊天窗口里粘过来的 ——
// 全角分号不丢收件人，比"少发一个人而没人发现"要好。
func MailRecipients(to string) []string {
	var out []string
	for _, part := range strings.FieldsFunc(to, func(r rune) bool {
		switch r {
		case ',', ';', '，', '；', '\n', '\r':
			return true
		}
		return false
	}) {
		if trimmed := strings.TrimSpace(part); trimmed != "" {
			out = append(out, trimmed)
		}
	}
	return out
}

// MailSender 把一封 [MailMessage] 交出去。
//
// 实现**必须**是阻塞的（返回 nil 即"服务器已收下这封信"），
// 且不得把凭据写进它返回的错误串（用 [Error.PublicMessage] 那一套口径）。
//
// # 为什么带 ctx 而不是一句 `Timeout` 字段
//
// SMTP 是三趟握手 + 一段流式正文，一次"发不出去"可以卡在任意一趟上。
// 调用方（推送那一轮是**串行**发十家）要能整体取消：定时任务被停掉、
// 用户在界面上按了停止，都不该再多等一个连接超时。
// 本层的其他出口（[HTTPClient]、[WebSocketDialer]）都是同一个形状，
// 这里不带 ctx 会成为唯一的例外。
type MailSender interface {
	SendMail(ctx context.Context, message MailMessage) error
}
