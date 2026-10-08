/**
 * 配置中心（`/tasks/config`，以及从任务卡进来的 `/tasks/<任务名>/config`）。
 *
 * # 这一页是什么（用户口径 2026-10-05，三轮定死）
 *
 * 1. 「重构任务页面，参考这种风格：上面切换任务配置还是推送配置，下面左边列出所有配置大项，
 *    右边主窗口是具体配置」——参照的是 NCM（网易云音乐任务系统）的「策略配置」页；
 * 2. 「配置应该分为公共配置和账号私有配置」；
 * 3. 「我说的是**每个任务的配置**分为公共和账号私有」（举例：每个账号的推送不一样、
 *    上传路径可能不一样）；
 * 4. 「我准备拆分一下，**推送单独给配置页面**」⇒ 推送搬去 `account-config-page.tsx`
 *    （`/accounts/settings`，账号页的子视图）；
 *    ⚠⚠ 2026-10-08 又搬了一次（用户口径「**很显然我说的和云盘账户没有关系**」＋
 *    「**丢弃现有情况，重写**」）：**推送渠道**是"每个登录用户一份"的 ⇒ 现在在
 *    「推送」页（`push-page.tsx` 的 `pushConfigCard()` ✓），
 *    `account-config-page.tsx` 只剩**账号级**设置与功能开关 ✓。
 * 5. 拆完之后「账号私有配置」那一半**只剩指路**（每账号一行 + 一个跳去推送页的按钮，
 *    而那一页从账号页也进得去）⇒ 用户 2026-10-05 看完图一句「去掉」⇒ **整块删掉**，
 *    本页就只剩「公共配置」。
 *
 * ⚠ 删的是**那一半的界面**，不是口径：「每个任务分公共 / 账号私有」仍然成立 —— 等上游做出
 * 按账号的项（上传路径这类）再把那半加回来。今天做不成按账号开关的原因也还在：后端的
 * `skipTasks` 存的是**上游任务 id**（`Vec<i64>`；判定见 `mcloud-tasks/src/app_task.rs` 的
 * `is_configured_skip`），而界面上只有任务名 ⇒ 拿到 id 之前"跳过这个任务"做不成开关 ✗。
 * 「画一个存不下去的控件比空着更糟」这条不变 ✓。
 *
 * ⇒ 本页右栏只有一半：**公共配置**（这个任务的默认参数，`ui/param-field.tsx`），
 *    落盘走 `PUT /api/tasks/{name}/config`。**按账号**的账号级设置全在 `/accounts/settings`
 *    那一页；而**推送渠道**是**登录用户级**的，在「推送」页（`/push`）✓（2026-10-08）。
 *
 * # 地址
 *
 * `/tasks/config` = "我就想看看有哪些配置"；`/tasks/<任务名>/config` = 任务卡上点「配置」
 * （预选好那一项）。两种进法同一页，深链、刷新、后退都成立。
 *
 * ⚠ 它**不是第 10 条导航项**：`nav.ts` 里"侧栏 9 条封顶"是硬上限，而配置页属于「任务」
 * （`parsePath` 按第一段归属 ⇒ 侧栏仍高亮「任务」）。子路由的代价与前提写在 `ui/router.ts`
 * 文件头（末段**不能带点**）。
 *
 * # 移动端形态
 *
 * 两栏在 375 宽上是放不下的（左栏被挤成一条竖线那种事故在本仓发生过两次，见
 * `docs/design/cli-webui-design.md` §2.2.3）⇒ 手机上改成**列表 → 点进详情**的下钻，
 * 详情页顶上有一条「← 全部任务配置」的返回。
 *
 * ⚠⚠ **两种形态只渲染一份**（`wide` 这个 matchMedia 信号决定），**不是** `hidden md:block`
 * + `md:hidden` 两份都铺进 DOM —— 后者节点数翻倍，2026-10-05 历史页实测被抓出来过
 * （DevTools 里 30,124 个节点；见 `history-page.tsx` 里那段 ⚠）。断点必须与 CSS 的 `md`
 * 同一档（768px），对不上的后果是"两份都藏起来"（白屏）或"两份都显示"。
 *
 * # 未保存的改动不会被顺手丢掉
 *
 * 切一个任务、切回任务页再回来……草稿都留在会话内存里（`taskDrafts`，模块级）。理由：这一页
 * 的"改一处、看一眼再改下一处"是常态，而"点了一下左边那一项，右边刚敲的全没了"是最容易让人
 * 骂人的那种静默丢失。代价说清：**不落盘**，刷新浏览器就没了 —— 真要长期保留请点「保存」。
 */

