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

// ─────────────────────────── 单 DES ───────────────────────────
//
// ⚠ `des-ecb` 在本机 Node 22 / OpenSSL 3.5 上**不可用**
// （`error:0308010C:digital envelope routines::unsupported` —— 单 DES 被挪进了
// legacy provider）。但**"不可用"不等于"做不了"**：
//
//   `des-ede3-ecb` 是 3DES，三把子密钥相同时
//   E_K(D_K(E_K(x))) == E_K(x)，即**退化成单 DES**。
//
// 所以把 8 字节密钥复制成 24 字节喂给 `des-ede3-ecb` 即可，不需要 legacy provider。
// 这条是**实测**出来的，不是推断：`desEcbNoPaddingB64` 那 18 条向量逐字节全过。
//
// 早先这里挂着一句"Node 的 des-ecb 需 legacy provider"，于是整段被当成"验不了"
// 豁免掉了 —— 那是把"某个 API 名不可用"当成了"这个算法不可验证"。

/** 密钥归一化：UTF-8 字节 → **截到 8 / 不足补 0**（向量名 `key-short-zero-padded` 锁的就是它）。 */
function normalizeKey8(keyStr) {
  const k = Buffer.alloc(8);
  Buffer.from(keyStr, "utf8").copy(k, 0, 0, 8);
  return k;
}

/** 明文零补到 8 的整数倍（**已对齐时不追加整块**，空串仍是空串）。 */
function zeroPad8(buf) {
  if (buf.length % 8 === 0) return buf;
  const out = Buffer.alloc(Math.ceil(buf.length / 8) * 8);
  buf.copy(out);
  return out;
}

function desKey24(keyStr) {
  const k = normalizeKey8(keyStr);
  return Buffer.concat([k, k, k]);
}

function desEcbEncryptNoPad(keyStr, msgBuf) {
  const c = crypto.createCipheriv("des-ede3-ecb", desKey24(keyStr), null);
  c.setAutoPadding(false);
  return Buffer.concat([c.update(msgBuf), c.final()]);
}

function desEcbDecryptNoPad(keyStr, ctBuf) {
  const c = crypto.createDecipheriv("des-ede3-ecb", desKey24(keyStr), null);
  c.setAutoPadding(false);
  return Buffer.concat([c.update(ctBuf), c.final()]);
}

/** 手工去 PKCS#7 补位（`des-ede3-ecb` 上关掉了自动补位，得自己剥）。 */
function pkcs7Unpad8(buf) {
  const n = buf[buf.length - 1];
  if (n < 1 || n > 8) throw new Error(`PKCS#7 补位字节非法：${n}`);
  return buf.subarray(0, buf.length - n);
}

/**
 * `getTn` —— 设备指纹序列化（原实现 `shared/src/smid/index.ts`）。
 *
 * 规则：键名排序后逐值拼接；number **整数** ×10000、**小数** floor(×10000)；
 * object（非 null）递归；其余走 `String(v)`（所以 `null` → `"null"`）。
 *
 * ⚠ **这条的自检强度弱于上面那些**：生成侧 `gen-crypto-vectors.cjs` 也是 JS，
 * 同一套语义 ⇒ 它只锁"**向量文件与规则一致**"（手工改坏一个字符会被抓出来），
 * 锁不了"规则本身对不对" —— 后者由 Go 侧 `core.GetTn` 的用例负责。
 * 相比之下 DES 那段是真交叉验证（Node OpenSSL vs 生成侧 crypto-js，两套实现）。
 *
 * ⚠ 已知差异：JS 的 `JSON.parse` 把 `7.0` 读成整数 7，而 serde_json 会当 f64
 * （决定要不要 floor）。向量里没有 `x.0` 形式的字面量，所以这里不受影响；
 * 真加进去了，本脚本与 Go 侧会给出不同答案。
 */
