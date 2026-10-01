//! 任务注册表。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{TaskContext, TaskResult};

/// 任务元信息，供 `/api/capabilities` 暴露给前端。
///
/// ⚠ `rename_all = "camelCase"` 不能省：`group_label` 落到 JSON 里必须是 `groupLabel`
/// —— 前端读的是后者。而**测试装置（fixture）里的 JSON 是手写的**：它写对了、
/// 真机发错了，门禁照样全绿（2026-10-01 实际发生：线上分组名全回落成兜底的"其它"）。
/// 钉住字段名的是 `crates/mcloud-server/tests/routes.rs` 里那条断言 key 的用例。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskMeta {
    pub name: String,
    pub title: String,
    pub description: String,
    /// 是否在 UI 中隐藏（例如兑换奖品走独立页面）。
    pub hidden: bool,
    /// 分组键（`signin` / `live` / `app` / `device`）—— 界面按它分栏。
    pub group: TaskGroup,
    /// 分组的**中文名**。
    ///
    /// ⚠ 由后端给，**不是**让前端维护一份"键→名字"的映射：
    /// 本仓已经栽过一次 —— 任务页曾把签到族的 8 个名字硬编码在前端当分组依据，
    /// 而后端的注册表才是权威。前端每多一份这类表，就多一处迟早漂移的地方。
    pub group_label: String,
    /// 该任务支持的参数。
    ///
    /// 从前这里只有**参数名**（`Vec<String>`），于是类型/默认值全靠前端猜 ——
    /// 猜错的表现是"填了没用"。现在把类型、默认值、取值范围一起给出去。
    #[serde(default)]
    pub params: Vec<ParamSpec>,
}

/// 任务分组。只影响界面怎么分栏，不影响执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskGroup {
    /// AI豆中心那一族（签到、领取、快照…）。
    Signin,
    /// 直播小红花。
    Live,
    /// 应用与 AI（趣玩、拍拍、App 任务…）。
    ///
    /// ⚠ 这是**默认值**（`TaskGroup::default()`）—— 没标分组的任务都落在这一栏。
    /// 原来手写了一个 `impl Default`，clippy 的 `derivable_impls` 要求改成
    /// `derive` + `#[default]`（两者语义完全一样，只是写法）。
    #[default]
    App,
    /// 消息与设备。
    Device,
}

impl TaskGroup {
    /// 界面上的分组标题。
    pub fn label(self) -> &'static str {
        match self {
            TaskGroup::Signin => "AI豆中心",
            TaskGroup::Live => "直播小红花",
            TaskGroup::App => "应用与 AI",
            TaskGroup::Device => "消息与设备",
        }
    }
}

/// 参数该用什么控件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Bool,
    Number,
    Text,
}

/// 一个参数的说明（前端据此生成表单 + 校验）。
///
/// ⚠ 这里是**唯一的权威**：任务的 `run` 怎么解析这个参数，就得在这里说清楚；
/// 两边不一致时，用户看到的是"填了没用"。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    pub name: String,
    /// 界面上的标签。
    pub title: String,
    pub kind: ParamKind,
    /// 默认值（JSON）。前端用它预填"从没配过"的任务。
    pub default: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// 一句话说明（"这个参数到底干什么"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl ParamSpec {
    pub fn boolean(name: &str, title: &str, default: bool, help: &str) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            kind: ParamKind::Bool,
            default: serde_json::Value::Bool(default),
            min: None,
            max: None,
            unit: None,
            help: Some(help.into()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn number(
        name: &str,
        title: &str,
        default: f64,
        min: f64,
        max: f64,
        unit: &str,
        help: &str,
    ) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            kind: ParamKind::Number,
            default: serde_json::json!(default),
            min: Some(min),
            max: Some(max),
            unit: Some(unit.into()),
            help: Some(help.into()),
        }
    }

    pub fn text(name: &str, title: &str, help: &str) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            kind: ParamKind::Text,
            default: serde_json::Value::String(String::new()),
            min: None,
            max: None,
            unit: None,
            help: Some(help.into()),
        }
    }
}

