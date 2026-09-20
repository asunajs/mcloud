#!/usr/bin/env bash
# 六项门禁一键跑（README「门禁」一节的可执行版本）。
#
# 为什么需要它：门禁靠手动跑；而手动跑会忘、会漏、会只跑其中几条。
# 这个脚本把六条串起来，任一条失败即非零退出，并打出"哪一条挂了"。
#
# 用法：
#   bash scripts/gates.sh          # 全跑
#   bash scripts/gates.sh test     # 只跑指定门禁（test|clippy|fmt|deps|idents|golden）
#
# ── Windows / Git Bash 上的工具链定位 ────────────────────────────────
# 本机 Rust 不在默认的 ~/.rustup（那里是空的，rustup 每次都会尝试联网
# 同步 stable 通道并失败，一卡几分钟）。真实位置由下面几个变量决定，
# 已在本机核实：
#   RUSTUP_HOME = /e/devtools/.rustup        工具链 stable-x86_64-pc-windows-msvc
#   CARGO_HOME  = /e/devtools/.cargo
#   MSVC        = C:\BuildTools\VC\Tools\MSVC\14.44.35207
#   SDK         = C:\Program Files (x86)\Windows Kits\10\10.0.26100.0
#
# 两个必须绕开的坑：
#   1. rustup shim 会被 PATH 命中并触发联网同步 —— 所以要把工具链的真实
#      bin 目录放在 PATH **最前面**，让 rustc/cargo 直接命中真身。
#   2. Git Bash 自带 coreutils 的 `link`（GNU link），它会顶替 MSVC 的
#      link.exe，报 `link: extra operand`。所以必须把 MSVC 的 Hostx64/x64
#      目录也放进 PATH。
#
# 如果你在 macOS / Linux 上跑，这些都不需要 —— 下面会自己判断并跳过。
set -uo pipefail

cd "$(dirname "$0")/.."

# 工具链定位（仅 Windows 需要）
# ────────────────────────────────────────────────────────────────────
# 关键：这里必须产出 **Windows 原生路径**（`C:\...`），因为 INCLUDE / LIB
# 是交给 MSVC 的 cl.exe / link.exe 读的，它们不认 `/c/...` 这种 Git Bash 形式。
# 所以不做 cygpath 往返（那是多余的，且目录里含空格时容易拼错），
# 直接用 `C:/...` 正斜杠形式 —— 正斜杠 Windows 工具完全可以接受。
TOOLCHAIN_BIN=""
if [ -d /e/devtools/.rustup/toolchains ]; then
  export RUSTUP_HOME="${RUSTUP_HOME:-/e/devtools/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-/e/devtools/.cargo}"

  # 取已安装的 stable 工具链目录（不写死 triple，换机器也能用）
  for d in "$RUSTUP_HOME"/toolchains/stable-*/bin; do
    [ -d "$d" ] && TOOLCHAIN_BIN="$d" && break
  done

  [ -n "$TOOLCHAIN_BIN" ] && export PATH="$TOOLCHAIN_BIN:$PATH"

  # MSVC 工具集：版本目录随安装变化，所以扫而不是写死
  MSVC_BIN=""
  for d in /c/BuildTools/VC/Tools/MSVC/*/bin/Hostx64/x64; do
    if [ -x "$d/link.exe" ]; then
      MSVC_BIN="$d"
      break
    fi
  done

  if [ -n "$MSVC_BIN" ]; then
    export PATH="$MSVC_BIN:$PATH"

    # 从 .../VC/Tools/MSVC/<ver>/bin/Hostx64/x64 回到 .../VC/Tools/MSVC/<ver>，
    # 正好 3 层 dirname（x64 → Hostx64 → bin）。
    MSVC_VER=$(dirname "$(dirname "$(dirname "$MSVC_BIN")")")
    MSVC_VER_WIN="C:${MSVC_VER#/c}"

    # SDK 版本目录同理扫出来（不写死 10.0.26100.0）
    SDK_INC=""
    for v in "/c/Program Files (x86)/Windows Kits/10"/Include/*; do
      [ -d "$v/ucrt" ] && SDK_INC="$v" && break
    done

    if [ -n "$SDK_INC" ]; then
      SDK_INC_WIN="C:${SDK_INC#/c}"
      SDK_LIB_WIN="${SDK_INC_WIN/\/Include\//\/Lib\/}"
      export INCLUDE="$MSVC_VER_WIN/include;$SDK_INC_WIN/ucrt;$SDK_INC_WIN/shared;$SDK_INC_WIN/um"
      export LIB="$MSVC_VER_WIN/lib/x64;$SDK_LIB_WIN/ucrt/x64;$SDK_LIB_WIN/um/x64"
    fi
  fi
fi

# 缺链接器时提前给出可读的提示，而不是让它烂在编译输出里
if command -v rustc >/dev/null 2>&1; then
  RUSTC_VER=$(rustc --version 2>&1)
else
  echo "!! 找不到 rustc。Windows 上请确认 /e/devtools/.rustup/toolchains/stable-*/bin 存在。"
  exit 1
