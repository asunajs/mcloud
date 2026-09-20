#!/usr/bin/env python3
"""【取证工具，日常不需要】从 WebView 的 localStorage 里读出官方实发的 `deviceId`。

## ⚠ 先读这段：这个脚本已经不是日常路径了（2026-09-19 晚）

`deviceId` 现在是 **Rust 自己算出来的**：

    mcloud smid refresh --account <账号>      # 上报特征、换一个并落库
    mcloud signin --account <账号> --receive  # 不传 --device-id，自动从库里取

因为签到页的 `fp.min.js` 就是数美设备指纹，它的 `getDeviceId()` 返回
`'B' + localStorage['SMID']`，而 `SMID` 是它往 `slw.h5cmpassport.com:9090/deviceprofile/v4`
上报后拿回的 `detail.deviceId` —— 与 `legacy/crates/mcloud-upstream/src/smid.rs` 是**同一个客户端**
（host / organization / appId / RSA 公钥逐字相同）。

**那这个脚本还留着干什么？** 拿**官方实发值**跟 Rust 产出对拍 ——
它是唯一能证明"两边拿到的是同一个命名空间里的 id"的证据来源。
所以它是取证工具，不是运行时依赖。

## 这东西是什么

`receiveV3`（领云朵）与 `exchangeV3`（兑换）必须带一个 `deviceId`，否则上游回
`615`；内容不对回 `614`（两档实证见 `docs/capture/app-signin-samples.txt` §4.2）。

## 为什么能从磁盘上捞到

签到页的 JS 自己就把它缓存了（`newsignin.336f18c3.js` 逐字）：

```js
a = true === e.headers.isDeviceId;
c = localStorage.getItem("deviceId");
if (!a || c) { /* 已有缓存，直接用 */ }
else { c = await Promise.race([<bridge 调用>, timeout(5e3)]); localStorage.setItem("deviceId", c); }
```

即：**只在第一次取，之后永远走缓存**。所以只要这台机器上的官方 App 打开过一次
签到页，值就躺在 WebKit 的 `localStorage` 里了 —— 而且它是**设备级、跨次稳定**的。
2026-09-19 验过：缓存里的值与 HAR 里实际发出去的值**逐字节相同**（sha256 一致）。

## 形状

```
89 字符 = 'B' + 86 个数据字符 + **2 个** '='   ← 规范只要 1 个，这里多一个
'B' 之后的 88 字符按 base64 解 → 恒 64 字节
```

⚠ 早先这里写的是"base64 解码 → 65 字节 = [1][16][48]、首字节 0x05/0x06 不是魔数"。
**那是把 `'B'` 前缀也当成了 base64 数据**：`'B'` 与 SMID 首字符拼出第一个字节，
所以看起来"首字节是变的"。真实结构是 `'B'` + 64 字节，**没有 65 字节这回事**。
下面 `shape_of()` 里那段"不检查首字节"的注释也是同一个误判的产物 ——
现在它仍然不检查，但理由是**那 64 字节是服务端下发的 id、不是密文**。

⚠ **那个多余的 `=` 不是笔误，是它本来的样子**（三台设备零反例），上游也确实收下了
（真值实测 `code=0`）。所以实现侧要**原样透传**，别"顺手规范化"成 88 字符。
"少一个 `=`" 会不会被拒**没有实测**，不要赌。

## 用法

    python3 scripts/h5-device-id.py             # 打印那个值
    python3 scripts/h5-device-id.py --json      # 带出处（容器 / origin 哈希 / 同站其它键）
    python3 scripts/h5-device-id.py --all       # 所有命中，不只第一个
    python3 scripts/h5-device-id.py --self-test # 自检（造合成库再读出来）

退出码：`0` 找到 / `2` 没找到 / `1` 用法或环境问题。

⚠ 输出的是**真值**。别把它粘进任何会进版本库的文件。
"""

from __future__ import annotations

import argparse
import base64
import json
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

# WebKit 的 localStorage 表名（iOS / macOS 一致）。
TABLE = "ItemTable"
KEY = "deviceId"

# 值的形状：89 字符（含 1 个冗余 '='）→ 65 字节。只用于**体检**，不是解密。
EXPECTED_CHARS = 89
EXPECTED_BYTES = 65

