#!/usr/bin/env node
/**
 * 生成「移动云盘 PC 端登录密码学黄金向量」（golden vectors）。
 *
 * 为什么需要它：
 *   `docs/capture/apiServer.js` 只能看到调用点（`Utils.decryptByAes(result.data)`、
 *   `Utils.encryptByAesCBC(params.imageCode)`），实现与密钥在 `/out/renderer/common/utils.js`。
 *   该文件不在 `docs/capture/` 里（见 VENDOR_SOURCE_PATHS 豁免说明），所以这里把
 *   **实现契约**固化成可断言的数据，代替把客户端源码整份拖进仓库。
 *
 * 为什么可信：
 *   下面的 4 条 `puzzleAnswer` 向量来自真实抓包的 `puzzleVerfycode` 字段
 *   （见 docs/capture/logout-sms-requests.txt）。本脚本用**与原版同版本的 crypto-js 4.x**
 *   复算，必须逐字节命中 `expectB64`；命中即证明密钥/IV/模式/padding/分段全部正确。
 *
 * 用法（需要一个能 require 到 crypto-js 4.x 的 node_modules）：
 *   NODE_PATH=<somewhere>/node_modules node scripts/gen-login-crypto-vectors.cjs
 *
 * 注意：
 *   - 本文件只固化算法与常量，不含任何账号凭据。
 *   - 常量 `CREATPUZZLE03F9A` / `E526E1F3F9765444` 是客户端硬编码的**算法常量**
 *     （源码注释误写成 "替换为实际的..."/"挑战ID"，实际是固定 IV，非运行时值）。
 */

const fs = require("node:fs");
const path = require("node:path");
const CryptoJS = require("crypto-js");

// ───────────────────────── 原版常量（与 utils.js 逐字对齐） ─────────────────────────

/** utils.js: decryptByAes / encryptByAes —— 生产环境密钥，AES-128-ECB/PKCS7 */
const AES_ECB_KEY_PROD = "qPqDw263XgFgL3u8";

/** utils.js: decryptByAes —— 测试环境密钥（$window$_systemEnvironment ∈ {11, 22}），AES-128-ECB/PKCS7 */
const AES_ECB_KEY_TEST = "2ErfJus1Ofr@2o24";

/** utils.js: encryptByAesCBC —— 固定密钥，AES-128-CBC/PKCS7 */
const PUZZLE_KEY = "CREATPUZZLE03F9A";

/** utils.js: encryptByAesCBC —— **固定 IV**（不是挑战 ID，别被注释误导），16 个 ASCII 字节 */
const PUZZLE_IV = "E526E1F3F9765444";

/** utils.js: encryptByHash —— 账号密码登录 SHA1 前缀 */
const HASHTEXT = "fetion.com.cn:";

/** 环境变量取值：0 = 生产；11 / 22 = 测试 */
const ENV_PROD = 0;
const ENV_TEST = [11, 22];

// ───────────────────────── 原版算法复刻（逐行对应 utils.js） ─────────────────────────

/**
 * utils.js: decryptByAes(input, key)
 *
 * ⚠ 关键差异：入参 input 是 **hex 字符串**（`CryptoJS.enc.Hex.parse(input)`），
 *   不是 base64；输出 UTF-8 明文。
 */
function decryptByAes(inputHex, key) {
  const k = CryptoJS.enc.Utf8.parse(key);
  const ct = CryptoJS.enc.Hex.parse(inputHex);
  return CryptoJS.AES.decrypt({ ciphertext: ct }, k, {
    mode: CryptoJS.mode.ECB,
    padding: CryptoJS.pad.Pkcs7,
  }).toString(CryptoJS.enc.Utf8);
}

/** utils.js: encryptByAes(input, key) —— 输出 base64（CryptoJS 默认 OpenSSL 序列化） */
function encryptByAes(input, key) {
  const k = CryptoJS.enc.Utf8.parse(key);
  return CryptoJS.AES.encrypt(input, k, {
    mode: CryptoJS.mode.ECB,
    padding: CryptoJS.pad.Pkcs7,
  }).toString();
}

