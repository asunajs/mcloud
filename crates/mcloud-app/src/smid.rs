//! 账号绑定的 smid 设备特征，以及"拿特征换 `deviceId`"的编排。
//!
//! # 这一层管的是生命周期
//!
//! 纯算法在 `mcloud_upstream::smid`，存储契约在 `mcloud_core::SmidState`
//! 与 store 的那三条窄写方法上。中间有一件事谁都没接：**什么时候生成、
//! 什么时候复用、什么时候作废**。本层就是那件事的答案。
//!
//! ```text
//! 首次使用  → 生成特征 → 落库（绑定）→ 上报换 deviceId → 落库
//! 之后每次  → 读库复用，不打网络
//! 主动更换  → 重新生成特征 + 清空旧 deviceId（等于换一台设备）
//! 重新登录  → 这两列都不动（换凭据不该换设备长相）
//! ```
//!
//! # 为什么绑定值得单独一层
//!
//! 旧实现（`packages/shared/src/smid/`）每次上报都现掷一份新特征，于是同一个账号
//! 在风控服务眼里**每次都换一台设备** —— 那正是最容易触发核验的姿态。
//! 绑定的实际收益就是上面那张图：上报从"每次请求"降到"每个账号一次"，
//! 而换 id 时旧 id 会进特征体的 `box`，让服务端看出"还是那台机器在换 id"。
//!
//! 算法能被参数化（`SmFingerprint` 是个普通结构体）是这件事的前提：它必须能原样
//! 序列化回库、再原样读出来。那条往返由上游的
//! `the_bound_fingerprint_round_trips_through_the_feature_body` 把住，
//! 这里只负责"库里读不出来时怎么办"—— 见 [`ensure_fingerprint`]。

use mcloud_core::{AccountId, Clock, Error, HttpClient, Random, Scope, SmidState, StateStore};
use mcloud_upstream::Upstream;
use mcloud_upstream::smid::{SmFingerprint, SmUpload, build_sm_data};

/// 上报用的 14 位时间串：东八区墙钟 `yyyyMMddHHmmss`。
///
/// 时区**必须**与特征体里写死的 `timezone: -480` 同一个来源，否则等于自报
/// "我在东八区，可我的时间戳是别处来的"。所以这里复用 `pure::daily_key` 用的
/// 那一个 [`mcloud_core::local_tz`]，而不是自己再写一遍偏移量 ——
/// "东八区"这个事实在项目里只有一处定义。
pub fn beijing_time_digits(clock: &impl Clock) -> String {
    clock
        .now()
        .with_timezone(&mcloud_core::local_tz())
        .format("%Y%m%d%H%M%S")
        .to_string()
}

/// 一次上报的时间上下文。
///
/// 单独成为一个类型是为了**可断言**：`old_device_id` 进的是特征体的 `box` 字段，
/// 而真实上报体是加密的，从请求里看不出来。所以"取对了旧 id"这一半在本层断言
/// （见 `upload_context_carries_the_previous_device_id_into_box`），
/// "把它放进 `box`"那一半在上游断言（`old_device_id_goes_into_box` + 那条 box 向量）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadContext {
    /// 14 位东八区时间串。
    pub time_digits: String,
    /// `Date.now()`：特征体里 `svm` 与 `pmf` 用它。
    pub now_millis: i64,
    /// 上一次的 `deviceId`（含 `B` 前缀）；**首次上报是空串**。
    pub old_device_id: String,
}

impl UploadContext {
    pub fn from_clock(clock: &impl Clock, old_device_id: &str) -> Self {
        Self {
            time_digits: beijing_time_digits(clock),
            now_millis: clock.now().timestamp_millis(),
            old_device_id: old_device_id.to_string(),
        }
    }

    /// 借给上游算法看的形式。
    pub fn as_upload(&self) -> SmUpload<'_> {
        SmUpload {
            time_digits: &self.time_digits,
            now_millis: self.now_millis,
            old_device_id: &self.old_device_id,
        }
    }
}

/// 一次特征绑定的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintBinding {
    /// 与库里一致的那份特征。
    pub fingerprint: SmFingerprint,
    /// 库里的两列（这个类型的 `Debug` 对 `device_id` 做了脱敏）。
    pub state: SmidState,
    /// 本次调用是否**新造**了特征（`true` = 库里原先没有）。
    pub newly_bound: bool,
}

impl FingerprintBinding {
    /// 上一次上报换回的 `deviceId`；`None` = 还没上报过。
    pub fn device_id(&self) -> Option<&str> {
        self.state.device_id.as_deref()
    }