function getTn(obj) {
  return Object.keys(obj)
    .sort()
    .map((k) => {
      const v = obj[k];
      if (typeof v === "number") {
        const scaled = v * 10000;
        return String(Number.isInteger(v) ? scaled : Math.floor(scaled));
      }
      if (v !== null && typeof v === "object") return getTn(v);
      return String(v);
    })
    .join("");
}

/** 从 SPKI DER 公钥里取模数 / 指数，返回**整数值的十进制串**（不受十六进制补零写法影响）。 */
function rsaModExpDecimal(publicKeyB64) {
  const jwk = crypto
    .createPublicKey({ key: unb64(publicKeyB64), format: "der", type: "spki" })
    .export({ format: "jwk" });
  const dec = (b64url) => BigInt("0x" + Buffer.from(b64url, "base64url").toString("hex")).toString(10);
  return { modulus: dec(jwk.n), exponent: dec(jwk.e) };
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

  // WebView / H5 那一层：同一套信封格式，**第三把钥匙**，AES-256。
  //
  // 这里额外钉住一件 192 分支没有的事：**IV 的可打印性**。
  // H5 用 `Bn.generateRandomString(16)` 造 IV（随机**字符**），原生用随机**字节** ——
  // 所以「前 16 字节能不能打印」就是「这个载荷是哪一层发的」的指纹。
  // 指纹一旦漂移（比如某天 H5 改成随机字节），这条断言会先响，
  // 而不是等到线上解不开才发现。
  hcyCoolFlagWebViewEnvelope256: (e, c) => {
    const iv = Buffer.from(e.ivHex, "hex");
    if (iv.length !== 16) throw new Error(`ivHex 应为 16 字节，实际 ${iv.length}`);
    const raw = unb64(e.payloadB64);
    if (!raw.subarray(0, 16).equals(iv)) {
      throw new Error(`载荷前置 IV 与 ivHex 不一致：${raw.subarray(0, 16).toString("hex")}`);
    }
    const printable = iv.every((b) => b >= 0x20 && b < 0x7f);
    if (e.ivKind === "ascii" && !printable) {
      throw new Error(`ivKind=ascii 但 IV 不是全可打印：${e.ivHex}`);
    }
    if (e.ivKind === "binary" && printable) {
      throw new Error(`ivKind=binary 但 IV 恰好全可打印：${e.ivHex}（换一条真实 IV）`);
    }
    const pt = aesCbcDecrypt(c.BODY_AES_KEY_256_H5, iv, raw.subarray(16), 32);
    return {
      actual: pt,
      expected: e.plaintext,
      extra: `AES-256，方向=${e.direction}，IV=${e.ivKind}`,
    };
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

  /**
   * `getGUID32()`：`MD5(UTF8(uuid))` 取**大写** hex，再拼 `-ENDIN` 得设备指纹。
   *
   * 规则逐字来自生成器（`scripts/gen-login-crypto-vectors.cjs` 的 `getGUID32`），
   * 所以本脚本能独立算一遍。
   *
   * ⚠ 这条验的是**向量文件自身**（声明的输入与输出对得上），
   * 不是"某一侧复刻了 md5(uuid)" —— **两侧都没有复刻**：
   * Go 与 Rust 都是 `hex(16 个随机字节)`，而上游只看得到那 32 个字符，
   * 分辨不出两者（见 `internal/app/device_test.go`）。
   * 别把这条的 ✓ 读成"这条路径被实现了"。
   */
  guid32: (e, c) => {
    const actual = md5hexUpper(e.uuidV4);
    if (e.expectLen !== undefined && actual.length !== e.expectLen) {
      throw new Error(`长度应为 ${e.expectLen}，实得 ${actual.length}`);
    }
    if (e.deviceFingerprint !== undefined) {
      const want = actual + (c.DEVICE_FINGERPRINT_SUFFIX ?? "-ENDIN");
      if (e.deviceFingerprint !== want) {
        throw new Error(`设备指纹对不上：\n      向量 ${e.deviceFingerprint}\n      算出 ${want}`);
      }
    }
    return { actual, expected: e.expectHexUpper, extra: `${e.uuidV4} → md5 大写` };
  },

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
  smidTn: (e) => ({
    actual: md5hex(getTn(e.target)),
    expected: e.expect,
    extra: "md5(getTn(target))，target.tn 为空串时求摘要",
  }),

  // —— 单 DES（见上面「单 DES」一节：`des-ecb` 不可用，但 3DES 退化可用）——
  desEcbNoPaddingB64: (e) => {
    const key = normalizeKey8(e.key);
    const msg = zeroPad8(Buffer.from(e.message, "utf8"));
    const actual = b64(desEcbEncryptNoPad(e.key, msg));
    const keyKind =
      Buffer.from(e.key, "utf8").length === 8
        ? "密钥 8 字节整"
        : Buffer.from(e.key, "utf8").length < 8
          ? "密钥零补到 8"
          : "密钥截到 8";
    return { actual, expected: e.expect, extra: `${keyKind}（${key.toString("hex")}），明文 ${msg.length} 字节` };
  },
  desEcbPkcs7Decrypt: (e) => ({
    actual: pkcs7Unpad8(desEcbDecryptNoPad(e.key, unb64(e.cipherB64))).toString("utf8"),
    expected: e.expectPlain ?? e.expect,
    extra: "手工剥 PKCS#7 补位",
  }),

  // —— getTn（纯字符串变换，见上面 getTn 的强度说明）——
  getTn: (e) => ({
    actual: getTn(e.input),
    expected: e.expect,
    extra: "键名排序 → 整数×10000 / 小数 floor(×10000) / 对象递归 / 其余 String(v)",
  }),

  // —— RSA PKCS#1 v1.5 加密 ——
  //
  // ⚠ 这条**没有可对拍的期望值**：PKCS#1 v1.5 的填充字节是随机的，所以向量自己
  // 声明 `byteExact: false`。能锁的只有**不变量**，而它们由下面的 `throw` 承担
  // （返回的 actual === expected 是刻意空转的，别把它当断言）。
  //
  // 其中"两次采样必须不同"这条是**判别力最强**的一条：它直接钉住"含随机填充"，
  // 与生成器里那句"确定性填充，可字节级对拍"的旧注释互斥 —— 两个说法只能活一个。
  rsaPkcs1v15B64: (e) => {
    const { modulus } = rsaModExpDecimal(e.publicKeyB64);
    const n = BigInt(modulus);
    for (const s of e.samples) {
      const buf = unb64(s);
      if (buf.length !== e.modulusBytes) {
        throw new Error(`采样长度应为 ${e.modulusBytes} 字节，实得 ${buf.length}`);
      }
      if (BigInt("0x" + buf.toString("hex")) >= n) {
        throw new Error(`密文整数值不小于 n —— 这不是一把合法公钥加密出来的`);
      }
    }
    if (new Set(e.samples).size !== e.samples.length) {
      throw new Error(
        `两次采样出现了相同值 —— 说明填充是确定的，与向量自己的 byteExact:false / "含随机填充" 矛盾`
      );
    }
    // 上面几条 `throw` 才是真断言；这里返回一对相同的字面量，是让框架的
    // `actual === expected` 空转通过（这条向量没有可对拍的期望值）。
    return {
      actual: "不变量成立",
      expected: "不变量成立",
      extra: `${e.samples.length} 次采样均为 ${e.modulusBytes} 字节、< n 且互不相同`,
    };
  },
};

/**
 * **对象形**（非数组）的段：结构是"一个对象装好几组期望值"，套不进逐条 entry 的形状。
 *
 * 校验器返回 `[{name, actual, expected, extra}]` —— 与数组段的条目同形，
 * 于是计数、失败格式、`--quiet` 全部复用同一套。
 */
const DICT_VERIFIERS = {
  // 自持密钥对：DER 公钥解出的 n/e 必须与向量里那四个字段同值，且 `cases` 的密文
  // 必须能被配对的私钥解回原 message —— 这是一条**字节级**的独立复算。
  rsaSelfKeyPair: (obj) => {
    const { modulus, exponent } = rsaModExpDecimal(obj.publicKeyB64);
    const out = [
      {
        name: "modulus",
        actual: modulus,
        expected: BigInt("0x" + obj.modulusHex).toString(10),
        extra: "DER 解出的 n 与 modulusHex 同值",
      },
      {
        name: "exponent",
        actual: exponent,
        // ⚠ 按**整数值**比，不按写法比：生成器把 exponentHex 走了一遍
        // `BigInt(...).toString(16)`，前导零被吃掉（`010001` → `10001`），
        // 而 modulusHex 是字节对齐的。两者写法不同但值必须相同。
        expected: BigInt("0x" + obj.exponentHex).toString(10),
        extra: "DER 解出的 e 与 exponentHex 同值（十六进制补零写法有意不同）",
      },
    ];
    const priv = crypto.createPrivateKey({
      key: unb64(obj.privateKeyPkcs8B64),
      format: "der",
      type: "pkcs8",
    });
    for (const c of obj.cases ?? []) {
      const plain = crypto
        .privateDecrypt({ key: priv, padding: crypto.constants.RSA_PKCS1_PADDING }, unb64(c.cipherB64))
        .toString("utf8");
      out.push({ name: `cases:${c.name}`, actual: plain, expected: c.message, extra: "私钥解密回原文" });
    }
    return out;
  },
};

/** 段名 → 一句话说明"为什么没验"。没列出来的段会在末尾统一提醒。 */
// ⚠ 这张表是**会过期**的：它曾经有 14 条，其中 6 条在 2026-09-21 被实测推翻 ——
//   不是"条件变了"，而是**当时就没验过**：
//     · `desEcbNoPaddingB64` / `desEcbPkcs7Decrypt` —— 理由写"Node 的 des-ecb 需
//       legacy provider"（**半真**：API 名确实不可用），但由此推出"所以验不了"是错的，
//       3DES 退化成单 DES 即可，21 条向量逐字节全过；
//     · 同两条还写"补位规则（零/空格）未确认"—— 向量名 `key-short-zero-padded` /
//       `key-long-truncated` 就是**专门锁这个**的，本文件 `normalizeKey16` 的注释
//       早就把这条道理讲过了（同一个文件里两处自相矛盾）；
//     · `getTn` 写"规则未在向量里给出"—— 向量里 4 条 input→expect 齐全，
//       而且 `scripts/gen-crypto-vectors.cjs` 里就躺着一份 JS 实现；
//     · `rsaPkcs1v15B64` / `rsaSelfKeyPair` 写"DER 包装收益低"—— 包装只有 15 行，
//       也已经在生成器里。
//   ⇒ 教训：**"某个 API 名不可用"不等于"这个算法不可验证"**；写豁免理由时，
//   要给出的是"验不了"，不是"想不出怎么验"。宁可写"本脚本没有第二份实现（抄一份
//   就是第二份真相）"，也不要写一个听起来合理的借口 —— 后者没人会回头复查。
const NOT_VERIFIED_REASONS = {
  parseAuth: "语义由 `internal/core/auth.go` 锁定（本脚本不做 TS 行为复刻）",
  identity: "同上（含 legacyKey/accountId 派生规则）",
  gzip: "向量自己声明不做字节级对拍（deflate 输出因实现而异）",
  smidDesRules:
    "整张改名表（25 条 DES 规则 + 每键一把密钥）在本脚本里没有第二份实现 —— 抄一份进来就是「第二份真相」；由 Go 侧 `internal/upstream` 的 smid 用例断言。⚠ DES 本身**不是**障碍了，见上面「单 DES」",
  receiveRevivalReward:
    "纯响应体计数（cardType/rewardClouds/totalClouds），无算法可复算；由 Go 侧 `upstream.ParseRevivalReward` 断言",
  infoV3: "纯响应体（余额 / 签到日历），无算法可复算；由 Go 侧 `upstream.ParseSignInStatus` 断言",
  startSignIn: "同上；由 Go 侧 `upstream.ParseStartSignIn` 断言",
  prizeRecords: "纯响应体（奖品记录一条），无算法可复算；由 Go 侧 `upstream.ParsePrizeRecords` 断言",
};

/**
 * 本次**真的被查到**的豁免条目（= 真的有段落在「未覆盖」清单里）。
 *
 * ⚠ 与 `Object.keys(NOT_VERIFIED_REASONS).length` 是两回事：那个量的是**表**，
 * 这个量的是**事实**。向量文件里把段名改掉（或删掉整段）之后，表里那一条
 * 就**再也不会被查到** —— 而"没被查到"的表现是它**不打印**，也就是静默失效。
 * 判据同 §6.1 **#92 / #93**：**表过期不会自报**。
 */
const usedReasons = new Set();

/** 递归收集 Go 源文件（仓库相对路径）。 */
function walkGo(dir, out = []) {
  const abs = path.join(ROOT, dir);
  if (!fs.existsSync(abs)) return out;
  for (const e of fs.readdirSync(abs, { withFileTypes: true })) {
    const rel = path.posix.join(dir, e.name);
    if (e.isDirectory()) walkGo(rel, out);
    else if (e.name.endsWith(".go")) out.push(rel);
  }
  return out;
}

/**
 * 豁免理由里点名的**消费者**必须真的存在。
 *
 * 这张表的 **key** 有守卫（段名对不上会红），**理由文字**没有 ——
 * 而其中大部分是**可证伪**的：「由 Go 侧 `upstream.ParsePrizeRecords` 断言」
 * 「语义由 `internal/core/auth.go` 锁定」。
 *
 * ⚠ 判据：理由若说"这一节有人管"，那就要**指得出是谁**。指不出来，这条豁免就是
 * 把「没人验」伪装成「已交代」—— 而它是最坏的一种豁免，因为读的人会**因此不再看那一段**。
 *
 * ⚠⚠ 与向量文件里那些**写死的** `$note` 不同（那份只可能一开始就错、
 * 不会后来变错），**这些引用会烂**：把 `ParsePrizeRecords` 改个名，豁免照样打印，
 * 而这一节从此**真的没人验**。
 *
 * 只核对**反引号点名**的引用；纯叙述（"同上" / "向量自己声明…"）不做机械核对，
 * 但会**列出来**，让"这条没人核过"是可见的。
 */
function checkReasonConsumers() {
  // 这张表的"消费者"钉的是 **Go 侧**（internal/ 与 cmd/）—— 那些代码在 2026-09-21
  // 之后属于**同级仓库 mcloud**，不在本仓库里。跨仓解析超出本脚本范围；
  // 但**不假装核过**：显式跳过并说明，核对由 mcloud 侧的同名门禁负责。
  if (!fs.existsSync(path.join(ROOT, "internal")) && !fs.existsSync(path.join(ROOT, "cmd"))) {
    return {
      problems: [],
      narration: [],
      skipped: "豁免表点名的消费者在 Go 侧（同级仓库 mcloud），本仓库不做跨仓解析 —— 未核，非通过",
    };
  }
  const problems = [];
  const narration = [];
  const goAll = [...walkGo("internal"), ...walkGo("cmd")];
  const src = new Map(goAll.map((f) => [f, fs.readFileSync(path.join(ROOT, f), "utf8")]));
  const prod = goAll.filter((f) => !f.endsWith("_test.go"));
  const tests = goAll.filter((f) => f.endsWith("_test.go"));

  for (const [section, reason] of Object.entries(NOT_VERIFIED_REASONS)) {
    const refs = [...reason.matchAll(/`([^`]+)`/g)].map((m) => m[1]);
    if (refs.length === 0) {
      narration.push(section);
      continue;
    }
    for (const ref of refs) {
      // ① 带 `/` 的当路径：文件或目录都要真的在
      if (ref.includes("/")) {
        if (!fs.existsSync(path.join(ROOT, ref))) {
          problems.push(`§${section} 的理由点名了 \`${ref}\`，但它在仓库里**不存在**`);
        }
        continue;
      }
      // ② `pkg.Symbol`：符号要在**生产代码**里定义，且被某个 `_test.go` 引用
      const m = /^(?:[a-z_]\w*\.)?([A-Z]\w+)$/.exec(ref);
      if (!m) continue; // 既不是路径也不是导出符号 ⇒ 不判
      const sym = m[1];
      const defined = prod.filter((f) => new RegExp(`func\\s+(?:\\([^)]*\\)\\s*)?${sym}\\b`).test(src.get(f)));
      const usedBy = tests.filter((f) => new RegExp(`\\b${sym}\\b`).test(src.get(f)));
      if (defined.length === 0) {
        problems.push(
          `§${section} 的理由说「由 \`${ref}\` 断言」，但 \`${sym}\` 在 internal/ 与 cmd/ 里**没有定义**`,
        );
      } else if (usedBy.length === 0) {
        problems.push(
          `§${section} 的理由说「由 \`${ref}\` 断言」，但**没有任何 \`_test.go\` 引用 \`${sym}\`**` +
            `（定义在 ${defined.join(", ")}）—— 这一节其实没人验`,
        );
      }
    }
  }
  return { problems, narration };
}

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

