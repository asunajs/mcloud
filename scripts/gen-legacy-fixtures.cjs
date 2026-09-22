#!/usr/bin/env node
/**
 * 从旧项目（mcloud-sign）的真实数据目录生成**脱敏夹具**。
 *
 * 为什么不是手写夹具：手写的 JSON 只能证明"我以为旧格式长这样"。
 * 只有从真实文件生成，才能保证字段名、可选性、数值范围与真实数据一致 ——
 * 而反序列化 struct 与真实格式不符，正是迁移里最容易静默出错的地方。
 *
 * 为什么必须脱敏并自检：旧 `data/state.json` 里是**可直接复用的凭据**
 * （jwtToken / deviceId / cloudPhoneRedpackToken）。夹具会进版本库，
 * 所以这里做三件事：
 *   1. 凭据类字段一律替换为**同长度的规律串**（保留"长度"这个形状信息）；
 *   2. 手机号做**确定性重映射**（19900000001 起，按首次出现顺序），
 *      且 `auth` 直接**重建**成 `base64("2:<映射后手机号>:ak|bk|ck|1893456000000|fk")` ——
 *      这样夹具里的 auth 依然可被 `parse_auth` 解析，链路能端到端测。
 *      其中 `expire` 用 13 位毫秒（与真实数据同量级，别改成秒）；
 *   3. 写盘后**反向自检**：把原件里所有敏感值拿去搜输出，找到任何一个就直接退出非 0。
 *      这一步是硬保险，防止将来有人加文件时忘了脱敏。
 *
 * 用法：
 *   node scripts/gen-legacy-fixtures.cjs
 *   MCLOUD_SIGN_SRC=/path/to/mcloud-sign node scripts/gen-legacy-fixtures.cjs
 */

const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')

const SRC = process.env.MCLOUD_SIGN_SRC || '/Users/catlair/Documents/workspace/mcloud-sign'
const SRC_DATA = path.join(SRC, 'packages/api/data')
const SRC_CONFIG = path.join(SRC, 'asign.json')
const OUT_DIR = path.resolve(__dirname, '..', 'test', 'fixtures', 'legacy')

/** 夹具里最多保留多少条运行历史（原文件 96 条，全量搬进仓库没有意义） */
const MAX_RUNS = 12

/** 凭据类键名 —— 值必须替换 */
const SECRET_KEY = /auth|token|secret|password|passwd|deviceid|device_id|cookie|session/i

// ───────────────────────── 确定性脱敏 ─────────────────────────

/** 用可复现的字母表生成指定长度的假值，保留"长度"这一形状信息 */
function fakeOfLength(
  n,
  alphabet = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789',
) {
  let out = ''
  for (let i = 0; i < n; i += 1) out += alphabet[i % alphabet.length]
  return out
}

/** 手机号重映射表：11 位数字且以 1 开头 → 19900000001 起的替补号 */
const phoneMap = new Map()
function mapPhone(phone) {
  if (phoneMap.has(phone)) return phoneMap.get(phone)
  const next = `199${String(phoneMap.size + 1).padStart(8, '0')}`
  phoneMap.set(phone, next)
  // 原号码本身也是 PII，必须进自检名单 —— 否则只覆盖"当 key 出现的手机号"，
  // 而它同样可能从别处（如元信息、日志文案）漏出去。
  recordSecret(phone)
  return next
}

function isPhoneLike(value) {
  return typeof value === 'string' && /^1\d{10}$/.test(value)
}

/**
 * `userDomainId` 重映射表：19 位纯数字 → `199` 开头的 19 位替补号。
 *
 * 它是**账号标识**，和手机号同属"进了版本库就必须清掉"的东西 ——
 * 界线不在"能不能进日志"（日志是本地排障资产，标识信息留着有用），
 * 而在"能不能进 GitHub"：仓库是全量留痕的，一旦推上去就收不回来。
 *
 * 旧版本这里有个**静默失效**：`sanitize` 里写着"userDomainId 是 19 位纯数字"
 * 的注释分支，但 `SECRET_KEY` 正则里根本没有 `domainid`，那个分支永远走不到。
 * 于是真实值原样落进了 `state.json`，而自检也没报 —— 因为没人把它登记进
 * `originalSecrets`。教训：**脱敏规则的注释不等于规则本身**，必须有不变量断言兜底。
 *
 * 保留"19 位、纯数字"这个可判别形状：夹具贴近真实，守卫也好断言（见 199 前缀不变量）。
 */
const domainIdMap = new Map()
function mapDomainId(id) {
  if (domainIdMap.has(id)) return domainIdMap.get(id)
  const next = `199${String(domainIdMap.size + 1).padStart(16, '0')}`
  domainIdMap.set(id, next)
  recordSecret(id)
  return next
}

