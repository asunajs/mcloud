/**
 * 兑换（抢兑）：读奖品清单 → 挑一件 → 二次确认 → 真花豆下单。
 *
 * # 这一页与别处最大的不同：写操作**真的扣豆**
 *
 * 所以三件事是硬要求，不是修饰：
 * ① 点"兑换"先弹**二次确认**，确认框里写出**具体这一件的名字与价格**
 *    （只写"确定吗"等于没确认）；
 * ② 判据没过的档位按钮 **disabled**，并把"为什么不能兑"当面写出来 ——
 *    那正是 `plan.reason()` 存在的意义；
 * ③ 不同档位**文案分档**，别把"本月已兑"与"余额不足"显示成同一句
 *    （前者要等下个月，后者充豆就行，用户的下一步动作完全不同）。
 *
 * # 一份清单里有两个"时刻"
 *
 * 清单那条路由是**只读**的，且**不查** `infoV3`（账号级请求，多打一枪
 * 多一分风控噪声）⇒ 它给的档位里**没有**"余额不足"这一档。
 * 也就是说界面上的"可兑"比下单前的真判据**宽**。
 * 后端把这件事连同人话说明一起发过来（`balanceAware` / `balanceNote`），
 * 这里**直接显示**，不自己编 —— 编出来的说明会在后端改口径时变成谎话。
 */

