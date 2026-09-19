//! App 端（iOS 客户端）验证码登录的**驱动**。
//!
//! 与 PC 那条链路（[`crate::login`]）刻意分开，因为它们的**形状**不同，
//! 不只是参数不同：
//!
//! | | PC | App |
//! | --- | --- | --- |
//! | 请求体 | 明文 JSON | **加密**（AES-ECB + 末 10 位前移） |
//! | `Authorization` 前缀 | `pc` | `mobile` |
//! | 滑块 | 第一方取图接口，可自动解 | **同一个第一方取图接口，可自动解** |
//!
//! # 发码的滑块：**协议优先，H5 兜底**
//!
//! 这里曾经写着"App 端没有自动重试环，短信是那个 H5 发的"。**那条结论是错的** ——
//! 错在把 **App 客户端的行为**（它选择开 WebView）当成了**协议的限制**。
//!
//! 实测（2026-09-19，跑通）：
//!
//! ```text
//! getSmsCode(空) → 200059554
//!   → POST /user/verfycode 取第一方题图（**与 getSmsCode 同一台主机**）
//!   → puzzleLeft 用公开常量解开 = 拖动坐标
//!   → getSmsCode(带坐标) → 0000，短信已发
//! ```
//!
//! 所以 App 端**有**和 PC 一样可自动完成的滑块，而且**编码完全一样**
//! （复用 [`mcloud_upstream::login::puzzle_verfycode_encrypt`]）。
//! H5 地址（[`captcha_h5_url`]）退化为兜底：**只在滑块重试用尽时**才交给用户。
//!
//! ⚠ 那个接口**计失败次数**（`200059553` = 该挑战作废）。所以重试有上限，
//! 且每次都**重取新题**——拿旧答案再试只会再失败一次。
//!
//! 依据：`docs/capture/app-sms-login-samples.txt` §2.1。
//!
//! # 追加澄清：上面那个环**不是**"App 自己的滑块被自动化了"
//!
//! 它复用的是 **PC 端那套**取题与编码（`slider_puzzle_request` 直接调
//! [`mcloud_upstream::login::puzzle_verfycode_encrypt`]），只是 `clientType` 与请求头
//! 换成 App 的 —— 属**围魏救赵**。所以"App 端滑块已实现自动化"这句**不成立**：
//! App 自己那条路（原生开 WebView、短信由 H5 发）并没有被自动化，真机走的仍是它。
//!
//! 也就是说，上面被推翻的只是"**协议**不允许 App 走自动这条路"这一层推断，
//! 而"真机在开 H5"那个观察从来没错过 —— 两层别混，改这段的人（包括 AI）尤其注意。
//!
//! # 凭据边界
//!
//! [`LoginOutcome::user_info`] 里带着 `authToken`：**不打印、不写日志**，
//! 只用于落库与展示摘要。

use chrono::DateTime;
use mcloud_capabilities::login::persist_login_with_prefix;
use mcloud_core::{AccountId, Clock, Error, HttpClient, Random, Scope, StateStore};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::{AppProfile, ios};
use mcloud_upstream::app_login::{
    AppLoginCrypto, AppLoginOutcome, AppSendSmsOutcome, captcha_h5_url, classify_login,
    classify_send_sms, new_login_random, send_sms_code_body, send_sms_code_body_with_slider,
    sms_thirdlogin_body,
};
use serde_json::Value;

use crate::login::{LoginOutcome, SmsCredential};

/// 滑块这条路最多试几次。
///
/// 上限存在的意义是**别把账号试进限流**：那个接口计失败次数
/// （`200059553` = 该挑战作废）。与 PC 端同一个理由，只是 App 端
/// 每次重试都要多打一次 `/user/verfycode`，代价更高。
pub const SMS_MAX_ATTEMPTS: usize = 3;

/// `x-yun-user-timestamp` 的取值：**微秒**。
///
/// 实测是 16 位（`1789743328974257`）——比毫秒多三位。
/// 写成毫秒会让上游看到一个 1970 年前后的时间戳，而**不会**报参数错，
/// 属于"错了也看不出来"的那类问题，所以由本函数统一换算：
/// 调用点拿不到"单位搞错"的机会。
pub fn user_timestamp(clock: &impl Clock) -> i64 {
    clock.now().timestamp_micros()
}

