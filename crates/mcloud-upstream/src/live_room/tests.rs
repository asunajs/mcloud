//! 直播侧协议层的测试。
//!
//! # 判据来源优先级（本项目一贯口径）
//!
//! **实拍黄金向量 > 实拍文档逐字 > 原项目实现**
//!
//! `memberApplySignatures` 三条取自 `test/golden/live-room-vectors.json`
//! —— 那是**权威**，期望值一个字节都不许改。其余用例的响应文本逐字来自
//! `docs/capture/app-live-redflower-samples.txt` §5.2 / §5.3（2026-09-22 实拍）。

use super::*;
use serde_json::Value;

fn vectors() -> Value {
    // 用 `CARGO_MANIFEST_DIR` 而不是相对 `cwd`：测试运行的 cwd 不保证是
    // crate 根，相对路径会让这条测试在不同调用方式下时灵时不灵。
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/live-room-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

// ══════════════════════════ `member/apply` 签名（黄金向量）══════════════════════════

/// ⭐⭐ `memberApplySignatures` 三条**逐字**对拍 —— 权威判据。
///
/// 三条各自钉住一个边界：
///
/// | 用例 | 钉住的东西 |
/// |---|---|
/// | `plain` | 基准形状（13 位时间戳、65 字符 token、48 字节设备串） |
/// | `leading-zero-in-last10` | ⚠ 末 10 位**以 0 开头**：`1784700000001` → `4700000001` |
/// | `short-token` | ⚠ token **短于 10 字符**（`A03SHORT`，8 字符） |
///
/// # 为什么 `leading-zero-in-last10` 这条非有不可
///
/// 末 10 位里带前导零时，「把它当**整数**处理」的写法会把 `4700000001`
/// 变成 `4700000001`（看着一样）—— 但再往前一步、把中间结果过一遍
/// `parse::<i64>()` 再 `to_string()`，前导零就没了。
/// 更常见的是「取末 10 位」被写成「取后 10 个字符的数字部分」，
/// 于是 `0000000001` 那种时间戳（末 10 位**全是**前导零）会得到 `1`。
///
/// 这两种写法在 `plain` 上**都是绿的** —— 只有这条能抓住它们。
#[test]
fn golden_member_apply_signatures_match_all_three_vectors() {
    let v = vectors();
    let cases = v["memberApplySignatures"]
        .as_array()
        .expect("向量文件缺少 memberApplySignatures 数组");
    assert_eq!(cases.len(), 3, "黄金向量应当正好三条");

    for case in cases {
        let name = case["name"].as_str().unwrap_or("<无名>");
        let timestamp = case["timestamp"].as_str().expect("缺少 timestamp");
        let access_token = case["accessToken"].as_str().expect("缺少 accessToken");
        let device_info_hex = case["deviceInfoHex"].as_str().expect("缺少 deviceInfoHex");

        // ① 末 10 位
        assert_eq!(
            timestamp_suffix_10(timestamp),
            case["timestampSuffix10"].as_str().unwrap(),
            "[{name}] timestampSuffix10 对不上"
        );

        // ② systemMd5
        assert_eq!(
            system_md5(device_info_hex, timestamp).expect("systemMd5 不该失败"),
            case["systemMd5"].as_str().unwrap(),
            "[{name}] systemMd5 对不上"
        );

        // ③ tokenMd5
        assert_eq!(
            token_md5(access_token, timestamp),
            case["tokenMd5"].as_str().unwrap(),
            "[{name}] tokenMd5 对不上"
        );
    }
}

/// ⭐ `leading-zero-in-last10`：末 10 位**以 `0` 开头**，而且必须原样保留。
///
/// 这条单独再钉一遍（不与上一条共用循环），因为它是「前导零被吃掉」这类
/// 错误的**唯一**抓手 —— 上面那条循环里的断言一旦被人改成
/// 「`trim_start_matches('0')` 再比」就发现不了。
#[test]
fn golden_leading_zero_in_last_ten_digits_survives() {
    let v = vectors();
    let case = v["memberApplySignatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "leading-zero-in-last10")
        .expect("黄金向量里应当有 leading-zero-in-last10");

    let timestamp = case["timestamp"].as_str().unwrap();
    assert_eq!(timestamp, "1784700000001");

    let suffix = timestamp_suffix_10(timestamp);
    // ⚠ 逐字相等：取的是「末 **10 个字符**」这个字符串操作的结果。
    assert_eq!(suffix, "4700000001");
    assert_eq!(suffix.len(), 10, "末 10 位必须正好 10 个字符");
    assert!(suffix.starts_with('4'));
    // 完整时间戳与末 10 位的**长度差**必须是 3（13 位时间戳）。
    assert_eq!(timestamp.len() - suffix.len(), 3);
    // 末 10 位就是完整串的后缀（逐字，不是"数值相等"）。
    assert!(
        timestamp.ends_with(suffix),
        "末 10 位必须是原串的**后缀**：{timestamp} vs {suffix}"
    );
}

/// ⚠ 这条用例真正钉的东西：**末 10 位里含前导零**时的三种写法差异。
///
/// `leading-zero-in-last10` 那条的末 10 位（`4700000001`）本身**没有**前导零
/// —— 它的名字说的是「**完整时间戳**的末 10 位那一段中间有零」
/// （`1784**700000**001`）。
///
/// 所以前导零真正被吃掉的情形要用**构造**的输入来钉（见
/// [`suffix_ten_keeps_all_leading_zeros`]）。这里记录的是两者**不可互换**：
/// 拿 `parse::<i64>().to_string()` 去处理 `0000000001` 会得到 `"1"`，
/// 而 `4700000001` 走同样的路**看不出差别** —— 这就是为什么两条测试都要有。
#[test]
fn golden_leading_zero_case_differs_from_an_all_zero_suffix() {
    // 黄金向量那一条：整形化**看不出**问题
    let suffix = "4700000001";
    assert_eq!(suffix.parse::<i64>().unwrap().to_string(), suffix);

    // 而真正的前导零用例：整形化会吃掉 9 个零
    let all_zero = timestamp_suffix_10("1780000000001");
    assert_eq!(all_zero, "0000000001");
    assert_ne!(
        all_zero.parse::<i64>().unwrap().to_string(),
        all_zero,
        "整形化会吃掉前导零 —— 这正是「取末 10 个字符」不能写成数值操作的原因"
    );
}

/// ⚠ 一条**构造**的前导零用例（不是黄金向量）—— 把「末 10 位全是前导零」也钉住。
///
/// 黄金向量那一条只有**一个**前导零（`4700000001`）。若实现写成
/// 「去掉前导零」而不是「取末 10 个字符」，两个用例都会红；
/// 但若写成「`parse::<i64>()` 之后 `to_string()`」，只有**这条**会红 ——
/// 因为它才是真的会变短的那个。
#[test]
fn suffix_ten_keeps_all_leading_zeros() {
    // 末 10 位 = `0000000001`
    assert_eq!(timestamp_suffix_10("1780000000001"), "0000000001");
    assert_eq!(timestamp_suffix_10("1780000000001").len(), 10);
    // 末 10 位 = `0000000000`
    assert_eq!(timestamp_suffix_10("1780000000000"), "0000000000");
    // 全是零
    assert_eq!(timestamp_suffix_10("0000000000000"), "0000000000");
}

/// 时间戳**短于 10 位**时原样返回 —— 对齐 JS 的 `slice(-10)`。
///
/// 别加长度校验：那会把一次**协议差异**变成一个「参数错误」，
/// 而真正该看见的是**签名对不上**。
#[test]
fn suffix_ten_passes_short_input_through() {
    assert_eq!(timestamp_suffix_10("123"), "123");
    assert_eq!(timestamp_suffix_10(""), "");
    assert_eq!(timestamp_suffix_10("0123456789"), "0123456789"); // 正好 10 位
    assert_eq!(timestamp_suffix_10("01234567890"), "1234567890"); // 11 位
}

/// ⚠ `short-token`：token **短于 10 字符**时，JS 的 `slice(0,10)` 给整串、
/// `slice(10)` 给空串。
///
/// 直接切片的写法在 Rust 里会 **panic**，而「补零到 10 位」会算出一个
/// 与原客户端**不同**的串 —— 两种都不行。
#[test]
fn short_token_matches_js_slice_semantics() {
    let v = vectors();
    let case = v["memberApplySignatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "short-token")
        .expect("黄金向量里应当有 short-token");

    let token = case["accessToken"].as_str().unwrap();
    assert_eq!(token, "A03SHORT");
    assert!(token.len() < 10, "这条用例的 token 必须短于 10 字符");

    let timestamp = case["timestamp"].as_str().unwrap();
    assert_eq!(
        token_md5(token, timestamp),
        case["tokenMd5"].as_str().unwrap()
    );

    // ⚠ 短 token 的口径：`slice(0,10)` 给整串、`slice(10)` 给空串
    // ⇒ 拼接结果是 `"Bearer " + 整串token + ts10`（**没有** token 尾部）。
    let head = token; // 短 ⇒ 整串
    let tail = ""; // 短 ⇒ 空
    let expected = md5_hex_bytes(format!("Bearer {head}4790184674{tail}").as_bytes());
    assert_eq!(
        token_md5(token, timestamp),
        expected,
        "短 token 时时间戳应当插在**整串之后**（因为 head 就是整串、tail 是空）"
    );

    // 反证：若照「补零到 10 位」走，会得到一个与原客户端**不同**的串。
    let padded = format!("{token:0<10}");
    assert_ne!(padded, token, "补零会改掉 token");
    assert_ne!(
        token_md5(token, timestamp),
        md5_hex_bytes(format!("Bearer {padded}4790184674").as_bytes()),
        "补零到 10 位是错的口径 —— 黄金向量那条会红"
    );
}

/// 好 token（≥ 10 字符）与坏 token 的算法**不是**「都对整串求哈希」。
///
/// 若把 `token_md5` 写成 `md5("Bearer " + token + ts10)`（时间戳放**末尾**），
/// 黄金向量三条都会红 —— 这条只是把「时间戳插在第 10 个字符之后」这件事
/// 用一个**能读出来的**断言再钉一遍。
#[test]
fn token_md5_inserts_timestamp_after_the_tenth_char() {
    let token = "A03CBHoAAAAA19900000000SYNTHETICTOKENFORVECTORS000000000000000000";
    let timestamp = "1784790184674";
    let ts10 = "4790184674";

    let got = token_md5(token, timestamp);
    // 口径 A（正确）：`Bearer ` + token[..10] + ts10 + token[10..]
    let correct =
        md5_hex_bytes(format!("Bearer {}{ts10}{}", &token[..10], &token[10..]).as_bytes());
    // 口径 B：时间戳拼在**末尾**
    let tail_form = md5_hex_bytes(format!("Bearer {token}{ts10}").as_bytes());
    // 口径 C：只对 token 求哈希、不含时间戳
    let no_ts = md5_hex_bytes(format!("Bearer {token}").as_bytes());

    assert_eq!(got, correct);
    assert_ne!(got, tail_form, "时间戳不该拼在末尾");
    assert_ne!(got, no_ts, "签名里必须含时间戳");
}

/// `systemMd5` 的第一段是**解码后的 48 字节**，不是那 96 个 hex 字符。
///
/// 拿 hex 字面量（字符串）去算会得到一个**完全不同**的摘要，而它看起来
/// 一样「像 md5」。这条把两种口径分开。
#[test]
fn system_md5_hashes_the_decoded_bytes_not_the_hex_text() {
    let v = vectors();
    let case = &v["memberApplySignatures"][0];
    let hex_text = case["deviceInfoHex"].as_str().unwrap();
    let timestamp = case["timestamp"].as_str().unwrap();
    let suffix = timestamp_suffix_10(timestamp);

    let decoded = hex::decode(hex_text).unwrap();
    assert_eq!(decoded.len(), 48, "实拍设备串是 48 字节");

    // 正确：48 字节 ‖ 时间戳
    let mut payload = decoded.clone();
    payload.extend_from_slice(suffix.as_bytes());
    assert_eq!(
        system_md5(hex_text, timestamp).unwrap(),
        md5_hex_bytes(&payload)
    );

    // 错的口径：96 个 hex **字符** ‖ 时间戳
    let wrong = md5_hex_bytes(format!("{hex_text}{suffix}").as_bytes());
    assert_ne!(
        system_md5(hex_text, timestamp).unwrap(),
        wrong,
        "第一段必须是解码后的字节，不是 hex 字面量"
    );
}

/// ⚠ 设备串解码后的 48 字节里有**非 UTF-8** 序列 ⇒ 不能先转 `String`。
///
/// # 这条在防什么
///
/// `mcloud_crypto::md5_hex` 只收 `&str`。为了用它，最自然的写法是
/// `md5_hex(&String::from_utf8_lossy(&payload))` —— 而 `from_utf8_lossy` 会把
/// 非 UTF-8 字节**改写成 `U+FFFD`**，于是摘要不对，而且**不报错**。
///
/// 黄金向量的那一串恰好以 `0x83` 开头（续字节但不能单独成立），
/// 所以这条断言用**真向量**就能抓住它。
#[test]
fn system_md5_survives_non_utf8_device_bytes() {
    let v = vectors();
    let case = &v["memberApplySignatures"][0];
    let hex_text = case["deviceInfoHex"].as_str().unwrap();
    let timestamp = case["timestamp"].as_str().unwrap();

    let decoded = hex::decode(hex_text).unwrap();
    assert!(
        std::str::from_utf8(&decoded).is_err(),
        "这条用例的前提是设备串不是合法 UTF-8 —— 若不是，这条测试就失去意义了"
    );

    // 有损转换确实会改字节 —— 这就是不能走 `md5_hex(&str)` 的原因。
    let lossy = String::from_utf8_lossy(&decoded);
    assert_ne!(
        lossy.as_bytes(),
        decoded.as_slice(),
        "from_utf8_lossy 改写了字节"
    );

    // 正确实现走字节，所以与黄金向量相等。
    assert_eq!(
        system_md5(hex_text, timestamp).unwrap(),
        case["systemMd5"].as_str().unwrap()
    );
}

/// 非 hex 的 `deviceInfo` 要**报错**，不能静默算一个别的摘要。
#[test]
fn system_md5_rejects_non_hex_device_info() {
    // 实拍客户端的头值是 `other/unknown`（非 hex）—— 那个值**不能**喂给本函数。
    assert!(system_md5("other/unknown", "1784790184674").is_err());
    assert!(system_md5("zz", "1784790184674").is_err());
    // 奇数长度也是非法 hex
    assert!(system_md5("abc", "1784790184674").is_err());
}

// ══════════════════════════ `/cmlive` 信封 ══════════════════════════

/// 实拍 `roomBeat` 的响应（§5.2，逐字，删去无关 null 字段）送进信封。
const ROOM_BEAT_NOT_STARTED: &str = r#"{"rtnMsg":null,"rtnCode":"0","object":{
  "status":0,
  "channelId":134,
  "title":"中秋佳节云相伴！看直播赢好礼",
  "expectStartTime":"2026-09-23 14:30:00",
  "pullUrl":null,"chatRoomId":null,"startTime":null,"endTime":null,
  "anchor":{"id":1430,"authorId":56,"nickname":"中国移动云盘"},
  "activities":[]
}}"#;

/// ⭐⭐ `rtnCode` 是**字符串** `"0"`（实拍）⇒ 解析成功、`ok()` 为真。
#[test]
fn rtn_code_string_zero_is_success() {
    let env = CmliveEnvelope::parse(ROOM_BEAT_NOT_STARTED).expect("实拍响应应当解析成功");
    assert_eq!(env.code(), "0");
    assert!(env.ok());
    // 实拍里 `rtnMsg` 就是 `null` ⇒ 成功响应不带消息是**常态**。
    assert_eq!(
        env.message(),
        None,
        "rtnMsg 为 null 时应当是 None 而不是空串"
    );
}

/// ⭐⭐ **`rtnCode` 刻意只认字符串** —— 数字 `0` 必须**解析失败**。
///
/// # 这条测的不是「解析写严了」，是那盏报警灯还在不在
///
/// 「顺手把数字也转成字符串」是一行很自然的兜底。加上它之后：
///
/// - 今天的行为**看不出任何差别**（上游一直发字符串）；
/// - 而「上游把这一族改成了数字码」这个信号就**再也不会浮出来** ——
///   那是协议族整个换了一层的征兆，正是最该立刻看见的一类变化。
///
/// ⇒ 抹平类型差异的代价是**把报警灯拆掉**。所以这里断言的是
/// 「数字码必须炸」，而不是「数字码也能用」。
#[test]
fn rtn_code_refuses_numeric_code() {
    let numeric = r#"{"rtnCode":0,"rtnMsg":"success","object":{}}"#;
    let err = CmliveEnvelope::parse(numeric)
        .expect_err("数字 rtnCode 必须解析失败 —— 这一族用字符串码，抹平类型等于拆掉报警灯");
    let text = err.to_string();
    assert!(
        text.contains("不是字符串") && text.contains("数字"),
        "错误信息要说清「拿到的是数字、这一族要字符串」：{text}"
    );
    // 错误里要留下"这一族用字符串码"这条线索，否则看的人第一反应是"解析写严了"。
    assert!(
        text.contains("字符串码"),
        "错误信息要指向文件头那条约定：{text}"
    );
}

/// `rtnCode` 缺失 / 是 `null` / 是数组 —— 一律失败，且都指回 `rtnCode`。
#[test]
fn rtn_code_must_be_present_and_string_typed() {
    for body in [
        r#"{"rtnMsg":"x","object":{}}"#,
        r#"{"rtnCode":null,"rtnMsg":"x","object":{}}"#,
        r#"{"rtnCode":[],"rtnMsg":"x","object":{}}"#,
        r#"{"rtnCode":true,"rtnMsg":"x","object":{}}"#,
        // ⚠ `code`（签到族的键名）在这里**不算数** —— 两个信封不能互换。
        r#"{"code":0,"msg":"success","result":{}}"#,
    ] {
        assert!(
            CmliveEnvelope::parse(body).is_err(),
            "这些都必须失败：{body}"
        );
    }
}

/// ⚠ 拿**签到族的响应**喂给直播信封：必须失败，而不是静默取到空。
///
/// 这正是坑表 #108 的现场 —— Go 侧 `Envelope.Payload()` 取 `data` / `result`，
/// 喂给它 `/cmlive` 的响应会**静默返回 `(nil, false)` 且不报错**，
/// 表现为「登录成功却取不到 token」。
#[test]
fn signin_shaped_response_is_not_silently_accepted() {
    let signin_body = r#"{"code":0,"msg":"success","result":{"access_token":"X"}}"#;
    let err = CmliveEnvelope::parse(signin_body).expect_err("签到族的响应不该被直播信封接受");
    assert!(
        err.to_string().contains("rtnCode"),
        "错误要直接点出缺的是 rtnCode：{err}"
    );
}

/// 非 `"0"` 的字符串码 ⇒ `ok()` 为假，且错误里带上原文。
///
/// `-9999` 是 `getCmPassToken` 的实测值（原项目注释）；`"AUTH_FAIL"` 是
/// 一个**演示**用的非数字码 —— 它存在的唯一目的是说明：判据是
/// **与 `"0"` 字符串相等**，不是「能不能转成数字」。
#[test]
fn non_zero_rtn_code_is_failure_and_keeps_the_raw_code() {
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"-9999","rtnMsg":"token 失效","object":null}"#)
        .expect("字符串码 -9999 应当能解析出来（失败发生在 ok() 那一步）");
    assert!(!env.ok());
    assert_eq!(env.code(), "-9999");

    let err = env.ok_object("取 token").expect_err("非 0 码必须失败");
    let text = err.to_string();
    assert!(text.contains("-9999"), "错误里要带原始码：{text}");
    assert!(text.contains("token 失效"), "错误里要带上游消息：{text}");

    // 非数字码也一样（不用"能不能转数字"来判）。
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"AUTH_FAIL","rtnMsg":"","object":{}}"#).unwrap();
    assert!(!env.ok(), "非数字码不该被当成成功");
}

