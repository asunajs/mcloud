#!/usr/bin/env node
/**
 * 黄金向量自检（`test/golden/*.json`）。
 *
 * 存在的理由：向量文件是**要进版本库的产物**，但"有人手工改坏了一个字符"这件事
 * 只有真正算一遍才发现。Rust 侧只 `include_str!` 了其中几个文件 ——
 * 尤其 `app-business-crypto-vectors.json`（App 业务接口的两把锁）此前**没有任何消费者**，
 * 属于"写下来就没人再看"的死文件。本脚本给它配一个独立实现（Node `crypto`），
 * 与 Rust 侧互为交叉验证：一边过一边不过，说明**其中一边错了**，不是"算法大概对"。
 *
 * 覆盖范围是**显式**的：`VERIFIERS` 里没有的段一律打印出来，不假装通过。
 *
 * 用法：
 *   node scripts/check-golden-vectors.cjs
 *   node scripts/check-golden-vectors.cjs --quiet     # 只打印失败与汇总
 *
 * 退出码：0 = 全部已覆盖段通过；1 = 有断言失败或 JSON 解析失败。
 */

"use strict";

const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");

const ROOT = path.resolve(__dirname, "..");
const GOLDEN_DIR = path.join(ROOT, "test", "golden");
const QUIET = process.argv.includes("--quiet");

// ─────────────────────────── 工具 ───────────────────────────

const sha256hex = (s) => crypto.createHash("sha256").update(s, "utf8").digest("hex");
const sha1hexUpper = (s) => crypto.createHash("sha1").update(s, "utf8").digest("hex").toUpperCase();
const md5hex = (s) => crypto.createHash("md5").update(s, "utf8").digest("hex");
const md5hexUpper = (s) => md5hex(s).toUpperCase();
const b64 = (buf) => buf.toString("base64");
const unb64 = (s) => Buffer.from(s, "base64");

/** AES-CBC 加密；`iv` 是 Buffer 或 ASCII 字符串。 */
function aesCbcEncrypt(keyStr, iv, plainStr, keyBytes = 16) {
  const c = crypto.createCipheriv(
    `aes-${keyBytes * 8}-cbc`,
    Buffer.from(keyStr, "utf8"),
    Buffer.from(iv)
  );
  return Buffer.concat([c.update(Buffer.from(plainStr, "utf8")), c.final()]);
}

/** AES-CBC 解密并去掉 PKCS7 补位（`final()` 已自动去补）。 */
function aesCbcDecrypt(keyStr, iv, cipherBuf, keyBytes = 16) {
  const c = crypto.createDecipheriv(
    `aes-${keyBytes * 8}-cbc`,
    Buffer.from(keyStr, "utf8"),
    Buffer.from(iv)
  );
  return Buffer.concat([c.update(cipherBuf), c.final()]).toString("utf8");
}

/**
 * crypto-js 风格的密钥归一化：UTF-8 字节 → **截到 16 / 不足补 0**。
 *
 * `aes128CbcFixedIvHex` 那 20 条向量里有 10 条的 `key` 名字就叫
 * `key-short-zero-padded` / `key-long-truncated` —— 它们锁的**正是这个行为**，
 * 所以这里必须照做，而不是当成"密钥不合法"报错。
 */
function normalizeKey16(keyStr) {
  const k = Buffer.alloc(16);
  Buffer.from(keyStr, "utf8").copy(k, 0, 0, 16);
  return k;
}

function aesCbcFixedKeyEncrypt(keyStr, iv, plainStr) {
  const c = crypto.createCipheriv("aes-128-cbc", normalizeKey16(keyStr), Buffer.from(iv));
  return Buffer.concat([c.update(Buffer.from(plainStr, "utf8")), c.final()]);
}

function aesEcbDecrypt(keyStr, cipherBuf) {
  const c = crypto.createDecipheriv("aes-128-ecb", Buffer.from(keyStr, "utf8"), null);
  c.setAutoPadding(true);
  return Buffer.concat([c.update(cipherBuf), c.final()]).toString("utf8");
}

