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

/**
 * 本脚本自己的相对路径。
 *
 * 它**也会被扫** —— 这是有意的：这个文件里抄一条真实值当例子就是事故
 * （本文件的头注释记着第一版正是这么栽的：跑起来第一件事就是抓到它自己）。
 *
 * ⚠ 但它**不能**用来证明「某条 `ALLOWED` 还活着」：表就在这个文件里，
 * 加一条假条目就等于自证。所以 `allowedSeen` 不收本文件的命中 —— 见那里。
 */
const SELF_REL = "scripts/check-identifiers.cjs";

/** 合成号段前缀。 */
const SYNTHETIC_PREFIX = "199";

/**
 * 允许出现的非 199 值。
 *
 * 只有三类能进这里：
 * 1. 一眼假的大路货占位号（`13812345678` 这种，谁都知道是假的）；
 * 2. **刻意**非 199 的测试构造值 —— 因为要测"不是合成号段"的情形；
 * 3. **应用级白盒常量** —— 从上游客户端源码原样取出的协议常量（如 H5 离线包里的
 *    `logProjectId`）。它所有用户共用、**不是账号凭据**，且必须逐字保留才能与
 *    上游源码对照；只是形状恰好像手机号，才落进这条规则的射程。
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
  ["19200000002", "掩码冲突测试（多冲突组）：刻意非 199，与 190/191 同族"],
  ["19300000002", "掩码冲突测试（多冲突组）：刻意非 199，与 190/191 同族"],
  [
    "10000000003",
    "应用级白盒常量：H5 离线包 index-af02d869.js 里的 `logProjectId`（`let $r = \"10000000003\"`）。" +
      "所有用户共用、**非账号凭据**；`h5_env` 非 dev2Prod 时它切到 `1111`。" +
      "必须逐字保留才能与上游源码对照，形状恰好像手机号才被这条规则命中",
  ],
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
 *
 * `docs/capture/pcclient-api.js` 同理：它是**同一个客户端** `app.asar` 里的
 * `out/renderer/api/api.js`（首行 `console.log('************api.js')`），
 * 与 `apiServer.js` 是同一目录下的姊妹文件。
 *
 * ⚠ 两份都来自 PC 客户端，但**共享同一个豁免理由是巧合、不是保证** ——
 * 以后再抓新的客户端源码进来，要**逐个**判断并加进这里，别因为"目录名叫 capture"
 * 就默认整目录安全。这个清单漏过 `pcclient-api.js` 一次：当时它没触发守卫，
 * 只是**恰巧**不含真实标识，属于运气而不是守住了。
 */
const VENDOR_SOURCE_PATHS = ["docs/capture/apiServer.js", "docs/capture/pcclient-api.js"];

/** 只认这些后缀，避免去读图片/二进制。 */
const TEXT_EXT = new Set([
  ".rs", ".cjs", ".mjs", ".js", ".ts", ".json", ".md", ".toml", ".sh",
  ".yml", ".yaml", ".html", ".css", ".vue", ".txt",
  // `.go` 是**迁移后的实现语言**。它与 `.rs` 同理、且风险相同：签名密钥、
  // 设备串、`toSourceId` 这类协议常量会大量出现在这里，而它们**形状上**
  // 就长得像真实标识。漏掉它，脱敏守卫在 Go 侧等于不存在 ——
  // 而"不存在"的表现是静默通过，不是报错。
  ".go",
  // `.py` 是**抓包侧脚本**的语言（`applog-capture.py` / `reqable-capture.py` /
  // `h5-device-id.py`）—— 它们天天跟真实流量打交道，是最容易把真实标识顺手写进
  // 字面量的一类文件。漏了它，守卫就漏掉了风险最高的一角。
  ".py",
  // `.tsx` 是前端组件的后缀。它与上面 `.go` / `.py` 是**同一种漏法**：
  // `.ts` 在表里、`.tsx` 不在，差一个字母，而"不在"的表现同样是静默通过。
  // （实测：`web/src/pages/*.tsx` 六个文件当时一个都没被扫到。）
  ".tsx",
  // `.csv` 是**抓包导出的请求/响应表**（`docs/capture/thirdlogin-*.csv`）。
  // 抓包产物是本仓库里最可能夹带真实号码的一类文件 —— 它不在表里的时候，
  // 守卫恰好对"风险最高的一角"闭着眼。见 docs/capture 的说明。
  ".csv",
  // 无后缀 —— `LICENSE` / `.gitignore` / `.githooks/pre-commit` 这些。
  // 它们里有两类**真脚本**（`.githooks/` 下的钩子），漏掉同样是静默的。
  "",
]);

