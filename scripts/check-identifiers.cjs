#!/usr/bin/env node
/**
 * 仓库级标识泄漏守卫：**进了版本库的东西里不许有真实手机号 / userDomainId**。
 *
 * # 为什么需要它（而单测里的守卫不够）
 *
 * `test_support::assert_only_synthetic_identifiers` 只管**夹具与黄金向量**
 * 这两类生成物——它们由脚本产出，扫一遍就知道脱敏有没有生效。
 * 但泄漏还能从另一条路进来：**手写的源码字面量**。
 * 本仓库真就中过一次，而且中的方式很刁：
 *
 *   `migration.rs` / `legacy_import.rs` 的测试里写着一条 `"Basic Mjox…"` 开头的
 *   base64 字面量。解码后是 `2:<真实手机号>:ak|bk|…`，即真实号码。
 *
 * （这里刻意不把那串 base64 抄进来当例子 —— 抄进来就等于让本脚本自己触发一次失败。
 * 第一版就是这么写的，跑起来第一件事就是抓到它自己。）
 *
 * 字面正则扫不到（号码被 base64 藏了），生成器自检也不管源码，
 * 于是它一路通过了两道守卫。**本脚本把 base64 载荷解码后再扫一遍**，堵的就是这条。
 *
 * # 判据
 *
 * 合成值统一是 `199` 开头（11 位手机号、19 位 userDomainId）——
 * 与生成器、与 Rust 侧单测用的是同一条不变量。少数**有意非 199** 的
 * 占位/构造值登记在下面的白名单里，每条都要写清理由。
 *
 * # 用法
 *
 *     node scripts/check-identifiers.cjs
 *
 * 退出码 0 = 干净，1 = 有疑似真实标识。
 */

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const REPO_ROOT = path.join(__dirname, "..");

/** 合成号段前缀。 */
const SYNTHETIC_PREFIX = "199";

/**
 * 允许出现的非 199 值。
 *
 * 只有两类能进这里：
 * 1. 一眼假的大路货占位号（`13812345678` 这种，谁都知道是假的）；
 * 2. **刻意**非 199 的测试构造值 —— 因为要测"不是合成号段"的情形。
 *
 * 真实值（哪怕只是"从真实值派生"）**一律不准登记**，必须改成 199 号段。
 */
const ALLOWED = new Map([
  ["12345678901", "顺序号占位（`12345678901`）"],
  [
    "1234567890123456789",
    "顺序号占位（`1234567890123456789`）：sanitize 的标识重映射单测用它当第二个输入",
  ],
  ["13800000000", "test_support 守卫自测：**故意**用非合成值证明守卫会失败"],
  ["13812345678", "经典占位号（`138-1234-5678`）"],
  ["17700000000", "占位号"],
  ["1000000000000000001", "test_support 守卫自测：**故意**用非合成 userDomainId"],
  ["19000000001", "掩码映射测试：刻意非 199，用来与合成号段区分"],
  ["19100000001", "掩码冲突测试：刻意非 199"],
  ["19100000002", "掩码映射测试：刻意非 199"],
]);

/** 11 位 = 手机号；19 位 = userDomainId。两侧要求非字母数字，避免误判十六进制串。 */
const SHAPES = [
  ["手机号", /(?<![0-9A-Za-z])(1\d{10})(?![0-9A-Za-z])/g],
  ["userDomainId", /(?<![0-9A-Za-z])(1\d{18})(?![0-9A-Za-z])/g],
];

/** 看起来像 base64 的片段；太短没有判别力。 */
const B64_CHUNK = /[A-Za-z0-9+/]{16,}={0,2}/g;

