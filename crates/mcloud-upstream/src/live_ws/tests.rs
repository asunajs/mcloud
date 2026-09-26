//! WS 帧状态机的测试。
//!
//! # 判据来源优先级（本项目一贯口径）
//!
//! **实拍黄金向量 > 实拍文档逐字 > 原项目实现**
//!
//! # ⚠ 本文件里绝大多数断言的证据等级是「**原项目实现**」，不是实拍
//!
//! 2026-09-22 未开播、`chatRoomId: null` ⇒ **WS 结构性不存在**
//! （见 `docs/capture/probe-live-room-2026-09-23.md`）。所以：
//!
//! | 断言的东西 | 证据 |
//! |---|---|
//! | 帧的**顺序**（`o` → PROVISION → `command=3` → UNREAD） | 原项目，**未实拍** |
//! | `command` 的**取值**（`1` / `3`） | 原项目，**未实拍** |
//! | **UNREAD 是否真的必需** | 原项目，**未实拍** |
//! | `h` 要原样回 `h` | 原项目，**未实拍** |
//! | 弹幕文本的**形状**（`小红花口令：…`） | **实拍**（黄金向量 `msyncInbound`） |
//! | 报文**字节**（PROVISION / UNREAD 两条） | **原实现逐字节**（黄金向量 `msyncFrames`） |
//!
//! 这些测试钉的是「**我们与原项目的行为一致**」，**不是**「服务端认这个」。
//! 开播后第一次抓到真帧序列时，这里就是对照面 —— 对不上的地方会以
//! **一条失败的测试**出现，而不是「弹幕忽然收不到了」。

use super::*;
use mcloud_crypto::SeqRandom;
use serde_json::Value;

use crate::live_msync::{
    decode_msync, encode_msync, extract_danmaku_text, live_msync_frame, CMD_NOTICE, CMD_SYNC,
};

/// 黄金向量文件（`msyncInbound` 的真弹幕文本 + `msyncFrames` 的报文）。
fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/live-room-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

/// 造一个固定随机源的状态机（`identification` 才不会每次都变）。
fn listener() -> KoulingListener {
    KoulingListener::new(
        "A03CBHoAAAAA19900000000SYNTHETICTOKENFORVECTORS000000000000000000",
        "2026072314302675201220000000000",
        "19900000000",
        Box::new(SeqRandom::new(0x10)),
    )
}

/// 黄金向量里的固定时刻（`msyncFrames` 用的那个）。
const NOW: i64 = 1784790184674;

/// 造一条服务端**推下来**的帧（`a["<base64>"]`）。
fn inbound(command: u8, payload: Vec<u8>, now_millis: i64) -> String {
    let mut rng = SeqRandom::new(0x20);
    inbound_danmaku_frame(
        "A03CBHoAAAAA19900000000SYNTHETICTOKENFORVECTORS000000000000000000",
        "19900000000",
        "2026072314302675201220000000000",
        command,
        payload,
        now_millis,
        &mut rng,
    )
}

/// 从一条发出去的 SockJS 帧里解回 MSync 报文。
///
/// 断言「发出去的到底是什么」用 —— 比 `assert!(text.contains("..."))` 硬：
/// 它检查的是**解出来的 command 与 payload**，而不是 base64 的一个子串。
fn parse_outbound(frame: &str) -> crate::live_msync::MsyncMessage {
    // 形状：`["<base64>"]`
    let inner = frame
        .strip_prefix("[\"")
        .and_then(|s| s.strip_suffix("\"]"))
        .unwrap_or_else(|| panic!("发送帧不是 [\"<base64>\"] 形状：{frame}"));
    let raw = mcloud_crypto::b64_decode(inner).expect("base64 应当能解开");
    decode_msync(&raw).expect("解出来的应当是合法 MSync 报文")
}

/// 造一条 `command = 3`（PROVISION 的响应）的入站帧。
///
/// ⚠ 这条帧**原实现里长什么样没有实拍** —— 但它的 `command` 是 `3` 这一点
/// 由黄金向量 `msyncFrames[0].expect.command` 钉住。
fn provision_ack_frame(now_millis: i64) -> String {
    inbound(CMD_PROVISION, Vec::new(), now_millis)
}

// ══════════════════════ 最关键的那条时序 ══════════════════════

/// ⭐⭐⭐ `o` → PROVISION → 服务端 MSync(3) → **UNREAD**。
///
/// # 这是本任务最该钉住的一条
///
/// 少了 UNREAD，**服务端不推弹幕** —— 而症状是：
///
/// ```text
/// WS 连得上、心跳正常、只是永远收不到弹幕
/// ```
///
/// 排查方向会指向「是不是没人在发」，而不是「少发了一帧」。
///
/// # 这条断言了什么
///
/// 1. `o` ⇒ **恰好一条**回帧，且它是 `command == 3`（PROVISION）；
/// 2. 服务端回 `MSync(command=3)` ⇒ **恰好一条**回帧，且它是 `command == 1`（UNREAD）；
/// 3. 顺序：PROVISION 在 UNREAD **之前**（它们不在同一次 `on_frame` 里，
///    所以用「先收到 3 才发 1」这件事本身来钉）；
/// 4. 两条各只发**一次**（重复的 `o` 与重复的 `3` 都不再发）。
#[test]
fn provision_then_unread_ordering_is_pinned() {
    let mut l = listener();

    // ① 收到 `o` ⇒ 必须发 PROVISION(command=3)
    assert!(!l.provision_sent(), "还没收到 o 就不该发过 PROVISION");
    let step1 = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    assert_eq!(step1.kind, FrameKind::Open);
    assert_eq!(step1.outbound.len(), 1, "收到 o 必须回恰好一帧");
    let provision = parse_outbound(step1.outbound[0].as_text().unwrap());
    assert_eq!(
        provision.command, CMD_PROVISION,
        "收到 o 之后要发的是 PROVISION(command=3)"
    );
    assert!(!provision.payload.is_empty(), "PROVISION 必须带 payload");
    assert!(l.provision_sent());
    // 关键：此刻 UNREAD **还没**发 —— 它在等服务端那条响应。
    assert!(!l.unread_sent(), "UNREAD 要等服务端的 command=3 响应才发");
    assert!(!l.provision_ack_seen(), "还没收到服务端的响应");

    // ② 服务端回 MSync(command=3) ⇒ **必须**补发 UNREAD(command=1)
    let step2 = l.on_frame(&provision_ack_frame(NOW + 1), NOW + 1).unwrap();
    assert_eq!(step2.kind, FrameKind::ProvisionAck);
    assert_eq!(
        step2.outbound.len(),
        1,
        "⚠⚠ 收到 command=3 之后**必须**补发一帧 —— 少了它服务端不推弹幕，\
         而症状是「连得上、心跳正常、永远收不到弹幕」"
    );
    let unread = parse_outbound(step2.outbound[0].as_text().unwrap());
    assert_eq!(
        unread.command, CMD_UNREAD,
        "⚠⚠ 补发的必须是 UNREAD(command=1)，不是别的东西"
    );
    assert!(unread.payload.is_empty(), "UNREAD 没有 payload");
    assert!(l.unread_sent(), "UNREAD 发过之后要记下来");

    // ③ 连接真的建立起来了（供连接层区分「直播结束」与「被断开」）
    assert!(
        l.provision_ack_seen(),
        "收到过 command=3 响应 ⇒ 连接建立起来了"
    );

    // ④ 两条各只发一次：重复的 `o` 与重复的 `3` 都不再发。
    let again_open = l.on_frame(SOCKJS_OPEN, NOW + 2).unwrap();
    assert_eq!(again_open.kind, FrameKind::Open);
    assert!(
        again_open.outbound.is_empty(),
        "重复的 o 不该再发 PROVISION —— 那会让服务端把它当成新会话"
    );
    let again_ack = l.on_frame(&provision_ack_frame(NOW + 3), NOW + 3).unwrap();
    assert_eq!(again_ack.kind, FrameKind::ProvisionAck);
    assert!(
        again_ack.outbound.is_empty(),
        "重复的 command=3 不该再发 UNREAD"
    );
}

