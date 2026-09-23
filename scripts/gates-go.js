#!/usr/bin/env node
// Go 侧门禁一键跑 —— 唯一现役实现。2026-09-22 由 bash 版（250 行）逐条迁移而来：
// 横幅文案、退出码、日志落盘约定（含成功时清空 last.log）保持一致。
//
// 十条：
//
//   build      "能编译"的最直接判据
//   vet        静态检查 —— ⚠ **覆盖很窄**，见下
//   fmt        gofmt
//   test       go test（**多一个 -race**，见下）
//   deps       scripts/check-go-deps.js
//   filenames  scripts/check-filenames.js
//   idents     scripts/check-identifiers.cjs
//   golden     scripts/check-golden-vectors.cjs
//   jsontags   scripts/check-go-jsontags.cjs
//   md-tables  scripts/check-md-tables.cjs（**仓库级**的文档表格检查，不是 Go 专属 ——
//              与 filenames / idents 同类；见 docs/migration-pitfalls.md #124）
//
// 用法：
//   node scripts/gates-go.js              # 全跑
//   node scripts/gates-go.js test deps    # 只跑指定几条
//
// ⚠ 为什么 test 必须带 -race：
//
//   Rust 时代 `&mut R rng` 保证同一随机源不被并发共享 —— 那是**编译期保证**。
//   Go 的 io.Reader 是引用语义，并发用同一个源是 data race，而它**不一定**
//   让测试变红：可能只是偶发地产生错误数据（签名对不上、deviceId 错一位），
//   于是被当成"上游抽风"。所以 -race 是那条保证在 Go 侧的替代品，不是可选项。
//
// ⚠ vet 不等于完整静态检查：
//
//   `go vet` 只覆盖"几乎肯定是错"的一小撮检查（printf 格式串、锁被复制、
//   无用的赋值…）。补齐要引 golangci-lint，而那是**外部依赖** —— 骨架阶段
//   刻意一个第三方依赖都不引（见 docs/go-migration-plan.md）。
//   **在那之前，不要把 vet 当完整静态检查用**：它过了不代表这一关过了。
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import process from 'node:process'
import { cd } from 'zx'

cd(new URL('..', import.meta.url).pathname)

// ⚠⚠ 这是**唯一**一份清单。下面的 `ORDER` 与末尾横幅都必须从它派生，
//   别再手写第二份 —— 早先 `ORDER` 是把清单逐字重抄一遍，于是
//   **加了门禁却忘了加进 `ORDER` ⇒ 它默认永远不跑，而且什么都不报**
//   （名字合法性校验只保证"名字合法"，不保证"它会被跑到"）。
const GATE_NAMES = [
  'build',
  'vet',
  'fmt',
  'test',
  'deps',
  'filenames',
  'idents',
  'golden',
  'jsontags',
  'md-tables',
]
const ALL = [...GATE_NAMES]

// ⚠ 为什么用显式包路径，而不是惯用的 `./...`：
//   `./...` 曾把 `legacy/target/`（7 GB 构建产物）也遍历一遍
//   —— legacy/ 已于 2026-09-21 剥离，该遍历问题不复存在；
//   显式包范围**保留**：它还顺带把"只测交付物"写进了定义。
//   代价是"新加一个顶层目录要记得改这里"。这个代价由 check-go-deps.js 的
//   「所有 .go 文件都必须在 cmd/ 或 internal/ 下」那条断言兜住 —— 漏不掉。
const GO_PKGS = ['./cmd/...', './internal/...']

// 与 bash 版同构：每条门禁把**全部**输出（含降级警告）写进自己的 RUN_LOG，
// 跑完整份转印 —— 前面通过的门禁不留字样污染后面的诊断。
//
// ⚠ 这里用 spawnSync 而不是 zx 的 `$`：要拿文件描述符接 stdout/stderr
//   （`stdio: ['ignore', fd, fd]`），而 `$` 是 promise 语义、且自带输出处理，
//   对不齐 bash 版"整份转印"的时序。捕获类的小命令走下面的 `capture`。
const runToLog = (fd, cmd, args, opts = {}) => {
  const r = spawnSync(cmd, args, { stdio: ['ignore', fd, fd], ...opts })
  if (r.error) {
    fs.writeSync(fd, `!! ${cmd} 无法执行：${r.error.message}\n`)
    return 1
  }
  return r.status ?? 1
}