function aesEcbEncrypt(keyStr, plainStr) {
  const c = crypto.createCipheriv("aes-128-ecb", Buffer.from(keyStr, "utf8"), null);
  return b64(Buffer.concat([c.update(Buffer.from(plainStr, "utf8")), c.final()]));
}

// ─────────────────── 每段一个校验器 ───────────────────
//
// 每个校验器收 (entry, constants)，返回 { actual, expected }。
// 抛异常 = 这条向量算不出来（会记为失败并打印原因）。

/**
 * 签名盐表。
 *
 * 两个向量文件各带一份：`crypto-vectors.json` 是**完整表**（含"未知 saltKey 回落
 * 到 1"这条规则），`app-signin-vectors.json` 只声明签到页用的那一把。
 * 这里统一取，避免把 `sekaMdYYLIZfbCfm` 硬编码进脚本 —— 盐表是 `mcloud-core` 的
 * `SIGN_SALT_MAP`，向量是它的投影，脚本不该有第三份副本。
 */
function signSalt(constants, saltKey) {
  const map = constants.SIGN_SALT_MAP;
  if (map) return map[String(saltKey)] ?? map["1"];
  if (
    constants.H5_SIGN_SALT &&
    Number(saltKey) === Number(constants.H5_SIGN_SALT_KEY ?? 2)
  ) {
    return constants.H5_SIGN_SALT;
  }
  throw new Error(`向量文件没给出 saltKey=${saltKey} 对应的盐`);
}

/**
 * `md5(盐 + id + timestamp + nonce + other + 盐)` —— PC 端签名与 App 签到页签名
 * 是**同一个算式**，只是盐值不同，所以两段共用一个实现。
 * `other` 是 H5 留的预留位（签到恒为空串，PC 有时是路径）。
 */
function signHeaderMd5(e, constants) {
  const salt = signSalt(constants, e.saltKey);
  const id = e.id ?? e.requestId;
  if (id === undefined) throw new Error("向量缺 id / requestId");
  return md5hex(`${salt}${id}${e.timestamp}${e.nonce}${e.other ?? ""}${salt}`);
}

