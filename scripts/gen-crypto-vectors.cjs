#!/usr/bin/env node
/**
 * 从 TS 原版实现生成「密码学黄金向量」（golden vectors）。
 *
 * 为什么需要它：
 *   Rust 重写最容易悄悄跑偏的地方就是加密。凭肉眼比对代码不可靠，
 *   所以这里**直接调用原版用的同一套库**（crypto-js 4.x + node:crypto），
 *   把输入/输出固化成 JSON，让 Rust 侧断言逐字节一致（见 legacy/crates/mcloud-core/src/crypto.rs）。
 *
 * 用法（必须在 mcloud-sign 仓库里跑，因为需要它的 node_modules 提供 crypto-js）：
 *   NODE_PATH=/Users/catlair/Documents/workspace/mcloud-sign/node_modules \
 *     node scripts/gen-crypto-vectors.cjs
 *
 * 注意：
 *   - 本文件只固化**算法与常量**，不包含任何账号凭据；输入输出均为构造值。
 *   - 随机 IV / 随机数相关的路径无法产出确定值，改为产出「单向可验证」向量：
 *     TS 产出密文 → Rust 必须能解回明文（以及固定 IV 的字节级对拍）。
 */

const fs = require("node:fs");
const path = require("node:path");
const nodeCrypto = require("node:crypto");
const CryptoJS = require("crypto-js");

// ───────────────────────── 原版常量（与 TS 源码逐字对齐） ─────────────────────────

/** core/src/auth/sign.ts: SALT_MAP */
const SIGN_SALT_MAP = { 1: "seedMdYYLIZfbCxg", 2: "sekaMdYYLIZfbCfm" };

/** core/src/utils/encrypt.ts: REFRESH_TOKEN_KEY —— 注意 24 字节，即 AES-192 */
const REFRESH_TOKEN_KEY = "2olBaQGYnEKoStYomsd1n7ax";

/** shared/src/smid/index.ts: SKLAND_SM_CONFIG.publicKey */
const SKLAND_PUBLIC_KEY =
  "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQC8KHAcHbkCn5rxGgGJE+07tY+pt86D/oZ7sA51FaEBv2jgno2TI9zHJVYKJynmiKpixgwUcv93EfWIrU/p/UCs5Vu+odS3I4UBp3R7IZ1A0W01FkumAHYW2PQpMm8ueQKPLUq/idkpG/9b2JDv/qU+Ks36nbUPwlW4CjdfrV+V9QIDAQAB";

/** runtime/src/crypto.ts: aesEncrypt 的固定 IV —— 注意是 **16 个 ASCII 字节**（0x30..0x38），不是 hex 解出来的 8 字节 */
const AES_FIXED_IV = "0102030405060708";

/**
 * 动态 IV 路径的对拍用 IV：必须是**真正的 16 字节**。
 *
 * 踩过的坑：一开始图省事直接复用了 `AES_FIXED_IV` 并走 `enc.Hex.parse()`，
 * 但那个字符串解成 hex 只有 8 字节 —— CryptoJS 不会报错，会拿半截 IV 默默算出一个
 * 无法解回的密文。所以这里显式给一个 16 字节的 hex。
 */
const DYNAMIC_IV_HEX = "000102030405060708090a0b0c0d0e0f";

// ───────────────────────── 原版算法复刻 ─────────────────────────

function toWordArray(data) {
  return CryptoJS.enc.Utf8.parse(data);
}

/** runtime/src/crypto.ts: md5（hex） */
function md5(data) {
  return CryptoJS.MD5(toWordArray(data)).toString(CryptoJS.enc.Hex);
}

/** runtime/src/crypto.ts: sha256（hex） */
function sha256(data) {
  return CryptoJS.SHA256(toWordArray(data)).toString(CryptoJS.enc.Hex);
}

/** shared/src/push/index.ts: HmacSHA256 → base64 */
function hmacSha256B64(message, key) {
  return CryptoJS.HmacSHA256(toWordArray(message), toWordArray(key)).toString(CryptoJS.enc.Base64);
}

