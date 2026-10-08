//! `pipeline <编排id>` —— 跑一条**已配置**的编排，跑完就退出。
//!
//! # 为什么 CLI 只能跑、不能建
//!
//! 编排是"配一次、之后自动跑"的东西：写进 `pipelines` 存储后就很少动。
//! 而它的编辑器（拖步骤、改参数、调 `continue_on_error`）是最难写对的一页。
//! 与其在终端里发明一套 flag 语法，不如：
//!
//! - **创建/修改** → WebUI 或直接编辑配置文件
//! - **运行** → 这里（排障时想看清退出码，或 WebUI 打不开时）
//!
//! 少一个入口，就少一处会与 WebUI 漂移的语义。
//!
//! # 退出码
//!
//! - `0` —— 编排整体 `ok`
//! - `1` —— 有失败/部分失败
//! - `2` —— 配置错误（找不到这条、有步骤引用了不存在的任务）
//!
//! ⚠ `partial` 也算失败（退 1）：编排里有一步没成，"整体算成功"会让
//! 定时任务在"每天都有一步失败"的情况下绿灯一个月。

use anyhow::Context;
use mcloud_core::PipelineId;

use mcloud_app::App;

const EXIT_CONFIG: i32 = 2;

/// `pipeline run <编排id>`
///
/// # 为什么是 `pipeline run <id>` 而不是 `pipeline <id>`
///
/// 与 `auth refresh` 保持同一形状：**组 + 动作 + 目标**。
/// 两个命令族形状不一致时，人记的是两套；而"少打一个 `run`"这种错
/// 会退 2 并打一屏用法 —— 能用，但没必要让人撞一次。
///
/// 裸 `pipeline <id>` 也接受：只有这一个动作，不产生歧义。
pub async fn run_pipeline(app: &App, args: &[String]) -> anyhow::Result<()> {
    // 接受 `pipeline run <id>` 与 `pipeline <id>` 两种写法。
    let args: Vec<String> = match args.first().map(String::as_str) {
        Some("run") => args[1..].to_vec(),
        _ => args.to_vec(),
    };

    let id = match args.first() {
        Some(v) => v.clone(),
        None => {
            eprintln!("用法：pipeline run <编排id>");
            eprintln!();
            eprintln!("跑一条已配置的编排，跑完就退出。");
            eprintln!("编排的创建与修改在 WebUI 的「编排」页（或直接编辑配置文件）。");
            eprintln!();
            print_pipelines(app).await;
            std::process::exit(EXIT_CONFIG);
        }
    };

    let pid = PipelineId::new(id.clone());
    let pipeline = match app.handle.store.pipeline(&pid).await? {
        Some(p) => p,
        None => {
            eprintln!("找不到编排：{id}");
            print_pipelines(app).await;
            std::process::exit(EXIT_CONFIG);
        }
    };

    run_one(app, &pipeline).await
}

