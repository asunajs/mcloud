#!/usr/bin/env node
// 文件名门禁：非 Go/Rust 源文件一律 kebab-case。bash 原版 scripts/check-filenames.sh
// 于 2026-09-22 迁移为 zx —— 判据、豁免表与输出文案逐字保持。
//
// 规则出处：本仓库 `docs/repo-layout.md`「文件命名」（判据的唯一现行来源）。
//
// # 为什么值得单独一条门禁
//
// 这条规则以前**只写在文档里**，于是 `web/src/pages/` 下三个 `AuthPage.tsx`
// 一路活到被用户点名。规则写在文档里没人执行，就会反复被违反 ——
// 而违反的表现极其安静：编译能过、测试全绿、界面正常，
// 只有下一个人加文件时"照着旁边那个写"，于是错误自我复制。
//
// *.rs / *_test.go / *_linux.go 这类"文件名即语义"的命名由各自工具链强制，
// 改名即失效，所以整类不适用本规则 —— 与下面豁免表不是一回事：
// 那是"按路径点名的例外"，这是"整类文件不归这条管"。
//
// # 判据
//
// kebab-case = 全小写 + 数字 + `-` 分隔。按 `.` 把文件名拆成段，**每段**都判：
//   ^[a-z0-9]+(-[a-z0-9]+)*$
// 这样 `gen-auth-vectors.cjs`（段 `gen-auth-vectors` / `cjs`）与
// `foo.d.ts`（段 `foo` / `d` / `ts`）都自然通过，不必为扩展名写特例。
//
// # 豁免：三类，各有各的理由，**不允许"顺手加一个"**
//
// 1. **生态固定名** —— 工具按字面找它们，改名即失效。
// 2. **上游客户端源码素材** —— 原样取出的协议对照物，改名等于伪造证据。
// 3. **生成物 / 依赖目录** —— 不是人写的，不在管辖范围。
import process from 'node:process'
import { $, cd } from 'zx'

cd(new URL('..', import.meta.url).pathname)

// 逐条对应原 bash 的 case 分支：`*` 按 case 语义匹配任意字符（含 `/`）。
// 新增一条必须同时满足：① 属于上面三类之一；② 在旁边的注释里写清属于哪一类。
// ⚠ **不要**因为"这个文件不好改名"就加进来 —— 第 2 类的判据是
//   "它是从上游原样取出的证据"，而不是"改名麻烦"。
const EXEMPTS = [
  // ① 生态固定名（AGENTS.md：Agent 规范文件，工具按这个字面名找，改名即失效）
  'Cargo.toml',
  'Cargo.lock',
  'README.md',
  'LICENSE',
  'THIRD-PARTY-NOTICES.md',
  'rust-toolchain.toml',
  'AGENTS.md',
  '*/Cargo.toml',
  '*/Cargo.lock',
  '*/rust-toolchain.toml',
  '*/README.md',
  '*/LICENSE',
  '*/.npmrc',
  '*/.gitignore',
  '*/.gitattributes',
  '*/index.html', // Vite / 静态站点按字面找它
  '*/package.json',
  '*/package-lock.json',
  '*/tsconfig.json',
  // GitHub Actions 按**路径**发现 workflow，文件名本身无语义。本仓库目前没有 CI
  // （2026-09-20 删掉了 ci.yml，理由见 docs/dev-gates.md），这条豁免保留 ——
  // 它约束的是"万一再加回来时该怎么写"。
  '*/.github/workflows/*',
  // vendor 的第三方 skill：`SKILL.md` 是规范的固定名，且内容是上游原样文件，
  // 改名会让"固定版本 + 校验和"的来源声明失效（见 THIRD-PARTY-NOTICES.md）。
  '.workbuddy/skills/*/SKILL.md',
  '.workbuddy/skills/*/*/SKILL.md',
  // ② 上游客户端源码素材（原样取出，不得改名）
  'docs/capture/apiServer.js',
  'docs/capture/pcclient-api.js',
  // 旧项目 mcloud-sign 的原样导出件：`$meta.json` 带 `$` 是旧项目的数据文件名，
  // 改名会破坏"新库这一份 ↔ 旧项目那一份"的一一对应
  //（`internal/core/legacy_test.go` 按名断言）。
  'test/fixtures/legacy/$meta.json',
  // ③ 生成物 / 依赖
  '*/node_modules/*',
  '*/target/*',
  '*/dist/*',
]

// bash `case` 的 `*` 匹配任意串（含 `/`），这里同语义地翻成 `.*`。
const exemptRe = EXEMPTS.map((glob) => {
  const re = glob
    .split('*')
    .map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
    .join('.*')
  return new RegExp(`^${re}$`)
})
const isExempt = (p) => exemptRe.some((re) => re.test(p))

const SEG_KEBAB = /^[a-z0-9]+(-[a-z0-9]+)*$/

// 只用 `git ls-files`（构建产物与编辑器临时文件不参与判断）。
const files = (await $`git ls-files`.nothrow()).stdout.split('\n').filter(Boolean)

let fail = false
let checked = 0
const offenders = []

for (const path of files) {
  // `*.rs`：文件名就是模块名；`*.go`：`_test.go` 后缀与 `_linux.go` 这类
  // 构建约束都按字面匹配 —— 整类不适用 kebab-case，理由见文件头。
  if (path.endsWith('.rs') || path.endsWith('.go')) continue
  if (isExempt(path)) continue

  const base = path.slice(path.lastIndexOf('/') + 1)
  const offending = base.split('.').find((seg) => seg !== '' && !SEG_KEBAB.test(seg))

  checked += 1
  if (offending !== undefined) {
    offenders.push(`${path} :: 段 "${offending}" 不是 kebab-case`)
    fail = true
  }
}

if (fail) {
  for (const line of offenders) console.log(`!! ${line}`)
  console.log('')
  console.log('文件名门禁未通过。规则见 docs/repo-layout.md「文件命名」：')
  console.log('  非 Rust 文件一律 kebab-case（全小写、- 分隔、不用空格与下划线）。')
  console.log('')
  console.log('重命名用 git mv 以保留历史：')
  console.log('  git mv path/OldName.tsx path/old-name.tsx')
  console.log('')
  console.log('若确实是「上游原样取出的素材」，把它加进 scripts/check-filenames.js 的豁免表，')
  console.log('并在注释里写明属于哪一类 —— 不允许因为「改名麻烦」而豁免。')
  process.exit(1)
}

console.log(`文件名门禁通过：${checked} 个文件均为 kebab-case（Rust 源文件除外）`)
