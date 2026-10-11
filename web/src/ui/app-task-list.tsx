/**
 * App 任务那张表的**可视化**（「上游状态」页的第一格 ✓）。
 *
 * # 为什么单独抽一个组件
 *
 * 那张表是 16 次拉取、去重后 25~29 条，每条带
 * `taskId / name / groupId / state / currStep / process / marketName / buttonAppLink`。
 * 在这之前它只在运行详情里以 JSON 形态躺着 —— 用户 2026-10-07 原话
 * 「**实现一个页面可视化 appTaskList**」✓。
 *
 * ⚠ 数据**不再**来自运行明细：用户同一天又定了口径「这个表单独放一个专门查看上游各种
 * 状态的页面，**需要主动获取**，而不是跟着其他任务一起」✓ ⇒ 现在由
 * `GET /api/upstream/app-task-list` 现取一份（类型见 `api.ts` 的
 * [`UpstreamAppTaskList`] ✓），**不落运行历史**。
 *
 * ⚠ 名字里**带 HTML**（上游就这么给的，见 `mcloud-tasks::clean_task_name` 的说明）：
 * `分享文件领好礼<span id='share_title'>2/7</span>` ⇒ 显示前必须剥掉，
 * 否则页面上会出现一坨标签字面量 ✗。
 *
 * ⚠ 分组的**中文名照抄旧 TS**（`tasks/app/results.ts` 的 `GROUP_NAMES` ✓）——
 * 别自己另起一套名字：同一份数据在两处叫法不同，对账时就得多问一句"这是不是同一个组" ✗。
 */

import { ref } from '@asunajs/lite'

import type { UpstreamAppTaskItem, UpstreamAppTaskList } from '../api'
import { IconChevronDown } from './icons'

/** 分组的中文名（照抄旧 TS `results.ts:GROUP_NAMES` ✓）。 */
const GROUP_NAMES: Record<string, string> = {
  day: '每日任务',
  month: '每月任务',
  new: '新用户任务',
  time: '热门任务',
  hiddenabc: '隐藏任务',
  hidden: '隐藏任务',
  beiyong1: '临时任务',
  cloudEmail: '云邮箱任务',
}

/** 剥掉上游名字里的 HTML 标签（顺带把空白收一下 ✓）。 */
const plainName = (name: string): string => name.replace(/<[^>]*>/g, '').trim()

/**
 * 状态 → 徽章（`FINISH` 是**唯一**的"完成"✓，别把 `WAIT` 也画成绿的 ✗）。
 *
 * ⚠⚠ `WAIT` **不写死**「待做」✗（2026-10-11 用户口径：「**与原接口文案一致**」）：
 * 上游那一格 `buttonProperty.out.text` 是**逐条**的 —— 真机实拍同一趟里 `state=WAIT`
 * 的两条，1029「查看"我的账单"」是 **「去完成」** ✓、而「连续31天开启邮箱APP通知」是
 * **「开启中」** ✓。一刀切写「待做」会把"上游已经给了具体动作"这件事盖掉 ✗。
 * ⇒ 有就**照抄** ✓；读不出才回落「**未完成**」✓（与档位按钮上那个词一致，不再另造一个 ✗）。
 */
const stateView = (i: UpstreamAppTaskItem): { label: string; cls: string } => {
  const state = i.state ?? ''
  // 上游那格是**三档**文案（`text` / `textMiddle` / `textFinish` ✓，2026-10-11 真机实拍）：
  //
  // | 任务 | text（未开始） | textMiddle（进行中） | textFinish（已完成） |
  // | --- | --- | --- | --- |
  // | 550 开启云盘APP通知领好礼 | 去开启 | 领AI豆 | 已领取 |
  // | 551 连续31天开启云盘APP通知 | 去开启 | **开启中** | 领AI豆 |
  // | 1079 连续31天开启邮箱短信通知 | 去开启 | **开启中** | 领AI豆 |
  //
  // ⇒ 只读 `text` 就会把进行中的条目全显示成「去开启」✗（用户口径：「我在 app 看到的是已开启」✓）。
  // **阶段怎么判**：`actionMiddle` 空/非空 —— 实拍 551/1079 是 `"jump"` ✓、550 是 `""` ✓。
  const pick = (first?: string | null, ...rest: (string | null | undefined)[]): string => {
    for (const v of [first, ...rest]) {
      const t = (v ?? '').trim()
      if (t !== '') return t
    }
    return ''
  }
  const middle = (i.buttonActionMiddle ?? '').trim()
  if (state === 'FINISH') {
    const t = pick(i.buttonTextFinish, i.buttonOutText)
    return { label: t === '' ? '已完成' : t, cls: 'badge-success' }
  }
  if (state === 'WAIT') {
    const t =
      middle === ''
        ? pick(i.buttonOutText, i.buttonTextMiddle)
        : pick(i.buttonTextMiddle, i.buttonOutText)
    return { label: t === '' ? '未完成' : t, cls: 'badge-warning' }
  }
  if (state === 'DOING') return { label: '进行中', cls: 'badge-info' }
  return { label: state === '' ? '未知' : state, cls: 'badge-ghost' }
}