fi

# ── 门禁定义 ────────────────────────────────────────────────────────
declare -A GATES=(
  [test]="cargo test --workspace"
  [clippy]="cargo clippy --all-targets -- -D warnings"
  [fmt]="cargo fmt --check"
  [deps]="bash scripts/check-deps.sh"
  [idents]="node scripts/check-identifiers.cjs"
  [golden]="node scripts/check-golden-vectors.cjs"
)

ORDER=(test clippy fmt deps idents golden)

if [ $# -gt 0 ]; then
  ORDER=("$@")
fi

FAILED=()

# 同时留一份完整输出：失败时用来判别"真失败"还是"环境拦了写盘"。
# 不用 `mktemp -t` —— 它在 Git Bash 上会返回 `C:\Users\...\Temp\...` 这种
# Windows 原生路径，与 MSYS 路径混用会让后续清理报 CanonicalizePath 错。
GATE_LOG="target/gates-last.log"
mkdir -p target 2>/dev/null
: > "$GATE_LOG"

for name in "${ORDER[@]}"; do
  cmd="${GATES[$name]:-}"
  if [ -z "$cmd" ]; then
    echo "!! 未知门禁: $name（可选：${!GATES[*]}）"
    exit 2
  fi

  printf '\n\033[1m──── %s ────\033[0m\n' "$name"

  # 写盘被拦时**自动重试**：本机（Windows + 杀毒软件实时扫描）会让 cargo
  # 随机地写不进 target/ 里的某一个文件，而每次撞上的文件都不一样。
  # 同一份代码换个时刻再跑通常就过了 —— 所以重试是这里**唯一**有效的对策。
  #
  # 只对 "写不进去" 这一种失败重试（`拒绝访问/os error 5/LNK1104/permission denied`），
  # 编译错误与测试失败一律立即返回 —— 否则会把真问题掩盖成"重试五次都失败"。
  attempt=1
  max_attempts=5
  while :; do
    if eval "$cmd" 2>&1 | tee -a "$GATE_LOG"; then
      printf '\033[32m✓ %s 通过\033[0m\n' "$name"
      break
    fi

    # 真·写盘被拦：cargo 打印的是 `error:` 开头的行，且带下面这些字样之一
    if [ "$attempt" -lt "$max_attempts" ] \
      && grep -qE "^(error|error\[)" "$GATE_LOG" \
      && grep -qE "拒绝访问|os error 5|LNK1104|permission denied" "$GATE_LOG"; then
      attempt=$((attempt + 1))
      printf '\033[33m· %s 写盘被拦，第 %d 次重试…\033[0m\n' "$name" "$attempt"
      : > "$GATE_LOG" # 重试前清掉上一次的输出，判别只看本次
      sleep 3
      continue
    fi

    printf '\033[31m✗ %s 失败\033[0m\n' "$name"
    FAILED+=("$name")
    break
  done
done

echo
echo "工具链: $RUSTC_VER"

# 写盘被拦的问题单独提示，免得白排查半天。
#
# 这里必须**区分警告与错误**：cargo 在收尾时会报
#   warning: failed to garbage collect finalized incremental compilation
#            session directory ...: 拒绝访问。 (os error 5)
# 这条**不影响构建结果**（增量缓存清不掉而已），但它的字样与真失败一模一样，
# 最初版本的 `grep -qs "os error 5"` 因此在"构建成功"时也会打印下面这段
# "环境问题、不是代码问题"的提示 —— 反过来会让人以为失败都是环境问题。
#
# 判据：只看**以 `error` 开头**的行（cargo 的真失败都以它打头），
# 并且排除 toolchain 缺失那类提前退出。
if [ ${#FAILED[@]} -gt 0 ] && grep -qE "^(error|error\[)" "$GATE_LOG"; then
  echo
  if grep -qE "^(error\[)|^error: could not compile" "$GATE_LOG"; then
    echo "上面有编译错误 —— 这可能是代码问题，也可能是写盘被拦（两者输出很像）。"
    echo "若错误里带 \"拒绝访问 (os error 5)\" / LNK1104，则是环境拦了 target/ 的写。"
  fi
  if grep -qE "拒绝访问|os error 5|LNK1104" "$GATE_LOG"; then
    echo
    echo "提示：出现 \"拒绝访问 (os error 5)\" —— 这通常不是代码问题。"
    echo "      cargo 需要写 target/ 与 cargo registry，被沙箱或杀毒软件拦了。"
    echo "      本仓库的做法是**重试**（同一份代码换个时刻就能过）；"
    echo "      在允许写这两处的环境里跑会更稳。"
    echo "      注意：只出现在 warning 行（incremental 清理）时可以忽略。"
  fi
fi

if [ ${#FAILED[@]} -gt 0 ]; then
  printf '\033[31m门禁未通过: %s\033[0m\n' "${FAILED[*]}"
  echo "完整输出: $GATE_LOG"
  exit 1
fi
printf '\033[32m六项门禁全绿\033[0m\n'
: > "$GATE_LOG"