/**
 * 敏感键名下的长值 —— **形状类规则抓不到的那一类**。
 *
 * 实测踩到：真实 `authToken` 形如 `<8 字符前缀>|1|RCS|<13 位毫秒>|<以 _ 与 . 分段的载荷>`
 * （下面写合成值当例子 —— 本仓库踩过一次：真实前缀被抄进了这条注释**和一条提交信息**，
 * 改文件时没改注释、也没改 commit message，`git log -S` 一搜就现形）。
 * `|` 会把串切成每段都不足 40 字符的碎片，于是"长串形状"规则一段都认不出来 ——
 * 而它是**可直接复用的凭据**，比手机号更该拦（实测它就这样进过版本库，守卫没报）。
 *
 * 所以换一个维度：**不按值的形状，按键名找**。
 * 刻意只认 JSON 形态（`"key": "value"`），不认 JS 源码里的 `token: token` ——
 * 否则会把变量赋值当成明文凭据，噪声大到没法用。
 */
const SENSITIVE_KEY_VALUE =
  /"(auth_token|authToken|authorization|access_token|accessToken|token|jwt|dycpwd|password|passwd|secret|cookie)"\s*:\s*"([^"]{20,})"/gi;

/**
 * 一眼就是占位/合成值的长值 —— 不算违规。
 *
 * 这条与上面那条同等重要：规则的目的是"拦住真实凭据"，**不是**"逼着人把文档里的样例
 * 改成 `<REDACTED>`"。协议文档要靠样例说明 token 的分段与长度，整段替换会把文档价值
 * 一起抹掉 —— 那就是过度脱敏。
 *
 * 判据刻意保守，只放行一眼假的：含占位关键词、整值同字符重复、或去掉分隔符后字符种类 ≤ 4。
 */
function isObviousPlaceholder(value) {
  if (/REDACTED|PLACEHOLDER|SYNTH|EXAMPLE|SAMPLE|DUMMY|FAKE|XXXX/i.test(value)) {
    return true;
  }
  // 含非 ASCII：中文说明（`<32位大写…sID>` 这种）不可能是凭据
  if (/[^\x20-\x7E]/.test(value)) return true;
  if (/^(.)\1+$/.test(value)) return true;
  // 凭据总含一段**不透明的长串**。`ak|bk|ck|…|fk` 这种"短段拼接"是刻意构造的黄金向量，
  // 真正的 token / 口令散列一定有一截连续几十字符的载荷 —— 用这条把两者分开。
  if (!/[A-Za-z0-9+/=_-]{16,}/.test(value)) return true;
  const distinct = new Set(value.replace(/[^A-Za-z0-9]/g, "")).size;
  return distinct <= 4;
}

/**
 * 不做"用户标识"扫描的路径 —— 抓下来的**上游客户端源码**。
 *
 * `docs/capture/apiServer.js` 是移动客户端的源码（vendor 素材，用于对照协议实现），
 * 不是本项目的产出，也不是用户数据：里面的号码之类是上游自带的调试残留，
 * 既不来自用户、也不可复用。对它做脱敏只会毁掉这份对照素材。
 *
 * 注意这**不是**把值登记进 `ALLOWED` —— 那等于承认"这是要留在仓库里的真实标识"，
 * 与"真实值一律不准登记"冲突。这里是按**来源**豁免，理由写在注释里。
 */
const VENDOR_SOURCE_PATHS = ["docs/capture/apiServer.js"];

/** 只认这些后缀，避免去读图片/二进制。 */
const TEXT_EXT = new Set([
  ".rs", ".cjs", ".mjs", ".js", ".ts", ".json", ".md", ".toml", ".sh",
  ".yml", ".yaml", ".html", ".css", ".vue", ".txt",
  // `.py` 是**抓包侧脚本**的语言（`applog-capture.py` / `reqable-capture.py` /
  // `h5-device-id.py`）—— 它们天天跟真实流量打交道，是最容易把真实标识顺手写进
  // 字面量的一类文件。漏了它，守卫就漏掉了风险最高的一角。
  ".py",
]);

function trackedFiles() {
  // `-c` 已暂存 + `-o` 未跟踪 + `--exclude-standard` 尊重 .gitignore。
  // 用两者并集：新文件在 `git add` 之前也要被拦住。
  const out = execFileSync("git", ["ls-files", "-co", "--exclude-standard"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
  });
  return out.split("\n").filter(Boolean);
}

