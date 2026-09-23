// 最小 WebSocket 客户端（RFC 6455）—— 只做**文本帧**，够用即止。
//
// 对应 Rust 侧的 `mcloud-io` 里那条 reqwest/tungstenite 通路（Go 侧手写）。
//
// # 为什么手写，不用第三方库
//
// ① 本仓库 Go 侧**零第三方新增**（AGENTS.md §3.4）；
// ② 这条链只需要**一个**动作：连上去、发几条文本、收一串文本。
//
//	完整的 WebSocket 库会带来自动重连、permessage-deflate、子协议协商、
//	二进制帧……每一样都是一处"和客户端不一样但没人发现"的地方；
//
// ③ 手写之后能**逐字节断言**握手与掩码（见 websocket_test.go）——
//
//	引库之后那些字节成了库的内部实现，只能"相信它是对的"。
//
// # ⚠⚠ 三处最容易写错、而且错了**都不会报错**的地方
//
//  1. **客户端发出的帧必须掩码**（RFC 6455 §5.3）。不掩码时**有些**服务端
//     会直接断开（表现为"连上就掉"），另一些会照常处理 ——
//     于是它在本地能跑、在真服务端上不行。
//  2. **服务端发出的帧必须不掩码**；收到了掩码帧要**报错**而不是照解。
//     照解会让一个"我们连到了非 WebSocket 的东西"变成"弹幕内容乱码"。
//  3. **`Sec-WebSocket-Accept` 必须校验**。不校验时，一个把我们领到
//     HTTP 404 页面 / 反代登录页的连接**照样"握手成功"**，
//     然后所有帧都解析失败 —— 而报错会指向"帧格式不对"，不指向"连错地方了"。
package mcloudio

