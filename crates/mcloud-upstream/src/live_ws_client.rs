//! 直播弹幕 WebSocket 的**连接层**（真实 IO）。
//!
//! # 这一层只做 IO，**不认帧**
//!
//! 本模块里**不允许**出现任何"这一帧该回什么"的判定。每读到一个文本帧，
//! 原样交给 [`crate::live_ws`] 的纯状态机（[`KoulingListener::on_frame`]），
//! 把它返回的 `outbound` 按序写出去。
//!
//! 这不是风格偏好，是被**可验证性**逼出来的：那条链只有在主播开播那一小时
//! 才存在（2026-09-22 未开播时 `chatRoomId: null`，WS 结构性不存在）。
//! 若把"收到 MSync(command=3) 要补发 UNREAD"这类判定写进读循环，
//! 就只能靠真的开一条 WS 来验证它 —— 而那个窗口一个月也开不了几次。
//! 拆开之后，同一段时序在单测里喂一串字面量就能跑完（见 `live_ws_client::tests`）。
//!
//! 本模块自己的分支**全是传输层**的：文本 / 二进制 / ping / pong / close。
//! 没有一条在判断"业务上这帧是什么意思"。
//!
//! # 断线的三态：为什么必须分开
//!
//! | 状态 | 判据 | 含义 |
//! |---|---|---|
//! | [`WsEnd::ListenedEnough`] | 我们自己的上限到点 | **正常结束**，返回口令 |
//! | [`WsEnd::ServerClosed`] | 服务端发了 `c[...]` 或 WS Close 帧 | 对面主动收工 |
//! | [`WsEnd::PeerGone`] | 流 EOF / 传输错误，且**还没进过房间** | 像"直播结束了" |
//! | [`WsEnd::Disconnected`] | 流 EOF / 传输错误，但**进过房间** | 像"SockJS 超时踢人" |
//!
//! ⚠⚠ **SockJS 超时断开与"直播结束"长得一模一样** —— 两者在 TCP 层都是
//! EOF/Close，没有任何可区分的外部特征。本模块能提供的唯一判据是
//! [`ListenReport::entered_room`]（即状态机**已经收到过** `o` 与
//! MSync(command=3) 的响应）：
//!
//! - **收到过** ⇒ 这条连接**真的**进过直播间，那它断掉更可能是 SockJS 心跳
//!   超时（我们回 `h` 不及时，或服务端侧踢）⇒ [`WsEnd::Disconnected`]；
//! - **没收到过** ⇒ 根本没进成房间 ⇒ [`WsEnd::PeerGone`]。
//!
//! 但**本模块不替调用方下"直播结束了"这个结论** —— 那是编排层
//! （`mcloud-tasks`）的判断，它手上才有房间 `status` 与开播门的信息。
//! 这里只如实报出过程事实。
//!
//! # 与参考实现的对应
//!
//! 对应 `../mcloud/internal/app/live_room_drive.go` 的 `ListenWS`（194–302 行）
//! 的 ⑤ 步（连 WS 并听）。该函数的 ①–④ 步（建会话、取 WS 凭据、房间心跳、
//! 进房间）是 HTTP 侧的事，**不在本模块**。

use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use mcloud_core::{Clock, Error, Result};
use mcloud_crypto::{OsRandom, Random};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::live_consts::{BASE_VIDEO_IM_WS, LIVE_ORIGIN, LIVE_USER_AGENT};
use crate::live_ws::{FrameKind, KoulingListener, Outbound};

/// 本模块实际使用的 WS 流类型。
///
/// 用别名而不是到处写全路径：`MaybeTlsStream<TcpStream>` 是 `connect_async`
/// 真实返回的类型（`wss` 走 **native-tls**、`ws` 走明文 —— 见本 crate 的 `Cargo.toml`）。
/// 写死一处，换连接方式时只改这里。
pub type LiveWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// SockJS 路径里的 sessionId 长度。
const SESSION_ID_LEN: usize = 13;

/// `SockJSSessionID` 用的字符表（36 进制）。
const SESSION_ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// serverId 的取值范围（0–999）。
///
/// 它是 SockJS 用来做**多节点粘滞**的路由段。服务端不认识的值不是错误
/// （SockJS 会回一个 `c[...]` 关闭帧），但也没必要乱给。
const SERVER_ID_BOUND: u32 = 1000;

