// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
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
pub(crate) const DEFAULT_TZ: &str = "Asia/Shanghai";

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
