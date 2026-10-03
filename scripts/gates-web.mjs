#!/usr/bin/env node
/**
 * Web 门禁：**编译两条 + lite 编译期负例一条 + lite 真 DOM 验收一条**。
 *
 * 为什么加后两条：原来只有 `typecheck` + `build`，那两条只证明"能编译"，
 * 不证明页面正确 —— lite 的行为靠 `web/lite/regress/`（真 DOM 交互）与
 * `web/lite/demo/`（编译产物断言）把，而它们此前**只能手敲**，
 * 于是"提交前全绿"里的前端部分实际上是没门的。
 *
 * ⚠ 没浏览器就**明说跳过**，不算通过。`regress` 与 `demo` 要无头 Chrome
 * （本机 puppeteer 缓存里那份），CI 的 ubuntu runner 上没有 —— 那一条会显示
 * `skipped(无浏览器)`，摘要里看得见。"跳过 ≠ 通过"是本项目的一贯口径。
 *
 * 用法：
 *   node scripts/gates-web.mjs           # 默认：typecheck + build + lite 冒烟 + 窄屏（后两条并行）≈ 110 s
 *   node scripts/gates-web.mjs --full    # 提交前：三个认证态全跑 + 泄漏量测（三条并行）≈ 143 s（2026-10-03 实测 2m22.8s）
 *
 * 为什么默认只跑 ready：`interact` 的三个认证态里 ready 占了一半以上时间（27 s / 8 s / 10 s），
 * 而改脚本时的快速回路不该等一分钟。提交前用 `--full`（约定 §9.2 要的那条）。
 *
 * ⚠ 从**任何目录**调用都行：路径全部相对本文件算（曾经写死相对 `target`，
 * 不在仓库根跑就会把日志写到别处、并诡异地"找不到 web"）。
 * 日志：<仓库根>/target/gates-web-<name>.log 与 target/gates-web-last.log
 */
import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const ROOT = path.resolve(new URL('.', import.meta.url).pathname, '..')
const WEB = path.join(ROOT, 'web')

/** 有没有可用的无头 Chrome（只看本机 puppeteer 缓存，不去联网下载）。 */
const hasBrowser = () => {
  const base = path.join(os.homedir(), '.cache', 'puppeteer', 'chrome-headless-shell')
  try {
    for (const v of fs.readdirSync(base)) {
      const dir = path.join(base, v)
      try {
        for (const sub of fs.readdirSync(dir)) {
          if (sub.startsWith('chrome-headless-shell-') && fs.existsSync(path.join(dir, sub, 'chrome-headless-shell'))) return true
        }
      } catch {
        /* 版本目录里还有半截下载，跳过 */
      }
    }
  } catch {
    /* 目录不存在 = 没装 */
  }
  return false
}

const GATES = [
  { name: 'typecheck', cmd: 'npm', args: ['run', 'typecheck'] },
  { name: 'build', cmd: 'npm', args: ['run', 'build'] },
  // 编译期负例：纯 Node，CI 也跑得动 —— 该抛的没抛就是编译器的门坏了
  { name: 'lite 编译期负例', cmd: 'node', args: ['lite/regress/compiler.mjs'] },
  // 窄屏（375，真视口）：宽屏那条抓不到"手机上没法用"这一类（2026-10-01 用户连报两个）
  // ⚠ 名字里留一段 ASCII：日志文件名按 `[^0-9a-zA-Z]+` 清洗，纯中文名会退化成
  // `gates-web--.log`（跟别的门禁分不清）。
  { name: 'narrow-375 窄屏布局验收', cmd: 'node', args: ['lite/regress/narrow.mjs'], browser: true },
  {
    name: 'lite 真 DOM 验收',
    // all.mjs 的默认就是"三个认证态全跑"，所以 --full 只需**不传** --quick
    cmd: 'node',
    args: ['lite/regress/all.mjs', '--skip-build', ...(process.argv.includes('--full') ? [] : ['--quick'])],
    browser: true,
  },
  /**
   * ⚠ 只在 `--full`（提交前那次）跑：要无头 Chrome 走 8 个页面 × 2 轮。
   * **实测 126 s**（预热 8 页 + 2 轮 + 浏览器启动）—— 是六条里最慢的一条 ✓，
   * 所以默认那条快回路（改脚本用）不加它 ✓。
   *
   * 为什么非有不可：这一类泄漏**功能上完全看不出来** —— 页面全对、别的门禁全绿，
   * 只是内存与 DOM 节点一路涨 ✗。2026-10-02 就是这么漏到用户眼前的（用户报「4M → 8M」）：
   * 当时只有一份要**真会话 cookie** 的脚本 ✗（`/tmp/mcloud-cookie.txt`），别人复现不了 ✗。
   * 现在走 `--fixture` ✓（夹具后端 + 内核分配端口，不碰 3000、不要 cookie ✓）。
   */
  ...(process.argv.includes('--full')
    ? [
        {
          name: '前端泄漏量测',
          cmd: 'node',
          args: ['lite/regress/leak.mjs', '--fixture', '--rounds', '2'],
          browser: true,
        },
      ]
    : []),
]

