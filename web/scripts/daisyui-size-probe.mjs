#!/usr/bin/env node
/**
 * daisyUI 体积探针 —— 量"移除 daisyUI 到底省多少、代价是什么"。
 *
 * ⚠ **不碰 `src/app.css`**：变体写进临时文件 `src/__daisy_probe.css`，用一份**继承真配置**的
 * 临时 vite 配置把整个 app 重构建一遍（只把 `./app.css` 那条 import 顶到探针文件），跑完删除
 * （为什么这样设计见下面 `PROBE_CSS` 那段注释 —— 一句话：
 * 本仓常有多条工作线并行，"覆写再还原"那个模式会抹掉别人**未提交**的改动）。
 *
 * 会动的东西只有两处，且都会收尾：`src/__daisy_probe.css` + `__daisy_probe_vite.config.ts`
 * （临时，跑完删）与 `dist/`（跑完用真 app.css 重新 `npm run build` 一次，保证产物与源码一致 ✓）。
 *
 * 用法：
 *   cd web && node scripts/daisyui-size-probe.mjs        # ≈ 6 次全量构建，含一次自检
 *
 * 五个变体（后两个是"真要迁移"的形状）：
 *   V0 现状（app.css 的逐字节副本 ⇒ 拿来验量法本身）
 *   V6 把**没在用**的组件补进 exclude（一行改动，零风险）
 *   V5 连 `collapse` 也排掉（页面会坏 ⇒ 用来看 collapse 单独值多少字节）
 *   V1 整个主插件去掉（两个主题块留着）
 *   V4 完全去 daisyUI + 自写令牌层（=下面 `TOKEN_LAYER` 那份）
 */

import { readFileSync, writeFileSync, rmSync, readdirSync, statSync, existsSync, mkdirSync } from 'node:fs'
import { gzipSync } from 'node:zlib'
import { execSync } from 'node:child_process'
import { join, dirname, basename } from 'node:path'
import { fileURLToPath } from 'node:url'

const WEB = join(dirname(fileURLToPath(import.meta.url)), '..')
const APP_CSS = join(WEB, 'src', 'app.css')

// ⚠⚠ 这个探针**只在 daisyUI 还在依赖里时能跑**（2026-10-04 加这道闸）。
//
// daisyUI 已于 2026-10-02 从 `web/package.json` + lock 里移除（`grep -c daisyui
// web/package-lock.json` ⇒ 0），而本脚本要读 `node_modules/daisyui/components` 来枚举
// 组件 ⇒ 今天跑起来只会得到一句 `ENOENT`，看不出"为什么"。所以这里先说明再退出。
//
// 这份脚本现在的用途**只剩历史分析**（`docs/daisyui-removal-analysis.md` 记的就是它当年的
// 读数与取舍，那份文档里也写了"今天跑不动"）。量 CSS 体积请改用
// `cd web && npm run build` 的末行读数（`web/scripts/gzip-sidecars.mjs` 会打 raw + gzip）。
if (!existsSync(join(WEB, 'node_modules', 'daisyui', 'components'))) {
  console.error(
    '这个探针需要 daisyUI 依赖（它枚举 node_modules/daisyui/components），\n' +
      '而 daisyUI 已于 2026-10-02 从本仓移除 ⇒ 跑不了。\n' +
      '量 CSS 体积请用：cd web && npm run build（看末行 raw / gzip 读数）。\n' +
      '想复现当年的对照，请 checkout 到移除 daisyUI 之前的那次提交再跑。',
  )
  process.exit(2)
}

/**
 * 页面里所有"class 位置可能出现的 token"。
 *
 * ⚠ 只取 **class 属性**里的词，别的字符串一概不算 —— 第一版图扫全文，
 * 于是 `<footer>`、`const filter = ref(…)` 被当成"用过 daisyUI 的 footer/filter"，
 * 少算了近 1 KB；第二版图扫**所有**字符串/模板串，于是某段模板串里的
 * `const filter` 又把 `filter` 捞回来了（同一个假阳性换了个来源）。
 * ⇒ 现在是**按 `class=` 定位、花括号配对取整段**（`${cond ? 'btn-soft' : 'btn-primary'}`
 * 这种嵌在三元里的类名也能吃到 —— 早先"截到第一个 `}` 就停"的版本会漏，
 * 而漏判的方向是**危险的**：会把真在用的组件排掉）。
 */