/// `object` 是**裸字符串**（`cmpassLoginByCmToken` 实测：就是那串 JWT 本身）。
#[test]
fn cmpass_login_object_is_the_bare_jwt() {
    let body = r#"{"rtnMsg":null,"rtnCode":"0","object":"<JWT 180 字符>"}"#;
    let env = CmliveEnvelope::parse(body).unwrap();
    assert_eq!(parse_cmpass_login(&env).unwrap(), "<JWT 180 字符>");

    // 空串 / 非字符串都要失败 —— 拿一个空凭据去开 WS 只会得到一次匿名连接。
    assert!(
        parse_cmpass_login(&CmliveEnvelope::parse(r#"{"rtnCode":"0","object":""}"#).unwrap())
            .is_err()
    );
    assert!(
        parse_cmpass_login(&CmliveEnvelope::parse(r#"{"rtnCode":"0","object":{}}"#).unwrap())
            .is_err()
    );
    assert!(parse_cmpass_login(&CmliveEnvelope::parse(r#"{"rtnCode":"0"}"#).unwrap()).is_err());
}

/// `userToken` 的解析：`access_token` 必填、`expires_in` 可选。
#[test]
fn user_token_is_parsed_from_object() {
    let body =
        r#"{"rtnMsg":null,"rtnCode":"0","object":{"access_token":"A03XXX","expires_in":5184000}}"#;
    let env = CmliveEnvelope::parse(body).unwrap();
    let token = parse_user_token(&env).unwrap();
    assert_eq!(token.access_token, "A03XXX");
    assert_eq!(token.expires_in, 5184000, "实拍是 5184000（60 天）");

    // `expires_in` 缺失不判失败（它只是提示）。
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"0","object":{"access_token":"A03"}}"#).unwrap();
    assert_eq!(parse_user_token(&env).unwrap().expires_in, 0);

    // `access_token` 缺失要失败。
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"0","object":{"expires_in":1}}"#).unwrap();
    assert!(parse_user_token(&env).is_err());
}

// ══════════════════════════ 请求形状 ══════════════════════════

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// 实拍 §5.3 钉住的三个基址与前缀。
#[test]
fn endpoints_use_the_measured_prefixes() {
    let jwt = "J";
    assert!(tab_list_request(jwt)
        .url
        .starts_with("https://v.cmcc-cs.cn/cmlive/api/"));
    assert!(tab_content_list_request(127, jwt)
        .url
        .starts_with("https://v.cmcc-cs.cn/cmlive/api/"));
    assert!(room_beat_request("147606", jwt)
        .unwrap()
        .url
        .contains("/cmlive/api/room/front/roomBeat"));
    assert!(current_user_info_request(jwt)
        .url
        .contains("/cmlive/api/user/auth/getCurrentUserInfoNew"));
    assert!(user_token_request(jwt)
        .unwrap()
        .url
        .contains("/cmlive/api/user/customer/userToken"));

    // ⚠ IM 是**另一个端口（20103）**，不是 WS 的 20102。
    let apply = apply_room_request("2026", "u", "cmlive", "T", "8306", "1784790184674").unwrap();
    assert!(
        apply
            .url
            .starts_with("https://videoim.cmcc-cs.cn:20103/v3/room/member/apply"),
        "端口写错 20102 拿到的是超时而不是错误码：{}",
        apply.url
    );
}

/// `/cmlive` 的头集合（实拍 §5.2）。
#[test]
fn cmlive_requests_carry_the_measured_header_set() {
    let req = tab_list_request("JWTVAL");
    assert_eq!(
        header(&req, "Accept"),
        Some("application/json, text/plain, */*")
    );
    assert_eq!(header(&req, "X-Requested-With"), Some("XMLHttpRequest"));
    assert_eq!(header(&req, "Accept-Language"), Some("zh-CN,zh-Hans;q=0.9"));
    assert_eq!(header(&req, "Origin"), Some(LIVE_ORIGIN));
    assert_eq!(header(&req, "User-Agent"), Some(LIVE_USER_AGENT));
    // 直播侧的凭据走标准 `Authorization` 头，**不是** `jwttoken`。
    assert_eq!(header(&req, "Authorization"), Some("JWTVAL"));
    assert_eq!(header(&req, "jwttoken"), None, "直播侧不用 jwttoken 头");
}

/// ⚠ 浏览器自己产生的头**不该**由构造器补。
///
/// `Cookie` / `Sec-Fetch-*` / `Connection` 是 WebView 的事；纯协议实现里
/// 既没有 cookie jar 也不受 fetch 元数据约束。`X-Tingyun` 是听云 APM 的
/// 采样串（含本机标识），我们**造不出真值** —— 伪造它既没有实测依据、
/// 又会往日志里多写一处设备标识。
#[test]
fn cmlive_requests_do_not_fake_browser_only_headers() {
    let reqs = [
        tab_list_request("J"),
        tab_content_list_request(127, "J"),
        room_beat_request("147606", "J").unwrap(),
        current_user_info_request("J"),
        user_token_request("J").unwrap(),
        cmpass_login_request("SSO").unwrap(),
    ];
    for req in reqs {
        for forbidden in [
            "Cookie",
            "Connection",
            "Sec-Fetch-Site",
            "Sec-Fetch-Mode",
            "X-Tingyun",
        ] {
            assert_eq!(
                header(&req, forbidden),
                None,
                "{forbidden} 不该由构造器补：{}",
                req.url
            );
        }
    }
}

/// `cmpassLoginByCmToken` 在这一步**还没有凭据** ⇒ 不能带 `Authorization`。
#[test]
fn cmpass_login_request_carries_no_authorization() {
    let req = cmpass_login_request("SSO-TOKEN").unwrap();
    assert_eq!(header(&req, "Authorization"), None);
    let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["cmpassToken"], "SSO-TOKEN");
}

/// `tabContentList` 的查询参数**顺序**与实拍逐字一致。
#[test]
fn tab_content_list_query_order_matches_the_capture() {
    let url = tab_content_list_request(127, "J").url;
    let qs = url.split('?').nth(1).expect("应当有查询串");
    assert_eq!(qs, "tabId=127&contentType=0&pageNo=1&channelId=134");
}

/// `tabList` 的 `channelId` 是查询参数。
#[test]
fn tab_list_query_matches_the_capture() {
    assert!(tab_list_request("J").url.ends_with("?channelId=134"));
}

/// `roomBeat` 的 body：`roomId` / `channelId` 是**字符串**，`sourceApp` 是数字。
///
/// ⚠ 原项目传的是**数字**。照实拍走：`roomId` 是 31 位大整数，
/// 过一遍 JS 的 Number 有精度风险，客户端把它写成字符串是**对的**。
#[test]
fn room_beat_body_matches_the_capture() {
    let req = room_beat_request("147606", "J").unwrap();
    let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["roomId"], "147606", "roomId 必须是字符串");
    assert!(body["roomId"].is_string(), "写成数字会丢精度");
    assert_eq!(body["channelId"], "134", "channelId 也必须是字符串");
    assert!(body["channelId"].is_string());
    assert_eq!(body["sourceApp"], 0);
    // `Referer` 指向**直播间页**，而且带 roomId。
    let referer = header(&req, "Referer").expect("roomBeat 要带 Referer");
    assert!(referer.contains("roomId=147606"), "{referer}");
}

// ══════════════════════════ 房间发现 ══════════════════════════

/// `tabList` 的解析：实拍两个页签 `127 盘盘福利` / `129 精彩回放`。
#[test]
fn tab_list_parses_the_two_measured_tabs() {
    let body = r#"{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[
      {"id":127,"name":"盘盘福利"},
      {"id":129,"name":"精彩回放"}
    ]}}"#;
    let tabs = parse_tab_list(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(tabs.len(), 2);
    assert_eq!(
        tabs[0],
        LiveTab {
            id: 127,
            name: "盘盘福利".into()
        }
    );
    assert_eq!(
        tabs[1],
        LiveTab {
            id: 129,
            name: "精彩回放".into()
        }
    );
}

/// ⭐⭐ **两个页签都要查** —— 这是刻意偏离原项目的那一处。
///
/// 依据是实拍 §5.3：`147606` **同时**出现在 127 与 129 里，
/// 而 `147608`（国庆场）**只在 129 里**。
///
/// ⇒ 只查 127 会在「房间还没进福利页签」时判成**没开播**，
/// 而那正是「漏掉整场口令」这一侧的错误。
///
/// ⚠ 而且这里连**上游没给这两个页签**的情况都要兜住：写死 id 的实现
/// 会在上游换页签编号时去查一个不存在的页签，而它回的是**空列表**
/// （不是错误）⇒ 表现成「今天没有直播」。
#[test]
fn both_tabs_are_queried_even_when_tab_list_omits_them() {
    // 上游给了完整两个
    let full = vec![
        LiveTab {
            id: 127,
            name: "盘盘福利".into(),
        },
        LiveTab {
            id: 129,
            name: "精彩回放".into(),
        },
    ];
    let ids = tab_ids_to_query(&full);
    assert!(ids.contains(&LIVE_TAB_ID_BENEFITS), "必须查 127：{ids:?}");
    assert!(ids.contains(&LIVE_TAB_ID_REPLAY), "必须查 129：{ids:?}");

    // 上游只给了 127（旧项目只查这个 ⇒ 147608 那类房间会被漏掉）
    let partial = vec![LiveTab {
        id: 127,
        name: "盘盘福利".into(),
    }];
    let ids = tab_ids_to_query(&partial);
    assert!(
        ids.contains(&LIVE_TAB_ID_REPLAY),
        "上游没给 129 也要兜底去查 —— 147608 只在 129 里：{ids:?}"
    );

    // 上游给了别的编号：既查它，也不丢那两个已知的
    let other = vec![LiveTab {
        id: 200,
        name: "新页签".into(),
    }];
    let ids = tab_ids_to_query(&other);
    assert_eq!(ids, vec![200, LIVE_TAB_ID_BENEFITS, LIVE_TAB_ID_REPLAY]);

    // 上游一个都没给（tabList 空）⇒ 仍然查那两个
    assert_eq!(
        tab_ids_to_query(&[]),
        vec![LIVE_TAB_ID_BENEFITS, LIVE_TAB_ID_REPLAY]
    );

    // 不重复
    let dup = vec![
        LiveTab {
            id: 129,
            name: "回放".into(),
        },
        LiveTab {
            id: 129,
            name: "回放".into(),
        },
    ];
    let ids = tab_ids_to_query(&dup);
    assert_eq!(
        ids.iter().filter(|id| **id == 129).count(),
        1,
        "去重：{ids:?}"
    );
}

/// ⚠ 字段类型是数字，但**数字字符串也要收**。
///
/// 实拍 §5.3 末尾记着这个坑：上游解码保号（`json.Number`）时，
/// 只写 `as_i64` 一种形态的取值助手会**静默返回 0** ——
/// 表现为「tab id 全是 0、拉到 0 间房」。
#[test]
fn numeric_fields_accept_numeric_strings() {
    let body = r#"{"rtnCode":"0","object":{"dataList":[{"id":"127","name":"盘盘福利"}]}}"#;
    let tabs = parse_tab_list(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(tabs[0].id, 127, "字符串形态的数字也要认，否则静默变成 0");

    // 房间那一侧同样
    let body = r#"{"rtnCode":"0","object":{"dataList":[
      {"room":{"id":"147606","status":"0","channelId":"134","title":"T",
               "anchor":{"id":"1430","nickname":"中国移动云盘"}}}
    ]}}"#;
    let rooms = parse_tab_content_list(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(rooms[0].id, 147606);
    assert_eq!(rooms[0].status, 0);
    assert_eq!(rooms[0].channel_id, 134);
}