fs.mkdirSync(path.join(ROOT, 'target'), { recursive: true })
const results = []

/** 把一条门禁的输出落盘（名字里的非字母数字按老口径清洗成 `-`）。 */
const logPath = (gate) =>
  path.join(ROOT, 'target', `gates-web-${gate.name.replace(/[^0-9a-zA-Z]+/g, '-')}.log`)

/**
 * 跑一条门禁。同步版给"必须先于浏览器那几条"的那几条用 ——
 * `build` 得先出 `web/dist`，浏览器的三条才有东西可服务 ✗ 别并行到一起去。
 */
function runSync(gate) {
  const t = Date.now()
  process.stdout.write(`── ${gate.name} … `)
  if (browserMissing(gate)) return
  const r = spawnSync(gate.cmd, gate.args, { cwd: WEB, encoding: 'utf8' })
  const out = `${r.stdout ?? ''}${r.stderr ?? ''}`
  fs.writeFileSync(logPath(gate), out)
  const ok = r.status === 0
  results.push({ name: gate.name, ok })
  console.log(`${ok ? '✓' : '✗'} (${((Date.now() - t) / 1000).toFixed(1)}s)`)
  if (!ok) console.log(out.split('\n').slice(-30).join('\n'))
  return ok
}

/**
 * 并行版：给三条要无头 Chrome 的门禁用。
 *
 * # 为什么可以并行（2026-10-03 实测省下约 2 分钟）
 *
 * 它们**互不依赖**：每条自己起夹具后端（`serve(..., 0)`，端口由内核分配、不碰 3000）
 * 与自己的 Chrome profile ✓，读的只是已经构建好的 `web/dist` ✓。
 * 串行时是 30.9 + 84.6 + 127.0 = **242 s**，并行后约等于最慢那条（**127 s**）。
 *
 * ⚠ 三条都**不是靠计时**断言的（数堆/数节点/量计算样式 + 固定 dwell），
 * 所以互相抢 CPU 不会让它们变脆 ✓ —— 但真机只 2 核时别并行，那时抢 CPU 反而拖慢。
 */
function runParallel(gate) {
  return new Promise((resolve) => {
    const t = Date.now()
    if (browserMissing(gate)) return resolve()
    const child = spawn(gate.cmd, gate.args, { cwd: WEB, encoding: 'utf8' })
    let out = ''
    child.stdout.on('data', (d) => (out += d))
    child.stderr.on('data', (d) => (out += d))
    child.on('close', (code) => {
      fs.writeFileSync(logPath(gate), out)
      const ok = code === 0
      results.push({ name: gate.name, ok })
      console.log(`── ${gate.name} … ${ok ? '✓' : '✗'} (${((Date.now() - t) / 1000).toFixed(1)}s)`)
      if (!ok) console.log(out.split('\n').slice(-30).join('\n'))
      resolve()
    })
  })
}

/** 要浏览器的那条在本机没浏览器就"跳过 ≠ 通过"，记一条 skipped 免得摘要里当成功。 */
function browserMissing(gate) {
  if (!gate.browser || hasBrowser()) return false
  console.log('⊘ 跳过（本机没有无头 Chrome，这条**不算通过**）')
  results.push({ name: gate.name, ok: true, skipped: true })
  return true
}

// ① 顺序跑"不依赖浏览器"的那几条：typecheck / build / lite 编译期负例。
//    `build` 必须在浏览器的三条之前 —— 它们服务的是它产出的 `web/dist` ✗。
const serial = GATES.filter((g) => !g.browser)
const parallel = GATES.filter((g) => g.browser)
let failedEarly = false
for (const gate of serial) {
  if (runSync(gate) === false) {
    failedEarly = true
    break
  }
}

// ② 浏览器那几条并行。
if (!failedEarly) {
  const cores = os.cpus().length
  if (parallel.length > 1 && cores >= 4) {
    console.log(`（下面 ${parallel.length} 条并行跑：互不依赖，各自起夹具与浏览器）`)
    await Promise.all(parallel.map(runParallel))
  } else {
    for (const gate of parallel) runSync(gate)
  }
}

const failed = results.filter((r) => !r.ok)
const summary = results.map((r) => `${r.name}: ${r.skipped ? 'skipped(无浏览器)' : r.ok ? 'pass' : 'FAIL'}`).join(' | ')

if (failed.length > 0) {
  fs.writeFileSync(path.join(ROOT, 'target', 'gates-web-last.log'), `FAILED — ${summary}\n`)
  console.error(`\n门禁未通过：${summary}`)
  process.exit(1)
}

// 全绿时清掉 last 日志：别拿它的空不空当"跑没跑"的证据
fs.rmSync(path.join(ROOT, 'target', 'gates-web-last.log'), { force: true })
const skipped = results.filter((r) => r.skipped).map((r) => r.name)
console.log(`\n全绿（${results.length} 条）：${summary}`)
if (skipped.length) console.log(`⚠ 其中 ${skipped.join('、')} 被跳过 —— 那部分**没有**被验证过。`)
