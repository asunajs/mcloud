//! `GET /ycloud/signin/task/click?key=<key>&id=<n>` —— 「这条任务我做完了」。
//!
//! # ⚠ 这是**写**请求，而且不可回滚
//!
//! 它在告诉上游"这条任务做过了"，云朵可能**当场入账** ⇒ 没有"撤销打卡"这一口。
//! 所以调用方在发它之前必须已经真的做了那件事（传了文件、建了笔记……），
//! 而不是拿它当"试试看会不会给"的探针。
//!
//! 依据：旧 TS `packages/core/src/tasks/app/helpers.ts` 的 `clickTask`
//! （`GET …/task/click?key=${taskStr ?? "task"}&id=${id}`，判据 `result.code === 0`）。
//!
//! # `id` 必须为正 —— 不给"发一发 `id=0` 试试"留口子
//!
//! 旧实现没有"id 为 0 也要发"的调用点。而发一发 `id=0` 的打卡是**拿用户的账
//! 去试探上游的容错** —— 那种请求即使上游回了 `code:0`，也说明不了任何事。
//! ⇒ 在这里就挡成 `Err`，别让它有机会出网。

use serde_json::Value;

use mcloud_core::{Error, HttpRequest, Result};

use crate::app_signin::{h5, percent_encode, SignInEnvelope};
use crate::endpoints::MCLOUD_HOST;
use crate::live_consts::BASE_MCLOUD;

/// `task/click` 的路径。
pub const TASK_CLICK_PATH: &str = "/ycloud/signin/task/click";

/// 默认的 `key`（旧实现 `${taskStr ?? "task"}` 的兜底值）。
pub const TASK_CLICK_DEFAULT_KEY: &str = "task";

/// 构造打卡请求。
///
/// `id` 是任务的数字 id。
///
/// ⚠ `key` **不是**任务分组（从前这里写成 `day` / `month` / `time` 那几组 ✗，没有依据）。
/// 参考实现默认是 [`TASK_CLICK_DEFAULT_KEY`]（`helpers.ts:10` 的 `taskStr ?? "task"`），
/// 只有个别任务用专用值：`409 → task2`（`runners.ts:107`）、`478/481 → randomCloudTask`
/// （`index.ts:51,54`）；挑选逻辑在 `mcloud_tasks::app_task` 的 `click_key`。
///
/// ⚠ `key` 走 `percent_encode`：它是查询串里的一格 —— "少一层编码"的表现是上游回一个
/// **业务码**，不像"我们拼错了 URL"。
/// `POST {BASE_MCLOUD}/ycloud/visitlog/journaling` —— 云盘侧的**访问日志**（"点了哪条任务"）。
///
/// # 为什么要有这一发（2026-10-11 真机实拍）
///
/// 真机上点一条任务的"打卡"其实是**两发**（抓包仓 `app-task-run-account-samples.txt` §12.6 ✓）：
///
/// ```text
/// 08:32:43  GET  m.mcloud.139.com/ycloud/signin/task/click?key=task&id=1029
/// 08:32:43  POST m.mcloud.139.com/ycloud/visitlog/journaling
///           体: module=uservisit&optkeyword=139newsignin_index_task_1029&sourceid=1077&marketName=newsign_139mail
///           → {"code":0,"msg":"success"}
/// ```
///
/// 同一份抓包里 `optkeyword` 一共四个取值：`…_index_pv` / `…_index_client` /
/// `…_index_app_client`（**进页面就有** ✓）与 **`…_index_task_<id>`（只在点那一条时出现 ✓）**。
/// ⇒ 本仓此前**只有** `task/click` 那一发 ✗，这一发是**缺的那一半** ✓。
///
/// ⚠ `activityid` 请求头跟着 `marketName` 走（与 [`crate::app_task_list_v2`] 那口的规矩一致 ✓）；
/// 体里 `sourceid=1077` 是实拍那一格的值（`139newsignin` 那个页面的来源号 ✓）。
pub fn visitlog_journaling_request(task_id: i64, marketname: &str) -> Result<HttpRequest> {
    if task_id <= 0 || marketname.is_empty() {
        return Err(Error::Config(format!(
            "访问日志需要正整数任务 id 与非空 marketname，实得 id={task_id} marketname={marketname:?}"
        )));
    }
    let body = format!(
        "module=uservisit&optkeyword=139newsignin_index_task_{task_id}\
         &sourceid=1077&marketName={}",
        percent_encode(marketname)
    );
    // ⚠ `h5()` 会先写死 `activityid: sign_in_3` 与 `appversion: 12.6.0.0` ✗ ——
    //   照 [`crate::app_task_list_v2`] 那口的既定做法**先剥再补** ✓
    //   （[`HttpRequest::header`] 是 `push`，同名两枚头会让"哪枚生效"变成传输层的事 ✗）。
    let mut req = h5(
        false,
        HttpRequest::post(format!("{BASE_MCLOUD}/ycloud/visitlog/journaling")),
        "true",
    );
    req.headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("activityid") && !k.eq_ignore_ascii_case("appversion")
    });
    let req = req
        .header("activityid", marketname)
        .header("appversion", "11.4.1.0")
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded;charset=UTF-8",
        )
        .text_body(body);
    Ok(req)
}

