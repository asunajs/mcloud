// SMTP 发送器 —— [core.MailSender] 的标准库实现。
//
// 对应旧实现 `packages/shared/src/push/email.node.ts`（nodemailer 那一版）。
//
// # 为什么只用标准库
//
// AGENTS.md §3.4：Go 侧零第三方新增。`net/smtp` 够用，但**不够顺**：
// 它没有导出的 AUTH LOGIN（只有 PLAIN 与 CRAM-MD5），也不管"隐式 TLS 怎么连"，
// 所以这两件在本文件里各有一小段自己写的代码（[loginAuth]、[SMTPSender.dial] 那一步）。
//
// # 两条 TLS 通路，判据是端口（不是我们发明的）
//
//   - 465（[core.MailDefaultPort]）⇒ **隐式 TLS**：连上去那一刻就是 TLS，
//     不做 STARTTLS 协商。[core.MailMessage.ImplicitTLS] 就是这一条的判据。
//   - 其余端口（587 / 25）⇒ 先明文握手，看到 `STARTTLS` 再升级。
//
// 旧 nodemailer 的 `secure: port === 465` 是同一个判据；这里把它收成一个函数，
// 是为了让"填了 25 却按 465 连"这类配置错误在界面上说得清楚（见 `core/mail.go`）。
//
// # ⚠ 这一族**零实拍**（未测试的点写在这里，不藏在注释末尾）
//
// 本仓没有任何一台真 SMTP 服务商的样本（QQ / 163 要**授权码**而不是登录口令那句
// 来自常识与旧文档，不是我们打出来的）。下面这些格子**只**在进程内的假服务器
// （`mail_fakeserver_test.go`）上验过协议步骤：
//
//   - 真服务商的 AUTH 机制集合（有的只给 LOGIN，有的只给 PLAIN，有的要 XOAUTH2）；
//   - 465 上个别服务商的"隐式 TLS 但证书链不完整"；
//   - `900 lines too many` 那类运营商侧限流的表现。
//
// 补样方法：拿一个测试邮箱（不是云盘账号）手工配一次、真发一封，
// 把**服务器原文**（含 EHLO 能力行与错误码）逐字记进 `docs/capture/`。
//
// # 一处刻意不同于旧实现：**没有 TLS 就不登录**
//
// 明文连接上发凭据 = 授权码上网。旧 nodemailer 在非 465 端口上如果服务端不给
// STARTTLS 会照登（它只有一句 `secure` 判断）。这里改成**拒绝发送**并说明原因：
// 少发一封是可见的失败，授权码被掐走是不可见的损失。
// 要放开这一条需要用户拍板（登记在 `docs/pending-decisions.md`）。
package mcloudio

import (
	"context"
	"crypto/tls"
	"encoding/base64"
	"fmt"
	"io"
	"mime"
	"mime/quotedprintable"
	"net"
	"net/smtp"
	"os"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/catlair/mcloud/internal/core"
)

// mailDialTimeout 是"连上去"这一步的上限。
//
// 与 [io.Client] 的 20s 同量级，但**刻意不共用那个常量**：SMTP 握手是三步
// （connect / EHLO / AUTH），这里 20s 只压第一步；真出问题时"连不上"与
// "连上了但服务端拖着不回"要能分开看，所以后续步骤各自另设 deadline。
const mailDialTimeout = 20 * time.Second

