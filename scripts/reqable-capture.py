#!/usr/bin/env python3
"""Reqable 抓包桥 —— 驱动本机 Reqable 抓手机 App 流量，并把记录转成标准 HAR。

这是 P3「按新抓包重写上游接口」的第一环：
`抓包 → mcloud fixture extract → 写接口 → L2 回放`。
本脚本负责第一环，产出喂给第二环的 HAR。

# 为什么是 Reqable 而不是 CDP

目标流量在**装在这台 Mac 上的 iOS 版 App**（`mCloud.app`，bundle id
`cn.10086.mobileClouds`）里。它不是浏览器页面，CDP 抓不到；而 Reqable 作为
**系统代理**接管后，iOS App 走 CFNetwork 的请求会自动进来。

# 前置条件（缺一不可）

1. Reqable 在跑，且**已接管系统代理**（`networksetup -getsecurewebproxy Wi-Fi`
   应显示 `127.0.0.1:9000`）。Reqable 里的开关是"系统代理"。
2. 目标 App 已启动过（记录里的 `application.name` 才能对上）。
3. 抓 HTTPS 要求 Reqable 的 CA 已被信任。**macOS 上 Reqable 自己会装**，
   所以本机抓包不需要额外操作 —— 这也是选它而不是自建代理的原因。

# 本机踩过的坑（照抄别人脚本时容易踩）

- **MCP 握手时序是硬要求**：`initialize` 必须**先收到回应**，才能发
  `notifications/initialized`；三条消息一次性灌进去，对端直接回
  `-32600 Received notifications/initialized before initialize`。
- **不能拿 `readline()` 配 deadline**：对端不吐字节时它会一直阻塞，
  超时判断根本没机会执行。这里用后台线程读 + 队列带超时取。
- **记录里的 `body` 是对象不是字符串**：形状为 `{text, mime, encoding}`。
  按字符串处理时 `len()` 量到的是字典键数（恒为 3），
  会误判成"响应体只有 3 字节"。
- **访问本机 CDP/控制端口要绕开代理**：本机开着 Clash（`HTTP_PROXY=127.0.0.1:60925`），
  curl 本机端口会走代理并失败。用 `--noproxy '*'`。
- **`capture_live_filter` 的过滤器是 `oneOf`**：形状为
  `{"type":"keyword","pattern":"..."}` 或 `{"type":"url","urls":[...]}`，
  不是 `{field, operator, value}`。空数组 `[]` 返回全部记录 ID。

# 用法

    # 看状态 / 开关抓包
    scripts/reqable-capture.py status
    scripts/reqable-capture.py on
    scripts/reqable-capture.py off
    scripts/reqable-capture.py clear

    # 汇总当前记录（默认只看目标 App）
    scripts/reqable-capture.py summary
    scripts/reqable-capture.py summary --all
    scripts/reqable-capture.py summary --host 139.com

    # 单条完整详情（URL / 头 / 请求体 / 响应体）
    scripts/reqable-capture.py detail 64

    # 边操作边看新记录
    scripts/reqable-capture.py watch

    # 导出标准 HAR，喂给 mcloud fixture extract
    scripts/reqable-capture.py har har/2026-09-18-签到.har --host 139.com

    # 转夹具（下一步，见 README 的门禁与夹具章节）
    cargo run -p mcloud-cli -- fixture extract --har har/xxx.har --out test/fixtures/http
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import subprocess
import sys
import threading
import time

# Reqable 自带的 stdio MCP 服务。用绝对路径：它不在 PATH 里。
MCP_BIN = "/Applications/Reqable.app/Contents/Helpers/mcp-server"
# 目标 App 在记录里的进程名（iOS App 在 Mac 上以 App 名出现）
DEFAULT_APP = os.environ.get("MCLOUD_CAPTURE_APP", "mCloud")


class Mcp:
    """Reqable MCP 的最小 stdio 客户端。

    只用到 4 个工具：抓包开关、状态、清空、取记录。
    """

    def __init__(self, scope: str = "all") -> None:
        self.proc = subprocess.Popen(
            [MCP_BIN, "--scope", scope],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._lines: queue.Queue = queue.Queue()
        threading.Thread(target=self._pump, daemon=True).start()
        self._next_id = 1
        self._handshake()

    def _pump(self) -> None:
        for line in self.proc.stdout:
            self._lines.put(line)
        self._lines.put(None)  # 流结束哨兵

    def _send(self, obj: dict) -> None:
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def _await_id(self, want_id: int, budget: float = 60.0):
        end = time.time() + budget
        while time.time() < end:
            try:
                line = self._lines.get(timeout=max(0.1, end - time.time()))
            except queue.Empty:
                return None
            if line is None:
                return None
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == want_id:
                return msg
        return None

    def _handshake(self) -> None:
        # 时序不能改：先 initialize，**等回应**，再发 initialized。
        self._send({
            "jsonrpc": "2.0",
            "id": self._next_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "mcloud-capture", "version": "1.0"},
            },
        })
        init = self._await_id(self._next_id, 20)
        self._next_id += 1
        if init is None:
            raise RuntimeError("Reqable MCP initialize 无回应（Reqable 没在跑？）")
        if "error" in init:
            raise RuntimeError(f"initialize 失败: {init['error']}")
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call(self, name: str, args: dict | None = None, budget: float = 90.0):
        my_id = self._next_id
        self._next_id += 1
        self._send({
            "jsonrpc": "2.0",
            "id": my_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": args or {}},
        })
        msg = self._await_id(my_id, budget)
        if msg is None:
            raise RuntimeError(f"{name} 无回应（超时 {budget}s）")
        if "error" in msg:
            raise RuntimeError(f"{name} 失败: {msg['error']}")
        return msg.get("result", {})

    def close(self) -> None:
        try:
            self.proc.kill()
        except Exception:
            pass

    def __enter__(self) -> Mcp:
        return self

    def __exit__(self, *_exc) -> None:
        self.close()


def unwrap(result: dict) -> str:
    """MCP 工具结果：`content[]` 里的 text 段拼起来就是载荷。"""
    parts = []
    for item in result.get("content", []) or []:
        if item.get("type") == "text":
            parts.append(item.get("text", ""))
        else:
            parts.append(json.dumps(item, ensure_ascii=False))
    return "\n".join(parts)


def call_json(client: Mcp, name: str, args: dict | None = None):
    """调用工具并把载荷当 JSON 解析；不是 JSON 就原样返回字符串。"""
    raw = unwrap(client.call(name, args))
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw


# ---------------------------------------------------------------- 记录访问


def record_ids(client: Mcp) -> list[int]:
    """当前保留的全部记录 ID（空过滤器即"全部"）。"""
    ids = call_json(client, "capture_live_filter", {"filters": []})
    return ids if isinstance(ids, list) else []


def fetch_all(client: Mcp, progress: bool = False) -> list[dict]:
    records = []
    ids = record_ids(client)
    for index, rid in enumerate(ids, 1):
        if progress and index % 25 == 0:
            print(f"  已读 {index}/{len(ids)}", file=sys.stderr)
        rec = call_json(client, "capture_live_get_by_id", {"id": rid})
        if isinstance(rec, dict):
            records.append(rec)
    return records


def app_name(rec: dict) -> str:
    return (rec.get("application") or {}).get("name") or "?"


def body_text(body) -> str:
    """记录里的 body 是 `{text, mime, encoding}` 对象或 null，**不是字符串**。"""
    if body is None:
        return ""
    if isinstance(body, str):
        return body
    if isinstance(body, dict):
        return body.get("text") or ""
    return ""


def body_mime(body) -> str:
    return body.get("mime") or "" if isinstance(body, dict) else ""


def host_of(rec: dict) -> str:
    url = rec.get("url") or ""
    if "://" not in url:
        return ""
    rest = url.split("://", 1)[1]
    return rest.split("/", 1)[0].split("?", 1)[0]


def keep(rec: dict, app: str | None, host: str | None) -> bool:
    if app and app.lower() not in app_name(rec).lower():
        return False
    if host and host.lower() not in host_of(rec).lower():
        return False
    return True


def status_of(rec: dict) -> str:
    """响应状态；没有 response 字段说明请求没拿到响应（TLS 被拒 / 中断）。"""
    resp = rec.get("response")
    if not resp:
        return "无响应"
    return str(resp.get("code", "-"))


# ---------------------------------------------------------------- HAR 转换


def to_har(records: list[dict], out_path: str) -> int:
    """转成标准 HAR 1.2 —— `mcloud fixture extract` 吃的就是这个格式。"""
    entries = []
    for rec in records:
        req = rec.get("request") or {}
        resp = rec.get("response") or {}

        req_headers = [
            {"name": h.get("name", ""), "value": h.get("value", "")}
            for h in (req.get("headers") or [])
        ]
        resp_headers = [
            {"name": h.get("name", ""), "value": h.get("value", "")}
            for h in (resp.get("headers") or [])
        ]

        request: dict = {
            "method": req.get("method", "GET"),
            "url": rec.get("url") or "",
            "httpVersion": req.get("protocol", "HTTP/1.1"),
            "headers": req_headers,
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": -1,
        }
        req_body = body_text(req.get("body"))
        if req_body:
            request["postData"] = {
                "mimeType": body_mime(req.get("body")),
                "text": req_body,
            }

        resp_body = body_text(resp.get("body"))
        content: dict = {
            "size": len(resp_body),
            "mimeType": body_mime(resp.get("body")),
        }
        if resp_body:
            content["text"] = resp_body

        entries.append({
            "startedDateTime": (rec.get("connection") or {}).get("timestamp", ""),
            "time": 0,
            "request": request,
            "response": {
                "status": resp.get("code", 0),
                "statusText": resp.get("status", ""),
                "httpVersion": resp.get("protocol", "HTTP/1.1"),
                "headers": resp_headers,
                "cookies": [],
                "content": content,
                "redirectURL": "",
                "headersSize": -1,
                "bodySize": -1,
            },
            "cache": {},
            "timings": {"send": 0, "wait": 0, "receive": 0},
            # 非标准字段：保留进程名，方便事后按 App 拆分。extract 会忽略未知字段。
            "_application": app_name(rec),
        })

    har = {
        "log": {
            "version": "1.2",
            "creator": {"name": "mcloud reqable-capture", "version": "1.0"},
            "entries": entries,
        }
    }
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        json.dump(har, handle, ensure_ascii=False, indent=1)
    return len(entries)


# ---------------------------------------------------------------- 子命令


def cmd_status(_args) -> int:
    with Mcp() as client:
        print(unwrap(client.call("capture_live_status")))
    return 0


def cmd_on(_args) -> int:
    with Mcp() as client:
        print(unwrap(client.call("capture_live_set_enabled", {"enabled": True})))
    return 0


def cmd_off(_args) -> int:
    with Mcp() as client:
        print(unwrap(client.call("capture_live_set_enabled", {"enabled": False})))
    return 0


def cmd_clear(_args) -> int:
    with Mcp() as client:
        print(unwrap(client.call("capture_live_clear")))
    return 0


def cmd_summary(args) -> int:
    with Mcp() as client:
        records = fetch_all(client, progress=True)

    app = None if args.all else args.app
    kept = [r for r in records if keep(r, app, args.host)]

    by_app: dict[str, int] = {}
    for rec in records:
        by_app[app_name(rec)] = by_app.get(app_name(rec), 0) + 1

    print(f"总记录 {len(records)} 条，命中 {len(kept)} 条\n")
    print("按进程分组：")
    for name, count in sorted(by_app.items(), key=lambda kv: -kv[1]):
        print(f"  {count:5d}  {name}")
    print()

    # 无响应的单列出来：通常是 TLS 握手被拒（证书固定）或请求中断
    stalled = [r for r in kept if not r.get("response")]
    if stalled:
        print(f"⚠ 无响应 {len(stalled)} 条（TLS 被拒 / 中断）：")
        for rec in stalled[:10]:
            print(f"    [{rec.get('id')}] {host_of(rec)}")
        print()

    for rec in kept:
        req = rec.get("request") or {}
        print(f"[{rec.get('id'):>5}] {app_name(rec):<12} "
              f"{req.get('method', '?'):<6} {status_of(rec):<6} "
              f"{host_of(rec):<34} {(rec.get('url') or '')[:96]}")
    return 0


def cmd_detail(args) -> int:
    with Mcp() as client:
        rec = call_json(client, "capture_live_get_by_id", {"id": args.id})
    print(json.dumps(rec, ensure_ascii=False, indent=2))
    return 0


def cmd_watch(args) -> int:
    """轮询新记录并实时打印 —— 边在 App 里操作边看。

    输出**逐行 flush**：重定向到文件时 stdout 是块缓冲的，
    不 flush 的话 `watch > log` 会看起来"什么都没抓到"（实际只是没落盘）。
    """
    seen: set[int] = set()
    print(f"监听中（每 {args.interval}s 一次，只看 {args.app or '全部进程'}）；Ctrl-C 退出\n",
          flush=True)
    try:
        with Mcp() as client:
            while True:
                for rid in record_ids(client):
                    if rid in seen:
                        continue
                    seen.add(rid)
                    rec = call_json(client, "capture_live_get_by_id", {"id": rid})
                    if not isinstance(rec, dict):
                        continue
                    if not keep(rec, args.app, args.host):
                        continue
                    req = rec.get("request") or {}
                    print(f"[{rid:>5}] {app_name(rec):<12} {req.get('method', '?'):<6} "
                          f"{status_of(rec):<6} {(rec.get('url') or '')[:110]}", flush=True)
                time.sleep(args.interval)
    except KeyboardInterrupt:
        print("\n已停止监听", flush=True)
    return 0


def cmd_har(args) -> int:
    with Mcp() as client:
        records = fetch_all(client, progress=True)

    app = None if args.all else args.app
    kept = [r for r in records if keep(r, app, args.host)]
    count = to_har(kept, args.out)
    print(f"已写出 {count} 条到 {args.out}（源记录 {len(records)} 条）")
    if count == 0:
        print("⚠ 0 条 —— 检查 --app / --host 是否写错，或抓包是否开着")
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Reqable 抓包桥：驱动本机 Reqable 抓手机 App 流量并导出 HAR",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("status", help="抓包是否在运行").set_defaults(func=cmd_status)
    sub.add_parser("on", help="开始抓包").set_defaults(func=cmd_on)
    sub.add_parser("off", help="停止抓包").set_defaults(func=cmd_off)
    sub.add_parser("clear", help="清空当前记录").set_defaults(func=cmd_clear)

    p_sum = sub.add_parser("summary", help="汇总当前记录")
    p_sum.add_argument("--app", default=DEFAULT_APP, help=f"进程名子串（默认 {DEFAULT_APP}）")
    p_sum.add_argument("--host", help="主机名子串")
    p_sum.add_argument("--all", action="store_true", help="不限进程")
    p_sum.set_defaults(func=cmd_summary)

    p_det = sub.add_parser("detail", help="打印单条完整详情")
    p_det.add_argument("id", type=int)
    p_det.set_defaults(func=cmd_detail)

    p_watch = sub.add_parser("watch", help="实时打印新记录")
    p_watch.add_argument("--app", default=DEFAULT_APP)
    p_watch.add_argument("--host")
    p_watch.add_argument("--interval", type=float, default=2.0)
    p_watch.set_defaults(func=cmd_watch)

    p_har = sub.add_parser("har", help="导出标准 HAR")
    p_har.add_argument("out")
    p_har.add_argument("--app", default=DEFAULT_APP)
    p_har.add_argument("--host")
    p_har.add_argument("--all", action="store_true")
    p_har.set_defaults(func=cmd_har)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
