#!/usr/bin/env bash
# 七项门禁一键跑（docs/dev-gates.md「七条门禁」一节的可执行版本）。
#
# 为什么需要它：门禁靠手动跑；而手动跑会忘、会漏、会只跑其中几条。
# 这个脚本把七条串起来，任一条失败即非零退出，并打出"哪一条挂了"。
#
# 用法：
#   bash scripts/gates.sh          # 全跑
#   bash scripts/gates.sh test     # 只跑指定门禁（test|clippy|fmt|deps|filenames|idents|golden）
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

# 工具链定位：**唯一实现在 `scripts/toolchain.sh`**，这里只 source。
#
# 不要在这下面再写一份 —— 历史上这段逻辑有三份拷贝且已经分叉过：
# hook 那份少了 INCLUDE / LIB，导致 Windows 上 `clippy` / `test` 在 hook 里
# 链接失败、在这里却能过。同一个仓库、同一组命令，两处结论不同。
#
# 找不到工具链时它自己会打印"缺什么、去哪看"，然后返回非零 —— 我们直接退出。
. scripts/toolchain.sh || exit 1

# ── 门禁定义 ────────────────────────────────────────────────────────
#
# ⚠ 这里**故意不用 `declare -A`**（关联数组要 bash 4.0+）。
#
# macOS 自带的 `/bin/bash` 是 **3.2.57** —— Apple 因为 GPLv3 停在了 2007 年
# 那个版本，且不会升。`bash` 在 PATH 上默认就是它（Homebrew 的 bash 5 得
# 自己装、且通常叫 `/opt/homebrew/bin/bash`）。
#
# 失败形态极其难认：`declare -A` 报 invalid option 后，`GATES` 会退化成
# **普通索引数组**，于是紧跟的 `[test]="…"` 里那个 `[test]` 被当成
# **算术下标**求值 —— 变量 `test` 未定义，在 `set -u` 下直接：
#
#     scripts/gates.sh: line 42: test: unbound variable
#
# 脚本一行都没跑就退出，退出码还是 0。本文件明明有 `Darwin | Linux` 分支
# （见 `scripts/toolchain.sh`），即"支持 macOS"是写下来的意图 ——
# 所以这不是"不支持"，是**在支持的平台上悄悄不工作**。
# 它在 Git Bash（bash 4.4/5.x）上测不出来，只在 macOS 上现形。
#
# `case` 分派是 3.2 就能跑的等价写法，没有理由为它要求 bash 4。
GATE_NAMES="test clippy fmt deps filenames idents golden"

gate_cmd() {
  case "$1" in
    test)      echo "cargo test --workspace" ;;
    clippy)    echo "cargo clippy --all-targets -- -D warnings" ;;
    fmt)       echo "cargo fmt --check" ;;
    deps)      echo "bash scripts/check-deps.sh" ;;
    filenames) echo "bash scripts/check-filenames.sh" ;;
    idents)    echo "node scripts/check-identifiers.cjs" ;;
    golden)    echo "node scripts/check-golden-vectors.cjs" ;;
    *)         echo "" ;;
  esac
}

ORDER=(test clippy fmt deps filenames idents golden)

if [ $# -gt 0 ]; then
  ORDER=("$@")
fi

FAILED=()
# 失败门禁的**名称 → 本次输出文件**。诊断只看本次输出，不看累积日志。
FAILED_LOGS=()

# 同时留一份完整输出（跨门禁累积，供人工翻看）。
# 不用 `mktemp -t` —— 它在 Git Bash 上会返回 `C:\Users\...\Temp\...` 这种
# Windows 原生路径，与 MSYS 路径混用会让后续清理报 CanonicalizePath 错。
GATE_LOG="target/gates-last.log"
# 每个门禁这次跑的输出单独一个文件。
#
# # 为什么必须分开（这是修掉的一个真实误报）
#
# 原来所有门禁都往同一个 `$GATE_LOG` 里 `tee -a` 追加，而末尾的环境诊断
# 是 `grep` 整个文件的。于是**前面成功的门禁留下的字样会污染后面的诊断**：
# 实测 `clippy` 因一个多余的 `return` 真失败时，日志里因为别处带着
# "拒绝访问" 而打印了"这通常不是代码问题"—— 把人往错误方向带。
# 分开之后，诊断只看"挂掉的这条门禁自己说了什么"。
mkdir -p target 2>/dev/null
: > "$GATE_LOG"

for name in "${ORDER[@]}"; do
  cmd="$(gate_cmd "$name")"
  if [ -z "$cmd" ]; then
    echo "!! 未知门禁: $name（可选：$GATE_NAMES）"
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
  # 本次门禁的输出文件（每次重试覆盖；诊断只认它）
  RUN_LOG="target/gates-${name}.log"
  while :; do
    if eval "$cmd" >"$RUN_LOG" 2>&1; then
      cat "$RUN_LOG"                              # 通过也照常显示，保持原观感
      cat "$RUN_LOG" >> "$GATE_LOG"
      printf '\033[32m✓ %s 通过\033[0m\n' "$name"
      break
    fi
    cat "$RUN_LOG"

    # 真·写盘被拦：cargo 打印的是 `error:` 开头的行，且带下面这些字样之一
    if [ "$attempt" -lt "$max_attempts" ] \
      && grep -qE "^(error|error\[)" "$RUN_LOG" \
      && grep -qE "拒绝访问|os error 5|LNK1104|permission denied" "$RUN_LOG"; then
      attempt=$((attempt + 1))
      printf '\033[33m· %s 写盘被拦，第 %d 次重试…\033[0m\n' "$name" "$attempt"
      sleep 3
      continue
    fi

    printf '\033[31m✗ %s 失败\033[0m\n' "$name"
    FAILED+=("$name")
    FAILED_LOGS+=("$RUN_LOG")
    cat "$RUN_LOG" >> "$GATE_LOG"
    break
  done
done

echo
echo "工具链: $MCLOUD_RUSTC_VERSION"

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
# 并且**只看挂掉那条门禁自己的输出**（`$FAILED_LOGS`），不看累积日志 ——
# 累积日志会被前面通过的门禁污染（见上面 `RUN_LOG` 的说明）。
for i in "${!FAILED[@]}"; do
  name="${FAILED[$i]}"
  log="${FAILED_LOGS[$i]}"
  [ -f "$log" ] || continue

  if grep -qE "^(error\[)|^error: could not compile" "$log"; then
    echo
    echo "[$name] 上面有编译错误 —— 这可能是代码问题，也可能是写盘被拦（两者输出很像）。"
    echo "        若错误里带 \"拒绝访问 (os error 5)\" / LNK1104，则是环境拦了 target/ 的写。"
  fi
  if grep -qE "拒绝访问|os error 5|LNK1104" "$log"; then
    echo
    echo "[$name] 出现 \"拒绝访问 (os error 5)\" —— 这条有环境因素的嫌疑："
    echo "         cargo 需要写 target/ 与 cargo registry，可能被沙箱或杀毒软件拦了。"
    echo "         本仓库的做法是**重试**（同一份代码换个时刻就能过）。"
    echo "         注意：只出现在 warning 行（incremental 清理）时可以忽略。"
  fi
done

if [ ${#FAILED[@]} -gt 0 ]; then
  printf '\033[31m门禁未通过: %s\033[0m\n' "${FAILED[*]}"
  echo "完整输出: $GATE_LOG"
  exit 1
fi
printf '\033[32m七项门禁全绿\033[0m\n'
: > "$GATE_LOG"
