//! `cron <定时id>` —— 跑一条**已配置**的定时，跑完就退出。
//!
//! # 为什么不常驻
//!
//! 这个子命令是给**系统 crontab** 调的：
//!
//! ```text
//! 0 8 * * *  /usr/local/bin/mcloud cron daily-sign >> /var/log/mcloud.log 2>&1
//! ```
//!
//! 时间由 crontab 管，CLI 只负责"把这一条跑了"。
//!
//! # 为什么不让 CLI 自己常驻调度
//!
//! 常驻就要回答"进程挂了怎么办""两个实例同时跑怎么办""重启后补不补错过的点"
//! —— 而这三件事在 `mcloud-server` 的调度器里已有完整答案（水位线、
//! 错过策略、并发闸）。CLI 再实现一份就是**两处语义**，
//! 而它们的差异极难发现：一个补跑、一个不补。
//!
//! # ⚠ 与 server 的调度器不要同时开同一条
//!
//! 水位线是"读 → 判 → 写"，**不是跨进程的原子认领**。
//! server 正在跑调度时，外部 crontab 再触发同一条，两边都可能判定"到点了"，
//! 于是同一个点跑两次。所以这里启动时明确打印一句警告（见下）。
//!
//! # 退出码
//!
//! - `0` —— 成功，或**这次不该跑**（未到点/已禁用/正在跑另一份）
//! - `1` —— 跑了但任务失败
//! - `2` —— 配置错误（找不到这条、表达式坏了、任务名不认识）
//!
//! "不该跑"退 `0` 是刻意的：crontab 会把非零退出码当成"这次执行出错"，
//! 而"今天这次不用跑"是**正常**的。退了 1，监控会天天报假警。

use anyhow::Context;
use mcloud_core::ScheduleId;
use mcloud_engine::Decision;

use mcloud_app::App;

/// 退出码：配置错误。与 `bin/cli.rs` 里用法错误的退码保持一致。
const EXIT_CONFIG: i32 = 2;

