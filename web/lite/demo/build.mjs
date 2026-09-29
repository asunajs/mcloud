/**
 * 用 lite 的插件编译 TSX 版 demo，打成自包含页面落 /tmp，再用无头 Chrome 看自测结果。
 * 同时把编译后的 JS 留一份（/tmp/lite-demo.compiled.js）—— 那是"黄金输出"。
 */
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

const dir = path.dirname(new URL(import.meta.url).pathname)
execFileSync('npx', ['vite', 'build', '--config', path.join(dir, 'vite.config.ts')], { cwd: path.join(dir, '../..'), stdio: 'pipe' })

const js = fs.readFileSync('/tmp/lite-demo/demo.mjs', 'utf8')
fs.writeFileSync('/tmp/lite-demo.compiled.js', js)
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>lite demo</title></head>
<body><div id="app"></div><div id="frag"></div>
<script>window.addEventListener('error',function(e){var p=document.createElement('pre');p.id='err';p.textContent=['ERR: '+(e.message||e.error),(e.error&&e.error.stack||'')].join(String.fromCharCode(10));document.body.appendChild(p)});</script>
<script type="module">${js}</script></body></html>`
fs.writeFileSync('/tmp/lite-demo.html', html)
console.log(`编译产物: /tmp/lite-demo.compiled.js (${Buffer.byteLength(js)} B)   页面: /tmp/lite-demo.html`)