# 同站点里还有这些键，说明这个 origin 就是签到页那个 —— 多命中时用来排序。
SIBLING_HINTS = ("marketName", "localToken", "token", "TINGYUN_STORAGE")

# ⚠ 光靠 localStorage 的键名**认不出是哪个 App**：本机上另一个 139 系 App
# （139 邮箱，`com.leadtone.mig.139pe.iPhone`）的 localStorage 键名一模一样，
# 也有 `deviceId`、`marketName`、`token`…… 拿错了值，运行时就是 `614`。
# mCloud 自己的网络日志（`scripts/applog-capture.py` 用的那个）是可靠指纹：
# 它的 log 目录里是 `FXTrace<时间戳>.log`，139 邮箱那边是 `<bundleid> <日期>.log`。
TARGET_LOG_GLOB = "Data/Documents/log/FXTrace*.log"

DEFAULT_ROOT = Path.home() / "Library" / "Containers"
GLOB = "*/Data/Library/WebKit/WebsiteData/*/*/*/LocalStorage/localstorage.sqlite3"


def decode_value(raw) -> str:
    """localStorage 里的字符串在 sqlite 里是 UTF-16LE；容错认 UTF-8 / UTF-16BE。"""
    if isinstance(raw, str):
        return raw
    if not isinstance(raw, (bytes, bytearray)):
        return str(raw)
    for enc in ("utf-16-le", "utf-8", "utf-16-be"):
        try:
            text = bytes(raw).decode(enc)
        except UnicodeDecodeError:
            continue
        # 解错了会出现 NUL 或替换字符，直接淘汰。
        if "\x00" not in text and "\ufffd" not in text:
            return text
    return bytes(raw).decode("utf-8", "replace")


def shape_of(value: str) -> dict:
    """给一个值做形状体检 —— 只描述，不改值。

    ⚠ 不检查首字节：实测见过 0x05 与 0x06，它是**变的**，拿它当魔数会误判。
    """
    info: dict = {
        "length": len(value),
        "bytes": None,
        "firstByte": None,
        "paddingRedundant": False,
        "shapeOk": False,
    }
    core = value.rstrip("=")
    # 规范 base64 的 padding 是 0..2 个；多出来的就是冗余。
    canonical_pad = (-len(core)) % 4
    info["paddingRedundant"] = (len(value) - len(core)) > canonical_pad
    try:
        raw = base64.b64decode(core + "=" * canonical_pad, validate=True)
    except Exception:
        return info
    info["bytes"] = len(raw)
    info["firstByte"] = raw[0] if raw else None
    info["shapeOk"] = len(raw) == EXPECTED_BYTES
    return info


def read_db(db: Path) -> tuple[str | None, list[str]]:
    """读一个 localstorage.sqlite3：返回 (`deviceId` 的值, 该库的全部键)。

    WebKit 进程活着的时候库是带锁的，所以**先连 -wal / -shm 一起拷到临时目录**再读；
    直接开原库有拿到旧快照、甚至被拒绝的风险。
    """
    with tempfile.TemporaryDirectory(prefix="h5-device-id-") as tmp:
        dst = Path(tmp) / db.name
        shutil.copy2(db, dst)
        for suffix in ("-wal", "-shm"):
            side = db.with_name(db.name + suffix)
            if side.exists():
                shutil.copy2(side, dst.with_name(dst.name + suffix))
        try:
            conn = sqlite3.connect(f"file:{dst}?mode=ro", uri=True)
        except sqlite3.Error:
            return None, []
        try:
            keys = [r[0] for r in conn.execute(f"SELECT key FROM {TABLE}")]
            row = conn.execute(
                f"SELECT value FROM {TABLE} WHERE key = ? LIMIT 1", (KEY,)
            ).fetchone()
        except sqlite3.Error:
            # 不是 WebKit 的 localStorage，或者表还没建。
            return None, []
        finally:
            conn.close()
    if row is None:
        return None, keys
    return decode_value(row[0]), keys


def is_target_container(container: Path) -> bool:
    """这个容器是不是 mCloud 的？看它有没有 App 自己的 `FXTrace*.log`。"""
    try:
        return any(container.glob(TARGET_LOG_GLOB))
    except OSError:
        return False


