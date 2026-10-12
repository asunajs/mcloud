//! CLI 子命令实现。
//!
//! 放在库里（而不是 `src/bin/` 下）有两个理由：
//!
//! 1. `src/bin/` 下的每个 `.rs` 都会被 Cargo 当成**独立二进制**，
//!    于是 `cli_login.rs` 也会变成一个可执行文件；
//! 2. 放在库里才能被 `cargo test` 直接测到（二进制里的 `#[cfg(test)]`
//!    也能跑，但没法从外部复用）。
//!
//! `src/bin/` 已经没有了（2026-10-03 合并成单个二进制 `mcloud`）：入口是
//! `src/main.rs`，它按 argv 分发到 [`run`]。

pub mod auth;
pub mod config;
pub mod cron;
pub mod daily;
pub mod live_room;
pub mod login;
pub mod pipeline;
pub mod push;
pub mod run;
