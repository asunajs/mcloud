//! 旧项目数据迁移的**执行策略**。
//!
//! 为什么这件事在 `mcloud-server` 而不是 `mcloud-cli`：
//! 迁移要知道"数据库放哪、归档放哪、什么时候跑"——这是**接线**，属于组装根的职责。
//! CLI 只是它的一个前端；方案 §8.3 要求的"首次启动自动迁移"是另一个前端，
//! 两者必须共用同一份实现，否则"手动迁的"和"启动时迁的"迟早出现差异。
//!
//! 真正的导入逻辑在 `mcloud_io::import_legacy`，这里只负责编排：
//! **先归档、再建库、后导入**，并把结果整理成前端（CLI/日志）可直接消费的形状。

use mcloud_core::Clock;
use mcloud_io::{
    LegacyImportError, LegacySource, MigrationReport, SqliteStore, SystemClock,
    archive_legacy_source, diagnose_legacy, import_legacy,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// 一次迁移要做的事：从哪读、写到哪、归档到哪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    /// 旧数据位置（`asign.json` + `packages/api/data/`）。
    pub source: LegacySource,
    /// 目标 SQLite 文件。
    pub db_path: PathBuf,
    /// 旧数据的归档目录。
    pub backup_dir: PathBuf,
}

impl MigrationPlan {
    /// 按旧项目布局从仓库根推导：读 `<root>/asign.json` 与 `<root>/packages/api/data/`。
    ///
    /// 写入位置**不在这里推导** —— 旧仓库自始至终是只读归档，
    /// 把"写到哪"默认成旧仓库里的某个路径，是将来最容易误伤的默认值。
    pub fn read_from_repo_root(
        repo_root: impl AsRef<Path>,
        db_path: PathBuf,
        backup_dir: PathBuf,
    ) -> Self {
        Self {
            source: LegacySource::from_repo_root(repo_root),
            db_path,
            backup_dir,
        }
    }
}

/// 迁移结果。
///
/// `db_path` 是 `Option` 而不是 `PathBuf`：预演**不建库**，
/// 用 `None` 把这件事表达在类型里，比让调用方去猜"预演时这个字段有没有意义"可靠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// 报告（字段名与 `GET /api/...` 风格一致，脚本可直接消费）。
    pub report: Value,
    /// 归档的源文件数；预演恒为 0。
    pub archived: usize,
    /// 实际写入的库；预演为 `None`。
    pub db_path: Option<PathBuf>,
    /// 是否预演。
    pub dry_run: bool,
}

impl MigrationOutcome {
    /// 给人看的多行输出。
    ///
    /// 渲染放在这里而不是 CLI：CLI 与服务端日志将来都会用，
    /// 两边各写一份就会出现"命令行说 12 条、日志说 0 条"。
    pub fn human_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        lines.push(if self.dry_run {
            "迁移预演（未写入任何文件）".to_string()
        } else {
            "迁移完成".to_string()
        });
        lines.push(self.report["summary"].as_str().unwrap_or("").to_string());

        if self.archived > 0 {
            lines.push(format!("已归档 {} 个源文件", self.archived));
        }
        if let Some(path) = &self.db_path {
            lines.push(format!("数据库: {}", path.display()));
        }

        let issues = self.report["issues"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if issues.is_empty() {
            lines.push("没有需要人工确认的问题".to_string());
        } else {
            lines.push(String::new());
            lines.push(format!("需要确认的问题（{} 条）：", issues.len()));
            for issue in issues {
                lines.push(format!("  - {}", issue.as_str().unwrap_or("")));
            }
        }
        lines
    }

    /// 问题总数。
    pub fn issue_count(&self) -> usize {
        self.report["issues"].as_array().map_or(0, Vec::len)
    }
}