// SMTPSender 是标准库版邮件发送器。
//
// 三个可选字段都是为了"能在没有真服务器的情况下断言协议步骤"而留的口子，
// ⚠ 它们**不改变**生产路径的判据：`TLSConfig` 为 nil 时仍然验根证书与服务器名，
// `dial` 为 nil 时仍然按端口决定隐式 TLS 还是明文 —— 测试里换掉的只有
// "连到哪个地址"这一件事。
type SMTPSender struct {
	// TLSConfig 为 nil ⇒ 用默认（系统根证书 + 按 Host 验服务器名 + TLS 1.2 起）。
	TLSConfig *tls.Config
	// Clock 只用于 `Date:` 头；nil ⇒ **不写**那一行（收件侧的服务器会补 `Received`/`Date`）。
	// 之所以要注时钟而不是就地 `time.Now()`：那一行是可断言的形状。
	Clock core.Clock
	// Timeout 是整封邮件（含握手与 DATA）的总上限；0 ⇒ [mailDialTimeout] 的三倍。
	//
	// 调用方给了带 deadline 的 ctx 时**以 ctx 为准**（见 [SMTPSender.SendMail]）：
	// 推送那一轮是串行发十家，上面取消了这里还继续等 60s 是不成立的。
	Timeout time.Duration
	// LocalName 是 EHLO/HELO 里报的自己那台机器的名字；"" ⇒ [os.Hostname]，再失败 ⇒ "localhost"。
	LocalName string
	// dial 是"建立 TCP/TLS 连接"那一步。生产为 nil ⇒ 走 [defaultMailDial]。
	dial func(ctx context.Context, implicitTLS bool, host string, port int,
		cfg *tls.Config, timeout time.Duration) (net.Conn, error)
}

// NewSMTPSender 返回默认配置的发送器（验证书、20s×3 总时限）。
func NewSMTPSender() *SMTPSender { return &SMTPSender{} }

