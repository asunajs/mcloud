/** 把两侧打成 IIFE、合成一个页面，落 /tmp（不进仓库产物）。 */
import fs from 'node:fs'
import path from 'node:path'
import { build } from 'vite'
import vueJsxVapor from 'vue-jsx-vapor/vite'

const dir = path.dirname(new URL(import.meta.url).pathname)
const out = '/tmp/lite-bench'
fs.rmSync(out, { recursive: true, force: true })

const common = { outDir: null, emptyOutDir: true, target: 'es2022', minify: true, write: true }
await build({
  configFile: false, logLevel: 'error',
  build: { ...common, outDir: path.join(out, 'lite'), lib: { entry: path.join(dir, 'lite-side.ts'), formats: ['iife'], name: 'liteSide', fileName: () => 'bench.js' } },
})
await build({
  configFile: false, logLevel: 'error',
  plugins: [vueJsxVapor({ macros: true, interop: false })],
  // Vue 的 esm-bundler 产物要求这几个编译期常量被替换掉（app 构建里由 Vite 默认做，lib 模式要自己来）
  define: {
    'process.env.NODE_ENV': '"production"',
    __VUE_OPTIONS_API__: 'false',
    __VUE_PROD_DEVTOOLS__: 'false',
    __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: 'false',
  },
  build: { ...common, outDir: path.join(out, 'vue'), lib: { entry: path.join(dir, 'vue-side.tsx'), formats: ['iife'], name: 'vueSide', fileName: () => 'bench.js' } },
})

const runner = `
const N = 1000
window.benchData = { items: Array.from({ length: N }, (_, i) => ({ id: i, label: 'x', n: i })) }
const apiOf = (side) => window[side + 'Api']
const clear = () => { document.querySelector('#lite').innerHTML = ''; document.querySelector('#vue').innerHTML = '' }

const OPS = [
  ['挂载 ' + N + ' 行', (A) => A.mountTo()],
  ['改一处文本 ×100', (A) => { for (let i = 0; i < 100; i++) A.update1() }],
  ['改三处文本 ×100（不 batch）', (A) => { for (let i = 0; i < 100; i++) A.update3() }],
  ['追加 100 行 ×10', (A) => { for (let i = 0; i < 10; i++) A.append(100) }],
  ['反转 2000 行 ×10', (A) => { for (let i = 0; i < 10; i++) A.reverse() }],
  ['同序重排（纯 diff）×10', (A) => { for (let i = 0; i < 10; i++) A.reshuffle() }],
  ['卸载', (A) => A.unmount()],
]

const run = (side, sel) => {
  const raw = apiOf(side)
  const A = Object.assign({}, raw, { mountTo: () => raw.mount(sel) })
  const res = []
  clear()
  for (const [label, fn] of OPS) {
    const t = performance.now()
    fn(A)
    res.push([label, performance.now() - t])
  }
  return res
}

const runs = { lite: [], vue: [] }
for (let i = 0; i < 3; i++) { runs.lite.push(run('lite', '#lite')); runs.vue.push(run('vue', '#vue')) }
const best = (side, label) => Math.min(...runs[side].map((r) => r.find((x) => x[0] === label)[1]))
const pre = document.createElement('pre'); pre.id = 'result'
const lines = ['操作'.padEnd(34) + 'lite'.padStart(9) + 'vue'.padStart(10) + '比值'.padStart(9)]
for (const [label] of OPS) {
  const l = best('lite', label), v = best('vue', label)
  lines.push(label.padEnd(32) + (l.toFixed(1) + 'ms').padStart(10) + (v.toFixed(1) + 'ms').padStart(10) + ((v / l).toFixed(2) + 'x').padStart(9))
}
pre.textContent = lines.join(String.fromCharCode(10))
document.body.appendChild(pre)
`
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>lite vs vue bench</title></head>
<body><div id="lite"></div><div id="vue"></div>
<script>window.addEventListener('error',function(e){var p=document.createElement('pre');p.id='err';p.textContent='ERR: '+(e.message||e.error);document.body.appendChild(p)});</script>
<script>${fs.readFileSync(path.join(out, 'lite/bench.js'), 'utf8')}</script>
<script>${fs.readFileSync(path.join(out, 'vue/bench.js'), 'utf8')}</script>
<script>${runner}</script>
</body></html>`
fs.writeFileSync('/tmp/lite-bench.html', html)
console.log('对拍页: /tmp/lite-bench.html')
