//! 导出文档的**唯一**实现 —— CLI `mcloud config export` 与 WebUI `GET /api/config/export`
//! 共用这一份。
//!
//! # 为什么单独一个模块
//!
//! 用户口径（2026-10-12）：「**导出功能最好也能在 webui 使用**」✓。于是同一个"导出"
//! 有了两个入口，而两个入口**必须**导出同一种形状：导出的文件是要**跨机器搬**的
//! （导出的意义就是搬到另一台机器直接用 ✓），CLI 导出的文件要能被 WebUI 那个实例
//! 导入，反过来也一样 ✓。
//!
//! 写两份实现必然漂移 —— 而漂移的表现是"用界面导出的文件导不进去"，那种错误
//! 要等用户真的换了机器才现形 ✗。所以"导出哪几类 + 文件形状 + 稳定排序 + 敏感标记"
//! 只有**这一份**：CLI 与路由都调 [`collect`] / [`to_json`]。
//!
//! # 哪些进文件、哪些不进
//!
//! 键名照 `data/` 下的文件名，导出文件与 `data/*.json` 能对着看，排查时不用再翻译一层。
//!
//! | 键 | 存储里的落点 | 是什么 | 归属 |
//! |---|---|---|---|
//! | `push-rules` | `data/push-rules.json` | 推送规则（**渠道 token 在里面**） | per-owner |
//! | `push-panel` | `data/push-panel.json` | 面板行为的绑定 | per-owner |
//! | `exchange-subscriptions` | `data/exchange-subscriptions.json` | 兑换页订阅的奖品 | per-owner |
//! | `auth-refresh` | `data/auth-refresh.json` | 凭据自动刷新的钟点（`HH:MM`） | per-owner |
//! | `pipelines` | `data/pipelines.json` | 任务编排 | per-owner |
//! | `schedules` | `data/schedules.json` | 定时（含系统维护那两条） | per-owner |
//! | `settings` | `data/settings.json` | 实例级设置 | **实例级** |
//! | `accounts` | `data/accounts.json` | **账号列表与每个账号的凭据** | per-owner |
//! | `account-state` | `data/state.json` | 账号运行期状态（按账号 id） | per-account |
//! | `account-smid` | `data/smid.json` | **账号的设备指纹**（按账号 id） | per-account |
//! | `pc-device` | `data/pc_device.txt` | **本机 PC 设备指纹**（一枚） | **实例级** |
//!
//! # 凭据为什么**必须**带上（2026-10-12 用户口径，推翻了上一轮）
//!
//! 上一轮按"登录态搬到另一台机器没用"的口径把凭据排除在外。用户随即纠正：
//! 「**凭据也需要导出啊，不导出我怎么使用**」✓ —— 导出的意义就是搬到另一台机器
//! **直接用**：不带上凭据，新机器上得把每个账号重新登录一遍，这个功能等于废一半 ✓。
//!
//! 所以进文件的是这几样（**按 `data/` 下实际存储的名字核过** ✓）：
//!
//! * `accounts` —— `data/accounts.json` 里那个 `Vec<Account>`。凭据在 `auth` 那一格：
//!   `raw`（原始 base64，写回上游用 ✓）、`platform` / `phone` / `token`、
//!   `expire`（**epoch 毫秒**，不是秒 ✓）。另外 `device_id`（登录时冒充的那台设备）、
//!   `pc_platform`（windows / macos，决定后续每个请求的 UA 与头集合）、
//!   `user_domain_id`（刷凭据时"刷哪个用户域"，**无法从凭据反推** ✓）也一并带上 ——
//!   缺任何一个，新机器上这个账号都跑不起来或刷不了 ✓。
//! * `account-state` —— `data/state.json`，按**账号 id** 存的一格 JSON
//!   （本月备份读数、`device_id` / `pc_platform` 的镜像等 ✓）。带上它是为了让
//!   "这个账号在源机器上跑到哪儿了"跟着搬过去 ✓。
//! * `account-smid` —— `data/smid.json`，按**账号 id** 存的设备指纹
//!   （`fingerprint_json` + 上报换回的 `device_id` ✓）。**这是"凭据跟设备绑"那一半** ✓：
//!   `mcloud-core::smid` 的文件头写着两者的分工 —— 登录态"能冒充这个用户的凭据"、
//!   设备指纹"这台设备长什么样" ✓。只搬凭据不搬指纹，上游看到的就是
//!   "同一个账号换了一台长相完全不同的设备"，风控服务最想看到的正是这种漂移信号 ✗。
//! * `pc-device` —— `data/pc_device.txt`，机器级那一枚（32 位大写 hex + `-ENDIN` ✓）。
//!
//! ## `pc-device` 与 `account-smid` 到底是实例级还是 per-owner
//!
//! 这是**核过的结论**，依据都在代码里：
//!
//! * `account-smid`（`data/smid.json`）是 **per-account** —— `JsonStore` 的
//!   `Db::smid` 是 `HashMap<String, StoredSmid>`，键是**账号 id** ✓
//!   （`crates/mcloud-store/src/json_store.rs`，`persist_smid` / `remove_account`
//!   都按账号 id 增删 ✓）。而账号是 per-owner 的 ⇒ 它**间接**是 per-owner 的 ✓。
//! * `pc-device`（`data/pc_device.txt`）是**实例级**（整台机器一枚，**不是** per-owner ✗）：
//!   `PcDeviceStore` 的读写没有 owner 参数 ✓、`JsonStore` 里它就是**一个文件一个值**
//!   （`crates/mcloud-core/src/pc_device.rs` 的文件头写着"机器级" ✓）。
//!
//! 界面上那个「更换本机设备指纹」按钮（`web/src/pages/accounts-page.tsx`，
//! `POST /api/login/device/rotate`）换的正是 `pc-device` 这一枚 ✓；它**不**动任何账号的
//! `account-smid` ✓ —— 两条通道分开写，正是 `pc_device.rs` / `smid.rs` 两个文件头
//! 反复强调的那件事（"换凭据不该换设备长相"）✓。
//!
//! ⚠ 顺带把 `pc_device.rs` 的另一句也带上：今天这枚**已经不再参与出码或登录** ——
//! 二维码的 `dID` 每张码现生成、四条登录路径按账号取设备 ✓ ⇒ 带上它是为了
//! "新机器上 `login device show` 看到的还是原来那台设备"，**不是**"不带上账号就登不上" ✓。
//! 真正让账号在新机器上直接可用的是 `accounts` + `account-smid` + `account-state` ✓。
//!
//! # 不进文件的
//!
//! * `data/sessions.json` —— WebUI 的**登录闸**会话（键是浏览器 cookie 的 token 哈希 ✓）。
//!   它是"谁能进这个 WebUI"，不是"云盘账号的凭据"：新机器上有自己的管理员口令
//!   （`mcloud user add` ✓），把旧 cookie 的哈希搬过去没有对应的浏览器 ✓。
//! * `data/history.json` / `data/logs/` —— 运行历史与日志：那是记录，不是"配置" ✓。
//! * `data/kouling-pool.json` —— 全场共享的口令池（实例级、且是**运行期**抓来的 ✓）。
//! * `data/users.json` —— 本机的用户与口令哈希。它**属于这台机器**：导入侧的口径是
//!   "重指到**本机**那个用户" ✓，把源机器的用户搬过来正是要避免的事 ✗。
//!
//! # 敏感标记
//!
//! 顶层 `containsCredentials: true` + 一句 `notice` ✓。文件里有账号凭据与推送渠道
//! token ⇒ 它就是一份登录态，不该随手发、不该进版本库 ✓。
//!
//! # 实例级设置（`data/settings.json`）怎么处置
//!
//! 取舍照旧：**导出带上，导入默认不动**；要一起改得显式加 `--with-instance`。
//!
//! ⚠ `pc-device` **不**走那个开关：它跟的是"凭据要能用"这条线（`--with-instance`
//! 管的是整台机器的策略格），所以导入时**一律应用** ✓ —— 见 `cli/config.rs` 的 `apply`。