const VERIFIERS = {
  // —— App 业务接口（形态 B）：两把锁 ——
  signatureSha256: (e) => ({ actual: sha256hex(e.input), expected: e.expect }),
  encodeDataDecrypt: (e, c) => {
    const raw = unb64(e.encodeData);
    if (raw.length !== 32) throw new Error(`载荷应为 32 字节（16 IV + 16 密文），实际 ${raw.length}`);
    const iv = raw.subarray(0, 16);
    const ivAsciiOk = iv.toString("ascii") === e.ivAscii;
    return {
      actual: aesCbcDecrypt(c.APP_CLIENT_KEY, iv, raw.subarray(16)),
      expected: e.plaintext,
      extra: `前置 IV == ivAscii: ${ivAsciiOk}`,
    };
  },

  /**
   * `hcy-cool-flag` 的信封：**请求体与响应体都是**同一个格式
   *     payloadB64 = base64( IV(16 字节，**二进制**) ‖ AES-128-CBC/PKCS7(key, iv=IV, JSON) )
   * 注意这里的 IV 是二进制，和 `encodeData` 的"16 个可打印 ASCII"**不是**一个约定 ——
   * 两者共用同一把 `APP_CLIENT_KEY` 之外的独立密钥（`BODY_AES_KEY`）。
   */
  hcyCoolFlagEnvelope: (e, c) => {
    const iv = Buffer.from(e.ivHex, "hex");
    if (iv.length !== 16) throw new Error(`ivHex 应为 16 字节，实际 ${iv.length}`);
    if (e.direction === "response") {
      const raw = unb64(e.payloadB64);
      if (!raw.subarray(0, 16).equals(iv)) {
        throw new Error(`载荷前置 IV 与 ivHex 不一致：${raw.subarray(0, 16).toString("hex")}`);
      }
      return { actual: aesCbcDecrypt(c.BODY_AES_KEY, iv, raw.subarray(16)), expected: e.plaintext };
    }
    // 加密方向：IV 前置拼在密文前面
    const payload = b64(Buffer.concat([iv, aesCbcEncrypt(c.BODY_AES_KEY, iv, e.plaintext)]));
    return { actual: payload, expected: e.payloadB64, extra: `方向=${e.direction}` };
  },

  /**
   * **第二把**整包信封钥匙（2026-09-20 补）。
   *
   * 与 `hcyCoolFlagEnvelope` **格式完全相同**（`base64(IV(16 字节二进制) ‖ CT)`、双向），
   * 差别只在钥匙与算法强度：`BODY_AES_KEY_192`（24 字节 → **AES-192**）。
   *
   * 为什么两把都要留：**它们的证据来源不同** ——
   * `BODY_AES_KEY`（AES-128）是在发码链路上用已知明文暴力命中 + 活体探针验证的；
   * `BODY_AES_KEY_192` 是在旧项目 396 条实拍上逐条解开的，而同一批条目上 AES-128 那把 **0 命中**。
   * 换句话说：这不是"新钥匙取代旧钥匙"，而是"**至少两把，按链路分**"。
   * 判据只能是逐条实拍，不能是"哪把更晚发现"。
   */
  hcyCoolFlagBodyEnvelope192: (e, c) => {
    const iv = Buffer.from(e.ivHex, "hex");
    if (iv.length !== 16) throw new Error(`ivHex 应为 16 字节，实际 ${iv.length}`);
    const raw = unb64(e.payloadB64);
    if (!raw.subarray(0, 16).equals(iv)) {
      throw new Error(`载荷前置 IV 与 ivHex 不一致：${raw.subarray(0, 16).toString("hex")}`);
    }
    const pt = aesCbcDecrypt(c.BODY_AES_KEY_192, iv, raw.subarray(16), 24);
    return { actual: pt, expected: e.plaintext, extra: `AES-192，方向=${e.direction}` };
  },

  // —— 签名头（PC 端 `signHeader` 与 App 签到页 `h5Signature` 同一个算式）——
  //
  // 两段分开登记（不是一个别名）是有意的：它们的**消费方不同** ——
  // `signHeader` 由 `mcloud-core` 的签名测试读，`h5Signature` 由
  // `mcloud-upstream::app_signin` 读。分开写，覆盖汇总里能一眼看出
  // "签到那条到底验没验"，而不是被合并成一行。
  signHeader: (e, c) => ({
    actual: signHeaderMd5(e, c),
    expected: e.expectMd5,
    extra: `saltKey=${e.saltKey}${e.other ? ` other=${e.other}` : ""}`,
  }),
  h5Signature: (e, c) => ({
    actual: signHeaderMd5(e, c),
    expected: e.expectMd5,
    extra: `saltKey=${e.saltKey} other=${JSON.stringify(e.other ?? "")}`,
  }),

  /**
   * `envelopes`：成功 / 失败信封的样本。
   *
   * 本脚本能独立验的是**码值归一化**这条规则 —— 顶层 `code` 与 `raw.code` 的关系：
   * 数字原样；字符串 `"0000"` 归一到 `0`（`Envelope::code()` 的行为，实拍见
   * `querySpecTokenV2-success`，它的 `raw.code` 是字符串而顶层是数字）。
   * 其余语义（`success` 优先于 `code` 判失败、`payload` 取 `data|result`）
   * 由 Rust 侧 `Envelope` 的测试锁定，这里不重复实现 —— 复刻一遍只会得到
   * 同一个 bug 的两份拷贝。
   */
  envelopes: (e) => {
    if (e.raw === undefined) throw new Error("向量缺 raw");
    const rawCode = e.raw.code;
    if (rawCode === undefined) throw new Error("raw 里没有 code");
    const normalized =
      typeof rawCode === "string" ? (rawCode === "0000" ? 0 : Number(rawCode)) : rawCode;
    if (typeof normalized !== "number" || Number.isNaN(normalized)) {
      throw new Error(`raw.code 无法归一：${JSON.stringify(rawCode)}`);
    }
    return {
      actual: normalized,
      expected: e.code,
      extra: `raw.code=${JSON.stringify(rawCode)}`,
    };
  },

  // —— mcloud-sign TS 原版（crypto-vectors.json）——
  md5: (e) => ({ actual: md5hex(e.input), expected: e.expect }),
  sha256: (e) => ({ actual: sha256hex(e.input), expected: e.expect }),
  hmacSha256B64: (e) => ({
    actual: crypto.createHmac("sha256", e.key).update(e.message, "utf8").digest("base64"),
    expected: e.expect,
  }),
  aes128CbcFixedIvHex: (e, c) => ({
    actual: aesCbcFixedKeyEncrypt(e.key, c.AES_FIXED_IV, e.message).toString("hex"),
    expected: e.expect,
    extra: `key「${e.key}」归一化为 16 字节`,
  }),
  aesDynamicIv: (e) => {
    const iv = Buffer.from(e.ivHex, "hex");
    return {
      actual: b64(Buffer.concat([iv, aesCbcEncrypt(e.key, iv, e.plaintext, e.keyBytes)])),
      expected: e.expectB64,
      extra: `AES-${e.keyBytes * 8}（keyBytes=${e.keyBytes}），IV 前置`,
    };
  },

  // —— PC 客户端登录（pcclient / login-crypto）——
  sha1Upper: (e) => ({ actual: sha1hexUpper(e.input), expected: e.expect }),
  sha1Password: (e, c) => ({
    actual: sha1hexUpper((c.HASHTEXT ?? "") + e.password),
    expected: e.expectHexUpper,
    extra: `前缀「${c.HASHTEXT ?? ""}」`,
  }),
  md5SafeVerify: (e) => ({ actual: md5hexUpper(e.input), expected: e.expectHexUpper }),
  puzzleAnswer: (e, c) => {
    // 前两条（真实抓包）自带 `ivAscii`；后六条（合成）省略该字段 → 用常量 PUZZLE_IV。
    const iv = Buffer.from(e.ivAscii ?? c.PUZZLE_IV, "ascii");
    const payload = b64(Buffer.concat([iv, aesCbcEncrypt(c.PUZZLE_KEY, iv, e.plainText)]));
    if (e.totalBytes !== undefined && Buffer.from(payload, "base64").length !== e.totalBytes) {
      throw new Error(`总长应为 ${e.totalBytes}，实得 ${Buffer.from(payload, "base64").length}`);
    }
    if (e.soMatchesRealCapture && e.realCaptureB64 && e.realCaptureB64 !== payload) {
      throw new Error(`与真实抓包不一致：\n      抓包 ${e.realCaptureB64}\n      算出 ${payload}`);
    }
    if (e.cipherB64 !== undefined && b64(unb64(payload).subarray(16)) !== e.cipherB64) {
      throw new Error(`密文段对不上：\n      向量 ${e.cipherB64}\n      算出 ${b64(unb64(payload).subarray(16))}`);
    }
    return { actual: payload, expected: e.expectB64, extra: `IV=${iv.toString("ascii")}` };
  },
  puzzleAesCbcIvPrefixed: (e, c) => {
    const iv = Buffer.from(c.PUZZLE_AES_IV, "ascii");
    return {
      actual: b64(Buffer.concat([iv, aesCbcEncrypt(c.PUZZLE_AES_KEY, iv, e.plaintext)])),
      expected: e.expectB64,
    };
  },
  puzzleLeftDecrypt: (e, c) => {
    // ⚠ 这里**不是**用常量 PUZZLE_AES_IV：向量自己声明"IV 每次随机"，
    // 所以 IV 就拼在载荷前 16 字节里。用常量 IV 会得到"第一块乱码 + 后一块正确"
    // 的假象（CBC 只有第一块依赖 IV）—— 本脚本第一版就这么错过。
    const raw = unb64(e.cipherB64);
    if (raw.length !== 32) throw new Error(`载荷应为 32 字节（16 IV + 16 密文），实际 ${raw.length}`);
    return {
      actual: aesCbcDecrypt(c.PUZZLE_AES_KEY, raw.subarray(0, 16), raw.subarray(16)),
      expected: e.expectPlain,
      extra: "IV 取自载荷前 16 字节",
    };
  },
  loginDataDecrypt: (e, c) => ({
    actual: aesEcbDecrypt(c.LOGIN_AES_KEY, Buffer.from(e.cipherHex, "hex")),
    expected: e.expectPlain,
  }),
  aesEcbHexDecrypt: (e) => ({
    actual: aesEcbDecrypt(e.key, Buffer.from(e.inputHex, "hex")),
    expected: e.expectPlain,
  }),
  aesEcbEncrypt: (e) => ({ actual: aesEcbEncrypt(e.key, e.plainText), expected: e.expectB64 }),

  // —— 原版编译产物（auth-vectors.json）——
  padBase64: (e) => {
    const pad = e.input.length % 4 === 0 ? "" : "=".repeat(4 - (e.input.length % 4));
    return { actual: e.input + pad, expected: e.expect };
  },

  // —— smid（cmpassport 设备指纹）——
  // 这两段是纯 md5 拼接，所以本脚本能用 `node:crypto` **独立**算一遍：
  // 生成侧用的是 crypto-js，两边同一套输入得到同一个值才算交叉验证。
  smidString: (e) => {
    const v = `${e.timeDigits}${md5hex(e.uid)}00`;
    const actual = `${v}${md5hex(`smsk_web_${v}`).slice(0, 14)}0`;
    if (e.expectLen !== undefined && actual.length !== e.expectLen) {
      throw new Error(`长度应为 ${e.expectLen}，实得 ${actual.length}`);
    }
    return { actual, expected: e.expect, extra: "时间 14 位 + md5(uid) + 00 + md5 前 14 位 + 0" };
  },
  smidPriId: (e) => {
    const actual = md5hex(e.uid).slice(0, 16);
    if (e.expectLen !== undefined && actual.length !== e.expectLen) {
      throw new Error(`长度应为 ${e.expectLen}，实得 ${actual.length}`);
    }
    return { actual, expected: e.expect, extra: "md5(uid) 前 16 位 = 上报体的 AES 密钥" };
  },
};

