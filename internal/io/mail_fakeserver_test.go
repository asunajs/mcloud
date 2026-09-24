package mcloudio

// 进程内假 SMTP 服务器 —— 只为 `mail_test.go` 那几组协议断言服务。
//
// # 为什么自己写
//
// AGENTS.md §3.4：Go 侧零第三方新增。这一半协议（EHLO 能力行、STARTTLS 升级、
// AUTH 的两种机制、DATA 的点终结）用标准库写清楚约 200 行，比引一个测试依赖便宜，
// 而且**它自己不出产结论** —— 它只负责"服务器那样声明时客户端该怎么做"的场景，
// 判断言的口径全在 `mail_test.go`。
//
// # 它能证明什么、**不能**证明什么
//
// 能证明：协议步骤的顺序与形状（哪一枪在 TLS 之后发）、AUTH 机制怎么挑、
// 端口与 TLS 模式的耦合有没有传到连接那一层、信封与头部的**字节**、
// 错误文本有没有带出授权码。
//
// 不能证明：任何一家**真**服务商的行为。这台假服务器照 RFC 5321/4954 的最小形状写，
// 它对"QQ 邮箱只给 LOGIN"这类事实**零贡献** —— 那仍是待补的实拍（见 `mail.go` 抬头
// 与 `docs/pending-decisions.md`）。

import (
	"bufio"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"fmt"
	"math/big"
	"net"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// mailTestHost 是假服务器证书签给的那个名字，也是测试里 [core.MailMessage.Host] 的值。
//
// ⚠ 它**不能**解析得到（`.test` 是 RFC 6761 保留域）：一旦哪天真解析了，
// "测试里换掉的只有连到哪个地址"这句前提就不成立了 —— 桩会把信发到外面去。
const mailTestHost = "smtp.mcloud.test"

// fakeSMTPOptions 是这台假服务器的形状开关。
//
// 每一项都对应"客户端必须自己判断"的一格：做成参数而不是写死，是因为断言的价值
// 恰恰是"服务器那样声明时，我们这样做"。
type fakeSMTPOptions struct {
	// implicit 为真 ⇒ 连上来的第一条字节就是 TLS（465 那一种端点）。
	implicit bool
	// advertiseStartTLS 为真 ⇒ EHLO 能力行里给 `STARTTLS`。
	advertiseStartTLS bool
	// authMechs 是 EHLO 声明的机制集合（"PLAIN" / "LOGIN" / "XOAUTH2"…）。
	// 空 ⇒ 不声明 AUTH 这一行（内网免认证中继的形状）。
	authMechs []string
	// acceptAuth 为假 ⇒ AUTH 回 [authRejectText]。
	acceptAuth bool
	// authRejectText 是那句失败人话。**刻意**允许它含授权码原文：
	// 有的自建中继会把最后一条命令回显在错误应答里，脱敏那条用例照这个造。
	authRejectText string
	// rcptReject 非空 ⇒ **第二个** RCPT 回它（第一个照样收）。
	rcptReject string
	// dataReject 非空 ⇒ DATA 结束那一句回它（"552 mailbox full" 之类）。
	dataReject string
	// certHost 是证书签给谁；"" ⇒ [mailTestHost]。填别的名字用来验"名字不对必须拒"。
	certHost string
	// hangAfterGreeting 为真 ⇒ 打完招呼就拖着不回（超时/取消那两条用）。
	hangAfterGreeting bool
}

// dialMode 是一次拨号收到的"隐式 TLS 吗 + 端口"。
//
// 留着它是为了断言 [core.MailMessage.ImplicitTLS] 的判断**真的传到了**连接那一层：
// 那个纯函数在 `internal/core` 已经钉过，但"core 算对了、io 没照它连"是另一回事。
type dialMode struct {
	implicit bool
	port     int
}

// fakeSMTP 是一台跑起来的假服务器，生命周期由 `startFakeSMTP` 挂到 `t.Cleanup`。
type fakeSMTP struct {
	opts fakeSMTPOptions
	ln   net.Listener
	cert *x509.Certificate
	cfg  *tls.Config
	hang chan struct{}

	mu        sync.Mutex
	commands  []string // 依次收到的命令名（EHLO / STARTTLS / AUTH / MAIL / RCPT / DATA / QUIT）
	raw       []string // 原始命令行（含参数原文）
	body      string
	secure    bool // 服务器侧：当前这条连接读到的已经是 TLS
	rcptCount int
	dials     []dialMode

	authUser, authPass, authMech string
	authSteps                    []string // AUTH LOGIN 那两步的 base64 原文
}

// startFakeSMTP 起一台假服务器并接管它的生命周期。
func startFakeSMTP(t *testing.T, opts fakeSMTPOptions) *fakeSMTP {
	t.Helper()

	certHost := opts.certHost
	if certHost == "" {
		certHost = mailTestHost
	}
	cert, tlsCert := selfSignedMailCert(t, certHost)

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("假 SMTP 监听失败：%v", err)
	}
	srv := &fakeSMTP{
		ln:   ln,
		cert: cert,
		cfg:  &tls.Config{Certificates: []tls.Certificate{tlsCert}, MinVersion: tls.VersionTLS12},
		opts: opts,
		hang: make(chan struct{}),
	}
	go srv.serve()
	t.Cleanup(func() {
		_ = ln.Close()
		close(srv.hang)
	})
	return srv
}

