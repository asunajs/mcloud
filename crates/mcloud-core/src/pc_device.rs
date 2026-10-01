//! 机器级的 PC 设备指纹（`windowMachine`）。
//!
//! # 它今天与登录 / 出码**已经解耦**
//!
//! ⚠ 这是**当前事实**，与文件名的直觉不同：
//!
//! * **出二维码不用它** —— `mcloud_app::LoginService::start_qr` 每次出码
//!   现生成一枚（`fresh_device`）：一张码只服务一次登录，共用机器那枚
//!   会让第二个扫码的账号和第一个撞上同一台"设备"；
//! * **四条登录路径也不用它** —— 扫码、短信、账密、App 登录都按**账号**
//!   取/生成那一枚（`device_for_account`），动机同 [`crate::SmidState`]。
//!
//! 于是机器上这枚 `data/pc_device.txt` 只有两个消费者：
//! `GET /api/login/device`（CLI `login device show`）与
//! `POST /api/login/device/rotate`（CLI `login device reset`）。
//! **换掉它不会作废任何扫码会话，也不影响任何账号的登录** ——
//! 它只改变上面两个入口随后读到的那个值。
//!
//! # 为什么还留着、且必须是持久化的
//!
//! 它是"这台机器是谁"的登记处：登录路径按账号分设备之后，机器级这枚
//! 没有跟着删 —— 一是兼容老数据，二是 `login device show` 需要一个可看、
//! 可换的**稳定**值（每次读都重新生成的话，这个入口就没有意义）。
//! 将来若再出现"登录之前就需要一枚设备号"的场景，从这里取仍然是正解。
//!
//! 历史形态（**今天不成立，别再照此推理**）：早期四条登录路径与出码都取
//! 这一枚，于是它同时是服务端定位"这次扫码会话是哪台设备"的凭据（`dID`）。
//! 那时每次重新生成的表现是轮询**永远停在**
//! `200059541 未找到设备的二维码信息` —— 看起来像"码过期了"，
//! 极难往"设备号变了"上想。按账号分设备之后，这个失效模式已不在这条路上。
//!
//! 上游参考实现把它做成一个**文件路径**而不是一个字符串参数，注释说得直白：
//! 「传字符串进来会让'每次重启换一台设备'这种配置错误编译得过、跑得起来
//! 也看不出问题」。这里沿用同一个取舍：trait 只暴露"给我这个机器的指纹"，
//! 取不到时由上层生成并落盘（见下一节）。
//!
//! # 形状
//!
//! 32 位**大写**十六进制 + `-ENDIN` 后缀（实测值形如
//! `62AA2BDA29F99744536E142A83CA5B03-ENDIN`）。后缀是设备串的一部分，
//! 少一个字符就等于换了一台设备。

use async_trait::async_trait;

use crate::error::Result;

/// 设备指纹的后缀（客户端实测）。
pub const PC_DEVICE_SUFFIX: &str = "-ENDIN";

/// 机器级 PC 设备指纹的读写。
///
/// # 为什么不做成 `Store` 的超 trait
///
/// 与 [`crate::SmidStore`] 同样的理由：`Store` 是给任务用的组合接口，
/// 而这条能力今天只有 `GET /api/login/device` / `POST /api/login/device/rotate`
/// （CLI `login device show|reset`）这两个入口需要。加进超 trait 会让
/// **每个测试替身**都被迫实现一个用不到的 `device_id` —— 那是纯粹的噪音。
///
/// # 为什么只有"读 / 写"，没有"读不到就生成"
///
/// 生成要用随机源，而随机源与 hex 编码都在 `mcloud-crypto` ——
/// 让 `mcloud-core` 依赖它是**反向依赖**（见 docs/conventions.md §1）。
///
/// 所以这一层只声明"存哪里"，把"没有就生成"留给有随机源的上层
/// （`mcloud_upstream::login::load_or_create_pc_device`）。
/// 这个拆分同时让**上半段可测**：测试可以只验"读到旧值时不生成新的"，
/// 不需要真的走一遍随机。
#[async_trait]
pub trait PcDeviceStore: Send + Sync {
    /// 读这台机器的 PC 设备指纹。从没生成过时返回 `None`。
    ///
    /// 返回 `None` 而不是回落到一个临时值：调用方必须**显式**决定
    /// "没有就去生成并落盘"，否则一个临时值会被当成这台机器的登记值。
    async fn pc_device_id(&self) -> Result<Option<String>>;

    /// 写入机器级设备指纹（首次生成与主动更换都走它）。
    ///
    /// ⚠ 它**不与任何登录 / 扫码会话挂钩**：换掉只影响随后
    /// `pc_device_id()` 读到的值，不会让谁失效（见模块头）。
    async fn set_pc_device_id(&self, device_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_is_the_measured_one() {
        // 后缀是设备串的一部分，少一个字符就等于换了一台设备
        assert_eq!(PC_DEVICE_SUFFIX, "-ENDIN");
    }
}
