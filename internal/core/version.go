// 产品版本。
//
// 对应 Rust 侧 `env!("CARGO_PKG_VERSION")`（workspace 在 `legacy/Cargo.toml` 里
// 统一管理）。Go 没有编译期包版本注入，所以这里是一个**常量**。
//
// ⚠ 正因为它是常量，"改了这边忘了那边"是可能发生的：
// [Version] 与 `legacy/Cargo.toml` 的 `version` 必须**手工同步**，
// 而不同步的表现只是"两个产物报的版本号不一样" —— 没人会为此报错。
package core

// Version 是产品版本。
//
// ⚠ 对外（`/api/version`、`--version`）暴露的必须是**产品版本**，
// 而不是"碰巧实现它的那个包"的版本 —— 见 [ProductName] 的同一条理由。
const Version = "0.1.0"