/**
 * 只有 **Go 侧（同级仓库 mcloud）** 消费者的向量。
 *
 * 2026-09-21 剥离之后，`test/golden/` 是两个仓库各一份实体副本。这六份是
 * 为"钉住 Go 侧要复刻的形状"补的，Rust 侧从来没人读它们
 * （判据：`grep -rn include_str crates/ | grep <名字>` 全为 0）。
 * 不列出来，它们会在"消费者"一节被印成**死向量** —— 那是假警报，
 * 而假警报的代价是让人开始忽略真警报。
 *
 * ⚠ 这份表是**手维护**的，与 `NOT_VERIFIED_REASONS` 同一类风险：表过期不会自报。
 * 所以命中时反过来查一次 —— 本仓库若真有了消费者，就把"表里有死条目"算进失败。
 *
 * Go 侧的消费者（各自 `golden.Load("<名字>")`）：
 * `internal/io/datetime_test.go` / `internal/core/enum_test.go` /
 * `internal/core/legacy_test.go` / `internal/app/device_test.go` +
 * `internal/upstream/login_crypto_vectors_test.go` / `internal/core/user_test.go` /
 * `internal/core/traits_test.go`。
 */
const GO_SIDE_ONLY = new Set([
  "datetime-format-vectors.json",
  "enum-json-vectors.json",
  "legacy-shape-vectors.json",
  "login-crypto-vectors.json",
  "password-hash-vectors.json",
  "traits-shape-vectors.json",
]);

