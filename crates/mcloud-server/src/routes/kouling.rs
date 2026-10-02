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
    let now_ms = state.handle.clock.now_ms();
    let mut pool = state.handle.store.kouling_pool().await?;
    let cycle_before = pool.cycle_at_ms;
    pool.normalize(now_ms);
    if pool.cycle_at_ms != cycle_before {
        tracing::info!(
            old_cycle = cycle_before,
            new_cycle = pool.cycle_at_ms,
            "口令池周期已翻篇（读的时候才发现）"
        );
        state.handle.store.save_kouling_pool(&pool).await?;
    }
    // ⚠ 「下一场」必须**跳过已经过去的场次**（用户实测 2026-10-02：16:31 还显示 15:00 ✗）。
    //
    // 上游给的 `expectStartTime` 是**北京时间**的本地串（没有时区）⇒ 拿"现在"去比它时，
    // 必须用**同一个时区** ✗ —— 用机器时区的话，机器设在别处就会整体错几小时 ✗。
    // 所以这里固定 +08:00，与上游一致 ✓。
    let now_local = chrono::DateTime::from_timestamp_millis(now_ms)
        .map(|t| {
            let tz = chrono::FixedOffset::east_opt(8 * 3600).expect("固定 +08:00 一定合法");
            t.with_timezone(&tz)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        // 时间戳超出可表示范围（理论上到不了）⇒ 退回空串 ⇒ 筛不掉任何东西 ⇒
        // 也就是退回**旧行为**（宁可显示一个可能过去的时间，也不要整块变空 ✗）。
        .unwrap_or_default();
    Ok(Json(pool.summary(&now_local)))
}
