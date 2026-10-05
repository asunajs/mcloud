/**
 * 每账号推送（notify / PushConfig）编辑器 —— **工厂函数**，不是组件。
 *
 * # 为什么是工厂而不是组件
 *
 * 这块表单的**归属**在不同页面不同：账号页存的是"某个账号的一份设置"，
 * 配置中心页存的是"默认推送配置"。工厂只负责**画**与**攒 payload**，值存在
 * 编辑器自己的 ref 里、由调用方决定"什么时候 load / 什么时候 build"。
 * 抽成共享模块的唯一目的：**渠道表单本体只有这一份**（原先在
 * `pages/accounts-page.tsx` 里，配置中心页要复用时不能再抄第二份 ——
 * 两份实现迟早分叉，那时候"账号页能配、配置中心配不了"就是这么来的）。
 *
 * # ⚠ 为什么内部全是普通函数 + `get: () => string`（thunk），不是组件
 *
 * lite 是编译期的"指哪儿改哪儿"：一个 `{expr}` 只重建**读到过变化信号**的那一小块。
 * - 普通函数被 `{fields()}` / `{channelFields(key)}` 这样调用时，调用本身发生在一条
 *   effect 里；若在这里**当场**读出值（`const v = cfgText(...)`）再当字符串传下去，
 *   那条 effect 就订阅了 `settingsNotify` —— 敲一个字整块表单重建、焦点全丢
 *   （2026-10-03 review §2.2 记的就是这个真 bug，搬迁时**必须保持这个形状**）。
 *   写成 `value={get()}` 时读发生在**属性自己的 effect** 里，只更新那一个输入框。
 * - 组件不行：组件的 props 是在组件体里读的，而组件体只跑一次、不在任何 effect 里，
 *   读到的值不会更新（本框架没有虚拟 DOM 的那层 diff）。
 *
 * # 为什么"结构"与"值"分成两个 ref
 *
 * 渠道列表的**结构**（有哪些卡片）读 `settingsChannels`，卡片里的**值**读
 * `settingsNotify`。合成一个对象的话，往输入框敲一个字就会让整块渠道表单重建
 * ⇒ 每敲一下就丢一次焦点。
 *
 * # 为什么渠道配置留着**原始对象**
 *
 * PUT 是整体替换。而后端给每个渠道留了 `#[serde(flatten)] extra`（TS 的语义：多余
 * 字段原样展开进请求体），那些键我们**没有建模**。只挑认识的字段回写，等于把用户
 * 没碰过的字段静默抹掉 —— 所以界面持有 GET 回来的那个对象本身，只改看得见的键。
 */

import { ref } from 'lite'
import {
  NOTIFY_CHANNELS,
  type NotifyChannelKey,
  type PushConfig,
  type PushLogLevel,
} from '../api'
import { Select } from './select'

// ── 推送渠道的常量（与后端 DTO 一一对应）───────────────────────────────

/** 推送标题的默认值 —— 与后端 `mcloud_core::push::DEFAULT_TITLE` 同字面量。 */
const DEFAULT_NOTIFY_TITLE = 'mcloud 运行推送'

/** 推送渠道的人话名字。`Record` ⇒ 加渠道时 TS 会报缺一项。 */
const CHANNEL_LABEL: Record<NotifyChannelKey, string> = {
  pushplus: 'PushPlus',
  serverChan: 'Server 酱',
  workWeixinBot: '企业微信机器人',
  dingTalk: '钉钉机器人',
  customPost: '自定义 POST',
  workWeixin: '企业微信应用',
  tgBot: 'Telegram Bot',
  bark: 'Bark',
  twoIm: '回逍（2Im）',
  email: '邮箱',
}

/**
 * 渠道的**必填**字段 —— 与后端 `validate_notify` 逐条对应。
 *
 * 判据只有一条：这些字段**全空** ⇒ 这个渠道算"没配"，PUT 时整条不带
 * （⚠ **不发空对象**：`{token:""}` 会被后端当成"填了但为空"⇒ 400，而用户只是没填）。
 * 填了一部分就照发 —— 后端会指名缺哪一格，那比前端自己猜准。
 */
const CHANNEL_REQUIRED: Record<NotifyChannelKey, readonly string[]> = {
  pushplus: ['token'],
  serverChan: ['token'],
  workWeixinBot: ['url'],
  dingTalk: ['token'],
  customPost: ['url'],
  workWeixin: ['corpid', 'corpsecret', 'agentid'],
  tgBot: ['token', 'chat_id'],
  bark: ['key'],
  twoIm: ['key', 'sid'],
  email: ['host', 'from', 'pass'],
}

