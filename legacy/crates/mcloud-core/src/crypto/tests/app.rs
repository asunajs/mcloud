// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

// ── App 端（MCLoginCryptoUtil）：AES-ECB 加密 + 末 10 位错位 ──

/// App 端登录体的 AES 密钥（`MCLoginCryptoUtil +AESEncrypt:` 的
/// `isCurrentEnvironment = YES` 分支，反汇编实证）。
const APP_KEY: &str = "lYzvvBMUjucw+h^R";

/// 抓包里那条 `thirdlogin` 明文的**脱敏**形态（真号码换成 `199` 号段）。
/// 长度 186 与实测一致 —— 这是"形状没变"的硬约束。
const APP_LOGIN_PLAINTEXT: &str = concat!(
    r#"{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344","#,
    r#""pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","#,
    r#""msisdn":"19900000000","random":"hOjseB","clienttype":"412"}"#,
);

#[test]
fn app_login_plaintext_keeps_the_captured_length() {
    // 186 字节 → PKCS7 补 6 → 192 字节 → 384 个 hex 字符。
    // 这三个数都直接来自抓包（docs/capture/app-sms-login-samples.txt §6.1），
    // 一旦有人"顺手"改了字段，长度会先炸。
    assert_eq!(APP_LOGIN_PLAINTEXT.len(), 186);
}

#[test]
fn app_login_body_is_ecb_hex_with_the_tail_ten_moved_to_the_front() {
    // 期望值由本仓库之外、独立实现的 AES-ECB 算出（抓包侧的对照脚本），
    // 不是 Rust 侧自产自销：它同时钉住了 ECB 模式、PKCS7 填充、密钥、hex 小写，
    // 以及**错位方向**（末 10 位到最前，不是反过来）。
    let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap();
    assert_eq!(ecb.len(), 384);
    let wire = shift_tail10_to_front(&ecb);
    assert_eq!(
        wire,
        "2584e02d73b549d837f9fa6e4bdd4968c816497fba1be0770aeba9d7249a928db623ee6b11f26284588aa8dd943fdca3a5e7d21a36fdc347dbf16e1a2d0d55f23c77b1409ad5afff5600844ded9a613635003471e71be8e07b9b7fb8bbd14639e90ffa9716c22fb01670ee5b4b208efc0d59a0b16c087688ad5fd68561af8860b1e1d93ad7d00150cdb6bac04a3cc792a05347c7813afd2e06ca996da52959d7bc92881e4584c088cf7b01dc35341927e92c11d7e66e76c1625228a8d84c0eb0"
    );

    // 错位方向反了会得到完全不同的串 —— 单独钉一条，避免"看起来也像随机 hex"。
    assert_ne!(wire, shift_head10_to_back(&ecb));
    assert_eq!(
        &wire[..10],
        &ecb[ecb.len() - 10..],
        "前 10 位来自 ECB 密文的末 10 位"
    );
    assert_eq!(&wire[10..], &ecb[..ecb.len() - 10]);
}

#[test]
fn app_login_shift_is_its_own_inverse_on_real_lengths() {
    let wire =
        shift_tail10_to_front(&aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap());
    assert_eq!(shift_head10_to_back(&wire).len(), 384);

    // 往返：加密 → 错位 → 反错位 → 解密，必须回到原文
    let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap();
    let back = shift_head10_to_back(&shift_tail10_to_front(&ecb));
    assert_eq!(back, ecb);
    assert_eq!(
        aes_ecb_pkcs7_decrypt_hex(APP_KEY, &back).unwrap(),
        APP_LOGIN_PLAINTEXT
    );
}

