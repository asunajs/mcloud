#!/usr/bin/env node
// 前端（web/）门禁一键跑 —— `scripts/gates-go.sh` 的 web 版。
// 2026-09-22 由 bash 版逐条迁移而来：横幅文案、日志落盘约定、退出码保持一致。
//
// 为什么要有这个文件：2026-09-22 之前**整个仓库对 web 侧一条自动检查都没有**。
// `package.json` 里有 `typecheck` 与 `build` 两个脚本，但没有任何 hook / 脚本调它们。
// ⇒ 改坏一个 `.tsx` 不会让任何东西变红：它既不过类型检查，也不过打包，
//   直到下一次有人手跑 `npm run build` 才撞上 —— 而那时早就说不清是哪一笔改坏的。
//
// # 为什么不复用 gates-go.sh 那套循环
//
// 那 25 行 runner（`GATE_NAMES` / 逐条落盘 / 末尾"本次 N/M 条"横幅）是**通用**的，
// 但它外面包着全是 Go 专有的东西：`GO_PKGS`、`-race` 探测、`gofmt -l ./cmd ./internal`。
// 所以：runner 的形状照抄（同一套日志落盘与横幅约定，排障时只有一套肌肉记忆），
// 内容各自一份。⚠ 代价就是"两份 runner 会长歪"，看这段注释的人请一起检查。
//
// # 两条各拦什么
//
//   typecheck  `tsmc --noEmit`（ts-macro 的 CLI：先展开 `.tsx` 里的宏再查类型）
//   build      `vite build`    （Tailwind/daisyUI 的 CSS 管线、模块解析、产物落盘）
//
// ⚠ `typecheck` 通过 **不等于** 页面是对的：web 侧没有测试运行器
// （方案 §6.1 L1' 要的 vitest 至今未装，见 `docs/webui-code-guide.md` §12 未决项 3），
// 所以这两条只保证"编得出来"，不保证"跑得对、长得对"。
// 那条缺口只能靠真机核验补，别把这里当成功能正确性的证明。
//
// （bash 版还有一条"这台机器没有 node"的探测；本脚本本身就是 node 进程，
//   那条判据恒假，迁移时删掉 —— 环境前置收敛为根 `package.json` 的 engines。）
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { cd } from 'zx'

cd(new URL('..', import.meta.url).pathname)

const GATE_NAMES = ['typecheck', 'build']
const ALL = [...GATE_NAMES]
const WEB_DIR = 'web'

const gateRun = (name) => {
  switch (name) {
    case 'typecheck':
      return ['npm', ['--prefix', WEB_DIR, 'run', 'typecheck']]
    case 'build':
      return ['npm', ['--prefix', WEB_DIR, 'run', 'build']]
    default:
      // 与 gates-go.sh 同一条：登记了却没实现分支，得说清问题在脚本这张表上，
      // 而不是让人去查这条门禁本身。
      console.error(`!! 门禁 "${name}" 登记在 GATE_NAMES 里，但 gateRun 没有它的实现分支`)
      return null
  }
}

// 环境探测：先说清"这台机器缺什么"，再谈门禁红不红。
// ⚠ 判据来自 gates-go.sh 里 `-race` 那一节的同一条：环境问题被翻译成
//   "你的代码有问题"，会让人去查一行没错的代码。
//   但这里**不降级**：缺 node_modules 时 `tsc` 报的是几百条"找不到模块"，
//   那既不是门禁想说的话，也不是能一眼修掉的错 ⇒ 直接拦住并给出那条命令。
const envProblems = []
if (!fs.existsSync(path.join(WEB_DIR, 'node_modules'))) {
  envProblems.push(
    `!! 缺 ${WEB_DIR}/node_modules：先跑一次「cd ${WEB_DIR} && npm install」再提交。`,
    '   （依赖没装时 typecheck 会吐几百条「找不到模块」，那不是本门禁想报的错。）',
  )
}

const argv = process.argv.slice(2)
const order = argv.length > 0 ? argv : [...GATE_NAMES]

