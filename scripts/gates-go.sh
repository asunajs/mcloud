#!/usr/bin/env bash
# Go 侧门禁一键跑 —— `scripts/gates.sh`（Rust 侧）的 Go 版。
#
# 八条，与 Rust 侧七条的对应关系：
#
#   build      ≈ cargo build（"能编译"的最直接判据）
#   vet        ≈ cargo clippy --all-targets -- -D warnings（**覆盖窄得多**，见下）
#   fmt        ≈ cargo fmt --check
#   test       ≈ cargo test --workspace（**多一个 -race**，见下）
#   deps       ≈ bash scripts/check-deps.sh   →  Go 版：scripts/check-go-deps.sh
#   filenames  = 同一个脚本，**复用**
#   idents     = 同一个脚本，**复用**
#   golden     = 同一个脚本，**复用**
#
# ⚠ 后三条刻意复用 Rust 侧那三个脚本，不重写。
#   它们与语言无关（扫文件名 / 扫文本里的真实标识 / 独立复算向量），
#   重写一遍等于给自己造第二份真相 —— 而两份真相迟早会分叉，
#   分叉的那天没有人知道该信哪一份。
#
# 用法：
#   bash scripts/gates-go.sh              # 全跑
#   bash scripts/gates-go.sh test deps    # 只跑指定几条
#
# ⚠ 为什么 test 必须带 -race（Rust 侧没有对应物）：
#
#   Rust 里 `&mut R rng` 保证同一随机源不被并发共享 —— 那是**编译期保证**。
#   Go 的 io.Reader 是引用语义，并发用同一个源是 data race，而它**不一定**
#   让测试变红：可能只是偶发地产生错误数据（签名对不上、deviceId 错一位），
#   于是被当成"上游抽风"。所以 -race 是那条保证在 Go 侧的替代品，不是可选项。
#
# ⚠ 为什么 vet 不等于 clippy：
#
#   `go vet` 只覆盖"几乎肯定是错"的一小撮检查（printf 格式串、锁被复制、
#   无用的赋值…），比 clippy 窄得多。补齐要引 golangci-lint，而那是**外部
#   依赖** —— 骨架阶段刻意一个第三方依赖都不引（见 docs/go-migration-plan.md）。
#   **在那之前，不要把 vet 当成 clippy 用**：它过了不代表静态检查这一关过了。
#
# ⚠ bash 3.2 兼容（macOS 自带的 /bin/bash 永远是 3.2.57，且不会升）：
#   不用 declare -A / mapfile / ${v,,}。理由见 docs/dev-gates.md。
set -uo pipefail

cd "$(dirname "$0")/.."

GATE_NAMES="build vet fmt test deps filenames idents golden"

# gofmt -l 只**列出**未格式化的文件，退出码仍是 0 —— 所以不能直接当门禁用，
# 必须自己判"输出是否为空"。这一步很容易写错成 `gofmt -l . && echo ok`。
gate_fmt() {
  local out
  out="$(gofmt -l . 2>&1)"
  if [ -n "$out" ]; then
    echo "!! 以下文件未格式化（跑 'gofmt -w .' 可修）："
    echo "$out"
    return 1
  fi
  echo "ok gofmt：全部已格式化"
  return 0
}

gate_run() {
  case "$1" in
    build)     go build ./... ;;
    vet)       go vet ./... ;;
    fmt)       gate_fmt ;;
    test)      go test -race ./... ;;
    deps)      bash scripts/check-go-deps.sh ;;
    filenames) bash scripts/check-filenames.sh ;;
    idents)    node scripts/check-identifiers.cjs ;;
    golden)    node scripts/check-golden-vectors.cjs ;;
    *)         return 127 ;;
  esac
}

ORDER=(build vet fmt test deps filenames idents golden)
if [ $# -gt 0 ]; then
  ORDER=("$@")
fi

FAILED=()
# 日志放 target/（已在 .gitignore 里），用 gates-go- 前缀与 Rust 侧区分开 ——
# 两套门禁的日志混在一个文件里，排障时会看错那一条的输出。
LOG="target/gates-go-last.log"
mkdir -p target 2>/dev/null
: > "$LOG"

for name in "${ORDER[@]}"; do
  printf '\n\033[1m──── %s ────\033[0m\n' "$name"

  # 每条门禁的输出单独一个文件：诊断只看本次这条说了什么，
  # 不被前面通过的门禁留下的字样污染（这个坑 Rust 侧踩过，见 scripts/gates.sh）。
  RUN_LOG="target/gates-go-${name}.log"

  if gate_run "$name" >"$RUN_LOG" 2>&1; then
    cat "$RUN_LOG"
    cat "$RUN_LOG" >> "$LOG"
    printf '\033[32m✓ %s 通过\033[0m\n' "$name"
  else
    rc=$?
    if [ "$rc" -eq 127 ]; then
      echo "!! 未知门禁: $name（可选：$GATE_NAMES）"
      exit 2
    fi
    cat "$RUN_LOG"
    cat "$RUN_LOG" >> "$LOG"
    printf '\033[31m✗ %s 失败\033[0m\n' "$name"
    FAILED+=("$name")
  fi
done

if [ ${#FAILED[@]} -gt 0 ]; then
  printf '\033[31m门禁未通过: %s\033[0m\n' "${FAILED[*]}"
  echo "完整输出: $LOG"
  exit 1
fi
printf '\033[32m八项门禁全绿\033[0m\n'
: > "$LOG"
