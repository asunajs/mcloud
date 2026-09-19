//! 旧项目（mcloud-sign）数据导入：`asign.json` + `packages/api/data/*.json` → SQLite。
//!
//! # 幂等靠什么成立
//!
//! 三道保险叠在一起：
//!
//! 1. **所有写入都是 UPSERT**，主键稳定（账号用 `AccountId`、定时任务/组合用自身 id、
//!    导入的运行记录用 `legacy-<内容哈希>`）。于是"再跑一次"只是把同样的行再写一遍。
//! 2. **按源文件 sha256 记台账**（`imports` 表）：源没变就整份跳过。这一条省的是时间，
//!    不是正确性 —— 正确性由第 1 条保证。
//! 3. **台账在所有写入之后才落**。中途失败时台账没写，下次重跑会从头补完，
//!    而不是留下"一半数据 + 声称已导入"。
//!
//! # 为什么不做整包事务
//!
//! 看起来很该做，但这套写入是几十条互相独立的 UPSERT，
//! 而**真正危险的一步（掩码冲突）在任何写入之前就检查完了**。
//! 为"崩在中间时不用重跑一遍"去把 `SqliteStore` 的十来个写方法重构成 `&Connection` 版本，
//! 收益远小于把已经测过的写入路径全改一遍的风险。
//! 将来一旦出现"必须要么全成要么全不成"的写入（跨表一致性约束），再一起改。
//!
//! # 绝不静默扩大目标范围
//!
//! 旧版有一处陷阱：`findAccount` 里纯数字字符串被当成序号，于是"用手机号当账号引用"
//! 永远解析不到（见 `mcloud_core::legacy` 模块头）。这类引用**不能默认落到"全部账号"** ——
//! 那是把"过去什么都没跑"变成"现在对所有人跑"，方向完全反了。所以：
//! [`Schedule`] 有 `enabled`，一律**导入为停用**并报出问题；
//! [`Pipeline`] 没有这个字段，只能**整条跳过**并报出问题。宁可少一条，不可错一条。
//!
//! # 已知取舍
//!
//! - 旧 `history.json` 的 `logs` / `result` **不进 `runs` 表**（新表没有这两列）。
//!   但源文件原文整份归档在 `imports.raw` 里，信息没丢，将来要回填随时能回填。
//! - `cron` 表达式**不在导入时校验**（Rust 侧的 cron 解析器属于 P5）。
//!   这里只拒绝空串；真正的合法性由加载时校验兜住。
//! - 旧 `history.json` 的 200 条上限**不在这里执行**：截断是运行期策略，
//!   迁移的原则是能搬的都搬走。

use crate::store::{ImportRecord, SqliteStore};
use chrono::{DateTime, Utc};
use mcloud_core::legacy::{
    LegacyAccountView, LegacyBinding, LegacyConfig, LegacyError, LegacyPipeline, LegacyRun,
    LegacySchedule, LegacyStep, ReferenceAdvice, describe_legacy_accounts, find_by_label,
    normalize_exchange_windows, normalize_pipeline_accounts, normalize_webui, parse_rfc3339,
};
use mcloud_core::{
    Account, AccountId, AccountMode, AccountState, ConfigStore, Error, ExchangeBinding,
    ExchangeConfig, Pipeline, PipelineStep, RunRecord, RunStatus, Schedule, ScheduleTarget, Scope,
    StateStore, WebuiConfig, md5_hex, sha256_hex,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 旧数据目录里需要导入的文件（相对于 `packages/api/data/`）。
pub const LEGACY_DATA_FILES: [&str; 6] = [
    "state.json",
    "history.json",
    "pipelines.json",
    "schedules.json",
    "exchange.json",
    "webui.json",
];

/// 旧版定时任务的时区（旧 `scheduler.ts` 的 `DEFAULT_TIMEZONE`）。
const DEFAULT_TZ: &str = "Asia/Shanghai";

// ═══════════════════════════════════════════════════════════════════════════
// 源位置
// ═══════════════════════════════════════════════════════════════════════════

/// 旧项目的两个数据位置。
///
/// 它们**不在一起**：账号配置在仓库根的 `asign.json`，运行数据在 `packages/api/data/`
/// （`storage.ts` 的 `DATA_DIR` 默认相对进程 cwd，从 `packages/api` 启动时就是那里）。
/// 分开传，免得将来有人把 `asign.json` 也挪进 `data/` 之后这里静默读不到。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySource {
    /// 旧 `asign.json` 路径。
    pub config_path: PathBuf,
    /// 旧 `packages/api/data/` 目录。
    pub data_dir: PathBuf,
}

impl LegacySource {
    pub fn new(config_path: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
            data_dir: data_dir.into(),
        }
    }

    /// 按旧项目布局从仓库根推导两个位置。
    pub fn from_repo_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            config_path: root.join("asign.json"),
            data_dir: root.join("packages/api/data"),
        }
    }

    /// 实际存在的源文件：`asign.json` + 6 个数据文件，不存在的直接跳过。
    ///
    /// 归档与自检都用它，免得"哪些文件算旧数据"这件事在两处各写一遍。
    pub fn existing_files(&self) -> Vec<(&'static str, PathBuf)> {
        std::iter::once(("asign.json", self.config_path.clone()))
            .chain(
                LEGACY_DATA_FILES
                    .iter()
                    .map(|name| (*name, self.data_dir.join(name))),
            )
            .filter(|(_, path)| path.is_file())
            .collect()
    }
}

/// 把旧数据**原样复制**到 `dest`，返回复制的文件数。
///
/// 这不是"改坏了能回滚"意义上的备份：旧目录自始至终只读，一个字节都不改。
/// 它的用途是让新项目自带一份原始数据 —— 旧仓库（`Documents/workspace/mcloud-sign`）
/// 将来只作归档、乃至某天不再可访问时，这次迁移仍然能被检查、被重跑。
///
/// 已存在且内容相同的文件会被跳过，所以重复执行不会做无谓的写盘。
pub fn archive_legacy_source(source: &LegacySource, dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest).map_err(|e| LegacyImportError::Io {
        path: dest.display().to_string(),
        source: e,
    })?;

    let mut copied = 0;
    for (name, from) in source.existing_files() {
        let to = dest.join(name);
        let text = read_text(&from)?;
        if std::fs::read_to_string(&to).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        std::fs::write(&to, &text).map_err(|e| LegacyImportError::Io {
            path: to.display().to_string(),
            source: e,
        })?;
        copied += 1;
    }
    Ok(copied)
}

// ═══════════════════════════════════════════════════════════════════════════
// 报告
// ═══════════════════════════════════════════════════════════════════════════

/// 迁移中发现的问题。
///
/// 每一条都要能指向一个**具体可修的东西**，并且说明"我们替你做了什么决定"。
/// 迁移最忌讳"猜一个合理的值然后不说" —— 那会让问题延后到某天记录对不上时才爆出来。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationIssue {
    #[error(
        "{at}：账号引用「{digits}」在旧版被当成序号（越界）因而**从未生效**，\
         但按手机号能唯一匹配到 {account}；已按手机号导入，请确认目标是否正确"
    )]
    RecoveredAccountRef {
        at: String,
        digits: String,
        account: String,
    },

    #[error(
        "{at}：账号引用「{raw}」在旧版就没有生效过，现在也匹配不到任何账号；已{action}，请人工确认"
    )]
    DeadAccountRef {
        at: String,
        raw: String,
        action: &'static str,
    },

    #[error("{at}：缺少执行目标（`task` 与 `pipelineId` 都没有）；已停用，请指定目标后再启用")]
    MissingTarget { at: String },

    #[error("{at}：cron 表达式为空；已停用")]
    EmptyCron { at: String },

    #[error("{at}：步骤级 `account` 已废弃（旧版读取时就会丢掉）；该字段未导入")]
    DroppedStepAccount { at: String },

    #[error("{at}：没有任何步骤；已跳过整条组合")]
    EmptySteps { at: String },

    #[error("{at}：整条组合被跳过（账号引用无法解析，导入它会改变它的作用范围）")]
    PipelineSkipped { at: String },

    #[error("state.json：账号「{key}」在账号配置里找不到；已跳过（不凭空造账号）")]
    OrphanAccountState { key: String },

    #[error("state.json：账号「{key}」的状态是空的；已跳过")]
    EmptyAccountState { key: String },

    #[error("state.json：账号「{key}」的状态无法解析（{error}）；已跳过该账号")]
    UnparsableAccountState { key: String, error: String },

    #[error("state.json：账号「{key}」出现过未识别字段 {fields:?}；这些字段未导入（原文已归档）")]
    UnknownStateFields { key: String, fields: Vec<String> },

    #[error("{file} 第 {index} 条无法解析（{error}）；已跳过该条，其余照常导入")]
    UnparsableRecord {
        file: String,
        index: usize,
        error: String,
    },

    #[error("{file} 第 {index} 条的起止时间非法（start={start:?}）；已跳过该条")]
    BadRunTimestamp {
        file: String,
        index: usize,
        start: String,
    },

    #[error("history.json 第 {index} 条的展示名「{label}」{reason}；该条已导入但**未归属账号**")]
    UnresolvedRunAccount {
        index: usize,
        label: String,
        reason: &'static str,
    },

    #[error("exchange.json 的抢兑时刻非法（{error}）；已回落到默认窗口")]
    BadExchangeWindows { error: String },

    #[error("exchange.json 第 {index} 条绑定的奖品 ID 非法（{raw:?}）；已跳过该绑定")]
    BadPrizeId { index: usize, raw: String },

    #[error("exchange.json 第 {index} 条绑定没有指定账号；已跳过该绑定")]
    BindingWithoutAccount { index: usize },

    #[error("{path} 不存在；已跳过（该文件不是每台机器都有）")]
    MissingFile { path: String },
}