import (
	"bufio"
	"context"
	"crypto/sha1"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// webSocketGUID 是 RFC 6455 §1.3 规定的固定串（不是密钥，就是常量）。
const webSocketGUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

// wsMaxMessageBytes 是一条消息的上限（分片拼完之后的长度）。
//
// 8 MiB 是**防御性**取值：弹幕每条几十字节，正常值差着 5 个数量级。
// 没有上限的话，一个坏掉的服务端（或中间设备）能让我们无限吃内存 ——
// 而这条链是**长连接**，泄漏会一直涨到进程被杀。
const wsMaxMessageBytes = 8 << 20

// wsMaxHandshakeBytes 是握手响应的上限。
//
// ⚠ 必须有：握手是"读到 `\r\n\r\n` 为止"，而对面可能回一个**不带**这个结尾的
// 无限流（反代挂了之类）。没有上限就是一次挂死。
const wsMaxHandshakeBytes = 16 << 10

// wsOpcode 是帧的 opcode（RFC 6455 §5.2）。
type wsOpcode byte

const (
	wsOpContinuation wsOpcode = 0x0
	wsOpText         wsOpcode = 0x1
	wsOpBinary       wsOpcode = 0x2
	wsOpClose        wsOpcode = 0x8
	wsOpPing         wsOpcode = 0x9
	wsOpPong         wsOpcode = 0xA
)

// WebSocketDialer 是 [core.WebSocketDialer] 的真实实现。
//
// 随机源从 [core.Random] 注入（握手 key 与掩码 key 都要随机）：
// 与设备指纹同一个理由 —— 注入之后整条握手可以**逐字节**对拍，
// 而"掩码有没有生效"才可能被断言。
type WebSocketDialer struct {
	rng core.Random
}

// NewWebSocketDialer 构造拨号器。
func NewWebSocketDialer(rng core.Random) *WebSocketDialer {
	return &WebSocketDialer{rng: rng}
}

// 编译期断言：必须满足纯层定义的契约。
var _ core.WebSocketDialer = (*WebSocketDialer)(nil)

// Dial 升级到 WebSocket。
func (d *WebSocketDialer) Dial(
	ctx context.Context, rawURL string, headers []core.HeaderPair,
) (core.WebSocketConn, error) {
	target, err := parseWebSocketURL(rawURL)
	if err != nil {
		return nil, err
	}

	conn, err := dialRaw(ctx, target)
	if err != nil {
		return nil, err
	}

	reader := bufio.NewReaderSize(conn, 4096)
	key, err := d.handshakeKey()
	if err != nil {
		_ = conn.Close()
		return nil, err
	}

	if err := writeHandshake(conn, target, key, headers); err != nil {
		_ = conn.Close()
		return nil, err
	}
	if err := verifyHandshake(reader, key); err != nil {
		_ = conn.Close()
		return nil, err
	}

	return &webSocketConn{conn: conn, reader: reader, rng: d.rng}, nil
}

// wsTarget 是从 URL 解出来的连接目标。
type wsTarget struct {
	secure bool
	// hostPort 是 `net.Dial` 用的地址（**总是**带端口）。
	hostPort string
	// hostHeader 是 `Host:` 头的值（URL 里没写端口时**不带**端口 —— 与浏览器一致）。
	hostHeader string
	// serverName 是 TLS SNI / 证书校验用的主机名（不含端口）。
	serverName string
	// requestURI 是请求行的路径（含查询串，不含 fragment）。
	requestURI string
}

func parseWebSocketURL(rawURL string) (wsTarget, error) {
	parsed, err := url.Parse(rawURL)
	if err != nil {
		return wsTarget{}, fmt.Errorf("WebSocket URL 解析失败: %w", err)
	}
	var target wsTarget
	switch parsed.Scheme {
	case "wss":
		target.secure = true
	case "ws":
		target.secure = false
	default:
		return wsTarget{}, fmt.Errorf("不支持的 WebSocket scheme: %q", parsed.Scheme)
	}
	if parsed.Host == "" {
		return wsTarget{}, errors.New("WebSocket URL 缺少 host")
	}

	target.serverName = parsed.Hostname()
	target.hostHeader = parsed.Host
	target.hostPort = parsed.Host
	if parsed.Port() == "" {
		if target.secure {
			target.hostPort = net.JoinHostPort(target.serverName, "443")
		} else {
			target.hostPort = net.JoinHostPort(target.serverName, "80")
		}
	}
	target.requestURI = parsed.EscapedPath()
	if target.requestURI == "" {
		target.requestURI = "/"
	}
	if parsed.RawQuery != "" {
		target.requestURI += "?" + parsed.RawQuery
	}
	return target, nil
}

func dialRaw(ctx context.Context, target wsTarget) (net.Conn, error) {
	dialer := &net.Dialer{Timeout: 15 * time.Second}
	conn, err := dialer.DialContext(ctx, "tcp", target.hostPort)
	if err != nil {
		return nil, fmt.Errorf("WebSocket 连接 %s 失败: %w", target.hostPort, err)
	}
	if !target.secure {
		return conn, nil
	}

	tlsConn := tls.Client(conn, &tls.Config{ServerName: target.serverName, MinVersion: tls.VersionTLS12})
	if err := tlsConn.HandshakeContext(ctx); err != nil {
		_ = conn.Close()
		return nil, fmt.Errorf("WebSocket TLS 握手失败: %w", err)
	}
	return tlsConn, nil
}

// handshakeKey 生成 16 字节随机数的 base64（RFC 6455 §4.1 要求 16 字节）。
func (d *WebSocketDialer) handshakeKey() (string, error) {
	if d.rng == nil {
		return "", errors.New("WebSocketDialer 没有随机源")
	}
	var raw [16]byte
	d.rng.FillBytes(raw[:])
	return base64.StdEncoding.EncodeToString(raw[:]), nil
}

func writeHandshake(
	conn net.Conn, target wsTarget, key string, headers []core.HeaderPair,
) error {
	var request strings.Builder
	request.WriteString("GET " + target.requestURI + " HTTP/1.1\r\n")
	request.WriteString("Host: " + target.hostHeader + "\r\n")
	request.WriteString("Upgrade: websocket\r\n")
	request.WriteString("Connection: Upgrade\r\n")
	request.WriteString("Sec-WebSocket-Key: " + key + "\r\n")
	request.WriteString("Sec-WebSocket-Version: 13\r\n")
	for _, pair := range headers {
		// ⚠ 握手必需的那几个头**不让调用方覆盖**：覆盖掉 `Sec-WebSocket-Key`
		// 会让 accept 校验必然失败（报错指向"服务端不认"，不指向"我们自己改坏了"）。
		if isReservedHandshakeHeader(pair[0]) {
			continue
		}
		request.WriteString(pair[0] + ": " + pair[1] + "\r\n")
	}
	request.WriteString("\r\n")

	if _, err := io.WriteString(conn, request.String()); err != nil {
		return fmt.Errorf("WebSocket 握手请求发送失败: %w", err)
	}
	return nil
}

func isReservedHandshakeHeader(name string) bool {
	switch strings.ToLower(strings.TrimSpace(name)) {
	case "host", "upgrade", "connection", "sec-websocket-key",
		"sec-websocket-version", "sec-websocket-protocol", "sec-websocket-extensions":
		return true
	}
	return false
}

// verifyHandshake 读并校验 101 响应。
//
// 校验三件事：状态码是 101、`Upgrade: websocket`、`Sec-WebSocket-Accept` 等于
// `base64(sha1(key + GUID))`。第三条是**唯一**能证明"对面真的按 WebSocket 回了"
// 的东西 —— 少了它，一个 302 之后的登录页也能被当成握手成功。
func verifyHandshake(reader *bufio.Reader, key string) error {
	statusLine, headers, err := readHandshakeResponse(reader)
	if err != nil {
		return err
	}
	if !strings.Contains(statusLine, " 101") {
		return fmt.Errorf("WebSocket 握手被拒（状态行 %q）", statusLine)
	}
	if !strings.EqualFold(headerValue(headers, "upgrade"), "websocket") {
		return fmt.Errorf("WebSocket 握手响应缺少 Upgrade: websocket（实得 %q）",
			headerValue(headers, "upgrade"))
	}
	want := webSocketAccept(key)
	if got := headerValue(headers, "sec-websocket-accept"); got != want {
		return fmt.Errorf("Sec-WebSocket-Accept 不匹配：期望 %q 实得 %q（多半连到了非 WebSocket 端点）",
			want, got)
	}
	return nil
}

// readHandshakeResponse 读到空行为止，返回状态行与头。
//
// ⚠ 手写而不用 `http.ReadResponse`：后者要一个 `*http.Request` 且会顺带处理
// 分块编码、重定向语义 —— 而 101 之后**剩下的字节就是帧**，
// 任何一层缓冲都可能把它们吃掉。这里只用一个 bufio.Reader 读到头，
// 之后同一个 reader 直接交给帧解析。
func readHandshakeResponse(reader *bufio.Reader) (string, []core.HeaderPair, error) {
	var (
		statusLine string
		headers    []core.HeaderPair
		total      int
	)
	for {
		line, err := reader.ReadString('\n')
		total += len(line)
		if total > wsMaxHandshakeBytes {
			return "", nil, errors.New("WebSocket 握手响应超过上限（多半不是 WebSocket 端点）")
		}
		if err != nil {
			return "", nil, fmt.Errorf("WebSocket 握手响应读取失败: %w", err)
		}
		trimmed := strings.TrimRight(line, "\r\n")
		if trimmed == "" {
			break
		}
		if statusLine == "" {
			statusLine = trimmed
			continue
		}
		name, value, ok := strings.Cut(trimmed, ":")
		if !ok {
			continue
		}
		headers = append(headers, core.HeaderPair{
			strings.TrimSpace(name), strings.TrimSpace(value),
		})
	}
	if statusLine == "" {
		return "", nil, errors.New("WebSocket 握手响应为空")
	}
	return statusLine, headers, nil
}

func headerValue(headers []core.HeaderPair, name string) string {
	for _, pair := range headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1]
		}
	}
	return ""
}