/** runtime/src/crypto.ts: aesEncrypt —— AES-CBC/PKCS7，固定 IV，密钥截断/补零到 16，输出 hex */
function aesEncryptHex(message, key) {
  const keyBuf = CryptoJS.enc.Utf8.parse(key.substring(0, 16).padEnd(16, "\0"));
  const iv = CryptoJS.enc.Utf8.parse(AES_FIXED_IV);
  const encrypted = CryptoJS.AES.encrypt(message, keyBuf, {
    iv,
    mode: CryptoJS.mode.CBC,
    padding: CryptoJS.pad.Pkcs7,
  });
  return encrypted.ciphertext.toString(CryptoJS.enc.Hex);
}

/** shared/src/smid/index.ts: encryptDes —— DES-ECB/NoPadding，手工补零，输出 base64 */
function desEncryptB64(message, key) {
  const keyBytes = key.substring(0, 8).padEnd(8, "\0");
  let messageStr = String(message);
  const padLen = 8 - (messageStr.length % 8);
  if (padLen !== 8) messageStr += "\0".repeat(padLen);

  const keyWord = CryptoJS.enc.Utf8.parse(keyBytes);
  const messageWord = CryptoJS.enc.Utf8.parse(messageStr);
  const encrypted = CryptoJS.DES.encrypt(messageWord, keyWord, {
    mode: CryptoJS.mode.ECB,
    padding: CryptoJS.pad.NoPadding,
  });
  return encrypted.ciphertext.toString(CryptoJS.enc.Base64);
}

/** runtime/src/crypto.ts: desDecrypt —— DES-ECB/PKCS7，base64 进、utf8 出 */
function desDecrypt(b64, key) {
  const keyWord = CryptoJS.enc.Utf8.parse(key);
  const cipherWord = CryptoJS.enc.Base64.parse(b64);
  const decrypted = CryptoJS.DES.decrypt({ ciphertext: cipherWord }, keyWord, {
    mode: CryptoJS.mode.ECB,
    padding: CryptoJS.pad.Pkcs7,
  });
  return decrypted.toString(CryptoJS.enc.Utf8);
}

/** core/src/utils/encrypt.ts: aesEncryptWithDynamicIV —— 可变密钥长度，Base64(IV||CT) */
function aesDynamicIvEncrypt(data, key, ivHex) {
  const keyWord = CryptoJS.enc.Utf8.parse(key);
  const iv = ivHex ? CryptoJS.enc.Hex.parse(ivHex) : CryptoJS.lib.WordArray.random(16);
  if (iv.sigBytes !== 16) {
    throw new Error(`AES-CBC 的 IV 必须 16 字节，实际 ${iv.sigBytes}（ivHex=${ivHex}）`);
  }

  const encrypted = CryptoJS.AES.encrypt(data, keyWord, {
    iv,
    mode: CryptoJS.mode.CBC,
    padding: CryptoJS.pad.Pkcs7,
  });
  return {
    ivHex: iv.toString(CryptoJS.enc.Hex),
    cipherB64: iv.clone().concat(encrypted.ciphertext).toString(CryptoJS.enc.Base64),
  };
}

/** runtime/src/node.ts: rsaEncrypt —— PKCS#1 v1.5，输出 base64（确定性填充，可字节级对拍） */
function rsaEncryptB64(message, publicKeyBase64) {
  const formattedKey = `-----BEGIN PUBLIC KEY-----\n${publicKeyBase64
    .match(/.{1,64}/g)
    .join("\n")}\n-----END PUBLIC KEY-----`;
  return nodeCrypto
    .publicEncrypt(
      { key: formattedKey, padding: nodeCrypto.constants.RSA_PKCS1_PADDING },
      Buffer.from(message),
    )
    .toString("base64");
}

