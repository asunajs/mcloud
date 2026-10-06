/**
 * 账号配置（`/accounts/push` —— 账号页的子视图，侧栏仍高亮「账号」）。
 *
 * # 这一页是什么（用户口径 2026-10-05）
 *
 * 「我准备拆分一下，**推送单独给配置页面**」——原来推送与任务配置挤在
 * `config-page.tsx`（配置中心）的两个 tab 里，现在拆开：
 *
 * | 页面 | 地址 | 装什么 |
 * |---|---|---|
 * | 配置中心（任务） | `/tasks/config` | 每个任务的**公共配置** + **账号私有配置**（指路） |
 * | **账号配置**（本页） | `/accounts/push` | 每个账号的**推送渠道** + 账号级设置 + 功能开关 |
 *
 * ⚠ 地址留在账号页下面（`/accounts/push`）：侧栏**9 条封顶**是硬约定（`nav.ts`），
 * 推送不属于新分区，而它本来就是"每个账号一份"的东西 ⇒ 挂账号页最顺 ✓。
 *
 * # 与账号页的"设置弹窗"什么关系
 *
 * 弹窗里那两块（任务参数 / 功能开关）一直由 `SHOW_ACCOUNT_SETTINGS_BLOCKS` 藏着
 * （2026-10-02 用户口径「这些设置先取消显示，设置后面单独弄」）—— 本页就是"单独弄"的家 ✓。
 * 弹窗那边**一个字没动**：它照旧只显示"改昵称/设备/凭据"那几格，别顺手把它打开 ✗。
 *
 * # 两个编辑器都是共享模块
 *
 * - `ui/push-editor.tsx`：推送渠道（十条渠道 + 打码字段）
 * - `ui/account-fields.tsx`：账号级两格（备份等待 / 凭据阈值）+ 功能开关（AI 头像上限）
 *
 * ⇒ 本页只负责"挑账号、读设置、整体写回"。`PUT /api/accounts/{id}/settings` 是
 * **整体替换**：两个编辑器合起来正好覆盖 `AccountSettings` 的全部五格，保存时
 * `{...服务端那一份, ...acct.build(), notify: push.build()}` —— 少写一格就是把用户
 * 没碰过的设置静默抹掉 ✗。
 *
 * # 移动端与草稿
 *
 * 与配置中心同一条口径：宽屏左右两栏、窄屏「列表 ↔ 详情」下钻，**只渲染一份**
 * （`matchMedia`，不是 `hidden md:block` 双份进 DOM）；未保存的改动收进**模块级普通 Map**
 * （`onUnmounted` 也要写它 ⇒ **绝不在这里写信号**，否则旧页面"不被替换而被追加"，
 * 那次实测见 `config-page.tsx` 文件头那段 ⚠⚠）。
 */

import { onMounted, onUnmounted, ref } from 'lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  getAccountSettings,
  listAccounts,
  putAccountSettings,
  type AccountSettings,
  type AccountSummary,
} from '../api'
import { createPushEditor, type PushEditorSnapshot } from '../ui/push-editor'
import { createAccountFields, type AccountFieldsSnapshot } from '../ui/account-fields'
import { EmptyState } from '../ui/empty-state'
import { pushToast } from '../ui/toast'
import { navigate } from '../ui/router'
import { IconClose, IconSearch } from '../ui/icons'

export interface AccountConfigPageProps {
  /** 回账号页。 */
  onExit: () => void
}

type LoadState = 'loading' | 'error' | 'ready'

// ── 跨"进出一页"保留的偏好与草稿（模块级 = 会话内；不落盘）────────────────
//
// ⚠ 与 `config-page.tsx` 同一条口径：这些信号**只在 JSX 槽里读**，绝不在组件体顶层读
// （组件体里读模块级信号 ⇒ 外壳那个动态槽订阅它 ⇒ 一改就整页重建）。

/** 选中的账号 id（空串 = 还没选）。 */
const pickedAccount = ref('')
/** 账号列表的搜索词。 */
const query = ref('')
/** 手机上"详情是否已铺开"（false = 看列表）。宽屏不看它。 */
const drillOpen = ref(false)

/** 一个账号的草稿：两个编辑器各一份快照（都是纯数据，可 JSON 化）。 */
interface AccountDraft {
  push: PushEditorSnapshot
  fields: AccountFieldsSnapshot
}
/** ⚠ **普通 Map**，不是信号 —— `onUnmounted` 也要写它（见文件头最后一段）。 */
const drafts = new Map<string, AccountDraft>()
/** 草稿表的"版本号"。只在交互处理器里自增，用来唤醒读了草稿的那些槽。 */
const draftsRev = ref(0)

