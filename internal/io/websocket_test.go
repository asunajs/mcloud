package mcloudio

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"io"
	"net"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 测试脚手架：一个**只说字节**的假 WebSocket 服务端
// ─────────────────────────────────────────────────────────────────────────────
//
// 刻意不用任何 WebSocket 库：这条客户端是手写的，而它要防的正是
// "字节与原客户端不一样"（掩码、accept、分片）。用库去测等于让被测方与
// 判据方共用同一套实现 —— 那两边一起错的时候，测试是绿的。

// wsTestRng 是确定性随机源（掩码 key 固定，于是发送的帧可以逐字节断言）。
type wsTestRng struct{ next byte }

func (r *wsTestRng) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

// wsFakeServer 起一个 loopback 监听并接受一条连接。
type wsFakeServer struct {
	listener net.Listener
	conn     net.Conn
	reader   *bufio.Reader
	// key 是客户端发来的 `Sec-WebSocket-Key`。
	key string
	// requestLines 是握手请求的原始行（含请求行），供断言头集合。
	requestLines []string
}

func newWSFakeServer(t *testing.T, accept string, extraResponseHeaders string) *wsFakeServer {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("监听失败: %v", err)
	}
	t.Cleanup(func() { _ = listener.Close() })

	server := &wsFakeServer{listener: listener}
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		server.conn = conn
		server.reader = bufio.NewReader(conn)

		for {
			line, err := server.reader.ReadString('\n')
			if err != nil {
				return
			}
			trimmed := strings.TrimRight(line, "\r\n")
			if trimmed == "" {
				break
			}
			server.requestLines = append(server.requestLines, trimmed)
			if name, value, ok := strings.Cut(trimmed, ":"); ok &&
				strings.EqualFold(strings.TrimSpace(name), "sec-websocket-key") {
				server.key = strings.TrimSpace(value)
			}
		}
		if accept == "" {
			accept = webSocketAccept(server.key)
		}
		_, _ = io.WriteString(conn,
			"HTTP/1.1 101 Switching Protocols\r\n"+
				"Upgrade: websocket\r\n"+
				"Connection: Upgrade\r\n"+
				"Sec-WebSocket-Accept: "+accept+"\r\n"+
				extraResponseHeaders+
				"\r\n")
	}()
	return server
}

func (s *wsFakeServer) url() string {
	return "ws://" + s.listener.Addr().String() + "/ws/123/abc/websocket"
}

// awaitConn 等到服务端拿到连接（避免测试与 accept 抢跑）。
func (s *wsFakeServer) awaitConn(t *testing.T) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for s.conn == nil {
		if time.Now().After(deadline) {
			t.Fatal("假服务端没收到连接")
		}
		time.Sleep(time.Millisecond)
	}
}