def scan(root: Path, container: Path | None) -> list[dict]:
    """扫出所有带 `deviceId` 的缓存，按"最像签到页那个"排好序。"""
    if container is not None:
        candidates = sorted(
            container.glob("Data/Library/WebKit/WebsiteData/*/*/*/LocalStorage/localstorage.sqlite3")
        )
    else:
        candidates = sorted(root.glob(GLOB))

    hits: list[dict] = []
    for db in candidates:
        if not db.is_file():
            continue
        value, keys = read_db(db)
        if value is None:
            continue
        if container is not None:
            owner, name = container, str(container)
        else:
            rel = db.relative_to(root)
            owner, name = root / rel.parts[0], rel.parts[0]
        hits.append(
            {
                "value": value,
                "database": str(db),
                "container": name,
                "origin": db.parent.parent.name,
                "siblingKeys": keys,
                "hintScore": sum(1 for h in SIBLING_HINTS if h in keys),
                "isTarget": is_target_container(owner),
                **shape_of(value),
            }
        )
    # 排序：形状合规优先 → 确实是 mCloud 的优先 → 同站提示键多者优先 → 路径稳定序。
    hits.sort(key=lambda h: (not h["shapeOk"], not h["isTarget"], -h["hintScore"], h["database"]))
    return hits


