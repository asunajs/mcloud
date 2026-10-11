//! `mcloud run` —— **一键跑默认编排**（**不带参数**那一支 ✓）。
//!
//! 用户口径（2026-10-07）：「实现一个默认的编排任务，让本项目能和 ts 版一样一键运行，
//! **cli 一条命令**，webui 一个定时」✓。
//!
//! 用户口径（2026-10-11）：「**`mcloud run` 为简写，主要功能是立即执行所有账号的
//! 系统的默认每日任务**」✓ ⇒ 两种形式**都留**：
//!
//! * `mcloud run`（**不带参数**）⇒ 立即把「系统默认每日任务」跑在**本机管理员名下的
//!   全部账号**上 ✓（= [`run_default`] ✓）；
//! * `mcloud run <任务名> <账号>`（**带参数**）⇒ 照旧只跑一次那个任务 ✓
//!   （那条路在 `cli/run.rs` 里，**一个字没动** ✓）。
//!
//! ⚠ 2026-10-11 用户裁定：**那条与 `mcloud run`（不带参数）逐格相同的一键子命令已删** ✗
//! （原先它是 [`run_all`] —— 用户原话：「**它是多余的**」✗；`AGENTS.md` 的「补充裁定 2」：
//! **CLI 要简单** ✓）⇒ 本模块只剩 `run_default` 这一个入口 ✓，删掉的那个 `run_all`
//! 只是转调它（`pub async fn run_all(...) { run_default(app).await }` ✓）。
//! ⚠ 那个词现在在 `crates/` 里只剩 `cli/run.rs` 那一处历史注释（记录"删了什么、为什么"✓）。
//!
//! # 与 `mcloud pipeline run daily` 的关系
//!
//! 两者跑的是**同一条**编排，执行体也是**同一个**（`cli/pipeline.rs` 的 `run_one` ✓）——
//! 差别只在"要不要先知道那个 id" ✓。**本模块不复制**账号解析、逐步播报、退出码那一套 ✓。
//!
//! # ⚠ 「系统默认每日任务」的判据**只有一份，不是这里发明的**
//!
//! 就是 `mcloud_core::pipeline::default_pipeline_id(管理员)`（= **`daily-<管理员id>`** ✓，
//! 见 `crates/mcloud-core/src/pipeline.rs` ✓）那条编排：
//!
//! * 库里**有** ⇒ 跑**库里那份**（他可能改过它 —— 去掉几步、改名，那一份才是他认可的
//!   "每天该跑的" ✓）；
//! * 库里**没有**（还没登录过、或他删了）⇒ 跑**内置预设**，**不写库** ✗
//!   （命令行顺手往用户库里塞东西是被否掉的行为 ✓）。
//!
//! 本机 `data/schedules.json` 里那条 `daily-<owner>` 定时跑的就是它 ✓
//! （`task: "pipeline"` + `body.pipelineId: "daily-<owner>"` ✓）—— 定时与命令行
//! **共用同一个判据** ⇒ "定时跑的那条链"与"`mcloud run` 跑的那条链"永远是同一条 ✓。
//!
//! # ⚠ 「所有账号」的判据也只有一份
//!
//! `store.accounts_for(管理员)` —— **唯一入口 `mcloud run`（不带参数）走
//! [`resolve_default_daily`] 那一份** ✓，
//! 与调度器 `Scheduler::target_accounts`
//! （`crates/mcloud-engine/src/scheduler.rs` ✓）对一条**已归属**定时做的事逐格相同：
//! 归属是**硬边界**，`accounts: []` 的语义是「**我名下的全部账号**」，
//! **不是**「库里全部账号」✗ —— 后者会让别人建的一条不带账号的定时去跑**你的**账号
//! （真花你的 AI豆、动你的网盘文件 ✗）。
//!
//! CLI 固定以**管理员**（`owner::legacy_owner` = 创建最早的那个 ✓）身份行事
//! （`AGENTS.md` 的「补充裁定 2」：CLI 仅操作管理员 ✓、不提供 `--user`/`--owner` ✗）。
//!
//! # 串行还是并发
//!
//! **串行** —— 不是这里选的，是引擎的既有事实：`ensure_supported_account_mode` 明确
//! **拒绝** `AccountMode::Parallel`（"当前只支持串行执行（上游限流）"✓），而默认编排的
//! `account_mode` 就是 `Sequential` ✓。理由也在那一段注释里：上游常有限流，
//! 并发打过去容易触发风控 ✓。⇒ 本命令**照旧**串行，不另开一条并发路 ✗。
//!
//! # 参数
//!
//! 不收额外参数：目标账号就是"**管理员名下的全部账号**"（与那条默认定时同一条口径 ✓）。
//! 想指定账号就用 `pipeline run <id> --account …` 那条路 ✓。