/**
 * 一行小字：`#106 · sign_in_3 · 第 3 步`。
 *
 * ⚠ 拼成一整句再交给 JSX（而不是在子节点位置写一串三元）：
 * lite 编译器对**子节点位置**的三元分支有约束（只认 JSX / null，见
 * `asunajs/lite` 的 `docs/design.md` §8.0），拼句子放在普通函数里就完全不受那条限制 ✓。
 */
const metaLine = (i: UpstreamAppTaskItem): string => {
  const bits: string[] = [`#${i.taskId ?? '—'}`, i.marketName ?? '—', `第 ${i.currStep ?? 0} 步`]
  if (i.process != null && i.process > 0) bits.push(`进度 ${i.process}`)
  // 上游那一格 `button.out.day`（**字符串** ✓）—— 实拍「连续31天开启邮箱APP通知」是 `"5"`，
  // 而它的 `process` 是 **0** ⇒ 只读 `process` 就会"App 里有进度、这里没有" ✗
  //（2026-10-11 用户口径 ✓）。
  // ⚠ 照 App 的样子写（用户截图：**进度条 + 「完成进度 (5/31)」** ✓）：
  //   `5` = 条目里的 `button.*.day` ✓（1021 实拍 `"5"` ✓）；
  //   `31` = 名字里的「连续**31**天」✓（App 那个分母也是这么来的 ✓）。
  //   分母读不出就只写分子 ✓，别编 ✗。
  const day = (i.buttonOutDay ?? '').trim()
  if (day !== '') {
    const total = /连续(\d+)天/.exec(plainName(i.name ?? ''))?.[1] ?? ''
    bits.push(total === '' ? `完成进度 ${day}` : `完成进度 (${day}/${total})`)
  }
  if (i.buttonCanReceive === 1) bits.push('可领取')
  if (i.enable === 0) bits.push('上游已停用')
  if (i.buttonAppLink != null && i.buttonAppLink !== '') bits.push(i.buttonAppLink)
  return bits.join(' · ')
}

type ListFilter = 'all' | 'waiting' | 'done'

/** 一个分组下的若干条。 */
interface Section {
  group: string
  rows: UpstreamAppTaskItem[]
}

