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

# pre-commit / pre-push 必须是可执行的。Windows 上 git 会用 sh 跑它们，
# 位不一定重要；但在 macOS/Linux 上少了 x 位 git 会直接静默跳过 ——
# 而"静默跳过"正是最难发现的那种失效，所以这里显式补上。
chmod +x .githooks/pre-commit .githooks/pre-push .githooks/run-gates.sh 2>/dev/null || true

git config core.hooksPath .githooks

echo "已安装：core.hooksPath = .githooks"
echo
echo "  pre-commit  跑 fmt / deps / filenames / idents      （秒级）"
echo "  pre-push    跑七条全量，含 clippy / test / golden    （分钟级）"
echo
echo "验证：git commit 时应当看到「提交前门禁（快）」的输出。"
echo "卸载：bash scripts/install-hooks.sh --unset"