// webSocketAccept 算 `Sec-WebSocket-Accept`（RFC 6455 §4.2.2）。
func webSocketAccept(key string) string {
	sum := sha1.Sum([]byte(key + webSocketGUID))
	return base64.StdEncoding.EncodeToString(sum[:])
}

// ─────────────────────────────────────────────────────────────────────────────
// 连接
// ─────────────────────────────────────────────────────────────────────────────

type webSocketConn struct {
	conn   net.Conn
	reader *bufio.Reader
	rng    core.Random

	writeMu sync.Mutex
	closed  bool
}

var _ core.WebSocketConn = (*webSocketConn)(nil)

// WriteText 发一条**掩码**的文本帧。
//
// ⚠ 加锁：SockJS 的心跳回包与 PROVISION/UNREAD 可能从不同 goroutine 发出，
// 两帧交错在线上就是一条坏报文，而它表现为"服务端忽然不推了"。
func (c *webSocketConn) WriteText(ctx context.Context, text string) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()

	if c.closed {
		return errors.New("WebSocket 已关闭")
	}
	if deadline, ok := ctx.Deadline(); ok {
		_ = c.conn.SetWriteDeadline(deadline)
		defer func() { _ = c.conn.SetWriteDeadline(time.Time{}) }()
	}

	frame, err := c.encodeTextFrame([]byte(text))
	if err != nil {
		return err
	}
	if _, err := c.conn.Write(frame); err != nil {
		return fmt.Errorf("WebSocket 发送失败: %w", err)
	}
	return nil
}