/// ⚠ 房间嵌在 `dataList[].room` 里 —— 别按 `dataList[].id` 找房间。
///
/// 实拍 §5.3 那张表 16 行逐字抄进来：两个页签共 16 条，其中 147606 出现**两次**。
#[test]
fn tab_content_list_parses_the_measured_table() {
    // 逐字来自 §5.3 的表格（只保留判据字段）。
    let body = r#"{"rtnMsg":null,"rtnCode":"0","object":{"dataList":[
      {"room":{"id":147490,"status":1,"channelId":29,"expectStartTime":"2026-09-22 16:30","anchor":{"nickname":"福建移动"},"title":"折扣福利大流量"}},
      {"room":{"id":147473,"status":0,"channelId":51,"expectStartTime":"2026-09-22 18:00","anchor":{"nickname":"福建移动"},"title":"权益特惠购"}},
      {"room":{"id":147062,"status":0,"channelId":243,"expectStartTime":"2026-09-23 00:00","anchor":{"nickname":"移动心选"},"title":"爱购商城福利"}},
      {"room":{"id":147606,"status":0,"channelId":134,"expectStartTime":"2026-09-23 14:30","anchor":{"nickname":"中国移动云盘"},"title":"中秋佳节云相伴！看直播赢好礼"}},
      {"room":{"id":147608,"status":0,"channelId":134,"expectStartTime":"2026-09-29 14:30","anchor":{"nickname":"中国移动云盘"},"title":"国庆出游AI搞定攻略！赢出行好礼！"}},
      {"room":{"id":146662,"status":6,"channelId":134,"expectStartTime":"2026-08-12 14:30","anchor":{"nickname":"中国移动云盘"},"title":"云盘提效小妙招！下单赢办公礼！"}}
    ]}}"#;
    let rooms = parse_tab_content_list(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(rooms.len(), 6);
    assert_eq!(rooms[0].id, 147490);
    assert_eq!(rooms[0].status, LIVE_ROOM_STATUS_LIVE);
    assert_eq!(rooms[0].anchor_nickname, "福建移动");
    // ⚠ `channelId` 有 29 / 51 / 243 / 134 四种 ⇒ 它是**房间的归属**，
    // 不是"我拉的是哪个频道"。拿它当过滤条件会一间都筛不出来。
    let channels: Vec<i64> = rooms.iter().map(|r| r.channel_id).collect();
    assert_eq!(channels, vec![29, 51, 243, 134, 134, 134]);
    assert!(
        rooms.iter().any(|r| r.channel_id != 134),
        "实拍里就有非 134 的房间 —— 拿 channelId 过滤会一间都筛不出来"
    );
}

