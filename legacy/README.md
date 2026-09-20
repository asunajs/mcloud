# legacy/ —— 冻结的 Rust 实现

> **只读。** 这里的代码不再开发，留下来只为当 Go 侧的**对拍基准**。
>
> 执行 `bash scripts/freeze-rust.sh` 之后，本目录会装上
> `crates/`、`Cargo.toml`、`Cargo.lock`、`rust-toolchain.toml`、`.cargo/`。
> 在那之前这里只有这份说明。

---

## 为什么冻结

2026-09-20 决定把实现从 Rust 迁到 Go，动机是**编译体验**：

改 `mcloud-core/src/lib.rs` 一行后，`cargo check --workspace` 实测
**1 分 58 秒**（`real 118.71s`，macOS arm64 / rustc 1.91.1）。
根因不是"Rust 慢"，而是 `mcloud-core` 之上 7 个 crate 全部要重新 check ——
这是单 workspace 的固有代价，不是调参能解决的。

完整论证、逐 crate 迁移要点与每步退出判据见
[`../docs/go-migration-plan.md`](../docs/go-migration-plan.md)。

## 什么不许改

**一行都不许改。**

它现在是**参照物**：Go 侧算出什么、Rust 侧算出什么，两边不一致时，
要能确定**是哪一边错了**。一旦这里被顺手改动，这个判断就没有基准了 ——
而改动**不会让任何测试变红**，所以也没有任何东西会提醒你。

发现 bug 也**不要**在这里修，去 Go 侧修。

## 怎么跑它的测试

```bash
cargo test --manifest-path legacy/Cargo.toml --workspace
```

⚠ 用 `--manifest-path`，**不要** `cd legacy` 再跑。仓库里其它工具
（`scripts/check-filenames.sh`、`scripts/check-identifiers.cjs` 等）
都以**仓库根**为工作目录，`cd` 会让它们找不到文件。

## ⚠ `legacy/test` 是符号链接

```
legacy/test -> ../test
```

它存在的唯一理由是：`crates/` 移进 `legacy/` 之后，**17 处**
`include_str!("../../../../../test/golden/*.json")` 的相对路径会**多一层**，
全部失效（`cargo test` 直接编译不过）。符号链接让它们重新指回仓库根的
`test/`，于是**源码一字未改** —— 这对"参照物"这个身份很重要。

- **macOS / Linux**：开箱即用。
- **Windows**：git 默认会把符号链接检出成一个普通文本文件（内容就是
  `../test`），`cargo test` 会失败。两个选择：① 用 `mklink /D` 手工重建；
  ② 放弃符号链接，把那 17 处 `include_str!` 的路径各加一个 `../`。
  ——本仓库当前的开发机是 macOS，这条只是提醒。

## ⚠ 两个 `legacy` 不是一回事

| 路径 | 是什么 |
| --- | --- |
| `legacy/`（这里） | **冻结的 Rust 实现** —— 本项目自己的代码 |
| `test/fixtures/legacy/` | **旧项目 `mcloud-sign` 的导出件** —— 另一份代码的素材 |

同名不同物。看到 `legacy` 先看清是哪个路径，别把两件事混成一件。