// serve 收一条连接、说完话就结束。
//
// 一台服务器只服务一封信：与测试的"一次 SendMail"一一对应。要测第二封就再起一台，
// 不共享状态 —— 否则"这一封的正文"与"上一封的命令序列"会互相污染，
// 而那类污染在并行跑的时候才红。
func (s *fakeSMTP) serve() {
	conn, err := s.ln.Accept()
	if err != nil {
		return
	}
	defer func() { _ = conn.Close() }()

	rwc := conn
	if s.opts.implicit {
		tlsConn := tls.Server(rwc, s.cfg)
		if err := tlsConn.Handshake(); err != nil {
			return
		}
		s.setTLS(true)
		rwc = tlsConn
	}

	r := bufio.NewReader(rwc)
	w := bufio.NewWriterSize(rwc, 1<<14)
	writeLine(w, "220 fake ESMTP mcloud-test")
	if s.opts.hangAfterGreeting {
		<-s.hang
		return
	}

	for {
		line, err := r.ReadString('\n')
		if trimmed := strings.TrimRight(line, "\r\n"); trimmed != "" {
			s.record(trimmed)
		}
		if err != nil {
			return
		}
		if done := s.handle(rwc, r, w, strings.TrimRight(line, "\r\n")); done {
			return
		}
	}
}

// handle 回一句命令；返回 true 表示这条连接到此为止。
//
// ⚠ 升级之后**不能**换掉 `r`/`w` 这两个对象本身，只能 `Reset`：调用侧的循环拿着
// 的是同一对指针，重新赋值只在 handle 内部生效，下一行命令又会读回明文缓冲。
func (s *fakeSMTP) handle(conn net.Conn, r *bufio.Reader, w *bufio.Writer, line string) bool {
	verb, rest, _ := strings.Cut(line, " ")
	switch strings.ToUpper(verb) {
	case "EHLO", "HELO":
		s.replyHello(w, strings.ToUpper(verb) == "EHLO")
		return false

	case "STARTTLS":
		writeLine(w, "220 Ready to start TLS")
		tlsConn := tls.Server(conn, s.cfg)
		if err := tlsConn.Handshake(); err != nil {
			return true
		}
		s.setTLS(true)
		r.Reset(tlsConn)
		w.Reset(tlsConn)
		return false

	case "AUTH":
		return s.handleAuth(r, w, rest)

	case "MAIL":
		s.resetTransaction()
		writeLine(w, "250 OK")
		return false

	case "RCPT":
		return s.handleRcpt(w)

	case "DATA":
		writeLine(w, "354 Send body")
		body, ok := readUntilDot(r)
		if !ok {
			return true
		}
		s.setBody(body)
		if text := s.opts.dataReject; text != "" {
			writeLine(w, text)
			return false
		}
		writeLine(w, "250 OK: queued")
		return false

	case "QUIT":
		writeLine(w, "221 Bye")
		return true
	}
	// 认不出的命令（含 AUTH 失败之后客户端 abort 的那句 `*`）回 500 而不是断开：
	// 提前关连接会让**客户端**的错误变成 `EOF`，把真正那句 5xx 盖掉。
	writeLine(w, "500 Unrecognized command")
	return false
}