use std::collections::BTreeMap;

use mcloud_app::AppHandle;
use mcloud_core::{
    auth_refresh, Account, AppSettings, ExchangeSubscription, PanelBindings, Pipeline, PushRule,
    Schedule, UserId,
};

/// 顶层版本键：导入时**必须**认它 —— 认不出就报错退非 0，一个字节都不改。
pub const FORMAT_KEY: &str = "mcloudConfig";

/// 当前格式版本。
///
/// ⚠ 本轮**没有** +1：加进来的每一格都是 `Option` + `#[serde(default)]` ✓ ⇒
/// 上一轮那份（不含凭据的）文件照样读得进来，只是"凭据那一类不动" ✓
/// （那正是 `Option` 的语义：文件里没写的键 = 不许动这一类 ✓）。
/// 改**已有**键的含义或形状时才 +1 —— 那时老文件必须被明确拒掉，而不是"读进去一半" ✓。
pub const FORMAT_VERSION: u32 = 1;

/// 导出文件里那句"这份文件别乱发"的说明（见模块头那段）。
pub const NOTICE: &str = "这份文件等于你的登录态：里面有每个云盘账号的凭据（token / 设备指纹）、\
推送渠道的 token 与 webhook 地址。别随手发给别人，也别提交进版本库（git）。\
搬到另一台机器上用 `mcloud config import <文件>` 导入 —— 导入后不用重新登录就能跑任务。";

