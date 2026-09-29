/**
 * 迁移回归的**总入口**：一条命令跑完"两侧都构建 + 全部比对"。
 *
 * 为什么要有它：迁移验收现在是好几支脚本（逐字符 / 交互 / 像素 / demo），
 * 各自记参数、各自构建 —— 切默认构建配置那种决定，必须能**一次跑完再拍**。
 *
 * 用法：
 *   node lite/regress/all.mjs            # 全量：9 路由 × ready + login/setup + 3 组交互
 *   node lite/regress/all.mjs --quick    # 冒烟：1 个路由 + 1 组交互（改脚本时用）
 *
 * 退出码非 0 = 有任何一项没过。⚠ 它**只读**仓库（产物落 /tmp），不动 `web/dist` 之外的任何东西。
 */
import { execFileSync } from 'node:child_process'
import path from 'node:path'

const dir = path.dirname(new URL(import.meta.url).pathname)
const web = path.join(dir, '../..')
const quick = process.argv.includes('--quick')

const ROUTES = quick ? ['settings'] : ['dashboard', 'accounts', 'tasks', 'exchange', 'live-room', 'schedules', 'pipelines', 'history', 'settings']
const VARIANTS = quick ? ['ready'] : ['ready', 'login', 'setup']

const run = (args, label) => {
  process.stdout.write(`── ${label} … `)
  try {
    const out = execFileSync('node', args, { cwd: web, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
    const bad = /(^|\n)\s*(✗|THREW)/.test(out)
    console.log(bad ? '✗' : '✓')
    if (bad) console.log(out.trim().split('\n').filter((l) => /✗|THREW|差异/.test(l)).slice(0, 12).join('\n'))
    return !bad
  } catch (e) {
    console.log('✗ 抛错')
    console.log(String(e.stdout ?? e.message).trim().split('\n').slice(-12).join('\n'))
    return false
  }
}

const bash = (cmd, label) => {
  process.stdout.write(`── ${label} … `)
  try {
    execFileSync('bash', ['-c', cmd], { cwd: web, stdio: ['ignore', 'pipe', 'pipe'] })
    console.log('✓')
    return true
  } catch (e) {
    console.log('✗')
    console.log(String(e.stdout ?? '') + String(e.stderr ?? ''))
    return false
  }
}

let ok = true
// 两侧产物都从当前源码重新构建：拿旧产物比对等于在测上一版
ok = bash('npx vite build --config vite.config.lite.ts', 'lite 包构建') && ok
ok = bash('npm run build >/dev/null 2>&1', 'Vue 包构建（web/dist）') && ok

for (const route of ROUTES) {
  ok = run([path.join(dir, 'compare.mjs'), '--route', route, '--variant', 'ready'], `逐字符 ${route}`) && ok
}
for (const variant of VARIANTS.filter((v) => v !== 'ready')) {
  ok = run([path.join(dir, 'compare.mjs'), '--route', 'settings', '--variant', variant], `逐字符 认证态 ${variant}`) && ok
}
for (const variant of VARIANTS) {
  ok = run([path.join(dir, 'interact.mjs'), '--variant', variant], `交互 + 像素 ${variant}`) && ok
}
if (!quick) ok = bash('node lite/demo/run.mjs', 'demo 断言（53 条）') && ok

console.log(ok ? `\n全绿：迁移验收通过${quick ? '（--quick 只跑了冒烟子集）' : ''}` : '\n✗ 有项目未通过（见上）')
process.exitCode = ok ? 0 : 1
