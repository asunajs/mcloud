//! 服务状态。

use std::sync::Arc;

use mcloud_app::AppHandle;

#[derive(Clone)]
pub struct ServerState {
    pub handle: Arc<AppHandle>,
}

impl ServerState {
    pub fn new(handle: Arc<AppHandle>) -> Self {
        Self { handle }
    }
}