func (s *fakeSMTP) replyHello(w *bufio.Writer, ehlo bool) {
	if !ehlo {
		writeLine(w, "250 fake")
		return
	}
	lines := []string{"fake"}
	if s.opts.advertiseStartTLS && !s.tlsNow() {
		lines = append(lines, "STARTTLS")
	}
	// AUTH 只在**已经**是 TLS 的连接上声明（隐式 TLS 那一种从一开始就是）。
	// 这台假服务器刻意照这个形状：明文就给 AUTH 的服务器确实存在，
	// 但客户端**不该**依赖它 —— 见 `mail.go` 的"没有 TLS 就不登录"。
	if len(s.opts.authMechs) > 0 && s.tlsNow() {
		lines = append(lines, "AUTH "+strings.Join(s.opts.authMechs, " "))
	}
	lines = append(lines, "SIZE 3588576", "8BITMIME", "HELP")
	for i, l := range lines {
		if i == len(lines)-1 {
			writeLine(w, "250 "+l)
		} else {
			writeLine(w, "250-"+l)
		}
	}
}

// handleAuth 走两种机制：`AUTH PLAIN <b64>` 一句完成；`AUTH LOGIN` 两次挑战。
func (s *fakeSMTP) handleAuth(r *bufio.Reader, w *bufio.Writer, rest string) bool {
	mech, arg, _ := strings.Cut(rest, " ")
	s.setAuthMech(mech)
	switch strings.ToUpper(mech) {
	case "PLAIN":
		decoded, err := base64.StdEncoding.DecodeString(arg)
		if err != nil {
			writeLine(w, "501 AUTH PLAIN 的参数不是 base64")
			return false
		}
		// authzid \0 authcid \0 passwd —— 我们发的是 `\0user\0pass`（授权方留空）。
		s.setAuthSteps([]string{arg})
		parts := strings.Split(string(decoded), "\x00")
		if len(parts) == 3 {
			s.setAuthCreds(parts[1], parts[2])
		}
		return s.finishAuth(w)

	case "LOGIN":
		writeLine(w, "334 "+base64.StdEncoding.EncodeToString([]byte("Username:")))
		user, raw1, ok := s.readB64Line(r, w)
		if !ok {
			return true
		}
		writeLine(w, "334 "+base64.StdEncoding.EncodeToString([]byte("Password:")))
		pass, raw2, ok := s.readB64Line(r, w)
		if !ok {
			return true
		}
		s.setAuthSteps([]string{raw1, raw2})
		s.setAuthCreds(user, pass)
		return s.finishAuth(w)
	}
	// 声明了我们没实现的机制（XOAUTH2 之类）时客户端**不该**发这一枪；
	// 真发到这里来说明机制挑选写错了，回一句看得懂的而不是静默忽略。
	writeLine(w, "504 Unrecognized authentication type")
	return false
}

func (s *fakeSMTP) finishAuth(w *bufio.Writer) bool {
	if s.opts.acceptAuth {
		writeLine(w, "235 Authentication successful")
		return false
	}
	text := s.opts.authRejectText
	if text == "" {
		text = "535 Authentication failed"
	}
	writeLine(w, text)
	return false
}

