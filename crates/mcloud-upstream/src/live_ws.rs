//! 直播弹幕 WebSocket 的**帧状态机**（纯层，不持有连接）。
//!
//! 对应 Go 参考的 `internal/upstream/live_ws.go`（389 行），也就是原项目
//! `packages/core/src/tasks/live-room-kouling.ts` 里 `getKoulingFromWebSocket`
//! 那一大段 `ws.onMessage` 的分支。
//!
//! # 为什么状态机在纯层、连接在 io 层
//!
//! 这条链最难的部分**不是连上**，而是**认帧**：SockJS 的四种文本帧、
//! MSync 的 command 分派、三级解码兜底、跨帧口令拼接 —— 它们**全是纯函数**。
//!
//! 把它们和连接绑在一起的话，「收到 `a["…"]` 之后该回什么」就只能靠
//! **真的开一条 WS** 来验证 —— 而 WS 只有在**主播开播那一小时**才存在
//! （2026-09-22 实拍：`chatRoomId: null` ⇒ WS 结构性不存在）。
//!
//! 拆开之后，帧序列可以用一串字面量在单测里跑完
//! （见 `live_ws/tests.rs`）；真实连接在 [`crate::live_ws_client`]，它
//! **只做 IO、不认帧**。
//!
//! # ⚠⚠ 最关键的一条时序：PROVISION 之后必须补发 UNREAD
//!
//! ```text
//! 服务端 → `o`                      （SockJS 打开）
//! 客户端 → PROVISION(command=3)      （带 Provision payload）
//! 服务端 → MSync(command=3)          （PROVISION 的响应）
//! 客户端 → **UNREAD(command=1)**     ← 少了这一条，服务端**不推弹幕**
//! ```
//!
//! 这是原实现里「关键：触发服务器推送消息」那句注释的来源，也是整条链
//! 最容易漏的一步 —— 漏了之后 **WS 连得上、心跳也正常、只是永远收不到弹幕**。
//! 排查方向会指向「是不是没人在发」，而不是「少发了一帧」。
//!
//! 所以测试里那条时序（`provision_then_unread_ordering_is_pinned`）断言的
//! 不只是「某个帧发出去了」，而是**顺序**与**只发一次**。
//!
//! # ⚠ 未实拍的部分（原文照录，别当成已实测）
//!
//! 2026-09-22 未开播、`chatRoomId: null` ⇒ **WS 结构性不存在**，
//! 所以下面三条**都没有实拍**，全部来自**原项目实现**：
//!
//! 1. **帧的顺序**（`o` → PROVISION → `command=3` 响应 → UNREAD）；
//! 2. **`command` 的取值**（`1`/`3` 这些数字本身）；
//! 3. **UNREAD 是否真的必需**（原实现注释说必需，但没有实测证据）。
//!
//! `../mcloud/docs/capture/probe-live-room-2026-09-23.md` 的运行记录里，这三条
//! 明明白白列在「🔌 要抓包」那一档。**别把本文件的行为描述成「已实测」。**
//!
//! 这正是把「每一帧要回什么」做成**可断言的返回值**的原因：开播后第一次
//! 抓到真帧序列时，对不上的地方会以**一条失败的测试**出现，
//! 而不是「弹幕忽然收不到了」。
//!
//! # ⚠ 实拍支撑的部分
//!
//! - **四种 SockJS 文本帧的存在性**（`o` / `h` / `a[...]` / `c[...]`）：
//!   实拍夹具 `../mcloud/test/fixtures/http/直播wss.json` 里有 13 片
//!   `imChatRoomHistoryMessage`（814 条弹幕）；
//! - **弹幕文本的形状**：`test/golden/live-room-vectors.json` 的
//!   `msyncInbound` 三条给出了真文本（`小红花口令：…`）。

use std::collections::HashSet;

use mcloud_core::{Error, Result};
use mcloud_crypto::Random;

use crate::live_msync::{
    decode_msync, encode_msync, extract_danmaku_frame, is_event_message, live_msync_frame,
    live_provision_payload, MsyncMessage, CMD_PROVISION, CMD_UNREAD,
};

// ══════════════════════════ SockJS 帧前缀 ══════════════════════════

/// SockJS 的「连接已打开」帧 —— 收到它才发 PROVISION。
pub const SOCKJS_OPEN: &str = "o";

/// SockJS 的「心跳」帧 —— **要原样回一个 `h`**。
///
/// 不回的话 SockJS 会在超时后主动断开，而**断开的样子与「直播结束了」一样**。
///
/// ⚠ 未实拍（见模块头）：原项目的被动心跳就是这样处理的。
pub const SOCKJS_HEARTBEAT: &str = "h";

/// SockJS 的「关闭」帧前缀（`c[3000,"Go away!"]`）。
///
/// ⚠ 它**不在** task 描述点名的四种帧里（那是 `o` / `h` / `a[...]` / `c[...]`
/// —— 其实 `c[...]` 就是它）。这里把它单独认出来是为了让「服务端主动关了」
/// 这个**可区分**的结论不要落进「不认识的帧」那一档：
/// 关闭与心跳超时看起来完全不同，但两者都会让「收不到弹幕」发生。
pub const SOCKJS_CLOSE_PREFIX: &str = "c[";