/**
 * 从别处跳进来时**预选**某个账号（配置中心「账号私有配置」那半的「改它的推送」用）。
 *
 * ⚠ 用模块级状态而不是 URL 参数：这一页的选择本来就是会话内状态（切走再回来还在），
 * 再为"预选哪一个"造一套 `/accounts/push/<id>` 解析只会多一条要维护的路径 ✗。
 * 调用方**先调它再 `navigate()`** ✓。
 */
export const preselectAccount = (id: string) => {
  pickedAccount.value = id
  drillOpen.value = true
}

export const AccountConfigPage = (props: AccountConfigPageProps) => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const accounts = ref<AccountSummary[]>([])

  // 两个编辑器（共享模块；见文件头）
  const push = createPushEditor()
  const acct = createAccountFields()
  const detailState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  const detailError = ref('')
  const saving = ref(false)
  /** 这个账号**完整**的一份设置（GET 回来的）。 */
  const acctSettings = ref<AccountSettings | null>(null)
  /** 载入（或保存）之后的那两份快照文本：未保存修改就是它与当前快照不等。 */
  const pushBaseline = ref('')
  const fieldsBaseline = ref('')
  /** 请求序号：连着点两个账号时，迟到的响应不能覆盖新的选中。 */
  let seq = 0

  const WIDE_QUERY = '(min-width: 768px)'
  const wide = ref(matchMedia(WIDE_QUERY).matches)
  let mq: MediaQueryList | undefined
  const onWideChange = () => {
    if (mq) wide.value = mq.matches
  }

  /**
   * 登记「刷新」给外壳 navbar（与各页同一条约定）。
   *
   * ⚠ 子视图的 `onMounted` **先于**父页面（账号页）跑 ⇒ 直接登记会被父页面那份盖掉，
   * 表现是"在这一页点刷新，刷的是账号列表" ✗ ⇒ 推到微任务里落在外层之后 ✓。
   */
  let alive = true
  let prevRefresh: ReturnType<typeof setPageRefresh> = null

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      accounts.value = await listAccounts()
      ensureSelection()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '读不到账号列表'
      state.value = 'error'
    }
  }

  const accountItems = (): AccountSummary[] => {
    const q = query.value.trim().toLowerCase()
    if (!q) return accounts.value
    return accounts.value.filter((a) =>
      [a.nickname ?? '', a.label ?? '', a.id].some((t) => t.toLowerCase().includes(q)),
    )
  }

  /** 搜索框只在项数多时才出现（三五个账号扫一眼就到底，多一个输入框只是噪音）。 */
  const searchable = (): boolean => accounts.value.length > 6

  const ensureSelection = () => {
    if (!accounts.value.some((a) => a.id === pickedAccount.value)) {
      pickedAccount.value = accounts.value[0]?.id ?? ''
    }
    if (pickedAccount.value && detailState.value === 'idle') void loadSettings(pickedAccount.value)
  }

  const snapshotText = (): string => JSON.stringify(push.snapshot()) + '|' + JSON.stringify(acct.snapshot())
  const baseText = (): string => pushBaseline.value + '|' + fieldsBaseline.value
  const dirty = (): boolean => detailState.value === 'ready' && snapshotText() !== baseText()

  const loadSettings = async (id: string) => {
    detailState.value = 'loading'
    detailError.value = ''
    const mine = ++seq
    try {
      const s = await getAccountSettings(id)
      if (mine !== seq || pickedAccount.value !== id) return
      acctSettings.value = s
      push.load(s.notify)
      acct.load(s)
      // 基线取**服务端那一份**（在草稿之前）⇒ 有草稿时"未保存修改"才是对的
      pushBaseline.value = JSON.stringify(push.snapshot())
      fieldsBaseline.value = JSON.stringify(acct.snapshot())
      const draft = drafts.get(id)
      if (draft) {
        push.restore(draft.push)
        acct.restore(draft.fields)
      }
      detailState.value = 'ready'
    } catch (e) {
      if (mine !== seq) return
      // ⚠ 后端对"别人的账号 / 不存在"回同一个 404（不泄露存在性）⇒ 原话照抄
      detailError.value = e instanceof Error ? e.message : '读不到这个账号的设置'
      detailState.value = 'error'
    }
  }

  /** 把当前这一份脏草稿收进模块级表（干净的就把旧草稿删掉）。⚠ 只写普通 Map。 */
  const stashDraft = () => {
    const id = pickedAccount.value
    if (!id || detailState.value !== 'ready') return
    if (dirty()) drafts.set(id, { push: push.snapshot(), fields: acct.snapshot() })
    else drafts.delete(id)
  }

  /** 读草稿的存在性（顺带订阅 `draftsRev`，见文件头）。 */
  const hasDraft = (id: string): boolean => {
    void draftsRev.value
    return drafts.has(id)
  }

  const selectAccount = (id: string) => {
    if (id !== pickedAccount.value) stashDraft()
    draftsRev.value++
    pickedAccount.value = id
    drillOpen.value = true
    void loadSettings(id)
  }

  /**
   * 保存这个账号的配置。
   *
   * ⚠⚠ **整体替换**语义：请求体必须齐五格。两个编辑器合起来正好覆盖 `AccountSettings`，
   * `...s` 那一下是给"以后 DTO 加了格子、编辑器还没跟上"兜底的（那种格子至少不会被抹成
   * 默认值）。**加了新格子就必须给它画控件**，否则用户看不到、也改不了。
   */
  const save = async () => {
    const s = acctSettings.value
    const id = pickedAccount.value
    if (!s || !id) return
    let next: AccountSettings
    try {
      // 本地能判的当场说（空、不是整数）；**范围**仍然留给后端
      next = { ...s, ...acct.build(), notify: push.build() }
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '配置不完整')
      return
    }
    saving.value = true
    try {
      const after = await putAccountSettings(id, next)
      acctSettings.value = after
      // 用**后端回读的那一份**回填：整体替换的语义下，"我以为存进去的"不算数
      push.load(after.notify)
      acct.load(after)
      pushBaseline.value = JSON.stringify(push.snapshot())
      fieldsBaseline.value = JSON.stringify(acct.snapshot())
      drafts.delete(id)
      draftsRev.value++
      pushToast('success', '已保存 —— 只对这个账号生效')
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '保存失败')
    } finally {
      saving.value = false
    }
  }

  /**
   * 放弃修改 = 丢掉草稿、重新拉服务端那一份。
   *
   * ⚠ 重新 GET 而不是"把基线塞回去"：基线只存了文本，而"服务端那份长什么样"只有服务端
   * 知道 —— 本地拼一份出来迟早与它分叉。
   */
  const discard = () => {
    const id = pickedAccount.value
    if (!id) return
    drafts.delete(id)
    draftsRev.value++
    detailState.value = 'idle'
    void loadSettings(id)
    pushToast('info', '已回到上次保存的那一份')
  }

  onMounted(() => {
    void load()
    // ⚠ 推到微任务：子视图的 onMounted 先于父页面跑（见上面那段注释）
    queueMicrotask(() => {
      if (!alive) return
      prevRefresh = setPageRefresh({
        run: () => {
          void load()
          if (pickedAccount.value) void loadSettings(pickedAccount.value)
        },
      })
    })
    mq = matchMedia(WIDE_QUERY)
    mq.addEventListener('change', onWideChange)
    onWideChange()
  })

  onUnmounted(() => {
    alive = false
    // ⚠ 只写普通 Map，绝不碰信号（含 `draftsRev`）——理由见文件头最后一段
    stashDraft()
    setPageRefresh(prevRefresh)
    mq?.removeEventListener('change', onWideChange)
    mq = undefined
  })

  // ── 渲染 ──────────────────────────────────────────────────────────────

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
    <div id="acct-rail" class="flex flex-col gap-3">
      {searchable() ? (
        <label class="input input-sm flex items-center gap-2">
          <IconSearch class="size-4 shrink-0 opacity-50" />
          <input
            type="search"
            class="min-w-0 flex-1"
            placeholder="搜昵称 / 手机号…"
            aria-label="搜索账号"
            value={query.value}
            onInput={(e) => (query.value = (e.target as HTMLInputElement).value)}
          />
        </label>
      ) : null}

      <div class="flex items-center justify-between px-1">
        <span class="text-xs font-medium opacity-70">账号</span>
        <span class="badge badge-soft badge-sm">{accounts.value.length} 项</span>
      </div>

      {/* ⚠ 作用域写在左栏上：这一页只影响**左边选中的那一个**账号（推送是账号级的） */}
      <p class="px-1 text-[11px] leading-4 opacity-60">
        每个账号各一份：只影响左边选中的那一个账号。
      </p>

      {/* ⚠ `.map()` 必须在**元素**的子节点位置（三元分支 / 片段里的 map 认领不到 key ⇒
          编译期红「这里的 key 什么都不做」，见 `config-page.tsx` 里那段 ⚠） */}
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
              <span class="block truncate text-sm font-medium">{a.nickname || a.label || a.id}</span>
              {/* ⚠ 副行只在**有昵称**时才画：没昵称时主行印的就是 `label`，再印一遍是重复 */}
              {a.nickname ? (
                <span class="block truncate font-mono text-[11px] opacity-60">{a.label}</span>
              ) : null}
            </span>
            {hasDraft(a.id) ? <span class="badge badge-warning badge-sm shrink-0">未保存</span> : null}
          </button>
        ))}
      </div>

      {accounts.value.length === 0 ? (
        <EmptyState
          title="还没有已登录的账号。"
          hint="推送是按账号配的：先登录一个云盘账号，再回来配它。"
          action={
            <button type="button" class="btn btn-soft" onClick={() => navigate('/accounts')}>
              去登录
            </button>
          }
        />
      ) : null}
      {accounts.value.length > 0 && accountItems().length === 0 ? (
        <EmptyState title="没有匹配的账号。" hint="换个关键词试试（搜的是昵称、掩码手机号与账号 id）。" />
      ) : null}
    </div>
  )

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
            <button type="button" class="btn btn-sm btn-soft" onClick={() => void loadSettings(pickedAccount.value)}>
              重试
            </button>
          </div>
        ) : null}

        {detailState.value === 'ready' ? (
          <>
            <div class="flex flex-wrap items-start justify-between gap-2">
              <h2 class="card-title min-w-0 text-base">
                {accounts.value.find((a) => a.id === pickedAccount.value)?.nickname ||
                  accounts.value.find((a) => a.id === pickedAccount.value)?.label ||
                  pickedAccount.value}
              </h2>
              {actions()}
            </div>

            <p class="text-sm opacity-70">
              <span class="badge badge-soft badge-sm mr-1">账号级</span>
              这一页只对左边这个账号生效（本仓没有进程级兜底）。按任务的项（例如"跳过哪些任务"）
              在「任务配置」那一页里，两处各管各的。
            </p>

            <div class="flex flex-col gap-2">
              <h4 class="text-sm font-medium">推送渠道</h4>
              {/* 十条渠道 + 打码字段的整块表单在 `ui/push-editor.tsx`（账号页的设置弹窗共用） */}
              {push.fields()}
            </div>

            <div class="flex flex-col gap-2">
              <h4 class="text-sm font-medium">账号级设置</h4>
              <p class="text-xs leading-5 opacity-60">
                与具体任务无关的两格（范围以后端为准，前端不实现第二遍）。
              </p>
              {/* 两格（备份等待 / 凭据阈值）在 `ui/account-fields.tsx` */}
              {acct.accountFields()}
            </div>

            <div class="flex flex-col gap-2">
              <h4 class="text-sm font-medium">功能开关</h4>
              {/* AI 头像上限那一格：账号页的设置弹窗里被 `SHOW_ACCOUNT_SETTINGS_BLOCKS`
                  藏着，这里是它真正的家（2026-10-02「设置后面单独弄」）*/}
              {acct.featureFields()}
            </div>
          </>
        ) : null}

        {detailState.value === 'idle' ? (
          <p class="py-10 text-center text-sm opacity-70">左边挑一个账号，这里配它的推送与账号级设置。</p>
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
      {/* ⚠ `#acct-detail` 这个 id 是给门禁用的锚（左右两边都印着账号名 ⇒ 只能靠容器区分） */}
      <div id="acct-detail" class="min-w-0">
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
          ← 全部账号
        </button>
        <div id="acct-detail" class="min-w-0">
          {detail()}
        </div>
      </div>
    ) : (
      rail()
    )

  // ⚠ 与设置页/配置中心同一条口径：整页宽度封顶（超大屏别把表单拉成一条长线）
  return (
    <div class="mx-auto flex w-full max-w-7xl flex-col gap-4">
      <div class="flex flex-wrap items-center gap-2">
        <button
          type="button"
          class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
          aria-label="回账号页"
          onClick={() => props.onExit()}
        >
          <IconClose class="size-4" />
        </button>
        <h1 class="text-base font-medium">账号配置</h1>
        <span class="text-xs opacity-60">推送渠道 + 账号级设置</span>
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
