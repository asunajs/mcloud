#!/usr/bin/env python3
"""App 自身网络日志 → HAR —— 抓包的第二条通路，而且比代理那条更强。

`抓包 → mcloud fixture extract → 写接口 → L2 回放` 里的第一环有两条路：

| | `reqable-capture.py` | 本脚本 |
|---|---|---|
| 数据来源 | Reqable 当系统代理 | App 自己写的 `FXTrace*.log` |
| 覆盖 | 走代理的请求 | **App 内部所有**网络调用 |
| 请求体 | 有（密文原样） | 有，**外加 `encodeData` 之外的明文信封** |
| 请求头 | 有 | 有（`HTTP Header` 整块 dump）|
| 响应体 | 有 | 有（`Raw Response String`）|
| 保留期 | 看 Reqable 设置 | **不到 2 小时**（5×10 MB 封顶）|

两者互补。本脚本存在的理由是第二条路有两个硬约束，得靠工具兜住：

1. **保留窗口极短**。实测 5 个文件 × 10 MB 只覆盖 **1 小时 46 分**（06:26→08:12）。
   原因不是 App 忙，而是它在空转轮询：`toolBox/load/recommend` 与
   `IOutLink/queryRecentFriendLinkList` 合计占了 99% 的体积（106 分钟里 5928 + 5769 次）。
   所以**看到有用的日志必须当场 `snapshot` 出来**，回头再读只会读到轮询。
2. **信噪比极低**。默认剔掉上面两个轮询接口，剩下的才是你要的协议样本。

# 日志里到底有什么（实测的块结构）

每个 HTTP 调用会写**两块**，中间夹着其他日志：

```
*************Request Start***************

API Name:		toolBox/load/recommend
Method:			POST
Service:		MCCaiXunService
TimeStamp:		1789770367013262

HTTP URL:
	https://middle.yun.139.com/openapi/toolBox/load/recommend
HTTP Header:
{
	Content-Type = application/json;charset=UTF-8;
	Authorization = Basic <base64 的 mobile:<手机号>:<authToken>>;
	x-DeviceInfo = 1|...|67F4C56A...|EAB9BF94B8DE|...;
	signature = <64 位 hex>;
}
HTTP Body:
	{"areaCode":"023",...,"encodeData":"...","encodeType":"2",...}
*************Request end***************
```

```
=====================API Response==================

Status:	200	(no error)
TimeStamp:		1789770465012572
Duration:		68.12 ms
Tid:6a724bb6-fa19-4f33-958d-9cc5b7fb7d53

Request URL:
	https://middle.yun.139.com/openapi/toolBox/load/recommend
Raw Response String:
	{"resultCode":200,...}

---------------  Related Request Content  --------------

HTTP URL:   ← 与上面 Request 块重复，解析时**忽略**
...
=====================Response end====================
```

请求块与响应块**按 URL 做 FIFO 配对**（日志是追加写的，顺序即时间序）。

# 凭据纪律（重要）

`Authorization` 头是 `Basic base64("mobile:<真实手机号>:<真实 authToken>")` ——
**可直接复用的凭据**，而且 base64 一解就现形（`check-identifiers.cjs` 的
base64 那条规则拦的就是它）。因此：

- HAR **只写 `har/`**（已在 `.gitignore` 里），脱敏由 `mcloud fixture extract`
  在写盘之前完成，不靠"记得别提交"；
- `dump` 默认**把凭据头打码**，要看原值得显式加 `--show-secrets`。

# 用法

    # 当前日志有哪些、覆盖多长时间
    scripts/applog-capture.py list

    # 接口分布（默认已剔掉空转轮询）
    scripts/applog-capture.py summary
    scripts/applog-capture.py summary --all

    # 把日志抢出来（**做完 App 操作后立刻执行**）
    scripts/applog-capture.py snapshot /tmp/fxlog-2026-09-19

    # 看某条接口的完整请求 + 响应（凭据打码）
    scripts/applog-capture.py dump 领云朵

    # 导出 HAR，喂给夹具抽取器
    scripts/applog-capture.py har har/2026-09-19-核心域.har --path /user/
    go run ./cmd/mcloud fixture extract \
        --har har/2026-09-19-核心域.har --out test/fixtures/http

# 日志目录

默认是那台装在本机的 iOS App 的容器路径，可用 `--dir` 或环境变量
`MCLOUD_APPLOG_DIR` 覆盖（换设备 / 换安装时用得上）。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
from datetime import datetime
from urllib.parse import urlsplit

# 装在本机的 iOS App 的日志目录（bundle id cn.10086.mobileClouds）
DEFAULT_DIR = os.environ.get(
    "MCLOUD_APPLOG_DIR",
    os.path.expanduser(
        "~/Library/Containers/1F157E31-0EE5-40A7-81CE-D20E9704B387"
        "/Data/Documents/log"
    ),
)

# 空转轮询：占 99% 体积，与四个域都无关。默认剔除，`--all` 关掉这条。
DEFAULT_NOISE = (
    "toolBox/load/recommend",
    "IOutLink/queryRecentFriendLinkList",
)

# 打码的凭据头。值本身**不进日志**，只留长度与前 4 位，够定位就行。
SECRET_HEADERS = ("authorization", "signature", "cookie", "x-yun-sign")

REQ_START = re.compile(r"^\*+Request Start\*+")
REQ_END = re.compile(r"^\*+Request end\*+")
RESP_START = re.compile(r"^=+API Response=+")
RESP_END = re.compile(r"^=+Response end=+")
RELATED = re.compile(r"^-{5,}.*Related Request Content.*-{5,}")
LOG_LINE = re.compile(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} \[")
FIELD_LINE = re.compile(r"^[A-Za-z][A-Za-z0-9 _-]*:")


def is_marker(line: str) -> bool:
    """块内取值时的终止判据 —— 命中任意一条说明值已经读完了。"""
    return bool(
        REQ_START.match(line)
        or REQ_END.match(line)
        or RESP_START.match(line)
        or RESP_END.match(line)
        or RELATED.match(line)
        or LOG_LINE.match(line)
        or FIELD_LINE.match(line)
    )


def take_value(lines: list[str], index: int) -> str:
    """读 `字段名:` 之后的那个值块（值通常一行，但响应体可能是多行 JSON）。"""
    collected: list[str] = []
    j = index + 1
    while j < len(lines) and not is_marker(lines[j]):
        collected.append(lines[j].lstrip("\t").rstrip())
        j += 1
    while collected and not collected[0]:
        collected.pop(0)
    while collected and not collected[-1]:
        collected.pop()
    return "\n".join(collected)


def log_files(directory: str) -> list[str]:
    if not os.path.isdir(directory):
        return []
    names = [n for n in os.listdir(directory) if n.startswith("FXTrace") and n.endswith(".log")]
    # 文件名里的时间戳是**创建时刻**，字典序即创建序，与内容时间序一致
    return [os.path.join(directory, n) for n in sorted(names)]


# ---------------------------------------------------------------- 解析


def parse_request(lines: list[str], start: int) -> dict:
    rec: dict = {"kind": "request", "headers": [], "body": "", "url": ""}
    i = start + 1
    while i < len(lines):
        line = lines[i]
        if REQ_END.match(line) or REQ_START.match(line) or RESP_START.match(line):
            break
        if line.startswith("API Name:"):
            rec["api_name"] = line.split(":", 1)[1].strip()
        elif line.startswith("Method:"):
            rec["method"] = line.split(":", 1)[1].strip()
        elif line.startswith("Service:"):
            rec["service"] = line.split(":", 1)[1].strip()
        elif line.startswith("TimeStamp:"):
            rec["timestamp_us"] = line.split(":", 1)[1].strip()
        elif line.startswith("HTTP URL:"):
            rec["url"] = take_value(lines, i)
        elif line.startswith("HTTP Header:"):
            rec["headers"] = parse_header_block(take_value(lines, i))
        elif line.startswith("HTTP Body:"):
            rec["body"] = take_value(lines, i)
        i += 1
    return rec


def parse_response(lines: list[str], start: int) -> dict:
    rec: dict = {"kind": "response", "url": "", "body": ""}
    i = start + 1
    while i < len(lines):
        line = lines[i]
        if RESP_END.match(line) or RESP_START.match(line) or REQ_START.match(line):
            break
        if line.startswith("Status:"):
            rec["status"] = line.split(":", 1)[1].strip().split("\t")[0].strip()
        elif line.startswith("TimeStamp:"):
            rec["timestamp_us"] = line.split(":", 1)[1].strip()
        elif line.startswith("Duration:"):
            rec["duration_ms"] = line.split(":", 1)[1].strip()
        elif line.startswith("Tid:"):
            rec["tid"] = line.split(":", 1)[1].strip()
        elif line.startswith("Request URL:"):
            rec["url"] = take_value(lines, i)
        elif line.startswith("Raw Response String:"):
            rec["body"] = take_value(lines, i)
        elif RELATED.match(line):
            # 之后是"Related Request Content"，与 Request 块逐字重复 —— 不读
            break
        i += 1
    return rec


def parse_header_block(block: str) -> list[dict]:
    """`HTTP Header` 是 NSDictionary 的 dump：每行 `名字 = 值;`，值里可能含 `;`。

    所以**按第一个 ` = ` 切**、只剥**一个**尾分号 ——
    `Content-Type = application/json;charset=UTF-8;` 的值里有分号，
    用 `split(';')` 会把它切碎。
    """
    out: list[dict] = []
    for raw in block.splitlines():
        line = raw.strip()
        if not line or line in "{}":
            continue
        if line.endswith(";"):
            line = line[:-1]
        if " = " not in line:
            continue
        name, value = line.split(" = ", 1)
        out.append({"name": name.strip(), "value": value.strip()})
    return out


def parse_log(path: str) -> list[dict]:
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        lines = handle.read().splitlines()

    records: list[dict] = []
    for i, line in enumerate(lines):
        if REQ_START.match(line):
            records.append(parse_request(lines, i))
        elif RESP_START.match(line):
            records.append(parse_response(lines, i))
    return records


def pair(records: list[dict]) -> list[dict]:
    """请求 / 响应按 URL 做 FIFO 配对（日志顺序即时间顺序）。"""
    pending: dict[str, list[dict]] = {}
    paired: list[dict] = []
    unpaired_req = 0
    unpaired_resp = 0

    for rec in records:
        url = rec.get("url") or ""
        if rec["kind"] == "request":
            pending.setdefault(url, []).append(rec)
            continue
        queue = pending.get(url)
        if queue:
            req = queue.pop(0)
            paired.append({"request": req, "response": rec})
        else:
            unpaired_resp += 1

    unpaired_req = sum(len(q) for q in pending.values())
    return paired, unpaired_req, unpaired_resp


def load_all(directory: str) -> tuple[list[dict], int, int]:
    records: list[dict] = []
    for path in log_files(directory):
        records.extend(parse_log(path))
    return pair(records)


# ---------------------------------------------------------------- 过滤与呈现


def path_of(url: str) -> str:
    return urlsplit(url).path


def host_of(url: str) -> str:
    return urlsplit(url).netloc


def noise_hit(url: str, noise: tuple[str, ...]) -> bool:
    return any(n in url for n in noise)


def keep(entry: dict, args) -> bool:
    url = entry["request"].get("url") or ""
    if not getattr(args, "all", False) and noise_hit(url, DEFAULT_NOISE):
        return False
    if getattr(args, "host", None) and args.host not in host_of(url):
        return False
    if getattr(args, "path", None) and args.path not in path_of(url):
        return False
    if getattr(args, "service", None):
        if args.service.lower() not in (entry["request"].get("service") or "").lower():
            return False
    return True


def redact(headers: list[dict], show_secrets: bool) -> list[dict]:
    if show_secrets:
        return headers
    out = []
    for header in headers:
        if header["name"].lower() in SECRET_HEADERS:
            value = header["value"]
            out.append({"name": header["name"], "value": f"<{len(value)} 字符，已打码>"})
        else:
            out.append(header)
    return out


def to_har(entries: list[dict], out_path: str) -> int:
    """标准 HAR 1.2 —— `mcloud fixture extract` 吃的就是它。

    `request.method` 是抽取器的**必填**字段，本脚本能填是因为日志的
    Request 块里有 `Method:`（实测 11796 条 POST / 2 条 GET）。
    """
    har_entries = []
    for entry in entries:
        req, resp = entry["request"], entry["response"]
        started = iso_from_us(req.get("timestamp_us"))

        request: dict = {
            "method": req.get("method") or "POST",
            "url": req.get("url") or "",
            "httpVersion": "HTTP/1.1",
            "headers": req.get("headers") or [],
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": -1,
        }
        if req.get("body"):
            request["postData"] = {
                "mimeType": "application/json;charset=UTF-8",
                "text": req["body"],
            }

        body = resp.get("body") or ""
        content: dict = {
            "size": len(body),
            "mimeType": "application/json" if body[:1] in "{[" else "text/plain",
        }
        if body:
            content["text"] = body

        try:
            status = int(resp.get("status") or 0)
        except ValueError:
            status = 0

        har_entries.append({
            "startedDateTime": started,
            "time": duration_ms(resp.get("duration_ms")),
            "request": request,
            "response": {
                "status": status,
                "statusText": "",
                "httpVersion": "HTTP/1.1",
                "headers": [],
                "cookies": [],
                "content": content,
                "redirectURL": "",
                "headersSize": -1,
                "bodySize": -1,
            },
            "cache": {},
            "timings": {"send": 0, "wait": 0, "receive": 0},
            # 非标准字段：来源标记 + 服务名，方便事后按 SDK 分组。extract 会忽略。
            "_source": "applog",
            "_service": req.get("service") or "",
            "_api_name": req.get("api_name") or "",
        })

    har = {
        "log": {
            "version": "1.2",
            "creator": {"name": "mcloud applog-capture", "version": "1.0"},
            "entries": har_entries,
        }
    }
    directory = os.path.dirname(os.path.abspath(out_path))
    os.makedirs(directory, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        json.dump(har, handle, ensure_ascii=False, indent=1)
    return len(har_entries)


def iso_from_us(value: str | None) -> str:
    if not value or not value.isdigit():
        return ""
    try:
        return datetime.fromtimestamp(int(value) / 1_000_000).astimezone().isoformat()
    except (ValueError, OSError, OverflowError):
        return ""


def duration_ms(value: str | None) -> float:
    if not value:
        return 0.0
    match = re.match(r"([\d.]+)", value)
    return float(match.group(1)) if match else 0.0


# ---------------------------------------------------------------- 子命令


def cmd_list(args) -> int:
    files = log_files(args.dir)
    if not files:
        print(f"没有日志文件：{args.dir}")
        print("（App 没启动过？或容器 id 变了 —— 用 --dir 指过去）")
        return 1

    print(f"日志目录：{args.dir}\n")
    total = 0
    for path in files:
        size = os.path.getsize(path)
        total += size
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            lines = handle.read().splitlines()
        stamps = [ln[:19] for ln in lines if LOG_LINE.match(ln)]
        span = f"{stamps[0]}  →  {stamps[-1]}" if stamps else "(无时间戳)"
        reqs = sum(1 for ln in lines if REQ_START.match(ln))
        resps = sum(1 for ln in lines if RESP_START.match(ln))
        print(f"{os.path.basename(path):<34} {size / 1048576:6.1f} MB  "
              f"请求 {reqs:5d}  响应 {resps:5d}  {span}")
    print(f"\n合计 {total / 1048576:.1f} MB")
    print("⚠ 保留窗口不到 2 小时 —— 看到有用的立刻 snapshot")
    return 0


def cmd_snapshot(args) -> int:
    files = log_files(args.dir)
    if not files:
        print(f"没有日志文件：{args.dir}")
        return 1
    os.makedirs(args.out, exist_ok=True)
    for path in files:
        shutil.copy2(path, os.path.join(args.out, os.path.basename(path)))
    print(f"已复制 {len(files)} 个文件到 {args.out}")
    print("（日志是滚动覆盖的，这一步就是为了在它被挤掉之前留一份）")
    return 0


def cmd_summary(args) -> int:
    entries, unpaired_req, unpaired_resp = load_all(args.dir)
    kept = [e for e in entries if keep(e, args)]

    groups: dict[tuple[str, str], list[dict]] = {}
    for entry in kept:
        req = entry["request"]
        groups.setdefault((req.get("method") or "?", path_of(req.get("url") or "")), []).append(entry)

    print(f"配对成功 {len(entries)} 条，命中 {len(kept)} 条"
          f"（未配对请求 {unpaired_req} / 未配对响应 {unpaired_resp}）")
    if not args.all:
        print(f"已剔除空转轮询：{', '.join(DEFAULT_NOISE)}")
    print()

    rows = sorted(groups.items(), key=lambda kv: -len(kv[1]))
    for (method, path), items in rows:
        hosts = {host_of(e["request"].get("url") or "") for e in items}
        statuses = {e["response"].get("status") or "-" for e in items}
        services = {(e["request"].get("service") or "-") for e in items}
        durations = sorted(duration_ms(e["response"].get("duration_ms")) for e in items)
        median = durations[len(durations) // 2] if durations else 0.0
        print(f"{len(items):5d}  {method:<5} {'/'.join(sorted(statuses)):<8} "
              f"{median:7.1f}ms  {path}")
        print(f"        host={','.join(sorted(hosts))}  service={','.join(sorted(services))}")
    return 0


def cmd_dump(args) -> int:
    entries, _, _ = load_all(args.dir)
    hits = [
        e for e in entries
        if args.match in (e["request"].get("url") or "")
        or args.match in (e["request"].get("api_name") or "")
    ]
    if not hits:
        print(f"没有匹配 {args.match!r} 的记录")
        return 1

    print(f"匹配 {len(hits)} 条，显示前 {min(len(hits), args.limit)} 条\n")
    for index, entry in enumerate(hits[: args.limit], 1):
        req, resp = entry["request"], entry["response"]
        print("=" * 72)
        print(f"[{index}] {req.get('method', '?')} {req.get('url')}")
        print(f"    API Name : {req.get('api_name', '-')}")
        print(f"    Service  : {req.get('service', '-')}")
        print(f"    Status   : {resp.get('status', '-')}  耗时 {resp.get('duration_ms', '-')}")
        print("    Headers  :")
        for header in redact(req.get("headers") or [], args.show_secrets):
            print(f"      {header['name']}: {header['value']}")
        print("    Body     :")
        print(f"      {req.get('body') or '(空)'}")
        print("    Response :")
        print(f"      {resp.get('body') or '(空)'}")
    if not args.show_secrets:
        print("\n（凭据头已打码；要看原值加 --show-secrets）")
    return 0


def cmd_har(args) -> int:
    entries, unpaired_req, unpaired_resp = load_all(args.dir)
    kept = [e for e in entries if keep(e, args)]
    if not kept:
        print("⚠ 0 条 —— 检查 --path / --host / --service 是否写错，或日志已经滚掉了")
        return 1
    count = to_har(kept, args.out)
    print(f"已写出 {count} 条到 {args.out}（配对 {len(entries)} 条，"
          f"未配对请求 {unpaired_req} / 未配对响应 {unpaired_resp}）")
    print(f"\n下一步：\n  go run ./cmd/mcloud fixture extract "
          f"--har {args.out} --out test/fixtures/http")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="App 自身网络日志 → HAR（抓包第二通路）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--dir", default=DEFAULT_DIR, help="日志目录")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("list", help="列出日志文件、体积与时间跨度").set_defaults(func=cmd_list)

    p_snap = sub.add_parser("snapshot", help="把当前日志复制出来（防滚动覆盖）")
    p_snap.add_argument("out")
    p_snap.set_defaults(func=cmd_snapshot)

    p_sum = sub.add_parser("summary", help="接口分布")
    p_sum.add_argument("--host")
    p_sum.add_argument("--path")
    p_sum.add_argument("--service")
    p_sum.add_argument("--all", action="store_true", help="包含空转轮询")
    p_sum.set_defaults(func=cmd_summary)

    p_dump = sub.add_parser("dump", help="打印某条接口的完整请求与响应")
    p_dump.add_argument("match", help="URL 或 API Name 的子串")
    p_dump.add_argument("--limit", type=int, default=1)
    p_dump.add_argument("--show-secrets", action="store_true", help="不打码凭据头")
    p_dump.set_defaults(func=cmd_dump)

    p_har = sub.add_parser("har", help="导出标准 HAR")
    p_har.add_argument("out")
    p_har.add_argument("--host")
    p_har.add_argument("--path")
    p_har.add_argument("--service")
    p_har.add_argument("--all", action="store_true")
    p_har.set_defaults(func=cmd_har)

    args = parser.parse_args()
    if not os.path.isdir(args.dir):
        print(f"日志目录不存在：{args.dir}", file=sys.stderr)
        print("（装在本机的 iOS App 容器路径变了？用 --dir 指过去）", file=sys.stderr)
        return 1
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
