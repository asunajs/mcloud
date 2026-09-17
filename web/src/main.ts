import { createVaporApp } from 'vue'
import App from './app'
import './app.css'

// 不挂 vaporInteropPlugin：daisyUI 是纯 CSS 类，整棵树都在 Vapor 里，
// 没有需要建立边界的 VDOM 组件。
createVaporApp(App).mount('#app')
