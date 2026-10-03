/**
 * **默认前端构建配置 = lite**。
 *
 * ⚠⚠ 改完**必须** `npm run build` 再 `cargo build`：`rust-embed` 在**编译期**
 * 把 `web/dist` 嵌进二进制，而 `cargo build` **不跟踪** `web/dist` 的变化。
 * 顺序反了会内嵌旧资源（白屏 / chunk 404），而 `cargo build` 还会说 "Finished"。
 *
 * 两条 import 口径（都得留着，少一条就构建失败）：
 * * `resolve.alias` 的 `lite`：业务源码 `import { ref } from 'lite'` 与
 *   编译器生成的 `import { … } from 'lite'` 都走它。
 *   ⚠ 不能改用相对路径：每个源文件深度不同，`./lite/src/index` 会解析到不同地方。
 * * `plugins` 里的 `lite({ runtime: 'lite' })`：把 TSX 折成模板 + 逐槽 effect。
 *
 * 曾经这里还有 `vue` / `vue-jsx-vapor` 两条 alias —— 那是"业务源码一行不改就切到
 * 自研运行时"的迁移桥。2026-09-30 源码已经全部直接写 `from 'lite'`，桥拆了。
 */
import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import lite from './lite/vite.ts'

// 两个踩过的坑：
// 1. daisyUI 是 Tailwind 插件，不是 Vite 插件 —— 写进 plugins 会报
//    `handler is not a function`。它在 app.css 里用 `@plugin "daisyui"` 注册。
// 2. Tailwind 4 是 CSS-first，没有 tailwind.config.js（已废弃）。
export default defineConfig({
  plugins: [tailwindcss(), lite({ runtime: 'lite' })],
  resolve: {
    alias: [{ find: /^lite$/, replacement: new URL('./lite/src/index.ts', import.meta.url).pathname }],
  },
  server: {
    // 开发时后端在 mcloud-server（默认 38123）。
    //
    // 配代理而不是让前端跨域请求：生产形态本来就是同源
    // （后端直接发前端产物），开发期用代理抹平差异，
    // 才不会出现「开发能用、打包上去就不行」。
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:38123',
        changeOrigin: false,
      },
    },
  },
  build: {
    outDir: 'dist',
    target: 'es2022',
    sourcemap: false,
    // 关掉 modulepreload 的**垫片**（不是 preload 本身）。
    //
    // 它是给「不支持 `rel=modulepreload` 的浏览器」用的 polyfill。写这条时产物是
    // 单 chunk（`<link rel="modulepreload">` 数量为 0）⇒ 垫片没有东西可垫。
    //
    // ⚠ 2026-10-03 复核：**前提已经变了** —— `app.tsx` 现在按页动态 `import()`
    // （见 `docs/lite-framework.md` §8），`web/dist/assets` 有 20 个 chunk、
    // `index.html` 里有 1 条 `modulepreload`。这条设置**仍然保留**，因为：
    // 本仓的目标浏览器（macOS 上跑 WebUI 的 Chrome/Safari）全都支持
    // `rel=modulepreload`，垫片是给老浏览器兜底的，而这条路线里没有老浏览器。
    // 真要支持老浏览器时**先复核这一条**（这正是当初写下的那句提醒）。
    modulePreload: { polyfill: false },
  },
})