/** utils.js: encryptByAesCBC(plainText) —— 输出 Base64(IV || ciphertext)，IV 固定 */
function encryptByAesCBC(plainText) {
  const keyWordArray = CryptoJS.enc.Utf8.parse(PUZZLE_KEY);
  const ivWordArray = CryptoJS.enc.Utf8.parse(PUZZLE_IV);
  const encrypted = CryptoJS.AES.encrypt(plainText, keyWordArray, {
    iv: ivWordArray,
    mode: CryptoJS.mode.CBC,
    padding: CryptoJS.pad.Pkcs7,
  });
  return ivWordArray.concat(encrypted.ciphertext).toString(CryptoJS.enc.Base64);
}

/**
 * puzzleVerfycode 的解包：Base64 → [16B IV][ciphertext]
 *
 * 因 IV 固定，前 16 字节恒为 ASCII "E526E1F3F9765444"。
 * 这也解释了为什么两次滑块的 Base64 前缀完全一样 —— 不是"同一个挑战"，
 * 而是 IV 相同；挑战 ID 由 `POST /user/verfycode` 的响应单独给出。
 */
function splitPuzzlePayload(b64) {
  const raw = Buffer.from(b64, "base64");
  return { ivAscii: raw.subarray(0, 16).toString("latin1"), cipherB64: raw.subarray(16).toString("base64"), totalBytes: raw.length };
}

/** utils.js: encryptByHash(password) —— SHA1(HASHTEXT + password)，hex 大写 */
function encryptByHash(password) {
  return CryptoJS.SHA1(HASHTEXT + password).toString(CryptoJS.enc.Hex).toUpperCase();
}

/** utils.js: safeVerifyCodeEncrypt(secinfo) —— MD5，hex 大写 */
function safeVerifyCodeEncrypt(secinfo) {
  return CryptoJS.MD5(secinfo).toString().toUpperCase();
}

/** utils.js: getGUID32() —— MD5(UTF8(uuid4))，hex 大写（32 位） */
function getGUID32(uuidV4) {
  return CryptoJS.MD5(CryptoJS.enc.Utf8.parse(uuidV4)).toString(CryptoJS.enc.Hex).toUpperCase();
}

/** utils.js: 设备指纹 = getGUID32() + "-ENDIN" */
const DEVICE_FINGERPRINT_SUFFIX = "-ENDIN";

// ───────────────────────── 向量集 ─────────────────────────

