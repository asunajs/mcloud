/**
 * 打 bench 页：编译 `bench.tsx` → 拼一个自包含页面落 /tmp（不进仓库产物）。
 *
 * 拆成"build 写页 / run 抓结果"两半，与 `lite/demo/` 同构：
 * 只想看页面时 `node lite/bench/build.mjs`，要数就 `node lite/bench/run.mjs`。
 */
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

const dir = path.dirname(new URL(import.meta.url).pathname)
execFileSync('npx', ['vite', 'build', '--config', path.join(dir, 'vite.config.ts')], { cwd: path.join(dir, '../..'), stdio: 'pipe' })

const js = fs.readFileSync('/tmp/lite-bench/bench.mjs', 'utf8')

/**
 * 页内 runner：每项操作取 3 轮的**最小值**，同时跑行为断言。
 *
 * ⚠ 断言不是点缀。上一版对拍两侧同挂时会互相"证明"对方正确（见
 * `regress/` 的教训），所以这里每项操作后都钉一条"结果对不对"，
 * 数字只有在断言全绿时才有意义。
 */
const runner = `
const $ = (s) => document.querySelector(s)
const rows = () => [...document.querySelectorAll('#bench li')]
const checks = []
const ok = (name, cond, extra) => checks.push((cond ? 'PASS  ' : 'FAIL  ') + name + (extra ? '   (' + extra + ')' : ''))
const text = (n) => { const e = rows()[n]; return e ? e.textContent : String(e) }
let probe = null

const OPS = [
  { label: '挂载 1000 行', fn: (A) => A.mount('#bench'), after: () => {
    ok('挂载真的铺出 1000 行', rows().length === 1000, 'rows=' + rows().length)
    ok('首/末行文本由绑定写入', text(0) === 'x:0' && text(999) === 'x:999', text(0) + ' / ' + text(999))
    probe = rows()[500]
  } },
  { label: '改一处文本 ×100', fn: (A) => { for (let i = 0; i < 100; i++) A.update1() } },
  { label: '改三处文本 ×100（不 batch）', fn: (A) => { for (let i = 0; i < 100; i++) A.update3() } },
  { label: '改三处文本 ×100（batch）', fn: (A) => { for (let i = 0; i < 100; i++) A.update3b() }, after: () => {
    // 300 / 200 / 200：三步各自把 a 加了一百，b、c 各被后两步加了一百
    ok('三处文本都刷到了终值', $('#t0').textContent === '300' && $('#t1').textContent === '200' && $('#t2').textContent === '200',
      $('#t0').textContent + '/' + $('#t1').textContent + '/' + $('#t2').textContent)
  } },
  { label: '追加 100 行 ×10', fn: (A) => { for (let i = 0; i < 10; i++) A.append(100) }, after: () => {
    ok('追加到 2000 行', rows().length === 2000, 'rows=' + rows().length)
    ok('尾行是新行', text(1999) === 'y:99', text(1999))
    ok('追加没重建旧行', probe.isConnected)
  } },
  // ⚠ 10 次反转是**偶数**次 ⇒ 终态看着和起始一样，断言会失去意义。
  // 所以计时块之外再补一次（不污染数字），按奇偶校验真实顺序。
  { label: '反转 2000 行 ×10', fn: (A) => { for (let i = 0; i < 10; i++) A.reverse() }, after: (A) => {
    A.reverse()
    ok('反转是搬节点（奇偶可辨）', text(0) === 'y:99' && text(1999) === 'x:0', text(0) + ' / ' + text(1999))
  } },
  { label: '同序重排（纯 diff）×10', fn: (A) => { for (let i = 0; i < 10; i++) A.reshuffle() }, after: () => {
    ok('重排不重建节点', probe.isConnected && rows().length === 2000, 'rows=' + rows().length)
  } },
  { label: '卸载', fn: (A) => A.unmount(), after: () => {
    ok('卸载后容器清空', $('#bench').children.length === 0, 'left=' + $('#bench').children.length)
  } },
]

const run = () => {
  const A = window.liteApi
  A.reset()
  return OPS.map((op) => {
    const t = performance.now()
    op.fn(A)
    const ms = performance.now() - t
    if (op.after) op.after(A)
    return ms
  })
}

const ROUNDS = 3
const runs = []
for (let i = 0; i < ROUNDS; i++) runs.push(run())
const best = (i) => Math.min(...runs.map((r) => r[i]))

const lines = ['操作'.padEnd(30) + [1, 2, 3].map((i) => ('第' + i + '次').padStart(9)).join('') + '最优'.padStart(10)]
OPS.forEach((op, i) => {
  lines.push(op.label.padEnd(28) + runs.map((r) => (r[i].toFixed(2) + 'ms').padStart(9)).join('') + (best(i).toFixed(2) + 'ms').padStart(10))
})
const fails = checks.filter((c) => c.startsWith('FAIL'))
lines.push('')
lines.push(fails.length ? '断言 FAIL ' + fails.length + ' / ' + checks.length : '断言全绿 ' + checks.length + ' 条')
lines.push(...checks)
lines.push('')
lines.push(navigator.userAgent.replace(/.*Chrome\\//, 'Chrome ').replace(/ .*$/, ''))
const pre = document.createElement('pre')
pre.id = 'result'
pre.textContent = lines.join(String.fromCharCode(10))
document.body.appendChild(pre)
`

const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>lite bench</title></head>
<body><div id="bench"></div>
<script>window.addEventListener('error',function(e){var p=document.createElement('pre');p.id='err';p.textContent=['ERR: '+(e.message||e.error),(e.error&&e.error.stack||'')].join(String.fromCharCode(10));document.body.appendChild(p)});</script>
<script type="module">${js}</script>
<script type="module">${runner}</script></body></html>`
fs.writeFileSync('/tmp/lite-bench.html', html)
console.log(`bench 页: /tmp/lite-bench.html   编译产物 ${Buffer.byteLength(js)} B（已压缩）`)