/** 段名 → 一句话说明"为什么没验"。没列出来的段会在末尾统一提醒。 */
const NOT_VERIFIED_REASONS = {
  parseAuth: "语义由 Rust 侧 auth.rs 锁定（本脚本不做 TS 行为复刻）",
  identity: "同上（含 legacyKey/accountId 派生规则）",
  desEcbNoPaddingB64: "Node 的 des-ecb 需 legacy provider；补位规则（零/空格）未确认",
  desEcbPkcs7Decrypt: "同上",
  getTn: "字符串变换，规则未在向量里给出",
  gzip: "向量自己声明不做字节级对拍（deflate 输出因实现而异）",
  rsaPkcs1v15B64: "需公钥加密，Node 侧要做 DER 包装，收益低",
  rsaSelfKeyPair: "同上",
  guid32: "uuid→md5 的具体拼接未在向量里给出",
  smidTn: "getTn 的排序/缩放规则未在本脚本复刻（同 §getTn）；由 Rust 侧 + 生成侧两份实现交叉",
  smidDesRules: "同 §desEcbNoPaddingB64：Node 的 des-ecb 需 legacy provider，故只由 Rust 侧断言",
  receiveRevivalReward: "纯响应体计数（cardType/rewardClouds/totalClouds），无算法可复算；由 Rust 侧解析断言",
  infoV3: "纯响应体（余额 / 签到日历），无算法可复算；由 Rust 侧 parse_sign_in_status 断言",
  startSignIn: "同上；由 Rust 侧 parse_start_sign_in 断言",
  prizeRecords: "纯响应体（奖品记录一条），无算法可复算；由 Rust 侧 parse_prize_records 断言",
};