/// `dataList: null` 是**合法**的「这个页签没有房间」，不是解析错误。
#[test]
fn tab_content_list_null_datalist_is_empty_not_an_error() {
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"0","object":{"dataList":null}}"#).unwrap();
    assert!(parse_tab_content_list(&env).unwrap().is_empty());

    // 缺 `dataList` 键也回空表（同样是"没有房间"）。
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"0","object":{}}"#).unwrap();
    assert!(parse_tab_content_list(&env).unwrap().is_empty());

    // `dataList` 里没有 `room` 的条目要**跳过**，不是整条失败。
    let body = r#"{"rtnCode":"0","object":{"dataList":[
      {"type":1,"activity":{}},
      {"room":{"id":147606,"status":0,"anchor":{"nickname":"中国移动云盘"}}}
    ]}}"#;
    let rooms = parse_tab_content_list(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(rooms.len(), 1, "没有 room 那一层的条目要跳过");
    assert_eq!(rooms[0].id, 147606);
}

// ══════════════════════════ 三档 status 与门的判据 ══════════════════════════

/// ⭐⭐ 三档 `status` 的语义由实拍 §5.3 钉死。
#[test]
fn the_three_status_levels_are_distinct() {
    let mk = |status: i64| LiveRoom {
        id: 1,
        title: String::new(),
        status,
        channel_id: 134,
        expect_start_time: String::new(),
        anchor_id: 0,
        author_id: 0,
        anchor_nickname: KOULING_ANCHOR.to_string(),
    };
    let not_started = mk(LIVE_ROOM_STATUS_NOT_STARTED);
    let live = mk(LIVE_ROOM_STATUS_LIVE);
    let ended = mk(LIVE_ROOM_STATUS_ENDED);

    assert!(not_started.is_not_started() && !not_started.is_live() && !not_started.is_ended());
    assert!(live.is_live() && !live.is_not_started() && !live.is_ended());
    assert!(ended.is_ended() && !ended.is_live() && !ended.is_not_started());

    // ⚠ `0` 这一档是原项目**没记过**的（它只记了 1 与 6），
    // 而它恰好就是「今天抓不到 WS」的原因：`chatRoomId: null`。
    assert_eq!(LIVE_ROOM_STATUS_NOT_STARTED, 0);
    assert_eq!(LIVE_ROOM_STATUS_LIVE, 1);
    assert_eq!(LIVE_ROOM_STATUS_ENDED, 6);
    // 三档互不相等（写错一个常量不会报错，只会让门永远开或永远关）。
    assert_ne!(LIVE_ROOM_STATUS_NOT_STARTED, LIVE_ROOM_STATUS_LIVE);
    assert_ne!(LIVE_ROOM_STATUS_LIVE, LIVE_ROOM_STATUS_ENDED);
    assert_ne!(LIVE_ROOM_STATUS_NOT_STARTED, LIVE_ROOM_STATUS_ENDED);
}

