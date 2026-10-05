/**
 * 配置中心（`/tasks/config`，以及从任务卡进来的 `/tasks/<任务名>/config`）。
 *
 * # 这一页是什么（用户口径 2026-10-05）
 *
 * 「重构任务页面，参考这种风格：上面切换任务配置还是推送配置，下面左边列出所有配置大项，
 * 右边主窗口是具体配置」——参照的是 NCM（网易云音乐任务系统）的「策略配置」页。
 * 本仓对应的两块配置是：
 *
 * | 顶部 tab | 左边的「配置大项」 | 右边 |
 * |---|---|---|
 * | **任务配置** | 有参数的任务（注册表给的） | 那个任务的默认参数（`ParamField`） |
 * | **推送配置** | 已登录的账号 | 那个账号的运行结果推送渠道 |
 *
 * # 为什么是**一页**而不是两页
 *
 * 两块都是"挑一个对象 → 改它的一份配置 → 保存"，骨架一模一样；分开做会有两套
 * 列表/详情/未保存提示，第三块配置出现时还要再抄一遍。而且它们本来就同源：
 * 用户口径（2026-10-02）「这些设置先取消显示，**设置后面单独弄**」—— 推送那一块
 * 本来藏在账号设置弹窗里、一直没渲染（`accounts-page.tsx` 的
 * `SHOW_ACCOUNT_SETTINGS_BLOCKS`），这里就是它的家 ✓。
 *
 * ⚠ 它**不是第 10 条导航项**：`nav.ts` 里"侧栏 9 条封顶"是硬上限，而配置页属于「任务」
 * （`parsePath` 按第一段归属 ⇒ 侧栏仍高亮「任务」）。子路由的代价与前提写在 `ui/router.ts`
 * 文件头（末段**不能带点**）。
 *
 * # 移动端形态（用户口径：「设计一个更加适合移动端的界面」）
 *
 * 两栏在 375 宽上是放不下的（左栏被挤成一条竖线那种事故在本仓发生过两次，见
 * `docs/design/cli-webui-design.md` §2.2.3）⇒ 手机上改成**列表 → 点进详情**的下钻，
 * 详情页顶上有一条「← 全部…」的返回。
 *
 * ⚠⚠ **两种形态只渲染一份**（`wide` 这个 matchMedia 信号决定），**不是** `hidden md:block`
 * + `md:hidden` 两份都铺进 DOM —— 后者节点数翻倍，2026-10-05 历史页实测被抓出来过
 * （DevTools 里 30,124 个节点；见 `history-page.tsx` 里那段 ⚠）。断点必须与 CSS 的 `md`
 * 同一档（768px），对不上的后果是"两份都藏起来"（白屏）或"两份都显示"。
 *
 * # 未保存的改动不会被顺手丢掉
 *
 * 切一个配置项、切回任务页再回来、切到浏览器别的页签……草稿都留在会话内存里
 * （`taskDrafts` / `accountDrafts`，模块级）。理由：这一页的"改一处、看一眼再改下一处"
 * 是常态，而"点了一下左边那一项，右边刚敲的全没了"是最容易让人骂人的那种静默丢失。
 * 代价说清：**不落盘**，刷新浏览器就没了 —— 真要长期保留请点「保存」。
 */

import { onMounted, onUnmounted, ref } from 'lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  getAccountSettings,
  getTaskConfig,
  listAccounts,
  listCapabilities,
  putAccountSettings,
  putTaskConfig,
  type AccountSettings,
  type AccountSummary,
  type TaskConfigView,
  type TaskMeta,
  type TaskParamSpec,
} from '../api'
import { ParamField } from '../ui/param-field'
import { createPushEditor, type PushEditorSnapshot } from '../ui/push-editor'
import { EmptyState } from '../ui/empty-state'
import { pushToast } from '../ui/toast'
import { navigate } from '../ui/router'
import { IconClose, IconSearch } from '../ui/icons'

