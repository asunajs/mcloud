/**
 * 给 `web/dist` 里的文本产物写一份 `.gz` 边车。
 *
 * ## 为什么要有这个脚本
 *
 * 服务端原先靠 `CompressionLayer` **每请求现压**，这是首屏最贵的一步。实测（本机，
 * 130,575 B 的那个 JS 产物）：
 *
 * | 路径 | 耗时 |
 * |---|---|
 * | 不压（identity） | 4.8 ms |
 * | 每请求现压 gzip（串行） | 45.9–60.6 ms |
 * | 每请求现压 gzip（并发 10） | **86 ms p50 / 60 RPS** |
 * | 离线压一次（本脚本，zlib） | 3.4–4.9 ms，**只花一次** |
 *
 * 压一次存货、之后照着发，这条成本就从"每个请求"变成"每次构建"。
 * 服务端侧见 `crates/mcloud-server/src/static_files.rs`：有边车就发边车，
 * 没有就退回 `CompressionLayer`（两条路都在，缺一边也不会坏）。
 *
 * ## 为什么是独立脚本而不是 Vite 插件
 *
 * `web/tsconfig.json` 的 `include` 里有 `vite.config.ts`，而 `types` 只有
 * `vite/client`、**没装 `@types/node`** —— 在配置里 `import 'node:zlib'`
 * 会让 `npm run typecheck` 直接红。这个脚本不在 tsconfig 的 include 里，
 * 是纯 Node 的 `.mjs`，两条门都不受影响。
 *
 * ⚠ 必须挂在 `npm run build` 后面：`dist` 由 Vite 产出，边车要与它**同批**生成，
 * 否则会出现"新 JS + 旧边车"。debug 构建下服务端是运行时读盘的，这个不一致
 * 会立刻表现为页面拿到过期字节。
 */
import fs from 'node:fs'
import path from 'node:path'
import { gzipSync } from 'node:zlib'

/** `dist` 目录（本脚本在 `web/scripts/` 下）。 */
const DIST = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../dist')

/**
 * 值得压的扩展名。
 *
 * 只列**确定是文本**的：图片 / 字体 / 已经压过的格式再压一遍是净亏
 * （体积不降、还多一次构建开销和一份磁盘占用）。
 */
const COMPRESSIBLE = new Set(['.js', '.mjs', '.css', '.html', '.svg', '.json', '.webmanifest', '.txt'])

/**
 * 小于这个字节数就不写边车。
 *
 * gzip 有 20 B 起步的头 + 校验尾，小文件的收益可能为负；而且这类文件
 * （比如 1 KB 以下的图标 JSON）省下的那点字节还不够一次额外的请求判断。
 * 服务端遇到没有边车的文件会退回现压，行为不变。
 */
const MIN_BYTES = 512

/** 递归列出目录下所有文件。 */
const walk = (dir) =>
  fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name)
    return entry.isDirectory() ? walk(full) : [full]
  })

if (!fs.existsSync(DIST)) {
  // 与 `static_files.rs` 的口径一致：没构建过前端不是错误，
  // 只是"没有页面可发"—— 这里明确说出来，别静默当成功。
  console.error(`没有 ${DIST}；先跑一次 vite build（本脚本由 npm run build 串在后面）`)
  process.exit(1)
}

let count = 0
let rawBytes = 0
let gzBytes = 0
for (const file of walk(DIST)) {
  if (file.endsWith('.gz')) continue // 别压边车自己
  if (!COMPRESSIBLE.has(path.extname(file).toLowerCase())) continue

  const raw = fs.readFileSync(file)
  if (raw.length < MIN_BYTES) {
    // Vite 会清空 outDir，能走到这里的旧边车只可能是手工留下的 —— 一并清掉，
    // 免得"源文件已经不该有边车、边车却还在"。
    fs.rmSync(`${file}.gz`, { force: true })
    continue
  }

  // level 9：构建期只花一次，实测 130 KB 用 4.3–4.9 ms，比 level 6 再小 ~0.7%。
  const gz = gzipSync(raw, { level: 9 })
  fs.writeFileSync(`${file}.gz`, gz)
  count += 1
  rawBytes += raw.length
  gzBytes += gz.length
}

const pct = rawBytes === 0 ? 0 : (100 * (1 - gzBytes / rawBytes)).toFixed(0)
console.log(`gzip 边车：${count} 个文件，${rawBytes} B → ${gzBytes} B（省 ${pct}%）`)
