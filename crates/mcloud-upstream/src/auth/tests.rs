//! 凭据串解析的**黄金向量对拍**。
//!
//! 向量来自 `mcloud/test/golden/auth-vectors.json`（原版编译产物的实测对拍，
//! 由 `../mcloud-rust/scripts/gen-auth-vectors.cjs` 生成），本仓已同步。
//!
//! # 为什么必须对拍而不是自己写几条
//!
//! `parse_auth` 有三个"照直觉写就错"的地方（只替换一次 `Basic `、
//! 只取前 3 段、`Number(x) || 0`），它们全都体现在**边界样本**上：
//! `colon-in-token-truncated`、`lowercase-basic-prefix`、`token-expire-float`……
//! 自己编几条正常样本，三个坑一个都测不出来。

use super::*;
use serde_json::Value;

/// 向量文件相对**本 crate**的位置：`crates/mcloud-upstream/../../test/golden/`。
///
/// 用 `CARGO_MANIFEST_DIR` 而不是相对 `cwd`：测试运行的 cwd 不保证是 crate 根，
/// 相对路径会让这条测试在不同调用方式下时灵时不灵。
fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/auth-vectors.json"
    );
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读不到向量文件 {path}：{e}"));
    serde_json::from_str(&text).expect("向量文件不是合法 JSON")
}

/// 逐条对拍全部 22 个 `parseAuth` 用例。
///
/// # 三类用例的处理（向量自带的约定）
///
/// | 标记 | 要求 |
/// |---|---|
/// | `expect: null` | 必须返回 `None` |
/// | `malformed: true` | **只要求判为不可用**，不要求复刻乱码 |
/// | 正常 | 五个字段逐字对拍 |
///
/// 中间那一类是刻意的取舍：原版对病态输入会产出一个 `platform` 是乱码的
/// 对象（属实现缺陷）。向量文件的 `$note` 明说"不应继承"，所以我们判为
/// 不可用 —— 一条 platform 是乱码的凭据没有任何使用场景。
#[test]
fn parse_auth_matches_every_golden_vector() {
    let v = vectors();
    let cases = v["parseAuth"].as_array().expect("parseAuth 不是数组");
    assert_eq!(cases.len(), 22, "向量条数变了，先确认是哪一侧改动");

    let mut ok_count = 0;
    let mut null_count = 0;
    let mut malformed_count = 0;

    for case in cases {
        let name = case["name"].as_str().unwrap_or("(无名)");
        let input = case["input"].as_str().expect("input 不是字符串");
        let got = parse_auth(input);

        // ① expect 为 null ⇒ 必须 None
        if case["expect"].is_null() {
            assert!(got.is_none(), "用例 {name}：原版返回 null，我们应返回 None");
            null_count += 1;
            continue;
        }

        // ② 病态输入 ⇒ 只要求判为不可用
        if case["malformed"].as_bool().unwrap_or(false) {
            assert!(
                got.is_none(),
                "用例 {name}：病态输入应判为不可用（原版产出乱码 platform），实得 {got:?}"
            );
            malformed_count += 1;
            continue;
        }

        // ③ 正常用例：五个字段逐字对拍
        let expect = &case["expect"];
        let got = got.unwrap_or_else(|| panic!("用例 {name}：应当解出结果，实得 None"));

        // 缺失的键等价于空串（上游的串确实可能缺段）
        let want = |k: &str| expect.get(k).and_then(Value::as_str).unwrap_or("");

        assert_eq!(got.raw, want("auth"), "用例 {name}：auth 不一致");
        assert_eq!(got.phone, want("phone"), "用例 {name}：phone 不一致");
        assert_eq!(got.token, want("token"), "用例 {name}：token 不一致");
        assert_eq!(
            got.platform,
            want("platform"),
            "用例 {name}：platform 不一致"
        );
        // ⚠ 这一条是**单位断言**：向量的 token 第 4 段是 13 位毫秒，
        // 按秒解释会差 1000 倍（见 `AuthInfo::expire` 的说明）。
        //
        // 向量里有一条 `1.5`（小数毫秒）。本仓的 `expire` 是 `i64`，
        // 按 **截断**处理，与 Go 参考的 `ExpireMillis()`（`int64(a.Expire)`）
        // 一致；小数毫秒在真实 token 里不存在，为它把字段改成 `f64`
        // 会动到已落盘的 `accounts.json` 形态，代价不对等。
        let want_expire = expect["expire"].as_f64().map(|f| f as i64).unwrap_or(0);
        assert_eq!(
            got.expire, want_expire,
            "用例 {name}：expire 不一致（向量原值 {:?}）",
            expect["expire"]
        );
        ok_count += 1;
    }

    assert_eq!(ok_count + null_count + malformed_count, cases.len());
    // 三类都必须真的出现过，否则这条测试可能在某类上变成了空转
    assert!(ok_count >= 15, "正常用例太少：{ok_count}");
    assert!(null_count >= 1, "没有 null 用例");
    assert!(malformed_count >= 1, "没有病态用例");
}