// ─────────────────────────── 主流程 ───────────────────────────

/**
 * 哪些 golden 文件真的被读了。
 *
 * 两种语言的读取形态都要认，**不能只认 Rust 那一种**：
 *
 *   Rust  `include_str!("…/xxx.json")`
 *   Go    `golden.Load("xxx.json")`（见 internal/golden/golden.go）
 *
 * 只认一边的后果不是报错，而是**把 Go 侧真读了的向量标成"死向量"** ——
 * 于是这条防线的输出从"谁没被读"变成"谁没被 Rust 读"，而 Go 侧那份
 * 消费者从此看不见（本仓库只扫得到 `crates/`，Go 消费者在同级仓库）。
 */
function consumedByTests() {
  const used = new Set();

  // `cargo fmt` 会把超长的 `include_str!("…")` 折成多行（相对路径在
  // `src/<模块>/tests/` 下尤其容易顶到 max_width），所以括号与字面量
  // 之间可能有换行 —— 这里容忍空白，不然会误报"死向量"。
  const RS_RE = /include_str!\(\s*"([^"]+)"\s*\)/g;
  const GO_RE = /golden\.Load\(\s*"([^"]+)"\s*\)/g;

  const walk = (dir) => {
    let ents;
    try {
      ents = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return; // 目录不存在不是错误：仓库可能没有该侧代码
    }
    for (const ent of ents) {
      const p = path.join(dir, ent.name);
      if (ent.isDirectory()) {
        walk(p);
        continue;
      }
      // ⚠ **Go 侧必须限 `_test.go`**：本检查的结论被打印成「✓ 有测试读取」，
      // 而"某个 `.go` 文件里出现过这个字面量"比那句话**弱**：
      // 生产代码里的一个 `golden.Load("x.json")` 会被读成"有测试在读它"。
      // ⇒ 判据要**至少**和它印出来的那句话一样强。
      // （Rust 侧不这么限：那边的向量是 `include_str!` 进 `tests/` 模块或
      //  `#[cfg(test)] mod tests`，文件名不带 `_test` 后缀。）
      const re = ent.name.endsWith(".rs")
        ? RS_RE
        : ent.name.endsWith("_test.go")
          ? GO_RE
          : null;
      if (!re) continue;
      for (const m of fs.readFileSync(p, "utf8").matchAll(re)) {
        const base = path.basename(m[1]);
        if (base.endsWith(".json")) used.add(base);
      }
    }
  };

  // 本仓库的消费者只有一类：Rust 侧 `crates/`。
  // （历史上这里还扫 `internal/` 与 `cmd/` —— Go 侧在 mcloud 仓库时，
  // 两边同仓。2026-09-21 Rust 剥离为独立仓库后，Go 侧的消费者核对归 mcloud。）
  for (const root of ["crates"]) {
    walk(path.join(ROOT, root));
  }
  return used;
}

