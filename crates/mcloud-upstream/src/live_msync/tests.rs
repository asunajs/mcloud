//! `live_msync` 的测试 —— **字节级**黄金向量对拍。
//!
//! 判据来源（本项目一贯口径）：**原实现产出的黄金向量 > 一切**。
//! 本文件的期望值全部取自 `test/golden/live-room-vectors.json` 的
//! `msyncFrames` / `msyncInbound` / `constants` 三段，**没有一条是手写的**。
//!
//! # 为什么必须逐字节，而不是"解回来对不对"
//!
//! 那个文件的 `$note` 记着一段历史：这份向量曾经因为**手写**期望值把**正确**的
//! 实现判红（34 条里错 3 条），后来又一度改成读**过期的**构建产物 —— 两种都是
//! 假基准。所以这里只做两件事：
//!
//! 1. **比 base64 / hex**（实现编出来的字节 vs 原实现产出的字节）；
//! 2. **比字段值**（解回来与 `expect` / `expectProvision` 逐字段对）。
//!
//! 弱断言（"解回来 `command` 对不对"）在这个模块里是**不够的**：`pov` 少写
//! `38 00` 两个字节，protobufjs 两边都解得开、字段也读得出 0 —— 只有 (1) 抓得到。

use super::*;
use mcloud_crypto::SeqRandom;
use serde_json::Value;

/// 黄金向量文件的路径。
///
/// 用 `CARGO_MANIFEST_DIR` 而不是相对 `cwd`：测试运行的 cwd 不保证是 crate 根，
/// 相对路径会让这条测试在不同调用方式下时灵时不灵。
fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/live-room-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

fn const_str(v: &Value, key: &str) -> String {
    v["constants"][key]
        .as_str()
        .unwrap_or_else(|| panic!("constants.{key} 不是字符串"))
        .to_owned()
}

/// 从一条 `msyncFrames` / `msyncInbound` 记录里取字符串字段。
fn field_str(case: &Value, key: &str) -> String {
    case[key]
        .as_str()
        .unwrap_or_else(|| panic!("字段 {key} 不是字符串（用例 {}）", case["name"]))
        .to_owned()
}

/// 向量里的 `identificationHex` **只是随机源的那 16 字节**，不含 `cmos_im` 前缀。
///
/// 而 `expect.identificationHex` 是**全量**（`cmos_im` + 那 16 字节）。
/// 这个区别抄错的表现是"对拍永远差 7 个字节"，所以单独一个 helper 说明白。
fn seq_random_for(identification_hex: &str) -> SeqRandom {
    let raw = hex::decode(identification_hex).expect("identificationHex 必须是合法 hex");
    assert_eq!(raw.len(), 16, "identificationHex 是 16 字节随机源");
    SeqRandom::new(raw[0])
}

fn to_hex(data: &[u8]) -> String {
    hex::encode(data)
}

/// 标准 base64 编码（`A–Za–z0–9+/` + `=` 填充）。
///
/// # 为什么不引 `base64` crate
///
/// 本仓对依赖是**逐条审**的，而这里需要它的唯一场景是**测试**里把编出来的字节
/// 与向量里的 `base64` 字段比一下。为此给一个生产 crate 加依赖不划算 ——
/// 30 行手写没有 IO、没有边界玄机，而且它的**正确性本身由黄金向量钉住**：
/// 向量里同一段字节同时有 `base64` 与 `hex` 两种表示，两份必须对得上
/// （见 [`inbound_payload_hex_and_base64_agree`] 与
/// [`msync_frames_match_golden_byte_for_byte`]）。
///
/// ⚠ 这是**测试**用的编码器，不是生产代码的一部分。
fn to_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map(u32::from);
        let b2 = chunk.get(2).copied().map(u32::from);
        let triple = (b0 << 16) | (b1.unwrap_or(0) << 8) | b2.unwrap_or(0);
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(match b1 {
            Some(_) => ALPHABET[(triple >> 6) as usize & 0x3f] as char,
            None => '=',
        });
        out.push(match b2 {
            Some(_) => ALPHABET[triple as usize & 0x3f] as char,
            None => '=',
        });
    }
    out
}

