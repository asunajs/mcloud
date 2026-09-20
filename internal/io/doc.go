// Package mcloudio 是**唯一**允许做 IO 的层：HTTP 客户端、SQLite、系统随机源。
//
// 对应 Rust 侧的 crates/mcloud-io。
//
// # ⚠ 包名不是 io
//
// 目录叫 internal/io（与 Rust 的 crate 名一一对应），但包名是 mcloudio ——
// 标准库的 io 几乎每个文件都要用，重名会让每个 import 都得写别名。
// 目录名与包名不同在 Go 里完全合法。
//
// # 分层铁律
//
// 本层是"IO 隔离"这条约束的**唯一出口**：
//
//   - 纯层（core / upstream / capabilities / exec）**不得** import 本包；
//   - 组装根（app / server）**必须** import 本包。
//
// 第二个方向同样要断言，理由不是对称美学：没人接上 IO 实现是一个
// **静默的死配置** —— 编译能过、跑起来什么都没发生。
//
// 两条都由 scripts/check-go-deps.sh 断言。
//
// # ⚠ 与 Rust 侧的一处语义差异
//
// Rust 里"这一层不许有 IO"是**编译期事实**（纯层根本没依赖 reqwest）。
// Go 的接口是结构性实现 —— 任何包都能满足 core 定义的接口，所以这条约束
// 从编译期事实**降级成 lint 约定**。这正是 check-go-deps.sh 必须存在的原因。
package mcloudio
