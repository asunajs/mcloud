/**
 * 提示条渲染。与 `toast.ts` 的 store 分离：那个管状态，这个管样子。
 *
 * 位置固定在右上角（§5.11）—— 不用底部，会和移动端 dock 打架。
 *
 * # ⚠⚠ 容器**常驻**、列表走 `{toasts.value.map(...)}` 那个槽（2026-10-08 实测踩到）
 *
 * 原来这里是 `if (toasts.value.length === 0) return null` —— 在组件体里读信号。
 * 组件体只在**外层那个动态槽**跑的时候执行一次，所以那次读是**订阅在外层**的：
 * 它挂在 `app.tsx` 的"已登录外壳"那支里时能刷新（那一支是个动态槽 ✓），
 * 但把它提到**根**上之后外层不再是动态槽 ⇒ `toasts` 变了**没有任何东西重跑** ✗
 * —— 表现是"toast 一条都不显示"，而"不显示"和"没报错"在界面上长得一模一样。
 * （实测：2026-10-08 改完根挂载后，门禁里 8 条依赖 toast 的断言一起红。）
 *
 * ⇒ 现在容器常驻（空着时只是一个不可见、不吃指针事件的空 div），
 * 列表交给 JSX 里那个 `{}` 槽 —— **槽自己带 effect**，读 `toasts` 才真的订阅得上 ✓。
 *
 * # ⚠⚠ 容器 `pointer-events-none`、提示条自己 `pointer-events-auto`
 *
 * 错误类 toast **不自动消失**（`toast.ts` 的口径：那句话要读完才能决定下一步）。
 * 于是它会一直盖在右上角 —— 而那一带正是很多页面的主操作（推送页的「保存」就在
 * 卡片标题行右端）。容器不吃指针事件之后：**底下压着的按钮照点得中**，
 * 只有提示条自己那一块（含它的 ✕）拦事件 ✓。窄屏（320）尤其需要这条。
 *
 * ⚠ 尺寸不需要我们再管：daisyUI 的 `.toast` 自带
 * `width: max-content; max-width: calc(100vw - 2rem)` ⇒ 长文案换行、不溢出视口 ✓
 * （消息里那个 `break-words` 是给超长无空格串兜底的 ✓）。
 */

import { dismissToast, toasts, TOAST_CLASS } from './toast'
import { IconClose } from './icons'

export const ToastView = () => (
  <div class="toast toast-end toast-top pointer-events-none z-50">
    {toasts.value.map((t) => (
      <div
        key={t.id}
        class={`alert pointer-events-auto ${TOAST_CLASS[t.level]} text-sm`}
        role="status"
      >
        <span class="break-words">{t.message}</span>
        <button
          type="button"
          class="btn btn-square btn-ghost btn-xs"
          aria-label="关闭提示"
          onClick={() => dismissToast(t.id)}
        >
          <IconClose class="size-3" />
        </button>
      </div>
    ))}
  </div>
)
