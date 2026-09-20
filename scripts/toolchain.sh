#!/usr/bin/env bash
# 工具链环境（可 source）。**存在的唯一理由**：别让人再手搓这些变量。
#
# 用法：
#   source scripts/toolchain.sh
#   cargo test -p mcloud-core            # 现在可以了
#
# ── 这是唯一的实现 ───────────────────────────────────────────────────
#
# `scripts/gates.sh` 与 `.githooks/run-gates.sh` 都 **source 本文件**，
# 自己不重复写一遍。历史上这段逻辑有三份拷贝，并且已经分叉过：
# hook 那份少了 INCLUDE / LIB 的导出，导致 Windows 上 `clippy` / `test`
# 在 hook 里链接失败、在 `gates.sh` 里却能过 —— 同一仓库、同一组命令、
# 两个不同结论。**不要再复制这段代码**，改这里。
#
# ── 为什么必须有这个文件 ─────────────────────────────────────────────
#
# 本机 Rust 不在默认的 `~/.rustup`（那里是空的）。直接跑 `cargo` 会命中
# rustup 的 shim，它会**尝试联网同步 stable 通道**并下载一整套新工具链 ——
# 实测表现为"卡住 6 分钟然后超时"，而且它中途会 `recovering from a
# partially installed toolchain`、把正在用的 toolchain 组件删了重装。
# 那比构建失败危险得多。
#
# Windows 上两条必须同时满足：
#   1. `RUSTUP_HOME` / `CARGO_HOME` 指向真实位置（`E:/...` 正斜杠形式；
#      用 MSYS 的 `/e/...` 会被拼成 `E:/e/...` 而再次触发联网）。
#   2. 工具链的真实 `bin` 目录**放在 PATH 最前面**，让 `cargo` / `rustc`
#      直接命中真身、根本不经过 shim。
#
# 另：Git Bash 自带 `/usr/bin/link.exe`（GNU link），会顶替 MSVC 的
# link.exe 并报 `link: extra operand`，所以 MSVC 的 `Hostx64/x64`
# 也必须进 PATH。
#
# ── 按平台分支，而不是探测某台机器的路径 ────────────────────────────
#
# 三种环境被同等对待：本机 Windows+Git Bash / macOS 与 Linux /
# 已显式设好 RUSTUP_HOME 的场景。见下面 case 各段自己的说明。
#
# 只做**定位**：不安装、不联网。找不到就报错（调用方决定是退出还是继续）。

# 幂等：被 source 两次不重复往 PATH 里塞。
if [ -n "${MCLOUD_TOOLCHAIN_READY:-}" ]; then
  return 0 2>/dev/null || exit 0
fi

_mcloud_platform="$(uname -s 2>/dev/null || echo unknown)"