export interface ConfigCenterPageProps {
  /** URL 里预选的任务名（`/tasks/<任务名>/config`）；`/tasks/config` 进来时是 `null`。 */
  task: string | null
  /** 回任务中心。 */
  onExit: () => void
}

type LoadState = 'loading' | 'error' | 'ready'
type TabId = 'task' | 'push'

/** 一份任务参数草稿：表单值与"上次保存/载入的那一份"（未保存修改就是两者不等）。 */
interface TaskDraft {
  values: Record<string, unknown>
  saved: Record<string, unknown>
}

// ── 跨"进出一页"保留的偏好与草稿（模块级 = 会话内；不落盘）────────────────
//
// ⚠ 这些信号**只在 JSX 槽里读**，绝不在组件体顶层读：组件体里读一个模块级信号，
// 会让外壳（`app.tsx` 那个动态槽）订阅它 ⇒ 一改就整页重建、连列表滚动位置都没了。
// 本仓 `tasks-page.tsx` 的 `activeGroup` 是同一条口径（那个文件里写着为什么）。

/** 当前 tab。用户切走再回来不该被丢回第一个 tab。 */
const activeTab = ref<TabId>('task')
/** 选中的任务名 / 账号 id（空串 = 还没选）。 */
const pickedTask = ref('')
const pickedAccount = ref('')
/** 手机上"详情是否已铺开"（false = 看列表）。宽屏不看它。 */
const drillOpen = ref(false)
/** 配置项搜索词（只筛任务那一栏）。 */
const taskQuery = ref('')
/**
 * 草稿：任务名 → 那一份参数；账号 id → 共享编辑器的一份快照。只有**脏的**才留在里面。
 *
 * ⚠⚠ 这两张是**普通 Map**，不是信号 —— 因为往它们写的地方包括 `onUnmounted`。
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
const accountDrafts = new Map<string, PushEditorSnapshot>()
/** 草稿表的"版本号"。只在交互处理器里自增，用来唤醒读了草稿的那些槽（见上面那段 ⚠）。 */
const draftsRev = ref(0)