/// 发验证码的结果。
#[derive(Debug, Clone)]
pub enum AppCodeOutcome {
    /// 上游受理，短信已下发。`remain_get_times` 是配额余量。
    Sent {
        remain_get_times: Option<u32>,
        /// 是不是**自动过了一次滑块**才拿到的（给用户一句提示用）。
        solved_slider: bool,
    },
    /// 协议路径走不通了，**把 H5 地址交给用户**。
    ///
    /// **这不是常态** —— 只有滑块重试用尽（[`SMS_MAX_ATTEMPTS`]）才会到这里。
    /// 打开并完成拖动之后，短信会由那个页面发出，此时**直接调 [`app_sms_login`]**，
    /// **不要**再回来重试发码（实测客户端就是这么走的：
    /// H5 回调 `{code:200}` 之后直接 `thirdlogin`，中间没有第二次 `getSmsCode`）。
    NeedCaptcha {
        url: String,
        captcha_type: Option<i64>,
        /// 已经自动试过几次。
        attempts: usize,
        /// 最后发上去的坐标。**留着是为了诊断**：万一服务端要的答案口径
        /// 与自动解出的不同，这一行就是唯一的证据。
        last_answer: Option<String>,
    },
}

/// 发短信验证码（App 端）：**先自己过滑块，过不去才把 H5 交给用户**。
///
/// 每次调用都会重新构造请求体（含 `random`）并**明文**发送 ——
/// `getSmsCode` 是这条链路里唯一不加密的请求。
pub async fn app_send_sms_code<C: HttpClient>(
    upstream: &Upstream<'_, C>,
    profile: &AppProfile,
    phone: &str,
    clock: &impl Clock,
) -> Result<AppCodeOutcome, Error> {
    let mut solved_slider = false;
    let mut last_answer: Option<String> = None;
    let mut last_captcha_type: Option<i64> = None;

    for attempt in 1..=SMS_MAX_ATTEMPTS {
        let body = if solved_slider {
            // 每次都**重取新题**：`200059553` 的语义是"这个挑战作废了"，
            // 复用上一轮的答案只会再失败一次，而且白记一次失败次数。
            let answer = upstream
                .app_fetch_slider_puzzle(profile, user_timestamp(clock), phone)
                .await?
                .answer()?;
            last_answer = Some(answer.clone());
            send_sms_code_body_with_slider(phone, &answer)?
        } else {
            // 首次：`puzzleVerfycode` 照抄实测的空串（不是 `null`、也不是省略字段）。
            send_sms_code_body(phone, "")
        };

        let envelope = upstream
            .app_send_sms_code(profile, user_timestamp(clock), &body)
            .await?;

        match classify_send_sms(&envelope) {
            AppSendSmsOutcome::Sent { remain_get_times } => {
                return Ok(AppCodeOutcome::Sent {
                    remain_get_times,
                    solved_slider,
                });
            }
            // 需要滑块、或旧挑战已作废：都要**重取新题**
            AppSendSmsOutcome::NeedCaptcha { captcha_type }
            | AppSendSmsOutcome::SliderExhausted { captcha_type } => {
                last_captcha_type = captcha_type.or(last_captcha_type);
                if attempt == SMS_MAX_ATTEMPTS {
                    break;
                }
                solved_slider = true;
            }
            AppSendSmsOutcome::Other { code, message } => {
                // 原样带回上游的码与文案：不臆造"手机号错误"这类我们并不知道的结论
                return Err(Error::BadRequest(match code {
                    Some(code) => format!("发送验证码失败 code={code}: {message}"),
                    None => format!("发送验证码失败: {message}"),
                }));
            }
        }
    }

    // 协议路径用尽 —— 交还 H5 地址。这是**兜底**，不是首选。
    Ok(AppCodeOutcome::NeedCaptcha {
        url: captcha_h5_url(profile, phone),
        captcha_type: last_captcha_type,
        attempts: SMS_MAX_ATTEMPTS,
        last_answer,
    })
}