// readClientFrame 读一条**客户端**帧并返回 (opcode, 未掩码的 payload)。
//
// 同时断言客户端帧的两条 RFC 要求：MASK 位必须为 1、掩码必须真的生效。
func (s *wsFakeServer) readClientFrame(t *testing.T) (wsOpcode, []byte) {
	t.Helper()
	var header [2]byte
	if _, err := io.ReadFull(s.reader, header[:]); err != nil {
		t.Fatalf("读客户端帧头失败: %v", err)
	}
	if header[1]&0x80 == 0 {
		t.Fatal("客户端帧没有掩码位 —— 违反 RFC 6455 §5.3（有些服务端会直接断开）")
	}
	opcode := wsOpcode(header[0] & 0x0F)
	length := uint64(header[1] & 0x7F)
	switch length {
	case 126:
		var ext [2]byte
		_, _ = io.ReadFull(s.reader, ext[:])
		length = uint64(binary.BigEndian.Uint16(ext[:]))
	case 127:
		var ext [8]byte
		_, _ = io.ReadFull(s.reader, ext[:])
		length = binary.BigEndian.Uint64(ext[:])
	}
	var mask [4]byte
	if _, err := io.ReadFull(s.reader, mask[:]); err != nil {
		t.Fatalf("读掩码 key 失败: %v", err)
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(s.reader, payload); err != nil {
		t.Fatalf("读帧体失败: %v", err)
	}
	// 掩码是自反的：再异或一次就该回到原文。
	for i := range payload {
		payload[i] ^= mask[i%4]
	}
	return opcode, payload
}

// writeServerFrame 从服务端发一帧（**不掩码**，RFC 6455 §5.1）。
func (s *wsFakeServer) writeServerFrame(t *testing.T, fin bool, opcode wsOpcode, payload []byte) {
	t.Helper()
	first := byte(opcode)
	if fin {
		first |= 0x80
	}
	out := []byte{first}
	switch {
	case len(payload) < 126:
		out = append(out, byte(len(payload)))
	case len(payload) <= 0xFFFF:
		out = append(out, 126)
		out = binary.BigEndian.AppendUint16(out, uint16(len(payload)))
	default:
		out = append(out, 127)
		out = binary.BigEndian.AppendUint64(out, uint64(len(payload)))
	}
	out = append(out, payload...)
	if _, err := s.conn.Write(out); err != nil {
		t.Fatalf("服务端写帧失败: %v", err)
	}
}

func wsTestContext(t *testing.T) context.Context {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	return ctx
}

// ─────────────────────────────────────────────────────────────────────────────
// 握手
// ─────────────────────────────────────────────────────────────────────────────

// TestWebSocketAcceptMatchesRFCExample 钉住 accept 算法本身。
//
// 期望值是 **RFC 6455 §1.3 里的那个例子**（key `dGhlIHNhbXBsZSBub25jZQ==`
// → accept `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`）。
//
// ⚠ 这条用 RFC 的固定值而不是"自己算一遍再比"：后者是恒真的
// —— 算法错了它也跟着错，只是错得一致。
func TestWebSocketAcceptMatchesRFCExample(t *testing.T) {
	got := webSocketAccept("dGhlIHNhbXBsZSBub25jZQ==")
	want := "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
	if got != want {
		t.Fatalf("Sec-WebSocket-Accept 期望 %q，实得 %q", want, got)
	}
}

// TestWebSocketHandshakeAndMaskedTextFrame 是这条客户端最硬的一条。
//
// 反向验证：把 `encodeTextFrame` 里的 `0x80|byte(len(payload))` 改成
// `byte(len(payload))`（去掉 MASK 位）⇒ 假服务端的 `readClientFrame` 立刻报错。
func TestWebSocketHandshakeAndMaskedTextFrame(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})

	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), []core.HeaderPair{
		{"User-Agent", "test-agent"},
		{"Origin", "https://v.cmcc-cs.cn"},
	})
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	// 握手请求必须带齐四个头。
	joined := strings.Join(server.requestLines, "\n")
	for _, want := range []string{
		"GET /ws/123/abc/websocket HTTP/1.1",
		"Upgrade: websocket",
		"Connection: Upgrade",
		"Sec-WebSocket-Version: 13",
		"User-Agent: test-agent",
		"Origin: https://v.cmcc-cs.cn",
	} {
		if !strings.Contains(joined, want) {
			t.Errorf("握手请求缺少 %q\n实得:\n%s", want, joined)
		}
	}
	// key 必须是 16 字节的 base64（RFC 6455 §4.1）。
	rawKey, err := base64.StdEncoding.DecodeString(server.key)
	if err != nil || len(rawKey) != 16 {
		t.Fatalf("Sec-WebSocket-Key 不是 16 字节 base64: %q", server.key)
	}

	if err := conn.WriteText(ctx, "o"); err != nil {
		t.Fatalf("WriteText 失败: %v", err)
	}
	opcode, payload := server.readClientFrame(t)
	if opcode != wsOpText {
		t.Fatalf("opcode 期望 text，实得 0x%x", byte(opcode))
	}
	if string(payload) != "o" {
		t.Fatalf("payload 期望 %q，实得 %q", "o", payload)
	}
}

// TestWebSocketRejectsWrongAccept 钉住"必须校验 accept"。
//
// 反向验证：把 `verifyHandshake` 里那条 accept 比较删掉 ⇒ 本用例变红。
// 它防的是"连到了非 WebSocket 端点"（反代登录页 / 404 页），
// 那种情况下不校验也"握手成功"，然后所有帧解析失败 ——
// 报错会指向帧格式，不指向连错地方。
func TestWebSocketRejectsWrongAccept(t *testing.T) {
	server := newWSFakeServer(t, "AAAAAAAAAAAAAAAAAAAAAAAAAAA=", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})

	_, err := dialer.Dial(wsTestContext(t), server.url(), nil)
	if err == nil {
		t.Fatal("accept 不匹配时必须 Dial 失败")
	}
	if !strings.Contains(err.Error(), "Sec-WebSocket-Accept") {
		t.Fatalf("错误信息应指向 accept 校验，实得: %v", err)
	}
}

