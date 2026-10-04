//! `GET https://mrp.mcloud.139.com/market/signin/page/doTask?client=app&deviceId=<n>`
//! —— 「从固定入口进一次」的埋点。
//!
//! 依据：旧 TS `packages/core/src/tasks/app/runners.ts:98-108`（玩法 409「从固定入口
//! 访问云朵中心」）与 `:126-133`（玩法 1005「从固定入口进入云朵中心」），逐字：
//!
//! ```ts
//! const deviceId = ctx.state.deviceId || "";
//! await tryRequest(async () => {
//!   await ctx.http.get(
//!     `https://mrp.mcloud.139.com/market/signin/page/doTask?client=app&deviceId=${encodeURIComponent(deviceId)}`,
//!   );
//! });
//! await sleep(500);
//! await clickTask(ctx, 409, "task2");   // 1005 那支不传第三参 ⇒ 默认 key "task"
//! ```
//!
//! # 两个刻意的形状
//!
//! * **失败不挡路**：TS 用 `tryRequest` 包着、结果**不看** ⇒ 这里只负责把请求构造对，
//!   让调用方把错误报成"埋点没打成"，而不是让整条任务失败 ✓。
//! * **`deviceId` 照带**（编码后 ✓）：本仓这个值就是 smid 换回来那一个
//!   （`mcloud_tasks::signin::h5_device_id` ✓），与 TS 的 `ctx.state.deviceId` 同一来源 ✓。
//!   `deviceId` 缺席时 TS 发的是**空串**（`|| ""` ✓）—— 照抄，不"顺手"省掉这一格 ✗。
//!
//! ⚠ 路径里那一段是 `page/doTask`（**不是** `signin/doTask`）—— 与 `task/click`
//! 不在同一个前缀下，别按邻居的写法猜 ✗。

use mcloud_core::{HttpRequest, Result};

use crate::app_signin::{h5, percent_encode};

/// 「固定入口」埋点的完整地址（旧 TS 两个玩法共用这一条 ✓）。
pub const ENTRY_DO_TASK_URL: &str = "https://mrp.mcloud.139.com/market/signin/page/doTask";

/// 构造 `GET …/page/doTask?client=app&deviceId=<n>`。
///
/// `device_id` 为 `None`/空 ⇒ 发 `deviceId=`（与旧 TS 的 `|| ""` 同形 ✓）。
pub fn entry_do_task_request(device_id: Option<&str>) -> Result<HttpRequest> {
    let url = format!(
        "{ENTRY_DO_TASK_URL}?client=app&deviceId={}",
        percent_encode(device_id.unwrap_or(""))
    );
    // `isdeviceid=false`：与同族的 `task/click` 保持同一口径 ✓（那一发也不是
    // "取设备号"那一类）—— 别顺手改成 `true`，多一枚头上游认不认没有样本 ✗。
    Ok(h5(false, HttpRequest::get(url), "true"))
}