/** 键名是否形如 userDomainId / user_domain_id（归一化后判断） */
function isDomainIdKey(key) {
  return typeof key === 'string' && /domain[_-]?id/i.test(key)
}

/**
 * 重建 auth：`base64("2:<手机号>:<token>")`。
 *
 * 不保留原始 auth 的任何片段，只保留"它是一个合法 auth"这件事。
 * token 用固定串，expire 用一个**未来**时间戳，避免夹具随时间变成"已过期"。
 *
 * `expire` 必须是 **13 位毫秒**（2030-01-01）。这不是随便挑的：
 * 旧项目把它和 `Date.now()` 直接相减，真实 `asign.json` 里也是 13 位。
 * 早先这里写的是 10 位秒值 `1700000000`，结果整套夹具的量级都和真实数据不一致 ——
 * 于是"单位到底是秒还是毫秒"这个错误在测试里完全看不出来（见 `auth.rs` 模块头）。
 */
function rebuildAuth(phone) {
  const token = 'ak|bk|ck|1893456000000|fk'
  return `Basic ${Buffer.from(`2:${phone}:${token}`, 'utf8').toString('base64')}`
}

/** 收集原件里全部敏感值，供写盘后反向自检 */
const originalSecrets = new Set()
function recordSecret(value) {
  if (typeof value === 'string' && value.length >= 8) originalSecrets.add(value)
}

/** 递归脱敏：按 key 判断是否敏感，同时记录原始值 */
function sanitize(node, parentKey = '') {
  if (Array.isArray(node)) return node.map((item) => sanitize(item, parentKey))
  if (node === null || typeof node !== 'object') {
    if (typeof node === 'string' && SECRET_KEY.test(parentKey)) {
      recordSecret(node)
      return fakeOfLength(node.length)
    }
    if (typeof node === 'string' && isDomainIdKey(parentKey)) return mapDomainId(node)
    if (isPhoneLike(node)) return mapPhone(node)
    return node
  }
  const out = {}
  for (const [key, value] of Object.entries(node)) {
    // userDomainId 走专用映射（保持"19 位纯数字"形状，且统一 199 前缀）。
    // 必须排在 SECRET_KEY 之前：否则日后若有人把 domainid 塞进 SECRET_KEY，
    // 会退化成"数字字母表规律串"，形状判别特征与 199 前缀不变量都会丢。
    if (isDomainIdKey(key) && typeof value === 'string') {
      out[key] = mapDomainId(value)
      continue
    }
    if (SECRET_KEY.test(key) && typeof value === 'string') {
      recordSecret(value)
      // 敏感值本身若是纯数字（如 deviceId），用数字字母表保持"纯数字"这个可判别特征
      out[key] = fakeOfLength(value.length, /^\d+$/.test(value) ? '0123456789' : undefined)
      continue
    }
    // state.json 的键就是手机号 —— 也要重映射，否则夹具仍含 PII
    const mappedKey = isPhoneLike(key) ? mapPhone(key) : key
    if (isPhoneLike(key)) recordSecret(key)
    out[mappedKey] = sanitize(value, key)
  }
  return out
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'))
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')
}

// ───────────────────────── 各文件处理 ─────────────────────────

/** asign.json：保留 accounts 结构，auth 重建（这样夹具里的 auth 一定能被解析） */
function buildConfig() {
  const raw = readJson(SRC_CONFIG)
  const accounts = (raw.caiyun ?? []).map((account, index) => {
    recordSecret(account.auth)
    if (/^1\d{10}$/.test(account.phone ?? '')) recordSecret(account.phone)
    // 昵称（"账号1"这类）不含 PII，原样保留 —— 夹具要尽量贴近真实
    return {
      auth: rebuildAuth(mappedPhoneFor(account, index + 1)),
      nickname: account.nickname,
    }
  })
  return {
    version: raw.version ?? 2,
    caiyun: accounts,
  }
}

/** 取出该账号映射后的手机号：优先显式 phone，否则从 auth 里解出 */
function mappedPhoneFor(account, index) {
  const found = /^1\d{10}$/.test(account.phone ?? '')
    ? account.phone
    : (() => {
        try {
          const decoded = Buffer.from(
            String(account.auth).replace(/^Basic /, ''),
            'base64',
          ).toString('utf8')
          const phone = decoded.split(':')[1] ?? ''
          return /^1\d{10}$/.test(phone) ? phone : ''
        } catch {
          return ''
        }
      })()
  return mapPhone(found || `199${String(index).padStart(8, '0')}`)
}