/// 导入结果。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// 导入的账号数。
    pub accounts: usize,
    /// 导入的账号登录态数。
    pub account_states: usize,
    pub schedules: usize,
    pub pipelines: usize,
    /// 导入的运行历史条数。
    pub runs: usize,
    /// 因源文件未变而整份跳过的文件。
    pub skipped_unchanged: Vec<String>,
    pub issues: Vec<MigrationIssue>,
}

impl MigrationReport {
    pub fn has_issues(&self) -> bool {
        !self.issues.is_empty()
    }

    /// 一行式摘要（可安全进日志：不含任何凭据）。
    pub fn summary(&self) -> String {
        format!(
            "账号 {} / 登录态 {} / 定时任务 {} / 组合 {} / 历史 {} 条；跳过未变的文件 {} 个；问题 {} 个",
            self.accounts,
            self.account_states,
            self.schedules,
            self.pipelines,
            self.runs,
            self.skipped_unchanged.len(),
            self.issues.len()
        )
    }

    /// 问题清单（给 CLI 逐条打印）。
    pub fn issue_lines(&self) -> Vec<String> {
        self.issues.iter().map(|i| i.to_string()).collect()
    }

    /// 机器可读形态（CLI `--json` 与服务端共用一套字段名）。
    ///
    /// 问题只以**字符串**输出：枚举的内部形状是实现细节，
    /// 让脚本去依赖它等于把重构成本转嫁给使用者。
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "accounts": self.accounts,
            "accountStates": self.account_states,
            "schedules": self.schedules,
            "pipelines": self.pipelines,
            "runs": self.runs,
            "skippedUnchanged": self.skipped_unchanged,
            "issues": self.issue_lines(),
            "summary": self.summary(),
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 错误
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, thiserror::Error)]
pub enum LegacyImportError {
    #[error("读取 {path} 失败: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("解析 {path} 失败: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
    /// **这一条是"什么都还没写"的那个失败**：掩码冲突会在任何写入之前中止。
    #[error("账号身份映射失败，已中止迁移且未写入任何数据：{0}")]
    Identity(#[from] LegacyError),
    /// 库里还没有用户，账号没有人可以归。
    ///
    /// 单独成一个变体，是为了让前端能给出**可执行**的提示（"先跑 `mcloud user init`"），
    /// 而不是把它混进"数据库操作失败"里 —— 后者会让人去查数据库，
    /// 而这件事跟数据库一点关系都没有。
    #[error("库里还没有用户，导入的账号将无人可归：请先执行 `mcloud user init`")]
    NoOwner,
    #[error("数据库操作失败: {0}")]
    Store(#[from] Error),
}

pub type Result<T> = std::result::Result<T, LegacyImportError>;

// ═══════════════════════════════════════════════════════════════════════════
// 入口
// ═══════════════════════════════════════════════════════════════════════════

/// 把旧项目数据导入 `store`。可重复执行（幂等说明见模块头）。
///
/// `now` 显式传入而不是内部取 `Utc::now()`：台账的 `imported_at` 因此可测，
/// 也让"所有时间都从外部进来"这条纪律在 io 层同样成立。
///
/// # `scope` 决定导入的账号归谁
///
/// 账号从 v3 起有归属（`accounts.owner`），而**无主账号对谁都不可见**（见
/// `StateStore` 的隔离契约）。于是：
///
/// - [`Scope::User`]：导入的账号归那个用户 —— `user init` 之后由管理员执行的迁移；
/// - [`Scope::Local`]：导入的账号**保持无主**（`owner IS NULL`），
///   等之后初始化管理员时被 `claim_unowned_accounts` 认领。
///
/// 后者是 CLI 的路径，也是"先 migrate 再 init"这个顺序能成立的**原因**：
/// 本地运维本来就能操作所有数据，没必要为了搬旧数据先造一个用户。
pub async fn import_legacy(
    store: &SqliteStore,
    scope: Scope<'_>,
    source: &LegacySource,
    now: DateTime<Utc>,
) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();

    // ── 1. 账号清单：每次都重新读（后面的引用解析都要用），但只在源变了才写 ──
    let config_text = read_text(&source.config_path)?;
    let config: LegacyConfig = parse_whole(&source.config_path, &config_text)?;
    let views = describe_legacy_accounts(&config.caiyun);

    // ── 2. 掩码冲突检查：**必须在任何写入之前** ──
    // 两个手机号前 2 后 2 相同时会塌缩成一个 AccountId，历史数据会混在一起且不报错。
    // 这里直接失败，代价是"迁移没做"，而不是"迁移做错了"。
    let identity = mcloud_core::legacy::build_identity_map(&views)?;

    if source_changed(store, &source.config_path, &config_text)? {
        let written = write_accounts(store, scope, &views).await?;
        report.accounts = written as usize;
        commit_ledger(
            store,
            &source.config_path,
            &config_text,
            written,
            &report,
            now,
        )?;
    }

    // ── 3. 逐个数据文件 ──
    for name in LEGACY_DATA_FILES {
        let path = source.data_dir.join(name);
        if !path.is_file() {
            report.issues.push(MigrationIssue::MissingFile {
                path: path.display().to_string(),
            });
            continue;
        }
        let text = read_text(&path)?;
        if !source_changed(store, &path, &text)? {
            report.skipped_unchanged.push(name.to_string());
            continue;
        }
        let value: Value = parse_whole(&path, &text)?;

        let issues_before = report.issues.len();
        let written = match name {
            "state.json" => write_state(store, scope, &value, &identity, &mut report).await?,
            "history.json" => write_history(store, scope, &value, &views, &mut report).await?,
            "pipelines.json" => write_pipelines(store, scope, &value, &views, &mut report).await?,
            "schedules.json" => write_schedules(store, scope, &value, &views, &mut report).await?,
            "exchange.json" => write_exchange(store, &value, &views, &mut report).await?,
            "webui.json" => write_webui(store, &value, &mut report).await?,
            other => unreachable!("LEGACY_DATA_FILES 里没有 {other}"),
        };
        let new_issues = (report.issues.len() - issues_before) as i64;

        // 台账最后落：中途失败时台账没写，下次重跑会补完
        store.record_import(&ImportRecord {
            source: path.display().to_string(),
            sha256: sha256_hex(text.as_bytes()),
            imported_at: now.to_rfc3339(),
            record_count: written,
            issues: new_issues,
            raw: Some(text),
        })?;
    }

    Ok(report)
}

/// 只读预演：把导入跑在一个**内存库**上，跑完连库一起丢掉。
///
/// 为什么不另写一套"只检查、不写入"的代码：那样诊断逻辑和真实导入会各自演化，
/// 迟早出现"预演说没问题、真跑却报错"。这里让两者**走完全同一条路径**，
/// 唯一的区别是最后那个库被丢掉 —— 于是漂移在结构上不可能发生。
pub async fn diagnose_legacy(source: &LegacySource, now: DateTime<Utc>) -> Result<MigrationReport> {
    let scratch = SqliteStore::open_in_memory().map_err(LegacyImportError::Store)?;

    // 预演走 `Scope::Local`：它与真实导入是**同一条路径**，只是最后把库丢掉。
    // 这里不再需要造占位用户了 —— 本地范围不要求账号有主人
    //（导入的账号保持无主，真实迁移之后由认领流程接手）。
    import_legacy(&scratch, Scope::Local, source, now).await
}

// ═══════════════════════════════════════════════════════════════════════════
// 账号
// ═══════════════════════════════════════════════════════════════════════════

async fn write_accounts(
    store: &SqliteStore,
    scope: Scope<'_>,
    views: &[LegacyAccountView],
) -> Result<i64> {
    for view in views {
        let account = Account {
            id: view.account_id.clone(),
            // 导入即归属：旧数据里的账号交给执行导入的这个用户
            owner: scope.owner().cloned(),
            nickname: view.nickname.clone(),
            // 手机号非空时 AccountId 本身就是掩码，直接复用，避免两处掩码逻辑跑偏
            phone_masked: (!view.phone.is_empty()).then(|| view.account_id.as_str().to_string()),
            platform: view.platform.clone(),
            enabled: true,
            // 保持旧配置里的顺序
            sort_order: view.index as i64 - 1,
            // `0` 在旧语义里是"没有过期信息"，不是"1970 年过期"
            auth_expire: (view.auth_expire != 0).then_some(view.auth_expire),
            ..Account::new(view.account_id.as_str())
        };
        store.upsert_account(scope, &account).await?;
    }
    Ok(views.len() as i64)
}

async fn write_state(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    identity: &BTreeMap<String, AccountId>,
    report: &mut MigrationReport,
) -> Result<i64> {
    let Some(accounts) = value.get("accounts").and_then(Value::as_object) else {
        return Ok(0);
    };

    let mut written = 0i64;
    for (key, raw) in accounts {
        let state: mcloud_core::legacy::LegacyAccountState =
            match serde_json::from_value(raw.clone()) {
                Ok(state) => state,
                Err(err) => {
                    report.issues.push(MigrationIssue::UnparsableAccountState {
                        key: key.clone(),
                        error: err.to_string(),
                    });
                    continue;
                }
            };

        // 未知字段如实报出，而不是静默丢弃 —— 旧版本写过的东西凭什么当它不存在
        let unknown = state.unknown_fields();
        if !unknown.is_empty() {
            report.issues.push(MigrationIssue::UnknownStateFields {
                key: key.clone(),
                fields: unknown.iter().map(|f| (*f).to_string()).collect(),
            });
        }

        if state.is_empty() {
            report
                .issues
                .push(MigrationIssue::EmptyAccountState { key: key.clone() });
            continue;
        }

        // 键是旧 `accountKey`（手机号原文/昵称/"account"）。映射不到就不导入：
        // 凭空造一个账号会把"这是谁的数据"这件事变成永久的谜
        let Some(account_id) = identity.get(key) else {
            report
                .issues
                .push(MigrationIssue::OrphanAccountState { key: key.clone() });
            continue;
        };

        store
            .upsert_account_state(
                scope,
                &AccountState {
                    account_id: account_id.clone(),
                    jwt_token: state.jwt_token,
                    device_id: state.device_id,
                    user_domain_id: state.user_domain_id,
                    cloud_phone_redpack_token: state.cloud_phone_redpack_token,
                    // 旧 state.json 没有时间戳字段；"什么时候导入的"看台账
                    updated_at: None,
                },
            )
            .await?;
        written += 1;
    }

    report.account_states = written as usize;
    Ok(written)
}

// ═══════════════════════════════════════════════════════════════════════════
// 运行历史
// ═══════════════════════════════════════════════════════════════════════════

async fn write_history(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    let items = value
        .get("runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut written = 0i64;
    for (index, run) in parse_each_indexed::<LegacyRun>("history.json", &items, report) {
        let Some(started_at) = parse_rfc3339(&run.start) else {
            report.issues.push(MigrationIssue::BadRunTimestamp {
                file: "history.json".into(),
                index: index + 1,
                start: run.start.clone(),
            });
            continue;
        };

        // `runs[].account` 存的是**展示名**，不是标识 —— 这是旧版留下的第二个标识体系
        let account_id = match find_by_label(views, &run.account).as_slice() {
            [only] => Some(only.account_id.clone()),
            [] => {
                report.issues.push(MigrationIssue::UnresolvedRunAccount {
                    index: index + 1,
                    label: run.account.clone(),
                    reason: "匹配不到任何账号",
                });
                None
            }
            _ => {
                report.issues.push(MigrationIssue::UnresolvedRunAccount {
                    index: index + 1,
                    label: run.account.clone(),
                    reason: "对应多个账号（展示名重复）",
                });
                None
            }
        };

        store
            .insert_run(
                scope,
                &RunRecord {
                    id: legacy_run_id(&run),
                    account_id,
                    capability: run.task.clone(),
                    status: if run.ok {
                        RunStatus::Done
                    } else {
                        RunStatus::Failed
                    },
                    started_at,
                    finished_at: parse_rfc3339(&run.end),
                    // 旧数据从未采到"净增云朵"（原项目该字段的写入被注释掉了），
                    // 编不出来就不编：留空比瞎填一个 0 诚实
                    cloud_gained: None,
                    cloud_after: None,
                    message: run.error.clone(),
                },
            )
            .await?;
        written += 1;
    }

    report.runs = written as usize;
    Ok(written)
}

/// 旧运行记录的稳定 id：内容相同即同一条。
///
/// 用内容哈希而不是自增序号，是为了让"重跑导入"天然不产生重复行。
/// 键取 `task|account|start|end` —— 这四段就是旧记录的**身份**：
/// 重排文件、加空行都不会让同一条历史换个 id。
///
/// `logs` / `result` **刻意不参与哈希**：它们体积大（夹具里 history.json 有 97KB，
/// 几乎全是日志），且同一账号同一时刻不可能有两条不同的运行。
/// 把它们算进去只会让"日志被截断上限动过一次"变成一次全量 id 变更。
fn legacy_run_id(run: &LegacyRun) -> String {
    let canonical = format!("{}|{}|{}|{}", run.task, run.account, run.start, run.end);
    format!("legacy-{}", &md5_hex(canonical.as_bytes())[..16])
}

// ═══════════════════════════════════════════════════════════════════════════
// 任务组合
// ═══════════════════════════════════════════════════════════════════════════

async fn write_pipelines(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    let items = value
        .get("pipelines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut written = 0i64;
    for (_, pipeline) in parse_each_indexed::<LegacyPipeline>("pipelines.json", &items, report) {
        let at = format!("pipelines.json「{}」", pipeline.name);

        if pipeline.steps.is_empty() {
            report
                .issues
                .push(MigrationIssue::EmptySteps { at: at.clone() });
            continue;
        }

        // 废弃的步骤级账号：旧版读取时就丢掉，这里也丢，但要说一声
        for (index, step) in pipeline.steps.iter().enumerate() {
            if step.legacy_account.is_some() {
                report.issues.push(MigrationIssue::DroppedStepAccount {
                    at: format!("{at} 第 {} 步", index + 1),
                });
            }
        }

        let account_ids = match resolve_pipeline_accounts(views, &pipeline, &at, report) {
            Ok(ids) => ids,
            Err(()) => {
                // 组合没有 `enabled` 字段，没法"导入但停用"，只能整条跳过
                report
                    .issues
                    .push(MigrationIssue::PipelineSkipped { at: at.clone() });
                continue;
            }
        };

        store
            .upsert_pipeline(
                scope,
                &Pipeline {
                    id: non_empty(&pipeline.id)
                        .unwrap_or_else(|| fallback_id("pipe", &pipeline.name)),
                    owner: scope.owner().cloned(),
                    name: if pipeline.name.is_empty() {
                        "未命名组合".to_string()
                    } else {
                        pipeline.name.clone()
                    },
                    description: pipeline.description.clone(),
                    steps: pipeline
                        .steps
                        .iter()
                        .map(|step: &LegacyStep| PipelineStep {
                            task: step.task.clone(),
                            body: step.body.clone(),
                            continue_on_error: step.continue_on_error,
                        })
                        .collect(),
                    account_ids,
                    account_mode: match pipeline.account_mode.as_deref() {
                        Some("parallel") => AccountMode::Parallel,
                        _ => AccountMode::Sequential,
                    },
                    // 旧版注释写明"默认 true"
                    stop_on_error: pipeline.stop_on_error.unwrap_or(true),
                    created_at: pipeline.created_at.as_deref().and_then(parse_rfc3339),
                    updated_at: pipeline.updated_at.as_deref().and_then(parse_rfc3339),
                    last_run_at: pipeline.last_run_at.as_deref().and_then(parse_rfc3339),
                    last_status: pipeline
                        .last_status
                        .as_deref()
                        .and_then(parse_pipeline_status),
                    last_error: pipeline.last_error.clone(),
                    last_duration_ms: pipeline.last_duration_ms,
                },
            )
            .await?;
        written += 1;
    }

    report.pipelines = written as usize;
    Ok(written)
}

/// 解析组合的账号选择。**任一条无法解析就返回 `Err`** —— 组合没有"停用"这个状态，
/// 少导入一个账号等于悄悄扩大了它的作用范围。
fn resolve_pipeline_accounts(
    views: &[LegacyAccountView],
    pipeline: &LegacyPipeline,
    at: &str,
    report: &mut MigrationReport,
) -> std::result::Result<Vec<AccountId>, ()> {
    let Some(values) = normalize_pipeline_accounts(
        pipeline.accounts.as_deref(),
        pipeline.legacy_account.as_ref(),
    ) else {
        // `None` = 全部账号
        return Ok(Vec::new());
    };

    let mut ids = Vec::with_capacity(values.len());
    for reference in &values {
        match mcloud_core::legacy::advise_reference(views, Some(reference)) {
            ReferenceAdvice::AllAccounts => {
                // `accounts` 里出现空值已被 normalize 剔除，走到这里说明是别的东西
                report.issues.push(MigrationIssue::DeadAccountRef {
                    at: at.to_string(),
                    raw: reference.to_string(),
                    action: "跳过整条组合",
                });
                return Err(());
            }
            ReferenceAdvice::Same { index } => ids.push(views[index - 1].account_id.clone()),
            ReferenceAdvice::RecoveredFromPhone { digits, index } => {
                report.issues.push(MigrationIssue::RecoveredAccountRef {
                    at: at.to_string(),
                    digits,
                    account: views[index - 1].account_id.as_str().to_string(),
                });
                ids.push(views[index - 1].account_id.clone());
            }
            ReferenceAdvice::WasNeverEffective { raw } => {
                report.issues.push(MigrationIssue::DeadAccountRef {
                    at: at.to_string(),
                    raw,
                    action: "跳过整条组合",
                });
                return Err(());
            }
        }
    }
    Ok(ids)
}

// ═══════════════════════════════════════════════════════════════════════════
// 定时任务
// ═══════════════════════════════════════════════════════════════════════════

async fn write_schedules(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    let items = value
        .get("schedules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut written = 0i64;
    for (_, schedule) in parse_each_indexed::<LegacySchedule>("schedules.json", &items, report) {
        let at = format!("schedules.json「{}」", schedule.name);

        // 旧版 `if (!schedule.enabled) return;` —— 缺字段等于 falsy，等于不启用
        let mut enabled = schedule.enabled.unwrap_or(false);

        if schedule.cron.trim().is_empty() {
            report
                .issues
                .push(MigrationIssue::EmptyCron { at: at.clone() });
            enabled = false;
        }

        // 目标是二选一；旧版优先 pipelineId
        let target = match (
            non_empty(schedule.pipeline_id.as_deref().unwrap_or("")),
            non_empty(schedule.task.as_deref().unwrap_or("")),
        ) {
            (Some(id), _) => Some(ScheduleTarget::Pipeline { id }),
            (None, Some(code)) => Some(ScheduleTarget::Capability { code }),
            (None, None) => {
                report
                    .issues
                    .push(MigrationIssue::MissingTarget { at: at.clone() });
                enabled = false;
                None
            }
        };

        let account_ids = match schedule.account.as_ref() {
            None => Vec::new(), // 未指定 = 全部账号
            Some(reference) => {
                match mcloud_core::legacy::advise_reference(views, Some(reference)) {
                    ReferenceAdvice::AllAccounts => Vec::new(),
                    ReferenceAdvice::Same { index } => vec![views[index - 1].account_id.clone()],
                    ReferenceAdvice::RecoveredFromPhone { digits, index } => {
                        report.issues.push(MigrationIssue::RecoveredAccountRef {
                            at: at.clone(),
                            digits,
                            account: views[index - 1].account_id.as_str().to_string(),
                        });
                        vec![views[index - 1].account_id.clone()]
                    }
                    ReferenceAdvice::WasNeverEffective { raw } => {
                        // 关键：**不能**回落到"全部账号"
                        report.issues.push(MigrationIssue::DeadAccountRef {
                            at: at.clone(),
                            raw,
                            action: "停用该定时任务",
                        });
                        enabled = false;
                        Vec::new()
                    }
                }
            }
        };

        store
            .upsert_schedule(
                scope,
                &Schedule {
                    id: non_empty(&schedule.id)
                        .unwrap_or_else(|| fallback_id("sched", &schedule.name)),
                    owner: scope.owner().cloned(),
                    name: if schedule.name.is_empty() {
                        "未命名任务".to_string()
                    } else {
                        schedule.name.clone()
                    },
                    target,
                    cron: schedule.cron.clone(),
                    timezone: non_empty(schedule.timezone.as_deref().unwrap_or(""))
                        .unwrap_or_else(|| DEFAULT_TZ.to_string()),
                    account_ids,
                    body: schedule.body.clone(),
                    enabled,
                    created_at: schedule.created_at.as_deref().and_then(parse_rfc3339),
                    last_run_at: schedule.last_run_at.as_deref().and_then(parse_rfc3339),
                    last_status: schedule
                        .last_status
                        .as_deref()
                        .and_then(parse_schedule_status),
                    last_error: schedule.last_error.clone(),
                },
            )
            .await?;
        written += 1;
    }

    report.schedules = written as usize;
    Ok(written)
}

// ═══════════════════════════════════════════════════════════════════════════
// 定时抢兑
// ═══════════════════════════════════════════════════════════════════════════

async fn write_exchange(
    store: &SqliteStore,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    // 单对象文件：整体解析。手改坏了就应该报出来，而不是按"每条独立"硬凑一半
    let file: mcloud_core::legacy::LegacyExchangeFile = match serde_json::from_value(value.clone())
    {
        Ok(file) => file,
        Err(err) => {
            report.issues.push(MigrationIssue::UnparsableRecord {
                file: "exchange.json".into(),
                index: 0,
                error: err.to_string(),
            });
            return Ok(0);
        }
    };

    let windows = match normalize_exchange_windows(&file.windows) {
        Ok(windows) => windows,
        Err(err) => {
            report.issues.push(MigrationIssue::BadExchangeWindows {
                error: err.to_string(),
            });
            ExchangeConfig::default().windows
        }
    };

    let mut bindings: Vec<ExchangeBinding> = Vec::new();
    for (index, binding) in file.bindings.iter().enumerate() {
        let Some(prize_id) = read_prize_id(binding) else {
            report.issues.push(MigrationIssue::BadPrizeId {
                index,
                raw: binding
                    .prize_id
                    .as_ref()
                    .map(Value::to_string)
                    .unwrap_or_default(),
            });
            continue;
        };

        let Some(reference) = binding.account.as_ref() else {
            report
                .issues
                .push(MigrationIssue::BindingWithoutAccount { index });
            continue;
        };

        let account_id = match mcloud_core::legacy::advise_reference(views, Some(reference)) {
            ReferenceAdvice::AllAccounts => {
                report
                    .issues
                    .push(MigrationIssue::BindingWithoutAccount { index });
                continue;
            }
            ReferenceAdvice::Same { index: at } => views[at - 1].account_id.clone(),
            ReferenceAdvice::RecoveredFromPhone { digits, index: at } => {
                report.issues.push(MigrationIssue::RecoveredAccountRef {
                    at: format!("exchange.json 第 {index} 条绑定"),
                    digits,
                    account: views[at - 1].account_id.as_str().to_string(),
                });
                views[at - 1].account_id.clone()
            }
            ReferenceAdvice::WasNeverEffective { raw } => {
                report.issues.push(MigrationIssue::DeadAccountRef {
                    at: format!("exchange.json 第 {index} 条绑定"),
                    raw,
                    action: "跳过该绑定",
                });
                continue;
            }
        };

        // 旧版按 `account:prizeId` 去重，保留首次出现
        if bindings
            .iter()
            .any(|b| b.account_id == account_id && b.prize_id == prize_id)
        {
            continue;
        }
        bindings.push(ExchangeBinding {
            account_id,
            prize_id,
            prize_name: binding.prize_name.clone(),
        });
    }

    store
        .put_exchange_config(&ExchangeConfig {
            enabled: file.enabled && !bindings.is_empty(),
            windows,
            bindings,
            schedule_ids: file.schedule_ids.clone(),
            updated_at: file.updated_at.as_deref().and_then(parse_rfc3339),
        })
        .await?;

    Ok(1)
}

/// 奖项 ID：旧版走 `Number(...)`，所以数字字符串也算数。
fn read_prize_id(binding: &LegacyBinding) -> Option<i64> {
    let value = binding.prize_id.as_ref()?;
    let parsed = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))?;
    (parsed > 0).then_some(parsed)
}

// ═══════════════════════════════════════════════════════════════════════════
// 界面偏好
// ═══════════════════════════════════════════════════════════════════════════

async fn write_webui(
    store: &SqliteStore,
    value: &Value,
    report: &mut MigrationReport,
) -> Result<i64> {
    let file: mcloud_core::legacy::LegacyWebuiFile = match serde_json::from_value(value.clone()) {
        Ok(file) => file,
        Err(err) => {
            report.issues.push(MigrationIssue::UnparsableRecord {
                file: "webui.json".into(),
                index: 0,
                error: err.to_string(),
            });
            // 读不出来就用默认值：界面偏好不值得让一次迁移失败
            return write_webui_default(store).await;
        }
    };
    store.put_webui_config(&normalize_webui(&file)).await?;
    Ok(1)
}

async fn write_webui_default(store: &SqliteStore) -> Result<i64> {
    store.put_webui_config(&WebuiConfig::default()).await?;
    Ok(0)
}

// ═══════════════════════════════════════════════════════════════════════════
// 小工具
// ═══════════════════════════════════════════════════════════════════════════

fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| LegacyImportError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

fn parse_whole<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    serde_json::from_str(text).map_err(|e| LegacyImportError::Parse {
        path: path.display().to_string(),
        source: e,
    })
}

/// 逐条解析数组元素，坏的那条**只丢自己**。
///
/// 与 `mcloud_core::legacy::parse_each` 的区别：保留下标。
/// 报告里写「第 3 条」却指不出是哪一条，等于没说 —— 所以这里宁可自己留下标。
/// 下标对外是 **1 起**（给人看的），对内仍是 0 起。
fn parse_each_indexed<T: serde::de::DeserializeOwned>(
    file: &str,
    items: &[Value],
    report: &mut MigrationReport,
) -> Vec<(usize, T)> {
    let mut parsed = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match serde_json::from_value::<T>(item.clone()) {
            Ok(value) => parsed.push((index, value)),
            Err(err) => report.issues.push(MigrationIssue::UnparsableRecord {
                file: file.to_string(),
                index: index + 1,
                error: err.to_string(),
            }),
        }
    }
    parsed
}

