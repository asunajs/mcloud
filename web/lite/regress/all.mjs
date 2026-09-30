/**
 * lite 的**验收总入口**：一条命令跑完"构建 + 编译期负例 + 真 DOM 断言 + 三个认证态的交互"。
 *
 * 2026-09-30 起不再有"两侧对拍"（Vue 那一侧已随兼容层一起拆掉）。所以这里的判据是
 * 三条**绝对**的：编译期该抛的抛了（`compiler.mjs`）、真 DOM 上的行为对（`demo/run.mjs`）、
 * 点完之后界面确实变了且没抛异常（`interact.mjs`）。
 *
 * ⚠ "两侧一致"这种判据有个洞：**两侧都坏掉时它也绿**。上一版 `setup` 变体就是这样 ——
 * 确认口令填进了口令框（两个框的 `autocomplete` 在 setup 态撞车），闸门从没放行过，
 * 而逐字符比对照样全绿。换成绝对断言之后的第一次运行就把它抓了出来。
 *
 * 用法：
 *   node lite/regress/all.mjs            # 全量：构建 + 负例 + demo + bench + 3 个认证态交互
 *   node lite/regress/all.mjs --quick    # 冒烟：只跑 ready 一个认证态（改脚本时用）
 *   node lite/regress/all.mjs --skip-build # 不重新构建（`scripts/gates-web.mjs` 已建过 dist）
 *   node lite/regress/all.mjs --size     # 顺带打印运行时体积（不参与判定）
 *
 * 退出码非 0 = 有任何一项没过。⚠ 它**只读**仓库（临时产物落 /tmp 与 `lite/regress/.check-tmp`、
 * `lite/.size-tmp`），不动 `web/dist` 之外的任何东西，也**不碰** 3000 端口上那个 launchd 服务。
 */
import { execFileSync } from 'node:child_process'
import path from 'node:path'

const dir = path.dirname(new URL(import.meta.url).pathname)
const web = path.join(dir, '../..')
const quick = process.argv.includes('--quick')

/** 跑一支脚本：它的退出码 + 输出里有没有 `✗` 都算失败（脚本自己打印的失败行）。 */
const run = (args, label) => {
  process.stdout.write(`── ${label} … `)
  try {
    const out = execFileSync('node', args, { cwd: web, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
    // 三个失败标记来自各脚本自己的口径：`✗`（demo/compiler）、`THREW`（本该抛却没抛）、
    // `FAIL`（bench）。少认一个就等于那条闸门永远绿。
    const bad = /(^|\n)\s*(✗|THREW|FAIL)/.test(out)
    console.log(bad ? '✗' : '✓')
    if (bad) console.log(out.trim().split('\n').filter((l) => /✗|THREW|FAIL/.test(l)).slice(0, 14).join('\n'))
    return !bad
  } catch (e) {
    // ⚠ 这里**不能**只打 `e.stdout ?? e.message`：execFileSync 抛错时 `e.stdout` 通常是
    // 空字符串 ''，而空串**不是** nullish ⇒ `??` 不会回落到 message，于是真实原因被吞掉，
    // 只剩一行光秃秃的「✗ 抛错」（2026-10-01 就是这样把三条交互验收的报错弄丢的）。
    // 现在四处来源全打：栈 + message、子进程 stdout 尾部、子进程 stderr 尾部。
    console.log('✗ 抛错')
    const tail = (s, n) => String(s ?? '').trim().split('\n').filter(Boolean).slice(-n)
    const lines = []
    if (e && e.stack) {
      lines.push('--- 异常（' + (e.name ?? 'Error') + '）---', ...String(e.stack).split('\n'))
    } else {
      lines.push('--- 异常 ---', String((e && e.message) ?? e))
    }
    const outTail = tail(e && e.stdout, 14)
    if (outTail.length) lines.push('--- 子进程 stdout（尾部）---', ...outTail)
    const errTail = tail(e && e.stderr, 14)
    if (errTail.length) lines.push('--- 子进程 stderr（尾部）---', ...errTail)
    console.log(lines.join('\n'))
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

/**
 * 清掉测量/负例脚本的临时目录（`.size-tmp` / `.check-tmp`）。
 *
 * 它们**各自**在结束时删自己那份，但失败路径会留下残骸（`size.mjs` 上次抛在半路，
 * 于是 `lite/.size-tmp/` 在 `git status` 里挂了一整天）。脚本自己清不干净，
 * 所以总入口再兜一次 —— 顺手也让"跑过一次全绿"等于"工作区没脏"。
 */
const cleanTmp = () => {
  for (const f of ['../.size-tmp', '.check-tmp']) {
    try {
      execFileSync('rm', ['-rf', path.join(dir, f)], { stdio: 'ignore' })
    } catch {
      // 清不掉不是验收失败
    }
  }
}

let ok = true
// 先构建：交互验收跑的就是 `dist` 里那份产物，拿旧 dist 测等于在测上一版。
// `--skip-build` 只给"上游刚刚构建过"的门禁省这一下（见 `scripts/gates-web.mjs`）。
if (process.argv.includes('--skip-build')) {
  console.log('── 构建 … 跳过（--skip-build：由调用方保证 dist 是新的）')
} else {
  // ⚠ 用 `npm run build` 而不是裸 `npx vite build`：构建的**口径**只有一个
  // （2026-09-30 起 `npm run build` = vite build + gzip 边车）。裸 vite build 会让 dist
  // "是新的、但没有边车" —— 服务端会退回每请求现压，跑一次验收顺手把首屏优化弄丢。
  ok = bash('npm run build', '构建（npm run build → dist + gzip 边车）') && ok
}
ok = run([path.join(dir, 'compiler.mjs')], '编译期负例') && ok
// ⚠ 不要在这里写死 demo 的断言条数：断言会涨，写死了就会像上次那样显示 53 而实际已是 59
ok = run([path.join(dir, '../demo/run.mjs')], 'demo 真 DOM 断言') && ok
// bench 那一侧是**编译产物**的行为断言（开发者写法：条件/循环/事件/多信号一次改）
// + 一组很松的耗时护栏（挂载 > 500ms 判负 ⇒ 列表从"搬"退化成"重建"时能响）。
ok = run([path.join(dir, '../bench/run.mjs')], '编译产物行为 + 耗时护栏') && ok
for (const variant of quick ? ['ready'] : ['ready', 'login', 'setup']) {
  ok = run([path.join(dir, 'interact.mjs'), '--variant', variant], `交互验收 ${variant}`) && ok
}
if (process.argv.includes('--size')) {
  // 体积是**读数**不是闸门：它只打印，不参与 ok（超过阈值该怎么判要先跟用户定）
  try {
    console.log('\n' + execFileSync('node', [path.join(web, 'lite/size.mjs')], { cwd: web, encoding: 'utf8' }).trim())
  } catch (e) {
    console.log('体积测量失败：' + String(e.message ?? e))
  }
}

cleanTmp()
console.log(ok ? `\n全绿：lite 验收通过${quick ? '（--quick 只跑了冒烟子集）' : ''}` : '\n✗ 有项目未通过（见上）')
process.exitCode = ok ? 0 : 1
