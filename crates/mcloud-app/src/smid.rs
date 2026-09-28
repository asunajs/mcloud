//! 账号绑定的设备指纹（smid），以及"拿特征换 `deviceId`"的编排。
//!
//! # 这一层管的是生命周期
//!
//! 纯算法在 [`mcloud_upstream::smid`]，存储契约在 [`mcloud_core::SmidStore`]。
//! 中间有一件事谁都没接：**什么时候生成、什么时候复用、什么时候作废**。
//! 本层就是那件事的答案。
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
//! 旧实现每次上报都现掷一份新特征，于是同一个账号在风控服务眼里
//! **每次都换一台设备** —— 那正是最容易触发核验的姿态。
//! 绑定的实际收益就是上面那张图：上报从"每次请求"降到"每个账号一次"，
//! 而换 id 时旧 id 会进特征体的 `box`，让服务端看出"还是那台机器在换 id"。
//!
//! 算法能被参数化（`SmFingerprint` 是个普通结构体）是这件事的前提：它必须能
//! 原样序列化回库、再原样读出来。那条往返由 `mcloud-upstream` 的
//! `the_bound_fingerprint_round_trips_through_the_feature_body` 把住，
//! 这里只负责"库里读不出来时怎么办"—— 见 [`ensure_fingerprint`]。

use std::sync::Arc;

use mcloud_core::{
    beijing_time_digits, AccountId, Clock, Error, HttpClient, HttpRequest, HttpResponse, Result,
    SmidState, SmidStore,
};
use mcloud_crypto::{Random, SharedRandom};
use mcloud_upstream::smid::{build_sm_data, smid_device_id, SmFingerprint, SmUpload};