/** shared/src/smid/index.ts: getTn —— 排序后按规则拼接 */
function getTn(obj) {
  const sortedKeys = Object.keys(obj).sort();
  const result = [];
  for (const key of sortedKeys) {
    let v = obj[key];
    if (typeof v === "number" && !Number.isInteger(v)) {
      v = Math.floor(v * 10000).toString();
    } else if (typeof v === "number") {
      v = (v * 10000).toString();
    } else if (typeof v === "object" && v !== null) {
      v = getTn(v);
    } else {
      v = String(v);
    }
    result.push(v);
  }
  return result.join("");
}

/** core/src/auth/sign.ts: generateSignature */
function generateSignature(id, timestamp, nonce, saltKey, other = "") {
  const salt = SIGN_SALT_MAP[saltKey] || SIGN_SALT_MAP[1];
  return md5(`${salt}${id}${timestamp}${nonce}${other}${salt}`);
}

/** core/src/apis/ai-workbench.ts 侧同款：gzip 后把 OS 字节改成 19，再 base64 */
async function gzipB64WithOs19(obj) {
  const jsonStr = JSON.stringify(obj);
  const cs = new CompressionStream("gzip");
  const writer = cs.writable.getWriter();
  writer.write(new TextEncoder().encode(jsonStr));
  writer.close();
  const reader = cs.readable.getReader();
  const chunks = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.length;
  }
  const osByteBefore = out[9];
  if (out.length > 9) out[9] = 19;
  return {
    json: jsonStr,
    base64: Buffer.from(out).toString("base64"),
    osByteBefore,
    osByteAfter: out[9],
    plainLen: jsonStr.length,
  };
}

// ─────────────────── smid（shared/src/smid/index.ts）的常量与复刻 ───────────────────

/** `SKLAND_SM_CONFIG.organization` */
const SM_ORGANIZATION = "FXlyfmWg2AzwbrxDKSv5";
/** `SKLAND_SM_CONFIG.appId` */
const SM_APP_ID = "default";
/** `getSmData` 里写死的 plugins 串（不是随机池） */
const SM_PLUGINS =
  "ChromePDFViewerPortableDocumentFormatinternal-pdf-viewer2,ChromiumPDFViewerPortableDocumentFormatinternal-pdf-viewer2,MicrosoftEdgePDFViewerPortableDocumentFormatinternal-pdf-viewer2,PDFViewerPortableDocumentFormatinternal-pdf-viewer2,WebKitbuilt-inPDFPortableDocumentFormatinternal-pdf-viewer2";
/** `desTarget.url` */
const SM_PAGE_URL =
  "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin&sourceid=1002&token=";
/** `shared/src/constants.ts: CAIYUN_BASE_UA` */
const SM_UA =
  "Mozilla/5.0 (Linux; Android 14; 22041216C Build/TP1A.220624.014; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/128.0.6613.88 Mobile Safari/537.36";

/**
 * `DES_RULE` 逐字照搬：`key` = DES 密钥（`null` 表示旧代码的 `is_encrypt: 0`，
 * 即**改名但不加密**），`name` = `obfuscated_name`。
 *
 * ⚠ 这份表在向量里的用途是**产出期望值**，所以它必须"照搬"而不是"重新推导" ——
 * 真正的独立交叉验证是 Rust 侧那份 [`RULES`](legacy/crates/mcloud-upstream/src/smid.rs)，
 * 两边写歪同一条就会对不上（Node 用 crypto-js，Rust 用自带 DES 实现）。
 */