/// 向量锁住的单位就是**毫秒** —— 单独再钉一次，防止有人"顺手"改成秒。
#[test]
fn golden_vectors_confirm_expire_is_milliseconds() {
    let v = vectors();
    let case = v["parseAuth"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "plain")
        .expect("缺 plain 用例");

    let expire = case["expect"]["expire"].as_i64().unwrap();
    assert!(
        expire > 1_000_000_000_000,
        "向量的 expire 应是 13 位毫秒，实得 {expire}"
    );
    // 解析结果必须与向量同量级
    let got = parse_auth(case["input"].as_str().unwrap()).unwrap();
    assert_eq!(got.expire, expire);
    assert!(got.expire > 1_000_000_000_000, "解析必须保留毫秒量级");
}

/// `padBase64` 的行为也要对拍 —— 上游经常给缺填充的串。
#[test]
fn pad_base64_matches_golden_vectors() {
    let v = vectors();
    let cases = v["padBase64"].as_array().expect("padBase64 不是数组");
    let mut checked = 0;
    for case in cases {
        let input = case["input"].as_str().expect("input 不是字符串");
        let expect = case["expect"].as_str().expect("expect 不是字符串");
        assert_eq!(pad_base64(input), expect, "padBase64({input:?}) 不一致");
        checked += 1;
    }
    assert!(checked > 0);
}

// ══════════════════════════ 拼装与掩码 ══════════════════════════

/// 拼装必须能被自己的解析器读回（往返）。
#[test]
fn format_then_parse_round_trips() {
    for prefix in [PC_AUTH_PREFIX, APP_AUTH_PREFIX] {
        let auth = format_auth(prefix, "19900000001", "ak|bk|ck|1893456000000|fk").unwrap();
        let parsed = parse_auth(&auth).unwrap();
        assert_eq!(parsed.platform, prefix, "前缀必须原样保留");
        assert_eq!(parsed.phone, "19900000001");
        assert_eq!(parsed.token, "ak|bk|ck|1893456000000|fk");
        assert_eq!(parsed.expire, 1_893_456_000_000);
        assert_eq!(parsed.raw, auth);
    }
}

/// 前缀是必填参数：不能靠默认值静默变成 `pc`。
#[test]
fn format_auth_requires_explicit_prefix_and_non_empty_parts() {
    // 两种前缀产出不同的串 —— 这正是"必须显式选"的理由
    let pc = format_auth(PC_AUTH_PREFIX, "19900000001", "t").unwrap();
    let app = format_auth(APP_AUTH_PREFIX, "19900000001", "t").unwrap();
    assert_ne!(pc, app);
    assert_eq!(parse_auth(&pc).unwrap().platform, "pc");
    assert_eq!(parse_auth(&app).unwrap().platform, "mobile");

    // 缺账号或 token 必须报错
    assert!(format_auth(PC_AUTH_PREFIX, "", "t").is_err());
    assert!(format_auth(PC_AUTH_PREFIX, "19900000001", "").is_err());
}

/// PC 前缀**不区分** mac / win —— 这是平台必须单独落盘的根本原因。
#[test]
fn pc_prefix_is_identical_for_mac_and_windows() {
    // 两个平台拼出来的 auth 串逐字相同：光看凭据分不出来
    let a = format_auth(PC_AUTH_PREFIX, "19900000001", "tok").unwrap();
    let b = format_auth(PC_AUTH_PREFIX, "19900000001", "tok").unwrap();
    assert_eq!(a, b);

    // 因此 parse_auth 的 platform 字段两种平台都是 "pc"
    assert_eq!(parse_auth(&a).unwrap().platform, "pc");
    assert_eq!(parse_auth(&b).unwrap().platform, "pc");
}

#[test]
fn mask_phone_matches_reference_shape() {
    assert_eq!(mask_phone("19900000001"), "19****01");
    assert_eq!(mask_phone("13800138000"), "13****00");
    // 短号原样返回（掩码会让"这是脏数据"这件事消失）
    assert_eq!(mask_phone("123"), "123");
    assert_eq!(mask_phone(""), "");
}

#[test]
fn account_label_prefers_phone_then_nickname_then_placeholder() {
    assert_eq!(account_label(Some("昵称"), "19900000001"), "19****01");
    assert_eq!(account_label(Some("昵称"), ""), "昵称");
    assert_eq!(account_label(None, ""), "account");
    assert_eq!(account_label(Some(""), ""), "account");
}

/// 解析空串与非 base64 必须回 `None` 而不是 panic。
#[test]
fn parse_auth_rejects_empty_and_non_base64() {
    assert!(parse_auth("").is_none());
    assert!(parse_auth("!!!not-base64!!!").is_none());
}
