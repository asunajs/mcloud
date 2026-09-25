//! mcloud-store —— 持久化实现。
//!
//! # 换后端的边界在哪
//!
//! 业务代码只依赖 `mcloud_core` 里的几个子 trait
//! （`AccountStore` / `RunStore` / `LogStore` / `ScheduleStore` / `PipelineStore`），
//! 组合成的 `Store` 是注入点。所以换后端 = 新写一个实现了这五个 trait 的类型，
//! **不动任何业务代码**。
//!
//! 当前提供 [`JsonStore`]：数据常驻内存 + 进程内 `RwLock` + 写透磁盘。
//! 它的并发模型与取舍见该类型的模块文档。
//!
//! SQLite 后端尚未实现 —— 但 `Store` 的形状已经按"能换"设计好了，
//! 不是"以后要拆"的占位。

pub mod clock;
pub mod json_store;

pub use clock::SystemClock;
pub use json_store::{JsonStore, HISTORY_LIMIT};

/// 建一个默认后端。
///
/// 存在的意义是让调用方（`mcloud-app`）**不要**直接写 `JsonStore::open`：
/// 将来加 SQLite 时，只要在这里按配置分派，上层一行都不用改。
pub fn open_default(dir: impl AsRef<std::path::Path>) -> mcloud_core::Result<JsonStore> {
    JsonStore::open(dir)
}
