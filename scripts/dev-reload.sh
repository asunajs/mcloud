#!/usr/bin/env bash
#
# 改完 Rust 之后：**重建 → 签名 → 重启托管实例**，一条命令。
#
# # 为什么不是三行命令拼一下就行
#
# 顺序里有个坑：`cargo build` 会**原地覆写**正在跑的那个文件 ✗ ⇒ 内核可能把进程
# 判为"代码签名失效"直接 SIGKILL，而 `KeepAlive: true` 会立刻把它拉起来 ——
# 拉起来的是**还没签名的**新文件 ✗ ⇒ 又弹一次授权框（白签了）。
#
# 所以这里是 **先 `bootout` 停掉托管、再编、再签、最后 `bootstrap`** ✓：
# 整个编译期间没有进程会去 exec 那个半成品，弹框的窗口被彻底关掉 ✓。
#
# ⚠ 代价：编译那 1~2 分钟 WebUI 是**不可用**的（本地开发工具，可接受 ✓）。
#   只想改前端的话**不要**用这个脚本 —— 前端 `npm run build` + 刷新页面即可，
#   二进制压根不用重编（见 AGENTS.md「构建顺序」）。
#
# # 用法
#
#   scripts/dev-reload.sh              # 重建 + 签名 + 重启
#   scripts/dev-reload.sh --no-build   # 不重建，只重新签名 + 重启
#
# 代理 label 按关键词从 `launchctl list` 里发现（仓库里不写死用户名 ✓），
# 要覆盖就设 MCLOUD_LAUNCHD_LABEL。

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL="${MCLOUD_LAUNCHD_LABEL:-$(launchctl list 2>/dev/null | awk '$3 ~ /mcloud-webui/ { print $3; exit }')}"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
UID_="$(id -u)"

if [[ -z "$LABEL" || ! -f "$PLIST" ]]; then
  echo "✗ 找不到托管实例的 label/plist（label='${LABEL:-}'，plist=$PLIST）" >&2
  echo "  设 MCLOUD_LAUNCHD_LABEL=<label> 覆盖。" >&2
  exit 1
fi

echo "── ① 停掉托管（$LABEL）"
# ⚠ `kickstart -k` **不重读 plist** ✗ —— 改过 plist 就必须 bootout+bootstrap（踩过）
launchctl bootout "gui/$UID_/$LABEL" 2>/dev/null || true

if [[ "${1:-}" != "--no-build" ]]; then
  echo "── ② 重建（cargo build -p mcloud-server --bin mcloud）"
  cd "$REPO"
  # ⚠ 前端产物要先 `npm run build`（rust-embed 在编译期内嵌；cargo 不跟踪它 ✗）
  cargo build -p mcloud-server --bin mcloud
else
  echo "── ② 跳过重建（--no-build）"
  cd "$REPO"
fi

echo "── ③ 签名（固定身份 ⇒ 授权在重编后依然匹配 ✓）"
"$REPO/scripts/sign-local.sh"

echo "── ④ 重新加载并启动"
launchctl bootstrap "gui/$UID_" "$PLIST"
for _ in $(seq 1 24); do
  sleep 2
  if curl -s -o /dev/null --max-time 2 http://127.0.0.1:3000/api/health 2>/dev/null; then
    echo "✓ 已起来：$(curl -s --max-time 5 http://127.0.0.1:3000/api/health)"
    exit 0
  fi
done

cat >&2 <<EOF
✗ 180 秒内 3000 还没监听。

  先看两处：
    launchctl list | awk '\$3=="${LABEL}"'            # 第 2 列非 0 ⇒ 上次退出码非 0
    tail -5 ~/Library/Logs/mcloud-gost/webui.err.log  # 找 "Address already in use"

  ⚠ 若日志一片空白、进程存在却不监听：那是**卡在等授权框**（dyld 的 open() 被扣住）。
    别 kill 它 —— 点一下「允许」同一个进程会自己接着跑 ✓。
EOF
exit 1