/// 一次 App 登录里不变的那部分：**以谁的身份、冒充哪台设备、用哪张手机号的哪个码**。
///
/// 单独成结构而不是三个参数 —— 与 PC 的 [`crate::login::LoginContext`] 同一个理由：
/// 这三样凑齐才是"一次登录"，分开传容易在调用点被摆错顺序，
/// 而它们都是字符串，摆错了编译器不会吭声。
#[derive(Debug, Clone, Copy)]
pub struct AppLoginContext<'a> {
    /// 凭据归属谁 —— 由 [`Scope`] 表达（WebUI 用当前会话用户，CLI 用 `Scope::Local`）。
    pub scope: Scope<'a>,
    /// 冒充哪台设备。`deviceId` 是**每安装**指纹，必须持久化且永不漂移。
    pub profile: &'a AppProfile,
    /// 手机号 + 收到的验证码。
    pub credential: SmsCredential<'a>,
}

/// 用短信验证码登录（App 端）并落库。
///
/// 与 PC 的 [`crate::login::sms_login`] 共用同一条落库路径
/// （[`persist_login_with_prefix`]），差别只在 `Authorization` 载荷的第一段是 `mobile`。
///
/// # 为什么加密在这里、不在调用点
///
/// 请求体由 [`Upstream::app_thirdlogin`] 内部加密（它要 [`AppLoginCrypto`] 参数），
/// 所以调用点**没有机会**把明文发上去 —— 那会得到 `9103`，
/// 而 `9103` 的三种成因（没加密 / 用错密钥 / 错位方向反了）在日志里长得一模一样。
pub async fn app_sms_login<S, C, K>(
    store: &S,
    upstream: &Upstream<'_, C>,
    context: AppLoginContext<'_>,
    crypto: &AppLoginCrypto,
    clock: &K,
    rng: &mut impl Random,
) -> Result<LoginOutcome, Error>
where
    S: StateStore,
    C: HttpClient,
    K: Clock,
{
    let AppLoginContext {
        scope,
        profile,
        credential,
    } = context;

    let random = new_login_random(rng);
    let body = sms_thirdlogin_body(credential.phone, credential.code, &random);
    let envelope = upstream
        .app_thirdlogin(profile, user_timestamp(clock), crypto, &body)
        .await?;

    let encrypted = match classify_login(&envelope) {
        AppLoginOutcome::Success { encrypted } => encrypted,
        // `9441` 是**好**消息：它证明加密这一层全对了（密钥/模式/错位方向/hex 大小写）。
        AppLoginOutcome::SmsCodeWrong => {
            return Err(Error::BadRequest(
                "验证码不正确或已过期（9441）。加密这一层是通的，重新取一次验证码即可".into(),
            ));
        }
        // `9103` 是**我们**的问题，不是用户的问题 —— 归到 Internal 而不是 BadRequest，
        // 免得前端把它渲染成"请检查你输入的参数"。
        AppLoginOutcome::Undecryptable => {
            return Err(Error::Internal(
                "上游解不开请求体（9103）：本端的加密参数与实测不符，\
                 按 密钥 → 模式 → 错位方向 的顺序排查"
                    .into(),
            ));
        }
        AppLoginOutcome::LoginFailed => {
            return Err(Error::BadRequest(
                "上游拒绝登录（9999 登录失败！）。App 端出现这个码通常是设备头缺失或不完整".into(),
            ));
        }
        AppLoginOutcome::Other { code, message } => {
            // 原样带回上游的码与文案：不臆造"手机号错误"这类我们并不知道的结论
            return Err(Error::BadRequest(match code {
                Some(code) => format!("登录失败 code={code}: {message}"),
                None => format!("登录失败: {message}"),
            }));
        }
    };

    let plaintext = crypto.decrypt_body(&encrypted)?;
    let user_info: Value = serde_json::from_str(&plaintext)
        .map_err(|e| Error::Parse(format!("解出的会话载荷不是 JSON: {e}")))?;

    let account_id = persist_login_with_prefix(
        store,
        scope,
        profile.device().device_id(),
        &user_info,
        ios::AUTH_PREFIX,
        clock.now(),
    )
    .await?;

    Ok(LoginOutcome {
        account_id,
        user_info,
    })
}

/// App 端会话过期时刻。
///
/// 与 PC 同款：本机单机场景"可长期有效"，但仍给一个明确到期时间 ——
/// 无到期的会话在库里无法清理。真正该用多久由上游的 `atExpiretime` 决定，
/// 这里只是落库时的兜底值。
pub fn session_expiry(now: DateTime<chrono::Utc>, days: i64) -> DateTime<chrono::Utc> {
    now + chrono::Duration::days(days)
}

