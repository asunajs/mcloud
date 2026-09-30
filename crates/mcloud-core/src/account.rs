//! 账号与凭证模型。

use serde::{Deserialize, Serialize};

use crate::push::PushConfig;
use crate::user::UserId;

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
    /// 跳过的任务 **id**（数字）。旧 TS 的 `ctx.config.tasks.skipTasks` 就是 `number[]`，
    /// 判据是 `skipTasks?.includes(task.id)`（`mcloud-sign` 的
    /// `packages/core/src/tasks/app/index.ts:33`）。
    ///
    /// # ⚠ 形状变更与旧落盘值
    ///
    /// 本字段**落过盘**：旧形状是 `Vec<String>`（字段注释原文「跳过的任务名」），
    /// 所以 `accounts.json` 里可能还留着 `["签到", "看视频"]` 这种**任务名**数组。
    /// 任务名与任务 id 之间**没有可推导的对应关系**，无法迁移 ⇒ 走
    /// [`de_skip_task_ids`] **宽松读**：
    ///
    /// * 数字 ⇒ 收；
    /// * 数字字符串（`"117"`）⇒ 解析成数字收；
    /// * 别的一切（任务名、对象、`null`）⇒ **丢弃这一项**；
    /// * 整个字段不是数组 ⇒ 当**空表**（不报错、不 panic）。
    ///
    /// ⚠ 丢弃是**有意的**：把旧任务名原样留着，只会让「跳过名单」看起来配了东西
    /// 却一条都不生效（`includes` 恒 false）。代价是旧值不再出现在界面上 ——
    /// 由测试 `skip_tasks_accepts_the_old_shape` 钉住这一行为。
    #[serde(default, deserialize_with = "de_skip_task_ids")]
    pub skip_tasks: Vec<i64>,

    /// AI 新头像。
    pub ai_avatar_enabled: bool,
    pub ai_avatar_daily_limit: u32,

    /// 红包派对。
    pub redpack_enabled: bool,

    /// AI豆大作战。
    pub hc1t_enabled: bool,
    pub hc1t_game_secs: u64,
    pub hc1t_target_rank: u32,

    /// 春日拍拍大作战。
    pub play_ai_enabled: bool,

    /// 直播口令。
    pub kouling_enabled: bool,

    /// 推送渠道：这个账号的运行结果推到**哪些**渠道（每账号一份）。
    ///
    /// # 为什么是**每账号**而不是进程级
    ///
    /// 「结果发给谁」是账号自己的属性：同一台机器上可以既有归本人的账号、
    /// 也有代跑的账号，它们不该被迫共用一个收件地址；同一进程里 A、B 两个
    /// 账号也必须能各配各的渠道。而进程级 env 只能有一份 —— 那是**串号**：
    /// A 的渠道会被套到 B 的运行上（见 `mcloud-core::push` 模块头）。
    ///
    /// ⇒ **本仓刻意不做进程级兜底**：那种「一份 env 管全进程」的配置已整个删除。
    /// `None`（默认）⇒ 这个账号没配渠道 ⇒ **一个包都不发**（无 fallback）。
    ///
    /// ⚠ 与 TS 的差异：`mcloud-sign`（单用户）有一份全局配置，那是为了 DRY；
    /// 本仓多用户 ⇒ 收件地址只能落在**账号维度**。TS 那种「一份配置套多个
    /// 账号」的正确形态是**用户级模板**，本版**没做**（见 `mcloud-core::push`
    /// 的未做项），绝不把它偷偷实现成 fallback。
    ///
    /// ⚠ `skip_serializing_if` 是刻意的：`None` 不该落盘成一个显式的
    /// `null`，否则"没配"与"配成空"在老数据里分不开。
    ///
    /// ⚠ 半成品字段 `notify_webhook`（单个 webhook 串）不再被读取：serde 默认
    /// 忽略未知键，旧值会被**静默丢弃** —— 那是刻意的，那份模型语义错误
    /// （进程级兜底 = 串号），不做兼容迁移。要配就按 `PushConfig` 重配。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<PushConfig>,
}