import { onMounted, onUnmounted, ref } from '@asunajs/lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  getTaskConfig,
  listCapabilities,
  putTaskConfig,
  type TaskConfigView,
  type TaskMeta,
  type TaskParamSpec,
} from '../api'
import { ParamField } from '../ui/param-field'
import { EmptyState } from '../ui/empty-state'
import { pushToast } from '../ui/toast'
import { IconAlert, IconClose, IconSearch } from '../ui/icons'

export interface ConfigCenterPageProps {
  /** URL 里预选的任务名（`/tasks/<任务名>/config`）；`/tasks/config` 进来时是 `null`。 */
  task: string | null
  /** 回任务中心。 */
  onExit: () => void
}

type LoadState = 'loading' | 'error' | 'ready'

interface TaskDraft {
  values: Record<string, unknown>
  saved: Record<string, unknown>
}

// ── 跨"进出一页"保留的偏好与草稿（模块级 = 会话内；不落盘）────────────────
//
// ⚠ 这些信号**只在 JSX 槽里读**，绝不在组件体顶层读：组件体里读一个模块级信号，
// 会让外壳（`app.tsx` 那个动态槽）订阅它 ⇒ 一改就整页重建、连列表滚动位置都没了。

/** 选中的任务名（空串 = 还没选）。 */
const pickedTask = ref('')
/** 手机上"详情是否已铺开"（false = 看列表）。宽屏不看它。 */
const drillOpen = ref(false)
/** 配置项搜索词。 */
const taskQuery = ref('')

/**
 * 草稿：任务名 → 那一份参数。只有**脏的**才留在里面。
 *
 * ⚠⚠ 这张是**普通 Map**，不是信号 —— 因为往它写的地方包括 `onUnmounted`。
 *
 * 卸载期间写信号 = 在"拆掉旧页面"的那条 effect 求值里写 ⇒ 订阅它的槽当场重跑、往正被
 * 摘掉的节点里插东西 ⇒ **旧页面不被替换而是被追加**。这不是推理，是实测：
 * 2026-10-05 第一版就是信号版，结果"任务页 → 配置页 → 任务页"每来回一次，`main`
 * 底下就多一份任务页（1 → 2 → 3 → 4 份，`formCards` 一起翻倍）；而门禁里那条
 * 「任务页：选账号 → 运行」也因为页面上出现了两份账号选择器而点错了元素 ✗。
 * 本仓 `ui/page-refresh.ts:44-53` 记的是同一类事故（"挂载期间写信号 ⇒ 切页时旧页面
 * 不被替换而被追加"），那里是靠普通变量绕开的，这里同理。
 *
 * ⇒ 需要"草稿变了、左边徽章跟着变"的地方，读下面那颗 `draftsRev`；
 *   而**只在点击处理器里**自增它（处理器里写信号是安全的：那一刻没有任何 effect 在求值；
 *   卸载那条路不碰它 —— 重挂之后整页重渲染，徽章本来就是新的）。
 */
const taskDrafts = new Map<string, TaskDraft>()
/** 草稿表的"版本号"。只在交互处理器里自增，用来唤醒读了草稿的那些槽（见上面那段 ⚠）。 */
const draftsRev = ref(0)

