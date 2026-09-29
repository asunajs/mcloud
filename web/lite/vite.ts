/**
 * lite 的 Vite 插件：把 TSX 编译成 §4 的目标形态。
 *
 * `enforce: 'pre'` 是必须的 —— 它要**先于** esbuild 的 TS 处理拿到还带着 JSX 的源码
 * （本插件把 JSX 折掉、保留 TS 语法，再由 Vite 自己的 esbuild 去类型）。
 *
 * 用法（迁移时替换 `vue-jsx-vapor/vite`）：
 *
 * ```ts
 * import lite from './lite/vite'
 * plugins: [lite({ runtime: './lite/src/index' })]
 * ```
 */

import { compile } from './compiler.ts'

export interface LiteOptions {
  /** 生成代码里运行时从哪 import（相对**每个源文件**，或包名）。 */
  runtime?: string
  /** 处理哪些文件，默认 `.tsx`。 */
  include?: RegExp
}

export default function lite(options: LiteOptions = {}) {
  const runtime = options.runtime ?? 'lite'
  const include = options.include ?? /\.tsx$/
  return {
    name: 'lite',
    enforce: 'pre' as const,
    transform(code: string, id: string) {
      if (!include.test(id) || id.includes('node_modules')) return
      const { code: out } = compile(code, { runtime, filename: id })
      return { code: out, map: null }
    },
  }
}
