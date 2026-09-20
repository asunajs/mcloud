/**
 * 未实现页面的**空态**。
 *
 * 为什么要有这个而不是留白：空态是"告诉用户下一步做什么"的地方（设计指南 §6.3、
 * §5.10）。写「暂无数据」四个字等于什么也没说。
 *
 * 每页的说明文案来自 `nav.ts` 的 `blurb` —— 单一来源，改一处即全局生效。
 */

import type { PageId } from '../ui/nav'
import { navItem } from '../ui/nav'

export interface PlaceholderPageProps {
  id: PageId
  onGo: (id: PageId) => void
}

export const PlaceholderPage = (props: PlaceholderPageProps) => {
  const item = navItem(props.id)

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">{item.title}</h1>
        <span class="badge badge-soft badge-warning">待实现</span>
      </div>

      <div class="card bg-base-100 shadow-sm">
        <div class="card-body items-center gap-3 py-12 text-center">
          <span class="text-base-content/30">
            <item.icon class="size-10" />
          </span>
          <p class="text-sm opacity-70">{item.blurb}</p>
          <p class="max-w-prose text-xs opacity-60">
            页面骨架已就位（导航、主题、响应式布局都已接通），业务内容待后端接口就绪后实现。
          </p>
          <div class="mt-2 flex flex-wrap justify-center gap-2">
            <button
              type="button"
              class="btn btn-sm btn-soft btn-primary"
              onClick={() => props.onGo('dashboard')}
            >
              回到总览
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
