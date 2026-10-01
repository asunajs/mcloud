/**
 * lite 的 Vite 插件：把 TSX 编译成 §4 的目标形态。
 *
 * `enforce: 'pre'` 是必须的 —— 它要**先于**下游转换器拿到还带着 JSX 的源码
 * （本插件把 JSX 折掉、保留 TS 语法，再由下游去类型）。
 *
 * ⚠⚠ **下游不是 esbuild**（2026-10-01 核实）：Vite 8.3 的依赖只有
 * `rolldown` / `lightningcss` / `postcss` / `picomatch` / `tinyglobby`，
 * 包里**根本没有 esbuild** —— TS 与 JSX 现在是 rolldown 内建的 oxc 在转。
 * 旧注释（以及 `docs/lite-guide.md` 的几处）写"交给 Vite 的 esbuild"是 Vite 7
 * 时代的说法，已经改掉了。**别再按"是不是 esbuild"来判断谁先谁后**：
 * 唯一稳的判据是插件自己的 `enforce: 'pre'`。
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