/// SockJS 的数据帧前缀（`a["<base64>"]`，一条或多条消息的 JSON 数组）。
pub const SOCKJS_DATA_PREFIX: &str = "a[";

// ══════════════════════════ 帧的归类 ══════════════════════════

/// 一帧被归成了哪一类（只用于日志与测试断言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameKind {
    /// 不认识的帧（原实现是 debug 打一行然后丢掉）。
    #[default]
    Ignored,
    /// `h` —— 心跳。
    Heartbeat,
    /// `o` —— 连接已打开。
    Open,
    /// `c[...]` —— 服务端关闭。
    Closed,
    /// `command == 3` 的响应帧（**触发 UNREAD**）。
    ProvisionAck,
    /// 解出了文本的 SYNC / NOTICE 帧。
    Danmaku,
    /// 弹幕流里的**事件消息**（入场 / 点赞 / 系统 / 礼物）—— 有文本，但**不是口令来源**。
    ///
    /// ⚠ 它单列一档而不是并进 [`Self::Silent`]：这两种在日志里必须分得开。
    /// "这一帧什么都没解出来"（协议层没读懂）与"这一帧读懂了、但它是条礼物提示"
    /// 的排查方向完全不同 —— 前者要去看帧格式，后者要看上游的 `msgType` 表。
    EventMessage,
    /// 数据帧但解不出文本（心跳类的 SYNC、别的 command）。
    Silent,
    /// 数据帧但 base64 / protobuf 解不开。
    Malformed,
}

impl FrameKind {
    /// 稳定的短名（进日志与断言，别用 `Debug` 的输出当接口）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ignored => "ignored",
            Self::Heartbeat => "heartbeat",
            Self::Open => "open",
            Self::Closed => "closed",
            Self::ProvisionAck => "provision_ack",
            Self::Danmaku => "danmaku",
            Self::EventMessage => "event_message",
            Self::Silent => "silent",
            Self::Malformed => "malformed",
        }
    }
}

impl std::fmt::Display for FrameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 「处理完一帧之后要做什么」。
///
/// # 为什么是返回值而不是回调
///
/// 做成返回值，整条时序就**可断言**了：测试里喂一串帧文本，
/// 比一串「发出去的帧」和「收到的口令」。
///
/// 回调式的写法（`ws.on_message(|frame| { … ws.send(…) … })`）会把
/// 「这一帧该回什么」与「怎么发出去」绑死，于是**只有真开 WS 才能验证**
/// —— 而那条链只在主播开播那一小时才存在。
///
/// # 连接层要的四件事，这里都能表达
///
/// [`crate::live_ws_client`] 只做 IO、**不认帧**，它拿到的就是本结构。
/// 它需要能表达的四类动作与对应字段：
///
/// | 连接层要做的事 | 本结构怎么表达 |
/// |---|---|
/// | **什么都不做** | `outbound` 空 且 `close.is_none()` |
/// | **回一个文本帧** | `outbound` 里是 `String`（SockJS 全是文本帧） |
/// | **回一个字节帧** | [`Outbound::Binary`] 变体（当前**没有任何一帧**走它，留着是为了让连接层不必假设"永远只有文本"） |
/// | **关闭连接（带原因）** | [`Self::close`] |
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameActions {
    /// 要按顺序写出去的帧。
    ///
    /// ⚠ **顺序有意义**：PROVISION 必须先于 UNREAD。
    pub outbound: Vec<Outbound>,
    /// 这一帧解出的弹幕文本（空串 = 这一帧没有文本）。
    pub danmaku: String,
    /// 这一帧**新**提取到的口令（已按全局去重、保序）。
    pub codes: Vec<String>,
    /// 这一帧被归成了哪一类。
    pub kind: FrameKind,
    /// 连接层**应当**主动关闭时的原因（`None` = 继续跑）。
    ///
    /// # ⚠ 与「服务端关了」不是一回事（这两条要分得开）
    ///
    /// - **服务端发来 `c[...]`** ⇒ `kind == Closed`，但 `close` 是 `None`
    ///   —— 连接已经是关的，没有「再去关一次」这件事。上层据此报
    ///   「服务端关了这条连接」。
    /// - **本状态机要求关闭** ⇒ `close` 是 `Some(…)`，且 `kind` 通常是
    ///   `Malformed`（例如连续解不开的帧已经多到不值得再等）。
    ///
    /// 把两者折成一个字段的话，「被断开」与「我们自己放弃」就再也分不开
    /// —— 而这两件事的处置完全不同（一个该重连、一个该报错）。
    pub close: Option<CloseReason>,
}

