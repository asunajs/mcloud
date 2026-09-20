//! 登录的**驱动**：把能力层的状态机接到真实的等待与渲染上。
//!
//! # 为什么等待在这里、而不在状态机里
//!
//! `mcloud-capabilities::login` 刻意不 sleep —— 它只回答"下次隔多久、要不要停"。
//! 于是"每秒问一次"这句产品行为落在本层：CLI 用 tokio 定时器，
//! WebUI 用它自己的推送节奏。**两个前端共用的仍然是同一份状态机**，
//! 差别只在"谁来等"和"渲染成什么"（后者通过 [`LoginObserver`] 注入）。
//!
//! # 凭据边界
//!
//! [`LoginOutcome::user_info`] 里带着 `authToken`：它**不打印、不写日志**，
//! 只用于落库与即时展示摘要。

use chrono::DateTime;
use mcloud_capabilities::login::{QrFailure, QrLogin, QrProgress, persist_login};
use mcloud_core::{AccountId, Clock, Error, HttpClient, Random, Scope, StateStore};
use mcloud_upstream::Upstream;
use mcloud_upstream::login::{LoginPayloadDecryptor, PcPlatform, SendSmsOutcome};
use serde_json::Value;

/// 登录成功的结果。
#[derive(Debug, Clone)]
pub struct LoginOutcome {
    pub account_id: AccountId,
    /// 已解密的 `userInfo`。**含凭据**，只用于落库与展示摘要。
    pub user_info: Value,
}

impl LoginOutcome {
    /// 展示用摘要：只给手机号掩码这类可以直接印在终端上的信息。
    pub fn summary(&self) -> String {
        let account = self
            .user_info
            .get("account")
            .and_then(Value::as_str)
            .unwrap_or("");
        if account.is_empty() {
            self.account_id.to_string()
        } else {
            mcloud_core::pure::mask_phone(account)
        }
    }
}

/// 登录过程的观察者：二维码只给一次，进展每次轮询后给一次。
///
/// 做成 trait 而不是两个闭包参数：CLI 要打印到终端、WebUI 要推给浏览器，
/// 两者的状态（比如"已经推过了"）各自持有，接口只需要这两个动作。
pub trait LoginObserver {
    /// 拿到二维码载荷（**只调一次**，在开始轮询之前）。
    fn on_qr(&self, payload: &str);
    /// 每次轮询后的进展（含终态）。
    fn on_progress(&self, progress: &QrProgress);
}

/// 一次登录里不变的那部分：**以谁的身份、冒充哪个端、设备指纹是什么**。
///
/// 单独成结构而不是三个参数：这三样凑齐才是"一次登录"（凭据要落到谁名下、
/// 上游看到的是哪一端、服务端靠哪个 dID 定位会话），
/// 分开传容易在调用点被摆错顺序 —— 而它们都是字符串/枚举，摆错了编译器不会吭声。
#[derive(Debug, Clone, Copy)]
pub struct LoginContext<'a> {
    /// 凭据归属谁 —— 由 [`Scope`] 表达。
    ///
    /// WebUI 用 `Scope::user(当前会话用户)`；CLI 用 `Scope::Local`
    /// （本地运维不替谁认领，账号保持无主、等首个管理员初始化时认领）。
    pub scope: Scope<'a>,
    /// 冒充哪个客户端（影响 `clientType` / `cpid` / 版本号）。
    pub platform: PcPlatform,
    /// 设备指纹（`windowMachine`）—— **必须持久化且永不漂移**。
    pub device_id: &'a str,
    /// 二维码等待窗口（秒）——**只有扫码流程读它**，所以字段私有，
    /// 用 [`LoginContext::with_ttl`] 改。短信流程写 `LoginContext::new(..)` 即可，
    /// 不会被这个只属于另一条链路的旋钮打扰。
    ttl_seconds: i64,
}