/// 执行（或预演）迁移。
///
/// 顺序是刻意的：**归档在建库之前**。如果导入中途失败，
/// 原始数据至少已经在新项目里有一份可追溯的副本。
/// 预演走的是 `mcloud_io::diagnose_legacy` —— 它在内存库上跑**完全相同**的导入路径，
/// 所以"预演说没事、真跑报错"在结构上不会发生。
pub async fn run(
    plan: &MigrationPlan,
    dry_run: bool,
) -> Result<MigrationOutcome, LegacyImportError> {
    let now = SystemClock.now();

    let report = if dry_run {
        diagnose_legacy(&plan.source, now).await?
    } else {
        let archived = archive_legacy_source(&plan.source, &plan.backup_dir)?;
        let store = SqliteStore::open(&plan.db_path)?;
        let report = import_legacy(&store, &plan.source, now).await?;
        return Ok(outcome(report, archived, Some(plan.db_path.clone()), false));
    };

    Ok(outcome(report, 0, None, true))
}

fn outcome(
    report: MigrationReport,
    archived: usize,
    db_path: Option<PathBuf>,
    dry_run: bool,
) -> MigrationOutcome {
    MigrationOutcome {
        report: report.to_json(),
        archived,
        db_path,
        dry_run,
    }
}

/// 是否存在可迁移的旧数据（供"首次启动自动迁移"判断）。
///
/// 只要求 **`asign.json` 或任一数据文件**存在：旧项目里 `schedules.json` 实测为空、
/// 但在某些机器上可能在导入前还没被创建过，把"必须全都在"当条件会静默漏掉整次迁移。
pub fn has_legacy_data(source: &LegacySource) -> bool {
    !source.existing_files().is_empty()
}

