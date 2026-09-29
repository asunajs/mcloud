/**
 * 一条命令跑完 bench：**编译 → 无头 Chrome → 打印耗时表 + 行为断言**。
 *
 * 退出码非 0 的条件：页面抛错、任一断言 FAIL、或者数字明显不对
 * （挂载 1000 行 > 500ms —— 那通常意味着某处从"搬节点"退化成"重建整表"，
 * 与其说是慢，不如说是回归）。
 *
 * 用法：`node lite/bench/run.mjs`
 */
import { execFileSync, spawn } from 'node:child_process'

await import('./build.mjs')

const chromePath = execFileSync(
  'bash',
  ['-c', 'ls -d ~/.cache/puppeteer/chrome-headless-shell/*/chrome-headless-shell-*/chrome-headless-shell | head -1'],
  { encoding: 'utf8' },
).trim()

const dump = () =>
  new Promise((resolve) => {
    const child = spawn(chromePath, ['--user-data-dir=/tmp/lite-bench-profile', '--headless', '--disable-gpu', '--virtual-time-budget=10000', '--dump-dom', 'file:///tmp/lite-bench.html'], { stdio: ['ignore', 'pipe', 'ignore'] })
    let out = ''
    child.stdout.on('data', (d) => {
      out += d
    })
    const timer = setTimeout(() => {
      child.kill('SIGKILL')
      resolve(out)
    }, 60000)
    child.on('exit', () => {
      clearTimeout(timer)
      resolve(out)
    })
  })

const html = await dump()
const unesc = (s) => s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&').replace(/&quot;/g, '"')
const grab = (id) => {
  const m = html.match(new RegExp(`<pre id="${id}">([\\s\\S]*?)</pre>`))
  return m ? unesc(m[1]) : ''
}

const err = grab('err')
if (err) console.log(`页面抛错：\n${err}\n`)
const result = grab('result')
if (!result) {
  console.log('没抓到 #result —— 页面可能整个没跑起来。原始抓取片段：')
  console.log(JSON.stringify(html.slice(html.indexOf('<body'), html.indexOf('<body') + 400)))
  process.exit(1)
}
console.log(result)

const mountMs = Number((result.match(/挂载 1000 行.*?([\d.]+)ms\s*$/m) || [])[1] ?? NaN)
const inflated = Number.isFinite(mountMs) && mountMs > 500
if (result.includes('FAIL') || err || inflated) {
  if (inflated) console.log(`\n⚠ 挂载耗时 ${mountMs}ms 异常偏大 —— 先怀疑列表退化成整表重建。`)
  process.exit(1)
}
