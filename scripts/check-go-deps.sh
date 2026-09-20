#!/usr/bin/env bash
# Go 侧依赖方向门禁 —— `scripts/check-deps.sh` 的 Go 版。
#
# # 为什么 Go 侧这条比 Rust 侧那条更**必需**
#
# Rust 里「纯层不许有 IO」是**编译期事实**：`mcloud-core` 的 Cargo.toml 里
# 根本没有 reqwest，想违反也违反不了。Go 的接口是**结构性实现** ——
# 任何包都能满足 `internal/core` 定义的契约，所以「这一层不许有 IO」
# 从编译期事实**降级成 lint 约定**。没有这条门禁，它就只是一句写在文档里的话。
#
# 这不是"Go 更差"，是"Go 的这条防线换了个位置"：从编译器挪到了这里。
#
# # 三类断言（与 scripts/check-deps.sh 一一对应）
#
#   PURE_PKGS            纯层：不得含 IO 包、也不得依赖 internal/io
#   COMPOSITION_ROOTS    组装根：**必须**依赖 internal/io（反方向也要断言）
#   FORBIDDEN_EDGES      产物之间不得互相依赖，只能共用 internal/app
#
# ⚠ 用 `go list -deps` 而**不加** `-test`：
#   前者是生产依赖，与 Rust 侧的 `cargo tree --edges normal` 对应 ——
#   测试里允许用临时目录、假随机源之类的工具，不该被这条约束管到。
#   加了 `-test` 会把测试依赖也算进来，等于把约束放宽到没有。
#
# ⚠ bash 3.2（macOS 自带的 /bin/bash 永远是 3.2.57）兼容：
#   不用 declare -A / mapfile / ${v,,}。理由见 docs/dev-gates.md。
set -uo pipefail

cd "$(dirname "$0")/.."

MODULE="github.com/catlair/mcloud"

# 纯层：必须既不含 IO 库、也不依赖本仓 IO 层
PURE_PKGS=(core upstream capabilities exec)

# 组装根：**必须**依赖 internal/io —— 它负责把 io 的实现注入 core 定义的契约。
# 如果把组装根也当成"纯层"，门禁会永远红；而如果把组装根的检查整个去掉，
# 又会出现"没人接上 IO 实现"的静默死配置。所以两个方向都要断言。
COMPOSITION_ROOTS=(app server)

# 产物之间的**禁止**依赖。
#
# CLI 与 HTTP 服务是两个独立产物，它们只能共用 internal/app。
# 一旦 CLI 依赖了 server，命令行工具就会连带拿到路由 / 静态资源那一整套 ——
# 而这件事**不会让任何测试变红**，只会让二进制悄悄变大、
# 让"CLI 能在没有 HTTP 环境的地方跑"这条性质消失。所以要在这里挡住。
FORBIDDEN_EDGES=("cli:server")

# 纯层禁止出现的 import path。
#
# 只列两个标准库入口而不是穷举 —— 判据是"它是不是 IO 的出入口"：
#   net/http     HTTP 客户端与服务的唯一入口
#   database/sql 数据库的唯一入口
# 再加本仓的 IO 层（那是把上面两个包起来的地方）。
FORBIDDEN_IMPORTS=(
  "net/http"
  "database/sql"
  "$MODULE/internal/io"
)

fail=0

# 某包的生产依赖闭包（每行一个 import path）。
deps_of() {
  go list -deps -f '{{.ImportPath}}' "$1" 2>/dev/null
}

# 精确整行匹配 —— 用 -x 是为了让 "net/http" 不会误命中 "net/http/httputil"。
# 用这里的 <<< 而不是 `echo | grep`：grep -q 命中即退出会让 echo 吃到 SIGPIPE。
has_dep() {
  grep -qxF "$1" <<<"$2"
}

# ── 纯层 ────────────────────────────────────────────────────────────
for p in "${PURE_PKGS[@]}"; do
  deps="$(deps_of "./internal/$p")"

  if [ -z "$deps" ]; then
    echo "!! 无法解析 $MODULE/internal/$p 的依赖图（包是否存在？）"
    fail=1
    continue
  fi

  pkg_bad=0
  for bad in "${FORBIDDEN_IMPORTS[@]}"; do
    if has_dep "$bad" "$deps"; then
      echo "!! internal/$p 依赖了 $bad —— 违反 IO 隔离约束"
      fail=1
      pkg_bad=1
    fi
  done

  # 只在没报错时才打 ok。否则同一行会既报错又说 ok，
  # 读日志的人会以为这条过了 —— 那比不打更糟。
  if [ "$pkg_bad" -eq 0 ]; then
    echo "ok internal/$p（纯层）"
  fi
