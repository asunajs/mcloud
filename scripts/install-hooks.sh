#!/usr/bin/env bash
# 把 `.githooks/` 接成这个仓库的 git hooks。
#
# 做的事只有一件：`git config core.hooksPath .githooks`。
# 之所以需要跑一次，是因为 `.git/` 里的东西不入版本库 ——
# 克隆下来的人拿不到 hook，只能自己装（见 .githooks/README.md）。
#
# 用法：
#   bash scripts/install-hooks.sh          # 装
#   bash scripts/install-hooks.sh --unset  # 卸（恢复 git 默认行为）
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "${1:-}" = "--unset" ]; then
  git config --unset core.hooksPath 2>/dev/null || true
  echo "已卸载：core.hooksPath 恢复默认（看 .git/hooks/）"
  exit 0
fi

if [ ! -d .githooks ]; then
  echo "!! 找不到 .githooks/ 目录 —— 是不是不在仓库根目录？"
  exit 1
fi

# 只有 pre-commit / pre-push 是**启用**的 hook，它们必须是可执行的。Windows 上 git
# 会用 sh 跑它们，位不一定重要；但在 macOS/Linux 上少了 x 位 git 会直接静默跳过 ——
# 而"静默跳过"正是最难发现的那种失效，所以这里显式补上。
#
# （这里原先还 chmod 了 `.githooks/run-gates.sh` —— 那个 Rust 时代的执行器
#   2026-09-21 已随 Rust 基准移入同级仓库 `../mcloud-rust/.githooks/` 归档，
#   本仓库不再有它。）
chmod +x .githooks/pre-commit .githooks/pre-push 2>/dev/null || true

git config core.hooksPath .githooks

echo "已安装：core.hooksPath = .githooks"
echo
# ⚠ 这里刻意**不列**具体是哪几条门禁 —— 列了就是第二份真相，而它已经过期过一次：
#    本行原文写着「pre-push 跑七条全量，含 clippy」，那是 **Rust 侧**的说法
#    （Go 侧既没有 clippy，条数也不是七）。判据只有一个来源：`.githooks/README.md`
#    与 `scripts/gates-go.sh` 的头部注释。
echo "  pre-commit  秒级那几条：不需要编译的静态检查"
echo "  pre-push    全量：再加需要编译与跑测试的那几条"
echo
echo "具体是哪几条、为什么这样分：见 .githooks/README.md"
echo "验证：git commit 时应当看到「──── fmt ────」这样的分节输出。"
echo "卸载：bash scripts/install-hooks.sh --unset"