func (s *fakeSMTP) handleRcpt(w *bufio.Writer) bool {
	s.mu.Lock()
	s.rcptCount++
	n := s.rcptCount
	s.mu.Unlock()
	if s.opts.rcptReject != "" && n == 2 {
		writeLine(w, s.opts.rcptReject)
		return false
	}
	writeLine(w, "250 OK")
	return false
}

// readB64Line 读一步挑战应答，同时把**原文**（那串 base64）带回去。
//
// 原文也要留：`loginAuth` 是本实现里唯一"标准库没有、我们自己写"的一段，
// 它的两步各发了什么必须能按字节回看，只断言解出来的用户名/口令不够 ——
// 那两个值对了也可能是"顺序反了但两边一样"。
func (s *fakeSMTP) readB64Line(r *bufio.Reader, w *bufio.Writer) (string, string, bool) {
	line, err := r.ReadString('\n')
	if err != nil {
		return "", "", false
	}
	raw := strings.TrimRight(line, "\r\n")
	decoded, err := base64.StdEncoding.DecodeString(raw)
	if err != nil {
		writeLine(w, "501 挑战应答不是 base64")
		return "", raw, false
	}
	return string(decoded), raw, true
}

// readUntilDot 读正文直到单独一行为止。
//
// 顺序**必须**是"先判终结符、再解填充"：反过来的话，正文里那一行 "."（被客户端
// 写成 ".."）会先被解回 "."，再被误判成结束 —— 表现成"信发出去了但只有前半段"，
// 而这恰好是点填充存在的理由。`TestLeadingDotIsStuffedAndDoesNotEndTheBody` 钉这一格。
func readUntilDot(r *bufio.Reader) (string, bool) {
	var b strings.Builder
	for {
		line, err := r.ReadString('\n')
		if line == ".\r\n" || line == ".\n" {
			return strings.TrimSuffix(b.String(), "\r\n"), true
		}
		if err != nil {
			return b.String(), false
		}
		if strings.HasPrefix(line, "..") {
			line = line[1:]
		}
		b.WriteString(line)
	}
}

func writeLine(w *bufio.Writer, text string) {
	_, _ = w.WriteString(text + "\r\n")
	_ = w.Flush()
}

// ─────────────────────────────────────────────────────────────────────────────
// 观测侧
//
// 全部加锁：断言在 SendMail 返回之后读，而握手跑在另一条 goroutine 上。
// 没有这把锁 `go test -race` 会先红，而那条红会让人误以为实现有并发问题。
// ─────────────────────────────────────────────────────────────────────────────

func (s *fakeSMTP) record(line string) {
	fields := strings.Fields(line)
	if len(fields) == 0 {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.commands = append(s.commands, strings.ToUpper(fields[0]))
	s.raw = append(s.raw, line)
}

func (s *fakeSMTP) setTLS(on bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.secure = on
}

func (s *fakeSMTP) tlsNow() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.secure
}

func (s *fakeSMTP) setAuthMech(mech string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.authMech = strings.ToUpper(mech)
}

func (s *fakeSMTP) setAuthCreds(user, pass string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.authUser, s.authPass = user, pass
}

func (s *fakeSMTP) setAuthSteps(steps []string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.authSteps = append([]string(nil), steps...)
}

func (s *fakeSMTP) setBody(body string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.body = body
}

func (s *fakeSMTP) resetTransaction() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.rcptCount = 0
}

func (s *fakeSMTP) recordDial(implicit bool, port int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.dials = append(s.dials, dialMode{implicit: implicit, port: port})
}

// seen 是"这一枪发过没有"。
func (s *fakeSMTP) seen(verb string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, c := range s.commands {
		if c == strings.ToUpper(verb) {
			return true
		}
	}
	return false
}

