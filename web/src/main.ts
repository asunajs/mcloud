import { createVaporApp } from 'vue'
import App from './app'
import './app.css'

// 不挂 vaporInteropPlugin：daisyUI 是纯 CSS 类，整棵树都在 Vapor 里，
// 没有需要建立边界的 VDOM 组件。
//
// ⚠ `createVaporApp` 在 Vite 里能从 `vue` 解析到，是因为 Vite 走 `import`
// 条件导出（→ `vue.runtime-with-vapor`）。在 Node 里 `require('vue')` 拿到的是
// runtime-only 那份、没有这个导出 —— 那是包的条件导出差异，不是配置错误。
createVaporApp(App).mount('#app')

/*
 * 收掉 index.html 里的首屏加载动画。
 * 正常情况下 `body:has(#app > *)` 那条 CSS 已经先把它隐藏了，这里只是**兜底**：
 * 没有 :has() 的浏览器（或样式没生效时）也不会让转圈一直压在界面上。
 */
document.getElementById('boot')?.remove()