/**
 * 刻意**不扫**的后缀。每条要写理由。
 *
 * ⚠ 这张表与 `TEXT_EXT` **合起来**必须覆盖仓库里出现过的每一个后缀 ——
 * 有第三种情况就失败（见 `checkExtensionCoverage`）。
 *
 * 为什么要有这张表：`TEXT_EXT` 是手维护的，而它**已经漏过三次**
 * （`.go` / `.py` / `.tsx`+`.csv` 都是事后补的）。漏了不会自报，
 * 表现是"扫了 503 个文件，通过"。有了这张表，**新增一类文件必须显式做一次
 * 决定**（扫，或者写理由不扫），而不是被静默跳过。
 */
const NOT_SCANNED_EXT = new Map([
  [".mod", "go.mod：模块名与依赖声明，是构建清单、不含用户数据"],
  [".sum", "go.sum：依赖校验和清单，同上"],
  [".lock", "Cargo.lock（legacy 侧的依赖版本清单），同上"],
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

/**
 * 本次真正**命中**过的 `ALLOWED` 值 —— 只有它被用来放行一次违规时才算。
 *
 * ⚠ 这与 `ALLOWED.size` 是两回事，而且必须分开：
 * `ALLOWED.size` 量的是**表**，`allowedSeen.size` 量的是**事实**。
 * 末尾那行摘要如果打印表的大小，就等于说"登记了 12 条"——
 * 哪怕其中 11 条早就不存在了也照说不误（这正是门禁脚本上一轮踩过的坑：
 * 横幅量的是清单，不是这一次真的跑了什么）。
 *
 * ⚠⚠ **本文件自己的命中不算数**（`countAlive = false`）。
 * 表的正文就写在 `SELF_REL` 里，而 `SELF_REL` 也在扫描范围内 ——
 * 若把它算进去，**每一条 `ALLOWED` 都必然"还活着"**（被自己的正文命中），
 * 下面的死条目检查就**永远不会失败**：随便加一条假条目都能自证。
 * 实测踩到：加一条**仓库里根本没有**的假值（`["1370…009", "假条目"]`）后，
 * 仍然报"登记表 13 条、本次命中 13 条"、退 0。
 * ⚠ 写这段说明时踩了同一个坑：注释里抄了一个**形状完整**的假号码当例子，
 * 本脚本扫到自己、当场变红 —— 举例子时把数字串打断（上面那串里的 `…`）。
 * ⇒ 判据是「**别处**还有没有这个值」，不是「这个值在不在仓库里」。
 */
const allowedSeen = new Set();

function scan(label, text, violations, countAlive = true) {
  for (const [kind, re] of SHAPES) {
    for (const m of text.matchAll(re)) {
      const value = m[1];
      if (value.startsWith(SYNTHETIC_PREFIX)) continue;
      if (ALLOWED.has(value)) {
        if (countAlive) allowedSeen.add(value);
        continue;
      }
      violations.push({ label, kind, value });
    }
  }

  // 键名维度：见 SENSITIVE_KEY_VALUE 的说明。凭据**不打印值本身** ——
  // 报告会进 CI 日志，把一可用凭据再抄一遍就等于又泄一次；
  // 给前 6 后 4 足够定位（`grep` 一下就找到）。
  for (const m of text.matchAll(SENSITIVE_KEY_VALUE)) {
    const [, key, value] = m;
    if (ALLOWED.has(value)) {
      if (countAlive) allowedSeen.add(value);
      continue;
    }
    if (isObviousPlaceholder(value)) continue;
    const masked = `${value.slice(0, 6)}…${value.slice(-4)}`;
    violations.push({
      label,
      kind: `疑似凭据明文于 ${key}（${value.length} 字符）`,
      value: masked,
    });
  }
}

/**
 * 仓库里出现过的后缀，有没有**既不在 `TEXT_EXT`、也不在 `NOT_SCANNED_EXT`** 的。
 *
 * 返回 `后缀 → 文件名[]`（空的 Map = 覆盖完整）。
 * 这是把"守卫扫哪些文件"从**静默**改成**显式**的那一下。
 */
function checkExtensionCoverage(files) {
  const unregistered = new Map();
  for (const rel of files) {
    if (rel.startsWith("target/") || rel.startsWith("node_modules/")) continue;
    const ext = path.extname(rel);
    if (TEXT_EXT.has(ext) || NOT_SCANNED_EXT.has(ext)) continue;
    if (!unregistered.has(ext)) unregistered.set(ext, []);
    unregistered.get(ext).push(rel);
  }
  return unregistered;
}

function main() {
  const violations = [];
  const problems = [];
  const files = trackedFiles();
  const vendorSeen = new Set();
  let scanned = 0;

  for (const rel of files) {
    if (rel.startsWith("target/") || rel.startsWith("node_modules/")) continue;
    if (VENDOR_SOURCE_PATHS.includes(rel)) {
      vendorSeen.add(rel);
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

    // 本文件自己的命中可以**报违规**（抄了真实值当例子就是事故），
    // 但不许用来证明 `ALLOWED` 条目还活着 —— 表正文就在本文件里。见 `allowedSeen`。
    const countAlive = rel !== SELF_REL;
    scan(rel, text, violations, countAlive);
    decodedPayloads(text).forEach((decoded, i) => {
      scan(`${rel}（base64 载荷 #${i + 1}）`, decoded, violations, countAlive);
    });
  }

  // ── 下面几条查的都是**表本身**，不是扫出来的违规。分开报，别混在一起 ──

  // 1. 覆盖范围：有后缀两边都不在 ⇒ 有人加了一类新文件，守卫静默跳过了它。
  const unregistered = checkExtensionCoverage(files);
  if (unregistered.size > 0) {
    problems.push(
      "以下后缀既不在 TEXT_EXT（要扫）也不在 NOT_SCANNED_EXT（写理由不扫）——\n" +
        "  这类文件现在**被静默跳过**。请二选一：加进 TEXT_EXT，或加进 NOT_SCANNED_EXT 并写明理由。\n" +
        [...unregistered.entries()]
          .sort((a, b) => b[1].length - a[1].length)
          .map(([ext, list]) => `    ${ext === "" ? "(无后缀)" : ext}  ×${list.length}：${list.slice(0, 3).join(", ")}${list.length > 3 ? " …" : ""}`)
          .join("\n"),
    );
  }

  // 2. `ALLOWED` 里的死条目 —— **别处**已经找不到这个值了，这条理由过期了。
  //    与 check-go-jsontags.cjs 的 stale 检查同一套判据（那边早就有，这边一直没有）。
  //    ⚠ 判据的措辞是"别处"：本文件自己的命中不算（见 allowedSeen），
  //    否则这条检查是恒真的空操作。
  const staleAllowed = [...ALLOWED.keys()].filter((v) => !allowedSeen.has(v));
  if (staleAllowed.length > 0) {
    problems.push(
      "ALLOWED 里有**对不上**的条目（那个值已经不在仓库里了）—— 请删掉：\n" +
        staleAllowed.map((v) => `    ${v}`).join("\n"),
    );
  }

  // 3. `VENDOR_SOURCE_PATHS` 的豁免必须**真的在豁免一个东西**。
  //    这个清单自己漏过一次（见上面注释）：漏的表现是"没报错"，不是"报错"。
  const deadVendor = VENDOR_SOURCE_PATHS.filter((p) => !vendorSeen.has(p));
  if (deadVendor.length > 0) {
    problems.push(
      "VENDOR_SOURCE_PATHS 里有**对不上**的路径（文件改名 / 删除 / 路径写错）——\n" +
        "  豁免没生效，而这**不会**让扫描失败，只是悄悄不再豁免它。请修正或删掉：\n" +
        deadVendor.map((p) => `    ${p}`).join("\n"),
    );
  }
  const noopVendor = VENDOR_SOURCE_PATHS.filter(
    (p) => vendorSeen.has(p) && !TEXT_EXT.has(path.extname(p)),
  );
  if (noopVendor.length > 0) {
    problems.push(
      "VENDOR_SOURCE_PATHS 里有**空豁免**（后缀本来就不在扫描范围里，豁不豁免都一样）——\n" +
        "  它会让人以为「这一角已经处理过了」。请删掉或说明：\n" +
        noopVendor.map((p) => `    ${p}`).join("\n"),
    );
  }

  // ⚠ 两段都报完再退出，**不要**在 problems 那里提前 exit ——
  // 否则"表过期"会把"扫出真实标识"挡在报告之外，而那才是更要紧的那条。
  // （反向测试也依赖这一点：两条守卫要能各自单独观测到。）
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
  }

  if (problems.length > 0) {
    console.error(`\n守卫自身的问题：\n`);
    for (const p of problems) console.error(`  ✗ ${p}\n`);
    console.error(
      "这几条查的是**表**（扫哪些文件 / 哪些值算例外），不是扫出来的违规 ——\n" +
        "表过期不会自报，表现是「通过」，所以单独拦一次。\n",
    );
  }

  if (violations.length || problems.length) process.exit(1);

  // ⚠ 摘要里的两个数都取**观测值**，不取表的大小：
  // `allowedSeen` 是本次真命中过的，`vendorSeen` 是本次真跳过的。
  // 表的大小另有 `ALLOWED.size` / `VENDOR_SOURCE_PATHS.length` 可看，
  // 但那两个数是"我们登记了什么"，不是"这次发生了什么"。
  console.log(
    `检查通过：扫了 ${scanned} 个文本文件（字面 + base64 解码后），` +
      `登记表 ${ALLOWED.size} 条、本次命中 ${allowedSeen.size} 条，无未登记的疑似真实标识。` +
      `（另有 ${vendorSeen.size} 个上游源码素材按来源豁免，见 VENDOR_SOURCE_PATHS）`,
  );
}

main();
