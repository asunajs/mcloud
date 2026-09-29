/** 临时探针（用完即删）：遍历 src 下所有 .tsx，逐个编译并报告问题。 */
import fs from 'node:fs'
import path from 'node:path'
import { defineConfig } from 'vite'
import { compile } from './lite/compiler.ts'

const walk = (dir: string, out: string[] = []) => {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (p.endsWith('.tsx')) out.push(p)
  }
  return out
}

export default defineConfig({
  logLevel: 'error',
  plugins: [
    {
      name: 'probe',
      buildStart() {
        const files = walk('src')
        let ok = 0
        fs.mkdirSync('/tmp/probe', { recursive: true })
        for (const f of files) {
          const base = f.replace('src/', '')
          try {
            const { code } = compile(fs.readFileSync(f, 'utf8'), { runtime: 'lite', filename: f })
            fs.writeFileSync('/tmp/probe/' + base.replace(/\//g, '__'), code)
            ok++
            console.log(`✓ ${base}`)
          } catch (e) {
            console.log(`✗ ${base}  →  ${(e as Error).message}`)
          }
        }
        console.log(`\n合计 ${files.length} 个文件：编译通过 ${ok}，失败 ${files.length - ok}`)
      },
    },
  ],
  build: { outDir: '/tmp/probe-out', emptyOutDir: true, rollupOptions: { input: 'index.html' } },
})