/// 一个可执行任务。
///
/// `run` 接收原始 JSON 参数而非强类型结构，是为了让注册表保持同质、
/// 便于动态列举；参数校验放在各任务内部，错误以 `TaskOutcome::Skipped`
/// 或 `Error::Config` 返回。
#[async_trait]
pub trait Task: Send + Sync {
    fn name(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn description(&self) -> &'static str;

    fn hidden(&self) -> bool {
        false
    }

    /// 该任务属于哪一组（界面分栏用）。
    fn group(&self) -> TaskGroup {
        TaskGroup::default()
    }

    /// 该任务支持的参数。**默认没有** —— 只有真的读参数的实现才该覆写它。
    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    async fn run(&self, ctx: &TaskContext, body: &serde_json::Value) -> TaskResult;

    /// 组装元信息。
    fn meta(&self) -> TaskMeta {
        TaskMeta {
            name: self.name().into(),
            title: self.title().into(),
            description: self.description().into(),
            hidden: self.hidden(),
            group: self.group(),
            group_label: self.group().label().into(),
            params: self.params(),
        }
    }
}

/// 任务注册表。按名字稳定排序，保证 `/api/capabilities` 输出可复现。
#[derive(Default)]
pub struct Registry {
    tasks: BTreeMap<&'static str, Arc<dyn Task>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册任务。重名会被拒绝——静默覆盖会让「任务没生效」极难排查。
    pub fn register(&mut self, task: Arc<dyn Task>) -> Result<(), String> {
        let name = task.name();
        if self.tasks.contains_key(name) {
            return Err(format!("任务名重复注册：{name}"));
        }
        self.tasks.insert(name, task);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Task>> {
        self.tasks.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tasks.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// 全部任务元信息（含隐藏任务）。
    pub fn list(&self) -> Vec<TaskMeta> {
        self.tasks.values().map(|t| t.meta()).collect()
    }

    /// 仅可见任务，供前端菜单使用。
    pub fn list_visible(&self) -> Vec<TaskMeta> {
        self.tasks
            .values()
            .filter(|t| !t.hidden())
            .map(|t| t.meta())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskContext;

    struct Dummy(&'static str);

    #[async_trait]
    impl Task for Dummy {
        fn name(&self) -> &'static str {
            self.0
        }
        fn title(&self) -> &'static str {
            "dummy"
        }
        fn description(&self) -> &'static str {
            "测试用"
        }
        async fn run(&self, _c: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            Ok(mcloud_core::TaskOutcome::success("ok"))
        }
    }

    struct Hidden;
    #[async_trait]
    impl Task for Hidden {
        fn name(&self) -> &'static str {
            "exchange"
        }
        fn title(&self) -> &'static str {
            "兑换"
        }
        fn description(&self) -> &'static str {
            "隐藏任务"
        }
        fn hidden(&self) -> bool {
            true
        }
        async fn run(&self, _c: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            Ok(mcloud_core::TaskOutcome::success("ok"))
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut r = Registry::new();
        r.register(Arc::new(Dummy("sign"))).unwrap();
        assert!(r.contains("sign"));
        assert_eq!(r.len(), 1);
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut r = Registry::new();
        r.register(Arc::new(Dummy("sign"))).unwrap();
        let err = r.register(Arc::new(Dummy("sign"))).unwrap_err();
        assert!(err.contains("重复"), "got: {err}");
        assert_eq!(r.len(), 1, "重复注册不得改变注册表");
    }

    #[test]
    fn list_is_sorted_by_name() {
        let mut r = Registry::new();
        r.register(Arc::new(Dummy("sign"))).unwrap();
        r.register(Arc::new(Dummy("after"))).unwrap();
        let names: Vec<String> = r.list().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["after", "sign"]);
    }

    #[test]
    fn visible_excludes_hidden() {
        let mut r = Registry::new();
        r.register(Arc::new(Dummy("sign"))).unwrap();
        r.register(Arc::new(Hidden)).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r.list_visible().len(), 1);
        assert_eq!(r.list_visible()[0].name, "sign");
    }
}
