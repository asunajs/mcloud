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

/// PC 客户端平台形态。
///
/// # 为什么它是一个**持久化**的领域概念
///
/// 上游把 Windows 与 macOS 当成**两个不同的客户端**：`clientType`、`cpid`、
/// 版本号、渠道号、HTTP UA 全都不一样（客户端源码 `apiServer.js:10-12` 里
/// 用 `navigator.userAgent` 含不含 `mac` 二选一）。
///
/// 而**凭据本身分不出来**：PC 端 `Authorization` 的前缀两种平台都是 `pc`，
/// 光看 `accounts.json` 里的 auth 串，mac 与 win 长得一模一样。
/// 上游参考实现（`mcloud`）就卡在这里 —— 它的注释明说
/// 「库里没记 windows 还是 macos，只能由调用方拍板」，于是刷新凭据时
/// 只能靠人再选一次平台，选错就是"用一个平台的身份去续另一个平台的会话"。
///
/// 所以这个形态必须**随账号落盘**：它决定后续每一次请求的 UA 与头集合，
/// 不是一次性的展示字段。放在 `mcloud-core` 而不是 `mcloud-upstream`，
/// 是因为它要被 `mcloud-store` 序列化，而 store 与 upstream 是**同层兄弟**，
/// 让 core 依赖 upstream 会违反 §1 的分层铁律。
/// 具体的线上取值（`clientType` 等）由 `mcloud-upstream` 的 `impl` 提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PcPlatform {
    /// Windows 客户端。
    Windows,
    /// macOS 客户端。
    MacOs,
}

impl PcPlatform {
    /// 落盘 / 展示用的稳定标识。
    ///
    /// 与 serde 的表示保持一致，避免"日志里一种写法、JSON 里另一种"。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "macos",
        }
    }

    /// 从落盘标识解析。未知值返回 `None`，由调用方决定是报错还是忽略。
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "windows" | "win" | "win32" => Some(Self::Windows),
            "macos" | "mac" | "darwin" | "osx" => Some(Self::MacOs),
            _ => None,
        }
    }
}

impl std::fmt::Display for PcPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 解析后的凭证。
///
/// 原始形式是 `base64(platform:phone:token)`，`token` 内部以 `|` 分段，
/// 第 4 段是过期时间（**毫秒**时间戳）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthInfo {
    /// 原始 base64，写回配置时使用，保证不被我们改写形态。
    pub raw: String,
    pub platform: String,
    pub phone: String,
    pub token: String,
    /// 过期时间（**epoch 毫秒**）。解析不出时为 0。
    ///
    /// # ⚠ 单位是毫秒，不是秒
    ///
    /// 这一度写错过：本字段原先标注"秒级"并把差值除以 `86_400`。
    /// 实测（`mcloud/test/golden/auth-vectors.json`，以及真实
    /// `asign.json` 里的 `1893456000000`）token 第 4 段是**13 位毫秒**，
    /// 旧项目也直接与 `Date.now()` 相减：
    /// `isNeedRefresh()` 的判据是 `expire - Date.now() < days * 86400000`。
    ///
    /// 按秒解释的后果**不是报错而是静默算错**：`1893456000000` 会被当成
    /// 公元 61968 年，"永远不需要刷新" —— 直到上游回 90001 才发现。
    /// 所以访问器一律把单位写进名字。
    pub expire: i64,
}

impl AuthInfo {
    /// 距过期还有多少天（向下取整）。`expire` 为 0（未知）时返回 `None`。
    ///
    /// `now_ms` 是 **epoch 毫秒**，与 [`Self::expire`] 同单位。
    pub fn days_until_expiry(&self, now_ms: i64) -> Option<i64> {
        if self.expire <= 0 {
            return None;
        }
        Some((self.expire - now_ms) / 86_400_000)
    }

