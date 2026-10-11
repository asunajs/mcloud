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

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth_gate::{require_admin, CurrentUser};
use crate::error::{ApiError, ApiResult};
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
    /// 日志目录的用量与上限（用户可设 ✓）。
    pub log_usage: LogUsageView,
    /// **服务端日志**目录（`<数据目录>/server-logs`）—— 与运行日志分开 ✓。
    pub server_log_dir: String,
    /// 服务端日志的用量与上限 ✓。
    pub server_log_usage: LogUsageView,
    /// 用户可设的容量上限 —— 与 [`LogUsageView`] 一起给界面渲染"当前 / 上限" ✓。
    pub limits: LimitsView,
}

/// 日志目录的用量读数（`logUsage` ✓）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogUsageView {
    /// 目录里有几个日志文件（不含 `.bak` ✓）。
    pub files: usize,
    /// 目录合计字节（含 `.bak` ✓ —— 它照样占磁盘 ✓）。
    pub bytes: u64,
    /// 当前生效的总量上限（字节 ✓）—— 界面显示成 `占用 / 上限` ✓。
    pub max_bytes: u64,
}

/// 用户可设的**容量上限**（`limits` ✓）。
///
/// 用户口径 2026-10-09：「日志和历史不要全加入内存，使用本地记录，
/// **让用户设置最大大小**」✓ —— 这五格就是那个"最大大小" ✓
/// （前两格是**历史/运行日志**，后两格是**服务端日志**：内存窗口 + 磁盘总量 ✓）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsView {
    /// 运行历史保留多少条。
    pub history_limit: usize,
    /// **单次运行**留多少行日志。
    pub log_limit_per_run: usize,
    /// `data/logs/` 的总量上限（MB ✓）。
    pub log_total_mb: u64,
    /// **服务端日志**在内存里留多少行（窗口 ✓；更早的按需读盘 ✓）。
    pub server_log_mem_lines: usize,
    /// `data/server-logs/` 的总量上限（MB ✓）。
    pub server_log_total_mb: u64,
}

/// 改容量上限（`PUT /api/admin/storage` ✓）。
///
/// # ⚠ 为什么每格都是 `Option`（**给哪格改哪格**）
///
/// 原先这里是"三格都要给"（PUT = 整体替换 ✓）。2026-10-09 加了服务端日志那两格之后
/// 这条口径就**不能**再要了：界面上那份前端可能还是旧的（只发三格 ✓），
/// 严格要求五格 ⇒ 老页面一保存就 400 ✗ —— 而"加了一个设置项，
/// 另一个设置项突然存不了"是最难归因的那类回归 ✗。
///
/// ⇒ 缺席 = **保持原值** ✓；**一个都不给** ⇒ 400（那多半是调错了接口，
/// 静默成功会让人以为改了 ✓）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePatch {
    #[serde(default)]
    pub history_limit: Option<usize>,
    #[serde(default)]
    pub log_limit_per_run: Option<usize>,
    #[serde(default)]
    pub log_total_mb: Option<u64>,
    #[serde(default)]
    pub server_log_mem_lines: Option<usize>,
    #[serde(default)]
    pub server_log_total_mb: Option<u64>,
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

/// 目录里的文件数与合计字节（一层，不递归 ✓）—— 服务端日志目录就是这么用的
/// （它是平的：一天一个 `.jsonl` ✓）。
fn flat_dir_usage(dir: &Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files = 0usize;
    let mut bytes = 0u64;
    for e in entries.flatten() {
        if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            files += 1;
            bytes += file_bytes(&e.path());
        }
    }
    (files, bytes)
}

