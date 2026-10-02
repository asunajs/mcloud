//! 共享口令池的状态。
//!
//! # 为什么要有这个接口
//!
//! 直播页从"我填口令 → 去领"变成"**看池里有什么 → 去领**"（用户口径
//! 2026-10-01：口令全实例共享，够 2 条有效的全场就不必再抓）。界面要能回答
//! 三个问题，而且**必须由后端回答**（前端算不出来）：
//!
//! 1. 池里现在有几条、什么状态、从哪来；
//! 2. 够 2 条有效了没有（够了界面就能正大光明地写"不需要再抓"）；
//! 3. 这一场是什么时候开始的（周期）。
//!
//! 校验在服务端、判定也在服务端 —— 界面只负责显示。

use axum::extract::State;
use axum::Json;
use mcloud_core::PoolSummary;

use crate::error::ApiResult;
use crate::ServerState;

/// `GET /api/kouling`
///
/// ⚠ 它会**顺手归一化一次并落盘**：周期翻篇是"读的时候才发现该翻了"的
/// （本仓没有后台定时器去轮询），不写回的话界面会看到一份"过期但还在"的池，
/// 而任务那边读到的是新周期 —— 两边不一致，且这种不一致**没有任何迹象**。
/// 这个副作用只有这一条、而且是幂等的（同一个 `now` 下反复调用结果相同）。
pub async fn get_pool(State(state): State<ServerState>) -> ApiResult<PoolSummary> {
    let mut pool = state.handle.store.kouling_pool().await?;
    let cycle_before = pool.cycle_at_ms;
    pool.normalize(state.handle.clock.now_ms());
    if pool.cycle_at_ms != cycle_before {
        tracing::info!(
            old_cycle = cycle_before,
            new_cycle = pool.cycle_at_ms,
            "口令池周期已翻篇（读的时候才发现）"
        );
        state.handle.store.save_kouling_pool(&pool).await?;
    }
    Ok(Json(pool.summary()))
}