/// 要写出去的帧。
///
/// # 为什么现在就分文本与字节，尽管 SockJS 只发文本
///
/// 连接层（`live_ws_client`）手里是 `tokio_tungstenite` 的
/// `Message::{Text, Binary}` —— 它需要一个「发文本还是发字节」的判据。
/// 只给 `Vec<String>` 会**逼着连接层替我们做那个决定**（它只能猜「都是文本」），
/// 而那个假设一旦不成立，错的地方就在**连接层**了。
///
/// 现在没有任何一帧走 [`Outbound::Binary`]（SockJS 的帧协议就是文本），
/// 但让类型把这件事说清楚，比让下一个读连接层的人以为「永远只有文本」要好。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    /// 文本帧（SockJS 的 `h` 与 `["<base64>"]`）。
    Text(String),
    /// 二进制帧（**当前没有帧走它**，见上）。
    Binary(Vec<u8>),
}

impl Outbound {
    /// 按文本取内容（二进制帧返回 `None`）—— 测试与日志用。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Binary(_) => None,
        }
    }
}

impl From<String> for Outbound {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<&str> for Outbound {
    fn from(s: &str) -> Self {
        Self::Text(s.to_string())
    }
}

/// 状态机要求连接层关闭的原因。
///
/// ⚠ 这三个都是**我们这一侧**的判断，与「服务端关了」「网络断了」不是一回事
/// —— 后两者由连接层自己观察到（见 [`FrameActions::close`] 的说明）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// 解不开的帧**连续**太多（连接看起来是通的，但说的不是我们的协议）。
    ///
    /// ⚠ 与「偶尔一帧解不开」**不同**：单个坏帧是正常的（协议会演进），
    /// 连续坏帧说明**对面不是我们以为的那个端点**。
    TooManyMalformed,
    /// 服务端发来了关闭帧 `c[...]`（连接层据此可以提前收工，不必等超时）。
    ServerClosed,
    /// 对话已经拿到需要的口令，可以收了（由上层决定，本层不主动置）。
    Enough,
    /// 其它原因（自由描述，进日志）。
    Other(String),
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyMalformed => f.write_str("连续解不开的帧过多"),
            Self::ServerClosed => f.write_str("服务端关闭了连接"),
            Self::Enough => f.write_str("已收够口令"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl FrameActions {
    /// 只有归类、什么都不回。
    fn silent(kind: FrameKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }

    /// 有要发出去的东西吗。
    pub fn has_outbound(&self) -> bool {
        !self.outbound.is_empty()
    }

    /// 要发出去的文本帧（**把二进制帧筛掉**）。
    ///
    /// 便利方法，给测试与「只看文本」的调用方用。
    pub fn outbound_texts(&self) -> Vec<&str> {
        self.outbound.iter().filter_map(Outbound::as_text).collect()
    }
}

// ══════════════════════════ 跨帧拼接的口令头判据 ══════════════════════════

// ⚠ 「整条就是口令头」的判据**不在这里**，用 `live_kouling::is_kouling_head`
// （严格版：`(?:复制)?口令\d*[：:]` 结尾）。
//
// 2026-10-09 改掉的是**原来这里那个宽松版**（`text.contains("口令")`）：
// 实拍那条完整口令 `小红花口令：一键9图出圈` **含**「口令」二字，于是它会给
// **下一条**弹幕挂上"整条即口令"的状态 —— 而实拍那 15 条正是口令与
// 入场/礼物提示**交替**推的，下一条恰好是 `加入直播间` / `送了1个火箭` 时，
// 整条就被当成口令（用户日志里的「口令：加入直播间」就是这么来的）。
//
// 那件事的另一半（"弹幕文本 → 口令"只认前缀）在 `live_kouling`；
// 两处判据都收紧了，事件消息从**两条路**上都进不来。

/// 这段文本**本身还是个口令头/收尾词**，不能当口令用。
///
/// 对齐原实现的 `!/(?:复制)?口令|无随机/.test(trimmed)`
/// —— 注意它比 [`crate::live_kouling::is_kouling_head`] 多一个「无随机」。
fn is_kouling_head_fragment(text: &str) -> bool {
    text.contains("口令") || text.contains("无随机")
}

// ══════════════════════════ 状态机 ══════════════════════════

/// 「进房间 → 收口令」这条链的状态机。
///
/// 它**不持有连接**，也**不 sleep**：每收到一帧就调一次 [`Self::on_frame`]，
/// 由调用方决定什么时候停（原项目是超时，或「收到一个就返回」）。
///
/// # 它持有的状态只有四项
///
/// | 字段 | 作用 |
/// |---|---|
/// | `codes` / `seen` | 全局去重与保序 |
/// | `pending_head` | 跨帧拼接：上一条是口令头 ⇒ 下一条整条是口令 |
/// | `provision_sent` | PROVISION **只发一次** |
/// | `unread_sent` | UNREAD **只发一次** |
///
/// 「只发一次」不是优化：重复发 PROVISION 会让服务端把同一条连接
/// **当成新会话**（原实现如此）。
pub struct KoulingListener {
    access_token: String,
    /// 聊天室 id（`roomBeat` 给的 31 位那个）—— **不是**房间 id。
    chat_room_id: String,
    /// 手机号。`guid.name` 用的是它。
    phone: String,
    rng: Box<dyn Random + Send + Sync>,

    codes: Vec<String>,
    seen: HashSet<String>,

    /// 「上一帧整条是个口令头，下一帧整条就是口令」。
    pending_head: bool,
    /// PROVISION 是否已发出 —— 它**只能发一次**。
    provision_sent: bool,
    /// UNREAD 是否已发出 —— 它**只能发一次**。
    unread_sent: bool,
    /// 是否收到过服务端的 `command == 3` 响应。
    ///
    /// # 连接层为什么需要它
    ///
    /// 它回答的是「**这条连接真的建立起来了吗**」—— 与「TCP 连上了」
    /// **不是一回事**：TCP 连上只说明到了 SockJS 那一层，
    /// 而 PROVISION 的响应才是「对面认了这次进房」。
    ///
    /// [`crate::live_ws_client`] 用它来区分三态里的「**直播结束**」
    /// （从未收到过响应 ⇒ 这条连接建不起来 ⇒ 多半是房间关了），
    /// 与「被断开」（收到过响应又断了 ⇒ 是网络/服务端的事）。
    provision_ack_seen: bool,
    /// 服务端是否已经发过 `c[...]`（关闭）。发了之后不该再回任何东西。
    closed: bool,
    /// **连续**解不开的帧数（任何一帧正常解析都会清零）。
    consecutive_malformed: u32,
    /// 连续多少帧解不开就请连接层关闭。
    ///
    /// # 为什么要有这个上限
    ///
    /// 单个坏帧是**正常**的（协议会演进），所以不能一见坏帧就断。
    /// 但**连续**坏帧说明**对面不是我们以为的那个端点** ——
    /// 而那时继续挂着只会一直烧时间，且表现是「连上了、就是没弹幕」，
    /// 与真正该查的方向（授权、房间号）无关。
    ///
    /// ⚠ 这个阈值是**我们定的**，没有任何实拍依据 —— 它只是个
    /// 「别无限挂着」的兜底。别把它当成协议常量。
    malformed_limit: u32,
}

impl std::fmt::Debug for KoulingListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 手写 `Debug` 而不是 derive：`access_token` 是凭据，
        // 掉进任何一行日志都是泄漏（与 auth/session 那一层的口径一致）。
        f.debug_struct("KoulingListener")
            .field("chat_room_id", &self.chat_room_id)
            .field("phone", &"<REDACTED>")
            .field("access_token", &"<REDACTED>")
            .field("codes", &self.codes)
            .field("pending_head", &self.pending_head)
            .field("provision_sent", &self.provision_sent)
            .field("unread_sent", &self.unread_sent)
            .field("provision_ack_seen", &self.provision_ack_seen)
            .field("closed", &self.closed)
            .finish()
    }
}

