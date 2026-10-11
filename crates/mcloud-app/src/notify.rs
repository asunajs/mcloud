//! 推送投递：**规则回答"怎么发"，行为回答"什么时候发"**。
//!
//! # 分界（2026-10-08 冻结口径，用户原话）
//!
//! > 「我设置一个推送规则，**这个规则配置推送的 token，包括推送哪几个平台**」
//! > 「**不是规则去命中行为，而是行为选择规则，一个行为可以选择多个规则**」
//!
//! ⇒ 这一层只做两件事：
//!
//! * **运行类**（定时 / 编排跑完）：从发起方（`RunEvent::Started.origin` ✓）
//!   读出它绑的 `push_rules`（**可多条** ✓）与 `push_events`（结局过滤 ✓），
//!   然后**每条规则各按自己的 `config` 发一次** ✓；
//! * **面板类**（登录 / 性能）：从 `data/push-panel.json` 取这个 owner 对应行为的
//!   规则 + 节流 ✓ ⇒ 过闸（状态变化 / 冷却 / 每日上限 ✓）⇒ 每条规则各发一次 ✓。
//!
//! ⚠ **没有**"用户级渠道配置"那一层了 ✗（`config_for` / `PushConfigStore` /
//! `Notifier::configs` 全删 ✓）：用户口径是规则自带 token ✓ ——
//! "发不发、发给谁"全部来自**规则自己** ✓。连带着"这次运行是哪个账号的"也不需要了 ✓
//! （规则绑在定时/编排上，投递不需要再 `account → owner` 跳一次 ✓）。
//!
//! # 挂点：装饰 app 侧的 EventSink（候选 a），不是订阅 hub（候选 b）
//!
//! - Runner 把**每一次生命周期事件**都从**同一个** Arc<dyn EventSink> 发出去
//!   （runner.rs 的 self.sink.emit(...)），Finished 是终点事件，一次运行恰好过
//!   一次。在它外面套一层 = 在唯一的出口上加旁路，mcloud-engine 一行都不用改。
//! - 订阅 hub 是**按 run 拉**的模型：要先拿到 run_id 才能 subscribe，再反复
//!   drain 等终点，还得自己处理订阅者队列容量满时"丢最新"的情况。而这里事件
//!   是**推**过来的，不丢、也不需要额外的订阅生命周期。
//! - emit 是**同步**方法，所以投递必须 tokio::spawn 出去：HTTP 慢一秒，运行
//!   路径不能跟着慢一秒。**读规则表也是 async 的**（要读 store），同样
//!   不能发生在同步的 emit 里 —— 所以顺序是"先进 spawn，再 await 读规则，最后
//!   投递"。这也正是"投递失败影响不到运行结果"的结构性保证。
//!
//! # 一次运行是怎么变成推送的
//!
//! 1. Started：给这个 run 建一份 PushLogCollector，并记下 `origin`；
//! 2. Log：把每一条运行日志喂给它（error/info/warn/debug 逐字对应）；
//! 3. Finished：按 `origin` 找出**发起方**（`schedule:<id>` / `pipeline:<id>` ✓）⇒
//!    它绑的 `push_rules` / `push_events` ⇒ 逐条规则各发一次 ✓；
//!    `origin` 为 `None`（手动跑 ✓）或没绑规则 ⇒ **一个包都不发** ✓（0 个请求 ✓）。
//!
//! # 面板行为（登录 / 性能）：判定与发送都在这一层，**触发线已接**（2026-10-08）
//!
//! 判定与发送函数写在这一层 ✓，触发线**分两处**接（用户口径：「接啊」✓）：
//!
//! **接在哪**（给以后动这条线的人看，别自己另起一套）：
//!
//! | 面板行为 | 触发点 | 怎么调 |
//! |---|---|---|
//! | `perfSlow` / `perfFailStreak` | 一次运行收尾（`RunEvent::Finished` 那一跳 ✓ ⇒ `Notifier::perf_check` ✓） | 已接 ✓：`perf_alert(owner, task, account, duration_ms)` ⇒ 有值就 `deliver_panel(owner, PanelNotice { behavior, status: Abnormal, .. })` |
//! | `loginNew` | 登录**成功**之后（`mcloud-server` 的 `routes/auth.rs` ✓） | 已接 ✓：`remember_login_device(owner, "UA\|peer IP")` 回 `true` ⇒ `spawn_panel` ✓ |
//! | `loginFailed` | 登录**失败**那一跳（同上 ✓） | 已接 ✓：`spawn_panel(owner, … status: Abnormal …)` ✓（每次失败都算一次触发 ✓，节流交给 `deliver_panel` ✓） |
//!
//! ⚠ **`loginNew` 的"设备"是什么**：本仓**没有**真设备这个概念 ✗（登录请求里
//! 只有口令，见 `routes/auth.rs::login_device_key` ✓）⇒ 判据退化成
//! **「这个账号 + 这个 UA + 这个 peer IP」没见过的组合** ✓（落盘
//! `data/login-devices.json` ✓）。⚠ 这是**明说的近似**，不是"任意一次成功登录
//! 都算新设备" ✗ —— 后者是骗人 ✓（同一条口径见 `traits.rs` 的
//! `remember_login_device` ✓）。
//!
//! ⚠ `owner` 从哪来是**触发线的责任** ✓：性能那条是 `account → account.owner` ✓
//! （拿不到就**不发** ✓，见 `Notifier::perf_check` ✓），登录那条就是
//! "正在登录的那个人" ✓ —— 这一层刻意不接受 `account`，
//! 免得又长出一条"按账号猜用户"的路 ✗。
//!
//! # 节流：**保守默认 + 不许静默**
//!
//! 用户口径：「登录，性能，这些可能**频繁触发**，注意」✓ ⇒ 每个行为带
//! 冷却与每日上限（默认值见 `mcloud_core::BehaviorKey::default_throttle` ✓），
//! 判据是纯函数（`Throttle::decide` ✓），状态**落盘**（`data/push-throttle.json` ✓
//! —— 只放内存的话重启即忘记 ⇒ 重启后同一批事件再来一轮风暴 ✗）。
//!
//! 被拦下时**必须留痕** ✓（用户口径「不许静默」✗）：
//! ① `tracing::warn!` 一条（带原因与"今天已节流 N 次" ✓）；
//! ② 计数写进节流状态（**重启也还在** ✓）；
//! ③ [`PanelOutcome::Suppressed`] 回给调用方（**可测** ✓）。
//!
//! # 可撤性
//!
//! 没绑规则 ⇒ **连一个包都不会发出去**（`http.request` 一次都不调用 ✓）。
//! 删掉那条定时上的绑定、或把规则停用，就回到"这个行为没有推送" ✓。
//!
//! # 主动中止怎么判
//!
//! RunEvent::Finished 只带 TaskOutcome 与 duration_ms，**不带** RunStatus ——
//! 中止是在 Finished **之后**才写进 RunRecord.status 的（runner.rs：先
//! sink.emit(Finished)，再 cancel.release(run_id)，最后才盖 Cancelled）。所以
//! 事件的字段里看不出"这次是不是用户点的中止"。
//!
//! 本仓现成的判据是 CancelRegistry::is_cancelled(run_id)，它正是 runner.rs 里
//! was_cancelled（⇒ RunStatus::Cancelled）的来源。关键在**时机**：runner 是
//! "先 emit、后 release"，所以在装饰器**同步**处理 Finished 的那一刻，登记表里
//! 这一条还在、标志位还准。而 CancelRegistry 由 Runner::new 内部创建（装饰器
//! 必须先于 runner 存在，因为它就是 runner 的 sink）——因此用一个 CancelSlot
//! 槽位：runner 造好后立刻回填，AppBuilder::build 返回前一定填好，那时还没有
//! 任何一次运行开始，不存在竞态。
//!
//! 判出来是中止之后，结尾摘要记 warn（不是 error）—— see outcome_line 与
//! docs/architecture.md:452。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mcloud_core::push::{
    local_day, BehaviorKey, BehaviorStatus, PushEvent, PushLogCollector, PushLogEntry, PushRule,
    PushRuleId, ThrottleDecision, ThrottleReason, ThrottleState,
};
use mcloud_core::{
    AccountId, Clock, EventSink, HttpClient, HttpRequest, LogLevel, PipelineId, RunEvent, RunId,
    RunRecord, RunStatus, ScheduleId, Store, TaskOutcome, UserId,
};
use mcloud_engine::CancelRegistry;

use crate::push::{PushContext, PushRequest, PushResponse, PushTransport};

/// 「耗时异常」倍数判据的**内置默认**（`perf_slow` 那格的 `slow_factor` 为 `None` 时用它 ✓）。
///
/// ⚠ 它是**兜底**，不是唯一的取值来源 ✓：用户能在设置页给 `perf_slow` 写一个
/// `slowFactor`（`BehaviorBinding::slow_factor` ✓）—— 那条路优先 ✓。
/// ⚠ 全文件**只有这一个 3.0** ✓（兜底散成两处，改默认值就会漏改一处 ✗）。
const SLOW_FACTOR: f64 = 3.0;
/// 「连续失败」条数判据的**内置默认**（`perf_fail_streak` 那格的 `fail_streak` 为
/// `None` 时用它 ✓）。⚠ 同样只此一处 ✓。
const FAIL_STREAK: u32 = 3;

/// PushTransport 适配现有的 HttpClient。
///
/// 推送的 10s 超时必须在这一层真正生效：HttpClient::request 的签名里没有超时
/// 参数，所以用 tokio::time::timeout 包住 —— 这正是 PushRequest::timeout_ms 存在
/// 的理由（见 mcloud-app::push 模块头）。
struct HttpPushTransport {
    http: Arc<dyn HttpClient>,
    /// DNS 解析器（⚠ **可注入**：单测给假解析器，**不打真 DNS** ✗ —— 真打 DNS 的
    /// 单测会在离网机器上红、还会变慢 ✓）。
    resolver: Arc<dyn crate::push::PushHostResolver>,
    /// `MCLOUD_ALLOW_LOCAL_PUSH` 的取值（构造时读一次 ✓）。
    /// ⚠ 它是**服务级**开关（环境变量），不是"每个用户自己勾"的那一格 ✗。
    allow_local: bool,
}

/// 测试进程里用的解析器：**一律解析成公网 IP** ✓。
///
/// # 为什么必须有它
///
/// 单测里那些域名全是**假名**（`qyapi.test`、`sctapi.ftqq.com` 之类 ✓）⇒ 交给真解析器
/// 必然解析失败 ⇒ 投递这条路在单测里**永远不成功** ✗。症状是"等投递完成超时"
/// （`wait_calls` ✓）—— 看起来像**业务坏了**，其实是测试打了真 DNS ✗。
///
/// ⚠ 返回**公网**地址（`93.184.216.34`）：SSRF 闸门（`crate::push::validate_push_url_resolved` ✓）
/// 对私网/回环是**拒**的 ⇒ 用 `127.0.0.1` 的话每条投递都会被闸门拦下 ✗，
/// 那组"本地地址要拒"的用例在别处（直接传解析器 ✓）已经覆盖了 ✓。
#[cfg(test)]
struct TestResolver;

#[cfg(test)]
#[async_trait::async_trait]
impl crate::push::PushHostResolver for TestResolver {
    async fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<std::net::IpAddr>, String> {
        Ok(vec![std::net::IpAddr::from([93, 184, 216, 34])])
    }
}

impl HttpPushTransport {
    /// 生产用：真解析器 + 从环境变量读开关。
    ///
    /// ⚠ **测试进程**换成 [`TestResolver`]（假名不能真解析 ✓，理由见它的文档 ✓）。
    fn new(http: Arc<dyn HttpClient>) -> Self {
        let allow_local = crate::push::allow_local_push_from_env();
        #[cfg(test)]
        {
            Self::with_resolver(http, Arc::new(TestResolver), allow_local)
        }
        #[cfg(not(test))]
        {
            Self {
                http,
                resolver: Arc::new(crate::push::SystemResolver),
                allow_local,
            }
        }
    }

    /// 测试用：换解析器 / 显式给开关状态（⚠ 不在测试里改环境变量 ——
    /// `set_var` 是**进程级**的，并行测试会互相踩 ✗）。
    #[cfg(test)]
    fn with_resolver(
        http: Arc<dyn HttpClient>,
        resolver: Arc<dyn crate::push::PushHostResolver>,
        allow_local: bool,
    ) -> Self {
        Self {
            http,
            resolver,
            allow_local,
        }
    }
}

#[async_trait::async_trait]
impl PushTransport for HttpPushTransport {
    async fn send(&self, req: PushRequest) -> anyhow::Result<PushResponse> {
        // ⚠⚠ 用户配的地址**先过闸**（2026-10-04 复审 sv-8）：这一枪是**服务进程**发的，
        // 而地址整条来自用户配置 ⇒ 不设闸就是一条盲 SSRF（打本机路由、云元数据、
        // 内网段）。闸的判据与边界写在 `push::validate_push_url_resolved` 上 ✓。
        //
        // ⚠ 放在**这个**位置（唯一的 HTTP 出口）而不是各个渠道的 builder 里：
        // 渠道会多起来（customPost / 企业微信 / TG / serverchan / …），
        // 一处一处加早晚漏一处 ✗。这里是所有渠道的必经之路 ✓。
        //
        // ⚠⚠ 2026-10-09 起走的是**第二层**（`validate_push_url_resolved` ✓）：
        // 字面量那层挡不住 `http://127.0.0.1.nip.io:3998/`（**实测绕过** ✗）——
        // 名字里嵌的 IP 要**真的解析**才看得见 ✓。放行留痕（warn）在闸里面发 ✓。
        crate::push::validate_push_url_resolved(&req.url, self.resolver.as_ref(), self.allow_local)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        let http_req = HttpRequest {
            method: req.method.clone(),
            url: req.url.clone(),
            headers: req.headers.clone(),
            body: req.body_bytes(),
        };
        let timeout = Duration::from_millis(req.timeout_ms);
        match tokio::time::timeout(timeout, self.http.request(http_req)).await {
            Ok(Ok(resp)) => Ok(PushResponse {
                status: resp.status,
                body: resp.body,
            }),
            Ok(Err(error)) => Err(anyhow::anyhow!(error.to_string())),
            Err(_) => Err(anyhow::anyhow!("推送请求超时（{}ms）", req.timeout_ms)),
        }
    }
}

/// 投递器：按**行为绑的规则**投递，每条规则用**自己那份配置**发一次。
///
/// # 为什么只需要一格 `store`
///
/// 原先它有三格（`AccountStore` + 规则表 + 那个读用户级配置的窄 trait ✗）。
/// 2026-10-08 推翻重做后：
/// * 运行类要读的只有**发起方**（定时/编排）与**规则表** ✓；
/// * 面板类要读的只有**面板绑定 + 节流状态**（按 owner ✓，触发线给 owner ✓）。
///
/// 全都在 `Store` 上 ✓ ⇒ 一格就够 ✓（加宽依赖面的代价：测试替身要能造 `Store`；
/// 本文件的测试改用**真 `JsonStore`**（`mcloud-store` 本来就是本 crate 的依赖 ✓）
/// —— 顺带把"落盘 + 读回"也测了 ✓，比再写一个 40 个方法的替身划算 ✓）。
pub struct Notifier {
    /// **完整存储**（规则表 / 发起方 / 面板绑定 / 节流状态 / 运行历史 ✓）。
    ///
    /// # ⚠ 为什么是 `Option`
    ///
    /// 与本仓 `CancelSlot` 同一条口径：**没接上就零行为** ✓，不是崩 ✗。
    /// 只有测试会传 `None` ✓（= 一个包都不发 ✓）。
    store: Option<Arc<dyn Store>>,
    http: Arc<dyn HttpClient>,
    clock: Arc<dyn Clock>,
    /// **按账号**造 HTTP 客户端的工厂 —— 「直播自动定时」的重算要用它去上游现取
    /// 「下一场」（`live_auto::after_run` ✓）。
    ///
    /// # ⚠ 它与上面那个 `http` **不是一回事**
    ///
    /// `http` 是**推送投递**用的**裸**客户端：推送打的是用户自己配的外部地址，
    /// 一个云盘凭据都不能给（见 `AppBuilder` 里那段 ✓）。
    /// 而重算要拿**这个账号自己的票**去打直播侧 ⇒ 必须用按账号注入凭据的**工厂** ✓。
    /// 两者混用任一方向都是事故：拿裸客户端去取日程 = 未登录；
    /// 拿带凭据的工厂去推外部地址 = 把凭据发给第三方 ✗。
    ///
    /// ⚠ `Option` 同上：**没接上就零行为** ✓ —— 重算会走池里那份兜底，
    /// 不是 panic、也不是"今天没直播"（见 `live_auto::reschedule_live_auto` ✓）。
    http_for: Option<mcloud_engine::HttpClientFactory>,
    /// **在飞投递数** —— CLI 收尾要等它归零（见 [`Notifier::flush`] ✓）。
    inflight: Arc<AtomicUsize>,
    /// 归零时叫醒等待者 ✓。
    idle: Arc<tokio::sync::Notify>,
}

/// 面板行为的一条待发告警。
///
/// ⚠ 正文（`text`）由**触发线**给 ✓ —— 这一层只负责"该不该发、发给谁" ✓。
/// `title` 同理（例如"mcloud 性能告警 · 耗时异常" ✓）；留空则回落到规则自己
/// `config.title`（运行类那条路用的就是它 ✓）。
pub struct PanelNotice {
    pub behavior: BehaviorKey,
    /// 这次是**进入异常**还是**恢复正常** ✓ —— 恢复那一条**永远放行** ✓
    /// （用户口径：状态变化要报，否则用户以为它一直坏着 ✗）。
    pub status: BehaviorStatus,
    pub title: String,
    pub text: String,
}