/// 导出时那份"我是谁"的信息。
///
/// 它有两个用处：人看着知道这份是从哪个账号导的；导入时**按它算 id 重指**
/// （`daily-<owner>` 这类 id 是从 owner 推出来的）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnerRef {
    pub id: String,
    pub name: String,
}

/// `smid.json` 里一条记录的导出形态。
///
/// 与存储层的 `StoredSmid` **逐字段对齐**（那一个是私有的 ✓），也与
/// `mcloud_core::SmidState` 对齐 —— 中间这一层只是把 `SmidState`（刻意不派生
/// `Serialize`，见 `crates/mcloud-core/src/smid.rs` 的文件头 ✓）换成能进文件的形状 ✓。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SmidDoc {
    /// 绑定的设备特征（JSON 文本）。`None` = 还没绑过。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_json: Option<String>,
    /// 最近一次上报换回的 `deviceId`（含 `B` 前缀）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

/// 导出/导入的那一份文档。
///
/// 每一类都是 `Option`：`None` 既表示"导出时本机这一类是空的"，
/// 也表示"导入时**不许动**这一类"。导入侧只认后者 —— 文件里没写的键
/// 反序列化出来就是 `None`，于是"文件里没有的类别不动"是**结构上**成立的，
/// 不靠一句 `if` 记得去判。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExportDoc {
    #[serde(rename = "mcloudConfig")]
    pub format: u32,
    #[serde(
        rename = "exportedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub exported_at: Option<String>,
    /// 敏感标记：这份文件里有凭据（账号凭据 / 设备指纹 / 推送渠道 token）。
    ///
    /// 单独一个**布尔**而不是只靠 `notice` 那句话：机器要能一眼判出来
    /// （脚本、编辑器、将来的导入侧提示都用得上 ✓），而 `notice` 是给人读的 ✓。
    #[serde(
        rename = "containsCredentials",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub contains_credentials: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<OwnerRef>,
    #[serde(
        rename = "push-rules",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub push_rules: Option<Vec<PushRule>>,
    #[serde(
        rename = "push-panel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub push_panel: Option<PanelBindings>,
    #[serde(
        rename = "exchange-subscriptions",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub exchange_subscriptions: Option<Vec<ExchangeSubscription>>,
    #[serde(
        rename = "auth-refresh",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub auth_refresh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipelines: Option<Vec<Pipeline>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<Schedule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<AppSettings>,
    /// 账号列表（**凭据在里面** —— 见模块头）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<Account>>,
    /// 账号运行期状态，键 = 账号 id。
    #[serde(
        rename = "account-state",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub account_state: Option<BTreeMap<String, serde_json::Value>>,
    /// 账号的设备指纹，键 = 账号 id。
    #[serde(
        rename = "account-smid",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub account_smid: Option<BTreeMap<String, SmidDoc>>,
    /// 本机 PC 设备指纹（**实例级**，一枚）。
    #[serde(rename = "pc-device", default, skip_serializing_if = "Option::is_none")]
    pub pc_device: Option<String>,
}