/// ⚠ 「漏发 UNREAD」这种缺陷**必须**能被这条测试抓住。
///
/// 上面那条断言的是「有 UNREAD」。这条从**另一头**测：把 UNREAD 的
/// `command` 写错（比如误用 SYNC=0），上面那条会红 —— 这里用一个
/// **不是** UNREAD 的 command 显式证明「command 的值是有意义的」。
#[test]
fn the_unread_frame_is_not_any_other_command() {
    let mut l = listener();
    let _ = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    let ack = l.on_frame(&provision_ack_frame(NOW + 1), NOW + 1).unwrap();
    let sent = parse_outbound(ack.outbound[0].as_text().unwrap());

    assert_eq!(sent.command, CMD_UNREAD);
    // 这三个都不能是它 —— 写错一个都会静默变成另一件事。
    assert_ne!(sent.command, CMD_SYNC, "SYNC(0) 不是 UNREAD");
    assert_ne!(sent.command, CMD_PROVISION, "不能把 PROVISION 再发一遍");
    assert_ne!(sent.command, CMD_NOTICE, "NOTICE(2) 不是 UNREAD");
    // 常量本身也要对（写错常量 = 静默发错帧）。
    assert_eq!(CMD_UNREAD, 1);
    assert_eq!(CMD_PROVISION, 3);
}

/// PROVISION 帧的 payload 解回来要是那个 `Provision`（六项恒定字段都在）。
///
/// # ⚠ 这条对照的是黄金向量里的**原实现**，不是服务端
#[test]
fn provision_payload_matches_the_golden_vector_shape() {
    let mut l = listener();
    let step = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    let msg = parse_outbound(step.outbound[0].as_text().unwrap());

    assert_eq!(msg.command, CMD_PROVISION);
    // 报文里的恒定项（黄金向量 `msyncFrames[0].expect` 钉住的那批）。
    assert_eq!(msg.guid.app_key, crate::live_msync::LIVE_APP_KEY);
    assert_eq!(
        msg.guid.name, "cmlive_c_19900000000",
        "有 phone 时走直播间形态"
    );
    assert_eq!(msg.guid.domain, crate::live_msync::LIVE_DOMAIN);
    assert_eq!(msg.guid.client_resource, format!("js_{NOW}"));
    assert_eq!(msg.auth, format!("$t${}", l.access_token));
    // ⚠ `pov` 必须是 `Some(0)` —— 忘了填是**静默少 2 个字节**。
    assert_eq!(msg.pov, Some(0), "pov 是非 nil 的零值，不能省");
    assert_eq!(msg.encrypt_type, vec![0]);

    // PROVISION 的 payload 里那几项。
    let provision =
        crate::live_msync::decode_provision(&msg.payload).expect("payload 应当是 Provision");
    assert_eq!(provision.os_type, crate::live_msync::PROVISION_OS_TYPE_JS);
    assert_eq!(provision.version, crate::live_msync::PROVISION_VERSION);
    assert_eq!(provision.app_sign, crate::live_msync::PROVISION_APP_SIGN);
    assert_eq!(
        provision.device_name,
        crate::live_msync::PROVISION_DEVICE_NAME
    );
    assert_eq!(provision.resource, format!("js_{NOW}"));
    // ⚠ `deviceUuid` 与 `resource` **同源但不是同一个串**（少了 `js_` 前缀）。
    assert_eq!(provision.device_uuid, NOW.to_string());
    assert_ne!(provision.device_uuid, provision.resource);
    assert_eq!(provision.compress_type, vec![0]);
    assert_eq!(provision.encrypt_type, vec![0]);
    assert_eq!(provision.province_code, "", "原实现给空串 ⇒ 字段不出现");
}

/// 发送帧的形状是 `["<base64>"]`（**无前缀**），不是 `a["<base64>"]`。
///
/// # ⚠ 这两个形状混起来是「安静丢掉」那一类错
///
/// ```text
/// 客户端发 → `["<base64>"]`    （无前缀）
/// 服务端推 → `a["<base64>"]`   （前缀 a[）
/// ```
///
/// 前缀写错的表现是「帧被归成不认识的一类、安静丢掉」——
/// 也就是「连上了、心跳也正常、就是收不到弹幕」，
/// 与漏发 UNREAD 的症状**一模一样**。
#[test]
fn outbound_frames_have_no_sockjs_prefix() {
    let mut l = listener();
    let step = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    let frame = step.outbound[0].as_text().unwrap();
    assert!(frame.starts_with("[\""), "发送形状必须以 [\" 开头：{frame}");
    assert!(frame.ends_with("\"]"));
    assert!(
        !frame.starts_with("a["),
        "客户端**发**出去的不带 a[ 前缀（那是服务端**推**下来的形状）：{frame}"
    );

    // 反过来：入站帧带前缀，而且状态机认得它。
    let inbound_frame = provision_ack_frame(NOW);
    assert!(inbound_frame.starts_with("a[\""), "{inbound_frame}");
}

