#!/usr/bin/env node
/**
 * 从 TS 原版生成「账号身份解析」黄金向量。
 *
 * 与 `gen-crypto-vectors.cjs` 同一套路，但更进一步：本脚本**直接 import 原版编译产物**
 * （`packages/core/dist/core/src/auth/utils.js`），而不是在脚本里重写一遍逻辑。
 * 原因：身份解析决定 `AccountId`，而 `AccountId` 是旧 `data/state.json` 的唯一键 ——
 * 一旦这里对不上号，历史数据就会挂到错误的账号上，且**不会报错**，只会静默串号。
 *
 * 为什么要 shim：编译产物里 `import { rt } from "@asunajs/runtime"` 会解析到原仓库的
 * `packages/runtime/src/index.ts`（TS 源码，Node 跑不了）。我们在临时目录里放一个
 * `node_modules/@asunajs/runtime` 转发到 **已编译** 的 `dist/node.js`，
 * 再把编译产物原样拷一份过去，就能在不动原仓库任何文件的前提下引用真实实现。
 *
 * 用法：
 *   NODE_PATH=/Users/catlair/Documents/workspace/mcloud-sign/node_modules \
 *     node scripts/gen-auth-vectors.cjs
 */

const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { pathToFileURL } = require('node:url')

/** 原仓库根目录（也可用 MCLOUD_SIGN_SRC 覆盖） */
const SRC = process.env.MCLOUD_SIGN_SRC || '/Users/catlair/Documents/workspace/mcloud-sign'

const COMPILED_AUTH = path.join(SRC, 'packages/core/dist/core/src/auth/utils.js')
const COMPILED_RUNTIME = path.join(SRC, 'packages/runtime/dist/node.js')
const TS_ACCOUNT = path.join(SRC, 'packages/api/src/account.ts')

// ───────────────────────── shim：把 @asunajs/runtime 指到已编译产物 ─────────────────────────

function setUpShim() {
  for (const [label, file] of [
    ['core/auth/utils.js', COMPILED_AUTH],
    ['runtime/dist/node.js', COMPILED_RUNTIME],
  ]) {
    if (!fs.existsSync(file)) {
      throw new Error(
        `缺少编译产物 ${label}（${file}）。请先在原仓库执行构建，或设置 MCLOUD_SIGN_SRC。`,
      )
    }
  }

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-auth-vectors-'))
  const pkgDir = path.join(dir, 'node_modules/@asunajs/runtime')
  fs.mkdirSync(pkgDir, { recursive: true })
  fs.writeFileSync(
    path.join(pkgDir, 'package.json'),
    JSON.stringify(
      {
        name: '@asunajs/runtime',
        version: '0.0.0',
        type: 'module',
        main: 'index.js',
        exports: './index.js',
      },
      null,
      2,
    ),
  )
  fs.writeFileSync(
    path.join(pkgDir, 'index.js'),
    `export { rt } from ${JSON.stringify(COMPILED_RUNTIME)};\n`,
  )

  // 原样拷贝编译产物（不改一个字节），使其 import 的 @asunajs/runtime 命中上面的 shim
  const authDir = path.join(dir, 'auth')
  fs.mkdirSync(authDir, { recursive: true })
  const shimmedAuth = path.join(authDir, 'utils.js')
  fs.copyFileSync(COMPILED_AUTH, shimmedAuth)

  return { dir, shimmedAuth }
}

// ───────────────────────── 源码漂移守卫 ─────────────────────────

/**
 * `accountKey` / `accountLabel` / `resolvePhone` 这三行是"身份"的定义，
 * 但它们在原仓库里没有编译进可 import 的独立模块（被 config/logger 拖着）。
 * 所以这里不重写逻辑（重写就会漂移），而是**断言源码仍是那三行**；
 * 一旦原仓库改了，本脚本立刻报错，逼人重新生成向量。
 */
function assertIdentitySourceUnchanged() {
  const src = fs.readFileSync(TS_ACCOUNT, 'utf8')
  const expectations = [
    ['resolvePhone', 'return account.phone || parseAuth(account.auth)?.phone || "";'],
    ['accountLabel', 'return phone ? `${phone.slice(0, 2)}****${phone.slice(-2)}` : "账号";'],
    ['accountKey', 'return resolvePhone(account) || account.nickname || "account";'],
  ]
  for (const [name, snippet] of expectations) {
    if (!src.includes(snippet)) {
      throw new Error(
        `原仓库 ${name} 的实现已变化，向量可能失效。\n  期望片段: ${snippet}\n` +
          `请人工核对 packages/api/src/account.ts 后更新本脚本。`,
      )
    }
  }
}

