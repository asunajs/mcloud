import { listAccounts, type AccountSummary } from './api'
import { ref } from '@asunajs/lite'

/**
 * 账号 id → 给人看的名字。
 *
 * ⚠ **为什么要有这一层。**
 *
 * 账号主键是手机号的 SHA-256 前 16 位（`0f6050105ab111f4`）——「只当键用」。
 * 可运行记录、事件流、下拉框里带的都是**主键**，直接摆到界面上用户认不出是哪个号。
 * 每个页面各自去拉一次账号列表来映射，既重复又容易漏掉一处，
 * 于是收成这一个缓存。
 *
 * ⚠ 缓存**不失效**也没关系：账号被移除时后端那条记录已经不在了，
 * 而"移除后还显示旧名字"比"显示一串哈希"好。真要准就在移除后调
 * [`refreshAccountNames`]。
 *
 * ⚠⚠ **必须是信号（`ref`），不能是裸 Map。** 各页面普遍写成
 * `void refreshAccountNames(); void load()` —— 两个请求并发，账号列表常常**后到**。
 * 裸 Map 只改容器、不通知任何人 ⇒ 那一次渲染已经结束，表格会一直停在哈希上，
 * 直到用户手动刷新（症状："名单里全是 16 位哈希，切个页又好了"）。
 */
const names = ref<Map<string, string>>(new Map())

/** 名字优先级与账号页一致：**昵称 → 掩码手机号 → 主键**。 */
function pick(a: { nickname: string | null; label: string; id: string }): string {
  return a.nickname || a.label || a.id
}

/**
 * 把**已经拿到**的一份账号列表填进缓存。
 *
 * 给"同一份列表还有别的用途"的页面用（总览要拿它算凭据到期那几张指标卡）——
 * 否则它得为同一个 `/api/accounts` 再发一次请求，而两次结果之间还可能不一致
 * （一次成一次败 ⇒ 指标卡说 3 个账号、列表里的名字却是哈希）。
 *
 * 解析口径与 [`refreshAccountNames`] **共用** `pick`，不是第二份实现。
 */
export function rememberAccountNames(list: AccountSummary[]): void {
  // ⚠ 整张新 Map 赋值（而不是 `names.value.set(...)`）：Map 是内建对象，
  // 原地 `set` 不会让信号发现变化 —— 那就是上面说的"名字不出现"的根因。
  const next = new Map(names.value)
  for (const a of list) next.set(a.id, pick(a))
  names.value = next
}

/** 拉一次账号列表填缓存。拉不动就保持原样（界面退化成显示主键，而不是空白）。 */
export async function refreshAccountNames(): Promise<void> {
  try {
    // ⚠ 显式要**全部**（含停用）：这份缓存是给"显示名字"用的 —— 历史里那条记录
    // 不会因为账号被停用就变得认不出来。它**不是**挑账号的下拉数据源，
    // 所以在这里放全部不会让停用账号漏回下拉（各处下拉各自调 listAccounts()）。
    rememberAccountNames(await listAccounts({ includeDisabled: true }))
  } catch {
    // 静默：这只是个显示名字，失败不该让页面报错
  }
}

/** 查名字。**查不到就原样返回 id** —— 绝不返回空串（那会让界面一片空白）。 */
export function accountName(id: string): string {
  return names.value.get(id) || id
}

/**
 * 名字缓存的**身份**（每次更新都换一张新 Map ✓，见 [`rememberAccountNames`]）。
 *
 * 给"把查出来的名字再算成别的东西、想缓存一层"的页面当缓存键用 ✓：
 * 读它就是**订阅名字变化** ✓ —— 只按数据身份缓存（比如服务端日志页按 `lines` 缓存）
 * 的话，账号列表后到时那份缓存不会重算，界面就一直停在哈希上 ✗
 * （症状与模块头那条"名单里全是 16 位哈希，切个页又好了"是同一个 ✓）。
 */
export const accountNames = names