// ─────────────────────────────────────────────────────────────────────────────
// 听写配置
// ─────────────────────────────────────────────────────────────────────────────

/// 一次听写的输入参数。
///
/// 把参数收成一个结构体而不是给 [`listen_danmaku`] 加一长串位置参数：
/// 调用点（编排层）手上正好只有这几个值，而**位置参数里两个 `&str` 极易传反**
/// （`access_token` 与 `chat_room_id`），传反了不报错、只是鉴权失败或进房失败。
pub struct ListenRequest<'a> {
    /// WS 凭据（`userToken` 接口拿到的 `accessToken`）。
    ///
    /// 进 PROVISION 的 `auth` 字段（`"$t$" + token`），由状态机拼。
    pub access_token: &'a str,
    /// **聊天室** id（31 位），由 `roomBeat` 给出。
    ///
    /// ⚠ 它**不是**房间 id：`guid.name` 用的是手机号，而 `roomId` 只进
    /// PROVISION 的参数里。两者搞混的表现是"进房间失败"，不是"没弹幕"。
    pub chat_room_id: &'a str,
    /// 手机号（`guid.name`）。
    pub phone: &'a str,
    /// 听多久，到点主动断开。
    ///
    /// **不要**给无限时长：没有这个上限，一次"门关着"的调用会一直挂着，
    /// 而它看起来与"正在听"完全一样。
    pub listen_for: Duration,
}

/// 覆盖服务端地址与升级头（**测试用**）。
///
/// 生产路径永远用 [`LiveWsEndpoint::production`]；这个结构存在的唯一理由是
/// 让单测能把客户端指向本地假服务端 —— 而"离线可跑"正是本任务的硬要求。
#[derive(Debug, Clone)]
pub struct LiveWsEndpoint {
    /// WS 端点基址（生产：`wss://videoim.cmcc-cs.cn:20102`）。
    pub base: String,
    /// 升级请求要带的 `User-Agent`。
    pub user_agent: String,
    /// 升级请求要带的 `Origin`。
    pub origin: String,
}

impl Default for LiveWsEndpoint {
    fn default() -> Self {
        Self::production()
    }
}

