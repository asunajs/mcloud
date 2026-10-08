//! `mcloud run-all` —— **一键跑内置默认编排**。
//!
//! 用户口径（2026-10-07）：「实现一个默认的编排任务，让本项目能和 ts 版一样一键运行，
//! **cli 一条命令**，webui 一个定时」✓。
//!
//! # 与 `mcloud pipeline run daily` 的关系
//!
//! 两者跑的是**同一条**编排，执行体也是**同一个**（`cli/pipeline.rs` 的 `run_one` ✓）——
//! 差别只在"要不要先知道那个 id" ✓。**本模块不复制**账号解析、逐步播报、退出码那一套 ✓。
//!
//! ⚠ 库里那条 `daily` 是**服务启动时种的**（`AppHandle::seed_defaults` ✓，用户口径：
//! 「不自动新建怎么算默认」✓）。这里**不写库** ✗：库里有就跑库里那份（尊重用户改过的 ✓），
//! 没有（用户删了、或这台机器只跑 CLI）就跑内置预设 ✓ —— 命令行顺手往用户库里塞东西
//! 是被否掉的行为 ✓。
//!
//! # 参数
//!
//! 不收额外参数：目标账号就是"全部已启用账号"（与那条默认定时同一条口径 ✓）。
//! 想指定账号就用 `pipeline run <id> --account …` 那条路 ✓。

use mcloud_app::App;

use crate::cli::pipeline as cli_pipeline;

/// 跑内置默认编排。`args` 是 `run-all` 之后的参数（原样透传给 `pipeline run`）。
pub async fn run_all(app: &App, _args: &[String]) -> anyhow::Result<()> {
    // 默认编排是**每人一份**（`daily-<用户id>` ✓）⇒ 命令行要挑一份跑：
    // 取**最早那个用户**（= 最初的管理员，`owner::legacy_owner` ✓）自己那份 ——
    // 与"CLI 是本机管理员工具"这个定位一致 ✓。
    let users = app.handle.store.users().await?;
    let owner = mcloud_core::owner::legacy_owner(&users);
    let found = match &owner {
        Some(u) => {
            let id =
                mcloud_core::PipelineId::new(mcloud_core::pipeline::default_pipeline_id(Some(u)));
            app.handle.store.pipeline(&id).await?
        }
        None => None,
    };
    let pipeline = match found {
        // 库里有就用**库里那份** —— 他可能改过它（去掉几步、改名），
        // 那一份才是他认可的"每天该跑的" ✓。
        Some(p) => p,
        // 库里没有（还没登录过、或他删了）⇒ 用**内置预设**直接跑，**不写库** ✗ ——
        // 命令行不该顺手往用户库里塞东西 ✓。
        None => mcloud_core::pipeline::default_pipeline(None),
    };
    println!("默认编排：{}（{} 步）", pipeline.name, pipeline.steps.len());
    cli_pipeline::run_one(app, &pipeline).await
}