/// 面板行为一次投递的结果 ✓。
///
/// # 为什么要把结果回出去（而不是只记日志）
///
/// 用户口径是「节流被拦时**必须**在推送日志里留痕 ✓，**不许静默** ✗」——
/// 留痕做了三处（`tracing` / 落盘计数 / 这个返回值 ✓），而**只有这个返回值能被
/// 测试断言** ✓：日志没法在单测里稳定地抓 ✗，落盘计数要额外读一次文件 ✓。
/// 于是"拦下"这件事有了一个**明确的类型**，而不是"没发出去，原因不明" ✗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelOutcome {
    /// 没接存储 / 这个行为**没绑规则** ⇒ **0 个请求** ✓。
    Nothing,
    /// 被节流拦下 ✓（`throttled_today` = 今天已节流多少次 ✓）。
    Suppressed {
        reason: ThrottleReason,
        throttled_today: u32,
    },
    /// 放行，并且真的给 `rules` 条规则各发了一次 ✓
    /// （停用/没有正文可推的规则**不算**在里面 ✓）。
    Sent { rules: usize },
}

/// 发起方（定时 / 编排）绑的推送规则与结局过滤 ✓。
struct PushTarget {
    rules: Vec<PushRuleId>,
    events: Vec<PushEvent>,
}

impl Notifier {
    pub fn new(http: Arc<dyn HttpClient>, clock: Arc<dyn Clock>) -> Self {
        Self {
            store: None,
            http,
            clock,
            http_for: None,
            inflight: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// 接上**完整存储**（规则表 / 发起方 / 面板绑定 / 节流状态 / 运行历史 ✓）。
    /// 线上由 `AppBuilder` 接 ✓；测试可以不接 ✓（= 不推 ✓）。
    pub fn with_store(mut self, store: Option<Arc<dyn Store>>) -> Self {
        self.store = store;
        self
    }

    /// 接上**按账号**的 HTTP 客户端工厂（「直播自动定时」的重算用它去上游现取 ✓）。
    ///
    /// ⚠ 不接的后果**不是崩**、也**不是报错**：重算会退化成"读池里那份兜底"
    ///（上一版的行为 ✓）。所以线上**必须**接（`AppBuilder` 接了 ✓），
    /// 而"接没接上"由 `notify.rs` 那条从收尾链走一遍的用例钉住 ✓
    ///（否则这个漏接线会表现成"功能看着好的、其实是旧的" ✗）。
    pub fn with_http_for(mut self, http_for: mcloud_engine::HttpClientFactory) -> Self {
        self.http_for = Some(http_for);
        self
    }

    /// 记一次"投递开始了"（**必须在 spawn 之前**调用 ✓ —— 否则 flush 可能在
    /// 计数还没加上去时就看到 0 而直接返回 ✗）。
    ///
    /// ⚠ `pub(crate)`：收尾那一跳的**第二件事**（直播自动定时的重算 ✓）也用它，
    /// 见 [`DeliveryGuard`] 的说明 ✓。
    pub(crate) fn begin_delivery(&self) -> DeliveryGuard {
        self.inflight.fetch_add(1, Ordering::SeqCst);
        DeliveryGuard {
            inflight: Arc::clone(&self.inflight),
            idle: Arc::clone(&self.idle),
        }
    }

    /// 存储（给「直播自动定时」的重算用 ✓）。
    ///
    /// ⚠ `pub(crate)` 而不是 `pub`：外面拿存储要走 `AppHandle.store` ✓ ——
    /// 从投递器里掏存储是**收尾那一跳的内部需要**（`live_auto::after_run` ✓），
    /// 不该变成一条对外的口子 ✗。
    pub(crate) fn store(&self) -> Option<&Arc<dyn Store>> {
        self.store.as_ref()
    }

    /// 时钟（同上 ✓）。用**注入的**时钟而不是 `SystemTime::now()` ——
    /// 算出来的时刻要能在测试里钉住（`docs/conventions.md` §7.2 ✓）。
    pub(crate) fn clock(&self) -> &Arc<dyn Clock> {
        &self.clock
    }

    /// **按账号**的 HTTP 客户端工厂（给「直播自动定时」的重算去上游现取 ✓）。
    ///
    /// ⚠ `pub(crate)` 与 [`Self::store`] 同一条口径：这是**收尾那一跳的内部需要**
    ///（`live_auto::spawn_after_run` ✓），不该变成一条对外的口子 ✗。
    /// `None` = 没接线（只有测试会这样 ✓）⇒ 调用方走池里那份兜底 ✓。
    pub(crate) fn http_for(&self) -> Option<&mcloud_engine::HttpClientFactory> {
        self.http_for.as_ref()
    }

    /// 等**所有在飞的投递**落地。
    ///
    /// # 为什么 CLI 非要这一下（2026-10-07 用户口径「想想 cli 上该怎么做，记得兼容」✓）
    ///
    /// 投递是 `spawn` 出去的（`notify.rs` 的 `Finished` 分支 ✓）—— 对**长驻的服务**
    /// 这是对的 ✓（HTTP 慢不该拖住运行收尾 ✓）。但 **CLI 是短命进程** ✗：
    /// `mcloud run` 跑完就 `main` 返回 ⇒ 进程退出 ⇒ 在飞的推送**被一起掐掉** ✗，
    /// 而用户看到的却是"命令跑完了、退出码 0" ✗ —— 那比不推还坏 ✓。
    ///
    /// TS 那份（`mcloud-sign/packages/cli/src/index.ts` ✓）是 `await sendNotify(...)` ✓
    /// ⇒ **推完才退** ✓。这条 flush 就是把它补回来 ✓：CLI 收尾 `await notifier.flush()` ✓。
    pub async fn flush(&self) {
        loop {
            if self.inflight.load(Ordering::SeqCst) == 0 {
                return;
            }
            self.idle.notified().await;
        }
    }

    /// 找**这次运行该用哪些规则**（用户口径：「行为选择规则，一个行为可以选择
    /// **多个**规则」✓）。
    ///
    /// 认领顺序：发起方（`schedule:<id>` / `pipeline:<id>` ✓，见
    /// `RunEvent::Started.origin` ✓）→ 没有 ⇒ `None`（**手动跑不推** ✓，
    /// 用户口径与 §6 验收第 3 条：没绑规则 ⇒ **0 个请求** ✓）。
    ///
    /// ⚠ `origin` 认不出来（前缀不对 / 那条定时已被删 ✓）⇒ **不推**并 warn ✓：
    /// 这一版已经没有"用户级配置"可以回落了 ✗（那层整个删了 ✓），
    /// 硬凑一份出来就是"猜用户的凭据" ✗。
    async fn target_for(&self, store: &Arc<dyn Store>, origin: Option<&str>) -> Option<PushTarget> {
        let origin = origin?;
        if let Some(sid) = origin.strip_prefix("schedule:") {
            return match store.schedule(&ScheduleId::new(sid)).await {
                Ok(Some(sch)) => Some(PushTarget {
                    rules: sch.push_rules,
                    events: sch.push_events,
                }),
                Ok(None) => {
                    tracing::warn!(schedule = sid, "发起方定时不在库里，本次不发推送");
                    None
                }
                Err(e) => {
                    tracing::warn!(schedule = sid, error = %e, "读定时失败，本次不发推送");
                    None
                }
            };
        }
        if let Some(pid) = origin.strip_prefix("pipeline:") {
            return match store.pipeline(&PipelineId::new(pid)).await {
                Ok(Some(p)) => Some(PushTarget {
                    rules: p.push_rules,
                    events: p.push_events,
                }),
                Ok(None) => {
                    tracing::warn!(pipeline = pid, "发起方编排不在库里，本次不发推送");
                    None
                }
                Err(e) => {
                    tracing::warn!(pipeline = pid, error = %e, "读编排失败，本次不发推送");
                    None
                }
            };
        }
        // 认不出来的 origin（例如将来多一种发起方但这里没跟上 ✓）：**不推** ✓
        // 但要说出来 —— 静默不发是"配了规则却什么都不来"里最难查的一种 ✗。
        tracing::warn!(origin, "认不出发起方，本次不发推送");
        None
    }

    /// 按 id 取一条规则（**按 owner 取** ✓）。
    ///
    /// 面板类手里有 owner ✓（触发线给的 ✓），所以走 `push_rule(owner, id)` ——
    /// 单条读、还顺带把"别人的规则"挡在外面 ✓。
    ///
    /// ⚠ 运行类**不用它**：那条路手里没有"当前用户"（见 [`Self::rule_anywhere`] ✓）。
    async fn rule_of(
        &self,
        store: &Arc<dyn Store>,
        owner: &UserId,
        id: &PushRuleId,
    ) -> Option<PushRule> {
        match store.push_rule(owner, id).await {
            Ok(Some(rule)) => Some(rule),
            Ok(None) => {
                tracing::warn!(owner = %owner, rule = %id, "面板行为绑的规则不存在，跳过");
                None
            }
            Err(e) => {
                tracing::warn!(owner = %owner, rule = %id, error = %e, "读规则失败，跳过");
                None
            }
        }
    }

    /// 按 id 取一条规则（**不带 owner** ✓）。
    ///
    /// # 为什么运行类只能这样查
    ///
    /// 一次定时触发、一次编排跑完，手里只有 `Schedule` / `Pipeline` 与那个**账号** ——
    /// **没有"当前用户"这个概念** ✗（账号的归属是 `Account::owner`，而不是"正在看
    /// WebUI 的人" ✓）。而规则 id 本身已经够定位了 ✓（定时/编排上存的就是它 ✓）。
    /// 所以用 `all_push_rules()` 摊平再找 ✓ —— 与 `UserStore::all_push_rules` 的
    /// 说明同一条理由 ✓。
    async fn rule_anywhere(&self, store: &Arc<dyn Store>, id: &PushRuleId) -> Option<PushRule> {
        match store.all_push_rules().await {
            Ok(rules) => {
                let found = rules.into_iter().find(|r| &r.id == id);
                if found.is_none() {
                    tracing::warn!(rule = %id, "定时/编排绑的规则不存在，跳过");
                }
                found
            }
            Err(e) => {
                // 读不出来 ⇒ 不推并 warn（**不静默丢** ✓）：宁可少推一条，
                // 也别让"配了规则却什么都不来"变成查不出的谜 ✓。
                tracing::warn!(rule = %id, error = %e, "读推送规则失败，跳过");
                None
            }
        }
    }

    /// 「性能异常」判定（`perfSlow` / `perfFailStreak` 两个面板行为的触发线调它 ✓）。
    ///
    /// # 为什么从**运行历史**里算，而不是另采指标
    ///
    /// "性能异常"要回答的是"这次是不是不对劲" ✓ —— 而 `history.json` 里本来就有
    /// 每次运行的 `task / account / durationMs / outcome` ✓（`RunRecord` ✓）。
    /// 另起一套进程指标（CPU/内存）在这个项目里**没有采集器** ✗，硬造一个是自欺 ✓。
    ///
    /// 返回 `(行为, 告警正文)` ✓；`None` = 这次没异常 ✓。
    /// ⚠ 触发线**这一轮不接**（见模块头那张表：接在哪、怎么调 ✓）。
    ///
    /// ⚠ 要 `owner`：两个阈值（倍数 / 条数 ✓）是**行为上的可配项**，而绑定按 owner
    /// 分表（`data/push-panel.json` ✓）—— 与 [`Self::deliver_panel`] 同一条口径 ✓。
    /// 触发线手里本来就有 owner（`account → account.owner` ✓，见模块头 ✓）。
    pub async fn perf_alert(
        &self,
        owner: &UserId,
        task: &str,
        account: &str,
        duration_ms: u64,
    ) -> Option<(BehaviorKey, String)> {
        let store = self.store.as_ref()?;
        // ⚠ 阈值从**这个 owner 的绑定**里读（用户原话：「耗时异常 …… 选规则 ＋ **倍数**」
        // 「连续失败 …… 选规则 ＋ **条数**」✓）。`None` = 用户没动过 ⇒ 交给
        // `perf_verdict` 的兜底 ✓（**别在这里再写一份 3.0 / 3** ✗）。
        // 读绑定失败 ⇒ 按内置默认判 ✓（宁可阈值退回默认，也别让一次读盘失败变成"永不告警" ✗）。
        let (slow_factor, fail_streak) = match store.push_panel(owner).await {
            Ok(b) => (b.perf_slow.slow_factor, b.perf_fail_streak.fail_streak),
            Err(e) => {
                tracing::warn!(owner = %owner, error = %e, "读面板推送绑定失败，阈值按内置默认判");
                (None, None)
            }
        };
        let history = match store.runs(200).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "读运行历史失败，跳过性能判据");
                return None;
            }
        };
        // 只看**同一个任务 + 同一个账号**（同一任务的耗时才有可比性 ✓；
        // 账号也一起限定：不同账号的云盘大小差很多 ✓）。
        let same: Vec<&RunRecord> = history
            .iter()
            .filter(|r| r.task == task && r.account == account)
            .collect();
        perf_verdict(task, account, &same, duration_ms, slow_factor, fail_streak)
    }

    /// **触发线的入口**：把一次面板告警丢进后台投递（**不阻塞调用方** ✓）。
    ///
    /// # 为什么触发线必须走这一条，而不是自己 `spawn` + `deliver_panel`
    ///
    /// `deliver_panel` 会**同步走完 HTTP**（`PushRequest::timeout_ms` 最多 10s ✓）——
    /// 登录路由上 `await` 它，等于让"用户能不能进门"取决于"推送服务器快不快" ✗。
    /// 而 `begin_delivery` 是**私有**的（在飞计数就是 `flush` 的判据 ✓）⇒
    /// 触发线自己 spawn 就绕过了那个计数：CLI 收尾 `flush()` 会在它落地前返回 ✗。
    /// 收在一处，这两件事就不会各自漂移 ✓。
    ///
    /// ⚠ 计数**在 spawn 之前**加上（见 `Self::begin_delivery` ✓）——
    /// 否则 `flush` 可能在计数还没加上去时就看到 0 而直接返回 ✗。
    pub fn spawn_panel(self: &Arc<Self>, owner: UserId, notice: PanelNotice) {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let notifier = Arc::clone(self);
                let guard = notifier.begin_delivery();
                handle.spawn(async move {
                    let _guard = guard;
                    notifier.deliver_panel(&owner, notice).await;
                });
            }
            // 没有运行时（例如脱离 tokio 直接调）：只记一条日志，绝不 panic。
            Err(_) => tracing::warn!(owner = %owner, "没有 tokio 运行时，面板推送跳过"),
        }
    }

    /// **性能面板行为**的触发线（一次运行收尾调它**一次** ✓）。
    ///
    /// 顺序：`account → account.owner` ⇒ [`Self::perf_alert`] 判据 ⇒
    /// [`Self::deliver_panel`] 投递 ✓。
    ///
    /// # 为什么 owner 只能从账号取（拿不到就**不发**）
    ///
    /// 一次运行手里只有账号 id ✓（`RunRecord` 里**没有** owner 那一格 ✗），
    /// 而账号的归属是显式写死的（`Account::owner` ✓）⇒ 它是这条口径的唯一来源 ✓。
    /// 账号不在库里 / 那一格是 `None` ⇒ **不发** ✓ —— 猜一个用户出来，
    /// 等于把别人的规则当成他的用 ✗（与模块头"不许按账号猜用户"同一条 ✓）。
    ///
    /// # ⚠ 判据依赖"这次运行**已经落库**"
    ///
    /// `perf_alert` 是从运行历史里数的（连续失败条数 / 耗时中位数 ✓）⇒
    /// 本次那条必须在里面 ✓，否则"连败 2 次"要到第 3 次才报 ✗（差一次）。
    /// 这件事由 `runner.rs` 的**顺序**保证：它现在先 `upsert_run`、
    /// 再 `emit(Finished)` ✓（见那边的注释 ✓）。
    ///
    /// # ⚠ 主动中止**两边都不算**（2026-10-08 改的口径）
    ///
    /// 这一版之前：中止的运行 `outcome` 也是 `Failed`（任务在安全点返回错误 ✓）
    /// ⇒ 它会被计进"连续失败" ✗，而"用户自己按了中止"被报成"任务连续失败"
    /// 是纯噪声 ✓。现在 [`perf_verdict`] 按 `RunRecord::status` 把
    /// `Cancelled` 的记录**两边都排除**（连败 + 耗时中位数 ✓）——
    /// 理由与"中止不算失败"那条写在 [`perf_verdict`] 的说明里 ✓。
    async fn perf_check(&self, meta: &RunMeta, duration_ms: u64) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let account_id = AccountId::new(meta.account.clone());
        let owner = match store.account(&account_id).await {
            Ok(Some(account)) => match account.owner {
                Some(owner) => owner,
                None => {
                    tracing::debug!(
                        account = %meta.account,
                        "账号没有归属，性能面板推送跳过（不许猜用户）"
                    );
                    return;
                }
            },
            Ok(None) => {
                tracing::debug!(account = %meta.account, "账号不在库里，性能面板推送跳过");
                return;
            }
            Err(e) => {
                tracing::warn!(account = %meta.account, error = %e, "读账号失败，性能面板推送跳过");
                return;
            }
        };
        let Some((behavior, text)) = self
            .perf_alert(&owner, &meta.task, &meta.account, duration_ms)
            .await
        else {
            return;
        };
        // 这一层**不 spawn**：调用方（`NotifySink` 的 Finished 分支 ✓）本来就在
        // 一个 spawn 出去的任务里 ✓（见模块头"挂点"那段 ✓）。
        self.deliver_panel(
            &owner,
            PanelNotice {
                behavior,
                status: BehaviorStatus::Abnormal,
                title: format!("mcloud 性能告警 · {}", behavior.label()),
                text,
            },
        )
        .await;
    }

    /// 落盘节流状态。失败只 warn ✓（投递是旁路，不能因为写盘失败把一次运行带崩 ✗）。
    ///
    /// ⚠ 代价说清：写失败 ⇒ 重启后这次冷却记不住 ⇒ **可能多推一条** ✓
    /// （宁可多一条，不可崩一次 ✓）。
    async fn save_throttle(&self, store: &Arc<dyn Store>, owner: &UserId, state: &ThrottleState) {
        if let Err(e) = store.save_push_throttle(owner, state).await {
            tracing::warn!(owner = %owner, error = %e, "节流状态落盘失败：重启后这次冷却记不住");
        }
    }

    /// **面板行为**的一次投递：过闸 ⇒ 每条规则各发一次 ✓。
    ///
    /// 顺序上有一处是刻意的：**先把新状态落盘，再发** ✓。
    /// 反过来的话，"发出去之后、落盘之前"进程被杀 ⇒ 下次重启还会再发一条 ✗
    /// （而重启本来就可能紧接着一串触发 ✓）。与定时水位线同一条取舍：
    /// **宁可漏发一条，不可重发一轮** ✓。
    pub async fn deliver_panel(&self, owner: &UserId, notice: PanelNotice) -> PanelOutcome {
        let Some(store) = self.store.as_ref() else {
            tracing::debug!(owner = %owner, "没接存储，面板推送跳过");
            return PanelOutcome::Nothing;
        };
        let bindings = match store.push_panel(owner).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(owner = %owner, error = %e, "读面板推送绑定失败，本次不发");
                return PanelOutcome::Nothing;
            }
        };
        let binding = bindings.get(notice.behavior);
        if binding.rules.is_empty() {
            // 空 = 不推 ✓（与"没配"同义，不是"全推" ✗）。
            tracing::debug!(owner = %owner, behavior = ?notice.behavior, "这个面板行为没绑规则，不发");
            return PanelOutcome::Nothing;
        }

        let mut state = match store.push_throttle(owner).await {
            Ok(s) => s,
            Err(e) => {
                // 读不出来 ⇒ 按"从没发过"处理（全默认 ✓）。⚠ 这会让冷却与计数**这次**
                // 失效 —— 但反过来（因为读不到就不发）是"用户以为配好了却什么都不来" ✗，
                // 更坏 ✓。写盘失败另有一处 warn ✓。
                tracing::warn!(owner = %owner, error = %e, "读节流状态失败，本次按从未发送处理");
                ThrottleState::default()
            }
        };
        let now = self.clock.now_ms();
        let day = local_day(now);
        let decision =
            binding
                .throttle
                .decide(notice.status, now, day, &state.get(notice.behavior));

        let next = match decision {
            ThrottleDecision::Send(next) => next,
            ThrottleDecision::Suppressed { reason, next } => {
                state.set(notice.behavior, next.clone());
                self.save_throttle(store, owner, &state).await;
                // ⚠ **不许静默**（用户口径 ✓）：原因、今天已节流多少次、上限是多少
                // 都要在日志里 ✓ —— 用户拿着这条日志才知道"不是没触发，是被节流了" ✓。
                tracing::warn!(
                    owner = %owner,
                    behavior = ?notice.behavior,
                    reason = reason.label(),
                    throttled_today = next.throttled_today,
                    sent_today = next.sent_today,
                    daily_cap = binding.throttle.daily_cap,
                    cooldown_secs = binding.throttle.cooldown_secs,
                    "面板推送被节流拦下（今天已节流 {} 次）",
                    next.throttled_today
                );
                return PanelOutcome::Suppressed {
                    reason,
                    throttled_today: next.throttled_today,
                };
            }
        };

        // ⚠ 先落盘再发（见函数说明 ✓）。
        state.set(notice.behavior, next);
        self.save_throttle(store, owner, &state).await;

        let transport = HttpPushTransport::new(Arc::clone(&self.http));
        // ⚠ 面板告警**不是**"运行日志的转写" ⇒ 正文直接来自 `notice.text` ✓
        // （`onlyError` / `minLevel` 那两个过滤旋钮在这里不参与 ✓ —— 它们管的是
        // "一次运行的日志里挑哪几行" ✓）。但 `send_notify` 需要一个收集器记失败/跳过 ✓，
        // 所以给一份临时的（它不会被读 ✓）。
        let logs = PushLogCollector::new();
        let ctx = PushContext::new(&transport, self.clock.as_ref(), &logs);
        let mut sent = 0usize;
        for id in &binding.rules {
            let Some(rule) = self.rule_of(store, owner, id).await else {
                continue;
            };
            if !rule.enabled {
                tracing::debug!(rule = %id, "规则已停用，跳过");
                continue;
            }
            let config = rule.config.clone();
            let title = if notice.title.trim().is_empty() {
                config.title.clone()
            } else {
                notice.title.clone()
            };
            crate::push::send_notify(&ctx, &config, &title, &notice.text).await;
            sent += 1;
        }
        PanelOutcome::Sent { rules: sent }
    }

    /// **试发一条规则**（`POST /api/push/test` 用它 ✓）—— 用那条规则**自己的**
    /// 平台与凭据真发一条测试消息 ✓。
    ///
    /// # 为什么放在这一层，而不是路由那边自己建 transport
    ///
    /// 真发送要一个 [`PushContext`]，而生产那个传输实现（[`HttpPushTransport`] ✓，
    /// 带 SSRF 闸 ✓）是**本文件的私有类型** ✗。路由拿不到它，也不该另写一份
    /// （同一件事两个实现，迟早分叉 ✗ —— 见 `routes/push.rs` 里那段"缺什么"）。
    ///
    /// # ⚠ 这是**对外发消息**的动作
    ///
    /// 只在**用户显式点了"测试"**时调用 ✓ —— 任何自动路径（运行收尾 / 面板触发）
    /// 都不许调它 ✗：用户没点，就不该有一条消息出去 ✓。
    ///
    /// # 返回值 = 每个渠道这一次的结果
    ///
    /// 试发是"点一下就要看到结果"的动作，只回一个 `ok: true` 等于让用户去翻服务日志 ✗
    /// （而服务日志在界面上是另一个窗口 ✓）。所以把收集器里的行原样回出去 ✓：
    /// `Server酱已发送！` / `pushplus发送失败: HTTP 400` 都是渠道**自己**写的原话 ✓。
    ///
    /// ⚠ 它**不等于**"发成功了"：每行里的成败由 `send_request` 判（errcode / 非 2xx ✓），
    /// 与运行类推送**同一套判据** ✓。
    ///
    /// ⚠ 这里**不看** `rule.enabled`：用户点"测试"要验的是**凭据通不通** ✓，
    /// 一条被停用的规则照样该能试 ✓（停用管的是"自动路径发不发" ✓）。
    pub async fn send_test_rule(
        &self,
        rule: &PushRule,
        title: &str,
        text: &str,
    ) -> Vec<PushLogEntry> {
        let transport = HttpPushTransport::new(Arc::clone(&self.http));
        // 临时收集器（不会被读回运行历史 ✓）：它与运行类那条路用的是同一份
        // `send_notify` ✓ —— 试发因此**不可能**与真发送走两套判据 ✓。
        let logs = PushLogCollector::new();
        let ctx = PushContext::new(&transport, self.clock.as_ref(), &logs);
        crate::push::send_notify(&ctx, &rule.config, title, text).await;
        logs.entries()
    }

    /// 投递一次运行的结果。
    ///
    /// **刻意不返回 Result**：非 2xx、网络错、超时都只落进推送日志（各 send_*
    /// 内部消化）。这是"投递只是旁路"的落点 —— 通知失败不能反过来影响运行结果。
    async fn deliver(&self, run: FinishedRun) {
        let FinishedRun {
            run_id,
            meta,
            outcome,
            duration_ms,
            cancelled,
            logs,
        } = run;

        let Some(store) = self.store.as_ref() else {
            tracing::debug!(run = run_id.get(), "没接存储，推送跳过");
            return;
        };

        // ⓪ **性能面板行为**（`perfSlow` / `perfFailStreak` ✓）—— 与下面那条
        //    运行类的路**完全独立**：它不看 `origin`、也不看定时/编排绑了谁 ✓，
        //    只按 `account.owner` 找面板绑定 ✓。所以它必须排在那些提前 return
        //    **之前** ✓ —— 手动跑（`origin == None` ✓）的一次运行同样可能"变慢了"✓。
        //    （一次运行只判一次 ✓：这里正是收尾那一跳 ✓。）
        self.perf_check(&meta, duration_ms).await;

        // ① **发起方**绑的规则与结局过滤（用户口径：行为选择规则 ✓）。
        //    这个 await 发生在 spawn 出去的独立任务上，不在同步的 emit 里，
        //    它的延迟不算进运行收尾。
        let Some(target) = self.target_for(store, meta.origin.as_deref()).await else {
            return;
        };
        if target.rules.is_empty() {
            // ⭐ **没绑规则 ⇒ 0 个请求** ✓（§6 验收第 3 条）——
            //    这一版已经没有"用户级配置"可以回落 ✗（那层整个删了 ✓）。
            tracing::debug!(run = run_id.get(), "这次运行没绑推送规则，一个请求都不发");
            return;
        }

        // ② 结局过滤（空 = **跑完就推** ✓；非空 = 只在这些结局推 ✓）。
        let event = event_of(&outcome, cancelled);
        if !target.events.is_empty() && !target.events.contains(&event) {
            tracing::debug!(
                run = run_id.get(),
                ?event,
                "这个结局不在 push_events 里，不发"
            );
            return;
        }

        // ③ 把结尾摘要补成日志的最后一行 —— 它就是旧载荷里 severity + summary
        //    的接替者：类型决定 onlyError 会不会触发（见 outcome_line）。
        //
        //    ⚠ **只补一次**：多条规则共用同一批日志 ✓（一条规则补一遍的话，
        //    第二条规则的正文里就会出现两份摘要 ✗）。
        let (kind, summary) = outcome_line(&outcome, cancelled);
        logs.write(
            kind,
            format!(
                "{}@{} run#{} 用时{}ms：{}",
                meta.task,
                meta.account,
                run_id.get(),
                duration_ms,
                summary
            ),
            self.clock.now_ms(),
        );

        // ④ **每条规则各按自己的 `config` 发一次** ✓（§6 验收第 1、2 条）——
        //    平台与 token 全部来自那条规则自己 ✓，互相之间没有任何共享状态 ✓。
        let transport = HttpPushTransport::new(Arc::clone(&self.http));
        let ctx = PushContext::new(&transport, self.clock.as_ref(), &logs);
        for id in &target.rules {
            let Some(rule) = self.rule_anywhere(store, id).await else {
                continue;
            };
            if !rule.enabled {
                tracing::debug!(rule = %id, "规则已停用，跳过");
                continue;
            }
            let config = rule.config;
            // onlyError + minLevel 都在 `push_text` 里（它把两步收成一个函数 ✓），
            // 而这两个旋钮现在是**这条规则自己的** `config` 上的 ✓ ——
            // 于是"这条定时只推失败、那条连成功一起推"就真的表达得出来了 ✓。
            let Some(text) = config.push_text(&logs) else {
                tracing::debug!(rule = %id, run = run_id.get(), "不满足推送条件，跳过");
                continue;
            };
            let title = config.title.clone();
            crate::push::send_notify(&ctx, &config, &title, &text).await;
        }
    }
}