impl KoulingListener {
    /// 构造状态机。
    ///
    /// # `chat_room_id` 是**聊天室 id**，不是房间 id
    ///
    /// 两者都在手边（`roomBeat` 同时给出房间 id 与 `chatRoomId`），
    /// 而搞混的表现是**进房间失败**（`member/apply` 回非 0 码）——
    /// 那是明确报错的，比"连上了但没弹幕"好查。
    ///
    /// ⚠ `guid.name` 用的是 `phone`，而 `chat_room_id` 只进 PROVISION 的参数
    /// —— 两个别搞混（见 [`crate::live_msync::live_msync_frame`] 的 `phone` 分支）。
    pub fn new(
        access_token: impl Into<String>,
        chat_room_id: impl Into<String>,
        phone: impl Into<String>,
        rng: Box<dyn Random + Send + Sync>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            chat_room_id: chat_room_id.into(),
            phone: phone.into(),
            rng,
            codes: Vec::new(),
            seen: HashSet::new(),
            pending_head: false,
            provision_sent: false,
            unread_sent: false,
            provision_ack_seen: false,
            closed: false,
            consecutive_malformed: 0,
            // 见字段说明：这是我们定的兜底，不是协议常量。
            malformed_limit: 8,
        }
    }

    /// 改「连续解不开多少帧就请连接层关闭」的阈值。
    ///
    /// `0` = **不因为坏帧关闭**（一直挂着，直到调用方自己的超时）。
    /// 测试与「宁可多等一会儿」的调用方会用它。
    pub fn with_malformed_limit(mut self, limit: u32) -> Self {
        self.malformed_limit = limit;
        self
    }

    /// 已收集到的口令（保序）。
    pub fn codes(&self) -> &[String] {
        &self.codes
    }

    /// PROVISION 是否已发出。
    pub fn provision_sent(&self) -> bool {
        self.provision_sent
    }

    /// UNREAD 是否已发出。
    pub fn unread_sent(&self) -> bool {
        self.unread_sent
    }

    /// 是否收到过服务端的 PROVISION 响应（`command == 3`）。
    ///
    /// # 它回答的是「这条连接真的建立起来了吗」
    ///
    /// 与「TCP 连上了」**不是一回事**：TCP 连上只说明到了 SockJS 那一层，
    /// 而 PROVISION 的响应才是「对面认了这次进房」。
    ///
    /// [`crate::live_ws_client`] 用它区分「**直播结束**」（从未收到过响应
    /// ⇒ 这条连接建不起来）与「**被断开**」（收到过又断了）。
    pub fn provision_ack_seen(&self) -> bool {
        self.provision_ack_seen
    }

    /// 服务端是否发过关闭帧。
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// 处理一帧 SockJS 文本帧，返回要回发的帧与新口令。
    ///
    /// # `now_millis` 由调用方给（本层不取时钟）
    ///
    /// 它同时决定三处，而且**必须来自同一刻**：
    ///
    /// - `guid.clientResource` = `js_<毫秒>`；
    /// - PROVISION 的 `deviceUuid`（毫秒的**十进制字符串**，与上面同源但**不是同一个串**）；
    /// - MSync 的 8 字节 `timestamp`。
    ///
    /// 本层取时钟的话，单测就没法复现（黄金向量的期望值是固定的毫秒）。
    ///
    /// # 出错的语义
    ///
    /// - **解不开一帧不算整条链的失败**：还有下一帧（`a[...]` 里可以有多条）。
    ///   所以解不开时返回的是「归类为 `Malformed` 的 `FrameActions`」，
    ///   而 `Err` 只留给**发不出去**那种真错误。
    /// - 但**要把它记下来**（`kind = Malformed`）：只回一个空文本的话，
    ///   「一直收不到弹幕」与「弹幕本来就没有」就分不开了。
    pub fn on_frame(&mut self, text: &str, now_millis: i64) -> Result<FrameActions> {
        // 服务端已经关了：什么都不回（往一条已关的连接写只会得到错误）。
        if self.closed {
            return Ok(FrameActions::silent(FrameKind::Ignored));
        }

        // ⚠ 除坏帧外的**每一条**返回路径都要经过这里 —— 见 [`Self::note_malformed`]。
        // 早先的写法是「只在数据帧那一段记账」，于是心跳、`o`、`c[...]`
        // 那些 early return 都**绕过了清零** ⇒ 连续坏帧计数只增不减，
        // 一条正常的心跳流也会在攒够 8 个坏帧后被判成「对面不是我们的端点」。
        // 这个 bug 是 `a_good_frame_resets_the_malformed_counter` 抓出来的。
        let actions = self.on_frame_inner(text, now_millis)?;
        if actions.kind != FrameKind::Malformed {
            // 能归类的帧都说明「这条连接还在说我们的协议」⇒ 清零。
            self.consecutive_malformed = 0;
        }
        Ok(actions)
    }

    fn on_frame_inner(&mut self, text: &str, now_millis: i64) -> Result<FrameActions> {
        // ── `h`：**原样回一个 `h`** ──────────────────────────────────
        //
        // ⚠ 不回的话 SockJS 会在超时后主动断开 —— 而断开的样子
        // 与「直播结束了」**一样**。
        if text == SOCKJS_HEARTBEAT {
            return Ok(FrameActions {
                outbound: vec![Outbound::from(SOCKJS_HEARTBEAT)],
                kind: FrameKind::Heartbeat,
                ..Default::default()
            });
        }

        // ── `c[...]`：服务端关闭 ────────────────────────────────────
        if text.starts_with(SOCKJS_CLOSE_PREFIX) {
            self.closed = true;
            // ⚠ `close` 给的是 `None`（连接已经是关的、没有「再关一次」），
            // 但 `kind == Closed` 让上层能报「服务端关了」这个**可区分**的结论。
            // 见 [`FrameActions::close`] 的说明。
            return Ok(FrameActions::silent(FrameKind::Closed));
        }

        // ── `o`：连接打开 ⇒ 发 PROVISION ────────────────────────────
        if text == SOCKJS_OPEN {
            if self.provision_sent {
                // 重复的 `o`：SockJS 重连时会再发一次，但 PROVISION **只能发一次**
                // —— 重复发会让服务端把这条连接当成**新会话**。
                return Ok(FrameActions::silent(FrameKind::Open));
            }
            let frame = self.provision_frame(now_millis)?;
            self.provision_sent = true;
            return Ok(FrameActions {
                outbound: vec![Outbound::from(frame)],
                kind: FrameKind::Open,
                ..Default::default()
            });
        }

        // ── 其余都不是数据帧 ────────────────────────────────────────
        if !text.starts_with(SOCKJS_DATA_PREFIX) {
            return Ok(FrameActions::silent(FrameKind::Ignored));
        }

        let messages = match decode_sockjs_data(text) {
            Ok(messages) => messages,
            Err(_) => return Ok(self.malformed_actions()),
        };

        let mut actions = FrameActions::silent(FrameKind::Silent);
        let mut malformed = false;

        for message in messages {
            let raw = match mcloud_crypto::b64_decode(&message) {
                Ok(raw) => raw,
                Err(_) => {
                    malformed = true;
                    continue;
                }
            };
            let sync = match decode_msync(&raw) {
                Ok(sync) => sync,
                Err(_) => {
                    malformed = true;
                    continue;
                }
            };
            if self.handle_sync(&sync, now_millis, &mut actions)? {
                malformed = true;
            }
        }

        if malformed && actions.kind == FrameKind::Silent {
            actions.kind = FrameKind::Malformed;
        }
        // ⚠ 一帧解不开**不算**这条链的失败（`a[...]` 里可以有多条，也还有下一帧）。
        // 但**连续**坏帧要计数：对面不是我们以为的那个端点时，继续挂着
        // 只会一直烧时间（见 [`Self::malformed_limit`]）。
        //
        // 清零由 [`Self::on_frame`] 的外层统一做（它看的是**归类结果**，
        // 不是这里的 `malformed` 局部量）—— 这样 early return 的几条路径
        // 就不会绕过它。
        if actions.kind == FrameKind::Malformed {
            self.note_malformed(&mut actions);
        }
        Ok(actions)
    }

    /// 造一个「解不开」的 `FrameActions`，并计入连续坏帧。
    fn malformed_actions(&mut self) -> FrameActions {
        let mut actions = FrameActions::silent(FrameKind::Malformed);
        self.note_malformed(&mut actions);
        actions
    }

    /// 记一次坏帧；到了阈值就在 `actions` 上挂一个关闭请求。
    ///
    /// ⚠ **只负责"加一"**，清零在 [`Self::on_frame`] 里统一做
    /// —— 见那里关于 early return 的说明。
    fn note_malformed(&mut self, actions: &mut FrameActions) {
        self.consecutive_malformed = self.consecutive_malformed.saturating_add(1);
        if self.malformed_limit != 0 && self.consecutive_malformed >= self.malformed_limit {
            actions.close = Some(CloseReason::TooManyMalformed);
        }
    }

    /// 处理**一条**解出来的 MSync 报文，把结果并进 `actions`。
    ///
    /// 返回「这条报文算不算解不开」（`true` = 解不开）。
    fn handle_sync(
        &mut self,
        sync: &MsyncMessage,
        now_millis: i64,
        actions: &mut FrameActions,
    ) -> Result<bool> {
        // ── PROVISION 的响应：**必须**补发 UNREAD ───────────────────
        //
        // ⚠⚠ 少了这一条，服务端**不推弹幕** —— 而症状是
        // 「WS 连得上、心跳正常、只是永远收不到弹幕」。
        if sync.command == CMD_PROVISION {
            actions.kind = FrameKind::ProvisionAck;
            // 记下「这条连接真的建立起来了」—— 连接层据此区分
            // 「直播结束（从未建起来）」与「被断开」。见 [`Self::provision_ack_seen`]。
            self.provision_ack_seen = true;
            if !self.unread_sent {
                let frame = self.unread_frame(now_millis)?;
                self.unread_sent = true;
                actions.outbound.push(Outbound::from(frame));
            }
            return Ok(false);
        }

        let frame = match extract_danmaku_frame(sync.command, &sync.payload) {
            Ok(frame) => frame,
            Err(_) => {
                // 解不开**不算**这一帧的失败：还有下一帧。
                // 但要把它记下来（`Malformed`）—— 只回一个空文本的话，
                // 「一直收不到弹幕」与「弹幕本来就没有」就分不开了。
                actions.kind = FrameKind::Malformed;
                return Ok(true);
            }
        };
        if frame.text.is_empty() {
            // 心跳类的 SYNC、别的 command：正常，没有文本。
            return Ok(false);
        }

        // ── 事件消息：**整条丢掉**，它根本不是口令来源 ──────────────────
        //
        // ⚠⚠ 这里**必须**在跨帧拼接**之前**返回，两个理由都是硬的：
        //
        // 1. 判据是上游的 `ext.msgType`（实拍：2 入场 / 5 点赞 / 6 系统 / 7 礼物），
        //    而不是文本形状 —— 实拍里 `加入直播间`（五个字）与真口令
        //    `一键9图出圈`（五个字）在字形上分不开。
        // 2. 下面那段跨帧拼接是**唯一**一条"整条文本当口令"的路：
        //    上一条是口令头（`复制口令1：`）时，**下一条整条**会被当成口令。
        //    而弹幕流是**混着推**的 —— 头与内容之间随时会插进来一条入场/礼物提示。
        //    不在这里挡住，`加入直播间` 这种事件就会被当成口令投上去。
        //    （漏挡的代价：一次 `200108` 口令无效，外加用户看到一串莫名其妙的口令名。）
        if is_event_message(frame.msg_type) {
            actions.kind = FrameKind::EventMessage;
            // ⚠ `danmaku` **留空**：它表示"这一帧解出了弹幕文本"，
            // 而事件消息不该出现在任何"弹幕"口径里（也顺带说明为什么没进 `codes`）。
            return Ok(false);
        }

        let danmaku = frame.text;
        actions.kind = FrameKind::Danmaku;
        actions.danmaku = danmaku.clone();

        // ── 跨帧拼接 ────────────────────────────────────────────────
        //
        // 上一条弹幕**整条就是个口令头**（`复制口令1：` / 裸的 `小红花口令：`），
        // 下一条整条就是口令本身 —— 口令被服务端切成了两条消息。
        if self.pending_head {
            self.pending_head = false;
            let trimmed = danmaku.trim();
            // ⚠ 下一条**自己还是个口令头**时不算（那是连续两条头，
            // 中间那条口令丢了）—— 对齐原实现。
            // 也排除「无随机」那种收尾词。
            if !trimmed.is_empty() && !is_kouling_head_fragment(trimmed) {
                let added = self.add(trimmed);
                actions.codes.extend(added);
            }
        }

        // ── 口令：走 `live_kouling` 的**全形态**抽取器 ──────────────────
        //
        // ⚠⚠ 这里**刻意**不用"只认前缀「小红花口令：」"的那一版 ——
        // 本仓**曾**有一个 `live_kouling::extract_prefixed_kouling` 收窄版，
        // 2026-10-09 按用户口径（「不要只认，ts版有规则」）**已删除**：
        //
        // 1. **依据优先级**（用户口径 2026-09-28，写在 `AGENTS.md`）是
        //    **TS 参考实现 > 真机抓包 > 推断**。TS 的 `extractKouling` 认四种形态
        //    （`小红花口令：X` / `复制口令N：` / `口令是X` / `1:X 2:Y 无随机`），
        //    收窄成"只认前缀"是在**抓包没看到**的形态上做减法 —— 而那属于
        //    "拿推断盖过 TS"。
        // 2. **黄金向量钉着它**：`test/golden/live-room-vectors.json` 的
        //    `msyncInbound` 里那条 NOTICE 帧的文本就是
        //    `口令1:MClaw全能小助手 口令2:毕业之旅MClaw相伴`（实拍形状），
        //    `live_ws::tests::notice_payload_yields_two_kouling_codes` 期望解出两条。
        // 3. **代价不对称**：口令只在直播那一小时有效，**漏掉真口令没有第二次机会**；
        //    多试一条废口令的代价只是一次 `200108`（实拍证明零消耗）。
        //
        // ⚠ 事件消息进不来这件事**不靠这一层**（这一层从来就没把事件当口令：
        // `加入直播间` / `送了1个火箭` 不含任何口令形态，解出来本来就是空）。
        // 真正漏进来的那条路是**跨帧拼接**（见下），而它现在由上面那道
        // `is_event_message` 挡住 + 下面那个**严格**的口令头判据一起收口。
        for code in crate::live_kouling::extract_kouling(&danmaku) {
            let added = self.add(&code);
            actions.codes.extend(added);
        }

        // 本条**整条**是个口令头 ⇒ 下一条整条可能就是口令。
        //
        // ⚠ 判据必须是**严格**的"整条只是个头"（以冒号结尾），
        // 不能是"含口令二字"：实拍里那条完整口令 `小红花口令：一键9图出圈`
        // **也含**「口令」，用宽松判据会给它的**下一条**弹幕挂上拼接状态
        // —— 而弹幕流是**混着推**的（实拍那 15 条就是口令与入场/礼物交替），
        // 下一条正好是 `加入直播间` / `送了1个火箭` 时，整条会被当成口令，
        // 于是日志里出现「口令：加入直播间」（上游回一次 `200108`）。
        // 判据取自 `live_kouling`（那边是 `(?:复制)?口令\d*[：:]$`）。
        if crate::live_kouling::is_kouling_head(&danmaku) {
            self.pending_head = true;
        }

        Ok(false)
    }

    /// 记一个口令；已见过就返回空。
    fn add(&mut self, code: &str) -> Vec<String> {
        if code.is_empty() {
            return Vec::new();
        }
        if !self.seen.insert(code.to_string()) {
            return Vec::new();
        }
        self.codes.push(code.to_string());
        vec![code.to_string()]
    }

    /// `js_<毫秒>` —— PROVISION 与两条控制帧共用同一个形状。
    ///
    /// ⚠ 每次现取（原项目就是这样）—— 它不是「会话 id」，
    /// 而是**这一次请求的标识**。
    fn resource_at(now_millis: i64) -> String {
        format!("js_{now_millis}")
    }

    /// 构造 PROVISION 帧（`["<base64>"]`）。
    fn provision_frame(&mut self, now_millis: i64) -> Result<String> {
        let resource = Self::resource_at(now_millis);
        // ⚠ `deviceUuid` 是 `String(Date.now())` —— 与 `resource` **同源**，
        // 但**不是同一个串**（少了 `js_` 前缀）。照抄。
        let device_uuid = now_millis.to_string();
        let payload = live_provision_payload(&self.access_token, &resource, &device_uuid);
        self.encode_frame(CMD_PROVISION, payload, now_millis)
    }

    /// 构造 UNREAD 帧（`["<base64>"]`）。
    fn unread_frame(&mut self, now_millis: i64) -> Result<String> {
        self.encode_frame(CMD_UNREAD, Vec::new(), now_millis)
    }

    /// 把一条 MSync 报文包成 SockJS 的**发送**形状。
    ///
    /// # ⚠ 别与 [`sockjs_data_frame`] 混起来
    ///
    /// ```text
    /// 客户端发 → `["<base64>"]`    （无前缀，**本函数**）
    /// 服务端推 → `a["<base64>"]`   （前缀 `a[`，[`sockjs_data_frame`]）
    /// ```
    ///
    /// 前缀写错的表现是「帧被归成不认识的一类、安静丢掉」——
    /// 也就是「连上了、心跳也正常、就是收不到弹幕」，
    /// 与漏发 UNREAD 的症状**一模一样**。
    ///
    /// ⚠ 报文本身走 [`live_msync_frame`] 构造，**不在这里手拼结构体** ——
    /// 这条报文里有六项恒定字段（含那个必须显式给零值的 `pov`），
    /// 手拼时漏一项不会被服务端立刻拒掉，只会让报文与原客户端不同。
    fn encode_frame(&mut self, command: u8, payload: Vec<u8>, now_millis: i64) -> Result<String> {
        let message = live_msync_frame(
            &self.access_token,
            Some(&self.phone),
            &self.chat_room_id,
            command,
            payload,
            now_millis,
            self.rng.as_mut(),
        );
        Ok(sockjs_send_frame(&encode_msync(&message)))
    }
}

