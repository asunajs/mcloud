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
# ⚠ label 的发现要**两步**：先问 `launchctl list`（正跑着的那个最权威 ✓），
# 问不到就**退回磁盘上的 plist** ✓。
#
# 为什么非要有第二步（2026-10-03 实测踩到，代价是 WebUI 停了约 30 分钟 ✗）：
# 上一次上线只要停在 `bootout` 之后（脚本被中断、编译报错、机器重启），
# 服务就**已经不在** `launchctl list` 里了 ✗ —— 而那恰恰是最需要这个脚本
# 把服务拉回来的时刻 ✓。只有第一步的话它会直接以"找不到 label"退出 ✗，
# 于是"停掉的那个"再也没人拉起来 ✗。
# ⚠ `|| true` 不能省：`launchctl list` 在**被沙箱包住的 shell** 里会 exit 1（空输出 ✓，
# 实测：DSH 的 bash 沙箱下如此，放宽后同一条命令 exit 0 / 527 行 ✓）。而
# `set -e` 下 **`VAR=$(失败的命令)` 这行赋值本身就会终止脚本** ✗ —— 位置恰好在下面
# trap **安装之前** ⇒ 表现是「无任何输出、exit 1、服务没被碰但也没人知道为什么」✗
# （2026-10-04 实测踩到一次 ✓）。所以这里把它兜成"发现不到 label"这条**有出口**的路 ✓。
LABEL="${MCLOUD_LAUNCHD_LABEL:-}"
if [[ -z "$LABEL" ]]; then
  LABEL="$(launchctl list 2>/dev/null | awk '$3 ~ /mcloud-webui/ { print $3; exit }' || true)"
fi
if [[ -z "$LABEL" ]]; then
  # 从 plist 文件名反推 label（排除 `.bak-*` 那几份备份 ✓）
  PLIST_FILE="$(ls -1 "$HOME/Library/LaunchAgents/" 2>/dev/null \
    | grep -E 'mcloud.*\.plist$' | grep -v '\.bak' | head -1 || true)"
  LABEL="${PLIST_FILE%.plist}"
  [[ -n "$LABEL" ]] && echo "⚠ 托管当前**没在跑**（launchctl 里找不到）⇒ 按磁盘上的 plist 认它：$LABEL"
fi
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
UID_="$(id -u)"

if [[ -z "$LABEL" || ! -f "$PLIST" ]]; then
  echo "✗ 找不到托管实例的 label/plist（label='${LABEL:-}'，plist=$PLIST）" >&2
  echo "  设 MCLOUD_LAUNCHD_LABEL=<label> 覆盖。" >&2
  exit 1
fi

# ⚠⚠ 兜底：从下面那句 `bootout` 起，服务就是**停的** ✓ —— 这段窗口里任何一步失败
# （编译报错、脚本被 Ctrl-C、被上层工具超时掐掉）都会把它**留在停机状态** ✗。
# 2026-10-03 实测踩到过：两次中断 ⇒ WebUI 停了约 30 分钟 ✓（用户当场看出来 ✗）。
# ⇒ 只要**真的停过**（见下面 `STOPPED`），无论怎么退出都把它拉回来 ✓；先补一次签名，否则未签名的二进制会被 TCC
# 扣住、起而不监听 ✗（`|| true` 是必要的：异常退出时二进制可能正被写坏）。
# ⚠ 只有**真的 bootout 过**才需要把服务拉回来（2026-10-04 review dc-17）：
# 下面那条"别的 cargo 在跑"的早退路径也会 `exit 1`，但它**没停过服务** ⇒
# 那时去签名 + `bootstrap` 是白做，而且可能正撞上另一条线在写
# `target/debug/mcloud` ✗（`codesign` 与 `cargo` 抢同一个文件）。
STOPPED=0
restore_on_exit() {
  local code=$?
  if [[ $code -ne 0 && "$STOPPED" == "1" ]]; then
    echo "── ⚠ 中途失败（exit $code）⇒ 兜底把托管拉回来（先补签名）" >&2
    "$REPO/scripts/sign-local.sh" >/dev/null 2>&1 || true
    launchctl bootstrap "gui/$UID_" "$PLIST" 2>/dev/null || true
  fi
  exit "$code"
}
trap restore_on_exit EXIT

echo "── ① 停掉托管（$LABEL）"

# ⚠⚠ 停之前先看有没有**别的写入线**在编译 —— 两个理由：
#
# 1. cargo 对 `target/` 上独占锁，硬抢两边都慢（AGENTS.md 那条 ✓）。
# 2. 更要命的是本脚本**先停服务再编** ✗ ⇒ 一旦在锁上等几分钟，WebUI 就白停几分钟 ✗
#    （2026-10-03 实测踩到两次：一次约 30 分钟 ✓）。
#
# ⇒ 有别人在编就**先不动服务** ✓、明确报出来 ✓（要硬上就设 MCLOUD_SKIP_LOCK_CHECK=1）。
if [[ "${MCLOUD_SKIP_LOCK_CHECK:-0}" != "1" ]]; then
  OTHERS="$(pgrep -fl 'cargo (build|test|clippy|check|run)' 2>/dev/null | grep -v "$$" || true)"
  if [[ -n "$OTHERS" ]]; then
    echo "⚠ 检测到别的 cargo 在跑 ⇒ **不动服务**（它还在正常提供 WebUI ✓）：" >&2
    echo "$OTHERS" | sed 's/^/    /' >&2
    echo "  等它结束再跑一次；坚持要现在上就 MCLOUD_SKIP_LOCK_CHECK=1 $0" >&2
    exit 1
  fi
fi

# ⚠ `kickstart -k` **不重读 plist** ✗ —— 改过 plist 就必须 bootout+bootstrap（踩过）
launchctl bootout "gui/$UID_/$LABEL" 2>/dev/null || true
STOPPED=1   # 从这一行起服务是**停的** ⇒ 之后任何失败都由 trap 拉回来 ✓

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
# ⚠ 这一步在**被沙箱包住的 shell** 里会失败成 `Bootstrap failed: 5: Input/output error` ✗
# （2026-10-04 实测：同一个 plist、同一条命令，沙箱内 exit 5，放宽后 exit 0 ✓）
# ⇒ 报错里要带一句指路，否则会被误当成"plist 坏了"去瞎改 ✗。
if ! launchctl bootstrap "gui/$UID_" "$PLIST"; then
  echo "⚠ bootstrap 失败 ⇒ 上面若是 \`5: Input/output error\`，先怀疑 shell 被沙箱包着（不是 plist 的问题 ✗）" >&2
  exit 1
fi
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
    tail -5 ~/Library/Logs/mcloud/webui.err.log      # 找 "Address already in use"

  ⚠ 若日志一片空白、进程存在却不监听：那是**卡在等授权框**（dyld 的 open() 被扣住）。
    别 kill 它 —— 点一下「允许」同一个进程会自己接着跑 ✓。
EOF
exit 1