// 捕获型命令（只要文本，不接 fd）。`out` 是 stdout+stderr 的合并，
// 对齐 bash 的 `2>&1`；只要 stdout 的场景（如 `go env CC`）用 `stdout` 字段。
const capture = (cmd, args) => {
  const r = spawnSync(cmd, args, { encoding: 'utf8' })
  const stdout = r.stdout ?? ''
  const stderr = r.stderr ?? ''
  return { stdout, stderr, out: stdout + stderr, ok: !r.error }
}

// gofmt -l 只**列出**未格式化的文件，退出码仍是 0 —— 所以不能直接当门禁用，
// 必须自己判"输出是否为空"。这一步很容易写错成 `gofmt -l . && echo ok`。
//
// ⚠ 范围与 GO_PKGS 对齐（不用 `.`）。
const gateFmt = (fd) => {
  const { out, ok } = capture('gofmt', ['-l', './cmd', './internal'])
  // 命令没跑起来时 out 是空的，但那**不是**"全部已格式化"—— 与 bash 版
  // `2>&1` 会把 "gofmt: command not found" 打进 out 从而判红的行为对齐。
  if (!ok || out.trim() !== '') {
    fs.writeSync(fd, `!! 以下文件未格式化（跑 'gofmt -w .' 可修）：\n${out}`)
    return 1
  }
  fs.writeSync(fd, 'ok gofmt：全部已格式化\n')
  return 0
}

// ── `-race` 跑不跑得了：先探测，再决定 ──────────────────────────────
//
// `go test -race` 在 **Windows** 上必须开 cgo，而 cgo 必须有一个 Go 能用的
// C 编译器（`go env CC`，默认 gcc）。两道缺一，go 报的是：
//     go: -race requires cgo; enable cgo by setting CGO_ENABLED=1
//     cgo: C compiler "gcc" not found: exec: "gcc": executable file not found
// 两条都**不是代码问题**，但它们会让 `test` 红 —— 而 `test` 一红，这台机器
// 一条提交都推不出去。（2026-09-20 实测：Windows 机器上没有 gcc。）
//
// ⚠ 这里是**降级**，不是静默放过：
//   · 降级会大声说 —— 结果行与末尾汇总行都标出来；
//   · 理由是：一个"拦住一切"的检查，最后会被 `--no-verify` 整体绕过，
//     连带把其余几条一起废掉 —— 那样的实际价值是 **0**。
//     宁可少一条保证但**看得见**，不要"文档写着有、实际一次没跑"。
//
// 装上 mingw-w64 / w64devkit（或任何能让 `go env CC` 命中的编译器）后，
// 这里自己会恢复成带 -race，不用改脚本。
// `go env CC` 只取 stdout（bash 版把 stderr 丢了）；探测与警告行共用这一份。
const goCC = () => capture('go', ['env', 'CC']).stdout.trim()

const detectRace = () => {
  const cc = goCC()
  if (cc === '') return false
  // 同 `command -v`：只要可执行文件找得到就算命中（退出码无所谓）。
  const probe = spawnSync(cc, ['--version'], { stdio: 'ignore' })
  return !probe.error
}

let raceAvailableCache = null
const raceAvailable = () => (raceAvailableCache ??= detectRace())

let raceSkipped = false