/// 跑**一条已经在手的编排**（不查库 —— 查库那一步在调用方）。
///
/// # 为什么要拆出来
///
/// 2026-10-07 用户口径：「实现一个默认的编排任务…cli 一条命令」＋ 紧接着的更正
/// 「**默认编排也要和普通的自定义编排一个逻辑**」✓。
/// ⇒ "默认编排"不该有自己的一套跑法 ✗：`run-all` 与 `pipeline run <id>` 走
/// **同一个执行体**（账号解析、逐步播报、退出码逐格相同 ✓），差别只在"这条编排是从
/// 库里取的，还是内置预设" ✓。
///
/// ⚠ 只收 `&Pipeline`（不是 id）：预设因此能直接进来，而**不必**先写进库 ✓ ——
/// 命令行顺手往用户库里塞一条，正是被否掉的行为 ✓。
pub(crate) async fn run_one(app: &App, pipeline: &mcloud_core::Pipeline) -> anyhow::Result<()> {
    // 账号：配置里指定了就用它，否则**所有已启用账号**。
    let accounts = target_accounts(app, &pipeline.accounts).await?;
    if accounts.is_empty() {
        eprintln!("这条编排没有可跑的账号（配置为空且实例里没有已启用账号）");
        std::process::exit(EXIT_CONFIG);
    }

    println!(
        "编排：{}（{} 步 × {} 个账号）",
        pipeline.name,
        pipeline.steps.len(),
        accounts.len()
    );

    let outcome = app
        .handle
        .pipelines
        // ⚠ `pipeline` 现在已经是 `&Pipeline`（`run_one` 的入参）⇒ 这里**不再**取引用
        // （多一层 `&` 就是 `needless_borrow`，clippy `-D warnings` 会红 ✓）。
        .run(pipeline, &accounts, None)
        .await
        .context("编排执行失败")?;

    for (i, step) in outcome.steps.iter().enumerate() {
        // 每一步报：状态 + 这一步在几个账号上跑成了
        // ⚠ `StepRun` 带的是 `outcome: TaskOutcome`（成功/跳过/失败），
        // 不是 `StepStatus` —— 后者是**整步**的汇总。
        let ok = step.runs.iter().filter(|r| r.outcome.is_success()).count();
        println!(
            "  {}. [{}] {}（{ok}/{} 个账号成功）",
            i + 1,
            status_label(step.status),
            step.task,
            step.runs.len()
        );
    }
    if let Some(at) = outcome.stopped_after {
        println!("在第 {} 步中止（后续步骤未执行）", at + 1);
    }

    let overall = outcome.status();
    // ⚠ **先把在飞的推送等落地，再决定退出码**（2026-10-07，用户口径
    // 「想想 cli 上该怎么做，记得写的时候兼容」✓）：投递是 spawn 出去的 ✓，
    // 而 CLI 跑完就退 ⇒ 进程一没、在飞的 HTTP 就断了 ✗ —— 失败时那条恰恰最该到 ✓。
    app.handle.pushes.flush().await;
    println!("{}", outcome.summary_line());
    if overall == mcloud_engine::StepStatus::Ok {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

/// `StepStatus` 的中文标签。
///
/// 不在 `StepStatus` 上实现 `Display`：那是引擎层的类型，服务端/前端/推送
/// 各有各的措辞需求，把它钉死成一种语言会让别处只能绕开它。
fn status_label(s: mcloud_engine::StepStatus) -> &'static str {
    use mcloud_engine::StepStatus as S;
    match s {
        S::Ok => "成功",
        S::Partial => "部分成功",
        S::Error => "失败",
        S::Skipped => "跳过",
        S::Cancelled => "已中止",
    }
}

/// 解析目标账号：配置里有就用，没有就是所有账号。
///
/// 与 WebUI 的 `/api/pipelines/{id}/run` 用**同一条规则** ——
/// 两处各写一遍必然漂移，而漂移的表现是"CLI 跑 3 个账号、WebUI 跑 1 个"。
async fn target_accounts(
    app: &App,
    configured: &[String],
) -> anyhow::Result<Vec<mcloud_core::Account>> {
    let all = app.handle.store.accounts().await?;
    if configured.is_empty() {
        // ⚠ 本项目**没有**账号级启用/停用（见 `AuthRefreshService::refresh_all`）。
        // 上游参考实现这里过滤掉停用账号；我们没有那个概念，所以不过滤 ——
        // 而不是编一个恒为 `true` 的字段来"看起来在过滤"。
        return Ok(all);
    }
    let mut out = Vec::new();
    for want in configured {
        let id = mcloud_core::AccountId::new(want.clone());
        match all.iter().find(|a| a.id == id) {
            Some(a) => out.push(a.clone()),
            // 配置里点名的账号不存在 ⇒ 配置错误，不静默跳过：
            // 静默跳过的表现是"编排跑了但那个账号没动"。
            None => {
                eprintln!("编排点名的账号不存在：{want}");
                std::process::exit(EXIT_CONFIG);
            }
        }
    }
    Ok(out)
}

async fn print_pipelines(app: &App) {
    match app.handle.store.pipelines().await {
        Ok(list) if !list.is_empty() => {
            eprintln!();
            eprintln!("现有编排（{} 条）：", list.len());
            for p in list {
                let flag = if p.enabled { "" } else { " [已禁用]" };
                eprintln!("  {:<20} {:<10} {} 步{}", p.id, p.name, p.steps.len(), flag);
            }
        }
        Ok(_) => eprintln!("\n当前没有任何编排。在 WebUI 的「编排」页创建，或直接编辑配置文件。"),
        Err(e) => eprintln!("\n（读取编排列表失败：{e}）"),
    }
}