impl<'a> LoginContext<'a> {
    /// 常规上下文（二维码有效期取默认 120 秒）。
    pub const fn new(scope: Scope<'a>, platform: PcPlatform, device_id: &'a str) -> Self {
        Self {
            scope,
            platform,
            device_id,
            ttl_seconds: mcloud_capabilities::login::QR_TTL_SECONDS,
        }
    }

    /// 放宽二维码有效期。用于"把码发给别人扫"（终端打印、远程协助）。
    pub const fn with_ttl(mut self, seconds: i64) -> Self {
        self.ttl_seconds = seconds;
        self
    }
}

/// 什么都不做的观察者（非交互场景与测试用）。
pub struct SilentObserver;

impl LoginObserver for SilentObserver {
    fn on_qr(&self, _payload: &str) {}
    fn on_progress(&self, _progress: &QrProgress) {}
}

/// 扫码登录的完整驱动：出码 → 轮询（实测 1000ms 间隔）→ 成功后落库。
///
/// 退出条件全部交给状态机（[`QrLogin::next_delay_ms`]）：
/// 成功、失败、超时三种情况都返回 `None`，所以这个循环**不可能**出现
/// "该停没停"（客户端源码专门为它写过 `clearInterval` 的提醒）。
pub async fn qr_login<S, C, D, K, O>(
    store: &S,
    upstream: &Upstream<'_, C>,
    context: LoginContext<'_>,
    decryptor: &D,
    clock: &K,
    rng: &mut impl Random,
    observer: &O,
) -> Result<LoginOutcome, Error>
where
    S: StateStore,
    C: HttpClient,
    D: LoginPayloadDecryptor,
    K: Clock,
    O: LoginObserver,
{
    let LoginContext {
        scope,
        platform,
        device_id,
        ttl_seconds,
    } = context;

    let mut qr = QrLogin::start(platform, device_id, rng, clock.now()).with_ttl(ttl_seconds);
    observer.on_qr(&qr.qr_payload());

    loop {
        // 每个请求一个新的 `x-yun-tid`（实测要求）
        let tid = rng.uuid_v4();
        let progress = qr.poll(upstream, &tid, decryptor, clock.now()).await?;
        observer.on_progress(&progress);

        match progress {
            QrProgress::LoggedIn { user_info } => {
                let account_id =
                    persist_login(store, scope, device_id, &user_info, clock.now()).await?;
                return Ok(LoginOutcome {
                    account_id,
                    user_info,
                });
            }
            QrProgress::NotScanned | QrProgress::Scanned => {}
            QrProgress::Failed { reason } => {
                // 超时单独给 `Timeout`：调用方（脚本 / 监控）据此区分"等太久了"
                // 与"被上游拒了"，混进 BadRequest 会让两者不可区分。
                if let QrFailure::TtlElapsed { .. } = reason {
                    return Err(Error::Timeout(
                        "二维码已超时（本轮不再等待）。重新执行即可换一张新码".into(),
                    ));
                }
                // 限流：**退避后继续**，不是终态。
                //
                // 会话还有效，服务端只是要求慢一点；把用户赶回去重开一张码，
                // 只会让他更深地撞进风控（实测连着跑几次就是这个下场）。
                // 总时长仍受 TTL 约束，所以不会无限等下去。
                if let QrFailure::TooFrequent = reason {
                    tokio::time::sleep(TOO_FREQUENT_BACKOFF).await;
                    continue;
                }
                return Err(Error::BadRequest(format!(
                    "扫码登录未成功：{reason:?}。重新执行即可换一张新码"
                )));
            }
        }

        // 兜底：状态机的 `None` 是"别再问了"的唯一裁判（终态与超时都在上面处理过）。
        // 留着这一支是为了让"某天状态机多加了一种停止条件"时不会变成死循环。
        let Some(delay_ms) = qr.next_delay_ms(clock.now()) else {
            return Err(Error::Timeout(
                "二维码已超时（本轮不再等待）。重新执行即可换一张新码".into(),
            ));
        };
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }
}