// ───────────────────────── 向量构造 ─────────────────────────

/** 与 accountLabel 同一套掩码：前 2 + **** + 后 2 */
const maskLikeTs = (phone) => `${phone.slice(0, 2)}****${phone.slice(-2)}`

/** 构造一个 auth 字符串：base64("<platform>:<phone>:<token>")，padding 可选保留 */
function makeAuth(platform, phone, token, { stripPadding = false, prefix = '' } = {}) {
  let b64 = Buffer.from(`${platform}:${phone}:${token}`, 'utf8').toString('base64')
  if (stripPadding) b64 = b64.replace(/=+$/, '')
  return prefix + b64
}

function parseCase(name, input) {
  return { name, input }
}

async function main() {
  assertIdentitySourceUnchanged()
  const { dir, shimmedAuth } = setUpShim()
  let parseAuth
  let padBase64
  try {
    const mod = await import(pathToFileURL(shimmedAuth).href)
    parseAuth = mod.parseAuth
    padBase64 = mod.padBase64

    // token 内部用 **竖线** 分段（第 4 段是过期时间）。
    // 注意别手滑写成冒号：`doParseAuth` 先按 ":" 切分且只取前 3 段，
    // 冒号会让 token 被截断成一个短串，expire 随之变成 0 —— 这个坑本脚本第一版就踩过。
    //
    // expire 用 **13 位毫秒**（2030-01-01），与真实 `asign.json` 同量级。
    // 早先这里是 10 位秒值 `1700000000`，与真实数据差三个数量级，
    // 于是"expire 到底是秒还是毫秒"这个问题在向量里根本看不出来（见 `auth.rs` 模块头）。
    // 号码一律用 **199 开头的合成号**，绝不写真实号码。
    // 黄金向量会进版本库，写真实手机号就是 PII 泄漏 —— 早先这里用的是
    // 本机账号的真实号码（`19142…` / `15909…`），已全部替换。
    // 这条也由 Rust 侧断言兜住：`test/golden/*.json` 里的 11 位号码必须全为 199 开头。
    const TOKEN = 'ak|bk|ck|1893456000000|fk'
    const TOKEN_NO_EXPIRE = 'ak|bk|ck'
    const TOKEN_SHORT = 'a|b'

    const inputs = [
      // ── 正常形态 ──
      ['plain', makeAuth('2', '19900000001', TOKEN)],
      ['with-basic-prefix', makeAuth('2', '19900000001', TOKEN, { prefix: 'Basic ' })],
      ['padding-stripped', makeAuth('2', '19900000001', TOKEN, { stripPadding: true })],
      [
        'basic-and-stripped',
        makeAuth('2', '19900000001', TOKEN, { stripPadding: true, prefix: 'Basic ' }),
      ],
      ['platform-4', makeAuth('4', '19900000002', TOKEN)],
      ['token-without-expire', makeAuth('2', '19900000001', TOKEN_NO_EXPIRE)],
      ['token-short-fields', makeAuth('2', '19900000001', TOKEN_SHORT)],
      ['token-expire-not-numeric', makeAuth('2', '19900000001', 'a|b|c|notanumber|d')],
      ['token-expire-zero', makeAuth('2', '19900000001', 'a|b|c|0|d')],
      ['token-expire-negative', makeAuth('2', '19900000001', 'a|b|c|-1|d')],
      ['token-expire-float', makeAuth('2', '19900000001', 'a|b|c|1.5|d')],
      ['phone-only-two-fields', makeAuth('2', '19900000001', '')],
      ['empty-token', makeAuth('2', '19900000001', '')],
      ['empty-phone', makeAuth('2', '', TOKEN)],
      // token 里带冒号 → 被 split(":") 截断，只留下第一段。这条专门钉住"截断"这个行为。
      ['colon-in-token-truncated', makeAuth('2', '19900000001', 'a:b:c')],
      // ── 非 ASCII：phone 里带全角？不会出现，但 token 里可能有分隔符
      ['token-with-pipe-only', makeAuth('2', '19900000001', '||||')],
      // ── 应当解析失败（原版为 null，或解出乱码）──
      ['empty-string', ''],
      ['lowercase-basic-prefix', `basic ${makeAuth('2', '19900000001', TOKEN)}`],
      ['no-colon-at-all', Buffer.from('justonefield').toString('base64')],
      ['too-many-colons', Buffer.from('p:ph:to:k:en').toString('base64')],
      ['not-base64-at-all', '!!!not-base64!!!'],
      ['plain-text', 'hello world'],
    ]

    const parseAuthCases = []
    for (const [name, input] of inputs) {
      let result
      try {
        result = parseAuth(input)
      } catch (e) {
        result = { __threw: String(e && e.message ? e.message : e) }
      }
      parseAuthCases.push({
        name,
        input,
        expect: result,
        // 原版对"非 base64 / 解出非 UTF-8"是宽松的：不抛错，而是产出乱码 platform。
        // 这种编码病态输入我们不追求逐字复刻（见 Rust 侧 crypto/auth 模块注释），
        // 标记出来让 Rust 测试只断言"能识别为不可用"。
        malformed:
          result !== null &&
          typeof result === 'object' &&
          !result.phone &&
          !result.token &&
          !!result.platform &&
          !/^[0-9a-z]+$/.test(result.platform),
      })
    }

    const padCases = [
      ['len-0', ''],
      ['len-1', 'A'],
      ['len-2', 'AB'],
      ['len-3', 'ABC'],
      ['len-4', 'ABCD'],
      ['len-5', 'ABCDE'],
      ['len-6', 'ABCDEF'],
      ['len-7', 'ABCDEFG'],
      ['len-8', 'ABCDEFGH'],
      ['already-padded', makeAuth('2', '19900000001', TOKEN)],
    ].map(([name, input]) => ({ name, input, expect: padBase64(input) }))

    // 身份派生：用真实的 parseAuth 驱动，逻辑与 account.ts 的三行完全一致（且有漂移守卫）
    const identityInputs = [
      ['auth-only', { auth: makeAuth('2', '19900000001', TOKEN) }],
      ['auth-and-nickname', { auth: makeAuth('2', '19900000001', TOKEN), nickname: '账号A' }],
      [
        'explicit-phone-wins',
        { auth: makeAuth('2', '19999999999', TOKEN), phone: '19900000001', nickname: '账号A' },
      ],
      ['nickname-fallback-when-auth-invalid', { auth: '!!!not-base64!!!', nickname: '账号B' }],
      ['placeholders-when-both-missing', { auth: '!!!not-base64!!!' }],
      ['empty-auth-with-nickname', { auth: '', nickname: '账号C' }],
      ['cjk-nickname', { auth: makeAuth('2', '19900000001', TOKEN), nickname: '移动云盘·测试' }],
    ]

    const identityCases = identityInputs.map(([name, account]) => {
      const parsed = account.auth ? parseAuth(account.auth) : null
      const phone = account.phone || (parsed && parsed.phone) || ''
      const rawKey = phone || account.nickname || 'account'
      const label = account.nickname || (phone ? maskLikeTs(phone) : '账号')
      return {
        name,
        account,
        parsedPhone: (parsed && parsed.phone) || null,
        // 旧项目的 state.json 用的就是这个键（手机号原文）
        legacyKey: rawKey,
        // 新项目的 AccountId：手机号走掩码（避免把完整号码当主键），否则昵称
        accountId: phone ? maskLikeTs(phone) : rawKey,
        label,
      }
    })

    const out = {
      $generator: 'scripts/gen-auth-vectors.cjs',
      $source: `原版编译产物 ${path.relative(SRC, COMPILED_AUTH)}（经 shim 引用 @asunajs/runtime dist）`,
      $note:
        'Rust 侧 ../mcloud-rust/crates/mcloud-core/src/auth.rs 必须复现全部 expect*。' +
        'malformed=true 的用例只要求识别为「不可用」，不要求逐字复刻乱码（原版在编码病态输入下产出乱码 platform，属实现缺陷，不应继承）。',
      parseAuth: parseAuthCases,
      padBase64: padCases,
      identity: identityCases,
    }

    const outDir = path.resolve(__dirname, '..', 'test', 'golden')
    fs.mkdirSync(outDir, { recursive: true })
    const outFile = path.join(outDir, 'auth-vectors.json')
    fs.writeFileSync(outFile, JSON.stringify(out, null, 2) + '\n')

    const ok = parseAuthCases.filter((c) => c.expect && !c.malformed)
    const nul = parseAuthCases.filter((c) => c.expect === null)
    const mal = parseAuthCases.filter((c) => c.malformed)
    console.log(`wrote ${outFile}`)
    console.log(
      `parseAuth=${parseAuthCases.length}（正常 ${ok.length} / null ${nul.length} / 乱码 ${mal.length}）  ` +
        `padBase64=${padCases.length}  identity=${identityCases.length}`,
    )
    if (mal.length) {
      console.log('以下用例原版会产出乱码（Rust 侧只要求判为不可用）：')
      for (const c of mal)
        console.log(`   ${c.name} -> platform=${JSON.stringify(c.expect.platform)}`)
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

main().catch((e) => {
  console.error(e)
  process.exit(1)
})