/// [`AccountSettings::skip_tasks`] 的宽松反序列化。
///
/// # 为什么不直接 `Vec<i64>`
///
/// 这个字段**落过盘**，旧形状是 `Vec<String>`（任务名）。直接换成 `Vec<i64>`
/// 会让带旧值的 `accounts.json` **整份读不出来** —— 一个历史字段不该有那种破坏力。
///
/// 读法（每一项独立判，坏的丢掉、不报错）：
/// * 数字 ⇒ 收；
/// * 字符串 ⇒ 能解析成 `i64` 就收（例如 `"117"`），否则丢；
/// * 其余（对象 / 数组 / `null` / 布尔）⇒ 丢。
///
/// 整个字段不是数组（或整份读不成 JSON）⇒ 回**空表**：`skip_tasks` 是"用户
/// 明确点名要跳的"，读不出来时"一条都不跳"比"随便跳几条"安全。
fn de_skip_task_ids<'de, D>(de: D) -> std::result::Result<Vec<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: serde_json::Value =
        serde::Deserialize::deserialize(de).unwrap_or(serde_json::Value::Null);
    let serde_json::Value::Array(items) = raw else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| match item {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
            _ => None,
        })
        .collect())
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
            // 默认不配推送渠道：与"未配置就零行为"同一条 —— 装完不该有包发出去。
            notify: None,
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

    /// **归属**：这个账号属于哪个系统用户（WebUI 登录闸里的那个）。
    ///
    /// # 为什么是 Option
    ///
    /// 这一列是**后加的**：老 `accounts.json` 里没有它。直接写 `UserId`
    /// 会让老库整份读不出来 —— 而老数据必须照常可用。所以：
    ///
    /// * 读不到 ⇒ `None`（未归属）；
    /// * 加载时由存储层**回填**给那个唯一的管理员（解析见
    ///   `mcloud_core::owner::legacy_owner`）；
    /// * 新建的账号由路由层写成当前登录用户。
    ///
    /// `skip_serializing_if` 是刻意的：归属一旦确定就落盘，未归属时不写
    /// 一个显式的 `null` —— 否则「从没归属过」与「归属性被清空」在 JSON 里
    /// 长得一样，回填逻辑就没法判了。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserId>,

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
        assert_eq!(s.notify, None, "默认不该给任何账号配推送渠道");
    }

    /// 推送渠道落盘：配了就留得住，没配就不占格子。
    ///
    /// 这条是"每账号"的回归 —— 若把它做成进程级 env，这一列就不存在，
    /// 两个账号的 JSON 也分不出来谁该收到通知。
    #[test]
    fn notify_config_is_per_account_and_absent_when_unset() {
        let mut s = AccountSettings::default();
        let without = serde_json::to_string(&s).unwrap();
        assert!(
            !without.contains("notify"),
            "没配时不该写进 JSON：{without}"
        );

        s.notify = Some(serde_json::from_str(r#"{"serverChan":{"token":"tk"}}"#).unwrap());
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains(r#""notify":"#) && json.contains(r#""serverChan":{"token":"tk"}"#),
            "实得：{json}"
        );
        let back: AccountSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.notify, s.notify);
    }

    /// 旧半成品字段 `notify_webhook` 不再被读取（未知键忽略）。
    ///
    /// 这是"没有兜底、也不迁移错误模型"的回归：那个字段代表的是
    /// 「进程级兜底」，正是串号的来源，绝不让它复活成任何推送配置。
    #[test]
    fn legacy_notify_webhook_is_ignored() {
        let s: AccountSettings =
            serde_json::from_str(r#"{"notify_webhook":"https://old.test/hook"}"#).unwrap();
        assert_eq!(s.notify, None, "旧字段不许复活成推送配置");
    }

    /// ⚠ `skip_tasks` 形状变更的兼容回归：旧落盘值是**任务名**数组。
    ///
    /// 旧字段的语义是「跳过的任务名」（`Vec<String>`），新语义是任务 id
    /// （与旧 TS 的 `skipTasks: number[]` 对齐）。两者无对应关系 ⇒ 旧任务名
    /// 会被**丢弃**（不是崩、也不是让整份 `accounts.json` 读不出来）。
    /// 这条测试把「丢什么、留什么」写死，免得将来有人改成"读不出来就 panic"。
    #[test]
    fn skip_tasks_accepts_the_old_shape() {
        // 旧形状：任务名数组 —— 一个都不该留下，也不该让解析失败。
        let s: AccountSettings =
            serde_json::from_str(r#"{"skip_tasks":["签到","看视频"]}"#).unwrap();
        assert!(s.skip_tasks.is_empty(), "旧任务名按 id 用不出来，只能丢");

        // 新形状：数字数组原样收。
        let s: AccountSettings = serde_json::from_str(r#"{"skip_tasks":[117,437]}"#).unwrap();
        assert_eq!(s.skip_tasks, vec![117, 437]);

        // 混着来：数字字符串收下，杂项丢掉。
        let s: AccountSettings =
            serde_json::from_str(r#"{"skip_tasks":[117,"991",null,{"a":1},"不是数字"]}"#).unwrap();
        assert_eq!(s.skip_tasks, vec![117, 991]);

        // 整个字段形状不对 ⇒ 空表，不报错。
        let s: AccountSettings = serde_json::from_str(r#"{"skip_tasks":"117"}"#).unwrap();
        assert!(s.skip_tasks.is_empty());
        let s: AccountSettings = serde_json::from_str(r#"{"skip_tasks":{"id":117}}"#).unwrap();
        assert!(s.skip_tasks.is_empty());

        // 字段整个缺席 ⇒ 默认空表。
        let s: AccountSettings = serde_json::from_str(r#"{}"#).unwrap();
        assert!(s.skip_tasks.is_empty());
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
            owner: None,
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
            owner: None,
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
