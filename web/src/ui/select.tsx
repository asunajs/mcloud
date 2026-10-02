/**
 * 下拉选择器（单选 / 多选）—— 按 **TDesign** 的口径重做（用户口径 2026-10-02：
 * 「重新设计本项目下拉框，模仿 tdesign」）。
 *
 * # 为什么不继续用原生 `<select>`
 *
 * 原生只能单选：弹层（`::picker(select)`）里放不了复选框、搜索框、自定义行，
 * 「多选 + 全部账号」这个形态（用户口径 2026-10-01）它压根表达不了。
 * 而且弹层样式只有新版 Chrome 认，其它浏览器直接掉回系统控件。
 *
 * # 为什么不用 `<details>`（本组件替换掉的那版）
 *
 * `<details>` 的面板**在文档流里**：展开时把卡片撑高、把下面的内容顶下去 ——
 * 而 TDesign 的下拉是**浮层**；它也没有"点外部关闭""键盘上下选"。
 *
 * # TDesign 的口径（抄自 `tdesign-common/style/web/components/select/_var.less`）
 *
 * | 项 | TDesign | 本仓取值 |
 * |---|---|---|
 * | 触发器高度 | 32px（`--td-comp-size-m`） | **40px**（与 `.input` 同高）；`size="sm"` = 32px |
 * | 触发器圆角 | 3px（`--td-radius-default`） | `--radius-field`（8px，本仓档位） |
 * | 边框 | `--td-border-level-2-color`（#ddd） | `base-content 20%` 混色（与 `.input` 同一条） |
 * | hover 边框 | 品牌色 | 同左 |
 * | focus | 品牌边框 + `0 0 0 2px` 品牌 20% | 沿用本仓那条未分层的 `.select:focus`（主色 + 3px 光环） |
 * | 面板圆角 / 阴影 | 6px / `@shadow-2` 三层 | `--radius-box` / **阴影照抄三层** |
 * | 面板最大高 | 300px + `overscroll-behavior: contain` | 同左 |
 * | 面板内边距 | 6px（`@pop-padding-m`） | 同左 |
 * | 选项高 / 内边距 | 28px / `0 8px` | 桌面 28px、窄屏 44px（本仓 §6.1 触控硬要求）/ `0 8px` |
 * | 选项 hover | `#f3f3f3`（`--td-gray-color-1`） | `base-content 8%` 混色（跟随明暗主题） |
 * | 选中项 | 品牌色文字 + `--td-brand-color-light` 底 | `primary` + `primary 10%` 底 |
 * | 空数据 | 居中、`--td-text-color-disabled` | 同左 |
 *
 * ⚠ 高度与圆角**刻意不照抄**：本仓的输入框是 40px / 8px，下拉跟它们对齐比
 * "像素级复刻 TDesign" 更要紧 —— 同一行里两个控件不等高，一眼就是坏的。
 *
 * # 弹层挂在 `body` 上，不是留在原地
 *
 * 两个原因，都是实测会咬人的：
 *  1. `.modal-box` 是 `overflow-y: auto` —— 留在里面会被裁掉（兑换页「抢兑计划」、
 *     编排页的运行/定时弹窗都是弹窗里的表单）。
 *  2. 本仓的玻璃卡片带 `backdrop-filter`，而它会成为 `position: fixed` 后代的
 *     **包含块** ⇒ "按视口坐标算 fixed"会算歪。
 * 挂到 `body` 之后两条都不存在。
 *
 * # ⚠ lite 的两条语义决定了这个文件为什么长这样
 *
 * lite **没有重渲染**：父级槽一重跑就**新建组件实例**（旧的连节点一起拆掉）。
 * 而面板是**复用的**（挂在 body 上、只 `mount` 一次）。两条叠起来有三个坑，
 * 都在下面用 `⚠` 标了：
 *  ① 展开状态不能放在实例里 ⇒ 放模块级信号 `opened`；
 *  ② **不能**在组件体里写那个信号（那是在 effect 求值当中写 ⇒ 父级槽重跑 ⇒
 *     再建实例 ⇒ 再写，lite 直接抛「循环更新」）⇒ 状态更新一律写在事件处理器里；
 *  ③ 面板里的回调闭包会**过期**（它指着第一次打开时那份 props）⇒ 点击按 `id`
 *     查 `fresh` 里最新那一份，不直接用闭包。
 */