/// 被限频（`200059545 扫码登录过于频繁`）后的退避时长。
///
/// 30 秒是**拍的经验值**：太短会继续撞风控，太长则用户以为程序卡住了。
/// 真正的上限由 TTL 兜着（TTL 到了就结束，所以不会无限等）。
pub const TOO_FREQUENT_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// 短信验证码发码的最大尝试次数。
///
/// 滑块挑战有失败上限（实测 `200059553` 之后挑战作废），无限重试只会在风控那里
/// 越陷越深。3 次是**经验值**：够覆盖"一次判废后重取图"，又不至于把配额刷掉。
pub const SMS_MAX_ATTEMPTS: usize = 3;

/// 发短信验证码的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmsCodeSent {
    /// 配额余量（上游给的 `remainGetTimes`）。
    pub remain_get_times: Option<u32>,
    /// 是否经过了一次滑块校验（用于给用户一句"过了一次风控"的提示）。
    pub solved_slider: bool,
}

/// 发短信验证码；被风控要求滑块时**自动解出答案并重试**。
///
/// # 为什么这段循环在共享层
///
/// "被要求滑块 → 取图 → 解坐标 → 带答案重发 → 挑战作废则重取图"是**协议语义**，
/// 不是某个前端的交互细节。放这里，CLI 与 WebUI 才共用同一份，
/// 而不是各自演化出"一个会重取图、另一个不会"的差异。
///
/// 每次尝试用新的 `x-yun-tid` 与新的 `random`（实测客户端就是这么做的）。
pub async fn sms_send_code<C: HttpClient>(
    upstream: &Upstream<'_, C>,
    platform: PcPlatform,
    device_id: &str,
    phone: &str,
    rng: &mut impl Random,
) -> Result<SmsCodeSent, Error> {
    let mut sms = mcloud_capabilities::login::SmsLogin::start(platform, device_id, phone, rng);
    let mut solved_slider = false;
    // 记下最后发上去的坐标：滑块这条路目前是**强推断**（见 `SliderPuzzle::answer`），
    // 万一推断错了，失败信息里能直接看到"我们发了什么"，比一句"滑块未通过"有用得多。
    let mut last_answer: Option<String> = None;

    for attempt in 1..=SMS_MAX_ATTEMPTS {
        let tid = rng.uuid_v4();
        let outcome = if solved_slider {
            // 每次重新取图再解：`200059553` 的语义是**这个挑战作废了**，
            // 复用上一轮的答案只会再失败一次。
            let answer = sms.solve_slider(upstream, &rng.uuid_v4()).await?;
            last_answer = Some(answer.clone());
            sms.send_code_with_slider(upstream, &tid, &answer).await?
        } else {
            sms.send_code(upstream, &tid).await?
        };

        match outcome {
            SendSmsOutcome::Sent { remain_get_times } => {
                return Ok(SmsCodeSent {
                    remain_get_times,
                    solved_slider,
                });
            }
            // 需要滑块、或旧挑战已作废：都要**重新取图**
            SendSmsOutcome::NeedSlider { .. } | SendSmsOutcome::SliderExhausted { .. } => {
                if attempt == SMS_MAX_ATTEMPTS {
                    return Err(Error::RateLimited(format!(
                        "连续 {SMS_MAX_ATTEMPTS} 次滑块未通过，已放弃（最后发上去的坐标：{last_answer:?}）。\
                         若坐标明显不对，说明服务端要的答案口径与自动解出的不同 —— \
                         请把这一行留证。稍后再试，或改用扫码登录"
                    )));
                }
                solved_slider = true;
                sms.reroll(rng);
            }
            SendSmsOutcome::Other { code, message } => {
                // 原样带回上游的码与文案：不臆造"手机号错误"这类我们并不知道的结论
                return Err(Error::BadRequest(match code {
                    Some(code) => format!("发送验证码失败 code={code}: {message}"),
                    None => format!("发送验证码失败: {message}"),
                }));
            }
        }
    }

    // 结构上到不了：上面每个分支都返回。
    // 这里用一句防御性错误而不是 `unreachable!`：免得哪天有人把
    // `SMS_MAX_ATTEMPTS` 改成 0，把一个配置笔误变成一次 panic。
    Err(Error::Internal("发码流程没有产出结果".into()))
}

