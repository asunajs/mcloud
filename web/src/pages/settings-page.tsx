/**
 * 设置页 —— 目前这一页只有**推送**这一块（`GET/PUT /api/notify/config`、`POST /api/notify/test`）。
 *
 * # 三张卡各自解决一个问题
 *
 *  1. **总开关**：推不推、什么级别才推、标题叫什么。这四格是"全局门槛"，
 *     配错的表现是"渠道都配好了但一条都不发"；
 *  2. **渠道**：十家按后端给的 `order`（= 发送序）列举，每家一颗状态徽章 + 一枚编辑按钮。
 *     点开才谈凭据；
 *  3. **试发**：真发一条到选中的那几家。这一张卡是唯一会**出网打到第三方**的入口，
 *     所以它只列"已配的那几家"（后端对未配置的渠道直接 400，界面上就不该给出这个选项）。
 *
 * # 为什么这一页不摆"凭据"这件事
 *
 * 后端给出的是**脱敏视图**（`internal/server/notify_view.go`）：`secret` 那一档只出
 * "填了没 + 几个字符"，`keys` 只出键名。于是"留空 = 不改"不是这里的偷懒口径，而是
 * 唯一可能的语义 —— 界面上压根没有原值可回显。判据集中在 `ui/notify-form.ts`，
 * 那一族写侧语义（逐字段合并、整家置 `null` 才是清空、未知键 400）都在那儿兑。
 *
 * # Vapor：输入框一律非受控
 *
 * 与 `notify-channel-form.tsx` 顶部那条同一件事：草稿存在**普通对象**里（不放 ref），
 * 输入框只在创建那一下读初始值。需要响应式的只有按钮的禁用与结果区，那些走各自的
 * 属性绑定与子节点位置。**不能**把弹窗正文的 `<dialog>` 本体放进会重建的 effect ——
 * 正开着的窗换元素就等于当场关掉（本轮实机核验要看这一条）。
 *
 * # ⚠ 这一页的写法约定：分支条件把内容依赖的 ref 一起写进去
 *
 * 两条纪律照 `schedules-page.tsx` 的形状来：① 渲染卡片的分支条件把卡片内容依赖的 ref
 * 一并写进去（`state.value === 'ready' && view.value`，而不是只判 `state`）；
 * ② 其余"会自己变"的量（`testReport`）一律在**子节点位置**读，见结果区那一条注释。
 * 它们是**约定**（跟相邻页面同形、代价为零），不是本轮实测证明的因果 —— 见下面那条被推翻的归因。
 *
 * ## 现象"加载完一张卡都没有"的真根因：后端把 nil 清单发成了 `null`
 *
 * 前一版本本节写成"函数体里读 `view.value` = 只读一次，所以 `topCard()` 永远拿到空"。
 * **那个因果不成立**：`target/verify-notify-trace.cjs` 读到 `GET /api/notify/config`
 * 的响应体里是 `"configured":null`（Go 的 nil slice marshal 成 `null`），
 * `applyView` 顶上先把 `view.value` 赋上了值、随后那句 `next.configured.includes(name)`
 * 抛 TypeError ⇒ `load()` 落进 catch、`state` 停在 `'error'`，于是一张卡都不建。
 * 这正好解释当年那组自相矛盾的读数：子节点位置的 `view.value ? 'VIEW-PRESENT'` 如实读到
 * PRESENT（值确实有），卡片却是 0 张（`state` 不是 ready）。根因已在
 * `internal/server/notify_view.go` 修（nil 清单出网前裁成 `[]`），并配了线格式断言。
 */

import { onMounted, ref } from 'vue'
import {
  ApiError,
  deleteNotifyChannel,
  getNotifyConfig,
  putNotifyConfig,
  runNotifyTest,
  type NotifyConfigPatch,
  type NotifyConfigView,
  type NotifyTestReport,
} from '../api'
import { closeDialog, Dialog, openDialog } from '../ui/dialog'
import {
  channelLabel,
  channelStatus,
  levelLabel,
  OUTCOME_BADGE,
  OUTCOME_LABEL,
  STATUS_BADGE,
} from '../ui/notify-channels'
import {
  buildChannelPatch,
  buildCustomPostRows,
  buildTopPatch,
  levelOptions,
  newStore,
  newStoresForRows,
  topDraftOf,
  type DraftStore,
  type TopDraft,
} from '../ui/notify-form'
import { fmtTime } from '../ui/time'
import {
  channelDialogTitle,
  NotifyChannelForm,
  type ChannelShape,
} from './notify-channel-form'