/// 把一条已编码的 MSync 报文包成 SockJS 的**发送**形状：`["<base64>"]`。
///
/// 一个**只含一个字符串**的 JSON 数组 —— 这是 SockJS 的发送约定。
pub fn sockjs_send_frame(msync_bytes: &[u8]) -> String {
    format!("[\"{}\"]", mcloud_crypto::b64_encode(msync_bytes))
}

/// 把一条 MSync 报文包成 SockJS **推下来**的数据帧文本：`a["<base64>"]`。
///
/// # 它的用途
///
/// 让上层（任务编排层、以及 [`crate::live_ws_client`] 的离线测试）能造出
/// **入站帧**来测「收到帧之后该回什么」，而不必硬编码一段 base64。
///
/// # ⚠ 别与发送形状混起来
///
/// ```text
/// 服务端推 → `a["<base64>"]`   （前缀 `a[`，**本函数**）
/// 客户端发 → `["<base64>"]`    （无前缀，[`sockjs_send_frame`]）
/// ```
///
/// 两者的 base64 主体完全一样，只有外层不同 —— 而那个外层正是 SockJS 的
/// 帧协议。前缀写错的表现是「帧被归成不认识的一类、安静丢掉」。
pub fn sockjs_data_frame(msync_bytes: &[u8]) -> String {
    format!(
        "{SOCKJS_DATA_PREFIX}\"{}\"]",
        mcloud_crypto::b64_encode(msync_bytes)
    )
}

