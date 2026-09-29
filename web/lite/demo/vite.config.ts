/**
 * demo 的构建配置：**接上 lite 插件的现成例子**（迁移时把 `vue-jsx-vapor/vite` 换成它）。
 *
 * 用配置文件而不是在脚本里 import 插件，是因为 Vite 自己能加载 TS 配置、
 * 也能解析无扩展名的相对导入 —— Node 直接 import `.ts` 要带扩展名、还会被
 * 传递依赖卡住。
 */
import { defineConfig } from 'vite'
import lite from '../vite.ts'

// 不引 node: 内置模块：这份配置也在 tsconfig 的 include 里，而那里没引 node types
const entry = new URL('./main.tsx', import.meta.url).pathname

export default defineConfig({
  plugins: [lite({ runtime: '../src/index' })],
  build: {
    outDir: '/tmp/lite-demo',
    emptyOutDir: true,
    target: 'es2022',
    minify: false, // 看的是行为与生成代码，不压缩便于排查
    lib: { entry, formats: ['es'], fileName: () => 'demo.mjs' },
  },
})