use anyhow::Context;

use mcloud_app::App;
use mcloud_core::{Account, AccountId, Pipeline, TaskOutcome};

use crate::cli::pipeline as cli_pipeline;

/// 「没有可跑的账号」的退出码 —— 与 `pipeline run` 的配置错**同码** ✓
///（那边是 `cli/pipeline.rs` 的 `EXIT_CONFIG`，同一个数、同一条口径 ✓）。
const EXIT_CONFIG: i32 = 2;

/// 判据：**「系统默认每日任务」是哪条编排**（库里那份优先，没有就用内置预设 ✓）。
///
/// ⚠ 这是**唯一**一份实现 ✓：`mcloud run`（不带参数）走它 ——
/// 抄成两份迟早分叉（分叉的表现是"两条命令跑的链不一样"，而那最难发现 ✗）。
///
/// `owner` 为 `None`（全新库、用户表为空）⇒ 库里那条查不到，直接给内置预设 ✓
/// （`default_pipeline_id(None)` = `daily`，那是 **CLI 的临时预设**、**不落库** ✓）。
async fn load_default_pipeline(
    app: &App,
    owner: Option<&mcloud_core::UserId>,
) -> anyhow::Result<Pipeline> {
    let found = match owner {
        Some(u) => {
            let id =
                mcloud_core::PipelineId::new(mcloud_core::pipeline::default_pipeline_id(Some(u)));
            app.handle.store.pipeline(&id).await?
        }
        None => None,
    };
    Ok(match found {
        // 库里有就用**库里那份** —— 他可能改过它（去掉几步、改名），
        // 那一份才是他认可的"每天该跑的" ✓。
        Some(p) => p,
        // 库里没有（还没登录过、或他删了）⇒ 用**内置预设**直接跑，**不写库** ✗ ——
        // 命令行不该顺手往用户库里塞东西 ✓。
        None => mcloud_core::pipeline::default_pipeline(None),
    })
}

/// 「系统默认每日任务」的两个集合 —— **一处算清**，判据不复制 ✓。
#[derive(Debug, Clone)]
pub struct DefaultDaily {
    /// 判据给的那条编排（[`load_default_pipeline`] ✓）。
    pub pipeline: Pipeline,
    /// **管理员名下的全部账号**（`accounts_for(管理员)` ✓，不是库里全部 ✗）。
    pub accounts: Vec<Account>,
}

/// 算「系统默认每日任务」要跑的那条链 + 那批账号。
///
/// 两个集合的判据都在这里、都**复用既有那一处**（见模块头的两段说明 ✓）。
/// ⚠ 账号那一段与 `Scheduler::target_accounts` **逐格相同**：先把账号收在归属边界内，
/// 再按编排自己点名的账号做交集（`accounts: []` = 我名下全部 ✓）——
/// 顺序反过来就会让"别人点名的账号"漏进来 ✗。
pub async fn resolve_default_daily(app: &App) -> anyhow::Result<DefaultDaily> {
    let users = app.handle.store.users().await?;
    let owner = mcloud_core::owner::legacy_owner(&users);
    let pipeline = load_default_pipeline(app, owner.as_ref()).await?;
    let owned = match &owner {
        // 已归属：**只取主人的账号**（`accounts_for` 里含「单用户无主可见」规则 ✓）。
        Some(u) => app.handle.store.accounts_for(u).await?,
        // 未归属：只有"老库且用户表为空"才该出现（有用户时加载会回填 ✓）。
        // 没有用户 ⇒ 全库（此时不存在多用户 ✓，与调度器同一条处置 ✓）。
        None => app.handle.store.accounts().await?,
    };
    let accounts = if pipeline.accounts.is_empty() {
        owned
    } else {
        let wanted: Vec<AccountId> = pipeline
            .accounts
            .iter()
            .map(|s| AccountId::new(s.clone()))
            .collect();
        owned
            .into_iter()
            .filter(|a| wanted.contains(&a.id))
            .collect()
    };
    Ok(DefaultDaily { pipeline, accounts })
}