// SendMail 实现 [core.MailSender]。
//
// 错误一律是 [core.Error]：这一句会被 `upstream.sendEmail` 截断后放进渠道结论，
// 而那里对"没发出去"与"发出去但失败"的措辞不同。
//
// ⚠ 每一句错误文本都要过 [mailErrorText]：`AUTH LOGIN` 的两步只是 base64，
// 而中间件常把最后一条命令回显在应答里 —— 那句原文会进 `run_events`。
func (s *SMTPSender) SendMail(ctx context.Context, message core.MailMessage) error {
	if strings.TrimSpace(message.Host) == "" {
		return core.NewBadRequest("SMTP 主机名为空")
	}
	if len(core.MailRecipients(message.To)) == 0 {
		return core.NewBadRequest("没有收件人")
	}
	user := message.Username.Value()
	pass := message.Password.Value()
	if pass != "" && user == "" {
		// 有口令没登录名：AUTH 一定失败，而失败原因会被服务端回成一句看不懂的 5xx。
		// 在这一格挡住比让对面报错便宜。
		return core.NewBadRequest("填了授权码但没填登录名")
	}

	// 总时限：ctx 有 deadline 时以它为准，否则用 [SMTPSender.Timeout]。
	// 设在**连接**上而不是逐步计算：SMTP 是流式协议，可能卡在任意一趟
	// （EHLO 不回、DATA 续行不接），这些都要在同一刻断掉才算"这一封超时了"。
	deadline, hasDeadline := ctx.Deadline()
	timeout := s.Timeout
	if timeout <= 0 {
		timeout = 3 * mailDialTimeout
	}
	dialTimeout := mailDialTimeout
	if hasDeadline {
		if remaining := time.Until(deadline); remaining < dialTimeout {
			dialTimeout = remaining
		}
	} else {
		deadline = time.Now().Add(timeout)
		dialTimeout = min(mailDialTimeout, timeout)
	}
	if dialTimeout <= 0 {
		return core.NewTimeout("这封邮件在开始连接前就已经超时")
	}

	tlsConfig := s.mailTLSConfig(message.Host)
	dial := s.dial
	if dial == nil {
		dial = defaultMailDial
	}

	conn, derr := dial(ctx, message.ImplicitTLS(), message.Host, message.EffectivePort(),
		tlsConfig, dialTimeout)
	if derr != nil {
		return core.NewNetwork(fmt.Sprintf("连不上 %s:%d：%s",
			message.Host, message.EffectivePort(), mailErrorText(derr, pass)))
	}
	defer func() { _ = conn.Close() }()
	if !deadline.IsZero() {
		_ = conn.SetDeadline(deadline)
	}

	cli, err := smtp.NewClient(conn, message.Host)
	if err != nil {
		return core.NewUpstream(0, "SMTP 握手失败: "+mailErrorText(err, pass))
	}
	defer func() { _ = cli.Quit() }()

	if err := cli.Hello(s.helloName()); err != nil {
		return core.NewUpstream(0, "SMTP EHLO 失败: "+mailErrorText(err, pass))
	}

	// 隐式 TLS 时 `smtp.NewClient` 已经认出连接是 *tls.Conn（`TLSConnectionState`
	// 的第二个返回值为真），那种连接**不能再**发 STARTTLS —— 发出去对面会回
	// `503 已有 TLS`，而这一句看起来像"配置错了"。
	if _, alreadyTLS := cli.TLSConnectionState(); !alreadyTLS {
		if ok, _ := cli.Extension("STARTTLS"); ok {
			if err := cli.StartTLS(tlsConfig); err != nil {
				return core.NewUpstream(0, "STARTTLS 失败: "+mailErrorText(err, pass))
			}
		} else if pass != "" {
			// 上面那条"没有 TLS 就不登录"的落点。
			return core.NewBadRequest("这台 SMTP 不提供 TLS，拒绝明文送出授权码")
		}
	}

	if pass != "" {
		if err := s.authenticate(cli, message.Host, user, pass); err != nil {
			return err
		}
	}

	from := envelopeAddress(message)
	if err := cli.Mail(from); err != nil {
		return core.NewUpstream(0, "MAIL FROM 被拒: "+mailErrorText(err, pass))
	}
	for _, to := range core.MailRecipients(message.To) {
		if err := cli.Rcpt(to); err != nil {
			// 逐个收件人报：`550 no such user` 只针对那一个地址，
			// 报成"发失败"会让人以为整封没出去（其实另外两个收到了）。
			return core.NewUpstream(0, fmt.Sprintf("RCPT %s 被拒: %s", to, mailErrorText(err, pass)))
		}
	}

	data, err := cli.Data()
	if err != nil {
		return core.NewUpstream(0, "DATA 被拒: "+mailErrorText(err, pass))
	}
	if err := s.writeMailMessage(data, message); err != nil {
		_ = data.Close()
		return core.NewInternal("写邮件体失败: " + mailErrorText(err, pass))
	}
	// ⚠ **必须** Close 之后再判定成败：`DotWriter.Close()` 才是写出终结符 `\r\n.\r\n`
	// 并 flush 的那一刻，而服务器对整封信的意见（`250 Queued` 还是 `552 满`）
	// 就在这一句里。少了这一步，"信被服务器拒了"会表现成"发送返回成功"。
	if err := data.Close(); err != nil {
		return core.NewUpstream(0, "邮件体被拒: "+mailErrorText(err, pass))
	}
	return nil
}

// authenticate 按服务器**声明**的机制挑一个：PLAIN 优先（一句发完，
// 少一个来回），只有 LOGIN 时走 [loginAuth]（Go 标准库不导出它，国内邮箱常见）。
//
// ⚠ 服务器没声明 AUTH 而我们又有口令时，这里**不发** AUTH 直接往下走：
// 对面可能是内网免认证的中继，硬发一条 `AUTH` 会被回 `502 语法错误`，
// 那一句比"这一趟没认证"更难查。真被拒时会停在 `RCPT`，错误里带着收件人地址。
func (s *SMTPSender) authenticate(cli *smtp.Client, host, user, pass string) error {
	advertised := ""
	if ok, args := cli.Extension("AUTH"); ok {
		advertised = strings.ToUpper(args)
	}
	switch {
	case advertised == "":
		return nil
	case strings.Contains(advertised, "PLAIN"):
		if err := cli.Auth(smtp.PlainAuth("", user, pass, host)); err != nil {
			return core.NewAuth("SMTP 登录失败（PLAIN）: " + mailErrorText(err, pass))
		}
		return nil
	case strings.Contains(advertised, "LOGIN"):
		if err := cli.Auth(&loginAuth{username: user, password: pass}); err != nil {
			return core.NewAuth("SMTP 登录失败（LOGIN）: " + mailErrorText(err, pass))
		}
		return nil
	}
	// 声明了我们不认的机制（XOAUTH2 / CRAM-MD5 之类）。
	return core.NewBadRequest(fmt.Sprintf(
		"这台 SMTP 只声明了 %q，本仓没有实现那一种登录机制", advertised))
}

