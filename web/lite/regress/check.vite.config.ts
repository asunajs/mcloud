/**
 * `regress/compiler.mjs` 的临时构建配置：把 `lite/compiler.ts` 打成一个
 * **Node 能直接 import 的 ESM**，好在脚本里跑编译期负例。
 *
 * 为什么这么绕：`compiler.ts` 是 TS，而 Node 的类型剥离开关
 * （`--experimental-transform-types`）在 Node 22 上还没有 —— 闸门不能靠开关活着。
 * 走 vite 就与主产物同一套工具链，多出来的成本是一次约 0.3 s 的小构建。
 *
 * ⚠ `typescript` 必须是 external：把它打进临时产物等于每次跑闸门都压一遍 5MB，
 * 与本闸门要证明的东西毫无关系。
 *
 * ⚠ 路径一律绝对：本文件与临时目录都在 `lite/regress/` 下（`.check-tmp`），而
 * Vite 把相对 `outDir` 当成相对 **root**（= `web/`），不写绝对就会落错地方 ——
 * 写错一次就是 `UNRESOLVED_ENTRY`（踩过）。
 */
import path from 'node:path'
import { defineConfig } from 'vite'

const here = path.dirname(new URL(import.meta.url).pathname)
const tmp = path.join(here, '.check-tmp')

export default defineConfig({
  build: {
    outDir: path.join(tmp, 'out'),
    emptyOutDir: true,
    target: 'es2022',
    minify: false,
    sourcemap: false,
    // ⚠ 扩展名要写全：Vite 8 的 lib 模式**不会**给 `fileName()` 的返回值补 `.js`
    //（给 'compiler' 就产出裸文件 `compiler`，Node 那边 import 不到）
    lib: { entry: path.join(tmp, 'entry.ts'), formats: ['es'], fileName: () => 'compiler.js' },
    rollupOptions: { external: ['typescript'] },
  },
})