function buildState() {
  const raw = readJson(path.join(SRC_DATA, 'state.json'))
  return sanitize(raw)
}

function buildHistory() {
  const raw = readJson(path.join(SRC_DATA, 'history.json'))
  const runs = raw.runs ?? []
  return {
    $fixtureNote: `原文件共 ${runs.length} 条，夹具只保留最后 ${MAX_RUNS} 条（其余同构，搬进仓库无意义）`,
    runs: sanitize(runs.slice(-MAX_RUNS)),
  }
}

function buildPipelines() {
  return sanitize(readJson(path.join(SRC_DATA, 'pipelines.json')))
}

function buildSchedules() {
  return sanitize(readJson(path.join(SRC_DATA, 'schedules.json')))
}

function buildExchange() {
  return sanitize(readJson(path.join(SRC_DATA, 'exchange.json')))
}

function buildWebui() {
  return sanitize(readJson(path.join(SRC_DATA, 'webui.json')))
}

// ───────────────────────── main ─────────────────────────

function main() {
  for (const file of [
    'state.json',
    'history.json',
    'pipelines.json',
    'schedules.json',
    'exchange.json',
    'webui.json',
  ]) {
    const p = path.join(SRC_DATA, file)
    if (!fs.existsSync(p)) throw new Error(`缺少源文件 ${p}。旧项目是否还没跑过？`)
  }
  if (!fs.existsSync(SRC_CONFIG)) throw new Error(`缺少配置文件 ${SRC_CONFIG}`)

  const files = {
    'asign.json': buildConfig(),
    'state.json': buildState(),
    'history.json': buildHistory(),
    'pipelines.json': buildPipelines(),
    'schedules.json': buildSchedules(),
    'exchange.json': buildExchange(),
    'webui.json': buildWebui(),
  }

  const meta = {
    $generator: 'scripts/gen-legacy-fixtures.cjs',
    $source: '旧项目 mcloud-sign 的真实运行数据（packages/api/data/ 与 asign.json）',
    $note:
      '**已脱敏**：凭据类字段（auth/token/deviceId/cloudPhoneRedpackToken/session…）为同长度规律串；' +
      '手机号按首次出现顺序重映射为 19900000001 起；userDomainId 重映射为 199 开头的 19 位替补号；' +
      'auth 用映射后的手机号重建 —— ' +
      '因此夹具里的 auth 仍可被 parse_auth 解析，链路能端到端测。' +
      '**保留**的是字段名、可选性与数值形状（长度、是否纯数字），夹具因此仍贴近真实数据。' +
      '界线：标识信息（手机号、userDomainId）在**日志**里保留 —— 日志是本地排障资产、且不可直接复用；' +
      '但本目录是进版本库的静态文件，一旦推到远端就全量留痕、收不回来，因此标识一律清掉。',
    $accountLink:
      'asign.json 的 caiyun[i].nickname ↔ history.json 的 runs[].account（旧版存的是展示名，不是手机号）；' +
      'state.json 的键是手机号原文（旧版 accountKey），迁移后要换成掩码 AccountId。',
    // 只记"被分配了哪些脱敏号码"，**不记原号码**。
    // 旧版本这里是 `Object.fromEntries(phoneMap)`，也就是把真实手机号当 key 写进夹具 ——
    // 那是实打实的 PII，而且因为自检没扫元信息（见 assertNoSecretLeak）一路漏到了盘上。
    // 现在连"原号码→脱敏号"的可逆映射都不留：需要对应关系时查这台机器上的原件即可。
    $phoneAssignment: {
      $rule: '按首次出现顺序重映射为 19900000001 起；原号码属 PII，不写入夹具',
      $assigned: [...new Set(phoneMap.values())],
    },
    $sourceSha256: Object.fromEntries(
      [
        'asign.json',
        'state.json',
        'history.json',
        'pipelines.json',
        'schedules.json',
        'exchange.json',
        'webui.json',
      ].map((name) => {
        const p = name === 'asign.json' ? SRC_CONFIG : path.join(SRC_DATA, name)
        return [name, sha256(p)]
      }),
    ),
    $configPath: 'asign.json（旧项目根目录）',
    $dataDir: 'packages/api/data/',
  }

  fs.mkdirSync(OUT_DIR, { recursive: true })

  // 先全部序列化到内存（**含元信息**），做完全局自检再落盘：
  // 宁可什么都不写，也不能写半个泄漏的夹具。
  const rendered = new Map()
  for (const [name, value] of Object.entries(files)) {
    rendered.set(name, JSON.stringify(value, null, 2) + '\n')
  }
  // 元信息**必须一起参与自检**。它描述的是"脱敏做了什么"，正是最容易顺手写下
  // "原号码→脱敏号"这类映射的地方 —— 早先它没进 haystack，于是真实手机号
  // 被写进 $phoneMap 并成功落盘。
  rendered.set('$meta.json', JSON.stringify(meta, null, 2) + '\n')

  assertNoSecretLeak(rendered)
  assertOnlySyntheticIdentifiers(rendered)

  for (const [name, text] of rendered) {
    fs.writeFileSync(path.join(OUT_DIR, name), text)
  }

  // 顺带把"真实数据里有、而夹具里被截断/替换掉了什么"讲清楚
  const history = files['history.json']
  console.log(`wrote ${OUT_DIR}`)
  console.log(
    `  accounts=${files['asign.json'].caiyun.length}  state=${Object.keys(files['state.json'].accounts).length}` +
      `  runs=${history.runs.length}  pipelines=${files['pipelines.json'].pipelines.length}` +
      `  schedules=${files['schedules.json'].schedules.length}` +
      `  bindings=${files['exchange.json'].bindings.length}`,
  )
  console.log(`  手机号分配：${JSON.stringify(meta.$phoneAssignment.$assigned)}（原号码不落盘）`)
  console.log(`  userDomainId 分配：${JSON.stringify([...domainIdMap.values()])}（原值不落盘）`)
  console.log(
    `  自检：在全部输出（含 $meta.json）中搜索 ${originalSecrets.size} 个原始敏感值，均未命中`,
  )
  console.log(`  自检：全部手机号 / userDomainId 均为 199 前缀合成值`)
}

