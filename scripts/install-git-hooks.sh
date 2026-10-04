#!/bin/sh
# 把仓库里的两个 Web 门禁钩子装进 `.git/hooks/`（**软链** ⇒ 改 `scripts/hooks/*` 即刻生效 ✓）。
#
#   scripts/install-git-hooks.sh              # 装（已装过则覆盖自己的那份）
#   scripts/install-git-hooks.sh --uninstall  # 卸
#
# ⚠⚠ 三个"别踩"：
# 1. **不设 `core.hooksPath`**。那会把 `.git/hooks/` 整个废掉 —— 本机那里还住着
#    Qoder 的 `post-commit` / `post-checkout`（AI 代码追踪），一设就全哑 ✗。
#    所以走"往 `.git/hooks/` 里放软链"这条路 ✓。
# 2. **只碰 pre-commit / pre-push 这两个名字**。别人的钩子（含上面那两个）一个都不动 ✓。
# 3. 遇到**不是我们放的**同名钩子（既不是软链、也不指向 `scripts/hooks/`）就**拒绝覆盖** ✗
#    —— 那可能是别人手工写的门禁，覆盖掉等于静默拆掉一道闸；要么先自己挪走，
#    要么 `--force`（真会覆盖，文件会先备份成 `<name>.bak`）。
#
# 卸载：`scripts/install-git-hooks.sh --uninstall`（只删我们自己放的软链 ✓）。
set -e

root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ]; then
  echo "不在 git 仓库里，什么也没做" >&2
  exit 1
fi

hooks="$root/.git/hooks"
mkdir -p "$hooks"
NAMES="pre-commit pre-push"

if [ "$1" = "--uninstall" ]; then
  for name in $NAMES; do
    target="$hooks/$name"
    if [ -L "$target" ]; then
      rm "$target"
      echo "已卸：$target"
    elif [ -e "$target" ]; then
      echo "跳过（不是软链，可能是别人的钩子）：$target" >&2
    else
      echo "本来就没装：$target"
    fi
  done
  exit 0
fi

force=""
[ "$1" = "--force" ] && force=1

for name in $NAMES; do
  src="$root/scripts/hooks/$name"
  target="$hooks/$name"
  if [ ! -f "$src" ]; then
    echo "找不到 $src，跳过" >&2
    continue
  fi
  chmod +x "$src"
  if [ -e "$target" ] || [ -L "$target" ]; then
    # 已经是我们放的软链 ⇒ 直接重建（幂等）
    if [ -L "$target" ] && [ "$(readlink "$target")" = "../../scripts/hooks/$name" ]; then
      echo "已是最新：$target"
      continue
    fi
    if [ -z "$force" ]; then
      echo "✗ $target 已存在且不是我们放的软链 ⇒ 拒绝覆盖（先自己挪走，或加 --force）" >&2
      continue
    fi
    cp "$target" "$target.bak"
    echo "⚠ 覆盖前已备份：$target.bak"
    rm -f "$target"
  fi
  ln -s "../../scripts/hooks/$name" "$target"
  echo "已装：$target -> ../../scripts/hooks/$name"
done

echo "跳过某一次：git commit --no-verify / git push --no-verify"