impl ExportDoc {
    /// 文件里那个"源 owner"（没有就不重指 id）。
    pub fn source_owner(&self) -> Option<UserId> {
        self.owner.as_ref().map(|o| UserId::new(o.id.clone()))
    }
}

/// 读 store，组装成一份文档。
///
/// **稳定排序**（同一次导出跑两遍结果一样，方便 diff）：四类有 id 的按 id 升序；
/// 订阅按"订阅时刻 + 奖品 id"排（那本来就是它的语义顺序 —— 兑换页顶部按它从早到晚）；
/// 两张按账号 id 存的表用 `BTreeMap`（键序天然稳定 ✓）。
pub async fn collect(handle: &AppHandle, owner: &UserId) -> anyhow::Result<ExportDoc> {
    let store = &handle.store;
    let settings = store.settings().await?;

    let mut push_rules = store.push_rules(owner).await?;
    push_rules.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    let mut pipelines = store.pipelines_for(owner).await?;
    pipelines.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    let mut schedules = store.schedules_for(owner).await?;
    schedules.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    let mut subs = store.exchange_subscriptions(owner).await?;
    subs.sort_by_key(|s| (s.subscribed_at_ms, s.prize_id));

    // 钟点导**生效值**：自己的那份优先、没设过就回落到实例级那一格
    // （`auth_refresh::effective_at` 是这条口径**唯一**的实现）。
    // 用户要搬的是"我这台机器上几点刷"，而不是"我这一格写没写过"。
    let at = auth_refresh::effective_at(
        store.auth_refresh_at(owner).await?.as_deref(),
        &settings.auth_refresh_at,
    );

    // ── 凭据那一组 ─────────────────────────────────────────────────────
    //
    // ⚠ 只取**这个 owner 名下**的账号（`accounts_for` 是归属过滤的唯一入口 ✓），
    // 后面两张表再按这些账号 id 去读 —— 于是"别人的账号状态 / 别人的设备指纹"
    // 结构上就不可能进这份文件 ✓（这也是 WebUI 那条"别人的东西一个字节都不给"的实现 ✓）。
    let mut accounts = store.accounts_for(owner).await?;
    accounts.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    let mut account_state: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut account_smid: BTreeMap<String, SmidDoc> = BTreeMap::new();
    for a in &accounts {
        // `account_state` 读不到时给的是 `Null`（不是空对象）⇒ 没状态的账号不进文件 ✓
        // （进了的话，新机器上会多出一格空的运行期状态，看着像"跑到一半"）。
        let state = store.account_state(&a.id).await?;
        if !state.is_null() {
            account_state.insert(a.id.as_str().to_string(), state);
        }
        if let Some(s) = store.get_smid_state(&a.id).await? {
            account_smid.insert(
                a.id.as_str().to_string(),
                SmidDoc {
                    fingerprint_json: s.fingerprint_json,
                    device_id: s.device_id,
                },
            );
        }
    }

    // 机器级那一枚：**只读**（`pc_device_id_opt` 不生成、不落盘 ✓）——
    // 用 `pc_device_id()` 的话，"导出一下"就会顺手给这台机器造一枚设备指纹 ✗。
    let pc_device = handle.login.pc_device_id_opt().await?;

    Ok(ExportDoc {
        format: FORMAT_VERSION,
        exported_at: Some(mcloud_core::beijing_format(
            &*handle.clock,
            "%Y-%m-%d %H:%M:%S",
        )),
        contains_credentials: Some(true),
        notice: Some(NOTICE.to_string()),
        owner: store.user(owner).await?.map(|u| OwnerRef {
            id: u.id.to_string(),
            name: u.name,
        }),
        push_rules: Some(push_rules),
        push_panel: Some(store.push_panel(owner).await?),
        exchange_subscriptions: Some(subs),
        auth_refresh: Some(at),
        pipelines: Some(pipelines),
        schedules: Some(schedules),
        settings: Some(settings),
        accounts: Some(accounts),
        account_state: Some(account_state),
        account_smid: Some(account_smid),
        pc_device,
    })
}

pub fn to_json(doc: &ExportDoc) -> anyhow::Result<String> {
    let mut s = serde_json::to_string_pretty(doc)?;
    s.push('\n');
    Ok(s)
}