// encodeTextFrame 按 RFC 6455 §5.2 编一条掩码文本帧。
func (c *webSocketConn) encodeTextFrame(payload []byte) ([]byte, error) {
	if c.rng == nil {
		return nil, errors.New("WebSocketConn 没有随机源（掩码 key 取不到）")
	}
	var mask [4]byte
	c.rng.FillBytes(mask[:])

	out := make([]byte, 0, len(payload)+14)
	out = append(out, 0x80|byte(wsOpText)) // FIN=1, RSV=0, opcode=text
	switch {
	case len(payload) < 126:
		out = append(out, 0x80|byte(len(payload)))
	case len(payload) <= 0xFFFF:
		out = append(out, 0x80|126)
		out = binary.BigEndian.AppendUint16(out, uint16(len(payload)))
	default:
		out = append(out, 0x80|127)
		out = binary.BigEndian.AppendUint64(out, uint64(len(payload)))
	}
	out = append(out, mask[:]...)
	// 就地掩码：out 的 payload 段还没写，先预留再逐字节异或。
	start := len(out)
	out = append(out, payload...)
	for i := range out[start:] {
		out[start+i] ^= mask[i%4]
	}
	return out, nil
}

// ReadText 收一条完整消息。
//
// 分片（continuation）在这里拼完；ping 内部回 pong；close 回一个 close 并返回
// `io.EOF`。**控制帧可以插在分片中间**（RFC 6455 §5.4），所以拼装循环
// 不能假设"下一条一定是 continuation"。
func (c *webSocketConn) ReadText(ctx context.Context) (string, error) {
	if deadline, ok := ctx.Deadline(); ok {
		_ = c.conn.SetReadDeadline(deadline)
		defer func() { _ = c.conn.SetReadDeadline(time.Time{}) }()
	}

	var (
		assembled []byte
		fragments bool
	)
	for {
		fin, opcode, payload, err := c.readFrame()
		if err != nil {
			return "", err
		}

		switch opcode {
		case wsOpPing:
			if err := c.writeControl(wsOpPong, payload); err != nil {
				return "", err
			}
			continue
		case wsOpPong:
			// 我们不发 ping（SockJS 的 `h` 是应用层心跳），收到就忽略。
			continue
		case wsOpClose:
			_ = c.writeControl(wsOpClose, nil)
			_ = c.Close()
			return "", io.EOF
		case wsOpText:
			if fragments {
				return "", errors.New("WebSocket 帧序非法：分片未结束又来了新的文本帧")
			}
			assembled = payload
		case wsOpBinary:
			// SockJS 只用文本帧。二进制帧意味着对面不是我们以为的那个端点。
			return "", errors.New("WebSocket 收到二进制帧（SockJS 传输只用文本帧）")
		case wsOpContinuation:
			if !fragments {
				return "", errors.New("WebSocket 帧序非法：没有起始帧的 continuation")
			}
			assembled = append(assembled, payload...)
		default:
			return "", fmt.Errorf("WebSocket 未知 opcode 0x%x", byte(opcode))
		}

		if len(assembled) > wsMaxMessageBytes {
			return "", fmt.Errorf("WebSocket 消息超过上限 %d 字节", wsMaxMessageBytes)
		}
		if fin {
			return string(assembled), nil
		}
		fragments = true
	}
}

