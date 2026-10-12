//! `mcloud config export` / `mcloud config import <文件>` —— 把**列表型配置**
//! 从一台机器搬到另一台。
//!
//! # 为什么要有这两条命令
//!
//! 推送规则、面板绑定、兑换订阅、定时、编排这些**列表型**配置，界面上的入口全在
//! 后端 API 上（WebUI 只是个壳）⇒ 没有浏览器、只有 SSH、或者界面上某一页点不动
//! 的机器，用户**配不了**它们。这两条命令给出一条离线通路：在任意一台有界面的
//! 机器上配好，导出成一份 JSON，拷到无头机器上导入。
//!
//! # 为什么只加这一对命令
//!
//! `AGENTS.md` 的「补充裁定 2」写着 **CLI 要简单**（用户原话：「你把 webui 的功能
//! 都搬过去，cli 还是人用的吗」）。导出/导入是一对**通用的搬运口**，不给 CLI 添
//! 任何"配置面板"形状的东西 —— 所以是两条命令，不是"每个配置项各一条子命令"。
//!
//! # 哪些进文件、哪些不进
//!
//! 进文件的（键名**照 `data/` 下的文件名**，这样导出文件与 `data/*.json` 能对着看，
//! 排查时不用再翻译一层）：
//!
//! | 键 | 存储里的落点 | 是什么 |
//! |---|---|---|
//! | `push-rules` | `data/push-rules.json` | 推送规则（**渠道 token 在里面**） |
//! | `push-panel` | `data/push-panel.json` | 面板行为的绑定（登录新设备 / 登录失败 / 耗时异常 / 连续失败） |
//! | `exchange-subscriptions` | `data/exchange-subscriptions.json` | 兑换页订阅的那几件奖品 |
//! | `auth-refresh` | `data/auth-refresh.json` | 凭据自动刷新的钟点（`HH:MM`） |
//! | `pipelines` | `data/pipelines.json` | 任务编排 |
//! | `schedules` | `data/schedules.json` | 定时（含系统维护的那两条自动条目） |
//! | `settings` | `data/settings.json` | 实例级设置（见下面那段取舍） |
//!
//! 不进文件的：**凭据**（`accounts.json` / `state.json` / `smid.json` /
//! `sessions.json`）。那是登录态：搬到另一台机器**没用**（账号要在新机器上重新
//! 登录），却平白多一份泄漏面。
//!
//! 注意 `push-rules` 里**确实**带着推送渠道的 token 与 webhook 地址 —— 那些本来
//! 就是"配置"的一部分（不带就没法在另一台机器上把通知发出去）。所以文件里留了
//! 一个 `notice` 字段把这件事写明，让用户知道这份文件别乱发。
//!
//! # 实例级设置（`data/settings.json`）怎么处置
//!
//! 取舍：**导出带上，导入默认不动**；要一起改得显式加 `--with-instance`。
//!
//! 理由：那几格（口令下限、会话时长、历史/日志上限、实例级刷新钟点）是**整台机器**
//! 的策略，不是"我这个人的那份配置"。而这两条命令的语义是"把管理员名下那份配置搬
//! 到本机"——顺手把实例策略也改掉，就是那种"手滑一下、整台机器行为就变了"的形态。
//! 所以默认只在导出文件里**读出来给人看**，改不改由 `--with-instance` 显式说。
//!
//! # 导入的语义
//!
//! * **覆盖式**：文件里有的那一类**整类替换**（本机多出来的那几条会被删掉）；
//!   文件里**没有**的类别一个字节都不动。
//! * **先备份**：动手之前把要改的文件备成 `<名字>.bak-<时间戳>`，中途失败自动回滚。
//! * **先全解析、后落盘**：读文件、认版本、逐类校验、id 归属冲突检查全过了，才动第一笔。
//! * **归属重指**：文件里的 `owner` 一律改成**本机那个用户**（见 [`repoint_id`]）——
//!   两台机器的用户 id 是各自生成的 uuid，照抄过去那份数据谁也看不见。
//! * 导完打印**逐类条数**，并提醒**重启服务**才生效（`JsonStore` 启动时全量读进内存）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Context;
use mcloud_app::App;
use mcloud_core::push::BehaviorKey;
use mcloud_core::{
    auth_refresh, AppSettings, ExchangeSubscription, PanelBindings, Pipeline, PipelineId, PushRule,
    Schedule, ScheduleId, UserId,
};

use crate::cli::push::{cli_owner, reject_user_selection};

pub const USAGE: &str = "\
用法：
  mcloud config export [--out <路径>]       导出本机用户的配置
                                            （省略 --out 就打 stdout，可重定向到文件）
  mcloud config import <路径> [--with-instance]
                                            把那份 JSON **覆盖式**应用到本机用户
  mcloud config help                        打印帮助（`--help` / `-h` 同；
                                            打的是 `mcloud --help` 那份命令清单）
  mcloud config                             不带子命令 = 打印下面这份详细用法

CLI 上**没有** `--user` / `--owner` —— 两条命令都只作用于**本机那个用户**
（= 管理员，创建最早的那个；`mcloud user list` 里标着「管理员」的那个）。
想搬**别人**的配置：让他登录 WebUI 自己配（多用户是给不同设备的用户用的）。

导出/导入哪几类（键名就是 `data/` 下那几个文件名，导出文件里能一一对上）：
  push-rules              推送规则（注意：渠道 token 就在里面）
  push-panel              面板行为（登录新设备 / 登录失败 / 耗时异常 / 连续失败）的绑定
  exchange-subscriptions  兑换页订阅的那几件奖品
  auth-refresh            凭据自动刷新的钟点（HH:MM）
  pipelines               任务编排
  schedules               定时（含系统维护的「直播自动定时」「凭据自动刷新」两条）
  settings                实例级设置 —— 导出带上、导入默认不动（要一起改加 --with-instance）

登录凭据（token / 登录态）**不在**这份文件里 —— 那是账号的事，搬到另一台机器也没用。
导入是**覆盖式**：文件里有的那一类整类替换（本机多出来的那几条会被删掉）；
文件里**没有**的类别一个字节都不动。
导入前会先把要改的文件备份成 `<名字>.bak-<时间戳>`，中途失败自动回滚到导入前那份。
导入完**必须重启服务**才生效（`JsonStore` 启动时把那些文件全量读进内存）：
    scripts/dev-reload.sh --no-build";

/// 顶层版本键：导入时**必须**认它 —— 认不出就报错退非 0，一个字节都不改。
const FORMAT_KEY: &str = "mcloudConfig";

/// 当前格式版本。改形状时 +1（老文件会被**明确拒掉**，而不是"读进去一半"）。
const FORMAT_VERSION: u32 = 1;

/// 导出文件里那句"这份文件别乱发"的说明（见模块头那段）。
const NOTICE: &str = "这份文件是配置、不是凭据：它含推送渠道的 token 与 webhook 地址\
（原样导出 —— 不带就没法在另一台机器上把通知发出去），别随手发给别人。\
登录凭据（token / 登录态）不在里面，账号要在新机器上重新登录。";

/// 命令行解析结果（纯数据，便于单测）。
#[derive(Debug, PartialEq, Eq)]
enum Cmd {
    /// 打帮助（`help` / `--help` / `-h`，以及**不带子命令**）。
    Help,
    Export {
        out: Option<PathBuf>,
    },
    Import {
        path: PathBuf,
        with_instance: bool,
    },
}

