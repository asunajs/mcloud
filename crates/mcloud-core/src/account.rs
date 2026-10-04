//! 账号与凭证模型。

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
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

    /// AI 新头像**每天最多几轮**。
    ///
    /// ⚠ 这里只留"上限"，**没有** `ai_avatar_enabled` 那个总闸。
    pub ai_avatar_daily_limit: u32,

    // 这里**没有**三个"账号级总闸"：`ai_avatar_enabled` / `redpack_enabled` /
    // `play_ai_enabled`（以及更早删掉的 `kouling_enabled`）。用户口径（2026-10-03）：
    // 「**账号不应该有这个配置**」—— 任务**是用户显式点起来的**，"跑不跑"这一层已经
    // 由那个动作表达了；再叠一个藏在账号设置里的总闸，只会让显式发起的运行立刻回一句
    // "未启用"，而界面上**根本没有入口**能把它打开（那些开关所在的整块被
    // `SHOW_ACCOUNT_SETTINGS_BLOCKS` 关着）。留着的字段就是骗人的旋钮。
    //
    // ⚠ 留下的 `ai_avatar_daily_limit` **不属于**这一类：它是"每天最多几轮"的**上限**，
    // 不是"参不参加"的开关 —— 删了会真丢能力。

    // 这里**没有** `hc1t_*` 三个旋钮：`hc1t`（AI豆大作战）已被判「已过期、
    // 不实现」（`docs/task-inventory.md` 的 §P4 与差异表 D3），本仓也没有对应
    // 任务 —— 留着就是骗人的旋钮。将来真要实现时，随任务一起把字段加回来。

    // 这里**没有** `kouling_enabled`：用户口径（2026-10-03）「**账号不应该有这个配置**」。
    // 它原本是「直播口令·开启」这个账号级总闸 —— 关着时 `live-room` **一个请求都不发**、
    // 结论 `Skipped`（照 TS 的 `if (config.直播口令?.开启)` 搬来的）。问题是：任务
    // **是用户显式点起来的**，"跑不跑"这一层已经由那个动作表达了；再叠一个藏在账号
    // 设置里（且界面上根本没有入口 ✗）的总闸，只会让显式发起的运行在 1ms 内回一句
    // 「直播口令未启用」，而用户找不到任何地方把它打开。留着的字段就是骗人的旋钮 ——
    // 与上面 `hc1t_*` 那三个是同一条道理。
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
            ai_avatar_daily_limit: 10,
            // 默认不配推送渠道：与"未配置就零行为"同一条 —— 装完不该有包发出去。
            notify: None,
        }
    }
}

// ── 设置的**线格式**（GET/PUT /api/accounts/{id}/settings）──────────────

/// 账号设置在 HTTP 上的形状：`GET` 的响应体，也是 `PUT` 的请求体。
///
/// # 为什么不直接复用 [`AccountSettings`] 的 serde
///
/// 两者服务的目的**相反**，所以形状必须分开：
///
/// | | 落盘（[`AccountSettings`]） | HTTP（本类型） |
/// |---|---|---|
/// | 键名 | `snake_case`（`backup_wait_secs`）| `camelCase`（`backupWaitSecs`，与 `/api/schedules`、`/api/runs` 一致）|
/// | 未知键 | **忽略**（老库里残留的键不该让整份读不出来）| **400**（敲错键要当场知道）|
/// | 坏元素 | **丢弃**（`skip_tasks` 的旧形状见 [`de_skip_task_ids`]）| **400** |
///
/// 落盘那条宽松是**读兼容**的需要，不是"怎么写都行"。写入口照抄它的话，
/// 表现就是「用户把 `skipTasks` 敲成 `skipTask` ⇒ 一声不响地当没配」——
/// 而本仓最忌讳的就是"不报错、只是东西不对"。所以写入口另有一套严格形状。
///
/// # 为什么每个字段都必填
///
/// `PUT` 是**整体替换**：缺席的字段没有"保持不变"的意思。若给缺席字段兜一个
/// 默认值，表现就是「少传一个键 ⇒ 那项设置被悄悄重置」——那是最难查的一类失效。
/// 必填 ⇒ 少传就 400 并指名。`notify` 也必须显式给出（`null` = 不配推送）：
/// JSON 里"缺席"与"null"不是一回事，而"清空推送"在这个接口上必须能表达。
///
/// ⚠ 与落盘模型一一对应由**编译器**保证：两个方向的 `From`/`TryFrom` 都逐字段
/// 列举，[`AccountSettings`] 加字段而这里不加就编不过。键名的对应另有测试
/// `the_wire_keys_match_the_stored_keys` 钉住。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSettingsDto {
    /// 备份任务等待时间（秒）。`0` = 不等（既有语义，见 `backup_wait_ms`）。
    pub backup_wait_secs: u64,
    /// 剩余多少天时刷新凭证。
    pub refresh_token_days: i64,
    /// 跳过的任务 **id**（数字）。见 [`AccountSettings::skip_tasks`]。
    pub skip_tasks: Vec<i64>,
    pub ai_avatar_daily_limit: u32,
    /// 推送渠道。`null` = 不配（与落盘的 `None` 同义）。
    pub notify: Option<PushConfig>,
}