// TestWebSocketRejectsNon101 钉住状态码检查。
func TestWebSocketRejectsNon101(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("监听失败: %v", err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer func() { _ = conn.Close() }()
		reader := bufio.NewReader(conn)
		for {
			line, err := reader.ReadString('\n')
			if err != nil || strings.TrimRight(line, "\r\n") == "" {
				break
			}
		}
		_, _ = io.WriteString(conn, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
	}()

	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	_, err = dialer.Dial(wsTestContext(t), "ws://"+listener.Addr().String()+"/ws", nil)
	if err == nil {
		t.Fatal("非 101 响应必须 Dial 失败")
	}
	if !strings.Contains(err.Error(), "404") {
		t.Fatalf("错误信息应带回实际状态行，实得: %v", err)
	}
}

// TestWebSocketRefusesToOverrideHandshakeHeaders 钉住"必需头不可被调用方覆盖"。
//
// ⚠ 这条防的是一个**静默**问题：调用方传 `Sec-WebSocket-Key` 覆盖掉之后，
// 我们自己算的 accept 必然对不上，而报错会指向"服务端不认我们"。
func TestWebSocketRefusesToOverrideHandshakeHeaders(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})

	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), []core.HeaderPair{
		{"Sec-WebSocket-Key", "bogus"},
		{"Host", "evil.example"},
	})
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	joined := strings.Join(server.requestLines, "\n")
	if strings.Contains(joined, "bogus") {
		t.Error("调用方传的 Sec-WebSocket-Key 覆盖了握手 key")
	}
	if strings.Contains(joined, "evil.example") {
		t.Error("调用方传的 Host 覆盖了握手 Host")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 帧
// ─────────────────────────────────────────────────────────────────────────────

// TestWebSocketRejectsMaskedServerFrame 钉住"服务端帧不许掩码"。
//
// 反向验证：把 `readFrame` 里那条 `if masked { return err }` 删掉 ⇒ 本用例变红。
func TestWebSocketRejectsMaskedServerFrame(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), nil)
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	// 手工发一条**带掩码位**的服务端帧（RFC 禁止）。
	_, _ = server.conn.Write([]byte{0x80 | byte(wsOpText), 0x80 | 4, 1, 2, 3, 4, 'a' ^ 1, 'b' ^ 2, 'c' ^ 3, 'd' ^ 4})

	_, err = conn.ReadText(ctx)
	if err == nil {
		t.Fatal("收到掩码的服务端帧必须报错")
	}
	if !strings.Contains(err.Error(), "掩码") {
		t.Fatalf("错误信息应指向掩码，实得: %v", err)
	}
}

// TestWebSocketAssemblesFragments 钉住分片拼装。
//
// 反向验证：把 `wsOpContinuation` 那一支改成 `continue`（丢弃后续分片）
// ⇒ 本用例变红（拿到的会是半条）。
func TestWebSocketAssemblesFragments(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), nil)
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	server.writeServerFrame(t, false, wsOpText, []byte("a[\"QUJD"))
	// ⚠ 分片中间插一条 ping：RFC 6455 §5.4 允许，而"下一条一定是 continuation"
	// 的写法会在真服务端上偶发失败。
	server.writeServerFrame(t, true, wsOpPing, []byte("x"))
	server.writeServerFrame(t, true, wsOpContinuation, []byte("REVG\"]"))

	got, err := conn.ReadText(ctx)
	if err != nil {
		t.Fatalf("ReadText 失败: %v", err)
	}
	if got != `a["QUJDREVG"]` {
		t.Fatalf("分片拼装结果期望 %q，实得 %q", `a["QUJDREVG"]`, got)
	}

	// 客户端必须回了一个 pong（否则真服务端会认为链路断了）。
	opcode, payload := server.readClientFrame(t)
	if opcode != wsOpPong {
		t.Fatalf("ping 应当回 pong，实得 opcode 0x%x", byte(opcode))
	}
	if string(payload) != "x" {
		t.Fatalf("pong 应当原样带回 ping 的载荷，实得 %q", payload)
	}
}

