/**
 * 账号配置（`/accounts/settings` —— 账号页的子视图，侧栏仍高亮「账号」）。
 *
 * # 这一页是什么（用户口径 2026-10-05；2026-10-08 收掉推送那一块）
 *
 * 「我准备拆分一下，**推送单独给配置页面**」——原来推送与任务配置挤在
 * `config-page.tsx`（配置中心）的两个 tab 里，现在拆开：
 *
 * | 页面 | 地址 | 装什么 |
 * |---|---|---|
 * | 配置中心（任务） | `/tasks/config` | 每个任务的**公共配置** |
 * | **账号配置**（本页） | `/accounts/settings` | 账号级设置 + 功能开关 |
 * | **推送**（独立一页） | `/push` | **每个登录用户一份**的推送渠道 + 规则 |
 *
 * ⚠⚠ 2026-10-08 用户口径（「**很显然我说的和云盘账户没有关系**，为啥你这里云盘账户
 * 有配置」＋「**丢弃现有情况，重写**」）：**推送渠道整块从本页删掉了** ✗ ——
 * 渠道 / 规则 / 绑定**三层同一口径：每个登录用户一份** ✓。
 * ⚠ 它先落在「设置」页那张卡上，当天又按用户口径「并单独给推送一个」搬进了
 * **独立的「推送」页**（`/push`，`pages/push-page.tsx` ✓）—— 所以指路一律指「推送」页 ✓。
 * 本页只留**真的按账号算**的两块：账号级设置（备份等待 / 凭据阈值）与功能开关 ✓。
 *
 * ⚠ 地址是 **`/accounts/settings`**（2026-10-05 挂账号页下面、不新开导航项 ✓；
 * 2026-10-08 从 `/accounts/push` 改过来 ✓ —— 推送渠道搬走之后，
 * 路径里那个 `push` 就名不副实了 ✗）。老地址 `/accounts/push` **照旧能打开** ✓
 * （`accounts-page.tsx` 的视图判定里留着那一支 ✓）。
 *
 * # 与账号页的"设置弹窗"什么关系
 *
 * 弹窗里那两块（任务参数 / 功能开关）一直由 `SHOW_ACCOUNT_SETTINGS_BLOCKS` 藏着
 * （2026-10-02 用户口径「这些设置先取消显示，设置后面单独弄」）—— 本页就是"单独弄"的家 ✓。
 * 弹窗那边照旧只显示"改昵称/设备/凭据"那几格，别顺手把它打开 ✗。
 *
 * # 「凭据自动刷新」那张卡（2026-10-11 用户口径「刷新auth」✓）
 *
 * ⚠ 它是**每用户一份**的设置（`data/auth-refresh.json` 按 owner 存 ✓，接口
 * `GET|PUT /api/auth/refresh-schedule` ✓），**不是**账号级 —— 但它紧挨着
 * `refreshTokenDays`（「剩余多少天刷新token」✓）显示，
 * 因为两格回答的是**同一件事的两半**：
 *
 * | 格 | 是什么 | 归谁 |
 * |---|---|---|
 * | 「剩余多少天刷新token」（账号级 ✓） | **判据**：剩不到这么多天**才**刷 ✓ | 每个账号一份 ✓ |
 * | 「你自己**的**刷新时间」（本卡 ✓） | **钟点**：每天几点**去检查一次** ✓ | **每个登录用户一份** ✓ |
 *
 * ⚠⚠ 「**每用户一份**」是 2026-10-11 第二轮用户口径（原话：「**要每用户一个钟点**」✓）——
 * 改之前它是**实例级**一格（全实例共用一个钟点 ✗）。所以这张卡上的文案必须让用户
 * 一眼看出"**改的是我自己那一份**" ✓：
 * 改它**只影响你自己**的检查钟点，多用户实例里别人那一份**一个字节都不动** ✓
 *（后端只认会话里的那个用户 —— 连"指名改别人"都回 404 ✓）。
 *
 * ⇒ 到点之后**先走既有判据**：不需要刷就跳过，并把原因写进日志 ✓
 *（见 `mcloud_core::auth_refresh` 模块头 ✓）。
 * ⚠ 别把两者合并成一句"到点就刷" ✗ —— 那会让"还剩 300 天"的账号每天白打一次上游、换一串新凭据 ✗。
 *
 * # 编辑器是共享模块
 *
 * - `ui/account-fields.tsx`：账号级两格（备份等待 / 凭据阈值）+ 功能开关（AI 头像上限）
 *
 * ⇒ 本页只负责"挑账号、读设置、整体写回"。`PUT /api/accounts/{id}/settings` 是
 * **整体替换**：保存时 `{...服务端那一份, ...acct.build()}` —— 少写一格就是把用户
 * 没碰过的设置静默抹掉 ✗。
 *
 * # 移动端与草稿
 *
 * 与配置中心同一条口径：宽屏左右两栏、窄屏「列表 ↔ 详情」下钻，**只渲染一份**
 * （`matchMedia`，不是 `hidden md:block` 双份进 DOM）；未保存的改动收进**模块级普通 Map**
 * （`onUnmounted` 也要写它 ⇒ **绝不在这里写信号**，否则旧页面"不被替换而被追加"，
 * 那次实测见 `config-page.tsx` 文件头那段 ⚠⚠）。
 */