impl AccountSettingsDto {
    /// `backup_wait_secs` 的上限（秒）。
    ///
    /// 它等的是"服务端把刚发出的一次备份事件记进当月"，分钟级已经过分；
    /// 一小时以上一定是笔误，而代价是任务一直挂着不结束。
    pub const BACKUP_WAIT_SECS_MAX: u64 = 3_600;
    /// `refresh_token_days` 的上限（天）。
    ///
    /// 上限取得很宽（十年）是刻意的：它只用来钉住"显然不是天数"的输入
    ///（负数、六位数），不去规定用户该多激进地刷新。
    pub const REFRESH_TOKEN_DAYS_MAX: i64 = 3_650;
    /// `ai_avatar_daily_limit` 的上限（轮）。每轮都是一次上游调用。
    pub const AI_AVATAR_DAILY_LIMIT_MAX: u32 = 1_000;
    /// `skip_tasks` 的条数上限。
    pub const SKIP_TASKS_MAX: usize = 1_000;

    /// 范围与结构校验（**纯函数**，不碰存储 —— 所以能在 core 层测）。
    ///
    /// ⚠ 错误信息只说**字段名与规则**，绝不带字段值：`notify` 里有 token/secret，
    /// 而这些字符串会进 HTTP 响应，也可能被打进日志。
    pub fn validate(&self) -> Result<()> {
        if self.backup_wait_secs > Self::BACKUP_WAIT_SECS_MAX {
            return Err(Error::Config(format!(
                "backupWaitSecs 最多 {} 秒（实得 {}）：再长就是任务一直挂着不结束",
                Self::BACKUP_WAIT_SECS_MAX,
                self.backup_wait_secs
            )));
        }
        if !(0..=Self::REFRESH_TOKEN_DAYS_MAX).contains(&self.refresh_token_days) {
            return Err(Error::Config(format!(
                "refreshTokenDays 必须在 0..={} 之间（实得 {}）：负数会让「剩余天数低于阈值才刷新」永远算错",
                Self::REFRESH_TOKEN_DAYS_MAX,
                self.refresh_token_days
            )));
        }
        if self.ai_avatar_daily_limit > Self::AI_AVATAR_DAILY_LIMIT_MAX {
            return Err(Error::Config(format!(
                "aiAvatarDailyLimit 最多 {} 轮（实得 {}）：每一轮都是一次上游调用",
                Self::AI_AVATAR_DAILY_LIMIT_MAX,
                self.ai_avatar_daily_limit
            )));
        }
        if self.skip_tasks.len() > Self::SKIP_TASKS_MAX {
            return Err(Error::Config(format!(
                "skipTasks 最多 {} 条（实得 {}）",
                Self::SKIP_TASKS_MAX,
                self.skip_tasks.len()
            )));
        }
        if let Some(notify) = &self.notify {
            validate_notify(notify)?;
        }
        Ok(())
    }

