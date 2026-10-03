#!/usr/bin/env bash
#
# 给构建产物签名，让 macOS 的「可移除宗卷」授权**在重编译后依然匹配**。
#
# # 为什么需要它
#
# TCC 按「路径 + **代码要求**」记授权。而**未签名**二进制的要求就是**内容哈希** ⇒
# 每次 `cargo build` 都变成"另一个程序"，下次启动授权对不上，再弹一次框
# （系统日志原话 `Failed to match existing code requirement`）✗。
#
# 用**固定身份**签一次名，要求就变成"这条证书签的" ⇒ 换内容照样匹配 ✓，
# 从此重编译→重启**不再弹框**，也不会卡在启动阶段等你点 ✗。
#
# # 用法
#
#   scripts/sign-local.sh                    # 签 target/debug/mcloud
#   scripts/sign-local.sh <其它二进制>
#   MCLOUD_SIGN_IDENTITY="MacDev" scripts/sign-local.sh
#
# 身份从哪来：优先 `$MCLOUD_SIGN_IDENTITY`，否则取钥匙串里**第一个** codesigning
# 身份（`security find-identity -v -p codesigning`）。本机实测用的是 "MacDev" ✓。
#
# ⚠ 签完**必须**重启服务才生效；而重建之后**必须**重新签（`cargo build` 覆写的
#   是未签名的那份 ✗）—— 所以日常别手动拼这两步，用 `scripts/dev-reload.sh` ✓。
#
# ⚠ 找不到身份时**只警告不失败**：签名是"免弹框"的优化，不该把上线流程卡死 ✗。

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$REPO/target/debug/mcloud}"

if [[ ! -f "$BIN" ]]; then
  echo "✗ 构建产物不存在：$BIN" >&2
  exit 1
fi

IDENTITY="${MCLOUD_SIGN_IDENTITY:-}"
if [[ -z "$IDENTITY" ]]; then
  # `find-identity -v -p codesigning` 的输出形如：
  #   1) 3D79B3… "MacDev"
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(.*\)".*/\1/p' | head -1)"
fi

if [[ -z "$IDENTITY" ]]; then
  cat >&2 <<'EOF'
⚠ 钥匙串里没有可用的 codesigning 身份 ⇒ 这次**不签名**。
  后果：下次启动会按内容哈希校验授权，重编译后会再弹一次「允许」。
  要根治：建一个自签名代码签名证书（钥匙串访问 → 证书助理 → 创建证书 →
  类型选「代码签名」），或用 MCLOUD_SIGN_IDENTITY 指定一个已有身份。
EOF
  exit 0
fi

# `--timestamp=none`：本地签名不去连 Apple 的时间戳服务 ✓
# `--force`：覆盖 cargo 刚写下的那段（可能存在的旧签名/无签名）
codesign --force --sign "$IDENTITY" --timestamp=none "$BIN" || {
  echo "✗ 签名失败（身份：$IDENTITY）" >&2
  exit 1
}

# 校验一遍：签名坏了比不签更糟（进程会被内核直接拒掉 ✗）
if ! codesign --verify --strict "$BIN" 2>/dev/null; then
  echo "✗ 签名校验没过：$BIN" >&2
  exit 1
fi

echo "✓ 已签名（$IDENTITY）：$BIN"
# 打出**指定要求** —— TCC 认的就是它，重编译后要求不变才不会弹框 ✓
echo "  要求：$(codesign -d -r- "$BIN" 2>&1 | sed -n 's/^designated => //p')"