const gateTest = (fd) => {
  if (raceAvailable()) {
    // CGO_ENABLED 在没有 C 编译器时默认是 0，而 -race 要求 1 —— 显式设上。
    return runToLog(fd, 'go', ['test', '-race', ...GO_PKGS], {
      env: { ...process.env, CGO_ENABLED: '1' },
    })
  }
  raceSkipped = true
  const w = (line) => fs.writeSync(fd, `${line}\n`)
  w(`\x1b[33m⚠ -race 不可用：cgo 需要 C 编译器「${goCC()}」，本机没有。\x1b[0m`)
  w('\x1b[33m  ⇒ 本轮**不**做竞态检测（Rust 侧由 `&mut R` 在编译期保证的那条，\x1b[0m')
  w('\x1b[33m     在 Go 侧唯一替代品就是 -race —— 它现在没跑）。\x1b[0m')
  w('\x1b[33m  ⇒ 装 mingw-w64 / w64devkit 后自动恢复，无需改脚本。\x1b[0m')
  return runToLog(fd, 'go', ['test', ...GO_PKGS])
}

const gateRun = (fd, name) => {
  switch (name) {
    case 'build':
      return runToLog(fd, 'go', ['build', ...GO_PKGS])
    case 'vet':
      return runToLog(fd, 'go', ['vet', ...GO_PKGS])
    case 'fmt':
      return gateFmt(fd)
    case 'test':
      return gateTest(fd)
    case 'deps':
      return runToLog(fd, 'node', ['scripts/check-go-deps.js'])
    case 'filenames':
      return runToLog(fd, 'node', ['scripts/check-filenames.js'])
    case 'idents':
      return runToLog(fd, 'node', ['scripts/check-identifiers.cjs'])
    case 'golden':
      return runToLog(fd, 'node', ['scripts/check-golden-vectors.cjs'])
    case 'md-tables':
      return runToLog(fd, 'node', ['scripts/check-md-tables.cjs'])
    case 'jsontags':
      return runToLog(fd, 'node', ['scripts/check-go-jsontags.cjs'])
    default:
      // 名字已在跑之前按表校验过，走到这里说明**本表的实现分支漏了它**，
      // 即"登记了门禁但没实现"。别让它以 `✗ <名字> 失败` 收场 ——
      // 那会让人去查这条门禁本身，而问题在脚本这张表上。
      fs.writeSync(fd, `!! 门禁 "${name}" 登记在 GATE_NAMES 里，但 gateRun 没有它的实现分支\n`)
      return 127
  }
}

// 默认顺序 = 上面那份清单本身（派生，不重抄）。
let order = process.argv.slice(2).filter((s) => s !== '')
if (order.length === 0) order = [...GATE_NAMES]

// 校验门禁名：**在跑之前**按名字查表，不靠退出码。
//
// ⚠ 为什么不能沿用"退出码 127 = 未知门禁"（原写法）：
//   127 同时是 shell 的 "command not found" 退出码。于是
//   **Go 没装时**，`go build` 返回 127，脚本会打印
//       !! 未知门禁: build（可选：build vet fmt test deps filenames idents golden）
//   ——把"这台机器没有 Go"报成了"这个脚本写错了"，而且是**列着 build 自己
//   告诉你它不存在**，看上去自相矛盾，非常耗排查时间。（2026-09-20 实测。）
for (const name of order) {
  if (!GATE_NAMES.includes(name)) {
    console.log(`!! 未知门禁: ${name}（可选：${GATE_NAMES.join(' ')}）`)
    process.exit(2)
  }
}

const failed = []
// ── 耗时提示 ────────────────────────────────────────────────────────
// 每条门禁的结果行带自己的墙钟，末尾再给一个总耗时 —— "test 怎么这么久"
// 之前只能靠 `time` 包一层或看 go test 的输出，而 build/vet/deps 那几条
// 各自的耗时同样值得看见（慢的那条才是该先优化的那条）。
const fmtDur = (ms) => {
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  const t = Math.round(ms / 1000)
  return `${Math.floor(t / 60)}m${String(t % 60).padStart(2, '0')}s`
}
const runT0 = Date.now()

