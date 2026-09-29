/**
 * 量 lite 运行时自己的体积。
 *
 * 方法：把它当成一个库、用**与主产物完全相同的工具链**（Vite 8 / Rolldown / es2022 /
 * 默认压缩器）打一遍，再 gzip。对比对象是 Vue Vapor 的运行时地板
 * （43,656 B raw / 16,344 B gzip，见 docs/architecture.md §10.6）。
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

/** 三种裁剪程度，说明"按需删模块"能省多少。 */
const variants = {
  '全量（含 computed + store）': `
export * from '${src('index.ts')}'`,
  '去 store（项目当前用不到，状态就是模块级 ref）': `
export { ref, effect, batch, watch, computed } from '${src('signal.ts')}'
export * from '${src('dom.ts')}'
export * from '${src('control.ts')}'
export { createComponent, defineVaporComponent, mount, onMounted, onUnmounted, useSlots } from '${src('component.ts')}'`,
  '再删 computed（项目 0 处使用）': `
export { ref, effect, batch, watch } from '${src('signal.ts')}'
export * from '${src('dom.ts')}'
export * from '${src('control.ts')}'
export { createComponent, defineVaporComponent, mount, onMounted, onUnmounted, useSlots } from '${src('component.ts')}'`,
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
console.log('lite 运行时（生产构建 + gzip；Vue Vapor 地板 = 43,656 raw / 16,344 gzip）\n')
console.log(pad('裁剪', 46) + 'raw'.padStart(9) + 'gzip'.padStart(9) + '  对比 Solid 的 7KB')
let min = Infinity
for (const [name, raw, gz] of rows) {
  min = Math.min(min, gz)
  console.log(pad(name, 46) + String(raw).padStart(9) + String(gz).padStart(9) + '  ' + (gz < 7168 ? '✓ 小于' : '✗ 大于'))
}
console.log(`\n最小 ${min} B gzip = ${(min / 1024).toFixed(2)} KB`)
fs.rmSync(tmp, { recursive: true, force: true })