// writeMailMessage 写信封之外的那段文本（头部 + 空行 + 正文）。
//
// 正文走 quoted-printable：与旧 nodemailer 同口径，也免得依赖 `8BITMIME`
// （中文正文按 8bit 直发，链路上任何一跳把它折成 ASCII 就毁了）。
//
// 两处"看着小、其实是要写对的"：
//   - **换行**：QP 写入器与 `smtp` 的点填充写入器都会把裸 `\n` 归一成 `\r\n`，
//     所以这里只给 `\n`（两处都归一不会叠成 `\r\r\n`：QP 先出 CRLF，
//     点填充看到 `\r\n` 原样通过 —— 这一步由 `TestMailBodyBytes` 逐字钉住）。
//   - **末尾不补换行**：`DotWriter.Close()` 负责在终结符前面补那一个 `\r\n`，
//     这里再补一个会让每封邮件的正文都多一个空行。
//
// ⚠ 头部**逐行过** [mailHeaderText]：`Subject` 与 `From` 的显示名来自用户配置，
// 一句 `标题\r\nBcc: someone@else` 就是**头注入** —— 那不是"显示乱掉"，
// 是拿我们的连接给别人发信。
func (s *SMTPSender) writeMailMessage(w io.Writer, message core.MailMessage) error {
	at, hasDate := s.mailDate()
	if _, err := io.WriteString(w, mailHeaderBlock(message, at, hasDate)); err != nil {
		return err
	}
	qp := quotedprintable.NewWriter(w)
	if _, err := io.WriteString(qp, message.Text); err != nil {
		return err
	}
	return qp.Close()
}

// mailDate 取 `Date:` 头要用的时刻，以及"这一行到底写不写"。
//
// 没接时钟时**不写**那一行，而不是就地 `time.Now()`：本层是 IO，写一次系统时间
// 技术上没问题，但那一行是可断言的形状 —— 留成"注入了才写"，测试里才钉得住，
// 而收件侧的服务器本来就会补 `Received` 与 `Date`（缺了它信照样到）。
func (s *SMTPSender) mailDate() (time.Time, bool) {
	if s.Clock == nil {
		return time.Time{}, false
	}
	return s.Clock.Now(), true
}

// mailHeaderBlock 组装头部文本（LF 结尾，交给点填充写入器归一）。
func mailHeaderBlock(message core.MailMessage, at time.Time, hasDate bool) string {
	var b strings.Builder
	b.WriteString("From: " + mailFromHeader(message) + "\n")
	b.WriteString("To: " + mailHeaderText(strings.Join(core.MailRecipients(message.To), ", ")) + "\n")
	b.WriteString("Subject: " + encodedWord(message.Subject) + "\n")
	if hasDate {
		b.WriteString("Date: " + at.Format("Mon, 02 Jan 2006 15:04:05 -0700") + "\n")
	}
	b.WriteString("MIME-Version: 1.0\n")
	b.WriteString("Content-Type: text/plain; charset=utf-8\n")
	b.WriteString("Content-Transfer-Encoding: quoted-printable\n")
	b.WriteString("\n")
	return b.String()
}

