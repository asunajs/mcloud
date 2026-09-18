//! mcloud-app —— **组装层**。
//!
//! # 为什么单独一个 crate
//!
//! 方案 §3.2 要求两个独立产物：`mcloud-cli`（命令行）与 `mcloud-server`（HTTP/WebUI）。
//! 它们共享同一份接线 —— 数据库放哪、迁移怎么跑、登录怎么走、能力注册表怎么变成 JSON。
//! 但如果这些代码住在 `mcloud-server` 里，CLI 就**为了跑一次迁移把 axum 整棵树拖进来**：
//! 一个命令行工具多出几十个用不到的依赖。
//!
//! 所以组装层单独成 crate，两个产物都依赖它，而不是互相依赖。
//! "CLI 不含 axum"由此从一句期望变成一条可以写进 `scripts/check-deps.sh` 的断言。
//!
//! # 本层允许有 IO（而且是必须的）
//!
//! 它是**组装根**：`scripts/check-deps.sh` 断言组装根必须依赖 `mcloud-io`。
//! 把 io 的实现注入 core 定义的 trait，正是这一层的职责；
//! 少了这条断言，就会出现"没人接上 IO 实现"的静默死配置。
//!
//! # 这里**不**放的东西
//!
//! 领域规则（账号怎么隔离、登录结果怎么落成账号）在 `mcloud-core`；
//! 状态机（轮询节奏、终态判定）在 `mcloud-capabilities`；
//! 上游请求怎么拼在 `mcloud-upstream`。本层只做编排与接线。

#![forbid(unsafe_code)]

pub mod device;
pub mod login;
pub mod migration;
pub mod payload;
pub mod session;
pub mod user;