/** 解码 base64 载荷；只收"没有控制字符"的结果，滤掉 hex 被硬解出的二进制垃圾。 */
function decodedPayloads(text) {
  const out = [];
  for (const m of text.matchAll(B64_CHUNK)) {
    const chunk = m[0];
    for (const candidate of [chunk, chunk + "=".repeat((4 - (chunk.length % 4)) % 4)]) {
      let decoded;
      try {
        decoded = Buffer.from(candidate, "base64").toString("utf8");
      } catch {
        continue;
      }
      // 往返校验：长度不对或含替换字符说明它不是真的 base64 文本
      if (decoded.includes("\uFFFD")) continue;
      if ([...decoded].some((c) => c.charCodeAt(0) < 32 && c !== "\n" && c !== "\t")) continue;
      out.push(decoded);
      break;
    }
  }
  return out;
}

function scan(label, text, violations) {
  for (const [kind, re] of SHAPES) {
    for (const m of text.matchAll(re)) {
      const value = m[1];
      if (value.startsWith(SYNTHETIC_PREFIX)) continue;
      if (ALLOWED.has(value)) continue;
      violations.push({ label, kind, value });
    }
  }

  // 键名维度：见 SENSITIVE_KEY_VALUE 的说明。凭据**不打印值本身** ——
  // 报告会进 CI 日志，把一可用凭据再抄一遍就等于又泄一次；
  // 给前 6 后 4 足够定位（`grep` 一下就找到）。
  for (const m of text.matchAll(SENSITIVE_KEY_VALUE)) {
    const [, key, value] = m;
    if (ALLOWED.has(value) || isObviousPlaceholder(value)) continue;
    const masked = `${value.slice(0, 6)}…${value.slice(-4)}`;
    violations.push({
      label,
      kind: `疑似凭据明文于 ${key}（${value.length} 字符）`,
      value: masked,
    });
  }
}

function main() {
  const violations = [];
  let scanned = 0;
  let vendorSkipped = 0;

  for (const rel of trackedFiles()) {
    if (rel.startsWith("target/") || rel.startsWith("node_modules/")) continue;
    if (VENDOR_SOURCE_PATHS.includes(rel)) {
      vendorSkipped += 1;
      continue;
    }
    if (!TEXT_EXT.has(path.extname(rel))) continue;

    let text;
    try {
      text = fs.readFileSync(path.join(REPO_ROOT, rel), "utf8");
    } catch {
      continue;
    }
    scanned += 1;

    scan(rel, text, violations);
    decodedPayloads(text).forEach((decoded, i) => {
      scan(`${rel}（base64 载荷 #${i + 1}）`, decoded, violations);
    });
  }

  if (violations.length) {
    console.error(
      `检查失败：发现 ${violations.length} 处疑似真实标识。\n` +
        `合成值必须是 ${SYNTHETIC_PREFIX} 开头；这是本地日志与版本库的分界线 ——\n` +
        `日志里留着无所谓，仓库推上去就收不回来。\n`,
    );
    const seen = new Set();
    for (const v of violations) {
      const key = `${v.label}|${v.value}`;
      if (seen.has(key)) continue;
      seen.add(key);
      console.error(`  ${v.label}: ${v.kind} ${v.value}`);
    }
    console.error(
      `\n若某值确实是刻意为之的测试构造值，请连同理由登记到\n` +
        `  scripts/check-identifiers.cjs 的 ALLOWED 表。\n` +
        `真实值一律不准登记 —— 必须改成 ${SYNTHETIC_PREFIX} 号段。`,
    );
    process.exit(1);
  }

  console.log(
    `检查通过：扫了 ${scanned} 个文本文件（字面 + base64 解码后），` +
      `共发现 ${ALLOWED.size} 个已登记的非 ${SYNTHETIC_PREFIX} 值，无未登记的疑似真实标识。` +
      (vendorSkipped > 0
        ? `（另有 ${vendorSkipped} 个上游源码素材按来源豁免，见 VENDOR_SOURCE_PATHS）`
        : ""),
  );
}

main();
