//! 网盘签到任务。
//!
//! 这是最小可用的真实任务：走完「取状态 → 签到 → 汇报结果」的完整路径，
//! 作为其余任务的实现模板。

use async_trait::async_trait;
use serde_json::json;

use mcloud_core::{HttpRequest, Result, TaskOutcome};
use mcloud_engine::{Task, TaskContext, TaskResult};

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

    async fn run(&self, ctx: &TaskContext, body: &serde_json::Value) -> TaskResult {
        let dry_run = body
            .get("dryRun")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        ctx.progress(1, 2, "查询签到状态");

        if dry_run {
            // 演练模式：只探状态，不产生副作用
            let status = self.query_status(ctx).await?;
            ctx.info("演练模式：未提交签到");
            return Ok(TaskOutcome::success(format!(
                "演练完成，当前状态：{status}"
            )));
        }

        ctx.progress(2, 2, "提交签到");
        let summary = self.submit(ctx).await?;
        ctx.info(format!("签到结果：{summary}"));
        Ok(TaskOutcome::success(summary))
    }
}

impl SignTask {
    /// 查询签到状态。
    async fn query_status(&self, ctx: &TaskContext) -> Result<String> {
        let resp = ctx
            .http
            .request(
                HttpRequest::post("https://m.mcloud.139.com/ycloud/api/sign/query")
                    .json_body(&json!({})),
            )
            .await?;

        if !resp.is_success() {
            return Err(mcloud_core::Error::Network(format!(
                "签到状态查询失败，HTTP {}",
                resp.status
            )));
        }

        let v = resp.json()?;
        // 上游用 code 表达业务结果
        let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("");
        if !matches!(code, "0" | "0000" | "200") {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("未知错误");
            return Err(mcloud_core::Error::Upstream {
                code: code.to_string(),
                message: msg.to_string(),
            });
        }

        Ok(v.get("data")
            .and_then(|d| d.get("signed"))
            .and_then(|s| s.as_bool())
            .map(|signed| if signed { "已签到" } else { "未签到" })
            .unwrap_or("未知")
            .to_string())
    }

    /// 提交签到。
    async fn submit(&self, ctx: &TaskContext) -> Result<String> {
        let resp = ctx
            .http
            .request(
                HttpRequest::post("https://m.mcloud.139.com/ycloud/api/sign/doSign")
                    .json_body(&json!({})),
            )
            .await?;

        if !resp.is_success() {
            return Err(mcloud_core::Error::Network(format!(
                "签到请求失败，HTTP {}",
                resp.status
            )));
        }

        let v = resp.json()?;
        let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("");
        if !matches!(code, "0" | "0000" | "200") {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("未知错误")
                .to_string();
            // 已签到属于「跳过」而非失败
            if msg.contains("已签") || msg.contains("重复") {
                return Ok(TaskOutcome::skipped(msg).summary().to_string());
            }
            return Err(mcloud_core::Error::Upstream {
                code: code.to_string(),
                message: msg,
            });
        }

        let dou = v
            .get("data")
            .and_then(|d| d.get("cloudDou"))
            .and_then(|d| d.as_i64())
            .unwrap_or(0);
        Ok(format!("签到成功，获得 {dou} 云朵"))
    }
}
