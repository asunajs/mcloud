# Git hooks（本仓库：**只有归档，没有启用的 hook**）

这里唯一的文件 `run-gates.sh` 是 **Rust 时代的门禁执行器**，2026-09-21 随 Rust
实现从 [`../mcloud/`](../mcloud/) 剥离进来。它**不被任何东西调用**：

- 本仓库是冻结基准，不再开发，也就不需要挂在提交路径上的检查。
- 要验证这棵树，手工跑 `bash scripts/gates.sh`（七条，一键）。

保留它的理由与 Go 侧那两道闸的判据，分别见
[`docs/dev-gates.md`](../docs/dev-gates.md)（"按改动范围缩小检查"那几段的出处）
与 `../mcloud/.githooks/README.md`（现行 hook 的设计）。
将来 Go 侧要加"按改动范围收窄"的能力，从这份文件抄，别重新想一遍。
