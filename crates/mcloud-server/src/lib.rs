//! mcloud-server —— HTTP 服务层（**产物之一**）。
//!
//! 职责（P5 实现）：axum 路由 + 参数校验 + SSE 事件流 + `utoipa` 导出 `openapi.json`
//! + `rust-embed` 内嵌前端产物（单二进制可跑）。
//!
//! # 组装与 payload 已经下移到 `mcloud-app`
//!
//! 原因很实际：CLI 也要打印能力注册表、也要跑迁移，而它**不该为了这两件事
//! 把 axum 整棵树拖进来**。所以那些代码住在 `mcloud-app`，这里只重新导出，
//! 让"服务层依赖组装层"这一个方向成立（方案 §3.2 的两个产物）。
//!
//! # 契约没变
//!
//! 前端不消费 Rust 结构体，只消费 JSON 形状；形状一旦改动，
//! P5 的 OpenAPI `--check` 会立刻发现（防止前后端类型静默漂移）。

#![forbid(unsafe_code)]

/// 旧数据迁移的执行策略（实现见 `mcloud-app`）。
pub use mcloud_app::migration;

/// 能力注册表 / 版本信息 → 前端要吃的 JSON（实现见 `mcloud-app`）。
///
/// 重新导出而不是原地保留：CLI 与 HTTP 两条入口必须用**同一份**渲染逻辑，
/// 否则"CLI 看到的字段"与"浏览器看到的字段"迟早不一致。
pub use mcloud_app::payload::{capabilities_payload, capability_payload, version_payload};

#[cfg(test)]
mod tests {
    // 这些断言原本跟随实现，现在实现搬到了 `mcloud-app`，
    // 但**契约属于服务层**（它是对外 API 的形状），所以测试留在这里：
    // 哪天有人改坏了 payload 形状，这里会红，而不是等到 P5 的 OpenAPI 对拍。
    use mcloud_capabilities::REGISTRY;
    use mcloud_core::PRODUCT_NAME;

    use crate::{capabilities_payload, version_payload};

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
    }

    #[test]
    fn adding_a_capability_changes_only_the_registry() {
        // 设计承诺：payload 完全由 REGISTRY 推导，没有任何一处硬编码能力名。
        let payload = capabilities_payload();
        let all = payload["all"].as_array().unwrap();
        assert_eq!(all.len(), REGISTRY.len());
        for (index, cap) in REGISTRY.iter().enumerate() {
            assert_eq!(all[index]["code"], cap.code);
        }
    }

    #[test]
    fn version_payload_reports_the_product_not_the_crate() {
        let payload = version_payload();
        assert_eq!(payload["name"], PRODUCT_NAME);
        assert_eq!(payload["timezone"], "Asia/Shanghai");
        assert!(!payload["version"].as_str().unwrap().is_empty());
    }
}