import { mount, onMounted, onUnmounted, ref } from 'lite'
import { IconCheck, IconChevronDown, IconClose, IconSearch } from './icons'

export interface SelectOption {
  value: string
  label: string
  /** 行尾的次要说明（账号的平台、任务的分组…）。 */
  hint?: string
  disabled?: boolean
}

interface SelectCommon {
  /**
   * 稳定标识。
   *
   * ⚠ **必须给，而且要在重建前后保持不变**：面板挂在 body 上、展开状态在模块级，
   * 认人只能靠它（也用来查 `fresh` 里最新那份回调）。
   */
  id: string
  options: SelectOption[]
  placeholder?: string
  size?: 'sm' | 'md'
  disabled?: boolean
  class?: string
  ariaLabel?: string
  /** 选项多时给个搜索框（TDesign 的 `filterable`）。 */
  filterable?: boolean
  emptyText?: string
  /**
   * 面板顶部一行**独立开关**（账号选择器的「全部账号」）。
   *
   * 它不是"把每一项都勾上"：后端口径里「全部」是 `accounts: []` = 我名下全部，
   * 以后加了账号也算 —— 勾每一项会把名单锁死（见 `account-picker.tsx`）。
   */
  allRow?: { label: string; active: boolean; onToggle: () => void }
}

export interface SingleSelectProps extends SelectCommon {
  multiple?: false
  value: string
  onChange: (value: string) => void
}

export interface MultiSelectProps extends SelectCommon {
  multiple: true
  value: string[]
  onChange: (value: string[]) => void
}

export type SelectProps = SingleSelectProps | MultiSelectProps

/** 面板要用的东西。**模块级**：面板被挂到 `body` 上，拿不到实例闭包。 */
interface PanelModel {
  id: string
  /** 触发器在视口里的位置（打开那一刻量的）。 */
  anchor: { top: number; left: number; width: number; bottom: number }
  options: SelectOption[]
  /** 已选值。**面板自己维护**（见 `pickFresh`）—— 组件的 props 只是初始值。 */
  values: string[]
  multiple: boolean
  allRow?: SelectCommon['allRow']
  filterable: boolean
  emptyText: string
  /**
   * `options` + `allRow` 的签名。
   *
   * ⚠ 用途是给"组件体里那次同步"做闸门（见 `Select` 里那段）：
   * 同一个签名最多写一次信号，环就走不起来。
   */
  sig: string
}

/** 当前展开的面板（全局只有一个：同一时刻只该开一个下拉）。 */
const opened = ref<PanelModel | null>(null)
/** 搜索词。放模块级：面板重挂、组件重建都不该把它丢掉。 */
const query = ref('')
/** 面板的视口坐标（`position: fixed` 用）。 */
const placement = ref('')

/**
 * 每个 `id` **最近一次渲染**拿到的"点选"回调（⚠ 坑 ③）。
 *
 * 面板是复用的，它里面那个闭包会一直指着**第一次打开时**那份 props；多选连勾两项时，
 * 第二项就会拿着过期的 `values` 算名单（症状：勾了 B 把 A 顶掉；账号选择器更糟 ——
 * 上游 `account-picker` 的 diff 也会拿旧的 `picked`）。所以点击一律按 id 查最新那份。
 *
 * ⚠ 这里写的是**普通 Map**，不是信号 —— 在组件体里写信号会触发坑 ②。
 */
const fresh = new Map<string, (v: string) => void>()

/** 面板里点了某一项。 */
const pickById = (v: string) => {
  const id = opened.value?.id
  if (!id) return
  fresh.get(id)?.(v)
}

/** 挂到 `body` 上的那个容器 + 卸载器。 */
let portal: { host: HTMLElement; dispose: () => void } | null = null

/**
 * 按当前锚点算面板位置；下方放不下就翻到触发器上方。
 *
 * ⚠ **每次打开都要重量**：portal 只建一次、面板组件也就只 `onMounted` 一次 ——
 * 从"账号选择器"切到"弹窗里那个账号"时若不重量，面板会停在上一个触发器的位置
 * （2026-10-02 实测抓到的就是这个）。
 */