// TestWebSocketCloseFrameIsEOF 钉住"收到 close 是**正常结束**"。
//
// 反向验证：把 close 那一支的 `return "", io.EOF` 改成 `return "", errors.New(...)`
// ⇒ 本用例变红。它防的是"直播结束后被当成错误去重连"。
func TestWebSocketCloseFrameIsEOF(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), nil)
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	server.writeServerFrame(t, true, wsOpClose, nil)
	if _, err := conn.ReadText(ctx); !errors.Is(err, io.EOF) {
		t.Fatalf("close 帧应当返回 io.EOF，实得 %v", err)
	}
}

// TestWebSocketPeerDisconnectIsEOF 钉住"对端直接断开"也是 io.EOF。
//
// 与上一条分开：这是**没有** close 帧的那条路（网络断了 / 服务端进程被杀），
// 它比 close 帧常见得多。
func TestWebSocketPeerDisconnectIsEOF(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), nil)
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	_ = server.conn.Close()
	if _, err := conn.ReadText(ctx); !errors.Is(err, io.EOF) {
		t.Fatalf("对端断开应当返回 io.EOF，实得 %v", err)
	}
}

// TestWebSocketRejectsBinaryFrame 钉住"二进制帧意味着连错了地方"。
func TestWebSocketRejectsBinaryFrame(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), nil)
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	defer func() { _ = conn.Close() }()
	server.awaitConn(t)

	server.writeServerFrame(t, true, wsOpBinary, []byte{1, 2, 3})
	if _, err := conn.ReadText(ctx); err == nil {
		t.Fatal("二进制帧必须报错")
	}
}

// TestWebSocketWriteTextAfterCloseFails 钉住关闭后的行为（不是静默成功）。
func TestWebSocketWriteTextAfterCloseFails(t *testing.T) {
	server := newWSFakeServer(t, "", "")
	dialer := NewWebSocketDialer(&wsTestRng{next: 1})
	ctx := wsTestContext(t)
	conn, err := dialer.Dial(ctx, server.url(), nil)
	if err != nil {
		t.Fatalf("Dial 失败: %v", err)
	}
	server.awaitConn(t)

	if err := conn.Close(); err != nil {
		t.Fatalf("Close 失败: %v", err)
	}
	if err := conn.Close(); err != nil {
		t.Fatalf("Close 必须幂等，第二次实得: %v", err)
	}
	if err := conn.WriteText(ctx, "o"); err == nil {
		t.Fatal("关闭后写必须报错，不能静默成功")
	}
}

// TestParseWebSocketURLFillsDefaults 钉住 URL 解析的三个默认值。
//
// ⚠ 这三条**都会静默出错**：端口补错是连不上（看得出来），
// 而 `Host:` 头多带一个 `:443` 是**能连上但服务端可能不认**
// —— 浏览器在 URL 里没写端口时**不带**端口，这里照抄。
func TestParseWebSocketURLFillsDefaults(t *testing.T) {
	cases := []struct {
		url        string
		hostPort   string
		hostHeader string
		serverName string
		requestURI string
		secure     bool
	}{
		{
			"wss://videoim.cmcc-cs.cn:20102/ws/123/abc/websocket",
			"videoim.cmcc-cs.cn:20102", "videoim.cmcc-cs.cn:20102",
			"videoim.cmcc-cs.cn", "/ws/123/abc/websocket", true,
		},
		{
			"wss://example.com/ws",
			"example.com:443", "example.com", "example.com", "/ws", true,
		},
		{
			"ws://127.0.0.1/path?x=1",
			"127.0.0.1:80", "127.0.0.1", "127.0.0.1", "/path?x=1", false,
		},
		{
			"wss://example.com",
			"example.com:443", "example.com", "example.com", "/", true,
		},
	}
	for _, c := range cases {
		t.Run(c.url, func(t *testing.T) {
			got, err := parseWebSocketURL(c.url)
			if err != nil {
				t.Fatalf("解析失败: %v", err)
			}
			if got.hostPort != c.hostPort || got.hostHeader != c.hostHeader ||
				got.serverName != c.serverName || got.requestURI != c.requestURI ||
				got.secure != c.secure {
				t.Fatalf("期望 %+v，实得 %+v", c, got)
			}
		})
	}
}

// TestParseWebSocketURLRejectsNonWebSocketScheme 钉住 scheme 白名单。
func TestParseWebSocketURLRejectsNonWebSocketScheme(t *testing.T) {
	for _, raw := range []string{"https://example.com/ws", "http://example.com/ws", "ftp://x/y", "wss://"} {
		if _, err := parseWebSocketURL(raw); err == nil {
			t.Errorf("%q 应当被拒", raw)
		}
	}
}