fn room(id: i64, status: i64, nickname: &str) -> LiveRoom {
    LiveRoom {
        id,
        title: String::new(),
        status,
        channel_id: 134,
        expect_start_time: String::new(),
        anchor_id: 0,
        author_id: 0,
        anchor_nickname: nickname.to_string(),
    }
}

/// ⭐⭐ 门的判据是**两个条件都要**，而**昵称才是真正干活的**。
///
/// 实拍 §5.3 那张表钉死：`tabId=127` 里就有一间 `status=1` 的「福建移动」（147490）。
/// ⇒ 只看 `status === 1` 会**误判**成「有在播房间」，
/// 于是去听一个根本不会发口令的直播间。
#[test]
fn kouling_rooms_needs_both_status_and_exact_nickname() {
    let rooms = vec![
        // status=1 但**别人**在播 —— 只看 status 的话这条会被误选
        room(147490, LIVE_ROOM_STATUS_LIVE, "福建移动"),
        room(147473, LIVE_ROOM_STATUS_NOT_STARTED, "福建移动"),
        room(147062, LIVE_ROOM_STATUS_NOT_STARTED, "移动心选"),
        // ⚠ 下面这两个是**包含**关系的陷阱：都含「移动」，但不是精确相等
        room(147421, LIVE_ROOM_STATUS_LIVE, "移动心选"),
        room(147422, LIVE_ROOM_STATUS_LIVE, "移动臻选"),
        // 真正的目标：名字对，但现在没在播 ⇒ 门**不开**
        room(147606, LIVE_ROOM_STATUS_NOT_STARTED, KOULING_ANCHOR),
        room(146662, LIVE_ROOM_STATUS_ENDED, KOULING_ANCHOR),
        // 名字对且正在播 ⇒ 唯一的命中
        room(147600, LIVE_ROOM_STATUS_LIVE, KOULING_ANCHOR),
    ];
    let hit = kouling_rooms(&rooms);
    assert_eq!(hit.len(), 1, "只应命中那一间：{hit:?}");
    assert_eq!(hit[0].id, 147600);

    // ⚠ 三个「包含」陷阱：用 `contains` 判的实现会把它们全选进来。
    for name in ["福建移动", "移动心选", "移动臻选"] {
        assert!(
            !room(1, LIVE_ROOM_STATUS_LIVE, name).is_kouling_anchor(),
            "{name} 不该被当成那个主播 —— 判据是精确相等，不是包含"
        );
    }
    // 判据也不该是"昵称里含云盘"这种更松的东西
    assert!(!room(1, LIVE_ROOM_STATUS_LIVE, "中国移动云盘官方号").is_kouling_anchor());
    assert!(!room(1, LIVE_ROOM_STATUS_LIVE, " 中国移动云盘 ").is_kouling_anchor());
}