const SM_DES_RULE = {
  appId: { key: "a9kbown8", name: "um" },
  box: { key: null, name: "aw" },
  canvas: { key: "ultuhs59", name: "iu" },
  clientSize: { key: "7s9eixjh", name: "na" },
  organization: { key: "14e30pp1", name: "nl" },
  os: { key: "j862vr1f", name: "lz" },
  platform: { key: "pgutfbo7", name: "lm" },
  plugins: { key: "x98c92bm", name: "hy" },
  pmf: { key: "gwvvsmr4", name: "yu" },
  protocol: { key: null, name: "protocol" },
  referer: { key: "gka1ljjt", name: "mf" },
  res: { key: "ht2n6dwf", name: "ut" },
  rtype: { key: "k2p9ifij", name: "fm" },
  sdkver: { key: "q4ij0mv5", name: "ir" },
  status: { key: "moyspi7i", name: "kp" },
  subVersion: { key: "egsyl21v", name: "dk" },
  svm: { key: "yf780e9w", name: "ee" },
  time: { key: "btb17oag", name: "jt" },
  timezone: { key: "l94h8clc", name: "bl" },
  tn: { key: "crnw0x7e", name: "as" },
  trees: { key: "x6the9nq", name: "ij" },
  ua: { key: "bewicpxu", name: "pu" },
  url: { key: "llj8yd1x", name: "sb" },
  version: { key: null, name: "version" },
  vpw: { key: "f1zlae4b", name: "qb" },
};

/** `getSmId()` 的拼接部分（时间与 uid 由入参给，才能产出确定值）。 */
function smidStringFrom(timeStr, uid) {
  const v = `${timeStr}${md5(uid)}00`;
  const smskWeb = md5(`smsk_web_${v}`).substring(0, 14);
  return `${v}${smskWeb}0`;
}

/** `getSmData()` 开头：`priId = md5Hash(uid).substring(0, 16)`，AES 的密钥。 */
function smPrivateId(uid) {
  return md5(uid).substring(0, 16);
}

/** `encryptObjectByDesRules()`：命中规则就改名（并按需加密），没命中原名原值带出。 */
function encryptObjectByDesRules(obj) {
  const result = {};
  for (const [key, value] of Object.entries(obj)) {
    const rule = SM_DES_RULE[key];
    if (rule) {
      result[rule.name] = rule.key ? desEncryptB64(String(value), rule.key) : value;
    } else {
      result[key] = value;
    }
  }
  return result;
}

/**
 * 一份**完整**的上报特征体（字段与顺序都按旧 `getSmData` 的 `desTarget`）。
 *
 * 除 `box` / `smid` 外全部固定，所以 `tn` 与改名结果都可字节级对拍。
 * `tn` 留空串是刻意的：旧代码就是"先算摘要、再写回 `tn`"，顺序反了整份指纹作废。
 */
function smTarget(overrides) {
  return {
    protocol: 4,
    organization: SM_ORGANIZATION,
    appId: SM_APP_ID,
    os: "web",
    version: "3.0.0",
    sdkver: "3.0.0",
    box: "",
    rtype: "all",
    // 真接一个算出来的 smid（长度 63），别放一串零 —— 夹具要像真流量
    smid: smidStringFrom("20260919153000", "6b4a3c2d-1111-4222-8333-444444444444"),
    subVersion: "1.0.0",
    time: 7,
    platform: "Linux armv8l",
    clientSize: "0_0_0_0_375_667_375_667",
    res: "375_667_2_2",
    status: "0010",
    timezone: -480,
    vpw: "vpw-00000000-0000-4000-8000-000000000001",
    svm: 1789743328974,
    trees: "trees-00000000-0000-4000-8000-000000000001",
    pmf: 1789743328974,
    cdp: 0,
    maxTouchPoints: 5,
    connectionRtt: 50,
    cpucount: 8,
    battery: { charging: 0, level: 0.5 },
    canvas: "1a2b3c4d",
    plugins: SM_PLUGINS,
    referer: "",
    ua: SM_UA,
    url: SM_PAGE_URL,
    tn: "",
    ...overrides,
  };
}

/** 旧 `getTn(desTarget)` 之后紧跟的那一步：`tn = md5Hash(getTn(desTarget))`。 */
function smTn(target) {
  return md5(getTn(target));
}

// ───────────────────────── 向量集 ─────────────────────────