export const AppTaskListView = (props: { matrix: UpstreamAppTaskList }) => {
  const filter = ref<ListFilter>('all')

  /**
   * 每一组的开合。
   *
   * ⚠ 必须自己记一份（照 `exchange-page.tsx` 那套 ✓）：`open` 是**属性**，
   * 每次重渲染都会照 `props` 重新写一遍 ⇒ 不记的话"用户折起来的那一组"
   * 会在切换筛选时自己弹开 ✗。
   * ⚠ `onToggle` 里"值没变就直接返回"那道闸不能省：把 `open` 写回 DOM 会**再触发一次
   * `toggle` 事件**，不设闸就是"事件 → 改状态 → 改属性 → 事件"的回环。
   */
  const groupOpen = ref<Record<string, boolean>>({})
  const openOf = (g: string): boolean => groupOpen.value[g] ?? true
  const onToggle = (g: string, open: boolean) => {
    if (groupOpen.value[g] === open) return
    groupOpen.value = { ...groupOpen.value, [g]: open }
  }

  /** 当前档位下的条目。 */
  const items = (): UpstreamAppTaskItem[] => {
    const all = props.matrix.items ?? []
    if (filter.value === 'all') return all
    const wantDone = filter.value === 'done'
    return all.filter((i) => (i.state === 'FINISH') === wantDone)
  }

  /**
   * 按 `groupId` 分组。
   *
   * ⚠ 组的顺序 = 条目在表里**第一次出现**的先后（上游那一趟给什么顺序就是什么顺序 ✓）——
   * 不另排序：这份表的用处之一正是"上游是怎么摆的" ✓。
   */
  const sections = (): Section[] => {
    const out: Section[] = []
    for (const it of items()) {
      const g = it.groupId ?? ''
      const hit = out.find((s) => s.group === g)
      if (hit) hit.rows.push(it)
      else out.push({ group: g, rows: [it] })
    }
    return out
  }

  const counts = (): { total: number; waiting: number; done: number } => {
    const all = props.matrix.items ?? []
    let waiting = 0
    let done = 0
    for (const i of all) {
      if (i.state === 'FINISH') done += 1
      else waiting += 1
    }
    return { total: all.length, waiting, done }
  }

  return (
    <div class="flex flex-col gap-3">
      {/*
        ⚠ 这一行**只说档位说不了的事**（2026-10-07 用户：「**不应出重显示**」✗）。
        以前这里把 `去重 N 条 / 未完成 A / 已完成 B` 也念了一遍，而紧接着的档位按钮上
        就是 `全部 N / 未完成 A / 已完成 B` ⇒ 同一批数字**连着出现两遍** ✗。
        现在只留两样：
          * **拉到 16/16 次** —— 这是"这份清单有多可信"（少于 16 说明少了几组，
            而那几组里的条目**根本不会出现**，跟"表里没有"长得一样 ✗）档位说不了 ✓；
          * **上游原始 N 条** —— 去重前有多少（档位上的 `全部 N` 是**去重后**的 ✓）。
        计数留给档位（本仓既有口径：不用点进去就知道哪一档有东西 ✓）。
      */}
      <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs opacity-70">
        <span>
          拉到 {props.matrix.succeeded ?? 0}/{props.matrix.fetches ?? 0} 次
        </span>
        <span>上游原始 {props.matrix.rawCount ?? 0} 条</span>
      </div>
      <div class="join w-fit">
        <button
          type="button"
          class={`btn btn-sm join-item ${filter.value === 'all' ? 'btn-primary' : 'btn-soft'}`}
          onClick={() => (filter.value = 'all')}
        >
          全部 {counts().total}
        </button>
        <button
          type="button"
          class={`btn btn-sm join-item ${filter.value === 'waiting' ? 'btn-primary' : 'btn-soft'}`}
          onClick={() => (filter.value = 'waiting')}
        >
          未完成 {counts().waiting}
        </button>
        <button
          type="button"
          class={`btn btn-sm join-item ${filter.value === 'done' ? 'btn-primary' : 'btn-soft'}`}
          onClick={() => (filter.value = 'done')}
        >
          已完成 {counts().done}
        </button>
      </div>

      {sections().length === 0 ? (
        <p class="text-sm opacity-70">这一档下没有条目。</p>
      ) : null}

      {sections().map((s) => (
        <details
          key={s.group}
          open={openOf(s.group)}
          onToggle={(e) => onToggle(s.group, (e.target as HTMLDetailsElement).open)}
          class="disclosure bg-base-200"
        >
          {/*
            ⚠ 同上：**不用 `disclosure-arrow`**（那条 `float: right` 的箭头只在单行
            行盒里落得对，这里是 flex 行 ⇒ 会跑到框外 ✗）。箭头自己画 ✓。
          */}
          <summary class="disclosure-title text-sm">
            <span class="flex items-center justify-between gap-2">
              <span class="flex items-center gap-2">
                <span>{GROUP_NAMES[s.group] ?? '其它任务'}</span>
                <span class="badge badge-sm badge-ghost">{s.rows.length}</span>
              </span>
              <IconChevronDown
                class={`size-4 shrink-0 opacity-60 transition-transform ${openOf(s.group) ? '' : '-rotate-90'}`}
              />
            </span>
          </summary>
          <div class="disclosure-content flex flex-col gap-1">
            {s.rows.map((i) => (
              <div
                key={String(i.taskId)}
                class="flex items-start justify-between gap-2 rounded-box bg-base-200 px-3 py-2"
              >
                <div class="flex min-w-0 flex-col gap-0.5">
                  <span class="break-words text-sm">{plainName(i.name ?? '')}</span>
                  <span class="break-words font-mono text-xs opacity-60">{metaLine(i)}</span>
                  {i.description != null && i.description !== '' ? (
                    <span class="break-words text-xs opacity-50">{i.description}</span>
                  ) : null}
                </div>
                <span class={`badge badge-sm shrink-0 ${stateView(i).cls}`}>
                  {stateView(i).label}
                </span>
              </div>
            ))}
          </div>
        </details>
      ))}
    </div>
  )
}