/// 一次上报的时间上下文。
///
/// 单独成为一个类型是为了**可断言**：`old_device_id` 进的是特征体的 `box` 字段，
/// 而真实上报体是加密的，从请求里看不出来。所以"取对了旧 id"这一半在本层断言
/// （见 `upload_context_carries_the_previous_device_id_into_box`），
/// "把它放进 `box`"那一半在上游断言。
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
    /// 从时钟与"上一次的 deviceId"造一份上下文。
    ///
    /// 时间串取自 [`mcloud_core::beijing_time_digits`]，与特征体里写死的
    /// `timezone: -480` 同一个来源 —— 见那个函数的说明。
    pub fn from_clock(clock: &dyn Clock, old_device_id: &str) -> Self {
        Self {
            time_digits: beijing_time_digits(clock),
            now_millis: clock.now_ms(),
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
    /// 库里的状态（这个类型的 `Debug` 对两列都做了脱敏）。
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
pub async fn ensure_fingerprint<S: SmidStore + ?Sized>(
    store: &S,
    account_id: &AccountId,
    rng: &mut impl Random,
) -> Result<FingerprintBinding> {
    if let Some(state) = store.get_smid_state(account_id).await? {
        if let Some(raw) = state.fingerprint_json.as_deref() {
            let fingerprint = SmFingerprint::from_json_str(raw).map_err(|_| {
                // 不回吐原文：那串就是这台设备的长相，日志里不该有第二份
                Error::Parse(format!(
                    "账号 {} 绑定的设备特征无法解析，请主动重新绑定（不会自动重掷）",
                    account_id.as_str()
                ))
            })?;
            return Ok(FingerprintBinding {
                fingerprint,
                state,
                newly_bound: false,
            });
        }
    }
    bind_new_fingerprint(store, account_id, rng).await
}

/// **主动更换**设备特征：生成一份新的并绑定，旧的 `deviceId` 当场作废。
///
/// 这是"绑定"的另一半：绑死而没有出口，等于把一次风控判定变成永久状态。
/// 写新特征与清空旧 id 是**同一次窄写**（见 `SmidStore::set_smid_fingerprint`），
/// 所以这里没有、也没法补第二步。
pub async fn rotate_fingerprint<S: SmidStore + ?Sized>(
    store: &S,
    account_id: &AccountId,
    rng: &mut impl Random,
) -> Result<FingerprintBinding> {
    bind_new_fingerprint(store, account_id, rng).await
}

/// 生成 + 落库，两条路径共用的那一步。
async fn bind_new_fingerprint<S: SmidStore + ?Sized>(
    store: &S,
    account_id: &AccountId,
    rng: &mut impl Random,
) -> Result<FingerprintBinding> {
    let fingerprint = SmFingerprint::generate(rng);
    let json = fingerprint.to_json_string()?;
    store.set_smid_fingerprint(account_id, &json).await?;
    Ok(FingerprintBinding {
        fingerprint,
        state: SmidState {
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
pub async fn report_device_id<C: HttpClient + ?Sized, S: SmidStore + ?Sized>(
    http: &C,
    store: &S,
    account_id: &AccountId,
    clock: &dyn Clock,
    rng: &mut impl Random,
) -> Result<String> {
    let binding = ensure_fingerprint(store, account_id, rng).await?;
    upload_and_store(http, store, account_id, &binding, clock, rng).await
}

/// 这个账号现在能用的 `deviceid`：库里有就直接返回（**不打网络**），没有才换一次。
///
/// 业务请求该调的是这一个。绑定之所以省流量，省就在这里。
pub async fn device_id<C: HttpClient + ?Sized, S: SmidStore + ?Sized>(
    http: &C,
    store: &S,
    account_id: &AccountId,
    clock: &dyn Clock,
    rng: &mut impl Random,
) -> Result<String> {
    let binding = ensure_fingerprint(store, account_id, rng).await?;
    if let Some(id) = binding.device_id() {
        return Ok(id.to_string());
    }
    upload_and_store(http, store, account_id, &binding, clock, rng).await
}

/// 组 body → 上报 → 落库。
///
/// 落库失败会把这个刚换来的 id 丢掉，下次重新上报换一个。反过来（先给请求用、
/// 再落库失败）会留下"用过了但库里没有"的状态，那个更难查。
async fn upload_and_store<C: HttpClient + ?Sized, S: SmidStore + ?Sized>(
    http: &C,
    store: &S,
    account_id: &AccountId,
    binding: &FingerprintBinding,
    clock: &dyn Clock,
    rng: &mut impl Random,
) -> Result<String> {
    let context = UploadContext::from_clock(clock, binding.box_value());
    let payload = build_sm_data(&binding.fingerprint, &context.as_upload(), rng)?;
    let id = smid_device_id(http, &binding.fingerprint, &payload).await?;
    store.set_smid_device_id(account_id, &id).await?;
    Ok(id)
}

/// 进程级的 smid 编排器。
///
/// 把"HTTP 传输 + 存储 + 时钟 + 随机源"打包成一个可注入的对象，
/// 让调用方（组装层 / 任务）不必每次凑齐四个参数。
///
/// # 为什么随机源是 [`SharedRandom`]
///
/// [`Random`] 的方法收 `&mut self`，而本服务要被多个并发请求共享。
/// `SharedRandom` 把锁压在 `fill_bytes` 内部，并且只提供**同步**闭包入口 ——
/// 于是"持锁跨 `.await`"在类型层面写不出来（见 docs/conventions.md §6）。
pub struct SmidService {
    http: Arc<dyn HttpClient>,
    store: Arc<dyn SmidStore>,
    clock: Arc<dyn Clock>,
    rng: SharedRandom,
}

impl SmidService {
    /// 组装一个 smid 服务。
    pub fn new(
        http: Arc<dyn HttpClient>,
        store: Arc<dyn SmidStore>,
        clock: Arc<dyn Clock>,
        rng: Box<dyn Random>,
    ) -> Self {
        Self {
            http,
            store,
            clock,
            rng: SharedRandom::new(rng),
        }
    }

    /// 库里有就用，没有才上报换一个。**业务请求走这个。**
    pub async fn device_id(&self, account_id: &AccountId) -> Result<String> {
        device_id(
            &*self.http,
            &*self.store,
            account_id,
            &*self.clock,
            &mut self.rng.clone(),
        )
        .await
    }

    /// 每次都真的上报一次，换一个新的 `deviceId` 并落库。
    pub async fn report(&self, account_id: &AccountId) -> Result<String> {
        report_device_id(
            &*self.http,
            &*self.store,
            account_id,
            &*self.clock,
            &mut self.rng.clone(),
        )
        .await
    }

    /// 主动更换设备指纹（等于换一台新设备），并立刻上报换新 id。
    ///
    /// 先换长相再上报，两步都做完才返回：只换长相不换 id 会留下
    /// "新指纹 + 旧 id"的状态，而那正是风控最想看到的漂移信号。
    pub async fn rotate(&self, account_id: &AccountId) -> Result<String> {
        rotate_fingerprint(&*self.store, account_id, &mut self.rng.clone()).await?;
        self.report(account_id).await
    }

    /// 只看库里绑定的状态，不打网络。
    pub async fn state(&self, account_id: &AccountId) -> Result<Option<SmidState>> {
        self.store.get_smid_state(account_id).await
    }
}

// ==================== 请求路径上的接线 ====================

/// 自动为每个请求补 `deviceid` 头的 `HttpClient` 装饰器。
///
/// # 为什么需要它
///
/// `deviceid` 的取法不是"读配置里那个字符串"，而是**先上报特征换一个**
/// （见本模块开头的生命周期图）。如果没人把它接到请求路径上，那份算法
/// 就是"有实现没人调用"—— 编译通过、测试全绿，而线上每个请求都缺这个头。
///
/// # 为什么是装饰器
///
/// 与 `H5Client` 同一个理由：任务代码只依赖 `mcloud_core::HttpClient`，
/// 不该知道"我的 deviceid 从哪来"。放在这一层意味着**取 id 只有一处实现**，
/// 而它是懒加载的（第一次请求时才上报，之后走库）。
///
/// # 失败时的行为：**不带这个头**，而不是让请求失败
///
/// 取 id 要打第三方风控服务，它可能超时。这时让业务请求整个失败是错的 ——
/// 一个风控服务的抖动不该让签到跑不起来。所以取不到就原样发出去，
/// 由上游按自己的规则回错（那才是权威判据）。
pub struct SmidClient {
    /// 底层传输（真正发请求的那个）。
    transport: Arc<dyn HttpClient>,
    /// 本账号的 smid 编排。
    smid: Arc<SmidService>,
    /// 本账号的 id。
    account_id: AccountId,
}

impl SmidClient {
    /// 为一个账号包一层。
    pub fn new(
        transport: Arc<dyn HttpClient>,
        smid: Arc<SmidService>,
        account_id: AccountId,
    ) -> Self {
        Self {
            transport,
            smid,
            account_id,
        }
    }
}

#[async_trait::async_trait]
impl HttpClient for SmidClient {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        // 取 id：库里有就直接返回，没有才上报换一次。
        let device_id = match self.smid.device_id(&self.account_id).await {
            Ok(id) => Some(id),
            Err(e) => {
                // 不把错误往上抛：见类型文档。这里只记一行，请求照发。
                tracing::warn!(
                    account = %self.account_id,
                    error = %e,
                    "取 smid deviceId 失败，本次请求不带 deviceid 头"
                );
                None
            }
        };

        let mut req = req;
        if let Some(id) = device_id {
            // 调用方若自己指定了 deviceid，优先用它（便于调试台覆盖）。
            let has_own = req
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("deviceid"));
            if !has_own {
                req.headers.push(("deviceid".into(), id));
            }
        }
        self.transport.request(req).await
    }
}

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{HttpRequest, HttpResponse};
    use mcloud_crypto::SeqRandom;
    use std::sync::Mutex;

    const ACCOUNT: &str = "199****0001";
    /// 风控服务给的裸 id（`199` 号段，脱敏约定）。
    const RAW_ID_1: &str = "1990000000000000001";
    const RAW_ID_2: &str = "1990000000000000002";

    fn account_id() -> AccountId {
        AccountId::new(ACCOUNT)
    }

    /// UTC 2026-09-19T06:30:00Z → 东八区 2026-09-19 14:30，同一天。
    fn clock() -> FixedClock {
        FixedClock(1_789_799_400_000)
    }

    /// 内存版 `SmidStore`：只存两个账号的状态，用来观察窄写行为。
    #[derive(Default)]
    struct MemSmid {
        state: Mutex<std::collections::HashMap<String, SmidState>>,
        /// 记录 `set_smid_fingerprint` 的调用次数（"只在首次绑定"要靠它证明）。
        binds: Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl SmidStore for MemSmid {
        async fn get_smid_state(&self, id: &AccountId) -> Result<Option<SmidState>> {
            Ok(self.state.lock().unwrap().get(id.as_str()).cloned())
        }
        async fn set_smid_fingerprint(&self, id: &AccountId, json: &str) -> Result<()> {
            *self.binds.lock().unwrap() += 1;
            self.state.lock().unwrap().insert(
                id.as_str().to_string(),
                SmidState {
                    fingerprint_json: Some(json.to_string()),
                    // 换指纹连带清 deviceId —— 与真实实现的语义一致
                    device_id: None,
                },
            );
            Ok(())
        }
        async fn set_smid_device_id(&self, id: &AccountId, device_id: &str) -> Result<()> {
            let mut m = self.state.lock().unwrap();
            let entry = m.entry(id.as_str().to_string()).or_default();
            entry.device_id = Some(device_id.to_string());
            Ok(())
        }
    }

    /// 脚本假客户端：按队列返回 `{"code":1100,...}`，并记下请求。
    struct Scripted {
        ids: Mutex<Vec<String>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Scripted {
        fn new(raw_ids: &[&str]) -> Self {
            Self {
                ids: Mutex::new(raw_ids.iter().map(|s| (*s).to_string()).collect()),
                seen: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for Scripted {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(req);
            let raw = {
                let mut ids = self.ids.lock().unwrap();
                if ids.is_empty() {
                    "199fallback".to_string()
                } else {
                    ids.remove(0)
                }
            };
            Ok(HttpResponse {
                status: 200,
                body: serde_json::json!({"code": 1100, "detail": {"deviceId": raw}}).to_string(),
                headers: Vec::new(),
            })
        }
    }

    fn rng() -> SeqRandom {
        SeqRandom::new(1)
    }

    #[test]
    fn beijing_time_digits_is_east_eight() {
        // UTC 06:30 → 东八区 14:30
        assert_eq!(beijing_time_digits(&clock()), "20260919143000");
    }

    #[test]
    fn beijing_time_digits_crosses_the_date_line_forward() {
        // UTC 2026-09-19T18:00:00Z → 东八区 2026-09-20 02:00（跨日）
        //
        // 这条防的是"用了 UTC 却以为在东八区"：两者在同一天时看不出来，
        // 必须挑一个跨日的时刻才能暴露偏移到底有没有生效。
        let c = FixedClock(1_789_840_800_000);
        assert_eq!(beijing_time_digits(&c), "20260920020000");
    }

    #[test]
    fn upload_context_carries_the_previous_device_id_into_box() {
        let ctx = UploadContext::from_clock(&clock(), "BOLD");
        assert_eq!(ctx.old_device_id, "BOLD");
        assert_eq!(ctx.as_upload().old_device_id, "BOLD");
        assert_eq!(ctx.time_digits, "20260919143000");
    }

    #[tokio::test]
    async fn first_use_binds_a_fingerprint_then_reuses_it() {
        let store = MemSmid::default();
        let id = account_id();
        let mut r = rng();

        let a = ensure_fingerprint(&store, &id, &mut r).await.unwrap();
        assert!(a.newly_bound, "首次必须新绑一份");
        assert_eq!(a.device_id(), None, "还没上报过，没有 deviceId");

        let b = ensure_fingerprint(&store, &id, &mut r).await.unwrap();
        assert!(!b.newly_bound, "第二次必须复用，不能重掷");
        assert_eq!(a.fingerprint, b.fingerprint, "设备长相不该变");
        assert_eq!(*store.binds.lock().unwrap(), 1, "只该落库一次");
    }

    #[tokio::test]
    async fn device_id_reports_once_then_serves_from_the_store() {
        let store = MemSmid::default();
        let http = Scripted::new(&[RAW_ID_1]);
        let id = account_id();
        let mut r = rng();

        let first = device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();
        assert_eq!(first, format!("B{RAW_ID_1}"));
        assert_eq!(http.calls(), 1);

        // 第二次必须**不打网络**：绑定省流量就省在这里
        let second = device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();
        assert_eq!(second, first);
        assert_eq!(http.calls(), 1, "库里有 id 就不该再上报");
    }

    #[tokio::test]
    async fn report_always_hits_the_upstream() {
        let store = MemSmid::default();
        let http = Scripted::new(&[RAW_ID_1, RAW_ID_2]);
        let id = account_id();
        let mut r = rng();

        let a = report_device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();
        let b = report_device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();
        assert_eq!(a, format!("B{RAW_ID_1}"));
        assert_eq!(b, format!("B{RAW_ID_2}"));
        assert_eq!(http.calls(), 2, "report 每次都真的打上游");
        // 但**特征没换**：只是换 id，不是换设备
        assert_eq!(*store.binds.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn rotating_replaces_the_fingerprint_and_clears_the_old_id() {
        let store = MemSmid::default();
        let http = Scripted::new(&[RAW_ID_1, RAW_ID_2]);
        let id = account_id();
        let mut r = rng();

        let before = ensure_fingerprint(&store, &id, &mut r).await.unwrap();
        device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();

        let after = rotate_fingerprint(&store, &id, &mut r).await.unwrap();
        assert!(after.newly_bound);
        assert_ne!(before.fingerprint, after.fingerprint, "换指纹必须换长相");
        assert_eq!(
            after.device_id(),
            None,
            "旧 id 是旧长相换来的，必须当场作废"
        );
    }

    #[tokio::test]
    async fn a_broken_stored_fingerprint_is_an_error_not_a_silent_rebind() {
        let store = MemSmid::default();
        let id = account_id();
        // 库里塞半截 JSON
        store.set_smid_fingerprint(&id, "{\"ua\":").await.unwrap();
        let err = ensure_fingerprint(&store, &id, &mut rng())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "{err:?}");
        // 而且**没有**顺手重掷一份：重掷等于静默换掉这台设备的长相
        assert_eq!(
            *store.binds.lock().unwrap(),
            1,
            "读不出来时报错，不自动重绑"
        );
    }

    #[tokio::test]
    async fn the_upload_carries_the_previous_id_in_box() {
        let store = MemSmid::default();
        let http = Scripted::new(&[RAW_ID_1, RAW_ID_2]);
        let id = account_id();
        let mut r = rng();

        device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();
        // 第二次上报：box 里必须带着第一次那个 id
        report_device_id(&http, &store, &id, &clock(), &mut r)
            .await
            .unwrap();

        let seen = http.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        let body: serde_json::Value =
            serde_json::from_str(seen[1].body_str().unwrap()).unwrap();
        // 上报体是加密的，box 只能通过解密看 —— 这里断言"带了值"而不是空串。
        //
        // ⚠ 原来这里是两句（`len() > 0` 与 `!is_empty()`）—— 后者蕴含前者，
        // 所以第一句是**永远不会单独失败**的死断言。合成一句：
        // 断言重复时，读的人会以为它们在查两件不同的事。
        assert!(!body["data"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn smid_service_shares_one_random_source_across_calls() {
        let store = std::sync::Arc::new(MemSmid::default());
        let http = std::sync::Arc::new(Scripted::new(&[RAW_ID_1]));
        let svc = SmidService::new(
            http,
            store.clone(),
            std::sync::Arc::new(clock()),
            Box::new(SeqRandom::new(1)),
        );
        let id = account_id();
        let a = svc.device_id(&id).await.unwrap();
        let b = svc.device_id(&id).await.unwrap();
        assert_eq!(a, b, "同一个服务实例复用同一份绑定");
    }

    // ── 请求路径上的接线 ────────────────────────────────────────────────

    /// 记录收到的请求，用来断言 `deviceid` 头有没有被补上。
    struct Recorder {
        seen: Mutex<Vec<HttpRequest>>,
    }

    #[async_trait::async_trait]
    impl HttpClient for Recorder {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(req);
            Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
                headers: Vec::new(),
            })
        }
    }

    fn header_of(req: &HttpRequest, name: &str) -> Option<String> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    #[tokio::test]
    async fn smid_client_adds_the_deviceid_header() {
        let store = std::sync::Arc::new(MemSmid::default());
        let recorder = std::sync::Arc::new(Recorder {
            seen: Mutex::new(Vec::new()),
        });
        let smid = std::sync::Arc::new(SmidService::new(
            std::sync::Arc::new(Scripted::new(&[RAW_ID_1])),
            store,
            std::sync::Arc::new(clock()),
            Box::new(SeqRandom::new(1)),
        ));
        let client = SmidClient::new(recorder.clone(), smid, account_id());

        client
            .request(HttpRequest::get("https://m.mcloud.139.com/x"))
            .await
            .unwrap();
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(
            header_of(&seen[0], "deviceid").as_deref(),
            Some(format!("B{RAW_ID_1}").as_str()),
            "请求必须带上换来的 deviceid"
        );
    }

    #[tokio::test]
    async fn smid_client_does_not_override_a_caller_supplied_deviceid() {
        let store = std::sync::Arc::new(MemSmid::default());
        let recorder = std::sync::Arc::new(Recorder {
            seen: Mutex::new(Vec::new()),
        });
        let smid = std::sync::Arc::new(SmidService::new(
            std::sync::Arc::new(Scripted::new(&[RAW_ID_1])),
            store,
            std::sync::Arc::new(clock()),
            Box::new(SeqRandom::new(1)),
        ));
        let client = SmidClient::new(recorder.clone(), smid, account_id());

        client
            .request(HttpRequest::get("https://x/").header("deviceid", "B-MANUAL"))
            .await
            .unwrap();
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(
            header_of(&seen[0], "deviceid").as_deref(),
            Some("B-MANUAL"),
            "调用方显式指定的值优先（调试台要能覆盖）"
        );
    }

    /// 取 id 失败时**请求照发**，只是不带那个头。
    ///
    /// 这是刻意的：取 id 要打第三方风控服务，它抖动不该让签到整个跑不起来。
    /// 缺头的后果由上游按自己的规则回错 —— 那才是权威判据。
    #[tokio::test]
    async fn smid_client_still_sends_the_request_when_the_id_cannot_be_obtained() {
        /// 永远 500：模拟风控服务抖动。
        struct Failing;

        #[async_trait::async_trait]
        impl HttpClient for Failing {
            async fn request(&self, _req: HttpRequest) -> Result<HttpResponse> {
                Ok(HttpResponse {
                    status: 500,
                    body: "boom".into(),
                    headers: Vec::new(),
                })
            }
        }

        let store = std::sync::Arc::new(MemSmid::default());
        let recorder = std::sync::Arc::new(Recorder {
            seen: Mutex::new(Vec::new()),
        });
        let smid = std::sync::Arc::new(SmidService::new(
            std::sync::Arc::new(Failing),
            store,
            std::sync::Arc::new(clock()),
            Box::new(SeqRandom::new(1)),
        ));
        let client = SmidClient::new(recorder.clone(), smid, account_id());

        let resp = client
            .request(HttpRequest::get("https://m.mcloud.139.com/x"))
            .await
            .expect("取 id 失败不该让请求本身失败");
        assert_eq!(resp.status, 200, "发出去的是底层传输的响应");
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "请求仍然发出去了");
        assert_eq!(header_of(&seen[0], "deviceid"), None, "只是没有那个头");
    }

    #[tokio::test]
    async fn smid_service_rotate_changes_the_device() {
        let store = std::sync::Arc::new(MemSmid::default());
        let http = std::sync::Arc::new(Scripted::new(&[RAW_ID_1, RAW_ID_2]));
        let svc = SmidService::new(
            http,
            store.clone(),
            std::sync::Arc::new(clock()),
            Box::new(SeqRandom::new(1)),
        );
        let id = account_id();
        let first = svc.device_id(&id).await.unwrap();
        let second = svc.rotate(&id).await.unwrap();
        assert_ne!(first, second, "换设备必须换回另一个 id");
    }
}