/// 短信登录的凭据：**手机号 + 收到的验证码**。
///
/// 合成一个类型，是因为它们总是一起出现、一起使用；而两个并排的 `&str`
/// 参数最容易在调用点被写反 —— 写反了编译器不会吭声。
#[derive(Debug, Clone, Copy)]
pub struct SmsCredential<'a> {
    pub phone: &'a str,
    /// 6 位明文验证码（短信方式下 `dycpwd` 就是明文，不是哈希）。
    pub code: &'a str,
}

/// 用短信验证码登录并落库。
///
/// 与 [`qr_login`] 共用同一套落库路径（[`persist_login`]）：两者最终都产出
/// `auth`，差别只在拿到它的方式。
pub async fn sms_login<S, C, D, K>(
    store: &S,
    upstream: &Upstream<'_, C>,
    context: LoginContext<'_>,
    credential: SmsCredential<'_>,
    decryptor: &D,
    clock: &K,
    rng: &mut impl Random,
) -> Result<LoginOutcome, Error>
where
    S: StateStore,
    C: HttpClient,
    D: LoginPayloadDecryptor,
    K: Clock,
{
    let LoginContext {
        scope,
        platform,
        device_id,
        ..
    } = context;

    let tid = rng.uuid_v4();
    let sms =
        mcloud_capabilities::login::SmsLogin::start(platform, device_id, credential.phone, rng);
    let user_info = sms
        .login(upstream, &tid, credential.code, decryptor)
        .await?;

    let account_id = persist_login(store, scope, device_id, &user_info, clock.now()).await?;
    Ok(LoginOutcome {
        account_id,
        user_info,
    })
}