    /// 从 HTTP 请求体解析并校验，得到可落盘的 [`AccountSettings`]。
    ///
    /// 三步，顺序是刻意的：
    ///
    /// 1. 最外层必须是 JSON **对象**；
    /// 2. `notify` 的**顶层键**必须是认识的渠道 —— [`PushConfig`] 自己会静默
    ///    忽略未知键（`dingTalk` 敲成 `dingtalk` 就被吞掉、一声不响），
    ///    而一个不生效的渠道比不写更难查，所以这里宁可 400；
    /// 3. 交给本类型的严格 serde（未知键 / 缺字段 / 类型不对都 400），
    ///    再过 [`Self::validate`] 的范围与渠道结构校验。
    ///
    /// ⚠ `notify` 里**渠道内部**的未知键**不**报错，这是刻意的：`TokenConfig.extra`
    /// 这类 `#[serde(flatten)]` 是 TS 的既定语义（多余字段原样展开进请求体），
    /// 不是笔误。那层无法区分"有意的附加字段"与"敲错的键名"，所以不做检查。
    pub fn from_wire(raw: serde_json::Value) -> Result<AccountSettings> {
        if !raw.is_object() {
            return Err(Error::Config("请求体必须是一个 JSON 对象".into()));
        }
        if let Some(notify) = raw.get("notify") {
            if !notify.is_null() {
                let obj = notify
                    .as_object()
                    .ok_or_else(|| Error::Config("notify 必须是对象或 null".into()))?;
                if let Some(known) = known_notify_keys() {
                    for key in obj.keys() {
                        if !known.iter().any(|k| k == key) {
                            return Err(Error::Config(format!(
                                "notify 里不认识的渠道「{key}」；认识的是：{}",
                                known.join("、")
                            )));
                        }
                    }
                }
            }
        }
        // serde 的错误文本可能引用**用户自己刚提交的**那个越界值（例如把字符串
        // 传给了数字字段）。它只回给提交者本人，且本路由不把请求体写进日志 ——
        // 与"错误信息不许带出**别的**字段"（见 validate）是两条不同的边界。
        let dto: AccountSettingsDto =
            serde_json::from_value(raw).map_err(|e| Error::Config(format!("设置格式不对：{e}")))?;
        AccountSettings::try_from(dto)
    }
}

/// [`PushConfig`] 认识的顶层键（含渠道名与 `title` / `onlyError` / `minLevel`）。
///
/// 不手抄一份：直接问 `PushConfig::default()` 的序列化结果，加渠道时它自己会跟上。
/// 序列化失败时回 `None`，调用方**跳过**这道检查（它是防笔误的，不是安全边界）。
fn known_notify_keys() -> Option<Vec<String>> {
    match serde_json::to_value(PushConfig::default()) {
        Ok(serde_json::Value::Object(map)) => Some(map.keys().cloned().collect()),
        _ => None,
    }
}