type LoadState = 'loading' | 'error' | 'ready'

/** 弹窗 id 全站唯一（组件指南 §5.8）。 */
const CHANNEL_ID = 'dlg-notify-channel'
const DELETE_ID = 'dlg-notify-channel-delete'

const NOTICE_ALERT = {
  success: 'alert alert-soft alert-success text-sm',
  info: 'alert alert-soft alert-info text-sm',
} as const

type NoticeTone = keyof typeof NOTICE_ALERT

export const SettingsPage = () => {
  const state = ref<LoadState>('loading')
  const view = ref<NotifyConfigView | null>(null)
  const errorMessage = ref('')
  const actionError = ref('')
  const notice = ref<{ text: string; tone: NoticeTone } | null>(null)

  /** 一次只让一个写在飞（保存顶层 / 保存一家 / 删除 / 试发共用这一枚）。 */
  const busy = ref(false)

  // ── 总开关那四格：非受控，所以是普通对象 ──
  // 重新 `load()` 之后由 [resetTopDraft] 换掉；打字只写进这个对象，不触发任何重建。
  let topDraft: TopDraft = { enabled: false, onlyError: false, minLevel: '', title: '' }
  /** 那四格有没有被动过 —— 只服务保存按钮的禁用，不参与建补丁。 */
  const topTouched = ref(false)

  // ── 渠道编辑弹窗 ──
  const editingName = ref('')
  /** 草稿本身**不是** ref：见文件顶部那条（每敲一个字重建整窗 = 输入框失焦）。 */
  let editStore: DraftStore = {}
  let editRows: DraftStore[] = []
  const revealSecrets = ref(false)
  const formError = ref('')

  // ── 删除确认 ──
  const deleteName = ref('')
  const deleting = ref(false)

  // ── 试发 ──
  let testMessageDraft = ''
  const testTargets = ref<string[]>([])
  const testReport = ref<NotifyTestReport | null>(null)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const res = await getNotifyConfig()
      applyView(res.config)
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /**
   * 视图落位，并把**跟着视图走**的本地状态一起对齐。
   *
   * 保存成功后直接吃 PUT 回的那一份新视图（后端两处都是同一个 payload 构造函数），
   * 不再多跑一次 GET —— 少一次往返，也就少一个"保存成功但刷新失败"的中间态。
   */
  const applyView = (next: NotifyConfigView) => {
    view.value = next
    resetTopDraft(next)
    // 已配的渠道被删掉时，勾着的试发目标要跟着掉，否则会送一个后端 400 的名字。
    testTargets.value = testTargets.value.filter((name) => next.configured.includes(name))
    testReport.value = null
  }

  const resetTopDraft = (source: NotifyConfigView) => {
    topDraft = topDraftOf(source)
    topTouched.value = false
  }

  onMounted(() => void load())

  /** 补丁送出 → 视图落位；失败时把后端那句人话留在页顶（弹窗内的错误另有那条）。 */
  const submitPatch = async (patch: Parameters<typeof putNotifyConfig>[0]) => {
    actionError.value = ''
    const res = await putNotifyConfig(patch)
    applyView(res.config)
  }

  // ─────────────────────────────────────────────────────────────────────────
  // 总开关
  // ─────────────────────────────────────────────────────────────────────────

  const saveTop = async () => {
    const current = view.value
    if (!current || busy.value) return
    busy.value = true
    actionError.value = ''
    try {
      await submitPatch(buildTopPatch(topDraft, current))
      notice.value = { text: '已保存总开关', tone: 'success' }
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '保存失败'
    } finally {
      busy.value = false
    }
  }

  const topCard = () => {
    const current = view.value
    if (!current) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">推送总开关</h2>
            <span class="text-xs opacity-60">
              这份配置整个实例共用（一张单行表，不区分是谁改的）。上次更新：
              {fmtTime(current.updatedAt)}
            </span>
          </div>

          <label class="flex min-h-11 cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              class="toggle toggle-primary"
              aria-label="开启推送"
              checked={topDraft.enabled}
              onChange={(e) => {
                topDraft.enabled = (e.target as HTMLInputElement).checked
                topTouched.value = true
              }}
            />
            <span class="text-sm">开启推送（关掉时下面所有渠道都不会发）</span>
          </label>

          <label class="flex min-h-11 cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              class="toggle toggle-primary"
              aria-label="只推失败"
              checked={topDraft.onlyError}
              onChange={(e) => {
                topDraft.onlyError = (e.target as HTMLInputElement).checked
                topTouched.value = true
              }}
            />
            <span class="text-sm">只推有失败的那一趟（成功的就不吵你）</span>
          </label>

          <label class="flex flex-col gap-1 text-sm md:flex-row md:items-center md:gap-3">
            <span class="shrink-0 opacity-70">正文门槛</span>
            <select
              class="select w-full md:max-w-xs"
              aria-label="正文门槛"
              value={topDraft.minLevel}
              onChange={(e) => {
                topDraft.minLevel = (e.target as HTMLSelectElement).value
                topTouched.value = true
              }}
            >
              {levelOptions(current, levelLabel).map((option) => (
                <option value={option.value}>{option.label}</option>
              ))}
            </select>
            <span class="text-xs opacity-60">低于这一档的过程话不进正文。</span>
          </label>

          <label class="flex flex-col gap-1 text-sm">
            <span class="opacity-70">标题（留空 = 用默认「{current.defaultTitle}」）</span>
            <input
              class="input w-full"
              aria-label="推送标题"
              placeholder={current.defaultTitle}
              value={topDraft.title}
              onInput={(e) => {
                topDraft.title = (e.target as HTMLInputElement).value
                topTouched.value = true
              }}
            />
          </label>

          <div class="mt-1 flex items-center gap-2">
            <button
              type="button"
              class="btn btn-primary min-h-11 md:min-h-0"
              disabled={!topTouched.value || busy.value}
              onClick={() => void saveTop()}
            >
              保存总开关
            </button>
            {/* 没改动时按钮是灰的，这一句解释**为什么灰**（否则像坏了）。 */}
            <span class="text-xs opacity-60">只有改动了的格子会送出去。</span>
          </div>
        </div>
      </div>
    )
  }

  // ─────────────────────────────────────────────────────────────────────────
  // 渠道列表
  // ─────────────────────────────────────────────────────────────────────────

  /** 一家在当前视图里的样子（认不出来的名字不猜，直接跳过渲染）。 */
  const channelViewOf = (name: string) => view.value?.channels[name] ?? null

  const openChannel = (name: string) => {
    const one = channelViewOf(name)
    if (!one) return
    editingName.value = name
    formError.value = ''
    revealSecrets.value = false
    // 每次开窗都从"没填"开始：留空 = 不改，所以重开一次不该带着上一次的半成品。
    editStore = newStore(one.fields)
    editRows =
      name === 'customPost'
        ? newStoresForRows(one.fields, one.count ?? (one.items?.length ?? 0))
        : []
    openDialog(CHANNEL_ID)
  }

  const shape = (): ChannelShape | null => {
    const name = editingName.value
    const one = name ? channelViewOf(name) : null
    if (!name || !one) return null
    return {
      name,
      view: one,
      store: editStore,
      rows: editRows,
      existing: one.count ?? (one.items?.length ?? 0),
    }
  }

  const saveChannel = async () => {
    const current = shape()
    if (!current || busy.value) return
    busy.value = true
    actionError.value = ''
    try {
      // 两家分两条路建，不是因为形状像到不能合并 —— 对象型是**逐格合并**，
      // customPost 是**整块替换**（判据见 `ui/notify-form.ts` 那两个函数各自的抬头）。
      let value: Record<string, unknown> | Record<string, unknown>[]
      if (current.name === 'customPost') {
        const built = buildCustomPostRows(current.view, current.rows)
        if (!built.ok) {
          // 弹窗**内**报错：页顶那条在遮罩背后，用户看不到就等于没提示。
          formError.value = built.error
          return
        }
        value = built.rows
      } else {
        const built = buildChannelPatch(current.view, current.store)
        if (!built.ok) {
          formError.value = built.error
          return
        }
        value = built.patch
      }
      const patch: NotifyConfigPatch = { channels: { [current.name]: value } }
      await submitPatch(patch)
      closeDialog(CHANNEL_ID)
      editingName.value = ''
      notice.value = { text: `已保存 ${channelLabel(current.name)}`, tone: 'success' }
    } catch (e) {
      const message = e instanceof ApiError ? e.message : '保存失败'
      // 400 有两句来源：某一格形状不对，或**这一家必填还没齐**（后端合并后要过
      // `core.NotifyConfig.Validate()` 那道闸）。两句都该落在窗内，用户还在填。
      formError.value = message
    } finally {
      busy.value = false
    }
  }

  const askDelete = (name: string) => {
    deleteName.value = name
    openDialog(DELETE_ID)
  }

  const confirmDelete = async () => {
    const name = deleteName.value
    if (!name || deleting.value) return
    deleting.value = true
    actionError.value = ''
    try {
      // 整家去掉走 [deleteNotifyChannel]：那是本族唯一发 `null` 的口子。
      const res = await deleteNotifyChannel(name)
      applyView(res.config)
      closeDialog(DELETE_ID)
      closeDialog(CHANNEL_ID)
      editingName.value = ''
      deleteName.value = ''
      notice.value = { text: `已删除 ${channelLabel(name)}`, tone: 'info' }
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '删除失败'
    } finally {
      deleting.value = false
    }
  }

  const channelRow = (name: string) => {
    const one = channelViewOf(name)
    if (!one) {
      // 后端加了渠道而前端没跟上：这一格**照样给按钮**（表单全靠视图渲染，不需要认识它），
      // 名字退回原始字符串，让用户至少配得到。
      return (
        <div class="flex flex-wrap items-center justify-between gap-2 border-t border-base-300 py-3 first:border-t-0">
          <span class="break-words text-sm">{channelLabel(name)}</span>
          <button
            type="button"
            class="btn btn-soft btn-sm min-h-11 md:min-h-0"
            onClick={() => openChannel(name)}
          >
            填写
          </button>
        </div>
      )
    }
    const status = channelStatus(one.configured, one.missing)
    return (
      <div class="flex flex-wrap items-center justify-between gap-2 border-t border-base-300 py-3 first:border-t-0">
        <div class="flex min-w-0 flex-wrap items-center gap-2">
          <span class="break-words text-sm">{channelLabel(name)}</span>
          <span class={STATUS_BADGE[status.tone]}>{status.text}</span>
        </div>
        <div class="flex items-center gap-2">
          <span class="text-xs opacity-60">
            {name === 'customPost'
              ? `现有 ${String(one.count ?? 0)} 条`
              : `${String(one.fields.length)} 格`}
          </span>
          <button
            type="button"
            class="btn btn-soft btn-sm min-h-11 md:min-h-0"
            onClick={() => openChannel(name)}
          >
            {one.configured ? '修改' : '填写'}
          </button>
        </div>
      </div>
    )
  }

  const channelCard = () => {
    const current = view.value
    if (!current) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-2">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">推送渠道</h2>
            <span class="text-xs opacity-60">
              配了 {String(current.configured.length)} 家 · 发得到 {String(current.sendable.length)} 家
            </span>
          </div>
          <p class="text-xs leading-5 opacity-60">
            顺序就是发送顺序：一家一趟，按这里从上往下依次发。凭据不回显，所以编辑时
            <span class="break-words">留空表示不改</span>，要整家去掉请用「删除这一家」。
          </p>
          {/* 十家按后端的 `order` 摆。这里不认名字，只认视图给的那一串。 */}
          <div>{current.order.map((name) => channelRow(name))}</div>
        </div>
      </div>
    )
  }

  const channelDialog = () => (
    <Dialog id={CHANNEL_ID} title="推送渠道" size="md" onClose={() => (formError.value = '')}>
      {/* 动态量全在**子节点位置**：`<dialog>` 本体由外层那个 effect 建一次就不动，
          换一家 / 报错 / 勾"显示凭据"只重建里面的这一段。 */}
      <div class="flex flex-col gap-3">
        <h3 class="text-base font-medium">
          {channelDialogTitle(shape()?.view ?? null, editingName.value)}
        </h3>
        {NotifyChannelForm({
          shape,
          formError: () => formError.value,
          busy: () => busy.value,
          reveal: () => revealSecrets.value,
          onSave: () => void saveChannel(),
          onDelete: () => {
            const current = editingName.value
            if (current) askDelete(current)
          },
          onCancel: () => {
            closeDialog(CHANNEL_ID)
            editingName.value = ''
          },
          onToggleReveal: (on) => (revealSecrets.value = on),
          onAddRow: () => {
            const one = shape()
            if (one) editRows = [...editRows, newStore(one.view.fields)]
          },
          onRemoveRow: (index) => {
            editRows = editRows.filter((_, at) => at !== index)
          },
        })}
      </div>
    </Dialog>
  )

  const deleteDialog = () => (
    <Dialog id={DELETE_ID} title="删除这一家？" size="sm">
      <p class="break-words text-sm leading-6">{deleteHint()}</p>
      <div class="modal-action flex-wrap gap-2">
        <button
          type="button"
          class="btn btn-soft min-h-11 md:min-h-0"
          onClick={() => closeDialog(DELETE_ID)}
        >
          取消
        </button>
        <button
          type="button"
          class="btn btn-error min-h-11 md:min-h-0"
          disabled={deleting.value}
          onClick={() => void confirmDelete()}
        >
          {deleting.value ? '删除中…' : '确认删除'}
        </button>
      </div>
    </Dialog>
  )

  /** 确认窗要说的是**影响面**（组件指南 §6.4）：删的是哪一家、掉了会怎样。 */
  const deleteHint = () => {
    const name = deleteName.value
    const one = name ? channelViewOf(name) : null
    if (!name || !one) return '（这一家已经不在列表里了，关掉这一窗就行。）'
    const detail =
      name === 'customPost'
        ? `它现在 ${String(one.count ?? 0)} 条地址`
        : `已填的那些格（含凭据）`
    return `将去掉 ${channelLabel(name)} 这一家的整份配置：${detail}，删掉后要重新填一遍才能再发。只影响这一家，其他九家与总开关都不动。`
  }

  // ─────────────────────────────────────────────────────────────────────────
  // 试发
  // ─────────────────────────────────────────────────────────────────────────

  const toggleTarget = (name: string, on: boolean) => {
    testTargets.value = on
      ? [...testTargets.value, name]
      : testTargets.value.filter((item) => item !== name)
  }

  const sendTest = async () => {
    if (busy.value) return
    busy.value = true
    actionError.value = ''
    testReport.value = null
    try {
      // 标题不送 = 用后端那一份（与总开关的"留空"同一口径），所以这里只送正文与目标。
      const res = await runNotifyTest({
        message: testMessageDraft,
        channels: testTargets.value,
      })
      testReport.value = res
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '试发失败'
    } finally {
      busy.value = false
    }
  }

  const testLine = (result: { channel: string; outcome: string; reason: string }) => {
    const badge = OUTCOME_BADGE[result.outcome] ?? 'badge badge-soft'
    return (
      <li class="flex flex-wrap items-center gap-2">
        <span class="break-words text-sm">{channelLabel(result.channel)}</span>
        <span class={badge}>{OUTCOME_LABEL[result.outcome] ?? result.outcome}</span>
        {result.reason ? (
          <span class="break-words text-xs opacity-70">{result.reason}</span>
        ) : null}
      </li>
    )
  }

  const testCard = () => {
    const current = view.value
    if (!current) return null
    const options = current.order.filter((name) => current.configured.includes(name))
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">试发一条</h2>
          <p class="text-xs leading-5 opacity-60">
            这一条会**真发到选中的那几家**（第三方服务，不碰云盘账号）。
            它绕过了总开关、只推失败与正文门槛那三道闸 —— 所以试发成功不等于自动推会响，
            反过来也一样。
          </p>

          <label class="flex flex-col gap-1 text-sm">
            <span class="opacity-70">正文</span>
            <textarea
              class="textarea w-full"
              rows={3}
              aria-label="试发正文"
              placeholder="这是一条测试消息"
              value={testMessageDraft}
              onInput={(e) => (testMessageDraft = (e.target as HTMLTextAreaElement).value)}
            />
            <span class="text-xs opacity-60">留空则由后端用它那句默认小结发出去。</span>
          </label>

          <fieldset class="fieldset gap-2">
            <legend class="fieldset-legend">发到哪几家</legend>
            {options.length === 0 ? (
              <p class="text-sm opacity-70">还没有配好的渠道：先在上一张卡里填一家。</p>
            ) : (
              options.map((name) => (
                <label class="flex min-h-11 cursor-pointer items-center gap-2 text-sm">
                  <input
                    type="checkbox"
                    class="checkbox checkbox-sm checkbox-primary"
                    checked={testTargets.value.includes(name)}
                    aria-label={`试发到 ${channelLabel(name)}`}
                    onChange={(e) =>
                      toggleTarget(name, (e.target as HTMLInputElement).checked)
                    }
                  />
                  <span class="break-words">{channelLabel(name)}</span>
                  {current.sendable.includes(name) ? null : (
                    <span class="badge badge-soft badge-warning">缺必填</span>
                  )}
                </label>
              ))
            )}
            <p class="fieldset-label">
              只列已配的那几家：没配的渠道点了后端会直接拒。
            </p>
          </fieldset>

          <div class="flex items-center gap-2">
            <button
              type="button"
              class="btn btn-primary min-h-11 md:min-h-0"
              disabled={busy.value || testTargets.value.length === 0}
              onClick={() => void sendTest()}
            >
              {busy.value ? '发送中…' : '发送这一条'}
            </button>
            <span class="text-xs opacity-60">
              {testTargets.value.length === 0 ? '先勾至少一家' : `将发到 ${String(testTargets.value.length)} 家`}
            </span>
          </div>

          {/* ⚠ 这一位**必须**在子节点位置读 `testReport.value`：放进 `testCard()` 的函数体
              就变成"外层 effect 建一次就定格"，而外层那条只跟着 `view` 走 ——
              表现是点完发送、结果区永远不出现（本轮实测到的，见文件顶部那条纪律）。 */}
          {testReport.value ? reportBlock(testReport.value) : null}
        </div>
      </div>
    )
  }

  /** 试发结果那一块（只吃实参，不读 ref）。 */
  const reportBlock = (report: NotifyTestReport) => (
    <div class="flex flex-col gap-2 rounded-box border border-base-300 p-3">
      <p class="break-words text-sm">
        {report.summary}
        <span class="text-xs opacity-60">（标题「{report.title}」）</span>
      </p>
      <ul class="flex flex-col gap-1">{report.results.map((result) => testLine(result))}</ul>
      <p class="text-xs leading-5 opacity-60">
        这一条没有经过的闸：总开关 {report.gates.enabled ? '开' : '关'}、只推失败{' '}
        {report.gates.onlyError ? '开' : '关'}、门槛 {levelLabel(report.gates.minLevel)}。
      </p>
    </div>
  )

  // ─────────────────────────────────────────────────────────────────────────
  // 页面骨架
  // ─────────────────────────────────────────────────────────────────────────

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="min-w-0">
        <h1 class="text-base font-medium">设置</h1>
        <p class="break-words text-xs opacity-70">推送设置：正文门槛、十家渠道与试发。</p>
      </div>
      <button
        type="button"
        class="btn btn-soft btn-sm min-h-11 md:min-h-0"
        disabled={state.value === 'loading'}
        onClick={() => void load()}
      >
        刷新
      </button>
    </div>
  )

  return (
    <div class="flex flex-col gap-4">
      {header()}
      {notice.value ? (
        <div class={NOTICE_ALERT[notice.value.tone]}>
          <span class="break-words">{notice.value.text}</span>
        </div>
      ) : null}
      {actionError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{actionError.value}</span>
        </div>
      ) : null}
      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner loading-lg text-primary" />
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
      {state.value === 'ready' && view.value ? (
        <>
          {topCard()}
          {channelCard()}
          {testCard()}
        </>
      ) : null}

      {channelDialog()}
      {deleteDialog()}
    </div>
  )
}
