// WebSocket 的**契约**（纯层定义，实现在 internal/io）。
//
// # 为什么要有这一对接口，而不是让上游层直接开连接
//
// 这条链最难验证的部分是**认帧**（SockJS 的四种帧、MSync 的 command 分派、
// 三级解码兜底、跨帧口令拼接）—— 它们全是纯函数，已经在
// [upstream.KoulingListener] 里做成了可断言的返回值。
// 把连接也放进去的话，"收到 `a["…"]` 之后该回什么"就只能靠**真的开一条 WS**
// 来验证，而这条 WS 只在**主播开播那一小时**才存在
// （2026-09-22 抓包时 `roomBeat.object.status = 0`、`chatRoomId = null`，
// WS 结构性地不存在，所以它连一次都没被跑过）。
//
// 拆成"契约在纯层、实现走 net"之后：
//   - 帧序列用一串字面量就能跑完整条时序（见 upstream 的帧状态机测试）；
//   - 换一个实现（回放夹具、内存管道）不需要改上游层一个字符。
//
// # 只做文本帧
//
// SockJS 的 websocket 传输只用**文本帧**，所以这里刻意不暴露二进制 ——
// 多一个 opcode 就多一处"两边对不上但没人发现"的地方。
// ping/pong/close 由实现内部处理（它们不是给业务看的）。
package core

import "context"

// WebSocketConn 是一条**已升级完成**的 WebSocket 连接。
//
// 实现必须保证：
//   - 并发调用 WriteText 是安全的（帧不能交错）；
//   - ReadText 返回 `io.EOF` 表示对端**正常**关闭（收到 close 帧或连接断开），
//     而不是"这一帧没数据"；
//   - Close 幂等。
type WebSocketConn interface {
	// WriteText 发一条文本帧。
	WriteText(ctx context.Context, text string) error
	// ReadText 收一条完整消息（跨帧分片由实现拼好）。
	//
	// ⚠ 返回 `io.EOF` 是**正常结束**：对端发 close 帧、或连接被对端关掉。
	// 调用方（[upstream.KoulingListener] 的驱动）据此收尾，
	// 而不是把它当成错误重连 —— 直播结束后服务端就是这么关的。
	ReadText(ctx context.Context) (string, error)
	// Close 关闭连接。幂等，且**不等待**对端回 close 帧。
	Close() error
}

// WebSocketDialer 建 WebSocket 连接。
//
// ⚠ 签名里没有"超时"参数：超时走 `ctx`。理由与 [Clock] 同源 ——
// 一段时间的唯一载体是 `time.Duration`/`ctx`，多一个裸毫秒参数
// 就多一处"有人按秒传了"的机会。
type WebSocketDialer interface {
	// Dial 升级到 WebSocket 并返回连接。
	//
	// `headers` 是**额外的**握手头（UA / Origin 之类）；握手必需的那几个
	// （Upgrade / Connection / Sec-WebSocket-*）由实现自己加，
	// 调用方不该也无法覆盖它们。
	Dial(ctx context.Context, url string, headers []HeaderPair) (WebSocketConn, error)
}
