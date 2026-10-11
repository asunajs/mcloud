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

// ══════════════════ 同一条埋点的**新路径**（2026-10-10 加） ══════════════════

/// 云朵中心那一页自己发的「我打开了」（**新路径**）。
///
/// 与 [`ENTRY_DO_TASK_URL`] 是**同一件事**的迁移前后两态，两条今天都活着：
///
/// | 路径 | 出处 | 形状 |
/// |---|---|---|
/// | `POST https://m.mcloud.139.com/ycloud/signin/page/doTaskPost` | **App 实拍**（12.6.0，`../mcloud-capture/docs/capture/app-signin-samples.txt` §3.4 与 §6：H5 拦截器给这一发注入 `deviceId`） | JSON 体两键 + `isdeviceid: true` |
/// | `GET https://mrp.mcloud.139.com/market/signin/page/doTask` | 旧 TS `runners.ts:98-108`；Go 探针 2026-09-26 实测回 `{"code":0,"msg":"success"}` | 查询串两键，**不带** `isdeviceid` |
///
/// ⚠ 加这一发的动因：旧路径那一发是 **TS 时代的形状**，而 App 12.6.0 自己走的是这条
/// （2026-10-10 补齐，按「抓包 > TS > 推断」的优先级 ✓）。
///
/// ⚠⚠ **别把 409 / 1005 翻账的功劳记在它头上**（同一天实测过）：只补这一发、不改打卡 key 时，
/// 三个账号仍然 `WAIT → WAIT`（run 289 / 290）；真正让它们翻 `FINISH` 的是**打卡 key** 那一格
/// —— `click()` 原先两处共用一个 `click_key(id)` ⇒ 409 把 `task2` 打了**两发** ✗，
/// 而参考实现是 `task`（先打卡）→ `task2`（补发）✓。见 `crates/mcloud-tasks/src/app_task.rs`
/// 的 `click_with_key` 与 `docs/app-task-status.md` §5-9。
/// 两条上报都留着：删哪一条都得先有"它没用"的读数，而现在**没有** ✓。
pub const DO_TASK_POST_URL: &str = "https://m.mcloud.139.com/ycloud/signin/page/doTaskPost";

/// 构造 `POST …/ycloud/signin/page/doTaskPost`，体两键 `client` / `deviceId`。
///
/// - 体**逐字**来自实拍：`{"client":"app","deviceId":"<89 字符的 smid 设备号>"}`；
///   JSON 里 `+` / `/` / `=` **不转义**（同一个值在 query string 里才转义成 `%2B` / `%2F`，
///   见 [`entry_do_task_request`]）—— 别顺手改成"统一编码" ✗。
/// - `isdeviceid: true`：实拍里这一发**有**它（H5 拦截器只给标了 `isDeviceId` 的请求注入
///   `deviceId`）⇒ 与旧路径那一发的 `false` **不同**，两边各按各的实拍。
/// - `market_name`：这一发要冒充**哪一页**。`activityid` 跟着本次的 `marketname` 走，
///   与 [`crate::app_task_list_v2::task_list_v2_request`] 同一条规矩；`None` ⇒ 用 `h5()`
///   的默认（签到页 `sign_in_3`）。
///   ⚠ 后压的那一条胜（[`crate::h5_client`] 的 `dedup_headers` 取**最后**一条）。
pub fn do_task_post_request(
    device_id: Option<&str>,
    market_name: Option<&str>,
) -> Result<HttpRequest> {
    let body = serde_json::json!({
        "client": "app",
        "deviceId": device_id.unwrap_or(""),
    });
    let req = h5(
        true,
        HttpRequest::post(DO_TASK_POST_URL).json_body(&body),
        "true",
    );
    Ok(match market_name {
        Some(m) => req.header("activityid", m),
        None => req,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// 新路径：方法 / 地址 / 体 / `isdeviceid` 四格逐条钉住。
    #[test]
    fn do_task_post_matches_the_capture() {
        let req = do_task_post_request(Some("BAAA="), None).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(
            req.url,
            "https://m.mcloud.139.com/ycloud/signin/page/doTaskPost"
        );
        assert_eq!(
            req.body_str().unwrap(),
            r#"{"client":"app","deviceId":"BAAA="}"#,
            "体两键、JSON 里不转义"
        );
        assert_eq!(header(&req, "isdeviceid"), Some("true"));
        assert_eq!(
            header(&req, "activityid"),
            Some("sign_in_3"),
            "缺省落签到页"
        );
    }

    /// ⭐ 同一个值两种编码：query 里转义、JSON 体里**不**转义 —— 别混用。
    #[test]
    fn the_same_device_id_is_encoded_two_ways_on_purpose() {
        let raw = "B+/=";
        let old = entry_do_task_request(Some(raw)).unwrap();
        assert!(
            old.url.contains("deviceId=B%2B%2F%3D"),
            "query 里要转义：{}",
            old.url
        );
        let new = do_task_post_request(Some(raw), None).unwrap();
        assert!(
            new.body_str().unwrap().contains(r#""deviceId":"B+/=""#),
            "JSON 体里原样：{}",
            new.body_str().unwrap()
        );
    }

    /// 设备号缺席 ⇒ 两发都发**空串**（与旧 TS 的 `|| ""` 同形），不是省掉这一格。
    #[test]
    fn a_missing_device_id_still_sends_the_key() {
        let req = do_task_post_request(None, None).unwrap();
        assert_eq!(req.body_str().unwrap(), r#"{"client":"app","deviceId":""}"#);
        let old = entry_do_task_request(None).unwrap();
        assert!(
            old.url.ends_with("deviceId="),
            "旧那一发同样带空串：{}",
            old.url
        );
    }

    /// 指定市场名 ⇒ `activityid` 被**压成**它（不是并列两个）。
    #[test]
    fn a_market_name_overrides_the_activity_id() {
        let req = do_task_post_request(Some("BAAA="), Some("newsign_139mail")).unwrap();
        let ids: Vec<&str> = req
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("activityid"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(
            ids.len(),
            2,
            "h5() 那条 + 后压的一条，由传输层去重取最后一条"
        );
        assert_eq!(ids.last(), Some(&"newsign_139mail"));
    }
}