const layout = () => {
  const m = opened.value
  if (!m) return
  const gap = 4
  // TDesign 的面板最高 300px ⇒ 用它估"下面还放得下吗"
  const below = window.innerHeight - m.anchor.bottom
  const flip = below < 300 && m.anchor.top > below
  const top = flip ? m.anchor.top - gap : m.anchor.bottom + gap
  placement.value = flip
    ? `top:${top}px;left:${m.anchor.left}px;width:${m.anchor.width}px;translate:0 -100%`
    : `top:${top}px;left:${m.anchor.left}px;width:${m.anchor.width}px`
}

/**
 * 面板挂到哪个容器里。
 *
 * 默认 `body`：能同时躲开 `.modal-box` 的 `overflow-y:auto`（会裁掉）与玻璃卡片的
 * `backdrop-filter`（会成为 `position:fixed` 后代的包含块，坐标算歪）。
 *
 * ⚠⚠ **有模态框时必须挂进那个 `<dialog>`**：`showModal()` 会把对话框以外的**一切**
 * 变成 **inert**，而 inert 的内容**收不到点击** —— 面板会"看得见、点不着"。
 * 2026-10-02 实测（兑换页「定时抢兑」）：面板画在弹窗之上、选中态也对，但真鼠标点在
 * 选项上，事件目标是弹窗里的 `fieldset` ⇒ 被 `onDown` 当成"点外面"直接收起。
 * 挂进 `<dialog>` 之后它就不是 inert 内容了，同时仍在 top layer 里（对话框本身在）。
 *
 * 嵌套弹窗取**最后**一个（后开的在上面）。
 */
const portalMount = (): HTMLElement => {
  const dialogs = document.querySelectorAll<HTMLElement>('dialog[open]')
  return dialogs.length > 0 ? dialogs[dialogs.length - 1]! : document.body
}

const openPanel = (model: PanelModel) => {
  opened.value = model
  layout()
  if (portal) return
  const host = document.createElement('div')
  host.className = 'select-portal'
  portalMount().appendChild(host)
  // `mount` 返回卸载器；它会把 `onUnmounted` 的钩子一并跑掉
  portal = { host, dispose: mount(SelectPanel, host) }
}

/** 面板元素（同一时刻只有一个）。lite 没有 DOM ref，按类名取就够。 */
const panelEl = () =>
  document.querySelector<HTMLElement>('.select-portal .select-panel') ?? null

const closePanel = () => {
  opened.value = null
  query.value = ''
  if (!portal) return
  // 先把面板从 top layer 摘出来，再拆节点（不然它可能"留在最上层"一帧）
  panelEl()?.hidePopover?.()
  portal.dispose()
  portal.host.remove()
  portal = null
}

/** 实例序号：`onUnmounted` 里靠它判断"我是不是这个 id 的最后一代"。 */
let seq = 0
const latest = new Map<string, number>()

/**
 * 面板本身。**没有 props** —— 它读模块级信号。
 *
 * ⚠ lite 没有重渲染：组件体只跑一次，所以下面**所有**会变的东西都必须写在
 * 槽（`{…}`）里读信号，不能在函数体里 `const m = opened.value` 存下来。
 */
