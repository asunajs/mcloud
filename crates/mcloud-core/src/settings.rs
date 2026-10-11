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

/// 运行历史保留条数的默认值（= `mcloud_store::HISTORY_LIMIT` 那个老常量）。
pub const DEFAULT_HISTORY_LIMIT: usize = 200;

/// 单次运行日志行数上限的默认值（= [`crate::LOG_LIMIT_PER_RUN`]）。
pub const DEFAULT_LOG_LIMIT_PER_RUN: usize = crate::LOG_LIMIT_PER_RUN;

/// 日志目录总量上限的默认值（MB）。
///
/// 取 64 MB：本仓实测一条运行日志文件在 KB 量级、140 个文件才 556 KB
/// ⇒ 64 MB 装得下**几万**次运行的日志 ✓，而它同时把"日志把磁盘写满"
/// 这条真会发生的事封住 ✓（用户 2026-10-09 口径：「让用户设置最大大小」）。
pub const DEFAULT_LOG_TOTAL_MB: u64 = 64;

/// 服务端日志**内存窗口**行数的默认值（= `mcloud_server::logbuf::RING_CAPACITY` 那个老常量）。
///
/// ⚠ 与 [`DEFAULT_LOG_LIMIT_PER_RUN`] 不是一回事：那个是"一次运行留多少行"，
/// 这个是"**整个进程**的日志在内存里留多少行"（WebUI「日志」页看的那份 ✓）。
pub const DEFAULT_SERVER_LOG_MEM_LINES: usize = 2000;

/// 服务端日志**磁盘**总量上限的默认值（MB）。
///
/// 取 64 MB：本仓实测服务端日志约 200~300 B/行 ⇒ 64 MB ≈ 二十几万行 ≈
/// 按每天几千行算能装**几个月** ✓（用户 2026-10-09 口径：「让用户设置最大大小」）。
pub const DEFAULT_SERVER_LOG_TOTAL_MB: u64 = 64;

/// 口令下限允许的范围。
pub const MIN_PASSWORD_LEN_RANGE: (usize, usize) = (4, 64);

/// 会话时长允许的范围（小时）：1 小时 ~ 90 天。
pub const SESSION_TTL_HOURS_RANGE: (u32, u32) = (1, 24 * 90);

/// 历史保留条数的允许范围。
///
/// 下限 20 而不是 1：历史页是**按条**看的，只留个位数等于把"回看"这件事做没了 ✗。
/// 上限 2000：每条记录本身很小（KB 级），2000 条仍在"整份 JSON 重写"能接受的量级 ✓。
pub const HISTORY_LIMIT_RANGE: (usize, usize) = (20, 2000);

/// 单次运行日志行数的允许范围。
///
/// 下限 200 = 老常量 [`crate::RUN_LOG_LIMIT`]（详情里带的那 200 行 ⇒ 再小就没得看 ✗）。
/// 上限 20000：按实测均 119 B/行 ≈ 2.4 MB / 次运行 —— 再大就不是"日志"而是"倒数据"了 ✗。
pub const LOG_LIMIT_PER_RUN_RANGE: (usize, usize) = (200, 20_000);

/// 日志目录总量上限的允许范围（MB）。
pub const LOG_TOTAL_MB_RANGE: (u64, u64) = (1, 4096);

/// 服务端日志内存窗口行数的允许范围。
///
/// 下限 200 = [`crate::RUN_LOG_LIMIT`]（界面上那一屏的体量 ⇒ 再小就"看不了现场"✗）；
/// 上限 20000：按实测均 ~250 B/行 ≈ 5 MB 常驻 —— 再大就不是"窗口"而是"把日志全装回内存"✗
/// （而那正是用户 2026-10-09 要消掉的那件事 ✓）。
pub const SERVER_LOG_MEM_LINES_RANGE: (usize, usize) = (200, 20_000);

/// 服务端日志磁盘总量上限的允许范围（MB）。
pub const SERVER_LOG_TOTAL_MB_RANGE: (u64, u64) = (1, 4096);

