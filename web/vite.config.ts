/**
 * **默认前端构建配置 = lite**（框架本体已剥离到 `asunajs/lite`，见 `docs/web/lite.md`）。
 *
 * ⚠⚠ 改完**必须** `npm run build` 再 `cargo build`：`rust-embed` 在**编译期**
 * 把 `web/dist` 嵌进二进制，而 `cargo build` **不跟踪** `web/dist` 的变化。
 * 顺序反了会内嵌旧资源（白屏 / chunk 404），而 `cargo build` 还会说 "Finished"。
 *
 * ⚠ **2026-10-07 起没有 `resolve.alias` 了**：以前要它，是因为框架就在仓里
 * （`web/lite/`）、业务源码写裸名 `lite`。现在它是一份**正常的依赖**
 * （`@asunajs/lite`），业务源码也改成写包名 ⇒ 交给 Vite 的常规依赖解析即可
 * （运行时是 TS 源码：dev 走依赖预打包、build 走 Rolldown，两条都实测过 ✓）。
 * 留着 alias 反而让"它到底是依赖还是仓里的源码"变得含糊 ✗。
 *
 * 曾经这里还有 `vue` / `vue-jsx-vapor` 两条 alias —— 那是"业务源码一行不改就切到
 * 自研运行时"的迁移桥。2026-09-30 源码已经全部直接写运行时入口，桥拆了。
 */
import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import lite from '@asunajs/lite/vite'

// 两个踩过的坑：
// 1. daisyUI 是 Tailwind 插件，不是 Vite 插件 —— 写进 plugins 会报
//    `handler is not a function`。它在 app.css 里用 `@plugin "daisyui"` 注册。
// 2. Tailwind 4 是 CSS-first，没有 tailwind.config.js（已废弃）。
export default defineConfig({
  // `runtime` 显式写包名：编译器生成的 `import { … } from '<runtime>'` 就靠它。
  // （不写也行 —— 插件的默认值已经是 `@asunajs/lite` —— 但显式写出来，
  //  "产物里的 import 长什么样"在这一行就看得见 ✓。）
  plugins: [tailwindcss(), lite({ runtime: '@asunajs/lite' })],
  server: {
    // 开发时后端就是 `mcloud`（同一份二进制，不带参数即起服务；默认 38123）。
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
    // （见框架仓 `asunajs/lite` 的 `docs/design.md` §8），`web/dist/assets` 有 20 个 chunk、
    // `index.html` 里有 1 条 `modulepreload`。这条设置**仍然保留**，因为：
    // 本仓的目标浏览器（macOS 上跑 WebUI 的 Chrome/Safari）全都支持
    // `rel=modulepreload`，垫片是给老浏览器兜底的，而这条路线里没有老浏览器。
    // 真要支持老浏览器时**先复核这一条**（这正是当初写下的那句提醒）。
    modulePreload: { polyfill: false },
  },
})
