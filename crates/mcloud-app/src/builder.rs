//! 应用组装。

use std::path::PathBuf;
use std::sync::Arc;

use mcloud_core::{Account, HttpClient, Result};

use crate::sink::BroadcastSink;
use crate::AppHandle;

/// 应用构建器。
pub struct AppBuilder {
    data_dir: PathBuf,
    timeout_secs: u64,
    broadcast_capacity: usize,
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            timeout_secs: 30,
            broadcast_capacity: 256,
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
        let sink = Arc::new(BroadcastSink::new(self.broadcast_capacity));

        let mut registry = mcloud_engine::Registry::new();
        mcloud_tasks::register_all(&mut registry).map_err(mcloud_core::Error::Config)?;
        let registry = Arc::new(registry);

        // 每个账号一个客户端：凭证绑定在客户端上
        let timeout = self.timeout_secs;
        let http_for: mcloud_engine::HttpClientFactory = Arc::new(move |account: &Account| {
            let profile = mcloud_upstream::ClientProfile {
                basic_token: Some(account.auth.raw.clone()),
                jwt_token: None,
                device_id: Some(account.device_id.as_str().to_string()),
                user_agent: None,
                timeout_secs: timeout,
            };
            match mcloud_upstream::ReqwestClient::new(profile) {
                Ok(c) => Arc::new(c) as Arc<dyn HttpClient>,
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
        });

        let runner = Arc::new(mcloud_engine::Runner::new(
            Arc::clone(&registry),
            Arc::clone(&store) as Arc<dyn mcloud_core::Store>,
            Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
            Arc::clone(&sink) as Arc<dyn mcloud_core::EventSink>,
            http_for,
        ));

        Ok(App {
            handle: AppHandle {
                runner,
                registry,
                store,
                clock,
                sink,
            },
            data_dir: self.data_dir,
        })
    }
}

/// 已组装的应用。
pub struct App {
    pub handle: AppHandle,
    pub data_dir: PathBuf,
}

impl App {
    pub fn runner(&self) -> &Arc<mcloud_engine::Runner> {
        &self.handle.runner
    }
    pub fn registry(&self) -> &Arc<mcloud_engine::Registry> {
        &self.handle.registry
    }
    pub fn sink(&self) -> &Arc<BroadcastSink> {
        &self.handle.sink
    }
}