const usedClassTokens = () => {
  const files = execSync(`find src -name '*.tsx' -o -name '*.ts'`, { cwd: WEB, encoding: 'utf8' })
    .trim().split('\n')
  const tokens = new Set()
  const push = (blob) => {
    for (const tok of blob.split(/[^A-Za-z0-9_-]+/)) {
      if (/^[a-z][a-z0-9_-]*$/.test(tok)) tokens.add(tok)
    }
  }
  for (const f of files) {
    const s = readFileSync(join(WEB, f), 'utf8')
    for (const m of s.matchAll(/class\s*=\s*/g)) {
      let i = m.index + m[0].length
      const q = s[i]
      if (q === '"' || q === "'") {
        const end = s.indexOf(q, i + 1)
        push(s.slice(i + 1, end))
      } else if (q === '{') {
        // 配对到匹配的 `}`，并把里面所有引号串一并收下
        let depth = 0, end = -1
        for (let k = i; k < s.length; k++) {
          if (s[k] === '{') depth++
          else if (s[k] === '}' && --depth === 0) { end = k; break }
        }
        const body = s.slice(i + 1, end)
        push(body)
        for (const q2 of body.matchAll(/["'`]([^"'`]*)["'`]/g)) push(q2[1])
      }
    }
  }
  return tokens
}

/** 一个组件名在页面里到底用没用（`collapse` 本身或 `collapse-arrow` 这类都算用过）。 */
const makeIsUsed = (tokens) => (name) =>
  [...tokens].some((t) => t === name || t.startsWith(name + '-'))

/** 从 app.css 里读当前 `exclude:` 清单（决定"还剩哪些组件在白付字节"）。 */
const currentExclude = (css) =>
  new Set(
    (css.match(/exclude:\s*([^;]+);/)?.[1] ?? '')
      .replace(/\s+/g, ' ')
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean),
  )

/** 把若干组件名追加进 `exclude:`（返回新文本）。 */
const excludeMore = (css, names) => {
  const m = css.match(/(exclude:\s*)([^;]+)(;)/)
  return css.slice(0, m.index + m[1].length) + m[2] + ', ' + names.join(', ') + css.slice(m.index + m[1].length + m[2].length)
}

/** 删掉 `@plugin "…" { … }` 整块（按花括号配对；marker 要含 ` {` 才不会被注释误伤）。 */
const stripPlugin = (css, marker) => {
  const i = css.indexOf(marker)
  if (i < 0) return css
  let depth = 0
  let end = -1
  for (let k = css.indexOf('{', i); k < css.length; k++) {
    if (css[k] === '{') depth++
    else if (css[k] === '}') if (--depth === 0) { end = k + 1; break }
  }
  while (end < css.length && /[\s]/.test(css[end])) end++
  return css.slice(0, i) + css.slice(end)
}

/**
 * 迁移原型：自维护的主题令牌层。
 * 顶掉 daisyUI 的三样东西 —— ① Tailwind 颜色/圆角注册（`@theme`），
 * ② 形状与 daisyUI 变量（`--border/--size-field/--depth/--noise/--fx-noise`），
 * ③ base 里那套 `:root` 底色 + 滚动锁（app.css 自己有两处在写它）。
 * 取值**逐条抄自**当前产物里 daisyUI 算出来的那两份，不是新设计的配色。
 */
const TOKEN_LAYER = `
@theme {
  --color-base-100: #fff;
  --color-base-200: #f8fafc;
  --color-base-300: #eee;
  --color-base-content: #18181b;
  --color-neutral: #09090b;
  --color-neutral-content: #e4e4e7;
  --color-primary: #4f46e5;
  --color-primary-content: #fff;
  --color-info: #0f7396;
  --color-info-content: #fff;
  --color-success: #0a7d55;
  --color-success-content: #fff;
  --color-warning: #8a6100;
  --color-warning-content: #fff;
  --color-error: #c0233f;
  --color-error-content: #fff;
  --radius-selector: 0.5rem;
  --radius-field: 0.5rem;
  --radius-box: 0.75rem;
}
:root {
  color-scheme: light;
  --size-selector: 0.25rem;
  --size-field: 0.25rem;
  --border: 1px;
  --depth: 0;
  --noise: 0;
  --fx-noise: none;
  --root-bg: var(--color-base-100);
  --page-scroll-lock: initial;
  --page-overflow: var(--page-scroll-lock) hidden;
  --page-has-backdrop: var(--page-scroll-lock) 1;
  --primary-gradient: linear-gradient(135deg, #6366f1 0%, #4f46e5 100%);
  --primary-glow: rgba(79, 70, 229, 0.24);
  background-color: var(--root-bg);
  color: var(--color-base-content);
  scrollbar-color: color-mix(in oklab, currentColor 35%, transparent) transparent;
}
[data-theme="dark"] {
  color-scheme: dark;
  --color-base-100: #0f172a;
  --color-base-200: #07111f;
  --color-base-300: #1e293b;
  --color-base-content: #f2f8ff;
  --color-primary: #6366f1;
  --color-primary-content: #fff;
  --color-info: #0ea5e9;
  --color-info-content: #04283a;
  --color-success: #10b981;
  --color-success-content: #04231a;
  --color-warning: #f59e0b;
  --color-warning-content: #3a2c00;
  --color-error: #ef4444;
  --color-error-content: #fff;
  --primary-gradient: linear-gradient(135deg, #818cf8 0%, #6366f1 100%);
  --primary-glow: rgba(99, 102, 241, 0.28);
}
:root:not(span) {
  overflow: var(--page-overflow);
}
.collapse {
  display: block;
  /* ⚠⚠ 这句**删不得**。Tailwind 4 自带一个同名 utility，会把该类名的元素
     直接判成"折叠不可见"；daisyUI 的组件一直用一条 revert-layer 的规则顶着它。
     一旦不再引它的 collapse 组件，光 display:block 挡不住 ⇒
     那三处 details 整块**内容消失**（不是"样式变丑"，是看不见了）。
     ⚠ 这条注释里**故意不写选择器原文** —— Tailwind 4 扫源码**原文**，
     注释里的类名也会被当候选类名，凭空生成一条死 CSS。
     （更彻底的做法：把页面里的类名换成不撞 utility 的 disclosure。） */
  visibility: visible;
  border-radius: var(--radius-box);
}
.collapse-title,
.collapse > summary {
  display: block;
  min-height: calc(var(--size-field, 0.25rem) * 10);
  padding: calc(var(--size-field, 0.25rem) * 2.5) calc(var(--size-field, 0.25rem) * 4);
  font-weight: 600;
  cursor: pointer;
  list-style: none;
}
.collapse > summary::-webkit-details-marker {
  display: none;
}
.collapse-title:hover,
.collapse > summary:hover {
  background-color: color-mix(in oklab, var(--color-base-content) 8%, transparent);
}
.collapse-content {
  padding: 0 calc(var(--size-field, 0.25rem) * 4) calc(var(--size-field, 0.25rem) * 4);
}
.collapse-arrow > summary::after {
  content: "";
  float: right;
  width: 0.625rem;
  height: 0.625rem;
  margin-top: 0.5rem;
  border-top: 2px solid currentColor;
  border-inline-end: 2px solid currentColor;
  rotate: 45deg;
  transition: rotate 0.2s;
}
.collapse-arrow[open] > summary::after {
  rotate: 225deg;
}
`

/**
 * ⚠⚠ **不动 `src/app.css`**（改成这样是有原因的）：
 * 第一版图是"把变体写进 src/app.css，跑完再还原"，而本仓**常有多条工作线并行** ——
 * 19:16 那轮实测到 app.css 在探针跑的间隙被人改了（git diff 从 47/11 变成 226/66）。
 * 那个模式一旦撞上"别人在两次构建之间写入"，还原就会**把别人的改动抹掉**，
 * 而对方的工作**没有提交** ⇒ 撤不回来。⇒ 改成现在这样：
 * 变体写到**临时文件** `src/__daisy_probe.css`，跑完把临时文件和产物目录删掉。
 *
 * ⚠⚠ **但必须构建整个真 app，不能只构建那一份 CSS**（第二版栽在这儿）：
 * 起初临时配置只 import 探针 CSS，`@tailwindcss/vite` 的候选类名是**沿模块图**扫的，
 * 于是 `src/pages/*.tsx` 一条都没进扫描 ⇒ 探针 V0 比真实构建**少 2,661 B**
 * （实测 65,855 vs 68,516）—— 差的就是页面真正用到的那批 utility。
 * 现在临时配置 **`import` 真实的 `vite.config.ts`**（同一套 plugins：tailwindcss + lite，
 * 同一个 `lite` alias，同一份 build 选项），entry 仍是 `index.html`，
 * 只在 alias 里**多插一条**把 `src/main.ts` 那句 `import './app.css'` 顶到探针 CSS。
 * ⇒ 两边扫的文件集合、插件、构建选项全同，只有 CSS 来源不同，数才可比。
 * （代价：每个变体要连 JS 一起编，一次探针从 ≈8 s 变成下面实测的秒数。）
 *
 * 自检：V0 = `src/app.css` 的**逐字节副本**，它的 CSS 产物必须和真实构建**一模一样**
 * （表里会打 `V0 vs 真实 dist` 两个数，不一致就说明这套量法不成立）。
 */
const PROBE_CSS = join(WEB, 'src', '__daisy_probe.css')
const PROBE_CFG = join(WEB, '__daisy_probe_vite.config.ts')
const PROBE_OUT = join(WEB, '__daisy_probe_out')

/** 真·构建：量当前 `dist/assets/*.css`（脚本跑完会重新 build 一次，让 dist 与源码对齐）。 */
function measureRealBundle() {
  execSync('npx vite build', { cwd: WEB, stdio: 'pipe' })
  const css = readdirSync(join(WEB, 'dist', 'assets')).filter((n) => n.endsWith('.css'))
  if (css.length !== 1) throw new Error(`期望 1 份 CSS，实到 ${css.length}`)
  const buf = readFileSync(join(WEB, 'dist', 'assets', css[0]))
  const text = buf.toString('utf8')
  return { raw: buf.length, gzip: gzipSync(buf, { level: 9 }).length, text, layer: componentsLayerBytes(text) }
}

/**
 * `--dump`：把每个变体的**产物 CSS 文本**留下（`.daisy_probe_dump/V0.css` 这类小文件），
 * 用来回答"这两个变体到底差在哪几行"—— 只比字节数会看不出机制。
 * ⚠ 开 `--dump` 时这个目录**不自动删**（要看内容就别清），手动
 * `rm -rf web/.daisy_probe_dump`。
 */
const DUMP = process.argv.includes('--dump')
const DUMP_DIR = join(WEB, '.daisy_probe_dump')

/** 包一层：量数 +（可选）留产物。 */
function buildVariant(css, label) {
  const r = buildVariantOnce(css)
  if (DUMP) writeFileSync(join(DUMP_DIR, label.split(' ')[0] + '.css'), r.text)
  return r
}

/**
 * 构建"整个真 app，但 CSS 换成这一份变体"，量它的 CSS 产物。
 * 临时配置**继承** `web/vite.config.ts`，只改三处：outDir、把 `./app.css` 顶到探针文件、
 * 日志静音（见上面那段注释：这三处之外都不许动，否则数不可比）。
 */
function buildVariantOnce(css) {
  if (DUMP) mkdirSync(DUMP_DIR, { recursive: true })
  writeFileSync(PROBE_CSS, css)
  writeFileSync(
    PROBE_CFG,
    `import real from './vite.config'\n` +
      `const alias = (real.resolve?.alias ?? []).slice()\n` +
      // 插在**末尾**：`lite` 那条在前，先按真配置匹配。
      `alias.push({ find: './app.css', replacement: ${JSON.stringify(PROBE_CSS)} })\n` +
      `export default {\n` +
      `  ...real,\n` +
      `  resolve: { ...(real.resolve ?? {}), alias },\n` +
      `  build: { ...real.build, outDir: ${JSON.stringify(basename(PROBE_OUT))}, emptyOutDir: true },\n` +
      `  logLevel: 'error',\n` +
      `}\n`,
  )
  try {
    execSync(`npx vite build --config ${basename(PROBE_CFG)}`, { cwd: WEB, stdio: 'pipe' })
  } catch (e) {
    throw new Error(`探针构建失败：\n${e.stderr?.toString() ?? e.message}`)
  }
  const out = join(PROBE_OUT, 'assets')
  const names = existsSync(out) ? readdirSync(out).filter((n) => n.endsWith('.css')) : []
  if (names.length !== 1) throw new Error(`探针构建没出 CSS（实到 ${names.length} 份）—— 这套量法不成立`)
  const buf = readFileSync(join(out, names[0]))
  const text = buf.toString('utf8')
  return { raw: buf.length, gzip: gzipSync(buf, { level: 9 }).length, text, layer: componentsLayerBytes(text) }
}

const cleanup = () => {
  for (const p of [PROBE_CSS, PROBE_CFG]) if (existsSync(p)) rmSync(p, { force: true })
  rmSync(PROBE_OUT, { recursive: true, force: true })
  if (!DUMP) rmSync(DUMP_DIR, { recursive: true, force: true })
}

/**
 * 取产物里"真有一条以该类开头的规则"的类集合。
 *
 * ⚠ 不能拿 `\.CLASS` 直接搜 —— app.css 自己写了
 * `.card,.stats,…,.rounded-box.bg-base-100{…}` 这种**组合**选择器，
 * 于是 `.bg-base-100` 这个串在产物里永远存在（实测就是这个把 V1 的缺口
 * 报成"无"，而它其实把页面用了 51 次的 `bg-base-100` utility 整个丢了）。
 * ⇒ 要求类名前面不能是 `[\w.:-]`，即它得是**独立**选择器的一部分。
 */
const classSet = (cssText) => {
  const found = new Set()
  for (const m of cssText.matchAll(/(?<![\w.:#-])\.([A-Za-z][A-Za-z0-9_-]*)/g)) found.add('.' + m[1])
  return found
}

/**
 * 产物里 `@layer components{…}` 那一层的**字节数**（花括号配对）。
 * 它是"本仓手写的自建控件"那一层 —— 用来证明 daisyUI 在组件层**一个字节都没付**
 * （几个变体里这个数应当完全相同；不同就说明有人在往这层里塞 daisyUI 的东西）。
 */
const componentsLayerBytes = (cssText) => {
  const m = /@layer components\s*\{/.exec(cssText)
  if (!m) return -1
  const open = m.index + m[0].length - 1
  let depth = 0
  for (let k = open; k < cssText.length; k++) {
    if (cssText[k] === '{') depth++
    else if (cssText[k] === '}' && --depth === 0) return k + 1 - m.index
  }
  return -1
}

const orig = readFileSync(APP_CSS, 'utf8')
const tokens = usedClassTokens()
const isUsed = makeIsUsed(tokens)
const components = readdirSync(join(WEB, 'node_modules/daisyui/components')).filter((d) =>
  statSync(join(WEB, 'node_modules/daisyui/components', d)).isDirectory(),
)
const excluded = currentExclude(orig)
const stillIn = components.filter((c) => !excluded.has(c))
const unused = stillIn.filter((c) => !isUsed(c))

console.log(`daisyUI 组件共 ${components.length} 个；exclude 已挡 ${excluded.size} 个`)
console.log(`页面 class 位置收割到的**不同** token：${tokens.size} 个`)
console.log(`仍在发 CSS 的 ${stillIn.length} 个：${stillIn.join(' ')}`)
console.log(`其中页面**一处都没用**的 ${unused.length} 个：${unused.join(' ') || '（无）'}`)
console.log()

const v4css = stripPlugin(
  stripPlugin(
    stripPlugin(orig, '@plugin "daisyui" {').replace(
      '@import "tailwindcss";',
      '@import "tailwindcss";' + TOKEN_LAYER,
    ),
    '@plugin "daisyui/theme" {',
  ),
  '@plugin "daisyui/theme" {',
)

const variants = [
  ['V0 现状（app.css 的逐字节副本）', orig],
  [`V6 再排掉那 ${unused.length} 个没用的`, excludeMore(orig, unused)],
  ['V5 连 collapse 也排掉', excludeMore(orig, unused.concat(['collapse']))],
  ['V1 去掉主插件（留主题块）', stripPlugin(orig, '@plugin "daisyui" {')],
  ['V4 完全去 daisyUI + 自写令牌层', v4css],
]

const out = []
try {
  for (const [label, css] of variants) {
    if (!css.includes('@import "tailwindcss";')) throw new Error(`${label}：变体生成有误（丢了 tailwind import）`)
    out.push({ label, ...buildVariant(css, label) })
  }
  // 自检：同一时刻用**真 app.css** 走一遍真实构建，V0 必须与它**逐字节同**（不只是同长度）
  const real = measureRealBundle()
  const same = real.text === out[0].text
  console.log(`量法自检：真实构建 raw=${real.raw} gzip=${real.gzip} ／ 探针 V0 raw=${out[0].raw} gzip=${out[0].gzip} ⇒ ${same ? '逐字节一致 ✓ 这套量法成立' : '**不一致 ✗ 这套数不可信**'}`)
  if (!same) {
    const i = [...real.text].findIndex((c, k) => c !== out[0].text[k])
    console.log(`  首个差异在偏移 ${i}：真实「${real.text.slice(Math.max(0, i - 60), i + 60)}」`)
    console.log(`            探针「${out[0].text.slice(Math.max(0, i - 60), i + 60)}」`)
  }
  console.log('⚠ 注意：探针跑的时候，`src/__daisy_probe.css` 也在 Tailwind 的扫描范围内 ——')
  console.log('  真构建与探针都带着它，所以两边可比；删掉它之后真实 CSS 会略小（那部分死 CSS 本就不该有）。')
} finally {
  cleanup()
  execSync('npm run build --silent', { cwd: WEB, stdio: 'pipe' }) // 临时文件已删 ⇒ 产物回到线上口径
  console.log('✓ 临时文件与探针产物目录已删除，`dist` 用真实源码重构建过（含 gzip 边车）')
}

const count = (s, needle) => s.split(needle).length - 1

const base = out[0]
console.log()
console.log(
  '变体'.padEnd(30),
  'raw'.padStart(8),
  'gzip'.padStart(8),
  'Δgzip'.padStart(8),
  '自建@layer components'.padStart(22),
  'var(--color- 处'.padStart(16),
  '@supports 兜底块'.padStart(15),
)
for (const v of out) {
  console.log(
    v.label.padEnd(30),
    String(v.raw).padStart(8),
    String(v.gzip).padStart(8),
    String(v.gzip - base.gzip).padStart(8),
    String(v.layer).padStart(22),
    String(count(v.text, 'var(--color-')).padStart(16),
    String(count(v.text, '@supports (color:color-mix')).padStart(15),
  )
}
console.log(
  '⚠ 后两列是**核对机制用的**，不是体积。V4 的 `var(--color-` 比 V0 少 36 处、自建\n' +
    '   `@layer components` 也少 527 B —— 已经查到是什么了：自写 `@theme` 让 `--color-*`\n' +
    '   在 `:root` 变成**静态可读**，lightningcss 就把本仓那批\n' +
    '   `color-mix(… var(--color-base-content) 20% …)` 的**降级兜底那一条**折成字面色\n' +
    '   （实测 `.status`：`var(…)` → `#18181b33`），而 `@supports (color:color-mix…)` 里\n' +
    '   那条照旧是 var 引用（两块计数没掉）⇒ 支持 color-mix 的浏览器（= 本项目全部目标）\n' +
    '   **仍然随主题换色**，只有不支持的老浏览器拿到冻在浅色的那版。\n' +
    '   ⇒ 这一条是**省字节**、不是坏味道，但迁移时要知道它发生过（别把层大小变化当回归）。',
)
console.log()
const attr = (a, b) => out.find((x) => x.label.startsWith(a)).gzip - out.find((x) => x.label.startsWith(b)).gzip
console.log(`拆账：${unused.length} 个没用到的组件 = ${attr('V0', 'V6')} B gzip`)
console.log(`      collapse 组件                  = ${attr('V6', 'V5')} B gzip`)
console.log(`      base + 两份主题块 − 自写令牌层 = ${attr('V5', 'V4')} B gzip`)
console.log()
// ⚠ 同名撞车探针：Tailwind 4 自己有个 utility 会把该类名判成"折叠不可见"，
//   daisyUI 的组件用一条 revert-layer 顶着；不引 daisyUI 又没人写 visible ⇒ 内容消失。
for (const v of out) {
  const tw = /\.collapse\{visibility:collapse\}/.test(v.text)
  const guard = /visibility:revert-layer/.test(v.text)
  const vis = /\.collapse\{[^}]*visibility:visible/.test(v.text)
  console.log(`${v.label.padEnd(30)} Tailwind 那条=${tw ? '在' : '无'}  daisyUI 顶住=${guard ? '在' : '无'}  自写 visible=${vis ? '在' : '无'}  ⇒ ${tw && !guard && !vis ? '**内容会消失**' : 'ok'}`)
}

const usedClasses = tokens
for (const v of out.slice(1)) {
  const have = classSet(v.text)
  const lost = [...usedClasses].filter((c) => c && classSet(base.text).has('.' + c) && !have.has('.' + c))
  console.log(`${v.label} 相对 V0 丢掉的"页面在用的类"：${lost.length ? lost.join(' ') : '无'}`)
}