case "$_mcloud_platform" in
  MINGW* | MSYS* | CYGWIN*)
    # ── Windows + Git Bash ──────────────────────────────────────────
    # 两个坑见文件头的说明（rustup shim / GNU link）。
    #
    # INCLUDE / LIB 必须是 **Windows 原生路径**（`C:/...`），因为它们是交给
    # MSVC 的 cl.exe / link.exe 读的，不认 `/c/...` 这种 Git Bash 形式。
    # 不做 cygpath 往返（目录含空格时容易拼错），直接用 `C:/...` ——
    # 正斜杠 Windows 工具完全可以接受。
    export RUSTUP_HOME="${RUSTUP_HOME:-E:/devtools/.rustup}"
    export CARGO_HOME="${CARGO_HOME:-E:/devtools/.cargo}"

    # 取已安装的 stable 工具链目录（不写死 triple，换机器也能用）
    _tc_bin=""
    for d in /e/devtools/.rustup/toolchains/stable-*/bin; do
      [ -d "$d" ] && _tc_bin="$d" && break
    done

    if [ -n "$_tc_bin" ]; then
      export PATH="$_tc_bin:$PATH"
      # 形如 stable-x86_64-pc-windows-msvc。钉住它，避免 rustup 去找别的通道。
      export RUSTUP_TOOLCHAIN="$(basename "$(dirname "$_tc_bin")")"
    else
      echo "!! 找不到 Rust 工具链：\$RUSTUP_HOME=$RUSTUP_HOME 下没有 toolchains/stable-*/bin" >&2
      return 1 2>/dev/null || exit 1
    fi

    # MSVC 工具集：版本目录随安装变化，所以扫而不是写死
    _msvc_bin=""
    for d in /c/BuildTools/VC/Tools/MSVC/*/bin/Hostx64/x64; do
      if [ -x "$d/link.exe" ]; then
        _msvc_bin="$d"
        break
      fi
    done

    if [ -n "$_msvc_bin" ]; then
      export PATH="$_msvc_bin:$PATH"

      # 从 .../VC/Tools/MSVC/<ver>/bin/Hostx64/x64 回到 <ver>：3 层 dirname
      _msvc_ver=$(dirname "$(dirname "$(dirname "$_msvc_bin")")")
      _msvc_ver_win="C:${_msvc_ver#/c}"

      _sdk_inc=""
      for v in "/c/Program Files (x86)/Windows Kits/10"/Include/*; do
        [ -d "$v/ucrt" ] && _sdk_inc="$v" && break
      done

      if [ -n "$_sdk_inc" ]; then
        _sdk_inc_win="C:${_sdk_inc#/c}"
        _sdk_lib_win="${_sdk_inc_win/\/Include\//\/Lib\/}"
        export INCLUDE="$_msvc_ver_win/include;$_sdk_inc_win/ucrt;$_sdk_inc_win/shared;$_sdk_inc_win/um"
        export LIB="$_msvc_ver_win/lib/x64;$_sdk_lib_win/ucrt/x64;$_sdk_lib_win/um/x64"
      else
        echo "!! 找不到 Windows SDK 的 Include 目录（要 /c/Program Files (x86)/Windows Kits/10/Include/<ver>/ucrt）" >&2
        return 1 2>/dev/null || exit 1
      fi
    else
      echo "!! 找不到 MSVC 的 link.exe（要 /c/BuildTools/VC/Tools/MSVC/*/bin/Hostx64/x64/link.exe）" >&2
      return 1 2>/dev/null || exit 1
    fi
    ;;

  Darwin | Linux)
    # ── macOS / Linux ───────────────────────────────────────────────
    # 装好的 rustup 自己在 PATH 上，不需要补任何东西。
    # 这一段**故意是空的**，但保留它 —— 它让"在类 Unix 上跑"成为一条
    # 被写下来的路径，而不是靠 case 落空悄悄跳过。
    # 要在这里加东西之前，先读文件头 Windows 那段的两个坑，
    # 确认不是把某台机器的特殊路径又写了进来。
    ;;

  *)
    echo "!! 未识别的平台：$_mcloud_platform" >&2
    echo "   本脚本只在 Windows + Git Bash 与 macOS / Linux 上验证过。" >&2
    echo "   若你在别的环境（WSL / 容器 / BSD）上跑，请把结论补进 docs/dev-gates.md 的「跨环境」一节。" >&2
    # 不退出：平台不认识不等于工具链不可用（比如 WSL 里 rustup 装好了）。
    # 让下面的 rustc 探测去决定成败。
    ;;
esac

if ! command -v rustc >/dev/null 2>&1; then
  echo "!! 找不到 rustc（当前平台：$_mcloud_platform）。" >&2
  echo "   Windows + Git Bash：确认 \$RUSTUP_HOME（默认 E:/devtools/.rustup）下有 toolchains/stable-*/bin。" >&2
  echo "   macOS / Linux：     确认 rustup 已装且在 PATH 上（rustup show）。" >&2
  echo "   本脚本只定位、不安装 —— 也不要靠它联网装工具链。" >&2
  return 1 2>/dev/null || exit 1
fi

export MCLOUD_TOOLCHAIN_READY=1
export MCLOUD_RUSTC_VERSION="$(rustc --version 2>&1)"
