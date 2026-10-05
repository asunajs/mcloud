/**
 * 一条命令跑完 demo 自测：**编译 TSX → 无头 Chrome 打开 → 打印断言结果**。
 *
 * 为什么要这个脚本：断言只有在浏览器里跑才有意义（要真 DOM、真 `insertBefore`），
 * 而每次手敲 chrome 命令行既容易漏（先 `--dump-dom` 再 grep）又会踩
 * 该脚本记下来的那三个抓取坑（原记在迁移期的 `regress/compare.mjs` 里，
 * 那个脚本 2026-09-30 随 Vue 兼容层一起删了）。这里把"抓"这件事收在一处。
 *
 * 用法：`node lite/demo/run.mjs`（退出码非 0 = 有断言失败）
 */
import { execFileSync, spawn } from 'node:child_process'
// ⚠ 无头 chrome 的公共参数（含可选的 --no-sandbox）只有一份来源，见该文件头
import { headlessFlags } from '../chrome-flags.mjs'

// 导入即构建：写 /tmp/lite-demo.html（见 build.mjs）。它内部是同步 execFileSync，
// 但本进程**没有**在跑的 HTTP 服务，所以不存在迁移期那套（`regress/compare.mjs`，已删）
// 里那种"阻塞事件循环互锁"。
await import('./build.mjs')

const chromePath = execFileSync(
  'bash',
  ['-c', 'ls -d ~/.cache/puppeteer/chrome-headless-shell/*/chrome-headless-shell-*/chrome-headless-shell | head -1'],
  { encoding: 'utf8' },
).trim()

const dump = () =>
  new Promise((resolve) => {
    const child = spawn(chromePath, ['--user-data-dir=/tmp/lite-demo-profile', ...headlessFlags(), '--virtual-time-budget=8000', '--dump-dom', 'file:///tmp/lite-demo.html'], { stdio: ['ignore', 'pipe', 'ignore'] })
    let out = ''
    child.stdout.on('data', (d) => {
      out += d
    })
    // 护栏：页面里若有渲染死循环，chrome 不会自己退出（这个脚本存在的原因之一）
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
if (result.includes('FAIL')) process.exit(1)
