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

/** 只认这些后缀，避免去读图片/二进制。 */
const TEXT_EXT = new Set([
  ".rs", ".cjs", ".mjs", ".js", ".ts", ".json", ".md", ".toml", ".sh",
  ".yml", ".yaml", ".html", ".css", ".vue", ".txt",
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
}

function main() {
  const violations = [];
  let scanned = 0;

  for (const rel of trackedFiles()) {
    if (rel.startsWith("target/") || rel.startsWith("node_modules/")) continue;
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
      `共发现 ${ALLOWED.size} 个已登记的非 ${SYNTHETIC_PREFIX} 值，无未登记的疑似真实标识。`,
  );
}

main();