function main() {
  const files = fs.readdirSync(GOLDEN_DIR).filter((f) => f.endsWith(".json")).sort();
  if (files.length === 0) {
    console.error(`✗ ${GOLDEN_DIR} 里没有 *.json`);
    process.exit(1);
  }

  const used = consumedByTests();
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
        // 对象形的段（`infoV3` / `startSignIn` / `rsaSelfKeyPair`…）。
        // 有 `DICT_VERIFIERS` 的就跑，返回的条目与数组段**同形**，计数与报错复用同一套。
        // ⚠ 没有校验器的**必须**出现在"未覆盖"清单里 —— 早先这里直接 `continue`，
        // 于是它们在报告里**彻底消失**，整份输出读起来像"所有段都验过了"。
        // **没验就得说出来**，这是本脚本存在的理由（见文件头）。
        if (entries && typeof entries === "object") {
          const verifyDict = DICT_VERIFIERS[section];
          if (!verifyDict) {
            noVerifier.set(section, [...(noVerifier.get(section) ?? []), file]);
            continue;
          }
          let rows;
          try {
            rows = verifyDict(entries);
          } catch (err) {
            failures.push(`${file} §${section}\n      算不出来：${err.message}`);
            rows = [];
          }
          let okDict = 0;
          for (const r of rows) {
            const label = `${file} §${section} ${r.name}`.trim();
            if (r.actual === r.expected) {
              okDict++;
              checked++;
              if (!QUIET) fileLines.push(`    ✓ ${label}${r.extra ? `  (${r.extra})` : ""}`);
            } else {
              failures.push(
                `${label}\n      期望 ${r.expected}\n      实得 ${r.actual}` +
                  (r.extra ? `\n      ${r.extra}` : "")
              );
            }
          }
          if (rows.length) sectionReport.push({ file, section, ok: okDict, total: rows.length });
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
      console.log(`\n[${file}]${used.has(file) ? "" : "   ⚠ 尚无测试读取"}`);
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
      // ⚠ 记录"这条豁免**这次真的被用到了**"——表里剩下的、没被记到的就是**死条目**。
      if (NOT_VERIFIED_REASONS[section] !== undefined) usedReasons.add(section);
      console.log(`  · §${section.padEnd(24)} ${why}`);
      console.log(`    ${inFiles.join(", ")}`);
    }
  }

  // ── 豁免表自己的两个洞（与 §6.1 #92/#93 同一类：表过期不会自报）──
  const deadReasons = Object.keys(NOT_VERIFIED_REASONS).filter((k) => !usedReasons.has(k));
  const reasonCheck = checkReasonConsumers();
  console.log("\n── 豁免表自查 ──");
  console.log(
    `  ${deadReasons.length === 0 ? "✓" : "✗"} 条目 ${Object.keys(NOT_VERIFIED_REASONS).length} 条、` +
      `本次命中 ${usedReasons.size} 条（未被命中的 = 段名已不存在）`,
  );
  for (const k of deadReasons) {
    console.log(`      ✗ 死条目 \`${k}\`：没有任何向量段叫这个名字了，请删掉或改对`);
  }
  console.log(
    reasonCheck.skipped
      ? `  · 理由点名的消费者核对：**跳过** —— ${reasonCheck.skipped}`
      : `  ${reasonCheck.problems.length === 0 ? "✓" : "✗"} 理由点名的消费者都能解析` +
        `（${Object.keys(NOT_VERIFIED_REASONS).length - reasonCheck.narration.length} 条有引用，` +
        `${reasonCheck.narration.length} 条纯叙述）`,
  );
  for (const p of reasonCheck.problems) console.log(`      ✗ ${p}`);
  if (reasonCheck.narration.length) {
    console.log(`      · 纯叙述（**没人机械核过**，自己看一眼）：${reasonCheck.narration.join(", ")}`);
  }

  console.log("\n── 脱敏（镜像 Rust 侧守卫）──");
  if (identityProblems.length === 0) {
    console.log(`  ✓ ${files.length} 个文件、字面 + base64 解码后，均只出现 ${SYNTHETIC_PREFIX} 开头的合成标识`);
  } else {
    for (const p of identityProblems) console.log(`  ✗ ${p}`);
  }

  console.log("\n── 消费者 ──");
  let staleGoSide = [];
  for (const f of files) {
    if (used.has(f)) {
      if (GO_SIDE_ONLY.has(f)) staleGoSide.push(f);
      console.log(
        `  ✓ 有测试读取  ${f}` +
          (GO_SIDE_ONLY.has(f)
            ? `   ⚠ 但它也在"只有 Go 侧消费者"表里 ⇒ **表过期**，请删掉这条`
            : ""),
      );
    } else if (GO_SIDE_ONLY.has(f)) {
      console.log(`  · 本仓库无消费者，**Go 侧有**（同级仓库 mcloud）  ${f}`);
    } else {
      console.log(`  ⚠ 无任何测试读取（死向量）  ${f}`);
    }
  }

  for (const f of staleGoSide) {
    console.log(`  ✗ GO_SIDE_ONLY 里有死条目 \`${f}\`：本仓库已经有它的消费者了，请从表里删掉`);
  }
  const tableProblems =
    deadReasons.length + reasonCheck.problems.length + staleGoSide.length;
  const totalProblems = failures.length + identityProblems.length + tableProblems;
  console.log(
    `\n${totalProblems === 0 ? "✓ 通过" : "✗ 失败"}：断言 ${checked} 条全绿` +
      (totalProblems ? `，问题 ${totalProblems} 处` : "") +
      (tableProblems ? `（其中**豁免表自身** ${tableProblems} 处 —— 见上面「豁免表自查」）` : ""),
  );
  if (failures.length) {
    console.log("\n── 失败明细 ──");
    for (const f of failures) console.log(`  ✗ ${f}`);
  }
  process.exit(totalProblems === 0 ? 0 : 1);
}

main();
