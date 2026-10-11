//! 运行日志路由（**分页** ✓）。
//!
//! 用户口径（2026-10-09）：「大日志做虚拟加载或者分页，避免一次加载太多」✓ ——
//! 一次运行的日志可以有上万行（上限由设置决定 ✓），一次性全给前端既慢又白给
//! （用户一屏只看十几行 ✓）。所以这里按 [`LogPageQuery`] 取页 ✓：
//!
//! | 参数 | 语义 |
//! |---|---|
//! | 都不给 | **最后** `limit` 行（"打开就是最近这些" ✓） |
//! | `before=<seq>` | `seq < before` 的最后 `limit` 行（**往上翻页** ✓） |
//! | `after=<seq>` | `seq > after` 的最前 `limit` 行（断线续传 ✓，既有语义） |
//!
//! ⚠ 返回条数 < `limit` 就说明**到头了**（前端据此收起"加载更早" ✓）——
//! 所以**别**为了"补齐"而返回空行 ✗。

use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use serde::Deserialize;

use mcloud_core::{log_line_of, slice_page, LogLine, LogPageQuery, RunId, Seq};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::routes::runs::{run_access, RunAccess};
use crate::ServerState;

/// 一页默认多少行。
///
/// 500 行的体量 ≈ 60 KB（实测均 119 B/行 ✓）：一次请求就够铺满好几屏，
/// 而"看更早的"是**用户主动**的一下（不会因为打开页面就拉走几万行 ✓）。
const DEFAULT_PAGE: usize = 500;

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// 客户端**已经看到**的序号，用于断线续传。
    ///
    /// 语义是"已看到 n"，返回 n 之后的。
    /// 混淆成"下一个要取"会**静默跳过一条**。
    #[serde(default)]
    pub after: Option<u64>,
    /// 往上翻页的游标：返回 `seq < before` 的最后 `limit` 行 ✓。
    #[serde(default)]
    pub before: Option<u64>,
    /// 这一页最多几行（缺省 [`DEFAULT_PAGE`] ✓）。
    ///
    /// ⚠ 会被夹到**设置里那一格**（`AppSettings::log_limit_per_run` ✓）——
    /// 它是单次运行的日志总量上限，一页比它还大没有意义 ✓。
    #[serde(default)]
    pub limit: Option<usize>,
}

/// 取某次运行的一页日志。
///
/// 内存里有的优先（最新），没有则回落到存储层。
///
/// 返回 **404**（运行记录不存在）与"存在但暂无日志"是两回事：
/// 前者该报错，后者是正常运行状态（刚启动还没打日志）。
pub async fn list(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Path(id): Path<u64>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Vec<LogLine>> {
    let run_id = RunId::new(id);

    // ⚠⚠ 2026-10-06 审计 `SEC-20`：可见性口径必须与 `/api/runs/{id}/events` **一致**。
    //
    // 旧代码只查**存储层**（`store.run_for`）⇒ 一条**正在跑**的 run 一律 404
    //（运行记录是**结束时**才落盘的），而同一时刻 `/events` 是 200 ——
    // 同一个 run、两个端点给出相反的答案 ✗。方向虽然是失败关闭（不泄露），
    // 但"运行中读不到日志"是**功能损失**：前端想边跑边看日志就只能走 SSE。
    //
    // `run_access` 三处都看（本服务发起登记 / 存储层 / 内存中枢），并且
    // "别人的"与"压根不存在"回**同一个 404**（不泄露存在性）✓。
    match run_access(&state, &cur.0, run_id).await? {
        RunAccess::Allowed => {}
        RunAccess::Denied | RunAccess::Unknown => {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "运行记录不存在：{id}"
            ))));
        }
    }

    // 上限取**设置**里那一格（用户可设 ✓）：一页不该比"一次运行能留的全部"还大 ✓
    let per_run = state.handle.store.settings().await?.log_limit_per_run;
    let page = LogPageQuery {
        after: q.after.map(Seq::seen),
        before: q.before.map(Seq::seen),
        limit: q.limit.unwrap_or(DEFAULT_PAGE).clamp(1, per_run),
    };

    // 内存历史优先：它还包含**尚未落盘**的那部分（落盘是 2 秒攒批的 ✓）。
    //
    // ⚠ 往上翻页（`before`）**例外**，直接走存储层：那些行早就落盘了 ✓，
    // 而内存里只留最近 `HISTORY_PER_RUN` 条 ⇒ 翻老行本来就得靠盘 ✓
    //（两条路的判据是**同一份** `slice_page` ✓，不会给出不同的页 ✓）。
    if q.before.is_none() {
        let since = q.after.map(Seq::seen).unwrap_or(Seq::ZERO);
        let in_memory: Vec<LogLine> = state
            .handle
            .hub
            .replay(run_id, since)
            .into_iter()
            // ⚠ 判据只有一份（`log_line_of` ✓）：内存这一路与盘上那一路必须给出
            // **同样的行**（含进度行与分类 ✓）—— 各写一份的话，"刚跑完看到的"
            // 与"重启后看到的"就会不一样 ✗。时刻取**事件自己的**（不是写死 0 ✓）。
            .filter_map(|s| log_line_of(&s.event, s.seq))
            .collect();

        if !in_memory.is_empty() {
            return Ok(Json(slice_page(&in_memory, &page).lines));
        }
    }

    // 回落到存储层（进程重启后、或事件已被回收）✓ —— 它按需读盘，不会
    // 为了这一页把整个日志文件读进内存 ✗（见 `JsonStore::logs_page` ✓）。
    Ok(Json(
        state.handle.store.logs_page(run_id, page).await?.lines,
    ))
}