#[test]
fn app_login_shift_is_identity_for_short_strings() {
    // 客户端是 `p = MAX(len, 10) - 10`：长度 ≤ 10 时两段拼回来等于原串。
    // 这里照抄行为而不是"顺手校验长度"—— 上游真回了个短串时，
    // 抛错会让调用方拿到一个和客户端不一致的结果。
    for s in ["", "a", "0123456789", "short"] {
        assert_eq!(shift_tail10_to_front(s), s, "{s}");
        assert_eq!(shift_head10_to_back(s), s, "{s}");
    }
    // 恰好 11 位：末 10 位（`123456789A`）到最前，首字符 `0` 落到末尾
    assert_eq!(shift_tail10_to_front("0123456789A"), "123456789A0");
    // 反过来：末字符 `A` 提到最前，前 10 位顺次后移
    assert_eq!(shift_head10_to_back("0123456789A"), "A0123456789");
    // 两者互为逆运算 —— 这是"错位方向没写反"的判据。
    // （n = 11 时两个方向在字面上会重合，所以只断言"往返回到原值"才判得准。）
    assert_eq!(
        shift_head10_to_back(&shift_tail10_to_front("0123456789A")),
        "0123456789A"
    );
    assert_eq!(
        shift_tail10_to_front(&shift_head10_to_back("0123456789A")),
        "0123456789A"
    );
}

#[test]
fn app_login_wrong_key_is_what_9103_looks_like() {
    // 活体探针的反面：换一把 key（App 的备用 key）加密，上游会回 `9103 输入参数不合法`。
    // 本测试不需要联网，它只钉住"两把 key 产出的密文确实不同"这件事 ——
    // 这样"密钥选错"在单测层面就能被发现，而不是要等一次真实请求。
    let alt = aes_ecb_pkcs7_encrypt_hex("+09T5lHhO+smS7x@", APP_LOGIN_PLAINTEXT).unwrap();
    let prod = aes_ecb_pkcs7_encrypt_hex(APP_KEY, APP_LOGIN_PLAINTEXT).unwrap();
    assert_ne!(alt, prod);
    // 用错 key 去解会**明确失败**（PKCS7 非法），而不是静默产出半个凭据
    assert!(aes_ecb_pkcs7_decrypt_hex("+09T5lHhO+smS7x@", &prod).is_err());
}

#[test]
fn app_login_encrypt_length_rule_matches_the_capture() {
    // hex_len = 32 * ceil((pt_len + 1) / 16)：抓包里 186 → 384，这条规律写死，
    // 免得有人把 PKCS7 换成"零填充"（那样 186 字节会变成 192 但末块内容不同，
    // 长度看不出来 —— 所以长度只是必要条件，不是充分条件，真正的判据是上面的密文相等）。
    for pt_len in [1usize, 7, 15, 16, 17, 186, 200] {
        let text = "x".repeat(pt_len);
        let hex = aes_ecb_pkcs7_encrypt_hex(APP_KEY, &text).unwrap();
        let expect_blocks = (pt_len + 1).div_ceil(16);
        assert_eq!(
            hex.len(),
            expect_blocks * 32,
            "明文 {pt_len} 字节时密文长度不对"
        );
    }
}

// ── App 业务接口（形态 B）：`signature` / `encodeData` / `hcy-cool-flag` 整包信封 ──
//
// 这三条都吃 test/golden/app-business-crypto-vectors.json。常量**从向量文件里读**，
// 不在这里抄一遍 —— 否则"改了向量没改代码"两边会一起漂。

#[test]
fn app_signature_field_order_is_rebuilt_from_the_captured_input() {
    // 逐条把 `clientId=…&clientKey=…&msgId=…&timeStamp=…&version=…` 拼出来与实拍串比对：
    // 这一条锁的是**字段顺序**，不是哈希算法。顺序错了哈希必错，
    // 但"哈希对了"不足以说明顺序对（也可能是向量抄错），所以两件事分开断言。
    let v = app_business_vectors();
    let c = &v["constants"];
    let (cid, ckey) = (
        c["APP_CLIENT_ID"].as_str().unwrap(),
        c["APP_CLIENT_KEY"].as_str().unwrap(),
    );

    let cases = v["signatureSha256"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input = case["input"].as_str().unwrap();
        let rebuilt = format!(
            "clientId={cid}&clientKey={ckey}&msgId={}&timeStamp={}&version={}",
            case["msgId"].as_str().unwrap(),
            case["timeStamp"].as_str().unwrap(),
            case["version"].as_str().unwrap(),
        );
        assert_eq!(rebuilt, input, "{name}: 字段顺序/取值与实拍串不一致");
        assert_eq!(
            sha256_hex(input.as_bytes()),
            case["expect"].as_str().unwrap(),
            "{name}"
        );
        // ⚠ `version` 是**按请求头原样代入**（带 V），不是把 `V1.0` 写死。
        assert!(case["version"].as_str().unwrap().starts_with('V'), "{name}");
    }
}

