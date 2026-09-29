import { mount } from 'lite'
import App from './app'
import './app.css'

// 挂根。以前这里写的是 `createVaporApp(App).mount('#app')` —— 那是**从 Vue 迁过来时**
// 为了"入口一行都不用改"留的壳，壳下面就是这一句。2026-09-30 壳删了，入口跟着直写。
//
// ⚠ `mount` 返回的是卸载器，本项目**没人接**：整棵树只挂一次，
// 切页/切登录态都靠信号，没有"换个根再挂一次"的场景。
mount(App, '#app')

/*
 * 收掉 index.html 里的首屏加载动画。
 * 正常情况下 `body:has(#app > *)` 那条 CSS 已经先把它隐藏了，这里只是**兜底**：
 * 没有 :has() 的浏览器（或样式没生效时）也不会让转圈一直压在界面上。
 */
document.getElementById('boot')?.remove()
