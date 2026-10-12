//! `mcloud config export` / `mcloud config import <文件>` —— 把配置（含**账号凭据**）
//! 从一台机器搬到另一台，搬完**不用重新登录**。
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
//! # 导出/导入哪几类、文件什么形状
//!
//! **不在这个文件里定义** —— 唯一那一份在 [`crate::config_doc`] ✓
//! （CLI 与 WebUI 的 `GET /api/config/export` 共用它，两份必然漂移 ✗）。
//! 那里列了完整的表：进文件的**十一类**（含**账号与凭据**、**设备指纹**）、
//! 不进的那几类（`sessions.json` / `history.json` 与 `logs/` / `kouling-pool.json` /
//! `users.json`）、以及敏感标记 `containsCredentials` ✓。
//!
//! ⚠ **凭据是带的**（2026-10-12 用户口径：「**凭据也需要导出啊，不导出我怎么使用**」✓）：
//! 导出的意义就是搬到另一台机器**直接用**，不带上凭据就得把每个账号重新登录一遍，
//! 这个功能等于废一半 ✓。上一轮那份"凭据不进文件"的口径**已被推翻** ✓。
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
//! ⚠ `pc-device`（本机设备指纹）**不**走这个开关：它跟的是"凭据要能用"那条线
//! （`--with-instance` 管的是整台机器的策略格），所以导入时**一律应用** ✓。
//!
//! # 导入的语义：**按 id 合并**（2026-10-12 用户裁定「不要整体吧」）
//!
//! * **列表型类别一律按 id 合并**（账号 / 推送规则 / 面板绑定 / 兑换订阅 / 定时 / 编排）：
//!   * 文件里有、本机也有同一个 id ⇒ **覆盖那一条**（逐字段以文件为准）；
//!   * 文件里有、本机没有 ⇒ **新增**；
//!   * 本机有、文件里没有 ⇒ **原样留着**（不删）。
//!
//!   上一版是"文件里有哪一类就把那一类整类替换"（本机多出来的会被删掉），用户裁定不要
//!   那个。这条对**所有**列表型类别是同一份口径，不是"一类一套" —— 读数也共用
//!   [`merge_counts`] 一处。
//! * 文件里**没有**的类别一个字节都不动（键缺席 = 这一类不许动，`Option` 的语义兜着）。
//! * **先备份**：动手之前把要改的文件备成 `<名字>.bak-<时间戳>`，中途失败自动回滚。
//! * **先全解析、后落盘**：读文件、认版本、逐类校验、id 归属冲突检查全过了，才动第一笔。
//! * **归属重指**：文件里的 `owner` 一律改成**本机那个用户**（见 [`repoint_id`]）——
//!   两台机器的用户 id 是各自生成的 uuid，照抄过去那份数据谁也看不见。
//!   账号那一类走的是**同一套**：`accounts.json` 里每个账号的 `owner` 也重指到本机用户
//!   （用的是同一道冲突闸，见 [`check_id_conflicts`]）。
//! * **归属闸不放松**：文件里那条 id 若在本机属于**别人**，导入当场停下并回滚，一条都不写
//!   —— 合并语义说的是"本机多出来的留着"，不是"把别人的抢过来"。
//! * **按账号 id 存的那两张表**（`state.json` / `smid.json`）本来就是按键合并（它们装着
//!   **本机所有账号**的运行期状态与设备指纹，别的用户的也在里面）—— 这一版起与别的类别
//!   同一条口径了：文件里给了哪个账号，就写哪个账号那一格，其余一个字节都不动。
//! * **实例级那几格是标量**（`--with-instance` 管的那份 `settings.json`）⇒ 覆盖语义照旧、
//!   不必"合并"：标量没有"本机多出来的一条"这回事。默认不动，加了开关才整格覆盖。
//! * 导完打印**逐类条数**（含"本机另有 N 条原样保留"），并提醒**重启服务**才生效
//!   （`JsonStore` 启动时全量读进内存）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Context;
use mcloud_app::App;
use mcloud_core::push::BehaviorKey;
use mcloud_core::{
    auth_refresh, Account, AccountId, ExchangeSubscription, PanelBindings, Pipeline, PipelineId,
    Schedule, ScheduleId, UserId,
};

use crate::cli::push::{cli_owner, reject_user_selection};
use crate::config_doc::{self, ExportDoc, FORMAT_KEY, FORMAT_VERSION};

/// 导出文件里"面板绑定"那一格的键名。
///
/// 它必须与 `config_doc::ExportDoc` 的 `#[serde(rename = "push-panel")]` 是同一个字面量
/// （`serde` 的 rename 只吃字面量，没法引常量）—— 改一处要改两处，单测里那条
/// "文件里只给一格面板绑定"的用例就是钉这件事的。
const KEY_PUSH_PANEL: &str = "push-panel";

pub const USAGE: &str = "\
用法：
  mcloud config export [--out <路径>]       导出本机用户的配置
                                            （省略 --out 就打 stdout，可重定向到文件）
  mcloud config import <路径> [--with-instance]
                                            把那份 JSON **按 id 合并**到本机用户
                                            （同 id 覆盖、本机多出来的保留）
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
  accounts                账号列表与**每个账号的凭据**（token / 到期时间 / 设备号 / 平台 / 用户域）
  account-state           账号运行期状态（按账号 id）
  account-smid            账号的**设备指纹**（按账号 id；凭据是跟设备绑的，所以要带上）
  pc-device               本机 PC 设备指纹（实例级，一枚）

⚠ 这份文件**等于你的登录态**（顶层 `containsCredentials: true` 标着它）：
里面有账号凭据、设备指纹与推送渠道的 token ⇒ 别随手发给别人，也别提交进版本库。
导出的意义就是搬到另一台机器**直接用** —— 导入之后不用重新登录就能跑任务。
导入是**按 id 合并**：文件里有的那一类，同一个 id 就把本机那一条**覆盖**掉（逐字段以文件
为准）、本机没有的就**新增**，而**本机多出来的原样留着**（不删）；文件里**没有**的类别
一个字节都不动。账号 / 推送规则 / 面板绑定 / 兑换订阅 / 定时 / 编排一律是这条口径
（`account-state` / `account-smid` 本来就按账号 id 合并）。
文件里那条 id 若在本机属于**别人**，导入会当场停下并回滚，一条都不写（CLI 不抢别人的东西）。
`--with-instance` 是**另一回事**：实例级那几格（口令下限 / 会话时长 / 历史与日志上限 /
实例级刷新钟点）是**标量**，没有「本机多出来的一条」这回事 ⇒ 加了它才整格覆盖，不加就一格
都不动。列表类那一半**不需要**这个开关，合并是默认行为。
导入前会先把要改的文件备份成 `<名字>.bak-<时间戳>`，中途失败自动回滚到导入前那份。
导入完**必须重启服务**才生效（`JsonStore` 启动时把那些文件全量读进内存）：
    scripts/dev-reload.sh --no-build";

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

// ── export ─────────────────────────────────────────────────────────────

/// 那句"这份文件等于你的登录态"的提醒（**只有一处措辞** —— 写盘与打 stdout 两条路共用）。
const CREDENTIAL_WARNING: &str =
    "注意：这份文件等于你的登录态（含账号凭据、设备指纹与推送渠道的 token）—— \
     别随手发给别人，也别提交进版本库。";

async fn export(app: &App, out: Option<PathBuf>) -> anyhow::Result<()> {
    let owner = cli_owner(app).await?;
    // ⚠ 文档形状与"导哪几类"只有一份实现（`crate::config_doc`），WebUI 那条路由调的是
    // 同一个函数 ⇒ 界面导出的文件与这里导出的**逐字节同形** ✓。
    let doc = config_doc::collect(&app.handle, &owner).await?;
    let json = config_doc::to_json(&doc)?;
    match out {
        // 省略 `--out` ⇒ **只有** JSON 进 stdout（`mcloud config export > f.json` 能用）。
        //
        // ⚠ 提醒走 **stderr**：stdout 要留给 JSON（重定向进文件时一个字都不能多 ✗），
        // 而"这文件别乱发"这句必须在**终端上**看得见 ✓（写进 stdout 会被一起重定向走，
        // 用户把文件发出去时反而没看到那句警告）。
        None => {
            print!("{json}");
            eprintln!("{CREDENTIAL_WARNING}");
        }
        Some(path) => {
            write_private(&path, json.as_bytes())?;
            println!(
                "已导出本机用户「{owner}」的配置到 {}：{}",
                path.display(),
                config_doc::describe(&doc).join(" / ")
            );
            println!("{CREDENTIAL_WARNING}");
            println!("      拷到目标机器后：mcloud config import <文件>（导入后不用重新登录）");
        }
    }
    Ok(())
}