// 日志放 target/（已在 .gitignore 里），用 gates-go- 前缀与 Rust 时代区分开 ——
// 两套门禁的日志混在一个文件里，排障时会看错那一条的输出。
const LOG = 'target/gates-go-last.log'
fs.mkdirSync('target', { recursive: true })
fs.writeFileSync(LOG, '')

const durations = []
for (const name of order) {
  console.log(`\n\x1b[1m──── ${name} ────\x1b[0m`)

  // 每条门禁的输出单独一个文件：诊断只看本次这条说了什么。
  const RUN_LOG = `target/gates-go-${name}.log`
  const gateT0 = Date.now()
  const fd = fs.openSync(RUN_LOG, 'w')
  const status = gateRun(fd, name)
  fs.closeSync(fd)
  const dur = fmtDur(Date.now() - gateT0)
  durations.push(`${name} ${dur}`)

  const content = fs.readFileSync(RUN_LOG, 'utf8')
  process.stdout.write(content)
  fs.appendFileSync(LOG, content)

  if (status === 0) {
    if (name === 'test' && raceSkipped) {
      console.log(`\x1b[33m⚠ ${name} 通过（降级：未跑 -race，${dur}）\x1b[0m`)
    } else {
      console.log(`\x1b[32m✓ ${name} 通过（${dur}）\x1b[0m`)
    }
  } else {
    // 名字的合法性已在上面按表校验过，所以走到这里就是**真的失败了** ——
    // 不要再拿退出码去猜"是不是未知门禁"（127 撞车，见上面校验那段）。
    console.log(`\x1b[31m✗ ${name} 失败（${dur}）\x1b[0m`)
    failed.push(name)
  }
}

if (failed.length > 0) {
  console.log(`\x1b[31m门禁未通过: ${failed.join(' ')}\x1b[0m`)
  console.log(`完整输出: ${LOG}`)
  console.log(`总耗时 ${fmtDur(Date.now() - runT0)}（逐条：${durations.join('，')}）`)
  process.exit(1)
}
// ── 横幅必须说清**这次到底跑了哪几条** ─────────────────────────────────
//
// ⚠ 早先这里无条件打"九项门禁全绿"，于是只跑 1 条也报"9/9"。而 **pre-commit
//   跑的是子集**，所以**每一次提交**都打印一句"九项门禁全绿"。
//   这条横幅是要被抄进 `docs/status.md` 的（"门禁实况：九项门禁全绿"），
//   抄错一次就再也说不清 —— 与"绿只对**跑的那个版本**成立"是同一条。
//
// ⚠ 条数从 ALL 派生（不写字面量）；中文数字查表是为了保住文档里
//   已经在用的"十项门禁全绿"这个说法，超出表长就退回阿拉伯数字。
//   （上面 265–268 行引的是**当时**的横幅文案，属于历史，不随条数改。）
const CN_NUM = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九', '十', '十一', '十二']
const cnAll = ALL.length < CN_NUM.length ? CN_NUM[ALL.length] : String(ALL.length)

const raceNote = raceSkipped ? ' —— ⚠ 但 test 未跑 -race（本机缺 C 编译器）' : ''
const skipped = ALL.filter((g) => !order.includes(g))

if (skipped.length === 0) {
  console.log(`\x1b[32m${cnAll}项门禁全绿${raceNote}\x1b[0m`)
} else {
  console.log(
    `\x1b[32m门禁通过：${order.join(' ')}（本次 ${order.length}/${ALL.length} 条）${raceNote}\x1b[0m`,
  )
  console.log(`\x1b[33m⚠ 未跑：${skipped.join(' ')}\x1b[0m`)
  console.log('\x1b[33m  ⇒ 别把这当成"全绿"：上面这几条**这次没验**。\x1b[0m')
}
console.log(`总耗时 ${fmtDur(Date.now() - runT0)}（逐条：${durations.join('，')}）`)
// 成功时清空 last.log：于是它实际是"上一次**失败**的现场"（名字没说这件事）。
// ⚠ web 侧**故意不**清那份，两边别顺手对齐（见 gates-web.js 末尾）。
fs.writeFileSync(LOG, '')