for (const name of order) {
  if (!ALL.includes(name)) {
    console.log(`!! 未知门禁: ${name}（可选：${GATE_NAMES.join(' ')}）`)
    process.exit(2)
  }
}

if (envProblems.length > 0) {
  for (const line of envProblems) console.error(line)
  console.log('\x1b[31mweb 门禁没跑起来：环境缺东西（上面那句就是原因）\x1b[0m')
  process.exit(1)
}

fs.mkdirSync('target', { recursive: true })
const LOG = 'target/gates-web-last.log'
fs.writeFileSync(LOG, '')

const failed = []
// 耗时提示：与 gates-go.js 同款（每条结果行带自己的墙钟，末尾给总耗时）。
// 两处各自一份 fmtDur 是**故意的** —— 为 6 行共用一个小模块文件不值当。
const fmtDur = (ms) => {
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  const t = Math.round(ms / 1000)
  return `${Math.floor(t / 60)}m${String(t % 60).padStart(2, '0')}s`
}
const runT0 = Date.now()
const durations = []
for (const name of order) {
  console.log(`\n\x1b[1m──── web:${name} ────\x1b[0m`)

  const spec = gateRun(name)
  if (spec === null) {
    failed.push(name)
    continue
  }

  // 与 bash 的 `>"$RUN_LOG" 2>&1` 同构：两个流都并进每门禁自己的日志文件，
  // 再整份转印到终端 —— 前面门禁的输出不会污染这条的诊断（同 gates-go 的约定）。
  const RUN_LOG = `target/gates-web-${name}.log`
  const gateT0 = Date.now()
  const fd = fs.openSync(RUN_LOG, 'w')
  const [cmd, args] = spec
  const r = spawnSync(cmd, args, { stdio: ['ignore', fd, fd] })
  fs.closeSync(fd)
  const dur = fmtDur(Date.now() - gateT0)
  durations.push(`${name} ${dur}`)

  const content = fs.readFileSync(RUN_LOG, 'utf8')
  process.stdout.write(content)
  fs.appendFileSync(LOG, content)

  if (r.status === 0) {
    console.log(`\x1b[32m✓ ${name} 通过（${dur}）\x1b[0m`)
  } else {
    console.log(`\x1b[31m✗ ${name} 失败（${dur}）\x1b[0m`)
    failed.push(name)
  }
}

if (failed.length > 0) {
  console.log(`\x1b[31mweb 门禁未通过: ${failed.join(' ')}\x1b[0m`)
  console.log(`完整输出: ${LOG}`)
  console.log(`总耗时 ${fmtDur(Date.now() - runT0)}（逐条：${durations.join('，')}）`)
  process.exit(1)
}

// ── 横幅：与 gates-go.sh 同一条判据 ──────────────────────────────────
//
// 跑子集时**不许**打"全绿"。这条在 Go 侧是修过的真事故（早先 `gates-go.sh fmt`
// 也报"九项门禁全绿"），这里从一开始就按修好的写法来。
if (order.length === ALL.length) {
  console.log(`\x1b[32mweb 门禁 ${ALL.length} 条全绿\x1b[0m`)
} else {
  console.log(
    `\x1b[32mweb 门禁通过：${order.join(' ')}（本次 ${order.length}/${ALL.length} 条）\x1b[0m`,
  )
  for (const g of ALL) {
    if (!order.includes(g)) console.log(`\x1b[33m⚠ 未跑：${g}\x1b[0m`)
  }
  console.log('\x1b[33m  ⇒ 别把这当成"全绿"：上面这几条**这次没验**。\x1b[0m')
}
console.log(`总耗时 ${fmtDur(Date.now() - runT0)}（逐条：${durations.join('，')}）`)

// ⚠ 与 Go 侧的一处**故意**不同：这里不清空 $LOG。
//   gates-go.sh 末尾那句 `: > "$LOG"` 让"成功那次的输出"不留在盘上，
//   于是 target/gates-go-last.log 实际是"上一次**失败**的现场"（名字没说这件事）。
//   这里留着，是为了让"这次 web 门禁验了什么"在提交之后还能回看 ——
//   要跟着 Go 侧统一成哪一个，是另一个话题，别顺手改齐。
