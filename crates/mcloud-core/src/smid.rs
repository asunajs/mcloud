//! 账号绑定的设备指纹状态（smid）。
//!
//! # 为什么设备指纹与登录态**不是**同一样东西
//!
//! 两者都挂在账号上，但生命周期完全不同：
//!
//! | | 什么时候变 | 语义 |
//! | --- | --- | --- |
//! | 登录态（`authorization` / `jwtToken`） | 每次登录、每次刷新 | "能冒充这个用户的凭据" |
//! | 设备指纹（`fingerprint_json`） | **几乎不变**，只在主动换指纹时 | "这台设备长什么样" |
//!
//! 混在一起存的直接后果是：**任何一次取凭据都会顺手把指纹也读进内存**，
//! 而两者的泄露面完全不是一回事；更糟的是登录路径做整块覆盖写入时，
//! 会**顺手把指纹清掉** —— 换凭据不该换掉设备长相。
//!
//! 所以指纹单独一个结构、单独一条持久化通道，并使用**窄写**：
//! 只动它自己那两列。
//!
//! # 这一层只搬文本
//!
//! `fingerprint_json` 是 `mcloud_upstream::smid::SmFingerprint` 的序列化结果。
//! 把那个类型放进 core 会让 core 反向依赖协议层（违反分层铁律），
//! 所以这里存 JSON 串、由调用方负责解析 —— 代价是"库里那串读不出来"
//! 必须由调用方处理（那里的规矩是**报错，不是悄悄重掷一份**）。

use crate::account::AccountId;
use crate::error::{Error, Result};
use async_trait::async_trait;

/// 一个账号的 smid 状态。
///
/// # 刻意不派生 `Serialize`
///
/// 这两样都不是能吐给前端的东西：`device_id` 是第三方风控服务给这台设备的标识。
/// 少一个 `Serialize`，就少一条"顺手塞进 HTTP 响应"的路。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SmidState {
    /// 绑定账号的设备特征（JSON 文本）。`None` = 还没绑过。
    pub fingerprint_json: Option<String>,
    /// 最近一次上报换回的 `deviceId`（含 `B` 前缀）。
    ///
    /// **会变**：它只用作下一次上报特征体里的 `box` 字段，
    /// 让风控服务看出"还是那台机器在换 id"。
    pub device_id: Option<String>,
}

impl SmidState {
    /// 还没绑定过任何东西的空状态。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 是否绑过指纹。
    pub fn has_fingerprint(&self) -> bool {
        self.fingerprint_json.is_some()
    }
}

impl std::fmt::Debug for SmidState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只暴露"有没有"与长度，足以判断"有没有被清掉"，又不把长相打进日志。
        fn describe(value: &Option<String>) -> Option<String> {
            value
                .as_ref()
                .map(|v| format!("<redacted {} 字符>", v.chars().count()))
        }
        f.debug_struct("SmidState")
            .field("fingerprint_json", &describe(&self.fingerprint_json))
            .field("device_id", &describe(&self.device_id))
            .finish()
    }
}

/// 设备指纹的持久化契约（**窄写**）。
///
/// # 为什么单独一个 trait，而不是塞进 `AccountStore`
///
/// 沿用本项目「拆成聚焦的子 trait」的既有做法（见 `traits.rs` 对 `LogStore`
/// 的说明）：不需要设备指纹的调用方不该被迫实现或依赖这几个方法。
/// 更要紧的是**窄写必须是一个独立的动作**：把它塞进"保存账号状态"那种
/// 整块覆盖的写里，登录路径就会顺手把指纹清掉（见本模块文档）。
#[async_trait]
pub trait SmidStore: Send + Sync {
    /// 读这个账号的 smid 状态。从没绑过时返回 `None`。
    ///
    /// 返回 `None` 而不是空 `SmidState`：两者语义不同 ——
    /// 「从没绑过」要走"生成一份并落库"，而"绑过但 deviceId 丢了"不该重新生成长相。
    async fn get_smid_state(&self, id: &AccountId) -> Result<Option<SmidState>>;

    /// 写入**新的**设备特征，并把 `device_id` 清空。
    ///
    /// # 为什么清空 deviceId 与写特征是**同一个动作**
    ///
    /// 换指纹的语义是"这台机器换个长相"。旧的 `deviceId` 是**旧长相**换来的那一个，
    /// 留着它，下一次上报就等于自相矛盾：新的屏幕/ua/canvas，配的却是旧设备的 id ——
    /// 那正是风控服务最想看到的"设备在漂移"信号。
    ///
    /// 所以这两件事不能拆成两次调用：中间崩溃会留下"新指纹 + 旧 id"，
    /// 而且那种状态**看不出来**（两列各自都合法）。
    async fn set_smid_fingerprint(&self, id: &AccountId, fingerprint_json: &str) -> Result<()>;

    /// 记下最近一次上报换回的 `deviceId`（含 `B` 前缀）。
    ///
    /// 只在**上报成功**之后调用：拿到的 id 会被下一次上报当作 `box` 带回去，
    /// 一个失败的、或半截的响应值写进去，等于让这台设备从此自称一个来历不明的 id。
    ///
    /// **不**动 `fingerprint_json`：两者是独立事实。
    async fn set_smid_device_id(&self, id: &AccountId, device_id: &str) -> Result<()>;
}

/// 把 `SmidStore` 的错误统一收敛 —— 实现方不必各自重复这段。
pub fn smid_storage_error(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Storage(format!("{what}：{e}"))
}