/**
 * 渠道里必须是 **JSON 数字**的字段（后端是 `i64` / `u16`）。
 *
 * 界面只能给字符串，提交时转 —— 不转的话后端回的是
 * `invalid type: string, expected i64`，一句用户看不懂的话。
 * `chat_id` 也在里面，但它可以是 `@channelname`：不是整数的就原样留着（后端收 `Value`）。
 */
const CHANNEL_NUMERIC: Partial<Record<NotifyChannelKey, readonly string[]>> = {
  workWeixin: ['agentid'],
  tgBot: ['chat_id'],
  email: ['port'],
}

/**
 * 每个渠道一句话说明（配在哪、有什么坑）。
 *
 * ⚠ 这些是**界面上的字**，不是 markdown：别写 `**粗体**` —— 它会被原样画出来
 * （用户看到的是两个星号）。要强调就用「」。原稿从账号页搬过来时带着 `**`，
 * 而那一块当时**没渲染**（`SHOW_ACCOUNT_SETTINGS_BLOCKS=false`）所以一直没人看见；
 * 2026-10-05 配置中心把它渲染出来才发现 ✗。
 */
const CHANNEL_HINT: Record<NotifyChannelKey, string> = {
  pushplus: 'PushPlus（微信推送）：在 pushplus.plus 拿 token，一条 token 就是一个收件人。',
  serverChan: 'Server 酱：sct.ftqq.com 拿 SendKey（SCT 开头），填进来即可。',
  workWeixinBot: '企业微信「群机器人」：群设置 → 群机器人 → 添加 → 复制 Webhook 地址。',
  dingTalk:
    '钉钉「群机器人」：群设置 → 智能群助手 → 添加机器人 → 自定义 → 拿 Webhook 里的 access_token；安全设置选了「加签」时还要填密钥。',
  customPost:
    '自定义 POST：给你自己的接口发一条 HTTP 请求。URL 里可以用 {title} / {text} 占位符。',
  workWeixin:
    '企业微信「应用消息」：要企业 ID（corpid）、应用密钥（corpsecret）与应用 ID（agentid）三样；与上面的「机器人」不是一回事。',
  tgBot:
    'Telegram Bot：@BotFather 拿 token；chat_id 是收件会话（群 id 是负数，也可以写 @channelname）。',
  bark: 'Bark（iOS 推送）：App 首页那串 key。',
  twoIm: '回逍（2Im）：key 与 sid 都要填。',
  email:
    '⚠ 本版「不会真的发信」（后端还没实现 SMTP）。配了只会存下来、不会生效 —— 摆在这里是为了让旧配置看得见、改得动。',
}

/** 推送日志级别选项。级别越小越严重，过滤规则是 `level <= max`。 */
const MIN_LEVELS: readonly { value: PushLogLevel; label: string }[] = [
  { value: 'error', label: 'error —— 只推错误（最安静）' },
  { value: 'warn', label: 'warn —— 错误 + 警告' },
  { value: 'info', label: 'info —— 默认：再带上信息与成功' },
  { value: 'debug', label: 'debug —— 全推（最吵）' },
]

/** 数字串转 JSON 数字；不是整数就原样保留（例如 chat_id 写 `@channelname`）。 */
const toJsonScalar = (raw: string): string | number => {
  const t = raw.trim()
  return /^-?\d+$/.test(t) && Number.isSafeInteger(Number(t)) ? Number(t) : t
}

// ── 冻结接口 ────────────────────────────────────────────────────────────

/**
 * 一份 `notify` 里**配了哪些渠道**（中文名）—— 给别处做摘要用。
 *
 * 例：配置中心的「任务配置 → 账号私有配置」那半要一眼看出"每个账号的推送不一样"
 * （2026-10-05 用户口径：「比如每个账号的推送就不一样」），就是靠它。
 *
 * ⚠ 渠道清单（`NOTIFY_CHANNELS`）与中文名（`CHANNEL_LABEL`）**只有这一份** ——
 * 在页面里自己再列一遍"有哪些渠道"就是第二份表，迟早与这里漂 ✗。
 */
export const notifyChannelLabels = (n: PushConfig | null | undefined): string[] =>
  n ? NOTIFY_CHANNELS.filter((k) => n[k] != null).map((k) => CHANNEL_LABEL[k]) : []

export interface PushEditorSnapshot {
  on: boolean
  title: string
  onlyError: boolean
  minLevel: PushLogLevel
  /** 渠道配置对象（原样持有，含我们没建模的键）。 */
  configs: Record<string, unknown>
  /** 有哪些渠道（结构）。 */
  channels: NotifyChannelKey[]
  /** customPost 的模板体文本（每行对应一条模板）。 */
  templateBodies: string[]
}