function main() {
  const out = {
    $generator: "scripts/gen-login-crypto-vectors.cjs",
    $source: "移动云盘 PC 端 /out/renderer/common/utils.js（asar 提取，未入库）",
    $cryptoJsVersion: require("crypto-js/package.json").version,
    $note:
      "Rust 侧必须复现本文件全部 expect* 字段。puzzleAnswer 四条来自真实抓包，" +
      "用同版本 crypto-js 复算逐字节命中，是密钥/IV/模式/padding/分段正确性的硬证据。",
    constants: {
      AES_ECB_KEY_PROD,
      AES_ECB_KEY_TEST,
      PUZZLE_KEY,
      PUZZLE_IV,
      HASHTEXT,
      ENV_PROD,
      ENV_TEST,
      DEVICE_FINGERPRINT_SUFFIX,
    },
    puzzleAnswer: [],
    aesEcbHexDecrypt: [],
    aesEcbEncrypt: [],
    sha1Password: [],
    md5SafeVerify: [],
    guid32: [],
  };

  // ① 滑块答案：真实抓包密文 → 必须复算出同一串
  const PUZZLE_REAL = [
    { name: "real-1", plain: "461", expectB64: "RTUyNkUxRjNGOTc2NTQ0NPXSgBfdneawSGgfCa/g/Rg=" },
    { name: "real-2", plain: "391", expectB64: "RTUyNkUxRjNGOTc2NTQ0NEA6F1MQJO7Ejcwfa1IsHCs=" },
  ];
  for (const s of PUZZLE_REAL) {
    const got = encryptByAesCBC(s.plain);
    const seg = splitPuzzlePayload(got);
    out.puzzleAnswer.push({
      name: s.name,
      plainText: s.plain,
      expectB64: got,
      soMatchesRealCapture: got === s.expectB64,
      realCaptureB64: s.expectB64,
      ivAscii: seg.ivAscii,
      ivIsConstant: seg.ivAscii === PUZZLE_IV,
      cipherB64: seg.cipherB64,
      totalBytes: seg.totalBytes,
    });
  }
  // 构造值：覆盖 padding 边界与坐标量级
  for (const p of [
    { name: "coord-0", plain: "0" },
    { name: "coord-3-digit", plain: "999" },
    { name: "coord-4-digit", plain: "1234" },
    { name: "coord-boundary-15", plain: "A".repeat(15) },
    { name: "coord-boundary-16", plain: "A".repeat(16) },
    { name: "coord-boundary-17", plain: "A".repeat(17) },
  ]) {
    const got = encryptByAesCBC(p.plain);
    out.puzzleAnswer.push({
      name: p.name,
      plainText: p.plain,
      expectB64: got,
      synthetic: true,
      cipherB64: splitPuzzlePayload(got).cipherB64,
      totalBytes: splitPuzzlePayload(got).totalBytes,
    });
  }

  // ② 登录成功 data：ECB + hex 入参
  for (const key of [
    { name: "prod", key: AES_ECB_KEY_PROD },
    { name: "test", key: AES_ECB_KEY_TEST },
  ]) {
    for (const p of [
      { name: "len-4", plain: "461" },
      { name: "len-15", plain: "0123456789abcde" },
      { name: "len-16", plain: "0123456789abcdef" },
      { name: "len-17", plain: "0123456789abcdefg" },
      { name: "empty", plain: "" },
      { name: "json-min", plain: '{"a":1}' },
    ]) {
      const b64 = encryptByAes(p.plain, key.key);
      const hex = Buffer.from(b64, "base64").toString("hex");
      out.aesEcbHexDecrypt.push({
        name: `${key.name}__${p.name}`,
        key: key.key,
        inputHex: hex,
        expectPlain: p.plain,
        expectDecrypt: decryptByAes(hex, key.key),
      });
      out.aesEcbEncrypt.push({
        name: `${key.name}__${p.name}`,
        key: key.key,
        plainText: p.plain,
        expectB64: b64,
      });
    }
  }

  // ③ 账号密码登录密码哈希
  for (const p of ["Passw0rd!", "123456", "p@ss 中文"]) {
    out.sha1Password.push({ password: p, expectHexUpper: encryptByHash(p) });
  }

  // ④ 安全验证码
  for (const s of ["sec-info-sample", "1234"]) {
    out.md5SafeVerify.push({ input: s, expectHexUpper: safeVerifyCodeEncrypt(s) });
  }

  // ⑤ 设备指纹 / 会话 ID
  for (const u of [
    "11111111-2222-4333-8444-555555555555",
    "8f14e45f-ceea-467a-9c1b-000000000001",
  ]) {
    const g = getGUID32(u);
    out.guid32.push({
      uuidV4: u,
      expectHexUpper: g,
      expectLen: g.length,
      deviceFingerprint: g + DEVICE_FINGERPRINT_SUFFIX,
    });
  }

  const outDir = path.resolve(__dirname, "..", "test", "golden");
  fs.mkdirSync(outDir, { recursive: true });
  const outFile = path.join(outDir, "login-crypto-vectors.json");
  fs.writeFileSync(outFile, JSON.stringify(out, null, 2) + "\n");

  console.log(`wrote ${outFile}`);
  console.log(`crypto-js ${out.$cryptoJsVersion}`);
  console.log(
    Object.entries(out)
      .filter(([k]) => !k.startsWith("$") && k !== "constants")
      .map(([k, v]) => `${k}=${v.length}`)
      .join("  "),
  );

  const realOk = out.puzzleAnswer.filter((v) => v.soMatchesRealCapture).length;
  console.log(`puzzleAnswer 命中真实抓包: ${realOk}/${PUZZLE_REAL.length}`);
  if (realOk !== PUZZLE_REAL.length) {
    console.error("❌ 未能复现真实抓包密文 —— 密钥/IV/模式/padding/分段有误，禁止入库");
    process.exit(1);
  }

  for (const v of out.aesEcbHexDecrypt) {
    if (v.expectDecrypt !== v.expectPlain) {
      console.error(`❌ ECB 往返失败: ${v.name} -> "${v.expectDecrypt}"`);
      process.exit(1);
    }
  }
  console.log("✅ ECB hex 往返全部一致");
}

main();