/// 跑一条 `config` 命令。`args` 是**去掉 `config` 之后**的参数。
pub async fn run(app: &App, args: &[String]) -> anyhow::Result<()> {
    let cmd = parse(args)?;
    match cmd {
        Cmd::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Cmd::Export { out } => export(app, out).await,
        Cmd::Import {
            path,
            with_instance,
        } => import_cmd(app, &path, with_instance).await,
    }
}

/// 解析参数（**只有这一份** —— 行为与用法文本不会分叉）。
///
/// 不带子命令 ⇒ [`Cmd::Help`]：这是"给人话"，不是"悄悄做点什么"
/// （与 `cli/live_room.rs` 不带子命令 = 打帮助同一条口径）。
fn parse(args: &[String]) -> anyhow::Result<Cmd> {
    // CLI 不提供"选谁"的参数（判据在 `cli/push.rs::reject_user_selection`）。
    // 先扫一遍整条 argv：只在首位扫的话，`config import f.json --user bob`
    // 那一格会被**静默忽略** —— 那比报错糟得多。
    reject_user_selection(args)?;
    match args.first().map(String::as_str) {
        Some("export") => parse_export(&args[1..]),
        Some("import") => parse_import(&args[1..]),
        Some("help" | "--help" | "-h") | None => Ok(Cmd::Help),
        Some(other) => anyhow::bail!("未知的 config 子命令：{other}\n\n{USAGE}"),
    }
}

fn parse_export(rest: &[String]) -> anyhow::Result<Cmd> {
    let mut out = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--out" => {
                i += 1;
                let raw = rest.get(i).context("--out 后面要给一个路径")?;
                out = Some(PathBuf::from(raw));
            }
            other => anyhow::bail!("export 不认识这个参数：{other}\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(Cmd::Export { out })
}

fn parse_import(rest: &[String]) -> anyhow::Result<Cmd> {
    let mut path: Option<PathBuf> = None;
    let mut with_instance = false;
    for a in rest {
        match a.as_str() {
            "--with-instance" => with_instance = true,
            // 以 `-` 开头的都不是路径 ⇒ 当场拒（免得被当成文件名去读）
            other if other.starts_with('-') => {
                anyhow::bail!("import 不认识这个参数：{other}\n\n{USAGE}")
            }
            other => {
                if path.is_some() {
                    anyhow::bail!("import 只接一个文件（多给了「{other}」）\n\n{USAGE}");
                }
                path = Some(PathBuf::from(other));
            }
        }
    }
    let path = path.context("用法：mcloud config import <文件> [--with-instance]")?;
    Ok(Cmd::Import {
        path,
        with_instance,
    })
}

// ── 导出文件的形状 ──────────────────────────────────────────────────────

/// 导出时那份"我是谁"的信息。
///
/// 它有两个用处：人看着知道这份是从哪个账号导的；导入时**按它算 id 重指**
/// （见 [`repoint_id`] —— `daily-<owner>` 这类 id 是从 owner 推出来的）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OwnerRef {
    id: String,
    name: String,
}

/// 导出/导入的那一份文档。
///
/// 每一类都是 `Option`：`None` 既表示"导出时本机这一类是空的"，
/// 也表示"导入时**不许动**这一类"。导入侧只认后者 —— 文件里没写的键
/// 反序列化出来就是 `None`，于是"文件里没有的类别不动"是**结构上**成立的，
/// 不靠一句 `if` 记得去判。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ExportDoc {
    #[serde(rename = "mcloudConfig")]
    format: u32,
    #[serde(
        rename = "exportedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    exported_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<OwnerRef>,
    #[serde(
        rename = "push-rules",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    push_rules: Option<Vec<PushRule>>,
    #[serde(
        rename = "push-panel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    push_panel: Option<PanelBindings>,
    #[serde(
        rename = "exchange-subscriptions",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    exchange_subscriptions: Option<Vec<ExchangeSubscription>>,
    #[serde(
        rename = "auth-refresh",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    auth_refresh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pipelines: Option<Vec<Pipeline>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schedules: Option<Vec<Schedule>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings: Option<AppSettings>,
}

impl ExportDoc {
    /// 文件里那个"源 owner"（没有就不重指 id）。
    fn source_owner(&self) -> Option<UserId> {
        self.owner.as_ref().map(|o| UserId::new(o.id.clone()))
    }
}

/// 读 store，组装成一份文档。
///
/// **稳定排序**（同一次导出跑两遍结果一样，方便 diff）：三类有 id 的按 id 升序；
/// 订阅按"订阅时刻 + 奖品 id"排（那本来就是它的语义顺序 —— 兑换页顶部按它从早到晚）。
async fn collect(app: &App, owner: &UserId) -> anyhow::Result<ExportDoc> {
    let store = &app.handle.store;
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

    Ok(ExportDoc {
        format: FORMAT_VERSION,
        exported_at: Some(mcloud_core::beijing_format(
            &*app.handle.clock,
            "%Y-%m-%d %H:%M:%S",
        )),
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
    })
}

fn to_json(doc: &ExportDoc) -> anyhow::Result<String> {
    let mut s = serde_json::to_string_pretty(doc)?;
    s.push('\n');
    Ok(s)
}

/// 逐类一句话（导出与导入两边共用这一份措辞）。
fn describe(doc: &ExportDoc) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = &doc.push_rules {
        out.push(format!("推送规则 {} 条", v.len()));
    }
    if doc.push_panel.is_some() {
        out.push(format!("面板绑定 {} 个", BehaviorKey::ALL.len()));
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
    out
}

// ── export ─────────────────────────────────────────────────────────────

async fn export(app: &App, out: Option<PathBuf>) -> anyhow::Result<()> {
    let owner = cli_owner(app).await?;
    let doc = collect(app, &owner).await?;
    let json = to_json(&doc)?;
    match out {
        // 省略 `--out` ⇒ **只有** JSON 进 stdout（`mcloud config export > f.json` 能用）
        None => print!("{json}"),
        Some(path) => {
            write_private(&path, json.as_bytes())?;
            println!(
                "已导出本机用户「{owner}」的配置到 {}：{}",
                path.display(),
                describe(&doc).join(" / ")
            );
            println!("注意：文件里有推送渠道的 token 与 webhook 地址，别随手发给别人。");
            println!("      拷到目标机器后：mcloud config import <文件>");
        }
    }
    Ok(())
}

/// 写导出文件：**落盘就是 0600**，并且先写临时文件再改名。
///
/// 两条都照 `JsonStore::write_atomic` 的理由来：文件里有推送渠道的 token，
/// 先按默认权限（通常 0644）写、再 `chmod` 收紧，中间那一小段窗口同机其他用户
/// 正好读得到；而直接往目标路径写，半截文件会把上一份好文件顶掉。
fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));

    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
    };
    #[cfg(not(unix))]
    let file = std::fs::File::create(&tmp);

    let mut file =
        file.with_context(|| format!("建不了临时文件 {}（目录在不在？）", tmp.display()))?;

    use std::io::Write as _;
    if let Err(e) = file.write_all(bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("写不进 {}", tmp.display()));
    }
    // 内容落盘之后再改名：否则改名会把一个"还没落盘的空壳"顶上去。
    let _ = file.sync_all();
    drop(file);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("改名失败：{} → {}", tmp.display(), path.display()))?;
    Ok(())
}

// ── import ─────────────────────────────────────────────────────────────

/// 导入的读数（打印用；也是单测的断言面）。
#[derive(Debug, Default)]
struct Report {
    push_rules: Option<Counts>,
    push_panel: bool,
    exchange_subscriptions: Option<usize>,
    auth_refresh: Option<String>,
    pipelines: Option<Counts>,
    schedules: Option<Counts>,
    instance: Option<bool>,
}

