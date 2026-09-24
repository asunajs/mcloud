# Git hooks（2026-09-24 随解冻启用）

这个目录是**入库的** hooks 实现。Git 默认不看它，每个克隆跑一次安装：

```bash
git config core.hooksPath .githooks
```

> 历史：`run-gates.sh` 2026-09-21 随 Rust 实现从 [`../mcloud/`](../../mcloud/)
> 剥离进来时只作归档（仓库冻结、不开发，hook 不被任何东西调用）。
> 2026-09-24 解冻后重新挂上：两个 hook 壳 + 执行器恢复原职。

## 两道闸，各拦各的

| hook | 跑什么 | 范围 |
| --- | --- | --- |
| `pre-commit` | fmt / deps / filenames / idents（快的四条） | 按**暂存区**收窄；filenames 无增删改名时跳过 |
| `pre-push` | fmt / clippy / deps / filenames / idents / test / golden（全套） | 不跳过 —— 兜住 pre-commit 收窄掉的部分 |

两个壳都只调 `run-gates.sh <阶段>`，逻辑不重复。为什么 hook 不直接调
`scripts/gates.sh`、以及"按范围收窄"的判据，全在 `run-gates.sh` 文件头 ——
改之前先读那里。

## ⚠ 两层各自都足以让 hook 静默失效的东西

（这一节的判据来自 Go 侧 2026-09-21 的实测，那边踩过两层同时成立的坑：

| 层 | 现象 | 怎么验 |
| --- | --- | --- |
| **1. `core.hooksPath` 没设** | git 默认看 `.git/hooks/`，那里是空的 —— 提交照样成功 | `git config core.hooksPath` 应输出 `.githooks` |
| **2. hook 没有 x 位** | **git 对没有 x 位的 hook 直接跳过，不报错** | `git ls-files -s .githooks/` 应全是 `100755` |

两层都不报错，门禁**一次都不会跑**，而任何地方都看不出"以为有门禁"。
新克隆 / 换机器之后，装完自己验一遍这两条。

## 没有 CI 兜底

本仓库没有 CI（理由见 [`docs/dev-gates.md`](../docs/dev-gates.md)）。
`--no-verify` 之后**没有任何东西替你补上**那次检查 —— 只在确信是环境问题
（Windows 上 target/ 偶发写盘被拦）时才用。

要从零验证整棵树，手动跑 `bash scripts/gates.sh`（七条一键）。