/// `anchor_rooms` 与 `kouling_rooms` **只差状态那一档**，用途完全不同。
///
/// - 门要「现在值不值得去听」⇒ 只算 `status == 1`；
/// - 界面要「下一场几点、上一场是否已结束」⇒ 预告与回放**都要留着**。
///
/// ⚠ 别拿 `anchor_rooms` 当门用：那会让界面在没开播时喊去打弹幕。
#[test]
fn anchor_rooms_keeps_all_statuses_but_is_not_a_gate() {
    let rooms = vec![
        room(147606, LIVE_ROOM_STATUS_NOT_STARTED, KOULING_ANCHOR),
        room(147608, LIVE_ROOM_STATUS_NOT_STARTED, KOULING_ANCHOR),
        room(146662, LIVE_ROOM_STATUS_ENDED, KOULING_ANCHOR),
        room(147490, LIVE_ROOM_STATUS_LIVE, "福建移动"),
    ];
    let all = anchor_rooms(&rooms);
    assert_eq!(all.len(), 3, "预告与回放都要留着");
    assert!(all.iter().all(|r| r.anchor_nickname == KOULING_ANCHOR));

    // 此刻没有一间在播 ⇒ **门不开**（尽管 anchor_rooms 非空）。
    assert!(
        kouling_rooms(&rooms).is_empty(),
        "没有 status==1 的房间时门必须关 —— 拿 anchor_rooms 当门会误判成在播"
    );
}