/// 推送渠道的**结构**校验。
///
/// 逐条对着发送侧真正会用到的字段：缺一个的后果是"配了等于没配"——
/// `mcloud-app::push` 的 `build_*` 会 `return None` / 整条跳过，
/// 表现是**一条都不发、也不报错**。
///
/// ⚠ 只报**字段名**，不报字段值（这些字段里有 token/secret）。
fn validate_notify(notify: &PushConfig) -> Result<()> {
    let mut any = false;

    if let Some(c) = &notify.pushplus {
        any = true;
        require_non_empty("notify.pushplus.token", &c.token)?;
    }
    if let Some(c) = &notify.server_chan {
        any = true;
        require_non_empty("notify.serverChan.token", &c.token)?;
    }
    if let Some(c) = &notify.work_weixin {
        any = true;
        // 应用消息是两步：先 corpid+corpsecret 换 access_token，再带 agentid 发。
        // 缺任何一个都到不了"发出去了"那一步。
        match c.corpid.as_deref() {
            Some(v) if !v.trim().is_empty() => {}
            _ => {
                return Err(Error::Config(
                    "notify.workWeixin.corpid 缺失：取 access_token 要用它".into(),
                ))
            }
        }
        match c.corpsecret.as_deref() {
            Some(v) if !v.trim().is_empty() => {}
            _ => {
                return Err(Error::Config(
                    "notify.workWeixin.corpsecret 缺失：取 access_token 要用它".into(),
                ))
            }
        }
        if c.agentid.is_none() {
            return Err(Error::Config(
                "notify.workWeixin.agentid 缺失：发应用消息要用它".into(),
            ));
        }
    }
    if let Some(c) = &notify.work_weixin_bot {
        any = true;
        require_non_empty("notify.workWeixinBot.url", &c.url)?;
    }
    if let Some(c) = &notify.tg_bot {
        any = true;
        require_non_empty("notify.tgBot.token", &c.token)?;
        // 判据照发送侧（`build_tg_bot` 的 JS 真值口径）：`0` / `""` / `null` 都算缺，
        // 缺了整条跳过 —— 那是"配了不发"的典型长相。
        match c.chat_id.as_ref() {
            Some(v) if js_truthy(v) => {}
            _ => {
                return Err(Error::Config(
                    "notify.tgBot.chat_id 缺失或为空：没有它发送侧会整条跳过".into(),
                ))
            }
        }
    }
    if let Some(c) = &notify.bark {
        any = true;
        require_non_empty("notify.bark.key", &c.key)?;
    }
    if let Some(c) = &notify.two_im {
        any = true;
        require_non_empty("notify.twoIm.key", &c.key)?;
        require_non_empty("notify.twoIm.sid", &c.sid)?;
    }
    if let Some(c) = &notify.ding_talk {
        any = true;
        require_non_empty("notify.dingTalk.token", &c.token)?;
    }
    if let Some(c) = &notify.email {
        any = true;
        // 本版不发邮件（见 `mcloud-app::push::EMAIL_NOT_IMPLEMENTED`），
        // 但配了就要配全 —— 一份缺字段的配置只会让人以为它生效了。
        require_non_empty("notify.email.pass", &c.pass)?;
        require_non_empty("notify.email.from", &c.from)?;
        require_non_empty("notify.email.host", &c.host)?;
    }
    if let Some(c) = &notify.custom_post {
        any = true;
        let items = c.items();
        if items.is_empty() {
            return Err(Error::Config(
                "notify.customPost 一个模板都没有：空数组等于没配".into(),
            ));
        }
        for (idx, item) in items.iter().enumerate() {
            require_non_empty(&format!("notify.customPost[{idx}].url"), &item.url)?;
        }
    }

    if !any {
        return Err(Error::Config(
            "notify 里没有任何渠道（全为 null）：这样配一条都发不出去；要关掉推送请把 notify 设为 null"
                .into(),
        ));
    }
    Ok(())
}

/// 必填字段的非空校验。**只报路径，不报值**。
fn require_non_empty(path: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Config(format!(
            "{path} 不能为空：配了这个渠道却发不出去"
        )));
    }
    Ok(())
}

/// JS 的真值口径（与 `mcloud-app::push` 的 `js_truthy` 同义）：
/// `0` / `""` / `null` / `false` 为假，对象与数组恒真。
fn js_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => true,
    }
}

impl From<AccountSettings> for AccountSettingsDto {
    fn from(s: AccountSettings) -> Self {
        Self {
            backup_wait_secs: s.backup_wait_secs,
            refresh_token_days: s.refresh_token_days,
            skip_tasks: s.skip_tasks,
            ai_avatar_daily_limit: s.ai_avatar_daily_limit,
            notify: s.notify,
        }
    }
}

impl TryFrom<AccountSettingsDto> for AccountSettings {
    type Error = Error;