#[test]
fn app_encode_data_is_a_16_ascii_iv_prefix_not_a_binary_one() {
    // `encodeData` 与整包信封**长得像但不是同一个约定**：这里的前 16 字节必须是
    // 可打印 ASCII（实拍 6158/6158 都是），整包信封那边是二进制。
    // 这条断言存在的意义就是"别把两者互相套用"。
    let v = app_business_vectors();
    let key = v["constants"]["APP_CLIENT_KEY"].as_str().unwrap();
    for case in v["encodeDataDecrypt"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let payload = case["encodeData"].as_str().unwrap();
        let raw = b64_decode(payload).unwrap();
        assert_eq!(raw.len(), 32, "{name}: 16 IV + 16 密文（手机号一块就够）");
        assert_eq!(
            &raw[..16],
            case["ivAscii"].as_str().unwrap().as_bytes(),
            "{name}: 前置 IV 必须是那 16 个 ASCII 字符"
        );
        assert_eq!(
            aes_dynamic_iv_decrypt(key, payload).unwrap(),
            case["plaintext"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn app_cool_flag_body_envelope_192_decrypts_every_real_capture() {
    // **这是 App 整包信封的真钥匙**（2026-09-20 定案）。
    // 证据：旧项目 `mcloud-sign/har/` 里 396 条实拍（`Authorization` 前缀 `mobile` ⇒ App 流量）
    // 逐条解开、零例外；同一批条目上 AES-128 那把 **0 命中**。
    let v = app_business_vectors();
    let key = v["constants"]["BODY_AES_KEY_192"].as_str().unwrap();

    // 钥匙字面量与旧项目 `packages/core/src/utils/encrypt.ts:3` 的 `REFRESH_TOKEN_KEY` 同一把，
    // 长度 24 ⇒ **AES-192**。这两条一起断言，免得有人"顺手"把它归一化成 16 字节。
    assert_eq!(key.len(), 24, "24 字节 ⇒ AES-192，不是 AES-128");
    assert_eq!(key, REFRESH_TOKEN_KEY);

    let cases = v["hcyCoolFlagBodyEnvelope192"].as_array().unwrap();
    assert!(cases.len() >= 5, "至少要有请求与响应两个方向的实拍");
    let mut directions = std::collections::BTreeSet::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let payload = case["payloadB64"].as_str().unwrap();
        directions.insert(case["direction"].as_str().unwrap().to_string());

        let raw = b64_decode(payload).unwrap();
        assert_eq!(
            hex::encode(&raw[..16]),
            case["ivHex"].as_str().unwrap(),
            "{name}: 前置 IV 与 ivHex 不一致"
        );
        assert_eq!((raw.len() - 16) % 16, 0, "{name}: 密文不是整块");
        assert_eq!(
            aes_dynamic_iv_decrypt(key, payload).unwrap(),
            case["plaintext"].as_str().unwrap(),
            "{name}"
        );
    }
    assert!(
        directions.contains("request") && directions.contains("response"),
        "信封是**双向**的，向量里两个方向都要有：{directions:?}"
    );
}

#[test]
fn app_cool_flag_body_envelope_192_is_not_the_128_bit_key() {
    // 这条锁的是"**至少两把钥匙，按链路分**"这个结论本身：
    // 发码链路那把 AES-128 钥匙在整包信封的实拍上**解不出正确明文**。
    // 只断言"结果不等于期望明文"而不是"必定报错" ——
    // 换错钥匙时 PKCS7 有 1/256 的概率碰巧合法，那时解出的是乱码而不是 Err。
    let v = app_business_vectors();
    let k192 = v["constants"]["BODY_AES_KEY_192"].as_str().unwrap();
    let k128 = v["constants"]["BODY_AES_KEY"].as_str().unwrap();
    assert_ne!(k192, k128, "两把钥匙必须不同 —— 否则这条测试没有意义");

    for case in v["hcyCoolFlagBodyEnvelope192"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let payload = case["payloadB64"].as_str().unwrap();
        let expect = case["plaintext"].as_str().unwrap();
        assert_eq!(
            aes_dynamic_iv_decrypt(k192, payload).unwrap(),
            expect,
            "{name}"
        );
        assert_ne!(
            aes_dynamic_iv_decrypt(k128, payload).ok().as_deref(),
            Some(expect),
            "{name}: AES-128 那把不该能开出这条明文"
        );
    }
}

#[test]
fn app_cool_flag_webview_envelope_256_decrypts_every_real_capture() {
    // **第三把钥匙：H5 / WebView 那一层**（2026-09-20 定案）。
    // 出处不是二进制，而是 iOS App 沙盒里的 **H5 离线包**：
    //   Data/Documents/AIOfflinePKG/stable/ai-helper/assets/index-af02d869.js
    // 里 `Kr.coolFlagPivotal = "h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1"`，
    // 调用点 `AES_CBCDecrypt({ data, key: coolFlagPivotal, getIVFromEncrypt: true })`。
    //
    // 四组交叉对照（全部干净）：
    //   WebView × AES-256 = 3/3 ；原生 × AES-256 = 0/2
    //   WebView × AES-192 = 0/3 ；原生 × AES-192 = 3/3
    let v = app_business_vectors();
    let key = v["constants"]["BODY_AES_KEY_256_H5"].as_str().unwrap();

    // 32 字节 ⇒ AES-256。⚠ 这把钥匙**含一个 `+`** —— 曾经因为它不在 `[A-Za-z0-9]` 里，
    // 穷举二进制字面量时被整批漏掉（21173 个候选全落空）。断言长度顺便钉住这件事。
    assert_eq!(key.len(), 32, "32 字节 ⇒ AES-256");
    assert!(
        key.contains('+'),
        "这把钥匙含非字母数字字符，别按「纯字母数字」筛候选"
    );

    let cases = v["hcyCoolFlagWebViewEnvelope256"].as_array().unwrap();
    assert!(cases.len() >= 3, "至少要有请求与响应两个方向的样本");
    let mut directions = std::collections::BTreeSet::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let payload = case["payloadB64"].as_str().unwrap();
        directions.insert(case["direction"].as_str().unwrap().to_string());

        let raw = b64_decode(payload).unwrap();
        assert_eq!(
            hex::encode(&raw[..16]),
            case["ivHex"].as_str().unwrap(),
            "{name}: 前置 IV 与 ivHex 不一致"
        );
        assert_eq!((raw.len() - 16) % 16, 0, "{name}: 密文不是整块");

        // 指纹断言：`ivKind` 是"这条载荷由哪一层发出"的声明，必须与字节事实相符。
        let printable = raw[..16].iter().all(|b| (0x20..0x7f).contains(b));
        match case["ivKind"].as_str().unwrap() {
            "ascii" => assert!(printable, "{name}: 声明 ascii，但 IV 不可打印"),
            "binary" => assert!(!printable, "{name}: 声明 binary，但 IV 全可打印"),
            other => panic!("{name}: 未知 ivKind {other}"),
        }

        assert_eq!(
            aes_dynamic_iv_decrypt(key, payload).unwrap(),
            case["plaintext"].as_str().unwrap(),
            "{name}"
        );
    }
    assert!(
        directions.contains("request") && directions.contains("response"),
        "信封是**双向**的，向量里两个方向都要有：{directions:?}"
    );
}

#[test]
fn app_cool_flag_families_are_keyed_by_who_sent_it_not_by_version() {
    // 这条锁的是本轮的**核心结论**：三把钥匙是「**谁发的**」的函数，不是版本号。
    //   · 原生 App（FXHttpBaseAccessor）→ AES-192 `2olBaQGYnEKoStYomsd1n7ax`
    //   · H5 / WebView（`coolFlagPivotal`）→ AES-256 `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1`
    // 所以**两把钥匙必须互不开**：拿错钥匙不是"降级成乱码"，而是解不开。
    let v = app_business_vectors();
    let k192 = v["constants"]["BODY_AES_KEY_192"].as_str().unwrap();
    let k256 = v["constants"]["BODY_AES_KEY_256_H5"].as_str().unwrap();
    assert_eq!(
        (k192.len(), k256.len()),
        (24, 32),
        "24→AES-192 / 32→AES-256"
    );

    for case in v["hcyCoolFlagBodyEnvelope192"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let payload = case["payloadB64"].as_str().unwrap();
        let expect = case["plaintext"].as_str().unwrap();
        assert_eq!(
            aes_dynamic_iv_decrypt(k192, payload).unwrap(),
            expect,
            "{name}"
        );
        // 用 `assert_ne!` 而不是 `is_err()`：换错钥匙时 PKCS7 有 1/256 的概率碰巧合法，
        // 那时解出的是乱码而不是 Err —— 判据要盯"明文对不对"，不是"报不报错"。
        assert_ne!(
            aes_dynamic_iv_decrypt(k256, payload).ok().as_deref(),
            Some(expect),
            "{name}: AES-256 那把不该能开出原生族的明文"
        );
    }

    for case in v["hcyCoolFlagWebViewEnvelope256"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let payload = case["payloadB64"].as_str().unwrap();
        let expect = case["plaintext"].as_str().unwrap();
        assert_eq!(
            aes_dynamic_iv_decrypt(k256, payload).unwrap(),
            expect,
            "{name}"
        );
        assert_ne!(
            aes_dynamic_iv_decrypt(k192, payload).ok().as_deref(),
            Some(expect),
            "{name}: AES-192 那把不该能开出 WebView 族的明文"
        );
    }
}

#[test]
fn app_cool_flag_iv_printability_tracks_the_sending_layer() {
    // 「前 16 字节能不能打印」= 「哪一层发的」的**指纹**，不是协议约定：
    //   · H5 的 IV 由 `Bn.generateRandomString(16)` 生成 —— 随机**字符**
    //   · 原生的 IV 是随机**字节**
    //
    // ⚠ 这是**统计指纹**：16 个随机字节恰好全落在可打印区间的概率约 (95/256)^16 ≈ 1.4e-7。
    // 所以它**几乎**不会误判，但理论上会。这条断言的作用是"指纹一旦漂移先响"——
    // 若哪天某条原生实拍的 IV 恰好全可打印，测试失败本身就是要的信息（说明这条判据不牢），
    // 而不是"改断言把它压下去"。
    let v = app_business_vectors();

    for case in v["hcyCoolFlagBodyEnvelope192"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let raw = b64_decode(case["payloadB64"].as_str().unwrap()).unwrap();
        assert!(
            !raw[..16].iter().all(|b| (0x20..0x7f).contains(b)),
            "{name}: 原生族的 IV 应基本不可能全可打印"
        );
    }

    // ⚠ 反向**不能**断言："可打印 IV ⇒ WebView 族"不成立 ——
    // `encodeData` 也是可打印 IV（见 `app_encode_data_is_a_16_ascii_iv_prefix_not_a_binary_one`）。
    // 所以这条指纹只用来在**已知是 hcy-cool-flag 整包信封**的前提下挑钥匙。
    let req_ascii = v["hcyCoolFlagWebViewEnvelope256"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["ivKind"] == "ascii")
        .count();
    assert!(req_ascii >= 2, "WebView 族的请求侧样本应至少 2 条");
}

#[test]
fn sha1_upper_matches_pc_client_encrypt_by_hash() {
    let v = pc_vectors();
    for case in v["sha1Upper"].as_array().unwrap() {
        let got = sha1_hex_upper(case["input"].as_str().unwrap().as_bytes());
        assert_eq!(got, case["expect"].as_str().unwrap(), "{}", case["name"]);
    }

    // 大小写不是风格问题：结果直接进请求体的 dycpwd，服务端按十六进制解析
    let hashed = sha1_hex_upper(b"fetion.com.cn:pass1234");
    assert_eq!(hashed, hashed.to_ascii_uppercase());
    assert_eq!(hashed.len(), 40, "SHA-1 是 40 位十六进制");
}

#[test]
fn app_cool_flag_h5_key_also_switches_with_h5_env() {
    // H5 那把钥匙**不是常量**，而是「运行环境」的函数。原文（H5 离线包
    // `AIOfflinePKG/stable/ai-helper/assets/index-af02d869.js`）：
    //
    //   const Kr = { ..., coolFlagPivotal: "h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1", ... };
    //   const _r = T("h5_env");
    //   !_r || "dev2Prod" === _r || (
    //       Kr.userZonePivotal = "2ErfJus1Ofr@2o24",
    //       Kr.coolFlagPivotal = "PVGDwmcvfs1uV3d1",
    //       $r = "1111");
    //
    // 默认值来自同一文件的 `localStorage.getItem("h5_env") || "dev2Prod"`。
    // 真机没设 `h5_env` ⇒ 走 dev2Prod ⇒ 用 AES-256 那把（与实拍命中一致）。
    let v = app_business_vectors();
    let c = &v["constants"];
    let prod = c["BODY_AES_KEY_256_H5"].as_str().unwrap();
    let non_prod = c["BODY_AES_KEY_128_H5_NONPROD"].as_str().unwrap();

    assert_eq!(prod.len(), 32, "dev2Prod 用 32 字节 ⇒ AES-256");
    assert_eq!(non_prod.len(), 16, "非 dev2Prod 用 16 字节 ⇒ AES-128");
    // 两把必须真的不同，否则这条测试没有信息量
    assert_ne!(prod, non_prod);
    // 且非 dev2Prod 那把不能与另外两族撞车 —— 撞了就说明我把常量写错了
    for other in [
        c["BODY_AES_KEY"].as_str().unwrap(),
        c["BODY_AES_KEY_192"].as_str().unwrap(),
    ] {
        assert_ne!(non_prod, other, "环境变体钥匙不该与其它族相同");
    }
    // ⚠ 这条结论的意义：**"按谁发的分"之外还有"按哪个环境分"这一维。**
    // 只按"谁发的"挑钥匙，在把 `h5_env` 设过的设备上会**静默失效**。
}

#[test]
fn app_cool_flag_envelope_192_also_covers_the_ose_ilog_host() {
    // 适用范围按**信封**算，不按域名算：`ose.caiyun.feixin.10086.cn/richlifeApp/devapp/ILog`
    // 与 `yun.139.com` 是两套子系统，但同一把 AES-192 钥匙照样能解它的响应。
    let v = app_business_vectors();
    let key = v["constants"]["BODY_AES_KEY_192"].as_str().unwrap();
    let case = v["hcyCoolFlagBodyEnvelope192"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"].as_str().unwrap().contains("ILog"))
        .expect("ILog 那条实拍应在 192 分节里");

    let iv = hex::decode(case["ivHex"].as_str().unwrap()).unwrap();
    assert_eq!(iv.len(), 16);
    // ⚠ 这条断言钉住一个**判据教训**：IV 的可打印性必须落在 **base64 解码后的字节**上。
    // 看 base64 字符串永远"可打印"（base64 字母表本身就是可打印 ASCII），
    // 据此会把 ILog 误判成 H5 族 —— 这正是上一轮犯过的错。
    assert!(
        !iv.iter().all(|b| (0x20..0x7f).contains(b)),
        "ILog 是原生请求，IV 是二进制；若这里变可打印，说明判据落错了层"
    );

    let payload = case["payloadB64"].as_str().unwrap();
    let expect = case["plaintext"].as_str().unwrap();
    assert_eq!(
        aes_dynamic_iv_decrypt(key, payload).unwrap(),
        expect,
        "ILog 响应应能用 AES-192 解出"
    );
    assert!(expect.contains("resultCode"), "ILog 响应是 XML 结果体");
    // 反向：另外两族都开不出它
    for other in [
        v["constants"]["BODY_AES_KEY"].as_str().unwrap(),
        v["constants"]["BODY_AES_KEY_256_H5"].as_str().unwrap(),
    ] {
        assert_ne!(
            aes_dynamic_iv_decrypt(other, payload).ok().as_deref(),
            Some(expect),
            "换别的族不该解出同一段明文"
        );
    }
}
