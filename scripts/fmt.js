#!/usr/bin/env node
// 全局 js/ts 格式化 —— 仓库的 "fmt 工具"（2026-09-22）。
//
// 引擎是 **oxfmt**（Oxc 全家桶，devDependency）：全仓扫一遍 7.6s，
// 换 prettier 时要几分钟，慢到没人会在提交前跑 —— 一个不被跑的检查等于没有。
// 配置在 `.oxfmtrc.json`（semi:false / singleQuote / printWidth:100，与手写风格一致；
// ignorePatterns 覆盖产物目录、黄金向量与外来素材目录）。
//
// # md 为什么**默认不在**格式化范围（用户 2026-09-22 拍板）
//
//   oxfmt 支持 markdown，但它重排表格时按**字符数**补对齐空格 ——
//   中文全角实际占两列，算宽必错：跑一遍会把本仓库手工对齐的中文表格
//   在纯文本视图里弄得更歪（实测 dev-gates/credential-boundary 等 28 篇）。
//   引擎没有"只关表格对齐"的开关，所以 md 整体让位，等上游修全角宽度再纳入。
//   确需批量动 md 时用 `--md`（自担上面的后果），别进默认档。
//
// 用法：
//   node scripts/fmt.js            # 格式化（写盘），并报告改了哪些文件与耗时
//   node scripts/fmt.js --check    # 只查不写；有未格式化文件则退出码 1（可进门禁）
//   node scripts/fmt.js --list     # 只列出会被改动的文件
//   node scripts/fmt.js --md       # 把 md 也纳入（见上，默认不纳）
//   node scripts/fmt.js --check web/src   # 末尾参数限定扫描子路径
import process from 'node:process'
import { $, cd } from 'zx'

cd(new URL('..', import.meta.url).pathname)

const argv = process.argv.slice(2)
const has = (flag) => {
  const i = argv.indexOf(flag)
  return i >= 0 ? (argv.splice(i, 1), true) : false
}

const check = has('--check')
const list = has('--list')
const withMd = has('--md')
const extra = argv.filter((a) => !a.startsWith('-'))
if (extra.some((a) => !/^[A-Za-z0-9_./-]+$/.test(a))) {
  console.error('!! 子路径参数只接受普通路径字符（防把 glob 当模式注入）')
  process.exit(2)
}

const JS_TS = ['**/*.js', '**/*.cjs', '**/*.mjs', '**/*.ts', '**/*.tsx']
const MD = ['**/*.md']
const patterns = withMd ? [...JS_TS, ...MD] : JS_TS

const mode = check ? '--check' : list ? '--list-different' : '--write'

if (withMd && !check && !list) {
  console.log(
    '\x1b[33m⚠ --md：oxfmt 按字符数补表格对齐，中文全角列会算错宽度（详见本文件头注）。\x1b[0m',
  )
}

const t0 = Date.now()
// write 档的 oxfmt 只报统计不列文件名 ⇒ 先花 <1s 拿一份"会被改"的清单，
// 跑完才说得出"这次动了哪些"。check/list 档自己就列，不必多跑。
const pre =
  mode === '--write' ? await $`npx oxfmt --list-different ${patterns} ${extra}`.nothrow() : null
const r = await $`npx oxfmt ${mode} ${patterns} ${extra}`.nothrow()
const text = `${r.stdout}`
if (text !== '' && !text.endsWith('\n')) process.stdout.write(`${text}\n`)
else process.stdout.write(text)
const secs = ((Date.now() - t0) / 1000).toFixed(1)

if (r.exitCode !== 0 && r.exitCode !== 1) {
  // 1 是 --check/--list-different 的"有文件不一致"，不是引擎错误；其余都要响。
  console.error(`!! oxfmt 退出码 ${r.exitCode}（不是"未格式化"，是跑挂了）`)
  process.exit(r.exitCode ?? 1)
}

const listSrc = pre ? `${pre.stdout}` : `${r.stdout}`
const lines = listSrc
  .split('\n')
  .filter((l) => l.trim() !== '' && !/^(Finished|Error|Format issues|warning|Checking)/.test(l))
const changed = lines.length

if (check) {
  if (r.exitCode === 1) {
    console.log(`!! 有 ${changed} 个文件未格式化 —— 跑「node scripts/fmt.js」修。`)
  } else {
    console.log(`ok 全部已格式化（js/ts${withMd ? '+md' : ''}）`)
  }
} else if (list) {
  if (changed === 0) console.log('（没有会被改动的文件）')
} else if (changed > 0) {
  console.log(`已格式化 ${changed} 个文件（js/ts${withMd ? ' + md' : ''}）：`)
  for (const l of lines) console.log(`  ${l}`)
} else {
  console.log(`已格式化：无文件需要改动（js/ts${withMd ? ' + md' : ''}）`)
}
console.log(`耗时 ${secs}s`)
process.exit(r.exitCode === 1 && check ? 1 : 0)