/// 把库里已存的 `user_domain_id` 回填进 App 档案（也就是 `x-yun-uni` 头）。
///
/// # 为什么读不到就算正常、而不是报错
///
/// 这个值是 `thirdlogin` **响应**里的 `userDomainId`，首次登录时本地确实没有 ——
/// 而上游对它的态度是"填错就把你定位到别人的用户域"，所以缺失时原样返回档案
/// （ [`AppProfile`] 因此把它做成可选），绝不编一个。
/// 登录一次之后它就在 `account_state` 里了，第二次起这个函数会带上它 ——
/// 与真机的行为一致（真机也不会为每次登录重新握手）。
///
/// # 为什么 `scope` 由调用方给
///
/// 传入的范围应当与**这一次登录写库时用的同一个**。不一致会出现
/// "读到了某个账号的用户域、却把它发给另一个账号"，那比读不到更糟。
pub async fn profile_with_stored_domain_id<S: StateStore>(
    store: &S,
    scope: Scope<'_>,
    profile: AppProfile,
    phone: &str,
) -> Result<AppProfile, Error> {
    // 账号 id 的派生与落库那条路同源（`persist_login` 用的就是 `account_label`），
    // 所以这里不需要"猜"登录之后这个手机号会落成哪个账号。
    let id = AccountId::new(mcloud_core::account_label(None, phone));
    let domain = store
        .get_account_state(scope, &id)
        .await?
        .and_then(|state| state.user_domain_id)
        .filter(|domain| !domain.is_empty());
    Ok(match domain {
        Some(domain) => profile.with_user_domain_id(domain),
        None => profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use mcloud_core::{HttpRequest, HttpResponse, User, UserId, UserStore};
    use mcloud_upstream::app_client::AppDevice;
    use serde_json::json;
    use std::cell::RefCell;

    /// 抓包里那台设备的脱敏形态（与 `app_client` 的单测同一份）。
    fn profile() -> AppProfile {
        AppProfile::new(AppDevice::ipad(
            "AAA00000000000000000000000000000",
            "AAA000000000",
        ))
    }

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap()
    }

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// 确定性随机源（`new_login_random` 的断言要可复现）。
    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    /// 逐次返回脚本响应的假客户端，并记下发出去的请求。
    struct ScriptedClient {
        responses: RefCell<Vec<Value>>,
        seen: RefCell<Vec<HttpRequest>>,
    }

    impl ScriptedClient {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: RefCell::new(responses),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn sent_body(&self, index: usize) -> String {
            String::from_utf8(self.seen.borrow()[index].body.clone().unwrap()).unwrap()
        }
    }

    impl HttpClient for ScriptedClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
            self.seen.borrow_mut().push(request);
            let mut queue = self.responses.borrow_mut();
            if queue.is_empty() {
                return Err(Error::Internal("脚本已用完".into()));
            }
            Ok(HttpResponse::new(
                200,
                serde_json::to_vec(&queue.remove(0)).unwrap(),
            ))
        }
    }

    async fn seeded_store() -> (mcloud_io::SqliteStore, UserId) {
        let s = mcloud_io::SqliteStore::open_in_memory().unwrap();
        let user = User::new("u1", "alice", "!", mcloud_core::Role::Admin, fixed_now());
        s.upsert_user(&user).await.unwrap();
        (s, user.id)
    }

    /// 一次登录的上下文。手机号固定用 `199` 号段（脱敏约定），
    /// 设备用抓包里那台的脱敏形态。
    fn context<'a>(
        profile: &'a AppProfile,
        user_id: &'a UserId,
        code: &'a str,
    ) -> AppLoginContext<'a> {
        AppLoginContext {
            scope: Scope::user(user_id),
            profile,
            credential: SmsCredential {
                phone: "19900000000",
                code,
            },
        }
    }

    /// 抓包里那个会话载荷的**形状**（脱敏值），字段名与嵌套层级照抄实测。
    fn session_payload() -> Value {
        json!({
            "account": "19900000000",
            "areaCode": "023",
            "atExpiretime": 2_592_000,
            "expiretime": "2592000",
            "authToken": "SYNTH001|1|RCS|1893456000000|AAAA",
            "deviceid": "AAA00000000000000000000000000000",
            "nationCode": "+86",
            "userDomainId": "1990000000000000001",
            "userid": "199000000000",
            "userExtInfo": [{"passID": "1990000000"}],
        })
    }

    /// 用真实实现把一个会话载荷封成上游会回的 `data` 密文。
    fn encrypted_session(crypto: &AppLoginCrypto) -> String {
        crypto
            .encrypt_body(&session_payload().to_string())
            .expect("加密是纯计算，不该失败")
    }

    /// 真实滑块挑战（2026-09-18 实测响应原文）：尺寸是**字符串**，
    /// `puzzleLeft` 用公开常量解出来是 `370`。
    fn slider_puzzle() -> Value {
        json!({"success": true, "code": "0000", "data": {
            "puzzle": "iVBORw0KGgoAAAANSUhEUgAA",
            "puzzleWidth": "96",
            "picture": "/9j/4AAQSkZJRgABAgAAAQAB",
            "picWidth": "680",
            "picHeight": "400",
            "picType": "png",
            "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="
        }})
    }

    /// 被要求滑块 → **自动解出答案**并重发。**全程不开浏览器**。
    ///
    /// 这是本模块这条改动的核心断言：App 端不再把 H5 地址甩给用户。
    #[tokio::test]
    async fn send_code_solves_the_slider_automatically() {
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059554", "message": "请先获取滑块拼图验证码", "data": {"captchaType": 2}}),
            slider_puzzle(),
            json!({"success": true, "code": "0000", "data": {"remainGetTimes": 7}}),
        ]);
        let upstream = Upstream::new(&client);

        let outcome = app_send_sms_code(
            &upstream,
            &profile(),
            "19900000000",
            &FixedClock(fixed_now()),
        )
        .await
        .unwrap();

        let AppCodeOutcome::Sent {
            remain_get_times,
            solved_slider,
        } = outcome
        else {
            panic!("第二次就该发出去，实际 {outcome:?}");
        };
        assert_eq!(remain_get_times, Some(7));
        assert!(solved_slider, "应当走了一次滑块");

        // 三次请求，顺序必须是：发码 → 取图 → 带答案重发
        let urls: Vec<String> = client.seen.borrow().iter().map(|r| r.url.clone()).collect();
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert!(urls[0].ends_with("/user/sms/getSmsCode"), "{urls:?}");
        assert!(urls[1].ends_with("/user/verfycode"), "{urls:?}");
        assert!(urls[2].ends_with("/user/sms/getSmsCode"), "{urls:?}");

        // 取图是**明文**请求，且**不带 Authorization**（实测裸请求即可用）
        let puzzle_body = client.sent_body(1);
        assert!(puzzle_body.contains("19900000000"), "{puzzle_body}");
        assert!(puzzle_body.contains("\"type\":2"), "{puzzle_body}");

        // 最强的一条：把我们**真的发出去**的密文解回来，必须是 370
        let body: Value = serde_json::from_str(&client.sent_body(2)).unwrap();
        let sent_answer = mcloud_core::aes_dynamic_iv_decrypt(
            mcloud_upstream::login::PUZZLE_AES_KEY,
            body["puzzleVerfycode"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(sent_answer.trim(), "370");

        // 首次那一条仍是空串（照抄实测，不是省略字段）
        let first: Value = serde_json::from_str(&client.sent_body(0)).unwrap();
        assert_eq!(first["puzzleVerfycode"], json!(""));

        // 不发 hcy-cool-flag：发了响应会变成解不开的密文
        assert!(
            !client.seen.borrow()[0]
                .headers
                .iter()
                .any(|(k, _)| k == "hcy-cool-flag"),
            "发了 hcy-cool-flag 会让响应变成解不开的密文"
        );
        // 时间戳是**微秒**（16 位）
        let ts = client.seen.borrow()[0]
            .headers
            .iter()
            .find(|(k, _)| k == "x-yun-user-timestamp")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(ts.len(), 16, "x-yun-user-timestamp 应是微秒：{ts}");
    }

    /// `200059553`（挑战作废）→ **必须重新取图**，不能复用旧答案。
    #[tokio::test]
    async fn send_code_refetches_the_puzzle_when_the_challenge_is_voided() {
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059554", "data": {"captchaType": 2}}),
            slider_puzzle(),
            json!({"success": false, "code": "200059553", "data": {"captchaType": 1}}),
            slider_puzzle(), // 重新取图：挑战已作废，旧答案无效
            json!({"success": true, "code": "0000", "data": {"remainGetTimes": 6}}),
        ]);
        let upstream = Upstream::new(&client);

        let outcome = app_send_sms_code(
            &upstream,
            &profile(),
            "19900000000",
            &FixedClock(fixed_now()),
        )
        .await
        .unwrap();

        let AppCodeOutcome::Sent {
            remain_get_times,
            solved_slider,
        } = outcome
        else {
            panic!("实际 {outcome:?}");
        };
        assert_eq!(remain_get_times, Some(6));
        assert!(solved_slider);

        let verfy_calls = client
            .seen
            .borrow()
            .iter()
            .filter(|r| r.url.ends_with("/user/verfycode"))
            .count();
        assert_eq!(verfy_calls, 2, "挑战作废后必须重新取图");
    }

    /// 自动过滑块也用尽 → **才**把 H5 地址交给用户（兜底，不是首选）。
    #[tokio::test]
    async fn send_code_falls_back_to_the_h5_url_after_the_attempt_limit() {
        // 每次都要滑块、每次都作废 → 到上限就停，别把配额与失败次数刷光。
        //
        // 请求序列（首次尝试**不取图** —— 它是"先空手试一次"）：
        //   发码 → [取图 → 发码] × (上限-1)
        let mut script = vec![json!({"code": "200059554", "data": {"captchaType": 2}})];
        for _ in 1..SMS_MAX_ATTEMPTS {
            script.push(slider_puzzle());
            script.push(json!({"code": "200059553", "data": {"captchaType": 1}}));
        }
        let expected_calls = 1 + (SMS_MAX_ATTEMPTS - 1) * 2;
        assert_eq!(script.len(), expected_calls);

        let client = ScriptedClient::new(script);
        let upstream = Upstream::new(&client);

        let outcome = app_send_sms_code(
            &upstream,
            &profile(),
            "19900000000",
            &FixedClock(fixed_now()),
        )
        .await
        .unwrap();

        let AppCodeOutcome::NeedCaptcha {
            url,
            attempts,
            last_answer,
            ..
        } = outcome
        else {
            panic!("到上限应降级成 H5 兜底，实际 {outcome:?}");
        };
        assert_eq!(attempts, SMS_MAX_ATTEMPTS, "不能无限试");
        assert!(
            url.starts_with("https://yun.139.com/fe/captcha/?data="),
            "{url}"
        );
        // 坐标要带出来：万一服务端要的答案口径变了，这是唯一线索
        assert_eq!(last_answer.as_deref(), Some("370"));

        assert_eq!(client.seen.borrow().len(), expected_calls, "上限是硬约束");
    }

    #[tokio::test]
    async fn send_code_reports_the_quota_when_accepted() {
        let client = ScriptedClient::new(vec![
            json!({"success": true, "code": "0000", "data": {"remainGetTimes": 4}}),
        ]);
        let upstream = Upstream::new(&client);

        let outcome = app_send_sms_code(
            &upstream,
            &profile(),
            "19900000000",
            &FixedClock(fixed_now()),
        )
        .await
        .unwrap();

        let AppCodeOutcome::Sent {
            remain_get_times,
            solved_slider,
        } = outcome
        else {
            panic!("0000 应归为 Sent");
        };
        assert_eq!(remain_get_times, Some(4));
        assert!(!solved_slider, "一次就过，没走滑块");
        assert_eq!(client.seen.borrow().len(), 1, "没要滑块就不该多打取图接口");
    }

    #[tokio::test]
    async fn login_encrypts_the_body_and_persists_the_app_session() {
        let (store, user_id) = seeded_store().await;
        let crypto = AppLoginCrypto::production();
        let client = ScriptedClient::new(vec![
            json!({"success": true, "code": "0000", "data": encrypted_session(&crypto)}),
        ]);
        let upstream = Upstream::new(&client);
        let clock = FixedClock(fixed_now());

        let profile = profile();
        let outcome = app_sms_login(
            &store,
            &upstream,
            context(&profile, &user_id, "119301"),
            &crypto,
            &clock,
            &mut SeqRandom(7),
        )
        .await
        .unwrap();

        // ① 发出去的体是**密文**：手机号与验证码都不该以明文出现
        let sent = client.sent_body(0);
        assert!(!sent.contains("19900000000"), "请求体里不能有手机号明文");
        assert!(!sent.contains("119301"), "请求体里不能有验证码明文");
        // 但能解回原样（证明发的是本端自己的密文，不是别的什么）
        let plaintext = crypto.decrypt_body(&sent).unwrap();
        assert!(plaintext.contains("119301"), "{plaintext}");
        assert!(plaintext.contains("19900000000"), "{plaintext}");

        // ② 解出的会话被如实带回
        assert_eq!(outcome.user_info["userid"], "199000000000");

        // ③ 落库：platform 必须是 `mobile` 而不是 PC 的 `pc`
        let accounts = store.list_accounts(Scope::user(&user_id)).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0].platform.as_deref(),
            Some("mobile"),
            "App 登录的 Authorization 前缀是 mobile"
        );
        assert_eq!(
            accounts[0].phone_masked.as_deref(),
            Some(mcloud_core::pure::mask_phone("19900000000").as_str()),
            "掩码口径与 core 一致（不在这里重写一遍）"
        );

        let state = store
            .get_account_state(Scope::user(&user_id), &outcome.account_id)
            .await
            .unwrap()
            .expect("登录态应已落库");
        assert_eq!(
            state.user_domain_id.as_deref(),
            Some("1990000000000000001"),
            "App 的 userDomainId 要能落进 user_domain_id"
        );
        assert_eq!(
            state.jwt_token.as_deref(),
            Some("SYNTH001|1|RCS|1893456000000|AAAA")
        );
        assert_eq!(
            state.device_id.as_deref(),
            Some("AAA00000000000000000000000000000")
        );
    }

    #[tokio::test]
    async fn sms_code_wrong_is_not_reported_as_our_bug() {
        let (store, user_id) = seeded_store().await;
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "9441", "message": "验证码不正确"}),
        ]);
        let upstream = Upstream::new(&client);

        let profile = profile();
        let err = app_sms_login(
            &store,
            &upstream,
            context(&profile, &user_id, "000000"),
            &AppLoginCrypto::production(),
            &FixedClock(fixed_now()),
            &mut SeqRandom(1),
        )
        .await
        .unwrap_err();

        // 9441 是用户的问题 → BadRequest（可重试、可提示），而不是 Internal
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(format!("{err}").contains("9441"), "{err}");
    }

    #[tokio::test]
    async fn undecryptable_body_is_reported_as_our_bug() {
        let (store, user_id) = seeded_store().await;
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "9103", "message": "输入参数不合法"}),
        ]);
        let upstream = Upstream::new(&client);

        let profile = profile();
        let err = app_sms_login(
            &store,
            &upstream,
            context(&profile, &user_id, "119301"),
            &AppLoginCrypto::production(),
            &FixedClock(fixed_now()),
            &mut SeqRandom(1),
        )
        .await
        .unwrap_err();

        // 9103 是**我们**的加密参数不对 → Internal，免得前端渲染成"请检查你输入的参数"
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(format!("{err}").contains("9103"), "{err}");
    }

    #[tokio::test]
    async fn login_sends_the_full_app_header_set() {
        let (store, user_id) = seeded_store().await;
        let crypto = AppLoginCrypto::production();
        let client = ScriptedClient::new(vec![
            json!({"success": true, "code": "0000", "data": encrypted_session(&crypto)}),
        ]);
        let upstream = Upstream::new(&client);

        let profile = profile();
        app_sms_login(
            &store,
            &upstream,
            context(&profile, &user_id, "119301"),
            &crypto,
            &FixedClock(fixed_now()),
            &mut SeqRandom(3),
        )
        .await
        .unwrap();

        let request = client.seen.borrow();
        let request = &request[0];
        assert_eq!(request.url, "https://user-njs.yun.139.com/user/thirdlogin");
        // 内容类型只出现一次（`with_app_headers` 会先删掉 `json_body` 塞的那个）
        let content_types: Vec<&str> = request
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(
            content_types,
            vec!["application/json;charset=UTF-8"],
            "不能出现两个 Content-Type"
        );
        // 关键的 App 端头都在
        for name in [
            "service_identifier_key",
            "x-DeviceInfo",
            "x-yun-client-info",
            "x-UserAgent",
            "x-DeviceName",
        ] {
            assert!(
                request.headers.iter().any(|(k, _)| k == name),
                "缺头 {name}"
            );
        }
        // 首次登录没有 userDomainId，因此不该带 x-yun-uni
        assert!(!request.headers.iter().any(|(k, _)| k == "x-yun-uni"));
    }

    /// 把一个手机号落成带 `user_domain_id` 的账号。
    ///
    /// 不经真实登录：这里要测的是"回填"，把网络脚本一起拖进来只会让失败原因变模糊。
    async fn seed_domain_id(store: &mcloud_io::SqliteStore, user_id: &UserId, domain: &str) {
        let id = AccountId::new(mcloud_core::account_label(None, "19900000000"));
        let mut account = mcloud_core::Account::new(id.as_str());
        account.owner = Some(user_id.clone());
        store
            .upsert_account(Scope::user(user_id), &account)
            .await
            .unwrap();
        let state = mcloud_core::AccountState {
            user_domain_id: Some(domain.to_string()),
            ..mcloud_core::AccountState::new(id.as_str())
        };
        store
            .upsert_account_state(Scope::user(user_id), &state)
            .await
            .unwrap();
    }

    /// 回填这条线**为什么存在**：`thirdlogin` 之后 `userDomainId` 就入库了，
    /// 第二次起的 App 请求要带着它（`x-yun-uni`）。没接上之前，已登录的 App
    /// 请求会静默缺这个头（`docs/rust-rewrite-plan.md` §7.2 记的缺口）。
    #[tokio::test]
    async fn a_known_account_gets_its_domain_id_back() {
        let (store, user_id) = seeded_store().await;
        seed_domain_id(&store, &user_id, "1990000000000000001").await;

        let filled =
            profile_with_stored_domain_id(&store, Scope::user(&user_id), profile(), "19900000000")
                .await
                .unwrap();

        assert_eq!(filled.user_domain_id(), Some("1990000000000000001"));
        // 档案里有值还不算，要真的落到发出去的那个头上
        assert!(
            filled
                .headers(user_timestamp(&FixedClock(fixed_now())))
                .iter()
                .any(|(k, v)| k == "x-yun-uni" && v == "1990000000000000001"),
            "回填的值必须进请求头，而不是只存进档案"
        );
    }

    /// 三种"没有"一律原样返回档案：库里查无此账号、那一列是空串、账号属于别人。
    ///
    /// 编一个值会让上游把这次请求定位到**别人**的用户域，比缺这个头更糟。
    #[tokio::test]
    async fn nothing_is_invented_when_the_domain_id_is_absent() {
        let (store, user_id) = seeded_store().await;
        let phone = "19900000000";

        // ① 首次登录：库里还没有这个账号
        let bare = profile_with_stored_domain_id(&store, Scope::user(&user_id), profile(), phone)
            .await
            .unwrap();
        assert_eq!(bare.user_domain_id(), None, "查无此账号时不要带 x-yun-uni");

        // ② 有账号，但那一列是空串（上游没给 / 旧数据）
        seed_domain_id(&store, &user_id, "").await;
        let empty = profile_with_stored_domain_id(&store, Scope::user(&user_id), profile(), phone)
            .await
            .unwrap();
        assert_eq!(empty.user_domain_id(), None, "空串不等于有值");

        // ③ 同一个手机号的数据属于别人 —— 按隔离契约读不到，也就不能带出去
        let other = User::new("u2", "bob", "!", mcloud_core::Role::User, fixed_now());
        store.upsert_user(&other).await.unwrap();
        let alien = profile_with_stored_domain_id(&store, Scope::user(&other.id), profile(), phone)
            .await
            .unwrap();
        assert_eq!(alien.user_domain_id(), None, "别人的账号不能回填进来");
    }

    #[test]
    fn user_timestamp_is_microseconds_not_milliseconds() {
        let ts = user_timestamp(&FixedClock(fixed_now()));
        // 16 位是微秒；13 位才是毫秒
        assert_eq!(ts.to_string().len(), 16, "{ts}");
        assert_eq!(ts, fixed_now().timestamp_micros());
    }
}