/// 序列化给外部消费（CLI `--json` / 日志）。
pub fn outcome_to_json(outcome: &MigrationOutcome) -> Value {
    json!({
        "dryRun": outcome.dry_run,
        "archived": outcome.archived,
        "dbPath": outcome.db_path.as_ref().map(|p| p.display().to_string()),
        "report": outcome.report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 真实脱敏夹具所在目录（`test/fixtures/legacy/`）。
    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test/fixtures/legacy");

    fn fixture_plan(work: &Path) -> MigrationPlan {
        MigrationPlan {
            source: LegacySource::new(
                PathBuf::from(FIXTURES).join("asign.json"),
                PathBuf::from(FIXTURES),
            ),
            db_path: work.join("mcloud.db"),
            backup_dir: work.join("legacy-source"),
        }
    }

    #[tokio::test]
    async fn dry_run_neither_creates_a_database_nor_archives() {
        let work = tempfile::tempdir().unwrap();
        let plan = fixture_plan(work.path());

        let outcome = run(&plan, true).await.expect("预演应成功");

        assert!(outcome.dry_run);
        assert_eq!(outcome.archived, 0);
        assert!(outcome.db_path.is_none(), "预演不建库");
        assert!(!plan.db_path.exists(), "预演不该在磁盘上留下数据库");
        assert!(!plan.backup_dir.exists(), "预演不该归档");

        // 预演的报告必须和真实导入一致 —— 这正是"共用同一条路径"的意义
        assert_eq!(outcome.report["accounts"], 2);
        assert_eq!(outcome.report["runs"], 12);
        assert_eq!(outcome.issue_count(), 0);
    }

    #[tokio::test]
    async fn real_run_archives_before_writing_the_database() {
        let work = tempfile::tempdir().unwrap();
        let plan = fixture_plan(work.path());

        let outcome = run(&plan, false).await.expect("迁移应成功");

        assert!(!outcome.dry_run);
        assert_eq!(outcome.archived, 7, "asign.json + 6 个数据文件");
        assert_eq!(outcome.db_path.as_deref(), Some(plan.db_path.as_path()));
        assert!(plan.db_path.exists(), "应当建出数据库");

        // 归档内容与源逐字一致：它是"可离线重放的原始数据"，不是改写过的副本
        for name in std::iter::once("asign.json").chain(mcloud_io::LEGACY_DATA_FILES) {
            let archived = std::fs::read_to_string(plan.backup_dir.join(name))
                .unwrap_or_else(|e| panic!("归档里应有 {name}: {e}"));
            let original = std::fs::read_to_string(PathBuf::from(FIXTURES).join(name)).unwrap();
            assert_eq!(archived, original, "{name} 归档内容应逐字一致");
        }

        // 库真的建了东西
        let store = SqliteStore::open(&plan.db_path).unwrap();
        assert_eq!(
            mcloud_core::StateStore::list_accounts(&store)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn archiving_is_idempotent() {
        let work = tempfile::tempdir().unwrap();
        let plan = fixture_plan(work.path());

        let first = run(&plan, false).await.unwrap();
        assert_eq!(first.archived, 7);

        let again = archive_legacy_source(&plan.source, &plan.backup_dir).unwrap();
        assert_eq!(again, 0, "内容相同的文件不该重复写盘");
    }

    #[tokio::test]
    async fn second_run_reports_everything_unchanged_and_stays_consistent() {
        let work = tempfile::tempdir().unwrap();
        let plan = fixture_plan(work.path());

        run(&plan, false).await.unwrap();
        let second = run(&plan, false).await.unwrap();

        assert_eq!(second.report["runs"], 0, "源没变就不重复写");
        assert_eq!(second.report["accounts"], 0);
        assert_eq!(
            second.report["skippedUnchanged"].as_array().unwrap().len(),
            mcloud_io::LEGACY_DATA_FILES.len()
        );
        assert_eq!(second.issue_count(), 0, "跳过不是问题");

        // 库里不因重跑而多行
        let store = SqliteStore::open(&plan.db_path).unwrap();
        assert_eq!(
            mcloud_core::StateStore::list_runs(&store, 100)
                .await
                .unwrap()
                .len(),
            12
        );
    }

    #[tokio::test]
    async fn human_lines_mention_the_dry_run_and_the_issues() {
        let work = tempfile::tempdir().unwrap();
        let plan = fixture_plan(work.path());

        let outcome = run(&plan, true).await.unwrap();
        let text = outcome.human_lines().join("\n");
        assert!(text.contains("预演"));
        assert!(text.contains("账号 2"));
        assert!(text.contains("没有需要人工确认的问题"));

        let real = run(&plan, false).await.unwrap();
        let text = real.human_lines().join("\n");
        assert!(text.contains("迁移完成"));
        assert!(text.contains("已归档 7 个源文件"));
        assert!(text.contains("数据库:"));
    }

    #[test]
    fn has_legacy_data_is_true_when_at_least_one_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let source = LegacySource::new(dir.path().join("asign.json"), dir.path());
        assert!(!has_legacy_data(&source), "空目录不算有旧数据");

        // 只有数据文件、没有 asign.json 也算 —— 否则整次迁移会被静默跳过
        std::fs::write(dir.path().join("history.json"), "{}").unwrap();
        assert!(has_legacy_data(&source));
    }

    #[tokio::test]
    async fn outcome_json_is_machine_consumable() {
        let work = tempfile::tempdir().unwrap();
        let plan = fixture_plan(work.path());
        let outcome = run(&plan, true).await.unwrap();

        let payload = outcome_to_json(&outcome);
        assert_eq!(payload["dryRun"], true);
        assert!(payload["dbPath"].is_null());
        assert_eq!(payload["report"]["runs"], 12);
        assert!(payload["report"]["summary"].is_string());
    }

    #[tokio::test]
    async fn identity_failure_leaves_no_database_behind() {
        let dir = tempfile::tempdir().unwrap();
        // 两个手机号掩码都是 19****16 → 身份映射必须失败
        // （`19900000016` 与 `19900000216`：都取自 199 合成号段，掩码故意撞车）
        std::fs::write(
            dir.path().join("asign.json"),
            r#"{ "caiyun": [
                { "auth": "Basic MjoxOTkwMDAwMDAxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr" },
                { "auth": "Basic MjoxOTkwMDAwMDIxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr" }
            ] }"#,
        )
        .unwrap();
        let plan = MigrationPlan {
            source: LegacySource::new(dir.path().join("asign.json"), dir.path()),
            db_path: dir.path().join("mcloud.db"),
            backup_dir: dir.path().join("backup"),
        };

        let err = run(&plan, false).await.expect_err("掩码冲突必须失败");
        assert!(matches!(err, LegacyImportError::Identity(_)), "{err:?}");

        // 归档是允许的（它是源数据的副本，无害），但**库必须是空的**
        let store = SqliteStore::open(&plan.db_path).unwrap();
        assert!(
            mcloud_core::StateStore::list_accounts(&store)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
