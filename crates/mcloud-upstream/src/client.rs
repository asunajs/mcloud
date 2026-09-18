//! 端档案（**雏形**）：请求头里"我们是哪个客户端、哪台设备"。
//!
//! 完整的多端档案设计见 `docs/client-profiles.md`；这里先落地 PC 端
//! **登录链路必需**的那部分 —— 等接第二端时再按那份文档长成完整形态。
//!
//! # 为什么必须有它（一次实测换来的）
//!
//! 同一个 `/user/thirdlogin` 请求：
//!
//! | 请求头 | 上游响应 |
//! | --- | --- |
//! | 只带 `x-yun-tid` | `{"code":"9999","message":"登录失败！"}` |
//! | 加上下面的设备头 | `{"code":"200059541","message":"未找到设备的二维码信息"}` |
//!
//! 后者才是文档里"码还没被扫"的正常状态。所以这几个头**不是装饰**：
//! 少了它们，扫码登录连第一步都进不去，而错误码（9999）还不是文档里任何一个。
//!
//! # 设备信息串的形状
//!
//! 客户端 `apiServer.js: getClientDeviceInfo()` 在**拿不到原生能力时**用的兜底模板：
//!
//! ```js
//! `||11|${appVersion}|PC|QkYtMjAyMDAzMTAxNjQ3|${windowMachine}|| Windows 10 (10.0)|1920X1040|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||`
//! ```
//!
//! 我们正是"没有原生能力"的那种客户端，所以**照抄这个模板**是最忠实的选择：
//! 它是客户端源码里的公开常量，不是我们编的。14 段的字段表见
//! `docs/client-profiles.md` §5.2。

use crate::login::PcPlatform;

/// 环境枚举段（第 2 段），兜底模板里的值。
const DEVICE_INFO_ENV: &str = "11";
/// 设备型号码（第 5 段）：base64 的 `BF-202003101647`，客户端兜底模板的常量。
const DEVICE_INFO_MODEL_B64: &str = "QkYtMjAyMDAzMTAxNjQ3";
/// 语言（第 10 段）：base64 的 `Chinese (Simplified)`。
const DEVICE_INFO_LOCALE_B64: &str = "Q2hpbmVzZSAoU2ltcGxpZmllZCk=";
/// OS 描述（第 8 段）。**前导空格是原样**，别顺手 trim 掉。
const DEVICE_INFO_OS: &str = " Windows 10 (10.0)";
/// 分辨率（第 9 段）。
const DEVICE_INFO_RESOLUTION: &str = "1920X1040";

/// 市场来源缺失时的兜底值（客户端源码：`x-MM-Source: "000"`）。
const MM_SOURCE_FALLBACK: &str = "000";
/// 华为渠道号（客户端源码在 pre-login 分支里硬编码）。
const HUAWEI_CHANNEL_SRC: &str = "10200153";

/// 拼设备信息串（14 段，`|` 分隔）。
///
/// # 关于平台差异
///
/// `clientType` / `cpid` / 版本号是**按平台**变的（见 [`PcPlatform`]），
/// 但这条兜底模板只有 Windows 版有实测依据。macOS 分支暂时复用同一串常量：
/// 它们本身是客户端源码里的公开值（不是编的），只是"mac 客户端到底发什么"
/// **尚无实测** —— 这一点记在 `docs/client-profiles.md` 的未知项里，
/// 拿到 macOS 流量后要回来替换。
pub fn device_info(platform: PcPlatform, device_id: &str) -> String {
    format!(
        "||{DEVICE_INFO_ENV}|{}|PC|{DEVICE_INFO_MODEL_B64}|{device_id}||{DEVICE_INFO_OS}|{DEVICE_INFO_RESOLUTION}|{DEVICE_INFO_LOCALE_B64}|||",
        platform.app_version()
    )
}

/// 客户端档案：一次登录里不变的部分。
///
/// 它把"我们是哪个端、哪台设备"收成一个值，于是每个请求构造点只需要
/// `profile.headers()`，不会有人漏掉某个头 —— 而漏一个头的代价是 9999。
#[derive(Debug, Clone)]
pub struct ClientProfile {
    platform: PcPlatform,
    device_id: String,
    device_info: String,
}