export interface PushEditor {
  /** 把 GET 回来的一份 notify 铺进表单（`null` = 这个账号不推）。 */
  load(n: PushConfig | null): void
  /** 攒成 PUT 里的 `notify`：总开关关着 ⇒ `null`；开着但一个渠道都没配 ⇒ 抛 Error（文案照抄原代码那条）。 */
  build(): PushConfig | null
  /** 「运行结果推送」**整块** DOM：启用推送开关 + 标题 + onlyError + 最低级别 + 渠道列表 + 空态文案。 */
  fields(): JSX.Element
  /** 总开关当前是否打开。 */
  enabled(): boolean
  /** 打开/关掉总开关（给想自己画开关的调用方；`fields()` 内部也有一个）。 */
  setEnabled(v: boolean): void
  /** 草稿快照 / 还原（调用方切账号时存/取；**不包含**明文显隐状态，还原后一律重新打码）。 */
  snapshot(): PushEditorSnapshot
  restore(s: PushEditorSnapshot): void
}

/**
 * 造一个推送编辑器。
 *
 * ⚠ 必须在**组件体里**调（`const push = createPushEditor()`）—— 内部的 ref 要靠
 * lite 在组件求值时登记；在模块顶层调只会在页面加载时建一份、所有账号共用。
 */
