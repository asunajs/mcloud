//! 登录状态机测试。
//!
//! 全程离线：假 HTTP 回放真实抓包过的业务码，`SeqRandom` 让 `sID` 可复现。

use std::sync::Mutex;

use async_trait::async_trait;
use mcloud_core::{HttpClient, HttpRequest, HttpResponse};

use super::*;
use mcloud_upstream::login::LoginDataDecryptor;

// ── 测试替身 ────────────────────────────────────────────────────────────

/// 按顺序回放响应，并记录收到过哪些请求。
#[derive(Default)]
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(replies: &[&str]) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.iter().rev().map(|s| s.to_string()).collect()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req);
        // 队尾取出（构造时已反转），用尽后重复最后一个
        let body = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or_else(|| "{}".into())
            }
        };
        Ok(HttpResponse {
            status: 200,
            body,
            headers: Vec::new(),
        })
    }
}

fn rng() -> mcloud_crypto::SeqRandom {
    mcloud_crypto::SeqRandom::new(1)
}

/// 加密一段 userInfo，模拟上游的成功响应里的 `data`。
fn encrypted_user_info(user_info: &str) -> String {
    use aes::cipher::{BlockEncrypt, KeyInit};
    let key = mcloud_upstream::login::LOGIN_AES_KEY.as_bytes();
    let mut buf = user_info.as_bytes().to_vec();
    let pad = 16 - (buf.len() % 16);
    buf.resize(buf.len() + pad, pad as u8);
    let cipher = aes::Aes128::new_from_slice(key).unwrap();
    let mut ct = Vec::new();
    for block in buf.chunks_exact(16) {
        let mut b = [0u8; 16];
        b.copy_from_slice(block);
        cipher.encrypt_block((&mut b).into());
        ct.extend_from_slice(&b);
    }
    hex::encode(&ct)
}

fn success_envelope(user_info: &str) -> String {
    format!(
        r#"{{"code":"0000","data":"{}"}}"#,
        encrypted_user_info(user_info)
    )
}

// ══════════════════════════ 扫码：平台贯穿 ══════════════════════════

/// 二维码载荷随平台变（`ver` 段）。
#[test]
fn qr_login_payload_carries_platform_version() {
    let win = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng(), 1_700_000_000_000);
    let mac = QrLogin::start(PcPlatform::MacOs, "DEV", &mut rng(), 1_700_000_000_000);

    assert!(win.qr_payload().contains("ver=8.8.6.20260829"));
    assert!(mac.qr_payload().contains("ver=4.2.1"));
    assert_ne!(win.qr_payload(), mac.qr_payload());
}

