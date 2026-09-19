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

mod helpers;
mod import;
mod report;
mod source;
mod write_accounts;
mod write_exchange;
mod write_history;
mod write_jobs;

pub use import::{Result, diagnose_legacy, import_legacy};
pub use report::{LegacyImportError, MigrationIssue, MigrationReport};
pub(crate) use source::DEFAULT_TZ;
pub use source::LEGACY_DATA_FILES;
pub use source::{LegacySource, archive_legacy_source};

pub(crate) use helpers::{
    commit_ledger, fallback_id, non_empty, parse_each_indexed, parse_pipeline_status,
    parse_schedule_status, parse_whole, read_text, source_changed,
};
pub(crate) use write_accounts::{fill_credentials, write_accounts, write_state};
pub(crate) use write_exchange::{write_exchange, write_webui};
pub(crate) use write_history::write_history;
pub(crate) use write_jobs::{write_pipelines, write_schedules};

#[cfg(test)]
mod tests;