/// `merge_rooms` 按 id 去重，**先出现的赢**。
///
/// 实拍：147606 **同时**出现在 127 与 129 里 ⇒ 不去重就会对同一间房
/// 做两次 `roomBeat`、开两条 WS。
#[test]
fn merge_rooms_dedupes_by_id_first_wins() {
    let t127 = vec![
        room(147606, LIVE_ROOM_STATUS_NOT_STARTED, KOULING_ANCHOR),
        room(147490, LIVE_ROOM_STATUS_LIVE, "福建移动"),
    ];
    // 同一个 147606 在 129 里再出现一次（实拍如此），但换了个"状态"以便区分先后。
    let t129 = vec![
        room(147606, LIVE_ROOM_STATUS_LIVE, "改过的"),
        room(147608, LIVE_ROOM_STATUS_NOT_STARTED, KOULING_ANCHOR),
    ];
    let merged = merge_rooms(&[t127, t129]);
    assert_eq!(merged.len(), 3, "147606 只该留一条：{merged:?}");
    assert_eq!(merged[0].id, 147606);
    // 先出现的赢 ⇒ 保留的是 127 那一份（status=0），不是 129 那份。
    assert_eq!(merged[0].status, LIVE_ROOM_STATUS_NOT_STARTED);
    assert_eq!(merged.iter().filter(|r| r.id == 147606).count(), 1);

    // 空输入
    assert!(merge_rooms(&[]).is_empty());
    assert!(merge_rooms(&[vec![], vec![]]).is_empty());
}

// ══════════════════════════ roomBeat ══════════════════════════

/// ⭐ `roomBeat` 是**「这间房有没有聊天室」的唯一判据**（坑表 #107）。
///
/// 未开播时 `chatRoomId` 是 `null` ⇒ `member/apply` 与 WS **根本不会发**
/// —— 这是**结构性事实**，不是「抓漏了」。
///
/// ⚠ 这条用**实拍响应逐字**（§5.2 09:05:44 那条），它正是
/// 「今天抓不到 WS」的现场证据。
#[test]
fn room_beat_reports_no_chat_room_when_not_started() {
    let beat = parse_room_beat(&CmliveEnvelope::parse(ROOM_BEAT_NOT_STARTED).unwrap()).unwrap();
    assert_eq!(beat.status, LIVE_ROOM_STATUS_NOT_STARTED);
    assert_eq!(beat.title, "中秋佳节云相伴！看直播赢好礼");
    assert_eq!(beat.channel_id, 134);
    assert_eq!(beat.expect_start_time, "2026-09-23 14:30:00");
    assert_eq!(beat.anchor_nickname, KOULING_ANCHOR, "门的判据今天仍成立");

    // ⭐ `chatRoomId: null` ⇒ 没有聊天室 ⇒ 后面两步都不该发。
    assert_eq!(beat.chat_room_id, None);
    assert!(
        !beat.has_chat_room(),
        "未开播 ⇒ 没有聊天室 ⇒ WS 与 member/apply 都不该发（结构性事实，不是抓漏了）"
    );
}

/// 开播时 `chatRoomId` 是 31 位数字串 ⇒ `has_chat_room()` 为真。
#[test]
fn room_beat_reports_chat_room_id_when_live() {
    let body = r#"{"rtnMsg":null,"rtnCode":"0","object":{
      "status":1,"channelId":134,"title":"中秋佳节云相伴！看直播赢好礼",
      "chatRoomId":"2026072314302675201220000000000",
      "anchor":{"id":1430,"nickname":"中国移动云盘"}
    }}"#;
    let beat = parse_room_beat(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(beat.status, LIVE_ROOM_STATUS_LIVE);
    assert_eq!(
        beat.chat_room_id.as_deref(),
        Some("2026072314302675201220000000000")
    );
    assert!(beat.has_chat_room());
}

/// ⚠ `chatRoomId` 是**空串**时也要当作「没有」。
///
/// 空串进不了 WS（会拼出一个空的 `cmlive_…` 名字），
/// 而且它比 `null` 更容易被「有没有这个键」的判据放过去。
#[test]
fn room_beat_treats_empty_chat_room_id_as_absent() {
    let body = r#"{"rtnCode":"0","object":{"status":0,"chatRoomId":""}}"#;
    let beat = parse_room_beat(&CmliveEnvelope::parse(body).unwrap()).unwrap();
    assert_eq!(beat.chat_room_id, None);
    assert!(!beat.has_chat_room(), "空串与 null 一样：没有聊天室");
}

/// `object` 不是对象 / 缺失 ⇒ 报错，不是「当成没有聊天室」。
///
/// 两者后面的动作相反：解析失败该**停下来**看，而没有聊天室是
/// **正常的「今天没直播」**。
#[test]
fn room_beat_rejects_a_non_object_payload() {
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"0","object":"oops"}"#).unwrap();
    assert!(parse_room_beat(&env).is_err());

    // `object` 整个缺失时 `ok_object` 会报「响应没有 object」。
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"0"}"#).unwrap();
    assert!(parse_room_beat(&env).is_err());

    // 业务失败时优先报业务错误，而不是「没有 object」。
    let env = CmliveEnvelope::parse(r#"{"rtnCode":"-1","rtnMsg":"房间不存在"}"#).unwrap();
    let err = parse_room_beat(&env).unwrap_err();
    assert!(err.to_string().contains("房间不存在"), "{err}");
}

// ══════════════════════════ member/apply 请求与响应 ══════════════════════════

/// 实拍那 19 个头里**没有** `appkey`。
///
/// 原项目加了 `appkey: cmos/ngvlcs`，但那个值是从 apply 的**响应**
/// （`data.appKey`）里抄来的 —— 属于把响应字段误当成请求要求。
/// 多发一个头通常无害，但它会掩盖一个事实：我们**并不知道**服务端要不要它。
#[test]
fn apply_request_does_not_send_the_response_only_appkey() {
    let req = apply_room_request(
        "2026",
        "19900000000",
        "cmlive",
        "A03T",
        "83065208",
        "1784790184674",
    )
    .unwrap();
    assert_eq!(
        header(&req, "appkey"),
        None,
        "appkey 只在响应里，不该当请求头发"
    );

    // 实拍的头都要在。
    assert_eq!(header(&req, "Accept"), Some("*/*"));
    assert_eq!(header(&req, "Version"), Some(LIVE_APPLY_VERSION));
    assert_eq!(header(&req, "timestamp"), Some("1784790184674"));
    assert_eq!(header(&req, "Authorization"), Some("Bearer A03T"));
    assert_eq!(header(&req, "Origin"), Some(LIVE_ORIGIN));
    assert_eq!(header(&req, "User-Agent"), Some(LIVE_USER_AGENT));
    assert_eq!(
        header(&req, "deviceInfo"),
        Some("83065208"),
        "头里是 hex 字面量"
    );
    assert!(header(&req, "systemMd5").is_some());
    assert!(header(&req, "tokenMd5").is_some());
}