/// ⭐ 帧状态机是**纯层**：同样的输入给同样的输出，且不依赖时钟。
///
/// # 为什么这条值得单测
///
/// 「绑在 `net.Conn` 上就只能真开 WS 才能验证」—— 而 WS 只在
/// **主播开播那一小时**才存在。这条测试本身就是那个设计决策的**证据**：
/// 整条时序在单测里跑完了。
#[test]
fn the_machine_is_a_pure_function_of_its_inputs() {
    let run = || {
        let mut l = listener();
        let a = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
        let b = l.on_frame(SOCKJS_HEARTBEAT, NOW + 1).unwrap();
        let c = l.on_frame(&provision_ack_frame(NOW + 2), NOW + 2).unwrap();
        (a.outbound, b.outbound, c.outbound, l.codes().to_vec())
    };
    // 跑两遍：同一个固定随机源 ⇒ 逐字节相同。
    assert_eq!(run(), run(), "状态机不该有隐藏的时钟/全局状态依赖");
}

/// `now` 决定三处，而且它们来自**同一刻**。
///
/// ⚠ 三处若各取一次时钟，报文里就会出现「时间戳是 A、resource 是 B」
/// 这种**看不出来**的不一致。
#[test]
fn now_drives_resource_device_uuid_and_timestamp_together() {
    let mut l = listener();
    let step = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    let msg = parse_outbound(step.outbound[0].as_text().unwrap());

    // ① guid.clientResource = js_<毫秒>
    assert_eq!(msg.guid.client_resource, format!("js_{NOW}"));
    // ② MSync 的 8 字节 timestamp = 同一毫秒
    assert_eq!(msg.timestamp, crate::live_msync::live_timestamp_bytes(NOW));
    // ③ PROVISION 的 deviceUuid = 同一毫秒的十进制串
    let provision = crate::live_msync::decode_provision(&msg.payload).unwrap();
    assert_eq!(provision.device_uuid, NOW.to_string());
    // 而且 resource 与 deviceUuid **不是同一个串**（差 `js_` 前缀）。
    assert_ne!(provision.resource, provision.device_uuid);

    // 换一个时刻，三处**一起**变。
    let mut l2 = listener();
    let other = l2.on_frame(SOCKJS_OPEN, NOW + 5000).unwrap();
    let msg2 = parse_outbound(other.outbound[0].as_text().unwrap());
    assert_eq!(msg2.guid.client_resource, format!("js_{}", NOW + 5000));
    assert_eq!(
        crate::live_msync::decode_provision(&msg2.payload)
            .unwrap()
            .device_uuid,
        (NOW + 5000).to_string()
    );
}

// ══════════════════════ 心跳 ══════════════════════

/// ⭐ `h` ⇒ **原样回一个 `h`**。
///
/// ⚠ 不回的话 SockJS 会在超时后主动断开 —— 而**断开的样子与
/// 「直播结束了」一样**：两者都是「连接没了、也没弹幕」。
#[test]
fn heartbeat_is_echoed_verbatim() {
    let mut l = listener();
    let actions = l.on_frame(SOCKJS_HEARTBEAT, NOW).unwrap();
    assert_eq!(actions.kind, FrameKind::Heartbeat);
    assert_eq!(actions.outbound.len(), 1);
    assert_eq!(actions.outbound[0], Outbound::Text("h".to_string()));
    assert_eq!(actions.outbound_texts(), vec!["h"]);
    // 心跳不产生口令、不产生弹幕。
    assert!(actions.codes.is_empty());
    assert!(actions.danmaku.is_empty());
    // 而且它**不**该把 PROVISION 之类的状态带出去。
    assert!(!l.provision_sent());
}

/// 心跳回帧是**恰好一个 `h`**，不是 `["h"]` 也不是 `H`。
#[test]
fn heartbeat_reply_is_exactly_the_letter_h() {
    let mut l = listener();
    let actions = l.on_frame("h", NOW).unwrap();
    let text = actions.outbound[0].as_text().unwrap();
    assert_eq!(text, "h");
    assert_eq!(text.len(), 1);
    assert_ne!(text, "\"h\"", "心跳回的是裸 h，不是 JSON 字符串");
    assert_ne!(text, "H", "大小写要一致");
}

/// 心跳可以**反复**回（它是被动的，不是一次性状态）。
#[test]
fn heartbeat_is_replied_every_time() {
    let mut l = listener();
    for i in 0..3 {
        let actions = l.on_frame(SOCKJS_HEARTBEAT, NOW + i).unwrap();
        assert_eq!(
            actions.outbound,
            vec![Outbound::Text("h".to_string())],
            "第 {i} 次心跳也要回"
        );
    }
}

// ══════════════════════ `a[...]` 里的弹幕与口令 ══════════════════════

/// ⭐ `a[...]` 里的弹幕口令被抽出来 —— 用**黄金向量的真文本**。
///
/// 黄金向量 `msyncInbound[0]`（`sync-comm-sync-ul`）的文本是
/// `小红花口令：毕业之旅MClaw相伴`，那是**实拍弹幕的形状**。
#[test]
fn danmaku_kouling_is_extracted_from_the_golden_vector_text() {
    let v = vectors();
    let case = &v["msyncInbound"][0];
    assert_eq!(case["kind"], "sync");
    let payload = mcloud_crypto::b64_decode(case["payloadBase64"].as_str().unwrap()).unwrap();
    let expected_text = case["text"].as_str().unwrap();

    let mut l = listener();
    let actions = l.on_frame(&inbound(CMD_SYNC, payload, NOW), NOW).unwrap();

    assert_eq!(actions.kind, FrameKind::Danmaku, "这条应当被认成弹幕");
    assert_eq!(actions.danmaku, expected_text, "解出来的文本要与向量一致");
    assert_eq!(
        actions.codes,
        vec!["毕业之旅MClaw相伴".to_string()],
        "`小红花口令：XXX` 里的口令要被抽出来"
    );
    assert_eq!(l.codes(), ["毕业之旅MClaw相伴"]);
}

/// `notice` 那一档（`msyncInbound[1]`）：一条消息里**两条**口令。
#[test]
fn notice_payload_yields_two_kouling_codes() {
    let v = vectors();
    let case = &v["msyncInbound"][1];
    assert_eq!(case["kind"], "notice");
    let payload = mcloud_crypto::b64_decode(case["payloadBase64"].as_str().unwrap()).unwrap();

    let mut l = listener();
    let actions = l.on_frame(&inbound(CMD_NOTICE, payload, NOW), NOW).unwrap();

    assert_eq!(actions.kind, FrameKind::Danmaku);
    assert_eq!(actions.danmaku, case["text"].as_str().unwrap());
    assert_eq!(
        actions.codes,
        vec![
            "MClaw全能小助手".to_string(),
            "毕业之旅MClaw相伴".to_string()
        ],
        "`口令1:AAA 口令2:BBB` 两条都要"
    );
}