impl ClientProfile {
    /// 构造档案（设备信息串只算一次）。
    pub fn new(platform: PcPlatform, device_id: impl Into<String>) -> Self {
        let device_id = device_id.into();
        let device_info = device_info(platform, &device_id);
        Self {
            platform,
            device_id,
            device_info,
        }
    }

    pub const fn platform(&self) -> PcPlatform {
        self.platform
    }

    /// 设备指纹（`windowMachine`）。它同时嵌在 [`ClientProfile::device_info`] 里。
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn device_info(&self) -> &str {
        &self.device_info
    }

    /// 登录链路要带的头（`Content-Type` 与每次新的 `x-yun-tid` 由请求构造补上）。
    ///
    /// 逐个对应客户端 `common/utils.js: sendRequest` 的 **pre-login 分支**
    /// （登录时还没有 `UserInfo`，走的正是这一支）：
    ///
    /// ```js
    /// xhr.setRequestHeader("x-yun-client-info", windowDeviceInfo);
    /// xhr.setRequestHeader("x-DeviceInfo",      windowDeviceInfo);  // 同一个值、两个头
    /// xhr.setRequestHeader("x-MM-Source",       appSource || "000");
    /// xhr.setRequestHeader("x-huawei-channelSrc","10200153");
    /// xhr.setRequestHeader("x-UserAgent",       "PC");
    /// ```
    pub fn headers(&self) -> Vec<(String, String)> {
        vec![
            // 两个头同一个值：不是笔误，客户端就是这么发的（大小写不同）
            ("x-yun-client-info".into(), self.device_info.clone()),
            ("x-DeviceInfo".into(), self.device_info.clone()),
            ("x-MM-Source".into(), MM_SOURCE_FALLBACK.into()),
            ("x-huawei-channelSrc".into(), HUAWEI_CHANNEL_SRC.into()),
            ("x-UserAgent".into(), "PC".into()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_reproduces_the_client_fallback_template() {
        let info = device_info(PcPlatform::Windows, "DEV-1");
        // 逐段对齐源码模板，尤其注意第 0/1 段是空的（前导 `||`）、尾部是 `|||`
        assert_eq!(
            info,
            "||11|8.8.6.20260829|PC|QkYtMjAyMDAzMTAxNjQ3|DEV-1|| Windows 10 (10.0)|1920X1040|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||"
        );

        let segments: Vec<&str> = info.split('|').collect();
        assert_eq!(segments.len(), 14, "字段表是 14 段，段数变了说明模板抄错了");
        assert_eq!(segments[0], "");
        assert_eq!(segments[1], "");
        assert_eq!(segments[2], "11");
        assert_eq!(segments[4], "PC");
        assert_eq!(segments[6], "DEV-1", "第 6 段是设备指纹（明文）");
        assert_eq!(segments[8], " Windows 10 (10.0)", "OS 段的前导空格是原样");
        assert_eq!(segments[13], "");
    }

    #[test]
    fn device_info_follows_the_platform_version() {
        // 版本号在设备串里，所以它必须跟着平台走（mac 的 clientType 配 win 的版本 = 自相矛盾）
        let mac = device_info(PcPlatform::MacOs, "D");
        assert!(mac.contains("|4.2.1|PC|"), "{mac}");
        assert!(device_info(PcPlatform::Windows, "D").contains("|8.8.6.20260829|PC|"));
    }

    #[test]
    fn headers_carry_both_device_info_names_and_the_channel() {
        let profile = ClientProfile::new(PcPlatform::Windows, "DEV-1");
        let headers = profile.headers();
        let get = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("缺少头 {name}"))
        };

        // 客户端发的是两个不同大小写的头、同一个值
        assert_eq!(get("x-yun-client-info"), get("x-DeviceInfo"));
        assert_eq!(get("x-yun-client-info"), profile.device_info());
        assert_eq!(get("x-MM-Source"), "000");
        assert_eq!(get("x-huawei-channelSrc"), "10200153");
        assert_eq!(get("x-UserAgent"), "PC");
        // 设备指纹必须在设备串里（服务端按 dID + sID 定位会话）
        assert!(get("x-yun-client-info").contains(profile.device_id()));
    }
}