export const ConfigCenterPage = (props: ConfigCenterPageProps) => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')

  /** 注册表里的全部任务（含没有参数的 —— 那一栏只列可配置的）。 */
  const tasks = ref<TaskMeta[]>([])
  const accounts = ref<AccountSummary[]>([])

  // ── 任务那半边 ────────────────────────────────────────────────────────
  /** 当前任务的参数说明。**必须拿它**：有没有参数、每格的类型/范围都由后端给。 */
  const specs = ref<TaskParamSpec[]>([])
  /** 表单当前值 / 上次载入（或保存）的那一份。 */
  const values = ref<Record<string, unknown>>({})
  const saved = ref<Record<string, unknown>>({})
  const detailState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  const detailError = ref('')
  const saving = ref(false)
  /**
   * 请求序号：连着点两个任务时，先发的那个响应可能后到 —— 迟到的不能覆盖新的选中
   * （否则右边显示的是 A 的标题、B 的字段）。
   */
  let taskSeq = 0

  // ── 推送那半边 ────────────────────────────────────────────────────────
  //
  // 编辑器本体在 `ui/push-editor.tsx`（账号设置弹窗与这里**共用一份**，
  // 十条渠道的表单不可能维护两遍）。这里只负责"挑账号、读设置、整体写回"。
  const push = createPushEditor()
  const pushState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  const pushError = ref('')
  const pushSaving = ref(false)
  /**
   * 这个账号**完整**的一份设置（`GET` 回来的）。
   *
   * ⚠ `PUT /api/accounts/{id}/settings` 是**整体替换**：备份等待、凭据刷新阈值、
   * 跳过的任务这些字段这里一个都不显示，但保存时必须原样带回去 —— 漏一个就是把用户
   * 没碰过的设置静默抹掉。
   */
  const acctSettings = ref<AccountSettings | null>(null)
  /** 载入（或保存）之后的那一份快照文本：未保存修改就是它与当前快照不等。 */
  const pushBaseline = ref('')
  let acctSeq = 0

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

  /** 刷新 = 列表 + 当前选中的那一份（都是重新从后端拉，草稿被服务端那份覆盖）。 */
  const refresh = () => {
    void load()
    if (activeTab.value === 'task' && pickedTask.value) void loadTaskConfig(pickedTask.value)
    if (activeTab.value === 'push' && pickedAccount.value) void loadAccountSettings(pickedAccount.value)
  }

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [caps, accts] = await Promise.all([listCapabilities(), listAccounts()])
      // 隐藏任务不进列表（与任务页同一条口径：`hidden` 是"别在界面上出现"的意思）
      tasks.value = caps.tasks.filter((t) => !t.hidden)
      accounts.value = accts
      state.value = 'ready'
      ensureSelection()
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '读不到配置清单'
      state.value = 'error'
    }
  }

  /** 有参数的才算"一个配置大项"（没参数的任务点进去只有一句"没有可配置的参数"）。 */
  const configurable = (): TaskMeta[] => tasks.value.filter((t) => t.params.length > 0)

  /** 侧栏那一栏要显示的任务：可配置的 + **URL 预选进来的那一个**（哪怕它没参数）。 */
  const taskItems = (): TaskMeta[] => {
    const q = taskQuery.value.trim().toLowerCase()
    return tasks.value
      .filter((t) => t.params.length > 0 || t.name === pickedTask.value)
      .filter(
        (t) =>
          q === '' ||
          `${t.title} ${t.name} ${t.description} ${t.groupLabel}`.toLowerCase().includes(q),
      )
  }

  /** 账号那一栏（同样吃搜索词：手机上号多的时候，名字比列表位置好认）。 */
  const accountItems = (): AccountSummary[] => {
    const q = taskQuery.value.trim().toLowerCase()
    if (q === '') return accounts.value
    return accounts.value.filter((a) =>
      `${a.nickname ?? ''} ${a.label} ${a.id}`.toLowerCase().includes(q),
    )
  }

  /**
   * 要不要显示搜索框。
   *
   * ⚠ 判据用的是**没被搜索词过滤过的总数**：拿过滤后的长度判，会让"筛到只剩一条"
   * 的那一刻搜索框自己消失 —— 用户连清空关键词的地方都没了。
   *
   * 阈值 6 是取舍：三五个配置大项扫一眼就到底，多一个输入框只是噪音；而上百个账号
   * 时没有它就等于没法用。（不按 tab 分别设 —— 两个 tab 是同一个控件。）
   */
  const searchable = (): boolean =>
    (activeTab.value === 'task' ? configurable().length : accounts.value.length) > 6

  const ensureSelection = () => {
    if (state.value !== 'ready') return
    // 任务：没选过（或者选的那个被摘了）⇒ 落到第一个可配置的
    if (!pickedTask.value || !taskItems().some((t) => t.name === pickedTask.value)) {
      pickedTask.value = configurable()[0]?.name ?? ''
    }
    if (pickedTask.value && detailState.value === 'idle') void loadTaskConfig(pickedTask.value)
    // 账号：同上
    if (!accounts.value.some((a) => a.id === pickedAccount.value)) {
      pickedAccount.value = accounts.value[0]?.id ?? ''
    }
    if (activeTab.value === 'push' && pickedAccount.value && pushState.value === 'idle') {
      void loadAccountSettings(pickedAccount.value)
    }
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

  /** 把当前这一份脏草稿收进模块级表（干净的就把旧草稿删掉）。 */
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

  const draftOfAccount = (id: string): PushEditorSnapshot | undefined => {
    void draftsRev.value
    return accountDrafts.get(id)
  }

  const selectTask = (name: string) => {
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
      pushToast('error', e instanceof Error ? e.message : '保存失败')
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

  // ── 推送：读 / 存 / 草稿 ──────────────────────────────────────────────

  const pushSnapshotText = (): string => JSON.stringify(push.snapshot())

  const loadAccountSettings = async (id: string) => {
    pushState.value = 'loading'
    pushError.value = ''
    const seq = ++acctSeq
    try {
      const s = await getAccountSettings(id)
      if (seq !== acctSeq || pickedAccount.value !== id) return
      acctSettings.value = s
      push.load(s.notify)
      // 基线取**服务端那一份**（在草稿之前）⇒ 有草稿时"未保存修改"才是对的
      pushBaseline.value = pushSnapshotText()
      const draft = accountDrafts.get(id)
      if (draft) push.restore(draft)
      pushState.value = 'ready'
    } catch (e) {
      if (seq !== acctSeq) return
      // ⚠ 后端对"别人的账号 / 不存在"回同一个 404（不泄露存在性）⇒ 原话照抄，
      // 不要在这里改写成"网络错误"。
      pushError.value = e instanceof Error ? e.message : '读不到这个账号的设置'
      pushState.value = 'error'
    }
  }

  const pushDirty = (): boolean => pushState.value === 'ready' && pushSnapshotText() !== pushBaseline.value

  const stashAccountDraft = () => {
    const id = pickedAccount.value
    if (!id || pushState.value !== 'ready') return
    if (pushDirty()) accountDrafts.set(id, push.snapshot())
    else accountDrafts.delete(id)
  }

  const selectAccount = (id: string) => {
    if (id !== pickedAccount.value) stashAccountDraft()
    draftsRev.value++
    pickedAccount.value = id
    drillOpen.value = true
    void loadAccountSettings(id)
  }

  const savePush = async () => {
    const s = acctSettings.value
    // ⚠ 账号 id 不在 `AccountSettings` 里（那是"设置本身"，不含主键）—— 用它只会得到
    // `undefined`。id 的来源是"当前选中的那个账号"。
    const id = pickedAccount.value
    if (!s || !id) return
    pushSaving.value = true
    try {
      // 整体替换：只换 `notify` 那一格，别的字段原样带回去（见 `acctSettings` 的 ⚠）
      const next: AccountSettings = { ...s, notify: push.build() }
      const after = await putAccountSettings(id, next)
      acctSettings.value = after
      // 用**后端回读的那一份**回填：整体替换的语义下，"我以为存进去的"不算数
      push.load(after.notify)
      pushBaseline.value = pushSnapshotText()
      accountDrafts.delete(id)
      draftsRev.value++
      pushToast('success', '已保存 —— 这个账号的运行结果会按这份配置推送')
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '保存失败')
    } finally {
      pushSaving.value = false
    }
  }

  /**
   * 放弃修改 = 丢掉草稿、重新拉服务端那一份。
   *
   * ⚠ 这里**重新 GET**而不是"把基线快照塞回去"：基线只存了文本（用来比"脏没脏"），
   * 而"服务端那份长什么样"只有服务端知道 —— 本地拼一份出来迟早与它分叉。
   * 代价是一次往返，只在用户主动点这一下时发生。
   */
  const discardPush = () => {
    const id = pickedAccount.value
    if (!id) return
    accountDrafts.delete(id)
    draftsRev.value++
    pushState.value = 'idle'
    void loadAccountSettings(id)
    pushToast('info', '已回到上次保存的那一份')
  }

  // ── 生命周期 ──────────────────────────────────────────────────────────

  onMounted(() => {
    /**
     * ⚠ 这几件事**全部推到微任务里**，不在 `onMounted` 当场做。
     *
     * `onMounted` 是 `insert` 期间被回调的 —— 那一刻还在外壳（`app.tsx`）那条槽 effect
     * 的求值当中。`activeTab` / `pickedTask` 这些是**模块级**信号（页面会重挂，状态要留住），
     * 当场写它们 = 在 effect 求值里写信号 ⇒ 订阅它们的槽当场重跑。lite 的重入上限（100 层）
     * 会把真正的互写抓出来，但"本来没必要在渲染期写"的事就别去踩那条线。
     * 推到微任务里就落在外壳那轮渲染之后 ⇒ 一次干净的更新 ✓。
     *
     * 顺序：**先定预选**（`ensureSelection` 读它），再登记刷新，最后取数。
     */
    queueMicrotask(() => {
      if (!alive) return
      // URL 预选：从任务卡点「配置」进来时，那一项就是选中的，手机上直接铺它的详情
      if (props.task) {
        pickedTask.value = props.task
        activeTab.value = 'task'
        drillOpen.value = true
      }
      prevRefresh = setPageRefresh({ run: () => refresh() })
      void load()
    })
    // ⚠ 断点监听建在**挂载时**（不是组件体里）：组件体只跑一次，但把它写成模块级
    // 又会漏解绑；建在这里、卸载时解绑，边界最清楚（`history-page.tsx` 同款）。
    mq = matchMedia(WIDE_QUERY)
    mq.addEventListener('change', onWideChange)
    onWideChange()
  })

  onUnmounted(() => {
    alive = false
    // 走之前把这一份收进草稿 —— 回来时"我刚才敲的那些"还在。
    // ⚠ 这里**只写普通 Map，绝不碰任何信号**（含 `draftsRev`）：卸载期间写信号会让
    // 正在被摘掉的子树重跑、旧页面于是"不被替换而被追加"（见文件头那段 ⚠⚠ 的实测）。
    // 徽章不需要在这里唤醒 —— 下一次挂载整页重渲染，读到的就是最新的草稿表。
    stashTaskDraft()
    stashAccountDraft()
    setPageRefresh(prevRefresh)
    mq?.removeEventListener('change', onWideChange)
    mq = undefined
  })

  // ── 渲染 ──────────────────────────────────────────────────────────────

  const openTab = (id: TabId) => {
    activeTab.value = id
    // ⚠ 搜索词**跟着 tab 清掉**：两个 tab 搜的是不同的东西（任务名 / 账号名），
    // 带着上一栏的词切过去，多半只会看到"没有匹配的账号" —— 那不是用户想问的。
    taskQuery.value = ''
    // 换 tab 等于换了一批"配置大项" ⇒ 手机上先看列表（宽屏这个标志不被读）
    drillOpen.value = false
    if (id === 'push' && pickedAccount.value && pushState.value === 'idle') {
      void loadAccountSettings(pickedAccount.value)
    }
  }

  const tabs = () => (
    // ⚠ 窄屏：`flex-1` 而不是 `w-full` —— 与左边那颗 ✕ 同一行，五个字的位置就是
    // 44px 的垂直空间（`w-full` 会把它挤到下一行）。见规范 §2.2.3 那条"join w-full"。
    <div role="tablist" class="join min-w-0 flex-1 md:w-auto md:flex-none" aria-label="配置分类">
      <button
        type="button"
        role="tab"
        aria-selected={activeTab.value === 'task'}
        class={`btn join-item btn-sm min-h-11 flex-1 whitespace-nowrap md:min-h-0 md:flex-none ${
          activeTab.value === 'task' ? 'btn-primary' : 'btn-soft'
        }`}
        onClick={() => openTab('task')}
      >
        任务配置
        <span class="ml-1 text-xs opacity-70">{configurable().length}</span>
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={activeTab.value === 'push'}
        class={`btn join-item btn-sm min-h-11 flex-1 whitespace-nowrap md:min-h-0 md:flex-none ${
          activeTab.value === 'push' ? 'btn-primary' : 'btn-soft'
        }`}
        onClick={() => openTab('push')}
      >
        推送配置
        <span class="ml-1 text-xs opacity-70">{accounts.value.length}</span>
      </button>
    </div>
  )

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
            class="grow"
            autocomplete="off"
            placeholder={activeTab.value === 'task' ? '搜任务名 / 标题…' : '搜账号名…'}
            aria-label="搜索配置项"
            value={taskQuery.value}
            onInput={(e) => (taskQuery.value = (e.target as HTMLInputElement).value)}
          />
        </label>
      ) : null}

      <div class="flex items-center justify-between px-1">
        <span class="text-xs font-medium opacity-70">
          {activeTab.value === 'task' ? '配置模块' : '推送账号'}
        </span>
        <span class="badge badge-soft badge-sm">
          {activeTab.value === 'task' ? configurable().length : accounts.value.length} 项
        </span>
      </div>

      {/*
        ⚠⚠ 两个 tab **各写各的 `.map()`**，而且它必须待在**元素**的子节点位置上：
        ① 写进三元分支的那个 map（`cond ? xs.map(...) : []`）走通用表达式路径 ⇒ 编译器
        当场红：「这里的 `key` 什么都不做」（本页第一版就是这样，`npm run build` 直接失败）；
        ② 片段（`<>…</>`）同样不行 —— 编译器把片段当"动态插入"处理（`compiler.ts:477-481`），
        它里面的 map 一样认领不到 key。所以下面用 `<div>` 而不是 `<>` 包。
        判据与其它编译期边界见 `docs/web/lite-guide.md` §8.2 / §10。
      */}
      {activeTab.value === 'task' ? (
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
      ) : null}

      {activeTab.value === 'push' && accounts.value.length === 0 ? (
        <EmptyState
          title="还没有已登录的账号。"
          hint="推送是按账号配的：先到「账号」页登录一个云盘账号，再回来配推送。"
          action={
            <button type="button" class="btn btn-soft" onClick={() => navigate('/accounts')}>
              去账号页
            </button>
          }
        />
      ) : null}

      {activeTab.value === 'push' && accounts.value.length > 0 ? (
        <div class="flex flex-col gap-1">
          {accountItems().map((a) => (
            <button
              key={a.id}
              type="button"
              aria-current={pickedAccount.value === a.id ? 'true' : null}
              class={`flex min-h-11 w-full items-center gap-2 rounded-field px-3 py-2 text-left ${
                pickedAccount.value === a.id ? 'bg-primary text-primary-content' : 'hover:bg-base-200'
              }`}
              onClick={() => selectAccount(a.id)}
            >
              <span class="min-w-0 flex-1">
                <span class="block truncate text-sm font-medium">
                  {a.nickname || a.label || a.id}
                </span>
                {/* ⚠ 副行只在**有昵称**时才画：没有昵称时主行印的就是 `label`（掩码手机号），
                    再印一遍等于同一句话占两行 */}
                {a.nickname ? (
                  <span class="block truncate font-mono text-[11px] opacity-60">{a.label}</span>
                ) : null}
              </span>
              {draftOfAccount(a.id) ? (
                <span class="badge badge-warning badge-sm shrink-0">未保存</span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}

      {/* 空态分两种：**这一栏本来就没有东西** 与 **搜索没命中** —— 前者要给出下一步，
          后者只要提醒换个词（混成一句"暂无数据"就两种都没说清） */}
      {activeTab.value === 'task' && configurable().length === 0 ? (
        <EmptyState
          title="没有任何任务需要配置。"
          hint="任务参数由后端注册表给：有参数的任务才会出现在这一栏（签到族那些没有参数）。"
        />
      ) : null}
      {activeTab.value === 'task' && configurable().length > 0 && taskItems().length === 0 ? (
        <EmptyState title="没有匹配的配置项。" hint="换个关键词试试（搜的是标题、任务名与说明）。" />
      ) : null}
      {activeTab.value === 'push' && accounts.value.length > 0 && accountItems().length === 0 ? (
        <EmptyState title="没有匹配的账号。" hint="换个关键词试试（搜的是昵称、掩码手机号与账号 id）。" />
      ) : null}
    </div>
  )

  /** 详情顶上那条操作栏：未保存徽章 + 放弃修改 + 保存（宽屏与手机同一份）。 */
  const actions = (opts: { dirty: boolean; savingRef: () => boolean; onDiscard: () => void; onSave: () => void }) => (
    <div class="flex flex-wrap items-center gap-2">
      {opts.dirty ? <span class="badge badge-warning badge-sm">有未保存修改</span> : null}
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        disabled={!opts.dirty}
        onClick={() => opts.onDiscard()}
      >
        放弃修改
      </button>
      <button
        type="button"
        class="btn btn-sm btn-primary min-h-11 md:min-h-0"
        disabled={opts.savingRef()}
        onClick={() => opts.onSave()}
      >
        {opts.savingRef() ? '保存中…' : '保存'}
      </button>
    </div>
  )

  /** 任务那一栏的详情：那个任务的默认参数。 */
  const taskDetail = () => (
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
              {actions({
                dirty: taskDirty(),
                savingRef: () => saving.value,
                onDiscard: () => discardTask(),
                onSave: () => void saveTask(),
              })}
            </div>

            <p class="text-sm opacity-70">
              这里存的是这个任务的默认参数：任务页点「运行」时会带上它。
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
                        （2026-10-03 review §2.2；正解见 `accounts-page.tsx:1192-1199`）。 */}
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
          </>
        ) : null}

        {detailState.value === 'idle' ? (
          <p class="py-10 text-center text-sm opacity-70">左边挑一个任务，这里改它的参数。</p>
        ) : null}
      </div>
    </div>
  )

  /** 推送那一栏的详情：这个账号的运行结果推送。 */
  const pushDetail = () => (
    <div class="card min-w-0 bg-base-100 shadow-sm">
      <div class="card-body gap-4">
        {pushState.value === 'loading' ? (
          <div class="flex justify-center py-16">
            <span class="loading loading-lg text-primary" />
          </div>
        ) : null}

        {pushState.value === 'error' ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{pushError.value}</span>
            <button
              type="button"
              class="btn btn-sm btn-soft"
              onClick={() => void loadAccountSettings(pickedAccount.value)}
            >
              重试
            </button>
          </div>
        ) : null}

        {pushState.value === 'ready' ? (
          <>
            <div class="flex flex-wrap items-start justify-between gap-2">
              <h2 class="card-title min-w-0 text-base">
                {accounts.value.find((a) => a.id === pickedAccount.value)?.nickname ||
                  accounts.value.find((a) => a.id === pickedAccount.value)?.label ||
                  pickedAccount.value}
              </h2>
              {actions({
                dirty: pushDirty(),
                savingRef: () => pushSaving.value,
                onDiscard: () => discardPush(),
                onSave: () => void savePush(),
              })}
            </div>

            <p class="text-sm opacity-70">
              推送按账号配（本仓没有进程级兜底）。这里只改推送这一块：
              同一个账号还有备份等待、凭据刷新阈值等设置，保存时原样带回去。
            </p>

            {/* 十条渠道 + 打码字段的整块表单在 `ui/push-editor.tsx`（账号设置弹窗共用） */}
            {push.fields()}
          </>
        ) : null}

        {pushState.value === 'idle' ? (
          <p class="py-10 text-center text-sm opacity-70">左边挑一个账号，这里配它的推送渠道。</p>
        ) : null}
      </div>
    </div>
  )

  const detail = () => (activeTab.value === 'task' ? taskDetail() : pushDetail())

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
          ← {activeTab.value === 'task' ? '全部任务配置' : '全部账号推送'}
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
        {tabs()}
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

      {state.value === 'ready' ? (
        wide.value ? (
          wideBody()
        ) : (
          narrowBody()
        )
      ) : null}
    </div>
  )
}