function SelectPanel() {
  /** 键盘高亮的下标（`ArrowDown`/`ArrowUp` 走它）。 */
  const cursor = ref(0)

  const visible = () => {
    const m = opened.value
    if (!m) return []
    const q = query.value.trim().toLowerCase()
    if (!m.filterable || q === '') return m.options
    return m.options.filter(
      (o) => o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q),
    )
  }

  const onDown = (e: MouseEvent) => {
    const t = e.target as HTMLElement | null
    if (!t || typeof t.closest !== 'function') return
    // 触发器自己管开/关；面板内部的点击（勾选项、点搜索框）都不该关
    if (t.closest('.select-portal') || t.closest('.select')) return
    closePanel()
  }

  const onKey = (e: KeyboardEvent) => {
    if (!opened.value) return
    if (e.key === 'Escape') {
      closePanel()
      return
    }
    const list = visible().filter((o) => !o.disabled)
    if (list.length === 0) return
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      cursor.value =
        e.key === 'ArrowDown'
          ? (cursor.value + 1) % list.length
          : (cursor.value - 1 + list.length) % list.length
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      const hit = list[Math.min(cursor.value, list.length - 1)]
      if (hit) pickById(hit.value)
    }
  }

  /**
   * 滚动就收起。
   *
   * 面板用的是**打开那一刻**量的坐标；页面（或弹窗）一滚就错位。
   * TDesign 会重算位置，这里选等价但更简单的行为 —— 收起。
   * ⚠ 必须放过面板自己的滚动：否则选项一多，滚一下就把面板关了。
   */
  const onScroll = (e: Event) => {
    const t = e.target as HTMLElement | null
    if (t && typeof t.closest === 'function' && t.closest('.select-portal')) return
    closePanel()
  }

  onMounted(() => {
    layout()
    /**
     * ⚠ **必须进 top layer**：`<dialog>` 用 `showModal()` 打开时整个进 top layer，
     * 而 top layer 画在**所有** z-index 之上 —— 面板挂在 body 上、z-index 2000 也没用。
     * 2026-10-02 实测：兑换页「定时抢兑」弹窗里的下拉，面板算得准准的，但
     * `elementFromPoint` 命中的是弹窗里的 `fieldset`（= 被盖住了）。
     * `popover="manual"` + `showPopover()` 把面板也放进 top layer ⇒ 后开的在上面。
     * 用 `manual` 而不是 `auto`：轻关闭（点外面）由上面的 `onDown` 自己管，
     * `auto` 会再加一层 UA 的关闭逻辑，两边打架。
     */
    panelEl()?.showPopover?.()
    // 可搜索时自动聚焦（TDesign 同）。同一时刻只有一个面板，按类名取当前那个就够 ——
    // lite 没有 DOM ref，为它引一套引用机制不划算。
    document.querySelector<HTMLInputElement>('.select-portal .select-search-input')?.focus()
    window.addEventListener('resize', closePanel)
    window.addEventListener('scroll', onScroll, true)
    document.addEventListener('mousedown', onDown, true)
    document.addEventListener('keydown', onKey)
  })
  onUnmounted(() => {
    window.removeEventListener('resize', closePanel)
    window.removeEventListener('scroll', onScroll, true)
    document.removeEventListener('mousedown', onDown, true)
    document.removeEventListener('keydown', onKey)
  })

  const picked = (v: string) => (opened.value?.values ?? []).includes(v)

  return (
    <div
      class="select-panel"
      style={placement.value}
      popover="manual"
      role="listbox"
      aria-multiselectable={opened.value?.multiple ? 'true' : 'false'}
      aria-label="选项"
    >
      {opened.value?.filterable ? (
        <div class="select-search">
          <IconSearch class="size-3.5 opacity-50" />
          <input
            type="text"
            class="select-search-input"
            placeholder="搜索"
            aria-label="搜索选项"
            value={query.value}
            onInput={(e) => {
              query.value = (e.target as HTMLInputElement).value
              cursor.value = 0
            }}
          />
        </div>
      ) : null}

      {opened.value?.allRow ? (
        <div
          role="option"
          aria-selected={opened.value?.allRow?.active ? 'true' : 'false'}
          class={`select-option ${opened.value?.allRow?.active ? 'select-option-on' : ''}`}
          onClick={() => opened.value?.allRow?.onToggle()}
        >
          <span
            class={`checkbox select-option-box ${opened.value?.allRow?.active ? 'checkbox-on' : ''}`}
            aria-hidden="true"
          />
          <span class="select-option-label">{opened.value?.allRow?.label}</span>
        </div>
      ) : null}

      {visible().length === 0 ? (
        <div class="select-empty">{opened.value?.emptyText ?? '暂无数据'}</div>
      ) : null}

      {visible().map((o) => (
        <div
          key={o.value}
          role="option"
          data-value={o.value}
          aria-selected={picked(o.value) ? 'true' : 'false'}
          class={`select-option ${picked(o.value) ? 'select-option-on' : ''} ${
            o.disabled ? 'select-option-off' : ''
          }`}
          onClick={() => {
            if (o.disabled) return
            pickById(o.value)
          }}
        >
          {opened.value?.multiple ? (
            <span
              class={`checkbox select-option-box ${picked(o.value) ? 'checkbox-on' : ''}`}
              aria-hidden="true"
            />
          ) : null}
          <span class="select-option-label">{o.label}</span>
          {o.hint ? <span class="select-option-hint">{o.hint}</span> : null}
          {opened.value?.multiple ? null : picked(o.value) ? (
            <IconCheck class="size-4 select-option-check" />
          ) : null}
        </div>
      ))}
    </div>
  )
}