/// 取消登记表的**迟到句柄**。
///
/// 装饰器必须先于 Runner 存在（Runner 拿它当 sink），而登记表是 Runner::new
/// 内部造的 —— 所以先给一个空槽位，runner 造好后立刻回填。AppBuilder::build
/// 在返回前一定填好，而那时还没有任何一次运行开始，因此不需要额外的同步。
pub type CancelSlot = Arc<OnceLock<Arc<CancelRegistry>>>;

/// 空槽位。
pub fn cancel_slot() -> CancelSlot {
    Arc::new(OnceLock::new())
}

/// 「性能异常」的**纯判据**（时间/历史/两个阈值都由调用方给 ✓ ⇒ 可测 ✓）。
///
/// 两条：
/// * 连续失败：同一任务最近**连着**失败 ≥ `fail_streak` 次；
/// * 耗时异常：这次耗时 > 历史（`duration_ms > 0` 的那些）**中位数** × `slow_factor`，
///   且样本 ≥ 5（三个样本的"中位数"没有意义 ✗）。
///
/// ⚠ 两个阈值都是 `Option`（来自绑定 ✓）：`None` = 用户没动过 ⇒ **在下面这一处**
/// 兜底成 [`FAIL_STREAK`] / [`SLOW_FACTOR`] ✓（别在调用方再写一份默认值 ✗）。
///
/// # ⚠⚠ 被**主动中止**的运行两边都不算（2026-10-08 用户口径）
///
/// 中止的运行 `outcome` 多半是 `Failed`（任务在安全点 `check_cancelled()?` 退出 ✓），
/// 但它**不是**"任务自己失败" ✗ —— 是用户按了按钮 ✓。所以：
///
/// * **连败**：中止的那条**跳过**（不计数、也**不断链** ✓）—— 历史
///   「失败、失败、中止、失败」的连败是 **3**（不是 4，也不是 2 ✓）。
///   中止本身不算失败 ✓，但它也不该把用户前面真实的连败**洗掉** ✓；
/// * **耗时中位数**：中止的那条**也排除** ✓ —— 它量的是"用户什么时候按的中止"，
///   不是"这个任务要跑多久" ✗。留在样本里会把中位数**拉低**（中止通常发生得早 ✓），
///   于是下一次正常运行反而被判成"耗时异常" ✗ —— 一个纯噪声的告警 ✓。
///
/// 判据用 `r.status != RunStatus::Cancelled`（`RunRecord` 上的生命周期状态 ✓），
/// **不是**看 `outcome`：中止与真失败在 `outcome` 上分不开 ✗
///（同一个理由见 `RunRecord::needs_attention` ✓）。
///
/// ⚠ 判"最近连着失败"用的是 `take_while`（**从头开始连着数** ✓）——
/// 中间只要成功过一次，计数就断了 ✓。写成 `filter().count()` 会把"历史上失败过
/// 三次"也算成"连续失败三次" ✗。
fn perf_verdict(
    task: &str,
    account: &str,
    same: &[&RunRecord],
    duration_ms: u64,
    slow_factor: Option<f64>,
    fail_streak: Option<u32>,
) -> Option<(BehaviorKey, String)> {
    // ⚠ 兜底**只在这里**：`None` = 用户没动过 ⇒ 跟随内置默认 ✓。
    let fail_streak = fail_streak.unwrap_or(FAIL_STREAK);
    let slow_factor = slow_factor.unwrap_or(SLOW_FACTOR);
    // ⚠ **只筛一次**（两条判据共用 ✓）：中止的记录两边都不该参与 ✓
    // —— 见上面那段说明 ✓。
    let counted: Vec<&RunRecord> = same
        .iter()
        .copied()
        .filter(|r| r.status != RunStatus::Cancelled)
        .collect();
    if fail_streak > 0 {
        let streak = counted
            .iter()
            .take_while(|r| !r.outcome.is_success())
            .count() as u32;
        if streak >= fail_streak {
            return Some((
                BehaviorKey::PerfFailStreak,
                format!(
                    "任务 {task}@{account} 已连续失败 {streak} 次（阈值 {fail_streak}）—— 看一眼是不是凭据过期或上游改版了"
                ),
            ));
        }
    }

    // ⚠ 拿**历史**样本算中位数：`same` 里可能已经含这次自己（收尾时已落库 ✓），
    // 把异常算进去会被自己抬高 ✓ —— 这里只做 `duration_ms > 0` 的过滤，
    // 与旧实现同一口径（`run_id != 0` 那条过滤对"这次"不起作用，见 git 历史 ✓）。
    let mut durations: Vec<u64> = counted
        .iter()
        .filter(|r| r.duration_ms > 0)
        .map(|r| r.duration_ms)
        .collect();
    durations.sort_unstable();
    if durations.len() >= 5 {
        let median = durations[durations.len() / 2];
        if median > 0 && (duration_ms as f64) > (median as f64) * slow_factor {
            return Some((
                BehaviorKey::PerfSlow,
                format!(
                    "任务 {task}@{account} 这次用了 {duration_ms}ms，历史中位数 {median}ms（{:.1}×，阈值 {slow_factor:.1}×）—— 可能是上游变慢或限流",
                    duration_ms as f64 / median as f64
                ),
            ));
        }
    }
    None
}