/// 列表型那一类的条数读数。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Counts {
    total: usize,
    added: usize,
    updated: usize,
    removed: usize,
}

impl Counts {
    fn line(&self, unit: &str) -> String {
        format!(
            "共 {} {unit}（新增 {}、更新 {}、删除 {}）",
            self.total, self.added, self.updated, self.removed
        )
    }
}

impl Report {
    fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(c) = &self.push_rules {
            out.push(format!("  推送规则              {}", c.line("条")));
        }
        if self.push_panel {
            out.push(format!(
                "  面板绑定              已替换（{} 个行为）",
                BehaviorKey::ALL.len()
            ));
        }
        if let Some(n) = self.exchange_subscriptions {
            out.push(format!("  兑换订阅              共 {n} 条"));
        }
        if let Some(at) = &self.auth_refresh {
            out.push(format!("  刷新钟点              {at}"));
        }
        if let Some(c) = &self.pipelines {
            out.push(format!("  编排                  {}", c.line("条")));
        }
        if let Some(c) = &self.schedules {
            out.push(format!("  定时                  {}", c.line("条")));
        }
        if let Some(applied) = self.instance {
            out.push(format!(
                "  实例设置              {}",
                if applied {
                    "已应用（--with-instance）"
                } else {
                    "没动（要一起改，加 --with-instance）"
                }
            ));
        }
        out
    }
}

async fn import_cmd(app: &App, path: &Path, with_instance: bool) -> anyhow::Result<()> {
    let owner = cli_owner(app).await?;

    // ① 读文件 + 认版本 + 逐类校验 + id 归属冲突检查：**全在这一步做完**，
    //    任一条不过就到此为止 —— 盘上一个字节都还没动。
    //
    //    后两步的报错都**带上文件名**：只说"`schedules` 里的 cron 不合法"，
    //    而用户手上可能同时有几份导出文件 —— 说清是哪一份才叫说清了。
    let doc = read_doc(path)?;
    validate_doc(&doc).map_err(|e| anyhow::anyhow!("{} 里有不合法的配置：{e}", path.display()))?;
    check_id_conflicts(app, &owner, &doc)
        .await
        .map_err(|e| anyhow::anyhow!("{} 与本机对不上：{e}", path.display()))?;

    // ② 备份要改的那几个文件（失败回滚的依据）。
    let names = touched_files(&doc, with_instance);
    let stamp = mcloud_core::beijing_format(&*app.handle.clock, "%Y%m%d-%H%M%S");
    let backups = backup_files(&app.handle.data_dir, &names, &stamp)?;

    // ③ 落盘。中途任何一步失败 ⇒ 把备份盖回去（并说清回滚有没有做干净）。
    let report = match apply(app, &owner, &doc, with_instance).await {
        Ok(r) => r,
        Err(e) => {
            let errs = rollback(&backups);
            if errs.is_empty() {
                anyhow::bail!("导入失败，已回滚到导入前那份：{e}");
            }
            anyhow::bail!(
                "导入失败，且回滚没做干净：{e}；回滚报错：{}",
                errs.join("；")
            );
        }
    };

    println!("已导入到本机用户「{owner}」名下：");
    for line in report.lines() {
        println!("{line}");
    }
    println!();
    println!("注意：导入只改了盘上的文件，跑着的服务还是它启动时读进内存的那一份 ——");
    println!("      要生效得重启服务：");
    println!("        scripts/dev-reload.sh --no-build");
    Ok(())
}

/// 读文件并认出"这是不是本命令导出的配置"。
///
/// 四种失败各说清是哪一种（文件读不到 / 不是 JSON / 没有版本键 / 版本不认识），
/// 并且**都带上文件名** —— 只报一句"解析失败"等于让人去猜是哪个文件坏了。
fn read_doc(path: &Path) -> anyhow::Result<ExportDoc> {
    let text = std::fs::read_to_string(path).with_context(|| {
        format!(
            "读不到导入文件：{}（先确认路径对不对；导出用 `mcloud config export --out <文件>`）",
            path.display()
        )
    })?;

    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("{} 不是合法 JSON：{e}", path.display()))?;

    {
        let version = value.get(FORMAT_KEY);
        match version.and_then(|v| v.as_u64()) {
            Some(n) if n == u64::from(FORMAT_VERSION) => {}
            Some(n) => anyhow::bail!(
                "{} 里的 `{FORMAT_KEY}` 是 {n}，本命令只认 {FORMAT_VERSION} \
                 —— 这份文件多半来自另一个版本的 mcloud",
                path.display()
            ),
            None => match version {
                Some(v) => anyhow::bail!(
                    "{} 里的 `{FORMAT_KEY}` 不是数字（实得 {v}）—— 文件被改坏了？",
                    path.display()
                ),
                None => anyhow::bail!(
                    "{} 顶层没有 `{FORMAT_KEY}` 字段 —— 这不是本命令导出的配置\
                     （`mcloud config export` 生成的文件里一定有它）",
                    path.display()
                ),
            },
        }
    }

    serde_json::from_value(value).with_context(|| {
        format!(
            "{} 的形状不对（按 `mcloud config export` 导出的那份逐类核对一遍）",
            path.display()
        )
    })
}

/// 逐类校验：**存之前**就把不合法的拦下（与 WebUI 路由走的是同一批判据）。
///
/// 这一步是"要么全改、要么不改"的落点：校验不过 ⇒ 盘上一个字节都没动。
fn validate_doc(doc: &ExportDoc) -> anyhow::Result<()> {
    if let Some(rules) = &doc.push_rules {
        ensure_unique("push-rules", rules.iter().map(|r| r.id.as_str()))?;
        for r in rules {
            if r.id.as_str().trim().is_empty() {
                anyhow::bail!("`push-rules` 里有一条没有 id（名字是「{}」）", r.name);
            }
            r.validate().map_err(|e| {
                anyhow::anyhow!("`push-rules` 里的「{}」（{}）不合法：{e}", r.name, r.id)
            })?;
        }
    }
    if let Some(panel) = &doc.push_panel {
        panel
            .validate()
            .map_err(|e| anyhow::anyhow!("`push-panel` 不合法：{e}"))?;
    }
    if let Some(subs) = &doc.exchange_subscriptions {
        // 与 `save_exchange_subscriptions` 同一判据（重复的 `prize_id` 会让同一件
        // 在顶部出现两行）—— 在这一步拦住，就不必等到写盘时失败再回滚。
        let mut seen = HashSet::new();
        for s in subs {
            if !seen.insert(s.prize_id) {
                anyhow::bail!(
                    "`exchange-subscriptions` 里奖品 id {} 出现了两次 —— 文件被改坏了",
                    s.prize_id
                );
            }
        }
    }
    if let Some(at) = &doc.auth_refresh {
        if auth_refresh::parse_clock(at).is_none() {
            anyhow::bail!("`auth-refresh` 要是 24 小时制的 HH:MM（实得「{at}」）");
        }
    }
    if let Some(pipes) = &doc.pipelines {
        ensure_unique("pipelines", pipes.iter().map(|p| p.id.as_str()))?;
        for p in pipes {
            if p.id.as_str().trim().is_empty() {
                anyhow::bail!("`pipelines` 里有一条没有 id（名字是「{}」）", p.name);
            }
            p.validate().map_err(|e| {
                anyhow::anyhow!("`pipelines` 里的「{}」（{}）不合法：{e}", p.name, p.id)
            })?;
        }
    }
    if let Some(schs) = &doc.schedules {
        ensure_unique("schedules", schs.iter().map(|s| s.id.as_str()))?;
        for s in schs {
            if s.id.as_str().trim().is_empty() {
                anyhow::bail!("`schedules` 里有一条没有 id（名字是「{}」）", s.name);
            }
            if s.task.trim().is_empty() {
                anyhow::bail!("`schedules` 里的「{}」（{}）没有任务名", s.name, s.id);
            }
            // cron 判据只有一处（`Schedule::validate_expr`），别在这儿再写一遍
            Schedule::validate_expr(&s.expr).map_err(|e| {
                anyhow::anyhow!(
                    "`schedules` 里的「{}」（{}）的 cron 表达式不合法：{e}",
                    s.name,
                    s.id
                )
            })?;
        }
    }
    if let Some(set) = &doc.settings {
        set.validate()
            .map_err(|e| anyhow::anyhow!("`settings`（实例级设置）不合法：{e}"))?;
    }
    Ok(())
}