/// 一次「系统默认每日任务」的收尾读数。
///
/// # 为什么把它**返回**出来，而不是只打印
///
/// "报什么、退几"是这条命令的**行为契约**（用户口径：逐账号逐任务报结果 ✓、
/// 有失败 ⇒ 非 0 ✓）—— 只打印的话，单测只能去抓 stdout ✗，
/// 而抓 stdout 的单测最容易变成"改个标点就红"的噪音 ✓。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyOutcome {
    /// 跑的那条编排的 id（判据给的 ✓）。
    pub pipeline: String,
    /// **逐账号逐任务**的结论：`(账号, 任务, 结论标签)` ✓
    ///（标签是 `成功` / `跳过` / `部分成功` / `失败` ✓）。
    pub runs: Vec<(String, String, String)>,
    /// 退出码：**有失败 ⇒ 非 0** ✓（0 = 整体 `ok`；1 = 有失败/部分成功/被中止；
    /// 2 = 没有可跑的账号）。
    pub code: i32,
}

/// `mcloud run`（**不带参数**）—— **立即**把「系统默认每日任务」跑在
/// **管理员名下的全部账号**上 ✓。
///
/// ⚠ **不在这里 `exit`**：退出码随 [`DailyOutcome::code`] 返回，由调用方
/// （`cli/run.rs` ✓）决定 —— 这样单测能断言退出码而**不会把测试进程干掉** ✓。
pub async fn run_default(app: &App) -> anyhow::Result<DailyOutcome> {
    let target = resolve_default_daily(app).await?;
    let pipeline = &target.pipeline;
    let id = pipeline.id.to_string();

    if target.accounts.is_empty() {
        // ⚠ 与 `run_one` 同一条处置（打一屏说明 + 退 2）："没账号可跑"**不是**跑通了 ✓，
        // 也不该静默当成功 —— 那会让人以为今天的任务已经跑过了 ✗。
        eprintln!(
            "系统默认每日任务：{}（{} 步）",
            pipeline.name,
            pipeline.steps.len()
        );
        eprintln!("⚠ 本机管理员名下没有可跑的账号 —— 先登一个：`mcloud login qr` ✓");
        return Ok(DailyOutcome {
            pipeline: id,
            runs: Vec::new(),
            code: EXIT_CONFIG,
        });
    }

    println!(
        "系统默认每日任务：{}（{} 步 × {} 个账号）",
        pipeline.name,
        pipeline.steps.len(),
        target.accounts.len()
    );

    let outcome = app
        .handle
        .pipelines
        .run(pipeline, &target.accounts, None)
        .await
        .context("系统默认每日任务执行失败")?;

    // 逐账号逐任务报结果 ✓（步骤在外、账号在内 —— 与引擎的执行顺序一致 ✓）。
    let mut runs = Vec::new();
    for (i, step) in outcome.steps.iter().enumerate() {
        println!(
            "  {}. [{}] {}",
            i + 1,
            cli_pipeline::status_label(step.status),
            step.task
        );
        for r in &step.runs {
            let label = outcome_label(&r.outcome);
            println!("       · {} {} — {}", r.account, label, r.outcome.summary());
            runs.push((r.account.clone(), r.task.clone(), label.to_string()));
        }
    }
    if let Some(at) = outcome.stopped_after {
        println!("在第 {} 步中止（后续步骤未执行）", at + 1);
    }

    // ⚠ **先把在飞的推送等落地，再决定退出码**（与 `run_one` 同一条理由 ✓）：
    // 投递是 spawn 出去的 ✓，而 CLI 跑完就退 ⇒ 进程一没、在飞的 HTTP 就被掐了 ✗。
    app.handle.pushes.flush().await;
    println!("{}", outcome.summary_line());

    Ok(DailyOutcome {
        pipeline: id,
        runs,
        code: exit_code(outcome.status()),
    })
}

/// 退出码：**只有整体 `ok` 才是 0** ✓（与 `pipeline run` 逐格相同 —— 同一件事不该有
/// 两套退出码判据 ✓）。`Partial` 也算非 0：编排里有一步没成，"整体算成功"会让
/// 定时/脚本在"每天都有一步失败"的情况下绿灯一个月 ✗。
fn exit_code(status: mcloud_engine::StepStatus) -> i32 {
    if status == mcloud_engine::StepStatus::Ok {
        0
    } else {
        1
    }
}

