#!/usr/bin/env node
// 按**这次暂存了什么**决定该跑哪几条门禁 —— `.githooks/pre-commit` 的分类器。
// 2026-09-22 由 bash 版逐条迁移而来：三行 `KEY=` 输出契约与退出码逐字保持。
//
// # 要解决的这件事是真的
//
// pre-commit 原先写死 `gates-go.sh fmt deps filenames idents jsontags` 五条。
// ⇒ 改一行 `docs/*.md` 也要过一遍要 Go 工具链的检查（`gofmt -l` / `deps` / `jsontags`）；
//   而改 `.tsx` 时**那五条一条都验不到前端**，前端那边当时还没有任何自动检查。
// 两件事合起来是同一句话：**闸口的内容与被改的东西无关**。
//
// # 为什么"按范围跳过"在这家是真的安全
//
// ⚠ 这一条是整个设计的前提，不成立就不要改这个文件：
//
//   1. **pre-push 不跳过。** 它跑的是全套（Go 九条 + web 两条），
//      所以"提交时按范围省下来的"必须在推送前补齐 —— 推才是影响别人的动作。
//      两道闸的分工见 `.githooks/README.md`。
//   2. **两条与语言无关的守卫恒跑**：`filenames`（命名）与 `idents`（脱敏）。
//      脱敏那条要扫的是**文本**，`.md` / `.json` / `.py` 一样能泄漏真实标识 ——
//      把它按语言切掉，等于给"最不该漏的那一条"开一个永远存在的洞。
//      ⇒ 分类器的输出里这两条**不在可跳的集合里**，不是"这次没匹配上"。
//   3. **分类器不确定时是全跑，不是全跳。** 见下面 `fallback` 那条分支：
//      拿不到暂存清单（未诞生分支、git 报错）时给"什么都跑"，
//      因为"少验"在这里是不可接受的失效方向。
//
// # 分类规则就两条
//
//   *.go / go.mod / go.sum   → 追加 Go 的三条静态门禁（fmt deps jsontags）
//   web/** 下任何文件         → 追加 web 的 typecheck
//
// ⚠ **不**按"`web/*.ts` 才算、`.css` 不算"细分：typecheck 1.9 秒，
// 而细分要把"哪些扩展名影响打包"写成第三份清单（vite 的配置才是那份的真出处）。
// 同理 `build` 不在提交档 —— 它写 `web/dist/`，与 Go 侧 `build` 一样属"要产出物的那条"，
// 留到 pre-push。判据是"**发现问题的时刻与副作用**"，不是快慢（README 同一条）。
//
// 用法：
//   node scripts/gates-scope.js            # 读暂存区（hook 用这个）
//   node scripts/gates-scope.js staged     # 同上，写法展开一点
//   printf 'web/src/a.tsx\n' | node scripts/gates-scope.js stdin   # 自己测分类用
//
// 输出（stdout，三行 `KEY=`，值可为空）：
//   GATES_GO=<喂给 gates-go 的门禁名>
//   GATES_WEB=<喂给 gates-web 的门禁名>
//   NOTE=<这一行为什么这么分，给人看的>
//
// 退出码：0 分类完成；1 门禁脚本自己语法就不对（这条就是"给门禁的门禁"）；2 用法错误。
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import process from 'node:process'
import { cd } from 'zx'

cd(new URL('..', import.meta.url).pathname)

const MODE = process.argv[2] ?? 'staged'

// 恒跑的那两条（语言无关的守卫，见上面第 2 条前提）。
const ALWAYS = 'filenames idents'
// 只有 Go 代码被碰到才加的三条静态门禁。
const GO_STATIC = 'fmt deps jsontags'
// 只有 web 被碰到才加的那一条。
const WEB_STATIC = 'typecheck'