/// 源内容与台账里的 sha256 不一致（或从没导入过）时为 `true`。
fn source_changed(store: &SqliteStore, path: &Path, text: &str) -> Result<bool> {
    let digest = sha256_hex(text.as_bytes());
    Ok(match store.import_record(&path.display().to_string())? {
        Some(record) => record.sha256 != digest,
        None => true,
    })
}

fn commit_ledger(
    store: &SqliteStore,
    path: &Path,
    text: &str,
    records: i64,
    report: &MigrationReport,
    now: DateTime<Utc>,
) -> Result<()> {
    store.record_import(&ImportRecord {
        source: path.display().to_string(),
        sha256: sha256_hex(text.as_bytes()),
        imported_at: now.to_rfc3339(),
        record_count: records,
        issues: report.issues.len() as i64,
        raw: Some(text.to_string()),
    })?;
    Ok(())
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

/// 旧记录缺 id 时用的兜底 id。
///
/// 用**名称的内容哈希**而不是下标：下标会在文件重排后变，那就等于每次重排都换一批新记录，
/// 幂等立刻失效。
fn fallback_id(kind: &str, name: &str) -> String {
    format!("legacy-{kind}-{}", &md5_hex(name.as_bytes())[..12])
}

fn parse_schedule_status(value: &str) -> Option<mcloud_core::ScheduleStatus> {
    match value {
        "ok" => Some(mcloud_core::ScheduleStatus::Ok),
        "error" => Some(mcloud_core::ScheduleStatus::Error),
        "skipped" => Some(mcloud_core::ScheduleStatus::Skipped),
        _ => None,
    }
}

fn parse_pipeline_status(value: &str) -> Option<mcloud_core::PipelineStatus> {
    match value {
        "ok" => Some(mcloud_core::PipelineStatus::Ok),
        "error" => Some(mcloud_core::PipelineStatus::Error),
        "partial" => Some(mcloud_core::PipelineStatus::Partial),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 测试
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Role, Scope, User, UserId, UserStore};
    use mcloud_core::{ScheduleStatus, WEBUI_PAGES};
    use std::path::PathBuf;

    /// 真实脱敏夹具所在目录（`test/fixtures/legacy/`）。
    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test/fixtures/legacy");

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("内存库应可打开")
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-17T00:00:00Z")
            .expect("固定时刻")
            .with_timezone(&Utc)
    }

    /// 测试用的导入者。
    ///
    /// 账号从 v3 起有归属，而 `accounts.owner` 有外键指向 `users` ——
    /// 也就是说**导入之前必须真的有一个用户**。这正是生产路径的要求
    /// （先 `user init` 再 `migrate`），测试里同样照办，不给自己开免检通道。
    const TEST_USER: &str = "test-user";

    fn uid() -> UserId {
        UserId::new(TEST_USER)
    }

    async fn ensure_user(store: &SqliteStore) {
        store
            .upsert_user(&User::new(TEST_USER, TEST_USER, "!", Role::Admin, now()))
            .await
            .expect("建测试用户");
    }

    /// 导入的统一入口：建好首个用户，并把旧数据导到它名下。
    ///
    /// 这里刻意走 `Scope::User` 而不是 `Scope::Local`：两者在导入器里的差别只是
    /// "账号归谁"，而这些用例关心的正是归属正确 —— 所以用 User 范围覆盖它。
    /// `Scope::Local`（账号保持无主）由 `diagnose_legacy` 的预演用例覆盖。
    async fn run_import(store: &SqliteStore, source: &LegacySource) -> Result<MigrationReport> {
        ensure_user(store).await;
        import_legacy(store, Scope::user(&uid()), source, now()).await
    }

    /// 夹具目录本身就是合法的旧项目布局：`asign.json` 与数据文件同目录。
    fn fixture_source() -> LegacySource {
        LegacySource::new(
            PathBuf::from(FIXTURES).join("asign.json"),
            PathBuf::from(FIXTURES),
        )
    }

    /// 把夹具整份复制到临时目录，供"改坏一个文件"的场景使用。
    fn copy_fixtures() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("临时目录");
        for name in std::iter::once("asign.json").chain(LEGACY_DATA_FILES) {
            let text = std::fs::read_to_string(PathBuf::from(FIXTURES).join(name))
                .unwrap_or_else(|e| panic!("读夹具 {name} 失败: {e}"));
            std::fs::write(dir.path().join(name), text).expect("写夹具");
        }
        dir
    }

    fn source_of(dir: &tempfile::TempDir) -> LegacySource {
        LegacySource::new(dir.path().join("asign.json"), dir.path())
    }

    fn write_file(dir: &tempfile::TempDir, name: &str, body: &str) {
        std::fs::write(dir.path().join(name), body).expect("写文件");
    }

    fn remove_file(dir: &tempfile::TempDir, name: &str) {
        std::fs::remove_file(dir.path().join(name)).expect("删文件");
    }

    /// 夹具里两个账号的掩码 AccountId（前 2 + `****` + 后 2）。
    const ACC_1: &str = "19****01"; // 手机号 19900000001，昵称「账号2」
    const ACC_2: &str = "19****02"; // 手机号 19900000002，昵称「账号1」

    fn id(raw: &str) -> AccountId {
        AccountId::new(raw)
    }

    // ── 端到端 ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fixture_import_lands_every_entity() {
        let s = store();
        let report = run_import(&s, &fixture_source())
            .await
            .expect("真实夹具应可导入");

        assert_eq!(report.accounts, 2);
        assert_eq!(report.account_states, 2);
        assert_eq!(report.pipelines, 1, "夹具里只有 1 条组合");
        assert_eq!(report.schedules, 0, "夹具的 schedules.json 是空的");
        assert_eq!(report.runs, 12, "夹具里有 12 条运行历史");
        assert!(
            report.skipped_unchanged.is_empty(),
            "首次导入不该跳过任何文件，实际跳过 {:?}",
            report.skipped_unchanged
        );
        assert!(
            !report.has_issues(),
            "干净的旧数据不该产生问题，实际：{:#?}",
            report.issue_lines()
        );

        // 落库结果与报告一致 —— 报告说导了却查不到是最坏的失败方式
        assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
        assert_eq!(
            s.list_runs(Scope::user(&uid()), 100).await.unwrap().len(),
            12
        );
        assert_eq!(
            s.list_pipelines(Scope::user(&uid())).await.unwrap().len(),
            1
        );
        assert!(
            s.list_schedules(Scope::user(&uid()))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn accounts_are_keyed_by_mask_not_by_phone() {
        let s = store();
        run_import(&s, &fixture_source()).await.unwrap();

        let accounts = s.list_accounts(Scope::user(&uid())).await.unwrap();
        let ids: Vec<&str> = accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec![ACC_1, ACC_2], "应按配置顺序落库");

        // 手机号原文**不得**成为账号主键：那会把可识别信息带进所有下游表
        assert!(
            accounts.iter().all(|a| !a.id.as_str().contains("1990000")),
            "账号 id 不应是手机号原文"
        );

        let first = s
            .get_account(Scope::user(&uid()), &id(ACC_1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.nickname.as_deref(), Some("账号2"));
        assert_eq!(first.phone_masked.as_deref(), Some(ACC_1));
        assert_eq!(first.sort_order, 0);
        assert!(first.enabled);
        // 夹具的 expire 是 13 位毫秒（2030-01-01），不是 0，所以应当带过来
        assert_eq!(first.auth_expire, Some(1_893_456_000_000));
    }

    #[tokio::test]
    async fn state_keys_are_remapped_through_the_identity_map() {
        let s = store();
        run_import(&s, &fixture_source()).await.unwrap();

        // state.json 的键是手机号原文；迁移后必须挂到掩码 id 上
        let state = s
            .get_account_state(Scope::user(&uid()), &id(ACC_1))
            .await
            .unwrap()
            .unwrap();
        assert!(state.jwt_token.is_some(), "凭据应随迁移一起搬过去");
        assert!(state.device_id.is_some());
        // userDomainId 是纯标识：运行期（库 / 日志 / Debug）保留 —— 它有排障价值且不可复用；
        // 但**进版本库的东西里必须是合成值**。界线是"能不能进 GitHub"，不是"能不能进日志"：
        // 日志是本地资产、随用随丢；仓库是全量留痕、推上去就收不回来。
        assert!(state.user_domain_id.is_some());
        assert!(state.cloud_phone_redpack_token.is_some());
        // 旧 state.json 没有时间戳，"导入时间"看台账
        assert!(state.updated_at.is_none());

        // 反向：拿手机号原文当 id 必须查不到东西
        assert!(
            s.get_account_state(Scope::user(&uid()), &id("19900000001"))
                .await
                .unwrap()
                .is_none(),
            "手机号原文不该成为 id"
        );
        assert!(
            !s.list_accounts(Scope::user(&uid()))
                .await
                .unwrap()
                .iter()
                .any(|a| a.id.as_str() == "19900000001")
        );
    }

    #[tokio::test]
    async fn runs_attach_to_accounts_by_display_name() {
        let s = store();
        run_import(&s, &fixture_source()).await.unwrap();

        let runs = s.list_runs(Scope::user(&uid()), 100).await.unwrap();
        let mut per_account = std::collections::BTreeMap::new();
        for run in &runs {
            if let Some(account) = &run.account_id {
                *per_account.entry(account.as_str().to_string()).or_insert(0) += 1;
            }
        }
        assert_eq!(per_account.get(ACC_1), Some(&6), "「账号2」的 6 条历史");
        assert_eq!(per_account.get(ACC_2), Some(&6), "「账号1」的 6 条历史");
        assert!(
            runs.iter().all(|r| r.account_id.is_some()),
            "夹具里每条历史的展示名都应能唯一解析"
        );

        // 状态映射：ok=true → Done，ok=false → Failed
        assert_eq!(
            runs.iter().filter(|r| r.status == RunStatus::Done).count(),
            10
        );
        assert_eq!(
            runs.iter()
                .filter(|r| r.status == RunStatus::Failed)
                .count(),
            2
        );
        // 失败记录的 error 原文要搬到 message，否则失败原因就丢了
        assert!(runs.iter().any(|r| {
            r.message
                .as_deref()
                .is_some_and(|m| m.contains("prizeIdOrName"))
        }));
        // 旧数据从未采过云朵净增（原项目写入被注释），编不出来就留空
        assert!(
            runs.iter()
                .all(|r| r.cloud_gained.is_none() && r.cloud_after.is_none())
        );
    }

    #[tokio::test]
    async fn pipeline_keeps_steps_order_and_defaults_to_sequential() {
        let s = store();
        run_import(&s, &fixture_source()).await.unwrap();

        let pipelines = s.list_pipelines(Scope::user(&uid())).await.unwrap();
        let pipeline = &pipelines[0];
        assert_eq!(pipeline.name, "轻量巡检");
        assert_eq!(
            pipeline
                .steps
                .iter()
                .map(|s| s.task.as_str())
                .collect::<Vec<_>>(),
            vec!["prizes", "receive"],
            "步骤顺序即执行顺序，不能重排"
        );
        assert_eq!(pipeline.account_mode, AccountMode::Sequential);
        assert!(pipeline.stop_on_error);
        assert!(
            pipeline.account_ids.is_empty(),
            "旧数据没指定 accounts = 全部账号"
        );
        assert_eq!(pipeline.last_duration_ms, Some(2481));
        // 旧 id 是 UUID，应当原样保留而不是换成兜底 id
        assert_eq!(pipeline.id, "84848ee7-3b94-483b-80fe-2ee33c93c7fe");
    }

    #[tokio::test]
    async fn exchange_windows_and_webui_prefs_round_trip() {
        let s = store();
        run_import(&s, &fixture_source()).await.unwrap();

        let exchange = s.exchange_config().await.unwrap();
        assert_eq!(exchange.windows, vec!["09:59:40", "11:59:40"]);
        assert!(!exchange.enabled, "旧数据没有绑定，启用没有意义");
        assert!(exchange.bindings.is_empty());
        assert!(exchange.updated_at.is_some());

        let webui = s.webui_config().await.unwrap();
        // 旧版两个布尔字段的缺省方向相反，这里校验它被归一化成了确定值
        assert!(webui.remember_last_page, "true 原样搬过来");
        assert!(!webui.log_expanded);
        assert!(!webui.background_run);
        assert_eq!(webui.last_page, "tasks");
        assert!(WEBUI_PAGES.contains(&webui.last_page.as_str()));
    }

    // ── 幂等 ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn second_run_skips_unchanged_files() {
        let s = store();
        run_import(&s, &fixture_source()).await.unwrap();

        let again = run_import(&s, &fixture_source()).await.unwrap();
        assert_eq!(again.accounts, 0, "asign.json 没变，不重复写");
        assert_eq!(again.runs, 0);
        assert_eq!(again.pipelines, 0);
        assert_eq!(again.schedules, 0, "空数组也不该被当成有新内容");
        assert_eq!(
            again.skipped_unchanged.len(),
            LEGACY_DATA_FILES.len(),
            "6 个数据文件都应被跳过，实际 {:?}",
            again.skipped_unchanged
        );
        assert!(
            !again.has_issues(),
            "跳过不是问题：{:#?}",
            again.issue_lines()
        );

        // 库里的行数不变
        assert_eq!(
            s.list_runs(Scope::user(&uid()), 100).await.unwrap().len(),
            12
        );
        assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn re_import_after_touch_does_not_duplicate_rows() {
        let dir = copy_fixtures();
        let s = store();
        run_import(&s, &source_of(&dir)).await.unwrap();
        let before: Vec<String> = s
            .list_runs(Scope::user(&uid()), 100)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();

        // 改内容一个字节（末尾加换行）：sha256 变了 → 整份重跑。
        // 这一条验的才是真正的幂等：**台账失效之后**，UPSERT 主键仍然稳定。
        let text = std::fs::read_to_string(dir.path().join("history.json")).unwrap();
        write_file(&dir, "history.json", &format!("{text}\n"));

        let report = run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(report.runs, 12, "重跑会重新解析 12 条");
        assert!(
            !report
                .skipped_unchanged
                .contains(&"history.json".to_string()),
            "history.json 变了，不该被跳过"
        );

        let after: Vec<String> = s
            .list_runs(Scope::user(&uid()), 100)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(after, before, "行数不变，主键也不变");
        assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
        assert_eq!(
            s.list_pipelines(Scope::user(&uid())).await.unwrap().len(),
            1
        );
    }

    // ── 安全失败 ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mask_collision_aborts_before_writing_anything() {
        let dir = copy_fixtures();
        // 两个手机号前 2 后 2 相同 → 掩码都是 19****16 → 会塌缩成同一个 AccountId
        write_file(
            &dir,
            "asign.json",
            r#"{
  "caiyun": [
    { "auth": "Basic MjoxOTkwMDAwMDAxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr", "nickname": "甲" },
    { "auth": "Basic MjoxOTkwMDAwMDIxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr", "nickname": "乙" }
  ]
}"#,
        );

        let s = store();
        let err = run_import(&s, &source_of(&dir))
            .await
            .expect_err("掩码冲突必须让迁移失败");
        assert!(
            matches!(err, LegacyImportError::Identity(_)),
            "应当是身份映射失败，实际 {err:?}"
        );

        // 关键：不是"导入了一半"，而是"什么都没动"
        assert!(
            s.list_accounts(Scope::user(&uid()))
                .await
                .unwrap()
                .is_empty(),
            "中止时不该写入任何账号"
        );
        assert!(
            s.list_runs(Scope::user(&uid()), 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            s.list_pipelines(Scope::user(&uid()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            s.list_imports().unwrap().is_empty(),
            "中止时也不该留下台账，否则重跑会被误判成已导入"
        );
    }

    #[tokio::test]
    async fn missing_file_is_reported_and_the_rest_still_imports() {
        let dir = copy_fixtures();
        remove_file(&dir, "schedules.json");

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        assert_eq!(report.runs, 12, "少一个文件不影响别的文件");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::MissingFile { path } if path.ends_with("schedules.json"))),
            "缺失文件要报出来：{:#?}",
            report.issue_lines()
        );
    }

    #[tokio::test]
    async fn corrupt_json_fails_loudly_instead_of_importing_halfway() {
        let dir = copy_fixtures();
        write_file(&dir, "history.json", "{ 这不是 JSON");

        let s = store();
        let err = run_import(&s, &source_of(&dir))
            .await
            .expect_err("整份 JSON 坏掉应当直接失败");
        assert!(
            matches!(err, LegacyImportError::Parse { .. }),
            "应当是解析错误，实际 {err:?}"
        );
    }

    #[tokio::test]
    async fn one_bad_run_record_does_not_take_down_the_rest() {
        let dir = copy_fixtures();
        // 4 条记录，分别是：好的 / 类型错的 / 不是对象的 / 展示名认不出的
        write_file(
            &dir,
            "history.json",
            r#"{
  "runs": [
    { "task": "sign", "account": "账号1", "ok": true, "start": "2026-09-16T11:00:00Z", "end": "2026-09-16T11:00:01Z" },
    { "task": "sign", "account": "账号1", "ok": 1 },
    "这不是对象",
    { "task": "sign", "account": "查无此人", "ok": true, "start": "2026-09-16T11:02:00Z", "end": "2026-09-16T11:02:01Z" }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        // 能归属的那条照常落库并可读
        let runs = s.list_runs(Scope::user(&uid()), 100).await.unwrap();
        assert_eq!(runs.len(), 1, "只有能归属的记录会出现在用户列表里");
        assert_eq!(runs[0].account_id, Some(id(ACC_2)), "「账号1」→ 19****02");

        // 展示名认不出的那条**仍然落库**（历史不能因为归属解析失败就丢掉），
        // 只是它无法归属，所以不出现在任何用户的列表里 ——
        // 它该被看见的地方是迁移报告，不是某个人的"我的历史"。
        let orphaned: i64 = {
            let conn = s.connection().lock().expect("数据库锁中毒");
            conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE account_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(orphaned, 1, "无归属的旧记录仍要保留，否则历史就断了");

        let lines = report.issue_lines();
        assert!(
            lines.iter().any(|l| l.contains("history.json 第 2 条")),
            "下标要指得准：{lines:#?}"
        );
        assert!(lines.iter().any(|l| l.contains("第 3 条")));
        assert!(lines.iter().any(|l| l.contains("查无此人")));
    }

    #[tokio::test]
    async fn bad_timestamp_is_reported_and_skipped() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "history.json",
            r#"{ "runs": [ { "task": "sign", "account": "账号1", "ok": true, "start": "昨天下午" } ] }"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(report.runs, 0);
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::BadRunTimestamp { .. })),
            "{:#?}",
            report.issue_lines()
        );
        assert!(
            s.list_runs(Scope::user(&uid()), 100)
                .await
                .unwrap()
                .is_empty()
        );
    }

    // ── 账号引用：绝不静默扩大范围 ─────────────────────────────────────────

    #[tokio::test]
    async fn phone_number_reference_is_recovered_and_flagged() {
        let dir = copy_fixtures();
        // 旧版把纯数字串当序号 → 19900000001 越界 → 从未生效。
        // 但它显然是手机号，且能唯一匹配，所以要恢复 + 报出来让人确认。
        write_file(
            &dir,
            "schedules.json",
            r#"{
  "schedules": [
    { "id": "s1", "name": "早班签到", "task": "sign", "cron": "0 0 8 * * *",
      "account": "19900000001", "enabled": true }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(report.schedules, 1);

        let schedule = &s.list_schedules(Scope::user(&uid())).await.unwrap()[0];
        assert_eq!(schedule.account_ids, vec![id(ACC_1)], "恢复到了正确的账号");
        assert!(schedule.enabled, "恢复成功就不该停用它");
        assert_eq!(schedule.timezone, DEFAULT_TZ, "旧版缺省是上海");
        assert_eq!(
            schedule.target,
            Some(ScheduleTarget::Capability {
                code: "sign".into()
            })
        );
        assert_eq!(schedule.cron, "0 0 8 * * *");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::RecoveredAccountRef { .. })),
            "恢复是行为变更，必须报出来：{:#?}",
            report.issue_lines()
        );
    }

    #[tokio::test]
    async fn unresolvable_reference_disables_the_schedule_instead_of_broadening_it() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "schedules.json",
            r#"{
  "schedules": [
    { "id": "s1", "name": "幽灵账号的任务", "task": "sign", "cron": "0 0 8 * * *",
      "account": "早就删掉的账号", "enabled": true }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        let schedule = &s.list_schedules(Scope::user(&uid())).await.unwrap()[0];
        assert!(
            !schedule.enabled,
            "解析不到账号必须停用 —— 回落到'全部账号'等于把'过去什么都没跑'变成'对所有人跑'"
        );
        assert!(
            schedule.account_ids.is_empty(),
            "原有账号引用不可用时不能凭空补一组账号"
        );
        assert!(
            report.issues.iter().any(|i| matches!(
                i,
                MigrationIssue::DeadAccountRef { action, .. } if *action == "停用该定时任务"
            )),
            "{:#?}",
            report.issue_lines()
        );
    }

    #[tokio::test]
    async fn schedule_without_target_or_cron_is_imported_disabled() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "schedules.json",
            r#"{
  "schedules": [
    { "id": "s1", "name": "没有目标", "cron": "0 0 8 * * *", "enabled": true },
    { "id": "s2", "name": "没有 cron", "task": "sign", "cron": "  ", "enabled": true },
    { "id": "s3", "name": "默认关闭", "task": "sign", "cron": "0 0 8 * * *" }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(report.schedules, 3, "三条都要留下，靠 enabled 表达处置");

        let schedules = s.list_schedules(Scope::user(&uid())).await.unwrap();
        assert!(schedules.iter().all(|s| !s.enabled));
        assert_eq!(
            schedules
                .iter()
                .find(|s| s.name == "没有目标")
                .unwrap()
                .target,
            None
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::MissingTarget { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::EmptyCron { .. }))
        );
    }

    #[tokio::test]
    async fn pipeline_binding_a_dead_account_is_skipped_whole() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "pipelines.json",
            r#"{
  "pipelines": [
    { "id": "p1", "name": "好组合", "steps": [{ "task": "prizes" }] },
    { "id": "p2", "name": "坏组合", "steps": [{ "task": "prizes" }], "accounts": ["查无此人"] },
    { "id": "p3", "name": "空组合", "steps": [] },
    { "id": "p4", "name": "混合引用", "steps": [{ "task": "sign", "account": "账号1" }],
      "accounts": [2, "19900000001"], "accountMode": "parallel" }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        assert_eq!(report.pipelines, 2, "好组合与混合引用各一条");
        let names: Vec<String> = s
            .list_pipelines(Scope::user(&uid()))
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, vec!["好组合", "混合引用"], "坏组合与空组合都不入库");

        let mixed = s
            .get_pipeline(Scope::user(&uid()), "p4")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            mixed.account_ids,
            vec![id(ACC_2), id(ACC_1)],
            "序号 2 → 第 2 个账号；手机号 → 恢复成第 1 个账号"
        );
        assert_eq!(mixed.account_mode, AccountMode::Parallel);
        // 步骤级 account 已废弃，必须丢掉
        assert!(mixed.steps[0].body.is_none());
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::PipelineSkipped { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::EmptySteps { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::DroppedStepAccount { .. })),
            "被丢掉的废弃字段要说一声：{:#?}",
            report.issue_lines()
        );
    }

    #[tokio::test]
    async fn empty_account_list_means_all_accounts() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "pipelines.json",
            r#"{
  "pipelines": [
    { "id": "p1", "name": "空数组", "steps": [{ "task": "sign" }], "accounts": [] },
    { "id": "p2", "name": "全是空值", "steps": [{ "task": "sign" }], "accounts": [null, ""] }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(report.pipelines, 2);
        assert!(
            s.list_pipelines(Scope::user(&uid()))
                .await
                .unwrap()
                .iter()
                .all(|p| p.account_ids.is_empty()),
            "旧版 `if (accounts.length)` 为假 → 全部账号"
        );
        assert!(
            !report.has_issues(),
            "空列表是已知且正确的语义，不该报成问题：{:#?}",
            report.issue_lines()
        );
    }

    // ── 抢兑与界面偏好 ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn exchange_windows_are_normalized_and_duplicates_collapse() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "exchange.json",
            r#"{
  "enabled": true,
  "windows": ["11:59:40", " 09:59:40 ", "095940", "09:59:40"],
  "bindings": [
    { "account": "账号1", "prizeId": "42", "prizeName": "话费" },
    { "account": "账号1", "prizeId": 42, "prizeName": "重复绑定" },
    { "account": 1, "prizeId": 42 },
    { "account": "19900000002", "prizeId": 7 }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        let exchange = s.exchange_config().await.unwrap();
        assert_eq!(
            exchange.windows,
            vec!["09:59:40", "11:59:40"],
            "空白被裁掉、5 位补齐、重复折叠、整体排序"
        );
        assert!(exchange.enabled, "有绑定才谈得上启用");
        // 注意夹具里配置顺序是 [账号2, 账号1]，所以「账号1」= 序号 2 = 19****02
        assert_eq!(
            exchange
                .bindings
                .iter()
                .map(|b| (b.account_id.as_str().to_string(), b.prize_id))
                .collect::<Vec<_>>(),
            vec![
                (ACC_2.to_string(), 42),
                (ACC_1.to_string(), 42),
                (ACC_2.to_string(), 7),
            ],
            "第 2 条与第 1 条同账号同奖品 → 折叠；第 4 条用手机号引用 → 按掩码归一后也是 19****02/7"
        );
        assert_eq!(
            exchange.bindings[0].prize_name.as_deref(),
            Some("话费"),
            "折叠时保留首次出现的奖品名"
        );
        // 「19900000002」是手机号原文 → 旧版当成序号越界（手机号认不出是账号），
        // 这里应当恢复并留下记录，而不是静默吃掉
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::RecoveredAccountRef { .. })),
            "手机号引用被恢复必须报出来：{:#?}",
            report.issue_lines()
        );
    }

    #[tokio::test]
    async fn exchange_with_no_window_falls_back_to_the_default() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "exchange.json",
            r#"{ "enabled": true, "windows": ["", " "], "bindings": [] }"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        let exchange = s.exchange_config().await.unwrap();
        assert_eq!(exchange.windows, vec!["09:59:40", "11:59:40"]);
        assert!(!exchange.enabled, "没有绑定就谈不上抢兑");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::BadExchangeWindows { .. })),
            "{:#?}",
            report.issue_lines()
        );
    }

    #[tokio::test]
    async fn broken_bindings_are_dropped_one_by_one() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "exchange.json",
            r#"{
  "enabled": true,
  "windows": ["09:59:40"],
  "bindings": [
    { "account": "账号1", "prizeId": "不是数字" },
    { "prizeId": 42 },
    { "account": "查无此人", "prizeId": 42 },
    { "account": "账号1", "prizeId": 42 }
  ]
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        let exchange = s.exchange_config().await.unwrap();
        assert_eq!(exchange.bindings.len(), 1, "只有最后一条是完整的");
        assert_eq!(exchange.bindings[0].account_id, id(ACC_2));
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::BadPrizeId { .. }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::BindingWithoutAccount { index: 1 }))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, MigrationIssue::DeadAccountRef { .. }))
        );
    }

    #[tokio::test]
    async fn unknown_page_falls_back_to_the_default_page() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "webui.json",
            r#"{ "rememberLastPage": true, "lastPage": "被删掉的页面", "logExpanded": true }"#,
        );

        let s = store();
        run_import(&s, &source_of(&dir)).await.unwrap();

        let webui = s.webui_config().await.unwrap();
        assert!(WEBUI_PAGES.contains(&webui.last_page.as_str()));
        assert!(webui.log_expanded);
        assert!(
            !webui.background_run,
            "旧版 `backgroundRun === true` 才为真，缺省是 false"
        );
    }

    // ── 台账与报告 ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn ledger_archives_the_original_text_and_issue_counts() {
        let dir = copy_fixtures();
        let s = store();
        run_import(&s, &source_of(&dir)).await.unwrap();

        let imports = s.list_imports().unwrap();
        assert_eq!(
            imports.len(),
            LEGACY_DATA_FILES.len() + 1,
            "6 个数据文件 + asign.json"
        );

        let history = imports
            .iter()
            .find(|r| r.source.ends_with("history.json"))
            .expect("history.json 的台账");
        assert_eq!(history.record_count, 12);
        assert_eq!(history.issues, 0);
        assert_eq!(history.imported_at, now().to_rfc3339());

        // 原文归档：旧 history.json 的 logs / result 不在新表里，
        // 但整份原文在台账里，信息没丢，将来要回填随时能回填
        let raw = history.raw.as_deref().expect("原文应被归档");
        assert!(raw.contains("\"logs\""));
        assert_eq!(
            raw,
            std::fs::read_to_string(dir.path().join("history.json")).unwrap()
        );

        // 台账的摘要形式不能泄露原文（它可能含凭据）
        let summary = history.summary();
        assert!(!summary.contains("logs"));
        assert!(summary.contains("history.json"));
    }

    #[tokio::test]
    async fn report_summary_is_a_single_safe_line() {
        let s = store();
        let report = run_import(&s, &fixture_source()).await.unwrap();
        let line = report.summary();
        assert!(line.contains("账号 2"));
        assert!(line.contains("历史 12 条"));
        assert!(!line.contains('\n'), "摘要必须是一行");
        assert_eq!(report.issue_lines().len(), report.issues.len());
    }

    #[tokio::test]
    async fn state_only_backfills_account_state_not_accounts() {
        let dir = copy_fixtures();
        let s = store();
        // 先只导入账号（把别的数据文件都删掉），模拟"账号清单早就导完了"
        for name in LEGACY_DATA_FILES {
            remove_file(&dir, name);
        }
        run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);

        // 再把 state.json 放回来：这次只该补登录态
        let text = std::fs::read_to_string(PathBuf::from(FIXTURES).join("state.json")).unwrap();
        write_file(&dir, "state.json", &text);

        let report = run_import(&s, &source_of(&dir)).await.unwrap();
        assert_eq!(report.accounts, 0, "asign.json 没变就不重写账号");
        assert_eq!(report.account_states, 2);
        assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
        assert!(
            s.get_account_state(Scope::user(&uid()), &id(ACC_1))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn orphan_and_empty_state_entries_are_reported_not_invented() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "state.json",
            r#"{
  "accounts": {
    "19900000001": { "jwtToken": "t", "deviceId": "d" },
    "17700000000": { "jwtToken": "不该被导入" },
    "19900000002": { "jwtToken": "", "deviceId": "", "userDomainId": "", "cloudPhoneRedpackToken": "" },
    "19900000003": { "jwtToken": "t", "新字段": 1 }
  }
}"#,
        );

        let s = store();
        let report = run_import(&s, &source_of(&dir)).await.unwrap();

        assert_eq!(report.account_states, 1, "只有 19900000001 是可用的");
        assert!(
            s.get_account_state(Scope::user(&uid()), &id(ACC_1))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            s.get_account_state(Scope::user(&uid()), &id(ACC_2))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.list_accounts(Scope::user(&uid()))
                .await
                .unwrap()
                .iter()
                .all(|a| a.id.as_str() != "17****00"),
            "找不到归属的登录态不能凭空造账号"
        );

        let lines = report.issue_lines();
        assert!(lines.iter().any(|l| l.contains("找不到")));
        assert!(lines.iter().any(|l| l.contains("状态是空的")));
        assert!(
            lines.iter().any(|l| l.contains("新字段")),
            "未识别字段要如实报出而不是静默丢弃：{lines:#?}"
        );
    }

    #[tokio::test]
    async fn schedule_last_status_is_mapped_or_dropped() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "schedules.json",
            r#"{
  "schedules": [
    { "id": "s1", "name": "a", "task": "sign", "cron": "* * * * * *", "lastStatus": "error", "lastError": "上游 500" },
    { "id": "s2", "name": "b", "task": "sign", "cron": "* * * * * *", "lastStatus": "skipped" },
    { "id": "s3", "name": "c", "task": "sign", "cron": "* * * * * *", "lastStatus": "从没见过的状态" }
  ]
}"#,
        );

        let s = store();
        run_import(&s, &source_of(&dir)).await.unwrap();

        assert_eq!(
            s.get_schedule(Scope::user(&uid()), "s1")
                .await
                .unwrap()
                .unwrap()
                .last_status,
            Some(ScheduleStatus::Error)
        );
        assert_eq!(
            s.get_schedule(Scope::user(&uid()), "s1")
                .await
                .unwrap()
                .unwrap()
                .last_error
                .as_deref(),
            Some("上游 500")
        );
        assert_eq!(
            s.get_schedule(Scope::user(&uid()), "s2")
                .await
                .unwrap()
                .unwrap()
                .last_status,
            Some(ScheduleStatus::Skipped)
        );
        // 认不出来的状态宁可为空，也不该猜一个
        assert_eq!(
            s.get_schedule(Scope::user(&uid()), "s3")
                .await
                .unwrap()
                .unwrap()
                .last_status,
            None
        );
    }

    #[tokio::test]
    async fn schedule_prefers_the_pipeline_target_when_both_are_present() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "schedules.json",
            r#"{
  "schedules": [
    { "id": "s1", "name": "两个目标都有", "task": "sign", "pipelineId": "p1", "cron": "0 0 8 * * *" },
    { "id": "s2", "name": "只有 pipelineId", "pipelineId": " p1 ", "cron": "0 0 8 * * *" }
  ]
}"#,
        );

        let s = store();
        run_import(&s, &source_of(&dir)).await.unwrap();

        for id in ["s1", "s2"] {
            assert_eq!(
                s.get_schedule(Scope::user(&uid()), id)
                    .await
                    .unwrap()
                    .unwrap()
                    .target,
                Some(ScheduleTarget::Pipeline { id: "p1".into() }),
                "旧版优先 pipelineId；{id} 的目标不对"
            );
        }
    }

    #[tokio::test]
    async fn records_without_ids_get_stable_fallback_ids() {
        let dir = copy_fixtures();
        write_file(
            &dir,
            "pipelines.json",
            r#"{ "pipelines": [ { "name": "没 id 的组合", "steps": [{ "task": "sign" }] } ] }"#,
        );
        write_file(
            &dir,
            "schedules.json",
            r#"{ "schedules": [ { "name": "没 id 的任务", "task": "sign", "cron": "0 0 8 * * *" } ] }"#,
        );

        let s = store();
        run_import(&s, &source_of(&dir)).await.unwrap();
        let first_pipeline = s.list_pipelines(Scope::user(&uid())).await.unwrap()[0]
            .id
            .clone();
        let first_schedule = s.list_schedules(Scope::user(&uid())).await.unwrap()[0]
            .id
            .clone();

        assert!(first_pipeline.starts_with("legacy-pipe-"));
        assert!(first_schedule.starts_with("legacy-sched-"));

        // 改内容触发重跑，兜底 id 必须不变 —— 否则幂等立刻失效
        write_file(
            &dir,
            "pipelines.json",
            r#"{ "pipelines": [ { "name": "没 id 的组合", "steps": [{ "task": "prizes" }] } ] }"#,
        );
        run_import(&s, &source_of(&dir)).await.unwrap();

        let pipelines = s.list_pipelines(Scope::user(&uid())).await.unwrap();
        assert_eq!(pipelines.len(), 1, "兜底 id 变了就会多出一行");
        assert_eq!(pipelines[0].id, first_pipeline);
        assert_eq!(
            s.list_schedules(Scope::user(&uid())).await.unwrap()[0].id,
            first_schedule
        );
    }
}
