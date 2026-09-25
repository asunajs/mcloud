#!/usr/bin/env node
/**
 * Web 门禁：与 mcloud 一致 —— **只有两条**（typecheck + build）。
 *
 * 为什么是这两条：本项目没有引入测试运行器（mcloud 侧同样没有，见其未决项 3）。
 * 所以 `typecheck` + `build` 只证明"能编译"，**不证明页面正确** ——
 * 页面正确性必须真机（或浏览器）核验，不能写成"有测试了"。
 *
 * 用法：node scripts/gates-web.mjs
 * 日志：target/gates-web-<name>.log 与 target/gates-web-last.log
 */
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

const GATES = [
  { name: 'typecheck', args: ['run', 'typecheck'] },
  { name: 'build', args: ['run', 'build'] },
]

fs.mkdirSync('target', { recursive: true })
const results = []

for (const gate of GATES) {
  process.stdout.write(`── ${gate.name} … `)
  const r = spawnSync('npm', gate.args, { cwd: 'web', encoding: 'utf8' })
  const out = `${r.stdout ?? ''}${r.stderr ?? ''}`
  fs.writeFileSync(path.join('target', `gates-web-${gate.name}.log`), out)
  const ok = r.status === 0
  results.push({ name: gate.name, ok })
  console.log(ok ? '✓' : '✗')
  if (!ok) {
    console.log(out.split('\n').slice(-30).join('\n'))
    break
  }
}

const failed = results.filter((r) => !r.ok)
const summary = results.map((r) => `${r.name}: ${r.ok ? 'pass' : 'FAIL'}`).join(' | ')

if (failed.length > 0) {
  fs.writeFileSync('target/gates-web-last.log', `FAILED — ${summary}\n`)
  console.error(`\n门禁未通过：${summary}`)
  process.exit(1)
}

// 全绿时清掉 last 日志：别拿它的空不空当"跑没跑"的证据
fs.rmSync('target/gates-web-last.log', { force: true })
console.log(`\n全绿（${results.length} 条）：${summary}`)