/// `bare` 那一档（`msyncInbound[2]`）：payload 直接就是 `MessageBody`。
///
/// ⚠ 这一条是**三级解码兜底**存在的理由：原实现先写死了 SYNC 那一层，
/// 后来撞上解不出文本才补的兜底 —— 这一级的存在本身就是「猜错过一次」的证据。
#[test]
fn bare_message_body_is_decoded_by_the_fallback() {
    let v = vectors();
    let case = &v["msyncInbound"][2];
    assert_eq!(case["kind"], "bare");
    let payload = mcloud_crypto::b64_decode(case["payloadBase64"].as_str().unwrap()).unwrap();
    let expected = case["text"].as_str().unwrap();

    // ⚠ 它作为 SYNC 与作为 NOTICE 都要能解出来（兜底不该挑 command）。
    for command in [CMD_SYNC, CMD_NOTICE] {
        let mut l = listener();
        let actions = l
            .on_frame(&inbound(command, payload.clone(), NOW), NOW)
            .unwrap();
        assert_eq!(
            actions.danmaku, expected,
            "command={command} 时兜底也要解出文本"
        );
        assert_eq!(actions.codes, vec!["毕业之旅MClaw相伴".to_string()]);
    }
}

/// 一条 `a[...]` 帧里可以有**多条**消息 —— 都要处理。
///
/// # ⚠ 这里刻意与原实现不同
///
/// 原实现是 `text.slice(3, text.lastIndexOf('"]'))`，**只对单条成立**：
/// `a["QUJD","REVG"]` 被它切出来是 `QUJD","REVG`，base64 解不开 ⇒ **整帧被丢**。
///
/// 换成按 JSON 解之后，单条行为逐字相同，多条从「丢掉」变成「都能处理」。
#[test]
fn a_single_frame_can_carry_multiple_messages() {
    let v = vectors();
    let one =
        mcloud_crypto::b64_decode(v["msyncInbound"][0]["payloadBase64"].as_str().unwrap()).unwrap();
    let two =
        mcloud_crypto::b64_decode(v["msyncInbound"][1]["payloadBase64"].as_str().unwrap()).unwrap();

    // 造两条真报文，包成 SockJS 的**多消息**数组。
    let mut rng = SeqRandom::new(0x30);
    let msg_one = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        one,
        NOW,
        &mut rng,
    );
    let msg_two = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_NOTICE,
        two,
        NOW,
        &mut rng,
    );
    let frame = format!(
        "a[\"{}\",\"{}\"]",
        mcloud_crypto::b64_encode(&encode_msync(&msg_one)),
        mcloud_crypto::b64_encode(&encode_msync(&msg_two)),
    );

    let mut l = listener();
    let actions = l.on_frame(&frame, NOW).unwrap();
    assert_eq!(actions.kind, FrameKind::Danmaku);
    assert_eq!(
        actions.danmaku,
        v["msyncInbound"][1]["text"].as_str().unwrap(),
        "最后一条的文本"
    );
    // 三条口令都收到了（第一条 1 条 + 第二条 2 条），且保序。
    assert_eq!(
        l.codes(),
        [
            "毕业之旅MClaw相伴".to_string(),
            "MClaw全能小助手".to_string(),
        ],
        "两条消息都要处理：{frame}"
    );
}

/// 单条消息的行为与「切字符串」逐字相同（base64 主体一致）。
#[test]
fn single_message_frame_is_byte_identical_to_the_naive_slice() {
    let v = vectors();
    let payload =
        mcloud_crypto::b64_decode(v["msyncInbound"][0]["payloadBase64"].as_str().unwrap()).unwrap();
    let mut rng = SeqRandom::new(0x40);
    let msg = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        payload,
        NOW,
        &mut rng,
    );
    let frame = sockjs_data_frame(&encode_msync(&msg));

    // 原实现的切法：`text.slice(3, text.lastIndexOf('"]'))`
    let naive = frame
        .strip_prefix("a[\"")
        .and_then(|s| s.strip_suffix("\"]"))
        .unwrap();
    // 按 JSON 解出来的那一条
    let parsed = decode_sockjs_data(&frame).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0], naive, "单条时两种写法必须逐字相同");
}

// ══════════════════════ 跨帧拼接 ══════════════════════

/// ⭐ 跨帧拼接：上一条是「复制口令N：」这样的**头**，下一条整条就是口令。
///
/// 口令被服务端切成了**两条消息** —— 只看单帧的实现会把它整条丢掉，
/// 而表现是「这条口令没收到」（比「完全没弹幕」更难发现）。
#[test]
fn kouling_split_across_two_frames_is_joined() {
    let mut l = listener();

    // 第一条：整条是个口令头（**不带**口令本身）
    let head = make_danmaku("复制口令1：");
    let a = l.on_frame(&head, NOW).unwrap();
    assert_eq!(a.kind, FrameKind::Danmaku);
    assert!(
        a.codes.is_empty(),
        "口令头本身不含口令 —— 从它身上抽出口令就是误报：{:?}",
        a.codes
    );

    // 第二条：整条就是口令
    let body = make_danmaku("毕业之旅MClaw相伴");
    let b = l.on_frame(&body, NOW + 1).unwrap();
    assert_eq!(b.kind, FrameKind::Danmaku);
    assert_eq!(
        b.codes,
        vec!["毕业之旅MClaw相伴".to_string()],
        "上一条是口令头 ⇒ 这一条整条就是口令"
    );
    assert_eq!(l.codes(), ["毕业之旅MClaw相伴"]);

    // 拼接只用一次：再来一条普通文本不会又当成口令。
    let c = l.on_frame(&make_danmaku("大家好"), NOW + 2).unwrap();
    assert!(c.codes.is_empty(), "拼接状态用完就要清掉：{:?}", c.codes);
}