// mailFromHeader 拼 `显示名 <地址>`，并且**只对显示名做** RFC 2047 编码。
//
// 地址取自 [envelopeAddress]（与 `MAIL FROM:` 同一个来源，不是"看着像就行"）：
// 两处不一致的表现为"信封说 a、头里写 b"，而有些服务商的 SPF/DMARC 判定
// 恰恰看的是这两个对不对得上。显示名为空时不留下 `From:  <…>` 那种多一个空格的形状。
//
// ⚠ 地址那一半**不能**编码：`=?utf-8?B?...?=@example.com` 不是合法 addr-spec。
func mailFromHeader(message core.MailMessage) string {
	address := envelopeAddress(message)
	name := strings.TrimSpace(displayNameOf(message))
	if name == "" {
		return "<" + address + ">"
	}
	return encodedWord(name) + " <" + address + ">"
}

// loginAuth 是 SMTP 的 `AUTH LOGIN`：两次挑战分别回 base64(用户名) 与 base64(口令)。
//
// 标准库不导出它（只有 PLAIN 与 CRAM-MD5），而国内邮箱的服务端常常**只**接受 LOGIN。
//
// ⚠ 这里按**步序**回答，不去匹配挑战文本。常见的实现是"看那句 prompt 里有没有
// user/pass"，但各家发出来的提示串并不统一（`Username:`、`User name:`、
// `PassWord:`……），匹配失败的表现是"永远卡在第一步"。步序是这条协议的固定形状。
type loginAuth struct {
	username, password string
	step               int
}

func (a *loginAuth) Start(_ *smtp.ServerInfo) (string, []byte, error) {
	return "LOGIN", nil, nil
}

func (a *loginAuth) Next(challenge []byte, more bool) ([]byte, error) {
	if !more {
		return nil, nil
	}
	a.step++
	switch a.step {
	case 1:
		return []byte(a.username), nil
	case 2:
		return []byte(a.password), nil
	}
	return nil, nil
}

// defaultMailDial 是生产那一条：465 用 TLS 直连，其余端口先 TCP。
//
// 两条都走 `*Context` 版本：`net.Dialer.Dial` 与 `tls.DialWithDialer` 只认 Timeout，
// ctx 取消（用户按停止、整轮推送超时）打不断它们 —— 那正是这一族要能取消的理由。
func defaultMailDial(ctx context.Context, implicitTLS bool, host string, port int,
	cfg *tls.Config, timeout time.Duration,
) (net.Conn, error) {
	addr := net.JoinHostPort(host, fmt.Sprintf("%d", port))
	dialer := &net.Dialer{Timeout: timeout}
	if !implicitTLS {
		return dialer.DialContext(ctx, "tcp", addr)
	}
	return (&tls.Dialer{NetDialer: dialer, Config: cfg}).DialContext(ctx, "tcp", addr)
}

// mailTLSConfig 取这份连接要用的 TLS 配置。
//
// 注：**不**提供 `InsecureSkipVerify` 的口子。要连自签证书的自建服务器，
// 正确做法是把那台机器的 CA 交进来（`SMTPSender.TLSConfig.RootCAs`），
// 而不是关掉校验 —— 关掉之后这句"我们验过对面是谁"就不成立了。
func (s *SMTPSender) mailTLSConfig(host string) *tls.Config {
	if s.TLSConfig != nil {
		// 复制一份再补 ServerName：改调用方给的对象是跨模块的副作用。
		cfg := s.TLSConfig.Clone()
		if cfg.ServerName == "" {
			cfg.ServerName = host
		}
		return cfg
	}
	return &tls.Config{ServerName: host, MinVersion: tls.VersionTLS12}
}

// helloName 是 EHLO/HELO 里报的自己那台机器的名字。
func (s *SMTPSender) helloName() string {
	if name := strings.TrimSpace(s.LocalName); name != "" {
		return name
	}
	if host, err := os.Hostname(); err == nil && strings.TrimSpace(host) != "" {
		return host
	}
	// 拿不到主机名 ⇒ "localhost" 是标准库自己的默认值，也是 EHLO 的合法名字。
	// 这里不报错：一台报不出自己名字的机器不该导致用户收不到推送。
	return "localhost"
}