/// 写导出文件：**落盘就是 0600**，并且先写临时文件再改名。
///
/// 两条都照 `JsonStore::write_atomic` 的理由来：文件里有账号凭据与推送渠道的 token，
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
    /// 面板绑定：文件里给了**几格**（那四格是固定的，本机不会多出第五格 ⇒ 报"给了几格"）。
    push_panel: Option<usize>,
    exchange_subscriptions: Option<Counts>,
    auth_refresh: Option<String>,
    pipelines: Option<Counts>,
    schedules: Option<Counts>,
    instance: Option<bool>,
    /// 账号那一类（**带凭据**）。
    accounts: Option<Counts>,
    /// 账号运行期状态：按账号 id 合并，所以只报"写了几份"。
    account_state: Option<usize>,
    /// 账号设备指纹：同上。
    account_smid: Option<usize>,
    /// 本机设备指纹是不是换了。
    pc_device: Option<bool>,
}

/// 列表型那一类的条数读数（**按 id 合并**那一套的唯一一份读数）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Counts {
    /// 文件里给了几条。
    total: usize,
    /// 本机原来没有 ⇒ 新增。
    added: usize,
    /// 本机原来有同一个 id ⇒ 覆盖。
    updated: usize,
    /// 本机有、文件里没有 ⇒ 原样留着（**上一版会把它们删掉**，这个数就是这次改动的意义）。
    kept: usize,
}

impl Counts {
    fn line(&self, unit: &str) -> String {
        format!(
            "文件里 {} {unit}（新增 {}、覆盖 {}）；本机另有 {} {unit}原样保留",
            self.total, self.added, self.updated, self.kept
        )
    }
}

/// 列表类**按 id 合并**的读数（**只有这一份** —— 每一类都调它，免得一类一套口径）。
///
/// `local` 是本机**管理员名下**现有的那些 id（`mine_*` 的读数），`from_file` 是文件里给的、
/// **重指之后**的 id。返回的 `kept` 就是"本机有、文件里没有 ⇒ 原样留着"的条数。
fn merge_counts(local: &HashSet<String>, from_file: impl IntoIterator<Item = String>) -> Counts {
    let file: HashSet<String> = from_file.into_iter().collect();
    let updated = local.intersection(&file).count();
    Counts {
        total: file.len(),
        added: file.len() - updated,
        updated,
        kept: local.len() - updated,
    }
}

/// 兑换订阅**按奖品 id 合并**：同 id 用文件里那条覆盖，本机多出来的**留在原位**。
///
/// 顺序只影响"订阅时刻完全相同"那种边角：落盘时 `save_exchange_subscriptions` 会再按
/// （订阅时刻, 奖品 id）排一遍，读回来与导出那份同序（那是它自己的口径，不归这里管）。
fn merge_subscriptions(
    local: Vec<ExchangeSubscription>,
    from_file: &[ExchangeSubscription],
) -> (Vec<ExchangeSubscription>, Counts) {
    let mut merged = local;
    let local_ids: HashSet<i64> = merged.iter().map(|s| s.prize_id).collect();
    let mut updated = 0;
    for s in from_file {
        match merged.iter_mut().find(|e| e.prize_id == s.prize_id) {
            Some(slot) => {
                *slot = s.clone();
                updated += 1;
            }
            None => merged.push(s.clone()),
        }
    }
    let counts = Counts {
        total: from_file.len(),
        added: from_file.len() - updated,
        updated,
        // 文件里的 `prize_id` 在校验那一步已经保证互不相同 ⇒ 命中的必是本机不同的那几条
        kept: local_ids.len() - updated,
    };
    (merged, counts)
}

/// 面板绑定**按行为格合并**：文件里写了哪几格就覆盖哪几格，其余保留本机。
///
/// 为什么不直接用反序列化出来的那份 `PanelBindings`：那四格都带 `serde(default = ...)`，
/// 文件里**漏写**的格子会被悄悄填成默认值 —— 那正是"把本机多出来的删掉"的另一种形状
/// （比如文件只写了 `loginNew`，本机那格 `perfSlow` 就会被清回默认）。
/// 导出的文件里四格必然都在，所以这一条对"导出再导入"与整格覆盖结果相同；
/// 差别只出现在**手写的**部分文件上，而那时"文件里没给的保留本机"才是与别的类别一致的口径。
///
/// 这里按**文件里的键名**盖（`PanelBindings` 上有 `rename_all = "camelCase"` ⇒
/// `loginNew` / `loginFailed` / `perfSlow` / `perfFailStreak`，不是 Rust 那边的字段名）。
///
/// 返回文件里真正给了几格（报告用）。
async fn merge_panel(
    app: &App,
    owner: &UserId,
    from_file: Option<&serde_json::Value>,
) -> anyhow::Result<usize> {
    let store = &app.handle.store;
    let mut merged = serde_json::to_value(store.push_panel(owner).await?)?;
    let mut n = 0;
    if let Some(serde_json::Value::Object(from_file)) = from_file {
        let base = merged
            .as_object_mut()
            .expect("`PanelBindings` 序列化出来就是一个 JSON 对象");
        for (key, value) in from_file {
            // 只认已知那几格：多出来的键（文件被改坏、或来自别的版本）不写进本机那一份
            if base.contains_key(key) {
                base.insert(key.clone(), value.clone());
                n += 1;
            }
        }
    }
    let panel: PanelBindings = serde_json::from_value(merged)
        .context("合并之后的面板绑定读不回来（文件里那一格形状不对？）")?;
    store.save_push_panel(owner, &panel).await?;
    Ok(n)
}

