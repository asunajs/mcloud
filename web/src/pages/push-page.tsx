/**
 * 「推送」页 —— **左边规则、右边配置**（2026-10-08 用户口径「可以参考这种布局，
 * 左边是规则，右边是配置」✓）。
 *
 * # 这一版改掉了什么
 *
 * ① **表格 + 弹窗编辑器整个换掉** ✗ —— 改成照**账号页**那套 master-detail：
 *    左栏规则列表（`#push-rail` ✓）、右栏选中规则的完整配置（`#push-detail` ✓）、
 *    右上「放弃修改 / 保存」✓、窄屏「列表 ↔ 详情」下钻（`matchMedia`，
 *    不是 `hidden md:block` 双份进 DOM ✓）。**弹窗不要了** ✗（删除确认那个
 *    `ConfirmDialog` 留着 —— 用户口径「删前确认」要的就是它 ✓）。
 * ② **静默时段整块删掉** ✗（2026-10-08 用户口径「**不要静默时间**」✓）——
 *    原来那两格 `type=time` 与那句说明一起没了 ✓，`quiet` 也不在请求体里了 ✓。
 *
 * # 规则里装什么（与后端 `PushRule` 一一对应）
 *
 * 名字 + `config`（**这条规则自己的平台与凭据**，表单本体是 `ui/push-editor.tsx`
 * 那一份工厂 ✓）+ 汇总方式（一次运行一条 / 每步一条）+ 启用 ✓。
 * 「**什么时候发**」不在这里 —— 那是**行为**那一侧选的：定时 / 编排在它们自己的
 * 表单里勾这条规则，登录 / 性能那四类在「设置 → 事件推送」里勾 ✓。
 *
 * # 未保存的改动与草稿
 *
 * 与账号配置页同一条口径：右栏一份表单、脏判定靠"快照文本与基线不等"✓；
 * 切走时把脏草稿收进**模块级普通 Map**（`onUnmounted` 也要写它 ⇒
 * **绝不在这里写信号** ✗，否则旧页面"不被替换而被追加"，那次实测见
 * `config-page.tsx` 文件头那段 ⚠⚠）。左栏那条规则上会出现「未保存」徽章 ✓。
 */
import { onMounted, onUnmounted, ref } from '@asunajs/lite'

import { setPageRefresh } from '../ui/page-refresh'
import {
  deletePushRule,
  getPushPanel,
  listPipelines,
  listPushEvents,
  listPushRules,
  listSchedules,
  savePushRule,
  PANEL_BEHAVIOR_KEYS,
  emptyPanelBindings,
  type PanelBindings,
  type Pipeline,
  type PushDigestMode,
  type PushEventOption,
  type PushRule,
  type PushRuleInput,
  type Schedule,
} from '../api'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { EmptyState } from '../ui/empty-state'
import { IconAlert } from '../ui/icons'
import { pushToast } from '../ui/toast'
import { createPushEditor, platformsOf, type PushEditorSnapshot } from '../ui/push-editor'
import { Select } from '../ui/select'

type LoadState = 'loading' | 'error' | 'ready'

// ── 跨"进出一页"保留的选中与草稿（模块级 = 会话内；不落盘）────────────────
//
// ⚠ 与账号配置页同一条口径：这些信号**只在 JSX 槽里读**，绝不在组件体顶层读
// （组件体里读模块级信号 ⇒ 外壳那个动态槽订阅它 ⇒ 一改就整页重建）。
//
// ⚠ 选中哪条规则**不进 URL** —— 账号页也是这么做的（`pickedAccount` 就是个
// 模块级 ref ✓）：地址仍然只是 `/push` ✓。

/** 选中的规则 id（空串 = 还没选）。 */
const pickedRule = ref('')
/** 右栏是不是"正在新建"（新建的规则还没有 id，保存时才生成 ✓）。 */
const creating = ref(false)
/** 手机上"详情是否已铺开"（false = 看列表）。宽屏不看它。 */
const drillOpen = ref(false)

