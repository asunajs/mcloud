/**
 * bench 的构建配置：**编译 `bench.tsx`**（跟 `demo/vite.config.ts` 同一套插件）。
 *
 * 与 demo 唯一的差别是 `minify: true` —— 基准要量"发出去的那份"，
 * 与生产同条件；demo 留 false 是为了看生成代码。
 */
import { defineConfig } from 'vite'
import lite from '../vite.ts'

// 不引 node: 内置模块：这份配置也在 tsconfig 的 include 里，而那里没引 node types
const entry = new URL('./bench.tsx', import.meta.url).pathname

export default defineConfig({
  plugins: [lite({ runtime: '../src/index' })],
  build: {
    outDir: '/tmp/lite-bench',
    emptyOutDir: true,
    target: 'es2022',
    minify: true,
    lib: { entry, formats: ['es'], fileName: () => 'bench.mjs' },
  },
})
