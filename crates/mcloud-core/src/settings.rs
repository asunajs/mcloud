//! 应用级设置 —— 能在 WebUI 里改的那几项。
//!
//! # 为什么默认值 = 原来写死的常量
//!
//! 这两项本来是 `const`（口令下限在 `mcloud-crypto::MIN_PASSWORD_LEN`、
//! 会话时长在 [`crate::WEB_SESSION_TTL_MS`]）。做成可改的设置之后，
//! **默认值必须与那两个常量一致** —— 否则"从没改过设置"的实例行为会与改之前
//! 不一样，而那是最难查的一类回归（没人会想到去查一个自己没碰过的开关）。
//!
//! 一致性由 `mcloud-server` 的测试钉住：那个 crate 同时依赖本模块与 crypto。
//!
//! # 为什么校验放在这里
//!
//! 上下限是**领域约束**，不是某个接口的偏好：HTTP 那边要按它回 400，
//! 将来 CLI 也要按它报错，两处各写一份必然分叉。所以校验跟着类型走，
//! 返回值是**给人看的一句话**（不需要程序分支，就不为它造错误枚举）。

use serde::{Deserialize, Serialize};

/// 口令长度下限的默认值（= `mcloud_crypto::MIN_PASSWORD_LEN`）。
pub const DEFAULT_MIN_PASSWORD_LEN: usize = 8;

/// 会话时长的默认值（= [`crate::WEB_SESSION_TTL_MS`]，7 天）。
pub const DEFAULT_SESSION_TTL_HOURS: u32 = 7 * 24;

/// 口令下限允许的范围。
pub const MIN_PASSWORD_LEN_RANGE: (usize, usize) = (4, 64);

/// 会话时长允许的范围（小时）：1 小时 ~ 90 天。
pub const SESSION_TTL_HOURS_RANGE: (u32, u32) = (1, 24 * 90);

/// 应用级设置。
///
/// ⚠ `#[serde(default)]` 不能省：`settings.json` 是**后来才有的**文件，
/// 老实例第一次读它时文件是空的（或缺字段），那时必须按默认值走 ——
/// 而不是"配置损坏，起不来"。加一个设置项也不该让老文件失效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 口令长度下限。
    ///
    /// ⚠ 只对**明文**生效：前端摘要恒 64 位，量不出人到底输了几位
    /// （所以 WebUI 会拿这个值在前端先卡一道，服务端那道留给 curl 这类直接调用）。
    pub min_password_len: usize,
    /// 新建会话的有效期。改它**只影响之后新建的会话**，已发出的不会被动缩短。
    pub session_ttl_hours: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            min_password_len: DEFAULT_MIN_PASSWORD_LEN,
            session_ttl_hours: DEFAULT_SESSION_TTL_HOURS,
        }
    }
}

impl AppSettings {
    /// 会话 TTL（毫秒）。**唯一的换算点** —— 别在别处再乘一遍。
    pub fn session_ttl_ms(&self) -> i64 {
        i64::from(self.session_ttl_hours) * 60 * 60 * 1000
    }

    /// 校验。返回 `Err(人话)` 时调用方直接 400 透出，不做二次翻译。
    pub fn validate(&self) -> Result<(), String> {
        let (lo, hi) = MIN_PASSWORD_LEN_RANGE;
        if self.min_password_len < lo || self.min_password_len > hi {
            return Err(format!("口令下限要在 {lo}~{hi} 之间"));
        }
        let (lo, hi) = SESSION_TTL_HOURS_RANGE;
        if self.session_ttl_hours < lo || self.session_ttl_hours > hi {
            return Err(format!("会话时长要在 {lo}~{hi} 小时之间（最长 90 天）"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ 这条测的不是"默认值是多少"，而是"**默认值等于那两个老常量**"。
    /// 它俩一旦漂移，所有没改过设置的实例行为都会变，而现象是"莫名其妙地不一样"。
    #[test]
    fn defaults_match_the_old_constants() {
        let d = AppSettings::default();
        assert_eq!(d.min_password_len, DEFAULT_MIN_PASSWORD_LEN);
        assert_eq!(d.session_ttl_ms(), crate::WEB_SESSION_TTL_MS);
    }

    #[test]
    fn validate_rejects_out_of_range() {
        let ok = AppSettings::default();
        assert!(ok.validate().is_ok());

        for bad in [3usize, 65] {
            let s = AppSettings {
                min_password_len: bad,
                ..ok
            };
            assert!(s.validate().is_err(), "下限 {bad} 应被拒");
        }
        for bad in [0u32, 24 * 91] {
            let s = AppSettings {
                session_ttl_hours: bad,
                ..ok
            };
            assert!(s.validate().is_err(), "会话 {bad} 小时应被拒");
        }
        // 边界必须放行（不然"最多 90 天"这句就是假的）
        assert!(AppSettings {
            min_password_len: 4,
            session_ttl_hours: 24 * 90
        }
        .validate()
        .is_ok());
    }

    /// 老文件缺字段要能读出来 —— 这是"加设置项不让老实例失效"的保证。
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AppSettings::default());
        let part: AppSettings = serde_json::from_str(r#"{"min_password_len":12}"#).unwrap();
        assert_eq!(part.min_password_len, 12);
        assert_eq!(part.session_ttl_hours, DEFAULT_SESSION_TTL_HOURS);
    }
}