    /// 进特征体 `box` 字段的那个值：**首次上报是空串**，不是 `null`、也不是省略。
    pub fn box_value(&self) -> &str {
        self.device_id().unwrap_or("")
    }
}

/// 读这个账号绑定的设备特征；**没有就生成一份并落库**。
///
/// 纯本地操作，不打网络。绑定一旦写成就不再变，除非显式 [`rotate_fingerprint`]。
///
/// # 库里那串读不出来时报错，不重掷
///
/// 悄悄生成一份新特征看着能自愈，实际是把"这个账号从某天起换了一台设备"这件事
/// 变成一次没有记录的事件 —— 以后查风控问题连起点都找不到。报错让人决定。
pub async fn ensure_fingerprint<S: StateStore>(
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    rng: &mut impl Random,
) -> Result<FingerprintBinding, Error> {
    if let Some(state) = store.get_smid_state(scope, account_id).await?
        && let Some(raw) = state.fingerprint_json.as_deref()
    {
        let fingerprint = SmFingerprint::from_json_str(raw).map_err(|_| {
            // 不回吐原文：那串就是这台设备的长相，日志里不该有第二份
            Error::Parse(format!(
                "账号 {} 绑定的设备特征无法解析，请用 `mcloud smid reset` 重新绑定（不会自动重掷）",
                account_id.as_str()
            ))
        })?;
        return Ok(FingerprintBinding {
            fingerprint,
            state,
            newly_bound: false,
        });
    }
    bind_new_fingerprint(store, scope, account_id, rng).await
}

/// **主动更换**设备特征：生成一份新的并绑定，旧的 `deviceId` 当场作废。
///
/// 这是"绑定"的另一半：绑死而没有出口，等于把一次风控判定变成永久状态。
/// 写新特征与清空旧 id 是**同一条 SQL**（见 [`StateStore::set_smid_fingerprint`]），
/// 所以这里没有、也没法补第二步。
pub async fn rotate_fingerprint<S: StateStore>(
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    rng: &mut impl Random,
) -> Result<FingerprintBinding, Error> {
    bind_new_fingerprint(store, scope, account_id, rng).await
}

/// 生成 + 落库，两条路径共用的那一步。
async fn bind_new_fingerprint<S: StateStore>(
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    rng: &mut impl Random,
) -> Result<FingerprintBinding, Error> {
    let fingerprint = SmFingerprint::generate(rng);
    let json = fingerprint.to_json_string()?;
    store.set_smid_fingerprint(scope, account_id, &json).await?;
    Ok(FingerprintBinding {
        fingerprint,
        state: SmidState {
            account_id: account_id.clone(),
            fingerprint_json: Some(json),
            device_id: None,
        },
        newly_bound: true,
    })
}

/// 上报绑定的特征，换回一个新的 `deviceId` 并落库。
///
/// **每次都真的打上游** —— 这是"换 id"那条路（刚 [`rotate_fingerprint`] 过、
/// 或上游明确说这个 id 作废时）。日常取 id 走 [`device_id`]。
pub async fn report_device_id<C: HttpClient, S: StateStore>(
    upstream: &Upstream<'_, C>,
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    clock: &impl Clock,
    rng: &mut impl Random,
) -> Result<String, Error> {
    let binding = ensure_fingerprint(store, scope, account_id, rng).await?;
    upload_and_store(upstream, store, scope, account_id, &binding, clock, rng).await
}

/// 这个账号现在能用的 `deviceid`：库里有就直接返回（**不打网络**），没有才换一次。
///
/// 业务请求该调的是这一个。绑定之所以省流量，省就在这里。
pub async fn device_id<C: HttpClient, S: StateStore>(
    upstream: &Upstream<'_, C>,
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    clock: &impl Clock,
    rng: &mut impl Random,
) -> Result<String, Error> {
    let binding = ensure_fingerprint(store, scope, account_id, rng).await?;
    if let Some(id) = binding.device_id() {
        return Ok(id.to_string());
    }
    upload_and_store(upstream, store, scope, account_id, &binding, clock, rng).await
}