/// 应用级设置。
///
/// ⚠ `#[serde(default)]` 不能省：`settings.json` 是**后来才有的**文件，
/// 老实例第一次读它时文件是空的（或缺字段），那时必须按默认值走 ——
/// 而不是"配置损坏，起不来"。加一个设置项也不该让老文件失效。
///
/// ⚠ **不是 `Copy`**（2026-10-11 起 ✓）：[`AppSettings::auth_refresh_at`] 是一个
/// `String`（`HH:MM` 落盘形态与界面/CLI 说的是同一个串 ✓）⇒ 有 `String` 字段的
/// 类型拿不到 `Copy` ✓。改之前它恰好全是 `Copy` 标量，所以那时是 `Copy` ——
/// 去掉它不影响任何调用点：`store.settings()` 本来就返回**所有权** ✓。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 口令长度下限。
    ///
    /// ⚠ 只对**明文**生效：前端摘要恒 64 位，量不出人到底输了几位
    /// （所以 WebUI 会拿这个值在前端先卡一道，服务端那道留给 curl 这类直接调用）。
    pub min_password_len: usize,
    /// 新建会话的有效期。改它**只影响之后新建的会话**，已发出的不会被动缩短。
    pub session_ttl_hours: u32,
    /// 运行历史保留多少条（超出后丢最旧的**及其日志**）。
    ///
    /// ⚠ 默认值 = 老常量 `mcloud_store::HISTORY_LIMIT` ✓（见本模块头那条口径：
    /// "没改过设置的实例行为必须与改之前一样"）。
    pub history_limit: usize,
    /// **单次运行**留多少行日志（超出丢最旧的）。
    ///
    /// 它是日志那一路的**行数**上限；文件总量另有 [`AppSettings::log_total_mb`] 管 ✓。
    pub log_limit_per_run: usize,
    /// `data/logs/` 的**总量**上限（MB）。超出后按**最旧**删日志文件 ✓。
    ///
    /// # 为什么是"字节总量"而不是"留多少个文件"
    ///
    /// 文件数不是用户关心的量：同样 200 个文件，一次运行 2000 行与一次运行 2 行
    /// 差两个数量级 ✓。用户要的是"日志最多占多大地方" ✓
    /// （2026-10-09 口径：「让用户设置**最大大小**」）。
    pub log_total_mb: u64,
    /// **服务端日志**（WebUI「日志」页那份：调度器、上游 http、认证……）
    /// 在**内存**里留多少行 ✓。
    ///
    /// ⚠ 默认值 = 老常量 `logbuf::RING_CAPACITY` ✓（那条口径：没改过设置的实例
    /// 行为必须与改之前一样 ✓）。它是**窗口**不是全量 —— 更早的行在磁盘上
    /// （[`AppSettings::server_log_total_mb`] 管着），翻页时按需读 ✓。
    pub server_log_mem_lines: usize,
    /// `data/server-logs/` 的**总量**上限（MB）。超出后按**最旧**删整天文件 ✓。
    pub server_log_total_mb: u64,
    /// **凭据自动刷新的预设时间**（东八区墙钟 `HH:MM`，例如 `"03:00"` ✓）——
    /// ⚠⚠ 它现在是**实例级默认值**（2026-10-11 用户口径：「**要每用户一个钟点**」✓），
    /// **不是**"那个钟点"本身 ✗。
    ///
    /// # 谁是"那个钟点"（改完之后）
    ///
    /// 每个用户自己那一份在 `data/auth-refresh.json` 里（**按 owner 存** ✓，
    /// 见 [`crate::auth_refresh::effective_at`] ✓）。本格的角色只剩一个：
    /// **还没自己设过的用户读到的就是它** ✓ —— 于是它是"每用户值的初始值" ✓。
    ///
    /// # 为什么**留着这个键**（不删、也不改名）
    ///
    /// * **它是老实例里唯一记得"用户改过几点"的地方** ✓ —— 升级前用户把钟点改成
    ///   22:30 就是写在这一格上的；删掉（或改名让 serde 读不到）等于那个值
    ///   **凭空回到 03:00** ✗，而那正是用户点名不许的事 ✓；
    /// * 留着之后**迁移天然幂等**：读的时候回落过去，**没有任何写盘动作** ⇒
    ///   不存在"跑第二遍把用户后来改的值盖掉"这条路 ✓
    ///   （理由与那张表的形状见 `mcloud_core::auth_refresh` 模块头 ✓）；
    /// * 改名要 `#[serde(rename = "auth_refresh_at")]` 才保得住旧键 ——
    ///   为"名字更准"付一次落盘键名的兼容成本，不划算 ✗
    ///   （含义由本注释与 [`crate::auth_refresh::effective_at`] 的注释钉住 ✓）。
    ///
    /// ⚠ 别的代码**不该**再拿它当"用户设的钟点"读 ✗：读用户钟点走
    /// [`crate::auth_refresh::effective_at`]（或 `mcloud_app::auth_refresh_auto` 里
    /// 那个已经组装好的读法 ✓），写用户钟点走 `UserStore::save_auth_refresh_at` ✓。
    /// 今天只有**手改 `data/settings.json`** 会让这一格变 ✓
    ///（WebUI 两条设置接口都是读-改-写、各改自己那几格 ✓，谁也不碰它 ✓）。
    ///
    /// ⚠ 与 `AccountSettings::refresh_token_days` **不是同一件事**，别合并 ✗：
    /// 那一格是"剩余不足 N 天就刷"的**阈值**（按账号 ✓，是既有判据 ✓），
    /// 这一格是"每天几点检查一次"的**钟点** —— 到点之后**先走既有判据**
    /// （不需要刷就跳过 ✓，见 `mcloud_core::auth_refresh` 的模块头 ✓）。
    ///
    /// # 为什么是字符串而不是"两个整数"
    ///
    /// `HH:MM` 是用户看得见、也输得进去的形态 ✓，而落盘形态与它**逐字相同**
    /// ⇒ 界面 / CLI / 日志三处说的是同一个串，不需要任何换算 ✓。
    /// 解析成"当天毫秒偏移"只有一处：`mcloud_core::auth_refresh::parse_clock` ✓。
    ///
    /// ⚠ 默认值 = `crate::auth_refresh::DEFAULT_AUTH_REFRESH_AT` ✓
    /// （那条口径：没改过设置的实例行为必须与默认值一致 ✓）。
    pub auth_refresh_at: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            min_password_len: DEFAULT_MIN_PASSWORD_LEN,
            session_ttl_hours: DEFAULT_SESSION_TTL_HOURS,
            history_limit: DEFAULT_HISTORY_LIMIT,
            log_limit_per_run: DEFAULT_LOG_LIMIT_PER_RUN,
            log_total_mb: DEFAULT_LOG_TOTAL_MB,
            server_log_mem_lines: DEFAULT_SERVER_LOG_MEM_LINES,
            server_log_total_mb: DEFAULT_SERVER_LOG_TOTAL_MB,
            auth_refresh_at: crate::auth_refresh::DEFAULT_AUTH_REFRESH_AT.to_string(),
        }
    }
}

