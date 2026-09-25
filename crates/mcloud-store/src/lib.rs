//! mcloud-store —— 持久化实现。
//!
//! 当前用 JSON + 原子写（tmp → rename），接口经 `mcloud_core::Store` 抽象，
//! 后续可替换为 SQLite 而不改动业务代码。

pub mod clock;
pub mod json_store;

pub use clock::SystemClock;
pub use json_store::JsonStore;
