/**
 * 量 lite 运行时自己的体积。
 *
 * 方法：把它当成一个库、用**与主产物完全相同的工具链**（Vite 8 / Rolldown / es2022 /
 * 默认压缩器）打一遍，再 gzip。
 *
 * 参照物（历史结论，来自 docs/architecture.md §10.6 的实测）：Vue Vapor 运行时地板
 * ≈ 43,656 B raw / 16,344 B gzip。本项目在 2026-09-30 把"为兼容 Vue 而留的壳"
 * （`createVaporApp` / `defineVaporComponent` / `renderEffect` / `computed` / `field` /
 * `createStore`）全删了 —— 那些是**普查里 0 处使用**的东西，删之前它们占的字节
 * 就在下面这张表的"全量"里。
 *
 * 用法：node lite/size.mjs
 */

import fs from 'node:fs'
import path from 'node:path'
import { gzipSync } from 'node:zlib'
import { build } from 'vite'

const dir = path.dirname(new URL(import.meta.url).pathname)
const src = (f) => path.join(dir, 'src', f)
const tmp = path.join(dir, '.size-tmp')

/**
 * 两档裁剪，说明"按需删模块"还能省多少。
 *
 * ⚠ 这里的导出清单必须与 `src/index.ts` **同步**：那份文件才是编译产物真正 import 的
 * 入口（上一版没同步，于是 `computed` 都删了还在量"含 computed 的全量"，
 * 报出来的数字与实际产物无关）。
 */
const variants = {
  '全量（= src/index.ts 导出的全部）': `export * from '${src('index.ts')}'`,
  '再去掉只有 demo/bench 用的内部件': `
export { batch, effect, ref, watch } from '${src('signal.ts')}'
export { createComponent, mount, onMounted, onUnmounted, useSlots } from '${src('component.ts')}'
export { createFor } from '${src('control.ts')}'
export { lazySlot, on, remove, setAttr, setClass, setNodes, setProp, setValue, spread, template } from '${src('dom.ts')}'`,
}

fs.rmSync(tmp, { recursive: true, force: true })
fs.mkdirSync(tmp, { recursive: true })
const rows = []
for (const [name, code] of Object.entries(variants)) {
  const entry = path.join(tmp, 'entry.ts')
  fs.writeFileSync(entry, code)
  const out = path.join(tmp, 'out-' + rows.length)
  await build({
    configFile: false,
    logLevel: 'error',
    build: {
      outDir: out,
      emptyOutDir: true,
      target: 'es2022',
      minify: true,
      lib: { entry, formats: ['es'], fileName: 'lite' },
      rollupOptions: { output: { minify: true } },
    },
  })
  const file = path.join(out, fs.readdirSync(out).find((f) => f.endsWith('.js')))
  const buf = fs.readFileSync(file)
  rows.push([name, buf.length, gzipSync(buf, { level: 6 }).length])
}

const pad = (s, n) => s + ' '.repeat(Math.max(0, n - [...s].reduce((w, c) => w + (c.charCodeAt(0) > 255 ? 2 : 1), 0)))
console.log('lite 运行时（生产构建 + gzip；参照：Vue Vapor 地板 ≈ 16,344 B gzip，Solid runtime ≈ 7 KB）\n')
console.log(pad('裁剪', 40) + 'raw'.padStart(9) + 'gzip'.padStart(9) + '  对比 Solid 的 7KB')
let min = Infinity
for (const [name, raw, gz] of rows) {
  min = Math.min(min, gz)
  console.log(pad(name, 40) + String(raw).padStart(9) + String(gz).padStart(9) + '  ' + (gz < 7168 ? '✓ 小于' : '✗ 大于'))
}
console.log(`\n最小 ${min} B gzip = ${(min / 1024).toFixed(2)} KB`)
fs.rmSync(tmp, { recursive: true, force: true })