/// 逐类一句话（导出与导入两边共用这一份措辞）。
pub fn describe(doc: &ExportDoc) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = &doc.push_rules {
        out.push(format!("推送规则 {} 条", v.len()));
    }
    if doc.push_panel.is_some() {
        out.push(format!(
            "面板绑定 {} 个",
            mcloud_core::push::BehaviorKey::ALL.len()
        ));
    }
    if let Some(v) = &doc.exchange_subscriptions {
        out.push(format!("兑换订阅 {} 条", v.len()));
    }
    if let Some(v) = &doc.auth_refresh {
        out.push(format!("刷新钟点 {v}"));
    }
    if let Some(v) = &doc.pipelines {
        out.push(format!("编排 {} 条", v.len()));
    }
    if let Some(v) = &doc.schedules {
        out.push(format!("定时 {} 条", v.len()));
    }
    if doc.settings.is_some() {
        out.push("实例设置 1 份".to_string());
    }
    if let Some(v) = &doc.accounts {
        out.push(format!("账号 {} 个（含凭据）", v.len()));
    }
    if let Some(v) = &doc.account_state {
        out.push(format!("账号状态 {} 份", v.len()));
    }
    if let Some(v) = &doc.account_smid {
        out.push(format!("账号设备指纹 {} 份", v.len()));
    }
    if doc.pc_device.is_some() {
        out.push("本机设备指纹 1 枚".to_string());
    }
    out
}

/// 下载文件名里那一段"用户名"的安全形态。
///
/// # 为什么要过一道
///
/// 用户名是用户自己起的（可以是中文、可以带空格与引号 ✓），而它要进
/// `Content-Disposition` 头 —— 带引号或换行的值会**破坏这个头**（响应头注入 ✗）。
/// 所以 ASCII 那一份（`filename=`）只留 `[A-Za-z0-9._-]`，其余换成 `_`；
/// 中文名走 `filename*=UTF-8''…` 那一份（RFC 5987 ✓），浏览器优先用它 ✓。
///
/// 空名（用户没起名 / 取不到）回落到 `owner`。
pub fn ascii_filename_part(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.trim_matches('_').is_empty() {
        "owner".to_string()
    } else {
        s
    }
}

/// `filename*=UTF-8''…` 里那段百分号编码（RFC 5987）。
///
/// 只保留 `attr-char`（字母数字与 `!#$&+-.^_`|~`），其余一律 `%XX` ——
/// 引号、空格、中文都落进 `%XX`，于是这个头不可能被值里的字符破坏 ✓。
pub fn percent_encode_filename(name: &str) -> String {
    let mut out = String::new();
    for b in name.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric()
            || matches!(
                c,
                '!' | '#' | '$' | '&' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
            )
        {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 门：ASCII 那一段不能把响应头破坏掉（用户名里的引号 / 换行 / 中文）。
    #[test]
    fn ascii_part_cannot_break_the_header() {
        assert_eq!(ascii_filename_part("admin"), "admin");
        assert_eq!(ascii_filename_part("a b\"c"), "a_b_c");
        assert_eq!(ascii_filename_part("张三"), "owner");
        assert_eq!(ascii_filename_part(""), "owner");
        assert_eq!(ascii_filename_part("   "), "owner");
        // 换行（响应头注入的经典形状）也一并被换成下划线
        assert_eq!(ascii_filename_part("a\r\nX: y"), "a__X__y");
    }

    /// 门：`filename*=UTF-8''…` 只出 `attr-char` 与 `%XX`（引号 / 空格 / 中文全编码）。
    #[test]
    fn percent_encoded_part_is_attr_char_only() {
        assert_eq!(
            percent_encode_filename("mcloud-config.json"),
            "mcloud-config.json"
        );
        assert_eq!(percent_encode_filename("张三"), "%E5%BC%A0%E4%B8%89");
        assert_eq!(percent_encode_filename("a b\"c"), "a%20b%22c");
        let enc = percent_encode_filename("名字 (1).json");
        assert!(
            !enc.contains(' ') && !enc.contains('"') && !enc.contains('('),
            "{enc}"
        );
    }
}
