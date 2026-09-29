import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import vueJsxVapor from 'vue-jsx-vapor/vite'

// 两个踩过的坑：
// 1. daisyUI 是 Tailwind 插件，不是 Vite 插件 —— 写进 plugins 会报
//    `handler is not a function`。它在 app.css 里用 `@plugin "daisyui"` 注册。
// 2. Tailwind 4 是 CSS-first，没有 tailwind.config.js（已废弃）。
export default defineConfig({
  plugins: [
    tailwindcss(),
    vueJsxVapor({
      macros: true,
      // 刻意不开 interop：daisyUI 是纯 CSS 类，不需要 VDOM 组件孤岛。
      interop: false,
    }),
  ],
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
    // 它是给「不支持 `rel=modulepreload` 的浏览器」用的 polyfill，而本产物是
    // **单 chunk**（HTML 里 `<link rel="modulepreload">` 数量为 0，见
    // `docs/architecture.md` §10.6）⇒ 那条垫片**没有任何东西可垫**，
    // 纯粹白带 0.7KB。真要做按需拆包时记得回来复核这一条。
    modulePreload: { polyfill: false },
  },
})