/// [`TaskOutcome`] 的中文标签（四档全给：`Skipped` **不是**失败 ✓，
/// `Partial` 两边的账都要说清 ✓ —— 见 `mcloud_core::run` 那段说明 ✓）。
fn outcome_label(o: &TaskOutcome) -> &'static str {
    match o {
        TaskOutcome::Success { .. } => "成功",
        TaskOutcome::Skipped { .. } => "跳过",
        TaskOutcome::Partial { .. } => "部分成功",
        TaskOutcome::Failed { .. } => "失败",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use mcloud_app::AppBuilder;
    use mcloud_core::{
        AccountSettings, AuthInfo, DeviceId, HttpClient, HttpRequest, HttpResponse, PasswordHash,
        User, UserId,
    };

    /// 「谁来都报网络错」的假上游 —— **绝不打真上游** ✗
    /// （与 `tests/routes.rs` 的 `DeadUpstream` 同款 ✓）。
    struct DeadUpstream;

    #[async_trait::async_trait]
    impl HttpClient for DeadUpstream {
        async fn request(&self, req: HttpRequest) -> mcloud_core::Result<HttpResponse> {
            Err(mcloud_core::Error::Network(format!(
                "CLI 测试的假上游：不打网络（{}）",
                req.url
            )))
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("mcloud-cli-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// 临时数据目录上的应用，**按账号的 HTTP 客户端是假的** ✓。
    fn test_app(tag: &str) -> (App, std::path::PathBuf) {
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

    fn seed_user(app: &App, id: &UserId, name: &str, created_at_ms: i64) {
        let user = User {
            id: id.clone(),
            name: name.into(),
            password_hash: PasswordHash("hashed".into()),
            created_at_ms,
        };
        futures::executor::block_on(app.handle.store.save_user(&user)).expect("落库");
    }

    fn account(id: &str, owner: &UserId) -> Account {
        Account {
            id: AccountId::new(id),
            nickname: None,
            auth: AuthInfo {
                raw: "raw-token".into(),
                platform: "android".into(),
                phone: id.into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("dev".into()),
            pc_platform: None,
            user_domain_id: None,
            owner: Some(owner.clone()),
            enabled: true,
            settings: AccountSettings::default(),
        }
    }

    fn seed_account(app: &App, a: Account) {
        futures::executor::block_on(app.handle.store.save_account(&a)).expect("落库");
    }

    /// ⭐ 门：**两个集合都要对** ——
    /// ① 账号集合 = **管理员名下的全部账号**（不是库里全部 ✗、不含别人的 ✗）；
    /// ② 任务集合 = **既有判据**（`default_pipeline` / `default_pipeline_id` ✓）
    ///    那条默认每日任务链（不是这里发明的 ✗）。
    ///
    /// ⚠ 这一条**不跑**任何任务（只算两个集合 ✓）⇒ 与上游无关、也不会慢。
    #[tokio::test]
    async fn default_daily_takes_the_admin_accounts_and_the_builtin_daily_chain() {
        let (app, dir) = test_app("run-default-sets");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, &admin, "admin", 100);
        seed_user(&app, &other, "other", 200);
        seed_account(&app, account("a1", &admin));
        seed_account(&app, account("a2", &admin));
        // 别人的账号：**必须一个都不进** ✓
        seed_account(&app, account("b1", &other));

        let target = resolve_default_daily(&app).await.expect("判据应能算出来");

        // ① 账号集合
        let got: Vec<String> = target.accounts.iter().map(|a| a.id.to_string()).collect();
        assert_eq!(
            got,
            vec!["a1".to_string(), "a2".to_string()],
            "「所有账号」= **管理员名下**的全部账号（不是库里全部、也不含别人的）"
        );

        // ② 任务集合 —— 逐条对齐**既有判据**
        let want = mcloud_core::pipeline::default_pipeline(None);
        let names: Vec<&str> = target
            .pipeline
            .steps
            .iter()
            .map(|s| s.task.as_str())
            .collect();
        let want_names: Vec<&str> = want.steps.iter().map(|s| s.task.as_str()).collect();
        assert_eq!(names, want_names, "跑的必须是既有判据那条默认每日任务链");
        assert!(!names.is_empty(), "默认每日任务不该是空链");
        // 库里没有 `daily-<管理员>` ⇒ 用**内置预设**（CLI 的临时预设，**不写库** ✓）
        assert_eq!(
            target.pipeline.id.as_str(),
            mcloud_core::pipeline::default_pipeline_id(None),
            "库里没有那条时用内置预设 ✓"
        );
        // 而"要查的那条"由归属决定 ⇒ 管理员那份的 id 是 `daily-<管理员id>` ✓
        assert_eq!(
            mcloud_core::pipeline::default_pipeline_id(Some(&admin)),
            "daily-admin",
            "默认编排/默认定时的 id 判据（`daily-<用户id>` ✓）"
        );
        // 内置预设**不落库** ✓（命令行不该顺手往用户库里塞东西）
        assert!(
            futures::executor::block_on(app.handle.store.pipelines())
                .expect("读库")
                .is_empty(),
            "命令行跑默认编排**不写库** ✗"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 门：**库里那份优先** ✓ —— 用户改过默认编排（去掉几步）之后，
    /// `mcloud run` 跑的是**他改过的那份**，不是内置预设 ✗。
    #[tokio::test]
    async fn default_daily_prefers_the_stored_pipeline() {
        let (app, dir) = test_app("run-default-stored");
        let admin = UserId::new("admin");
        seed_user(&app, &admin, "admin", 100);

        let id = mcloud_core::pipeline::default_pipeline_id(Some(&admin));
        let mut stored = mcloud_core::pipeline::default_pipeline(Some(admin.clone()));
        stored.name = "我改过的每日".into();
        stored.steps.truncate(2);
        futures::executor::block_on(app.handle.store.save_pipeline(&stored)).expect("落库");

        let target = resolve_default_daily(&app).await.expect("判据应能算出来");
        assert_eq!(target.pipeline.id.as_str(), id.as_str());
        assert_eq!(target.pipeline.name, "我改过的每日");
        assert_eq!(target.pipeline.steps.len(), 2, "跑的是库里那份（改过的）✓");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 门：**逐账号逐任务报结果** + **有失败 ⇒ 非 0** ✓。
    ///
    /// 用一条**库里改过的**单步编排（`notice` ✓）跑两个账号，上游是假的（谁来都报网络错）
    /// ⇒ 两发都失败 ⇒ 退出码非 0 ✓，而读数里**两个 (账号, 任务) 组合都在** ✓
    /// （这正是"逐账号逐任务"的意思：不是只报一个汇总数 ✗）。
    #[tokio::test]
    async fn default_daily_reports_every_account_and_task_and_fails_loud() {
        let (app, dir) = test_app("run-default-reports");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, &admin, "admin", 100);
        seed_user(&app, &other, "other", 200);
        seed_account(&app, account("a1", &admin));
        seed_account(&app, account("a2", &admin));
        seed_account(&app, account("b1", &other));

        // 库里那条默认每日：砍成**一步**（`notice`，无 sleep ✓）⇒ 这一趟只发两枪。
        let id = mcloud_core::pipeline::default_pipeline_id(Some(&admin));
        let mut stored = mcloud_core::pipeline::default_pipeline(Some(admin.clone()));
        stored.steps.retain(|s| s.task == "notice");
        assert_eq!(stored.steps.len(), 1);
        futures::executor::block_on(app.handle.store.save_pipeline(&stored)).expect("落库");

        let outcome = run_default(&app).await.expect("跑得起来");

        assert_eq!(outcome.pipeline, id, "跑的是库里那条默认每日 ✓");
        assert_eq!(
            outcome.runs,
            vec![
                ("a1".to_string(), "notice".to_string(), "失败".to_string()),
                ("a2".to_string(), "notice".to_string(), "失败".to_string()),
            ],
            "逐账号逐任务：两个账号各一条结论，且**别人的账号一条都没有** ✓"
        );
        assert_ne!(outcome.code, 0, "有失败 ⇒ 退出码非 0 ✓");
        assert_eq!(outcome.code, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐⭐ 门（**本线的核心**）：`mcloud run`（**不带参数**）的账号范围 =
    /// **本机管理员名下的全部账号** ✓
    /// —— 库里**两个用户、三条账号**（管理员两条 + 别人的一条）⇒ 只跑管理员那两条，
    /// **别人的一条都没有** ✓。
    ///
    /// # 为什么非要有这一条
    ///
    /// 改前走的是 `cli/pipeline.rs::run_one`（那时这条门钉的是已删掉的那条一键子命令
    /// `run_all`，2026-10-11 用户裁定删掉它之后，入口收敛成 `mcloud run` 不带参数 ⇒
    /// 本用例改为直接钉 `run_default` ✓，断言一条没少 ✓），而那边的账号判据是
    /// `store.accounts()`（= **库里全部账号** ✗）⇒ 多用户实例里会跑到**别人名下**的账号上 ✗
    /// （真花别人的 AI豆、动别人的网盘文件 ✗）。而单用户实例上"库里全部"**恰好等于**
    /// "管理员名下全部" ⇒ **既有用例全绿也照样漏** ✗ —— 所以这条用例把"多用户"造出来，
    /// 让那个漏**必然**暴露 ✓。
    ///
    /// 上游是假的（谁来都报网络错 ✓，**绝不打真上游** ✗）⇒ 两发都失败、退出码 1 ✓；
    /// 跑没跑、跑了谁的，看读数就够 ✓。
    #[tokio::test]
    async fn default_daily_only_touches_the_admin_accounts() {
        let (app, dir) = test_app("run-default-admin-only");
        let admin = UserId::new("admin");
        let other = UserId::new("other");
        seed_user(&app, &admin, "admin", 100);
        seed_user(&app, &other, "other", 200);
        seed_account(&app, account("a1", &admin));
        seed_account(&app, account("a2", &admin));
        // 别人的账号：**必须一个都不进** ✓
        seed_account(&app, account("b1", &other));

        // 默认每日砍成**一步**（`notice`，无 sleep ✓）⇒ 这一趟只该发两枪。
        let mut stored = mcloud_core::pipeline::default_pipeline(Some(admin.clone()));
        stored.steps.retain(|s| s.task == "notice");
        assert_eq!(stored.steps.len(), 1);
        futures::executor::block_on(app.handle.store.save_pipeline(&stored)).expect("落库");

        let outcome = run_default(&app).await.expect("跑得起来");

        assert_eq!(
            outcome.runs,
            vec![
                ("a1".to_string(), "notice".to_string(), "失败".to_string()),
                ("a2".to_string(), "notice".to_string(), "失败".to_string()),
            ],
            "`mcloud run`（不带参数）只跑**管理员名下**那两条（`accounts_for(管理员)` ✓），\
             别人的账号一条都不许进 ✗"
        );
        assert_eq!(outcome.code, 1, "假上游 ⇒ 两发都失败 ⇒ 非 0 ✓");

        // 再从**落库的读数**独立核一遍（不看上面那份返回值）：跑过的账号集合里**不许有 b1** ✓
        let mut ran: Vec<String> = futures::executor::block_on(app.handle.store.runs(50))
            .expect("读历史")
            .iter()
            .map(|r| r.account.clone())
            .collect();
        ran.sort();
        assert_eq!(
            ran,
            vec!["a1".to_string(), "a2".to_string()],
            "落库的读数同样只有管理员那两条 —— 别人的一条都没有 ✓"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 门：退出码判据**只有一份**（与 `pipeline run` 逐格相同 ✓）——
    /// 整体 `ok` ⇒ 0；有失败/部分成功/被中止 ⇒ 1 ✓。
    #[test]
    fn exit_code_is_zero_only_for_a_fully_ok_chain() {
        use mcloud_engine::pipeline::{StepRun, StepSummary};
        use mcloud_engine::StepStatus;

        let step = |status: StepStatus, outcome: TaskOutcome| StepSummary {
            index: 0,
            task: "notice".into(),
            status,
            runs: vec![StepRun {
                step_index: 0,
                task: "notice".into(),
                account: "a1".into(),
                run_id: 1,
                outcome,
                cancelled: false,
            }],
        };
        let code = |status: StepStatus, outcome: TaskOutcome| {
            exit_code(
                mcloud_engine::PipelineOutcome {
                    steps: vec![step(status, outcome)],
                    stopped_after: None,
                    cancelled: false,
                }
                .status(),
            )
        };

        assert_eq!(code(StepStatus::Ok, TaskOutcome::success("签到成功")), 0);
        assert_eq!(
            code(StepStatus::Ok, TaskOutcome::skipped("活动没开")),
            0,
            "跳过**不是**失败 ⇒ 不算非 0 ✓"
        );
        assert_eq!(code(StepStatus::Error, TaskOutcome::failed("上游 401")), 1);
        assert_eq!(
            code(
                StepStatus::Partial,
                TaskOutcome::partial("领到 2 笔，失败 1 笔")
            ),
            1,
            "部分成功也算非 0（不然每天都有一步失败也会绿灯 ✗）"
        );
    }
}
