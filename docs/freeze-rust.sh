#!/usr/bin/env bash
# 【历史脚本归档】2026-09-20 在 mcloud 仓库执行的"把 crates/ 移进 legacy/"的一次性
# 搬迁脚本，随代码剥离移入本仓库 docs/ 存档 —— **不要在 mcloud-rust 里再跑它**
# （它假设自己在 mcloud 仓库根，且那次冻结已完成、剥离也已发生）。
#
# 把 Rust 实现冻结到 legacy/ —— 只读保留，作 Go 侧的对拍基准。
#
# 背景与决策见 docs/go-migration-plan.md §3；冻结后的约定见 legacy/README.md。
#
# 用法：
#   bash scripts/freeze-rust.sh --dry-run    # 只打印要做什么，不动任何文件
#   bash scripts/freeze-rust.sh              # 真的做
#
# # ⚠ 为什么必须先确认"那几个路径下没有未提交改动"
#
# `git mv` 只搬**已跟踪**的文件。未跟踪的新文件（例如刚写下还没 add 的
# `crates/mcloud-server/src/routes/`）会被**留在原地**，于是目录被劈成两半：
# 一半在 `legacy/crates/`、一半还在 `crates/` —— 而两边都能编译一部分，
# 现象是「莫名其妙的重复定义」，不是「文件丢了」。排查这个要很久。
#
# 所以本脚本宁可拒绝执行，也不做半套。
#
# 幂等：已经冻结过（`legacy/Cargo.toml` 存在）就报「已冻结」并以 0 退出。
#
# ⚠ bash 3.2 兼容（macOS 自带的 /bin/bash 永远是 3.2.57）：
#   不用 declare -A / mapfile / ${v,,}，也不在 set -u 下展开空数组。
#   理由见 docs/dev-gates.md。
set -uo pipefail

cd "$(dirname "$0")/.."

DRY=0
if [ "${1:-}" = "--dry-run" ]; then
  DRY=1
fi

# 冻结要搬走的路径。**检查范围与搬运范围必须一致** ——
# 否则会漏掉某一类改动，而那正是"半套"的来源。
PATHS=(crates Cargo.toml Cargo.lock rust-toolchain.toml .cargo)

say() { printf '%s\n' "$*"; }

# ── 幂等 ────────────────────────────────────────────────────────────
if [ -e "legacy/Cargo.toml" ]; then
  say "已冻结：legacy/Cargo.toml 存在，无需重复执行。"
  exit 0
fi

# ── 前置：这几个路径下不得有未提交改动（含未跟踪文件）──────────────
#
# dry-run 只警告不拒绝 —— 它不改任何文件，而"先跑一遍 dry-run 看能不能跑"
# 正是评估这件事的正常用法。
dirty="$(git status --porcelain -- "${PATHS[@]}")"
if [ -n "$dirty" ]; then
  if [ "$DRY" -eq 1 ]; then
    say "⚠ dry-run：以下路径下有未提交改动 —— 真跑时会被拒绝。"
    say ""
    say "$dirty"
    say ""
  else
    say "!! 拒绝执行：以下路径下有未提交改动。"
    say ""
    say "$dirty"
    say ""
    say "git mv 只搬已跟踪的文件；未跟踪的新文件会被留在原地，"
    say "于是目录被劈成两半 —— 而两边都能编译一部分，现象是「莫名其妙的重复定义」。"
    say ""
    say "先把它们提交或清理掉。若另一个会话正在改 crates/，等它停手再跑。"
    exit 1
  fi
fi

say "把 Rust 实现冻结到 legacy/ …"
say ""

# 每个动作都先打印再执行 —— dry-run 与真跑走的是**同一条**代码路径，
# 所以 dry-run 的输出可以当作"将要发生什么"的准确描述。
run() {
  if [ "$DRY" -eq 1 ]; then
    say "  [dry-run] $*"
  else
    say "  \$ $*"
    "$@" || { say ""; say "!! 上面这条失败了，中止。"; exit 1; }
  fi
}

if [ "$DRY" -eq 1 ]; then
  say "  [dry-run] mkdir -p legacy"
else
  mkdir -p legacy
fi

run git mv crates legacy/crates
run git mv Cargo.toml legacy/Cargo.toml
run git mv Cargo.lock legacy/Cargo.lock
run git mv rust-toolchain.toml legacy/rust-toolchain.toml
run git mv .cargo legacy/.cargo

# ⚠ 这一条是整个冻结里唯一"不明显"的动作，所以单独解释：
#
# `crates/` 移进 `legacy/` 之后，17 处
# `include_str!("../../../../../test/golden/*.json")` 的相对路径会**多一层**，
# 全部失效 —— `cargo test` 直接编译不过。符号链接让它们重新指回仓库根的
# `test/`，于是**源码一字未改**。详见 legacy/README.md。
if [ "$DRY" -eq 1 ]; then
  say "  [dry-run] ln -s ../test legacy/test"
else
  ln -s ../test legacy/test
  say "  \$ ln -s ../test legacy/test"
fi

say ""
if [ "$DRY" -eq 1 ]; then
  say "（dry-run，没有改动任何文件）"
  exit 0
fi

say "冻结完成。剩下三件事必须手工做 —— 脚本不替你做，因为每一件都要判断："
say ""
say "  1. .githooks/ 的 hook 改指向 scripts/gates-go.sh。"
say "     Rust 侧不再拦提交；留着旧 hook 会让每次提交都跑一遍 2 分钟的 cargo。"
say ""
say "  2. 各跑一次，确认冻结没弄坏东西："
say "       bash scripts/gates-go.sh"
say "       cargo test --manifest-path legacy/Cargo.toml --workspace"
say ""
say "  3. 更新 README.md 与 docs/status.md 里「现在用什么语言」的说法。"
say ""
say "⚠ 提交时用 git status 确认 legacy/test 是**符号链接**（模式 120000），"
say "   而不是被展开成一份目录副本。"