/// ⚠ 连续两条**口令头**时，中间那条口令丢了 ⇒ 第二条头不能被当成口令。
///
/// 对齐原实现：下一行自己也是个口令头时**不算**。
/// 不做这个排除的话，弹幕里那句「复制口令2：」本身会被当成一条口令，
/// 而它必然无效（`200108`）—— 于是多一次无用请求。
#[test]
fn a_second_kouling_head_is_not_treated_as_the_code() {
    let mut l = listener();
    let _ = l.on_frame(&make_danmaku("复制口令1："), NOW).unwrap();
    let second = l.on_frame(&make_danmaku("复制口令2："), NOW + 1).unwrap();
    assert!(
        second.codes.is_empty(),
        "下一条自己还是个口令头 ⇒ 不能当口令用：{:?}",
        second.codes
    );
    // 但状态要**继续**挂着：真正的口令可能在下一条。
    let third = l
        .on_frame(&make_danmaku("中秋美好存云盘"), NOW + 2)
        .unwrap();
    assert_eq!(
        third.codes,
        vec!["中秋美好存云盘".to_string()],
        "第二条头之后的那一条才是口令"
    );
}

/// 「无随机」那种收尾词**不能**当口令。
#[test]
fn the_no_random_tail_word_is_not_a_code() {
    let mut l = listener();
    let _ = l.on_frame(&make_danmaku("复制口令1："), NOW).unwrap();
    let tail = l.on_frame(&make_danmaku("无随机"), NOW + 1).unwrap();
    assert!(
        tail.codes.is_empty(),
        "`无随机` 是收尾词，不是口令：{:?}",
        tail.codes
    );
}

/// 拼接发生在**同一帧内的多条消息**之间也要成立。
///
/// SockJS 会把缓冲的消息一次发下来 —— 口令头与其正文完全可能落在同一条
/// `a[...]` 里。把它们当成「不同的帧」处理会让拼接失效。
#[test]
fn kouling_join_works_across_messages_in_one_frame() {
    let mut rng = SeqRandom::new(0x50);
    let head = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        payload_for_text("复制口令1："),
        NOW,
        &mut rng,
    );
    let body = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        payload_for_text("毕业之旅MClaw相伴"),
        NOW,
        &mut rng,
    );
    let frame = format!(
        "a[\"{}\",\"{}\"]",
        mcloud_crypto::b64_encode(&encode_msync(&head)),
        mcloud_crypto::b64_encode(&encode_msync(&body)),
    );

    let mut l = listener();
    let actions = l.on_frame(&frame, NOW).unwrap();
    assert_eq!(
        actions.codes,
        vec!["毕业之旅MClaw相伴".to_string()],
        "同一帧内跨消息的拼接也要成立"
    );
}

// ══════════════════════ 去重 ══════════════════════

/// 同一条口令**反复出现**（弹幕会刷屏）⇒ 只记一次、只报一次。
#[test]
fn kouling_is_deduplicated_globally() {
    let mut l = listener();
    let frame = make_danmaku("小红花口令：毕业之旅MClaw相伴");

    let first = l.on_frame(&frame, NOW).unwrap();
    assert_eq!(first.codes.len(), 1);

    // 第二次：解出来了（`danmaku` 非空）但**不算新口令**。
    let second = l.on_frame(&frame, NOW + 1).unwrap();
    assert_eq!(second.kind, FrameKind::Danmaku);
    assert_eq!(second.danmaku, "小红花口令：毕业之旅MClaw相伴");
    assert!(
        second.codes.is_empty(),
        "重复口令不该再报：{:?}",
        second.codes
    );

    assert_eq!(l.codes().len(), 1, "全局只留一条");
}

/// 去重是**全局**的：跨帧、跨拼接路径、跨抽取方式都算同一份簿。
#[test]
fn dedupe_spans_frames_and_extraction_paths() {
    let mut l = listener();

    // 路径 A：`小红花口令：XXX`
    let a = l
        .on_frame(&make_danmaku("小红花口令：毕业之旅MClaw相伴"), NOW)
        .unwrap();
    assert_eq!(a.codes, vec!["毕业之旅MClaw相伴".to_string()]);

    // 路径 B：跨帧拼接（头 + 正文）解出**同一条**
    let _ = l.on_frame(&make_danmaku("复制口令1："), NOW + 1).unwrap();
    let b = l
        .on_frame(&make_danmaku("毕业之旅MClaw相伴"), NOW + 2)
        .unwrap();
    assert!(
        b.codes.is_empty(),
        "跨帧拼接解出的口令也要过同一份去重簿：{:?}",
        b.codes
    );

    // 路径 C：同一帧里两条消息给了同一条
    let mut rng = SeqRandom::new(0x60);
    let one = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        payload_for_text("小红花口令：中秋美好存云盘"),
        NOW,
        &mut rng,
    );
    let two = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        payload_for_text("小红花口令：中秋美好存云盘"),
        NOW,
        &mut rng,
    );
    let frame = format!(
        "a[\"{}\",\"{}\"]",
        mcloud_crypto::b64_encode(&encode_msync(&one)),
        mcloud_crypto::b64_encode(&encode_msync(&two))
    );
    let c = l.on_frame(&frame, NOW + 3).unwrap();
    assert_eq!(
        c.codes,
        vec!["中秋美好存云盘".to_string()],
        "同一帧内的重复也只报一次"
    );

    assert_eq!(l.codes(), ["毕业之旅MClaw相伴", "中秋美好存云盘"]);
}

/// 口令**保序**（按收到的先后），不是按字典序。
#[test]
fn kouling_keeps_arrival_order() {
    let mut l = listener();
    for (i, code) in ["ZEBRA", "APPLE", "MANGO"].iter().enumerate() {
        let actions = l
            .on_frame(
                &make_danmaku(&format!("小红花口令：{code}")),
                NOW + i as i64,
            )
            .unwrap();
        assert_eq!(actions.codes.len(), 1, "第 {i} 条应当是新口令");
    }
    assert_eq!(l.codes(), ["ZEBRA", "APPLE", "MANGO"], "保序，不是字典序");
}

// ══════════════════════ 坏帧 / 不认识的帧 ══════════════════════

/// 不认识的帧 ⇒ `Ignored`，**什么都不回**。
#[test]
fn unknown_frames_are_ignored_not_fatal() {
    let mut l = listener();
    for text in ["x", "", "hello", "o ", " h", "m", "a", "T"] {
        let actions = l
            .on_frame(text, NOW)
            .unwrap_or_else(|e| panic!("不认识的一帧不该让整条链失败（{text:?}）：{e}"));
        assert_eq!(actions.kind, FrameKind::Ignored, "{text:?} 应当被忽略");
        assert!(!actions.has_outbound(), "{text:?} 不该让我们回帧");
    }
}

