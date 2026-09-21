// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use mcloud_core::Error;
use mcloud_core::legacy::LegacyError;
use serde_json::Value;

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
    /// **本次实际补上**整串 `Authorization` / 手机号的账号数。
    ///
    /// "实际"是刻意的：没有缺口可补的账号不计入。否则第二次运行还会报同样的数字，
    /// 而用户正是靠这个数字判断"我那个老库补上了吗"。
    ///
    /// 与 `account_states` **不是一回事**，也不是它的子集：凭据来自 `asign.json`，
    /// 登录态来自 `state.json`，两个文件的台账各自独立。老库（v7 之前导过的）
    /// 会看到 `account_states` 为 0 而这一项非 0 —— 那正是这条回填要修的东西。
    pub credentials_filled: usize,
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
            "账号 {} / 登录态 {} / 凭据回填 {} / 定时任务 {} / 组合 {} / 历史 {} 条；\
             跳过未变的文件 {} 个；问题 {} 个",
            self.accounts,
            self.account_states,
            self.credentials_filled,
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
            "credentialsFilled": self.credentials_filled,
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
    #[error("数据库操作失败: {0}")]
    Store(#[from] Error),
}
