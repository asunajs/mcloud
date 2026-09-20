#!/usr/bin/env bash
# 工具链环境（可 source）。**存在的唯一理由**：别让人再手搓这些变量。
#
# 用法：
#   source scripts/toolchain.sh
#   cargo test -p mcloud-core            # 现在可以了
#
# ── 为什么必须有这个文件 ─────────────────────────────────────────────
#
# 本机 Rust 不在默认的 `~/.rustup`（那里是空的）。直接跑 `cargo` 会命中
# rustup 的 shim，它会**尝试联网同步 stable 通道**并下载一整套新工具链 ——
# 实测表现为"卡住 6 分钟然后超时"，而且它中途会 `recovering from a
# partially installed toolchain`、把正在用的 toolchain 组件删了重装。
# 那比构建失败危险得多。
#
# 两条必须同时满足：
#   1. `RUSTUP_HOME` / `CARGO_HOME` 指向真实位置（`E:/...` 正斜杠形式；
#      用 MSYS 的 `/e/...` 会被拼成 `E:/e/...` 而再次触发联网）。
#   2. 工具链的真实 `bin` 目录**放在 PATH 最前面**，让 `cargo` / `rustc`
#      直接命中真身、根本不经过 shim。
#
# 另：Git Bash 自带 `/usr/bin/link.exe`（GNU link），会顶替 MSVC 的
# link.exe 并报 `link: extra operand`，所以 MSVC 的 `Hostx64/x64`
# 也必须进 PATH。
#
# 逻辑与 `scripts/gates.sh` 的头部**保持一致**（那里是权威实现）。
# 改一处要同步另一处 —— 两边分叉的后果是"门禁能过、手工跑不过"，
# 那正是最难排查的一类。

# 只在 Windows / Git Bash 下处理；其他平台原样不动。
if [ -d /e/devtools/.rustup/toolchains ]; then
  export RUSTUP_HOME="${RUSTUP_HOME:-E:/devtools/.rustup}"
  export CARGO_HOME="${CARGO_HOME:-E:/devtools/.cargo}"

  # 钉住工具链，避免 rustup 去找别的通道
  RUSTUP_TOOLCHAIN_BIN=""
  for d in /e/devtools/.rustup/toolchains/stable-*/bin; do
    [ -d "$d" ] && RUSTUP_TOOLCHAIN_BIN="$d" && break
  done
  if [ -n "$RUSTUP_TOOLCHAIN_BIN" ]; then
    export PATH="$RUSTUP_TOOLCHAIN_BIN:$PATH"
    # 形如 stable-x86_64-pc-windows-msvc
    TOOLCHAIN_NAME=$(basename "$(dirname "$RUSTUP_TOOLCHAIN_BIN")")
    export RUSTUP_TOOLCHAIN="$TOOLCHAIN_NAME"
  fi

  # MSVC：版本目录随安装变化，所以扫而不是写死
  for d in /c/BuildTools/VC/Tools/MSVC/*/bin/Hostx64/x64; do
    if [ -x "$d/link.exe" ]; then
      export PATH="$d:$PATH"

      # 从 .../VC/Tools/MSVC/<ver>/bin/Hostx64/x64 回到 <ver>：3 层 dirname
      MSVC_VER=$(dirname "$(dirname "$(dirname "$d")")")
      MSVC_VER_WIN="C:${MSVC_VER#/c}"

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
      break
    fi
  done
fi