fn ensure_unique<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            anyhow::bail!("`{kind}` 里 {id} 出现了两次 —— 文件被改坏了");
        }
    }
    Ok(())
}

/// 定时与编排的 id 是**全局唯一**的（`save_schedule` / `save_pipeline` 按 id upsert），
/// 所以"文件里这条 id 在本机已经属于**别人**"必须**当场停下**：照 upsert 走下去
/// 等于把别人的那条**抢过来**，而 CLI 的立场是"只动本机管理员那一个用户"。
async fn check_id_conflicts(app: &App, owner: &UserId, doc: &ExportDoc) -> anyhow::Result<()> {
    let src = doc.source_owner();
    if let Some(schs) = &doc.schedules {
        let all = app.handle.store.schedules().await?;
        for s in schs {
            let id = ScheduleId::new(repoint_id(s.id.as_str(), src.as_ref(), owner));
            if let Some(hit) = all
                .iter()
                .find(|x| x.id == id && x.owner.as_ref().is_some_and(|o| o != owner))
            {
                anyhow::bail!(
                    "定时 {} 在本机属于**另一个用户**（{}）—— 导入会把它抢过来，先停在这里。\
                     要么改掉文件里的 id，要么让那个用户在 WebUI 上自己配。",
                    id,
                    hit.owner
                        .as_ref()
                        .map(UserId::to_string)
                        .unwrap_or_default()
                );
            }
        }
    }
    if let Some(pipes) = &doc.pipelines {
        let all = app.handle.store.pipelines().await?;
        for p in pipes {
            let id = PipelineId::new(repoint_id(p.id.as_str(), src.as_ref(), owner));
            if let Some(hit) = all
                .iter()
                .find(|x| x.id == id && x.owner.as_ref().is_some_and(|o| o != owner))
            {
                anyhow::bail!(
                    "编排 {} 在本机属于**另一个用户**（{}）—— 导入会把它抢过来，先停在这里。",
                    id,
                    hit.owner
                        .as_ref()
                        .map(UserId::to_string)
                        .unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}

/// 把 id 里"由 owner 推出来的那几条"重指到本机这个用户。
///
/// 三条 id 是**算出来的**、不是用户随手起的：默认编排/定时的 `daily-<owner>`
/// （`pipeline::default_pipeline_id`）、直播自动定时的 `auto-live-<owner>`
/// （`live_auto::live_auto_schedule_id`）、凭据自动刷新的 `auto-auth-refresh-<owner>`
/// （`auth_refresh::auth_refresh_schedule_id`）。
///
/// 跨机器导入时 owner 是**另一个 uuid** ⇒ 照抄这份 id 会留下"名字里带着别人 id"
/// 的一条，而系统下次重算时按本机 id 找不到它，会**再建一条**（于是两条定时都在跑）。
/// 所以这里按同一个推导换成本机 id。同机导入（源 owner 就是本机管理员）是**恒等**。
///
/// 文件里没写 `owner`（手写的小文件）⇒ 不重指：那时无从判断哪条 id 是推出来的。
fn repoint_id(id: &str, source: Option<&UserId>, local: &UserId) -> String {
    let Some(src) = source else {
        return id.to_string();
    };
    if src == local {
        return id.to_string();
    }
    let table = [
        (
            mcloud_core::pipeline::default_pipeline_id(Some(src)),
            mcloud_core::pipeline::default_pipeline_id(Some(local)),
        ),
        (
            mcloud_core::live_auto::live_auto_schedule_id(src).to_string(),
            mcloud_core::live_auto::live_auto_schedule_id(local).to_string(),
        ),
        (
            mcloud_core::auth_refresh::auth_refresh_schedule_id(src).to_string(),
            mcloud_core::auth_refresh::auth_refresh_schedule_id(local).to_string(),
        ),
    ];
    for (from, to) in table {
        if id == from {
            return to;
        }
    }
    id.to_string()
}

/// 定时/编排的参数里若引用了上面那类 id（默认编排那条定时的 `body.pipelineId`
/// 就是 `daily-<owner>`），一起重指 —— 否则定时指向一个本机不存在的编排。
fn repoint_body(body: &mut serde_json::Value, source: Option<&UserId>, local: &UserId) {
    let Some(src) = source else {
        return;
    };
    let Some(obj) = body.as_object_mut() else {
        return;
    };
    // 只认 `pipelineId` —— 那是本仓**唯一**一处读编排引用的地方
    // （`routes/schedules.rs`），不凭想象多认几个键。
    let key = "pipelineId";
    if let Some(serde_json::Value::String(v)) = obj.get(key) {
        let new = repoint_id(v, Some(src), local);
        if new != *v {
            obj.insert(key.to_string(), serde_json::Value::String(new));
        }
    }
}

/// 这一类在盘上对应哪个文件（键名照 `data/` 下的文件名）。
///
/// 名字与 `mcloud-store` 的 `files` 模块一一对应；那个模块是**私有**的，
/// 所以这里只能再写一份常量 —— 改文件名时两处一起改（单测里有往返用例兜着）。
fn touched_files(doc: &ExportDoc, with_instance: bool) -> Vec<&'static str> {
    let mut names = Vec::new();
    if doc.push_rules.is_some() {
        names.push("push-rules.json");
    }
    if doc.push_panel.is_some() {
        names.push("push-panel.json");
    }
    if doc.exchange_subscriptions.is_some() {
        names.push("exchange-subscriptions.json");
    }
    if doc.auth_refresh.is_some() {
        names.push("auth-refresh.json");
    }
    if doc.pipelines.is_some() {
        names.push("pipelines.json");
    }
    if doc.schedules.is_some() {
        names.push("schedules.json");
    }
    // 实例级：只有真去改它的时候才需要备份
    if with_instance && doc.settings.is_some() {
        names.push("settings.json");
    }
    names
}

/// 一份"导入前的原样"。
struct Backup {
    path: PathBuf,
    /// 备份文件；`None` = 这个文件本来**不存在**（回滚就是把它删掉）。
    saved: Option<PathBuf>,
}

/// 动手之前把要改的文件备一份：`<名字>.bak-<时间戳>`。
///
/// 时间戳用注入的 `Clock`（仓库约定：时间只来自 `Clock`），粒度到秒 ——
/// 同秒内跑两次导入，第二次的备份会盖掉第一次那份（可接受：要的只是"最近一次导入前"）。
fn backup_files(dir: &Path, names: &[&str], stamp: &str) -> anyhow::Result<Vec<Backup>> {
    let mut out = Vec::new();
    for name in names {
        let path = dir.join(name);
        if !path.exists() {
            out.push(Backup { path, saved: None });
            continue;
        }
        let saved = dir.join(format!("{name}.bak-{stamp}"));
        std::fs::copy(&path, &saved)
            .with_context(|| format!("备份 {} 失败（导入还没开始）", path.display()))?;
        out.push(Backup {
            path,
            saved: Some(saved),
        });
    }
    Ok(out)
}

/// 回滚：把备份盖回原位；本来不存在的文件就删掉。
///
/// 返回做不干净的那几条（空 = 干净）。**不吞错**：回滚失败必须说出来 ——
/// 那时盘上是一半新一半旧，用户得自己看着办。
fn rollback(backups: &[Backup]) -> Vec<String> {
    let mut errs = Vec::new();
    for b in backups {
        let r = match &b.saved {
            Some(saved) => std::fs::copy(saved, &b.path).map(|_| ()),
            None => match std::fs::remove_file(&b.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
        };
        if let Err(e) = r {
            errs.push(format!("{}：{e}", b.path.display()));
        }
    }
    errs
}

/// 真正落盘。**只在 `read_doc` / `validate_doc` / `check_id_conflicts` 全过之后调**。
async fn apply(
    app: &App,
    owner: &UserId,
    doc: &ExportDoc,
    with_instance: bool,
) -> anyhow::Result<Report> {
    let store = &app.handle.store;
    let src = doc.source_owner();
    let mut report = Report::default();

    // 规则：逐条 upsert（归属重指），本机多出来的按 id 删掉。
    if let Some(rules) = &doc.push_rules {
        let existing = store.push_rules(owner).await?;
        let want: HashSet<String> = rules.iter().map(|r| r.id.to_string()).collect();
        let mut counts = Counts {
            total: rules.len(),
            ..Default::default()
        };
        for r in rules {
            let mut rule = r.clone();
            rule.owner = Some(owner.clone());
            if existing.iter().any(|e| e.id == rule.id) {
                counts.updated += 1;
            } else {
                counts.added += 1;
            }
            store.save_push_rule(owner, &rule).await?;
        }
        for e in &existing {
            if !want.contains(e.id.as_str()) {
                store.remove_push_rule(owner, &e.id).await?;
                counts.removed += 1;
            }
        }
        report.push_rules = Some(counts);
    }

    if let Some(panel) = &doc.push_panel {
        store.save_push_panel(owner, panel).await?;
        report.push_panel = true;
    }

    if let Some(items) = &doc.exchange_subscriptions {
        store.save_exchange_subscriptions(owner, items).await?;
        report.exchange_subscriptions = Some(items.len());
    }

    if let Some(at) = &doc.auth_refresh {
        store.save_auth_refresh_at(owner, at).await?;
        report.auth_refresh = Some(at.clone());
    }

    // 编排放在定时**之前**：默认编排那条定时指向 `daily-<owner>`，
    // 先把编排落到本机，后面那条定时才有东西可指。
    if let Some(pipes) = &doc.pipelines {
        let existing = mine_pipelines(app, owner).await?;
        let mut counts = Counts {
            total: pipes.len(),
            ..Default::default()
        };
        let mut want: HashSet<String> = HashSet::new();
        for p in pipes {
            let mut pipe = p.clone();
            pipe.owner = Some(owner.clone());
            pipe.id = PipelineId::new(repoint_id(p.id.as_str(), src.as_ref(), owner));
            want.insert(pipe.id.to_string());
            if existing.iter().any(|e| e.id == pipe.id) {
                counts.updated += 1;
            } else {
                counts.added += 1;
            }
            store.save_pipeline(&pipe).await?;
        }
        for e in &existing {
            if !want.contains(e.id.as_str()) {
                store.remove_pipeline(&e.id).await?;
                counts.removed += 1;
            }
        }
        report.pipelines = Some(counts);
    }

    if let Some(schs) = &doc.schedules {
        let existing = mine_schedules(app, owner).await?;
        let mut counts = Counts {
            total: schs.len(),
            ..Default::default()
        };
        let mut want: HashSet<String> = HashSet::new();
        for s in schs {
            let mut sch = s.clone();
            sch.owner = Some(owner.clone());
            sch.id = ScheduleId::new(repoint_id(s.id.as_str(), src.as_ref(), owner));
            repoint_body(&mut sch.body, src.as_ref(), owner);
            want.insert(sch.id.to_string());
            if existing.iter().any(|e| e.id == sch.id) {
                counts.updated += 1;
            } else {
                counts.added += 1;
            }
            store.save_schedule(&sch).await?;
        }
        for e in &existing {
            if !want.contains(e.id.as_str()) {
                store.remove_schedule(&e.id).await?;
                counts.removed += 1;
            }
        }
        report.schedules = Some(counts);
    }

    // 实例级：**默认不动**（见模块头那段取舍），要改得显式说。
    if with_instance {
        if let Some(settings) = &doc.settings {
            store.save_settings(settings).await?;
            report.instance = Some(true);
        }
    } else if doc.settings.is_some() {
        report.instance = Some(false);
    }

    Ok(report)
}

/// 本机**管理员名下**的定时。
///
/// 用 `schedules()` + 按 `owner` 过滤，而不是 `schedules_for()`：后者按
/// `visible_to` 判可见性，单用户实例上连**未归属**（`owner = None`）的那些也算
/// 可见 —— 而"删除本机多出来的"这一步不该动那些无主条目（它们不是"这个用户
/// 名下的"）。收窄到显式归属，才与"只动本机管理员那一个用户"的立场一致。
async fn mine_schedules(app: &App, owner: &UserId) -> anyhow::Result<Vec<Schedule>> {
    Ok(app
        .handle
        .store
        .schedules()
        .await?
        .into_iter()
        .filter(|s| s.owner.as_ref() == Some(owner))
        .collect())
}

async fn mine_pipelines(app: &App, owner: &UserId) -> anyhow::Result<Vec<Pipeline>> {
    Ok(app
        .handle
        .store
        .pipelines()
        .await?
        .into_iter()
        .filter(|p| p.owner.as_ref() == Some(owner))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use mcloud_app::AppBuilder;
    use mcloud_core::push::{PushConfig, PushRuleId, TokenConfig};
    use mcloud_core::{Account, HttpClient, HttpRequest, HttpResponse, PasswordHash, User};

    /// 「谁来都报网络错」的假上游 —— 这几条用例**绝不打真上游**
    /// （与 `cli/daily.rs` 的 `DeadUpstream` 同款）。
    struct DeadUpstream;

    #[async_trait::async_trait]
    impl HttpClient for DeadUpstream {
        async fn request(&self, req: HttpRequest) -> mcloud_core::Result<HttpResponse> {
            Err(mcloud_core::Error::Network(format!(
                "config CLI 测试的假上游：不打网络（{}）",
                req.url
            )))
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("mcloud-cli-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn test_app(tag: &str) -> (App, PathBuf) {
        let dir = temp_dir(tag);
        let app = AppBuilder::new()
            .data_dir(dir.clone())
            .http_for(Arc::new(|_a: &Account| {
                Arc::new(DeadUpstream) as Arc<dyn HttpClient>
            }))
            .build()
            .expect("应用应能组装");
        (app, dir)
    }

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn seed_user(app: &App, id: &str, name: &str, created_at_ms: i64) {
        let user = User {
            id: UserId::new(id),
            name: name.into(),
            password_hash: PasswordHash("hashed".into()),
            created_at_ms,
        };
        futures::executor::block_on(app.handle.store.save_user(&user)).expect("落库");
    }

    fn rule(id: &str, name: &str) -> PushRule {
        let mut r = PushRule::new_default(id, None);
        r.name = name.into();
        r.config = PushConfig {
            pushplus: Some(TokenConfig {
                token: "1234".into(),
                extra: Default::default(),
            }),
            ..Default::default()
        };
        r
    }

    fn sub(prize_id: i64, name: &str, at_ms: i64) -> ExchangeSubscription {
        ExchangeSubscription {
            prize_id,
            name: name.into(),
            group_id: None,
            group_title: None,
            subscribed_at_ms: at_ms,
        }
    }

    fn pipeline(id: &str, name: &str) -> Pipeline {
        Pipeline {
            id: PipelineId::new(id),
            name: name.into(),
            accounts: Vec::new(),
            account_mode: mcloud_core::AccountMode::default(),
            stop_on_error: true,
            steps: vec![mcloud_core::PipelineStep {
                task: "notice".into(),
                body: serde_json::json!({}),
                continue_on_error: false,
            }],
            enabled: true,
            owner: None,
            push_rules: vec![PushRuleId::new("r1")],
            push_events: Vec::new(),
            last_run_ms: None,
        }
    }

    fn schedule(id: &str, name: &str, expr: &str, managed_by: Option<&str>) -> Schedule {
        Schedule {
            id: ScheduleId::new(id),
            name: name.into(),
            task: "notice".into(),
            accounts: Vec::new(),
            expr: expr.into(),
            timezone: "Asia/Shanghai".into(),
            body: serde_json::json!({}),
            enabled: true,
            managed_by: managed_by.map(str::to_string),
            managed_wanted: None,
            owner: None,
            push_rules: vec![PushRuleId::new("r1")],
            push_events: Vec::new(),
            last_fire_ms: None,
            last_run_ms: None,
            last_status: None,
            last_error: None,
            last_note: None,
            run_count: 3,
        }
    }

    async fn save_schedule_for(app: &App, owner: &UserId, s: Schedule) {
        let mut s = s;
        s.owner = Some(owner.clone());
        app.handle.store.save_schedule(&s).await.expect("落库");
    }

    /// 一份"每类都不空"的配置：两条规则、一份面板绑定、两条订阅、一个钟点、
    /// 一条编排、四条定时（含两条系统维护的自动条目）。别人名下也放点东西。
    ///
    /// 规则按真实落盘形状造（`owner` 那一格填上）—— 路由保存时就是这么写的，
    /// 而导入会把 `owner` 重指到本机管理员 ⇒ 造数据时省掉它，往返用例会
    /// 拿"导入前 owner=None、导入后 owner=Some"去比，那是**夹具**的问题、不是行为的问题。
    async fn seed_rich(app: &App, admin: &UserId, other: &UserId) {
        let store = &app.handle.store;
        let mut r1 = rule("r1", "规则一");
        r1.owner = Some(admin.clone());
        let mut r2 = rule("r2", "规则二");
        r2.owner = Some(admin.clone());
        let mut ro = rule("r-other", "别人的规则");
        ro.owner = Some(other.clone());
        store.save_push_rule(admin, &r1).await.unwrap();
        store.save_push_rule(admin, &r2).await.unwrap();
        store.save_push_rule(other, &ro).await.unwrap();

        let mut panel = PanelBindings::default();
        panel.login_new.rules = vec![PushRuleId::new("r1")];
        store.save_push_panel(admin, &panel).await.unwrap();

        store
            .save_exchange_subscriptions(
                admin,
                &[sub(11, "奖品甲", 1_000), sub(22, "奖品乙", 2_000)],
            )
            .await
            .unwrap();
        store.save_auth_refresh_at(admin, "22:30").await.unwrap();

        let mut pipe = pipeline("daily-admin", "每日一键（默认）");
        pipe.owner = Some(admin.clone());
        store.save_pipeline(&pipe).await.unwrap();

        save_schedule_for(app, admin, schedule("sch-1", "手建一条", "0 8 * * *", None)).await;
        save_schedule_for(
            app,
            admin,
            schedule("daily-admin", "每日一键（默认）", "0 8 * * *", None),
        )
        .await;
        save_schedule_for(
            app,
            admin,
            schedule(
                "auto-live-admin",
                "直播自动定时",
                "0 30 15 * * *",
                Some(mcloud_core::MANAGED_BY_LIVE_AUTO),
            ),
        )
        .await;
        save_schedule_for(
            app,
            admin,
            schedule(
                "auto-auth-refresh-admin",
                "凭据自动刷新",
                "0 0 3 * * *",
                Some(mcloud_core::MANAGED_BY_AUTH_REFRESH),
            ),
        )
        .await;
        save_schedule_for(
            app,
            other,
            schedule("sch-other", "别人的定时", "0 9 * * *", None),
        )
        .await;
    }

    /// 把"数据目录下那几个文件"的原始字节读出来 —— 独立于 store 内存的那份读数。
    fn snapshot_files(dir: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        [
            "push-rules.json",
            "push-panel.json",
            "exchange-subscriptions.json",
            "auth-refresh.json",
            "pipelines.json",
            "schedules.json",
            "settings.json",
        ]
        .into_iter()
        .map(|name| (name.to_string(), std::fs::read(dir.join(name)).ok()))
        .collect()
    }

    /// 门：**往返逐字段相同**。
    ///
    /// 造一份有内容的配置 ⇒ export 到临时文件 ⇒ 清空（规则删光、定时删光、编排删光、
    /// 面板回默认、订阅清空、钟点改掉）⇒ import ⇒ 与导出前**逐字段**比，
    /// 并且把 `managed_by` / `enabled` / cron 表达式这几格单独点一遍名。
    #[tokio::test]
    async fn export_then_import_round_trips_every_field() {
        let (app, dir) = test_app("config-roundtrip");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_user(&app, "other", "other", 200);
        seed_rich(&app, &admin, &other).await;
        let store = &app.handle.store;

        let before = collect(&app, &admin).await.expect("导出");
        let json = to_json(&before).expect("序列化");
        let path = dir.join("export.json");
        write_private(&path, json.as_bytes()).expect("写导出文件");

        // 导出文件里有 token ⇒ 落盘就该是 0600（不是"先写后 chmod"）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "导出文件落盘就是 0600");
        }

        // 清空：每一类都改成"与导出前不一样"
        for r in store.push_rules(&admin).await.unwrap() {
            store.remove_push_rule(&admin, &r.id).await.unwrap();
        }
        for s in mine_schedules(&app, &admin).await.unwrap() {
            store.remove_schedule(&s.id).await.unwrap();
        }
        for p in mine_pipelines(&app, &admin).await.unwrap() {
            store.remove_pipeline(&p.id).await.unwrap();
        }
        store
            .save_push_panel(&admin, &PanelBindings::default())
            .await
            .unwrap();
        store
            .save_exchange_subscriptions(&admin, &[])
            .await
            .unwrap();
        store.save_auth_refresh_at(&admin, "01:23").await.unwrap();
        assert!(store.push_rules(&admin).await.unwrap().is_empty());
        assert!(store.schedules_for(&admin).await.unwrap().is_empty());

        import_cmd(&app, &path, false).await.expect("导入");

        // 逐字段比：导出时刻每次不同，抹掉它再比
        let mut want = before.clone();
        let mut got = collect(&app, &admin).await.expect("再导出");
        want.exported_at = None;
        got.exported_at = None;
        assert_eq!(got, want, "往返之后每一类逐字段相同");

        // 关键格点名（失败信息一眼看出是哪一类漂了）
        let rules = store.push_rules(&admin).await.unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id.as_str(), "r1", "按 id 稳定排序");
        assert!(rules[0].enabled, "enabled 原样搬过来");
        assert_eq!(
            rules[0].config.pushplus.as_ref().unwrap().token,
            "1234",
            "渠道凭据原样搬过来（它就是配置的一部分）"
        );
        assert_eq!(rules[0].owner, Some(admin.clone()), "归属重指到本机管理员");

        let schs = store.schedules_for(&admin).await.unwrap();
        assert_eq!(schs.len(), 4, "四条定时一条不少");
        let auto_live = schs
            .iter()
            .find(|s| s.is_managed_by(mcloud_core::MANAGED_BY_LIVE_AUTO))
            .expect("直播自动定时那条要在");
        assert_eq!(auto_live.expr, "0 30 15 * * *", "cron 表达式逐字相同");
        assert_eq!(auto_live.run_count, 3);
        let auto_auth = schs
            .iter()
            .find(|s| s.is_managed_by(mcloud_core::MANAGED_BY_AUTH_REFRESH))
            .expect("凭据自动刷新那条要在");
        assert!(auto_auth.enabled, "enabled 原样搬过来");

        assert_eq!(
            store.auth_refresh_at(&admin).await.unwrap().as_deref(),
            Some("22:30")
        );
        assert_eq!(store.exchange_subscriptions(&admin).await.unwrap().len(), 2);
        assert_eq!(
            store.push_panel(&admin).await.unwrap().login_new.rules,
            vec![PushRuleId::new("r1")]
        );
        assert_eq!(mine_pipelines(&app, &admin).await.unwrap().len(), 1);

        // 别人那一份一个字节都没动
        assert_eq!(store.push_rules(&other).await.unwrap().len(), 1);
        assert!(store
            .schedules()
            .await
            .unwrap()
            .iter()
            .any(|s| s.id.as_str() == "sch-other"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**只动该动的** —— 一份只有 `push-rules` 的文件导进去，
    /// 定时 / 编排 / 订阅 / 钟点 / 面板**一条都不许少**。
    #[tokio::test]
    async fn importing_one_category_leaves_the_others_untouched() {
        let (app, dir) = test_app("config-one-category");
        let admin = UserId::new("admin");
        seed_user(&app, "admin", "admin", 100);
        seed_rich(&app, &admin, &UserId::new("other")).await;
        let store = &app.handle.store;

        let schs_before = store.schedules_for(&admin).await.unwrap();
        let pipes_before = mine_pipelines(&app, &admin).await.unwrap();
        let subs_before = store.exchange_subscriptions(&admin).await.unwrap();
        let panel_before = store.push_panel(&admin).await.unwrap();

        // 只有规则那一类，而且只有一条（证明"这一类被整类替换了"）
        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "owner": { "id": "admin", "name": "admin" },
            "push-rules": [rule("r9", "换掉的一条")],
        });
        let path = dir.join("only-rules.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        import_cmd(&app, &path, false).await.expect("导入");

        let rules = store.push_rules(&admin).await.unwrap();
        assert_eq!(rules.len(), 1, "文件里那一类整类替换");
        assert_eq!(rules[0].id.as_str(), "r9");
        assert_eq!(rules[0].owner, Some(admin.clone()));

        assert_eq!(
            store.schedules_for(&admin).await.unwrap(),
            schs_before,
            "定时一条没少（文件里没有这一类）"
        );
        assert_eq!(
            mine_pipelines(&app, &admin).await.unwrap(),
            pipes_before,
            "编排一条没少"
        );
        assert_eq!(
            store.exchange_subscriptions(&admin).await.unwrap(),
            subs_before,
            "兑换订阅一条没少"
        );
        assert_eq!(
            store.push_panel(&admin).await.unwrap(),
            panel_before,
            "面板绑定没动"
        );
        assert_eq!(
            store.auth_refresh_at(&admin).await.unwrap().as_deref(),
            Some("22:30"),
            "刷新钟点没动"
        );
        // 实例级设置也没动（连 --with-instance 都没给）
        assert_eq!(
            store.settings().await.unwrap(),
            AppSettings::default(),
            "实例级设置没动"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**坏文件要报得清、且一个字节都不改**。
    ///
    /// 七种：路径不存在 / 不是合法 JSON / 没有版本键 / 版本不认识 / 形状不对 /
    /// 钟点不是 `HH:MM` / cron 表达式不合法。每种都断言：错误里带文件名、
    /// 退非 0（这里就是 `Err`），并且**落库读数**（原始文件字节 + store 里的条数）
    /// 与导入前完全一致 —— 也就是"要么全改、要么不改"。
    #[tokio::test]
    async fn bad_files_fail_loud_and_change_nothing() {
        let (app, dir) = test_app("config-bad");
        let admin = UserId::new("admin");
        seed_user(&app, "admin", "admin", 100);
        seed_rich(&app, &admin, &UserId::new("other")).await;
        let store = &app.handle.store;

        let files_before = snapshot_files(&dir);
        let rules_before = store.push_rules(&admin).await.unwrap();
        let schs_before = store.schedules_for(&admin).await.unwrap();

        let write = |name: &str, body: &str| -> PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            path
        };

        let cases: Vec<(PathBuf, &str)> = vec![
            (dir.join("nope.json"), "读不到导入文件"),
            (write("broken.json", "{ 这不是 JSON"), "不是合法 JSON"),
            (
                write("no-version.json", r#"{"hello":1}"#),
                "没有 `mcloudConfig` 字段",
            ),
            (
                write("bad-version.json", r#"{"mcloudConfig":99,"push-rules":[]}"#),
                "只认 1",
            ),
            (
                write(
                    "bad-shape.json",
                    r#"{"mcloudConfig":1,"push-rules":{"不是":"数组"}}"#,
                ),
                "形状不对",
            ),
            (
                write(
                    "bad-clock.json",
                    r#"{"mcloudConfig":1,"auth-refresh":"半夜"}"#,
                ),
                "HH:MM",
            ),
            (
                write(
                    "bad-cron.json",
                    r#"{"mcloudConfig":1,"schedules":[{"id":"s1","name":"x","task":"notice","expr":"不是 cron"}]}"#,
                ),
                "cron 表达式不合法",
            ),
        ];

        for (path, needle) in cases {
            let err = import_cmd(&app, &path, false)
                .await
                .expect_err("坏文件必须报错")
                .to_string();
            let name = path.display().to_string();
            assert!(
                err.contains(needle),
                "{name} 的报错里要有「{needle}」：{err}"
            );
            assert!(err.contains(&name), "报错里要说清是哪个文件：{err}");
        }

        // 落库读数独立核一遍：文件字节与 store 内容都还是导入前那份
        assert_eq!(
            snapshot_files(&dir),
            files_before,
            "盘上的文件一个字节都没改"
        );
        assert_eq!(store.push_rules(&admin).await.unwrap(), rules_before);
        assert_eq!(store.schedules_for(&admin).await.unwrap(), schs_before);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：跨机器导入 —— 文件里的 `owner` 与**由它推出来的 id** 都要落到本机管理员。
    ///
    /// 两台机器的用户 id 是各自生成的 uuid，照抄过去那份数据谁也看不见；
    /// 而 `daily-<owner>` / `auto-live-<owner>` / `auto-auth-refresh-<owner>`
    /// 这几条 id 是**从 owner 推出来的**，不重指的话系统下次重算会**再建一条**。
    #[tokio::test]
    async fn importing_from_another_machine_repoints_owner_and_derived_ids() {
        let (app, dir) = test_app("config-foreign");
        let admin = UserId::new("admin");
        seed_user(&app, "admin", "admin", 100);
        let store = &app.handle.store;

        let mut foreign_rule = rule("r1", "别的机器上的规则");
        foreign_rule.owner = Some(UserId::new("foreign"));
        let mut foreign_pipe = pipeline("daily-foreign", "每日一键（默认）");
        foreign_pipe.owner = Some(UserId::new("foreign"));
        let mut foreign_daily = schedule("daily-foreign", "每日一键（默认）", "0 8 * * *", None);
        foreign_daily.owner = Some(UserId::new("foreign"));
        foreign_daily.body = serde_json::json!({ "pipelineId": "daily-foreign" });
        let mut foreign_live = schedule(
            "auto-live-foreign",
            "直播自动定时",
            "0 30 15 * * *",
            Some(mcloud_core::MANAGED_BY_LIVE_AUTO),
        );
        foreign_live.owner = Some(UserId::new("foreign"));

        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "owner": { "id": "foreign", "name": "foreign" },
            "push-rules": [foreign_rule],
            "pipelines": [foreign_pipe],
            "schedules": [foreign_daily, foreign_live],
        });
        let path = dir.join("foreign.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        import_cmd(&app, &path, false).await.expect("导入");

        let rules = store.push_rules(&admin).await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].owner, Some(admin.clone()), "归属重指到本机管理员");

        let pipes = mine_pipelines(&app, &admin).await.unwrap();
        assert_eq!(pipes.len(), 1);
        assert_eq!(pipes[0].id.as_str(), "daily-admin", "推出来的 id 重指");
        assert_eq!(pipes[0].owner, Some(admin.clone()));

        let schs = mine_schedules(&app, &admin).await.unwrap();
        let ids: Vec<&str> = schs.iter().map(|s| s.id.as_str()).collect();
        assert!(
            ids.contains(&"daily-admin"),
            "默认定时那条 id 重指：{ids:?}"
        );
        assert!(
            ids.contains(&"auto-live-admin"),
            "直播自动定时那条 id 重指：{ids:?}"
        );
        let daily = schs
            .iter()
            .find(|s| s.id.as_str() == "daily-admin")
            .expect("默认定时要在");
        assert_eq!(
            daily.body.get("pipelineId").and_then(|v| v.as_str()),
            Some("daily-admin"),
            "定时指向的编排 id 一起重指（否则指向本机不存在的编排）"
        );
        assert!(schs
            .iter()
            .all(|s| s.owner == Some(admin.clone()) && !s.id.as_str().contains("foreign")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**id 撞到别人**就当场停下（CLI 只动本机管理员那一个用户）。
    #[tokio::test]
    async fn an_id_owned_by_someone_else_stops_the_import() {
        let (app, dir) = test_app("config-conflict");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_user(&app, "other", "other", 200);
        save_schedule_for(
            &app,
            &other,
            schedule("sch-shared", "别人的", "0 9 * * *", None),
        )
        .await;

        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "schedules": [schedule("sch-shared", "抢过来的", "0 7 * * *", None)],
        });
        let path = dir.join("conflict.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        let files_before = snapshot_files(&dir);
        let err = import_cmd(&app, &path, false)
            .await
            .expect_err("撞到别人名下的 id 必须报错")
            .to_string();
        assert!(err.contains("另一个用户"), "报错要说清为什么：{err}");
        assert_eq!(snapshot_files(&dir), files_before, "一个字节都没动");

        // 别人那条还是他原来的样子
        let sch = app
            .handle
            .store
            .schedule(&ScheduleId::new("sch-shared"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sch.name, "别人的");
        assert_eq!(sch.expr, "0 9 * * *");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：CLI **不提供"选谁"** —— `--user` / `--owner` / `-u` 一律报错。
    ///
    /// 判据只有一处（`cli/push.rs::reject_user_selection`）；这里钉的是
    /// "`config` 这一族也接上了它"，尤其**参数在中间**时不许被静默忽略。
    #[test]
    fn cli_never_selects_a_user() {
        for args in [
            argv(&["export", "--user", "bob"]),
            argv(&["export", "-u", "bob"]),
            argv(&["export", "--owner", "bob", "--out", "/tmp/x.json"]),
            argv(&["import", "/tmp/x.json", "--user", "bob"]),
        ] {
            let err = parse(&args).expect_err("选了人就得报错").to_string();
            assert!(
                err.contains("只作用于本机"),
                "报错要说清 CLI 只作用于本机那个用户：{err}"
            );
        }
    }

    /// 门：帮助与用法 —— `help` / `--help` / `-h` 打帮助，**不带子命令也要给人话**。
    #[test]
    fn help_flags_and_bare_config_are_human() {
        for flag in ["help", "--help", "-h"] {
            assert_eq!(parse(&argv(&[flag])).unwrap(), Cmd::Help, "{flag} 是看帮助");
        }
        // 不带子命令 = 打帮助（不是 panic，也不是"悄悄做点什么"）
        assert_eq!(parse(&[]).unwrap(), Cmd::Help);
        // 未知子命令要报错，且把用法一起打出来
        let err = parse(&argv(&["nope"]))
            .expect_err("未知子命令要报错")
            .to_string();
        assert!(err.contains("未知的 config 子命令"), "{err}");
        assert!(
            err.contains("mcloud config export"),
            "报错要带上用法：{err}"
        );

        // 两种命令的正常形状
        assert_eq!(
            parse(&argv(&["export", "--out", "/tmp/x.json"])).unwrap(),
            Cmd::Export {
                out: Some(PathBuf::from("/tmp/x.json"))
            }
        );
        assert_eq!(
            parse(&argv(&["export"])).unwrap(),
            Cmd::Export { out: None }
        );
        assert_eq!(
            parse(&argv(&["import", "/tmp/x.json"])).unwrap(),
            Cmd::Import {
                path: PathBuf::from("/tmp/x.json"),
                with_instance: false
            }
        );
        assert_eq!(
            parse(&argv(&["import", "/tmp/x.json", "--with-instance"])).unwrap(),
            Cmd::Import {
                path: PathBuf::from("/tmp/x.json"),
                with_instance: true
            }
        );

        // 参数缺失 / 不认识 / 多给：都是**用法错**，不静默
        for args in [
            argv(&["import"]),
            argv(&["export", "--out"]),
            argv(&["export", "--nope"]),
            argv(&["import", "a.json", "b.json"]),
            argv(&["import", "--nope", "a.json"]),
        ] {
            assert!(parse(&args).is_err(), "{args:?} 该报用法错");
        }

        // 帮助文本要说清这两条命令与实例级那个开关
        assert!(USAGE.contains("mcloud config export"));
        assert!(USAGE.contains("mcloud config import"));
        assert!(USAGE.contains("--with-instance"));
        assert!(USAGE.contains("--user"));
    }

    /// 门：`run` 的三条路都不 panic —— 不带子命令 / `help` 退 0，未知子命令报错。
    #[tokio::test]
    async fn run_without_a_subcommand_prints_usage_and_succeeds() {
        let (app, dir) = test_app("config-run");
        seed_user(&app, "admin", "admin", 100);

        assert!(run(&app, &[]).await.is_ok(), "不带子命令 = 打帮助、退 0");
        assert!(run(&app, &argv(&["help"])).await.is_ok());
        assert!(run(&app, &argv(&["nope"])).await.is_err());

        // 导出到 stdout 那条路（省略 --out）不该报错
        assert!(run(&app, &argv(&["export"])).await.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**没有用户**时给的是人话（不是 panic、不是"默默成功"）。
    #[tokio::test]
    async fn export_without_any_user_says_what_to_do() {
        let (app, dir) = test_app("config-no-user");
        let err = run(&app, &argv(&["export"]))
            .await
            .expect_err("没有用户时要报错")
            .to_string();
        assert!(err.contains("mcloud user add"), "要给出下一步：{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
