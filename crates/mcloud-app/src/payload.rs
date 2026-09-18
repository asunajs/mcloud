//! 能力注册表与版本信息 → **前端要吃的 JSON**。
//!
//! # 为什么值得单独一层
//!
//! 方案里"新增能力只改注册表、前端零改动"是一个**可验证的断言**：
//! 只要 payload 完全由 `REGISTRY` 推导、没有任何一处硬编码能力名，
//! 那么新增一个能力就不可能牵动服务层与前端。把它写成单测就等于把这个承诺钉住了
//! （见 `adding_a_capability_changes_only_the_registry`）。
//!
//! # 契约
//!
//! 前端不消费 Rust 结构体，只消费这里的 JSON 形状。因此形状一旦改动，
//! 产物层的 OpenAPI `--check` 会立刻发现（防止前后端类型静默漂移）。
//!
//! 这个模块原本在 `mcloud-server` 里 —— 但 CLI 也要打印能力注册表，
//! 而 CLI 不该为了打个表把 axum 拖进来（见 crate 头的说明）。

use mcloud_capabilities::{Capability, REGISTRY, list_batchable, list_visible};
use mcloud_core::{CapabilityKind, Group, RiskLevel};
use serde_json::{Value, json};

/// 单个能力的对外描述。
///
/// 前端只需按 `params` / `output` 渲染表单与结果卡片，
/// 因此新增能力**不需要改前端任何一行**。
pub fn capability_payload(cap: &Capability) -> Value {
    json!({
        "code": cap.code,
        "title": cap.title,
        "group": group_name(cap.group),
        "kind": kind_name(cap.kind),
        "description": cap.description,
        "hidden": cap.hidden,
        "runInBatch": cap.run_in_batch,
        "risk": risk_name(cap.risk),
        "needsConfirmation": cap.needs_confirmation(),
        "defaultAccountMode": account_mode_name(cap.default_account_mode),
        "params": (cap.params)(),
        "output": (cap.output)(),
    })
}

/// `/api/capabilities` 的响应体。
pub fn capabilities_payload() -> Value {
    json!({
        "all": REGISTRY.iter().map(capability_payload).collect::<Vec<_>>(),
        "visible": list_visible().iter().map(|c| c.code).collect::<Vec<_>>(),
        "batchable": list_batchable().iter().map(|c| c.code).collect::<Vec<_>>(),
        "groups": group_names(),
    })
}

/// `/api/version` 的响应体 —— 取代原构建期的 `__APP_VERSION__` 注入。
pub fn version_payload() -> Value {
    json!({
        // 对外暴露的必须是**产品名**，而不是"碰巧实现它的 crate"的名字
        "name": mcloud_core::PRODUCT_NAME,
        // 版本号取自本 crate 的 Cargo.toml：产品版本在 workspace 里统一管理
        "version": env!("CARGO_PKG_VERSION"),
        "timezone": mcloud_core::TZ_NAME,
    })
}

const fn group_name(group: Group) -> &'static str {
    match group {
        Group::Daily => "daily",
        Group::Activity => "activity",
        Group::Query => "query",
        Group::Exchange => "exchange",
        Group::Maintenance => "maintenance",
    }
}

const fn kind_name(kind: CapabilityKind) -> &'static str {
    match kind {
        CapabilityKind::Action => "action",
        CapabilityKind::Query => "query",
    }
}

const fn risk_name(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "low",
        RiskLevel::Medium => "medium",
        RiskLevel::High => "high",
    }
}

const fn account_mode_name(mode: mcloud_core::AccountMode) -> &'static str {
    match mode {
        mcloud_core::AccountMode::Sequential => "sequential",
        mcloud_core::AccountMode::Parallel => "parallel",
    }
}

fn group_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = REGISTRY
        .iter()
        .map(|c| group_name(c.group))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_exposes_declarative_form_metadata() {
        let payload = capabilities_payload();
        let exchange = payload["all"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["code"] == "exchange")
            .expect("exchange 应在 payload 中");

        assert_eq!(exchange["hidden"], true);
        assert_eq!(exchange["risk"], "high");
        assert_eq!(exchange["needsConfirmation"], true);
        assert_eq!(exchange["runInBatch"], false);
        assert_eq!(exchange["defaultAccountMode"], "sequential");

        // 前端需要的表单字段必须齐备
        let params = exchange["params"].as_array().unwrap();
        assert!(params.iter().any(|p| p["key"] == "prizeIdOrName"));
        assert!(params.iter().any(|p| p["key"] == "skipWait"));

        // 出参也要给（用于自动结果卡片）
        let output = exchange["output"].as_array().unwrap();
        assert!(output.iter().any(|o| o["key"] == "cloudCost"));
    }

    #[test]
    fn visible_and_batchable_lists_exclude_hidden_capability() {
        let payload = capabilities_payload();
        let visible = payload["visible"].as_array().unwrap();
        let batchable = payload["batchable"].as_array().unwrap();

        assert!(visible.iter().any(|c| c == "sign"));
        assert!(!visible.iter().any(|c| c == "exchange"));
        assert!(!batchable.iter().any(|c| c == "exchange"));
    }

    #[test]
    fn version_payload_comes_from_cargo_not_a_build_flag() {
        let payload = version_payload();

        assert_eq!(payload["name"], mcloud_core::PRODUCT_NAME);
        // 对外暴露的必须是**产品名**，而不是"碰巧实现它的 crate"的名字。
        // 少了这条，哪天把 version_payload 挪到别的 crate，响应就会悄悄改名。
        assert_ne!(
            payload["name"],
            env!("CARGO_PKG_NAME"),
            "name 不应等于实现它的 crate 名"
        );

        assert!(!payload["version"].as_str().unwrap().is_empty());
        assert_eq!(payload["timezone"], "Asia/Shanghai");
    }

    #[test]
    fn adding_a_capability_changes_only_the_registry() {
        // 这条断言表达的是设计承诺：payload 完全由 REGISTRY 推导，
        // 没有任何一处硬编码能力名。因此新增能力不会牵动服务层与前端。
        let payload = capabilities_payload();
        let all = payload["all"].as_array().unwrap();
        assert_eq!(all.len(), REGISTRY.len());
        for (index, cap) in REGISTRY.iter().enumerate() {
            assert_eq!(all[index]["code"], cap.code);
        }
    }

    #[test]
    fn groups_are_sorted_and_deduplicated() {
        let payload = capabilities_payload();
        let groups: Vec<&str> = payload["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g.as_str().unwrap())
            .collect();
        let mut sorted = groups.clone();
        sorted.sort_unstable();
        assert_eq!(groups, sorted);
        let unique: std::collections::BTreeSet<_> = groups.iter().collect();
        assert_eq!(unique.len(), groups.len(), "分组不应重复");
    }
}
