#!/usr/bin/env node
/**
 * Web 门禁：**两档**（2026-10-04 用户口径：「一种在 push 时再验证，时间长范围广；
 * 一种提交就验证必要的测试」）。
 *
 * * `--commit`（**默认就是它**）：提交档 —— 改完就能自查的最小集合。
 *   类型 + 构建 + 编译器负例 + 框架真 DOM 断言（demo）+ 编译产物行为护栏（bench）
 *   + **真页面点一遍**（交互验收 `ready` 一个认证态）。
 * * `--push`：推送档 —— 上面那档 **再加**三条"范围广/看不出来"的：
 *   320/375 两档窄屏布局、`login` / `setup` 两个认证态、前端泄漏量测。
 *   ⚠ 泄漏那条自己就占了绝大部分时间（9 个路由 × 2 轮 + 9 次浏览器启动），
 *   它就是"推送档为什么长"的**全部**原因 ✓。
 *
 * 两档的关系是**包含**：`--push` 跑的就是 `--commit` 的超集（同一条命令 + 三条）✓，
 * 不存在"跑了推送档反而漏了提交档某条"的情形 ✓。
 *
 * # 为什么这么分（而不是"提交时全跑"）
 *
 * 提交是高频动作：一档要等两分钟，人就会开始 `git commit --no-verify` ✗ ——
 * 那一档必须短到**愿意等**；而推送/合并是低频、影响面大，值得把"不看就发现不了"
 * 的那一类（内存与 DOM 泄漏、窄屏溢出、别的认证态）一次跑完 ✓。
 * ⇒ 分界不是"重要不重要"，而是"**贵不贵**"：便宜且常出错的留在提交档，
 * 贵的（泄漏量测）挪去推送档 ✓。
 *
 * # 用法
 *
 *   node scripts/gates-web.mjs            # = --commit（提交档）
 *   node scripts/gates-web.mjs --push     # 推送档（`--full` 是它的旧名，仍认）
 *
 * 装好钩子后这两档是自动的：`scripts/install-git-hooks.sh` 的 `pre-commit` 跑提交档、
 * `pre-push` 跑推送档 ✓（跳过一律用 `--no-verify`，跳了就是没验证过 ✓）。
 *
 * ⚠ 没浏览器就**明说跳过**，不算通过（`skipped(无浏览器)`），摘要里看得见 ——
 * 这是本项目的一贯口径：跳过 ≠ 通过。CI 的 ubuntu runner 上没有无头 Chrome，
 * 所以 CI 那条只覆盖不依赖浏览器的三条（见 `.github/workflows/ci.yml`）✓。
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

/**
 * 档位。`--full` 是**推送档的旧名**（2026-10-04 之前只有 `--full` 这一个开关，
 * 文档 / CI 注释 / release.yml 里都还留着它）⇒ 继续认，别再改回去 ✗。
 */
const MODE = process.argv.includes('--push') || process.argv.includes('--full') ? 'push' : 'commit'

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

/**
 * 门禁清单。`tier` = 从哪一档开始跑（`commit` 的两档都跑，`push` 只有推送档跑）。
 *
 * `browser: true` 的那几条性质：① 没有无头 Chrome ⇒ 印 `⊘ 跳过` 且**不算通过**；
 * ② 它们**并行**跑（彼此不依赖：各自起夹具后端、各自一个 Chrome profile，
 * 读的都是 `build` 刚产出的 `web/dist`）✓。
 */