/// 会话过期时刻。
///
/// CLI 的登录态"可长期有效"（本机单机），但仍然给一个明确的到期时间 ——
/// 无到期的会话在库里无法清理，也无法回答"这个登录态还能用多久"。
pub fn session_expiry(now: DateTime<chrono::Utc>, days: i64) -> DateTime<chrono::Utc> {
    now + chrono::Duration::days(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use mcloud_core::{HttpRequest, HttpResponse, StateStore, User, UserId, UserStore};
    use serde_json::json;
    use std::cell::RefCell;

    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    /// 固定时钟：TTL 不会到期，用来测"成功就停"。
    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// 每次读表都往前跳一小时的时钟：用来造"已经超时"，**不依赖真的等一小时**。
    ///
    /// 注意不能用 `Utc::now()`：`start_paused` 只把 tokio 的定时器虚拟化，
    /// chrono 读的仍是真实的系统时钟，那样根本造不出超时。
    struct JumpingClock(RefCell<i64>);

    impl JumpingClock {
        fn new() -> Self {
            Self(RefCell::new(0))
        }
    }

    impl Clock for JumpingClock {
        fn now(&self) -> DateTime<Utc> {
            let mut hours = self.0.borrow_mut();
            *hours += 1;
            fixed_now() + Duration::hours(*hours)
        }
    }

    /// 逐次返回脚本响应的假客户端。
    struct ScriptedClient {
        responses: RefCell<Vec<Value>>,
        sent: RefCell<usize>,
        /// 发出去的请求 —— 断言"发了什么、发了几次"时用。
        seen: RefCell<Vec<HttpRequest>>,
    }

    impl ScriptedClient {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: RefCell::new(responses),
                sent: RefCell::new(0),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl HttpClient for ScriptedClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
            *self.sent.borrow_mut() += 1;
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

    struct PlainDecryptor;

    impl LoginPayloadDecryptor for PlainDecryptor {
        fn decrypt_user_info(&self, ciphertext: &str) -> Result<Value, Error> {
            serde_json::from_str(ciphertext).map_err(|e| Error::Parse(e.to_string()))
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        qr: RefCell<Vec<String>>,
        progress: RefCell<Vec<QrProgress>>,
    }

    impl LoginObserver for RecordingObserver {
        fn on_qr(&self, payload: &str) {
            self.qr.borrow_mut().push(payload.to_string());
        }
        fn on_progress(&self, progress: &QrProgress) {
            self.progress.borrow_mut().push(progress.clone());
        }
    }

    fn user_info() -> Value {
        json!({
            "account": "19900000000",
            "authToken": "SYNTH001|1|RCS|1893456000000|AAAA",
            "userId": "1990000000000000001"
        })
    }

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap()
    }

    async fn seeded_store() -> (mcloud_io::SqliteStore, UserId) {
        let s = mcloud_io::SqliteStore::open_in_memory().unwrap();
        let user = User::new("u1", "alice", "!", mcloud_core::Role::Admin, fixed_now());
        s.upsert_user(&user).await.unwrap();
        (s, user.id)
    }

    #[tokio::test(start_paused = true)]
    async fn qr_login_walks_the_loop_and_persists_on_success() {
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059541"}),
            json!({"success": false, "code": "200059548"}),
            json!({"success": true, "code": "0000", "data": user_info().to_string()}),
        ]);
        let upstream = Upstream::new(&client);
        let (store, user_id) = seeded_store().await;
        let observer = RecordingObserver::default();

        let outcome = qr_login(
            &store,
            &upstream,
            LoginContext::new(Scope::user(&user_id), PcPlatform::Windows, "DEV-1"),
            &PlainDecryptor,
            &FixedClock(fixed_now()),
            &mut SeqRandom(0),
            &observer,
        )
        .await
        .unwrap();

        assert_eq!(outcome.account_id.as_str(), "19****00");
        assert_eq!(outcome.summary(), "19****00");
        assert!(
            !outcome.summary().contains("19900000000"),
            "摘要不得带出完整手机号"
        );

        // 二维码只给了一次，且带着会话 id。
        // `clone()` 是为了让临时借用立刻结束：在 async fn 里抱着 `RefCell` 的 guard
        // 跨过 await 点是 clippy 会拦的事（运行时可能 panic）。
        let qr = observer.qr.borrow().clone();
        assert_eq!(qr.len(), 1);
        assert!(qr[0].contains("sID="));

        // 进展按实测的顺序：未扫 → 已扫 → 成功
        let progress = observer.progress.borrow().clone();
        assert_eq!(progress.len(), 3);
        assert_eq!(progress[0], QrProgress::NotScanned);
        assert_eq!(progress[1], QrProgress::Scanned);
        assert!(matches!(progress[2], QrProgress::LoggedIn { .. }));

        // 三次请求，一条不多
        assert_eq!(*client.sent.borrow(), 3);

        // 落库：账号归属于登录者，登录态带上了凭据
        let account = store
            .get_account(Scope::user(&user_id), &outcome.account_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(account.owner.as_ref(), Some(&user_id));
        let state = store
            .get_account_state(Scope::user(&user_id), &outcome.account_id)
            .await
            .unwrap()
            .unwrap();
        assert!(state.jwt_token.is_some());
        assert_eq!(state.device_id.as_deref(), Some("DEV-1"));
    }

    /// 限频（`200059545`）**不是**终态：退避后继续问。
    ///
    /// 这条是实测换来的：连着跑几次扫码登录会被限频，而当时的实现把它当终态 → 直接失败。
    /// 用户看到"登录失败"就会重开一张码，于是更深地撞进风控。
    #[tokio::test(start_paused = true)]
    async fn qr_login_backs_off_on_rate_limit_instead_of_giving_up() {
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059545", "message": "扫码登录过于频繁"}),
            json!({"success": false, "code": "200059545", "message": "扫码登录过于频繁"}),
            json!({"success": true, "code": "0000", "data": user_info().to_string()}),
        ]);
        let upstream = Upstream::new(&client);
        let (store, user_id) = seeded_store().await;
        let observer = RecordingObserver::default();

        let outcome = qr_login(
            &store,
            &upstream,
            LoginContext::new(Scope::user(&user_id), PcPlatform::Windows, "DEV-1"),
            &PlainDecryptor,
            &FixedClock(fixed_now()),
            &mut SeqRandom(0),
            &observer,
        )
        .await
        .expect("限频之后必须继续尝试，而不是放弃");

        assert_eq!(outcome.account_id.as_str(), "19****00");
        assert_eq!(*client.sent.borrow(), 3, "限频两次之后应继续问到成功");

        // 观察者要能看到限频事实，前端才能提示"等一会儿"
        let progress = observer.progress.borrow().clone();
        assert!(progress.iter().any(|p| matches!(
            p,
            QrProgress::Failed {
                reason: QrFailure::TooFrequent
            }
        )));
    }

    #[tokio::test(start_paused = true)]
    async fn qr_login_stops_when_the_code_expires() {
        // 空脚本：一旦真发请求就会报"脚本已用完"
        let client = ScriptedClient::new(vec![]);
        let upstream = Upstream::new(&client);
        let (store, user_id) = seeded_store().await;

        let err = qr_login(
            &store,
            &upstream,
            LoginContext::new(Scope::user(&user_id), PcPlatform::Windows, "DEV-1"),
            &PlainDecryptor,
            &JumpingClock::new(),
            &mut SeqRandom(0),
            &SilentObserver,
        )
        .await
        .expect_err("超时必须结束循环");

        assert_eq!(err.kind(), mcloud_core::ErrorKind::Timeout, "{err}");
        assert_eq!(*client.sent.borrow(), 0, "超时不该再打扰上游");
    }

    #[tokio::test(start_paused = true)]
    async fn qr_login_reports_terminal_failure_instead_of_spinning() {
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059542"}),
            // 若循环没停，就会用到这里
            json!({"success": false, "code": "200059541"}),
        ]);
        let upstream = Upstream::new(&client);
        let (store, user_id) = seeded_store().await;

        let err = qr_login(
            &store,
            &upstream,
            LoginContext::new(Scope::user(&user_id), PcPlatform::Windows, "DEV-1"),
            &PlainDecryptor,
            &FixedClock(fixed_now()),
            &mut SeqRandom(0),
            &SilentObserver,
        )
        .await
        .expect_err("码失效必须结束");

        assert!(format!("{err}").contains("ExpiredUpstream"), "{err}");
        assert_eq!(*client.sent.borrow(), 1, "终态之后不得继续轮询");
    }

    /// 发码成功、不需要滑块：一次请求搞定。
    #[tokio::test(start_paused = true)]
    async fn sms_send_code_sends_directly_when_no_slider_is_required() {
        let client = ScriptedClient::new(vec![
            json!({"success": true, "code": "0000", "data": {"remainGetTimes": 8}}),
        ]);
        let upstream = Upstream::new(&client);

        let sent = sms_send_code(
            &upstream,
            PcPlatform::Windows,
            "DEV-1",
            "19900000000",
            &mut SeqRandom(0),
        )
        .await
        .unwrap();

        assert_eq!(sent.remain_get_times, Some(8));
        assert!(!sent.solved_slider);
        assert_eq!(*client.sent.borrow(), 1);
        let body: Value =
            serde_json::from_slice(client.seen.borrow()[0].body.as_ref().unwrap()).unwrap();
        // 实测 `mode`/`reqType` 是固定值，`random` 是 24 位十六进制（不是 UUID）
        assert_eq!(body["mode"], 0);
        assert_eq!(body["reqType"], 3);
        assert_eq!(body["random"].as_str().unwrap().len(), 24);
        assert!(body.get("puzzleVerfycode").is_none(), "首次不该带滑块答案");
    }

    /// 被要求滑块 → **自动解出答案**并重发，且发出去的答案必须能解回正确坐标。
    #[tokio::test(start_paused = true)]
    async fn sms_send_code_solves_the_slider_automatically() {
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059554", "message": "请先获取滑块拼图验证码", "data": {"captchaType": 2}}),
            // 真实响应形状（2026-09-18 实测）：尺寸是字符串，puzzleLeft 解出来是 370
            json!({"success": true, "code": "0000", "data": {
                "puzzle": "iVBORw0KGgoAAAANSUhEUgAA",
                "puzzleWidth": "96",
                "picture": "/9j/4AAQSkZJRgABAgAAAQAB",
                "picWidth": "680",
                "picHeight": "400",
                "picType": "png",
                "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="
            }}),
            json!({"success": true, "code": "0000", "data": {"remainGetTimes": 7}}),
        ]);
        let upstream = Upstream::new(&client);

        let sent = sms_send_code(
            &upstream,
            PcPlatform::Windows,
            "DEV-1",
            "19900000000",
            &mut SeqRandom(0),
        )
        .await
        .unwrap();

        assert_eq!(sent.remain_get_times, Some(7));
        assert!(sent.solved_slider, "应当走了一次滑块");
        assert_eq!(*client.sent.borrow(), 3, "发码 → 取图 → 带答案重发");

        let urls: Vec<String> = client.seen.borrow().iter().map(|r| r.url.clone()).collect();
        assert!(urls[0].ends_with("/user/sms/getSmsCode"), "{urls:?}");
        assert!(urls[1].ends_with("/user/verfycode"), "{urls:?}");
        assert!(urls[2].ends_with("/user/sms/getSmsCode"), "{urls:?}");

        // 最强的一条：把我们**真的发出去**的密文解回来，必须是 370
        let body: Value =
            serde_json::from_slice(client.seen.borrow()[2].body.as_ref().unwrap()).unwrap();
        let sent_answer = mcloud_core::aes_dynamic_iv_decrypt(
            mcloud_upstream::login::PUZZLE_AES_KEY,
            body["puzzleVerfycode"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(sent_answer.trim(), "370");
    }

    /// `200059553`（挑战作废）→ **必须重新取图**，不能复用旧答案。
    #[tokio::test(start_paused = true)]
    async fn sms_send_code_refetches_the_puzzle_when_the_challenge_is_voided() {
        let puzzle = json!({"success": true, "code": "0000", "data": {
            "puzzle": "a", "picture": "b",
            "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="
        }});
        let client = ScriptedClient::new(vec![
            json!({"success": false, "code": "200059554", "data": {"captchaType": 2}}),
            puzzle.clone(),
            json!({"success": false, "code": "200059553", "data": {"captchaType": 1}}),
            puzzle, // 重新取图：挑战已作废，旧答案无效
            json!({"success": true, "code": "0000", "data": {"remainGetTimes": 6}}),
        ]);
        let upstream = Upstream::new(&client);

        let sent = sms_send_code(
            &upstream,
            PcPlatform::Windows,
            "DEV-1",
            "19900000000",
            &mut SeqRandom(0),
        )
        .await
        .unwrap();
        assert_eq!(sent.remain_get_times, Some(6));

        let verfy_calls = client
            .seen
            .borrow()
            .iter()
            .filter(|r| r.url.ends_with("/user/verfycode"))
            .count();
        assert_eq!(verfy_calls, 2, "挑战作废后必须重新取图");
    }

    #[tokio::test(start_paused = true)]
    async fn sms_send_code_gives_up_after_the_attempt_limit() {
        // 每次都要滑块、每次都作废 → 到上限就停，别把配额刷光
        let client = ScriptedClient::new(vec![
            json!({"code": "200059554", "data": {"captchaType": 2}}),
            json!({"code": "0000", "data": {"puzzle": "a", "picture": "b", "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="}}),
            json!({"code": "200059553"}),
            json!({"code": "0000", "data": {"puzzle": "a", "picture": "b", "puzzleLeft": "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg="}}),
            json!({"code": "200059553"}),
        ]);
        let upstream = Upstream::new(&client);

        let err = sms_send_code(
            &upstream,
            PcPlatform::Windows,
            "DEV-1",
            "19900000000",
            &mut SeqRandom(0),
        )
        .await
        .expect_err("到上限必须放弃");
        assert_eq!(err.kind(), mcloud_core::ErrorKind::RateLimited, "{err}");
        assert!(
            format!("{err}").contains("扫码登录"),
            "要给出替代路径: {err}"
        );
    }

    /// 上游业务失败要**原样带回它的码与文案**：不臆造"手机号错误"这类我们并不知道的结论。
    ///
    /// 这里用的 `5010047` 是 2026-09-18 用非法号码真实调用发码接口观察到的
    /// （见 `docs/upstream-logout-sms-login.md` §4）。
    #[tokio::test(start_paused = true)]
    async fn sms_send_code_surfaces_upstream_failure_verbatim() {
        let client = ScriptedClient::new(vec![json!({
            "success": false,
            "code": "5010047",
            "message": "国家码和手机号不匹配或手机号格式错误"
        })]);
        let upstream = Upstream::new(&client);

        let err = sms_send_code(
            &upstream,
            PcPlatform::Windows,
            "DEV-1",
            "0000",
            &mut SeqRandom(0),
        )
        .await
        .unwrap_err();
        let detail = format!("{err}");
        assert!(detail.contains("5010047"), "{detail}");
        assert!(detail.contains("国家码和手机号不匹配"), "{detail}");
    }

    #[tokio::test(start_paused = true)]
    async fn sms_login_decrypts_and_persists() {
        let client = ScriptedClient::new(vec![
            json!({"success": true, "code": "0000", "data": user_info().to_string()}),
        ]);
        let upstream = Upstream::new(&client);
        let (store, user_id) = seeded_store().await;

        let outcome = sms_login(
            &store,
            &upstream,
            LoginContext::new(Scope::user(&user_id), PcPlatform::Windows, "DEV-1"),
            SmsCredential {
                phone: "19900000000",
                code: "119301",
            },
            &PlainDecryptor,
            &FixedClock(fixed_now()),
            &mut SeqRandom(0),
        )
        .await
        .unwrap();

        assert_eq!(outcome.account_id.as_str(), "19****00");
        let state = store
            .get_account_state(Scope::user(&user_id), &outcome.account_id)
            .await
            .unwrap()
            .unwrap();
        assert!(state.jwt_token.is_some());

        let body: Value =
            serde_json::from_slice(client.seen.borrow()[0].body.as_ref().unwrap()).unwrap();
        // 短信登录与扫码共用 /user/thirdlogin，靠 pintype 区分
        assert_eq!(body["pintype"], "5");
        assert_eq!(body["dycpwd"], "119301", "短信方式下 dycpwd 是明文验证码");
        assert_eq!(body["msisdn"], "19900000000");
        assert_eq!(body["extInfo"]["ifOpenAccount"], 1);
    }

    #[test]
    fn session_expiry_is_an_explicit_date_not_infinity() {
        let now = fixed_now();
        let expires = session_expiry(now, 30);
        assert_eq!(expires, now + Duration::days(30));
        // 无到期的会话既清不掉、也答不了"还能用多久"，所以这里必须是个具体时刻
        assert!(expires > now);
    }
}
