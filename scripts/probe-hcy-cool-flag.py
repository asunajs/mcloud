#!/usr/bin/env python3
"""探针：`hcy-cool-flag` 到底声明了什么。

**已经解开的结论**（见 `docs/capture/app-sms-login-samples.txt` §1.2）：

    hcy-cool-flag 不是"请把响应加密给我"，而是 **"我这个请求体是加密的"**。
    服务端收到它就把 body 当 base64 解，然后 AES-128-CBC 解密：
        载荷 = base64( IV(16 字节，二进制) ‖ AES-128-CBC/PKCS7(key, iv=IV, 明文=JSON) )
        key  = "uDjGSA8auFlbwDkY"     ← iOS 二进制里的字符串常量，暴力命中
    我们发明文 body 时，服务端 base64 解到 `{`(0x7b) 就报
    `01000019 动态解密错误，错误信息:Illegal base64 character 7b`，
    **而这个报错本身也是用同一把钥匙加密的** —— 这就是密钥被反推出来的入口。

本脚本做两件事（全部用**合成号码**，不会给任何真实号码发短信）：

  --baseline  三个基线用例：无该头 / 带 1 / 带 1（第二次），确认现状没漂
  --phase2    **决定性实验**：把请求体按上面的格式加密后发出去。
              若服务端不再报 `Illegal base64`（比如改回 `200059554` 请先过滑块），
              就说明"请求体加密"这条路径被我们走通了 —— 与 `signature` 那次
              活体探针同一性质：不是复现历史值，而是证明算法在真服务器上成立。
              同时测 `hcy-ios-option-enc`（二进制里与它相邻的另一个公共头名），
              用来解释"App 自己带了这个头却拿到明文响应"这个矛盾。

用法：
    python3 scripts/probe-hcy-cool-flag.py --baseline --save har/hcy-cool-flag-samples.json
    python3 scripts/probe-hcy-cool-flag.py --phase2
"""

import argparse
import base64
import json
import os
import time
import urllib.error
import urllib.request

URL = "https://user-njs.yun.139.com/user/sms/getSmsCode"
# 合成号码（199 号段）。**不要换成真实号码** —— 该接口的另一条分支会真的发短信。
SYN_PHONE = "19900000000"

# 从 iOS 二进制里暴力命中的响应/请求体密钥（与 clientKey 一样，是硬编码的客户端常量）
BODY_AES_KEY = b"uDjGSA8auFlbwDkY"

CLIENT_INFO = (
    "1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|"
    "AAA00000000000000000000000000000|AAA000000000|ipados 26.6|375 * 667|"
    "zh-Hans-CN|0.0000|0.0000|aVBhZA==|0|;"
)

BASE_HEADERS = {
    "Content-Type": "application/json;charset=UTF-8",
    "Pragma": "no-cache",
    "x-yun-app-channel": "10000020",
    "x-yun-net-type": "1",
    "x-yun-svc-type": "1",
    "x-huawei-channelSrc": "10000020",
    "x-yun-uni": "1990000000000000001",  # 合成 userDomainId
    "x-yun-api-version": "v1",
    "x-UserAgent": "412|iPad|ios 26.6|12.6.0-200",
    "x-yun-client-info": CLIENT_INFO,
    "Accept-Language": "zh-CN",
    "service_identifier_key": "service_identifier_userDomain",
    "User-Agent": "mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)",
    "x-DeviceName": "aVBhZA==",
    "x-DeviceInfo": CLIENT_INFO.replace("|aVBhZA==|0|;", "|;"),
    "x-yun-user-agent": "iPad|iPad|ios 26.6|mCloud12.6.0-200",
    "x-SvcType": "1",
}

BODY = {
    "mode": "0",
    "phoneNumber": SYN_PHONE,
    "reqType": "3",
    "clientType": "412",
    "random": "qwert",
    "puzzleVerfycode": "",
}


# ─────────────────────── 加解密（与抓包结论逐字一致）───────────────────────


def pkcs7(data, block=16):
    n = block - len(data) % block
    return data + bytes([n]) * n


def encrypt_body(obj, iv=None):
    """`base64( IV(16) ‖ AES-128-CBC/PKCS7(key, iv, JSON) )`。"""
    from Crypto.Cipher import AES

    iv = iv or os.urandom(16)
    pt = json.dumps(obj, separators=(",", ":"), ensure_ascii=False).encode()
    ct = AES.new(BODY_AES_KEY, AES.MODE_CBC, iv).encrypt(pkcs7(pt))
    return base64.b64encode(iv + ct).decode()


