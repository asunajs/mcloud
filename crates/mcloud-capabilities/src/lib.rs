//! mcloud-capabilities —— 能力层。
//!
//! 原 TS 版本的能力散落在 `packages/api/src/tasks/*`（19 个 `TaskModule`）+ 前端硬编码表单里。
//! 这里改成**声明式注册表**：能力的入参与出参用 `FieldSpec` 描述，
//! 于是 WebUI 表单、API 校验、组合步骤参数、CLI `--help` 全部由声明生成。
//!
//! # 状态
//!
//! P0 只建立注册表与声明结构（本文件）。19 个能力的实现属 P4，
//! 搬迁顺序见方案 §7.1（查询型低风险 → 日常核心 → 活动类 → 高风险的 kouling / exchange）。
//!
//! # 约束
//!
//! 本 crate **禁止** IO：能力的执行依赖由 `mcloud-exec` 在 P4 通过泛型注入，
//! 执行入口（runId / 并发闸 / 事件流）不在本 crate 内。

#![forbid(unsafe_code)]

use mcloud_core::{AccountMode, CapabilityKind, FieldSpec, Group, RiskLevel};
use std::sync::LazyLock;

/// 能力声明。
///
/// `params` / `output` 用函数指针而不是常量，是为了让"默认值依赖配置"的情况
/// （例如账号下拉选项来自当前配置）可以在调用时求值，同时保持结构体可 `const` 构造。
pub struct Capability {
    /// 稳定 code，用于 API 路由 / 组合步骤 / 定时任务引用（如 `sign`）。
    pub code: &'static str,
    pub title: &'static str,
    pub group: Group,
    pub kind: CapabilityKind,
    pub description: &'static str,

    /// 在通用任务列表与编排选择器中隐藏（独立页面运行）。
    ///
    /// 现有唯一用例是 `exchange`：它有自己的奖品页与定时抢兑规则，
    /// 混在通用列表里既难用也危险（`RiskLevel::High`）。
    pub hidden: bool,
    /// 是否参与组合编排与批量执行。
    pub run_in_batch: bool,
    pub risk: RiskLevel,
    /// 多账号默认执行方式。**默认串行**，因为高并发容易触发风控。
    pub default_account_mode: AccountMode,

    pub params: fn() -> Vec<FieldSpec>,
    pub output: fn() -> Vec<FieldSpec>,
}

impl Capability {
    /// 前端表单是否应要求二次确认。
    pub const fn needs_confirmation(&self) -> bool {
        matches!(self.risk, RiskLevel::High)
    }
}

fn no_output() -> Vec<FieldSpec> {
    Vec::new()
}

fn account_selector() -> Vec<FieldSpec> {
    vec![
        FieldSpec {
            key: "accounts".into(),
            label: "账号".into(),
            kind: mcloud_core::FieldKind::MultiSelect,
            required: false,
            default: None,
            options: None, // 由服务端在 P5 按当前配置填充
            help: Some("留空 = 全部账号".into()),
        },
        FieldSpec {
            key: "accountMode".into(),
            label: "执行方式".into(),
            kind: mcloud_core::FieldKind::Select,
            required: false,
            default: Some(serde_json::json!("sequential")),
            options: Some(vec![
                mcloud_core::SelectOption {
                    label: "串行（默认，防风控）".into(),
                    value: serde_json::json!("sequential"),
                    help: None,
                },
                mcloud_core::SelectOption {
                    label: "并行".into(),
                    value: serde_json::json!("parallel"),
                    help: Some("高并发容易触发限流".into()),
                },
            ]),
            help: None,
        },
    ]
}

fn exchange_params() -> Vec<FieldSpec> {
    let mut params = account_selector();
    params.push(
        FieldSpec::text("prizeIdOrName", "奖品")
            .required()
            .with_help("奖品 ID 或名称，来自兑换列表"),
    );
    params.push(FieldSpec::bool("skipWait", "跳过等待", true).with_help("关闭后等到整点开抢窗口"));
    params
}

fn exchange_output() -> Vec<FieldSpec> {
    vec![
        FieldSpec::text("cloudCost", "消耗云朵"),
        FieldSpec::text("prizeName", "奖品名称"),
        FieldSpec::text("result", "结果"),
    ]
}

fn cloud_snapshot_output() -> Vec<FieldSpec> {
    vec![
        FieldSpec::text("cloudCurrent", "当前云朵"),
        FieldSpec::text("cloudDiff", "日增量"),
        FieldSpec::text("cloudDiffWeek", "周增量"),
        FieldSpec::text("updatedAt", "采集时间"),
    ]
}

