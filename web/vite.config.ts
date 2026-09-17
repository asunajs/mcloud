import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import vueJsxVapor from 'vue-jsx-vapor/vite'

// 注意两件事，都是踩过的坑：
// 1. daisyUI 是 Tailwind 插件，不是 Vite 插件 —— 写进 plugins 会报
//    `handler is not a function`。它在 app.css 里用 `@plugin "daisyui"` 注册。
// 2. Tailwind 4 是 CSS-first，没有 tailwind.config.js（已废弃）。
export default defineConfig({
  plugins: [
    tailwindcss(),
    vueJsxVapor({
      macros: true,
      // 刻意不开 interop：daisyUI 是纯 CSS 类，不需要 VDOM 组件孤岛。
      // 将来若引入依赖虚拟 DOM 的库（如实时代码编辑器），再开启即可。
      interop: false,
    }),
  ],
  build: {
    outDir: 'dist',
    // 产物会被 rust-embed 编进二进制，体积敏感
    target: 'es2022',
    sourcemap: false,
  },
})
