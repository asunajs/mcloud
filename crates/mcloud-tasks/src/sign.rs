//! 网盘签到：查状态 → 提交签到 → **回查状态确认**。
//!
//! # 端点按实拍，不按旧 TS
//!
//! 旧项目写的是 `/market/signin/page/startSignIn`，而实拍是
//! **`/ycloud/signin/page/startSignIn`**（前缀不同）。这类"照着旧代码写就错"
//! 的地方在 mcloud 的 `docs/business-feature-inventory.md` §5.1 有专门一节。
//!
//! # ⚠ 判据是 `infoV3`，不是 `startSignIn` 的响应
//!
//! mcloud 的实测记录：
//!
//! > `startSignIn` 响应里的 `cal` 与 `todaySignIn` **会骗人** ——
//! > 实测两次，`startSignIn` 说没签，但紧接着 `infoV3` 已经是 `s:true` 了。
//! > 判成败一律回查 `infoV3`。
//!
//! 所以本任务的流程是"提交后**再查一次**"，用第二次 `infoV3` 的
//! `cal[d == 今天].s` 作为唯一判据。只信 `startSignIn` 的实现会把
//! 一次成功的签到报成失败 —— 而用户看到失败又点一次，就是重复请求。
//!
//! # 旧实现的坑：全链吞错
//!
//! 旧项目每个失败分支只打日志、从不抛错 ⇒ 对 runner 永远"成功"，
//! 于是定时任务能在"天天报错"的情况下绿灯一个月。这里失败一律
//! 返回 `Err` 或 `TaskOutcome::failed`。

use async_trait::async_trait;
use serde_json::Value;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskResult};

/// 签到状态查询（**权威判据**）。
pub const INFO_V3: &str = "https://m.mcloud.139.com/ycloud/signin/page/infoV3";

/// 执行签到。
pub const START_SIGN_IN: &str = "https://m.mcloud.139.com/ycloud/signin/page/startSignIn";

pub struct SignTask;

#[async_trait]
impl Task for SignTask {
    fn name(&self) -> &'static str {
        "sign"
    }

    fn title(&self) -> &'static str {
        "网盘签到"
    }

    fn description(&self) -> &'static str {
        "每日签到并领取云朵奖励"
    }

    fn params(&self) -> Vec<String> {
        vec!["dryRun".into()]
    }

    async fn run(&self, ctx: &TaskContext, body: &Value) -> TaskResult {
        let dry_run = body.get("dryRun").and_then(Value::as_bool).unwrap_or(false);

        ctx.progress(1, 3, "查询签到状态");
        let before = self.query_status(ctx).await?;
        ctx.info(format!(
            "当前余额 {}，本月已签 {} 天{}",
            before.total,
            before.month_days,
            if before.signed_today {
                "，今日已签到"
            } else {
                ""
            }
        ));

        if before.signed_today {
            return Ok(TaskOutcome::success(format!(
                "今日已签到，余额 {}",
                before.total
            )));
        }

        if dry_run {
            ctx.info("演练模式：未提交签到");
            return Ok(TaskOutcome::success(format!(
                "演练完成，当前余额 {}，今日未签到",
                before.total
            )));
        }

        ctx.check_cancelled()?;

        ctx.progress(2, 3, "提交签到");
        self.submit(ctx).await?;

        // ⚠ 提交的响应**不作为判据** —— 回查才是。
        // 少这一步的实现会把成功的签到报成失败。
        ctx.progress(3, 3, "回查签到结果");
        let after = self.query_status(ctx).await?;

        if !after.signed_today {
            return Ok(TaskOutcome::failed(format!(
                "提交签到后回查仍未签到（余额 {}）",
                after.total
            )));
        }

        let gained = after.total - before.total;
        let msg = if gained > 0 {
            format!("签到成功，获得 {gained} 云朵，余额 {}", after.total)
        } else {
            // 已签过或服务端延迟入账：签到状态是真的，但余额没动。
            // 不报失败 —— 状态是权威的；把数字如实说出来。
            format!(
                "签到成功（余额未变，当前 {}；可能是补签或延迟入账）",
                after.total
            )
        };
        ctx.info(&msg);
        Ok(TaskOutcome::success(msg))
    }
}

/// `infoV3` 里我们关心的字段。
#[derive(Debug, Clone, PartialEq)]
pub struct SignStatus {
    /// 现行余额（豆）。
    pub total: i64,
    /// 本月已签天数。
    pub month_days: i64,
    /// 今日是否已签 —— **唯一判据**。
    pub signed_today: bool,
    /// 连续签到天数。
    pub sign_count: i64,
}

impl SignTask {
    /// 查签到状态。
    async fn query_status(&self, ctx: &TaskContext) -> Result<SignStatus> {
        let resp = ctx
            .http
            .request(HttpRequest::get(format!("{INFO_V3}?client=app")))
            .await?;
        parse_status(&resp.body)
    }

