/**
 * **试接**：用 lite 的工具链构建整个真实应用（迁移演练用，还没替换默认配置）。
 *
 * 关键手法：**不改一行业务源码** ——
 * * `resolve.alias` 把 `vue` / `vue-jsx-vapor` 指到 lite 的运行时；
 * * 编译器生成代码里的运行时导入统一写成裸名 `lite`，再由同一个 alias 解析
 *   （用相对路径不行：每个源文件深度不同，`./lite/src/index` 会解析到不同的地方）。
 */
import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import lite from './lite/vite.ts'

const runtime = new URL('./lite/src/index.ts', import.meta.url).pathname

export default defineConfig({
  plugins: [tailwindcss(), lite({ runtime: 'lite' })],
  resolve: {
    alias: [
      { find: /^lite$/, replacement: runtime },
      { find: /^vue$/, replacement: runtime },
      { find: /^vue-jsx-vapor$/, replacement: runtime },
    ],
  },
  build: { outDir: '/tmp/lite-app', emptyOutDir: true, target: 'es2022', sourcemap: false },
})