// commandOrder 是收到的命令名序列，用来断言"先 EHLO 再 STARTTLS 再 AUTH"这一类事。
func (s *fakeSMTP) commandOrder() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return strings.Join(s.commands, " ")
}

// rawLines 是收到的全部原文（含参数）。
func (s *fakeSMTP) rawLines() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.raw...)
}

func (s *fakeSMTP) creds() (string, string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.authUser, s.authPass
}

// authSteps 是 AUTH 那几步的 base64 原文（PLAIN 一步、LOGIN 两步）。
func (s *fakeSMTP) steps() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.authSteps...)
}

// authMechSeen 是服务器实际收到的机制名（大写）。
func (s *fakeSMTP) authMechSeen() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.authMech
}

// dialsSeen 是这台服务器被拨号时收到的"端口 + 要不要隐式 TLS"。
func (s *fakeSMTP) dialsSeen() []dialMode {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]dialMode(nil), s.dials...)
}

func (s *fakeSMTP) bodyText() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.body
}

// ─────────────────────────────────────────────────────────────────────────────
// 证书与拨号桩
// ─────────────────────────────────────────────────────────────────────────────

// selfSignedMailCert 现造一张**自签**证书（ECDSA P-256，前后各一小时）。
//
// 刻意不给 `ip:` SAN：连接地址是 127.0.0.1 而验证名字用 [mailTestHost]
// （由 [SMTPSender.mailTLSConfig] 从 Host 填进 ServerName）。于是"证书名对不对"
// 在测试里有唯一答案，而 [fakeSMTPOptions.certHost] 那一格能把它签错来验反向。
func selfSignedMailCert(t *testing.T, host string) (*x509.Certificate, tls.Certificate) {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatalf("造密钥失败：%v", err)
	}
	tmpl := &x509.Certificate{
		SerialNumber:          big.NewInt(1),
		Subject:               pkix.Name{CommonName: host},
		DNSNames:              []string{host},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign,
		BasicConstraintsValid: true,
		IsCA:                  true,
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		t.Fatalf("自签证书失败：%v", err)
	}
	parsed, err := x509.ParseCertificate(der)
	if err != nil {
		t.Fatalf("解析自己造的证书失败：%v", err)
	}
	return parsed, tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key}
}

// senderFor 造一个"只把地址换掉"的发送器。
//
// 换掉的**只有**"连到哪个地址"这一件事：证书仍由 [rootPool] 验、ServerName 仍按 Host 填、
// 端口仍决定要不要 TLS 直连 —— 那三样正是这一族最容易写反的地方。
func (s *fakeSMTP) senderFor(t *testing.T, clock core.Clock) *SMTPSender {
	t.Helper()
	pool := x509.NewCertPool()
	pool.AddCert(s.cert)
	addr := s.ln.Addr().String()

	return &SMTPSender{
		TLSConfig: &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12},
		Clock:     clock,
		dial: func(ctx context.Context, implicitTLS bool, _ string, port int,
			cfg *tls.Config, timeout time.Duration) (net.Conn, error) {
			s.recordDial(implicitTLS, port)
			dialer := &net.Dialer{Timeout: timeout}
			conn, err := dialer.DialContext(ctx, "tcp", addr)
			if err != nil {
				return nil, err
			}
			if s.opts.implicit != implicitTLS {
				// 端口与 TLS 模式**没**对上：桩不猜，直接失败。
				// 让假服务器去报一句握手错误，会被读成"实现的问题"。
				_ = conn.Close()
				return nil, fmt.Errorf("测试桩：端口 %d 要求的 TLS 模式与假服务器的形状不符", port)
			}
			if !implicitTLS {
				return conn, nil
			}
			tlsConn := tls.Client(conn, cfg)
			if err := tlsConn.HandshakeContext(ctx); err != nil {
				_ = conn.Close()
				return nil, err
			}
			return tlsConn, nil
		},
	}
}