pub fn task_click_request(key: &str, task_id: i64) -> Result<HttpRequest> {
    if task_id <= 0 {
        return Err(Error::Config(format!(
            "打卡需要正整数任务 id，实得 {task_id}（不发 id=0 的试探请求）"
        )));
    }
    let key = if key.is_empty() {
        TASK_CLICK_DEFAULT_KEY
    } else {
        key
    };
    let url = format!(
        "{MCLOUD_HOST}{TASK_CLICK_PATH}?key={}&id={task_id}",
        percent_encode(key)
    );
    // `isdeviceid=false`：这一发不是"取设备号"那一类，照同族既有口径
    // （`h5(false, …)`），别顺手改成 `true` —— 多一枚 `isdeviceid` 头
    // 上游认不认没有样本。
    Ok(h5(false, HttpRequest::get(url), "true"))
}

/// `task/click` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskClick {
    /// `result.msg` —— 给人看的那一句（旧实现在"开启通知"那条里会把它打进日志）。
    ///
    /// ⚠ 用 `Option` 分开"上游没给这一格"与"给了空串"：四发实拍里
    /// **没有任何一条带回 `result`**（全是 `{"code":0,"msg":"success"}`），
    /// 所以这一格今天恒为 `None`。声明它只是因为旧实现会读它。
    pub notice: Option<String>,
    /// 顶层 `msg`（实拍恒 `success`）。
    ///
    /// ⚠ 这是**信封**那一句，不是结论：`code != 0` 时它也可能是人话，
    /// 所以它不参与成败判定（那个由 [`SignInEnvelope::ensure_ok`] 做）。
    pub message: String,
}

/// 解析打卡响应。
pub fn parse_task_click(env: &SignInEnvelope) -> Result<TaskClick> {
    env.ensure_ok("打卡（task/click）")?;
    let notice = env
        .result
        .as_ref()
        .and_then(|v| v.get("msg"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(TaskClick {
        notice,
        message: env.message.clone(),
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod visitlog_tests {
    use super::*;

    /// 真机实拍的那一发（抓包仓 `app-task-run-account-samples.txt` §12.6 ✓）：
    /// URL 落在 `m.mcloud.139.com/ycloud/visitlog/journaling` ✓、
    /// 体是**表单**（不是 XML ✗）：`module=uservisit&optkeyword=139newsignin_index_task_<id>&sourceid=1077&marketName=<市场>` ✓、
    /// `activityid` 请求头跟着市场走 ✓。
    #[test]
    fn visitlog_journaling_matches_the_captured_shape() {
        let req = visitlog_journaling_request(1029, "newsign_139mail").expect("拼得出");
        assert_eq!(req.method, "POST");
        assert!(
            req.url.ends_with("/ycloud/visitlog/journaling"),
            "URL 不对：{}",
            req.url
        );
        // 体是**表单**（逐字对齐实拍 ✓）。
        let body = String::from_utf8(req.body.clone().unwrap_or_default()).expect("体是 UTF-8");
        assert_eq!(
            body,
            "module=uservisit&optkeyword=139newsignin_index_task_1029&sourceid=1077&marketName=newsign_139mail"
        );
        // ⚠ 同名头**只能一枚**：`h5()` 会先写死 `activityid: sign_in_3` / `appversion: 12.6.0.0`，
        //   本函数必须**先剥再补**（两枚同名头会让"哪枚生效"变成传输层的事 ✗）。
        let pick = |name: &str| -> Vec<String> {
            req.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
                .collect()
        };
        assert_eq!(pick("activityid"), vec!["newsign_139mail".to_string()]);
        assert_eq!(pick("appversion"), vec!["11.4.1.0".to_string()]);
        assert!(
            pick("content-type")
                .iter()
                .any(|v| v.contains("x-www-form-urlencoded")),
            "Content-Type 不对：{:?}",
            pick("content-type")
        );
    }

    /// 拿不到 id / 市场名时**不拼半成品**（宁可报错，也不发一条缺格的上报 ✗）。
    #[test]
    fn visitlog_journaling_refuses_incomplete_input() {
        assert!(visitlog_journaling_request(0, "newsign_139mail").is_err());
        assert!(visitlog_journaling_request(1029, "").is_err());
    }
}