/// `cron <定时id>`
pub async fn run_schedule(app: &App, args: &[String]) -> anyhow::Result<()> {
    let id = match args.first() {
        Some(v) => v.clone(),
        None => {
            eprintln!("用法：cron <定时id>");
            eprintln!();
            eprintln!("跑一条已配置的定时任务，跑完就退出。给系统 crontab 调用：");
            eprintln!("  0 8 * * *  mcloud cron daily-sign");
            eprintln!();
            print_schedules(app).await;
            std::process::exit(EXIT_CONFIG);
        }
    };

    let sid = ScheduleId::new(id.clone());
    let schedule = match app.handle.store.schedule(&sid).await? {
        Some(s) => s,
        None => {
            // 找不到这条是**配置错误**：多半是 id 打错了，或 crontab 里
            // 写了一条还没建的定时。两者都该让人立刻看到，而不是静默退出 0。
            eprintln!("找不到定时任务：{id}");
            print_schedules(app).await;
            std::process::exit(EXIT_CONFIG);
        }
    };

    // 先自己判一次"现在该不该跑"。
    //
    // ⚠ 这一步**不推进水位线** —— 真正的认领在 `Scheduler::tick` 里做，
    // 而且是"打上游之前推进"（宁可漏跑不可重跑）。这里只是提前给出
    // 一个说得清的结果，免得每次 crontab 调用都去跑一遍不该跑的。
    let now = app.handle.clock.now_ms();
    match mcloud_engine::scheduler::decide(&schedule, now) {
        Ok(Decision::Due { point_ms }) => {
            tracing::info!(schedule = %id, point_ms, "cron 触发");
        }
        Ok(Decision::NotYet { next_ms }) => {
            // 正常情况：crontab 与定时表达式的相位没对齐。
            // 退 0（见文件头"退出码"）。
            println!("未到执行时间（下次 {}）", fmt_ms(next_ms));
            return Ok(());
        }
        Ok(Decision::Disabled) => {
            println!("定时已禁用：{id}");
            return Ok(());
        }
        Ok(Decision::NeverFires) => {
            eprintln!("表达式永远不会触发：{}", schedule.expr);
            std::process::exit(EXIT_CONFIG);
        }
        Ok(Decision::Missed { .. }) => {
            // `decide` 的 Missed 表示"错过了且超出宽限窗口"。
            // 交给 tick 去按策略处置（它会把水位推到当前并记为弃掉）。
            tracing::info!(schedule = %id, "错过了触发点，交由调度器处置");
        }
        Err(e) => {
            eprintln!("表达式无法解析：{}（{e}）", schedule.expr);
            std::process::exit(EXIT_CONFIG);
        }
    }

    // ⚠ 交叉进程的重复触发提醒。见文件头最后一段。
    tracing::warn!(
        schedule = %id,
        "正在用 CLI 触发定时：若 mcloud-server 的调度器同时开着，同一条可能被跑两次"
    );

    // 真正执行。用调度器自己的 `tick`，而不是在这里重新拼一遍
    // "跑任务 + 记水位 + 记结果" —— 那正是"两处语义"的开端。
    //
    // `tick` 会处理**所有**到点的定时，而我们只要这一条。
    // 这是刻意的取舍：`tick` 里那套水位/并发/错过策略不该在 CLI 里重写。
    // 代价是同一条 crontab 下若有多条定时同时到点，它们会一起被处理 ——
    // 那其实是**正确**的（它们本来也就该跑），只是日志里会多几条。
    let results = app.handle.scheduler.tick().await.context("调度触发失败")?;

    let mut failed = false;
    let mut found = false;
    for (fired_id, outcome) in &results {
        if fired_id != &sid {
            continue;
        }
        found = true;
        println!("[{}] {}", fired_id, describe_outcome(outcome));
        if matches!(outcome, mcloud_engine::FireOutcome::UnknownTask(_)) {
            std::process::exit(EXIT_CONFIG);
        }
        // 只有在"真的跑了"时才可能失败；SkippedBusy 之类不算失败
        if let mcloud_engine::FireOutcome::Fired { run_id } = outcome {
            if let Some(rec) = app
                .handle
                .store
                .run(mcloud_core::RunId::new(*run_id))
                .await?
            {
                if rec.outcome.is_failure() {
                    failed = true;
                }
            }
        }
    }

    if !found {
        // 到点了却没被 tick 处理 —— 说明它没进"该跑"的集合。正常竞态
        // （另一个进程刚好抢先），退 0。
        println!("这次未执行：{id}（可能已被另一个进程认领）");
        return Ok(());
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn describe_outcome(o: &mcloud_engine::FireOutcome) -> String {
    use mcloud_engine::FireOutcome as F;
    match o {
        F::Fired { run_id } => format!("已触发，运行 #{run_id}"),
        // 编排没有单次运行的 run_id（它内部要跑 N 步），所以单独一句话说清。
        F::FiredPipeline => "已触发（编排）".into(),
        F::SkippedBusy => "跳过：同一任务+账号正在跑".into(),
        F::AbandonedMissed => "放弃：错过太久".into(),
        // ⚠ 不能写死"注册表里没有任务"：这个变体现在也承载**编排**那边的配置错误
        // （`body` 缺 `pipelineId`、编排不存在或不属于你），写死会得到
        // "注册表里没有任务 编排不存在或不属于这个用户：x" 这种骗人的句子。
        F::UnknownTask(t) => format!("配置错误：{t}"),
    }
}

fn fmt_ms(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => ms.to_string(),
    }
}

/// 把现有定时列出来 —— id 打错时这份清单最有用。
async fn print_schedules(app: &App) {
    match app.handle.store.schedules().await {
        Ok(list) if !list.is_empty() => {
            eprintln!();
            eprintln!("现有定时（{} 条）：", list.len());
            for s in list {
                let flag = if s.enabled { "" } else { " [已禁用]" };
                eprintln!("  {:<20} {:<16} {}{}", s.id, s.expr, s.name, flag);
            }
        }
        Ok(_) => eprintln!(
            "\n当前没有任何定时任务。在 WebUI 的「定时」页创建，或直接编辑 schedules 文件。"
        ),
        Err(e) => eprintln!("\n（读取定时列表失败：{e}）"),
    }
}