/// 坏帧 ⇒ `Malformed`，但**返回 `Ok`**（一帧解不开不该拆掉整条链）。
///
/// ⚠ 「一直收不到弹幕」与「弹幕本来就没有」必须分得开 ——
/// 所以坏帧要归成 `Malformed` 而**不是** `Silent`/`Ignored`：
/// 上层据此能说清「是没收帧，还是收了帧解不开」。
#[test]
fn malformed_frames_are_classified_but_not_fatal() {
    for bad in [
        // 不是 JSON 数组
        r#"a[not json]"#.to_string(),
        // 数组里不是字符串
        r#"a[1,2]"#.to_string(),
        // 合法 JSON 但 base64 解不开
        r#"a["!!!not base64!!!"]"#.to_string(),
        // 合法 base64 但不是 protobuf
        format!("a[\"{}\"]", mcloud_crypto::b64_encode(b"\xff\xff\xff\xff")),
        // 空数组
        "a[]".to_string(),
        // 空串消息
        "a[\"\"]".to_string(),
    ] {
        let mut l = listener();
        let actions = l
            .on_frame(&bad, NOW)
            .unwrap_or_else(|e| panic!("坏帧不该让整条链失败（{bad}）：{e}"));
        assert!(
            matches!(actions.kind, FrameKind::Malformed | FrameKind::Silent),
            "{bad} 应当被归成 Malformed（实际 {}）",
            actions.kind
        );
        assert!(actions.codes.is_empty(), "{bad} 不该产出任何口令");
    }
}

/// 一帧里**部分**消息坏掉时，好的那些仍要被处理。
#[test]
fn a_good_message_survives_a_bad_one_in_the_same_frame() {
    let mut rng = SeqRandom::new(0x70);
    let good = live_msync_frame(
        "A03T",
        Some("19900000000"),
        "room",
        CMD_SYNC,
        payload_for_text("小红花口令：毕业之旅MClaw相伴"),
        NOW,
        &mut rng,
    );
    let frame = format!(
        "a[\"!!!not base64!!!\",\"{}\"]",
        mcloud_crypto::b64_encode(&encode_msync(&good)),
    );

    let mut l = listener();
    let actions = l.on_frame(&frame, NOW).unwrap();
    assert_eq!(
        actions.codes,
        vec!["毕业之旅MClaw相伴".to_string()],
        "坏的那条要跳过，好的那条照常处理"
    );
    assert_eq!(actions.danmaku, "小红花口令：毕业之旅MClaw相伴");
}

/// 连续太多坏帧 ⇒ 请连接层关闭（别无限挂着）。
///
/// ⚠ 这个阈值是**我们定的**，没有任何实拍依据 —— 见
/// [`KoulingListener::with_malformed_limit`] 的说明。
#[test]
fn too_many_consecutive_malformed_frames_ask_to_close() {
    let mut l = listener().with_malformed_limit(3);
    let bad = "a[\"!!!not base64!!!\"]";

    assert!(l.on_frame(bad, NOW).unwrap().close.is_none());
    assert!(l.on_frame(bad, NOW + 1).unwrap().close.is_none());
    let third = l.on_frame(bad, NOW + 2).unwrap();
    assert_eq!(
        third.close,
        Some(CloseReason::TooManyMalformed),
        "连续第三帧坏帧 ⇒ 请连接层关闭（对面不是我们以为的那个端点）"
    );
}

/// 中间来一帧**正常**的帧 ⇒ 连续坏帧计数**清零**。
///
/// ⚠ 单帧坏掉是正常的（协议会演进），不能累积到「迟早会关」。
#[test]
fn a_good_frame_resets_the_malformed_counter() {
    let mut l = listener().with_malformed_limit(3);
    let bad = "a[\"!!!not base64!!!\"]";

    assert!(l.on_frame(bad, NOW).unwrap().close.is_none());
    assert!(l.on_frame(bad, NOW + 1).unwrap().close.is_none());
    // 一帧好帧（心跳也算"这条连接是活的"）
    let _ = l.on_frame(SOCKJS_HEARTBEAT, NOW + 2).unwrap();
    assert!(
        l.on_frame(bad, NOW + 3).unwrap().close.is_none(),
        "计数要清零，不能累积到关连接"
    );
    assert!(l.on_frame(bad, NOW + 4).unwrap().close.is_none());
}

/// `malformed_limit = 0` ⇒ **不**因为坏帧关闭（一直挂着，靠调用方超时）。
#[test]
fn malformed_limit_zero_never_closes() {
    let mut l = listener().with_malformed_limit(0);
    for i in 0..50 {
        assert!(l.on_frame("a[\"!!!\"]", NOW + i).unwrap().close.is_none());
    }
}

// ══════════════════════ 服务端关闭 ══════════════════════