export const ConfigCenterPage = (props: ConfigCenterPageProps) => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')

  /** 注册表里的全部任务（含没有参数的 —— 这一页只列可配置的）。 */
  const tasks = ref<TaskMeta[]>([])

  /** 当前任务的参数说明。**必须拿它**：有没有参数、每格的类型/范围都由后端给。 */
  const specs = ref<TaskParamSpec[]>([])
  /** 表单当前值 / 上次载入（或保存）的那一份。 */
  const values = ref<Record<string, unknown>>({})
  const saved = ref<Record<string, unknown>>({})
  const detailState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  const detailError = ref('')
  /**
   * 保存失败的那一句（**弹窗 + 就近**，2026-10-08 用户口径「所有的提示都是优先弹窗式」）。
   *
   * ⚠ 它画在卡片标题那一行下面（「保存」就在那一行右端）——
   * 这一页的保存以前**只有**右上角 toast，长表单里滚动之后看不到，
   * 而 `detailError` 是"读不出来"（配重试），不是"存不进去"，两者不能混用 ✓。
   */
  const saveError = ref('')
  const saving = ref(false)
  /**
   * 请求序号：连着点两个任务时，先发的那个响应可能后到 —— 迟到的不能覆盖新的选中
   * （否则右边显示的是 A 的标题、B 的字段）。
   */
  let taskSeq = 0

  // ── 宽屏 / 手机（两种形态只渲染一份，见文件头）────────────────────────
  const WIDE_QUERY = '(min-width: 768px)'
  const wide = ref(matchMedia(WIDE_QUERY).matches)
  let mq: MediaQueryList | undefined
  const onWideChange = () => {
    if (mq) wide.value = mq.matches
  }

  /**
   * 登记「刷新」给外壳 navbar（与各页同一条约定，见 `ui/page-refresh.ts`）。
   *
   * # ⚠⚠ 为什么要 `queueMicrotask` 把它推到挂载之后
   *
   * 这一页是**子视图**（任务页里切出来的）。lite 里子组件的 `onMounted` **先于**
   * 父页面（任务页）的 `onMounted` 跑 ⇒ 若在这里直接登记，紧接着父页面那份会把它
   * **盖掉** ✗ ⇒ 表现是「在配置页点刷新，刷的是任务列表、不是这份配置」。
   * 推到微任务里就落在外层之后 ⇒ 子视图这份生效 ✓。
   *
   * 卸载时把外层那份**还回去**（`prev`）而不是置 `null` —— 置 null 会让回到任务页后
   * 那个刷新图标点了没反应 ✗。`alive` 处理"还没轮到这个微任务就被卸载"的竞态。
   */
  let alive = true
  let prevRefresh: ReturnType<typeof setPageRefresh> = null

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const caps = await listCapabilities()
      // 隐藏任务不进列表（与任务页同一条口径：`hidden` 是"别在界面上出现"的意思）
      tasks.value = caps.tasks.filter((t) => !t.hidden)
      ensureSelection()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '读不到任务列表'
      state.value = 'error'
    }
  }

  /** 可配置的任务 = 有参数的（签到族那些没参数 ⇒ 不进这一页）。 */
  const configurable = (): TaskMeta[] => tasks.value.filter((t) => t.params.length > 0)

  /** 搜索过滤后的任务（选中的那个即使被搜掉也留着，否则右边会"悬空"）。 */
  const taskItems = (): TaskMeta[] => {
    const q = taskQuery.value.trim().toLowerCase()
    const list = configurable()
    if (!q) return list
    return list.filter(
      (t) =>
        t.name === pickedTask.value ||
        [t.title, t.name, t.description].some((x) => (x ?? '').toLowerCase().includes(q)),
    )
  }

  /** 搜索框只在项数多时才出现（三五个配置项扫一眼就到底，多一个输入框只是噪音）。 */
  const searchable = (): boolean => configurable().length > 6

  const ensureSelection = () => {
    if (!pickedTask.value || !taskItems().some((t) => t.name === pickedTask.value)) {
      pickedTask.value = configurable()[0]?.name ?? ''
    }
    if (pickedTask.value && detailState.value === 'idle') void loadTaskConfig(pickedTask.value)
  }

  // ── 任务：读 / 存 / 草稿 ──────────────────────────────────────────────

  const loadTaskConfig = async (name: string) => {
    detailState.value = 'loading'
    detailError.value = ''
    const seq = ++taskSeq
    try {
      const v: TaskConfigView = await getTaskConfig(name)
      if (seq !== taskSeq || pickedTask.value !== name) return
      specs.value = v.specs ?? []
      const fresh = { ...v.params }
      const draft = taskDrafts.get(name)
      // ⚠ 草稿优先：切走前没保存的改动，回来时还在（而不是被服务端那份悄悄盖掉）。
      // `saved` 也跟着草稿走 —— 否则"未保存修改"会显示成"已经保存过了"。
      values.value = draft ? { ...draft.values } : fresh
      saved.value = draft ? { ...draft.saved } : fresh
      detailState.value = 'ready'
    } catch (e) {
      if (seq !== taskSeq) return
      detailError.value = e instanceof Error ? e.message : '读不到这个任务的配置'
      detailState.value = 'error'
    }
  }

  /** 逐格比"当前值 / 上次保存的那一份"（比整体 `JSON.stringify` 更抗键序变化）。 */
  const taskDirty = (): boolean => {
    for (const s of specs.value) {
      if (JSON.stringify(values.value[s.name] ?? null) !== JSON.stringify(saved.value[s.name] ?? null)) {
        return true
      }
    }
    return false
  }

  /** 把当前这一份脏草稿收进模块级表（干净的就把旧草稿删掉）。⚠ 只写普通 Map。 */
  const stashTaskDraft = () => {
    const name = pickedTask.value
    if (!name || detailState.value !== 'ready') return
    if (taskDirty()) taskDrafts.set(name, { values: values.value, saved: saved.value })
    else taskDrafts.delete(name)
  }

  /**
   * 读一份草稿 —— **顺带订阅 `draftsRev`**。
   *
   * 草稿表是普通 Map（见文件头那段 ⚠⚠），改它自己不会触发任何重渲染；而左边那一行
   * 要跟着亮/灭"未保存"。所以这里先摸一下那颗版本信号：真正读了草稿的槽因此订阅上它，
   * 至于"什么时候该自增"只有点击处理器知道（卸载那条路不许碰它）。
   */
  const draftOfTask = (name: string): TaskDraft | undefined => {
    void draftsRev.value
    return taskDrafts.get(name)
  }

  const selectTask = (name: string) => {
    saveError.value = ''
    if (name !== pickedTask.value) stashTaskDraft()
    draftsRev.value++
    pickedTask.value = name
    // 手机上点了就铺详情；宽屏两边都在，这个标志不被读
    drillOpen.value = true
    void loadTaskConfig(name)
  }

  const saveTask = async () => {
    const name = pickedTask.value
    if (!name) return
    saveError.value = ''
    saving.value = true
    try {
      const view = await putTaskConfig(name, values.value)
      specs.value = view.specs ?? specs.value
      const fresh = { ...view.params }
      values.value = fresh
      saved.value = fresh
      taskDrafts.delete(name)
      draftsRev.value++
      pushToast('success', '已保存 —— 任务页点「运行」时会带上这份参数')
    } catch (e) {
      const msg = e instanceof Error ? e.message : '保存失败'
      saveError.value = msg
      // 弹窗优先：错误类 toast 不自动消失（见 `ui/toast.ts`）
      pushToast('error', msg)
    } finally {
      saving.value = false
    }
  }

  /** 放弃修改：回到上次载入/保存的那一份（**不落盘**）。 */
  const discardTask = () => {
    values.value = { ...saved.value }
    stashTaskDraft()
    draftsRev.value++
    pushToast('info', '已回到上次保存的那一份')
  }

  /** 填回默认值：只改表单，**不落盘** —— 想生效还得点保存（用户随时能反悔）。 */
  const resetTaskDefaults = () => {
    const next: Record<string, unknown> = {}
    for (const s of specs.value) next[s.name] = s.default
    values.value = next
    pushToast('info', '已填回默认值，点「保存」才会生效')
  }

  onMounted(() => {
    void load()
    // ⚠ 推到微任务：子视图的 onMounted 先于父页面跑（见上面那段注释）
    queueMicrotask(() => {
      if (!alive) return
      prevRefresh = setPageRefresh({
        run: () => {
          void load()
          if (pickedTask.value) void loadTaskConfig(pickedTask.value)
        },
      })
    })
    mq = matchMedia(WIDE_QUERY)
    mq.addEventListener('change', onWideChange)
    onWideChange()
  })

  onUnmounted(() => {
    alive = false
    // 走之前把这一份收进草稿 —— 回来时"我刚才敲的那些"还在。
    // ⚠ 这里**只写普通 Map，绝不碰任何信号**（含 `draftsRev`）：卸载期间写信号会让
    // 正在被摘掉的子树重跑、旧页面于是"不被替换而被追加"（见文件头那段 ⚠⚠ 的实测）。
    stashTaskDraft()
    setPageRefresh(prevRefresh)
    mq?.removeEventListener('change', onWideChange)
    mq = undefined
  })

  // ── 渲染 ──────────────────────────────────────────────────────────────

  /** 左边那一栏（宽屏是侧栏，手机是整屏列表）。 */
  const rail = () => (
    <div id="config-rail" class="flex flex-col gap-3">
      {/* 搜索框用**总数**判要不要出现（见 `searchable()`）—— ⚠ 一个 `input` 就是一条
          常驻的订阅，别让它为三五个配置项白占一行 */}
      {searchable() ? (
        <label class="input input-sm flex items-center gap-2">
          <IconSearch class="size-4 shrink-0 opacity-50" />
          <input
            type="search"
            class="min-w-0 flex-1"
            placeholder="搜任务名 / 标题…"
            aria-label="搜索配置项"
            value={taskQuery.value}
            onInput={(e) => (taskQuery.value = (e.target as HTMLInputElement).value)}
          />
        </label>
      ) : null}

      <div class="flex items-center justify-between px-1">
        <span class="text-xs font-medium opacity-70">可配置任务</span>
        <span class="badge badge-soft badge-sm">{configurable().length} 项</span>
      </div>

      {/* 作用域写在左栏上：这一页改的是**全实例共用**的那一份（按账号的在「账号配置」页） */}
      <p class="px-1 text-[11px] leading-4 opacity-60">
        全实例共用一份：改完哪个账号跑这个任务都用它。
      </p>

      {/* ⚠⚠ `.map()` 必须待在**元素**的子节点位置上：
          ① 写进三元分支的那个 map（`cond ? xs.map(...) : []`）走通用表达式路径 ⇒ 编译器
          当场红：「这里的 `key` 什么都不做」（本页第一版就是这样，`npm run build` 直接失败）；
          ② 片段（`<>…</>`）同样不行 —— 编译器把片段当"动态插入"处理，它里面的 map 一样
          认领不到 key。所以下面用 `<div>` 而不是 `<>` 包。
          判据与其它编译期边界见 `asunajs/lite` 的 `docs/guide.md` §8.2 / §10。 */}
      <div class="flex flex-col gap-1">
        {taskItems().map((t) => (
          <button
            key={t.name}
            type="button"
            aria-current={pickedTask.value === t.name ? 'true' : null}
            class={`flex min-h-11 w-full items-center gap-2 rounded-field px-3 py-2 text-left ${
              pickedTask.value === t.name ? 'bg-primary text-primary-content' : 'hover:bg-base-200'
            }`}
            onClick={() => selectTask(t.name)}
          >
            <span class="min-w-0 flex-1">
              <span class="block truncate text-sm font-medium">{t.title}</span>
              <span class="block truncate font-mono text-[11px] opacity-60">{t.name}</span>
            </span>
            {draftOfTask(t.name) ? (
              <span class="badge badge-warning badge-sm shrink-0">未保存</span>
            ) : null}
          </button>
        ))}
      </div>

      {/* 空态分两种：**这一栏本来就没有东西** 与 **搜索没命中** —— 前者要给出下一步，
          后者只要提醒换个词（混成一句"暂无数据"就两种都没说清） */}
      {configurable().length === 0 ? (
        <EmptyState
          title="没有任何任务需要配置。"
          hint="任务参数由后端注册表给：有参数的任务才会出现在这一栏（签到族那些没有参数）。"
        />
      ) : null}
      {configurable().length > 0 && taskItems().length === 0 ? (
        <EmptyState title="没有匹配的配置项。" hint="换个关键词试试（搜的是标题、任务名与说明）。" />
      ) : null}
    </div>
  )

  /** 详情顶上那条操作栏：未保存徽章 + 放弃修改 + 保存（宽屏与手机同一份）。 */
  const actions = () => (
    <div class="flex flex-wrap items-center gap-2">
      {taskDirty() ? <span class="badge badge-warning badge-sm">有未保存修改</span> : null}
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        disabled={!taskDirty()}
        onClick={() => discardTask()}
      >
        放弃修改
      </button>
      <button
        type="button"
        class="btn btn-sm btn-primary min-h-11 md:min-h-0"
        disabled={saving.value}
        onClick={() => void saveTask()}
      >
        {saving.value ? '保存中…' : '保存'}
      </button>
    </div>
  )

  /** 任务那一栏的详情：公共配置 + 账号私有配置两半。 */
  const detail = () => (
    <div class="card min-w-0 bg-base-100 shadow-sm">
      <div class="card-body gap-4">
        {detailState.value === 'loading' ? (
          <div class="flex justify-center py-16">
            <span class="loading loading-lg text-primary" />
          </div>
        ) : null}

        {detailState.value === 'error' ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{detailError.value}</span>
            <button
              type="button"
              class="btn btn-sm btn-soft"
              onClick={() => void loadTaskConfig(pickedTask.value)}
            >
              重试
            </button>
          </div>
        ) : null}

        {detailState.value === 'ready' ? (
          <>
            <div class="flex flex-wrap items-start justify-between gap-2">
              <h2 class="card-title min-w-0 text-base">
                {tasks.value.find((t) => t.name === pickedTask.value)?.title ?? pickedTask.value}
                <span class="ml-1 font-mono text-xs font-normal opacity-40">{pickedTask.value}</span>
              </h2>
              {actions()}
            </div>

            {/* ⚠ 失败那一句在**标题行下面**（保存键就在那一行右端），不是表单底部 ✓ */}
            {saveError.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <IconAlert class="size-4 shrink-0" />
                <span class="break-words">{saveError.value}</span>
              </div>
            ) : null}

            {/* ── 公共配置：这个任务的默认参数，所有账号共用这一份 ─────────── */}
            <div class="flex flex-col gap-3">
              <div class="flex flex-wrap items-center gap-2">
                <h3 class="text-sm font-medium">公共配置</h3>
                <span class="badge badge-soft badge-sm">所有账号共用</span>
              </div>
              <p class="text-sm opacity-70">
                这个任务的默认参数：任务页点「运行」时会带上它，哪个账号跑都一样。
                定时任务与编排步骤各自带自己的参数，不受这一页影响。
              </p>

              {specs.value.length === 0 ? (
                <p class="text-sm opacity-70">这个任务没有可配置的参数。</p>
              ) : (
                <div class="grid min-w-0 gap-4 lg:grid-cols-2">
                  {specs.value.map((s) => (
                    <div key={s.name} class="min-w-0">
                      {/* ⚠ `get` 是 thunk：在槽里写 `value={values.value[s.name]}`
                          会让这个槽订阅值信号 ⇒ 每敲一个字整块重建、焦点全丢
                          （2026-10-03 review §2.2）。 */}
                      <ParamField
                        spec={s}
                        get={() => values.value[s.name]}
                        onInput={(v) => {
                          values.value = { ...values.value, [s.name]: v }
                        }}
                      />
                    </div>
                  ))}
                </div>
              )}

              <div class="card-actions justify-end">
                <button
                  type="button"
                  class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                  disabled={saving.value}
                  onClick={() => resetTaskDefaults()}
                >
                  填回默认值
                </button>
              </div>
            </div>

          </>
        ) : null}

        {detailState.value === 'idle' ? (
          <p class="py-10 text-center text-sm opacity-70">左边挑一个任务，右边改它的默认参数。</p>
        ) : null}
      </div>
    </div>
  )

  /** 宽屏：左右两栏（左栏自己滚，页面整体滚到底也不用回去找）。 */
  const wideBody = () => (
    <div class="grid min-w-0 gap-4 md:grid-cols-[15rem_minmax(0,1fr)]">
      {/* ⚠ 侧栏 sticky 在 navbar 下面（navbar 是 `sticky top-0`，高约 4rem）；
          超长列表自己滚，别让整页跟着长 */}
      <div class="md:sticky md:top-20 md:max-h-[calc(100dvh-7rem)] md:self-start md:overflow-y-auto md:pr-1">
        {rail()}
      </div>
      {/* ⚠ `#config-detail` 这个 id 是给门禁用的锚（`interact.mjs` 要断言"换一项 ⇒
          右边换了"，而左右两边都印着同一个任务名 —— 只能靠容器区分） */}
      <div id="config-detail" class="min-w-0">
        {detail()}
      </div>
    </div>
  )

  /** 手机：列表 ↔ 详情，一次只出现一个（两个都铺进 DOM 会让节点数翻倍）。 */
  const narrowBody = () =>
    drillOpen.value ? (
      <div class="flex flex-col gap-3">
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 w-full justify-start"
          onClick={() => (drillOpen.value = false)}
        >
          ← 全部任务配置
        </button>
        <div id="config-detail" class="min-w-0">
          {detail()}
        </div>
      </div>
    ) : (
      rail()
    )

  // ⚠ 整页宽度封顶，与设置页同一条口径（规范 §2.2.4 那条"pc 端别拉满整屏"）：
  // 2560 宽的显示器上不封顶的话，右栏那个输入框会长到看不出边界。
  return (
    <div class="mx-auto flex w-full max-w-7xl flex-col gap-4">
      <div class="flex flex-wrap items-center gap-2">
        <button
          type="button"
          class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
          aria-label="回任务中心"
          onClick={() => props.onExit()}
        >
          <IconClose class="size-4" />
        </button>
        <h1 class="text-base font-medium">任务配置</h1>
        <span class="text-xs opacity-60">每个任务的默认参数，所有账号共用这一份</span>
      </div>

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (wide.value ? wideBody() : narrowBody()) : null}
    </div>
  )
}