/// 组 body → 上报 → 落库。
///
/// 落库失败会把这个刚换来的 id 丢掉，下次重新上报换一个。反过来（先给请求用、
/// 再落库失败）会留下"用过了但库里没有"的状态，那个更难查。
async fn upload_and_store<C: HttpClient, S: StateStore>(
    upstream: &Upstream<'_, C>,
    store: &S,
    scope: Scope<'_>,
    account_id: &AccountId,
    binding: &FingerprintBinding,
    clock: &impl Clock,
    rng: &mut impl Random,
) -> Result<String, Error> {
    let context = UploadContext::from_clock(clock, binding.box_value());
    let payload = build_sm_data(&binding.fingerprint, &context.as_upload(), rng)?;
    let device_id = upstream
        .smid_device_id(&binding.fingerprint, &payload)
        .await?;
    store
        .set_smid_device_id(scope, account_id, &device_id)
        .await?;
    Ok(device_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{
        Account, AccountState, HttpRequest, HttpResponse, Role, User, UserId, UserStore,
    };
    use mcloud_io::SqliteStore;
    use serde_json::json;
    use std::cell::RefCell;

    const ACCOUNT: &str = "199****0001";
    /// 风控服务给的裸 id（`199` 号段，脱敏约定）。
    const RAW_ID_1: &str = "1990000000000000001";
    const RAW_ID_2: &str = "1990000000000000002";

    fn user_id() -> UserId {
        UserId::new("u-smid")
    }

    fn account_id() -> AccountId {
        AccountId::new(ACCOUNT)
    }

    async fn store() -> SqliteStore {
        let s = SqliteStore::open_in_memory().unwrap();
        let user = user_id();
        s.upsert_user(&User::new(
            user.as_str(),
            "smid 测试",
            "!",
            Role::Admin,
            now(),
        ))
        .await
        .unwrap();
        s.upsert_account(
            Scope::user(&user),
            &Account {
                owner: Some(user.clone()),
                ..Account::new(ACCOUNT)
            },
        )
        .await
        .unwrap();
        s
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        use chrono::{TimeZone, Utc};
        // UTC 06:30 → 东八区 14:30，同一天
        Utc.with_ymd_and_hms(2026, 9, 19, 6, 30, 0).unwrap()
    }

    struct FixedClock(chrono::DateTime<chrono::Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.0
        }
    }

    /// UTC 06:30 → 东八区 14:30，同一天。
    fn clock() -> FixedClock {
        FixedClock(now())
    }

    /// 只装随机数的确定性随机源（同 `app_login` 的那一个）。
    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    /// 脚本假客户端：按队列返回 `{"code":1100,"detail":{"deviceId":…}}`，并记请求。
    struct Scripted {
        ids: RefCell<Vec<String>>,
        seen: RefCell<Vec<HttpRequest>>,
    }

    impl Scripted {
        fn new(raw_ids: &[&str]) -> Self {
            Self {
                ids: RefCell::new(raw_ids.iter().map(|s| (*s).to_string()).collect()),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.seen.borrow().len()
        }
    }

    impl HttpClient for Scripted {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
            self.seen.borrow_mut().push(request);
            // 从队首取：这样"第一次拿到 id1、第二次拿到 id2"读起来跟执行顺序一致
            let next = {
                let mut ids = self.ids.borrow_mut();
                (!ids.is_empty()).then(|| ids.remove(0))
            };
            let Some(raw) = next else {
                return Err(Error::Internal("脚本已用完：这次不该再打上游".into()));
            };
            Ok(HttpResponse::new(
                200,
                json!({ "code": 1100, "detail": { "deviceId": raw } })
                    .to_string()
                    .into_bytes(),
            ))
        }
    }

    async fn stored(s: &SqliteStore) -> SmidState {
        let u = user_id();
        s.get_smid_state(Scope::user(&u), &account_id())
            .await
            .unwrap()
            .expect("已绑定")
    }

    // ── 时间 ────────────────────────────────────────────────────────────────

    #[test]
    fn time_digits_is_beijing_wall_clock_with_14_digits() {
        let digits = beijing_time_digits(&clock());
        assert_eq!(digits, "20260919143000", "UTC 06:30 要看成同日 14:30");
        assert_eq!(digits.len(), 14);
        assert!(
            digits.chars().all(|c| c.is_ascii_digit()),
            "旧实现给上游的就是纯数字串：{digits}"
        );
    }

    #[test]
    fn time_digits_rolls_the_date_across_utc_midnight() {
        use chrono::{TimeZone, Utc};
        // 东八区已是次日 07:30；按 UTC 拼串会差一天，而 `timezone` 字段仍写着 -480
        let late = FixedClock(Utc.with_ymd_and_hms(2026, 9, 18, 23, 30, 0).unwrap());
        assert_eq!(beijing_time_digits(&late), "20260919073000");
    }

    #[test]
    fn upload_context_carries_the_previous_device_id_into_box() {
        let context = UploadContext::from_clock(&clock(), "B1990000000000001");
        assert_eq!(context.time_digits, "20260919143000");
        assert_eq!(context.now_millis, now().timestamp_millis());
        // 这一半在本层断言；"进 box 字段"那一半在上游对拍
        assert_eq!(context.as_upload().old_device_id, "B1990000000000001");
    }

    #[test]
    fn box_value_is_an_empty_string_before_the_first_report() {
        let binding = FingerprintBinding {
            fingerprint: SmFingerprint::caiyun_android_web(),
            state: SmidState::new(ACCOUNT),
            newly_bound: true,
        };
        assert_eq!(binding.device_id(), None);
        assert_eq!(
            binding.box_value(),
            "",
            "旧实现首次上报 `box` 就是空串，不是 null 也不是省略字段"
        );
    }

    // ── 绑定 ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn first_use_binds_the_fingerprint_and_reuses_it_afterwards() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);

        let first = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(7))
            .await
            .unwrap();
        assert!(first.newly_bound);
        assert_eq!(first.device_id(), None);
        assert_eq!(first.box_value(), "");

        let stored = stored(&s).await;
        assert_eq!(
            stored.fingerprint_json.as_deref(),
            Some(first.fingerprint.to_json_string().unwrap().as_str()),
            "库里那份必须与生成那份逐字节相同，否则下次读出来是另一台设备"
        );

        // 换一个随机源再读：还是同一份特征 —— 绑定不会重新掷
        let second = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(200))
            .await
            .unwrap();
        assert!(!second.newly_bound);
        assert_eq!(second.fingerprint, first.fingerprint);
    }

    #[tokio::test]
    async fn the_reuse_path_writes_nothing() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(1))
            .await
            .unwrap();
        let before = stored(&s).await;

        ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(99))
            .await
            .unwrap();
        assert_eq!(
            stored(&s).await,
            before,
            "复用路径上随机源换了，两列也不该动"
        );
    }

    #[tokio::test]
    async fn rotation_replaces_the_fingerprint_and_voids_the_device_id() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        let old = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(1))
            .await
            .unwrap()
            .fingerprint;
        s.set_smid_device_id(scope, &account_id(), "B1990000000000001")
            .await
            .unwrap();

        let rotated = rotate_fingerprint(&s, scope, &account_id(), &mut SeqRandom(50))
            .await
            .unwrap();
        assert!(rotated.newly_bound);
        assert_eq!(rotated.device_id(), None, "新长相还没有 id");
        assert_ne!(rotated.fingerprint, old, "换特征必须真的换一份");

        // 库里那份就是刚生成的那份，而且旧 id 已被清空
        let stored = stored(&s).await;
        assert_eq!(
            SmFingerprint::from_json_str(stored.fingerprint_json.as_deref().unwrap()).unwrap(),
            rotated.fingerprint
        );
        assert_eq!(stored.device_id, None);

        // 再读一次不该又换一份
        let again = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(1))
            .await
            .unwrap();
        assert!(!again.newly_bound);
        assert_eq!(again.fingerprint, rotated.fingerprint);
    }

    #[tokio::test]
    async fn an_unreadable_bound_fingerprint_is_an_error_not_a_reroll() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        // 直接写库造一份读不出来的特征：正常路径生不出这种数据
        s.set_smid_fingerprint(scope, &account_id(), "{ not json")
            .await
            .unwrap();

        let err = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(1))
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("无法解析") && text.contains("reset"),
            "{text}"
        );
        assert!(!text.contains("not json"), "错误串不该回吐特征原文：{text}");
        assert_eq!(
            stored(&s).await.fingerprint_json.as_deref(),
            Some("{ not json"),
            "库里那份坏数据必须原样留着 —— 自动重掷等于替用户悄悄换设备"
        );
    }

    // ── 换 deviceId ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn device_id_reports_once_and_then_comes_from_the_store() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        let http = Scripted::new(&[RAW_ID_1]);
        let upstream = Upstream::new(&http);
        let mut rng = SeqRandom(3);

        let id = device_id(&upstream, &s, scope, &account_id(), &clock(), &mut rng)
            .await
            .unwrap();
        // `B` 前缀是我们加的（上游给的是裸值）
        assert_eq!(id, "B1990000000000000001");
        assert_eq!(http.calls(), 1);
        assert_eq!(stored(&s).await.device_id.as_deref(), Some(id.as_str()));

        // 第二次：同样的调用不该再打上游，也不该换特征
        let fingerprint_before = ensure_fingerprint(&s, scope, &account_id(), &mut rng)
            .await
            .unwrap()
            .fingerprint;
        let again = device_id(&upstream, &s, scope, &account_id(), &clock(), &mut rng)
            .await
            .unwrap();
        assert_eq!(again, id);
        assert_eq!(http.calls(), 1, "绑定之后上报只该发生这一次");
        assert_eq!(
            ensure_fingerprint(&s, scope, &account_id(), &mut rng)
                .await
                .unwrap()
                .fingerprint,
            fingerprint_before
        );

        // 请求发到那台风控主机，而且**不带**任何云盘凭据
        let request = &http.seen.borrow()[0];
        assert!(
            request
                .url
                .starts_with("https://slw.h5cmpassport.com:9090/deviceprofile/v4"),
            "{}",
            request.url
        );
        let names: Vec<String> = request
            .headers
            .iter()
            .map(|(k, _)| k.to_ascii_lowercase())
            .collect();
        for forbidden in ["authorization", "jwttoken", "x-yun-uni"] {
            assert!(
                !names.iter().any(|k| k == forbidden),
                "第三方风控主机不该看到云盘凭据：{names:?}"
            );
        }
    }

    #[tokio::test]
    async fn explicit_report_always_uploads_and_replaces_the_stored_id() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        let http = Scripted::new(&[RAW_ID_1, RAW_ID_2]);
        let upstream = Upstream::new(&http);
        let mut rng = SeqRandom(3);

        let first = report_device_id(&upstream, &s, scope, &account_id(), &clock(), &mut rng)
            .await
            .unwrap();
        assert_eq!(first, "B1990000000000000001");
        assert_eq!(http.calls(), 1, "report 是显式换 id，不读缓存");

        // 换 id 时的上下文带着旧 id（`box` 那一跳在上游对拍）
        let binding = ensure_fingerprint(&s, scope, &account_id(), &mut rng)
            .await
            .unwrap();
        assert_eq!(binding.box_value(), "B1990000000000000001");

        let second = report_device_id(&upstream, &s, scope, &account_id(), &clock(), &mut rng)
            .await
            .unwrap();
        assert_eq!(second, "B1990000000000000002");
        assert_eq!(http.calls(), 2);
        assert_eq!(
            stored(&s).await.device_id.as_deref(),
            Some("B1990000000000000002"),
            "最新的 id 要覆盖旧的"
        );
    }

    #[tokio::test]
    async fn rotation_then_next_use_reports_again() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        let http = Scripted::new(&[RAW_ID_1, RAW_ID_2]);
        let upstream = Upstream::new(&http);

        let id = device_id(
            &upstream,
            &s,
            scope,
            &account_id(),
            &clock(),
            &mut SeqRandom(3),
        )
        .await
        .unwrap();
        assert_eq!(http.calls(), 1);

        rotate_fingerprint(&s, scope, &account_id(), &mut SeqRandom(77))
            .await
            .unwrap();
        assert_eq!(stored(&s).await.device_id, None);

        // 换过特征之后第一次取 id 必须重新上报 —— 继续用旧 id 就是新长相配旧设备
        let next = device_id(
            &upstream,
            &s,
            scope,
            &account_id(),
            &clock(),
            &mut SeqRandom(78),
        )
        .await
        .unwrap();
        assert_eq!(http.calls(), 2);
        assert_ne!(next, id);
    }

    #[tokio::test]
    async fn relogin_does_not_move_the_bound_fingerprint() {
        let s = store().await;
        let u = user_id();
        let scope = Scope::user(&u);
        let binding = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(1))
            .await
            .unwrap();
        s.set_smid_device_id(scope, &account_id(), "B1990000000000001")
            .await
            .unwrap();

        // 重新登录：整体覆盖一份只带新 token 的快照
        s.upsert_account_state(
            scope,
            &AccountState {
                jwt_token: Some("JWT-NEW".into()),
                ..AccountState::new(ACCOUNT)
            },
        )
        .await
        .unwrap();

        let after = ensure_fingerprint(&s, scope, &account_id(), &mut SeqRandom(200))
            .await
            .unwrap();
        assert!(!after.newly_bound);
        assert_eq!(
            after.fingerprint, binding.fingerprint,
            "换凭据不该换设备长相"
        );
        assert_eq!(after.device_id(), Some("B1990000000000001"));
    }

    #[tokio::test]
    async fn an_unowned_account_cannot_be_bound() {
        // 隔离由存储层把关（见 store.rs 的 smid 那几条）；这里只确认编排层没有绕开它
        let s = SqliteStore::open_in_memory().unwrap();
        let stranger = UserId::new("someone-else");
        let err = ensure_fingerprint(&s, Scope::user(&stranger), &account_id(), &mut SeqRandom(1))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), mcloud_core::ErrorKind::NotFound);
    }
}