/// 能力注册表。
///
/// 用显式列表而不是宏 —— 可 grep、可 diff，新增能力只改这一处。
/// P4 会补齐全部 19 项；这里先登记 3 项以验证声明式形态成立。
pub static REGISTRY: LazyLock<Vec<Capability>> = LazyLock::new(|| {
    vec![
        Capability {
            code: "sign",
            title: "每日签到",
            group: Group::Daily,
            kind: CapabilityKind::Action,
            description: "移动云盘每日签到并领取奖励",
            hidden: false,
            run_in_batch: true,
            risk: RiskLevel::Low,
            default_account_mode: AccountMode::Sequential,
            params: account_selector,
            output: no_output,
        },
        Capability {
            code: "exchange",
            title: "兑换奖品",
            group: Group::Exchange,
            kind: CapabilityKind::Action,
            description: "消耗云朵兑换奖品（独立页面运行）",
            hidden: true,
            run_in_batch: false,
            risk: RiskLevel::High, // 不可逆消耗云朵
            default_account_mode: AccountMode::Sequential,
            params: exchange_params,
            output: exchange_output,
        },
        Capability {
            code: "cloud-snapshot",
            title: "云朵余额快照",
            group: Group::Query,
            kind: CapabilityKind::Query, // 无副作用，可反复执行
            description: "采集账号云朵余额并写入日快照",
            hidden: false,
            run_in_batch: false,
            risk: RiskLevel::Low,
            default_account_mode: AccountMode::Sequential,
            params: account_selector,
            output: cloud_snapshot_output,
        },
    ]
});

/// 按 code 查找能力。
pub fn get(code: &str) -> Option<&'static Capability> {
    REGISTRY.iter().find(|c| c.code == code)
}

/// 可在通用任务列表中展示的能力（过滤掉 `hidden`）。
pub fn list_visible() -> Vec<&'static Capability> {
    REGISTRY.iter().filter(|c| !c.hidden).collect()
}

/// 可参与组合编排的能力。
pub fn list_batchable() -> Vec<&'static Capability> {
    REGISTRY
        .iter()
        .filter(|c| c.run_in_batch && !c.hidden)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for cap in REGISTRY.iter() {
            assert!(seen.insert(cap.code), "能力 code 重复: {}", cap.code);
        }
    }

    #[test]
    fn params_keys_are_unique_within_a_capability() {
        for cap in REGISTRY.iter() {
            let params = (cap.params)();
            let mut seen = std::collections::HashSet::new();
            for spec in &params {
                assert!(
                    seen.insert(spec.key.clone()),
                    "{} 的参数 key 重复: {}",
                    cap.code,
                    spec.key
                );
            }
        }
    }

    #[test]
    fn exchange_is_hidden_and_high_risk() {
        let exchange = get("exchange").expect("exchange 必须在注册表中");
        assert!(exchange.hidden, "兑换不应出现在通用任务列表");
        assert!(!exchange.run_in_batch, "兑换不应参与组合编排");
        assert!(exchange.needs_confirmation(), "消耗云朵必须二次确认");
    }

    #[test]
    fn exchange_prize_param_is_required() {
        let exchange = get("exchange").unwrap();
        let prize = (exchange.params)()
            .into_iter()
            .find(|f| f.key == "prizeIdOrName")
            .expect("应有奖品参数");
        assert!(prize.required, "奖品是必填项");
    }

    #[test]
    fn query_capabilities_are_not_batchable_and_are_visible() {
        let snapshot = get("cloud-snapshot").unwrap();
        assert_eq!(snapshot.kind, CapabilityKind::Query);
        assert!(!snapshot.run_in_batch, "查询型能力不进批量执行");
        assert!(!snapshot.hidden);
    }

    #[test]
    fn default_account_mode_is_sequential() {
        for cap in REGISTRY.iter() {
            assert_eq!(
                cap.default_account_mode,
                AccountMode::Sequential,
                "{} 默认应为串行以防风控",
                cap.code
            );
        }
    }

    #[test]
    fn visible_list_excludes_hidden() {
        let visible: Vec<&str> = list_visible().iter().map(|c| c.code).collect();
        assert!(visible.contains(&"sign"));
        assert!(!visible.contains(&"exchange"));
    }

    #[test]
    fn registry_serializes_for_frontend_consumption() {
        let cap = get("exchange").unwrap();
        let payload = serde_json::json!({
            "code": cap.code,
            "title": cap.title,
            "hidden": cap.hidden,
            "needsConfirmation": cap.needs_confirmation(),
            "params": (cap.params)(),
            "output": (cap.output)(),
        });
        let text = payload.to_string();
        assert!(text.contains("\"skipWait\""));
        assert!(text.contains("\"sequential\""));
        assert!(text.contains("\"cloudCost\""));
    }
}
