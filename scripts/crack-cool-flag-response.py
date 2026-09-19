#!/usr/bin/env python3
"""已知明文攻击：`hcy-cool-flag` 的密文响应用什么密钥。

背景（见 docs/capture/app-sms-login-samples.txt §1.1）：
  带 `hcy-cool-flag` 打 `/user/sms/getSmsCode`，响应体是一个**裸 JSON 字符串**
  （192 个 base64 字符），解出**固定 144 字节**、每次请求都不同。
  此前结论是"密钥来源未知"，因为**没有已知明文**可用来判定候选钥匙对不对。

本脚本补上那个缺失的前提：

    明文 = 同一份 JSON 的 **`\\uXXXX` 转义**形态
         = 140 字节 → PKCS7 补 4 → 密文 **144 字节**，且**无 IV 前缀**
    （原样 UTF-8 是 107 字节 → 112 字节，对不上 144；加 16 字节前缀是 128，也对不上）

于是"猜钥匙"变成**可判定**的：解密结果里必须出现 `200059554` 或能解析成 JSON。

用法：
    python3 scripts/crack-cool-flag-response.py har/hcy-cool-flag-samples.json
    python3 scripts/crack-cool-flag-response.py har/hcy-cool-flag-samples.json --sm4
"""

import argparse
import base64
import json
import os
import re
import sys

from Crypto.Cipher import AES

BINARIES = [
    "/Applications/mCloud.app/Wrapper/mCloud_iPhone.app/mCloud_iPhone",
]
FRAMEWORK_DIR = "/Applications/mCloud.app/Wrapper/mCloud_iPhone.app/Frameworks"

MARKERS = [b"200059554", b'"success"', b"captchaType"]

KEY_LENGTHS = (16, 24, 32)


def candidate_keys():
    """从 App 二进制与所有 framework 里取"看起来能当密钥"的字符串。"""
    paths = list(BINARIES)
    if os.path.isdir(FRAMEWORK_DIR):
        for name in sorted(os.listdir(FRAMEWORK_DIR)):
            p = os.path.join(FRAMEWORK_DIR, name, name.split(".")[0])
            if os.path.isfile(p):
                paths.append(p)
    seen = set()
    for p in paths:
        try:
            data = open(p, "rb").read()
        except OSError:
            continue
        for m in re.finditer(rb"[\x21-\x7e]{%d,32}" % min(KEY_LENGTHS), data):
            s = m.group()
            for n in KEY_LENGTHS:
                if len(s) == n and s not in seen:
                    seen.add(s)
    return sorted(seen)


def plaintext_candidates():
    raw = {
        "success": False,
        "code": "200059554",
        "message": "请先获取滑块拼图验证码",
        "data": {"captchaType": 2},
    }
    return [
        ("CJK 转义 \\uXXXX", json.dumps(raw, separators=(",", ":"), ensure_ascii=True).encode()),
        ("原样 UTF-8", json.dumps(raw, separators=(",", ":"), ensure_ascii=False).encode()),
    ]


def load_ciphertexts(path):
    j = json.load(open(path, encoding="utf-8"))
    out = []
    for r in j["results"]:
        try:
            p = json.loads(r["body"])
        except (json.JSONDecodeError, KeyError):
            continue
        if isinstance(p, str):
            out.append((r["label"], base64.b64decode(p)))
    return out


def looks_right(pt):
    return any(m in pt for m in MARKERS)


def try_aes(ct, key, iv):
    for mode, ivarg in (("ECB", None), ("CBC", iv)):
        try:
            # ⚠ pycryptodome 对 ECB **不接受显式 None** 作 IV
            # （`AES.new(k, MODE_ECB, None)` → TypeError: IV is not meaningful for the ECB mode）。
            # 必须按模式分别构造参数，不能统一传 ivarg。
            c = AES.new(key, AES.MODE_ECB) if ivarg is None else AES.new(key, AES.MODE_CBC, ivarg)
            pt = c.decrypt(ct)
            # 手工去 PKCS7（不信任库的自动去补，避免"凑巧合法"的假阳性）
            n = pt[-1]
            if 1 <= n <= 16 and pt[-n:] == bytes([n]) * n:
                pt = pt[:-n]
            if looks_right(pt):
                return mode, pt
        except (ValueError, KeyError, TypeError):
            continue
    return None


def try_sm4(ct, key, iv):
    try:
        from gmssl.sm4 import CryptSM4, SM4_DECRYPT
    except ImportError:
        return None
    for mode in ("ECB", "CBC"):
        try:
            c = CryptSM4()
            c.set_key(key, SM4_DECRYPT)
            pt = c.crypt_ecb(ct) if mode == "ECB" else c.crypt_cbc(iv, ct)
            n = pt[-1]
            if 1 <= n <= 16 and pt[-n:] == bytes([n]) * n:
                pt = pt[:-n]
            if looks_right(pt):
                return mode, pt
        except Exception:  # noqa: BLE001 — 第三方库的异常类型不稳定
            continue
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("samples", help="har/hcy-cool-flag-samples.json")
    ap.add_argument("--sm4", action="store_true", help="同时试 SM4（需 gmssl）")
    ap.add_argument("--limit", type=int, default=0, help="只试前 N 个候选（调试用）")
    args = ap.parse_args()

    ciphertexts = load_ciphertexts(args.samples)
    if not ciphertexts:
        print("样本里没有密文响应，先跑 scripts/probe-hcy-cool-flag.py")
        return 1

    print("── 已知明文（长度必须能凑出 144）──")
    pts = plaintext_candidates()
    for name, b in pts:
        pad = 16 - len(b) % 16
        print(f"  {name:18s} {len(b):3d} 字节 → 无前缀总长 {len(b) + pad}")

    print("\n── 密文样本 ──")
    for label, ct in ciphertexts:
        print(f"  {label}: {len(ct)} 字节")

    keys = candidate_keys()
    if args.limit:
        keys = keys[: args.limit]
    print(f"\n候选密钥 {len(keys)} 个（长度 {KEY_LENGTHS}）")

    hits = []
    for label, ct in ciphertexts:
        ivs = {"零 IV": bytes(16), "前 16 字节": ct[:16], "后 16 字节": ct[-16:]}
        for i, k in enumerate(keys, 1):
            if i % 20000 == 0:
                print(f"  … {i}/{len(keys)}", file=sys.stderr)
            for ivname, iv in ivs.items():
                got = try_aes(ct, k, iv)
                if got:
                    mode, pt = got
                    hits.append((label, k, mode, ivname, pt))
                    print(f"\n★ 命中！{label}\n  key={k!r} mode=AES-{mode} iv={ivname}\n  {pt[:160]!r}")
            if args.sm4:
                for ivname, iv in (("零 IV", bytes(16)), ("前 16 字节", ct[:16])):
                    got = try_sm4(ct, k, iv)
                    if got:
                        mode, pt = got
                        hits.append((label, k, f"SM4-{mode}", ivname, pt))
                        print(f"\n★ 命中！{label}\n  key={k!r} mode=SM4-{mode} iv={ivname}\n  {pt[:160]!r}")

    print(f"\n{'✓ 命中 ' + str(len(hits)) + ' 条' if hits else '✗ 未命中'}"
          f"（候选 {len(keys)} × 密文 {len(ciphertexts)} × 模式 {'AES+SM4' if args.sm4 else 'AES'}）")
    return 0 if hits else 2


if __name__ == "__main__":
    sys.exit(main())
