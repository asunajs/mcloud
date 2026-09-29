/**
 * 编译期闸门：这些写法**必须**在编译时炸掉（或必须编出指定形态）。
 *
 * 为什么单独一条门：编译器的错误都是"抛出去"的，也就是说**没人写负例时，
 * 删掉那一句 `throw` 照样全绿** —— 而 `key` 那条恰恰是"看起来无害、实际静默错位"
 * 的典型：非 map 位置的 `key` 被丢掉之后，界面当时是对的，等列表一重排就乱。
 * 负例是唯一能把它钉住的东西。
 *
 * 为什么在 Node 里跑而不是在浏览器里：`compile()` 是纯函数（源码进、字符串出），
 * 判据全在返回值与抛错上，不需要真 DOM（那是 `lite/demo/run.mjs` 的活）。
 *
 * 用法：`node lite/regress/compiler.mjs`（退出码非 0 = 有负例没按预期抛错）
 */
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

const dir = path.dirname(new URL(import.meta.url).pathname)
const web = path.join(dir, '../..')
const tmp = path.join(dir, '.check-tmp')

/**
 * 一条用例：
 * * `throws` + `msg` —— 编译必须抛错，且信息里含 `msg`（信息也是产物：那是要给
 *   下一个人看的"为什么这里不该写 key"，改文案时会红在这里，是想要的效果）；
 * * `ok` + `emit` / `noEmit` —— 编译必须过，输出里必须出现 / 不能出现这些片段。
 */
const CASES = [
  {
    name: '普通元素上的 key ⇒ 抛错',
    src: `const A = () => <div key="a">x</div>`,
    throws: '这里的 `key` 什么都不做',
  },
  {
    name: '组件上的 key ⇒ 抛错',
    src: `const A = () => <Row key="r" />`,
    throws: '这里的 `key` 什么都不做',
  },
  {
    name: '条件分支里的 key ⇒ 抛错（两个分支都查）',
    src: `const A = (p) => (p ? <li key="y">y</li> : <li key="n">n</li>)`,
    throws: '这里的 `key` 什么都不做',
  },
  {
    name: '片段里的 key ⇒ 抛错',
    src: `const A = () => (<><span key="1">a</span><span>b</span></>)`,
    throws: '这里的 `key` 什么都不做',
  },
  {
    name: '`.map()` 返回的元素上的 key ⇒ 正常，且走 createFor',
    src: `const A = (l) => <ul>{l.value.map((x) => <li key={x.id}>{x.n}</li>)}</ul>`,
    ok: ['createFor(', 'l.value'],
  },
  {
    name: '渲染体读了索引 ⇒ positional=true（重排时重建那一行）',
    src: `const A = (l) => <ul>{l.value.map((x, i) => <li key={x.id}>第 {i + 1} 步</li>)}</ul>`,
    ok: ['createFor(', ', true)'],
  },
  {
    name: '渲染体没读索引 ⇒ positional=false（重排时把行搬走，保住焦点/光标）',
    src: `const A = (l) => <ul>{l.value.map((x, i) => <li key={x.id}>{x.n}</li>)}</ul>`,
    ok: ['createFor(', ', false)'],
  },
  {
    name: '块体 `.map()` ⇒ 不建 createFor（预处理在 effect 外跑，读信号会冻住）',
    src: `const A = (l) => <ul>{l.value.map((x) => { const t = x.n; return <li>{t}</li> })}</ul>`,
    ok: [],
    noOk: ['createFor('],
  },
  {
    name: '指令式属性 ⇒ 抛错（本框架没有 v-if 这一族）',
    src: `const A = () => <div v-if="ok">x</div>`,
    throws: '不支持指令式属性',
  },
  {
    name: '驼峰指令 `vIf` 同样抛错',
    src: `const A = () => <div vIf={true}>x</div>`,
    throws: '不支持指令式属性',
  },
  {
    // ⚠ 这一条命中的**不是** `checkDirective`：带 `@` 的属性名 TSX 本身就解析不过，
    // 由 `compile` 开头的 `parseError` 拦（没有那道门时，产物是"元素被吃掉、尾巴留原文"
    // 的残码，下游只报一句 `Unexpected token`，看着像编译器的 bug）。
    name: '`@click` 事件简写 ⇒ 抛错（TSX 解析不过，报在这一层而不是 esbuild）',
    src: `const A = () => <div @click={f}>x</div>`,
    throws: '源码解析失败',
  },
  {
    name: '带冒号的命名空间属性**不该**被当成指令（SVG 用得到）',
    src: `const A = () => <svg xlink:href="#g">x</svg>`,
    ok: ['xlink:href'],
  },
]