// envelopeAddress 取 `MAIL FROM:` 里那个**裸地址**。
//
// ⚠ 不能用 `message.From`：那是 `"标题 <地址>"`（显示名拼法照旧实现），
// 整串塞进 `MAIL FROM:<…>` 会被回 `501 syntax`，而那句错误看起来像"地址非法"。
// 兜底顺序：`<>` 里的 → 整串本身 → 登录名。
func envelopeAddress(message core.MailMessage) string {
	if open := strings.LastIndex(message.From, "<"); open >= 0 {
		if close := strings.Index(message.From[open:], ">"); close > 0 {
			if inner := strings.TrimSpace(message.From[open+1 : open+close]); inner != "" {
				return inner
			}
		}
	}
	if trimmed := strings.TrimSpace(message.From); trimmed != "" && !strings.Contains(trimmed, " ") {
		return trimmed
	}
	return message.Username.Value()
}

// displayNameOf 取 `"标题 <地址>"` 里那个显示名；没有 `<` 时给空串。
//
// 地址那一半不在这里取：它由 [envelopeAddress] 统一负责，头部与信封共用同一个来源
// （理由见 [mailFromHeader]）。
func displayNameOf(message core.MailMessage) string {
	if open := strings.LastIndex(message.From, "<"); open > 0 {
		return strings.TrimSpace(message.From[:open])
	}
	return ""
}

// encodedWord 把可能含中文的头值做成 `=?utf-8?B?…?=`。
//
// 纯 ASCII 时原样返回（不必编码，也方便人读日志）。旧 nodemailer 也是这个方向
// （它按 RFC 2047 编 Subject 与显示名）。
//
// ⚠ 那一句 `?b?` → `?B?` 不是强迫症：Go 的 `mime.BEncoding` 写**小写**，
// 而旧实现（那份实际跑通过的字节）写大写。大小写两样到底各认到哪一档，我们
// **没有样本**（未测试），所以照旧实现那一种 —— 它至少已经被收件方接受过。
func encodedWord(value string) string {
	value = mailHeaderText(value)
	if isASCII(value) {
		return value
	}
	return strings.Replace(mime.BEncoding.Encode("utf-8", value), "?b?", "?B?", 1)
}

// mailHeaderText 掐掉头值里的换行 —— 这是上面说的**头注入**那一道。
//
// 只删 `\r` `\n`（并把连续空白收成单个空格）：头注入需要的是一个换行，
// 而"标题里带换行"本来就是非法的，把它压成空格不丢信息。
func mailHeaderText(value string) string {
	if !strings.ContainsAny(value, "\r\n") {
		return value
	}
	cleaned := strings.NewReplacer("\r\n", " ", "\n", " ", "\r", " ").Replace(value)
	return strings.Join(strings.Fields(cleaned), " ")
}

func isASCII(value string) bool {
	for i := 0; i < len(value); i++ {
		if value[i] >= utf8.RuneSelf {
			return false
		}
	}
	return true
}

// mailErrorText 把 SMTP/TLS 错误里可能出现的**授权码原文**换掉。
//
// ⚠ `net/smtp` 不会回显口令，但中间件（自建中继、调试代理）常把最后一条命令
// 连同参数一起写进应答，而 `AUTH LOGIN` 的那两步是**可逆的 base64**。
// 这一句是要进渠道结论、进而进 `run_events` 的，所以先脱敏再截断。
func mailErrorText(err error, secret string) string {
	if err == nil {
		return ""
	}
	text := err.Error()
	if secret != "" {
		text = strings.ReplaceAll(text, secret, "‹已脱敏›")
		if encoded := base64.StdEncoding.EncodeToString([]byte(secret)); encoded != "" {
			text = strings.ReplaceAll(text, encoded, "‹已脱敏›")
		}
	}
	return text
}
