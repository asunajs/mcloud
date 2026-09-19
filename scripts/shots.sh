#!/usr/bin/env bash
# 真实设备视口下的 WebUI 截图 / 布局体检。
#
# 为什么需要这个脚本（而不是直接 `msedge --headless --screenshot`）：
#   Edge / Chrome 的 headless **窗口**有最小宽度限制（实测约 492px）。
#   传 `--window-size=390,844` 会被静默钳到 492，
#   于是截出来的"移动端"其实是 492px 宽的桌面端 —— 卡片看起来"溢出屏幕"，
#   但那不是 CSS 的错，是截图工具的错。
#   实证：在 `--window-size=390` 下，页面里 `window.innerWidth` 仍是 492。
#
#   正确做法是走 CDP 的 `Emulation.setDeviceMetricsOverride` —— 它改的是
#   **渲染视口**而不是操作系统窗口，能真正拿到 390×844。
#   脚本每次都会回读 `innerWidth/innerHeight` 复核，不让你靠猜。
#
# 用法：
#   bash scripts/shots.sh                 # 全流程：起服务 → 建管理员 → 截图 → 停服务
#   bash scripts/shots.sh inspect         # 只做布局体检（不写图）
#
# 产物落在 data/（已在 .gitignore 里，不会入库）。

set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

PORT="${MCLOUD_PORT:-8787}"
DB="data/preview.db"
EDGE="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
NODE="C:/Users/Administrator/.workbuddy-ai/binaries/node/versions/22.22.2-2/node.exe"
PROFILE="E:/workspace/mcloud/data/cdp-profile"
TMP="data/tmp"
MODE="${1:-shots}"

# 系统装了 HTTP 代理，会把 localhost 的请求也拦下来 —— curl 必须绕开
export NO_PROXY='127.0.0.1,localhost'
export no_proxy='127.0.0.1,localhost'

if [ ! -x "$EDGE" ]; then
  echo "找不到 Edge：$EDGE" >&2
  echo "（agent-browser 的 Chromium 下载在本机不通，所以用系统自带的 Edge）" >&2
  exit 1
fi

mkdir -p data "$TMP"

# ── 清理上一轮残留 ────────────────────────────────────────────────
# 数据库被上一个服务进程占着时 rm 会失败，所以先按端口找进程杀掉。
if netstat -ano 2>/dev/null | grep -q ":$PORT .*LISTENING"; then
  echo "端口 $PORT 被占用，先清掉旧进程…"
  PID=$(netstat -ano 2>/dev/null | grep ":$PORT .*LISTENING" | awk '{print $NF}' | head -1)
  if [ -n "$PID" ]; then
    powershell.exe -NoProfile -Command "Stop-Process -Id $PID -Force" 2>/dev/null || true
    sleep 1
  fi
fi
rm -f "$DB" "$DB-shm" "$DB-wal" 2>/dev/null || true
rm -rf "$PROFILE"

# ── 起服务 ────────────────────────────────────────────────────────
./target/debug/mcloud-server.exe --db "$DB" --port "$PORT" > data/srv.log 2>&1 &
SRV=$!
for _ in $(seq 1 40); do
  code=$(curl -s --noproxy '*' -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/setup" 2>/dev/null || true)
  [ "$code" = "200" ] && break
  sleep 0.5
done
if [ "${code:-}" != "200" ]; then
  echo "服务没起来，看 data/srv.log：" >&2
  cat data/srv.log >&2
  kill "$SRV" 2>/dev/null
  exit 1
fi
echo "✓ 服务已就绪 http://127.0.0.1:$PORT"

# ── 建管理员并登录取会话 ──────────────────────────────────────────
curl -s --noproxy '*' -X POST "http://127.0.0.1:$PORT/api/setup" \
  -H 'Content-Type: application/json' \
  -d '{"name":"admin","password":"preview123"}' -o /dev/null
# setup 已存在时返回 400，不影响；这里只关心下面的会话
curl -s --noproxy '*' -D "$TMP/hdr.txt" -X POST "http://127.0.0.1:$PORT/api/session" \
  -H 'Content-Type: application/json' \
  -d '{"name":"admin","password":"preview123"}' -o /dev/null
CK=$(grep -i 'set-cookie' "$TMP/hdr.txt" | head -1 | sed 's/.*mcloud_session=\([^;]*\).*/\1/' | tr -d '\r\n')
if [ -z "$CK" ]; then
  echo "拿不到会话 cookie，无法截需要登录的页面" >&2
  kill "$SRV" 2>/dev/null
  exit 1
fi
echo "✓ 会话已建立（token ${#CK} 字符）"

# ── 起带调试端口的 Edge ───────────────────────────────────────────
"$EDGE" --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
  --remote-debugging-port=9333 --user-data-dir="$PROFILE" \
  "about:blank" > data/edge.log 2>&1 &
EDGE_PID=$!
sleep 3

cleanup() {
  kill "$EDGE_PID" 2>/dev/null
  kill "$SRV" 2>/dev/null
  wait 2>/dev/null
}
trap cleanup EXIT

BASE="http://127.0.0.1:$PORT"
COOKIE="mcloud_session=$CK"

if [ "$MODE" = "inspect" ]; then
  echo ""
  echo "########## 桌面 1280×900 ##########"
  "$NODE" scripts/inspect-layout.cjs "$BASE/#/" 1280 900 "$COOKIE"
  echo ""
  echo "########## 移动 390×844 ##########"
  "$NODE" scripts/inspect-layout.cjs "$BASE/#/" 390 844 "$COOKIE"
  exit 0
fi

shot() { # hash out w h [--dark]
  echo "── $2 @ $3×$4 ${5:-}"
  "$NODE" scripts/shot-auth.cjs "$BASE" "$1" "$2" "$3" "$4" "$COOKIE" ${5:-}
}

# 未登录态：故意不带 cookie
"$NODE" scripts/shot-auth.cjs "$BASE" "#/" "data/shot-1-login-desktop.png" 1280 900 "-"
"$NODE" scripts/shot-auth.cjs "$BASE" "#/" "data/shot-2-login-mobile.png" 390 844 "-"

shot "#/" "data/shot-3-dash-desktop.png" 1280 900
shot "#/" "data/shot-4-dash-mobile.png" 390 844
shot "#/accounts" "data/shot-5-accounts.png" 1280 900
shot "#/" "data/shot-6-dash-dark.png" 1280 900 --dark

echo ""
echo "── 产物 ──"
ls -la data/shot-*.png