impl Report {
    fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(c) = &self.push_rules {
            out.push(format!("  推送规则              {}", c.line("条")));
        }
        if let Some(n) = self.push_panel {
            out.push(format!(
                "  面板绑定              按行为合并（文件里给了 {n}/{} 格，其余保留本机）",
                BehaviorKey::ALL.len()
            ));
        }
        if let Some(c) = &self.exchange_subscriptions {
            out.push(format!("  兑换订阅              {}", c.line("条")));
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
        if let Some(c) = &self.accounts {
            out.push(format!("  账号（含凭据）        {}", c.line("个")));
        }
        if let Some(n) = self.account_state {
            out.push(format!(
                "  账号状态              写入 {n} 份（按账号 id 合并）"
            ));
        }
        if let Some(n) = self.account_smid {
            out.push(format!(
                "  账号设备指纹          写入 {n} 份（按账号 id 合并）"
            ));
        }
        if let Some(replaced) = self.pc_device {
            out.push(format!(
                "  本机设备指纹          {}",
                if replaced {
                    "已换成文件里那一枚"
                } else {
                    "没动"
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
    let (doc, raw) = read_doc(path)?;
    validate_doc(&doc).map_err(|e| anyhow::anyhow!("{} 里有不合法的配置：{e}", path.display()))?;
    check_id_conflicts(app, &owner, &doc)
        .await
        .map_err(|e| anyhow::anyhow!("{} 与本机对不上：{e}", path.display()))?;

    // ② 备份要改的那几个文件（失败回滚的依据）。
    let names = touched_files(&doc, with_instance);
    let stamp = mcloud_core::beijing_format(&*app.handle.clock, "%Y%m%d-%H%M%S");
    let backups = backup_files(&app.handle.data_dir, &names, &stamp)?;

    // ③ 落盘。中途任何一步失败 ⇒ 把备份盖回去（并说清回滚有没有做干净）。
    let report = match apply(app, &owner, &doc, &raw, with_instance).await {
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
///
/// 返回**解析好的文档**与**文件里那份原始 JSON**两份：原始那一份只给 `push-panel` 用 ——
/// 那四格在类型里都带 `serde(default = ...)`，文件里漏写的格子会**悄悄变成默认值**，
/// 而"按 id 合并"要的是"文件里没给的那一格保留本机" ⇒ 只有拿原始 JSON 逐键盖上去才做得到
/// （见 [`merge_panel`]）。别的类别不需要它：它们按 id 合并，读出来的列表就是全部信息。
fn read_doc(path: &Path) -> anyhow::Result<(ExportDoc, serde_json::Value)> {
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

    let doc = serde_json::from_value(value.clone()).with_context(|| {
        format!(
            "{} 的形状不对（按 `mcloud config export` 导出的那份逐类核对一遍）",
            path.display()
        )
    })?;
    Ok((doc, value))
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
    // ── 凭据那一组（2026-10-12 起进文件 ✓）────────────────────────────
    if let Some(accounts) = &doc.accounts {
        ensure_unique("accounts", accounts.iter().map(|a| a.id.as_str()))?;
        for a in accounts {
            if a.id.as_str().trim().is_empty() {
                anyhow::bail!(
                    "`accounts` 里有一个账号没有 id（昵称是「{}」）",
                    a.nickname.clone().unwrap_or_default()
                );
            }
            // 凭据本身不做"合不合法"的判断（那是上游的事，形状也可能变 ✓）——
            // 只拦**空的**：空的凭据搬过去等于没搬，而它看起来又像"搬成功了" ✗。
            if a.auth.raw.trim().is_empty() && a.auth.token.trim().is_empty() {
                anyhow::bail!(
                    "`accounts` 里的账号 {} 没有凭据（`auth.raw` 与 `auth.token` 都是空的）\
                     —— 搬过去也用不了，先停在这里",
                    a.id
                );
            }
        }
    }
    if let Some(smid) = &doc.account_smid {
        for (id, s) in smid {
            if id.trim().is_empty() {
                anyhow::bail!("`account-smid` 里有一个空账号 id");
            }
            // 指纹那一格是"JSON 文本"，读不出来就是文件被改坏了 —— 早拦，
            // 免得写进盘之后上游按"库里那串读不出来"报错（那时已经改了一半 ✗）。
            if let Some(fp) = &s.fingerprint_json {
                if serde_json::from_str::<serde_json::Value>(fp).is_err() {
                    anyhow::bail!("`account-smid` 里账号 {id} 的 `fingerprintJson` 不是合法 JSON");
                }
            }
        }
    }
    if let Some(dev) = &doc.pc_device {
        if !pc_device_shape_ok(dev) {
            anyhow::bail!(
                "`pc-device` 形状不合法（应形如 32 位**大写** HEX 加 `{}` 后缀，实得「{dev}」）",
                mcloud_core::PC_DEVICE_SUFFIX
            );
        }
    }
    Ok(())
}

/// `pc-device` 的形状判据（**与存储层同一套**）。
///
/// `JsonStore::parse_pc_device_id` 是私有的、在另一个 crate 里 ⇒ 这里只能再写一份；
/// 判据本身很短，而**漏了它的代价**是"写进去一个半截 id ⇒ 下一次读判成没有 ⇒
/// 每轮生成一枚新的"（那正是 `pc_device.txt` 要防的事 ✓），所以宁可在这里早拦一次 ✓。
fn pc_device_shape_ok(raw: &str) -> bool {
    let trimmed = raw.trim();
    let Some(body) = trimmed.strip_suffix(mcloud_core::PC_DEVICE_SUFFIX) else {
        return false;
    };
    body.len() == 32 && body.chars().all(|c| c.is_ascii_hexdigit()) && body == body.to_uppercase()
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

/// 定时 / 编排 / **账号**的 id 在本机已经属于**别人**时，当场停下。
///
/// 定时与编排的 id 是**全局唯一**的（`save_schedule` / `save_pipeline` 按 id upsert），
/// 账号 id 也是全局唯一的（它是从上游那个账号推出来的 ✓，两台机器上同一个云盘账号
/// 就是同一个 id ✓）—— 照 upsert 走下去等于把别人那条**抢过来**，
/// 而 CLI 的立场是"只动本机管理员那一个用户" ✓。
///
/// ⚠ 账号这一格与定时/编排走的是**同一道闸**（不是另起一套 ✓）：判据都是
/// "本机这条 id 的 `owner` 存在、且不是本机用户" ⇒ 停下 ✓。区别只有一处 ——
/// 账号 id **不重指**（它不是从 owner 推出来的，它就是上游那个账号的标识 ✓）。
async fn check_id_conflicts(app: &App, owner: &UserId, doc: &ExportDoc) -> anyhow::Result<()> {
    if let Some(accounts) = &doc.accounts {
        let all = app.handle.store.accounts().await?;
        for a in accounts {
            if let Some(hit) = all
                .iter()
                .find(|x| x.id == a.id && x.owner.as_ref().is_some_and(|o| o != owner))
            {
                anyhow::bail!(
                    "账号 {} 在本机属于**另一个用户**（{}）—— 导入会把它抢过来，先停在这里。\
                     要么让那个用户在 WebUI 上自己处理，要么先从文件里删掉这一个账号。",
                    a.id,
                    hit.owner
                        .as_ref()
                        .map(UserId::to_string)
                        .unwrap_or_default()
                );
            }
        }
    }
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
    // ── 凭据那一组（2026-10-12 起进文件 ✓）────────────────────────────
    // 账号列表与凭据：按账号 id 合并（与规则 / 定时同一条口径：同 id 覆盖、本机多出来的留着）。
    if doc.accounts.is_some() {
        names.push("accounts.json");
    }
    // 按账号 id 合并的那两张：备份它们是**必须**的（改的是整份文件），
    // 合并只决定"写哪几格"，不决定"要不要整份重写" —— 见 `apply` 里那段说明。
    if doc.account_state.is_some() {
        names.push("state.json");
    }
    if doc.account_smid.is_some() {
        names.push("smid.json");
    }
    // 机器级那一枚：跟的是"凭据要能用"那条线，不走 --with-instance ✓（见模块头）。
    if doc.pc_device.is_some() {
        names.push("pc_device.txt");
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
///
/// `raw` 是导入文件那份**原始 JSON**（只给 [`merge_panel`] 用，见 `read_doc` 的说明）。
async fn apply(
    app: &App,
    owner: &UserId,
    doc: &ExportDoc,
    raw: &serde_json::Value,
    with_instance: bool,
) -> anyhow::Result<Report> {
    let store = &app.handle.store;
    let src = doc.source_owner();
    let mut report = Report::default();

    // 规则：**按 id 合并** —— 文件里给的逐条 upsert（归属重指到本机用户），
    // 本机多出来的那几条**一个都不删**（上一版在这里 remove，用户裁定不要）。
    if let Some(rules) = &doc.push_rules {
        let existing = store.push_rules(owner).await?;
        let local: HashSet<String> = existing.iter().map(|e| e.id.to_string()).collect();
        for r in rules {
            let mut rule = r.clone();
            rule.owner = Some(owner.clone());
            store.save_push_rule(owner, &rule).await?;
        }
        report.push_rules = Some(merge_counts(&local, rules.iter().map(|r| r.id.to_string())));
    }

    // 面板绑定：**按行为格合并**（见 [`merge_panel`]）。
    if doc.push_panel.is_some() {
        let n = merge_panel(app, owner, raw.get(KEY_PUSH_PANEL)).await?;
        report.push_panel = Some(n);
    }

    // 兑换订阅：**按奖品 id 合并**（同 id 覆盖、本机多出来的留着）。
    if let Some(items) = &doc.exchange_subscriptions {
        let (merged, counts) =
            merge_subscriptions(store.exchange_subscriptions(owner).await?, items);
        store.save_exchange_subscriptions(owner, &merged).await?;
        report.exchange_subscriptions = Some(counts);
    }

    if let Some(at) = &doc.auth_refresh {
        store.save_auth_refresh_at(owner, at).await?;
        report.auth_refresh = Some(at.clone());
    }

    // 编排放在定时**之前**：默认编排那条定时指向 `daily-<owner>`，
    // 先把编排落到本机，后面那条定时才有东西可指。
    // 合并口径同上：文件里给的逐条 upsert，本机多出来的留着。
    if let Some(pipes) = &doc.pipelines {
        let existing = mine_pipelines(app, owner).await?;
        let local: HashSet<String> = existing.iter().map(|e| e.id.to_string()).collect();
        let mut from_file = Vec::with_capacity(pipes.len());
        for p in pipes {
            let mut pipe = p.clone();
            pipe.owner = Some(owner.clone());
            pipe.id = PipelineId::new(repoint_id(p.id.as_str(), src.as_ref(), owner));
            from_file.push(pipe.id.to_string());
            store.save_pipeline(&pipe).await?;
        }
        report.pipelines = Some(merge_counts(&local, from_file));
    }

    if let Some(schs) = &doc.schedules {
        let existing = mine_schedules(app, owner).await?;
        let local: HashSet<String> = existing.iter().map(|e| e.id.to_string()).collect();
        let mut from_file = Vec::with_capacity(schs.len());
        for s in schs {
            let mut sch = s.clone();
            sch.owner = Some(owner.clone());
            sch.id = ScheduleId::new(repoint_id(s.id.as_str(), src.as_ref(), owner));
            repoint_body(&mut sch.body, src.as_ref(), owner);
            from_file.push(sch.id.to_string());
            store.save_schedule(&sch).await?;
        }
        report.schedules = Some(merge_counts(&local, from_file));
    }

    // ── 凭据那一组（2026-10-12 起进文件 ✓）────────────────────────────
    //
    // 账号：**按账号 id 合并**（与规则 / 定时同一条口径）—— 文件里给的逐条 upsert
    // （**归属重指到本机用户**），本机**名下**多出来的那些**原样留着**，不删。
    //
    // 归属重指在这里是**必须**的：两台机器的 `UserId` 是各自生成的 uuid，
    // 照抄过去那份数据谁也看不见（`visible_to` 只认相等）—— 与定时 / 编排走的是**同一套**
    // 策略（账号 id 本身**不**重指：它是上游那个账号的标识，两台机器上同一个云盘账号
    // 就是同一个 id）。
    //
    // 合并的读数只按"**显式归属**"算（`mine_accounts`，与 `mine_schedules` 同款）：
    // 无主条目（`owner = None`）不算"这个用户名下的" ⇒ 它既不进"保留"那个数，
    // 也不受"文件里没有就留着"这条保护 —— 它与文件里同 id 的那条相遇时照旧被覆盖、
    // 并认到本机管理员名下（这是上一版逐条 upsert 就有的行为，这次没改）。
    if let Some(accounts) = &doc.accounts {
        let existing = mine_accounts(app, owner).await?;
        let local: HashSet<String> = existing.iter().map(|e| e.id.to_string()).collect();
        for a in accounts {
            let mut acc = a.clone();
            acc.owner = Some(owner.clone());
            store.save_account(&acc).await?;
        }
        report.accounts = Some(merge_counts(
            &local,
            accounts.iter().map(|a| a.id.to_string()),
        ));
    }

    // 下面这两张**按账号 id 合并**（文件里给了哪个账号，就写哪个账号那一格；
    // 其余一个字节都不动）—— 从这一版起，它俩与别的类别是**同一条口径**了
    // （上一版只有它俩是合并、其余是整类替换，那正是这次要抹平的不一致）。
    if let Some(state) = &doc.account_state {
        let mut n = 0;
        for (id, value) in state {
            store
                .save_account_state(&AccountId::new(id.clone()), value)
                .await?;
            n += 1;
        }
        report.account_state = Some(n);
    }

    if let Some(smid) = &doc.account_smid {
        let mut n = 0;
        for (id, s) in smid {
            let id = AccountId::new(id.clone());
            // 指纹与 `deviceId` 是**两列独立事实**（`SmidStore` 的文件头 ✓），
            // 而"换长相"那条窄写会**顺手把旧 deviceId 清空**（新的长相配旧的 id
            // 才是自相矛盾 ✓）⇒ 顺序必须是**先写指纹、再把文件里那个 id 写回去** ✓。
            if let Some(fp) = &s.fingerprint_json {
                store.set_smid_fingerprint(&id, fp).await?;
            }
            if let Some(dev) = &s.device_id {
                store.set_smid_device_id(&id, dev).await?;
            }
            n += 1;
        }
        report.account_smid = Some(n);
    }

    // 机器级那一枚：**不走 `--with-instance`**（它跟的是"凭据要能用"那条线 ✓，
    // 见模块头那段）—— 文件里有它就应用 ✓。本来就是同一枚时不写（省一次盘 IO ✓）。
    if let Some(dev) = &doc.pc_device {
        let already = app.handle.login.pc_device_id_opt().await?.as_deref() == Some(dev.as_str());
        if !already {
            app.handle.login.set_pc_device_id(dev).await?;
        }
        report.pc_device = Some(!already);
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
/// 可见 —— 而合并的读数只该算"这个用户名下的"那几条（无主条目不是）。
/// 收窄到显式归属，才与"只动本机管理员那一个用户"的立场一致。
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

/// 本机**管理员名下**的账号（判据与 [`mine_schedules`] 同款：只看**显式归属** ✓）。
async fn mine_accounts(app: &App, owner: &UserId) -> anyhow::Result<Vec<Account>> {
    Ok(app
        .handle
        .store
        .accounts()
        .await?
        .into_iter()
        .filter(|a| a.owner.as_ref() == Some(owner))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use mcloud_app::AppBuilder;
    use mcloud_core::push::{PushConfig, PushRuleId, TokenConfig};
    use mcloud_core::{
        Account, AccountSettings, AppSettings, AuthInfo, DeviceId, ExchangeSubscription,
        HttpClient, HttpRequest, HttpResponse, PanelBindings, PasswordHash, PcPlatform, PushRule,
        User,
    };

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

    /// 一个账号（`owner` 留空，由调用方填 —— 与 [`seed_rich`] 同一条理由）。
    fn account(id: &str, nickname: Option<&str>, raw: &str, token: &str, expire: i64) -> Account {
        Account {
            id: AccountId::new(id),
            nickname: nickname.map(str::to_string),
            auth: AuthInfo {
                raw: raw.into(),
                platform: "android".into(),
                phone: id.into(),
                token: token.into(),
                expire,
            },
            device_id: DeviceId("B-dev".into()),
            pc_platform: None,
            user_domain_id: None,
            owner: None,
            enabled: true,
            settings: AccountSettings::default(),
        }
    }

    /// 造一份"**已经登录过**"的凭据组（2026-10-12 起这一组要进导出文件 ✓）。
    ///
    /// 两件事都要有，缺一条这类用例就测不到真东西：
    /// * **账号本身的凭据**（`auth.raw` / `token` / `expire` / `phone` + 设备号 / 平台 / 用户域）；
    /// * **与设备绑的那一半**：`state.json`（运行期状态）与 `smid.json`（设备指纹）✓，
    ///   外加机器级那一枚 `pc_device.txt` ✓。
    ///
    /// 别人名下也放一个账号：往返与跨机器那几条要能证明"别人的一个字节都没进来" ✓。
    async fn seed_credentials(app: &App, admin: &UserId, other: &UserId) {
        let store = &app.handle.store;

        let mut a1 = account(
            "13800000000",
            Some("主力号"),
            "raw-1",
            "tok-1|seg|seg|1790000000000",
            1_790_000_000_000,
        );
        a1.owner = Some(admin.clone());
        a1.device_id = DeviceId("B-admin-1".into());
        a1.pc_platform = Some(PcPlatform::MacOs);
        a1.user_domain_id = Some("1234567890123456789".into());
        a1.settings.backup_wait_secs = 7;
        store.save_account(&a1).await.unwrap();
        store
            .save_account_state(
                &a1.id,
                &serde_json::json!({ "cur_month_backup": "202610", "device_id": "B-admin-1" }),
            )
            .await
            .unwrap();
        // ⚠ 顺序照 `apply` 里的口径：先写指纹（它会清掉 deviceId ✓），再把 deviceId 写回去 ✓
        store
            .set_smid_fingerprint(&a1.id, r#"{"screen":"1920x1080"}"#)
            .await
            .unwrap();
        store.set_smid_device_id(&a1.id, "B-admin-1").await.unwrap();

        let mut a2 = account("13900000001", None, "raw-2", "tok-2|seg|seg|0", 0);
        a2.owner = Some(admin.clone());
        a2.pc_platform = Some(PcPlatform::Windows);
        store.save_account(&a2).await.unwrap();
        store
            .set_smid_fingerprint(&a2.id, r#"{"screen":"2560x1440"}"#)
            .await
            .unwrap();

        let mut ao = account(
            "13700000002",
            Some("别人的号"),
            "raw-o",
            "tok-o|seg|seg|0",
            0,
        );
        ao.owner = Some(other.clone());
        store.save_account(&ao).await.unwrap();
        store
            .save_account_state(&ao.id, &serde_json::json!({ "谁的": "别人" }))
            .await
            .unwrap();

        // 机器级那一枚（实例级 ✓）：形状必须是 32 位大写 hex + `-ENDIN`（存储层会校验 ✓）
        app.handle
            .login
            .set_pc_device_id("62AA2BDA29F99744536E142A83CA5B03-ENDIN")
            .await
            .unwrap();
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
            "accounts.json",
            "state.json",
            "smid.json",
            "pc_device.txt",
        ]
        .into_iter()
        .map(|name| (name.to_string(), std::fs::read(dir.join(name)).ok()))
        .collect()
    }

    /// 门：**往返逐字段相同**（2026-10-12 起**含凭据那一组** ✓）。
    ///
    /// 造一份有内容的配置 ⇒ export 到临时文件 ⇒ 清空（规则删光、定时删光、编排删光、
    /// 面板回默认、订阅清空、钟点改掉、**账号删光、设备指纹换掉**）⇒ import
    /// ⇒ 与导出前**逐字段**比，并且把 `managed_by` / `enabled` / cron 表达式 /
    /// `auth.token` / `auth.expire` / `pc_platform` 这几格单独点一遍名。
    #[tokio::test]
    async fn export_then_import_round_trips_every_field() {
        let (app, dir) = test_app("config-roundtrip");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_user(&app, "other", "other", 200);
        seed_rich(&app, &admin, &other).await;
        seed_credentials(&app, &admin, &other).await;
        let store = &app.handle.store;

        let before = config_doc::collect(&app.handle, &admin)
            .await
            .expect("导出");
        let json = config_doc::to_json(&before).expect("序列化");
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
        // 凭据那一组也清空：账号删光（`remove_account` 会顺手带走它的 state 与 smid ✓）、
        // 机器级那一枚换成**另一枚**（证明导入真的把文件里那枚写回去了 ✓）。
        for a in mine_accounts(&app, &admin).await.unwrap() {
            store.remove_account(&a.id).await.unwrap();
        }
        app.handle
            .login
            .set_pc_device_id("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA-ENDIN")
            .await
            .unwrap();
        assert!(store.push_rules(&admin).await.unwrap().is_empty());
        assert!(store.schedules_for(&admin).await.unwrap().is_empty());
        assert!(mine_accounts(&app, &admin).await.unwrap().is_empty());

        import_cmd(&app, &path, false).await.expect("导入");

        // 逐字段比：导出时刻每次不同，抹掉它再比
        let mut want = before.clone();
        let mut got = config_doc::collect(&app.handle, &admin)
            .await
            .expect("再导出");
        want.exported_at = None;
        got.exported_at = None;
        assert_eq!(got, want, "往返之后每一类逐字段相同（含账号与凭据）");

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

        // ── 凭据那几格点名（失败信息一眼看出是哪一格漂了 ✓）────────────
        let accs = mine_accounts(&app, &admin).await.unwrap();
        assert_eq!(accs.len(), 2, "两个账号都回来了");
        let a1 = accs
            .iter()
            .find(|a| a.id.as_str() == "13800000000")
            .expect("主力号要在");
        assert_eq!(a1.auth.raw, "raw-1", "原始凭据串原样搬过来");
        assert_eq!(a1.auth.token, "tok-1|seg|seg|1790000000000");
        assert_eq!(a1.auth.expire, 1_790_000_000_000, "到期时间逐字相同");
        assert_eq!(a1.auth.phone, "13800000000");
        assert_eq!(a1.device_id.as_str(), "B-admin-1", "设备号搬过来了");
        assert_eq!(a1.pc_platform, Some(PcPlatform::MacOs), "平台搬过来了");
        assert_eq!(a1.user_domain_id.as_deref(), Some("1234567890123456789"));
        assert_eq!(a1.nickname.as_deref(), Some("主力号"));
        assert_eq!(a1.settings.backup_wait_secs, 7);
        assert_eq!(a1.owner, Some(admin.clone()), "归属重指到本机管理员");
        assert_eq!(
            store
                .get_smid_state(&a1.id)
                .await
                .unwrap()
                .unwrap()
                .fingerprint_json
                .as_deref(),
            Some(r#"{"screen":"1920x1080"}"#),
            "设备指纹搬过来了（凭据是跟设备绑的）"
        );
        assert_eq!(
            store
                .get_smid_state(&a1.id)
                .await
                .unwrap()
                .unwrap()
                .device_id
                .as_deref(),
            Some("B-admin-1"),
            "上报换回的 deviceId 也回来了"
        );
        assert_eq!(
            store.account_state(&a1.id).await.unwrap()["cur_month_backup"],
            serde_json::json!("202610"),
            "账号运行期状态搬过来了"
        );
        assert_eq!(
            app.handle
                .login
                .pc_device_id_opt()
                .await
                .unwrap()
                .as_deref(),
            Some("62AA2BDA29F99744536E142A83CA5B03-ENDIN"),
            "机器级那一枚被写回文件里那个值"
        );

        // 别人那一份一个字节都没动
        assert_eq!(store.push_rules(&other).await.unwrap().len(), 1);
        assert!(store
            .schedules()
            .await
            .unwrap()
            .iter()
            .any(|s| s.id.as_str() == "sch-other"));
        let others = store.accounts_for(&other).await.unwrap();
        assert_eq!(others.len(), 1, "别人的账号没被带走");
        assert_eq!(others[0].auth.raw, "raw-o", "别人的凭据原样");
        assert_eq!(
            store.account_state(&others[0].id).await.unwrap()["谁的"],
            serde_json::json!("别人"),
            "别人的运行期状态没被覆盖（按账号 id 合并那条口径）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 断言：这一组 `data/` 文件的**字节**里，只有 `changed` 那几个变了，其余逐字节相同。
    ///
    /// 判据落在**原始字节**上（不是 store 的内存读数）："文件里没有的类别一个字节都不动"
    /// 这条只有看盘才算数 —— 内存读数相同、盘上被重写一遍（例如顺序变了、多了个空数组）
    /// 同样是把没被要求改的东西改了。
    fn assert_only_these_files_changed(
        before: &[(String, Option<Vec<u8>>)],
        after: &[(String, Option<Vec<u8>>)],
        changed: &[&str],
    ) {
        for (name, was) in before {
            if changed.contains(&name.as_str()) {
                continue;
            }
            let now = after
                .iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("{name} 不见了"));
            assert_eq!(
                &now.1, was,
                "{name} 不该被这次导入动到（该变的只有 {changed:?}）"
            );
        }
    }

    /// 门：**只动该动的**，而且那一类是**按 id 合并**（2026-10-12 用户裁定「不要整体吧」）。
    ///
    /// 两份钉子合在一条里（同一份夹具）：
    /// * **合并**：文件里给两条规则 —— 一条与本机 `r1` 同 id（内容不同）、一条本机没有；
    ///   本机另有 `r2`（本机独有）⇒ 导入后 `r1` 被文件**逐字段**覆盖、`r2` **仍在**、
    ///   文件里那条新的新增。上一版这里是"整类替换"，`r1`/`r2` 会被整类换掉。
    /// * **只动该动的**：一份只有 `push-rules` 的文件导进去，定时 / 编排 / 订阅 / 钟点 /
    ///   面板 / **账号与凭据**一个都不许少，它们对应的**文件字节**也一个都不许变。
    #[tokio::test]
    async fn importing_one_category_leaves_the_others_untouched() {
        let (app, dir) = test_app("config-one-category");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_rich(&app, &admin, &other).await;
        seed_credentials(&app, &admin, &other).await;
        let store = &app.handle.store;

        let schs_before = store.schedules_for(&admin).await.unwrap();
        let pipes_before = mine_pipelines(&app, &admin).await.unwrap();
        let subs_before = store.exchange_subscriptions(&admin).await.unwrap();
        let panel_before = store.push_panel(&admin).await.unwrap();
        let accounts_before = mine_accounts(&app, &admin).await.unwrap();
        let pc_before = app.handle.login.pc_device_id_opt().await.unwrap();
        let rules_before = store.push_rules(&admin).await.unwrap();
        assert_eq!(
            rules_before.len(),
            2,
            "夹具：本机管理员名下有 r1 / r2 两条规则"
        );
        let files_before = snapshot_files(&dir);

        // 文件里只有规则那一类，两条：`r1` 与本机同 id（内容逐格不同 ⇒ 覆盖）、
        // `r9` 本机没有（⇒ 新增）。本机独有的 `r2` 不许被删。
        let mut r1_from_file = rule("r1", "文件里那一条");
        r1_from_file.enabled = false;
        r1_from_file.config.pushplus = Some(TokenConfig {
            token: "9999".into(),
            extra: Default::default(),
        });
        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "owner": { "id": "admin", "name": "admin" },
            "push-rules": [r1_from_file, rule("r9", "新增的一条")],
        });
        let path = dir.join("only-rules.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        import_cmd(&app, &path, false).await.expect("导入");

        // ── 合并：同 id 覆盖（逐字段）、本机独有的留着、文件里新的新增 ──────────
        let rules = store.push_rules(&admin).await.unwrap();
        let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["r1", "r2", "r9"],
            "同 id 的原地覆盖、本机独有的留着、文件里新的追加"
        );
        let r1 = rules.iter().find(|r| r.id.as_str() == "r1").unwrap();
        assert_eq!(r1.name, "文件里那一条", "同 id 那条被文件覆盖（名字）");
        assert_eq!(
            r1.config.pushplus.as_ref().unwrap().token,
            "9999",
            "逐字段：渠道 token 也以文件为准"
        );
        assert!(!r1.enabled, "逐字段：enabled 也以文件为准");
        assert_eq!(r1.owner, Some(admin.clone()), "归属重指到本机管理员");
        let r2 = rules.iter().find(|r| r.id.as_str() == "r2").unwrap();
        let r2_before = rules_before.iter().find(|r| r.id.as_str() == "r2").unwrap();
        assert_eq!(
            r2, r2_before,
            "本机独有那条逐字段原样 —— 这就是这次改动的核心（上一版会把它删掉）"
        );
        assert_eq!(
            rules.iter().find(|r| r.id.as_str() == "r9").unwrap().owner,
            Some(admin.clone())
        );
        // 别人名下那条规则一个字节都没动
        assert_eq!(store.push_rules(&other).await.unwrap().len(), 1);

        // ── 只动该动的：别的类别连**盘上字节**都没变 ─────────────────────────
        assert_only_these_files_changed(&files_before, &snapshot_files(&dir), &["push-rules.json"]);

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
        // 凭据那一组：文件里**没有** `accounts` ⇒ 账号一个都不许动
        //（这一条尤其要紧：写歪了就会把本机账号全删掉）
        assert_eq!(
            mine_accounts(&app, &admin).await.unwrap(),
            accounts_before,
            "文件里没有 `accounts` ⇒ 本机账号一个都没动"
        );
        assert_eq!(
            app.handle.login.pc_device_id_opt().await.unwrap(),
            pc_before,
            "文件里没有 `pc-device` ⇒ 本机那一枚没动"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**账号那一类也是按 id 合并** —— 本机管理员名下 3 个账号，文件里只给 1 个
    /// （与其中一个同 id）⇒ 导入后仍是 3 个：那 1 个被文件逐字段覆盖，另外 2 个原样
    /// （尤其**凭据字段**：`raw` / `token` / `expire` 一个都不许被动）。
    ///
    /// 上一版这里是"整类替换"：那 2 个账号（连凭据）会被删掉 —— 用户裁定不要那个。
    #[tokio::test]
    async fn importing_accounts_merges_by_id_and_keeps_the_local_ones() {
        let (app, dir) = test_app("config-account-merge");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_user(&app, "other", "other", 200);
        let store = &app.handle.store;

        // 本机管理员名下三个账号，各有各的凭据与那几格
        let mut a1 = account(
            "13800000000",
            Some("主力号"),
            "raw-1",
            "tok-1|seg|seg|1",
            111,
        );
        a1.owner = Some(admin.clone());
        a1.device_id = DeviceId("B-1".into());
        a1.settings.backup_wait_secs = 7;
        store.save_account(&a1).await.unwrap();
        let mut a2 = account(
            "13900000001",
            Some("备用号"),
            "raw-2",
            "tok-2|seg|seg|2",
            222,
        );
        a2.owner = Some(admin.clone());
        store.save_account(&a2).await.unwrap();
        let mut a3 = account("13900000003", None, "raw-3", "tok-3|seg|seg|3", 333);
        a3.owner = Some(admin.clone());
        store.save_account(&a3).await.unwrap();
        // 别人名下也放一个：合并只按**本机管理员名下**算，别人的一个字节都不许动
        let mut ao = account("13700000002", Some("别人的号"), "raw-o", "tok-o", 0);
        ao.owner = Some(other.clone());
        store.save_account(&ao).await.unwrap();

        let before = mine_accounts(&app, &admin).await.unwrap();
        assert_eq!(before.len(), 3, "夹具：本机管理员名下 3 个账号");
        let files_before = snapshot_files(&dir);

        // 文件里只有 1 个账号，与 `13800000000` 同 id：逐字段都换了
        let mut from_file = account(
            "13800000000",
            Some("文件里的号"),
            "raw-new",
            "tok-new|seg|seg|9",
            999,
        );
        from_file.device_id = DeviceId("B-file".into());
        from_file.pc_platform = Some(PcPlatform::Windows);
        from_file.user_domain_id = Some("9999999999999999999".into());
        from_file.settings.backup_wait_secs = 42;
        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "owner": { "id": "admin", "name": "admin" },
            "accounts": [from_file],
        });
        let path = dir.join("accounts-merge.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        import_cmd(&app, &path, false).await.expect("导入");

        let after = mine_accounts(&app, &admin).await.unwrap();
        assert_eq!(after.len(), 3, "本机另外两个账号（连凭据）不许被删");

        let a1_after = after
            .iter()
            .find(|a| a.id.as_str() == "13800000000")
            .unwrap();
        assert_eq!(a1_after.auth.raw, "raw-new", "同 id 那个：凭据以文件为准");
        assert_eq!(a1_after.auth.token, "tok-new|seg|seg|9");
        assert_eq!(a1_after.auth.expire, 999, "到期时间逐字以文件为准");
        assert_eq!(a1_after.auth.phone, "13800000000");
        assert_eq!(a1_after.nickname.as_deref(), Some("文件里的号"));
        assert_eq!(a1_after.device_id.as_str(), "B-file");
        assert_eq!(a1_after.pc_platform, Some(PcPlatform::Windows));
        assert_eq!(
            a1_after.user_domain_id.as_deref(),
            Some("9999999999999999999")
        );
        assert_eq!(a1_after.settings.backup_wait_secs, 42);
        assert_eq!(a1_after.owner, Some(admin.clone()), "归属重指到本机管理员");

        for id in ["13900000001", "13900000003"] {
            let was = before.iter().find(|a| a.id.as_str() == id).unwrap();
            let now = after.iter().find(|a| a.id.as_str() == id).unwrap();
            assert_eq!(
                now, was,
                "{id} 逐字段原样（凭据 raw / token / expire 尤其不许动）"
            );
        }
        assert_eq!(
            store.accounts_for(&other).await.unwrap().len(),
            1,
            "别人的账号没动"
        );
        // 文件里只有 `accounts` ⇒ 盘上只有 accounts.json 变
        assert_only_these_files_changed(&files_before, &snapshot_files(&dir), &["accounts.json"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**其余列表型类别同样是"按 id 合并"**（别一类一套口径）。
    ///
    /// 一次导入里把编排 / 定时 / 兑换订阅 / 面板绑定都摆上：
    /// * 编排与定时：同 id 的覆盖、本机独有的留着、文件里新的新增；
    /// * 兑换订阅：按**奖品 id** 合并（同 id 覆盖、本机多出来的留着）；
    /// * 面板绑定：按**行为格**合并 —— 文件里只给了 `login_new` ⇒ 本机那格 `perf_slow`
    ///   保留（要是拿反序列化出来那份整格写下去，它会回落成默认值，等于把本机的清掉）。
    #[tokio::test]
    async fn the_other_list_categories_merge_by_id_too() {
        let (app, dir) = test_app("config-merge-others");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_user(&app, "other", "other", 200);
        seed_rich(&app, &admin, &other).await;
        let store = &app.handle.store;

        // 本机再加一条**独有**的编排，并把面板那两格都绑上规则
        let mut p_local = pipeline("p-local", "本机独有的编排");
        p_local.owner = Some(admin.clone());
        store.save_pipeline(&p_local).await.unwrap();
        let mut panel = store.push_panel(&admin).await.unwrap();
        panel.login_new.rules = vec![PushRuleId::new("r1")];
        panel.perf_slow.rules = vec![PushRuleId::new("r2")];
        store.save_push_panel(&admin, &panel).await.unwrap();

        let pipes_before = mine_pipelines(&app, &admin).await.unwrap();
        let schs_before = mine_schedules(&app, &admin).await.unwrap();
        assert_eq!(pipes_before.len(), 2, "夹具：本机两条编排");
        assert_eq!(schs_before.len(), 4, "夹具：本机四条定时");
        assert_eq!(store.exchange_subscriptions(&admin).await.unwrap().len(), 2);
        let files_before = snapshot_files(&dir);

        // 文件里：编排两条（`daily-admin` 同 id、`p-file` 新）、定时两条（`daily-admin`
        // 同 id、`sch-file` 新）、订阅两条（奖品 11 同 id、33 新）、面板一格（`login_new`）
        let mut pipe_over = pipeline("daily-admin", "文件里的默认编排");
        pipe_over.enabled = false;
        let mut sch_over = schedule("daily-admin", "文件里的默认定时", "0 7 * * *", None);
        sch_over.run_count = 99;
        let panel_from_file = {
            // 那四格的键名是 camelCase（`PanelBindings` 上有 `rename_all = "camelCase"`）——
            // 覆盖用的正是**文件里那些键名**，所以这里也照它写。
            let mut v = serde_json::to_value(PanelBindings::default()).unwrap();
            let obj = v.as_object_mut().unwrap();
            obj.retain(|k, _| k == "loginNew");
            obj.get_mut("loginNew").unwrap()["rules"] = serde_json::json!(["r-file"]);
            v
        };
        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "owner": { "id": "admin", "name": "admin" },
            "pipelines": [pipe_over, pipeline("p-file", "文件里新加的编排")],
            "schedules": [
                sch_over,
                schedule("sch-file", "文件里新加的定时", "0 6 * * *", None),
            ],
            "exchange-subscriptions": [
                sub(11, "文件里的奖品甲", 3_000),
                sub(33, "文件里新加的奖品", 4_000),
            ],
            "push-panel": panel_from_file,
        });
        let path = dir.join("merge-others.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        import_cmd(&app, &path, false).await.expect("导入");

        // ── 编排 ──────────────────────────────────────────────────────────
        let pipes = mine_pipelines(&app, &admin).await.unwrap();
        let ids: Vec<&str> = pipes.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "本机两条都在 + 文件里新的一条：{ids:?}");
        assert!(ids.contains(&"p-local"), "本机独有的编排留着：{ids:?}");
        assert!(ids.contains(&"p-file"), "文件里新的编排新增：{ids:?}");
        let over = pipes
            .iter()
            .find(|p| p.id.as_str() == "daily-admin")
            .unwrap();
        assert_eq!(over.name, "文件里的默认编排", "同 id 的编排被文件覆盖");
        assert!(!over.enabled, "逐字段：enabled 也以文件为准");
        assert_eq!(over.owner, Some(admin.clone()));

        // ── 定时 ──────────────────────────────────────────────────────────
        let schs = mine_schedules(&app, &admin).await.unwrap();
        let sids: Vec<&str> = schs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(sids.len(), 5, "本机四条都在 + 文件里新的一条：{sids:?}");
        for keep in ["sch-1", "auto-live-admin", "auto-auth-refresh-admin"] {
            assert!(
                sids.contains(&keep),
                "本机独有的定时留着（{keep}）：{sids:?}"
            );
        }
        assert!(sids.contains(&"sch-file"), "文件里新的定时新增：{sids:?}");
        let daily = schs
            .iter()
            .find(|s| s.id.as_str() == "daily-admin")
            .unwrap();
        assert_eq!(daily.name, "文件里的默认定时", "同 id 的定时被文件覆盖");
        assert_eq!(daily.expr, "0 7 * * *", "cron 表达式逐字以文件为准");
        assert_eq!(daily.run_count, 99, "逐字段：run_count 也以文件为准");
        assert_eq!(daily.owner, Some(admin.clone()));

        // ── 兑换订阅（按奖品 id）──────────────────────────────────────────
        let subs = store.exchange_subscriptions(&admin).await.unwrap();
        assert_eq!(subs.len(), 3, "11 覆盖、22 留着、33 新增");
        let s11 = subs.iter().find(|s| s.prize_id == 11).unwrap();
        assert_eq!(s11.name, "文件里的奖品甲", "同 id 的订阅以文件为准");
        assert_eq!(s11.subscribed_at_ms, 3_000);
        let s22 = subs.iter().find(|s| s.prize_id == 22).unwrap();
        assert_eq!(s22.name, "奖品乙", "本机多出来的那条订阅留着");
        assert_eq!(s22.subscribed_at_ms, 2_000);
        assert!(subs.iter().any(|s| s.prize_id == 33));

        // ── 面板绑定（按行为格）───────────────────────────────────────────
        let panel = store.push_panel(&admin).await.unwrap();
        assert_eq!(
            panel.login_new.rules,
            vec![PushRuleId::new("r-file")],
            "文件里给了的那一格被覆盖"
        );
        assert_eq!(
            panel.perf_slow.rules,
            vec![PushRuleId::new("r2")],
            "文件里没给的那一格保留本机（不是回落成默认）"
        );

        // ── 只动该动的：盘上只有这四类变 ──────────────────────────────────
        assert_only_these_files_changed(
            &files_before,
            &snapshot_files(&dir),
            &[
                "pipelines.json",
                "schedules.json",
                "exchange-subscriptions.json",
                "push-panel.json",
            ],
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：**坏文件要报得清、且一个字节都不改**。
    ///
    /// 九种：路径不存在 / 不是合法 JSON / 没有版本键 / 版本不认识 / 形状不对 /
    /// 钟点不是 `HH:MM` / cron 表达式不合法 / `accounts` 里凭据是空的 /
    /// `pc-device` 形状不对。每种都断言：错误里带文件名、
    /// 退非 0（这里就是 `Err`），并且**落库读数**（原始文件字节 + store 里的条数）
    /// 与导入前完全一致 —— 也就是"要么全改、要么不改"。
    #[tokio::test]
    async fn bad_files_fail_loud_and_change_nothing() {
        let (app, dir) = test_app("config-bad");
        let admin = UserId::new("admin");
        seed_user(&app, "admin", "admin", 100);
        seed_rich(&app, &admin, &UserId::new("other")).await;
        seed_credentials(&app, &admin, &UserId::new("other")).await;
        let store = &app.handle.store;

        let files_before = snapshot_files(&dir);
        let rules_before = store.push_rules(&admin).await.unwrap();
        let schs_before = store.schedules_for(&admin).await.unwrap();
        let accounts_before = mine_accounts(&app, &admin).await.unwrap();

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
            // 凭据那一组：空的凭据搬过去等于没搬，而它看起来又像"搬成功了" ⇒ 当场拦 ✓
            (
                write(
                    "bad-empty-auth.json",
                    r#"{"mcloudConfig":1,"accounts":[{"id":"13800000000","auth":{"raw":"","platform":"android","phone":"13800000000","token":"","expire":0},"device_id":"D"}]}"#,
                ),
                "没有凭据",
            ),
            // 半截的设备指纹：写进去会被判成"没有" ⇒ 每轮生成一枚新的（正是它要防的事）
            (
                write(
                    "bad-pc-device.json",
                    r#"{"mcloudConfig":1,"pc-device":"半截的指纹"}"#,
                ),
                "pc-device",
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
        assert_eq!(
            mine_accounts(&app, &admin).await.unwrap(),
            accounts_before,
            "账号（含凭据）一个字节都没动"
        );

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

    /// 门：**账号**撞到别人名下也当场停下（与定时/编排走的是**同一道闸** ✓）。
    ///
    /// 账号 id 是从上游那个账号推出来的、两台机器上同一个云盘账号就是同一个 id
    /// ⇒ 照 upsert 走下去会把别人的账号**抢过来**（连凭据一起换掉 ✗）。
    #[tokio::test]
    async fn an_account_owned_by_someone_else_stops_the_import() {
        let (app, dir) = test_app("config-account-conflict");
        let other = UserId::new("other");
        seed_user(&app, "admin", "admin", 100);
        seed_user(&app, "other", "other", 200);

        let mut theirs = account("13800000000", Some("别人的号"), "raw-theirs", "tok", 0);
        theirs.owner = Some(other.clone());
        app.handle.store.save_account(&theirs).await.unwrap();

        let doc = serde_json::json!({
            "mcloudConfig": 1,
            "accounts": [account("13800000000", Some("抢过来的"), "raw-mine", "tok2", 0)],
        });
        let path = dir.join("conflict-account.json");
        write_private(
            &path,
            serde_json::to_string_pretty(&doc).unwrap().as_bytes(),
        )
        .unwrap();

        let files_before = snapshot_files(&dir);
        let err = import_cmd(&app, &path, false)
            .await
            .expect_err("撞到别人名下的账号必须报错")
            .to_string();
        assert!(err.contains("另一个用户"), "报错要说清为什么：{err}");
        assert!(err.contains("13800000000"), "报错要说清是哪个账号：{err}");
        assert_eq!(snapshot_files(&dir), files_before, "一个字节都没动");

        // 别人那个账号连凭据都没被换掉
        let still = app
            .handle
            .store
            .account(&AccountId::new("13800000000"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(still.auth.raw, "raw-theirs", "别人的凭据不许被换");
        assert_eq!(still.owner, Some(other));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门（2026-10-12 用户口径「凭据也需要导出啊」）：**换一台机器导入之后不用重新登录**。
    ///
    /// 这是那件事的自动化版本：造一个"已登录"的实例 ⇒ 导出 ⇒ 换**另一个数据目录**
    /// （= 另一台机器，管理员 id 也换一个）导入 ⇒ 逐字段核 ——
    /// 凭据、设备指纹、运行期状态、机器级那一枚都要在，且归属重指到新机器那个用户。
    #[tokio::test]
    async fn credentials_survive_a_move_to_another_machine() {
        // ── 源机器：一个"已登录"的实例 ─────────────────────────────────
        let (src, src_dir) = test_app("config-move-src");
        let src_admin = UserId::new("src-admin");
        seed_user(&src, "src-admin", "src-admin", 100);
        seed_credentials(&src, &src_admin, &UserId::new("src-other")).await;

        let src_doc = config_doc::collect(&src.handle, &src_admin)
            .await
            .expect("源机器导出");
        assert_eq!(
            src_doc.accounts.as_ref().map(Vec::len),
            Some(2),
            "源机器上管理员名下两个账号（别人那个不该在里面）"
        );
        let json = config_doc::to_json(&src_doc).expect("序列化");
        let path = src_dir.join("export.json");
        write_private(&path, json.as_bytes()).expect("写导出文件");

        // ── 目标机器：**另一个数据目录**、另一个管理员 id、一开始空空如也 ──
        let (dst, dst_dir) = test_app("config-move-dst");
        let dst_admin = UserId::new("dst-admin");
        seed_user(&dst, "dst-admin", "dst-admin", 100);
        assert!(mine_accounts(&dst, &dst_admin).await.unwrap().is_empty());
        assert!(dst.handle.login.pc_device_id_opt().await.unwrap().is_none());

        import_cmd(&dst, &path, false).await.expect("跨机器导入");

        let dst_doc = config_doc::collect(&dst.handle, &dst_admin)
            .await
            .expect("目标机器再导出");

        // ① 账号**逐字段**相同 —— 只把 `owner` 换成目标机器那个用户
        let mut want = src_doc.accounts.clone().unwrap();
        let have = dst_doc.accounts.clone().unwrap();
        assert_eq!(have.len(), 2, "两个账号都搬过去了");
        for a in &mut want {
            a.owner = Some(dst_admin.clone());
        }
        assert_eq!(have, want, "账号逐字段（含 auth 里的凭据）相同");

        // ② 关键凭据格点名（失败信息一眼看出是哪一格漂了）
        let a1 = have
            .iter()
            .find(|a| a.id.as_str() == "13800000000")
            .expect("主力号要在");
        assert_eq!(a1.auth.raw, "raw-1", "原始凭据串搬过来了");
        assert_eq!(a1.auth.token, "tok-1|seg|seg|1790000000000");
        assert_eq!(a1.auth.expire, 1_790_000_000_000);
        assert_eq!(a1.device_id.as_str(), "B-admin-1");
        assert_eq!(a1.pc_platform, Some(PcPlatform::MacOs));
        assert_eq!(a1.user_domain_id.as_deref(), Some("1234567890123456789"));
        assert_eq!(a1.owner, Some(dst_admin.clone()), "归属重指到新机器的用户");

        // ③ 与设备绑的那一半：运行期状态 / 账号设备指纹 / 机器级那一枚
        assert_eq!(dst_doc.account_state, src_doc.account_state);
        assert_eq!(dst_doc.account_smid, src_doc.account_smid);
        assert_eq!(dst_doc.pc_device, src_doc.pc_device);
        assert_eq!(
            dst.handle
                .store
                .get_smid_state(&AccountId::new("13800000000"))
                .await
                .unwrap()
                .unwrap()
                .fingerprint_json
                .as_deref(),
            Some(r#"{"screen":"1920x1080"}"#)
        );

        // ④ 新机器上这些账号是**可用状态**：看得到（归属对 ✓）、凭据非空、没过期 ✓
        let visible = dst.handle.store.accounts_for(&dst_admin).await.unwrap();
        assert_eq!(visible.len(), 2, "账号在新机器上属于本机用户，看得见");
        for a in &visible {
            assert!(!a.auth.raw.is_empty() && !a.auth.token.is_empty());
            assert!(a.enabled, "启用状态搬过来了");
        }
        assert!(
            visible
                .iter()
                .any(|a| a.auth.expire > 0 && a.auth.days_until_expiry(1_790_000_000_000).is_some()),
            "到期时间能算出来（不是 0）—— 也就是这个凭据在本机是**能用的**"
        );

        // ⑤ 别人那个账号**没有**跟着搬过来（它不属于源机器的管理员）
        assert!(
            !have.iter().any(|a| a.id.as_str() == "13700000002"),
            "别人名下的账号不该进这份文件"
        );

        let _ = std::fs::remove_dir_all(&src_dir);
        let _ = std::fs::remove_dir_all(&dst_dir);
    }

    /// 门：**凭据字段确实在导出里** —— 防的是"以后有人顺手把它删掉"。
    ///
    /// 判据落在**序列化后的文本**上（不是结构体字段名）：字段名改了、`serde` 的
    /// rename 被去掉、或某一类被从 `collect` 里摘掉，这一条都会红 ✓。
    #[tokio::test]
    async fn export_text_carries_the_credential_marker_and_fields() {
        let (app, dir) = test_app("config-marker");
        let admin = UserId::new("admin");
        seed_user(&app, "admin", "admin", 100);
        seed_credentials(&app, &admin, &UserId::new("other")).await;

        let doc = config_doc::collect(&app.handle, &admin)
            .await
            .expect("导出");
        let json = config_doc::to_json(&doc).expect("序列化");

        // ① 顶层那个**明确的敏感标记**
        assert!(
            json.contains(r#""containsCredentials": true"#),
            "顶层要有 containsCredentials: true —— 这文件等于登录态"
        );
        // ② `notice` 那句要改成"别乱发、别进版本库"那一版
        assert!(
            json.contains("别提交进版本库"),
            "notice 要说清别乱发、别进版本库：{json}"
        );
        // ③ 十一类键都在（少了哪一类这条就红）
        for key in [
            "push-rules",
            "push-panel",
            "exchange-subscriptions",
            "auth-refresh",
            "pipelines",
            "schedules",
            "settings",
            "accounts",
            "account-state",
            "account-smid",
            "pc-device",
        ] {
            assert!(json.contains(&format!("\"{key}\"")), "导出里缺了 `{key}`");
        }
        // ④ **凭据本身**那几个字段（token / 到期时间 / 账号标识）
        for needle in [
            r#""raw": "raw-1""#,
            r#""token": "tok-1|seg|seg|1790000000000""#,
            r#""expire": 1790000000000"#,
            r#""phone": "13800000000""#,
            r#""user_domain_id": "1234567890123456789""#,
            r#""fingerprint_json": "{\"screen\":\"1920x1080\"}""#,
            r#""pc-device": "62AA2BDA29F99744536E142A83CA5B03-ENDIN""#,
        ] {
            assert!(json.contains(needle), "导出里缺了 {needle}：{json}");
        }
        // ⑤ 别人的账号**不在**（归属过滤那条边界）
        assert!(
            !json.contains("13700000002") && !json.contains("raw-o"),
            "别人名下的账号与凭据都不该进这份文件"
        );

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
        // ⚠ 2026-10-12 起凭据也进文件 ⇒ 用法里必须把这件事说清（否则用户以为
        // "导出的只是配置"，把这份登录态随手发出去 ✗）
        for key in ["accounts", "account-state", "account-smid", "pc-device"] {
            assert!(USAGE.contains(key), "用法里要说清导出带上 `{key}`");
        }
        assert!(USAGE.contains("登录态"), "用法里要说清这文件等于登录态");
        assert!(
            USAGE.contains("不用重新登录"),
            "用法里要说清导入后不用重新登录"
        );
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