export const createPushEditor = (): PushEditor => {
  // 全局字段。数字一律以**字符串**存：原样回显用户敲的东西，解析放到提交那一刻。
  const fNotifyOn = ref(false)
  const fNotifyTitle = ref(DEFAULT_NOTIFY_TITLE)
  const fOnlyError = ref(false)
  const fMinLevel = ref<PushLogLevel>('info')
  /** 渠道 → 原始配置对象（GET 回来的那份，见文件头"留着原始对象"）。 */
  const settingsNotify = ref<Record<string, unknown>>({})
  /** 当前显示哪几张渠道卡片。结构与值分开存，理由见文件头。 */
  const settingsChannels = ref<NotifyChannelKey[]>([])
  /** `customPost` 每条模板的请求体（JSON 文本），下标与模板一一对应。 */
  const templateBodies = ref<string[]>([])
  /** 打码字段的「显示」开关，按字段 id 记。 */
  const shownSecrets = ref<Record<string, boolean>>({})

  /** 取一个渠道的原始配置对象（`customPost` 是数组，单独处理）。 */
  const channelCfg = (key: NotifyChannelKey): Record<string, unknown> | null => {
    const v = settingsNotify.value[key]
    return v != null && typeof v === 'object' && !Array.isArray(v)
      ? (v as Record<string, unknown>)
      : null
  }

  /** 读渠道配置里的一个字段，按**文本**给输入框（数字也照原样）。 */
  const cfgText = (key: NotifyChannelKey, field: string): string => {
    const v = channelCfg(key)?.[field]
    return v == null ? '' : String(v)
  }

  /**
   * 改渠道配置里的一个字段。
   *
   * 空串 ⇒ **删掉这一格**：`""` 会被后端当成"填了但为空"（`require_non_empty` ⇒ 400），
   * 而用户的意思只是"这一格不填"。删掉它，可选字段才是真的缺席。
   */
  const setChannelField = (key: NotifyChannelKey, field: string, value: string): void => {
    const next: Record<string, unknown> = { ...(channelCfg(key) ?? {}) }
    if (value.trim() === '') delete next[field]
    else next[field] = value
    settingsNotify.value = { ...settingsNotify.value, [key]: next }
  }

  /** 回逍那两格在 `query` 里；没有别的键时把空的 `query` 删掉，别留一个空对象。 */
  const setTwoImQuery = (field: string, value: string): void => {
    const cfg: Record<string, unknown> = { ...(channelCfg('twoIm') ?? {}) }
    const q: Record<string, unknown> = { ...((cfg.query as Record<string, unknown>) ?? {}) }
    if (value.trim() === '') delete q[field]
    else q[field] = value
    if (Object.keys(q).length === 0) delete cfg.query
    else cfg.query = q
    settingsNotify.value = { ...settingsNotify.value, twoIm: cfg }
  }

  const twoImQueryText = (field: string): string => {
    const q = channelCfg('twoIm')?.query as Record<string, unknown> | undefined
    const v = q?.[field]
    return v == null ? '' : String(v)
  }

  /** `customPost` 允许单个模板或数组；界面统一按数组处理。 */
  const templateList = (): Record<string, unknown>[] => {
    const v = settingsNotify.value.customPost
    if (v == null) return []
    return Array.isArray(v) ? (v as Record<string, unknown>[]) : [v as Record<string, unknown>]
  }

  /**
   * 按**下标**迭代模板。
   *
   * ⚠ 列表项刻意是**数字**而不是模板对象：`createFor` 判"要不要重建这一行"用的是
   * 对象身份。改一个字段就是一次 `{...tpl}`，新对象 ⇒ 整行重建 ⇒ 输入框丢焦点。
   * 数字按下标比是同一份值，行被复用，只有字段绑定重跑。
   */
  const templateIndexes = (): number[] => templateList().map((_tpl, i) => i)

  const templateText = (index: number, field: string): string => {
    const v = templateList()[index]?.[field]
    return v == null ? '' : String(v)
  }

  const setTemplateField = (index: number, field: string, value: string): void => {
    const items = templateList().map((tpl, i) => {
      if (i !== index) return tpl
      const next: Record<string, unknown> = { ...tpl }
      if (value.trim() === '') delete next[field]
      else next[field] = value
      return next
    })
    settingsNotify.value = { ...settingsNotify.value, customPost: items }
  }

  /**
   * 改一条模板的请求体文本。
   *
   * ⚠ 刻意**不**写成 `templateBodies.value.map(...)`：两个数组（模板与请求体）
   * 一旦长度不一致，`map` 到不了那个下标 ⇒ 赋值出来还是"没有这一格" ⇒ 紧接着
   * 那条 `value={templateBodies.value[i] ?? ''}` 的绑定把用户刚敲的字**回写成空串**。
   * 实测踩到过一次（`addChannel` 漏了配对），症状是"打字打不进去"。
   * 这里先补齐长度，让"数组短了"不可能变成"输入框自己变空"。
   */
  const setTemplateBody = (index: number, text: string): void => {
    const next = [...templateBodies.value]
    while (next.length <= index) next.push('')
    next[index] = text
    templateBodies.value = next
  }

  /** 模板体在界面上是一段 JSON 文本；`data` 与 `body` 在后端是 `data || body`。 */
  const bodyTextOf = (tpl: Record<string, unknown>): string => {
    const v = tpl.data ?? tpl.body
    return v == null ? '' : JSON.stringify(v, null, 2)
  }

  const addTemplate = (): void => {
    const list = templateList()
    // 请求体数组与模板数组**必须等长**（见 `setTemplateBody` 的 ⚠）：先按模板对齐一份，
    // 再一起加一条，否则新加的那条从一出生就没有自己的请求体格子。
    const bodies = list.map((_tpl, i) => templateBodies.value[i] ?? '')
    settingsNotify.value = { ...settingsNotify.value, customPost: [...list, {}] }
    templateBodies.value = [...bodies, '']
  }

  const removeTemplate = (index: number): void => {
    settingsNotify.value = {
      ...settingsNotify.value,
      customPost: templateList().filter((_tpl, i) => i !== index),
    }
    templateBodies.value = templateBodies.value.filter((_t, i) => i !== index)
  }

  const addChannel = (key: NotifyChannelKey): void => {
    if (!settingsChannels.value.includes(key)) {
      settingsChannels.value = [...settingsChannels.value, key]
    }
    if (settingsNotify.value[key] == null) {
      settingsNotify.value = { ...settingsNotify.value, [key]: {} }
    }
    // `customPost` 的空数组会被后端判成"一个模板都没有"（⇒ 400）。新加的直接给一条
    // 空模板 —— 用户看到的是"该填 URL 了"，而不是"配了却被拒绝"。
    //
    // ⚠ 两个数组（模板 / 请求体文本）**必须同时就位**。上面那两行会把
    // `customPost` 设成 `{}`，而 `templateList()` 把它当**单模板**包成 `[{}]` ——
    // 所以"长度是 0 才补请求体"那个写法永远进不去，请求体数组留在空表上，
    // 结果就是"URL 打得进去、模板体那边一敲字就被回写成空串"。实测踩到过。
    if (key === 'customPost') {
      const list = templateList()
      if (list.length === 0) {
        settingsNotify.value = { ...settingsNotify.value, customPost: [{}] }
        templateBodies.value = ['']
      } else {
        templateBodies.value = list.map((_tpl, i) => templateBodies.value[i] ?? '')
      }
    }
  }

  const removeChannel = (key: NotifyChannelKey): void => {
    settingsChannels.value = settingsChannels.value.filter((k) => k !== key)
    const next = { ...settingsNotify.value }
    delete next[key]
    settingsNotify.value = next
    if (key === 'customPost') templateBodies.value = []
  }

  const availableChannels = (): NotifyChannelKey[] =>
    NOTIFY_CHANNELS.filter((k) => !settingsChannels.value.includes(k))

  /** 卡片标题旁那枚徽章：只看**必填字段填没填**（与提交时"算不算配了"同一条判据）。 */
  const channelState = (key: NotifyChannelKey): string => {
    if (key === 'customPost') {
      const n = templateList().filter((t) => String(t.url ?? '').trim() !== '').length
      return n === 0 ? '未填' : String(n) + ' 条模板'
    }
    const cfg = channelCfg(key)
    if (!cfg) return '未填'
    const missing = CHANNEL_REQUIRED[key].filter((f) => String(cfg[f] ?? '').trim() === '')
    return missing.length === 0 ? '已填好' : '缺 ' + missing.join('、')
  }

  const secretShown = (id: string): boolean => shownSecrets.value[id] === true

  const toggleSecret = (id: string): void => {
    shownSecrets.value = { ...shownSecrets.value, [id]: !secretShown(id) }
  }

  // ── 渲染原语 ────────────────────────────────────────────────────────
  //
  // ⚠ 这三份是账号页那三份的**私有副本**（账号页别处还在用文本 / 开关字段，
  // 这里不能去 import 它 —— 那会把两个页面的渲染层绑在一起）。
  // ⚠ 值一律走 thunk（`get: () => …`），理由见文件头。

  /** 一行普通输入框。 */
  const textField = (
    label: string,
    get: () => string,
    set: (v: string) => void,
    hint: string,
    type: 'text' | 'number' = 'text',
  ) => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">{label}</legend>
      <input
        type={type}
        class="input w-full"
        autocomplete="off"
        spellcheck={false}
        value={get()}
        onInput={(e) => set((e.target as HTMLInputElement).value)}
      />
      <p class="text-xs leading-5 opacity-60">{hint}</p>
    </fieldset>
  )

  /**
   * 一行**打码**输入框（token / secret / 带密钥的 webhook URL）。
   *
   * 默认 `type=password`，右边那颗「显示」按字段 id 记住是否明文 —— 打码防的是
   * **站在屏幕边上的人**，不是自己，所以查看必须一键可达。
   *
   * ⚠ 这些值**不进 console**：本文件里没有任何 `console.*`，报错也只带字段名
   * （后端 `validate` 就是这么约束的：错误信息里不许出现字段值）。
   */
  const secretField = (
    id: string,
    label: string,
    get: () => string,
    set: (v: string) => void,
    hint: string,
  ) => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">{label}</legend>
      <div class="join w-full">
        <input
          type={secretShown(id) ? 'text' : 'password'}
          class="input join-item w-full font-mono text-xs"
          autocomplete="off"
          spellcheck={false}
          value={get()}
          onInput={(e) => set((e.target as HTMLInputElement).value)}
        />
        <button
          type="button"
          class="btn join-item btn-soft"
          aria-pressed={secretShown(id)}
          onClick={() => toggleSecret(id)}
        >
          {secretShown(id) ? '隐藏' : '显示'}
        </button>
      </div>
      <p class="text-xs leading-5 opacity-60">{hint}</p>
    </fieldset>
  )

  /** 一个开关（daisyUI `toggle`）。 */
  const switchField = (
    label: string,
    get: () => boolean,
    set: (v: boolean) => void,
    hint: string,
  ) => (
    <label class="flex cursor-pointer flex-col gap-1">
      <span class="flex min-h-11 items-center gap-3 md:min-h-0">
        <input
          type="checkbox"
          class="toggle toggle-primary"
          checked={get()}
          onChange={(e) => set((e.target as HTMLInputElement).checked)}
        />
        <span class="text-sm">{label}</span>
      </span>
      <span class="text-xs leading-5 opacity-60">{hint}</span>
    </label>
  )

  /**
   * 一个渠道的具体字段。
   *
   * ⚠ 分支**只看 `key`**（普通字符串），一个 ref 都不当场读 —— 所以这块结构建一次
   * 就定了，改字段不会重建它（见文件头"值一律走 thunk"）。
   */
  const channelFields = (key: NotifyChannelKey) => {
    const get = (field: string) => () => cfgText(key, field)
    const set = (field: string) => (v: string) => setChannelField(key, field, v)

    if (key === 'pushplus' || key === 'serverChan') {
      return secretField(
        key + '.token',
        'Token',
        get('token'),
        set('token'),
        '在对应服务里生成的推送 token。留空 = 这个渠道不发。',
      )
    }
    if (key === 'workWeixinBot') {
      return (
        <>
          {secretField(
            'workWeixinBot.url',
            'Webhook URL',
            get('url'),
            set('url'),
            '群设置 → 群机器人 → 添加 → 复制 Webhook 地址。⚠ 这个 URL 本身就是密钥：拿到它就能往群里发消息。',
          )}
          {textField('消息类型 msgtype（可留空）', get('msgtype'), set('msgtype'), '留空 = 后端按 text 发。')}
        </>
      )
    }
    if (key === 'dingTalk') {
      return (
        <>
          {secretField(
            'dingTalk.token',
            'Access Token',
            get('token'),
            set('token'),
            'Webhook 地址里 access_token= 后面那一段。',
          )}
          {secretField(
            'dingTalk.secret',
            '加签密钥（可留空）',
            get('secret'),
            set('secret'),
            '只有机器人安全设置里选了「加签」才需要。',
          )}
        </>
      )
    }
    if (key === 'customPost') {
      return (
        <div class="flex flex-col gap-2">
          {templateIndexes().map((i) => (
            <div key={i} class="flex flex-col gap-2 rounded-box border border-base-300 p-3">
              {secretField(
                'customPost.' + i + '.url',
                '第 ' + (i + 1) + ' 条 URL',
                () => templateText(i, 'url'),
                (v) => setTemplateField(i, 'url', v),
                '支持 {title} / {text} 占位符。⚠ URL 里带密钥时要按密钥对待。',
              )}
              {textField(
                '第 ' + (i + 1) + ' 条方法（可留空）',
                () => templateText(i, 'method'),
                (v) => setTemplateField(i, 'method', v),
                '留空 = POST；后端会把它转成大写。GET 也允许。',
              )}
              <fieldset class="fieldset">
                <legend class="fieldset-legend">{'第 ' + (i + 1) + ' 条请求体模板（JSON，可留空）'}</legend>
                <textarea
                  class="textarea w-full font-mono text-xs"
                  rows={3}
                  value={templateBodies.value[i] ?? ''}
                  onInput={(e) => setTemplateBody(i, (e.target as HTMLTextAreaElement).value)}
                />
                <p class="text-xs leading-5 opacity-60">
                  必须是 JSON 对象。里面的 {'{title}'} / {'{text}'} 会被替换（不转义）。
                  留空 = 不发模板体。⚠ headers / timeout 等本版没有列出来的字段会原样保留。
                </p>
              </fieldset>
              <button
                type="button"
                class="btn btn-sm btn-soft min-h-11 self-start md:min-h-0"
                onClick={() => removeTemplate(i)}
              >
                删掉这条
              </button>
            </div>
          ))}
          <button
            type="button"
            class="btn btn-sm btn-soft self-start min-h-11 md:min-h-0"
            onClick={addTemplate}
          >
            再加一条模板
          </button>
        </div>
      )
    }
    if (key === 'workWeixin') {
      return (
        <>
          {textField(
            'corpid（企业 ID）',
            get('corpid'),
            set('corpid'),
            '企业微信后台 → 我的企业 → 企业 ID。',
          )}
          {secretField(
            'workWeixin.corpsecret',
            'corpsecret（应用密钥）',
            get('corpsecret'),
            set('corpsecret'),
            '企业微信后台 → 应用管理 → 你的应用 → Secret。',
          )}
          {textField(
            'agentid（应用 ID）',
            get('agentid'),
            set('agentid'),
            '必须是数字；缺了发送侧会整条跳过。',
            'number',
          )}
          {textField('touser（可留空）', get('touser'), set('touser'), '收件人，留空 = 后端默认 @all。')}
        </>
      )
    }
    if (key === 'tgBot') {
      return (
        <>
          {secretField('tgBot.token', 'Bot Token', get('token'), set('token'), '@BotFather 给的 token。')}
          {textField(
            'chat_id',
            get('chat_id'),
            set('chat_id'),
            '收件会话：私聊是数字，群是负数，也可以写 @channelname。',
          )}
          {textField(
            'apiHost（可留空）',
            get('apiHost'),
            set('apiHost'),
            '自建反代时才填；留空 = 官方 api.telegram.org。',
          )}
          {textField('proxy（可留空）', get('proxy'), set('proxy'), '形如 http://127.0.0.1:7890。')}
        </>
      )
    }
    if (key === 'bark') {
      return (
        <>
          {secretField('bark.key', 'Key', get('key'), set('key'), 'Bark App 首页那串 key。')}
          {textField(
            'level（可留空）',
            get('level'),
            set('level'),
            '常见取值 active / timeSensitive / passive / critical；留空 = 后端默认。',
          )}
        </>
      )
    }
    if (key === 'twoIm') {
      return (
        <>
          {secretField('twoIm.key', 'Key', get('key'), set('key'), '回逍后台的 key。')}
          {secretField('twoIm.sid', 'Sid', get('sid'), set('sid'), '回逍后台的 sid。')}
          {textField('msgtype（可留空）', get('msgtype'), set('msgtype'), '留空 = 后端默认。')}
          {textField(
            '分组 group（可留空）',
            () => twoImQueryText('group'),
            (v) => setTwoImQuery('group', v),
            '回逍的群组标识；留空 = 不按分组发。',
          )}
        </>
      )
    }
    if (key === 'email') {
      return (
        <>
          {textField('SMTP 主机', get('host'), set('host'), '例如 smtp.qq.com。')}
          {textField('端口（可留空）', get('port'), set('port'), '必须是数字；留空 = 后端默认。', 'number')}
          {textField('发件人 from', get('from'), set('from'), '例如 you@qq.com。')}
          {secretField(
            'email.pass',
            'SMTP 口令 / 授权码',
            get('pass'),
            set('pass'),
            '多数邮箱要的是「授权码」，不是登录口令。',
          )}
          {textField('收件人 to（可留空）', get('to'), set('to'), '留空 = 发给 from 自己。')}
        </>
      )
    }
    // 十个渠道都在上面的表里。走到这里说明加渠道时漏了一档 ——
    // 宁可把话说出来，也别静默渲染成一块空白。
    return <p class="text-xs text-error">这个渠道还没有表单（代码里漏了一档）。</p>
  }

  /** 推送那一块（总开关之外的全部内容）。 */
  const notifyFields = () => (
    <>
      {textField(
        '推送标题',
        () => fNotifyTitle.value,
        (v) => (fNotifyTitle.value = v),
        '留空 = 用默认「mcloud 运行推送」。',
      )}
      {switchField(
        '只在出错时推',
        () => fOnlyError.value,
        (v) => (fOnlyError.value = v),
        '开了之后只有 error 类型的日志会推 —— fail / fatal 不算（后端判的就是类型恰好等于 error）。',
      )}

      <fieldset class="fieldset">
        <legend class="fieldset-legend">最低日志级别</legend>
        <Select
          id="push-min-level"
          class="w-full"
          ariaLabel="最低日志级别"
          value={fMinLevel.value}
          options={MIN_LEVELS.map((lv) => ({ value: lv.value, label: lv.label }))}
          onChange={(v) => (fMinLevel.value = v as PushLogLevel)}
        />
        <p class="text-xs leading-5 opacity-60">级别越小越严重：error 只留错误，debug 全留。</p>
      </fieldset>

      <div class="flex flex-col gap-2">
        <span class="text-xs font-medium opacity-70">渠道（点开一个才能改它）</span>
        <div class="flex flex-col gap-2">
          {settingsChannels.value.map((key) => (
            <details key={key} class="disclosure disclosure-arrow bg-base-200">
              <summary class="disclosure-title text-sm font-medium">
                {CHANNEL_LABEL[key]}
                <span class="badge badge-soft badge-sm ml-2">{channelState(key)}</span>
              </summary>
              <div class="disclosure-content flex flex-col gap-2">
                <p class="text-xs leading-5 opacity-60">{CHANNEL_HINT[key]}</p>
                {channelFields(key)}
                <button
                  type="button"
                  class="btn btn-sm btn-soft btn-error min-h-11 self-start md:min-h-0"
                  onClick={() => removeChannel(key)}
                >
                  删除这个渠道
                </button>
              </div>
            </details>
          ))}
        </div>
        {availableChannels().length > 0 ? (
          <div class="flex flex-wrap items-center gap-1">
            <span class="text-xs opacity-70">+ 加渠道：</span>
            {availableChannels().map((key) => (
              <button
                key={key}
                type="button"
                class="btn btn-xs btn-soft min-h-9"
                onClick={() => addChannel(key)}
              >
                {CHANNEL_LABEL[key]}
              </button>
            ))}
          </div>
        ) : (
          <p class="text-xs opacity-60">十个渠道都已经在列表里了。</p>
        )}
      </div>
    </>
  )

  /**
   * 「运行结果推送」**整块**（含"启用推送"总开关与关着时的空态）。
   *
   * ⚠ 外层那个 `.flex.flex-col.gap-2` 与 `<h4>运行结果推送</h4>` 归调用方
   * （标题的层级是页面的排版决定，不是这块表单的），这里只出内容。
   */
  const fields = (): JSX.Element => (
    <>
      {switchField(
        '启用推送',
        () => fNotifyOn.value,
        (v) => (fNotifyOn.value = v),
        '关掉 = 这个账号一个渠道都不推（保存时 notify 写 null，会清掉下面的渠道配置）。本仓没有进程级兜底，每个账号各配各的。',
      )}
      {fNotifyOn.value ? (
        notifyFields()
      ) : (
        <p class="text-xs leading-5 opacity-60">
          当前不推送任何渠道。打开上面的开关才会出现渠道配置。
        </p>
      )}
    </>
  )

  /**
   * 把 GET 回来的一份 notify 铺进表单（`null` = 这个账号不推）。
   *
   * 渠道配置**原样拿住**（里面可能有我们没建模的键），另挑一份"有哪些渠道"给结构性 ref。
   */
  const load = (n: PushConfig | null): void => {
    fNotifyOn.value = n != null
    fNotifyTitle.value = n?.title ?? DEFAULT_NOTIFY_TITLE
    fOnlyError.value = n?.onlyError ?? false
    fMinLevel.value = n?.minLevel ?? 'info'

    const raw = (n ?? {}) as Record<string, unknown>
    const next: Record<string, unknown> = {}
    const keys: NotifyChannelKey[] = []
    for (const key of NOTIFY_CHANNELS) {
      const v = raw[key]
      if (v == null) continue
      next[key] = v
      keys.push(key)
    }
    settingsNotify.value = next
    settingsChannels.value = keys
    templateBodies.value = templateList().map(bodyTextOf)
    // 换一份数据 ⇒ 打码状态一律复位：上一份"显示着"的 token 不该在新数据上继续裸着。
    shownSecrets.value = {}
  }

  /**
   * 把一条渠道配置整理成要发出去的形状；返回 `null` = 这个渠道**没配**、整条不发。
   *
   * 三条规矩：
   * - 必填字段**全空** ⇒ `null`（不发空对象）；
   * - 数字字段转 JSON 数字（见 `CHANNEL_NUMERIC`）；
   * - `customPost` 的模板体在这里解析 JSON，解不开就**指名是第几条**。
   */
  const buildChannel = (key: NotifyChannelKey): unknown => {
    if (key === 'customPost') {
      const items = templateList()
        .map((tpl, i) => {
          const next: Record<string, unknown> = { ...tpl }
          const rawBody = (templateBodies.value[i] ?? '').trim()
          delete next.data
          delete next.body
          if (rawBody !== '') {
            let parsed: unknown
            try {
              parsed = JSON.parse(rawBody)
            } catch {
              throw new Error('自定义 POST 第 ' + (i + 1) + ' 条的模板体不是合法 JSON')
            }
            if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
              throw new Error('自定义 POST 第 ' + (i + 1) + ' 条的模板体必须是一个 JSON 对象')
            }
            next.data = parsed as Record<string, unknown>
          }
          return next
        })
        .filter((t) => String(t.url ?? '').trim() !== '')
      return items.length > 0 ? items : null
    }

    const cfg = channelCfg(key)
    if (!cfg) return null
    const next: Record<string, unknown> = { ...cfg }
    for (const field of CHANNEL_NUMERIC[key] ?? []) {
      const cur = next[field]
      if (cur == null || String(cur).trim() === '') delete next[field]
      else next[field] = toJsonScalar(String(cur))
    }
    if (CHANNEL_REQUIRED[key].every((f) => String(next[f] ?? '').trim() === '')) return null
    return next
  }

  /** 攒成 PUT 里的 `notify`（总开关关着 ⇒ `null`）。 */
  const build = (): PushConfig | null => {
    if (!fNotifyOn.value) return null
    const out: Record<string, unknown> = {
      title: fNotifyTitle.value.trim() === '' ? DEFAULT_NOTIFY_TITLE : fNotifyTitle.value.trim(),
      onlyError: fOnlyError.value,
      minLevel: fMinLevel.value,
    }
    let count = 0
    for (const key of settingsChannels.value) {
      const built = buildChannel(key)
      if (built == null) continue
      out[key] = built
      count++
    }
    if (count === 0) {
      // 后端对这份形状的原话是「notify 里没有任何渠道（全为 null）」⇒ 400。
      // 与其打一枪再回错误，不如在这里说同一件事（判据与后端一致，只是省一次往返）。
      throw new Error(
        '推送总开关开着，但一个渠道都没配。要么至少配一个，要么关掉总开关（关掉 = 这个账号不推送）。',
      )
    }
    return out as unknown as PushConfig
  }

  const enabled = (): boolean => fNotifyOn.value

  const setEnabled = (v: boolean): void => {
    fNotifyOn.value = v
  }

  /**
   * 草稿快照。
   *
   * ⚠ `configs` / `channels` / `templateBodies` 都只做**一层拷贝**：本模块改配置时
   * 一律"造新对象再赋值"（`{...cfg}` / `{...settingsNotify}`），从不原地改，
   * 所以旧快照里的内层对象不会被后来的编辑动到。改成深拷贝只是白花时间。
   * ⚠ **不含** `shownSecrets`：明文显隐是瞬时的界面状态，不是草稿的一部分。
   */
  const snapshot = (): PushEditorSnapshot => ({
    on: fNotifyOn.value,
    title: fNotifyTitle.value,
    onlyError: fOnlyError.value,
    minLevel: fMinLevel.value,
    configs: { ...settingsNotify.value },
    channels: [...settingsChannels.value],
    templateBodies: [...templateBodies.value],
  })

  /** 还原一份草稿（见 `snapshot` 的 ⚠）。还原后一律重新打码。 */
  const restore = (s: PushEditorSnapshot): void => {
    fNotifyOn.value = s.on
    fNotifyTitle.value = s.title
    fOnlyError.value = s.onlyError
    fMinLevel.value = s.minLevel
    settingsNotify.value = { ...s.configs }
    settingsChannels.value = [...s.channels]
    templateBodies.value = [...s.templateBodies]
    shownSecrets.value = {}
  }

  return { load, build, fields, enabled, setEnabled, snapshot, restore }
}