const ASCII_CASES = [
  { name: "empty", input: "" },
  { name: "ascii-short", input: "mcloud" },
  { name: "ascii-15", input: "0123456789abcde" },
  { name: "ascii-16", input: "0123456789abcdef" },
  { name: "ascii-17", input: "0123456789abcdefg" },
  { name: "salt-key-1", input: SIGN_SALT_MAP[1] },
  { name: "salt-key-2", input: SIGN_SALT_MAP[2] },
  { name: "cjk", input: "移动云盘签到" },
  { name: "mixed", input: "sign=ok&cloud=100★" },
];

const AES_KEY_CASES = [
  { name: "key-16-exact", key: "0123456789abcdef" },
  { name: "key-short-zero-padded", key: "short" },
  { name: "key-long-truncated", key: "0123456789abcdefEXTRA-IS-IGNORED" },
  { name: "key-refresh-token-24-truncated-to-16", key: REFRESH_TOKEN_KEY },
];

const DES_KEY_CASES = [
  { name: "key-8-exact", key: "a9kbown8" },
  { name: "key-short-zero-padded", key: "abc" },
  { name: "key-long-truncated", key: "x98c92bmTAIL" },
];

const DES_MSGS = [
  { name: "len-1", message: "7" },
  { name: "len-7", message: "web-1.0" },
  { name: "len-8-already-aligned", message: "Linuxarm" },
  { name: "len-9", message: "Linuxarmv" },
  { name: "len-16", message: "0_0_0_0_375_667_" },
  { name: "empty", message: "" },
];

