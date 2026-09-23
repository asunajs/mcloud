#!/usr/bin/env node
// Markdown 表格：**一行不能比表头多格**（多出来的格会被渲染器静默丢掉）。
//
// 为什么值得一条门禁：多出来的格在渲染时被**静默丢掉**（markdown-it 会把行截断到
// 表头宽度），而源文件里内容**都在**（grep 找得到）⇒ **"文件里有"≠"读得到"**。
// 本仓库最承重的那些判据全在 `docs/migration-pitfalls.md` 的表格里，被丢掉的又往往是
// **最后一列「做法」** —— 丢的正是"该怎么做"。
//
// ⚠⚠ **只管"多"这一个方向，是有意的**：格数**少于**表头时 GFM 会**补空格子**，
// **不丢内容**（行仍然是坏的，但那是**另一类**缺陷：漏了一列 / 行被硬换行拆成两行 ——
// 后者与"格数"根本不是同一个判据）。把两个方向塞进一条门禁，只会让这条门禁的
// 判据与它要防的伤害脱钩 —— 那正是坑表 #127 记的错。
// 已查到的"少于"那几处（`docs/caiyun-go-parameters-and-decisions.md` 两行、
// `docs/webui-code-guide.md` 一行被硬换行拆开）记在 `docs/status.md` §九，**未改**。
//
// ⚠⚠ **判据必须在源文件上，不能在渲染结果上**：渲染后每行都"等于"表头（因为被截断了），
// 所以"数渲染出来的 `<td>`"这个办法**永远看不出**这个缺陷。坑表 #124 记的就是这件事。
//
// ⚠ 计数必须按**转义规则**切（`(?<!\\)\|`）：格子里（**含行内代码**）的字面竖线要写成
// `\|`，不转义就会被当成分隔符 —— 于是"这一行自己"被多切几格、末几列被丢掉。
// ⚠ 别用 `&#124;` 代替：行内代码里实体不解析，会原样显示出来（实测过）。
//
// 已知缺陷（**有意豁免，见坑表 #124**）：`docs/migration-pitfalls.md` 里
// `#15–#27` 与 `#72–#74` 共 16 行多一个 `#` 列（表头是 `现象|根因|做法` 三格）。
// ⚠ 豁免**不按行号**（行号会漂）：按"那一行第一个格的编号"认，并且**反向检查** ——
// 豁免名单里某条若已经不存在了，本门禁同样**失败**（过期豁免比没有豁免更坏，见 #116）。
'use strict'

const { spawnSync } = require('node:child_process')
const fs = require('node:fs')
const path = require('node:path')

const ROOT = path.resolve(__dirname, '..')

// ── 扫描面 = **git 眼里的仓库内容**，不是手写清单 ──────────────────────
//
// ⚠⚠ 早先这里是手写的 `['AGENTS.md', 'README.md'] + docs/**`，而"手维护的清单"
//   正是本仓库点名过三次的坑（`TEXT_EXT` 漏过 `.go` / `.py` / `.tsx` / `.csv`；
//   `check-identifiers.cjs` 的 `ALLOWED` 曾恒真）。手写清单的失效方式永远是
//   **漏掉的不自报** —— `THIRD-PARTY-NOTICES.md`（顶层、手写、入库）与
//   `.githooks/README.md` 就都落在清单外，加门禁那一轮没人发现。
//   改用 `git ls-files`（与 `check-filenames.js` 同一约定）⇒ 新增一个 `.md`
//   自动进范围，**不需要谁记得来登记**。
//
// 排除项交给 `.gitignore`（`--exclude-standard`）—— 那是"什么算仓库内容"的**唯一**
//   出处，于是"能不能被扫"与"能不能被提交"对齐（`.gitignore` 里那两条注释说的就是这件事：
//   记忆 / 计划目录被忽略 ⇒ 也就不再被守卫扫）。⚠ 别改回 fs 遍历：那样 `.workbuddy-ai/`
//   下的记忆文件会重新落进扫描面，正好把 `.gitignore` 那条修好的事又弄坏。
//
// 唯一的额外排除是随仓库共享的**第三方技能目录**（VENDOR_DIRS），理由与
//   `check-identifiers.cjs` 的 `VENDOR_SOURCE_PATHS` 同款：那是上游内容、逐字取自
//   daisyUI，改了也会被下次更新覆盖。排除量**会打印出来**，不静默。
const VENDOR_DIRS = ['.workbuddy/skills/']