/// `GET /api/system/storage` —— **只有管理员**（安全审计 SEC-18）。
///
/// 它报的是绝对路径、二进制位置、每个数据文件的字节数 —— 那是"这台机器怎么装的"
/// 的完整地图，而它不是某个用户的数据，是**实例**的 ✗。单管理员部署下无感，
/// 多用户时它不该是任意登录者都能拿的情报 ✓。
pub async fn storage(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> ApiResult<StorageView> {
    require_admin(&state, &cur).await?;
    Ok(Json(view(&state).await?))
}

/// 组装存储视图（`GET` 与 `PUT` 共用 ✓ —— 免得"改完回的那份"与"再查一次"
/// 长得不一样 ✗）。
async fn view(state: &ServerState) -> Result<StorageView, ApiError> {
    // ⚠ 报**绝对路径**：这一块的用处就是"照着它去磁盘上找"，
    // 而数据目录可以是相对的（launchd 的 `MCLOUD_DATA_DIR=data` ⇒ 相对 cwd）。
    // `canonicalize` 失败（目录还没建）就退回原值 —— 报一个相对路径也比报错强。
    let raw: PathBuf = state.handle.data_dir.clone();
    let data_dir = std::fs::canonicalize(&raw).unwrap_or(raw);
    let log_dir = data_dir.join("logs");
    // ⚠ 服务端日志是**另一个目录**（`server-logs/`）：运行日志是"一次运行一个文件、
    // 整份重写"，服务端日志是"一天一个文件、追加写" ⇒ 两者的淘汰单位不同 ✓。
    let server_log_dir = data_dir.join(crate::logbuf::LOG_DIR_NAME);

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

    // 日志用量：走存储层那一份读数（它按需扫目录 ✓，不维护计数器 ——
    // 计数器要与删除/迁移/外部清理三处同步，漏一处就永远对不上 ✗）。
    let stats = state.handle.store.log_stats().await?;
    let s = state.handle.store.settings().await?;

    Ok(StorageView {
        data_dir: data_dir.display().to_string(),
        log_dir: log_dir.display().to_string(),
        binary: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(读不到)".into()),
        data_bytes: files.iter().map(|f| f.bytes).sum(),
        log_bytes: dir_bytes(&log_dir),
        files,
        log_usage: LogUsageView {
            files: stats.files,
            bytes: stats.bytes,
            max_bytes: stats.max_bytes,
        },
        server_log_dir: server_log_dir.display().to_string(),
        // 服务端日志目录是**平的**（一天一个文件 ✓）⇒ 一层就够，不走递归 ✓
        server_log_usage: {
            let (files, bytes) = flat_dir_usage(&server_log_dir);
            LogUsageView {
                files,
                bytes,
                max_bytes: s.server_log_total_bytes(),
            }
        },
        limits: LimitsView {
            history_limit: s.history_limit,
            log_limit_per_run: s.log_limit_per_run,
            log_total_mb: s.log_total_mb,
            server_log_mem_lines: s.server_log_mem_lines,
            server_log_total_mb: s.server_log_total_mb,
        },
    })
}

/// `PUT /api/admin/storage` —— 改**容量上限**（只有管理员 ✓，与 `restart` 同一条口径）。
///
/// # 为什么"读-改-写"
///
/// `AppSettings` 是一份**整体**（安全那两格也在里面 ✓）。直接拿请求体构造一个新
/// 的会把口令下限、会话时长重置成默认值 ✗ —— 那种 bug 只有在"先改容量、再登录"
/// 时才现形，极难查。所以两个接口各改**自己那几格**，其余原样带回去 ✓。
///
/// # 改完立刻生效
///
/// `JsonStore::save_settings` 里会把新上限**应用一次**（历史超出 ⇒ 丢最旧的；
/// 日志超预算 ⇒ 按最旧删 ✓）—— 用户把上限调小之后不必等下一次运行 ✓。
pub async fn put_storage(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(body): Json<StoragePatch>,
) -> ApiResult<StorageView> {
    require_admin(&state, &cur).await?;
    // ⚠ 一个都不给 ⇒ 400（见 `StoragePatch` 的说明：静默成功会让人以为改了 ✓）
    if body.history_limit.is_none()
        && body.log_limit_per_run.is_none()
        && body.log_total_mb.is_none()
        && body.server_log_mem_lines.is_none()
        && body.server_log_total_mb.is_none()
    {
        return Err(ApiError(mcloud_core::Error::Config(
            "至少要给一格上限（historyLimit / logLimitPerRun / logTotalMb / serverLogMemLines / serverLogTotalMb）"
                .into(),
        )));
    }
    let mut next = state.handle.store.settings().await?;
    // 缺席 = 保持原值 ✓（改哪格写哪格 —— 老前端只发三格也不会把新两格冲掉 ✓）
    if let Some(v) = body.history_limit {
        next.history_limit = v;
    }
    if let Some(v) = body.log_limit_per_run {
        next.log_limit_per_run = v;
    }
    if let Some(v) = body.log_total_mb {
        next.log_total_mb = v;
    }
    if let Some(v) = body.server_log_mem_lines {
        next.server_log_mem_lines = v;
    }
    if let Some(v) = body.server_log_total_mb {
        next.server_log_total_mb = v;
    }
    next.validate()
        .map_err(|e| ApiError(mcloud_core::Error::Config(e)))?;
    state.handle.store.save_settings(&next).await?;
    // ⚠ **服务端日志那两格要单独告诉 `logbuf`**：它活在进程级静态量里（不是走 store 读的 ✓）
    // ⇒ 不推给它的话，"改了内存行数/磁盘上限"要等到下次重启才生效 ✗
    // （而"改完看不出变化"正是用户最烦的那种反馈 ✓）。
    crate::logbuf::apply_settings(&next);
    tracing::info!(
        history_limit = next.history_limit,
        log_limit_per_run = next.log_limit_per_run,
        log_total_mb = next.log_total_mb,
        server_log_mem_lines = next.server_log_mem_lines,
        server_log_total_mb = next.server_log_total_mb,
        "存储上限已更新（立即生效：超出部分会按最旧的淘汰）"
    );
    Ok(Json(view(&state).await?))
}

/// 重启服务。
///
/// 返回 `202`（受理）：**此刻还没关**，300ms 后才优雅退出 —— 立刻触发的话
/// 这一个响应本身会被拦腰截断，前端只看得到"网络错误"，分不清"重启了"还是"崩了"。
///
/// ⚠ **只有管理员**（安全审计 SEC-07）：这是**实例级**动作，停的是所有人的服务，
/// 不是"重启我的东西"。多用户时它是现成的 DoS 开关（谁都能反复重启）✗。
pub async fn restart(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
) -> Result<StatusCode, crate::error::ApiError> {
    require_admin(&state, &cur).await?;
    let shutdown = Arc::clone(&state.handle.shutdown);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tracing::info!("WebUI 请求重启：优雅退出，等托管方（launchd / systemd）重拉");
        shutdown.notify_one();
    });
    Ok(StatusCode::ACCEPTED)
}