def decrypt_body(b64):
    """反过来：把响应体（裸 JSON 字符串）解回明文。"""
    from Crypto.Cipher import AES

    raw = base64.b64decode(b64)
    iv, ct = raw[:16], raw[16:]
    pt = AES.new(BODY_AES_KEY, AES.MODE_CBC, iv).decrypt(ct)
    n = pt[-1]
    if 1 <= n <= 16 and pt[-n:] == bytes([n]) * n:
        pt = pt[:-n]
    return pt.decode("utf-8", "replace")


# ─────────────────────────── 请求与展示 ───────────────────────────


def call(headers_extra, body_text):
    headers = dict(BASE_HEADERS)
    headers["x-yun-user-timestamp"] = str(int(time.time() * 1_000_000))
    headers.update(headers_extra)
    req = urllib.request.Request(URL, data=body_text.encode(), headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")


def describe(label, status, text):
    print(f"\n── {label} ──")
    print(f"HTTP {status}")
    body = text.strip()
    print(f"响应体（{len(body)} 字符）：{body[:160]}{'…' if len(body) > 160 else ''}")
    try:
        parsed = json.loads(body)
    except json.JSONDecodeError:
        print("  解析：不是合法 JSON")
        return None
    if isinstance(parsed, str):
        raw = base64.b64decode(parsed)
        print(f"  解析：**密文响应** {len(raw)} 字节 = IV 16 + 密文 {len(raw) - 16}")
        print(f"  解密 → {decrypt_body(parsed)}")
    else:
        print(f"  解析：明文 JSON 对象，code={parsed.get('code')!r} message={parsed.get('message')!r}")
    return parsed


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline", action="store_true")
    ap.add_argument("--phase2", action="store_true")
    ap.add_argument("--save", help="把原始响应写进这个 JSON 文件")
    args = ap.parse_args()
    if not (args.baseline or args.phase2):
        args.baseline = args.phase2 = True

    results = []

    def run(label, headers_extra, body_text):
        status, text = call(headers_extra, body_text)
        results.append({"label": label, "headers": headers_extra, "requestBody": body_text, "status": status, "body": text.strip()})
        describe(label, status, text)
        time.sleep(1)

    plain = json.dumps(BODY, separators=(",", ":"), ensure_ascii=False)

    if args.baseline:
        run("A 基线：不带 hcy-cool-flag", {}, plain)
        run("B 带 hcy-cool-flag: 1（明文体）", {"hcy-cool-flag": "1"}, plain)
        run("C 带 hcy-ios-option-enc: 1（明文体）", {"hcy-ios-option-enc": "1"}, plain)

    if args.phase2:
        enc = encrypt_body(BODY)
        run("D **加密体** + hcy-cool-flag: 1", {"hcy-cool-flag": "1"}, enc)
        run("E 加密体（不带任何 hcy 头）", {}, enc)
        run(
            "F 加密体 + hcy-cool-flag + hcy-ios-option-enc",
            {"hcy-cool-flag": "1", "hcy-ios-option-enc": "1"},
            enc,
        )

    print("\n── 判定 ──")
    for r in results:
        try:
            p = json.loads(r["body"])
        except json.JSONDecodeError:
            verdict = "非 JSON"
        else:
            if isinstance(p, str):
                try:
                    verdict = f"密文 → {decrypt_body(p)[:70]}"
                except Exception as e:  # noqa: BLE001
                    verdict = f"密文（解不开：{e}）"
            else:
                verdict = f"明文 code={p.get('code')!r}"
        print(f"  {r['label']:44s} {verdict}")

    if args.save:
        with open(args.save, "w", encoding="utf-8") as f:
            json.dump(
                {
                    "$note": "hcy-cool-flag 探针原始响应（合成号码，不含真实标识）",
                    "$url": URL,
                    "syntheticPhone": SYN_PHONE,
                    "bodyAesKey": BODY_AES_KEY.decode(),
                    "results": results,
                },
                f,
                ensure_ascii=False,
                indent=2,
            )
        print(f"\n已保存 → {args.save}")


if __name__ == "__main__":
    main()