/**
 * 硬保险：把原件里的敏感值逐个拿去搜**全部输出**（7 个数据文件 + `$meta.json`）。
 *
 * 只检查长度 ≥ 8 的值 —— 短值（如 "2"）会大量误命中，没有判别力。
 *
 * 调用方必须把 `$meta.json` 也放进 `rendered`：元信息是输出的一部分，
 * 而且是最容易"顺手写下原值以便对照"的地方。漏掉它等于给泄漏留了一扇后门。
 */
function assertNoSecretLeak(rendered) {
  const haystack = [...rendered.values()].join('\n')
  const leaked = []
  for (const secret of originalSecrets) {
    if (secret.length >= 8 && haystack.includes(secret)) leaked.push(secret)
  }
  if (leaked.length) {
    throw new Error(
      `夹具自检失败：输出中仍能找到 ${leaked.length} 个原始敏感值，已中止（未写盘）。\n` +
        leaked.map((s) => `  len=${s.length} ${JSON.stringify(s.slice(0, 12))}…`).join('\n') +
        `\n请检查本脚本的脱敏规则。`,
    )
  }
}

/**
 * 正向不变量：输出里的手机号 / userDomainId 必须都是**合成值**（199 前缀）。
 *
 * 为什么在"搜原值"之外还要有这一条：搜原值只能抓到**我们已经知道的原值**，
 * 全靠 `originalSecrets` 这个名单。旧版 userDomainId 正是栽在这里 ——
 * 没人把它登记进名单，于是原值落盘而自检一路放行。**扫不到，等于没扫**。
 *
 * 正向断言不看名单、只看形状，所以能兜住"以后新冒出来的标识字段"。
 * 与 Rust 侧的 `test_support::assert_only_synthetic_identifiers` 是同一条不变量，
 * 一个守在生成侧（写盘前），一个守在消费侧（夹具进版本库后）。
 */
function assertOnlySyntheticIdentifiers(rendered) {
  // 两侧都要求非字母数字，避免把 sha256 十六进制串里的数字段误判成标识。
  const SHAPES = [
    ['手机号', /(^|[^0-9A-Za-z])(1\d{10})([^0-9A-Za-z]|$)/g],
    ['userDomainId', /(^|[^0-9A-Za-z])(1\d{18})([^0-9A-Za-z]|$)/g],
  ]
  const bad = []
  for (const [name, text] of rendered) {
    for (const [kind, re] of SHAPES) {
      for (const m of text.matchAll(re)) {
        if (!m[2].startsWith('199'))
          bad.push(`${name}: ${kind} ${m[2]} 不是 199 前缀（疑似真实值）`)
      }
    }
  }
  if (bad.length) {
    throw new Error(
      `夹具自检失败：发现 ${bad.length} 个疑似真实标识，已中止（未写盘）。\n` +
        bad.map((s) => `  ${s}`).join('\n') +
        `\n合成值必须是 199 前缀。请检查脱敏规则是否漏了某个字段。`,
    )
  }
}

main()