    /// 校验通过才落成落盘模型 —— 这是"非法值进不了存储"的**唯一**关口。
    fn try_from(dto: AccountSettingsDto) -> Result<Self> {
        dto.validate()?;
        Ok(Self {
            backup_wait_secs: dto.backup_wait_secs,
            refresh_token_days: dto.refresh_token_days,
            skip_tasks: dto.skip_tasks,
            ai_avatar_daily_limit: dto.ai_avatar_daily_limit,
            notify: dto.notify,
        })
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

    /// **是否启用**（2026-10-01，用户口径："账号需要增加一个停用功能，这样在其他功能
    /// 下拉菜单就不显示"）。
    ///
    /// # 语义：停用 = 不再被**挑中**，不是删除
    ///
    /// 账号、凭据、历史记录全部留着；只是从各处"选账号"的下拉里消失。实现口径是
    /// **一处收口**：`GET /api/accounts` 默认就不返回停用的，账号页显式
    /// `?includeDisabled=true` 才看得到全部。页面各写一遍过滤，迟早漏一处 ——
    /// 而漏掉的那处正是"停用了还能选"。
    ///
    /// ⚠ **已存在的定时规则照旧跑**。停用改的是"以后选谁"，不是"过去的配置作废"；
    /// 要让某条规则也停，去定时页改它（那里的账号是手填的 id 列表）。
    ///
    /// # 为什么是 `bool` + `serde(default)`
    ///
    /// 老 `accounts.json` 里没有这一列，缺省必须读成 `true`：读成 `false` 会让
    /// 升级后的实例"所有账号突然都不能选"，而那种症状离"我改过什么"非常远。
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    #[serde(default)]
    pub settings: AccountSettings,
}

/// 缺省 = 启用。给老数据用（见 [`Account::enabled`]）。
fn default_enabled() -> bool {
    true
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

    /// 老库（没有 `enabled` 这一列）读出来必须是**启用**。
    ///
    /// 反了（读成 false）的症状是"升级后所有账号都不能选"，而那时没人会想到
    /// 是这一列 —— 所以用一条用例把它钉住。
    #[test]
    fn missing_enabled_reads_as_enabled() {
        let json = r#"{
            "id": "0f6050105ab111f4",
            "nickname": null,
            "auth": { "raw": "x", "platform": "android", "phone": "13800000000", "token": "t", "expire": 0 },
            "device_id": "D"
        }"#;
        let a: Account = serde_json::from_str(json).expect("老 JSON 要能读");
        assert!(a.enabled, "缺这一列必须当启用，不能当停用");
        // 落盘时写出来（否则下次读还是缺，回填永远不落）
        let back = serde_json::to_string(&a).unwrap();
        assert!(back.contains("\"enabled\":true"), "{back}");
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
        assert_eq!(s.refresh_token_days, 10);
        assert_eq!(s.notify, None, "默认不该给任何账号配推送渠道");
    }