async function main() {
  const out = {
    $generator: "scripts/gen-crypto-vectors.cjs",
    $source: "mcloud-sign TS 原版：crypto-js 4.x + node:crypto + CompressionStream",
    $cryptoJsVersion: require("crypto-js/package.json").version,
    $note:
      "Rust 侧（legacy/crates/mcloud-core/src/crypto.rs）必须复现本文件全部 expect* 字段。" +
      "随机性路径（动态 IV、RSA PKCS#1 v1.5 填充）无法字节级对拍，改为单向/自持密钥验证，见对应条目的 note。" +
      "补（2026-09-19）：smid* 四段由 legacy/crates/mcloud-upstream/src/smid.rs 的测试消费（它们的落点在那里，不在 core）。",
    constants: {
      SIGN_SALT_MAP,
      REFRESH_TOKEN_KEY,
      AES_FIXED_IV,
      SKLAND_PUBLIC_KEY,
      SM_ORGANIZATION,
      SM_APP_ID,
      SM_PLUGINS,
      SM_PAGE_URL,
      SM_UA,
    },
    md5: [],
    sha256: [],
    hmacSha256B64: [],
    aes128CbcFixedIvHex: [],
    desEcbNoPaddingB64: [],
    desEcbPkcs7Decrypt: [],
    aesDynamicIv: [],
    rsaPkcs1v15B64: [],
    signHeader: [],
    getTn: [],
    gzip: [],
    smidString: [],
    smidPriId: [],
    smidTn: [],
    smidDesRules: [],
  };

  for (const c of ASCII_CASES) {
    out.md5.push({ ...c, expect: md5(c.input) });
    out.sha256.push({ ...c, expect: sha256(c.input) });
  }

  out.hmacSha256B64.push({
    name: "dingtalk-sign-input",
    message: "1700000000000\nSEC0123456789abcdef",
    key: "SEC0123456789abcdef",
    expect: hmacSha256B64("1700000000000\nSEC0123456789abcdef", "SEC0123456789abcdef"),
  });
  out.hmacSha256B64.push({
    name: "empty-key",
    message: "mcloud",
    key: "",
    expect: hmacSha256B64("mcloud", ""),
  });
  out.hmacSha256B64.push({
    name: "cjk-message",
    message: "移动云盘",
    key: "k",
    expect: hmacSha256B64("移动云盘", "k"),
  });

  for (const k of AES_KEY_CASES) {
    for (const p of [
      { name: "plain-len-15", message: "0123456789abcde" },
      { name: "plain-len-16", message: "0123456789abcdef" },
      { name: "plain-len-17", message: "0123456789abcdefg" },
      { name: "plain-empty", message: "" },
      { name: "plain-cjk", message: "签到成功" },
    ]) {
      out.aes128CbcFixedIvHex.push({
        name: `${k.name}__${p.name}`,
        key: k.key,
        message: p.message,
        expect: aesEncryptHex(p.message, k.key),
      });
    }
  }

  for (const k of DES_KEY_CASES) {
    for (const m of DES_MSGS) {
      let expect;
      let error = null;
      try {
        expect = desEncryptB64(m.message, k.key);
      } catch (e) {
        error = String(e && e.message ? e.message : e);
      }
      out.desEcbNoPaddingB64.push({
        name: `${k.name}__${m.name}`,
        key: k.key,
        message: m.message,
        expect,
        ...(error ? { error } : {}),
      });
    }
  }

  // DES-ECB/PKCS7 解密方向：先拿 Pkcs7 加密产出密文，再验证能解回原文
  for (const c of [
    { name: "pkcs7-ascii", plain: "mcloud-sign" },
    { name: "pkcs7-exact-8", plain: "mcloudsg" },
    { name: "pkcs7-cjk", plain: "移动云盘" },
  ]) {
    const key = "lx1d3v7s";
    const keyWord = CryptoJS.enc.Utf8.parse(key);
    const enc = CryptoJS.DES.encrypt(c.plain, keyWord, {
      mode: CryptoJS.mode.ECB,
      padding: CryptoJS.pad.Pkcs7,
    });
    const b64 = enc.ciphertext.toString(CryptoJS.enc.Base64);
    out.desEcbPkcs7Decrypt.push({
      name: c.name,
      key,
      cipherB64: b64,
      expect: desDecrypt(b64, key),
      expectPlain: c.plain,
    });
  }

  // 动态 IV（AES-192，因为 key 是 24 字节）
  const dynPlain = {
    refreshToken: "FAKE_REFRESH_TOKEN_FOR_VECTOR",
    userDomainId: "FAKE_USER_DOMAIN_ID",
    at: 1700000000,
  };
  const dynJson = JSON.stringify(dynPlain);
  const fixed = aesDynamicIvEncrypt(dynJson, REFRESH_TOKEN_KEY, DYNAMIC_IV_HEX);
  if (fixed.ivHex.length !== 32) {
    throw new Error(`动态 IV 必须是 16 字节（32 个 hex 字符），实际 ${fixed.ivHex.length}`);
  }
  out.aesDynamicIv.push({
    name: "fixed-iv-byte-exact",
    key: REFRESH_TOKEN_KEY,
    keyBytes: Buffer.from(REFRESH_TOKEN_KEY, "utf8").length,
    ivHex: fixed.ivHex,
    plaintext: dynJson,
    expectB64: fixed.cipherB64,
    byteExact: true,
  });
  const random1 = aesDynamicIvEncrypt(dynJson, REFRESH_TOKEN_KEY);
  out.aesDynamicIv.push({
    name: "random-iv-directional",
    key: REFRESH_TOKEN_KEY,
    keyBytes: Buffer.from(REFRESH_TOKEN_KEY, "utf8").length,
    ivHex: random1.ivHex,
    plaintext: dynJson,
    expectB64: random1.cipherB64,
    byteExact: false,
    note: "IV 随机，无法字节级对拍；Rust 必须能用同一 key 解出 plaintext",
  });

  // ⚠ RSA PKCS#1 v1.5 **加密**填充含随机非零字节 → 同一明文多次加密结果不同（已实测），
  //   因此**无法字节级对拍**。改为两条可验证路径：
  //     1) 自持密钥对：Node 用自己生成的公钥加密 → Rust 必须能用对应私钥解回明文（跨实现互通）
  //     2) skland 真实公钥：只做结构断言（输出长度 = 模长、密文 < n）
  const selfPair = nodeCrypto.generateKeyPairSync("rsa", { modulusLength: 1024 });
  const selfPubB64 = selfPair.publicKey.export({ type: "spki", format: "der" }).toString("base64");
  const selfPrivB64 = selfPair.privateKey.export({ type: "pkcs8", format: "der" }).toString("base64");
  const selfJwk = selfPair.publicKey.export({ format: "jwk" });
  out.rsaSelfKeyPair = {
    note: "仅用于验证 Rust RSA 实现正确性；这是测试密钥，可公开。",
    publicKeyB64: selfPubB64,
    privateKeyPkcs8B64: selfPrivB64,
    modulusB64Url: selfJwk.n,
    exponentB64Url: selfJwk.e,
    modulusHex: Buffer.from(selfJwk.n, "base64url").toString("hex"),
    exponentHex: BigInt("0x" + Buffer.from(selfJwk.e, "base64url").toString("hex")).toString(16),
    cases: [],
  };
  for (const m of [
    { name: "uuid-v4-payload", message: "8f14e45f-ceea-467a-9c1b-000000000001" },
    { name: "single-char", message: "v" },
    { name: "max-117-bytes", message: "A".repeat(117) },
    { name: "cjk", message: "移动云盘" },
  ]) {
    out.rsaSelfKeyPair.cases.push({
      name: m.name,
      message: m.message,
      cipherB64: rsaEncryptB64(m.message, selfPubB64),
      // Rust 侧断言：用 privateKeyPkcs8B64 解出 message；再用 publicKeyB64 加密后自解也应等于 message
    });
  }

  out.rsaPkcs1v15B64.push({
    name: "skland-structure-only",
    publicKeyB64: SKLAND_PUBLIC_KEY,
    modulusBytes: 128,
    byteExact: false,
    note: "PKCS#1 v1.5 加密含随机填充，不做字节级对拍；只断言输出 128 字节且密文整数值 < n",
    samples: [
      rsaEncryptB64("8f14e45f-ceea-467a-9c1b-000000000001", SKLAND_PUBLIC_KEY),
      rsaEncryptB64("v", SKLAND_PUBLIC_KEY),
    ],
  });

  out.signHeader.push({
    name: "salt-1",
    id: "11111111-2222-4333-8444-555555555555",
    timestamp: 1700000000000,
    nonce: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    saltKey: 1,
    other: "",
    expectMd5: generateSignature(
      "11111111-2222-4333-8444-555555555555",
      1700000000000,
      "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      1,
      "",
    ),
  });
  out.signHeader.push({
    name: "salt-2-with-other",
    id: "11111111-2222-4333-8444-555555555555",
    timestamp: "1700000000000",
    nonce: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
    saltKey: 2,
    other: "/api/signin",
    expectMd5: generateSignature(
      "11111111-2222-4333-8444-555555555555",
      "1700000000000",
      "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      2,
      "/api/signin",
    ),
  });
  out.signHeader.push({
    name: "unknown-salt-falls-back-to-1",
    id: "x",
    timestamp: 1,
    nonce: "y",
    saltKey: 99,
    other: "",
    expectMd5: generateSignature("x", 1, "y", 99, ""),
  });

  out.getTn.push({
    name: "sorted-and-scaled",
    input: { protocol: 4, organization: "FXly", version: "3.0.0", timezone: -480, time: 7 },
    expect: getTn({ protocol: 4, organization: "FXly", version: "3.0.0", timezone: -480, time: 7 }),
  });
  out.getTn.push({
    name: "float-truncated-to-4-decimals",
    input: { a: 1.234567, b: 2, c: "x", d: null },
    expect: getTn({ a: 1.234567, b: 2, c: "x", d: null }),
  });
  out.getTn.push({
    name: "nested-object",
    input: { outer: "o", inner: { z: 1, a: "q" } },
    expect: getTn({ outer: "o", inner: { z: 1, a: "q" } }),
  });
  out.getTn.push({
    name: "undefined-dropped-by-object-keys--skip",
    input: { defined: "yes" },
    expect: getTn({ defined: "yes" }),
  });

  const gz = await gzipB64WithOs19({ protocol: 4, appId: "default", os: "web" });
  out.gzip.push({
    name: "sm-des-payload",
    ...gz,
    byteExact: false,
    note: "deflate 输出因实现而异，不做字节级对拍；要求：能 gunzip 回原 JSON 且 OS 字节 == 19",
  });

  // —— smid：拼接 / AES 密钥 / tn 摘要 / DES 改名表 ——
  for (const c of [
    {
      name: "uid-A",
      timeDigits: "20260919153000",
      uid: "8f14e45f-ceea-467a-9c1b-000000000001",
    },
    {
      name: "uid-B-midnight",
      timeDigits: "20270101000000",
      uid: "00000000-0000-4000-8000-000000000000",
    },
    {
      name: "uid-C-all-f",
      timeDigits: "20261231235959",
      uid: "ffffffff-ffff-4fff-8fff-ffffffffffff",
    },
  ]) {
    out.smidString.push({ ...c, expect: smidStringFrom(c.timeDigits, c.uid), expectLen: 63 });
    out.smidPriId.push({ name: c.name, uid: c.uid, expect: smPrivateId(c.uid), expectLen: 16 });
  }

  // 完整特征体的 tn：这条锁的是**顺序**（算摘要时 `tn` 必须还是空串）
  const smFirst = smTarget({});
  const smFirstTn = smTn(smFirst);
  out.smidTn.push({
    name: "full-web-fingerprint-first-upload",
    target: smFirst,
    expect: smFirstTn,
    note: "target.tn 为**空串**时求摘要，与旧代码 desTarget.tn = md5Hash(getTn(desTarget)) 同一步",
  });
  // 第二次上报：带上旧 deviceId（box）与另一个 smid，其余特征一字不动 ——
  // 这就是"绑定账号"的形态：只有 volatile 的变
  const smSecond = smTarget({
    box: "B1990000000000000000000000000000000000000000000001",
    smid: smidStringFrom("20260920091500", "9a2b3c4d-1111-4222-8333-444444444444"),
    svm: 1789829700000,
    pmf: 1789829700000,
  });
  const smSecondTn = smTn(smSecond);
  out.smidTn.push({ name: "second-upload-carries-old-device-id", target: smSecond, expect: smSecondTn });

  for (const [name, target, tn] of [
    ["first-upload", smFirst, smFirstTn],
    ["second-upload", smSecond, smSecondTn],
  ]) {
    const withTn = { ...target, tn };
    out.smidDesRules.push({
      name,
      target: withTn,
      expect: encryptObjectByDesRules(withTn),
      note:
        "整份改名表的字节级对拍：命中规则的键必须换成 obfuscated_name 并按那把 DES 密钥加密，" +
        "没命中的（smid/cdp/battery/maxTouchPoints/connectionRtt/cpucount）原名原值",
    });
  }

  const outDir = path.resolve(__dirname, "..", "test", "golden");
  fs.mkdirSync(outDir, { recursive: true });
  const outFile = path.join(outDir, "crypto-vectors.json");
  fs.writeFileSync(outFile, JSON.stringify(out, null, 2) + "\n");

  const counts = Object.entries(out)
    .filter(([k]) => !k.startsWith("$") && k !== "constants")
    .map(([k, v]) => (Array.isArray(v) ? `${k}=${v.length}` : `${k}=${v.cases ? v.cases.length : "n/a"}`))
    .join("  ");
  console.log(`wrote ${outFile}`);
  console.log(`crypto-js ${out.$cryptoJsVersion}`);
  console.log(counts);
  const desErrors = out.desEcbNoPaddingB64.filter((v) => v.error);
  if (desErrors.length) {
    console.log(`⚠ desEcbNoPadding 有 ${desErrors.length} 个用例在 TS 侧就报错（已记录，Rust 侧同样需处理）：`);
    for (const e of desErrors) console.log(`   ${e.name}: ${e.error}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