// ─────────────────── 脱敏扫描（镜像 Rust 侧守卫）───────────────────
//
// Rust 侧 `mcloud_core::test_support::assert_only_synthetic_identifiers` 只作用于
// **有测试去 include_str! 的文件**。本脚本对**每个** golden 文件都扫一遍，
// 补上"新加的向量文件还没接线、于是没人扫"这个洞。
const SYNTHETIC_PREFIX = "199";

function scanIdentifiers(text, label, problems) {
  for (const [width, kind] of [[11, "手机号"], [19, "userDomainId"]]) {
    const bytes = Buffer.from(text, "utf8");
    for (let start = text.indexOf("1"); start !== -1; start = text.indexOf("1", start + 1)) {
      const end = start + width;
      if (end > bytes.length) continue;
      const slice = bytes.subarray(start, end);
      if (![...slice].every((b) => b >= 0x30 && b <= 0x39)) continue;
      // 边界必须是"非字母数字"，否则会 19 位切成 11 位误判
      const bounded = (i) => i === undefined || i < 0 || i >= bytes.length || !isAlnum(bytes[i]);
      if (!bounded(start - 1) || !bounded(end)) continue;
      const candidate = text.slice(start, end);
      if (!candidate.startsWith(SYNTHETIC_PREFIX)) {
        problems.push(`${label} 出现疑似真实${kind} ${candidate}（只允许 ${SYNTHETIC_PREFIX} 开头的合成值）`);
      }
    }
  }
}