    /// 提交签到。
    ///
    /// 真实流量带 `deviceId` 查询参数与 `isdeviceid: true` 头。
    /// `deviceId` 是 base64，含 `+` `/` `=` —— **必须百分号编码**：
    /// `+` 在查询串里表示空格，不编码会被服务端解成另一个值
    /// （而且看起来"没报错"，只是行为不对）。
    async fn submit(&self, ctx: &TaskContext) -> Result<()> {
        let device_id = ctx.account.device_id.as_str();
        let url = format!(
            "{START_SIGN_IN}?client=app&deviceId={}",
            percent_encode(device_id)
        );
        let resp = ctx
            .http
            .request(HttpRequest::get(url).header("isdeviceid", "true"))
            .await?;

        // 只判业务码。`cal` 与 `todaySignIn` **不用** —— 它们会骗人，
        // 权威判据是回查 `infoV3`。
        let v: Value = serde_json::from_str(&resp.body)
            .map_err(|e| mcloud_core::Error::Parse(format!("startSignIn 响应不是 JSON：{e}")))?;
        ensure_code_zero(&v, "startSignIn")?;
        Ok(())
    }
}

/// 解析 `infoV3` 的响应。
///
/// 判据写法（mcloud 实拍口径）：`cal` 里找 `t == true`（今天）那一格，看 `s`。
/// 也接受 `currentMonth == 1 && d == 今天`，两条路互为印证。
pub fn parse_status(body: &str) -> Result<SignStatus> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| mcloud_core::Error::Parse(format!("infoV3 响应不是 JSON：{e}")))?;
    ensure_code_zero(&v, "infoV3")?;

    let result = v
        .get("result")
        .ok_or_else(|| mcloud_core::Error::Parse("infoV3 响应没有 result".into()))?;

    // `total` 才是现行余额。`beforeTotal` **不是** —— 它是明细页口径的另一个数，
    // 领取后不动。抄错这一个字段的表现是"签到成功但余额没变"。
    let total = get_i64(result, "total").unwrap_or(0);
    let month_days = get_i64(result, "monthDays").unwrap_or(0);
    let sign_count = get_i64(result, "signCount").unwrap_or(0);

    let signed_today = result
        .get("cal")
        .and_then(Value::as_array)
        .map(|days| {
            days.iter().any(|d| {
                // 优先用 `t`（"今天"标记）；退而用 currentMonth + 日期比对
                let is_today = d.get("t").and_then(Value::as_bool).unwrap_or(false);
                let this_month = d.get("currentMonth").and_then(Value::as_i64) == Some(1);
                is_today && this_month && d.get("s").and_then(Value::as_bool).unwrap_or(false)
            })
        })
        .unwrap_or(false);

    Ok(SignStatus {
        total,
        month_days,
        signed_today,
        sign_count,
    })
}

/// 业务码非 0 即失败。
///
/// `code` 可能是数字 `0` 也可能是字符串 `"0"` —— 两种都要认。
fn ensure_code_zero(v: &Value, what: &str) -> Result<()> {
    let ok = match v.get("code") {
        Some(Value::Number(n)) => n.as_i64() == Some(0),
        Some(Value::String(s)) => s == "0" || s == "0000",
        // 没有 code 字段时看 success（部分接口这样）
        None => v.get("success").and_then(Value::as_bool).unwrap_or(false),
        Some(Value::Null) => v.get("success").and_then(Value::as_bool).unwrap_or(false),
        Some(other) => {
            return Err(mcloud_core::Error::Parse(format!(
                "{what} 的 code 类型意外：{other}"
            )));
        }
    };
    if !ok {
        let msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("(无消息)");
        let code = v.get("code").map(|c| c.to_string()).unwrap_or_default();
        // 会话失效单独说清楚：它不是任务失败，是票的问题
        if mcloud_upstream::is_session_expired(&mcloud_core::HttpResponse {
            status: 200,
            body: v.to_string(),
            headers: Vec::new(),
        }) {
            return Err(mcloud_core::Error::Other(format!(
                "{what} 回会话失效（code={code}）—— {}",
                mcloud_upstream::describe(mcloud_upstream::session_source::CODE_SESSION_EXPIRED)
            )));
        }
        return Err(mcloud_core::Error::Other(format!(
            "{what} 失败：code={code} msg={msg}"
        )));
    }
    Ok(())
}

fn get_i64(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        // 上游有的字段是字符串形式的数字
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// 查询串值的百分号编码。
///
/// `deviceId` 是 base64，含 `+` `/` `=`。**`+` 在查询串里表示空格** ——
/// 不编码就会被服务端解成另一个值，而且看起来"没报错"，只是行为不对。
/// 抓包里这三个字符正是 `%2B` / `%2F` / `%3D`。
///
/// 只编码"会破坏查询串语义"的字符，其余原样保留 —— 与抓包一致
/// （抓包里字母数字都没被编码）。
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests;
