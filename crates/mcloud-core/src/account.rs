//! 账号与凭证模型。

use serde::{Deserialize, Serialize};

/// 账号标识，通常是手机号。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(pub String);

impl AccountId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 设备标识（smid）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 解析后的凭证。
///
/// 原始形式是 `base64(platform:phone:token)`，`token` 内部以 `|` 分段，
/// 第 4 段是过期时间（秒级时间戳）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthInfo {
    /// 原始 base64，写回配置时使用，保证不被我们改写形态。
    pub raw: String,
    pub platform: String,
    pub phone: String,
    pub token: String,
    /// 过期时间（秒级 unix 时间戳）。解析不出时为 0。
    pub expire: i64,
}

impl AuthInfo {
    /// 距过期还有多少天。`expire` 为 0（未知）时返回 `None`。
    pub fn days_until_expiry(&self, now_secs: i64) -> Option<i64> {
        if self.expire <= 0 {
            return None;
        }
        Some((self.expire - now_secs) / 86_400)
    }

    /// 是否需要在剩余天数低于阈值时刷新。
    pub fn needs_refresh(&self, now_secs: i64, threshold_days: i64) -> bool {
        match self.days_until_expiry(now_secs) {
            Some(d) => d < threshold_days,
            // 过期时间未知时保守地不刷新，避免无谓请求；由上层决定。
            None => false,
        }
    }
}

/// 账号级功能开关与参数。
///
/// 各活动默认值遵循「保守开启」：能稳定跑通的默认开，涉及额外消耗或
/// 易触发风控的默认关。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountSettings {
    /// 备份任务等待时间（秒）。
    pub backup_wait_secs: u64,
    /// 剩余多少天时刷新凭证。
    pub refresh_token_days: i64,
    /// 跳过的任务名。
    pub skip_tasks: Vec<String>,

    /// AI 新头像。
    pub ai_avatar_enabled: bool,
    pub ai_avatar_daily_limit: u32,

    /// 红包派对。
    pub redpack_enabled: bool,

    /// 云朵大作战。
    pub hc1t_enabled: bool,
    pub hc1t_game_secs: u64,
    pub hc1t_target_rank: u32,

    /// 春日拍拍大作战。
    pub play_ai_enabled: bool,

    /// 直播口令。
    pub kouling_enabled: bool,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            backup_wait_secs: 20,
            refresh_token_days: 10,
            skip_tasks: Vec::new(),
            ai_avatar_enabled: false,
            ai_avatar_daily_limit: 10,
            redpack_enabled: true,
            hc1t_enabled: false,
            hc1t_game_secs: 300,
            hc1t_target_rank: 500,
            play_ai_enabled: true,
            kouling_enabled: false,
        }
    }
}

/// 一个受管账号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub nickname: Option<String>,
    pub auth: AuthInfo,
    pub device_id: DeviceId,
    #[serde(default)]
    pub settings: AccountSettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth_with_expiry(expire: i64) -> AuthInfo {
        AuthInfo {
            raw: "x".into(),
            platform: "android".into(),
            phone: "13800000000".into(),
            token: "t".into(),
            expire,
        }
    }

    #[test]
    fn days_until_expiry_is_floor_division() {
        let a = auth_with_expiry(1_000_000_000 + 86_400 * 3 + 10);
        assert_eq!(a.days_until_expiry(1_000_000_000), Some(3));
    }

    #[test]
    fn unknown_expiry_reports_none_and_no_refresh() {
        let a = auth_with_expiry(0);
        assert_eq!(a.days_until_expiry(1_000_000_000), None);
        // 未知过期时间不主动刷新
        assert!(!a.needs_refresh(1_000_000_000, 10));
    }

    #[test]
    fn refresh_threshold_is_exclusive() {
        let now = 1_000_000_000;
        // 恰好 10 天：不刷新（阈值是「低于」）
        assert!(!auth_with_expiry(now + 86_400 * 10).needs_refresh(now, 10));
        // 9 天：刷新
        assert!(auth_with_expiry(now + 86_400 * 9).needs_refresh(now, 10));
    }

    #[test]
    fn default_settings_are_conservative() {
        let s = AccountSettings::default();
        assert!(!s.ai_avatar_enabled);
        assert!(!s.hc1t_enabled);
        assert!(!s.kouling_enabled);
        assert_eq!(s.refresh_token_days, 10);
    }
}