// writeControl 发一条**控制帧**（必须掩码，且长度 ≤ 125）。
func (c *webSocketConn) writeControl(opcode wsOpcode, payload []byte) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if c.closed {
		return nil // close 之后的控制帧没有意义，也不算错误
	}
	if len(payload) > 125 {
		payload = payload[:125]
	}
	var mask [4]byte
	if c.rng != nil {
		c.rng.FillBytes(mask[:])
	}
	out := make([]byte, 0, len(payload)+6)
	out = append(out, 0x80|byte(opcode), 0x80|byte(len(payload)))
	out = append(out, mask[:]...)
	start := len(out)
	out = append(out, payload...)
	for i := range out[start:] {
		out[start+i] ^= mask[i%4]
	}
	_, err := c.conn.Write(out)
	return err
}

// readFrame 读一帧，返回 (FIN, opcode, payload)。
func (c *webSocketConn) readFrame() (bool, wsOpcode, []byte, error) {
	var header [2]byte
	if _, err := io.ReadFull(c.reader, header[:]); err != nil {
		if errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) {
			// 对端直接断开（没发 close 帧）—— 与收到 close 帧同样是**正常结束**。
			return false, 0, nil, io.EOF
		}
		return false, 0, nil, fmt.Errorf("WebSocket 读帧头失败: %w", err)
	}

	fin := header[0]&0x80 != 0
	if header[0]&0x70 != 0 {
		return false, 0, nil, errors.New("WebSocket 帧设置了 RSV 位（没有协商任何扩展）")
	}
	opcode := wsOpcode(header[0] & 0x0F)
	masked := header[1]&0x80 != 0
	length := uint64(header[1] & 0x7F)

	switch length {
	case 126:
		var ext [2]byte
		if _, err := io.ReadFull(c.reader, ext[:]); err != nil {
			return false, 0, nil, fmt.Errorf("WebSocket 读 16 位长度失败: %w", err)
		}
		length = uint64(binary.BigEndian.Uint16(ext[:]))
	case 127:
		var ext [8]byte
		if _, err := io.ReadFull(c.reader, ext[:]); err != nil {
			return false, 0, nil, fmt.Errorf("WebSocket 读 64 位长度失败: %w", err)
		}
		length = binary.BigEndian.Uint64(ext[:])
	}

	// ⚠ 服务端帧**必须不掩码**（RFC 6455 §5.1）。照解掩码帧会让
	// "连到了非 WebSocket 的东西"变成"弹幕内容乱码"，排查方向被彻底带偏。
	if masked {
		return false, 0, nil, errors.New("WebSocket 服务端帧带了掩码位（违反 RFC 6455 §5.1）")
	}
	if length > wsMaxMessageBytes {
		return false, 0, nil, fmt.Errorf("WebSocket 帧长度 %d 超过上限", length)
	}

	payload := make([]byte, length)
	if _, err := io.ReadFull(c.reader, payload); err != nil {
		return false, 0, nil, fmt.Errorf("WebSocket 读帧体失败: %w", err)
	}
	return fin, opcode, payload, nil
}

// Close 关连接。幂等。
func (c *webSocketConn) Close() error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if c.closed {
		return nil
	}
	c.closed = true
	return c.conn.Close()
}