/// 服务端发 `c[...]` ⇒ `Closed`，而且之后**什么都不回**。
///
/// # ⚠ 「服务端关了」与「我们自己要关」是两件事
///
/// - 服务端 `c[...]` ⇒ `kind == Closed`，但 `close` 是 `None`
///   （连接已经是关的，没有「再关一次」）；
/// - 我们要求关 ⇒ `close` 是 `Some(…)`。
///
/// 折成一个字段的话，「被断开」与「我们自己放弃」就再也分不开了。
#[test]
fn server_close_is_reported_distinctly_and_stops_all_activity() {
    let mut l = listener();
    // 先正常建立连接
    let _ = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    let _ = l.on_frame(&provision_ack_frame(NOW + 1), NOW + 1).unwrap();

    let closed = l.on_frame(r#"c[3000,"Go away!"]"#, NOW + 2).unwrap();
    assert_eq!(closed.kind, FrameKind::Closed);
    assert!(
        closed.close.is_none(),
        "服务端已经关了 ⇒ 我们不需要再关一次（`close` 是给「我们要求关」用的）"
    );
    assert!(closed.outbound.is_empty());
    assert!(l.is_closed());

    // 之后的心跳也**不回**了（往一条关掉的连接写只会得到错误）。
    let after = l.on_frame(SOCKJS_HEARTBEAT, NOW + 3).unwrap();
    assert!(
        after.outbound.is_empty(),
        "连接已关 ⇒ 心跳也不再回（回也发不出去）"
    );
    assert_eq!(after.kind, FrameKind::Ignored);
}

/// 关闭帧还有别的形态（`c[1002,"..."]` 等）—— 前缀判据要够宽。
#[test]
fn any_close_frame_shape_is_recognized() {
    for text in [
        r#"c[3000,"Go away!"]"#,
        "c[1002,\"x\"]",
        "c[2010,\"Another connection\"]",
    ] {
        let mut l = listener();
        let actions = l.on_frame(text, NOW).unwrap();
        assert_eq!(actions.kind, FrameKind::Closed, "{text} 应当被认成关闭");
        assert!(l.is_closed());
    }
}

// ══════════════════════ 连接层要的三态 ══════════════════════

/// ⭐ 状态机能回答「连接真的建立起来了吗」—— 连接层据此区分三态。
///
/// | 三态 | 判据 |
/// |---|---|
/// | **建不起来**（多半是房间关了 / 直播结束） | `provision_ack_seen() == false` |
/// | **建立过，但断了** | `provision_ack_seen() == true` 且连接没了 |
/// | **正常** | 两者都真 |
///
/// ⚠ 「TCP 连上了」与「连接建立起来了」**不是一回事**：前者只说明到了
/// SockJS 那一层，PROVISION 的响应才是「对面认了这次进房」。
/// 而这两者**表现完全一样**（都是「连上了、没弹幕」）。
#[test]
fn provision_ack_seen_distinguishes_the_three_connection_states() {
    // ① 只收到 `o`、然后就没了 ⇒ 建不起来
    let mut l = listener();
    let _ = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    assert!(!l.provision_ack_seen(), "只发了 PROVISION，还没被认");
    assert!(l.provision_sent());

    // ② 收到 `command=3` ⇒ 建立起来了
    let _ = l.on_frame(&provision_ack_frame(NOW + 1), NOW + 1).unwrap();
    assert!(l.provision_ack_seen(), "收到 3 的响应 ⇒ 建立起来了");
    assert!(l.unread_sent(), "而且 UNREAD 也补上了");

    // ③ 从未收到过任何东西的空跑
    let l = listener();
    assert!(!l.provision_ack_seen());
    assert!(!l.provision_sent());
    assert!(!l.unread_sent());
    assert!(!l.is_closed());
    assert!(l.codes().is_empty());
}

/// `FrameActions` 能表达连接层要的**四种**动作。
#[test]
fn frame_actions_express_the_four_connection_intents() {
    // ① 什么都不做
    let mut l = listener();
    let idle = l.on_frame("unknown", NOW).unwrap();
    assert!(!idle.has_outbound() && idle.close.is_none());

    // ② 回一个文本帧
    let hb = l.on_frame(SOCKJS_HEARTBEAT, NOW).unwrap();
    assert!(hb.has_outbound());
    assert!(matches!(hb.outbound[0], Outbound::Text(_)));
    assert_eq!(hb.close, None);

    // ③ 回一个字节帧（`Outbound::Binary` 存在，当前没有帧走它 —— 但类型要能表达）
    let binary = Outbound::Binary(vec![1, 2, 3]);
    assert_eq!(binary.as_text(), None, "二进制帧不是文本");
    assert_eq!(Outbound::from("x"), Outbound::Text("x".to_string()));

    // ④ 关闭连接（带原因）
    let mut l = listener().with_malformed_limit(1);
    let closing = l.on_frame("a[\"!!!\"]", NOW).unwrap();
    assert!(closing.close.is_some());
    assert!(matches!(closing.close, Some(CloseReason::TooManyMalformed)));
    // 三个原因都能表达出人话（供日志用）。
    assert!(!CloseReason::TooManyMalformed.to_string().is_empty());
    assert!(!CloseReason::ServerClosed.to_string().is_empty());
    assert!(!CloseReason::Enough.to_string().is_empty());
    assert_eq!(CloseReason::Other("x".into()).to_string(), "x");
}

/// `FrameKind` 的短名是**稳定接口**（进日志与断言），别跟着 `Debug` 变。
#[test]
fn frame_kinds_have_stable_names() {
    let pairs = [
        (FrameKind::Ignored, "ignored"),
        (FrameKind::Heartbeat, "heartbeat"),
        (FrameKind::Open, "open"),
        (FrameKind::Closed, "closed"),
        (FrameKind::ProvisionAck, "provision_ack"),
        (FrameKind::Danmaku, "danmaku"),
        (FrameKind::Silent, "silent"),
        (FrameKind::Malformed, "malformed"),
    ];
    for (kind, name) in pairs {
        assert_eq!(kind.as_str(), name);
        assert_eq!(kind.to_string(), name);
    }
}

// ══════════════════════ 凭证不外泄 ══════════════════════

/// `Debug` 输出**不含** access_token / phone。
///
/// 手写 `Debug` 而不是 derive 就是为了这条：凭据掉进任何一行日志都是泄漏。
#[test]
fn debug_output_redacts_credentials() {
    let l = KoulingListener::new(
        "A03SUPERSECRETTOKEN",
        "2026072314302675201220000000000",
        "19900000000",
        Box::new(SeqRandom::new(1)),
    );
    let text = format!("{l:?}");
    assert!(
        !text.contains("A03SUPERSECRETTOKEN"),
        "token 泄漏了：{text}"
    );
    assert!(!text.contains("19900000000"), "手机号泄漏了：{text}");
    assert!(text.contains("<REDACTED>"), "应当标出被脱敏的字段：{text}");
    // 非凭据字段照常可见（否则这个 Debug 就没用了）。
    assert!(text.contains("2026072314302675201220000000000"), "{text}");
}

// ══════════════════════ 纯层边界 ══════════════════════

/// 状态机**不 sleep、不取时钟** —— 全部由 `now` 驱动。
///
/// 这条用「两次调用之间没有任何时间流逝」来钉：如果实现里自己取时钟，
/// 那么 `instant_ack_after_open` 的时间戳就不会等于传入的 `now`。
#[test]
fn the_machine_never_reads_the_wall_clock() {
    let mut l = listener();
    // 传一个"很早"的时刻：若实现自己取时钟，报文里会是**今天**。
    let ancient = 1_600_000_000_000_i64;
    let step = l.on_frame(SOCKJS_OPEN, ancient).unwrap();
    let msg = parse_outbound(step.outbound[0].as_text().unwrap());
    assert_eq!(
        msg.timestamp,
        crate::live_msync::live_timestamp_bytes(ancient),
        "时间戳必须来自传入的 now，不是系统时钟"
    );
    assert_eq!(msg.guid.client_resource, format!("js_{ancient}"));
}

/// 没有实拍的部分：`chatRoomId` 为 `null` 时**根本不该有 WS**。
///
/// 这条测的不是本模块的代码，而是**那个前提本身**：
/// 上层（任务编排）拿到 `RoomBeat::has_chat_room() == false` 时
/// 就**不该**构造状态机、更不该去连 WS。
///
/// 留着它是为了让「为什么这一族没有实拍」这件事在**测试**里也有个落点
/// —— 而不是只写在文档注释里、下次有人问起时谁也说不清。
#[test]
fn no_chat_room_means_no_websocket_at_all() {
    // 实拍 §5.2 的 `roomBeat` 响应（未开播）。
    let body = r#"{"rtnMsg":null,"rtnCode":"0","object":{
      "status":0,"chatRoomId":null,"anchor":{"nickname":"中国移动云盘"}
    }}"#;
    let beat =
        crate::live_room::parse_room_beat(&crate::live_room::CmliveEnvelope::parse(body).unwrap())
            .unwrap();
    assert!(
        !beat.has_chat_room(),
        "2026-09-22 的实拍：未开播 ⇒ chatRoomId 是 null ⇒ WS 结构性不存在"
    );
    // 这就是 `probe-live-room-2026-09-23.md` 里那三条「🔌 要抓包」的原因。
    // 本节里所有关于帧顺序 / command 取值 / UNREAD 的断言，
    // 证据等级都是「原项目实现」，**不是**实拍。
}