impl LiveWsEndpoint {
    /// 生产端点：地址与头**逐字**照抄参考实现（[`LiveWSHeaders`]）。
    ///
    /// ⚠ 与 `/cmlive` 那套 HTTP 头**不是同一组**，只有 `Origin` 是共用的。
    /// UA 用 iPad App 内嵌 WebView 的原样值 —— 2026-09-23 实拍
    /// （`docs/capture/app-live-ws-samples.txt` §一）证明真实 H5 客户端
    /// 在 WS 升级请求上发的就是它。
    ///
    /// ⚠ 但**别把实拍当成"服务端校验 UA"的判据**：实拍只能证明客户端
    /// **发了什么**，证明不了服务端**认什么**（坑表 #136 的教训）。
    pub fn production() -> Self {
        Self {
            base: BASE_VIDEO_IM_WS.to_string(),
            user_agent: LIVE_USER_AGENT.to_string(),
            origin: LIVE_ORIGIN.to_string(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 结果
// ─────────────────────────────────────────────────────────────────────────────

/// 这次听写**怎么结束的**。
///
/// 全部都是"正常返回"（不是 `Err`）—— 一次听满时长的运行不该以一句
/// "读 WebSocket 帧失败"结束，那读起来像故障（参考实现同一处也有这个顾虑）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsEnd {
    /// 听满 [`ListenRequest::listen_for`]，由我们主动断开。**正常结束**。
    ListenedEnough,
    /// 服务端明确关闭了这条连接（SockJS 的 `c[...]`，或 WS Close 帧）。
    ServerClosed,
    /// 连接断了，但**这条连接进过房间**（收到过 `o` + PROVISION 响应）。
    ///
    /// ⚠ 典型成因是 **SockJS 心跳超时踢人** —— 而它与"直播结束"从外面看一样，
    /// 判据只能是"进没进过房间"（见模块头）。
    Disconnected,
    /// 连接断了，而且**从来没进过房间**（连 `o` 都没收到就没了）。
    ///
    /// 这一支**长得像"直播结束了"**，但本层不下这个结论：也可能只是
    /// 服务端把我们踢了、或者路径上的 serverId 段给错了。
    PeerGone,
    /// 状态机自己要求收工（例如连续解不开的帧过多 —— 对面不像我们的端点）。
    Aborted,
}

impl WsEnd {
    /// 是否是我们自己按上限收的（正常结束）。
    pub fn is_normal(self) -> bool {
        matches!(self, WsEnd::ListenedEnough)
    }

    /// 是否属于"连接断了"（而非我们主动收工）。
    pub fn is_broken(self) -> bool {
        matches!(self, WsEnd::Disconnected | WsEnd::PeerGone)
    }
}

/// 一次听写的结果。
///
/// 除了口令，还带足够的**过程事实**让上层能如实描述发生了什么 ——
/// 缺了它们，"没收到口令"与"根本没连上"在上层就分不开了。
#[derive(Debug, Clone)]
pub struct ListenReport {
    /// 收到的口令（保序、已去重，由状态机维护）。
    pub codes: Vec<String>,
    /// 结束方式。
    pub end: WsEnd,
    /// 是否收到过 SockJS 的 `o`（打开帧）。
    ///
    /// 收到 `o` = TCP + TLS + WS 升级**全都成功**了，是"连上了"的硬证据。
    pub heard_open: bool,
    /// 是否收到过 MSync(command=3)，也就是 PROVISION 的响应。
    ///
    /// ⚠ **这一条是"真的要开始推弹幕了"的分界线**：只有 PROVISION 被服务端
    /// 确认之后才有 UNREAD，服务端才开始推弹幕。
    pub heard_provision_ack: bool,
    /// 是否收到过 **SockJS 的 `c[...]`**（应用层的"服务器关闭"帧）。
    ///
    /// ⚠ 与 [`Self::peer_sent_ws_close`] **不是一回事**，别合并：
    /// `c[...]` 是 SockJS 协议里服务端**明确**说"这条会话到此为止"；
    /// 而 WS 的 Close 帧只是传输层收工。
    pub heard_close_frame: bool,
    /// 是否收到过 **WS 传输层的 Close 帧**（对端挂断）。
    ///
    /// 它比 `c[...]` 弱得多：没进过房间时的 Close 就是"连上了又被挂断"，
    /// **长得像"直播结束了"**（见模块头三态表）。
    pub peer_sent_ws_close: bool,
    /// 收到了多少条**文本**帧（含心跳）。
    pub frames: usize,
    /// 有多少帧被状态机归为解不开（`FrameKind::Malformed`）。
    ///
    /// 单独计数而不只是打日志：若它一直涨而口令一直空，方向就是
    /// "帧格式对不上"（例如 MSync 字段号错了），而不是"没人发弹幕"。
    pub malformed: usize,
}

impl ListenReport {
    /// 空报告：连不上时用它，保证调用点拿到的是同一个形状。
    pub fn empty(end: WsEnd) -> Self {
        Self {
            codes: Vec::new(),
            end,
            heard_open: false,
            heard_provision_ack: false,
            heard_close_frame: false,
            peer_sent_ws_close: false,
            frames: 0,
            malformed: 0,
        }
    }

    /// 这次听写是否**真的进过**直播间（而不只是连上了 socket）。
    pub fn entered_room(&self) -> bool {
        self.heard_open && self.heard_provision_ack
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 地址与路径拼装
// ─────────────────────────────────────────────────────────────────────────────

/// 生成 SockJS 路径里的 13 字符 sessionId。
///
/// 对齐原项目 `Math.random().toString(36).substring(2, 15)`：36 进制取 13 位。
///
/// ⚠ 原实现取的是 `Math.random()` 的小数部分（**不是**安全随机）—— 这里用注入的
/// [`Random`] 逐位取 36 进制字符，**不照抄**它的熵源：它那一套没有可复现性，
/// 而 fixture 回放需要的恰好是可复现。服务端只把它当路径里的一段，不参与鉴权。
pub fn sockjs_session_id(rng: &mut dyn Random) -> String {
    let mut bytes = [0u8; SESSION_ID_LEN];
    rng.fill_bytes(&mut bytes);
    bytes
        .iter()
        // `% 36` 有极轻微的取模偏差（256 不是 36 的倍数），这里**刻意不修**：
        // 这个值不参与鉴权、不出现在任何凭据里，而"修"它要多一次拒绝采样，
        // 让测试里的固定序列变得难读。
        .map(|b| SESSION_ID_ALPHABET[(*b as usize) % SESSION_ID_ALPHABET.len()] as char)
        .collect()
}

/// 拼 SockJS 的 WebSocket 端点。
///
/// 形状（原项目逐字）：
/// `wss://videoim.cmcc-cs.cn:20102/ws/<serverId 0-999>/<sessionId>/websocket`
pub fn live_ws_url(base: &str, server_id: u32, session_id: &str) -> String {
    format!("{base}/ws/{server_id}/{session_id}/websocket")
}

/// 从随机源取一个 0–999 的 serverId。
fn random_server_id(rng: &mut dyn Random) -> u32 {
    let mut bytes = [0u8; 4];
    rng.fill_bytes(&mut bytes);
    u32::from_be_bytes(bytes) % SERVER_ID_BOUND
}

// ─────────────────────────────────────────────────────────────────────────────
// 连接
// ─────────────────────────────────────────────────────────────────────────────

/// 建立到直播间的 WebSocket 连接。
///
/// 返回的流已经完成 WS 升级（`wss` 走 **native-tls**，与 `reqwest` 同一套 TLS 栈）。
///
/// # 错误映射
///
/// 所有失败都映射为 [`Error::Network`]，**不把 tungstenite 的类型泄漏出去**：
/// 上层只看到 `mcloud_core` 的抽象（本仓铁律，与 `client.rs` 同款做法）。
pub async fn connect_live_ws(url: &str, endpoint: &LiveWsEndpoint) -> Result<LiveWsStream> {
    // 用 `into_client_request` + 手工塞头，而不是 `connect_async(url)` 直接连：
    // `User-Agent` 与 `Origin` 是服务端**看得见**的两项，不能靠默认值。
    let mut request = url
        .into_client_request()
        .map_err(|e| Error::Network(format!("WS 升级请求构造失败 {url}：{e}")))?;

    let headers = request.headers_mut();
    headers.insert(
        "User-Agent",
        endpoint
            .user_agent
            .parse()
            .map_err(|e| Error::Network(format!("User-Agent 非法：{e}")))?,
    );
    headers.insert(
        "Origin",
        endpoint
            .origin
            .parse()
            .map_err(|e| Error::Network(format!("Origin 非法：{e}")))?,
    );

    let (stream, _response) = connect_async(request)
        .await
        .map_err(|e| Error::Network(format!("连接直播间 WebSocket 失败：{e}")))?;
    Ok(stream)
}

/// 连上直播间并听一段时间，返回听到的口令。
///
/// # 这是 `ListenWS` 的 ⑤ 步
///
/// 参考实现（`live_room_drive.go` 194–302）的 ①–④ 步（建会话 / 取凭据 /
/// 房间心跳 / 进房间）是 HTTP 侧的事，**本函数不做**：它拿到的是那四步的产物
/// （`access_token` + `chat_room_id`），只负责连、听、收尾。
///
/// # 时序（全部由状态机决定，本层不认帧）
///
/// ```text
/// 服务端 → `o`                 → 客户端发 PROVISION(command=3)
/// 服务端 → MSync(command=3)    → 客户端发 UNREAD(command=1)   ← 漏了就永远没弹幕
/// 服务端 → `h`                 → 客户端原样回 `h`             ← 不回会被 SockJS 踢
/// 服务端 → `a["<base64>"]`     → 解出弹幕 → 抽口令
/// ```
///
/// # 到点主动断开
///
/// [`ListenRequest::listen_for`] 是**整体上限**：到点发一个 Close 帧并返回
/// 已收集的口令。没有它就等于无限挂着，而"门关着"与"正在听"从外面看一样。
pub async fn listen_danmaku(
    request: ListenRequest<'_>,
    endpoint: &LiveWsEndpoint,
    rng: &mut dyn Random,
    clock: &dyn Clock,
) -> Result<ListenReport> {
    let session_id = sockjs_session_id(rng);
    let url = live_ws_url(&endpoint.base, random_server_id(rng), &session_id);

    // ⚠ 状态机要 `Box<dyn Random + Send + Sync>`（它内部可能在被 `&mut` 借用时
    // 也要吐随机字节）。把调用方那一份借用挪进去是不可能的，所以这里**另起**
    // 一份随机源：它只用于 PROVISION 帧里的 `nonce` 一类字段，
    // 与 sessionId / serverId 的可复现性无关。
    let mut listener = KoulingListener::new(
        request.access_token,
        request.chat_room_id,
        request.phone,
        Box::new(OsRandom),
    );

    let stream = connect_live_ws(&url, endpoint).await?;
    listen_on_stream(stream, &mut listener, request.listen_for, clock).await
}

/// 在**已建立的**连接上听。
///
/// # 为什么对底层流是泛型的
///
/// 从 [`listen_danmaku`] 切出来，是为了让测试能把任意流喂进来。
/// 而"任意"必须包括 [`tokio::io::duplex`] 的内存双工 —— 它不是
/// `MaybeTlsStream<TcpStream>`。所以这里泛型到 `S: AsyncRead + AsyncWrite`，
/// 而不是写死 [`LiveWsStream`]：写死的话，**只有走真实 TCP 的测试能跑**，
/// 而"离线可跑"是本任务的硬要求。
///
/// 这不削弱生产路径：`listen_danmaku` 传进来的正是 [`LiveWsStream`]。
///
/// 听写上限与读竞争（见下），而"读了要回"这件事由 [`flush_outbound`] 在
/// `select!` 之外完成。
///
/// # 为什么是单一 `select!` 而不是"读 + 外层 timeout"
///
/// 听写上限必须与读**竞争**：写成 `timeout(listen_for, read_loop)` 的话，
/// 只有"读正在 pending"时超时才会被看见 —— 而 SockJS 长时间不推弹幕
/// 恰恰是最常见的状态。`select!` 让"到点"与"来帧"同时被观察。
pub async fn listen_on_stream<S>(
    mut stream: WebSocketStream<S>,
    listener: &mut KoulingListener,
    listen_for: Duration,
    clock: &dyn Clock,
) -> Result<ListenReport>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let deadline = Instant::now() + listen_for;
    let mut report = ListenReport::empty(WsEnd::PeerGone);

    loop {
        // ⚠ 读与写必须**分开借**：`select!` 的读分支借了 `stream`，
        // 而回帧又要写同一个 `stream`。所以读分支只产出「状态机要回什么」，
        // 真正写出去放到 `select!` 之后 —— 那时读的借用已经结束。
        let step = tokio::select! {
            // 到点：主动断开。不启用 `biased` —— 到点与来帧同时就绪时，
            // 多读一帧不改变结论，而少读一帧可能丢掉一条口令。
            _ = tokio::time::sleep_until(deadline) => ListenStep::TimedOut,
            incoming = stream.next() => ListenStep::Read(incoming),
        };

        match step {
            ListenStep::TimedOut => {
                report.end = WsEnd::ListenedEnough;
                // Close 帧尽量发；发不出去（对端已经没了）不算失败 ——
                // 我们**本来就**准备结束这一趟。
                let _ = stream.close(None).await;
                break;
            }
            ListenStep::Read(None) => {
                // 流结束：`tokio-tungstenite` 在连接已关闭/已结束时回 `None`
                // （不是错误）。⚠ 这正是"被断开"与"直播结束"共有的形状。
                break;
            }
            ListenStep::Read(Some(Err(e))) => {
                // 读完但传输层出错：这**不是**"读不到帧"，
                // 但也未必是故障 —— 对端 RST 也走这里。
                // 归类交给下面的 `classify_end`，不在这里下结论。
                tracing::debug!(error = %e, "直播间 WS 读取出错");
                break;
            }
            ListenStep::Read(Some(Ok(message))) => {
                let outcome = handle_message(message, listener, clock, &mut report)?;
                // 真正把回帧写出去。⚠ **顺序有意义**（PROVISION 先于 UNREAD），
                // `outbound` 本来就是有序的，照序发即可。
                flush_outbound(&mut stream, &outcome.outbound).await?;
                if !outcome.keep_reading {
                    break;
                }
            }
        }
    }

    report.codes = listener.codes().to_vec();
    report.end = classify_end(report.end, listener, &report);
    Ok(report)
}

/// `select!` 的一个分支：要么到点，要么读到一条东西。
///
/// 抽成枚举是为了让 `select!` 里**不出现对 `stream` 的写操作**：
/// 一旦读写挤在同一个表达式里，借用检查就会拒（而"改成 clone 一个流"
/// 是错的 —— WS 流不是能随便分身的）。
enum ListenStep {
    /// 听写上限到点。
    TimedOut,
    /// 读到一条（`None` = 流结束）。
    Read(Option<std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>),
}

/// [`handle_message`] 的产出：要回什么 + 还要不要继续读。
struct StepOutcome {
    /// 要按序写出去的帧。
    outbound: Vec<Outbound>,
    /// `false` = 循环该停了。
    keep_reading: bool,
}

/// 按**过程事实**给结束方式定档。
///
/// # 这是全模块唯一一处"解读"，而它只读状态机的事实，不做业务判断
///
/// 判据（依优先级）：
///
/// 1. 我们已经按上限收工 ⇒ [`WsEnd::ListenedEnough`]（正常结束，**压倒一切**）；
/// 2. **没进过房间**（没同时收到 `o` 与 PROVISION 响应）⇒ [`WsEnd::PeerGone`]
///    —— **长得像"直播结束了"**；
/// 3. 进过房间、且收到 SockJS 的 `c[...]` ⇒ [`WsEnd::ServerClosed`]
///    （对面**明确**说这条会话到此为止）；
/// 4. 进过房间、其余断开方式（WS Close / EOF / 读错误）⇒
///    [`WsEnd::Disconnected`] —— 更像心跳超时踢人。
///
/// # ⚠ 第 2 条必须排在"谁先挂断"**之前**
///
/// 第一版实现把"收到关闭帧 ⇒ `ServerClosed`"放在了最前，结果是：
/// **从没进过房间**时的一次 WS Close 也被归成 `ServerClosed` —— 而那一支的
/// 真实含义是"连上了、又被挂断"，属于**长得像"直播结束了"**的那一类。
/// 也就是说，一个纯粹的顺序错误会让"直播结束了"这个结论**永远不会出现**，
/// 而它恰恰是这条链最需要如实报出的那一支（见
/// `live_ws_client::tests::peer_gone_without_ever_entering_room`）。
///
/// ⚠ "进没进过房间"是第 2–4 条的**唯一**判据。这几种在 TCP 层完全同形，
/// 任何"从错误信息里猜"的做法都是编的。
fn classify_end(current: WsEnd, listener: &KoulingListener, report: &ListenReport) -> WsEnd {
    // ① 到点收工是我们自己的决定，不该被别的迹象改写 ——
    // 否则"听满 45 秒"会时不时被报成"被断开"。
    if current == WsEnd::ListenedEnough {
        return WsEnd::ListenedEnough;
    }
    // ② 进没进过房间是**更根本**的判据，所以排在"谁先挂断"之前。
    //
    // ⚠ 顺序不能反。若先看 `heard_close_frame`，那么"服务端 `c[...]`
    // 收工"固然会被正确归成 `ServerClosed`，但**从没进过房间**时的
    // 一次 WS Close 也会被归成 `ServerClosed` —— 而那一支的真实含义是
    // "连上了、又被挂断"，属于**长得像"直播结束了"**的那一类，
    // 必须落到 `PeerGone`。所以先分"进没进过房间"，再分"谁挂的"。
    let entered = listener.provision_ack_seen() && report.heard_open;
    if !entered {
        return WsEnd::PeerGone;
    }
    // ③ 进过房间之后：SockJS 的 `c[...]` 是服务端**明确**收工。
    if report.heard_close_frame {
        return WsEnd::ServerClosed;
    }
    // ④ 其余（含 WS Close / EOF / 读错误）⇒ 像心跳超时踢人。
    WsEnd::Disconnected
}

/// 处理一条入站消息，返回「要回什么 + 还要不要继续读」。
///
/// # 本函数是"只做 IO、不认帧"这条约束的落点
///
/// 这里出现的分支**全是传输层**的事（文本 vs 二进制 vs ping/pong vs close），
/// 没有一条在判断"业务上这帧是什么意思"。业务判定一律转交
/// [`KoulingListener::on_frame`]，它回什么就发什么。
fn handle_message(
    message: Message,
    listener: &mut KoulingListener,
    clock: &dyn Clock,
    report: &mut ListenReport,
) -> Result<StepOutcome> {
    // 默认什么都不回、继续读 —— 绝大多数分支都是这个形状。
    let quiet = StepOutcome {
        outbound: Vec::new(),
        keep_reading: true,
    };
    let stop = StepOutcome {
        outbound: Vec::new(),
        keep_reading: false,
    };

    match message {
        Message::Text(text) => {
            report.frames += 1;

            // ⚠ 这一行就是本层的全部"智能"：把文本原样交给纯状态机。
            // 时钟只用来给状态机打时间戳（它要"同一刻"派生三处字段）。
            let actions = listener.on_frame(text.as_str(), clock.now_ms())?;

            // ── 「服务端收工」的**活信号**是 `FrameKind::Closed` / `is_closed()` ──
            //
            // ⚠⚠ 别写成 `actions.close == Some(CloseReason::ServerClosed)`。
            // 那个变体在 `live_ws.rs` 里**只被定义、从不被产出**：`c[...]`
            // 那一支返回的是 `close: None` + `kind: FrameKind::Closed`
            // （状态机的设计意图：连接已经是关的，没有"再去关一次"这个动作，
            // 所以不该产出 `close`）。写那一支会得到一个**永远不命中**的分支 ——
            // 而更坏的是它"看起来"在干活：循环靠下面 `is_closed()` 兜住了，
            // 结果对、机制错，下一个读代码的人会以为那支在生效。
            match actions.kind {
                FrameKind::Open => report.heard_open = true,
                FrameKind::ProvisionAck => report.heard_provision_ack = true,
                // SockJS 的 `c[...]`：服务端**明确**说这条会话到此为止。
                FrameKind::Closed => report.heard_close_frame = true,
                FrameKind::Malformed => report.malformed += 1,
                _ => {}
            }

            // 状态机要求我们自己收工吗（例如连续解不开的帧已达上限 ——
            // 对面不像我们以为的那个端点）。
            //
            // 这里**只**处理"我们主动收工"那一类。`CloseReason::ServerClosed`
            // 刻意不列：它当前不可达（见上），列出来就是个死分支。
            // 真引入了它，`c[...]` 那一侧也已经由 `kind == Closed` 覆盖。
            let must_stop = match &actions.close {
                Some(reason) => {
                    tracing::debug!(reason = %reason, "状态机要求收工");
                    true
                }
                None => false,
            };

            // 服务端已经发过 `c[...]`（`is_closed()` 是它的**跨帧持久**形态）：
            // 状态机之后再喂帧都只会回 `Ignored`，早点停，
            // 别把剩下的连接时间浪费掉。
            let keep_reading = !must_stop && !listener.is_closed();

            // ⚠ 回帧要**原样带出去**，由调用方在借用结束之后真正写进连接。
            // 顺序有意义（`outbound` 已有序），这里不做任何重排。
            Ok(StepOutcome {
                outbound: actions.outbound,
                keep_reading,
            })
        }
        Message::Binary(_) => {
            // SockJS 的文本形态协议不会推二进制（心跳与数据都是文本）。
            // 真收到就丢掉，但**不能**当成错误：那是服务端的事，不是我们的故障。
            Ok(quiet)
        }
        Message::Ping(_) => {
            // WS 层心跳，与 SockJS 的 `h` **不是一回事**，但语义一样：要回。
            // ⚠ 由 `tokio-tungstenite` 自动回 pong，这里**不重复回** ——
            // 重复回会让线上出现两倍 pong。
            Ok(quiet)
        }
        Message::Pong(_) => Ok(quiet),
        Message::Close(frame) => {
            // 传输层 Close。⚠ 这是"被断开 / 直播结束 / SockJS 超时"三种里
            // 后两种所共有的**唯一**外部表现 —— 本层到此为止，如实上报
            // （归到哪一档由 `classify_end` 按"进没进过房间"决定）。
            if let Some(frame) = frame {
                tracing::debug!(
                    code = %frame.code,
                    reason = %frame.reason,
                    "直播间 WS 被对端关闭"
                );
            }
            report.peer_sent_ws_close = true;
            Ok(stop)
        }
        Message::Frame(_) => Ok(quiet),
    }
}

/// 把状态机要求的帧真正写进连接。
///
/// 从 `handle_message` 里切出来：那一层只该管"读到了什么"，
/// 而"怎么写"要拿 `&mut stream`，两者放一起会在 `select!` 的
/// 借用检查里打架（读分支借了 stream，写又要借）。
async fn flush_outbound<S>(stream: &mut WebSocketStream<S>, actions: &[Outbound]) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    for outbound in actions {
        match outbound {
            Outbound::Text(t) => stream
                .send(Message::text(t.clone()))
                .await
                .map_err(|e| Error::Network(format!("写 WebSocket 帧失败：{e}")))?,
            Outbound::Binary(b) => stream
                .send(Message::binary(b.clone()))
                .await
                .map_err(|e| Error::Network(format!("写 WebSocket 帧失败：{e}")))?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
