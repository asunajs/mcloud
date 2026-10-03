#!/usr/bin/env bash
#
# 从**内置盘**运行 mcloud，避开 macOS 的「可移除宗卷」授权弹窗。
#
# # 为什么需要这个脚本
#
# 工作区在**外置盘**上（USB 挂载的 APFS 卷这类可移除宗卷）。
# macOS 对**可移除宗卷**有隐私保护（TCC，服务名
# `kTCCServiceSystemPolicyRemovableVolumes`）：任何可执行文件首次访问
# 该卷上的文件，系统就弹一次「"mcloud" 想访问可移除宗卷上的文件」。
#
# 而这个弹窗是**反复**弹的，原因有两条叠加：
#
# 1. **每次 `cargo build` 都是一个新二进制**。TCC 按「路径 + 代码要求」记授权，
#    新产物的代码要求匹配不上旧记录 ⇒ 重新弹。系统日志里的原话是
#    `Failed to match existing code requirement for subject .../target/debug/mcloud`。
# 2. 进程在**外置盘上**（`target/debug/mcloud`），且要读外置盘上的 `data/`。
#
# ⇒ 光把二进制拷到内置盘**不够**（它照样读外置盘的 `data/`）。
#   必须让**二进制与数据目录都在内置盘**，进程才完全不碰那个卷。
#
# # 用法
#
#   scripts/run-local.sh              # 默认 127.0.0.1:3000
#   MCLOUD_ADDR=127.0.0.1:3999 scripts/run-local.sh
#
# 首次运行会把 `data/` 复制到内置盘；之后**以内置盘那份为准**。
# 要同步回仓库，用 `scripts/run-local.sh --sync-back`。
#
# ⚠ 编译照旧在外置盘（`target/` 有 18G，内置盘放不下）。
#   只有「**运行**」这一步搬到内置盘 —— 那一步才会触发 TCC。

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="${MCLOUD_RUN_DIR:-$HOME/dsh-run}"
BIN="$RUN_DIR/mcloud"
DATA="$RUN_DIR/data"
ADDR="${MCLOUD_ADDR:-127.0.0.1:3000}"

# 同步内置盘的数据回仓库（当你想把运行时产生的会话/日志带回去时用）。
if [[ "${1:-}" == "--sync-back" ]]; then
  if [[ -d "$DATA" ]]; then
    mkdir -p "$REPO/data"
    cp -r "$DATA/." "$REPO/data/"
    echo "已把 $DATA 同步回 $REPO/data"
  else
    echo "没有可同步的数据目录：$DATA" >&2
    exit 1
  fi
  exit 0
fi

mkdir -p "$RUN_DIR"

# ⓪ 守卫：**绝不**与 launchd 托管的实例抢 3000 端口。
#
# 这是血泪教训：本脚本早先没有这道守卫，于是在 launchd 代理
# （`~/Library/LaunchAgents/<label>.plist`，`KeepAlive: true`）
# 占着 3000 时又起了一个实例。结果是**崩溃重启循环**：
# launchd 那份绑不上端口就退出，KeepAlive 立刻重拉，如此往复 ——
# 每 20 秒（launchd 的重启节流）就会新起一个进程，
# 而每个新进程访问外置卷都会触发一次 macOS 的
# 「想访问可移除宗卷上的文件」弹窗，一天刷出 1000+ 条记录。
#
# 所以：有托管实例在跑就直接退出，把选择权交回用户。
# 要用本脚本（内置盘、不弹框）跑，先手动 `launchctl bootout` 那个代理。
#
# ⚠ 代理的 label 因机器而异（形如 `com.<用户名>.mcloud-webui`），这里**按关键词
#   从 launchctl 里发现**它 —— 仓库里不写死任何人的用户名。要覆盖就设
#   `MCLOUD_LAUNCHD_LABEL`。
LAUNCHD_LABEL="${MCLOUD_LAUNCHD_LABEL:-$(launchctl list 2>/dev/null | awk '$3 ~ /mcloud-webui/ { print $3; exit }')}"
if [[ -n "$LAUNCHD_LABEL" ]]; then
  cat >&2 <<EOF
✗ 检测到 launchd 代理 $LAUNCHD_LABEL 已加载。

  它在跑 <仓库>/target/debug/mcloud（外置盘），并带 KeepAlive。
  本脚本若同时启动，两份会**抢 3000 端口**，失败的那份被 KeepAlive
  反复重拉，每次重拉都触发一次 macOS「可移除宗卷」授权弹窗。

  二选一：

  A. 就用托管那份（推荐 —— 正常开发不弹窗）：
       什么都不用做，WebUI 已在 http://127.0.0.1:3000

  B. 改用本脚本（内置盘，连重编译后的弹窗也免掉）：
       launchctl bootout gui/$(id -u)/$LAUNCHD_LABEL
       # 想恢复托管：launchctl bootstrap gui/$(id -u) \\
       #   ~/Library/LaunchAgents/$LAUNCHD_LABEL.plist
EOF
  exit 1
fi

# ① 二进制：从仓库的构建产物拷到内置盘。
#    ⚠ 每次重新编译后都要再拷一次 —— 这正是弹窗的触发点。
SRC="$REPO/target/debug/mcloud"
if [[ ! -x "$SRC" ]]; then
  echo "构建产物不存在：$SRC" >&2
  echo "先跑：cargo build -p mcloud-server --bin mcloud" >&2
  exit 1
fi
if [[ ! -x "$BIN" || "$SRC" -nt "$BIN" ]]; then
  cp "$SRC" "$BIN"
  echo "已更新内置盘上的二进制：$BIN"
fi

# ② 数据目录：首次从仓库拷一份；已存在就**不覆盖**（那是运行时的真实状态）。
if [[ ! -d "$DATA" ]]; then
  mkdir -p "$DATA"
  if [[ -d "$REPO/data" ]]; then
    cp -r "$REPO/data/." "$DATA/"
    echo "已把 data/ 复制到内置盘：$DATA"
  fi
fi

echo "启动：$BIN"
echo "  地址：http://$ADDR"
echo "  数据：$DATA（内置盘 —— 进程不碰外置卷，所以不再弹授权框）"

# ③ 工作目录也设成内置盘：`process.cwd()` 参与 TCC 判定。
cd "$RUN_DIR"
exec env MCLOUD_ADDR="$ADDR" MCLOUD_DATA_DIR="$DATA" "$BIN"

