//! 系统：存储信息与重启服务。
//!
//! # 为什么把这两个放一起
//!
//! 它们回答的是同一类问题 —— **"这个进程跑在哪、占了多少、怎么让它重来一次"** ——
//! 与业务（账号/任务/日志）无关。WebUI 的「系统设置」页整块用它们。
//!
//! # 为什么"数据占用"和"日志占用"分开报
//!
//! 两者是**互不重叠**的两块：数据文件在 `data/` 顶层，日志在 `data/logs/`。
//! 合成一个数看着简洁，但用户想清日志腾空间时就无从判断该动哪边。
//!
//! # 重启为什么不 `process::exit`
//!
//! `main.rs` 的 `with_graceful_shutdown` 收到这个信号后会走**与 SIGTERM 完全相同**
//! 的收尾：停调度器、把攒着的日志落盘。`exit` 会把这些全跳过，代价是最后一批日志丢失。
//! 重拉交给托管方（launchd / systemd）：进程自己 exec 自己会丢掉 supervisor 的记账。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::error::ApiResult;
use crate::ServerState;

/// 构建期写进二进制的东西（见 `build.rs`）。
///
/// ⚠ 全是**兜底可空**的：编出来的机器上可能没有 git / date，
/// 那时这几行是空串，卡片少显示一行，而不是让服务起不来。
const BUILD_COMMIT: &str = include_str!(concat!(env!("OUT_DIR"), "/build-commit.txt"));
const BUILD_DIRTY: &str = include_str!(concat!(env!("OUT_DIR"), "/build-dirty.txt"));
const BUILD_TIME: &str = include_str!(concat!(env!("OUT_DIR"), "/build-time.txt"));
const BUILD_LOG: &str = include_str!(concat!(env!("OUT_DIR"), "/build-log.txt"));

/// 一条改动（提交）。
#[derive(Serialize)]
pub struct Change {
    pub hash: String,
    pub subject: String,
}

/// 「关于」卡片：版本、构建时间、本次构建包含的改动。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutView {
    pub name: &'static str,
    pub version: &'static str,
    /// 短提交号（可能为空：构建机上没有 git）。
    pub commit: String,
    /// 构建时工作区**有未提交改动**。
    ///
    /// 这一格不是装饰：release 部署会把工作区里所有未提交的东西一起编进线上
    /// （见 AGENTS.md），线上跑的到底是哪一份，就看这里。
    pub dirty: bool,
    pub build_time: String,
    /// `release` / `debug` —— debug 的东西不会被线上用到（线上跑 release）。
    pub profile: &'static str,
    /// 本次构建包含的最近若干条改动（来自 `git log`）。
    pub changes: Vec<Change>,
}

/// `GET /api/system/about` —— 「关于」卡片的数据。
///
/// # 为什么没有"检查更新"
///
/// 上游仓库是**私有**的：[`参考实现的仓库`] 匿名访问返回 404，
/// 而本服务的浏览器端**没有任何凭据**去查它的发布 —— 摆一个点下去必然失败的按钮，
/// 比没有更糟（用户会以为自己网络有问题）。所以这里的"更新说明"给的是
/// **本次构建实际包含的改动**：那是本机真拿得到、且真的有用的东西。
pub async fn about() -> ApiResult<AboutView> {
    let changes = BUILD_LOG
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| match l.split_once(' ') {
            Some((hash, subject)) => Change {
                hash: hash.to_string(),
                subject: subject.to_string(),
            },
            None => Change {
                hash: l.trim().to_string(),
                subject: String::new(),
            },
        })
        .collect();
    Ok(Json(AboutView {
        name: "mcloud",
        version: env!("CARGO_PKG_VERSION"),
        commit: BUILD_COMMIT.trim().to_string(),
        dirty: BUILD_DIRTY.trim() == "dirty",
        build_time: BUILD_TIME.trim().to_string(),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        changes,
    }))
}

#[derive(Serialize)]
pub struct FileSize {
    pub name: String,
    pub bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageView {
    /// 数据目录（绝对路径）。
    pub data_dir: String,
    /// 日志目录（`<数据目录>/logs`）。
    pub log_dir: String,
    /// 当前进程的可执行文件。
    pub binary: String,
    /// 数据文件合计（**不含**日志）。
    pub data_bytes: u64,
    /// 日志合计（`data/logs` 下递归）。
    pub log_bytes: u64,
    /// 数据目录顶层的数据文件（名字 + 大小）。
    ///
    /// 刻意**不含** `.bak`（那是覆盖前的备份，翻出来只会让人以为是两份数据）
    /// 与目录本身（`logs/` 单独报）。
    pub files: Vec<FileSize>,
}

/// 单个文件的字节数；读不到算 0（**不让这个接口失败** —— 报告"多大"失败毫无意义）。
fn file_bytes(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// 目录递归求和。权限不足/断裂的符号链接一律跳过。
fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => dir_bytes(&p),
                Ok(t) if t.is_file() => file_bytes(&p),
                _ => 0,
            }
        })
        .sum()
}

pub async fn storage(State(state): State<ServerState>) -> ApiResult<StorageView> {
    // ⚠ 报**绝对路径**：这一块的用处就是"照着它去磁盘上找"，
    // 而数据目录可以是相对的（launchd 的 `MCLOUD_DATA_DIR=data` ⇒ 相对 cwd）。
    // `canonicalize` 失败（目录还没建）就退回原值 —— 报一个相对路径也比报错强。
    let raw: PathBuf = state.handle.data_dir.clone();
    let data_dir = std::fs::canonicalize(&raw).unwrap_or(raw);
    let log_dir = data_dir.join("logs");

    let mut files: Vec<FileSize> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&data_dir) {
        for e in entries.flatten() {
            let path = e.path();
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            // `.bak` 是覆盖前的备份，不算"数据文件"（否则占用会凭空翻倍）
            if name.ends_with(".bak") {
                continue;
            }
            files.push(FileSize {
                name,
                bytes: file_bytes(&path),
            });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(StorageView {
        data_dir: data_dir.display().to_string(),
        log_dir: log_dir.display().to_string(),
        binary: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(读不到)".into()),
        data_bytes: files.iter().map(|f| f.bytes).sum(),
        log_bytes: dir_bytes(&log_dir),
        files,
    }))
}

/// 重启服务。
///
/// 返回 `202`（受理）：**此刻还没关**，300ms 后才优雅退出 —— 立刻触发的话
/// 这一个响应本身会被拦腰截断，前端只看得到"网络错误"，分不清"重启了"还是"崩了"。
pub async fn restart(
    State(state): State<ServerState>,
) -> Result<StatusCode, crate::error::ApiError> {
    let shutdown = Arc::clone(&state.handle.shutdown);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tracing::info!("WebUI 请求重启：优雅退出，等托管方（launchd / systemd）重拉");
        shutdown.notify_one();
    });
    Ok(StatusCode::ACCEPTED)
}
