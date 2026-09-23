#!/usr/bin/env python3
"""读一条 Reqable 抓包记录，**先脱敏再打印**。

# 为什么需要它

小红花 / 签到那一族的请求头里带着真凭据（`jwttoken` 905 字符 JWT、一整串 cookie、
`x-tingyun`）。直接 `reqable-capture.py detail <id>` 会把这些原值打进终端与对话，
而仓库的规矩是「凭据一律不落文档，只记形态与字段含义」（`docs/sanitize-guide.md`）。

⚠ 于是这里**不是**"打印后靠人眼跳读"，而是**在打印之前**就把值换成形状：
只留长度、可见字符集与前缀几个字符 ⇒ 想对拍"两处是不是同一串"够用，
想拿去用则不可能。这是刻意的下限，不是省事。

# 用法

    scripts/reqable-detail-masked.py 621
    scripts/reqable-detail-masked.py 511 600 621
    scripts/reqable-detail-masked.py 621 --show-body 3000   # 放宽响应体截断

# 退出码

    0  全部拿到
    1  至少一条没拿到（Reqlabe 没开、ID 不存在、解析失败）
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CAPTURE = REPO / "scripts" / "reqable-capture.py"

# 这些头的**值**整体换形（给长度 + 前缀，不给内容）。
# ⚠ 大小写不敏感比较 —— HTTP 头名本就大小写不敏感，这里漏一个就等于漏脱敏。
MASK_VALUE_HEADERS = {
    "jwttoken",
    "cookie",
    "set-cookie",
    "authorization",
    "x-tingyun",
    "x-signature",
    "cookietoken",
    "cookietokenkey",
    "token",
    "smidv2",
    "userdomainid",
}

# URL 里这些查询参数的值换形（`code` 是口令，属业务数据但同批里没必要外露）。
MASK_QUERY_PARAMS = {"token", "code", "cookietoken", "accesstoken"}

JWT_RE = re.compile(r"^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*$")


def shape(value: str) -> str:
    """把一个凭据值换成**形状**：长度 + 前缀 6 字符 + 是不是 JWT 三段。

    ⚠ 前缀只给 6 个字符：够分辨"这是 HS256 还是 RS256"（`eyJhbG` vs `eyJraW`），
    不够拿去用。
    """
    if value is None:
        return "<null>"
    if value == "":
        return "<empty>"
    kind = "JWT3段" if JWT_RE.match(value) else "文本"
    return "<%s %d字符 %s…>" % (kind, len(value), value[:6])


def mask_url(url: str) -> str:
    """把 URL 里敏感查询参数的值换形，`code` 另保留"几个字"这个可读信息。"""

    def repl(m: re.Match) -> str:
        key, val = m.group(1), m.group(2)
        if key.lower() not in MASK_QUERY_PARAMS:
            return m.group(0)
        if key.lower() == "code":
            # 口令是**业务**数据（主播当场公布的），不是凭据 ⇒ 给长度，不给内容。
            return "%s=<%d字符口令>" % (key, len(val))
        return "%s=%s" % (key, shape(val))

    return re.sub(r"([?&])([A-Za-z0-9_]+)=([^&#]*)",
                  lambda m: m.group(1) + repl(re.match(r"([A-Za-z0-9_]+)=(.*)", m.group(0)[1:])),
                  url)


def mask_headers(headers: list[dict]) -> str:
    parts = []
    for h in headers or []:
        name = h.get("name", "")
        value = h.get("value", "")
        if name.lower() in MASK_VALUE_HEADERS:
            parts.append("%s: %s" % (name, shape(value)))
        else:
            parts.append("%s: %s" % (name, value))
    return "\n    ".join(parts)


def fetch(rec_id: int) -> dict:
    proc = subprocess.run(
        [sys.executable, str(CAPTURE), "detail", str(rec_id)],
        cwd=str(REPO), capture_output=True, text=True, timeout=120,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip()[-300:] or "capture 脚本失败")
    return json.loads(proc.stdout)


def render(rec: dict, show_body: int) -> str:
    req = rec.get("request") or {}
    resp = rec.get("response") or {}
    app = rec.get("application") or {}
    conn = rec.get("connection") or {}
    lines = [
        "── 记录 %s ──" % rec.get("id"),
        "  进程   : %s" % app.get("name"),
        "  时刻   : %s" % conn.get("timestamp"),
        "  请求   : %s %s" % (req.get("method"), mask_url(rec.get("url", ""))),
        "  请求头 :",
        "    " + mask_headers(req.get("headers")),
    ]
    body = (req.get("body") or {})
    if body.get("text"):
        lines.append("  请求体 : (%s, %d字节)" % (body.get("mime"), len(body["text"])))
        lines.append("    " + body["text"][:show_body])
    else:
        lines.append("  请求体 : <空>")

    lines.append("  响应   : HTTP %s" % resp.get("code"))
    rbody = (resp.get("body") or {})
    text = rbody.get("text") or ""
    lines.append("  响应体 : (%s, %d字节)" % (rbody.get("mime"), len(text)))
    if text:
        shown = text[:show_body]
        lines.append("    " + shown + (" …[截断]" if len(text) > show_body else ""))
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("ids", nargs="+", type=int, help="抓包记录 ID")
    ap.add_argument("--show-body", type=int, default=800, help="响应体打印多少字符")
    args = ap.parse_args()

    failed = false = False
    for rec_id in args.ids:
        try:
            print(render(fetch(rec_id), args.show_body))
            print()
        except Exception as exc:  # 一条失败不该带走其余几条
            failed = True
            print("── 记录 %d ──\n  !! 取不到：%s\n" % (rec_id, exc), file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