/** 多选时触发器里最多平铺几个标签，其余折成 `+N`（TDesign 的 `minCollapsedNum` 同思路）。 */
const MAX_TAGS = 2

export function Select(props: SelectProps) {
  const isMulti = props.multiple === true
  const values: string[] = props.multiple === true ? props.value : props.value ? [props.value] : []
  // ⚠ 两个回调分开取：`props` 是联合类型，narrowing 进了闭包就不成立
  const onChangeMulti = props.multiple === true ? props.onChange : null
  const onChangeSingle = props.multiple === true ? null : props.onChange

  const labelOf = (v: string) => props.options.find((o) => o.value === v)?.label ?? v

  /**
   * `options` + `allRow` 的签名：值 + 禁用态 + 「全部」开关。
   * 账号列表、任务列表都是小数组（几十条），拼一遍不值一提。
   */
  const optionsSig = () =>
    `${props.allRow?.active ? 1 : 0}|${props.options
      .map((o) => `${o.value}${o.disabled ? '!' : ''}`)
      .join(',')}`

  /**
   * 点选一项。**这就是 `fresh` 里存的那份** —— 每次重建都会覆盖，于是它永远
   * 拿着最新的 `props`（⚠ 坑 ③）。
   *
   * ⚠ 名单的"当前值"取自**面板模型**而不是闭包里的 `values`：面板是复用的，
   * 连勾两项时闭包里那份是第一项之前的老值（⚠ 坑 ③ 的另一半）。
   */
  const pickFresh = (v: string) => {
    if (onChangeMulti) {
      const cur = opened.value?.values ?? values
      const next = cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v]
      const live = opened.value
      // 面板的勾选态**在这里**更新（事件处理器里，不是组件体里 —— 坑 ②）
      if (live) opened.value = { ...live, values: next }
      // 多选**不收起**：连着勾几个是常见操作（TDesign 同）
      onChangeMulti(next)
      return
    }
    closePanel()
    onChangeSingle?.(v)
  }
  fresh.set(props.id, pickFresh)

  /**
   * ⚠ 组件会因 props 变化被**重建**（卸载 + 新建），所以**不能**在卸载时无条件收面板
   * （多选每勾一项就会关一次）；只有"没有新一代顶上来"时才是真的走了（换页/关弹窗）。
   */
  seq += 1
  const mySeq = seq
  latest.set(props.id, mySeq)
  onUnmounted(() => {
    if (latest.get(props.id) === mySeq) closePanel()
  })

  const buildModel = (anchor: PanelModel['anchor']): PanelModel => ({
    id: props.id,
    anchor,
    options: props.options,
    values,
    multiple: isMulti,
    allRow: props.allRow,
    filterable: props.filterable === true,
    emptyText: props.emptyText ?? '暂无数据',
    sig: optionsSig(),
  })

  /**
   * 面板模型要**跟着 props 走**：勾了「全部账号」之后逐项要变灰、账号列表加载完要出现
   * —— 这些都在 `options` / `allRow` 里，而面板是复用的（不会自己重新拿到 props）。
   *
   * ⚠ 这一段是在**组件体**里写信号，正好踩坑 ②（effect 求值当中写 ⇒ 父级槽重跑 ⇒
   * 再建实例 ⇒ 再写 ⇒ lite 报「循环更新」，整页白掉）。所以拿 `sig` 做闸门：
   * 同一个签名只写一次，第二轮签名就相等了 ⇒ 环走不起来。
   * ⚠ 只同步 `options`/`allRow`，**不动 `values`**：多选的勾选态由面板自己维护
   * （`pickFresh`），这里覆盖回去会把刚勾上的那一下抹掉。
   */
  const sig = optionsSig()
  const live = opened.value
  if (live && live.id === props.id && live.sig !== sig) {
    opened.value = { ...live, options: props.options, allRow: props.allRow, sig }
  }

  const isOpen = () => opened.value?.id === props.id

  const toggle = (e: Event) => {
    if (props.disabled === true) return
    if (isOpen()) {
      closePanel()
      return
    }
    const el = e.currentTarget as HTMLElement
    const r = el.getBoundingClientRect()
    openPanel(buildModel({ top: r.top, left: r.left, width: r.width, bottom: r.bottom }))
  }

  const removeTag = (v: string, e: Event) => {
    // 别让这一下冒泡到触发器 —— 否则移除标签的同时又把面板开了
    e.stopPropagation()
    onChangeMulti?.(values.filter((x) => x !== v))
  }

  const triggerBody = () => {
    if (isMulti) {
      if (props.allRow?.active) {
        return <span class="select-text">{props.allRow.label}</span>
      }
      if (values.length === 0) {
        return <span class="select-placeholder">{props.placeholder ?? '请选择'}</span>
      }
      const shown = values.slice(0, MAX_TAGS)
      const rest = values.length - shown.length
      return (
        <span class="select-tags">
          {shown.map((v) => (
            <span key={v} class="select-tag">
              {labelOf(v)}
              <span
                class="select-tag-x"
                role="button"
                aria-label={`移除 ${labelOf(v)}`}
                onClick={(e) => removeTag(v, e)}
              >
                <IconClose class="size-3" />
              </span>
            </span>
          ))}
          {rest > 0 ? <span class="select-tag select-tag-more">+{rest}</span> : null}
        </span>
      )
    }
    const hit = props.options.find((o) => o.value === props.value)
    return hit ? (
      <span class="select-text">{hit.label}</span>
    ) : (
      <span class="select-placeholder">{props.placeholder ?? '请选择'}</span>
    )
  }

  return (
    /*
      ⚠ 用 `div[role=combobox]` 而**不是** `<button>`：多选的触发器里要放
      「标签 + 移除按钮」，而 `<button>` 里嵌可点元素是无效 HTML（浏览器会把
      它们拆出去）。所以这里手动管 `tabindex` / `aria-*` / 键盘。
    */
    <div
      id={props.id}
      role="combobox"
      aria-haspopup="listbox"
      aria-expanded={isOpen() ? 'true' : 'false'}
      aria-label={props.ariaLabel}
      aria-disabled={props.disabled === true ? 'true' : undefined}
      tabindex={props.disabled === true ? -1 : 0}
      /*
        `data-value`：当前值的**可读出口**。
        门禁（`lite/regress/interact.mjs`）要断言"选中的到底是哪个值"，而这是个
        div、没有 `select.value` 可读；顺带也方便在 DevTools 里一眼看出状态。
      */
      data-value={isMulti ? values.join(',') : props.value}
      /*
        `data-multiple`：单选 / 多选的**可读出口**，与 `data-value` 同一个理由
        （见上面那条注释）。门禁要能区分"这一栏是**多选**"与"这一栏整个没了 /
        又被改回单值控件"✗ —— 只断言"多选框在"的话，改回单选**照样绿** ✗。
      */
      data-multiple={isMulti ? 'true' : 'false'}
      class={`select ${props.size === 'sm' ? 'select-sm' : ''} ${props.class ?? ''} ${
        isOpen() ? 'select-open' : ''
      }`}
      onClick={toggle}
      onKeyDown={(e) => {
        const k = (e as KeyboardEvent).key
        if (k === 'ArrowDown' || k === 'Enter' || k === ' ') {
          e.preventDefault()
          if (!isOpen()) toggle(e)
        }
      }}
    >
      <span class="select-value">{triggerBody()}</span>
      <IconChevronDown class="size-4 select-arrow" />
    </div>
  )
}