/** 一条规则的草稿：右栏表单那一份快照（纯数据，可 JSON 化）。 */
interface RuleDraft {
  name: string
  enabled: boolean
  digest: PushDigestMode
  editor: PushEditorSnapshot
}
/** ⚠ **普通 Map**，不是信号 —— `onUnmounted` 也要写它（见文件头最后一段）。 */
const drafts = new Map<string, RuleDraft>()
/** 草稿表的"版本号"。只在交互处理器里自增，用来唤醒读了草稿的那些槽。 */
const draftsRev = ref(0)

export const PushPage = () => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')

  // ── 规则（规则 = 「怎么发」：平台 + 凭据 + 标题 + 汇总方式）──────────────
  const rules = ref<PushRule[]>([])
  const eventOptions = ref<PushEventOption[]>([])
  /**
   * 「删前确认」要说清**影响面**要的三份名单。
   *
   * ⚠ 后端**没有**"按规则反查绑定"的接口 ✗ ⇒ 这里各拉一份，在**前端**算绑定关系 ✓。
   * 面板绑定（`getPushPanel`）也在里面：那四类行为同样能绑规则 ✓，
   * 漏掉它会让"这条规则只绑在面板上"显示成"没绑"✗ —— 那是撒谎。
   */
  const schedules = ref<Schedule[]>([])
  const pipelines = ref<Pipeline[]>([])
  const panel = ref<PanelBindings>(emptyPanelBindings())

  /**
   * 平台与凭据表单（**这条规则自己的** ✓）。
   *
   * ⚠ 必须在组件体里调（内部 ref 靠 lite 在组件求值时登记 ✓，见 `push-editor.tsx`）。
   */
  const push = createPushEditor()

  // ── 右栏表单的几格（平台与凭据那几格在 `push` 里）───────────────────────
  const detailState = ref<'idle' | 'ready'>('idle')
  const fName = ref('')
  const fEnabled = ref(true)
  /**
   * 汇总方式。**这一版可以改**（旧表格页只在列里显示、不编辑 ✗）——
   * 用户口径「右边是配置」⇒ 它就该在右栏里 ✓。
   */
  const fDigest = ref<PushDigestMode>('perRun')
  /** 载入（或保存）之后的那份快照文本：未保存修改就是它与当前快照不等。 */
  const baseline = ref('')
  const saving = ref(false)
  /**
   * 保存 / 校验失败的原话。
   *
   * ⚠⚠ 呈现方式（2026-10-08 用户口径「所有的提示都是优先弹窗式」）：
   * ① `pushToast('error', …)` —— **弹窗优先**：用户点的是右上角那个「保存」，
   *    视线就在右上角，提示必须出现在那里；
   * ② 这一句**贴在卡片标题那一行底下**（= 保存键下方）—— "混合"，说清是哪件事错了。
   * ❌ 曾经它只画在**表单最底部**：保存键在顶上、红字在下面隔一屏，等于没提示
   *    （用户截图报的就是这一条）。
   */
  const submitError = ref('')
  /** 「名称」那一格自己的错 —— 贴在**那一格**下面（就近）。 */
  const nameError = ref('')

  const pendingDelete = ref<PushRule | null>(null)
  const deleting = ref(false)
  const deleteDialogId = 'push-rule-delete'

  const WIDE_QUERY = '(min-width: 768px)'
  const wide = ref(matchMedia(WIDE_QUERY).matches)
  let mq: MediaQueryList | undefined
  const onWideChange = () => {
    if (mq) wide.value = mq.matches
  }

  let alive = true
  let prevRefresh: ReturnType<typeof setPageRefresh> = null

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      // ⚠ 这里**每一个**并发端点都必须在 fixture（`web/regress/lib.mjs`）里有一份：
      // 少一个的表现不是"那一块空着"，而是**整页错误态** ✗。
      const [rs, evs, schs, pips, pn] = await Promise.all([
        listPushRules(),
        listPushEvents(),
        listSchedules(),
        listPipelines(),
        getPushPanel(),
      ])
      rules.value = rs
      eventOptions.value = evs
      schedules.value = schs
      pipelines.value = pips
      panel.value = pn
      state.value = 'ready'
      if (creating.value) {
        // 上次会话停在"新建"上 ⇒ 重新铺一张空表（表单 ref 随组件重建，得重来 ✓）
        beginCreate()
      } else {
        if (!rules.value.some((r) => r.id === pickedRule.value)) {
          pickedRule.value = rules.value[0]?.id ?? ''
        }
        if (pickedRule.value) applyRule(pickedRule.value)
        else detailState.value = 'idle'
      }
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /**
   * 只刷规则与三份绑定名单。
   *
   * ⚠ **不走 `load()`**：那会把 `state` 打回 `loading` ⇒ 整页闪一下再长回来 ✗。
   * 规则上的动作只影响这一块。
   */
  const refreshRules = async () => {
    try {
      const [rs, schs, pips, pn] = await Promise.all([
        listPushRules(),
        listSchedules(),
        listPipelines(),
        getPushPanel(),
      ])
      rules.value = rs
      schedules.value = schs
      pipelines.value = pips
      panel.value = pn
    } catch {
      // 静默失败：页面上已经有一份数据，刷不到不该把它换成错误页
    }
  }

  onMounted(() => {
    // ⚠ 推到微任务：子视图的 `onMounted` 先于父页面跑（账号配置页那条口径 ✓）
    queueMicrotask(() => {
      if (!alive) return
      prevRefresh = setPageRefresh({ run: () => void load() })
    })
    mq = matchMedia(WIDE_QUERY)
    mq.addEventListener('change', onWideChange)
    onWideChange()
    void load()
  })

  onUnmounted(() => {
    alive = false
    // ⚠ 只写普通 Map，绝不碰信号（含 `draftsRev`）——理由见文件头最后一段
    stashDraft()
    setPageRefresh(prevRefresh)
    mq?.removeEventListener('change', onWideChange)
    mq = undefined
  })

  // ── 表单与草稿 ────────────────────────────────────────────────────────

  /** 右栏当前这一份的纯数据快照（页面自己那几格 + 平台与凭据那份 ✓）。 */
  const snapshotText = (): string =>
    JSON.stringify({
      name: fName.value,
      enabled: fEnabled.value,
      digest: fDigest.value,
      editor: push.snapshot(),
    })

  const dirty = (): boolean => detailState.value === 'ready' && snapshotText() !== baseline.value

  /** 把当前这一份脏草稿收进模块级表（干净的就把旧草稿删掉）。⚠ 只写普通 Map。 */
  const stashDraft = () => {
    const id = pickedRule.value
    if (!id || creating.value || detailState.value !== 'ready') return
    if (dirty()) {
      drafts.set(id, {
        name: fName.value,
        enabled: fEnabled.value,
        digest: fDigest.value,
        editor: push.snapshot(),
      })
    } else {
      drafts.delete(id)
    }
  }

  /** 读草稿的存在性（顺带订阅 `draftsRev`，见文件头）。 */
  const hasDraft = (id: string): boolean => {
    void draftsRev.value
    return drafts.has(id)
  }

  /** 把一条规则铺进右栏（有草稿就把草稿盖上 ✓）。 */
  const applyRule = (id: string) => {
    const r = rules.value.find((x) => x.id === id)
    if (!r) {
      detailState.value = 'idle'
      return
    }
    creating.value = false
    fName.value = r.name
    fEnabled.value = r.enabled
    fDigest.value = r.digest ?? 'perRun'
    push.load(r.config)
    // 基线取**服务端那一份**（在草稿之前）⇒ 有草稿时"未保存修改"才是对的
    baseline.value = snapshotText()
    const d = drafts.get(id)
    if (d) {
      fName.value = d.name
      fEnabled.value = d.enabled
      fDigest.value = d.digest
      push.restore(d.editor)
    }
    submitError.value = ''
    nameError.value = ''
    detailState.value = 'ready'
  }

  const selectRule = (id: string) => {
    if (!creating.value && id !== pickedRule.value) stashDraft()
    draftsRev.value++
    pickedRule.value = id
    drillOpen.value = true
    applyRule(id)
  }

  /** 铺一张空的"新建"表（**不**碰草稿表 —— 那是 `startCreate` 的事 ✓）。 */
  const beginCreate = () => {
    creating.value = true
    pickedRule.value = ''
    fName.value = ''
    fEnabled.value = true
    fDigest.value = 'perRun'
    push.load(null)
    baseline.value = snapshotText()
    submitError.value = ''
    nameError.value = ''
    detailState.value = 'ready'
  }

  const startCreate = () => {
    if (!creating.value) stashDraft()
    draftsRev.value++
    drillOpen.value = true
    beginCreate()
  }

  /** 表单 → 请求体。**每一格都要给**（缺的会被后端的 `#[serde(default)]` 吃成默认值 ✗）。 */
  const ruleInput = (): PushRuleInput => ({
    name: fName.value.trim(),
    config: push.build(),
    digest: fDigest.value,
    enabled: fEnabled.value,
  })

  const save = async () => {
    submitError.value = ''
    nameError.value = ''
    if (!fName.value.trim()) {
      nameError.value = '请填写规则名称'
      // 弹窗优先 + 就近那一句（见 `submitError` 的说明）
      pushToast('error', '请填写规则名称 —— 「名称」那一格还空着')
      return
    }
    let input: PushRuleInput
    try {
      // ⚠ `push.build()` 会抛（一个平台都没配）—— 本地先说，省一次注定 400 的往返 ✓
      input = ruleInput()
    } catch (e) {
      const msg = e instanceof Error ? e.message : '配置不完整'
      submitError.value = msg
      pushToast('error', msg)
      return
    }
    // id 由前端生成（编辑时沿用原 id）：服务端不发明 id ——
    // 那样"改一条规则"与"建一条规则"就分不出来了
    const id = creating.value ? newRuleId() : pickedRule.value
    if (!id) return
    saving.value = true
    try {
      const saved = await savePushRule(id, input)
      drafts.delete(id)
      draftsRev.value++
      await refreshRules()
      creating.value = false
      pickedRule.value = id
      // 用**后端回读的那一份**回填：整体替换的语义下，"我以为存进去的"不算数
      const r = rules.value.find((x) => x.id === id) ?? saved
      fName.value = r.name
      fEnabled.value = r.enabled
      fDigest.value = r.digest ?? 'perRun'
      push.load(r.config)
      baseline.value = snapshotText()
      submitError.value = ''
      nameError.value = ''
      detailState.value = 'ready'
      pushToast('success', '已保存')
    } catch (e) {
      // 后端错误原样透出（它的话比前端猜的准）
      const msg = e instanceof Error ? e.message : '保存失败'
      submitError.value = msg
      // ⚠ 错误类 toast 由 `toast.ts` 保证**不自动消失**（要手动关）——
      // 保存失败带的信息（哪一格、为什么）读起来不止 3 秒，见那边的注释 ✓
      pushToast('error', msg)
    } finally {
      saving.value = false
    }
  }

  /**
   * 放弃修改 = 丢掉草稿、重新铺服务端那一份。
   *
   * ⚠ 重新铺而不是"把基线塞回去"：基线只存了文本，而"服务端那份长什么样"只有
   * 服务端（这一次是列表接口 ✓）知道 —— 本地拼一份出来迟早与它分叉。
   */
  const discard = () => {
    if (creating.value) {
      beginCreate()
      pushToast('info', '已清空新建的规则')
      return
    }
    const id = pickedRule.value
    if (!id) return
    drafts.delete(id)
    draftsRev.value++
    applyRule(id)
    pushToast('info', '已回到上次保存的那一份')
  }

  // ── 删除 ──────────────────────────────────────────────────────────────

  /**
   * 这条规则**绑在谁身上**（定时 / 编排 / 面板行为各找一遍）。
   *
   * 空数组 = 没绑 ⇒ **不生效** ✓。后端没有级联删除：规则删了，行为上那个 id 还在
   * （投递时按 id 找不到就跳过那一条 ✓）—— 所以这里只列"现在真的还指着它"的 ✓。
   */
  const bindingsOf = (ruleId: string): string[] => {
    const out: string[] = []
    for (const s of schedules.value) if ((s.pushRules ?? []).includes(ruleId)) out.push(`定时「${s.name}」`)
    for (const p of pipelines.value) if ((p.pushRules ?? []).includes(ruleId)) out.push(`编排「${p.name}」`)
    for (const k of PANEL_BEHAVIOR_KEYS) {
      // ⚠ `?? []`：后端对空数组是 `skip_serializing_if` ⇒ 这一格可能**压根不在** ✗
      if ((panel.value[k].rules ?? []).includes(ruleId)) out.push(`事件「${eventLabel(k)}」`)
    }
    return out
  }

  /**
   * 事件键 → 中文名。
   *
   * ⚠ 认不出的键**原样显示**（不吞、不换成"未知"）：后端加了事件而界面还没跟上时，
   * 用户至少看得见那条行为里挂的是什么 ✗ —— 藏起来就等于"我明明勾了它，怎么没了"。
   */
  const eventLabel = (key: string) => eventOptions.value.find((o) => o.key === key)?.label ?? key

  const askDeleteRule = (r: PushRule) => {
    pendingDelete.value = r
    openDialog(deleteDialogId)
  }

  /**
   * 删除确认里的**影响面**。
   *
   * 后端没有级联删除：规则没了，行为上那个 id 还在（投递时按 id 找不到就跳过那一条 ✓）。
   * 一个行为可以绑多条规则（用户口径 ✓）⇒ 删掉一条**只是少发一条**，不是"不推了" ✗。
   */
  const deleteImpact = () => {
    const r = pendingDelete.value
    const bound = r ? bindingsOf(r.id) : []
    if (bound.length === 0) {
      return `将删除「${r?.name ?? ''}」。它当前没有绑到任何定时 / 编排 / 事件推送上，删除后没有别的东西受影响。`
    }
    return `将删除「${r?.name ?? ''}」。绑着它的 ${bound.join('、')} 会少这一条规则（绑了别的规则的话照旧发，不是不推）。`
  }

  const doDeleteRule = async () => {
    const target = pendingDelete.value
    if (!target) return
    deleting.value = true
    try {
      await deletePushRule(target.id)
      closeDialog(deleteDialogId)
      pendingDelete.value = null
      drafts.delete(target.id)
      draftsRev.value++
      if (pickedRule.value === target.id) {
        pickedRule.value = ''
        detailState.value = 'idle'
      }
      pushToast('success', '已删除推送规则')
      await refreshRules()
      // 删的正是右栏这条 ⇒ 补选一条（没有别的了就停在空态 ✓）
      if (!pickedRule.value && !creating.value && rules.value.length > 0) {
        selectRule(rules.value[0].id)
      }
    } catch (e) {
      // 走 toast：删除是独立动作，"为什么删不掉"必须让用户读到
      closeDialog(deleteDialogId)
      pendingDelete.value = null
      pushToast('error', e instanceof Error ? e.message : '删除失败')
    } finally {
      deleting.value = false
    }
  }

  // ── 渲染 ──────────────────────────────────────────────────────────────

  /** 几格的**值**（列表行与详情都用从这里取 —— 两处各写一份迟早在某一处漏掉一种形态 ✗）。 */
  const rulePlatforms = (r: PushRule) => {
    const names = platformsOf(r.config)
    return names.length === 0 ? '（一个平台都没配）' : names.join('、')
  }

  const pickedName = (): string => rules.value.find((r) => r.id === pickedRule.value)?.name ?? ''

  const actions = () => (
    <div class="flex flex-wrap items-center gap-2">
      {dirty() ? <span class="badge badge-warning badge-sm">有未保存修改</span> : null}
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        disabled={!dirty()}
        onClick={() => discard()}
      >
        放弃修改
      </button>
      <button
        type="button"
        class="btn btn-sm btn-primary min-h-11 md:min-h-0"
        disabled={saving.value}
        onClick={() => void save()}
      >
        {saving.value ? '保存中…' : '保存'}
      </button>
    </div>
  )

  /** 左边那一栏（宽屏是侧栏，手机是整屏列表）。 */
  const rail = () => (
    <div id="push-rail" class="flex flex-col gap-3">
      <div class="flex items-center justify-between px-1">
        <span class="text-xs font-medium opacity-70">推送规则</span>
        <span class="badge badge-soft badge-sm">{rules.value.length} 条</span>
      </div>

      <button
        type="button"
        class="btn btn-sm btn-primary min-h-11 w-full md:min-h-0"
        onClick={startCreate}
      >
        新建规则
      </button>

      <p class="px-1 text-[11px] leading-4 opacity-60">
        规则 = 「怎么发」：这条规则自己的平台与凭据。至于「什么时候发」——
        由「定时」「编排」或「设置 → 事件推送」选它；一条都没被选中的规则不会生效。
      </p>

      {/* ⚠ `.map()` 必须在**元素**的子节点位置（三元分支 / 片段里的 map 认领不到 key ⇒
          编译期红，见 `config-page.tsx` 里那段 ⚠） */}
      <div class="flex flex-col gap-1">
        {rules.value.map((r) => (
          <div
            key={r.id}
            class={`flex items-center gap-1 rounded-field ${
              !creating.value && pickedRule.value === r.id
                ? 'bg-primary text-primary-content'
                : 'hover:bg-base-200'
            }`}
          >
            <button
              type="button"
              aria-current={!creating.value && pickedRule.value === r.id ? 'true' : null}
              class="flex min-h-11 min-w-0 flex-1 items-center gap-2 px-3 py-2 text-left"
              onClick={() => selectRule(r.id)}
            >
              <span class="min-w-0 flex-1">
                <span class="block truncate text-sm font-medium">{r.name}</span>
                <span class="block truncate text-[11px] opacity-60">{rulePlatforms(r)}</span>
              </span>
              {hasDraft(r.id) ? (
                <span class="badge badge-warning badge-sm shrink-0">未保存</span>
              ) : null}
              <span
                class={`badge badge-sm shrink-0 ${r.enabled ? 'badge-success' : 'badge-ghost'}`}
              >
                {r.enabled ? '启用' : '停用'}
              </span>
            </button>
            <button
              type="button"
              class="btn btn-ghost btn-xs mr-1 shrink-0"
              aria-label={`删除规则 ${r.name}`}
              onClick={() => askDeleteRule(r)}
            >
              删除
            </button>
          </div>
        ))}
      </div>

      {rules.value.length === 0 ? (
        <EmptyState
          title="还没有推送规则。"
          hint="建一条规则（填平台与凭据），再去「定时」「编排」或「设置 → 事件推送」里选上它 —— 没被选中的规则不会生效。"
          action={
            <button type="button" class="btn btn-primary" onClick={startCreate}>
              新建规则
            </button>
          }
        />
      ) : null}
    </div>
  )

  /**
   * 汇总方式那一格。
   *
   * ⚠ 单独一个函数、单独一个 `{digestField()}` 槽：`Select` 是**组件**，它的 props
   * 在组件体里当场读（lite 没有重渲染）⇒ 在这里读 `fDigest` 会把订阅压到这个槽上，
   * 改它只重建这一个下拉，不会把整块详情（含名称输入框的焦点）一起重建 ✗。
   */
  const digestField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">汇总方式</legend>
      <Select
        id="push-digest"
        class="w-full"
        ariaLabel="汇总方式"
        value={fDigest.value}
        options={[
          { value: 'perRun', label: '一次运行一条（默认）' },
          { value: 'perStep', label: '每一步一条（盯细节）' },
        ]}
        onChange={(v) => (fDigest.value = v as PushDigestMode)}
      />
      <p class="text-xs leading-5 opacity-60">
        一次运行推一条（默认）还是每一步各推一条；编排十几步时逐步推会刷屏。
      </p>
    </fieldset>
  )

  const detail = () => (
    <div class="card min-w-0 bg-base-100 shadow-sm">
      <div class="card-body gap-4">
        {detailState.value === 'idle' ? (
          <p class="py-10 text-center text-sm opacity-70">
            左边挑一条规则，这里改它的配置；或者点「新建规则」。
          </p>
        ) : null}

        {detailState.value === 'ready' ? (
          <>
            <div class="flex flex-wrap items-start justify-between gap-2">
              <h2 class="card-title min-w-0 text-base">
                {creating.value ? '新建推送规则' : pickedName() || '推送规则'}
              </h2>
              {actions()}
            </div>

            {/*
              ⚠⚠ 这一句**必须在标题行下面**（= 紧挨着右上角那个「保存」），不能放表单底部。
              用户截图报的正是"点上面的保存、红字出现在最底下" —— 中间隔一屏等于没提示。
              弹窗那一份由 `pushToast('error', …)` 给（见 `save()`）✓。
            */}
            {submitError.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <IconAlert class="size-4 shrink-0" />
                <span class="break-words">{submitError.value}</span>
              </div>
            ) : null}

            <p class="text-sm opacity-70">
              <span class="badge badge-soft badge-sm mr-1">规则级</span>
              这一页只改左边这一条规则。至于「什么时候发」，去「定时」「编排」或
              「设置 → 事件推送」里选它 —— 一条都没被选中的规则不会生效。
            </p>

            <fieldset class="fieldset">
              <legend class="fieldset-legend">名称</legend>
              <input
                class={`input w-full${nameError.value ? ' input-error' : ''}`}
                placeholder="例如 只在失败时叫我"
                value={fName.value}
                onInput={(e) => {
                  fName.value = (e.target as HTMLInputElement).value
                  nameError.value = ''
                }}
              />
              {/* 就近那一句：贴在这格下面（"混合"的另一半，指到具体哪一格 ✓） */}
              {nameError.value ? (
                <p class="text-xs text-error" role="alert">
                  {nameError.value}
                </p>
              ) : null}
            </fieldset>

            {/*
              ⭐ 平台与凭据**内嵌在这条规则里**（用户口径：「这个规则配置推送的 token，
              包括推送哪几个平台」✓）—— 表单本体是 `ui/push-editor.tsx` 那一份工厂 ✓。
            */}
            <div class="flex flex-col gap-2">{push.fields()}</div>

            {digestField()}

            <label class="label cursor-pointer justify-start gap-3">
              <input
                type="checkbox"
                class="checkbox"
                checked={fEnabled.value}
                onChange={(e) => {
                  fEnabled.value = (e.target as HTMLInputElement).checked
                }}
              />
              <span class="text-sm">启用这条规则（停用后，绑着它的行为会跳过这一条）</span>
            </label>

            <p class="text-xs opacity-60">
              「测试发送」这一版没有：后端那条接口还没接线（明确回 501）——
              摆一个点了只会报错的按钮，比没有这个按钮更糟。
            </p>
          </>
        ) : null}
      </div>
    </div>
  )

  /** 宽屏：左右两栏（左栏自己滚）。 */
  const wideBody = () => (
    <div class="grid min-w-0 gap-4 md:grid-cols-[15rem_minmax(0,1fr)]">
      <div class="md:sticky md:top-20 md:max-h-[calc(100dvh-7rem)] md:self-start md:overflow-y-auto md:pr-1">
        {rail()}
      </div>
      {/* ⚠ `#push-detail` 这个 id 是给门禁用的锚（左右两边都印着规则名 ⇒ 只能靠容器区分） */}
      <div id="push-detail" class="min-w-0">
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
          ← 全部规则
        </button>
        <div id="push-detail" class="min-w-0">
          {detail()}
        </div>
      </div>
    ) : (
      rail()
    )

  // ⚠ 与设置页/账号配置页同一条口径：整页宽度封顶（超大屏别把表单拉成一条长线）
  return (
    <div class="mx-auto flex w-full max-w-7xl flex-col gap-4">
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

      <ConfirmDialog
        id={deleteDialogId}
        title="删除推送规则"
        impact={<span>{deleteImpact()}</span>}
        confirmLabel="删除"
        busy={deleting.value}
        onConfirm={() => void doDeleteRule()}
      />
    </div>
  )
}

/**
 * 生成一个本地规则 id。
 *
 * 与定时页同一条口径：**服务端不发明 id**（那样"改一条"与"建一条"就分不出来了）。
 * 字符集只用到 `[a-z0-9-]`，与后端那个透明的 `PushRuleId(String)` 不冲突。
 */
const newRuleId = (): string =>
  `rule-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