// ── 给门禁自身的门禁：这些文件坏了，整道闸就是一次"静默没跑" ──────────
//
// 为什么放在这里而不是做成一条独立门禁：这些文件正是**接下来要被调用**的东西，
// 分类器又恰好是每次提交的第一环 —— 它先活着，才谈得上派发。
// 语法检查只查"读不读得进来"（抓引号没闭合、`fi` 少了这类），抓不到逻辑错；
// 而那类错的**现象**恰好是"门禁跑了、而且绿了"，是最贵的一种假绿。
//
// 迁移期同时有 `.sh`（仍是 bash）与 `.js`（zx）两种门禁，故**按扩展名选检查器**，
// 而不是维护一张会过期的清单：`.js`/`.mjs` 用 `node --check`，其余（`.sh` 与无后缀
// 的钩子）用 `bash -n`。新增一类文件后缀会在这里当场暴露（既非 node 也非 bash 的文件
// 用 bash -n 会因语法不通而红），逼作者显式表态。
function selfCheckSyntax() {
  const gateFiles = [
    'scripts/gates-go.js',
    'scripts/gates-go.sh',
    'scripts/gates-web.js',
    'scripts/gates-web.sh',
    'scripts/gates-scope.js',
    'scripts/gates-scope.sh',
    'scripts/check-filenames.js',
    'scripts/check-go-deps.js',
    'scripts/check-identifiers.cjs',
    'scripts/check-golden-vectors.cjs',
    'scripts/check-go-jsontags.cjs',
    '.githooks/pre-commit',
    '.githooks/pre-push',
  ]
  for (const f of gateFiles) {
    if (!fs.existsSync(f)) continue // 迁移中某一版可能尚未落地/已删；存在才检
    const isNode = /\.(m?js|cjs)$/.test(f)
    const r = isNode
      ? spawnSync('node', ['--check', f], { encoding: 'utf8' })
      : spawnSync('bash', ['-n', f], { encoding: 'utf8' })
    if (r.status !== 0) {
      const detail = (r.stderr || r.stdout || '').replace(/^/gm, '   ')
      process.stderr.write(`!! 门禁脚本自身语法不通：${f}\n${detail}`)
      process.exit(1)
    }
  }
}
selfCheckSyntax()

let files = ''
let fallback = false

if (MODE === 'staged') {
  // ⚠ `-c core.quotePath=false`：否则 git 会把含非 ASCII / 特殊字符的路径
  //   打成 `"web/\346\226\207…" ` 这种带引号的转义串，下面的 `web/` 前缀就匹配不上 ——
  //   现象是"改了前端的中文命名文件，前端门禁却没跑"，而且什么都不报。
  // ⚠ `--diff-filter=ACMRD`：**删除也要算**。删掉一个被 import 的模块，
  //   正是 typecheck 最该响的那一刻；漏掉 D 等于把它当成"没改"。
  const g = spawnSync(
    'git',
    ['-c', 'core.quotePath=false', 'diff', '--cached', '--name-only', '--diff-filter=ACMRD'],
    { encoding: 'utf8' },
  )
  if (g.status !== 0) {
    files = ''
    fallback = true
  } else {
    files = g.stdout
  }
} else if (MODE === 'stdin') {
  files = fs.readFileSync(0, 'utf8')
} else {
  console.error(`!! 未知模式: ${MODE}（可选：staged / stdin）`)
  process.exit(2)
}

const out = (go, web, note) => {
  console.log(`GATES_GO=${go}`)
  console.log(`GATES_WEB=${web}`)
  console.log(`NOTE=${note}`)
}

if (fallback) {
  // 拿不到清单 ⇒ **全跑**。失效方向必须是"多验"，不能是"少验"。
  out(
    `${ALWAYS} ${GO_STATIC}`,
    WEB_STATIC,
    '拿不到暂存清单（未诞生分支？git 报错），按最保守的一档全跑',
  )
  process.exit(0)
}

let go = ALWAYS
let web = ''

// 只动 Go 源码与依赖清单才算。`internal/**` 里的非 .go 文件（几乎没有）不必跑 gofmt。
// 逐条路径分别测结尾（不是整串正则）：避免多行拼接后的跨行误匹配。
const touched = files.split('\n').filter(Boolean)
if (
  touched.some(
    (f) =>
      /\.(go)$/.test(f) ||
      f === 'go.mod' ||
      f === 'go.sum' ||
      f.endsWith('/go.mod') ||
      f.endsWith('/go.sum'),
  )
) {
  go = `${go} ${GO_STATIC}`
}
if (touched.some((f) => f.startsWith('web/'))) {
  web = WEB_STATIC
}

let note
if (files.trim() === '') {
  note = '暂存区没有文件变更（空提交？），只跑恒过的两条'
} else if (web !== '' && go !== ALWAYS) {
  note = '同时改了 Go 与前端 ⇒ 两侧静态门禁都跑'
} else if (web !== '') {
  note = '只触及 web/ ⇒ 跳过 Go 静态门禁（pre-push 全跑兜底）'
} else if (go !== ALWAYS) {
  note = '触及 *.go ⇒ 加跑 fmt deps jsontags'
} else {
  note = '只触及文档/素材 ⇒ 只跑恒过的两条（filenames idents）'
}

out(go, web, note)