/// 从 `a[...]` 数据帧里解出消息数组。
///
/// # ⚠ 这里**刻意**与原实现不同
///
/// 原实现是 `text.slice(3, text.lastIndexOf('"]'))` —— **只对单条消息成立**。
/// SockJS 在缓冲时会一次发多条：`a["QUJD","REVG"]`，那种帧被它切出来是
/// `QUJD","REVG`，base64 解不开 ⇒ **整帧被丢**。
///
/// 换成按 JSON 解之后：
///
/// - **单条的行为逐字相同**（同一个 base64 串）；
/// - 多条则从「丢掉」变成「都能处理」。
///
/// 这是一处**只有收益的偏离**，写在这里是为了下一次有人对着原实现核代码时
/// 不会以为抄错了。
fn decode_sockjs_data(text: &str) -> Result<Vec<String>> {
    // 跳过 `a`，把剩下的 `[...]` 当 JSON 数组解。
    let json = &text[SOCKJS_DATA_PREFIX.len() - 1..];
    serde_json::from_str::<Vec<String>>(json)
        .map_err(|e| Error::Parse(format!("SockJS 数据帧不是合法 JSON 数组：{e}")))
}

/// 造一条**入站**的弹幕帧（`a["<base64>"]`），供上层与测试使用。
///
/// 它把 `live_msync` 的报文构造 + 编码 + SockJS 包装三步串起来 ——
/// 少了任何一步都得不到一条「服务端会发出来的样子」的帧。
pub fn inbound_danmaku_frame(
    access_token: &str,
    phone: &str,
    room_id: &str,
    command: u8,
    payload: Vec<u8>,
    now_millis: i64,
    rng: &mut dyn Random,
) -> String {
    let message = live_msync_frame(
        access_token,
        Some(phone),
        room_id,
        command,
        payload,
        now_millis,
        rng,
    );
    sockjs_data_frame(&encode_msync(&message))
}

#[cfg(test)]
mod tests;