import { onMounted, onUnmounted, ref } from '@asunajs/lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  getAccountSettings,
  getAuthRefreshSchedule,
  listAccounts,
  putAccountSettings,
  saveAuthRefreshSchedule,
  type AccountSettings,
  type AccountSummary,
  type AuthRefreshSchedule,
} from '../api'
import { createAccountFields, type AccountFieldsSnapshot } from '../ui/account-fields'
import { EmptyState } from '../ui/empty-state'
import { pushToast } from '../ui/toast'
import { navigate } from '../ui/router'
import { IconAlert, IconClose, IconSearch } from '../ui/icons'

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

/** 一个账号的草稿：账号级编辑器那一份快照（纯数据，可 JSON 化）。 */
interface AccountDraft {
  fields: AccountFieldsSnapshot
}
/** ⚠ **普通 Map**，不是信号 —— `onUnmounted` 也要写它（见文件头最后一段）。 */
const drafts = new Map<string, AccountDraft>()
/** 草稿表的"版本号"。只在交互处理器里自增，用来唤醒读了草稿的那些槽。 */
const draftsRev = ref(0)

export const AccountConfigPage = (props: AccountConfigPageProps) => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const accounts = ref<AccountSummary[]>([])

  // 账号级编辑器（共享模块；见文件头）
  const acct = createAccountFields()
  const detailState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  const detailError = ref('')
  /**
   * 保存失败的那一句（**弹窗 + 就近**，2026-10-08 用户口径「所有的提示都是优先弹窗式」）。
   * 画在卡片标题那一行下面（「保存」就在那一行右端）——
   * 以前这里**只有** toast，长表单里滚动之后看不到；而 `detailError` 是"读不出来"
   * （配重试），不是"存不进去"，两者不能混用 ✓。
   */
  const saveError = ref('')
  const saving = ref(false)

  // ── 凭据自动刷新（**每用户一份**那张卡 ✓，见文件头）────────────────────
  //
  // ⚠ `schedule` 为 `null` = 后端没有这条接口（老后端）⇒ **整张卡不画** ✓
  //（画一张只能报错的卡，用户拿它没办法 ✓ —— 与直播那张卡的处置一致 ✓）。
  const schedule = ref<AuthRefreshSchedule | null>(null)
  /** 输入框里的草稿（`HH:MM` ✓）。它**不是**权威值 —— 保存后拿后端回的那份回填 ✓。 */
  const scheduleAt = ref('')
  const scheduleEnabled = ref(false)
  const scheduleSaving = ref(false)
  const scheduleError = ref('')
  /** 这个账号**完整**的一份设置（GET 回来的）。 */
  const acctSettings = ref<AccountSettings | null>(null)
  /** 载入（或保存）之后的那份快照文本：未保存修改就是它与当前快照不等。 */
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

  /**
   * 读「凭据自动刷新」那张卡。
   *
   * ⚠ 读失败**不把整页打成 error**：账号列表与账号级设置与它无关 ✓ ——
   * 一个读不到的卡不该让"改昵称阈值"这件事也做不成 ✓（只把错误留在卡里 ✓）。
   *
   * ⚠ 回的是**你自己**那一份（后端按会话里的用户取 ✓）——
   * 界面这边**不需要**带任何"我是谁"（带了也没用：后端只认会话 ✓）。
   */
  const loadSchedule = async () => {
    try {
      const st = await getAuthRefreshSchedule()
      schedule.value = st
      scheduleAt.value = st.at
      scheduleEnabled.value = st.enabled
      scheduleError.value = ''
    } catch (e) {
      // 接口不存在（老后端 / 404）⇒ 整张卡不画 ✓；其余错误留在卡里 ✓
      schedule.value = null
      scheduleError.value = e instanceof Error ? e.message : '读不到凭据自动刷新的设置'
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

  const snapshotText = (): string => JSON.stringify(acct.snapshot())
  const baseText = (): string => fieldsBaseline.value
  const dirty = (): boolean => detailState.value === 'ready' && snapshotText() !== baseText()

  const loadSettings = async (id: string) => {
    detailState.value = 'loading'
    detailError.value = ''
    const mine = ++seq
    try {
      const s = await getAccountSettings(id)
      if (mine !== seq || pickedAccount.value !== id) return
      acctSettings.value = s
      acct.load(s)
      // 基线取**服务端那一份**（在草稿之前）⇒ 有草稿时"未保存修改"才是对的
      fieldsBaseline.value = JSON.stringify(acct.snapshot())
      const draft = drafts.get(id)
      if (draft) {
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
    if (dirty()) drafts.set(id, { fields: acct.snapshot() })
    else drafts.delete(id)
  }

  /** 读草稿的存在性（顺带订阅 `draftsRev`，见文件头）。 */
  const hasDraft = (id: string): boolean => {
    void draftsRev.value
    return drafts.has(id)
  }

  const selectAccount = (id: string) => {
    saveError.value = ''
    if (id !== pickedAccount.value) stashDraft()
    draftsRev.value++
    pickedAccount.value = id
    drillOpen.value = true
    void loadSettings(id)
  }

  /**
   * 保存这个账号的配置。
   *
   * ⚠⚠ **整体替换**语义：请求体必须齐 `AccountSettings` 的每一格。`...s` 那一下是给
   * "以后 DTO 加了格子、编辑器还没跟上"兜底的（那种格子至少不会被抹成默认值）。
   * **加了新格子就必须给它画控件**，否则用户看不到、也改不了。
   * ⚠ 推送渠道**不在这里**了（2026-10-08 收成登录用户级 ⇒ 见「推送」页 `/push` ✓）。
   */
  const save = async () => {
    const s = acctSettings.value
    const id = pickedAccount.value
    if (!s || !id) return
    let next: AccountSettings
    try {
      // 本地能判的当场说（空、不是整数）；**范围**仍然留给后端
      next = { ...s, ...acct.build() }
    } catch (e) {
      const msg = e instanceof Error ? e.message : '配置不完整'
      saveError.value = msg
      pushToast('error', msg)
      return
    }
    saveError.value = ''
    saving.value = true
    try {
      const after = await putAccountSettings(id, next)
      acctSettings.value = after
      // 用**后端回读的那一份**回填：整体替换的语义下，"我以为存进去的"不算数
      acct.load(after)
      fieldsBaseline.value = JSON.stringify(acct.snapshot())
      drafts.delete(id)
      draftsRev.value++
      pushToast('success', '已保存 —— 只对这个账号生效')
    } catch (e) {
      const msg = e instanceof Error ? e.message : '保存失败'
      saveError.value = msg
      // 弹窗优先：错误类 toast 不自动消失（见 `ui/toast.ts`）
      pushToast('error', msg)
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
    void loadSchedule()
    // ⚠ 推到微任务：子视图的 onMounted 先于父页面跑（见上面那段注释）
    queueMicrotask(() => {
      if (!alive) return
      prevRefresh = setPageRefresh({
        run: () => {
          void load()
          void loadSchedule()
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

  // ── 凭据自动刷新：存那两格（开关 / 你自己的刷新时间 ✓）─────────────────

  /**
   * 存一次（**只发改动的那一格** ✓）。
   *
   * ⚠ 后端是**双 `Option`** 语义（缺席 = 保持原样 ✓）⇒ 只改时间那一次**不带**
   * `enabled` —— 带上 `enabled: true` 会把用户关掉的开关静默打开 ✗，
   * 而那正是用户口径里点名不许的事 ✓。
   *
   * ⚠ 改的**只有你自己**那一份（后端按会话取 owner ✓）—— 这里**不带**任何
   * "给谁"的字段 ✓（后端收到指名的会回 404 ✓，见 `AuthRefreshScheduleInput` ✓）。
   *
   * ⚠ 返回的是**后端重算后的新状态** ⇒ 必须拿它回填（自己猜的那份与后端刚写下去的
   * 那份一旦不一致，用户看到的就是"我点了没反应" ✗）。
   */
  const saveSchedule = async (patch: { enabled?: boolean; at?: string }) => {
    scheduleSaving.value = true
    scheduleError.value = ''
    try {
      const st = await saveAuthRefreshSchedule(patch)
      schedule.value = st
      scheduleAt.value = st.at
      scheduleEnabled.value = st.enabled
      // ⚠ 说清"是你自己那份"（多用户下每人一个钟点 ✓）——
      // 只说"已保存"的话，用户会以为这一下把全实例都改了 ✗。
      pushToast(
        'success',
        st.enabled ? `已保存：你自己的刷新时间是每天 ${st.at}` : '已保存：自动刷新已关闭',
      )
    } catch (e) {
      // 失败：把草稿**退回**到服务端那一份（否则输入框里留着那个没存进去的值 ✓）
      const msg = e instanceof Error ? e.message : '保存失败'
      scheduleError.value = msg
      pushToast('error', msg)
      if (schedule.value) {
        scheduleAt.value = schedule.value.at
        scheduleEnabled.value = schedule.value.enabled
      }
    } finally {
      scheduleSaving.value = false
    }
  }

  /** 预设时间那一格的本地形状检查（**范围仍以后端为准** —— 前端不实现第二遍 ✓）。 */
  const scheduleAtLooksValid = (): boolean => /^\d{1,2}:\d{2}$/.test(scheduleAt.value.trim())

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
          hint="这里配的是「每个账号各一份」的账号级设置：先登录一个云盘账号，再回来配它。"
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

  /**
   * 「凭据自动刷新」那张卡（**每用户一份** ✓，见文件头）。
   *
   * ⚠ 它是**系统维护的那条定时**（`auto-auth-refresh-<你自己>` ✓）在界面上的家：
   * 开关与时间都落在那条定时与**你自己**那一份钟点上 ✓
   *（`GET/PUT /api/auth/refresh-schedule` ✓），
   * 而它在「定时」页里**也看得到**（带着 `managedBy: "auth-refresh"` ✓）。
   *
   * ⚠ 文案必须说清"**这是你自己那一份**"：多用户实例里每人一个钟点 ✓，
   * 改这里**不动别人** ✓ —— 不说的话，用户会以为"我改了这一下全实例都跟着变" ✗。
   */
  const scheduleCard = () => {
    const st = schedule.value
    // 后端没有这条接口（老后端 / 读失败）⇒ 整张卡不画（见 `schedule` 那段说明 ✓）
    if (st == null) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">凭据自动刷新</h2>
          <p class="text-xs leading-5 opacity-60">
            每天到下面这个钟点，系统会<b>检查一次</b><b>你自己名下</b>云盘账号的凭据 ——
            <b>该刷才刷</b>：还没到上面那个「剩余多少天刷新token」的账号会跳过，
            一枪都不打上游。
          </p>
          <p class="text-xs leading-5 opacity-60">
            ⚠ 这个钟点是<b>每个登录用户各一份</b>：改它<b>只动你自己</b>的检查时间，
            别人那一份不受影响。
          </p>

          <label class="flex min-h-11 cursor-pointer items-center gap-2">
            <input
              id="auth-refresh-enabled"
              type="checkbox"
              class="toggle toggle-primary"
              aria-label="凭据自动刷新开关"
              checked={scheduleEnabled.value}
              disabled={scheduleSaving.value}
              onChange={(e) => {
                const on = (e.target as HTMLInputElement).checked
                // 先在本地翻（手感即时 ✓），存失败时由 `saveSchedule` 退回 ✓
                scheduleEnabled.value = on
                void saveSchedule({ enabled: on })
              }}
            />
            <span class="text-sm">到点自动检查并刷新</span>
          </label>

          <fieldset class="fieldset">
            <legend class="fieldset-legend">你自己的刷新时间（东八区）</legend>
            <div class="flex flex-wrap items-center gap-2">
              <input
                id="auth-refresh-at"
                type="time"
                class="input input-sm w-32"
                aria-label="你自己的刷新时间"
                value={scheduleAt.value}
                disabled={scheduleSaving.value}
                onInput={(e) => (scheduleAt.value = (e.target as HTMLInputElement).value)}
              />
              <button
                type="button"
                class="btn btn-sm btn-primary min-h-11 md:min-h-0"
                disabled={scheduleSaving.value || !scheduleAtLooksValid() || scheduleAt.value === st.at}
                onClick={() => void saveSchedule({ at: scheduleAt.value })}
              >
                {scheduleSaving.value ? '保存中…' : '保存时间'}
              </button>
              {scheduleAt.value !== st.at && scheduleAtLooksValid() ? (
                <span class="badge badge-warning badge-sm">还没保存</span>
              ) : null}
            </div>
            {/* ⚠ 失败那一句就近 inline（保存键就在旁边 ✓）+ 上面那句 toast ✓ */}
            {scheduleError.value ? (
              <p class="mt-2 text-xs break-words text-error">{scheduleError.value}</p>
            ) : null}
          </fieldset>

          <p class="text-xs leading-5 opacity-60">
            下一次触发：
            <b>{st.nextAt ?? '算不出来'}</b>
            {st.enabled ? null : '（已关闭，到点不会跑）'}
          </p>
          {st.note ? (
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span class="break-words">{st.note}</span>
            </div>
          ) : null}
          <p class="text-xs leading-5 opacity-60">
            这条定时在「定时」页里也看得到（标着「系统维护」）。
            命令行上 <code>mcloud auth auto-schedule at HH:MM</code> 只作用于**本机那个用户** ——
            你自己的这份在这里改 ✓。
          </p>
        </div>
      </div>
    )
  }

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

            {/* ⚠ 失败那一句在**标题行下面**（保存键就在那一行右端），不是表单底部 ✓ */}
            {saveError.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <IconAlert class="size-4 shrink-0" />
                <span class="break-words">{saveError.value}</span>
              </div>
            ) : null}

            <p class="text-sm opacity-70">
              <span class="badge badge-soft badge-sm mr-1">账号级</span>
              这一页只对左边这个账号生效。按任务的项（例如"跳过哪些任务"）
              在「任务配置」那一页里，两处各管各的。
              {/* ⭐ 2026-10-08 用户就是被这一点绕住的（「为啥你这里云盘账户有配置」✗）⇒
                  在页面里**明说**推送搬到哪儿了，别让用户在这页找渠道 ✓。
                  ⚠ 落点当天又变过一次：渠道与规则一起进了**独立的「推送」页**（`/push` ✓，
                  用户口径「并单独给推送一个」）—— 别再指去「设置」页 ✗。 */}
              推送渠道不在这里：它是"你这个登录用户"一份的，在
              <button
                type="button"
                class="link link-primary"
                onClick={() => navigate('/push')}
              >
                推送
              </button>
              页里配。
            </p>

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
          <p class="py-10 text-center text-sm opacity-70">左边挑一个账号，这里配它的账号级设置。</p>
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
      <div class="flex min-w-0 flex-col gap-4">
        {/* ⚠ `#acct-detail` 这个 id 是给门禁用的锚（左右两边都印着账号名 ⇒ 只能靠容器区分） */}
        <div id="acct-detail" class="min-w-0">
          {detail()}
        </div>
        {/*
          「凭据自动刷新」是**每用户一份**的（2026-10-11 起 ✓），放在**按账号那份卡下面**、
          而不是塞进账号卡里 —— 混进去会让人以为"换个账号就是另一套时间" ✗。
          ⚠ 它也不在账号卡里，因为它**不按账号**：它是"**你自己**每天几点去检查" ✓。
        */}
        {scheduleCard()}
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
        <div class="flex flex-col gap-4">
          <div id="acct-detail" class="min-w-0">
            {detail()}
          </div>
          {scheduleCard()}
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
        <span class="text-xs opacity-60">账号级设置 + 功能开关</span>
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