    /// 删掉 `hc1t_*` 之后，带旧键的落盘值仍然读得回来。
    ///
    /// 这是"删一个**从没接线**的旋钮"为什么安全的判据：serde 忽略未知键，
    /// 所以老库里残留的 `hc1t_enabled` 不会让整份账号设置读不出来。
    #[test]
    fn legacy_hc1t_keys_are_ignored_after_removal() {
        let s: AccountSettings = serde_json::from_str(
            r#"{"hc1t_enabled":true,"hc1t_game_secs":600,"hc1t_target_rank":10}"#,
        )
        .expect("旧键必须被忽略，不能让整份设置读不出来");
        assert!(s.skip_tasks.is_empty());
        assert_eq!(s.ai_avatar_daily_limit, 10, "其余字段照常取默认");
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
            enabled: true,
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
            enabled: true,
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

    // ── 账号设置的**线格式**与校验（GET/PUT /api/accounts/{id}/settings）──

    /// 一份**合法**的请求体。各条测试在它上面只改一处，免得每回抄一遍。
    fn valid_settings_body() -> serde_json::Value {
        serde_json::json!({
            "backupWaitSecs": 20,
            "refreshTokenDays": 10,
            "skipTasks": [117, 437],
            "aiAvatarDailyLimit": 5,
            "notify": {
                "title": "我的推送",
                "onlyError": true,
                "minLevel": "warn",
                "serverChan": { "token": "SCT-SECRET-VALUE" }
            }
        })
    }

    /// ⭐ 合法设置能**存能读回**：先过线格式校验，再走一遍**落盘形态**
    /// （`snake_case` JSON）的序列化 / 反序列化 —— 与 `JsonStore` 的
    /// `write_json` / `read_json` 是同一条路。
    ///
    /// 路由层没有集成测试（`mcloud-server` 的测试在本仓不能跑，见 AGENTS.md），
    /// 所以"存得进、读得回"在这里用同一条序列化路径替代。
    #[test]
    fn a_valid_body_stores_and_reads_back() {
        let settings =
            AccountSettingsDto::from_wire(valid_settings_body()).expect("合法请求体必须通过");
        assert_eq!(settings.backup_wait_secs, 20);
        assert_eq!(settings.refresh_token_days, 10);
        assert_eq!(settings.skip_tasks, vec![117, 437]);
        assert_eq!(settings.ai_avatar_daily_limit, 5);
        let notify = settings.notify.clone().expect("notify 必须被存下来");
        assert_eq!(notify.title, "我的推送");
        assert!(notify.only_error);
        assert_eq!(
            notify.server_chan.as_ref().unwrap().token,
            "SCT-SECRET-VALUE"
        );

        let acc = Account {
            id: AccountId::new("13800000001"),
            nickname: None,
            auth: auth_with_expiry(0),
            device_id: DeviceId("D-1".into()),
            pc_platform: None,
            user_domain_id: None,
            owner: None,
            enabled: true,
            settings: settings.clone(),
        };
        let stored = serde_json::to_string(&acc).expect("落盘");
        let back: Account = serde_json::from_str(&stored).expect("读回");
        assert_eq!(back.settings, settings, "合法设置必须能存能读回");
    }

    /// `GET` 出来的对象必须能原样 `PUT` 回去（往返闭合）。
    #[test]
    fn the_wire_format_round_trips() {
        let settings = AccountSettingsDto::from_wire(valid_settings_body()).unwrap();
        let dto = AccountSettingsDto::from(settings.clone());
        let json = serde_json::to_string(&dto).unwrap();
        let back: AccountSettingsDto = serde_json::from_str(&json).unwrap();
        assert_eq!(back, dto, "响应体必须能被当成请求体原样收回：{json}");
        assert_eq!(
            AccountSettingsDto::from_wire(serde_json::from_str(&json).unwrap()).unwrap(),
            settings
        );
    }

    /// 既有默认值必须都落在校验范围内 —— 不许"加了校验就把默认值变成非法值"。
    #[test]
    fn the_existing_defaults_pass_validation() {
        let s = AccountSettings::default();
        let back = AccountSettings::try_from(AccountSettingsDto::from(s.clone())).unwrap();
        assert_eq!(back, s);
    }

    /// 落盘键与线格式键必须**一一对应**：给 `AccountSettings` 加字段而忘了
    /// 加进 DTO，那个字段在 HTTP 上就永远写不进去。
    #[test]
    fn the_wire_keys_match_the_stored_keys() {
        // 让 `notify` 也真的落盘（`None` 时它被 `skip_serializing_if` 跳过）。
        // ⚠ 用结构体初始化而不是先 default 再赋值：clippy 的 field_reassign_with_default 会拦后者。
        let settings = AccountSettings {
            notify: Some(PushConfig::default()),
            ..Default::default()
        };
        let stored = serde_json::to_value(&settings).unwrap();
        let wire = serde_json::to_value(AccountSettingsDto::from(settings)).unwrap();
        let mut stored_keys: Vec<String> = stored
            .as_object()
            .unwrap()
            .keys()
            .map(|k| snake_to_camel(k))
            .collect();
        let mut wire_keys: Vec<String> = wire.as_object().unwrap().keys().cloned().collect();
        stored_keys.sort();
        wire_keys.sort();
        assert_eq!(stored_keys, wire_keys, "落盘字段与线格式字段必须一一对应");
    }

    /// `snake_case` → `camelCase`（测试专用，只处理这几个字段名）。
    fn snake_to_camel(s: &str) -> String {
        let mut out = String::new();
        let mut upper_next = false;
        for c in s.chars() {
            if c == '_' {
                upper_next = true;
            } else if upper_next {
                out.extend(c.to_uppercase());
                upper_next = false;
            } else {
                out.push(c);
            }
        }
        out
    }

    /// ⭐ 未知键 ⇒ 400 并点名，**绝不**静默落库。
    #[test]
    fn unknown_keys_are_rejected_with_the_key_named() {
        let mut body = valid_settings_body();
        body["backupWaitSecond"] = serde_json::json!(5); // 敲错一个词
        let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
        assert!(err.contains("backupWaitSecond"), "错误要点名未知键：{err}");
    }

    /// 少传字段 ⇒ 400（而不是"悄悄重置成默认值"）。
    #[test]
    fn a_missing_field_is_rejected() {
        let mut body = valid_settings_body();
        body.as_object_mut().unwrap().remove("aiAvatarDailyLimit");
        let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
        assert!(
            err.contains("aiAvatarDailyLimit"),
            "错误要点名缺失字段：{err}"
        );
    }

    /// `skipTasks` 的类型必须真的是**数字**数组。
    ///
    /// 落盘那条路是宽松的（旧形状 `["签到"]` 会被丢掉、不报错），
    /// 写入口**不许**照抄：用户敲进来的坏类型必须当场 400。
    #[test]
    fn skip_tasks_must_be_numbers() {
        let mut body = valid_settings_body();
        body["skipTasks"] = serde_json::json!(["117"]);
        assert!(
            AccountSettingsDto::from_wire(body).is_err(),
            "数字字符串不许收（那是读老数据的宽容，不是写接口的）"
        );

        let mut body = valid_settings_body();
        body["skipTasks"] = serde_json::json!("117");
        assert!(
            AccountSettingsDto::from_wire(body).is_err(),
            "整体不是数组不许收"
        );

        let mut body = valid_settings_body();
        body["skipTasks"] = serde_json::json!([117, null]);
        assert!(
            AccountSettingsDto::from_wire(body).is_err(),
            "数组里有 null 不许收"
        );
    }

    /// 数值范围：越界 ⇒ 400 并点名；边界上的 `0` 是**合法**的（既有语义）。
    #[test]
    fn out_of_range_numbers_are_rejected() {
        for (field, value) in [
            ("backupWaitSecs", serde_json::json!(3_601u64)),
            ("refreshTokenDays", serde_json::json!(-1)),
            ("aiAvatarDailyLimit", serde_json::json!(1_001u64)),
        ] {
            let mut body = valid_settings_body();
            body[field] = value;
            let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
            assert!(err.contains(field), "{field} 越界必须 400 且点名：{err}");
        }

        // `0` 秒 = 不等；`0` 轮 = 本趟不生成。两者都是**既有语义**，别顺手拒掉。
        let mut body = valid_settings_body();
        body["backupWaitSecs"] = serde_json::json!(0);
        body["refreshTokenDays"] = serde_json::json!(0);
        body["aiAvatarDailyLimit"] = serde_json::json!(0);
        assert!(
            AccountSettingsDto::from_wire(body).is_ok(),
            "0 是合法值（不等 / 不生成），不是越界"
        );
    }

    /// `notify` 的渠道结构：配了却缺必需字段 ⇒ 400。
    ///
    /// 缺字段的后果不是报错，而是发送侧**整条跳过**（`配了等于没配`），
    /// 所以必须在写入口拦下。
    #[test]
    fn broken_channel_configs_are_rejected() {
        let cases: [(&str, serde_json::Value); 7] = [
            (
                "serverChan",
                serde_json::json!({"serverChan": {"token": "   "}}),
            ),
            ("chat_id", serde_json::json!({"tgBot": {"token": "tk"}})),
            (
                "twoIm",
                serde_json::json!({"twoIm": {"key": "k", "sid": ""}}),
            ),
            (
                "workWeixin",
                serde_json::json!({"workWeixin": {"corpid": "c"}}),
            ),
            ("customPost", serde_json::json!({"customPost": []})),
            (
                "customPost",
                serde_json::json!({"customPost": [{"url": ""}]}),
            ),
            ("token", serde_json::json!({"pushplus": {}})),
        ];
        for (needle, notify) in cases {
            let mut body = valid_settings_body();
            body["notify"] = notify;
            let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
            assert!(err.contains(needle), "错误该提到 {needle}：{err}");
        }
    }

    /// `notify` 里一个渠道都没有 ⇒ 400（否则用户以为配上了）。
    #[test]
    fn notify_without_any_channel_is_rejected() {
        let mut body = valid_settings_body();
        body["notify"] = serde_json::json!({"title": "x", "onlyError": true});
        let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
        assert!(err.contains("没有任何渠道"), "实得：{err}");
    }

    /// `notify` 顶层的渠道名敲错 ⇒ 400。
    ///
    /// `PushConfig` 自己会静默忽略未知键（`pushPlus` 会被整个吞掉），
    /// 而那是"看起来配好了、其实一条不发"的经典长相。
    #[test]
    fn a_misspelled_channel_name_is_rejected() {
        let mut body = valid_settings_body();
        body["notify"] = serde_json::json!({"pushPlus": {"token": "tk"}});
        let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
        assert!(err.contains("pushPlus"), "要点名不认识的渠道：{err}");
    }

    /// `notify: null` 是"清空推送渠道"的表达方式（缺席则 400，见上）。
    #[test]
    fn null_notify_clears_the_channels() {
        let mut body = valid_settings_body();
        body["notify"] = serde_json::Value::Null;
        let s = AccountSettingsDto::from_wire(body).unwrap();
        assert_eq!(s.notify, None);
    }

    /// 请求体不是 JSON 对象 ⇒ 400（不是 serde 那句含糊的报错）。
    #[test]
    fn a_non_object_body_is_rejected() {
        let err = AccountSettingsDto::from_wire(serde_json::json!([1, 2]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("JSON 对象"), "实得：{err}");
    }

    /// ⭐ 校验错误**绝不能**回显敏感值。
    ///
    /// 这些错误字符串会进 HTTP 响应、也可能被打进日志，而 `notify` 里全是
    /// token / secret。判据：错误只许说**字段**，不许说**值**。
    #[test]
    fn validation_errors_never_echo_secret_values() {
        const SECRETS: [&str; 3] = ["SECRET-SC-TOKEN", "SECRET-2IM-KEY", "SECRET-DT-TOKEN"];

        // a) 渠道结构错误：坏的是 twoIm.sid，同一份配置里别的渠道的真值也不许被带出来
        let mut body = valid_settings_body();
        body["notify"] = serde_json::json!({
            "serverChan": {"token": "SECRET-SC-TOKEN"},
            "twoIm": {"key": "SECRET-2IM-KEY", "sid": "  "},
            "dingTalk": {"token": "SECRET-DT-TOKEN"}
        });
        let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
        assert!(err.contains("twoIm"), "应当点名 twoIm：{err}");
        for s in SECRETS {
            assert!(!err.contains(s), "敏感值 {s} 出现在错误信息里：{err}");
        }

        // b) serde 的类型错误也不许顺手带出**别的**字段的真值
        let mut body = valid_settings_body();
        body["skipTasks"] = serde_json::json!("不是数组");
        body["notify"] = serde_json::json!({"serverChan": {"token": "SECRET-SC-TOKEN"}});
        let err = AccountSettingsDto::from_wire(body).unwrap_err().to_string();
        for s in SECRETS {
            assert!(!err.contains(s), "敏感值 {s} 出现在错误信息里：{err}");
        }
    }
}