/// 结尾摘要的日志类型 —— 旧 Severity 的语义接替者。
///
/// | TaskOutcome / 是否中止 | 类型 | 效果 |
/// |---|---|---|
/// | Success | success（level 3） | 不报红 |
/// | Skipped | info（level 3） | **不算错误**（docs/conventions.md:301） |
/// | Partial | warn（level 1） | 有成有败 ⇒ 不报红、但也别当成普通成功 ✓ |
/// | Failed | error（level 0） | onlyError 会触发；正文里报红 |
/// | 用户主动中止 | warn（level 1） | **不许是 error**（docs/architecture.md:452） |
///
/// 中止**优先于** TaskOutcome：被中止的运行业务上多半是 Failed（任务在安全点
/// check_cancelled()? 退出），但那不是要报红的失败 —— 那正是"用户自己点的中止，
/// 推送里也不该报红"。
fn outcome_line(outcome: &TaskOutcome, cancelled: bool) -> (&'static str, String) {
    if cancelled {
        return ("warn", format!("已中止：{}", outcome.summary()));
    }
    match outcome {
        TaskOutcome::Success { summary } => ("success", summary.clone()),
        TaskOutcome::Skipped { reason } => ("info", format!("跳过：{reason}")),
        // 部分成功：不报红（没到"整个挂了"），但也明确说"有没成的" ——
        // 用户 2026-10-03 实测踩到的正是"有失败却显示成功" ✗。
        TaskOutcome::Partial { summary } => ("warn", format!("部分成功：{summary}")),
        TaskOutcome::Failed { reason } => ("error", format!("失败：{reason}")),
    }
}

/// 运行日志级别 → 推送日志类型。
///
/// 两个枚举的取值刻意一一对应，于是只有真正的 error 运行日志（以及 Failed 的
/// 结尾摘要）会让 PushLogCollector::has_error 为真、让 onlyError 触发。
fn log_type(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// 在飞投递的**守卫**：`Drop` 时减一并叫醒 [`Notifier::flush`] ✓。
///
/// # 为什么用守卫，而不是在 `deliver` 末尾手动减一
///
/// `deliver` 里有多条**提前 return** 的路径（没接存储 ✓、认不出发起方 ✓、
/// 没绑规则 ✓、结局被过滤 ✓）—— 手动减一迟早会漏掉一条 ✗，而漏掉的那条会让
/// CLI 的 `flush` **永远等下去** ✗（命令挂死，比不推更难查 ✓）。`Drop` 覆盖所有出口 ✓。
///
/// ⚠ `pub(crate)`：**运行收尾里不止推送一件事** —— 「直播自动定时」的重算
///（`live_auto::spawn_after_run` ✓）也挂在同一个计数上，
/// 这样 CLI 的 `flush()` 会把**两件都等完**再退出 ✓
///（否则短命进程一退，刚算出来的时刻可能还没落盘就被掐掉 ✗）。
pub(crate) struct DeliveryGuard {
    inflight: Arc<AtomicUsize>,
    idle: Arc<tokio::sync::Notify>,
}

impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        // ⚠ `notify_one`（会**存一个许可** ✓）而不是 `notify_waiters`（只叫醒**正在等**的 ✓）：
        // flush 是"先查计数、再 await"两步 ✓，两步之间那次减一用 notify_waiters 就丢了 ✗
        // ⇒ flush 会一直等下去 ✗。
        self.idle.notify_one();
    }
}

/// 运行结果 → 推送事件（`push_events` 的判据 ✓）。
///
/// ⚠ 与 [`outcome_line`] **刻意分开**：那个决定"日志行标成什么颜色" ✓（`success/warn/error` ✓），
/// 这个决定"该不该按规则推" ✓。两件事的档位**不一样**：用户主动中止在日志里是 `warn` ✓
/// （不许报红 ✓），而在规则里是**独立的一个事件** ✓ —— 有人就是想"被中止时告诉我" ✓。
fn event_of(outcome: &TaskOutcome, cancelled: bool) -> PushEvent {
    if cancelled {
        return PushEvent::RunCancelled;
    }
    // ⚠ 四个变体都是**结构体变体**（带 summary/reason ✓）⇒ 一律 `{ .. }` 匹配 ✓。
    match outcome {
        TaskOutcome::Success { .. } => PushEvent::RunOk,
        TaskOutcome::Partial { .. } => PushEvent::RunPartial,
        // 跳过 = "这次没什么可做的" ✓ ⇒ 归到"成功"那一档：**要报就一起报** ✓，
        // 单独给它一个事件只会多出一个没人会选的开关 ✗。
        TaskOutcome::Skipped { .. } => PushEvent::RunOk,
        TaskOutcome::Failed { .. } => PushEvent::RunFailed,
    }
}

// ⚠ 这里原先有 `local_minutes`（"现在当天第几分钟"，东八区）—— 它只服务
// 「静默时段」那一句判据 ⇒ 随静默时段整块删掉 ✗（2026-10-08 用户口径
// 「不要静默时间」✓）。节流用的"哪一天"是 `mcloud_core::push::local_day` ✓，
// 与正文里的时刻（`local_time_hms`）同一条固定 +8 口径 ✓。

/// 一次运行的身份，来自 Started，供 Finished 组装推送。
#[derive(Debug, Clone, Default)]
struct RunMeta {
    task: String,
    account: String,
    /// **谁发起的**（`schedule:<id>` / `pipeline:<id>` / `None` ✓）——
    /// 推送规则绑在定时/编排上就靠它认领（见 `RunEvent::Started.origin` ✓）。
    origin: Option<String>,
}

/// 在跑的一次运行：身份 + 它自己的日志收集器。
struct RunState {
    meta: RunMeta,
    logs: Arc<PushLogCollector>,
}

/// Finished 那一刻定下来、交给 spawn 出去的任务去做投递的一组值。
struct FinishedRun {
    run_id: RunId,
    meta: RunMeta,
    outcome: TaskOutcome,
    duration_ms: u64,
    cancelled: bool,
    logs: Arc<PushLogCollector>,
}

/// 事件旁路：转发给内层 sink，同时收集日志、在 Finished 触发投递。
pub struct NotifySink {
    inner: Arc<dyn EventSink>,
    notifier: Arc<Notifier>,
    cancel: CancelSlot,
    /// 在跑的运行：run_id → (身份, 它自己的日志收集器)。
    ///
    /// 只在内存里攒、Finished 时删。Started 与 Finished 都从 Runner 的同一个
    /// sink 过，所以这一对一定成对出现（同一 task:account 已在运行时 runner
    /// 直接返回、两个事件都不发）。因此表的大小 = 并发在跑的 run 数。
    started: Mutex<HashMap<RunId, RunState>>,
}

impl NotifySink {
    pub fn new(inner: Arc<dyn EventSink>, notifier: Arc<Notifier>, cancel: CancelSlot) -> Self {
        Self {
            inner,
            notifier,
            cancel,
            started: Mutex::new(HashMap::new()),
        }
    }