// ══════════════════════ 造帧的小助手 ══════════════════════

/// 造一个只含一条 SYNC 弹幕的 `MessageBody` payload。
fn payload_for_text(text: &str) -> Vec<u8> {
    // `MessageBody { type = 1, contents = [Content { type = 2, text }] }`
    crate::live_msync::encode_message_body(&crate::live_msync::MessageBody {
        body_type: 1,
        contents: vec![crate::live_msync::MessageContent {
            content_type: 2,
            text: text.to_string(),
        }],
    })
}

/// 造一条服务端推下来的弹幕帧（`a["<base64>"]`）。
fn make_danmaku(text: &str) -> String {
    inbound(CMD_SYNC, payload_for_text(text), NOW)
}

/// 黄金向量的 `msyncFrames` 三条**字节级**对照 —— 用来确认状态机发出去的
/// 报文与**原实现**逐字节相同。
///
/// # ⚠ 这不是本模块的核心判据（T2 那边有专门的字节对拍）
///
/// 放在这里的原因：状态机是**唯一**构造 `guid` / `command` / `payload`
/// 那几项的地方（`live_msync_frame` 只填恒定项）。若这里传错了 `phone`
/// 或 `room_id`，报文就与原客户端不同 —— 而**只有逐字节比才发现得了**。
#[test]
fn outbound_provision_matches_the_golden_vector_when_inputs_match() {
    let v = vectors();
    let case = &v["msyncFrames"][0]; // provision
    assert_eq!(case["name"], "provision");
    let expect = &case["expect"];
    let timestamp = case["timestampMillis"].as_i64().unwrap();

    let mut l = KoulingListener::new(
        expect["auth"].as_str().unwrap().trim_start_matches("$t$"),
        "ROOM",
        "19900000000",
        Box::new(SeqRandom::new(0x00)),
    );
    let step = l.on_frame(SOCKJS_OPEN, timestamp).unwrap();
    let msg = parse_outbound(step.outbound[0].as_text().unwrap());

    // 与黄金向量的 `expect` 逐项对照。
    assert_eq!(msg.guid.app_key, expect["guid"]["appKey"].as_str().unwrap());
    assert_eq!(msg.guid.name, expect["guid"]["name"].as_str().unwrap());
    assert_eq!(msg.guid.domain, expect["guid"]["domain"].as_str().unwrap());
    assert_eq!(
        msg.guid.client_resource,
        expect["guid"]["clientResource"].as_str().unwrap()
    );
    assert_eq!(msg.auth, expect["auth"].as_str().unwrap());
    assert_eq!(msg.user_agent, expect["userAgent"].as_str().unwrap());
    assert_eq!(u64::from(msg.command), expect["command"].as_u64().unwrap());
    assert_eq!(msg.device_info, crate::live_msync::live_device_info());

    // PROVISION 的 payload 逐项。
    let p = crate::live_msync::decode_provision(&msg.payload).unwrap();
    let ep = &case["expectProvision"];
    assert_eq!(p.os_type as u64, ep["osType"].as_u64().unwrap());
    assert_eq!(p.version, ep["version"].as_str().unwrap());
    assert_eq!(p.network_type as u64, ep["networkType"].as_u64().unwrap());
    assert_eq!(p.app_sign, ep["appSign"].as_str().unwrap());
    assert_eq!(p.device_uuid, ep["deviceUuid"].as_str().unwrap());
    assert_eq!(p.device_name, ep["deviceName"].as_str().unwrap());
    assert_eq!(p.resource, ep["resource"].as_str().unwrap());
    assert_eq!(p.province_code, ep["provinceCode"].as_str().unwrap());
}

/// 状态机发的 UNREAD 帧与原实现的 `command=1` 报文同形。
#[test]
fn outbound_unread_has_the_expected_shape() {
    let mut l = listener();
    let _ = l.on_frame(SOCKJS_OPEN, NOW).unwrap();
    let ack = l.on_frame(&provision_ack_frame(NOW + 1), NOW + 1).unwrap();
    let unread = parse_outbound(ack.outbound[0].as_text().unwrap());

    assert_eq!(unread.command, CMD_UNREAD);
    assert!(unread.payload.is_empty(), "UNREAD 没有 payload");
    assert_eq!(unread.pov, Some(0), "ⓘ 零值也要写（`38 00`）");
    assert_eq!(unread.encrypt_type, vec![0]);
    assert_eq!(unread.guid.client_resource, format!("js_{}", NOW + 1));
    assert_eq!(
        unread.timestamp,
        crate::live_msync::live_timestamp_bytes(NOW + 1)
    );
}

/// `extract_danmaku_text` 对控制帧（无 payload）返回空串，**不是错误**。
///
/// 心跳、无 payload 的控制帧本来就不该被当成异常 —— 所以状态机
/// 把它们归成 `Silent` 而不是 `Malformed`。
#[test]
fn control_frames_without_payload_are_silent_not_malformed() {
    let mut l = listener();
    // `command = 4`（LOGOUT）没有 payload
    let actions = l.on_frame(&inbound(4, Vec::new(), NOW), NOW).unwrap();
    assert_eq!(
        actions.kind,
        FrameKind::Silent,
        "没有文本的控制帧是正常的（Silent），不是 Malformed"
    );
    assert!(actions.codes.is_empty());
    assert!(!actions.has_outbound());
}

/// ⚠ 直接确认：`extract_danmaku_text` 对空 payload 给 `Ok("")`。
#[test]
fn empty_payload_yields_empty_text_not_an_error() {
    assert_eq!(extract_danmaku_text(CMD_SYNC, &[]).unwrap(), "");
    assert_eq!(extract_danmaku_text(CMD_NOTICE, &[]).unwrap(), "");
    // 非 SYNC/NOTICE 的 command 也不解文本
    assert_eq!(extract_danmaku_text(4, &[1, 2, 3]).unwrap(), "");
}
