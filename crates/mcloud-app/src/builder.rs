//! 应用组装。

use std::path::PathBuf;
use std::sync::Arc;

use mcloud_core::{Account, EventSink, HttpClient, Result};

use crate::hub::EventHub;
use crate::notify;
use crate::AppHandle;

/// 应用构建器。
pub struct AppBuilder {
    data_dir: PathBuf,
    timeout_secs: u64,
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            timeout_secs: 30,
        }
    }
}

impl AppBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = dir.into();
        self
    }

    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// 组装应用。
    pub fn build(self) -> Result<App> {
        let store = Arc::new(mcloud_store::JsonStore::open(&self.data_dir)?);
        let clock = Arc::new(mcloud_store::SystemClock);
        // 事件中枢取代原来的纯广播：它额外负责序号分配、内存重放与日志落盘。
        let hub = Arc::new(EventHub::new(
            Arc::clone(&store) as Arc<dyn mcloud_core::Store>,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        ));

        let mut registry = mcloud_engine::Registry::new();
        mcloud_tasks::register_all(&mut registry).map_err(mcloud_core::Error::Config)?;
        let registry = Arc::new(registry);

        // **会话缓存**：进程级共享。
        //
        // 不落地（`jwttoken` 2 小时即废，而库里那一列装的是登录凭据 ——
        // 同名不同物）。进程重启就重取，代价是三条请求。
        //
        // 共享而不是每账号一份：这样"同账号同配方"即使由不同任务发起，
        // 也只建一次票 —— 用户连点三次签到，真实客户端是"页面还开着就用到底"。
        let sessions = Arc::new(mcloud_upstream::SessionCache::new());

        // **smid 编排**：一个进程一份，服务所有账号。
        //
        // 它用的是**独立的裸传输**（下面那个 `smid_transport`），刻意不复用
        // 业务客户端：`deviceprofile/v4` 是第三方风控服务，云盘凭据一个都不能
        // 发过去（见 `mcloud_upstream::smid` 的说明）。共用一个客户端等于
        // 把 `authorization` 交给它。
        //
        // 随机源是进程级共享的 OS 熵源：`smid` 字段里的 uuid 每次都该不同，
        // 而指纹的生成一次就落库，共享不会让两个账号"长得一样"
        // （每个账号的指纹只生成一次，且从库里复用）。
        let smid_http: Arc<dyn HttpClient> = Arc::new(
            mcloud_upstream::ReqwestClient::new(mcloud_upstream::ClientProfile {
                // 一个凭据都不给：这台主机不认云盘凭据，也不该看到。
                basic_token: None,
                jwt_token: None,
                device_id: None,
                user_agent: None,
                timeout_secs: self.timeout_secs,
                // smid 上报打的是**第三方风控**主机，与 PC 形态无关：
                // 带上 PC 设备头等于把"我是谁"多告诉一个不需要它的服务。
                pc: None,
            })
            .map_err(|e| mcloud_core::Error::Network(format!("构建 smid 客户端失败：{e}")))?,
        );
        let smid = Arc::new(crate::smid::SmidService::new(
            smid_http,
            Arc::clone(&store) as Arc<dyn mcloud_core::SmidStore>,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
            Box::new(mcloud_crypto::OsRandom::new()),
        ));

        // **登录编排**：一个进程一份。
        //
        // 它用**裸传输**（一个不注入任何凭据的 reqwest 客户端）：
        // 登录时还没有凭据，而登录的产物正是凭据。
        // 刻意**不**复用下面的业务客户端 —— 那个绑定在账号上。
        let login_http = Arc::new(
            mcloud_upstream::ReqwestClient::new(mcloud_upstream::ClientProfile {
                timeout_secs: self.timeout_secs,
                ..Default::default()
            })
            .map_err(|e| mcloud_core::Error::Network(format!("构建登录客户端失败：{e}")))?,
        ) as Arc<dyn HttpClient>;
        let login = Arc::new(crate::login::LoginService::with_os_random(
            login_http,
            Arc::clone(&store) as Arc<dyn mcloud_core::PcDeviceStore>,
            Arc::clone(&store) as Arc<dyn mcloud_core::AccountStore>,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        ));

        // 每个账号一个客户端：凭证绑定在客户端上。
        //
        // 两层装饰器，顺序有讲究：
        //
        // ```text
        // SmidClient  ← 补 deviceid 头（懒加载，第一次请求时才上报换 id）
        //   └ H5Client  ← 建会话、带 jwttoken、90001 刷新重放
        //       └ ReqwestClient  ← 真正发请求，按域名注入 authorization
        // ```
        //
        // `SmidClient` 在外层：它补的头要作用于**所有**出去的请求，
        // 包括 `H5Client` 建会话那三枪。
        let timeout = self.timeout_secs;
        let http_for: mcloud_engine::HttpClientFactory = {
            let smid = Arc::clone(&smid);
            Arc::new(move |account: &Account| {
                // PC 端账号要把"冒充哪个平台"告诉传输层：UA、设备串与渠道头都由它
                // 决定，而这些**只由账号落盘的那一列**决定（凭据分不出来 ——
                // PC 两种平台的 `Authorization` 前缀都是 `pc`）。
                //
                // `pc_platform` 为 `None`（App / 历史数据）时**不发** PC 设备头：
                // 拿 PC 身份去跑 App 账号会被上游按另一端的配额与风控处理。
                let pc = account.pc_platform.map(|platform| {
                    // 设备串第 6 段就是账号的设备指纹，`PcClient` 让两者在类型上
                    // 绑定，于是 `device_info()` 不可能在"没有设备号"时被调用。
                    mcloud_upstream::PcClient::new(platform, account.device_id.as_str())
                });

                match mcloud_upstream::ReqwestClient::new(mcloud_upstream::ClientProfile {
                    basic_token: Some(account.auth.raw.clone()),
                    // 刻意**不**给 `jwt_token`：票由 `H5Client` 按需获取。
                    // 从配置里读一张票是错的 —— 那张票 2 小时就废了。
                    jwt_token: None,
                    // 同样刻意**不**给静态 device_id：那个头由 `SmidClient`
                    // 按账号懒加载补上（它要先去风控服务换）。
                    device_id: None,
                    user_agent: None,
                    timeout_secs: timeout,
                    pc,
                }) {
                    Ok(c) => {
                        let transport = Arc::new(c) as Arc<dyn HttpClient>;
                        let with_session = Arc::new(mcloud_upstream::H5Client::new(
                            transport,
                            account.clone(),
                            mcloud_upstream::SessionRecipe::SignInPage,
                            Arc::clone(&sessions),
                        )) as Arc<dyn HttpClient>;
                        Arc::new(crate::smid::SmidClient::new(
                            with_session,
                            Arc::clone(&smid),
                            account.id.clone(),
                        )) as Arc<dyn HttpClient>
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "构建 HTTP 客户端失败");
                        // 退化到一个必然失败的客户端，让任务以网络错误结束，
                        // 而不是让整个进程崩溃
                        Arc::new(
                            mcloud_upstream::ReqwestClient::new(mcloud_upstream::ClientProfile {
                                timeout_secs: 1,
                                ..Default::default()
                            })
                            .expect("最小客户端应可构建"),
                        )
                    }
                }
            })
        };

        // **凭据续期**：一个进程一份，四个触发面共用。
        //
        // 与 `login` 不同：刷新**需要凭据**（要证明"我是这个 token 的持有者"），
        // 所以它用业务客户端工厂（按账号注入 Authorization），
        // 而不是登录用的那份裸传输。
        let auth_refresh = Arc::new(crate::auth_refresh::AuthRefreshService::new(
            Arc::clone(&store) as Arc<dyn mcloud_core::Store>,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
            // ⚠ 这里必须 `Arc::clone`：`http_for` 后面还要交给 `Runner`，
            // 直接 move 进去会让那一处"borrow of moved value"。
            Arc::clone(&http_for),
        ));

        // ── 推送渠道（**旁路**，地址按账号解析）────────────────────────────
        //
        // 挂点选在 sink（而不是订阅 hub）：runner 的每个事件都从这**同一个**
        // 出口过，在它外面套一层即可，engine 零改动（理由详见 crate::notify
        // 模块头）。
        //
        // ⚠ **总是**装饰一层（不按"启动时是否有配置"决定）：渠道是**每账号**的，
        // 配置在投递那一刻才从该账号的 settings 读，运行期间还能改。跳过装饰
        // 等于"账号级配置永远不会生效"。
        // "未配置就零行为"由解析兜住：账号没配 ⇒ 回 None ⇒ 一个包都不发
        // （见 notify::Notifier::config_for）。**没有**进程级兜底。
        let cancel_slot = notify::cancel_slot();
        // 推送打的是**用户自己配的外部地址**，云盘凭据一个都不能给：
        // 与 smid 的裸传输同一条理由（见上面 smid_http 的说明）。
        let notify_http: Arc<dyn HttpClient> = Arc::new(
            mcloud_upstream::ReqwestClient::new(mcloud_upstream::ClientProfile {
                timeout_secs: self.timeout_secs,
                ..Default::default()
            })
            .map_err(|e| mcloud_core::Error::Network(format!("构建推送客户端失败：{e}")))?,
        );
        let sink: Arc<dyn EventSink> = notify::decorate(
            Arc::clone(&hub) as Arc<dyn EventSink>,
            Arc::clone(&store) as Arc<dyn mcloud_core::AccountStore>,
            notify_http,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
            Arc::clone(&cancel_slot),
        );

        let runner = Arc::new(mcloud_engine::Runner::new(
            Arc::clone(&registry),
            Arc::clone(&store) as Arc<dyn mcloud_core::Store>,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
            sink,
            http_for,
        ));
        // 回填取消登记表：装饰器要先于 runner 存在，而登记表由 Runner::new
        // 内部创建。这里到 build() 返回之前一定填好，且还没开始任何一次运行。
        let _ = cancel_slot.set(runner.cancel_registry());

        // ⚠ 顺序：编排运行器**必须先建**，调度器要拿它（定时跑编排）。
        // 同一个实例也交给 `AppHandle::pipelines`（路由那条路）—— 共用一份。
        let pipelines = Arc::new(mcloud_engine::PipelineRunner::new(Arc::clone(&runner)));
        let scheduler = Arc::new(mcloud_engine::Scheduler::new(
            Arc::clone(&store) as Arc<dyn mcloud_core::Store>,
            Arc::clone(&runner),
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
            Arc::clone(&pipelines),
        ));
        let cancel = runner.cancel_registry();

        Ok(App {
            handle: AppHandle {
                data_dir: self.data_dir,
                shutdown: Arc::new(tokio::sync::Notify::new()),
                runner,
                registry,
                store,
                clock,
                hub,
                scheduler,
                pipelines,
                cancel,
                smid,
                login,
                auth_refresh,
            },
        })
    }
}

/// 已组装的应用。
///
/// ⚠ 数据目录在 [`AppHandle::data_dir`] 上，不在这儿 —— 服务器只有句柄，
/// 而「存储信息」要报它。
pub struct App {
    pub handle: AppHandle,
}

impl App {
    pub fn runner(&self) -> &Arc<mcloud_engine::Runner> {
        &self.handle.runner
    }
    pub fn registry(&self) -> &Arc<mcloud_engine::Registry> {
        &self.handle.registry
    }
    pub fn hub(&self) -> &Arc<EventHub> {
        &self.handle.hub
    }
    pub fn scheduler(&self) -> &Arc<mcloud_engine::Scheduler> {
        &self.handle.scheduler
    }
}
