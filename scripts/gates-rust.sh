#!/usr/bin/env bash
#
# Rust 侧门禁：**只跑改动到的 crate**（快回路），或全量（提交前那次）。
#
# # 为什么要有"只跑改动到的"
#
# 实测（2026-10-03，见 docs/build-perf.md §3.2）：本仓的耗时**不在编译慢**，
# 而在**三档命令各一套产物、互不复用** ✗：
#
#   cargo check -p mcloud-core     热的 8.0 s   /  冷的那一档 663.5 s
#   cargo clippy --workspace --all-targets     冷 19m13s
#   cargo test   --workspace                   冷 27m14s   ← 其中真跑测试只有 1.17 s
#
# ⇒ 全量那两条**一条都不能省**（提交前必须全绿 ✓），但**每次改一行都跑全量是纯浪费** ✗。
#   这个脚本按 `git diff` 找出改动落在哪些 crate，只对它们跑 clippy + test ✓。
#
# # 用法
#
#   scripts/gates-rust.sh            # 快回路：fmt（全仓，便宜）+ 只查改动的 crate
#   scripts/gates-rust.sh --full     # 提交前：全量（clippy --workspace --all-targets + test --workspace）
#   scripts/gates-rust.sh --changed  # 同上（默认就是它，写出来只是自解释）
#
# ⚠ 拿不准就跑 `--full`：这里**宁可退回全量**（见下面"保守"那几条），不会漏跑 ✓。
# ⚠ 前端那一档与本脚本无关 ⇒ 用 `node scripts/gates-web.mjs` ✓（两者可同时跑 ✓）。

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
export CARGO_HOME="${CARGO_HOME:-$PWD/.cargo-home}"
export PATH="$HOME/.cargo/bin:$PATH"

MODE="changed"
for arg in "$@"; do
  case "$arg" in
    --full) MODE="full" ;;
    --changed) MODE="changed" ;;
    *) echo "未知参数：$arg" >&2; exit 2 ;;
  esac
done

# fmt 全仓跑：它不编译、秒级 ✓（且反正只允许这一种写法 ✓）
echo "── fmt --all -- --check"
if ! cargo fmt --all -- --check; then
  echo "✗ 格式没过：跑 \`cargo fmt --all\` 再提交" >&2
  exit 1
fi
echo "✓ fmt"

if [[ "$MODE" == "full" ]]; then
  echo "── clippy --workspace --all-targets（全量）"
  cargo clippy --workspace --all-targets -- -D warnings || exit 1
  echo "── test --workspace（全量）"
  cargo test --workspace || exit 1
  echo "✓ 全量绿"
  exit 0
fi

# ── 快回路：把改动映射到 crate ──────────────────────────────────────────
#
# 保守三条（任一命中就退回全量）：
#   ① 改了 `Cargo.toml` / `Cargo.lock` —— 依赖图可能变，`-p` 的边界不再可信 ✗
#   ② 改了 `crates/` 之外的文件（build.rs 辅助脚本、web、docs…）—— 影响面说不清
#   ③ 一个 crate 都没识别出来（或工作区干净）—— 那就别猜
CHANGED="$( { git diff --name-only --diff-filter=ACMR HEAD; git ls-files -o --exclude-standard; } 2>/dev/null | sort -u)"

if [[ -z "$CHANGED" ]]; then
  echo "工作区干净 ⇒ 没有要查的 crate（要跑全量就加 --full）"
  exit 0
fi
if grep -qE '^(Cargo\.(toml|lock)|\.cargo/)' <<<"$CHANGED"; then
  echo "⚠ 动了 Cargo.toml/Cargo.lock ⇒ 退回全量"
  exec "$0" --full
fi
if grep -vqE '^crates/' <<<"$CHANGED"; then
  echo "⚠ 动了 crates/ 之外的文件 ⇒ 退回全量："
  grep -vE '^crates/' <<<"$CHANGED" | sed 's/^/    /'
  exec "$0" --full
fi

# `crates/mcloud-core/src/event.rs` → `mcloud-core`
CRATES="$(sed -n 's|^crates/\([^/]*\)/.*|\1|p' <<<"$CHANGED" | sort -u)"
if [[ -z "$CRATES" ]]; then
  echo "⚠ 没识别出 crate ⇒ 退回全量"
  exec "$0" --full
fi

PKGS=()
while read -r c; do [[ -n "$c" ]] && PKGS+=(-p "$c"); done <<<"$CRATES"

echo "改动落在：$(tr '\n' ' ' <<<"$CRATES")"
echo "── clippy ${PKGS[*]} --all-targets"
cargo clippy "${PKGS[@]}" --all-targets -- -D warnings || exit 1
echo "── test ${PKGS[*]}"
cargo test "${PKGS[@]}" || exit 1
echo "✓ 快回路绿（提交前记得跑一次 --full）"