def self_test() -> int:
    """造合成的 localStorage 库，再从里面读出来。只碰临时目录。"""
    failures: list[str] = []

    def check(name: str, cond: bool) -> None:
        print(f"  {'✓' if cond else '✗'} {name}")
        if not cond:
            failures.append(name)

    # 合成值：与真值同构（1 + 16 + 48 = 65 字节，且**带一个冗余 '='**），
    # 但内容全是 0xAB —— 不是任何真实设备的值。
    fake_raw = bytes([0xAB]) + b"\xab" * (EXPECTED_BYTES - 1)
    fake = base64.b64encode(fake_raw).decode() + "="  # ← 刻意多补一个
    assert len(fake) == EXPECTED_CHARS, len(fake)

    with tempfile.TemporaryDirectory(prefix="h5-device-id-selftest-") as tmp:
        root = Path(tmp) / "Containers"

        def make_db(uuid: str, value: str, keys) -> Path:
            d = root / uuid / "Data/Library/WebKit/WebsiteData/Default/HASH/HASH/LocalStorage"
            d.mkdir(parents=True)
            db = d / "localstorage.sqlite3"
            conn = sqlite3.connect(db)
            conn.execute(f"CREATE TABLE {TABLE} (key TEXT NOT NULL UNIQUE, value BLOB)")
            # WebKit 真的存 UTF-16LE，所以自检也存 UTF-16LE —— 别用 TEXT 糊过去。
            conn.execute(
                f"INSERT INTO {TABLE} (key, value) VALUES (?, ?)", (KEY, value.encode("utf-16-le"))
            )
            for k in keys:
                conn.execute(f"INSERT INTO {TABLE} (key, value) VALUES (?, ?)", (k, "x"))
            conn.commit()
            conn.close()
            return d

        # 假 mCloud：有 FXTrace 日志（归属指纹），但路径字母序**最靠后**
        make_db("ZZZ-WITHLOG", fake, SIBLING_HINTS)
        (root / "ZZZ-WITHLOG/Data/Documents/log").mkdir(parents=True)
        (root / "ZZZ-WITHLOG/Data/Documents/log/FXTrace2026091900000000.log").write_text("x")
        # 另一个 139 系 App：键名一样、路径最靠前，但日志名不同 —— 不该被选中
        make_db("AAA-OTHER139", fake, SIBLING_HINTS)
        (root / "AAA-OTHER139/Data/Documents/log").mkdir(parents=True)
        (root / "AAA-OTHER139/Data/Documents/log/com.other.app 2026-06-14.log").write_text("x")
        # 路径居中、干脆没有 log 目录 —— 也不该被选中
        make_db("MMM-NOLOG", fake, SIBLING_HINTS)

        hits = scan(root, None)
        check("能扫到 3 条命中", len(hits) == 3)
        check("值原样还原（UTF-16LE 解码正确）", all(h["value"] == fake for h in hits))
        check("形状判为合规", all(h["shapeOk"] for h in hits))
        check("字节数 65", all(h["bytes"] == EXPECTED_BYTES for h in hits))
        check("识别出冗余 '='", all(h["paddingRedundant"] for h in hits))
        check("同站提示键被读到", all(h["hintScore"] == len(SIBLING_HINTS) for h in hits))
        check(
            "FXTrace 指纹只认 mCloud 那个",
            [(h["container"], h["isTarget"]) for h in hits]
            == [("ZZZ-WITHLOG", True), ("AAA-OTHER139", False), ("MMM-NOLOG", False)],
        )
        check(
            "归属优先于路径字母序（有日志的 ZZZ 排在无日志的 AAA 之前）",
            hits[0]["container"] == "ZZZ-WITHLOG",
        )

        # 反向：把 key 换成别的，那条就不再命中。
        conn = sqlite3.connect(
            root / "MMM-NOLOG" / "Data/Library/WebKit/WebsiteData/Default/HASH/HASH/LocalStorage/localstorage.sqlite3"
        )
        conn.execute(f"UPDATE {TABLE} SET key = 'notDeviceId' WHERE key = ?", (KEY,))
        conn.commit()
        conn.close()
        check("key 改名后那条不再命中", len(scan(root, None)) == 2)

    # 形状体检本身
    check("长度不对 -> shapeOk=False", shape_of("YWJj")["shapeOk"] is False)
    check("非 base64 -> shapeOk=False", shape_of("!!!not base64!!!")["shapeOk"] is False)
    check("合规合成值 -> shapeOk=True", shape_of(fake)["shapeOk"] is True)
    # 规范 88 字符形态也应当判合规 —— 我们只体检，不强制那个冗余 '='。
    check("规范 88 字符形态也合规", shape_of(fake[:-1])["shapeOk"] is True)
    check("规范形态不带冗余标记", shape_of(fake[:-1])["paddingRedundant"] is False)

    print()
    if failures:
        print(f"自检失败 {len(failures)} 项：{', '.join(failures)}")
        return 1
    print("自检通过")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description="从 H5 的 localStorage 缓存里取出 receiveV3 要用的设备级 deviceId。",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("--container", type=Path, help="只扫这个容器目录（默认扫 ~/Library/Containers 下全部）")
    ap.add_argument("--root", type=Path, default=DEFAULT_ROOT, help=argparse.SUPPRESS)
    ap.add_argument("--json", action="store_true", help="输出 JSON（带出处与形状体检）")
    ap.add_argument("--all", action="store_true", help="打印所有命中，而不只是第一个")
    ap.add_argument("--self-test", action="store_true", help="跑自检（只碰临时目录）")
    args = ap.parse_args()

    if args.self_test:
        return self_test()

    if args.container is None and not args.root.exists():
        print(f"找不到容器根目录：{args.root}", file=sys.stderr)
        return 1

    hits = scan(args.root, args.container)
    if not hits:
        print(
            "没找到缓存的 deviceId。\n"
            "它由官方 App 打开签到页时写进 WebView 的 localStorage，"
            "所以这台机器上得先真的打开过一次签到页。",
            file=sys.stderr,
        )
        return 2

    chosen = hits if args.all else hits[:1]
    if not hits[0]["shapeOk"]:
        print(
            f"⚠ 取到的值形状不符（期望 {EXPECTED_BYTES} 字节），"
            "它可能不是这个接口要的那个 deviceId。",
            file=sys.stderr,
        )
    if sum(1 for h in hits if h["isTarget"]) > 1:
        print(
            f"⚠ 有 {len(hits)} 条命中、其中多条都带 mCloud 的日志指纹，无法自动分辨。"
            "用 --all 逐个看，或 --container 钉住那个容器。",
            file=sys.stderr,
        )
    if not hits[0]["isTarget"]:
        print(
            "⚠ 没能确认这个值属于 mCloud（那个容器里没有 App 自己的 FXTrace 日志）。"
            "如果本机装过别的 139 系 App，值可能取自它 —— 用 --json 看 container 再核对。",
            file=sys.stderr,
        )

    if args.json:
        print(json.dumps(chosen, ensure_ascii=False, indent=2))
    else:
        for h in chosen:
            print(h["value"])
        if not args.all and len(hits) > 1:
            print(f"（另有 {len(hits) - 1} 条命中，--all 可看全部）", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