const isAlnum = (b) =>
  (b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5a) || (b >= 0x61 && b <= 0x7a);

/** 文本里所有"看着像 base64"的片段解码后能当文本读的，返回出来。 */
function decodeBase64Payloads(text) {
  const out = [];
  for (const m of text.matchAll(/[A-Za-z0-9+/]{16,}={0,2}/g)) {
    const frag = m[0];
    if (frag.length % 4 !== 0) continue;
    let buf;
    try {
      buf = Buffer.from(frag, "base64");
    } catch {
      continue;
    }
    // 滤掉 hex 串（sha256 之类）硬解出来的二进制垃圾
    if (buf.length === 0 || [...buf].some((b) => b < 0x09 || (b > 0x0d && b < 0x20))) continue;
    out.push(buf.toString("utf8"));
  }
  return out;
}

// ─────────────────────────── 主流程 ───────────────────────────

/** 哪些 golden 文件真的被 Rust 侧读了（`include_str!`）。 */
function consumedByRust() {
  const used = new Set();
  const walk = (dir) => {
    for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, ent.name);
      if (ent.isDirectory()) walk(p);
      else if (ent.name.endsWith(".rs")) {
        // `cargo fmt` 会把超长的 `include_str!("…")` 折成多行（相对路径在
        // `src/<模块>/tests/` 下尤其容易顶到 max_width），所以括号与字面量
        // 之间可能有换行 —— 这里容忍空白，不然会误报"死向量"。
        for (const m of fs.readFileSync(p, "utf8").matchAll(/include_str!\(\s*"([^"]+)"\s*\)/g)) {
          const base = path.basename(m[1]);
          if (base.endsWith(".json")) used.add(base);
        }
      }
    }
  };
  walk(path.join(ROOT, "crates"));
  return used;
}

