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
 *   node scripts/gates-web.mjs           # 默认：typecheck + build + lite 冒烟（ready 一个认证态）≈ 33 s
 *   node scripts/gates-web.mjs --full    # 提交前：三个认证态全跑（ready / login / setup）≈ 77 s
 *
 * 为什么默认只跑 ready：`interact` 的三个认证态里 ready 占了一半以上时间（27 s / 8 s / 10 s），
 * 而改脚本时的快速回路不该等一分钟。提交前用 `--full`（约定 §9.2 要的那条）。
 *
 * ⚠ 从**任何目录**调用都行：路径全部相对本文件算（曾经写死相对 `target`，
 * 不在仓库根跑就会把日志写到别处、并诡异地"找不到 web"）。
 * 日志：<仓库根>/target/gates-web-<name>.log 与 target/gates-web-last.log
 */
import { spawnSync } from 'node:child_process'
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
  {
    name: 'lite 真 DOM 验收',
    // all.mjs 的默认就是"三个认证态全跑"，所以 --full 只需**不传** --quick
    cmd: 'node',
    args: ['lite/regress/all.mjs', '--skip-build', ...(process.argv.includes('--full') ? [] : ['--quick'])],
    browser: true,
  },
]

fs.mkdirSync(path.join(ROOT, 'target'), { recursive: true })
const results = []

for (const gate of GATES) {
  process.stdout.write(`── ${gate.name} … `)
  const t = Date.now()
  if (gate.browser && !hasBrowser()) {
    console.log(`⊘ 跳过（本机没有无头 Chrome，这条**不算通过**）`)
    results.push({ name: gate.name, ok: true, skipped: true })
    continue
  }
  const r = spawnSync(gate.cmd, gate.args, { cwd: WEB, encoding: 'utf8' })
  const out = `${r.stdout ?? ''}${r.stderr ?? ''}`
  fs.writeFileSync(path.join(ROOT, 'target', `gates-web-${gate.name.replace(/[^0-9a-zA-Z]+/g, '-')}.log`), out)
  const ok = r.status === 0
  results.push({ name: gate.name, ok })
  console.log(`${ok ? '✓' : '✗'} (${((Date.now() - t) / 1000).toFixed(1)}s)`)
  if (!ok) {
    console.log(out.split('\n').slice(-30).join('\n'))
    break
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