impl AppSettings {
    /// 会话 TTL（毫秒）。**唯一的换算点** —— 别在别处再乘一遍。
    pub fn session_ttl_ms(&self) -> i64 {
        i64::from(self.session_ttl_hours) * 60 * 60 * 1000
    }

    /// 日志总量上限的**字节**形态。**唯一的换算点** —— 别在别处再乘一遍。
    pub fn log_total_bytes(&self) -> u64 {
        self.log_total_mb * 1024 * 1024
    }

    /// 服务端日志总量上限的**字节**形态。**唯一的换算点** —— 别在别处再乘一遍。
    pub fn server_log_total_bytes(&self) -> u64 {
        self.server_log_total_mb * 1024 * 1024
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
        let (lo, hi) = HISTORY_LIMIT_RANGE;
        if self.history_limit < lo || self.history_limit > hi {
            return Err(format!("历史保留条数要在 {lo}~{hi} 之间"));
        }
        let (lo, hi) = LOG_LIMIT_PER_RUN_RANGE;
        if self.log_limit_per_run < lo || self.log_limit_per_run > hi {
            return Err(format!("单次运行日志行数要在 {lo}~{hi} 之间"));
        }
        let (lo, hi) = LOG_TOTAL_MB_RANGE;
        if self.log_total_mb < lo || self.log_total_mb > hi {
            return Err(format!("日志总量上限要在 {lo}~{hi} MB 之间"));
        }
        let (lo, hi) = SERVER_LOG_MEM_LINES_RANGE;
        if self.server_log_mem_lines < lo || self.server_log_mem_lines > hi {
            return Err(format!("服务端日志内存行数要在 {lo}~{hi} 之间"));
        }
        let (lo, hi) = SERVER_LOG_TOTAL_MB_RANGE;
        if self.server_log_total_mb < lo || self.server_log_total_mb > hi {
            return Err(format!("服务端日志总量上限要在 {lo}~{hi} MB 之间"));
        }
        // ⚠ 预设刷新时间的判据**只有一处**（`auth_refresh::parse_clock`）——
        // 在这里再写一遍"两段、冒号、范围"就是第二份口径，迟早与它分叉 ✗。
        if crate::auth_refresh::parse_clock(&self.auth_refresh_at).is_none() {
            return Err(format!(
                "凭据自动刷新的预设时间要是 24 小时制的 HH:MM（例如 03:00），当前是「{}」",
                self.auth_refresh_at
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ 这条测的不是"默认值是多少"，而是"**默认值等于那几个老常量**"。
    /// 它们一旦漂移，所有没改过设置的实例行为都会变，而现象是"莫名其妙地不一样"。
    ///
    /// ⚠ 与 `mcloud_store::HISTORY_LIMIT` 那一格的对照在 store 侧（那边同时依赖两边 ✓）。
    #[test]
    fn defaults_match_the_old_constants() {
        let d = AppSettings::default();
        assert_eq!(d.min_password_len, DEFAULT_MIN_PASSWORD_LEN);
        assert_eq!(d.session_ttl_ms(), crate::WEB_SESSION_TTL_MS);
        assert_eq!(
            d.log_limit_per_run,
            crate::LOG_LIMIT_PER_RUN,
            "单次运行日志上限的默认值必须等于老常量"
        );
        assert_eq!(d.history_limit, 200, "历史条数默认值 = 老的 HISTORY_LIMIT");
        assert_eq!(d.log_total_bytes(), DEFAULT_LOG_TOTAL_MB * 1024 * 1024);
        // ⚠ 服务端日志那两格同理：内存窗口的默认值 = `logbuf::RING_CAPACITY` 那个老常量
        //（2026-10-09 之前它就是"全部"，改成可设之后**默认必须照旧** ✓）
        assert_eq!(
            d.server_log_mem_lines, 2000,
            "服务端日志内存窗口默认值 = 老的 RING_CAPACITY"
        );
        assert_eq!(
            d.server_log_total_bytes(),
            DEFAULT_SERVER_LOG_TOTAL_MB * 1024 * 1024
        );
        // ⚠ 凭据自动刷新的预设时间同理：默认值必须 = `auth_refresh` 那个常量 ✓
        //（两处各写一遍的话，"从没改过设置"的实例与 `auth_refresh::daily_expr`
        //  会各说各话 ✗）。
        assert_eq!(
            d.auth_refresh_at,
            crate::auth_refresh::DEFAULT_AUTH_REFRESH_AT
        );
    }

    /// ⭐ 预设刷新时间**非法一律拒**（不静默兜默认值 ✗ —— 那会让"我设了 22:30、
    /// 它却在半夜刷"变成查不出来的事 ✓）。
    #[test]
    fn validate_rejects_a_bad_refresh_clock() {
        let ok = AppSettings::default();
        for bad in [
            "",
            "3",
            "0300",
            "24:00",
            "12:60",
            "十二点",
            "3:5:1",
            "-1:00",
        ] {
            assert!(
                AppSettings {
                    auth_refresh_at: bad.to_string(),
                    ..ok
                }
                .validate()
                .is_err(),
                "「{bad}」不是钟点，应被拒"
            );
        }
        for good in ["00:00", "03:00", "3:05", "23:59"] {
            assert!(
                AppSettings {
                    auth_refresh_at: good.to_string(),
                    ..ok
                }
                .validate()
                .is_ok(),
                "「{good}」是合法钟点，应放行"
            );
        }
    }

    #[test]
    fn validate_rejects_out_of_range() {
        let ok = AppSettings::default();
        assert!(ok.validate().is_ok());
        // ⚠ `AppSettings` 不是 `Copy`（`auth_refresh_at` 是 `String` ✓）⇒ 下面那些
        // `..ok` 的结构体更新会**部分移动**它，所以逐处写 `..ok.clone()` ✓。
        // 断言要看的是"只动了这一格、其余照旧" ✓。

        for bad in [3usize, 65] {
            let s = AppSettings {
                min_password_len: bad,
                ..ok.clone()
            };
            assert!(s.validate().is_err(), "下限 {bad} 应被拒");
        }
        for bad in [0u32, 24 * 91] {
            let s = AppSettings {
                session_ttl_hours: bad,
                ..ok.clone()
            };
            assert!(s.validate().is_err(), "会话 {bad} 小时应被拒");
        }
        // 边界必须放行（不然"最多 90 天"这句就是假的）
        assert!(AppSettings {
            min_password_len: 4,
            session_ttl_hours: 24 * 90,
            ..ok.clone()
        }
        .validate()
        .is_ok());

        // 容量那三格：范围两头都要卡住 ✓
        for bad in [HISTORY_LIMIT_RANGE.0 - 1, HISTORY_LIMIT_RANGE.1 + 1] {
            assert!(
                AppSettings {
                    history_limit: bad,
                    ..ok.clone()
                }
                .validate()
                .is_err(),
                "历史 {bad} 条应被拒"
            );
        }
        for bad in [LOG_LIMIT_PER_RUN_RANGE.0 - 1, LOG_LIMIT_PER_RUN_RANGE.1 + 1] {
            assert!(
                AppSettings {
                    log_limit_per_run: bad,
                    ..ok.clone()
                }
                .validate()
                .is_err(),
                "单次 {bad} 行应被拒"
            );
        }
        for bad in [
            SERVER_LOG_MEM_LINES_RANGE.0 - 1,
            SERVER_LOG_MEM_LINES_RANGE.1 + 1,
        ] {
            assert!(
                AppSettings {
                    server_log_mem_lines: bad,
                    ..ok.clone()
                }
                .validate()
                .is_err(),
                "服务端日志内存 {bad} 行应被拒"
            );
        }
        for bad in [
            SERVER_LOG_TOTAL_MB_RANGE.0 - 1,
            SERVER_LOG_TOTAL_MB_RANGE.1 + 1,
        ] {
            assert!(
                AppSettings {
                    server_log_total_mb: bad,
                    ..ok.clone()
                }
                .validate()
                .is_err(),
                "服务端日志总量 {bad} MB 应被拒"
            );
        }
        for bad in [LOG_TOTAL_MB_RANGE.0 - 1, LOG_TOTAL_MB_RANGE.1 + 1] {
            assert!(
                AppSettings {
                    log_total_mb: bad,
                    ..ok.clone()
                }
                .validate()
                .is_err(),
                "日志总量 {bad} MB 应被拒"
            );
        }
        assert!(AppSettings {
            history_limit: HISTORY_LIMIT_RANGE.0,
            log_limit_per_run: LOG_LIMIT_PER_RUN_RANGE.0,
            log_total_mb: LOG_TOTAL_MB_RANGE.0,
            ..ok.clone()
        }
        .validate()
        .is_ok());
        assert!(AppSettings {
            history_limit: HISTORY_LIMIT_RANGE.1,
            log_limit_per_run: LOG_LIMIT_PER_RUN_RANGE.1,
            log_total_mb: LOG_TOTAL_MB_RANGE.1,
            ..ok.clone()
        }
        .validate()
        .is_ok());
    }

    /// 老文件缺字段要能读出来 —— 这是"加设置项不让老实例失效"的保证。
    ///
    /// ⚠ 这一条是**升级路径**的钉子：`data/settings.json` 里那两格是 2026-10 之前
    /// 写下的，读回来必须补上三个新默认值（而不是报错、也不是 0 ✓ —— 0 会让
    /// "日志上限 0 MB"变成"一条日志都不留" ✗）。
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AppSettings::default());
        let part: AppSettings = serde_json::from_str(r#"{"min_password_len":12}"#).unwrap();
        assert_eq!(part.min_password_len, 12);
        assert_eq!(part.session_ttl_hours, DEFAULT_SESSION_TTL_HOURS);
        assert_eq!(part.history_limit, DEFAULT_HISTORY_LIMIT);
        assert_eq!(part.log_limit_per_run, DEFAULT_LOG_LIMIT_PER_RUN);
        assert_eq!(part.log_total_mb, DEFAULT_LOG_TOTAL_MB);
        // ⭐ 老实例那份（只有 `auth_refresh_at`、没有后来那些容量格 ✓）也要读得进，
        // 而且**钟点那个值要保住** —— 它是"每用户值的初始值"（见 `effective_at` ✓），
        // 读丢了就等于用户改过的钟点凭空回到 03:00 ✗。
        let old: AppSettings = serde_json::from_str(r#"{"auth_refresh_at":"22:30"}"#).unwrap();
        assert_eq!(old.auth_refresh_at, "22:30");
        assert_eq!(old.history_limit, DEFAULT_HISTORY_LIMIT, "其余格走默认值");
        assert_eq!(
            old.validate(),
            Ok(()),
            "老文件里那个钟点是合法的，不该让启动报错"
        );
    }

    /// 新三格能存能读（写盘 → 读回是同一份）✓。
    #[test]
    fn capacity_settings_roundtrip() {
        let s = AppSettings {
            history_limit: 500,
            log_limit_per_run: 5000,
            log_total_mb: 8,
            server_log_mem_lines: 500,
            server_log_total_mb: 4,
            ..AppSettings::default()
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"history_limit\":500"), "{j}");
        assert!(j.contains("\"log_total_mb\":8"), "{j}");
        assert!(j.contains("\"server_log_mem_lines\":500"), "{j}");
        assert!(j.contains("\"server_log_total_mb\":4"), "{j}");
        assert_eq!(serde_json::from_str::<AppSettings>(&j).unwrap(), s);
        assert_eq!(s.log_total_bytes(), 8 * 1024 * 1024);
        assert_eq!(s.server_log_total_bytes(), 4 * 1024 * 1024);
    }
}