    /// 是否需要在剩余天数低于阈值时刷新。
    ///
    /// `now_ms` 是 **epoch 毫秒**。
    pub fn needs_refresh(&self, now_ms: i64, threshold_days: i64) -> bool {
        match self.days_until_expiry(now_ms) {
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
    /// **登录时冒充的 PC 客户端形态**（Windows / macOS）。
    ///
    /// # 为什么必须落盘
    ///
    /// 它决定后续每一次请求的 `clientType` / `cpid` / `version` / UA / 渠道号。
    /// 而凭据本身**分不出来**：PC 端两种平台的 `Authorization` 前缀都是 `pc`。
    /// 不存这一列，刷新凭据时就只能让调用方再猜一次平台 ——
    /// 上游参考实现（`mcloud`）的注释把代价说得很清楚：
    /// 「库里没记 windows 还是 macos，只能由调用方拍板」。
    ///
    /// `None` 表示**不是 PC 端登录的账号**（App / 历史数据）。
    /// 刻意不用"默认 Windows"来兜底：猜一个的表现是
    /// 「用 Windows 的身份去续 macOS 的会话」，而上游只会回一个
    /// 含义模糊的业务码，排查时很难想到是平台错了。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc_platform: Option<PcPlatform>,

    /// 上游的用户域标识（19 位数字串）。
    ///
    /// # 为什么必须落盘
    ///
    /// 刷新凭据（`POST /user/auth/refreshToken`）的请求体**必须有**它：
    /// 它说"刷哪个用户域里的那个 token"。而它**无法从凭据里反推** ——
    /// 上游参考实现（`mcloud`）对此的注释是：
    /// 「库里没有用户域就直接判'这个账号刷不了'，编一个会把刷新打到
    /// 别人的用户域上」。
    ///
    /// 登录响应里有它（`userInfo.userDomainId`，与请求头 `x-yun-uni` 同值）。
    /// `None` 表示这个账号是**在本字段存在之前**登录的（历史数据）——
    /// 那时刷新会明确报"缺用户域，请重新登录"，而不是编一个去撞。
    ///
    /// ⚠ 它是**纯标识，不是凭据**（所以 `Debug` 不脱敏，排障时要看得见
    /// 刷的是哪个主体）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_domain_id: Option<String>,

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
        const DAY_MS: i64 = 86_400_000;
        let a = auth_with_expiry(1_700_000_000_000 + DAY_MS * 3 + 10);
        assert_eq!(a.days_until_expiry(1_700_000_000_000), Some(3));
    }

    /// 单位回归：`expire` 是**毫秒**。
    ///
    /// 这条用例的存在理由是本字段真的写错过（标注"秒级"、除以 `86_400`）。
    /// 用真实量级的 13 位时间戳断言，按秒解释会得到约 22000 天 —— 差 1000 倍。
    #[test]
    fn expire_is_milliseconds_not_seconds() {
        const DAY_MS: i64 = 86_400_000;
        let now = 1_893_456_000_000; // 与黄金向量同量级（2030-01-01 前后）
        let a = auth_with_expiry(now + DAY_MS * 10);
        assert_eq!(
            a.days_until_expiry(now),
            Some(10),
            "10 天后到期必须算出 10 天；按秒解释会得到约 22000 天"
        );
        // 真实量级的 13 位时间戳不可能被误当成 10 天以内的秒值
        assert!(
            a.days_until_expiry(now).unwrap() < 365,
            "真实时间戳的剩余天数不该是几千天"
        );
    }

    #[test]
    fn unknown_expiry_reports_none_and_no_refresh() {
        let a = auth_with_expiry(0);
        assert_eq!(a.days_until_expiry(1_700_000_000_000), None);
        // 未知过期时间不主动刷新
        assert!(!a.needs_refresh(1_700_000_000_000, 10));
    }

    #[test]
    fn refresh_threshold_is_exclusive() {
        const DAY_MS: i64 = 86_400_000;
        let now = 1_700_000_000_000;
        // 恰好 10 天：不刷新（阈值是「低于」）
        assert!(!auth_with_expiry(now + DAY_MS * 10).needs_refresh(now, 10));
        // 9 天：刷新
        assert!(auth_with_expiry(now + DAY_MS * 9).needs_refresh(now, 10));
    }

    #[test]
    fn default_settings_are_conservative() {
        let s = AccountSettings::default();
        assert!(!s.ai_avatar_enabled);
        assert!(!s.hc1t_enabled);
        assert!(!s.kouling_enabled);
        assert_eq!(s.refresh_token_days, 10);
    }

    // ── 平台形态 ────────────────────────────────────────────────────────

    #[test]
    fn pc_platform_round_trips_through_json() {
        for p in [PcPlatform::Windows, PcPlatform::MacOs] {
            let s = serde_json::to_string(&p).unwrap();
            assert_eq!(s, format!("\"{}\"", p.as_str()));
            assert_eq!(serde_json::from_str::<PcPlatform>(&s).unwrap(), p);
            assert_eq!(PcPlatform::parse(p.as_str()), Some(p));
        }
    }

    /// **两种平台落盘必须不同** —— 这正是「底层区分」的含义。
    ///
    /// 若把平台做成"只看 UA 的运行时开关"，这一列就不存在，
    /// mac 账号与 win 账号的 JSON 会一模一样，重启即丢失。
    #[test]
    fn mac_and_windows_persist_differently() {
        let mk = |p| Account {
            id: AccountId::new("19900000001"),
            nickname: None,
            auth: auth_with_expiry(0),
            device_id: DeviceId("D-ENDIN".into()),
            pc_platform: Some(p),
            user_domain_id: None,
            settings: AccountSettings::default(),
        };
        let win = serde_json::to_string(&mk(PcPlatform::Windows)).unwrap();
        let mac = serde_json::to_string(&mk(PcPlatform::MacOs)).unwrap();
        assert_ne!(win, mac, "mac 与 win 的落盘必须可区分");
        assert!(win.contains(r#""pc_platform":"windows""#), "实得：{win}");
        assert!(mac.contains(r#""pc_platform":"macos""#), "实得：{mac}");
    }

    /// 非 PC 账号（App / 历史数据）不写这一列，且能读回 `None`。
    #[test]
    fn absent_platform_is_omitted_and_reads_back_as_none() {
        let acc = Account {
            id: AccountId::new("19900000001"),
            nickname: None,
            auth: auth_with_expiry(0),
            device_id: DeviceId("d".into()),
            pc_platform: None,
            user_domain_id: None,
            settings: AccountSettings::default(),
        };
        let json = serde_json::to_string(&acc).unwrap();
        assert!(
            !json.contains("pc_platform"),
            "None 不该写进 JSON（否则老数据会被塞一个假平台）：{json}"
        );
        let back: Account = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pc_platform, None);
    }

    #[test]
    fn pc_platform_parse_accepts_aliases_and_rejects_junk() {
        assert_eq!(PcPlatform::parse("Windows"), Some(PcPlatform::Windows));
        assert_eq!(PcPlatform::parse("MAC"), Some(PcPlatform::MacOs));
        assert_eq!(PcPlatform::parse("darwin"), Some(PcPlatform::MacOs));
        assert_eq!(
            PcPlatform::parse("linux"),
            None,
            "未知平台不能静默落成 Windows"
        );
    }
}