function main() {
  const files = fs.readdirSync(GOLDEN_DIR).filter((f) => f.endsWith(".json")).sort();
  if (files.length === 0) {
    console.error(`✗ ${GOLDEN_DIR} 里没有 *.json`);
    process.exit(1);
  }

  const used = consumedByRust();
  const failures = [];
  const identityProblems = [];
  let checked = 0;
  const sectionReport = [];
  const noVerifier = new Map(); // 段名 → [文件…]

  for (const file of files) {
    const raw = fs.readFileSync(path.join(GOLDEN_DIR, file), "utf8");
    let data;
    try {
      data = JSON.parse(raw);
    } catch (err) {
      failures.push(`${file} JSON 解析失败：${err.message}`);
      continue;
    }

    // 1) 脱敏扫描（整段文本 + 所有 base64 载荷）
    scanIdentifiers(raw, file, identityProblems);
    for (const decoded of decodeBase64Payloads(raw)) {
      scanIdentifiers(decoded, `${file}（base64 解码后）`, identityProblems);
    }

    // 2) 逐段算
    const constants = data.constants ?? {};
    const fileLines = [];
    for (const [section, entries] of Object.entries(data)) {
      // `$` 开头的是元数据（`$source` / `$note` / `$evidence`），不是向量段；
      // `constants` 是各段共用的常量表，也不是段。
      if (section.startsWith("$") || section === "constants") continue;
      if (!Array.isArray(entries)) {
        // 单条实拍的段写成**对象**（`infoV3` / `startSignIn` / `receiveRevivalReward`）。
        // ⚠ 这些也必须出现在"未覆盖"清单里 —— 早先这里直接 `continue`，
        // 于是它们在报告里**彻底消失**，整份输出读起来像"所有段都验过了"。
        // **没验就得说出来**，这是本脚本存在的理由（见文件头）。
        if (entries && typeof entries === "object") {
          noVerifier.set(section, [...(noVerifier.get(section) ?? []), file]);
        }
        continue;
      }
      if (entries.length === 0) continue;
      const verify = VERIFIERS[section];
      if (!verify) {
        noVerifier.set(section, [...(noVerifier.get(section) ?? []), file]);
        continue;
      }
      let ok = 0;
      for (const [i, e] of entries.entries()) {
        const label = `${file} §${section}[${i}] ${e.name ?? e.plaintext ?? e.input ?? ""}`.trim();
        const shouldError = e.expectError === true;
        try {
          const { actual, expected, extra } = verify(e, constants);
          if (shouldError) {
            failures.push(`${label}\n      期望报错，却算出了 ${actual}`);
          } else if (actual === expected) {
            ok++;
            checked++;
            if (!QUIET) fileLines.push(`    ✓ ${label}${extra ? `  (${extra})` : ""}`);
          } else {
            failures.push(
              `${label}\n      期望 ${expected}\n      实得 ${actual}` +
                (extra ? `\n      ${extra}` : "")
            );
          }
        } catch (err) {
          if (shouldError) {
            ok++;
            checked++;
            const short = err.message.split("\n")[0].slice(0, 60);
            if (!QUIET) fileLines.push(`    ✓ ${label}  (如预期报错：${short}…)`);
          } else {
            failures.push(`${label}\n      算不出来：${err.message}`);
          }
        }
      }
      sectionReport.push({ file, section, ok, total: entries.length });
    }
    if (!QUIET && fileLines.length) {
      console.log(`\n[${file}]${used.has(file) ? "" : "   ⚠ 尚无 Rust 测试读取"}`);
      console.log(fileLines.join("\n"));
    }
  }

  // ── 汇总 ──
  console.log("\n── 覆盖汇总 ──");
  for (const { file, section, ok, total } of sectionReport) {
    console.log(`  ${ok === total ? "✓" : "✗"} ${file.padEnd(34)} §${section.padEnd(24)} ${ok}/${total}`);
  }
  if (noVerifier.size) {
    console.log("\n── 本脚本**未覆盖**的段（不算通过，别当背书）──");
    for (const [section, inFiles] of noVerifier) {
      const why = NOT_VERIFIED_REASONS[section] ?? "没有对应校验器";
      console.log(`  · §${section.padEnd(24)} ${why}`);
      console.log(`    ${inFiles.join(", ")}`);
    }
  }

  console.log("\n── 脱敏（镜像 Rust 侧守卫）──");
  if (identityProblems.length === 0) {
    console.log(`  ✓ ${files.length} 个文件、字面 + base64 解码后，均只出现 ${SYNTHETIC_PREFIX} 开头的合成标识`);
  } else {
    for (const p of identityProblems) console.log(`  ✗ ${p}`);
  }

  console.log("\n── 消费者 ──");
  for (const f of files) {
    console.log(`  ${used.has(f) ? "✓ 有 Rust 测试读取" : "⚠ 无任何 Rust 测试读取（死向量）"}  ${f}`);
  }

  const totalProblems = failures.length + identityProblems.length;
  console.log(
    `\n${totalProblems === 0 ? "✓ 通过" : "✗ 失败"}：断言 ${checked} 条全绿` +
      (totalProblems ? `，问题 ${totalProblems} 处` : "")
  );
  if (failures.length) {
    console.log("\n── 失败明细 ──");
    for (const f of failures) console.log(`  ✗ ${f}`);
  }
  process.exit(totalProblems === 0 ? 0 : 1);
}

main();
