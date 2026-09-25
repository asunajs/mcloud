//! 任务注册表。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{TaskContext, TaskResult};

/// 任务元信息，供 `/api/capabilities` 暴露给前端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMeta {
    pub name: String,
    pub title: String,
    pub description: String,
    /// 是否在 UI 中隐藏（例如兑换奖品走独立页面）。
    pub hidden: bool,
    /// 该任务支持的参数名，前端据此生成表单。
    #[serde(default)]
    pub params: Vec<String>,
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

    fn params(&self) -> Vec<String> {
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