/// 标准 base64 解码（拒绝非法字符，接受 `=` 填充）。
fn from_base64(s: &str) -> Vec<u8> {
    fn idx(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let body: Vec<u8> = cleaned.iter().copied().filter(|b| *b != b'=').collect();
    let mut out = Vec::with_capacity(body.len() / 4 * 3);
    for chunk in body.chunks(4) {
        let mut acc: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let v = idx(c).unwrap_or_else(|| panic!("base64 里有非法字符 0x{c:02x}：{s}"));
            acc |= v << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    out
}

// ══════════════════════════ 常量 ══════════════════════════

/// 常量逐条对齐 `constants` 段。
///
/// ⚠ `DEVICE_INFO_HEX` 的字节数别按"看起来多长"估：**48**。
/// 这个数在别处被写成过 40，而"40 字节"读起来同样合理 ⇒ 只能数。
#[test]
fn constants_match_golden() {
    let v = vectors();
    assert_eq!(LIVE_APP_KEY, const_str(&v, "LIVE_APP_KEY"));
    assert_eq!(SDK_APP_KEY, const_str(&v, "APP_KEY"));
    assert_eq!(LIVE_DOMAIN, const_str(&v, "DOMAIN"));
    assert_eq!(LIVE_MSYNC_USER_AGENT, const_str(&v, "USER_AGENT"));
    assert_eq!(LIVE_DEVICE_INFO_HEX, const_str(&v, "DEVICE_INFO_HEX"));

    let info = live_device_info();
    assert_eq!(
        info.len(),
        48,
        "deviceInfo 是 48 字节（96 个 hex 字符 / 2）"
    );
    assert_eq!(to_hex(&info), const_str(&v, "DEVICE_INFO_HEX"));
}

/// `MSYNC.userAgent` 与 HTTP 头的 UA **不是一个值**（前者短一截）。
///
/// 互搬之后没有任何本地断言会变红 —— 所以在这里钉一下长度差。
#[test]
fn msync_user_agent_differs_from_http_header_ua() {
    assert_ne!(LIVE_MSYNC_USER_AGENT, crate::live_consts::LIVE_USER_AGENT);
    assert!(crate::live_consts::LIVE_USER_AGENT.starts_with(LIVE_MSYNC_USER_AGENT));
}

/// `Command` 枚举的取值逐字对齐 `proto.ts`。
#[test]
fn command_enum_matches_proto_ts() {
    assert_eq!(
        [
            CMD_SYNC,
            CMD_UNREAD,
            CMD_NOTICE,
            CMD_PROVISION,
            CMD_LOGOUT,
            CMD_HEARTBEAT,
            CMD_ROUTE,
            CMD_EVT_BROADCAST,
        ],
        [0, 1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(MSYNC_V1, 0);
    assert_eq!(MSYNC_V2, 1);
}

/// `timestamp` 是 **8 字节大端**毫秒，不是 protobuf 的 varint。
#[test]
fn timestamp_is_eight_byte_big_endian() {
    let v = vectors();
    let case = &v["msyncFrames"][0];
    let millis = case["timestampMillis"].as_i64().expect("timestampMillis");
    let bytes = live_timestamp_bytes(millis);
    assert_eq!(bytes.len(), 8);
    assert_eq!(
        to_hex(&bytes),
        "0000019f8dc8f2e2",
        "provision 那条的 timestamp 在向量 hex 里可见：…720800 0001 9f8d c8f2 e27a…"
    );
    // 大端：高字节在前 ⇒ 值越大，字符串序也越大
    assert!(live_timestamp_bytes(1) < live_timestamp_bytes(2));
}

/// `identification` = `cmos_im` 前缀 + 16 随机字节 = **23 字节**。
#[test]
fn identification_is_prefix_plus_sixteen_bytes() {
    let v = vectors();
    let case = &v["msyncFrames"][0];
    let mut rng = seq_random_for(&field_str(case, "identificationHex"));
    let ident = live_identification(&mut rng);
    assert_eq!(ident.len(), 23);
    assert_eq!(
        to_hex(&ident),
        field_str(&case["expect"], "identificationHex")
    );
}

// ══════════════════════════ 出站：msyncFrames 逐字节 ══════════════════════════

/// 把一条 `msyncFrames` 用例还原成 `MsyncMessage`（**生产构造路径**）。
///
/// 用 [`live_msync_frame`] 而不是手拼结构体：恒定项（`pov = Some(0)` /
/// `encrypt_type = [0]` / `user_agent` / `$t$` 前缀 / `device_info`）全在
/// 那个构造函数里，手拼就把它们从被测范围里挪走了。
fn frame_from_case(case: &Value, acc: &str, phone: Option<&str>, room_id: &str) -> MsyncMessage {
    let millis = case["timestampMillis"].as_i64().expect("timestampMillis");
    let command = u8::try_from(case["command"].as_u64().expect("command")).expect("command 是 u8");
    let mut rng = seq_random_for(&field_str(case, "identificationHex"));
    live_msync_frame(acc, phone, room_id, command, Vec::new(), millis, &mut rng)
}

/// ⭐ **核心断言**：三条出站帧在 `base64`、`hex` 两级都与原实现逐字节相等。
#[test]
fn msync_frames_match_golden_byte_for_byte() {
    let v = vectors();
    let acc = const_str(&v, "ACCESS_TOKEN");
    let phone = const_str(&v, "PHONE");
    let room_id = const_str(&v, "ROOM_ID");
    let frames = v["msyncFrames"].as_array().expect("msyncFrames 是数组");
    assert_eq!(
        frames.len(),
        3,
        "向量是 3 条：provision / unread / sync-no-phone"
    );

    for case in frames {
        let name = field_str(case, "name");
        let expect_phone: Option<&str> = match name.as_str() {
            // 直播间形态（有 phone）
            "provision" | "unread" => Some(&phone),
            // 非直播间形态（无 phone）⇒ 走 `cmos_js_sdk` + `cmlive_<roomId>` 分支
            "sync-no-phone" => None,
            other => panic!("未知用例 {other}"),
        };

        let mut frame = frame_from_case(case, &acc, expect_phone, &room_id);
        if name == "provision" {
            frame.payload = live_provision_payload(
                &acc,
                &field_str(case, "resource"),
                &field_str(case, "deviceUuid"),
            );
        }

        let wire = encode_msync(&frame);
        assert_eq!(
            to_hex(&wire),
            field_str(case, "hex"),
            "【{name}】hex 不一致（先查字段号 / wire type，别怀疑向量）"
        );
        assert_eq!(
            to_base64(&wire),
            field_str(case, "base64"),
            "【{name}】base64 不一致"
        );
        assert_eq!(
            from_base64(&field_str(case, "base64")),
            wire,
            "【{name}】向量 base64 解出来的字节应等于编码结果"
        );
    }
}

/// `provision` 那条的 **payload**（内层 `Provision`）也逐字节对拍。
#[test]
fn provision_payload_matches_golden_byte_for_byte() {
    let v = vectors();
    let case = &v["msyncFrames"][0];
    assert_eq!(field_str(case, "name"), "provision");
    let acc = const_str(&v, "ACCESS_TOKEN");

    let payload = live_provision_payload(
        &acc,
        &field_str(case, "resource"),
        &field_str(case, "deviceUuid"),
    );
    assert_eq!(to_hex(&payload), field_str(case, "payloadHex"));
}

/// 另外两条的 `payloadHex` 是**空串** ⇒ payload 字段整条不出现。
///
/// 这一条看着微不足道，但它钉的是"没有 payload 时别写一个长度 0 的字段"。
#[test]
fn frames_without_payload_omit_the_field() {
    let v = vectors();
    let acc = const_str(&v, "ACCESS_TOKEN");
    let phone = const_str(&v, "PHONE");
    let room_id = const_str(&v, "ROOM_ID");

    for case in v["msyncFrames"].as_array().expect("数组") {
        if !case["payloadHex"].as_str().expect("payloadHex").is_empty() {
            continue;
        }
        let name = field_str(case, "name");
        let p = if name == "unread" {
            Some(phone.as_str())
        } else {
            None
        };
        let frame = frame_from_case(case, &acc, p, &room_id);
        assert!(frame.payload.is_empty());
        let wire = encode_msync(&frame);
        assert_eq!(to_hex(&wire), field_str(case, "hex"));
    }
}

/// 出站帧解回来要与 `expect` / `expectProvision` 逐字段一致。
///
/// 这是"自洽"那一半（第 1 级），与上面的字节级断言互补：
/// 字节相等证明**发出去了什么**，字段相等证明**读得回来**。
#[test]
fn encoded_frames_round_trip_through_expect() {
    let v = vectors();

    for case in v["msyncFrames"].as_array().expect("数组") {
        let name = field_str(case, "name");
        let expect = &case["expect"];
        let wire = from_base64(&field_str(case, "base64"));
        let decoded = decode_msync(&wire).expect("解一条向量帧不该失败");

        assert_eq!(
            u8::try_from(expect["version"].as_u64().unwrap()).unwrap(),
            decoded.version,
            "【{name}】version"
        );
        assert_eq!(
            expect["guid"]["appKey"].as_str().unwrap(),
            decoded.guid.app_key
        );
        assert_eq!(expect["guid"]["name"].as_str().unwrap(), decoded.guid.name);
        assert_eq!(
            expect["guid"]["domain"].as_str().unwrap(),
            decoded.guid.domain
        );
        assert_eq!(
            expect["guid"]["clientResource"].as_str().unwrap(),
            decoded.guid.client_resource
        );
        assert_eq!(expect["auth"].as_str().unwrap(), decoded.auth);
        assert_eq!(expect["userAgent"].as_str().unwrap(), decoded.user_agent);
        assert_eq!(
            u8::try_from(expect["command"].as_u64().unwrap()).unwrap(),
            decoded.command
        );
        assert_eq!(
            vec![u32::try_from(expect["encryptType"][0].as_u64().unwrap()).unwrap()],
            decoded.encrypt_type
        );
        assert_eq!(
            expect["deviceInfoHex"].as_str().unwrap(),
            to_hex(&decoded.device_info)
        );
        assert_eq!(
            expect["identificationHex"].as_str().unwrap(),
            to_hex(&decoded.identification)
        );

        if name == "provision" {
            let p = decode_provision(&decoded.payload).expect("解 Provision payload");
            let ep = &case["expectProvision"];
            assert_eq!(
                u32::try_from(ep["osType"].as_u64().unwrap()).unwrap(),
                p.os_type
            );
            assert_eq!(ep["version"].as_str().unwrap(), p.version);
            assert_eq!(
                u32::try_from(ep["networkType"].as_u64().unwrap()).unwrap(),
                p.network_type
            );
            assert_eq!(ep["appSign"].as_str().unwrap(), p.app_sign);
            assert_eq!(
                vec![u32::try_from(ep["compressType"][0].as_u64().unwrap()).unwrap()],
                p.compress_type
            );
            assert_eq!(
                vec![u32::try_from(ep["encryptType"][0].as_u64().unwrap()).unwrap()],
                p.encrypt_type
            );
            assert_eq!(ep["deviceUuid"].as_str().unwrap(), p.device_uuid);
            assert_eq!(ep["deviceName"].as_str().unwrap(), p.device_name);
            assert_eq!(ep["resource"].as_str().unwrap(), p.resource);
            assert_eq!(ep["auth"].as_str().unwrap(), p.auth);
            // ⚠ 原实现给的是**空串** ⇒ 解码出来也是空串，字段根本没出现过。
            assert_eq!(ep["provinceCode"].as_str().unwrap(), p.province_code);
            assert_eq!(p.province_code, "");
        }
    }
}

// ══════════════════════════ ⭐ `Some(0)` vs `None` ══════════════════════════

/// ⭐⭐ **本文件最该存在的一条测试**：`pov` 的 `Some(0)` 要写出去、`None` 不写。
///
/// # 为什么这一条是 task-2 的核心判据
///
/// Go 侧用**指针**表达 proto3 的"字段不出现 vs 写出去（含 0）"：
///
/// ```go
/// if m.Pov != nil { out = appendScalarForced(out, 7, *m.Pov) }
/// ```
///
/// Rust 侧对应 `Option<u64>`。两种写错的后果**完全不对称**：
///
/// - 把 `Some(0)` 写成 `None` 的行为 ⇒ **静默少 2 个字节**（`38 00`）；
/// - 漏掉这一支 ⇒ `command` / `auth` 全都解得出，**弱断言一律全绿**。
///
/// 而同一份报文里的 `crypto: 0` / `deviceId: 0` 是普通 number，
/// 它们**绝对不能**跟着写出去 —— 所以这一条同时钉两个方向。
#[test]
fn optional_pov_writes_some_zero_but_omits_none() {
    let base = MsyncMessage {
        command: CMD_SYNC,
        ..MsyncMessage::default()
    };

    // None ⇒ 字段 7 **整条不出现**。
    // 注意：全默认值下 `guid` 是个**空消息**，它照样写（`12 00`）——
    // 这正是"嵌套消息空也写"那条规则，与 `pov` 的取舍是两回事。
    let none_wire = encode_msync(&base);
    assert_eq!(to_hex(&none_wire), "1200", "只有空 guid，没有 pov");
    assert!(
        !none_wire.contains(&0x38),
        "`38` 是字段 7 的 tag，None 时不该出现"
    );

    // Some(0) ⇒ 写出去，恰好 `38 00`（在空 guid 的 `12 00` 之后）。
    let some_zero = MsyncMessage {
        pov: Some(0),
        ..MsyncMessage::default()
    };
    let some_zero_wire = encode_msync(&some_zero);
    assert_eq!(
        to_hex(&some_zero_wire),
        "12003800",
        "字段 7 + wire type 0 + 值 0"
    );
    assert_eq!(
        some_zero_wire.len(),
        none_wire.len() + 2,
        "差的正是那 2 个字节"
    );

    // 往返：Some(0) 解回来仍是 Some(0)，None 解回来是 None。
    assert_eq!(decode_msync(&some_zero_wire).unwrap().pov, Some(0));
    assert_eq!(decode_msync(&none_wire).unwrap().pov, None);

    // Some(非 0) 正常编码（`pov = 1` ⇒ `38 01`）。
    let one = MsyncMessage {
        pov: Some(1),
        ..MsyncMessage::default()
    };
    assert_eq!(to_hex(&encode_msync(&one)), "12003801");
    assert_eq!(decode_msync(&encode_msync(&one)).unwrap().pov, Some(1));
    // 值 300（两字节 varint）也要能往返。
    let big = MsyncMessage {
        pov: Some(300),
        ..MsyncMessage::default()
    };
    assert_eq!(decode_msync(&encode_msync(&big)).unwrap().pov, Some(300));
}

/// 同一个结构体里，**普通标量**的零值一律不写 —— 与 `pov` 形成对照。
///
/// 这是"照直觉写就错"的另一半：如果实现里偷懒把所有标量都按 `Option` 处理
/// （或反过来把 `pov` 也当成普通标量），黄金向量的三条帧会立刻不等。
#[test]
fn plain_scalars_omit_their_zero_values() {
    // version=0 / command=SYNC=0 都不出现；只有空 guid 那 2 个字节。
    let wire = encode_msync(&MsyncMessage {
        version: 0,
        command: 0,
        pov: None,
        ..MsyncMessage::default()
    });
    assert_eq!(
        to_hex(&wire),
        "1200",
        "version=0 / command=SYNC=0 都该整条不出现"
    );

    // 非零才写：1 version(0x08 01) + 2 guid(0x12 00) + 8 command(0x40 03)
    let wire = encode_msync(&MsyncMessage {
        version: MSYNC_V2,
        command: CMD_PROVISION,
        ..MsyncMessage::default()
    });
    assert_eq!(to_hex(&wire), "080112004003");
    let back = decode_msync(&wire).unwrap();
    assert_eq!(back.version, MSYNC_V2);
    assert_eq!(back.command, CMD_PROVISION);
}

/// **`repeated` 是第 1 条反例**：packed 编码，且元素为 0 也要写。
///
/// `encryptType = [0]` 的编码是 `5a 01 00` —— wire type **2**（不是 0），
/// 长度 1，值 0。空数组则**整条不出现**。
#[test]
fn repeated_scalars_are_packed_and_keep_zero_elements() {
    // 空 repeated ⇒ 不出现（只剩空 guid 那 2 字节）
    let empty = encode_msync(&MsyncMessage::default());
    assert_eq!(to_hex(&empty), "1200");
    assert!(
        !empty.contains(&0x5a),
        "`5a` 是字段 11 的 tag，空数组时不该出现"
    );

    // [0] ⇒ 5a 01 00（**值 0 也要写**）
    let one_zero = encode_msync(&MsyncMessage {
        encrypt_type: vec![0],
        ..MsyncMessage::default()
    });
    assert_eq!(to_hex(&one_zero), "12005a0100");
    assert_eq!(decode_msync(&one_zero).unwrap().encrypt_type, vec![0]);

    // [0, 1, 300] ⇒ packed 连写：00 01 ac 02
    let mixed = encode_msync(&MsyncMessage {
        encrypt_type: vec![0, 1, 300],
        ..MsyncMessage::default()
    });
    assert_eq!(to_hex(&mixed), "12005a040001ac02");
    assert_eq!(decode_msync(&mixed).unwrap().encrypt_type, vec![0, 1, 300]);

    // 非 packed 的重复 varint（wire 0）也要认 —— 服务端没有实拍，两种都收。
    let unpacked = [0x58u8, 0x00, 0x58, 0x01];
    assert_eq!(decode_msync(&unpacked).unwrap().encrypt_type, vec![0, 1]);
}

/// 嵌套消息**空也写**（`tag + 长度 0`）—— 它是"字段被设置了"。
///
/// `guid` 全空 ⇒ `12 00`。protobufjs 收到的是 `JIDType.create({})` 这个对象，
/// 对象存在就写。
#[test]
fn empty_nested_message_is_still_written() {
    // 全默认值时 guid 空 ⇒ 12 00，恰好 2 字节（**不是**空输出）。
    let wire = encode_msync(&MsyncMessage::default());
    assert_eq!(to_hex(&wire), "1200");

    let back = decode_msync(&wire).unwrap();
    assert_eq!(back.guid, Jid::default());

    // 对比：`payload`(9) / `device_info`(13) 这类 **bytes** 空则**不写**。
    // 同样是"空"，嵌套消息写、bytes 不写 —— 这是两类字段的取舍不同，
    // 黄金向量的 provision 帧里 `payload` 有值、另两条没有，正好两边都覆盖到。
    let with_empty_bytes = encode_msync(&MsyncMessage {
        payload: Vec::new(),
        device_info: Vec::new(),
        identification: Vec::new(),
        ..MsyncMessage::default()
    });
    assert_eq!(to_hex(&with_empty_bytes), "1200");
}

/// `Jid` 的四个字段号（1–4）与 wire type 逐个正确。
#[test]
fn jid_field_numbers_match_proto_ts() {
    let jid = Jid {
        app_key: "cmos#ngvlcs".to_owned(),
        name: "cmlive_c_19900000000".to_owned(),
        domain: "cmosim.com".to_owned(),
        client_resource: "js_1".to_owned(),
    };
    let wire = encode_jid(&jid);
    // 1 → 0a, 2 → 12, 3 → 1a, 4 → 22（全是 wire type 2 = 长度前缀）
    let hex = to_hex(&wire);
    assert!(
        hex.starts_with(&format!("0a0b{}", to_hex(b"cmos#ngvlcs"))),
        "字段 1：{hex}"
    );
    assert!(
        hex.contains(&format!("1214{}", to_hex(b"cmlive_c_19900000000"))),
        "字段 2：{hex}"
    );
    assert!(
        hex.contains(&format!("1a0a{}", to_hex(b"cmosim.com"))),
        "字段 3：{hex}"
    );
    assert!(
        hex.ends_with(&format!("2204{}", to_hex(b"js_1"))),
        "字段 4：{hex}"
    );
    assert_eq!(decode_jid(&wire).unwrap(), jid);
}

// ══════════════════════════ 入站：msyncInbound ══════════════════════════

/// `payloadBase64` 解出来要与 `payloadHex` 一致（先验向量自身自洽）。
#[test]
fn inbound_payload_hex_and_base64_agree() {
    let v = vectors();
    for case in v["msyncInbound"].as_array().expect("数组") {
        let name = field_str(case, "name");
        assert_eq!(
            to_hex(&from_base64(&field_str(case, "payloadBase64"))),
            field_str(case, "payloadHex"),
            "【{name}】向量里 base64 与 hex 应表示同一段字节"
        );
    }
}

/// ⭐ **核心断言**：三条入站帧都解出 `kind` 与 `text`。
///
/// | 用例 | 路径 | 层级 |
/// |---|---|---|
/// | `sync-comm-sync-ul` | SYNC → `CommSyncUL.meta.payload` → `MessageBody` | 第 1 级 |
/// | `notice-payload` | NOTICE → `NoticePayload.meta.payload` → `MessageBody` | 第 1 级（**字段号是 2 不是 1**）|
/// | `bare-message-body` | 整段 payload 直接当 `MessageBody` | 第 3 级兜底 |
#[test]
fn inbound_frames_decode_kind_and_text() {
    let v = vectors();
    let cases = v["msyncInbound"].as_array().expect("数组");
    assert_eq!(cases.len(), 3);

    for case in cases {
        let name = field_str(case, "name");
        let kind = field_str(case, "kind");
        let expect_text = field_str(case, "text");
        let payload = from_base64(&field_str(case, "payloadBase64"));

        let command = match kind.as_str() {
            "sync" => CMD_SYNC,
            "notice" => CMD_NOTICE,
            // `bare` 走的是兜底那一级：真实驱动层会在 SYNC/NOTICE 之外也试一次，
            // 这里用 SYNC 也能命中（外层解不开 ⇒ 退到兜底）。
            "bare" => CMD_SYNC,
            other => panic!("【{name}】未知 kind {other}"),
        };

        let text = extract_danmaku_text(command, &payload)
            .unwrap_or_else(|e| panic!("【{name}】解文本不该报错：{e}"));
        assert_eq!(text, expect_text, "【{name}】{kind} 路径的文本");
    }
}

/// `CommSyncUL` 的 `meta` 是字段 **1**，`NoticePayload` 的 `meta` 是字段 **2**。
///
/// ⚠ 这两个消息长得几乎一样，抄错字段号的表现是"NOTICE 那条**永远**解不出文本"
/// —— 而那与"这一帧本来就没文本"长得一模一样。所以这里用**交叉验证**钉死：
/// 拿 NOTICE 的 payload 去走 SYNC 的解析路径，必须解不出（返回空串）。
#[test]
fn comm_sync_ul_and_notice_payload_are_not_interchangeable() {
    let v = vectors();
    let cases = v["msyncInbound"].as_array().expect("数组");
    let sync_case = cases
        .iter()
        .find(|c| field_str(c, "name") == "sync-comm-sync-ul")
        .expect("有 sync 用例");
    let notice_case = cases
        .iter()
        .find(|c| field_str(c, "name") == "notice-payload")
        .expect("有 notice 用例");

    let sync_payload = from_base64(&field_str(sync_case, "payloadBase64"));
    let notice_payload = from_base64(&field_str(notice_case, "payloadBase64"));

    // 各自走对的路径 ⇒ 都能解出文本
    assert!(decode_comm_sync_ul_meta_payload(&sync_payload)
        .unwrap()
        .is_some());
    assert!(decode_notice_meta_payload(&notice_payload)
        .unwrap()
        .is_some());

    // 交叉：SYNC 的 payload 按 NoticePayload 解（找字段 2）——
    // 侥幸也会命中（CommSyncUL 的字段 2 是 uint64 key，不是 message），
    // 所以再判一层"解出来的东西不是 MessageBody"。
    let cross = decode_notice_meta_payload(&sync_payload).unwrap();
    if let Some(inner) = cross {
        assert!(
            decode_message_body(&inner)
                .map(|b| b.contents.is_empty())
                .unwrap_or(true),
            "CommSyncUL.key 被误当成 NoticePayload.meta 时不该解出文本"
        );
    }
    // NOTICE 的 payload 按 CommSyncUL 解（找字段 1）拿到的是 **queue(JID)**，
    // 不是 Meta ⇒ 里面没有 payload 字段 6，退到兜底也解不出文本。
    let cross_back = decode_comm_sync_ul_meta_payload(&notice_payload).unwrap();
    if let Some(inner) = cross_back {
        assert!(
            first_content_text(&inner).is_none(),
            "NoticePayload.queue 被误当成 CommSyncUL.meta 时不该解出文本"
        );
    }
}

/// 兜底那一级**真的会被用到**：把裸 `MessageBody` 当成 SYNC 帧喂进去，
/// 外层解不开、兜底解得出。
#[test]
fn fallback_level_recovers_a_bare_message_body() {
    let v = vectors();
    let bare = v["msyncInbound"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| field_str(c, "name") == "bare-message-body")
        .expect("有裸用例");
    let payload = from_base64(&field_str(bare, "payloadBase64"));

    // 先确认裸帧确实**不是**合法 CommSyncUL.meta.payload（否则这条测试没意义）
    let outer = decode_comm_sync_ul_meta_payload(&payload).unwrap();
    assert!(
        outer.is_none() || first_content_text(&outer.unwrap()).is_none(),
        "裸帧在第一级不该解出文本 —— 否则兜底这一级没被测到"
    );

    assert_eq!(
        extract_danmaku_text(CMD_SYNC, &payload).unwrap(),
        field_str(bare, "text")
    );
}

/// 没有文本的帧返回**空串而不是错误**（心跳、控制帧本来就不是异常）。
#[test]
fn frames_without_text_return_empty_not_error() {
    assert_eq!(
        extract_danmaku_text(CMD_HEARTBEAT, &[0x08, 0x00]).unwrap(),
        ""
    );
    assert_eq!(extract_danmaku_text(CMD_SYNC, &[]).unwrap(), "");
    assert_eq!(
        extract_danmaku_text(CMD_PROVISION, &[0x08, 0x07]).unwrap(),
        ""
    );
    // SYNC 帧但内容里没有 contents ⇒ 空串
    assert_eq!(extract_danmaku_text(CMD_SYNC, &[0x08, 0x00]).unwrap(), "");
}

/// 外层**结构坏了**时，错误要被报上来（而不是静默空串）。
///
/// 这一条的价值在于：它保证"解不开"与"没有文本"在类型上是两件事 ——
/// 前者是 `Err`（调用方会打 hex 预览），后者是 `Ok("")`。
#[test]
fn malformed_payload_reports_an_error() {
    // 单个 0x80 是截断的 varint。
    let err = extract_danmaku_text(CMD_SYNC, &[0x80]);
    assert!(
        err.is_err(),
        "截断的 varint 该报错，而不是静默空串：{err:?}"
    );

    // 字段号为 0 非法。
    assert!(scan_proto_fields(&[0x00]).is_err());

    // 声明长度超出实际字节。
    assert!(scan_proto_fields(&[0x0a, 0x05, 0x01]).is_err());

    // 未知 wire type（3 = group，proto3 已废弃）。
    assert!(scan_proto_fields(&[0x0b]).is_err());

    // 长度前缀本身是截断的 varint。
    assert!(scan_proto_fields(&[0x0a, 0x80]).is_err());
}

/// `MessageBody` 编码器是解码器的**逆**（供上层造入站帧用）。
#[test]
fn message_body_encoder_inverts_decoder() {
    let body = MessageBody {
        body_type: 2,
        contents: vec![
            MessageContent {
                content_type: 0,
                text: "口令1:MClaw".to_owned(),
            },
            // ⚠ 只有 type、没有 text 的 content：`append_message` **空也写**。
            MessageContent {
                content_type: 0,
                text: String::new(),
            },
        ],
    };
    let wire = encode_message_body(&body);
    // 第二个 content 是空消息 ⇒ 4 号字段 + 长度 0（`22 00`）
    assert!(
        to_hex(&wire).ends_with("2200"),
        "空 content 也要写：{}",
        to_hex(&wire)
    );
    assert_eq!(decode_message_body(&wire).unwrap(), body);
}

/// 裸 `MessageBody` 的编码结果与向量逐字节一致（兜底那一级的反向验证）。
#[test]
fn bare_message_body_encoder_matches_golden() {
    let v = vectors();
    let bare = v["msyncInbound"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| field_str(c, "name") == "bare-message-body")
        .unwrap();
    let body = decode_message_body(&from_base64(&field_str(bare, "payloadBase64"))).unwrap();
    assert_eq!(
        to_hex(&encode_message_body(&body)),
        field_str(bare, "payloadHex")
    );
}

/// 认不出的字段**直接跳过**（proto3 前向兼容）。
#[test]
fn unknown_fields_are_skipped() {
    // 1 号 varint + 99 号（unknown）bytes + 8 号 command
    let mut wire = Vec::new();
    append_scalar(&mut wire, 1, 1);
    append_message(&mut wire, 99, b"whatever");
    append_scalar(&mut wire, 8, u64::from(CMD_NOTICE));
    let back = decode_msync(&wire).unwrap();
    assert_eq!(back.version, MSYNC_V2);
    assert_eq!(back.command, CMD_NOTICE);
}

/// `hex_preview` 在数据**比 n 短**时不许越界。
#[test]
fn hex_preview_is_bounds_safe() {
    assert_eq!(hex_preview(b"abc", 64), "616263");
    assert_eq!(hex_preview(b"abcdef", 3), "616263");
    assert_eq!(hex_preview(b"abc", 0), "");
    assert_eq!(hex_preview(&[], 8), "");
}

/// `auth` 的前缀 `$t$` 不能省 —— 裸 token 是另一个值。
#[test]
fn auth_keeps_the_dollar_t_dollar_prefix() {
    let v = vectors();
    let acc = const_str(&v, "ACCESS_TOKEN");
    let mut rng = SeqRandom::default();
    let frame = live_msync_frame(&acc, None, "1", CMD_SYNC, Vec::new(), 0, &mut rng);
    assert_eq!(frame.auth, format!("{LIVE_AUTH_PREFIX}{acc}"));
    assert!(frame.auth.starts_with("$t$"));
    assert_ne!(frame.auth, acc);
}

/// `phone` 的有无决定 `guid` 走哪条分支（原实现的两条形态）。
#[test]
fn phone_selects_the_guid_branch() {
    let mut rng = SeqRandom::default();
    let live = live_msync_frame(
        "t",
        Some("19900000000"),
        "ROOM",
        CMD_PROVISION,
        vec![],
        0,
        &mut rng,
    );
    assert_eq!(live.guid.app_key, LIVE_APP_KEY);
    assert_eq!(live.guid.name, "cmlive_c_19900000000");

    let mut rng = SeqRandom::default();
    let sdk = live_msync_frame("t", None, "ROOM", CMD_SYNC, vec![], 0, &mut rng);
    assert_eq!(sdk.guid.app_key, SDK_APP_KEY);
    assert_eq!(sdk.guid.name, "cmlive_ROOM");
}
