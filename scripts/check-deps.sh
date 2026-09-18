#!/usr/bin/env bash
# 依赖方向门禁。
#
# 方案 §3.2 的断言：只有 `mcloud-io` 允许依赖 reqwest / rusqlite。
# 纯层一旦引入，就说明业务逻辑里混进了 IO ——
# 而"能对某个函数单独测试"正是靠这条边界成立的。
#
# 之所以要写成脚本而不是靠 review：这条边界会被无意破坏，
# 而且破坏之后测试仍然全绿（只是不再纯净），不会自己暴露。
set -euo pipefail

FORBIDDEN=(reqwest rusqlite)

# 纯层：必须既不含 IO 库、也不依赖 mcloud-io
PURE_CRATES=(mcloud-core mcloud-upstream mcloud-capabilities mcloud-exec)

# 组装根：**必须**依赖 mcloud-io —— 它负责把 io 的实现注入 core 定义的 trait。
# 如果把组装根也当成"纯层"，门禁会永远红；而如果把组装根的检查整个去掉，
# 又会出现"没人接上 IO 实现"的静默死配置。所以两个方向都要断言。
COMPOSITION_ROOTS=(mcloud-app mcloud-server)

# 产物之间的**禁止**依赖。
#
# CLI 与 HTTP 服务是两个独立产物，它们只能共用 `mcloud-app`（组装层）。
# 一旦 CLI 依赖了 mcloud-server，命令行工具就会连带拿到 axum / 路由 / 静态资源
# 那一整套 —— 而这件事**不会让任何测试变红**，只会让二进制悄悄变大、
# 让"CLI 能在没有 HTTP 环境的地方跑"这条性质消失。所以要在这里挡住。
FORBIDDEN_EDGES=("mcloud-cli:mcloud-server")

cd "$(dirname "$0")/.."

fail=0

# 用这里的 <<< 而不是 `echo | grep`：grep -q 命中即退出会让 echo 吃到 SIGPIPE，
# 在 set -o pipefail 下会打出一行刺眼的 "write error: Broken pipe"。
dep_in_tree() {
  grep -qE "^$1[[:space:]]" <<<"$2"
}

for crate in "${PURE_CRATES[@]}"; do
  # 只看 normal（生产）依赖；dev-dependencies 允许用临时目录等测试工具
  tree=$(cargo tree -p "${crate}" --edges normal --prefix none --no-dedupe 2>/dev/null || true)

  if [ -z "$tree" ]; then
    echo "!! 无法解析 ${crate} 的依赖树（crate 是否存在？）"
    fail=1
    continue
  fi

  for dep in "${FORBIDDEN[@]}"; do
    if dep_in_tree "$dep" "$tree"; then
      echo "!! ${crate} 依赖了 $dep —— 违反 IO 隔离约束"
      fail=1
    fi
  done

  # 比"不含 rusqlite"更本质的一条：纯层不能依赖 IO 层。
  # 例如 mcloud-upstream 需要签名/设备指纹时，用的是 mcloud-core::crypto，
  # 而不是去依赖 mcloud-io（那会把 reqwest 拖进它的依赖树）。
  if dep_in_tree "mcloud-io" "$tree"; then
    echo "!! ${crate} 依赖了 mcloud-io —— 纯层不得依赖 IO 层"
    fail=1
  fi

  echo "ok ${crate}（纯层）"
done

for crate in "${COMPOSITION_ROOTS[@]}"; do
  tree=$(cargo tree -p "${crate}" --edges normal --prefix none --no-dedupe 2>/dev/null || true)

  if [ -z "$tree" ]; then
    echo "!! 无法解析 ${crate} 的依赖树"
    fail=1
    continue
  fi

  if ! dep_in_tree "mcloud-io" "$tree"; then
    echo "!! ${crate} 未依赖 mcloud-io —— 组装根必须把 IO 实现接上"
    fail=1
  fi

  echo "ok ${crate}（组装根，允许含 IO）"
done

for edge in "${FORBIDDEN_EDGES[@]}"; do
  from="${edge%%:*}"
  to="${edge##*:}"
  tree=$(cargo tree -p "${from}" --edges normal --prefix none --no-dedupe 2>/dev/null || true)

  if [ -z "$tree" ]; then
    echo "!! 无法解析 ${from} 的依赖树"
    fail=1
    continue
  fi

  # 只看 `to` 作为**依赖**出现：`cargo tree` 的首行是 crate 自己，
  # 所以这里用反引号包住的判断会误伤自己（本脚本假定的路径名不含 `-p` 前缀）
  if dep_in_tree "$to" "$tree"; then
    echo "!! ${from} 依赖了 ${to} —— 产物之间不得互相依赖，只能共用 mcloud-app"
    fail=1
  fi
  echo "ok ${from} 未依赖 ${to}"
done

if [ "$fail" -ne 0 ]; then
  echo
  echo "依赖方向门禁未通过。请把 IO 实现下沉到 mcloud-io，"
  echo "纯层只依赖 mcloud-core 中的 trait。"
  exit 1
fi

echo
echo "依赖方向门禁通过：${PURE_CRATES[*]} 均不含 ${FORBIDDEN[*]}，且未依赖 mcloud-io"
