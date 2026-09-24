# 第三方内容声明

本仓库是 [mcloud](../mcloud/) 的 **Rust 实现**（2026-09-21 剥离，2026-09-24 解冻为独立项目）。
本文件原来与 mcloud 那份**逐字相同**（剥离时整份复制），于是它开始描述一个
本仓库里并不存在的东西 —— 所以按本仓库的实际内容重写过。

## 关于依赖的许可

`Cargo.lock` 里各依赖的许可随其 crate 自身分发，不在此重复列举。
如需汇总，可用 `cargo deny` 或 `cargo about` 生成。

## daisyUI 官方 agent skill —— **不在本仓库**

`.workbuddy/skills/daisyui/`（73 个 vendor 文件，MIT）服务于 `web/` 的 daisyUI 使用，
而 WebUI 与前端产物都留在 Go 侧仓库，没有随 Rust 一起剥离。
它的来源、固定版本、更新方式与完整许可声明见
[`../mcloud/THIRD-PARTY-NOTICES.md`](../mcloud/THIRD-PARTY-NOTICES.md)。
