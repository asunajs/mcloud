//! `live_ws_client` 的离线测试。
//!
//! # 为什么这里必须有一个"假 SockJS 服务端"
//!
//! **真实 WS 现在结构性不存在**：2026-09-22 / 09-23 两次实拍都是
//! `chatRoomId: null`（未开播），而 WS 只在**主播开播那一小时**才存在。
//! 也就是说，"连上真服务端"这条路径在本仓**无法被 CI 验证** ——
//! 那部分只能等开播后手工确认。
//!
//! 于是这里起一个**本地**的假 SockJS 服务端，把客户端真实地连上去
//! （真的 TCP、真的 WS 升级握手、真的 `tokio-tungstenite` 编解码），
//! 只把"对面是谁"换掉。被验证的因此是**连接层自己的那部分**：
//! 帧怎么收、怎么发、时序对不对、到点会不会断。
//!
//! # 两条通道，各测各的
//!
//! | 通道 | 覆盖什么 | 代价 |
//! |---|---|---|
//! | [`duplex`]（内存双工） | 帧时序与状态机接线 | 无 TCP，快 |
//! | 真 [`TcpListener`] | 升级握手 + 头 + 真实 socket | 慢一点，但更接近真 |
//!
//! [duplex]: tokio::io::duplex

use std::time::Duration;

use mcloud_core::testkit::FixedClock;
use mcloud_crypto::{Random, SeqRandom};
use tokio::io::duplex;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::live_msync::{
    encode_message_body, MessageBody, MessageContent, CMD_NOTICE, CMD_PROVISION, CMD_SYNC,
};
use crate::live_ws::{inbound_danmaku_frame, SOCKJS_HEARTBEAT, SOCKJS_OPEN};

// ─────────────────────────────────────────────────────────────────────────────
// 测试夹具
// ─────────────────────────────────────────────────────────────────────────────

/// 测试的总兜底上限。
///
/// # 为什么每条听写都要套一层"总超时"
///
/// 本模块**专门**在防"无限挂着"（那正是没有 `listen_for` 时会发生的事）。
/// 若测试脚手架自己有 bug（最初就发生过：假服务端 await 一个永不到来的回应，
/// 于是 `--test-threads=1` 下整个测试进程挂死），CI 会变成一个不结束的作业 ——
/// 而**不结束**比**失败**难查得多。
///
/// 所以每条涉及听写的测试都套这一层：超时即失败，附一句"客户端没按上限断开"。
/// 它兜的是**脚手架**的错，不是被测代码的错 —— 被测代码的到点断开由
/// [`listen_deadline_disconnects_even_when_server_is_silent`] 单独钉。
const TEST_BUDGET: Duration = Duration::from_secs(20);

/// 在总预算内跑一段听写，超时就是测试失败（见 [`TEST_BUDGET`]）。
async fn within_budget<F, T>(what: &str, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(TEST_BUDGET, fut).await {
        Ok(v) => v,
        Err(_) => panic!("「{what}」超过 {TEST_BUDGET:?} 还没结束 —— 脚手架或客户端「挂住」了"),
    }
}

/// 固定的毫秒时间戳（对齐黄金向量里那个时刻）。
const NOW_MS: i64 = 1_700_000_000_000;

/// 测试用的聊天室 id（31 位，形状与真实 `chatRoomId` 一致）。
const CHAT_ROOM_ID: &str = "1470000000000000000000000000000";

/// 测试用的凭据。
const ACCESS_TOKEN: &str = "test-access-token";

/// 测试用的手机号。
const PHONE: &str = "15000000000";

/// 造一条**入站的** SYNC 弹幕帧（`a["<base64>"]`），文本是 `text`。
///
/// 走 `encode_message_body` 手工造 payload，比硬编码 base64 可读得多 ——
/// 而且口令文本改了之后期望值跟着改，不会出现"测试绿了但测的是别的东西"。
fn inbound_sync(text: &str, rng: &mut dyn Random) -> String {
    let body = MessageBody {
        contents: vec![MessageContent {
            content_type: 1,
            text: text.to_string(),
        }],
        ..Default::default()
    };
    let payload = encode_message_body(&body);
    inbound_danmaku_frame(
        ACCESS_TOKEN,
        PHONE,
        CHAT_ROOM_ID,
        CMD_SYNC,
        payload,
        NOW_MS,
        rng,
    )
}