const GATES = [
  // ── 提交档：不依赖浏览器的三条（**串行**，`build` 必须先出 `web/dist`）──
  { tier: 'commit', name: 'typecheck', cmd: 'npm', args: ['run', 'typecheck'] },
  { tier: 'commit', name: 'build', cmd: 'npm', args: ['run', 'build'] },
  // 编译期负例：纯 Node，CI 也跑得动 —— 该抛的没抛就是编译器的门坏了
  { tier: 'commit', name: 'lite 编译期负例', cmd: 'node', args: ['lite/regress/compiler.mjs'] },

  // ── 提交档：要浏览器的四条（**并行**）──
  // demo：lite 的**真 DOM 断言**（绑定/列表复用/插槽/钩子/条件分支…）—— 框架自己的门
  { tier: 'commit', name: 'lite 真 DOM 验收', cmd: 'node', args: ['lite/demo/run.mjs'], browser: true },
  // bench：编译产物的行为断言 + 一组很松的耗时护栏（列表从"搬"退化成"重建"时能响）
  { tier: 'commit', name: '编译产物行为 + 耗时护栏', cmd: 'node', args: ['lite/bench/run.mjs'], browser: true },
  // 交互验收 ready：**真页面**点一遍（9 条路由 + 各页的交互与断言）—— 提交档里最值钱的一条
  { tier: 'commit', name: '交互验收 ready', cmd: 'node', args: ['lite/regress/interact.mjs', '--variant', 'ready'], browser: true },
  /**
   * 大体量验收：**分页与窗口化只有数据量够大时才现形** ✓（2026-10-05 用户口径
   * 「任务记录分页，详细日志虚拟加载」）。
   *
   * 为什么非进**提交档**不可：`ready` 夹具只有 **4 条运行 / 60 行日志** —— 这个量级下
   * "历史页不翻页也放得下"、"60 行全铺也才 1200 个节点" ⇒ 分页写错、窗口化失效
   * **全都看不出来** ✗。而真实数据是 113 条运行 / 800 行日志 / 单次 600 行 ✓。
   * 它顺带把两类"会安静地坏掉"的坑做成钉子：① 页面反复重挂载（实测 2 秒 218 次
   * `/api/logs` ✗）；② 滚动位置被弹走/被钳歪（滚动锚定、渲染期读尺寸）✓。
   *
   * 实测 **≈29 s**，比同档的 `ready`（≈40 s）短 ⇒ 并行跑**不增加**墙钟时间 ✓。
   */
  { tier: 'commit', name: '大体量验收（分页 + 窗口化）', cmd: 'node', args: ['lite/regress/bulk.mjs'], browser: true },

  // ── 推送档追加：贵／"不看就发现不了"的三类 ──
  // 窄屏两档（320 / 375 真视口）：宽屏跑不到的"手机上没法用"那一类
  // ⚠ 名字里留一段 ASCII：日志文件名按 `[^0-9a-zA-Z]+` 清洗，纯中文名会退化成
  // `gates-web--.log`（跟别的门禁分不清）。
  { tier: 'push', name: 'narrow-320-375 窄屏布局验收', cmd: 'node', args: ['lite/regress/narrow.mjs'], browser: true },
  // 另外两个认证态：走的是**另一条代码路径**（登录闸门 / 建管理员向导），ready 态覆盖不到
  { tier: 'push', name: '交互验收 login', cmd: 'node', args: ['lite/regress/interact.mjs', '--variant', 'login'], browser: true },
  { tier: 'push', name: '交互验收 setup', cmd: 'node', args: ['lite/regress/interact.mjs', '--variant', 'setup'], browser: true },
  /**
   * 泄漏量测：**只在推送档**。要无头 Chrome 走 9 个路由 × 2 轮（实测 ≈127 s，
   * 是全部十条里最慢的一条，比其余九条加起来还长 ✓ 所以它不进提交档 ✗）。
   *
   * 为什么非有不可：这一类泄漏**功能上完全看不出来** —— 页面全对、别的门禁全绿，
   * 只是内存与 DOM 节点一路涨 ✗。2026-10-02 就是这么漏到用户眼前的（用户报「4M → 8M」）：
   * 当时只有一份要**真会话 cookie** 的脚本 ✗（`/tmp/mcloud-cookie.txt`），别人复现不了 ✗。
   * 现在走 `--fixture` ✓（夹具后端 + 内核分配端口，不碰 3000、不要 cookie ✓）。
   */
  { tier: 'push', name: '前端泄漏量测', cmd: 'node', args: ['lite/regress/leak.mjs', '--fixture', '--rounds', '2'], browser: true },
]

/** 本次要跑的那几条（提交档 + 推送档追加）。 */
const ACTIVE = GATES.filter((g) => g.tier === 'commit' || MODE === 'push')

fs.mkdirSync(path.join(ROOT, 'target'), { recursive: true })
const results = []

console.log(`=== 档位：${MODE === 'push' ? '推送档（--push）' : '提交档（--commit，默认）'} · ${ACTIVE.length} 条 ===`)

/** 把一条门禁的输出落盘（名字里的非字母数字按老口径清洗成 `-`）。 */
const logPath = (gate) =>
  path.join(ROOT, 'target', `gates-web-${gate.name.replace(/[^0-9a-zA-Z]+/g, '-')}.log`)

/**
 * 跑一条门禁。同步版给"必须先于浏览器那几条"的那几条用 ——
 * `build` 得先出 `web/dist`，浏览器那几条才有东西可服务 ✗ 别并行到一起去。
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
 * 并行版：给要无头 Chrome 的门禁用。
 *
 * # 为什么可以并行（2026-10-03 实测省下约 2 分钟）
 *
 * 它们**互不依赖**：每条自己起夹具后端（`serve(..., 0)`，端口由内核分配、不碰 3000）
 * 与自己的 Chrome profile ✓，读的只是已经构建好的 `web/dist` ✓。
 * 串行时是 30.9 + 84.6 + 127.0 = **242 s**，并行后约等于最慢那条 ✓
 * （推送档最慢的是泄漏量测 ⇒ 并行把这档钉在 ≈130 s 而不是 ≈250 s ✓）。
 *
 * ⚠ 它们**不是靠计时**断言的（数堆/数节点/量计算样式 + 固定 dwell），
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
//    `build` 必须在浏览器的那几条之前 —— 它们服务的是它产出的 `web/dist` ✗。
const serial = ACTIVE.filter((g) => !g.browser)
const parallel = ACTIVE.filter((g) => g.browser)
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
  fs.writeFileSync(path.join(ROOT, 'target', 'gates-web-last.log'), `FAILED [${MODE}] — ${summary}\n`)
  console.error(`\n门禁未通过（${MODE} 档）：${summary}`)
  process.exit(1)
}

// 全绿时清掉 last 日志：别拿它的空不空当"跑没跑"的证据
fs.rmSync(path.join(ROOT, 'target', 'gates-web-last.log'), { force: true })
const skipped = results.filter((r) => r.skipped).map((r) => r.name)
console.log(`\n全绿（${MODE} 档 ${results.length} 条）：${summary}`)
if (skipped.length) console.log(`⚠ 其中 ${skipped.join('、')} 被跳过 —— 那部分**没有**被验证过。`)
if (MODE === 'commit') console.log('⚠ 提交档不含窄屏 / login / setup / 泄漏量测 —— 推送前请跑 `--push` ✓。')