    /// 锁中毒时取回内部值继续用（与 EventHub 同一理由：每次操作在锁内一次做完，
    /// 没有"写了一半"的中间态，跟着 panic 只会让整个服务停摆）。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RunId, RunState>> {
        self.started.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventSink for NotifySink {
    fn emit(&self, event: RunEvent) {
        // 出去之前先过一道脱敏（复核意见 §1.14）：
        // 这一路会把任务的日志/结论原样带出去（进程日志 / SSE / 推送 / 落盘），
        // 而上游回显里可能就有一枚凭证 ✗。形状由 mcloud-core 给死 ⇒ sink 只要一行 ✓。
        let event = event.scrubbed();
        // 先做推送侧的动作，再把事件转给内层（内层自己也会脱敏，幂等 ✓）。
        //
        // 判据（是否主动中止）依赖"runner 还没 release 取消登记表"——那与转发
        // 顺序无关：整段都在同一个同步 emit 调用里完成，而 runner 的 release 在
        // emit 返回之后。
        match &event {
            RunEvent::Started {
                run_id,
                task,
                account,
                origin,
                ..
            } => {
                self.lock().insert(
                    *run_id,
                    RunState {
                        meta: RunMeta {
                            task: task.clone(),
                            account: account.clone(),
                            origin: origin.clone(),
                        },
                        logs: Arc::new(PushLogCollector::new()),
                    },
                );
            }
            RunEvent::Log {
                run_id,
                level,
                kind,
                message,
                at_ms,
            } => {
                if let Some(state) = self.lock().get(run_id) {
                    // ⚠ 优先用**分类**（`kind` ✓）—— 推送那套 type 与它是同一套 ✓
                    // （`info/success/start/warn/error/debug` ✓）。
                    //
                    // ⚠⚠ `fail` **落成 `error`**（用户口径 2026-10-08：
                    // 「**某一笔没成**」**就是错误** ✓）。它原来落的是 `warn`
                    // （level 1）✗ —— 而开着「仅错误时推」时 `warn` **不推** ✓，
                    // 于是"这一笔失败了"被静默掉 ✗。用户看到的正是
                    // "有失败却显示成功"那一类（`outcome_line` 的 Partial 注释
                    // 记着同一个坑 ✓）。⇒ 只改**这一格的落法** ✓，别的语义不动 ✓。
                    let ty = match kind {
                        mcloud_core::LogKind::Success => "success",
                        mcloud_core::LogKind::Start => "start",
                        mcloud_core::LogKind::Fail => "error",
                        // 其余沿用按级别的映射（debug/info/warn/error ✓）
                        _ => log_type(*level),
                    };
                    state.logs.write(ty, message.clone(), *at_ms);
                }
            }
            RunEvent::Finished {
                run_id,
                outcome,
                duration_ms,
            } => {
                let id = *run_id;
                let cancelled = self
                    .cancel
                    .get()
                    .is_some_and(|registry| registry.is_cancelled(id));
                // Started 缺席（例如回放）也给一份空身份 + 空收集器：认不出
                // 发起方 ⇒ 不发。绝不 panic。
                let state = self.lock().remove(&id).unwrap_or_else(|| RunState {
                    meta: RunMeta::default(),
                    logs: Arc::new(PushLogCollector::new()),
                });
                let run = FinishedRun {
                    run_id: id,
                    meta: state.meta,
                    outcome: outcome.clone(),
                    duration_ms: *duration_ms,
                    cancelled,
                    logs: state.logs,
                };

                let notifier = Arc::clone(&self.notifier);
                // ⭐ **「直播自动定时」的重算**（用户口径 2026-10-08：「每次直播任务跑完就
                // 重算并 upsert」✓）接在**收尾这一跳**上：
                //
                // * `Finished` 是 `runner.rs` 里**唯一**的运行终点 ✓（那边的哨兵用例
                //   还钉住它在"记录落库之后"才发 ✓）⇒ 接在这里就天然满足
                //   "一次运行只算一次" ✓，写进任务步骤里的话一趟 N 个账号会算 N 次 ✗；
                // * 它与**推送无关**：没绑规则也要重算 ✓ ⇒ 不能藏在
                //   `deliver`（没规则就提前 return ✗）里面。
                //
                // ⚠ 先把它要的两个值取出来（`run` 下面要整体移进投递那个任务里 ✓）。
                let run_task = run.meta.task.clone();
                let run_account = run.meta.account.clone();
                match tokio::runtime::Handle::try_current() {
                    // 必须 spawn：emit 是同步的，而"读规则表 / 面板绑定"与 HTTP
                    // 都是异步的；在这里等待就把这两段延迟算进了运行收尾。
                    // 解析与投递都在任务内做，任何失败都不会回到运行路径上。
                    Ok(handle) => {
                        // ⚠ 计数**必须在 spawn 之前**加上（见 `begin_delivery` ✓）；
                        // 守卫随任务一起被 drop ⇒ 投递一结束就减一 ✓（含提前 return 的路径 ✓）。
                        //
                        // ⚠ 重算也用同一个计数（`DeliveryGuard` 的说明 ✓）：
                        // CLI 的 `flush()` 因此会把它一起等完 ✓ —— 短命进程退出前
                        // 那条定时必须已经落盘 ✓。
                        let guard = notifier.begin_delivery();
                        crate::live_auto::spawn_after_run(
                            &notifier,
                            run_task,
                            run_account,
                            handle.clone(),
                            guard,
                        );
                        let guard = notifier.begin_delivery();
                        handle.spawn(async move {
                            let _guard = guard;
                            notifier.deliver(run).await;
                        });
                    }
                    // 没有运行时（例如脱离 tokio 直接调 sink）：只记一条日志，
                    // 绝不 panic。
                    Err(_) => {
                        tracing::warn!(run = id.get(), "没有 tokio 运行时，推送跳过");
                    }
                }
            }
            _ => {}
        }

        self.inner.emit(event);
    }
}

/// 用**调用方自己造好的** [`Notifier`] 去装饰 sink。
///
/// # 为什么需要它（2026-10-07，CLI 兼容那件事）
///
/// CLI 收尾要 `await notifier.flush()` ✓（短命进程 ✗，不能把在飞的推送一起掐掉 ✗）。
/// 而"在函数内部造 Notifier"的写法让调用方拿不到那个句柄 ✗ ⇒ 这条入口让 builder
/// 自己造、自己留一份（挂到 `AppHandle.pushes` ✓），再把同一份交给 sink ✓ ——
/// 两边**是同一个计数器** ✓，flush 才有意义 ✓。
pub(crate) fn decorate_with(
    inner: Arc<dyn EventSink>,
    notifier: Arc<Notifier>,
    cancel: CancelSlot,
) -> Arc<dyn EventSink> {
    Arc::new(NotifySink::new(inner, notifier, cancel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{
        Account, AccountId, AccountSettings, AccountStore, AuthInfo, DeviceId, HttpResponse,
        LogKind, Pipeline, PipelineStep, PipelineStore, Result as CoreResult, RunStatus, RunStore,
        Schedule, ScheduleStore, ThrottleMode, UserStore,
    };
    use mcloud_store::JsonStore;
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicI64;

    const NOW_MS: i64 = 1_700_000_000_000;
    /// 测试里那个"登录用户"（规则 / 面板绑定 / 节流都按 owner 分表 ✓）。
    const OWNER: &str = "u-test";

    fn owner() -> UserId {
        UserId::new(OWNER)
    }

    /// 临时数据目录（与 `mcloud-store` 的测试同一个手法：pid + 纳秒避免互相抢 ✓）。
    fn tmpdir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "mcloud-notify-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// 用**真 `JsonStore`**（不再造 `Store` 替身 ✓）。
    ///
    /// # 为什么这里改用真实现（2026-10-08）
    ///
    /// 投递现在要读四样东西（规则表 / 发起方 / 面板绑定 / 节流状态 ✓），
    /// 而它们全在 `Store` 上 —— 写一个替身要补 40 来个方法 ✗，本仓没有那样的替身 ✓。
    /// `mcloud-store` 本来就是本 crate 的依赖 ✓ ⇒ 用真实现顺带把"落盘 + 读回"测了 ✓
    /// （"重启不能忘"那一条正是靠它才测得出 ✓）。
    fn store_in(dir: &PathBuf) -> Arc<JsonStore> {
        Arc::new(JsonStore::open(dir).expect("临时目录应当能打开"))
    }

    /// 造一条**自带凭据**的规则（用户口径：「这个规则配置推送的 token」✓）。
    async fn seed_rule(store: &JsonStore, id: &str, config_json: &str) {
        let mut rule = PushRule::new_default(id, Some(owner()));
        rule.config = serde_json::from_str(config_json).expect("测试配置必须能解析");
        store
            .save_push_rule(&owner(), &rule)
            .await
            .expect("存规则应当成功");
    }

    /// 造一条定时，绑 `rules` 条规则 + `events` 结局过滤。
    async fn seed_schedule(store: &JsonStore, id: &str, rules: &[&str], events: Vec<PushEvent>) {
        let sch = Schedule {
            id: ScheduleId::new(id),
            name: "测试".into(),
            task: "sign".into(),
            accounts: vec![],
            expr: "0 8 * * *".into(),
            timezone: "Asia/Shanghai".into(),
            body: serde_json::Value::Null,
            enabled: true,
            // 手建的（推送那几条用例只关心"这条定时绑了哪些规则"✓）。
            managed_by: None,
            managed_wanted: None,
            owner: Some(owner()),
            push_rules: rules.iter().map(|r| PushRuleId::new(*r)).collect(),
            push_events: events,
            last_fire_ms: None,
            last_run_ms: None,
            last_status: None,
            last_error: None,
            last_note: None,
            run_count: 0,
        };
        store.save_schedule(&sch).await.expect("存定时应当成功");
    }

    /// 造一条编排（手动跑编排那条路的对照物 ✓）。
    async fn seed_pipeline(store: &JsonStore, id: &str, rules: &[&str]) {
        let p = Pipeline {
            id: PipelineId::new(id),
            name: "测试链".into(),
            accounts: vec![],
            account_mode: mcloud_core::AccountMode::Sequential,
            stop_on_error: false,
            steps: vec![PipelineStep {
                task: "sign".into(),
                body: serde_json::Value::Null,
                continue_on_error: true,
            }],
            enabled: true,
            owner: Some(owner()),
            push_rules: rules.iter().map(|r| PushRuleId::new(*r)).collect(),
            push_events: Vec::new(),
            last_run_ms: None,
        };
        store.save_pipeline(&p).await.expect("存编排应当成功");
    }

    /// 给某个面板行为绑规则 + 节流（走**真落盘** ✓，与设置页那条路一致 ✓）。
    async fn bind_panel(
        store: &JsonStore,
        behavior: BehaviorKey,
        rules: &[&str],
        throttle: mcloud_core::Throttle,
    ) {
        let mut panel = store.push_panel(&owner()).await.unwrap();
        let slot = panel.get_mut(behavior);
        slot.rules = rules.iter().map(|r| PushRuleId::new(*r)).collect();
        slot.throttle = throttle;
        store
            .save_push_panel(&owner(), &panel)
            .await
            .expect("存面板绑定应当成功");
    }

    /// 假传输排队的一条应答。
    #[derive(Debug, Clone, Copy)]
    enum Reply {
        Http(u16),
        Fail,
    }

    /// 记录每一枪、并按队列给应答的假 HttpClient。
    struct FakeHttp {
        replies: Mutex<VecDeque<Reply>>,
        seen: Mutex<Vec<HttpRequest>>,
        calls: AtomicUsize,
    }

    impl FakeHttp {
        fn new() -> Arc<Self> {
            Self::with(Vec::new())
        }

        fn with(replies: Vec<Reply>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }

        fn urls(&self) -> Vec<String> {
            self.requests().into_iter().map(|r| r.url).collect()
        }

        /// 第 index 枪的体，解析成 JSON。
        fn body_at(&self, index: usize) -> Value {
            let raw = self.requests()[index]
                .body_str()
                .expect("体应是 UTF-8")
                .to_string();
            serde_json::from_str(&raw).expect("体应是合法 JSON")
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for FakeHttp {
        async fn request(&self, req: HttpRequest) -> CoreResult<HttpResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(req);
            match self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Reply::Http(200))
            {
                Reply::Http(status) => Ok(HttpResponse {
                    status,
                    body: "{}".into(),
                    headers: Vec::new(),
                }),
                Reply::Fail => Err(mcloud_core::Error::Other("网络断了".into())),
            }
        }
    }

    /// ⭐「**谁来都打挂**」的假 HTTP：证明"没绑规则 ⇒ 0 个请求" ✓（§6 验收第 3 条）。
    ///
    /// # 为什么用 panic 而不是计数
    ///
    /// 计数只能证明"没发出去" ✓；而 panic 连"有没有**走到**发送那一步"都证明了 ✓
    /// —— 后者才是这一条要钉的东西（"没绑规则"必须在**碰 HTTP 之前**就返回 ✓）。
    struct PanicHttp;

    #[async_trait::async_trait]
    impl HttpClient for PanicHttp {
        async fn request(&self, req: HttpRequest) -> CoreResult<HttpResponse> {
            panic!(
                "没绑规则 / 手动跑 ⇒ 一个包都不该发出来，却发了：{}",
                req.url
            );
        }
    }

    /// 会走的假时钟（冷却那几条要"过 30 分钟"✓，而 `FixedClock` 不走 ✓）。
    struct StepClock(AtomicI64);

    impl Clock for StepClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn step_clock() -> Arc<StepClock> {
        Arc::new(StepClock(AtomicI64::new(NOW_MS)))
    }

    fn advance(clock: &StepClock, ms: i64) {
        clock.0.store(NOW_MS + ms, Ordering::SeqCst);
    }

    fn fixed_clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock(NOW_MS))
    }

    /// 只收集事件的内层 sink。
    #[derive(Default)]
    struct CollectSink {
        events: Mutex<Vec<RunEvent>>,
    }

    impl CollectSink {
        fn len(&self) -> usize {
            self.events.lock().unwrap().len()
        }

        fn last(&self) -> Option<RunEvent> {
            self.events.lock().unwrap().last().cloned()
        }
    }

    impl EventSink for CollectSink {
        fn emit(&self, event: RunEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn started_from(run_id: u64, origin: Option<&str>) -> RunEvent {
        RunEvent::Started {
            run_id: RunId::new(run_id),
            task: "sign".into(),
            account: "19900000001".into(),
            at_ms: NOW_MS,
            origin: origin.map(str::to_string),
        }
    }

    fn logged(run_id: u64, level: LogLevel, message: &str) -> RunEvent {
        // ⚠ 分类与级别是**两件事**（`kind` 管徽章/推送 type ✓、`level` 管筛选 ✓），
        // 但生产里 `ctx.info/warn/error` 两格本来就同名 ⇒ 这里也按级别给出同名分类 ✓。
        let kind = match level {
            LogLevel::Debug => mcloud_core::LogKind::Debug,
            LogLevel::Info => mcloud_core::LogKind::Info,
            LogLevel::Warn => mcloud_core::LogKind::Warn,
            LogLevel::Error => mcloud_core::LogKind::Error,
        };
        RunEvent::Log {
            run_id: RunId::new(run_id),
            level,
            kind,
            message: message.into(),
            at_ms: NOW_MS,
        }
    }

    fn finished(run_id: u64, outcome: TaskOutcome, duration_ms: u64) -> RunEvent {
        RunEvent::Finished {
            run_id: RunId::new(run_id),
            outcome,
            duration_ms,
        }
    }

    /// 造一个投递器（默认固定时钟 ✓）。
    fn notifier(http: Arc<dyn HttpClient>, store: Arc<JsonStore>) -> Arc<Notifier> {
        Arc::new(Notifier::new(http, fixed_clock()).with_store(Some(store as Arc<dyn Store>)))
    }

    /// 完整可控的旁路 sink。
    fn sink_for(
        http: Arc<dyn HttpClient>,
        store: Arc<JsonStore>,
        cancel: Option<Arc<CancelRegistry>>,
    ) -> (Arc<dyn EventSink>, Arc<CollectSink>) {
        let inner = Arc::new(CollectSink::default());
        let slot = cancel_slot();
        if let Some(registry) = cancel {
            slot.set(registry).expect("空槽位应当设置成功");
        }
        let sink: Arc<dyn EventSink> = decorate_with(
            Arc::clone(&inner) as Arc<dyn EventSink>,
            notifier(http, store),
            slot,
        );
        (sink, inner)
    }

    /// 等 spawn 出去的投递跑完。
    async fn wait_calls(http: &FakeHttp, n: usize) {
        for _ in 0..1000 {
            if http.calls() >= n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("等待投递完成超时（期望 {n} 枪）");
    }

    /// 给 spawn 出去的任务一点时间，然后断言**一个包都没有**。
    async fn expect_no_calls(http: &FakeHttp) {
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(http.calls(), 0, "不该有任何投递");
    }

    /// 跑完一次运行（Started + Finished ✓）。
    fn run_once(
        sink: &Arc<dyn EventSink>,
        run_id: u64,
        origin: Option<&str>,
        outcome: TaskOutcome,
    ) {
        sink.emit(started_from(run_id, origin));
        sink.emit(finished(run_id, outcome, 10));
    }

    // ── (1) 规则自带凭据：换一条只影响它自己 ────────────────────────────

    /// ⭐ §6 验收第 1 条：**规则自带凭据** ⇒ 换一条规则的 token 只影响它自己 ✓。
    #[tokio::test]
    async fn each_rule_sends_with_its_own_credentials() {
        let d = tmpdir("own-creds");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"TOKEN-1"}}"#).await;
        seed_rule(&store, "r2", r#"{"serverChan":{"token":"TOKEN-2"}}"#).await;
        seed_schedule(&store, "s1", &["r1", "r2"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::failed("boom"));
        wait_calls(&http, 2).await;
        let urls = http.urls();
        assert!(
            urls.iter().any(|u| u.contains("TOKEN-1")),
            "r1 用自己的 token：{urls:?}"
        );
        assert!(
            urls.iter().any(|u| u.contains("TOKEN-2")),
            "r2 用自己的 token：{urls:?}"
        );

        // 改 r1 的 token ⇒ **只有它自己变** ✓（§6 第 1 条的另一半 ✓）
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"TOKEN-1-NEW"}}"#).await;
        let http2 = FakeHttp::new();
        let (sink2, _) = sink_for(
            Arc::clone(&http2) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink2, 2, Some("schedule:s1"), TaskOutcome::failed("boom"));
        wait_calls(&http2, 2).await;
        let urls2 = http2.urls();
        assert!(
            urls2.iter().any(|u| u.contains("TOKEN-1-NEW.send")),
            "r1 换成了新 token：{urls2:?}"
        );
        assert!(
            !urls2.iter().any(|u| u.contains("TOKEN-1.send")),
            "旧 token 不该再出现：{urls2:?}"
        );
        assert!(
            urls2.iter().any(|u| u.contains("TOKEN-2.send")),
            "r2 一个字都不该变：{urls2:?}"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    // ── (2) 一个行为绑多条规则 ⇒ 各发一次 ──────────────────────────────

    /// ⭐ §6 验收第 2 条：定时绑**两条**规则 ⇒ 一次运行**各发一次** ✓，
    /// 而且各自用自己的平台/token ✓。
    #[tokio::test]
    async fn a_schedule_bound_to_two_rules_fires_both_once() {
        let d = tmpdir("two-rules");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_rule(
            &store,
            "r2",
            r#"{"workWeixinBot":{"url":"https://qyapi.test/hook?key=K"}}"#,
        )
        .await;
        seed_schedule(&store, "s1", &["r1", "r2"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, inner) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::failed("boom"));
        wait_calls(&http, 2).await;

        assert_eq!(http.calls(), 2, "两条规则各一枪");
        let urls = http.urls();
        assert!(urls[0].contains("sctapi.ftqq.com/SC.send"), "{urls:?}");
        assert!(urls[1].contains("qyapi.test"), "{urls:?}");
        assert_eq!(inner.len(), 2, "事件要原样往下走");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 编排也是"行为"（手动跑编排时同样按它绑的规则推 ✓）。
    #[tokio::test]
    async fn a_pipeline_bound_to_a_rule_fires_it() {
        let d = tmpdir("pipeline-origin");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_pipeline(&store, "p1", &["r1"]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("pipeline:p1"), TaskOutcome::success("好了"));
        wait_calls(&http, 1).await;
        assert_eq!(http.calls(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ── (3) 没绑规则 ⇒ 0 个请求（打挂的 HTTP 证明）───────────────────────

    /// ⭐ §6 验收第 3 条：**没绑规则 ⇒ 0 个请求** ✓（假客户端"谁来都打挂" ✓）。
    ///
    /// 三种"没绑"都钉住：
    /// * 定时绑了个空数组 ✓；
    /// * **手动跑**（`origin: None` ✓）；
    /// * 绑了一个**不存在的**定时 id ✓（认不出 ⇒ 不推，而不是猜 ✓）。
    #[tokio::test]
    async fn no_bound_rule_sends_nothing() {
        let d = tmpdir("no-rules");
        let store = store_in(&d);
        // 规则**存在**，但没人绑它 ✓ —— 这一条正是"规则不再反向持有绑定"的形态 ✓。
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &[], vec![]).await;

        for origin in [Some("schedule:s1"), None, Some("schedule:不存在")] {
            let (sink, inner) = sink_for(Arc::new(PanicHttp), Arc::clone(&store), None);
            run_once(&sink, 1, origin, TaskOutcome::failed("boom"));
            for _ in 0..200 {
                tokio::task::yield_now().await;
            }
            assert_eq!(inner.len(), 2, "事件仍要原样往下走");
            assert!(matches!(inner.last(), Some(RunEvent::Finished { .. })));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 没接存储（只有测试会这样 ✓）⇒ 一个包都不发、不 panic ✓。
    #[tokio::test]
    async fn no_store_delivers_nothing() {
        let inner = Arc::new(CollectSink::default());
        let notifier = Arc::new(Notifier::new(Arc::new(PanicHttp), fixed_clock()));
        let sink = decorate_with(
            Arc::clone(&inner) as Arc<dyn EventSink>,
            notifier,
            cancel_slot(),
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::failed("boom"));
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(inner.len(), 2);
    }

    // ── (4) push_events 结局过滤 ────────────────────────────────────────

    /// ⭐ §6 验收第 4 条：`push_events` 真的生效（只写 `runFailed` ⇒ 成功那次不发 ✓）。
    #[tokio::test]
    async fn push_events_filters_run_outcomes() {
        let d = tmpdir("push-events");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![PushEvent::RunFailed]).await;

        // 成功那次 ⇒ 0 枪 ✓
        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::success("好了"));
        expect_no_calls(&http).await;

        // 失败那次 ⇒ 1 枪 ✓
        let http2 = FakeHttp::new();
        let (sink2, _) = sink_for(
            Arc::clone(&http2) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink2, 2, Some("schedule:s1"), TaskOutcome::failed("boom"));
        wait_calls(&http2, 1).await;
        assert_eq!(http2.calls(), 1);

        // 空 events = **跑完就推** ✓（同一份绑定、同一份规则 ✓）
        seed_schedule(&store, "s2", &["r1"], vec![]).await;
        let http3 = FakeHttp::new();
        let (sink3, _) = sink_for(
            Arc::clone(&http3) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink3, 3, Some("schedule:s2"), TaskOutcome::success("好了"));
        wait_calls(&http3, 1).await;
        assert_eq!(http3.calls(), 1, "空 push_events 跑完就推");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 主动中止是一个**独立事件**（`runCancelled`）✓：只写 `runFailed` 时它不发 ✓。
    #[tokio::test]
    async fn cancelled_is_its_own_event() {
        let d = tmpdir("cancelled-event");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![PushEvent::RunFailed]).await;

        let registry = Arc::new(CancelRegistry::new());
        registry.register(RunId::new(42)).cancel();
        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            Some(Arc::clone(&registry)),
        );
        run_once(
            &sink,
            42,
            Some("schedule:s1"),
            TaskOutcome::failed("已中止"),
        );
        expect_no_calls(&http).await;

        // 点名 `runCancelled` ⇒ 发 ✓，而且结尾摘要是 warn（不许报红 ✓）
        seed_schedule(&store, "s2", &["r1"], vec![PushEvent::RunCancelled]).await;
        let http2 = FakeHttp::new();
        let (sink2, _) = sink_for(
            Arc::clone(&http2) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            Some(Arc::clone(&registry)),
        );
        run_once(
            &sink2,
            42,
            Some("schedule:s2"),
            TaskOutcome::failed("已中止"),
        );
        wait_calls(&http2, 1).await;
        let desp = http2.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(desp.contains("已中止"), "{desp}");
        assert!(!desp.contains("[error"), "用户主动中止不许报红：{desp}");

        let _ = std::fs::remove_dir_all(&d);
    }

    // ── (5) 面板行为的节流 ──────────────────────────────────────────────

    fn notice(behavior: BehaviorKey, status: BehaviorStatus) -> PanelNotice {
        PanelNotice {
            behavior,
            status,
            title: "mcloud 性能告警".into(),
            text: "任务慢了".into(),
        }
    }

    /// ⭐ §6 验收第 5 条的前半：`OnChange` 在"持续异常"期间**只发一条**、
    /// **恢复时必发** ✓（哪怕还在冷却期内 ✓）。
    #[tokio::test]
    async fn panel_on_change_sends_once_and_always_on_recovery() {
        let d = tmpdir("panel-onchange");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        bind_panel(
            &store,
            BehaviorKey::PerfSlow,
            &["r1"],
            mcloud_core::Throttle {
                mode: ThrottleMode::OnChange,
                cooldown_secs: 1800,
                daily_cap: 10,
            },
        )
        .await;

        let http = FakeHttp::new();
        let n = notifier(Arc::clone(&http) as Arc<dyn HttpClient>, Arc::clone(&store));

        // 第一次进入异常 ⇒ 发 ✓
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::PerfSlow, BehaviorStatus::Abnormal)
            )
            .await,
            PanelOutcome::Sent { rules: 1 }
        );
        wait_calls(&http, 1).await;

        // 持续异常 ⇒ 拦，原因就是"状态没变化" ✓（**不是**冷却 —— 冷却那条另有测试 ✓）
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::PerfSlow, BehaviorStatus::Abnormal)
            )
            .await,
            PanelOutcome::Suppressed {
                reason: ThrottleReason::Unchanged,
                throttled_today: 1
            }
        );

        // ⭐ 恢复正常 ⇒ **必发** ✓（此刻距上次只隔了 0ms，冷却期内 ✓）
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::PerfSlow, BehaviorStatus::Normal)
            )
            .await,
            PanelOutcome::Sent { rules: 1 }
        );
        wait_calls(&http, 2).await;
        assert_eq!(http.calls(), 2, "异常一条 + 恢复一条");

        // 留痕**落盘**了（重启也还在 ✓）
        let st = store
            .push_throttle(&owner())
            .await
            .unwrap()
            .get(BehaviorKey::PerfSlow);
        assert_eq!(st.throttled_today, 1, "今天已节流 1 次");
        assert_eq!(st.sent_today, 2);
        assert_eq!(st.last_status, Some(BehaviorStatus::Normal));

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ §6 验收第 5 条的中段：**冷却期内第二次被拦** ✓（`every` 那档 ✓）。
    #[tokio::test]
    async fn panel_cooldown_blocks_the_second_trigger() {
        let d = tmpdir("panel-cooldown");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        bind_panel(
            &store,
            BehaviorKey::LoginFailed,
            &["r1"],
            mcloud_core::Throttle {
                mode: ThrottleMode::Every,
                cooldown_secs: 1800,
                daily_cap: 10,
            },
        )
        .await;

        let http = FakeHttp::new();
        let clock = step_clock();
        let n = Arc::new(
            Notifier::new(
                Arc::clone(&http) as Arc<dyn HttpClient>,
                Arc::clone(&clock) as Arc<dyn Clock>,
            )
            .with_store(Some(Arc::clone(&store) as Arc<dyn Store>)),
        );

        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::LoginFailed, BehaviorStatus::Abnormal)
            )
            .await,
            PanelOutcome::Sent { rules: 1 }
        );
        wait_calls(&http, 1).await;

        // 60 秒后又来一条 ⇒ 冷却期内被拦 ✓
        advance(&clock, 60_000);
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::LoginFailed, BehaviorStatus::Abnormal)
            )
            .await,
            PanelOutcome::Suppressed {
                reason: ThrottleReason::Cooldown,
                throttled_today: 1
            }
        );
        assert_eq!(http.calls(), 1, "被拦下不该发出去");

        // 过了 1800 秒 ⇒ 照发 ✓
        advance(&clock, 1_800_000);
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::LoginFailed, BehaviorStatus::Abnormal)
            )
            .await,
            PanelOutcome::Sent { rules: 1 }
        );
        wait_calls(&http, 2).await;
        assert_eq!(http.calls(), 2);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ §6 验收第 5 条的后段：**每日上限到了被拦** ✓，而且拦下时**有留痕** ✓。
    #[tokio::test]
    async fn panel_daily_cap_blocks_and_leaves_a_trace() {
        let d = tmpdir("panel-cap");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        bind_panel(
            &store,
            BehaviorKey::LoginNew,
            &["r1"],
            mcloud_core::Throttle {
                mode: ThrottleMode::Every,
                cooldown_secs: 0,
                daily_cap: 2,
            },
        )
        .await;

        let http = FakeHttp::new();
        let n = notifier(Arc::clone(&http) as Arc<dyn HttpClient>, Arc::clone(&store));

        for i in 0..2 {
            assert_eq!(
                n.deliver_panel(
                    &owner(),
                    notice(BehaviorKey::LoginNew, BehaviorStatus::Abnormal)
                )
                .await,
                PanelOutcome::Sent { rules: 1 },
                "第 {i} 条在上限内"
            );
        }
        wait_calls(&http, 2).await;

        // 第三条 ⇒ 上限拦下 ✓，而且**留痕**（返回值 + 落盘计数两处都要有 ✓）
        let outcome = n
            .deliver_panel(
                &owner(),
                notice(BehaviorKey::LoginNew, BehaviorStatus::Abnormal),
            )
            .await;
        assert_eq!(
            outcome,
            PanelOutcome::Suppressed {
                reason: ThrottleReason::DailyCap,
                throttled_today: 1
            },
            "到了上限必须拦，而且要说得出原因（不许静默）"
        );
        assert_eq!(http.calls(), 2, "被拦下不该发出去");

        let st = store
            .push_throttle(&owner())
            .await
            .unwrap()
            .get(BehaviorKey::LoginNew);
        assert_eq!(st.sent_today, 2);
        assert_eq!(st.throttled_today, 1, "「今天已节流 1 次」要落盘");

        // ⭐ 恢复正常**不受上限影响** ✓（上限拦的是"持续异常"的重复告警 ✓）
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::LoginNew, BehaviorStatus::Normal)
            )
            .await,
            PanelOutcome::Sent { rules: 1 }
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ **`daily_cap = 0` = 不限**，端到端也照发 ✓（用户可见的承诺 ✓）。
    ///
    /// ⚠ 2026-10-08 真机验证挖出来的真 bug：旧语义把 `0` 当成"一条都不发" ✗ ——
    /// 界面/CLI 都写着「0 = 不限」✓，用户照它设了 `0`，结果**一条都收不到** ✗。
    /// 这条从 `deliver_panel` 走一遍，钉住"0 真的发得出去"（不是只在纯函数里对 ✓）。
    #[tokio::test]
    async fn panel_daily_cap_zero_means_unlimited() {
        let d = tmpdir("panel-cap-zero");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        bind_panel(
            &store,
            BehaviorKey::LoginNew,
            &["r1"],
            mcloud_core::Throttle {
                mode: ThrottleMode::Every,
                cooldown_secs: 0,
                daily_cap: 0,
            },
        )
        .await;

        let http = FakeHttp::new();
        let n = notifier(Arc::clone(&http) as Arc<dyn HttpClient>, Arc::clone(&store));

        // 12 条 > 默认的 10 条上限 —— `0` = 不限 ⇒ 一条都不该被拦 ✓
        for i in 0..12 {
            assert_eq!(
                n.deliver_panel(
                    &owner(),
                    notice(BehaviorKey::LoginNew, BehaviorStatus::Abnormal)
                )
                .await,
                PanelOutcome::Sent { rules: 1 },
                "第 {i} 条在「不限」下必须发出去"
            );
        }
        wait_calls(&http, 12).await;
        assert_eq!(http.calls(), 12, "0 = 不限 ⇒ 12 条全部发出");

        let st = store
            .push_throttle(&owner())
            .await
            .unwrap()
            .get(BehaviorKey::LoginNew);
        assert_eq!(st.sent_today, 12);
        assert_eq!(st.throttled_today, 0);

        let _ = std::fs::remove_dir_all(&d);
    }

    /// 面板行为**没绑规则** ⇒ 0 个请求 ✓（连节流状态都不该动 ✓）。
    #[tokio::test]
    async fn panel_without_rules_sends_nothing() {
        let d = tmpdir("panel-no-rules");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        // 只绑给 perfSlow；loginNew 保持默认（空 ✓）
        bind_panel(
            &store,
            BehaviorKey::PerfSlow,
            &["r1"],
            BehaviorKey::PerfSlow.default_throttle(),
        )
        .await;

        let n = notifier(Arc::new(PanicHttp), Arc::clone(&store));
        assert_eq!(
            n.deliver_panel(
                &owner(),
                notice(BehaviorKey::LoginNew, BehaviorStatus::Abnormal)
            )
            .await,
            PanelOutcome::Nothing
        );
        assert_eq!(
            store
                .push_throttle(&owner())
                .await
                .unwrap()
                .get(BehaviorKey::LoginNew)
                .sent_today,
            0,
            "没绑规则连计数都不该动"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 面板行为绑的规则**被停用** ⇒ 那条不发（其余照发 ✓）。
    //
    // ⚠ 这里原先还绑了一条"全天静默"的 r3（用来判静默时段跳过）—— 随静默时段整块
    // 删掉 ✗（2026-10-08 用户口径「不要静默时间」✓）；"停用"那一支照旧钉住 ✓。
    #[tokio::test]
    async fn panel_skips_disabled_rules() {
        let d = tmpdir("panel-skip");
        let store = store_in(&d);
        // r1 正常 ✓、r2 停用 ✗
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        {
            let mut off = PushRule::new_default("r2", Some(owner()));
            off.enabled = false;
            off.config = serde_json::from_str(r#"{"serverChan":{"token":"OFF"}}"#).unwrap();
            store.save_push_rule(&owner(), &off).await.unwrap();
        }
        bind_panel(
            &store,
            BehaviorKey::PerfFailStreak,
            &["r1", "r2"],
            BehaviorKey::PerfFailStreak.default_throttle(),
        )
        .await;

        let http = FakeHttp::new();
        let n = notifier(Arc::clone(&http) as Arc<dyn HttpClient>, Arc::clone(&store));
        let outcome = n
            .deliver_panel(
                &owner(),
                notice(BehaviorKey::PerfFailStreak, BehaviorStatus::Abnormal),
            )
            .await;
        wait_calls(&http, 1).await;
        assert_eq!(outcome, PanelOutcome::Sent { rules: 1 }, "只有 r1 该发");
        assert!(http.urls()[0].contains("SC.send"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ 阈值是**行为上的可配项**（用户原话：「耗时异常 …… 选规则 ＋ **倍数**」✓）：
    /// 绑定里写 `slowFactor: 2.0` ⇒ 用 2.0 判；不写 ⇒ 兜底 3.0 ✓。
    #[tokio::test]
    async fn perf_slow_factor_comes_from_the_binding() {
        let d = tmpdir("panel-slow-factor");
        let store = store_in(&d);
        // 历史 5 条各 100ms ⇒ 中位数 100ms ✓
        for i in 0..5 {
            let mut r = run("sign", 100, true);
            r.run_id = RunId::new(300 + i);
            store.upsert_run(&r).await.unwrap();
        }
        let n = notifier(Arc::new(PanicHttp), Arc::clone(&store));

        // 250ms = 2.5×：**3 倍以内、2 倍以上**（专门挑这一档，两个阈值分得开 ✓）
        // ① 不写阈值 ⇒ 兜底 3.0 ⇒ 不报 ✓
        assert!(
            n.perf_alert(&owner(), "sign", "19900000001", 250)
                .await
                .is_none(),
            "没写阈值 ⇒ 跟随内置默认 3.0 ⇒ 2.5 倍不算异常"
        );

        // ② 显式写 slowFactor = 2.0 ⇒ **同一组样本**要报 ✓
        let mut panel = store.push_panel(&owner()).await.unwrap();
        panel.perf_slow.slow_factor = Some(2.0);
        store.save_push_panel(&owner(), &panel).await.unwrap();
        let hit = n
            .perf_alert(&owner(), "sign", "19900000001", 250)
            .await
            .expect("阈值 2.0 ⇒ 2.5 倍该报");
        assert_eq!(hit.0, BehaviorKey::PerfSlow);
        assert!(
            hit.1.contains("阈值 2.0×"),
            "正文要带用户设的阈值：{}",
            hit.1
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ 「连续失败 …… 选规则 ＋ **条数**」✓：写 `failStreak: 2` ⇒ 第 2 次就报；
    /// 不写 ⇒ 兜底 3 ⇒ 第 2 次不报、第 3 次才报 ✓。
    #[tokio::test]
    async fn perf_fail_streak_threshold_comes_from_the_binding() {
        let d = tmpdir("panel-fail-streak");
        let store = store_in(&d);
        let n = notifier(Arc::new(PanicHttp), Arc::clone(&store));

        // 连败 2 次 ✓
        for i in 0..2 {
            let mut r = run("sign", 10, false);
            r.run_id = RunId::new(400 + i);
            store.upsert_run(&r).await.unwrap();
        }

        // ① 不写阈值 ⇒ 兜底 3 ⇒ 第 2 次不报 ✓
        assert!(
            n.perf_alert(&owner(), "sign", "19900000001", 10)
                .await
                .is_none(),
            "没写阈值 ⇒ 跟随内置默认 3 ⇒ 连败 2 次还不报"
        );

        // ② 显式写 failStreak = 2 ⇒ 同一份历史、第 2 次就报 ✓
        let mut panel = store.push_panel(&owner()).await.unwrap();
        panel.perf_fail_streak.fail_streak = Some(2);
        store.save_push_panel(&owner(), &panel).await.unwrap();
        let hit = n
            .perf_alert(&owner(), "sign", "19900000001", 10)
            .await
            .expect("阈值 2 ⇒ 连败 2 次该报");
        assert_eq!(hit.0, BehaviorKey::PerfFailStreak);
        assert!(
            hit.1.contains("阈值 2）"),
            "正文要带用户设的阈值：{}",
            hit.1
        );

        // ③ 阈值清回"没动过"（`None`）⇒ 又变回第 3 次才报 ✓
        let mut panel = store.push_panel(&owner()).await.unwrap();
        panel.perf_fail_streak.fail_streak = None;
        store.save_push_panel(&owner(), &panel).await.unwrap();
        assert!(
            n.perf_alert(&owner(), "sign", "19900000001", 10)
                .await
                .is_none(),
            "清回默认 ⇒ 连败 2 次不报"
        );
        let mut third = run("sign", 10, false);
        third.run_id = RunId::new(402);
        store.upsert_run(&third).await.unwrap();
        assert_eq!(
            n.perf_alert(&owner(), "sign", "19900000001", 10)
                .await
                .expect("连败 3 次该报")
                .0,
            BehaviorKey::PerfFailStreak
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    // ── (7) 触发线：性能面板行为接在**运行收尾**上（2026-10-08 ✓）──────────

    /// 给某个账号造一条**归 `owner` 所有**的记录（性能那条触发线要
    /// `account → account.owner` ✓，见 `Notifier::perf_check` ✓）。
    ///
    /// `owner` 传 `None` 用来钉"归属不明 ⇒ **不发**"那一条 ✓。
    async fn seed_account(store: &JsonStore, id: &str, owner: Option<UserId>) {
        store
            .save_account(&Account {
                id: AccountId::new(id),
                nickname: None,
                auth: AuthInfo {
                    raw: "raw".into(),
                    platform: "android".into(),
                    phone: id.into(),
                    token: "t".into(),
                    expire: 0,
                },
                device_id: DeviceId("dev".into()),
                pc_platform: None,
                user_domain_id: None,
                owner,
                enabled: true,
                settings: AccountSettings::default(),
            })
            .await
            .expect("存账号应当成功");
    }

    /// **从"运行收尾"这条链走一遍**：先按 `runner.rs` 的顺序把这次运行落库 ✓、
    /// 再发 `Started` / `Finished` ✓（§6 验收明确要求"不是只调纯函数"✓）。
    ///
    /// ⚠ "先落库、再 emit"这个顺序**本身**由 `mcloud-engine` 的
    /// `finished_is_emitted_after_the_run_is_persisted` 钉住 ✓ —— 这里照它做，
    /// 不是自己发明一个更宽松的前提 ✓（否则这条测试就在骗人 ✓）。
    async fn run_through_finish(
        store: &JsonStore,
        sink: &Arc<dyn EventSink>,
        run_id: u64,
        task: &str,
        account: &str,
        duration_ms: u64,
        ok: bool,
    ) {
        let mut record = run(task, duration_ms, ok);
        record.run_id = RunId::new(run_id);
        record.account = account.into();
        store.upsert_run(&record).await.expect("落库应当成功");

        sink.emit(RunEvent::Started {
            run_id: RunId::new(run_id),
            task: task.into(),
            account: account.into(),
            at_ms: NOW_MS,
            origin: None,
        });
        sink.emit(finished(
            run_id,
            if ok {
                TaskOutcome::success("ok")
            } else {
                TaskOutcome::failed("boom")
            },
            duration_ms,
        ));
    }

    /// ⭐ §3.7 验收：**运行收尾真的会触发重算**（不是只测纯函数 ✓）。
    ///
    /// # 为什么从 `emit(Finished)` 这一跳走
    ///
    /// 「直播自动定时」的重算接在**运行收尾**上（用户口径：「每次直播任务跑完就重算
    /// 并 upsert」✓），而收尾那一跳就是 `NotifySink` 的 `Finished` 分支 ✓。
    /// 这里用**真** `decorate_with` 造出来的 sink 走一遍 ⇒
    /// `emit` ⇒ `live_auto::spawn_after_run` ⇒ 那条定时落库 ✓ ——
    /// 只调 `reschedule_live_auto` 的话，"接线到底接上没有"是测不到的 ✗。
    ///
    /// # 断言为什么用 `flush()` 而不是"再等一会儿"
    ///
    /// 重算挂在**同一个在飞计数**上（见 `DeliveryGuard` 的说明 ✓）⇒
    /// `flush()` 返回时它**必须已经落盘** ✓。"循环 yield 一千次再查"那种写法
    /// 在 CI 上会偶发变红（机器一忙就不够用 ✗），而且它掩盖的正是
    /// "计数有没有在 spawn 之前加上"这个真问题 ✓。
    #[tokio::test]
    async fn live_room_finish_reschedules_the_auto_live_schedule() {
        use mcloud_core::kouling::{KoulingPool, ProbeRoom};
        use mcloud_core::live_auto::{live_auto_schedule_id, MANAGED_BY_LIVE_AUTO};
        use mcloud_upstream::live_consts::KOULING_ANCHOR;

        let d = tmpdir("live-auto-finish");
        let store = store_in(&d);
        seed_account(&store, "19900000009", Some(owner())).await;
        // 池里放一场**未来**的预告场（池是实例级的 ✓，见 `live_auto` 模块头）。
        // 时刻取 2099 是刻意的：基准时钟 `NOW_MS` 是 2023-11-15，
        // 写一个相近的日期会让这条用例在"时钟往前走"时悄悄变成"已经过去"✗。
        let mut pool = KoulingPool::default();
        pool.normalize(NOW_MS);
        pool.note_probe(
            &[ProbeRoom {
                room_id: 147608,
                title: "预告".into(),
                status: 0,
                expect_start_time: "2099-01-01 10:00:00".into(),
                anchor_nickname: KOULING_ANCHOR.into(),
            }],
            false,
            NOW_MS,
        );
        store
            .save_kouling_pool(&pool)
            .await
            .expect("存口令池应当成功");

        let notifier = notifier(FakeHttp::new(), Arc::clone(&store));
        let sink: Arc<dyn EventSink> = decorate_with(
            Arc::new(CollectSink::default()),
            Arc::clone(&notifier),
            cancel_slot(),
        );
        let id = live_auto_schedule_id(&owner());

        // ① 先跑一个**别的**任务：它跟"下一场直播"毫无关系 ⇒ 不该建那条定时 ✓
        run_through_finish(&store, &sink, 1, "sign", "19900000009", 10, true).await;
        notifier.flush().await;
        assert!(
            store.schedule(&id).await.unwrap().is_none(),
            "非直播任务跑完不该动那条自动定时"
        );

        // ② 再跑一次**直播任务** ⇒ 收尾必须把那条定时算出来 ✓
        run_through_finish(&store, &sink, 2, "live-room", "19900000009", 10, true).await;
        notifier.flush().await;
        let s = store
            .schedule(&id)
            .await
            .unwrap()
            .expect("直播任务跑完 ⇒ 收尾必须把自动定时建出来");
        assert!(s.is_managed_by(MANAGED_BY_LIVE_AUTO), "必须带系统维护标记");
        assert_eq!(s.expr, "0 0 11 1 1 *", "2099-01-01 10:00 + 1 小时");
        assert!(s.enabled);
        assert_eq!(s.owner.as_ref(), Some(&owner()), "归属是跑的那个账号的主人");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ §6 验收第 1 条（性能）：**从运行收尾这条链**上验一次 ——
    /// 历史 5 次各 100ms、这次 250ms ⇒ 阈值 2.0 **发** ✓、默认 3.0 **不发** ✓。
    #[tokio::test]
    async fn perf_slow_fires_from_the_run_finish_chain() {
        let d = tmpdir("perf-slow-chain");
        let store = store_in(&d);
        seed_account(&store, "19900000001", Some(owner())).await;
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        // 历史 5 次各 100ms（同一个任务 + 同一个账号 ✓）
        for i in 0..5 {
            let mut r = run("sign", 100, true);
            r.run_id = RunId::new(10 + i);
            store.upsert_run(&r).await.unwrap();
        }
        bind_panel(
            &store,
            BehaviorKey::PerfSlow,
            &["r1"],
            BehaviorKey::PerfSlow.default_throttle(),
        )
        .await;

        // ① 阈值写 2.0 ⇒ 250ms = 2.5 倍 ⇒ 收尾那条链上**发一条** ✓
        let mut panel = store.push_panel(&owner()).await.unwrap();
        panel.perf_slow.slow_factor = Some(2.0);
        store.save_push_panel(&owner(), &panel).await.unwrap();

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_through_finish(&store, &sink, 100, "sign", "19900000001", 250, true).await;
        wait_calls(&http, 1).await;
        let desp = http.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(
            desp.contains("历史中位数 100ms"),
            "告警正文要带判据（中位数 / 倍数 ✓）：{desp}"
        );
        assert!(desp.contains("250ms"), "{desp}");

        // ② 阈值清回"没动过" ⇒ 兜底 3.0 ⇒ **同一组样本不发** ✓
        let mut panel = store.push_panel(&owner()).await.unwrap();
        panel.perf_slow.slow_factor = None;
        store.save_push_panel(&owner(), &panel).await.unwrap();
        let http2 = FakeHttp::new();
        let (sink2, _) = sink_for(
            Arc::clone(&http2) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_through_finish(&store, &sink2, 101, "sign", "19900000001", 250, true).await;
        expect_no_calls(&http2).await;

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ §6 验收第 2 条（连续失败）：连败 2 次 ⇒ `failStreak: 2` 时**第 2 次报** ✓；
    /// 默认 3 时第 2 次不报、第 3 次报 ✓ —— 两条都从**运行收尾**那条链上走 ✓。
    #[tokio::test]
    async fn perf_fail_streak_fires_from_the_run_finish_chain() {
        let d = tmpdir("perf-streak-chain");
        let store = store_in(&d);
        let acc1 = "19900000001";
        let acc2 = "19900000002";
        seed_account(&store, acc1, Some(owner())).await;
        seed_account(&store, acc2, Some(owner())).await;
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        bind_panel(
            &store,
            BehaviorKey::PerfFailStreak,
            &["r1"],
            BehaviorKey::PerfFailStreak.default_throttle(),
        )
        .await;

        // ① 阈值没动过 ⇒ 兜底 3：连败 2 次**不报** ✓
        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_through_finish(&store, &sink, 200, "sign", acc1, 10, false).await;
        expect_no_calls(&http).await;
        run_through_finish(&store, &sink, 201, "sign", acc1, 10, false).await;
        expect_no_calls(&http).await;

        // ② 第 3 次 ⇒ 到 3 了，**报** ✓
        run_through_finish(&store, &sink, 202, "sign", acc1, 10, false).await;
        wait_calls(&http, 1).await;
        let desp = http.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(desp.contains("连续失败 3 次"), "{desp}");

        // ③ 阈值写 2 ⇒ **第 2 次就报** ✓（换个账号，计数从头开始 ✓）
        //
        // ⚠ 节流是按 **owner + 行为** 记的（不是按账号 ✓）⇒ 换账号之前先把状态清掉，
        //    否则第二次会以 `Unchanged` 被拦 —— 那是节流那一档的事，不是本条的判据 ✓
        //    （节流本身另有 `panel_on_change_sends_once_and_always_on_recovery` ✓）。
        store
            .save_push_throttle(&owner(), &mcloud_core::ThrottleState::default())
            .await
            .unwrap();
        let mut panel = store.push_panel(&owner()).await.unwrap();
        panel.perf_fail_streak.fail_streak = Some(2);
        store.save_push_panel(&owner(), &panel).await.unwrap();

        let http2 = FakeHttp::new();
        let (sink2, _) = sink_for(
            Arc::clone(&http2) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_through_finish(&store, &sink2, 300, "sign", acc2, 10, false).await;
        expect_no_calls(&http2).await;
        run_through_finish(&store, &sink2, 301, "sign", acc2, 10, false).await;
        wait_calls(&http2, 1).await;
        let desp = http2.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(desp.contains("连续失败 2 次"), "{desp}");

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ §6 验收第 4 条：**没绑规则 / 没配面板** ⇒ 收尾那条链上**0 个请求** ✓。
    ///
    /// ⚠ 这里刻意让**判据成立**（连败 3 次 ⇒ 默认阈值就是 3 ✓）：要证的正是
    /// "判据成立、但没绑规则 ⇒ 一个包都不发" ✓ —— 判据不成立而没发，
    /// 证明不了任何事 ✗。
    #[tokio::test]
    async fn no_panel_binding_sends_nothing_from_the_run_finish_chain() {
        let d = tmpdir("panel-none-chain");
        let store = store_in(&d);
        seed_account(&store, "19900000001", Some(owner())).await;
        // 有规则，但**一条都没绑给面板行为** ✓（默认那份绑定就是"不绑任何规则"✓）
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_through_finish(&store, &sink, 400, "sign", "19900000001", 10, false).await;
        run_through_finish(&store, &sink, 401, "sign", "19900000001", 10, false).await;
        run_through_finish(&store, &sink, 402, "sign", "19900000001", 10, false).await;
        expect_no_calls(&http).await;
        assert_eq!(
            store
                .push_throttle(&owner())
                .await
                .unwrap()
                .get(BehaviorKey::PerfFailStreak)
                .sent_today,
            0,
            "没绑规则连节流计数都不该动"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⚠ 账号**没有归属**（`account.owner == None`）⇒ 性能告警**不发** ✓
    /// （用户口径：「拿不到就不发，不许猜」✓）。
    #[tokio::test]
    async fn perf_alert_needs_the_account_owner() {
        let d = tmpdir("perf-no-owner");
        let store = store_in(&d);
        seed_account(&store, "19900000001", None).await;
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        bind_panel(
            &store,
            BehaviorKey::PerfFailStreak,
            &["r1"],
            BehaviorKey::PerfFailStreak.default_throttle(),
        )
        .await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        for i in 0..3 {
            run_through_finish(&store, &sink, 500 + i, "sign", "19900000001", 10, false).await;
        }
        expect_no_calls(&http).await;

        let _ = std::fs::remove_dir_all(&d);
    }

    /// ⭐ §6 验收第 5 条：`LogKind::Fail`（**某一笔没成**）落成 type `"error"` ✓
    /// ⇒ 开着「仅错误时推」时**会推** ✓（用户 2026-10-08 口径：一笔失败就是错误 ✓）。
    ///
    /// 红测：把 `NotifySink::emit` 里那一格改回 `"warn"` ⇒ 这条**一个请求都没有** ✓
    /// （`warn` 过不了 `onlyError` 那道闸 ✓）—— 那正是要钉死的旧行为 ✗。
    #[tokio::test]
    async fn a_failed_line_is_an_error_for_only_error_rules() {
        let d = tmpdir("fail-kind");
        let store = store_in(&d);
        // 只推错误（`onlyError` ✓）；结尾摘要给 `success` ⇒ 那一条本身不该触发 ✓
        seed_rule(
            &store,
            "r1",
            r#"{"onlyError":true,"serverChan":{"token":"SC"}}"#,
        )
        .await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        sink.emit(started_from(1, Some("schedule:s1")));
        sink.emit(RunEvent::Log {
            run_id: RunId::new(1),
            // 级别是 warn、**分类**是 fail —— 判据只看分类那一格 ✓（见 `emit` ✓）
            level: LogLevel::Warn,
            kind: LogKind::Fail,
            message: "这一笔没成".into(),
            at_ms: NOW_MS,
        });
        sink.emit(finished(1, TaskOutcome::success("整趟跑完了"), 10));

        wait_calls(&http, 1).await;
        let desp = http.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(
            desp.contains("这一笔没成"),
            "「某一笔没成」必须能进只有错误才推的那条规则：{desp}"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    // ── (6) 旧形状落盘文件 / 缺文件 ⇒ 起得来 ────────────────────────────
    /// ⭐ §6 验收第 6 条：目录里躺着**旧形状**的推送文件 ⇒ 启动不报错 ✓；
    /// **缺文件**的场景也要能起来 ✓。
    #[tokio::test]
    async fn legacy_push_files_do_not_break_startup() {
        let d = tmpdir("legacy-files");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("push-rules.json"),
            r#"{"u-test":[{"id":"r1","name":"旧规则","events":["runFailed"],"channels":["serverChan"],"digest":"perRun","slowFactor":3.0,"failStreak":3,"enabled":true,"owner":"u-test"}]}"#,
        )
        .unwrap();
        std::fs::write(
            d.join("push-config.json"),
            r#"{"u-test":{"serverChan":{"token":"OLD"}}}"#,
        )
        .unwrap();

        // 起得来 ✓（`JsonStore::open` 内部先清旧形状 ✓）
        let store = store_in(&d);
        assert!(store.push_rules(&owner()).await.unwrap().is_empty());
        assert!(!d.join("push-config.json").exists());

        // 起得来之后跑一次运行：认不出/没绑 ⇒ **0 个请求**、不 panic ✓
        let (sink, inner) = sink_for(Arc::new(PanicHttp), Arc::clone(&store), None);
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::failed("boom"));
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(inner.len(), 2);

        // 缺文件（全新目录 ✓）
        let empty = tmpdir("legacy-absent");
        let s2 = store_in(&empty);
        assert!(s2.all_push_rules().await.unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&empty);
    }

    // ── 运行日志 → 正文（老断言全部保留，只是配置来自规则 ✓）──────────────

    /// ⚠ **推送是本项目最要紧的一条脱敏边界**：SSE 流与进程日志都还在本机，
    /// webhook 是**真的离开本机** ✗。而结论里的 `summary`/`reason` 是任务自己写的
    /// 自由文本 —— 上游把请求头/请求体回显进报错时，凭证就跟着一起出去 ✗。
    #[tokio::test]
    async fn webhook_body_never_carries_credentials() {
        let d = tmpdir("redact");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(
            &sink,
            1,
            Some("schedule:s1"),
            TaskOutcome::failed(
                "上游回显 password=hunter2，Authorization: Bearer deadbeefdeadbeef",
            ),
        );
        wait_calls(&http, 1).await;

        let body = http.body_at(0).to_string();
        assert!(!body.contains("hunter2"), "推送正文里带了口令：{body}");
        assert!(!body.contains("deadbeef"), "推送正文里带了 token：{body}");
        assert!(body.contains("上游回显"), "正常文案要留着：{body}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 标题用**规则自己那份**配置里的 title（不是硬编码 ✓）。
    #[tokio::test]
    async fn configured_title_is_used() {
        let d = tmpdir("title");
        let store = store_in(&d);
        seed_rule(
            &store,
            "r1",
            r#"{"title":"我的标题","serverChan":{"token":"SC"}}"#,
        )
        .await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::success("好了"));
        wait_calls(&http, 1).await;
        let body = http.body_at(0);
        assert_eq!(body["title"], "我的标题");
        assert_eq!(body["text"], "我的标题");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 结尾摘要带运行身份（旧载荷的 task/account/run/duration 语义接替者）。
    #[tokio::test]
    async fn ending_summary_carries_run_identity() {
        let d = tmpdir("summary");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 7, Some("schedule:s1"), TaskOutcome::success("好了"));
        wait_calls(&http, 1).await;
        let desp = http.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(desp.contains("sign@19900000001"), "任务/账号要在：{desp}");
        assert!(desp.contains("run#7"), "run 号要在：{desp}");
        assert!(desp.contains("10ms"), "时长要在：{desp}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 渠道凭证只该在自己那一格（URL），不该被写进正文。
    #[tokio::test]
    async fn channel_credential_is_not_in_the_body() {
        let d = tmpdir("cred-body");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC-SECRET"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::success("好了"));
        wait_calls(&http, 1).await;
        assert!(http.urls()[0].contains("SC-SECRET"), "凭证在 URL 里");
        let raw = serde_json::to_string(&http.body_at(0)).unwrap();
        assert!(!raw.contains("SC-SECRET"), "凭证不许进正文：{raw}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 一条渠道失败/抛错 ⇒ 其余渠道照常发、不影响运行结果 ✓。
    #[tokio::test]
    async fn one_channel_failure_does_not_stop_the_others() {
        for reply in [Reply::Http(500), Reply::Fail] {
            let d = tmpdir("one-fails");
            let store = store_in(&d);
            seed_rule(
                &store,
                "r1",
                r#"{"serverChan":{"token":"SC"},"workWeixinBot":{"url":"https://qyapi.test/hook"}}"#,
            )
            .await;
            seed_schedule(&store, "s1", &["r1"], vec![]).await;

            let http = FakeHttp::with(vec![reply]);
            let (sink, inner) = sink_for(
                Arc::clone(&http) as Arc<dyn HttpClient>,
                Arc::clone(&store),
                None,
            );
            run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::failed("boom"));
            wait_calls(&http, 2).await;

            assert_eq!(http.calls(), 2, "第一条失败不妨碍第二条");
            let urls = http.urls();
            assert!(urls[0].contains("sctapi.ftqq.com"));
            assert!(urls[1].contains("qyapi.test"));
            assert_eq!(inner.len(), 2, "运行结果一点没受影响");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// onlyError + minLevel：现在它们是**规则自己** `config` 上的旋钮 ✓
    /// —— 于是"这条定时只推失败、那条连成功一起推"表达得出来了 ✓。
    #[tokio::test]
    async fn only_error_gates_push_and_min_level_filters_text() {
        // 全 info（结尾 success 也是非 error）⇒ 不推 ✓
        let d = tmpdir("only-error");
        let store = store_in(&d);
        seed_rule(
            &store,
            "r1",
            r#"{"onlyError":true,"serverChan":{"token":"SC"}}"#,
        )
        .await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        sink.emit(started_from(1, Some("schedule:s1")));
        sink.emit(logged(1, LogLevel::Info, "普通日志"));
        sink.emit(finished(1, TaskOutcome::success("搞定"), 10));
        expect_no_calls(&http).await;

        // 含 error 且 minLevel=error ⇒ 发，正文只剩 error 级那几行 ✓
        let d2 = tmpdir("only-error-2");
        let store2 = store_in(&d2);
        seed_rule(
            &store2,
            "r1",
            r#"{"onlyError":true,"minLevel":"error","serverChan":{"token":"SC"}}"#,
        )
        .await;
        seed_schedule(&store2, "s1", &["r1"], vec![]).await;
        let http2 = FakeHttp::new();
        let (sink2, _) = sink_for(
            Arc::clone(&http2) as Arc<dyn HttpClient>,
            Arc::clone(&store2),
            None,
        );
        sink2.emit(started_from(1, Some("schedule:s1")));
        sink2.emit(logged(1, LogLevel::Info, "普通日志"));
        sink2.emit(logged(1, LogLevel::Error, "炸了"));
        sink2.emit(finished(1, TaskOutcome::success("搞定"), 10));
        wait_calls(&http2, 1).await;

        let desp = http2.body_at(0)["desp"].as_str().unwrap().to_string();
        assert!(desp.contains("炸了"), "error 行要在：{desp}");
        assert!(
            !desp.contains("普通日志"),
            "minLevel=error 时不该留 info 行：{desp}"
        );
        assert!(
            !desp.contains("搞定"),
            "success 摘要（level 3）也要被 error 过滤掉：{desp}"
        );

        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    /// 两条规则**各自的**过滤旋钮互不影响 ✓（§6 第 1 条在过滤这一层的形态 ✓）。
    #[tokio::test]
    async fn each_rule_has_its_own_filters() {
        let d = tmpdir("own-filters");
        let store = store_in(&d);
        seed_rule(
            &store,
            "r1",
            r#"{"onlyError":true,"serverChan":{"token":"STRICT"}}"#,
        )
        .await;
        seed_rule(&store, "r2", r#"{"serverChan":{"token":"LOOSE"}}"#).await;
        seed_schedule(&store, "s1", &["r1", "r2"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        // 这次**没有 error** ⇒ 只有 r2 发 ✓
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::success("好了"));
        wait_calls(&http, 1).await;
        assert_eq!(http.calls(), 1, "onlyError 那条不发");
        assert!(http.urls()[0].contains("LOOSE.send"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 投递失败 ⇒ 不影响运行结果、不 panic ✓。
    #[tokio::test]
    async fn delivery_failure_does_not_affect_the_run() {
        let d = tmpdir("delivery-fails");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::with(vec![Reply::Fail]);
        let (sink, inner) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        run_once(&sink, 1, Some("schedule:s1"), TaskOutcome::failed("boom"));
        wait_calls(&http, 1).await;

        assert_eq!(http.calls(), 1);
        assert_eq!(inner.len(), 2);
        assert!(matches!(
            inner.last(),
            Some(RunEvent::Finished {
                outcome: TaskOutcome::Failed { .. },
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 日志按 run 分桶：run1 的日志不会跑进 run2 的推送正文 ✓。
    #[tokio::test]
    async fn logs_are_scoped_per_run() {
        let d = tmpdir("logs-scoped");
        let store = store_in(&d);
        seed_rule(&store, "r1", r#"{"serverChan":{"token":"SC"}}"#).await;
        seed_schedule(&store, "s1", &["r1"], vec![]).await;

        let http = FakeHttp::new();
        let (sink, _) = sink_for(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            Arc::clone(&store),
            None,
        );
        sink.emit(started_from(1, Some("schedule:s1")));
        sink.emit(logged(1, LogLevel::Info, "run1 的日志"));
        sink.emit(started_from(2, Some("schedule:s1")));
        sink.emit(logged(2, LogLevel::Info, "run2 的日志"));
        sink.emit(finished(1, TaskOutcome::success("一"), 1));
        sink.emit(finished(2, TaskOutcome::success("二"), 2));
        wait_calls(&http, 2).await;

        let bodies: Vec<String> = (0..2)
            .map(|i| http.body_at(i)["desp"].as_str().unwrap().to_string())
            .collect();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("run1 的日志") && !b.contains("run2 的日志")),
            "run1 的正文只能有 run1 的日志：{bodies:?}"
        );
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("run2 的日志") && !b.contains("run1 的日志")),
            "run2 的正文只能有 run2 的日志：{bodies:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // ── 纯函数（老断言保留 ✓）────────────────────────────────────────────

    /// ⭐ 运行结果 → 推送事件（"哪条规则管得着"全靠它 ✓）。
    ///
    /// 钉住两条**容易写反**的：
    /// * **主动中止优先**：中止的运行 `outcome` 也可能是 `Failed` ✓ —— 必须先判
    ///   `cancelled` ✓（反过来会把"我自己点的中止"推成"运行失败" ✗）；
    /// * `Skipped` 归到"成功"那一档 ✓（"这次没什么可做"不该响 ✗）。
    #[test]
    fn outcome_maps_to_the_right_push_event() {
        let ok = TaskOutcome::Success {
            summary: "ok".into(),
        };
        let partial = TaskOutcome::Partial {
            summary: "半".into(),
        };
        let failed = TaskOutcome::Failed {
            reason: "挂了".into(),
        };
        let skipped = TaskOutcome::Skipped {
            reason: "没到点".into(),
        };
        assert_eq!(event_of(&ok, false), PushEvent::RunOk);
        assert_eq!(event_of(&partial, false), PushEvent::RunPartial);
        assert_eq!(event_of(&failed, false), PushEvent::RunFailed);
        assert_eq!(event_of(&skipped, false), PushEvent::RunOk);
        assert_eq!(event_of(&failed, true), PushEvent::RunCancelled);
    }

    // ⚠ 这里原先有 `local_minutes_is_beijing_and_wraps_at_midnight`（静默时段的
    // 分钟数换算）—— 随 `local_minutes` 与静默时段整块删掉 ✗（2026-10-08 用户口径
    // 「不要静默时间」✓）。

    /// outcome_line 的四档映射（旧 Severity::for_outcome 的接替者）逐条钉住。
    #[test]
    fn outcome_line_maps_like_the_old_severity() {
        assert_eq!(
            outcome_line(&TaskOutcome::success("好了"), false).0,
            "success"
        );
        assert_eq!(
            outcome_line(&TaskOutcome::skipped("已签到"), false).0,
            "info"
        );
        assert_eq!(outcome_line(&TaskOutcome::failed("炸了"), false).0, "error");
        // 中止优先于 TaskOutcome：即便业务结论是 Failed，也不许是 error
        assert_eq!(outcome_line(&TaskOutcome::failed("已中止"), true).0, "warn");
        assert_eq!(
            outcome_line(&TaskOutcome::success("居然成功"), true).0,
            "warn"
        );
    }

    /// 运行日志级别 → 推送类型的映射：只有 Error 会变成 error。
    #[test]
    fn log_level_maps_to_push_type() {
        assert_eq!(log_type(LogLevel::Debug), "debug");
        assert_eq!(log_type(LogLevel::Info), "info");
        assert_eq!(log_type(LogLevel::Warn), "warn");
        assert_eq!(log_type(LogLevel::Error), "error");
    }

    // ── 性能判据（面板行为的判定函数 ✓）────────────────────────────────

    fn run(task: &str, duration_ms: u64, ok: bool) -> RunRecord {
        RunRecord {
            run_id: RunId::new(1),
            task: task.into(),
            account: "19900000001".into(),
            started_at_ms: NOW_MS,
            duration_ms,
            outcome: if ok {
                TaskOutcome::success("ok")
            } else {
                TaskOutcome::failed("boom")
            },
            status: RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        }
    }

    /// 一条**被用户主动中止**的历史记录。
    ///
    /// ⚠ `outcome` 照样是 `Failed`（任务在安全点返回错误 ✓）——
    /// 这正是"只看 outcome 分不出中止与真失败"那条口径的来源 ✓
    ///（判据必须看 `status` ✓，见 `perf_verdict` 的说明 ✓）。
    fn run_cancelled(task: &str, duration_ms: u64) -> RunRecord {
        let mut record = run(task, duration_ms, false);
        record.status = RunStatus::Cancelled;
        record
    }

    /// 连续失败：**从头连着数** ✓（中间成功一次就断 ✓）。
    #[test]
    fn perf_fail_streak_counts_consecutive_only() {
        // 最近 3 条全失败 ⇒ 命中 ✓（阈值留空 ⇒ 兜底 `FAIL_STREAK = 3` ✓）
        let three = [
            run("sign", 10, false),
            run("sign", 10, false),
            run("sign", 10, false),
        ];
        let same: Vec<&RunRecord> = three.iter().collect();
        let hit = perf_verdict("sign", "19900000001", &same, 10, None, None).expect("连败三次该报");
        assert_eq!(hit.0, BehaviorKey::PerfFailStreak);

        // 中间夹一次成功 ⇒ 连续计数断在它那里 ✓（只算 1 次 ⇒ 不报 ✓）
        let broken = [
            run("sign", 10, false),
            run("sign", 10, true),
            run("sign", 10, false),
            run("sign", 10, false),
        ];
        let same: Vec<&RunRecord> = broken.iter().collect();
        assert!(
            perf_verdict("sign", "19900000001", &same, 10, None, None).is_none(),
            "历史失败过三次不等于**连续**失败三次"
        );
    }

    /// ⭐ **被中止的运行不算失败**（连败那一支 ✓）—— 历史「失败、失败、中止、失败」
    /// 的连败是 **3**（不是 4 ✓）：中止那条跳过、也**不断链** ✓。
    ///
    /// # 为什么这条必须有
    ///
    /// 中止的运行 `outcome` 也是 `Failed`（任务在安全点 `check_cancelled()?` 退出 ✓）
    /// ⇒ 只看 `outcome` 的话，"用户自己按了中止"会被报成"任务连续失败" ✗
    /// —— 一条纯噪声的告警，而且它会**加速**到达阈值（用户按 3 次中止就响 ✗）。
    #[test]
    fn perf_fail_streak_skips_cancelled_runs() {
        // 最新在前：失败、失败、中止、失败
        let history = [
            run("sign", 10, false),
            run("sign", 10, false),
            run_cancelled("sign", 10),
            run("sign", 10, false),
        ];
        let same: Vec<&RunRecord> = history.iter().collect();
        let hit = perf_verdict("sign", "19900000001", &same, 10, None, None)
            .expect("跳过中止之后仍是连败 3 次 ⇒ 该报");
        assert_eq!(hit.0, BehaviorKey::PerfFailStreak);
        assert!(
            hit.1.contains("连续失败 3 次"),
            "中止既不算失败、也不该断链（期望 3，不是 4 也不是 2）：{}",
            hit.1
        );

        // 全是中止 ⇒ **一次失败都没有** ⇒ 不报 ✓（"中止本身不算失败" ✓）
        let only_cancelled: Vec<RunRecord> = (0..4).map(|_| run_cancelled("sign", 10)).collect();
        let same: Vec<&RunRecord> = only_cancelled.iter().collect();
        assert!(
            perf_verdict("sign", "19900000001", &same, 10, None, None).is_none(),
            "四条中止不是四次失败"
        );
    }

    /// ⭐ **被中止的运行也不进耗时中位数**（用户口径：被中止的运行耗时不代表任务本身慢 ✓）。
    ///
    /// 判据：5 条正常（100ms）+ 6 条中止（100_000ms）——
    /// 中止的算进去会把中位数抬到 100_000ms（于是"慢"永远报不出来 ✗）；
    /// 排除之后中位数是 100ms ⇒ 这次的 10_000ms 该报 ✓。
    #[test]
    fn perf_slow_median_ignores_cancelled_runs() {
        let mut history: Vec<RunRecord> = (0..5).map(|_| run("sign", 100, true)).collect();
        history.extend((0..6).map(|_| run_cancelled("sign", 100_000)));
        let same: Vec<&RunRecord> = history.iter().collect();
        let hit = perf_verdict("sign", "19900000001", &same, 10_000, None, None)
            .expect("中止的耗时不该把中位数抬走 ⇒ 该报");
        assert_eq!(hit.0, BehaviorKey::PerfSlow);
        assert!(
            hit.1.contains("中位数 100ms"),
            "中位数只能由**没被中止**的样本算：{}",
            hit.1
        );
    }

    /// 耗时异常：样本 < 5 不算 ✓；超过中位数 3 倍（兜底阈值 ✓）才算 ✓。
    #[test]
    fn perf_slow_needs_enough_samples_and_a_real_outlier() {
        // 4 个样本 ⇒ 判据不成立（"中位数"没意义 ✓）
        let few: Vec<RunRecord> = (0..4).map(|_| run("sign", 100, true)).collect();
        let same: Vec<&RunRecord> = few.iter().collect();
        assert!(perf_verdict("sign", "19900000001", &same, 10_000, None, None).is_none());

        // 5 个样本、中位数 100ms ⇒ 10_000ms 是 100 倍 ⇒ 报 ✓
        let many: Vec<RunRecord> = (0..5).map(|_| run("sign", 100, true)).collect();
        let same: Vec<&RunRecord> = many.iter().collect();
        let hit =
            perf_verdict("sign", "19900000001", &same, 10_000, None, None).expect("100 倍该报");
        assert_eq!(hit.0, BehaviorKey::PerfSlow);
        assert!(hit.1.contains("中位数 100ms"), "{}", hit.1);

        // 只慢一倍 ⇒ 不报 ✓（兜底阈值是 3 倍）
        assert!(perf_verdict("sign", "19900000001", &same, 200, None, None).is_none());
    }

    /// ⭐ 面板行为的**判定**（触发线要调的那个 ✓）：从**运行历史**里算，
    /// 且只看同一个任务 + 同一个账号 ✓。
    ///
    /// ⚠ 触发线这一轮**不接**（见模块头 ✓），但判定函数本身必须已经能跑 ✓
    /// —— 否则"接上"那天才发现判据是错的 ✓。
    #[tokio::test]
    async fn perf_alert_reads_history_per_task_and_account() {
        let d = tmpdir("perf-alert");
        let store = store_in(&d);
        // 本账号同一个任务连败三次 ✓
        for i in 0..3 {
            let mut r = run("sign", 10, false);
            r.run_id = RunId::new(100 + i);
            store.upsert_run(&r).await.unwrap();
        }
        // 别的账号也有一条失败 —— 它**不该**算进本账号的连续计数 ✓
        let mut other = run("sign", 10, false);
        other.run_id = RunId::new(200);
        other.account = "19900000002".into();
        store.upsert_run(&other).await.unwrap();

        let n = notifier(Arc::new(PanicHttp), Arc::clone(&store));
        let hit = n
            .perf_alert(&owner(), "sign", "19900000001", 10)
            .await
            .expect("连败三次该报");
        assert_eq!(hit.0, BehaviorKey::PerfFailStreak);
        assert!(hit.1.contains("连续失败 3 次"), "{}", hit.1);

        // 别的账号只失败过一次 ⇒ 不报 ✓
        assert!(n
            .perf_alert(&owner(), "sign", "19900000002", 10)
            .await
            .is_none());
        // 没跑过的任务 ⇒ 不报 ✓
        assert!(n
            .perf_alert(&owner(), "backup", "19900000001", 10)
            .await
            .is_none());

        let _ = std::fs::remove_dir_all(&d);
    }
}

// ⚠ 上面那个 `mod tests { … }` 的收口在这里补一条：闸必须在**发之前**生效。

#[cfg(test)]
mod transport_gate_tests {
    use super::*;
    use mcloud_core::{HttpClient, HttpRequest, HttpResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 只数"发了几枪"的最小替身。
    #[derive(Default)]
    struct CountingHttp {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl HttpClient for CountingHttp {
        async fn request(&self, _req: HttpRequest) -> mcloud_core::Result<HttpResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
                headers: Vec::new(),
            })
        }
    }

    fn req(url: &str) -> PushRequest {
        PushRequest {
            method: "POST".into(),
            url: url.into(),
            headers: Vec::new(),
            body: Some(serde_json::json!({ "msgtype": "text" })),
            timeout_ms: 1_000,
        }
    }

    /// 假解析器：只认下面这张表，**不打真 DNS** ✓
    /// （真打 DNS 的单测会在离网机器上红、还会变慢 ✗）。
    struct FakeDns {
        table: Vec<(&'static str, Vec<&'static str>)>,
    }

    #[async_trait::async_trait]
    impl crate::push::PushHostResolver for FakeDns {
        async fn resolve(&self, host: &str, _port: u16) -> Result<Vec<std::net::IpAddr>, String> {
            match self.table.iter().find(|(h, _)| *h == host) {
                Some((_, ips)) => Ok(ips
                    .iter()
                    .map(|s| s.parse().expect("测试表里写的必须是 IP 字面量"))
                    .collect()),
                None => Err(format!("假解析器里没有 {host} 这条记录")),
            }
        }
    }

    /// 表里刻意放一条 `127.0.0.1.nip.io`：名字里嵌着回环 IP，而**字面量那层看不见**
    /// （它只看见一个普通域名 ✓）—— 这正是本轮要堵的那条（**实测绕过**过旧闸 ✗）。
    fn fake_dns() -> Arc<dyn crate::push::PushHostResolver> {
        Arc::new(FakeDns {
            table: vec![
                ("push.example.com", vec!["93.184.216.34"]),
                ("127.0.0.1.nip.io", vec!["127.0.0.1"]),
                ("nas.lan.test", vec!["10.0.0.5"]),
            ],
        })
    }

    /// ⚠⚠ 复审 `sv-8`：内网地址**一个包都不许发**（闸在 HTTP 出口，所有渠道共用）。
    ///
    /// 红测：把 `send` 里那句 `validate_push_url_resolved` 删掉 ⇒ `calls()` 会变成 5，红。
    #[tokio::test]
    async fn private_targets_never_reach_the_http_layer() {
        let http = Arc::new(CountingHttp::default());
        let transport = HttpPushTransport::with_resolver(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            fake_dns(),
            false,
        );

        for url in [
            // 字面量那层就挡下的
            "http://127.0.0.1:3000/api/system/restart",
            "http://169.254.169.254/latest/meta-data/",
            "http://192.168.1.10/hook",
            // ⚠⚠ **解析出来**才看得见的（本轮的核心）：名字里嵌着 IP
            "http://127.0.0.1.nip.io:3998/",
            // 解析出来是私网的名字
            "http://nas.lan.test/hook",
        ] {
            let err = transport
                .send(req(url))
                .await
                .expect_err("内网地址必须被拒，不能发出去");
            assert!(
                err.to_string().contains("拒绝发送"),
                "错因要能看出是「地址被拒」：{err}"
            );
        }
        assert_eq!(http.calls.load(Ordering::SeqCst), 0, "一个包都不该发出去");

        // 公网地址照旧能发（别把闸焊死在"永远拒"上）
        assert!(transport
            .send(req("https://push.example.com/hook"))
            .await
            .is_ok());
        assert_eq!(http.calls.load(Ordering::SeqCst), 1);
    }

    /// 服务级开关 `MCLOUD_ALLOW_LOCAL_PUSH` 开着 ⇒ 上面那一组**都放行**（照发 ✓）。
    ///
    /// ⚠ 它验的是"**这个开关真的接到了出口上**"（`calls()` 会涨 ✓）——
    /// 开关的解析规则与"因开关放行"的标记由 `push/tests.rs` 逐条钉 ✓。
    #[tokio::test]
    async fn local_push_switch_lets_private_targets_through() {
        let http = Arc::new(CountingHttp::default());
        let transport = HttpPushTransport::with_resolver(
            Arc::clone(&http) as Arc<dyn HttpClient>,
            fake_dns(),
            true,
        );

        for url in [
            "http://127.0.0.1:3000/api/system/restart",
            "http://169.254.169.254/latest/meta-data/",
            "http://192.168.1.10/hook",
            "http://127.0.0.1.nip.io:3998/",
            "http://nas.lan.test/hook",
            "https://push.example.com/hook",
        ] {
            assert!(
                transport.send(req(url)).await.is_ok(),
                "开关开着时这条本该照发：{url}"
            );
        }
        assert_eq!(
            http.calls.load(Ordering::SeqCst),
            6,
            "开关开着 ⇒ 一条都不该被闸拦下"
        );
    }
}