// 把 `compiler.ts` 打成一个 Node 能直接 import 的 ESM。
// ⚠ 不用 Node 自带的类型剥离：`--experimental-transform-types` 在 Node 22 上还没有，
// 而 `typescript` 那包**必须留成 external** —— 打进临时产物等于每次跑闸门都压一遍 5MB，
// 与本闸门要证明的东西毫无关系。
fs.rmSync(tmp, { recursive: true, force: true })
fs.mkdirSync(tmp, { recursive: true })
fs.writeFileSync(
  path.join(tmp, 'entry.ts'),
  // vite 的 lib 入口不能是仓库外的虚拟模块，那就写一个一行的转发。
  // ⚠ 本脚本的临时目录是 `lite/regress/.check-tmp`，所以 `../../` 才回到 `lite/`
  `export { compile } from '../../compiler.ts'\n`,
)
try {
  execFileSync('npx', ['vite', 'build', '--config', path.join(dir, 'check.vite.config.ts')], { cwd: web, stdio: 'pipe', encoding: 'utf8' })
} catch (e) {
  // 打纯文本：execFileSync 默认把 stdout/stderr 当 Buffer 印出来，几百行十六进制，
  // 真正那一句（UNRESOLVED_ENTRY 之类）反而被埋在下面
  console.log('✗ 临时构建失败：\n' + [String(e.stdout ?? ''), String(e.stderr ?? '')].join('\n').split('\n').slice(-14).join('\n'))
  process.exit(1)
}

const outDir = path.join(tmp, 'out')
const outFile = fs.existsSync(outDir) ? path.join(outDir, fs.readdirSync(outDir).find((f) => f.endsWith('.js')) ?? 'compiler.js') : ''
if (!outFile || !fs.existsSync(outFile)) {
  console.log('✗ 没打出临时产物（' + outDir + '）—— 检查 lite/regress/check.vite.config.ts')
  process.exit(1)
}
const { compile } = await import(pathToFileURL(outFile).href)

let bad = 0
for (const c of CASES) {
  let threw = null
  let out = ''
  try {
    out = compile(c.src, { filename: 'case.tsx' }).code
  } catch (e) {
    threw = String(e.message ?? e)
  }
  if (c.throws) {
    if (!threw) {
      bad++
      console.log(`  ✗ ${c.name} —— 期望抛错，结果编过了`)
    } else if (!threw.includes(c.throws)) {
      bad++
      console.log(`  ✗ ${c.name} —— 抛了，但不是预期的那一条`)
      console.log(`     期望含：${c.throws}`)
      console.log(`     实际：${threw.split('\n')[0]}`)
    } else {
      console.log(`  ✅ ${c.name}`)
    }
    continue
  }
  if (threw) {
    bad++
    console.log(`  ✗ ${c.name} —— 不该抛错`)
    console.log(`     ${threw.split('\n')[0]}`)
    continue
  }
  const missing = (c.ok ?? []).filter((frag) => !out.includes(frag))
  const extra = (c.noOk ?? []).filter((frag) => out.includes(frag))
  if (missing.length || extra.length) {
    bad++
    console.log(`  ✗ ${c.name}`)
    if (missing.length) console.log(`     产物里少了：${missing.join(' / ')}`)
    if (extra.length) console.log(`     产物里多了：${extra.join(' / ')}`)
    continue
  }
  console.log(`  ✅ ${c.name}`)
}

fs.rmSync(tmp, { recursive: true, force: true })
console.log(bad ? `\n✗ ${bad}/${CASES.length} 条不符合预期` : `\n✓ ${CASES.length} 条负例/形态全部符合预期`)
process.exitCode = bad ? 1 : 0