/// 造一条**入站的** PROVISION 响应帧（`command = 3`）。
///
/// ⚠ 这一帧是"要不要补发 UNREAD"的触发条件 —— 缺了它服务端永远不推弹幕，
/// 而现象是"连上了、心跳也正常、就是没弹幕"。所以测试必须真的发它。
fn inbound_provision_ack(rng: &mut dyn Random) -> String {
    inbound_danmaku_frame(
        ACCESS_TOKEN,
        PHONE,
        CHAT_ROOM_ID,
        CMD_PROVISION,
        Vec::new(),
        NOW_MS,
        rng,
    )
}

/// 造一条**入站的** NOTICE 弹幕帧（另一条三级兜底路径）。
fn inbound_notice(text: &str, rng: &mut dyn Random) -> String {
    let body = MessageBody {
        contents: vec![MessageContent {
            content_type: 1,
            text: text.to_string(),
        }],
        ..Default::default()
    };
    let payload = encode_message_body(&body);
    inbound_danmaku_frame(
        ACCESS_TOKEN,
        PHONE,
        CHAT_ROOM_ID,
        CMD_NOTICE,
        payload,
        NOW_MS,
        rng,
    )
}

/// 造一个确定性的状态机（随机源走固定序列）。
fn listener() -> KoulingListener {
    KoulingListener::new(
        ACCESS_TOKEN,
        CHAT_ROOM_ID,
        PHONE,
        Box::new(SeqRandom::new(0)),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// 通道一：内存双工（`tokio::io::duplex`）
// ─────────────────────────────────────────────────────────────────────────────

/// 一对通过内存双工连起来的 WS 端点：(客户端流, 服务端流)。
///
/// 服务端侧要**手工完成升级握手**（`accept_async`）—— 这正好也验证了
/// 客户端发的升级请求是合法的（否则 `accept_async` 会直接失败）。
///
/// ⚠ 客户端流的类型是 `WebSocketStream<DuplexStream>`，**不是** `LiveWsStream`
/// （那个是 `MaybeTlsStream<TcpStream>`）—— 这正是 [`listen_on_stream`] 必须
/// 对底层流泛型的原因。
async fn duplex_pair() -> (
    WebSocketStream<tokio::io::DuplexStream>,
    WebSocketStream<tokio::io::DuplexStream>,
) {
    let (client_io, server_io) = duplex(64 * 1024);

    // ⚠⚠ 两侧的升级握手**必须并发**跑。
    //
    // WS 升级是「客户端发请求 → 服务端回 101」。若像最直白那样
    // `client_async(..).await` 之后再 `accept_async(..).await`，
    // **客户端会一直等一个还没人被 poll 的响应** —— 测试直接挂死
    // （这个坑本文件真的踩过：`--test-threads=1` 下整个测试进程不结束，
    // 而不结束比失败难查得多）。`join!` 让两侧同时被驱动。
    //
    // 生产路径没有这个问题：那边对端是真的服务端，会自己回 101。
    let (client, server) = tokio::join!(
        tokio_tungstenite::client_async("ws://localhost/ws/0/abcdefghijklm/websocket", client_io),
        accept_async(server_io),
    );
    (
        client.expect("客户端升级握手失败").0,
        server.expect("服务端升级握手失败"),
    )
}

/// 把服务端要推的帧**按序**推完，并尽力收下客户端的每一帧回应。
///
/// # ⚠ 这里必须容忍"客户端提前闭嘴"
///
/// SockJS 是请求-响应式的，所以每推一帧都收一次回应 —— 这是让
/// "客户端该回什么"在**服务端侧**被真实观察到的唯一办法。
///
/// 但至少有一条测试（[`aborts_after_too_many_malformed_frames`]）里客户端会
/// **主动停止读取**（状态机因连续坏帧要求收工）。那时服务端再 await 回应就
/// 永远等不到 —— 会**挂死整个测试**。所以：
///
/// - 收回应带**短超时**，超时就认为"这一轮没有回应"，继续推下一帧；
/// - 一旦发现连接已经关了（`None` / `Err`），立刻停 —— 后面没什么可推的了。
///
/// 这样"脚本推完"与"客户端提前收工"两种情况都能正常结束。
async fn serve_script(
    mut server: WebSocketStream<tokio::io::DuplexStream>,
    script: Vec<String>,
) -> Vec<String> {
    let mut received = Vec::new();
    for frame in script {
        // 推不出去（对端已关）就是结束了，不是测试失败。
        if server.send(Message::text(frame)).await.is_err() {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(500), server.next()).await {
            Ok(Some(Ok(Message::Text(reply)))) => received.push(reply.to_string()),
            // 收到非文本帧（Close 等）：继续推没意义。
            Ok(Some(Ok(_))) => break,
            // 对端关了（`None`）或读出错 ⇒ 这条连接到此为止。
            Ok(Some(Err(_))) | Ok(None) => break,
            // 超时：这一轮没有回应。客户端可能已经收工了，
            // 但**不立刻放弃** —— 剩下的帧里也许还有能触发的。
            Err(_) => {}
        }
    }
    received
}

/// **核心时序测试**：`o` → PROVISION；MSync(3) → UNREAD；`h` → `h`。
///
/// # 这条链最容易漏的一步
///
/// 漏掉 UNREAD 之后：WS 连得上、心跳正常、**只是永远收不到弹幕**。
/// 排查方向会指向"是不是没人在发"，而不是"少发了一帧"。
/// 所以这里把三步的**顺序**逐条钉死，而不是只看"有没有发过 PROVISION"。
#[tokio::test]
async fn open_then_provision_ack_then_unread_in_order() {
    let (client, server) = duplex_pair().await;
    let mut rng = SeqRandom::new(7);

    // 服务端脚本：`o` → PROVISION 响应 → 心跳 `h` → 一条弹幕。
    let script = vec![
        SOCKJS_OPEN.to_string(),
        inbound_provision_ack(&mut rng),
        SOCKJS_HEARTBEAT.to_string(),
        inbound_sync("口令1：中秋美好存云盘", &mut rng),
    ];

    let mut listener = listener();
    let serve = tokio::spawn(serve_script(server, script));
    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");

    let received = within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    // ── 断言 ①：收到 `o` 之后的**第一个**回帧必须是 PROVISION ──────────
    assert!(
        !received.is_empty(),
        "收到 `o` 之后什么都没回 —— PROVISION 没发出去"
    );
    let provision = &received[0];
    assert!(
        provision.starts_with("[\""),
        "PROVISION 必须是 SockJS 的发送形状 `[\"<base64>\"]`，实际是 {provision}"
    );

    // ── 断言 ②：收到 MSync(command=3) 之后**必须**补发 UNREAD ───────────
    //
    // 这是全链最容易漏的一步：漏了它服务端**永远不推弹幕**。
    assert!(
        received.len() >= 2,
        "PROVISION 响应之后没有回第二帧 —— UNREAD 漏了（服务端将永远不推弹幕）。已收到：{received:?}"
    );
    let unread = &received[1];
    assert!(
        unread.starts_with("[\""),
        "UNREAD 也必须是 SockJS 发送形状，实际是 {unread}"
    );

    // 两条控制帧的 base64 主体必须**不同**（否则就是把 PROVISION 又发了一遍）。
    assert_ne!(
        provision, unread,
        "第二帧与第一帧逐字相同 —— 说明 UNREAD 没有真的被构造出来"
    );

    // ── 断言 ③：心跳 `h` 要**原样**回 `h` ──────────────────────────────
    //
    // 不回的话 SockJS 会在超时后主动断开，而断开的样子与"直播结束了"一样。
    assert!(
        received.iter().any(|f| f == SOCKJS_HEARTBEAT),
        "心跳没有原样回 `h`（SockJS 会超时断开）。已收到：{received:?}"
    );

    // ── 断言 ④：弹幕里的口令被收集 ────────────────────────────────────
    assert_eq!(
        report.codes,
        vec!["中秋美好存云盘".to_string()],
        "口令没有被收集"
    );
    assert!(report.heard_open, "没记下收到过 `o`");
    assert!(report.heard_provision_ack, "没记下收到过 PROVISION 响应");
    assert!(report.entered_room(), "这条连接应当被判定为「进过房间」");
}

/// 用状态机的**自述**再核一遍那条时序（不依赖服务端侧观察到的帧）。
///
/// 与服务端侧观察互为正反面：那边看"收到了什么"，这边看"状态机认为发过什么"。
/// 两边都成立，才排除"发了但发错内容"这种半对的情况。
#[tokio::test]
async fn listener_records_provision_and_unread_sent() {
    let (client, server) = duplex_pair().await;
    let mut rng = SeqRandom::new(11);

    let script = vec![SOCKJS_OPEN.to_string(), inbound_provision_ack(&mut rng)];
    let mut listener = listener();
    let serve = tokio::spawn(serve_script(server, script));

    let _report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert!(listener.provision_sent(), "PROVISION 应当发过");
    assert!(
        listener.unread_sent(),
        "UNREAD 应当发过 —— 漏了它服务端永远不推弹幕"
    );
    assert!(
        listener.provision_ack_seen(),
        "应当记下收到过 PROVISION 响应"
    );
}

/// ⚠ **漏 UNREAD 的对照实验**：不发 PROVISION 响应时，客户端**不该**发 UNREAD。
///
/// # 为什么这条测试是必要的
///
/// 上一条测试断言"发了 UNREAD"。但若实现改成了"收到 `o` 就无条件发两条"，
/// 那条测试**照样绿** —— 而真实的 bug（少发 UNREAD）与错误的修复
/// （多发一条）会同时藏进去。这条对照把"UNREAD 是**被 PROVISION 响应触发**的"
/// 这一点钉死：没有响应就没有 UNREAD。
#[tokio::test]
async fn no_unread_without_provision_ack() {
    let (client, server) = duplex_pair().await;

    // 只发 `o`，**不发** PROVISION 响应。
    let script = vec![SOCKJS_OPEN.to_string()];
    let mut listener = listener();
    let serve = tokio::spawn(serve_script(server, script));

    let _ = listen_on_stream(
        client,
        &mut listener,
        Duration::from_millis(300),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    let received = within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert!(
        listener.provision_sent(),
        "PROVISION 应当发过（由 `o` 触发）"
    );
    assert!(
        !listener.unread_sent(),
        "没有收到 PROVISION 响应却发了 UNREAD —— UNREAD 的触发条件错了"
    );
    // 服务端侧只应收到 PROVISION 那一帧。
    assert_eq!(
        received.len(),
        1,
        "只发了 `o` 却收到多于 1 帧回应：{received:?}"
    );
}

/// `h` 心跳**原样回 `h`**，且回的内容必须逐字相等（不是自造的 pong）。
#[tokio::test]
async fn heartbeat_is_echoed_verbatim() {
    let (client, server) = duplex_pair().await;

    let mut listener = listener();
    let serve = tokio::spawn(serve_script(server, vec![SOCKJS_HEARTBEAT.to_string()]));
    let _ = listen_on_stream(
        client,
        &mut listener,
        Duration::from_millis(400),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    let received = within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert_eq!(
        received,
        vec![SOCKJS_HEARTBEAT.to_string()],
        "心跳没有回一个逐字相同的 `h`"
    );
}

/// 弹幕口令被抽出（`a["…"]` 路径），NOTICE 那条兜底也要能走通。
#[tokio::test]
async fn danmaku_codes_collected_from_sync_and_notice() {
    let (client, server) = duplex_pair().await;
    let mut rng = SeqRandom::new(3);

    let script = vec![
        inbound_sync("口令1：一键9图出圈", &mut rng),
        inbound_notice("口令2：中秋美好存云盘", &mut rng),
    ];
    let mut listener = listener();
    let serve = tokio::spawn(serve_script(server, script));

    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert_eq!(
        report.codes,
        vec!["一键9图出圈".to_string(), "中秋美好存云盘".to_string()],
        "两条弹幕里的口令没有被按序收集"
    );
}

/// **听写时长上限到点主动断开**（不能无限挂着）。
///
/// # 为什么这条是硬要求
///
/// 没有上限时，"门关着"（服务端不发任何帧）与"正在听"从外面看完全一样 ——
/// 一次调用会一直挂着。所以这里用一个**什么都不推**的服务端，
/// 断言到点后确实回来了，且归因是"我们按上限收的"。
#[tokio::test]
async fn listen_deadline_disconnects_even_when_server_is_silent() {
    let (client, server) = duplex_pair().await;

    let mut listener = listener();
    // 服务端**一帧都不推**：这条测试专门测"没有帧时到点也会断"。
    let serve = tokio::spawn(async move {
        let mut server = server;
        // 挂着不推，等客户端自己断。收到 Close 帧就返回。
        while let Some(Ok(msg)) = server.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    let started = std::time::Instant::now();
    // ⚠ 这一条**刻意**不用 `within_budget`：它要测的就是"到点会断"，
    // 所以给它一个比被测上限（300ms）宽、但比 `TEST_BUDGET` 窄的窗口。
    // 到点没断 ⇒ 这个 `timeout` 先触发 ⇒ 直接报"客户端无限挂着"。
    let report = tokio::time::timeout(
        Duration::from_secs(3),
        listen_on_stream(
            client,
            &mut listener,
            Duration::from_millis(300),
            &FixedClock(NOW_MS),
        ),
    )
    .await
    .expect("到点没有断开 —— 客户端无限挂着（这正是要防的）")
    .expect("听写失败");

    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(250),
        "提前返回了（{elapsed:?}），说明不是因为到点才断的"
    );
    assert_eq!(
        report.end,
        WsEnd::ListenedEnough,
        "到点收工应当归为「听满时长」，而不是被当成断线"
    );
    assert!(report.end.is_normal());
    assert!(report.codes.is_empty(), "服务端没推任何弹幕，不该有口令");

    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");
}

/// 连不上（服务端在握手后立刻关闭）：要**区分**于"听满"。
///
/// 这一支对应"根本没进过房间" —— 也就是**看起来像"直播结束了"**的那种。
#[tokio::test]
async fn peer_gone_without_ever_entering_room() {
    let (client, server) = duplex_pair().await;

    // 服务端立刻关连接，连 `o` 都不发。
    tokio::spawn(async move {
        let mut server = server;
        let _ = server.close(None).await;
    });

    let mut listener = listener();
    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");

    assert!(
        report.end.is_broken(),
        "服务端直接关了连接，应当归为断线类，实际是 {:?}",
        report.end
    );
    assert!(
        !report.entered_room(),
        "从没收到过 `o` 与 PROVISION 响应，不该被判为进过房间"
    );
    assert_eq!(
        report.end,
        WsEnd::PeerGone,
        "没进过房间就断了 ⇒ PeerGone（长得像「直播结束了」）"
    );
}

/// **被断开**（进过房间之后才断）与 [`WsEnd::PeerGone`] 必须分得开。
///
/// # ⚠ 这两者为什么会混
///
/// SockJS 心跳超时踢人与"直播结束"在 TCP 层**完全同形**（都是 EOF/Close）。
/// 本仓能提供的唯一判据是"进没进过房间"，而这条测试钉的就是那个判据：
/// 同一个"服务端关闭"的动作，只因**之前收到过 `o` + PROVISION 响应**，
/// 归因就从 `PeerGone` 变成 `Disconnected`。
#[tokio::test]
async fn disconnected_after_entering_room() {
    let (client, server) = duplex_pair().await;
    let mut rng = SeqRandom::new(5);

    let script = vec![SOCKJS_OPEN.to_string(), inbound_provision_ack(&mut rng)];
    let mut listener = listener();
    // 推完两帧之后关掉服务端 ⇒ 客户端看到 EOF。
    let serve = tokio::spawn(serve_script(server, script));

    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert!(report.entered_room(), "这条连接应当被判为进过房间");
    assert_eq!(
        report.end,
        WsEnd::Disconnected,
        "进过房间之后被断开 ⇒ Disconnected（更像 SockJS 心跳超时踢人）"
    );
    assert_ne!(
        report.end,
        WsEnd::PeerGone,
        "「进过房间后断开」不能与「从没进过房间」混为一谈"
    );
}

/// 服务端在**进过房间之后**发 SockJS 的 `c[...]` ⇒ 明确归为 [`WsEnd::ServerClosed`]。
///
/// ⚠ 这条测试的**前提是"进过房间"**（脚本先发 `o` 与 PROVISION 响应）。
/// 没进过房间时的关闭落到 [`WsEnd::PeerGone`] —— 那一半由
/// [`close_frame_without_entering_room_is_peer_gone`] 覆盖。
/// 两条合起来才说明 `classify_end` 的优先级是对的。
#[tokio::test]
async fn server_close_frame_yields_server_closed() {
    let (client, server) = duplex_pair().await;
    let mut rng = SeqRandom::new(17);

    let mut listener = listener();
    let serve = tokio::spawn(serve_script(
        server,
        vec![
            SOCKJS_OPEN.to_string(),
            inbound_provision_ack(&mut rng),
            crate::live_ws::SOCKJS_CLOSE_PREFIX.to_string() + "3000,\"Go away\"]",
        ],
    ));

    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert!(report.entered_room(), "这条连接应当进过房间");
    assert!(report.heard_close_frame, "应当记下收到过 SockJS 关闭帧");
    assert_eq!(
        report.end,
        WsEnd::ServerClosed,
        "进过房间 + 服务端明确发 `c[...]` ⇒ ServerClosed（区别于无声的 EOF）"
    );
}

/// ⚠ **优先级测试**：关闭帧出现在**没进过房间**的连接上 ⇒ 必须是 [`WsEnd::PeerGone`]。
///
/// # 这条测试防的是一个真实的顺序错误
///
/// 第一版 `classify_end` 把"收到关闭帧 ⇒ ServerClosed"放在最前，于是
/// **从没进过房间**时的关闭也被归成 `ServerClosed` —— 而那一支的真实含义是
/// "连上了、又被挂断"，属于**长得像"直播结束了"**的那一类。
/// 后果是 `PeerGone` 这一档**永远不会出现**，而它恰恰是本次任务要求
/// 如实区分的那一态。
#[tokio::test]
async fn close_frame_without_entering_room_is_peer_gone() {
    let (client, server) = duplex_pair().await;

    let mut listener = listener();
    // 一上来就 `c[...]`：`o` 与 PROVISION 响应**都没有**。
    let serve = tokio::spawn(serve_script(
        server,
        vec![crate::live_ws::SOCKJS_CLOSE_PREFIX.to_string() + "3000,\"Go away\"]"],
    ));

    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert!(
        !report.entered_room(),
        "没有 `o` 与 PROVISION 响应，不该算进过房间"
    );
    assert!(
        report.heard_close_frame,
        "仍然要如实记下收到过关闭帧（事实本身不能丢）"
    );
    assert_eq!(
        report.end,
        WsEnd::PeerGone,
        "没进过房间 ⇒ PeerGone 优先于「收到关闭帧 ⇒ ServerClosed」\
         ——顺序反了会让「直播结束了」这一档永远不出现"
    );
}

/// 反复断坏帧时，状态机会要求收工，连接层要**照办**（不能一直空转到超时）。
#[tokio::test]
async fn aborts_after_too_many_malformed_frames() {
    let (client, server) = duplex_pair().await;

    // 一批**不是** base64 的数据帧：状态机会连续归为 Malformed。
    let bad: Vec<String> = (0..12)
        .map(|i| format!("a[\"not-base64-{i}!!!\"]"))
        .collect();
    // 状态机的默认上限是 8，这里给足 12 帧确保越线。
    let mut listener = listener().with_malformed_limit(3);
    let serve = tokio::spawn(serve_script(server, bad));

    let report = listen_on_stream(
        client,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");
    within_budget("服务端脚本", serve)
        .await
        .expect("服务端任务 panic");

    assert!(
        report.malformed >= 3,
        "应当记下解不开的帧数，实际 {}",
        report.malformed
    );
    assert!(
        !report.end.is_normal(),
        "连续坏帧越线后应当收工，而不是一直挂到超时；实际 {:?}",
        report.end
    );
    assert!(!report.entered_room());
}

// ─────────────────────────────────────────────────────────────────────────────
// 通道二：真实 TcpListener（真 socket + 真升级握手）
// ─────────────────────────────────────────────────────────────────────────────

/// 起一个本地假 SockJS 服务端，返回它的基址。
///
/// 这里走**真的** `TcpListener` + `connect_async`，因此把
/// "升级请求的 URL / 头 / WS 握手"整条也验证了 —— 而 `duplex` 那条
/// 用的是 `client_async`（不经过 DNS/TCP）。
///
/// 返回的 `JoinHandle` 的返回值是**服务端观察到的升级请求头**，
/// 供调用方断言 `User-Agent` / `Origin` 确实发出去了。
async fn spawn_fake_sockjs(
    script: Vec<String>,
) -> (
    String,
    tokio::task::JoinHandle<(Vec<(String, String)>, Vec<String>)>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口失败");
    let addr = listener.local_addr().expect("取本地地址失败");

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("接受连接失败");

        // ⚠ 用 `accept_hdr_async` 而不是 `accept_async`：要**看到**客户端发的头，
        // 才能断言 `User-Agent` / `Origin` 真的带上了（那是服务端看得见的两项）。
        let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&seen);
        let mut server = tokio_tungstenite::accept_hdr_async(
            stream,
            move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                  resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                // ⚠ 回调**必须**把 `resp` 原样还回去：它上面有 SockJS
                // 要求的升级响应头，自己造一个会让握手失败。
                if let Ok(mut guard) = sink.lock() {
                    for (name, value) in req.headers() {
                        guard.push((
                            name.as_str().to_ascii_lowercase(),
                            value.to_str().unwrap_or("").to_string(),
                        ));
                    }
                }
                Ok(resp)
            },
        )
        .await
        .expect("服务端升级握手失败");

        let mut received = Vec::new();
        for frame in script {
            // 与 [`serve_script`] 同款容忍：客户端可能提前收工，
            // 那时服务端再等回应就会挂死整个测试。
            if server.send(Message::text(frame)).await.is_err() {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(500), server.next()).await {
                Ok(Some(Ok(Message::Text(reply)))) => received.push(reply.to_string()),
                Ok(Some(Ok(_))) | Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => {}
            }
        }
        let headers = seen.lock().map(|g| g.clone()).unwrap_or_default();
        (headers, received)
    });

    (format!("ws://{addr}"), handle)
}

/// 经**真实 TCP** 走完一条最小可用时序：`o` → PROVISION → ack → UNREAD。
///
/// 这条测试覆盖的是 `duplex` 那条**覆盖不到**的部分：`connect_async` 的
/// URL 解析、TCP 连接、WS 升级握手、以及自定义头是否真的发出去了。
#[tokio::test]
async fn real_tcp_upgrade_and_core_sequence() {
    let mut rng = SeqRandom::new(13);
    let script = vec![SOCKJS_OPEN.to_string(), inbound_provision_ack(&mut rng)];
    let (base, handle) = spawn_fake_sockjs(script).await;

    let endpoint = LiveWsEndpoint {
        base,
        ..LiveWsEndpoint::production()
    };

    let mut listener = listener();
    let url = live_ws_url(&endpoint.base, 7, "abcdefghijklm");
    let stream = connect_live_ws(&url, &endpoint)
        .await
        .expect("真实 TCP 连接失败");

    let report = listen_on_stream(
        stream,
        &mut listener,
        Duration::from_secs(5),
        &FixedClock(NOW_MS),
    )
    .await
    .expect("听写失败");

    let (headers, received) = within_budget("假服务端", handle)
        .await
        .expect("服务端任务 panic");

    // ── 升级请求真的带上了那两个头吗 ────────────────────────────────────
    //
    // `User-Agent` 与 `Origin` 是服务端**看得见**的两项，而且与 `/cmlive`
    // 那套 HTTP 头不是同一组（只有 `Origin` 共用）。漏发不会立刻报错，
    // 只会让这次连接"看起来不像真实客户端"。
    let find = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        find("origin").as_deref(),
        Some(LIVE_ORIGIN),
        "升级请求必须带参考实现那个 Origin"
    );
    assert_eq!(
        find("user-agent").as_deref(),
        Some(LIVE_USER_AGENT),
        "升级请求必须带实拍的 iPad UA"
    );

    assert!(
        report.entered_room(),
        "真实 TCP 路径上应当完成 `o` + PROVISION 响应"
    );
    assert!(
        received.len() >= 2,
        "真实 TCP 路径上 UNREAD 漏了：{received:?}"
    );
    assert!(listener.unread_sent());
}

/// 连不上一个**没人监听**的端口 ⇒ 必须是 `Err(Error::Network)`，不能 panic 也不能挂死。
///
/// 这是"连不上"那一态的直接验证（另一条路是握手后立刻断开，见
/// [`peer_gone_without_ever_entering_room`]）。
#[tokio::test]
async fn connect_refused_maps_to_network_error() {
    // 先占一个端口再放掉，拿到一个"刚刚还开着、现在没人听"的地址。
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("绑定失败");
    let addr = probe.local_addr().expect("取地址失败");
    drop(probe);

    let endpoint = LiveWsEndpoint {
        base: format!("ws://{addr}"),
        ..LiveWsEndpoint::production()
    };
    let url = live_ws_url(&endpoint.base, 1, "abcdefghijklm");

    let result = tokio::time::timeout(Duration::from_secs(10), connect_live_ws(&url, &endpoint))
        .await
        .expect("连接一个无人监听的端口竟然挂住了");

    match result {
        Err(Error::Network(msg)) => {
            assert!(
                msg.contains("连接直播间 WebSocket 失败"),
                "错误信息该带上人话前缀，实际：{msg}"
            );
        }
        Err(other) => panic!("连不上应当映射为 Error::Network，实际是 {other:?}"),
        Ok(_) => panic!("连一个无人监听的端口竟然成功了"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯函数（地址拼装 / 随机形状）
// ─────────────────────────────────────────────────────────────────────────────

/// URL 形状逐字对齐参考实现
/// （`wss://videoim.cmcc-cs.cn:20102/ws/<serverId>/<sessionId>/websocket`）。
#[test]
fn url_shape_matches_reference() {
    let url = live_ws_url(BASE_VIDEO_IM_WS, 999, "abcdefghijklm");
    assert_eq!(
        url,
        "wss://videoim.cmcc-cs.cn:20102/ws/999/abcdefghijklm/websocket"
    );
    // 生产基址就是 IM 的 WS 端口 20102（**不是** HTTPS 的 20103）。
    assert!(url.contains(":20102"));
    assert!(url.ends_with("/websocket"));
}

/// sessionId 是 13 位 36 进制字符，且**可复现**（同一随机序列得到同一串）。
///
/// 可复现是这条链能被 fixture 回放的前提；而"服务端只把它当路径里的一段、
/// 不参与鉴权"是它可以用非安全随机源的理由。
#[test]
fn session_id_is_13_lowercase_base36_and_reproducible() {
    let a = sockjs_session_id(&mut SeqRandom::new(1));
    let b = sockjs_session_id(&mut SeqRandom::new(1));
    assert_eq!(a, b, "同一随机序列必须得到同一个 sessionId（可复现）");
    assert_eq!(a.len(), 13, "sessionId 必须是 13 位");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
        "sessionId 只能是 0-9a-z，实际 {a}"
    );

    let c = sockjs_session_id(&mut SeqRandom::new(2));
    assert_ne!(a, c, "不同随机序列应当得到不同 sessionId");
}

/// 生产端点：地址与两个头都来自常量表（不是本地字面量）。
#[test]
fn production_endpoint_uses_reference_constants() {
    let e = LiveWsEndpoint::production();
    assert_eq!(e.base, BASE_VIDEO_IM_WS);
    assert!(e.base.starts_with("wss://"), "生产必须走 wss");
    assert_eq!(e.user_agent, LIVE_USER_AGENT);
    assert_eq!(e.origin, LIVE_ORIGIN);
    // UA 是 iPad App 内嵌 WebView 的那个值（2026-09-23 实拍）。
    assert!(
        e.user_agent.contains("iPad"),
        "WS 升级请求的 UA 应当是 iPad 那个实拍值"
    );
}

/// `WsEnd` 的两组分类判据不能张冠李戴。
#[test]
fn ws_end_classification_helpers() {
    assert!(WsEnd::ListenedEnough.is_normal());
    assert!(!WsEnd::ListenedEnough.is_broken());

    for end in [WsEnd::Disconnected, WsEnd::PeerGone] {
        assert!(end.is_broken(), "{end:?} 应当属于断线类");
        assert!(!end.is_normal(), "{end:?} 不该被当成正常结束");
    }

    // ServerClosed 是"对面主动收工"，既不是我们收满、也不是网络断。
    assert!(!WsEnd::ServerClosed.is_normal());
    assert!(!WsEnd::ServerClosed.is_broken());
    assert!(!WsEnd::Aborted.is_normal());
    assert!(!WsEnd::Aborted.is_broken());
}

/// `entered_room` 需要 `o` **与** PROVISION 响应**同时**成立。
///
/// 只看其中一个会把"连上了 socket 但没进成房间"误判成"进过房间"，
/// 那会让 `Disconnected` / `PeerGone` 的分档直接失效。
#[test]
fn entered_room_requires_both_open_and_provision_ack() {
    let mut r = ListenReport::empty(WsEnd::PeerGone);
    assert!(!r.entered_room(), "什么都没有时不该算进过房间");

    r.heard_open = true;
    assert!(!r.entered_room(), "只收到 `o` 不算进过房间");

    r.heard_open = false;
    r.heard_provision_ack = true;
    assert!(!r.entered_room(), "只收到 PROVISION 响应不算进过房间");

    r.heard_open = true;
    assert!(r.entered_room(), "两条都收到才算进过房间");
}

/// ⚠⚠ 复审 `up-2`：对面**收下 TCP 但永不回握手**时，必须**按超时**报错 ——
/// 而不是永远挂着（旧写法 `connect_async` 无超时，`listen_for` 的 deadline 又在
/// 连上之后才起算 ⇒ 任务永远停在"正在听"，只有重启服务才结束）。
///
/// 红测：把 `connect_live_ws_within` 里那句 `tokio::time::timeout(handshake, ..)`
/// 拆掉 ⇒ 这条用例会一直挂到下面那句**兜底** `timeout`，红。
#[tokio::test]
async fn a_silent_peer_times_out_instead_of_hanging_forever() {
    // 只 accept、绝不回 101 的"黑洞"服务端
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口失败");
    let addr = listener.local_addr().expect("取本地地址失败");
    tokio::spawn(async move {
        let _conn = listener.accept().await;
        // 收下连接就装死
        std::future::pending::<()>().await;
    });

    let endpoint = LiveWsEndpoint {
        base: format!("ws://{addr}"),
        ..LiveWsEndpoint::production()
    };
    let url = live_ws_url(&endpoint.base, 1, "abcdefghijklm");
    let started = std::time::Instant::now();

    let err = tokio::time::timeout(
        Duration::from_secs(10), // 兜底：真挂住时别让整条测试卡死
        connect_live_ws_within(&url, &endpoint, Duration::from_millis(300)),
    )
    .await
    .expect("黑洞握手必须**按超时**返回，而不是永远挂着")
    .expect_err("必须报错，不能交回一个连接");

    let msg = err.to_string();
    assert!(msg.contains("超时"), "要说清是超时：{msg}");
    assert!(
        msg.contains("0.3"),
        "要把实际超时值说出来（注入的 300ms）：{msg}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "不能等满外层兜底：实际 {:?}",
        started.elapsed()
    );
}