/// ⚠ `Version` 取自**原项目**（`v23.0.2`），不是实拍。
///
/// 抓包里这个头被脱敏成 `<REDACTED>`。这条断言的作用是**把这处弱点标出来**：
/// 它一旦变了，红的是这条测试，而不是「进房间忽然失败」。
#[test]
fn apply_version_is_the_unverified_project_value() {
    assert_eq!(LIVE_APPLY_VERSION, "v23.0.2");
    // 与 PROVISION 的 `23.0.2` 只差一个 `v` 前缀 —— 若哪天有人"统一"它们，
    // 这条会红（而统一之后到底哪个对，我们并**没有**证据）。
    assert_eq!(LIVE_APPLY_VERSION.trim_start_matches('v'), "23.0.2");
}

/// `member/apply` 的 body 必须含 `message`（原项目漏了它）。
#[test]
fn apply_request_body_matches_the_capture() {
    let req = apply_room_request(
        "2026072314302675201220000000000",
        "19900000000",
        "cmlive",
        "A03T",
        "83065208",
        "1784790184674",
    )
    .unwrap();
    let body: Value = serde_json::from_str(req.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["roomId"], "2026072314302675201220000000000");
    assert_eq!(body["userName"], "19900000000");
    assert_eq!(body["invoker"], "cmlive");
    // ⚠ 原项目发的是 `{roomId, userName, invoker}`，实拍多了 `message: ""`。
    assert_eq!(
        body["message"], "",
        "实拍 body 里有 message（空串），原项目漏了它"
    );
    // 四个键都要在，且**没有**多出来的键。
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 4, "实拍正好四个字段：{keys:?}");
}

/// ⚠ `member/apply` 的响应走的是**另一套信封**（数字码 + `data`）。
///
/// 与 `/cmlive` 的 `rtnCode` + `object` **不是一套** ——
/// 别「顺手统一」成 [`CmliveEnvelope`]，那会让 `data` 永远取不到。
#[test]
fn apply_response_uses_the_numeric_code_envelope() {
    let body = r#"{"code":0,"msg":"接口调用成功","data":{
      "appId":"cmos_im","appKey":"cmos/ngvlcs",
      "roomId":"2026072314302675201220000000000"
    },"timestamp":1784790185000}"#;
    let result = parse_apply_room(body).unwrap();
    assert_eq!(result.app_id, "cmos_im");
    // ⭐ 响应里的 `appKey` —— 原项目就是从这里抄去当**请求头**的。
    assert_eq!(result.app_key, "cmos/ngvlcs");
    assert_eq!(result.room_id, "2026072314302675201220000000000");
    assert_eq!(result.server_timestamp, 1784790185000);
}

/// `code != 0` ⇒ 失败，错误里带码与原消息。
#[test]
fn apply_response_with_non_zero_code_fails() {
    let err = parse_apply_room(r#"{"code":400,"msg":"房间不存在"}"#).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("400"), "{text}");
    assert!(text.contains("房间不存在"), "{text}");
}

/// `data` 缺失**不判失败** —— 进房间这个动作本身已经成功了。
#[test]
fn apply_response_without_data_still_succeeds() {
    let result = parse_apply_room(r#"{"code":0,"msg":"接口调用成功","timestamp":1}"#).unwrap();
    assert_eq!(
        result,
        ApplyRoomResult {
            app_id: String::new(),
            app_key: String::new(),
            room_id: String::new(),
            server_timestamp: 1,
        }
    );
}

/// 房间 id 的字符串形态 —— **不过 `f64`**。
///
/// # ⚠ 31 位的聊天室 id 根本装不进 `i64`
///
/// 实拍那个**房间** id `147606` 是 6 位；而**聊天室** id（`chatRoomId`）是
/// `2026072314302675201220000000000` —— **31 位**，远超 `i64` 的上限
/// （约 `9.2e18`，19 位）。
///
/// 这正是 `roomBeat` 的 `roomId` **必须传字符串**的原因（实拍 §5.2）：
/// 它过一遍 JS 的 `Number` 就会丢精度，过一遍 `i64` 会**根本装不下**。
///
/// ⇒ 所以要分清两个 id 的类型归属：
/// **房间 id** 走 `i64`（[`live_room_id_string`] 负责转字符串），
/// 而**聊天室 id 全程是 `String`**（[`RoomBeat::chat_room_id`] 就是
/// `Option<String>`，不是数字类型）。
#[test]
fn room_id_string_keeps_full_precision() {
    // 实拍那个房间 id（6 位）。
    assert_eq!(live_room_id_string(147606), "147606");

    // 19 位已经在 `i64` 的极限附近 —— 而它超过了 `f64` 的 2^53
    // 精确整数范围（`9007199254740992`）。走一遍 `as f64` 会**静默**变数。
    let big = 9_007_199_254_740_993_i64; // 2^53 + 1
    let text = live_room_id_string(big);
    assert_eq!(text, "9007199254740993");
    assert_ne!(
        text,
        (big as f64).to_string(),
        "过 f64 会丢精度（2^53+1 会变成 2^53）—— 这就是要收敛到一个函数的原因"
    );

    // ⚠ 而**聊天室 id**（31 位）根本进不了 `i64`：它全程走 `String`。
    // 下面这行若取消注释会**编译不过** —— 这正是要记住的边界。
    //     let _: i64 = 2026072314302675201220000000000;
    let chat_room_id = "2026072314302675201220000000000";
    assert_eq!(chat_room_id.len(), 31);
    assert!(chat_room_id.parse::<i64>().is_err(), "31 位装不进 i64");
}

// ══════════════════════════ 常量归属 ══════════════════════════

/// 直播侧的 `targetSourceId` 与小红花侧**不是一个值**。
///
/// 用 `001005` 去打车直播这条链会一条都拿不到 —— 而它看起来只是
/// 「今天没有直播」。这条把两边的归属钉在**使用点**上。
#[test]
fn live_side_uses_the_live_target_source_id() {
    let live_source_id = crate::live_consts::LIVE_SOURCE_ID;
    assert_eq!(live_source_id, "017027");
    assert_ne!(live_source_id, crate::live_consts::TO_SOURCE_ID);
    // `roomBeat` 的 Referer 用的是**直播侧**那一份。
    let referer = header(&room_beat_request("147606", "J").unwrap(), "Referer")
        .unwrap()
        .to_string();
    assert!(referer.contains(&format!("targetSourceId={live_source_id}")));
    assert!(!referer.contains(crate::live_consts::TO_SOURCE_ID));
}
