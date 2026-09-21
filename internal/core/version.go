// 产品版本。
//
// 对应 Rust 侧 `env!("CARGO_PKG_VERSION")`。Go 没有编译期包版本注入，
// 所以这里是一个**常量**。
//
// 这里曾经有一条真约束：本常量与 Rust workspace（当时写作 `legacy/Cargo.toml`）
// 的 `version` 必须**手工同步**，而不同步的表现只是"两个产物报的版本号不一样" ——
// 没人会为此报错。2026-09-21 Rust 实现整体剥离为同级仓库 `../mcloud-rust/`
// 并且那边已冻结（不再开发），同步义务到此结束：**这里就是版本号的唯一来源**。
// 那边 Cargo.toml 停在 0.1.0 是冻结快照，不跟改。
package core

// Version 是产品版本。
//
// ⚠ 对外（`/api/version`、`--version`）暴露的必须是**产品版本**，
// 而不是"碰巧实现它的那个包"的版本 —— 见 [ProductName] 的同一条理由。
const Version = "0.1.0"
