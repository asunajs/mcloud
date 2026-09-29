/**
 * 迁移回归：**同一套 API fixture 下**，把 lite 包和 Vue 包的同一页面拿来逐节点比对。
 *
 * 体积和编译通过都不能回答"两个包渲染出的 DOM 是不是同一个" —— 这个脚本就是那个判据。
 *
 * 用法（先各自构建）：
 *   npx vite build --config vite.config.lite.ts     # → /tmp/lite-app
 *   npm run build                                    # → web/dist
 *   node lite/regress/compare.mjs --route settings [--variant ready|login|setup] [--from-raw]
 *
 * ⚠ 交互（点击、输入、切页之后）不在这里测 —— 那个用 `interact.mjs`。
 * 本脚本只看**首屏**：一次 `--dump-dom`，最省事也最不容易错。
 */
import fs from 'node:fs'
import { appSubtree, arg, dumpDom, hashRoute, readVariant, serve } from './lib.mjs'

const route = hashRoute(arg('--route', 'settings'))
const variant = readVariant()
const liteDir = arg('--lite', '/tmp/lite-app')
const vueDir = arg('--vue', 'dist')

fs.mkdirSync('/tmp/shots', { recursive: true })
/**
 * `--from-raw`：直接用上一次落盘的原始抓取（`/tmp/lite-raw.*.html`）。
 * 调**比对规则**（归一化、差异报告）时不必再跑一遍 chrome —— 抓一次几秒钟，调试期很划算。
 */
const fromRaw = process.argv.includes('--from-raw')
let raw
if (fromRaw) {
  raw = { lite: fs.readFileSync('/tmp/lite-raw.lite.html', 'utf8'), vue: fs.readFileSync('/tmp/lite-raw.vue.html', 'utf8') }
} else {
  const a = await serve(liteDir, 48151, variant)
  const b = await serve(vueDir, 48152, variant)
  raw = {
    lite: await dumpDom({ port: 48151, route, name: 'lite', budget: arg('--budget', '8000'), timeout: arg('--timeout', '90000') }),
    vue: await dumpDom({ port: 48152, route, name: 'vue', budget: arg('--budget', '8000'), timeout: arg('--timeout', '90000') }),
  }
  // 原始抓取一律落盘：失败时想细看（例如 side 里塞了诊断节点）不必再跑一遍
  for (const [name, html] of Object.entries(raw)) fs.writeFileSync(`/tmp/lite-raw.${name}.html`, html)
  a.close()
  b.close()
}

/**
 * 逐像素比对**不在这里做**：`--screenshot` 与虚拟时间预算的配合不稳 ——
 * 按钮的 `transition: background-color` 有时走完、有时截到过渡中的一帧
 * （实测同一命令两次，一次 0 个不同像素、一次 1,564 个，且色差只有 5 阶）。
 * 像素比对放在 `interact.mjs`：那里是 CDP + **真实时间**，可以先注入
 * "禁用所有 transition/animation" 的样式再截图，结果稳定。
 */

const lite = appSubtree(raw.lite)
const vue = appSubtree(raw.vue)
console.log(`页面 ${route}（variant=${variant}；lite ${liteDir} vs Vue ${vueDir}）`)
console.log(`  lite #app ${lite.length} B    Vue #app ${vue.length} B`)
if (lite && lite === vue) {
  console.log('  ✅ 逐字符一致；截图在 /tmp/shots/')
} else if (!lite || !vue) {
  console.log('  ✗ 有一侧没抓到 #app：')
  console.log('     lite 原始抓取: ' + JSON.stringify(raw.lite.slice(0, 200)))
  console.log('     vue  原始抓取: ' + JSON.stringify(raw.vue.slice(0, 200)))
  process.exitCode = 1
} else {
  const i = [...lite].findIndex((c, k) => c !== vue[k])
  console.log(`  ✗ 不一致，首个差异在第 ${i} 个字符`)
  console.log(`     lite: ${JSON.stringify(lite.slice(Math.max(0, i - 50), i + 70))}`)
  console.log(`     vue : ${JSON.stringify(vue.slice(Math.max(0, i - 50), i + 70))}`)
  process.exitCode = 1
}