done

# ── 组装根 ──────────────────────────────────────────────────────────
for p in "${COMPOSITION_ROOTS[@]}"; do
  deps="$(deps_of "./internal/$p")"

  if [ -z "$deps" ]; then
    echo "!! 无法解析 $MODULE/internal/$p 的依赖图"
    fail=1
    continue
  fi

  if ! has_dep "$MODULE/internal/io" "$deps"; then
    echo "!! internal/$p 未依赖 internal/io —— 组装根必须把 IO 实现接上"
    fail=1
  else
    echo "ok internal/$p（组装根，允许含 IO）"
  fi
done

# ── 产物之间 ────────────────────────────────────────────────────────
for edge in "${FORBIDDEN_EDGES[@]}"; do
  from="${edge%%:*}"
  to="${edge##*:}"
  deps="$(deps_of "./internal/$from")"

  if [ -z "$deps" ]; then
    echo "!! 无法解析 $MODULE/internal/$from 的依赖图"
    fail=1
    continue
  fi

  if has_dep "$MODULE/internal/$to" "$deps"; then
    echo "!! internal/$from 依赖了 internal/$to —— 产物之间不得互相依赖，只能共用 internal/app"
    fail=1
  else
    echo "ok internal/$from 未依赖 internal/$to"
  fi
done

# ── 测试专用包不得被生产代码依赖 ────────────────────────────────────
#
# internal/golden 会读磁盘，它只供 `_test.go` 使用。测试的 import 不在
# `go list -deps`（不带 -test）的输出里，所以这条能干净地判出"生产代码
# 误用了它"。若某天它出现在生产依赖图里，说明有东西放错了层。
# ⚠ 用显式包路径而不是 `./...`：`./...` 会去遍历 `legacy/target/`
#   （7 GB cargo 构建产物），既慢、又会在 cargo 同时在跑时撞上刚被删的临时目录
#   而**偶发失败**。理由与对价见 scripts/gates-go.sh 的 GO_PKGS 注释。
for pkg in $(go list ./cmd/... ./internal/... 2>/dev/null | grep -v "^$MODULE/internal/golden$"); do
  if has_dep "$MODULE/internal/golden" "$(deps_of "$pkg")"; then
    echo "!! $pkg 依赖了 internal/golden —— 它只供测试使用"
    fail=1
  fi
done
echo "ok 生产代码未依赖 internal/golden"

# ── 覆盖性：所有 .go 文件都必须在 cmd/ 或 internal/ 下 ──────────────
#
# 上面几条与 gates-go.sh 用的是**显式包路径**（不用 `./...`，理由见那边）。
# 代价是"新加一个顶层目录会被静默漏掉"。这条断言就是那个代价的对价：
# 漏掉的那一刻它会红，而不是让新目录**永远不被任何 Go 门禁扫到**。
#
# 用 `git ls-files` 而不是遍历文件系统：构建产物与编辑器临时文件不该参与判断。
stray="$(git ls-files '*.go' | grep -vE '^(cmd|internal)/' || true)"
if [ -n "$stray" ]; then
  echo "!! 以下 .go 文件不在 cmd/ 或 internal/ 下，不会被任何 Go 门禁覆盖："
  echo "$stray"
  echo "   → 要么把它挪进去，要么在 gates-go.sh 的 GO_PKGS 与本脚本里显式加上它。"
  fail=1
else
  echo "ok 所有 .go 文件都在 cmd/ 或 internal/ 下"
fi

if [ "$fail" -ne 0 ]; then
  echo
  echo "依赖方向门禁未通过。请把 IO 实现下沉到 internal/io，"
  echo "纯层只依赖 internal/core 中定义的契约。"
  echo "分层表见 docs/dev-gates.md「分层铁律」。"
  exit 1
fi

echo
echo "依赖方向门禁通过：${PURE_PKGS[*]} 均不含 ${FORBIDDEN_IMPORTS[*]}，且未依赖 internal/io"