import { onMounted, ref } from 'vue'
import {
  listAccounts,
  listExchangePrizes,
  runExchange,
  type AccountSummary,
  type ExchangePrizeView,
  type ExchangePrizesResponse,
  type ExchangeRunOutcome,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { fmtEpochMs } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 二次确认弹窗的 id。 */
const CONFIRM_ID = 'confirm-exchange'

/**
 * 判据档位 → 徽章颜色。
 *
 * 只分四类：可兑、**要等**（时间窗没到）、**已兑过**（本月/本组用掉了）、
 * 其余不可兑。刻意不按 17 档一一配色 —— 用户要区分的是"我该做什么"，
 * 而那只有这几类。
 */
const badgeClass = (prize: ExchangePrizeView): string => {
  if (!prize.blocks) return 'badge badge-success'
  if (prize.plan === 'not-started') return 'badge badge-info'
  if (prize.plan === 'exchanged-this-prize' || prize.plan === 'exchanged-this-group') {
    return 'badge badge-warning'
  }
  return 'badge badge-ghost'
}

/** 兑换结果的档位 → 提示条颜色。 */
const resultAlertClass = (status: string): string => {
  switch (status) {
    case 'ordered':
    case 'already-exchanged':
      return 'alert alert-success'
    case 'unknown':
    case 'slider-unresolved':
      return 'alert alert-warning'
    default:
      return 'alert alert-error'
  }
}

export const ExchangePage = () => {
  const state = ref<LoadState>('loading')
  /** 整页级的加载失败（要配"重试"，所以留在页内，弹窗会挡住入口）。 */
  const errorMessage = ref('')

  const accounts = ref<AccountSummary[]>([])
  const account = ref('')

  const list = ref<ExchangePrizesResponse | null>(null)
  /** 清单本身读失败时的页内提示（与整页失败分开：账号还在，可以重试）。 */
  const listError = ref('')

  /** 正在兑换的那一件（用于把那一颗按钮变成"兑换中"并锁住别的）。 */
  const busyPrizeId = ref<number | null>(null)
  /** 待确认的那一件。 */
  const pending = ref<ExchangePrizeView | null>(null)
  /** 上一次兑换的结论。 */
  const result = ref<ExchangeRunOutcome | null>(null)

  const loadAccounts = async () => {
    accounts.value = await listAccounts()
    if (account.value === '' && accounts.value.length > 0) {
      // ⚠ 用 `id`（路由的 `{id}` 就是它），**不是**手机号：
      // `AccountSummary` 里的 `id` 是账号标识（界面上常见 `19****01` 这种打码形态），
      // 拿手机号去当路径参数会 404。
      account.value = accounts.value[0]!.id
    }
  }

  const loadPrizes = async () => {
    if (account.value === '') {
      list.value = null
      return
    }
    listError.value = ''
    try {
      list.value = await listExchangePrizes(account.value)
    } catch (e) {
      list.value = null
      listError.value = e instanceof Error ? e.message : String(e)
    }
  }

  const load = async () => {
    state.value = 'loading'
    try {
      await loadAccounts()
      await loadPrizes()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : String(e)
      state.value = 'error'
    }
  }

  onMounted(load)

  /** 点"兑换"：先记下这一件，弹确认。 */
  const askExchange = (prize: ExchangePrizeView) => {
    pending.value = prize
    openDialog(CONFIRM_ID)
  }

  const doExchange = async () => {
    const prize = pending.value
    if (!prize) return
    busyPrizeId.value = prize.prizeId
    try {
      result.value = await runExchange(account.value, {
        prizeIdOrName: String(prize.prizeId),
      })
      closeDialog(CONFIRM_ID)
      pending.value = null
      // 兑完清单里的状态变了（余量、limit），**重读**才不是旧数。
      await loadPrizes()
    } catch (e) {
      // 后端把业务失败也折成了正常响应（那是结论不是错误），
      // 所以能走到这里的多半是"参数不成立/会话失效"这一类，照实弹出来。
      closeDialog(CONFIRM_ID)
      pending.value = null
      showError('兑换没跑起来', e instanceof Error ? e.message : String(e))
    } finally {
      busyPrizeId.value = null
    }
  }

  const priceText = (prize: ExchangePrizeView) =>
    prize.price == null ? '价格未知' : `${prize.price} 朵`

  return (
    <div class="flex flex-col gap-4">
      {/* ── 账号 + 刷新 ─────────────────────────────────────────────── */}
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">兑换</h2>
          <p class="text-sm opacity-70">
            挑一件兑换。下单会真的扣AI豆，所以点下去要再确认一次。
          </p>

          {accounts.value.length === 0 ? (
            <div class="alert alert-warning">
              <span>还没有已登录的账号 —— 兑换要用账号的票去下单。</span>
            </div>
          ) : (
            <fieldset class="fieldset">
              <legend class="fieldset-legend">账号</legend>
              <select
                class="select w-full md:w-80"
                value={account.value}
                onChange={(e) => {
                  account.value = (e.target as HTMLSelectElement).value
                  // 换了账号，上一件的结论就不属于这个账号了 —— 清掉，
                  // 否则"刚兑成功"那条提示会挂在一个没兑过的账号下面。
                  result.value = null
                  void loadPrizes()
                }}
              >
                {accounts.value.map((a) => (
                  <option value={a.id}>{a.nickname ?? a.id}</option>
                ))}
              </select>
            </fieldset>
          )}

          <div class="flex flex-col gap-2 text-xs opacity-60">
            {list.value ? (
              <span>
                清单读取于 {fmtEpochMs(list.value.nowMs)}。判据档位不含余额 ——
                {list.value.balanceNote}
              </span>
            ) : null}
          </div>
        </div>
      </div>

      {/* ── 上一次的结论 ────────────────────────────────────────────── */}
      {result.value ? (
        <div class={resultAlertClass(result.value.status)}>
          <div class="flex flex-col gap-1">
            <span class="font-medium">{result.value.summary}</span>
            <span class="text-xs opacity-80">
              档位 {result.value.plan}（{result.value.planReason}）
              {result.value.code !== 0 ? ` · 业务码 ${result.value.code}` : ''}
              {result.value.offset != null ? ` · 落点 ${result.value.offset}` : ''}
              {result.value.attempts === 0 ? ' · 一枪未发' : ` · 下单 ${result.value.attempts} 次`}
            </span>
            {result.value.order ? (
              <span class="text-xs opacity-80">
                订单号 {String(result.value.order.oid ?? '—')}
                {result.value.order.prizeName ? ` · ${String(result.value.order.prizeName)}` : ''}
              </span>
            ) : null}
            {result.value.error ? (
              <span class="text-xs opacity-80">说明：{result.value.error}</span>
            ) : null}
          </div>
        </div>
      ) : null}

      {/* ── 清单 ────────────────────────────────────────────────────── */}
      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading loading-lg" />
        </div>
      ) : state.value === 'error' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="读不到账号或清单。"
              hint={errorMessage.value}
              action={
                <button type="button" class="btn" onClick={load}>
                  重试
                </button>
              }
            />
          </div>
        </div>
      ) : listError.value !== '' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="奖品清单没读到。"
              hint={listError.value}
              action={
                <button type="button" class="btn" onClick={loadPrizes}>
                  重读清单
                </button>
              }
            />
          </div>
        </div>
      ) : !list.value || list.value.prizes.length === 0 ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="这个账号现在没有可兑换的奖品。"
              hint="清单由上游按活动与账号投放 —— 换个账号看看，或等活动开场后再来（清单只读，随时可以刷新）。"
              action={
                <button type="button" class="btn" onClick={loadPrizes}>
                  刷新清单
                </button>
              }
            />
          </div>
        </div>
      ) : (
        <div class="flex flex-col gap-3">
          {list.value.prizes.map((prize) => (
            <div class="card bg-base-100 shadow-sm">
              <div class="card-body">
                <div class="flex flex-wrap items-start justify-between gap-2">
                  <div class="flex min-w-0 flex-col gap-1">
                    <h3 class="card-title text-base">
                      {prize.name ?? `奖品 ${prize.prizeId}`}
                    </h3>
                    <span class="text-sm opacity-70">
                      {priceText(prize)} · 编号 {prize.prizeId}
                      {prize.minRemainderCount != null ? ` · 余量 ${prize.minRemainderCount}` : ''}
                    </span>
                  </div>
                  <span class={badgeClass(prize)}>{prize.blocks ? '不可兑' : '可兑'}</span>
                </div>

                {/* 判据没过时**必须**说清为什么 —— 那就是 reason 的用处 */}
                <p class="text-sm opacity-70">{prize.planReason}</p>

                {prize.startTime || prize.endTime ? (
                  <span class="text-xs opacity-60">
                    活动时间 {prize.startTime ?? '—'} ~ {prize.endTime ?? '—'}
                  </span>
                ) : null}

                <div class="card-actions justify-end">
                  <button
                    type="button"
                    class={prize.blocks ? 'btn' : 'btn btn-primary'}
                    disabled={prize.blocks || busyPrizeId.value !== null}
                    tabindex={prize.blocks ? -1 : undefined}
                    aria-disabled={prize.blocks ? 'true' : undefined}
                    onClick={() => askExchange(prize)}
                  >
                    {busyPrizeId.value === prize.prizeId ? '兑换中…' : '兑换'}
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* ── 二次确认 ────────────────────────────────────────────────── */}
      <ConfirmDialog
        id={CONFIRM_ID}
        title="确认兑换"
        impact={
          <span>
            将用{' '}
            <strong>{pending.value?.name ?? `奖品 ${pending.value?.prizeId ?? ''}`}</strong>
            {pending.value?.price != null ? <> 兑换这一件，扣 <strong>{pending.value.price} 颗 AI豆</strong></> : ' 兑这一件'}
            。扣掉的豆不会退回，同一个月同一件也不能重复兑。
          </span>
        }
        confirmLabel="扣豆兑换"
        busy={busyPrizeId.value !== null}
        onConfirm={doExchange}
      />
    </div>
  )
}