const git = spawnSync(
  'git',
  ['-c', 'core.quotePath=false', 'ls-files', '--cached', '--others', '--exclude-standard', '*.md'],
  { cwd: ROOT, encoding: 'utf8' },
)
// ⚠ 拿不到清单就**红**，不许退化成"0 个文件、通过"：静默跳过与"跑了且绿"在终端上
//   长得一模一样（同 `gates-scope.js` 那条"分类器不确定时全跑"的反面 —— 这里没有
//   "多验"的余地，因为判据的**输入**本身没了）。
if (git.status !== 0) {
  console.error('✗ 拿不到仓库文件清单（git ls-files 失败）—— 本门禁的扫描面无从确定。')
  console.error(`  ${(git.stderr || '').trim() || '(git 没有输出)'}`)
  process.exit(1)
}

const allMd = git.stdout.split('\n').filter(Boolean)
const FILES = allMd.filter((f) => !VENDOR_DIRS.some((d) => f.startsWith(d)))
const vendorSkipped = allMd.length - FILES.length

// 豁免：文件 → 该行第一个格里的编号（不含 `#` 与星号）。
const EXEMPT = {
  'docs/migration-pitfalls.md': [15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 72, 73, 74],
}

const CELL = /(?<!\\)\|/g
const SEP = /^\s*\|[\s:\-|]+\|\s*$/
const LEADING_NUM = /^\|\s*\*\*(\d+)\*\*/

const cells = (line) => line.trim().split(CELL).length - 2

const problems = []
const seenExempt = new Set()

for (const rel of FILES.sort()) {
  const lines = fs.readFileSync(path.join(ROOT, rel), 'utf8').split('\n')
  let block = []
  const flush = () => {
    if (block.length > 0) {
      const head = cells(block[0][1])
      for (const [n, line] of block.slice(1)) {
        if (SEP.test(line)) continue
        if (cells(line) <= head) continue
        const m = LEADING_NUM.exec(line)
        const key = m ? `${rel}#${m[1]}` : null
        if (m && (EXEMPT[rel] || []).includes(Number(m[1]))) {
          seenExempt.add(key)
          continue
        }
        problems.push({ rel, n, head, got: cells(line), key })
      }
    }
    block = []
  }
  lines.forEach((line, i) => {
    if (line.trimStart().startsWith('|')) block.push([i + 1, line])
    else flush()
  })
  flush()
}

// 反向检查：豁免名单里每一条都必须**仍然**是一条真实的违规。
const stale = []
for (const [rel, nums] of Object.entries(EXEMPT)) {
  for (const num of nums) if (!seenExempt.has(`${rel}#${num}`)) stale.push(`${rel}#${num}`)
}

if (stale.length > 0) {
  console.error(`✗ 豁免名单过期：${stale.join(' ')}`)
  console.error('  这些行要么已被修好、要么编号变了 ⇒ 请从 scripts/check-md-tables.cjs 的 EXEMPT 里删掉。')
  console.error('  ⚠ 过期豁免比没有豁免更坏：它会**盖住**以后真正新增的同号缺陷（坑表 #116）。')
  process.exit(1)
}

if (problems.length > 0) {
  console.error(`✗ 有 ${problems.length} 行的格数**多于**表头 —— 多出来的格会被静默丢掉：`)
  for (const p of problems.slice(0, 20)) {
    console.error(`  ${p.rel}:${p.n}  表头 ${p.head} 格 / 本行 ${p.got} 格（多 ${p.got - p.head} 格）`)
  }
  if (problems.length > 20) console.error(`  …另有 ${problems.length - 20} 行`)
  console.error('')
  console.error('  修法：把格子里（**含行内代码里**）的字面竖线写成 `\\|` —— 渲染出来仍是干净的竖线。')
  console.error('  ⚠ 别用 `&#124;`（行内代码里实体不解析）。⚠ 别靠"插一个空行"或数渲染结果。')
  process.exit(1)
}

console.log(
  `markdown 表格门禁通过：${FILES.length} 个文件、没有一行比表头多格` +
    `（其中 ${seenExempt.size} 行按坑表 #124 豁免；⚠ 本门禁**不管**"少于表头"那类）。`,
)
// 覆盖范围**显式打印**（同 `dev-gates.md` 第 8 条那条"没有校验器的段会被打印出来"）：
// 排除项不出现在输出里，就等于"扫了 N 个文件，通过"—— 漏掉的那部分不会自报。
console.log(
  `  扫描面 = git ls-files（跟踪 + 未跟踪未忽略的 *.md）` +
    `；另有 ${vendorSkipped} 个第三方技能目录下的 .md 按 VENDOR_DIRS 排除` +
    `${VENDOR_DIRS.length > 0 ? `（${VENDOR_DIRS.join(' ')}）` : ''}`,
)