/// **登录请求本身**随平台变 —— 这是"底层区分"在状态机上的落点。
#[tokio::test]
async fn qr_poll_sends_platform_specific_request() {
    let http = Scripted::new(&[r#"{"code":"200059541","message":"未扫"}"#]);
    let mut qr = QrLogin::start(PcPlatform::MacOs, "DEV-1", &mut rng(), 1_700_000_000_000);

    let p = qr
        .poll(
            &http,
            "TID",
            &LoginDataDecryptor::production(),
            1_700_000_000_000,
        )
        .await
        .unwrap();
    assert_eq!(p, QrProgress::NotScanned);

    let reqs = http.requests();
    assert_eq!(reqs.len(), 1);
    let body = reqs[0].body.as_deref().unwrap();
    // mac 的三个业务参数
    assert!(body.contains(r#""clientType":"710""#), "实得：{body}");
    assert!(body.contains(r#""cpid":"332""#), "实得：{body}");
    assert!(body.contains(r#""version":"4.2.1""#), "实得：{body}");

    // 登录请求的设备头里，渠道位是**常量** `10200153`（不随平台变）
    let names: Vec<(&str, &str)> = reqs[0]
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert!(
        names.contains(&("x-huawei-channelSrc", "10200153")),
        "mac 的 x-huawei-channelSrc 应当仍是常量 10200153：{names:?}"
    );
    // 而 `x-yun-app-channel` 属于登录后那一套，登录请求里不该出现
    assert!(
        !names.iter().any(|(k, _)| *k == "x-yun-app-channel"),
        "登录请求不该带 x-yun-app-channel：{names:?}"
    );
}

/// Windows 与 macOS 在同一步发出的请求**逐字不同**。
#[tokio::test]
async fn windows_and_macos_polls_differ_byte_for_byte() {
    let reply = [r#"{"code":"200059541"}"#];
    let mut w = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng(), 0);
    let mut m = QrLogin::start(PcPlatform::MacOs, "DEV", &mut rng(), 0);

    let hw = Scripted::new(&reply);
    let hm = Scripted::new(&reply);
    w.poll(&hw, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    m.poll(&hm, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();

    let rw = &hw.requests()[0];
    let rm = &hm.requests()[0];

    assert_eq!(rw.url, rm.url, "端点相同");
    assert_ne!(rw.body, rm.body, "body 必须因平台而异");

    // 登录头的差异落在**设备串**上（第 3 段是版本号），
    // 而不是渠道位 —— 那个是常量。
    let info = |r: &HttpRequest| {
        r.headers
            .iter()
            .find(|(k, _)| k == "x-DeviceInfo")
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_ne!(info(rw), info(rm), "设备串必须因平台而异");
    assert!(info(rw).contains("8.8.6.20260829"), "win：{}", info(rw));
    assert!(info(rm).contains("4.2.1"), "mac：{}", info(rm));

    // 渠道位两端相同
    let chan = |r: &HttpRequest| {
        r.headers
            .iter()
            .find(|(k, _)| k == "x-huawei-channelSrc")
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(chan(rw), "10200153");
    assert_eq!(chan(rm), "10200153");
}

/// 轮询的每个状态都被正确翻译，且未扫码**不是**终态。
#[tokio::test]
async fn poll_progress_and_terminality() {
    // 未扫码 → 非终态
    let http = Scripted::new(&[r#"{"code":"200059541"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    assert_eq!(p, QrProgress::NotScanned);
    assert!(!p.is_terminal());
    assert!(qr.next_delay_ms(0).is_some(), "还应继续轮询");

    // 已扫码 → 非终态（等确认）
    let http = Scripted::new(&[r#"{"code":"200059548"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    assert_eq!(p, QrProgress::Scanned);
    assert!(!p.is_terminal());

    // 已失效 → 终态
    let http = Scripted::new(&[r#"{"code":"200059542"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    assert_eq!(
        p,
        QrProgress::Failed {
            reason: QrFailure::ExpiredUpstream
        }
    );
    assert!(p.is_terminal());
    assert!(qr.next_delay_ms(0).is_none(), "终态后必须停止轮询");
}

/// 限流是**唯一**非终态的失败 —— 该退避接着问，不是换码重来。
#[tokio::test]
async fn too_frequent_is_not_terminal() {
    let http = Scripted::new(&[r#"{"code":"200059545","message":"扫码登录过于频繁"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();

    assert_eq!(
        p,
        QrProgress::Failed {
            reason: QrFailure::TooFrequent
        }
    );
    assert!(!p.is_terminal(), "限流不该终止会话");
    assert!(
        qr.next_delay_ms(0).is_some(),
        "应当还可以继续轮询（退避后）"
    );
}

/// `sID` 不匹配是调用方状态错误，属终态。
#[tokio::test]
async fn session_mismatch_is_terminal() {
    let http = Scripted::new(&[r#"{"code":"200059544"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    assert_eq!(
        p,
        QrProgress::Failed {
            reason: QrFailure::SessionMismatch
        }
    );
    assert!(p.is_terminal());
}

/// 成功：`data` 是密文，必须解出 `userInfo`。
#[tokio::test]
async fn success_decrypts_user_info() {
    let reply = success_envelope(r#"{"account":"19900000001","authToken":"tok-1"}"#);
    let http = Scripted::new(&[&reply]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);

    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    match p {
        QrProgress::LoggedIn { ref user_info } => {
            assert_eq!(user_info["account"], "19900000001");
            assert_eq!(user_info["authToken"], "tok-1");
        }
        other => panic!("应登录成功，实得 {other:?}"),
    }
    assert!(p.is_terminal());
    assert!(qr.next_delay_ms(0).is_none());
}

/// **终态之后不再发请求**（对应客户端的 `clearInterval`）。
#[tokio::test]
async fn terminal_state_stops_sending_requests() {
    let reply = success_envelope(r#"{"account":"19900000001","authToken":"t"}"#);
    let http = Scripted::new(&[&reply]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);

    let first = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    // 再问两次：不该产生新请求
    let again = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 500)
        .await
        .unwrap();
    let third = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 900)
        .await
        .unwrap();

    assert_eq!(first, again);
    assert_eq!(first, third);
    assert_eq!(http.requests().len(), 1, "终态后不该再发请求");
}

/// TTL 到点：终态是"我们不等了"，且**不再发请求**。
#[tokio::test]
async fn ttl_elapsed_fails_without_sending() {
    let http = Scripted::new(&[r#"{"code":"200059541"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    // 默认 120 秒，直接跳到 121 秒
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 121_000)
        .await
        .unwrap();

    assert_eq!(
        p,
        QrProgress::Failed {
            reason: QrFailure::TtlElapsed { ttl_seconds: 120 }
        }
    );
    assert_eq!(http.requests().len(), 0, "超时后不该发请求");
    assert!(qr.next_delay_ms(121_000).is_none());
}

/// `with_ttl` 能延长有效期（"把码发给别人扫"）。
#[test]
fn ttl_can_be_overridden() {
    let qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0).with_ttl(600);
    assert!(!qr.is_expired(599_000));
    assert!(qr.is_expired(600_000));
    // 默认值不受影响
    let d = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    assert!(d.is_expired(121_000));
    assert!(!d.is_expired(119_000));
}

/// 未知业务码**原样带回**，不硬编成"未知错误"。
#[tokio::test]
async fn unknown_code_is_preserved() {
    let http = Scripted::new(&[r#"{"code":"424242","message":"新码"}"#]);
    let mut qr = QrLogin::start(PcPlatform::Windows, "D", &mut rng(), 0);
    let p = qr
        .poll(&http, "T", &LoginDataDecryptor::production(), 0)
        .await
        .unwrap();
    match p {
        QrProgress::Failed {
            reason: QrFailure::Rejected { code, message },
        } => {
            assert_eq!(code, Some(424242));
            assert_eq!(message, "新码");
        }
        other => panic!("应原样带回，实得 {other:?}"),
    }
}

// ══════════════════════════ 短信 ══════════════════════════

#[tokio::test]
async fn sms_send_code_reports_quota_and_slider() {
    let http = Scripted::new(&[r#"{"code":"0000","data":{"remainGetTimes":2}}"#]);
    let sms = SmsLogin::start(PcPlatform::Windows, "D", "19900000001", &mut rng());
    assert_eq!(
        sms.send_code(&http, "T").await.unwrap(),
        SendSmsOutcome::Sent {
            remain_get_times: Some(2)
        }
    );
    // 请求体带手机号与平台
    let body = http.requests()[0].body.clone().unwrap();
    assert!(body.contains("19900000001"));
    assert!(body.contains(r#""clientType":"656""#));

    // 需要滑块
    let http = Scripted::new(&[r#"{"code":200059554,"data":{"captchaType":2}}"#]);
    let sms = SmsLogin::start(PcPlatform::MacOs, "D", "19900000001", &mut rng());
    assert_eq!(
        sms.send_code(&http, "T").await.unwrap(),
        SendSmsOutcome::NeedSlider {
            captcha_type: Some(2)
        }
    );
    // mac 的 clientType 也要跟着变
    assert!(http.requests()[0]
        .body
        .as_deref()
        .unwrap()
        .contains(r#""clientType":"710""#));
}

/// 滑块答案能一路传进 `puzzleVerfycode`。
#[tokio::test]
async fn sms_send_code_with_slider_embeds_the_answer() {
    let http = Scripted::new(&[r#"{"code":"0000"}"#]);
    let sms = SmsLogin::start(PcPlatform::Windows, "D", "19900000001", &mut rng());
    sms.send_code_with_slider(&http, "T", "461").await.unwrap();

    let body = http.requests()[0].body.clone().unwrap();
    assert!(body.contains("puzzleVerfycode"), "实得：{body}");
    // 字段名必须带上游那个拼写（verfy 不是 verify）
    assert!(!body.contains("puzzleVerifycode"), "字段名拼错了：{body}");
}

/// `reroll` 换掉 `random`（限流后必须换）。
#[test]
fn sms_random_can_be_rerolled() {
    let mut sms = SmsLogin::start(PcPlatform::Windows, "D", "19900000001", &mut rng());
    let before = sms.random().to_string();
    sms.reroll(&mut mcloud_crypto::SeqRandom::new(200));
    assert_ne!(sms.random(), before, "换一个 random 才能再发码");
}

/// 取滑块挑战能解出答案。
#[tokio::test]
async fn sms_slider_puzzle_is_parsed_and_solvable() {
    use mcloud_upstream::login::{PUZZLE_AES_IV, PUZZLE_AES_KEY};
    let enc =
        mcloud_crypto::encrypt_with_dynamic_iv(PUZZLE_AES_KEY.as_bytes(), PUZZLE_AES_IV, "370")
            .unwrap();
    let reply = format!(
        r#"{{"code":"0000","data":{{"picture":"BG","puzzle":"PZ","picWidth":"680","puzzleLeft":"{enc}"}}}}"#
    );
    let http = Scripted::new(&[&reply]);
    let sms = SmsLogin::start(PcPlatform::Windows, "D", "19900000001", &mut rng());
    let puzzle = sms.slider_puzzle(&http, "T").await.unwrap();

    assert_eq!(puzzle.background_b64, "BG");
    assert_eq!(puzzle.answer().unwrap(), "370", "无需图像识别即可解出");
}

/// 短信登录成功要解出 userInfo；失败要报错（不能静默返回空）。
#[tokio::test]
async fn sms_login_success_and_failure() {
    let reply = success_envelope(r#"{"account":"19900000001","authToken":"tok"}"#);
    let http = Scripted::new(&[&reply]);
    let sms = SmsLogin::start(PcPlatform::Windows, "D", "19900000001", &mut rng());
    let v = sms
        .login(&http, "T", "123456", &LoginDataDecryptor::production())
        .await
        .unwrap();
    assert_eq!(v["account"], "19900000001");

    // 验证码错误必须报错
    let http = Scripted::new(&[r#"{"code":9441,"message":"验证码不正确"}"#]);
    let sms = SmsLogin::start(PcPlatform::Windows, "D", "19900000001", &mut rng());
    let e = sms
        .login(&http, "T", "000000", &LoginDataDecryptor::production())
        .await;
    assert!(e.is_err(), "验证码错误必须报错");
}

// ══════════════════════════ 账密 ══════════════════════════

/// 账密登录：口令**不能**以明文出现在请求体里。
#[tokio::test]
async fn password_login_never_sends_the_plain_password() {
    let reply = success_envelope(r#"{"account":"19900000001","authToken":"tok"}"#);
    let http = Scripted::new(&[&reply]);
    let pw = PasswordLogin::new(PcPlatform::MacOs, "D");
    pw.login(
        &http,
        "T",
        "19900000001",
        "hunter2",
        &LoginDataDecryptor::production(),
    )
    .await
    .unwrap();

    let body = http.requests()[0].body.clone().unwrap();
    assert!(!body.contains("hunter2"), "明文口令泄漏进请求体：{body}");
    assert!(body.contains(&mcloud_upstream::login::password_hash("hunter2")));
    assert!(body.contains(r#""pintype":"9""#));
    assert!(body.contains(r#""clientType":"710""#), "平台要跟着走");
}

// ══════════════════════════ 落库：平台必须持久化 ══════════════════════════

/// **mac 与 win 登录后落盘的 JSON 不同** —— 用户点名的那个差异。
#[test]
fn login_persists_platform_into_json() {
    let info = serde_json::json!({"account":"19900000001","authToken":"tok"});

    let w = account_from_login(&info, PcPlatform::Windows, "DEV-1").unwrap();
    let m = account_from_login(&info, PcPlatform::MacOs, "DEV-1").unwrap();

    let jw = serde_json::to_string(&w).unwrap();
    let jm = serde_json::to_string(&m).unwrap();
    assert_ne!(jw, jm, "两种平台落盘必须可区分");
    assert!(jw.contains(r#""pc_platform":"windows""#), "实得：{jw}");
    assert!(jm.contains(r#""pc_platform":"macos""#), "实得：{jm}");
    assert_eq!(w.pc_platform, Some(PcPlatform::Windows));
    assert_eq!(m.pc_platform, Some(PcPlatform::MacOs));
}

/// 登录产出的账号字段都对：掩码主键、完整手机号、原样 auth 串。
#[test]
fn account_from_login_fields_are_correct() {
    let info = serde_json::json!({"account":"19900000001","authToken":"tok-abc"});
    let acc = account_from_login(&info, PcPlatform::Windows, "DEV-9").unwrap();

    assert_eq!(acc.id.as_str(), "19****01", "主键是掩码");
    assert_eq!(acc.auth.phone, "19900000001", "凭据里是完整号码");
    assert_eq!(acc.auth.platform, "pc", "PC 前缀（mac/win 都是它）");
    assert_eq!(acc.auth.token, "tok-abc", "token 是 authToken");
    assert_eq!(acc.device_id.as_str(), "DEV-9");
    assert_eq!(acc.pc_platform, Some(PcPlatform::Windows));
    // auth 串要能被重新解析回同样的内容
    let back = parse_auth(&acc.auth.raw).unwrap();
    assert_eq!(back.phone, acc.auth.phone);
    assert_eq!(back.token, acc.auth.token);
}

/// 缺字段必须报错，不能落一个半个凭据的账号。
#[test]
fn account_from_login_rejects_incomplete_user_info() {
    assert!(account_from_login(
        &serde_json::json!({"account":"1"}),
        PcPlatform::Windows,
        "D"
    )
    .is_err());
    assert!(account_from_login(
        &serde_json::json!({"authToken":"t"}),
        PcPlatform::Windows,
        "D"
    )
    .is_err());
    assert!(account_from_login(&serde_json::json!({}), PcPlatform::Windows, "D").is_err());
    // 空串等同缺失
    assert!(account_from_login(
        &serde_json::json!({"account":"","authToken":"t"}),
        PcPlatform::Windows,
        "D"
    )
    .is_err());
}

/// App 前缀（`mobile`）不会被默默写成 `pc`。
#[test]
fn explicit_prefix_is_respected() {
    let info = serde_json::json!({"account":"19900000001","authToken":"tok"});
    let app =
        account_from_user_info_with_prefix(&info, mcloud_upstream::APP_AUTH_PREFIX, None, "D")
            .unwrap();
    assert_eq!(app.auth.platform, "mobile");
    assert_eq!(app.pc_platform, None, "App 账号不该有 PC 平台");

    // 同一个 userInfo 用不同前缀 ⇒ 不同的 auth 串
    let pc = account_from_login(&info, PcPlatform::Windows, "D").unwrap();
    assert_ne!(pc.auth.raw, app.auth.raw);
}

/// 落库走的是两个 store 方法，且状态里带上平台。
#[tokio::test]
async fn persist_login_writes_account_and_state() {
    use mcloud_core::{AccountStore, Result as CoreResult};

    #[derive(Default)]
    struct Rec {
        saved: Mutex<Vec<Account>>,
        states: Mutex<Vec<(String, serde_json::Value)>>,
    }

    #[async_trait]
    impl AccountStore for Rec {
        async fn accounts(&self) -> CoreResult<Vec<Account>> {
            Ok(self.saved.lock().unwrap().clone())
        }
        async fn account(&self, id: &AccountId) -> CoreResult<Option<Account>> {
            Ok(self
                .saved
                .lock()
                .unwrap()
                .iter()
                .find(|a| &a.id == id)
                .cloned())
        }
        async fn save_account(&self, account: &Account) -> CoreResult<()> {
            self.saved.lock().unwrap().push(account.clone());
            Ok(())
        }
        async fn remove_account(&self, _id: &AccountId) -> CoreResult<()> {
            Ok(())
        }
        async fn account_state(&self, _id: &AccountId) -> CoreResult<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        async fn save_account_state(
            &self,
            id: &AccountId,
            state: &serde_json::Value,
        ) -> CoreResult<()> {
            self.states
                .lock()
                .unwrap()
                .push((id.as_str().to_string(), state.clone()));
            Ok(())
        }
    }

    let info = serde_json::json!({"account":"19900000001","authToken":"tok"});
    let acc = account_from_login(&info, PcPlatform::MacOs, "DEV-X").unwrap();
    let store = Rec::default();

    persist_login(
        &store,
        &LoginResult {
            account: acc.clone(),
        },
    )
    .await
    .unwrap();

    assert_eq!(store.saved.lock().unwrap().len(), 1);
    let states = store.states.lock().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].0, "19****01");
    assert_eq!(states[0].1["device_id"], "DEV-X");
    assert_eq!(states[0].1["pc_platform"], "macos");
}

// ══════════════════════════ 退出 ══════════════════════════

/// 退出是**尽力而为**：服务端回 `token 不能为空` 也不算失败。
#[tokio::test]
async fn logout_swallows_upstream_failure() {
    let http = Scripted::new(&[r#"{"code":1000001,"message":"用户token不能为空"}"#]);
    let client = PcClient::new(PcPlatform::Windows, "D");
    // 必须返回 Ok —— 本地凭据已经清了，这不算失败
    assert!(logout(&http, &client, "T", "19900000001").await.is_ok());

    let body = http.requests()[0].body.clone().unwrap();
    assert!(body.contains("19900000001"));
}

/// 退出请求也要带设备头，且渠道位是**常量**（与登录一致）。
#[tokio::test]
async fn logout_request_carries_platform_headers() {
    let http = Scripted::new(&["{}"]);
    let client = PcClient::new(PcPlatform::MacOs, "D");
    logout(&http, &client, "T", "19900000001").await.unwrap();
    let req = &http.requests()[0];
    assert!(req.url.ends_with("/user/logout"));
    assert!(req
        .headers
        .iter()
        .any(|(k, v)| k == "x-huawei-channelSrc" && v == "10200153"));
    // 登录链路的请求都只用那 5 个头（`x-yun-app-channel` 是登录后的）
    assert!(!req.headers.iter().any(|(k, _)| k == "x-yun-app-channel"));
}
