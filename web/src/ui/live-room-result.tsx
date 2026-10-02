/**
 * 直播结果的**结构化明细**：领到的花、逐条口令的落点。
 *
 * # 为什么单独一个模块
 *
 * 这套渲染原来长在直播页里，于是**只有"当前这次"能看到它** ✗ ——
 * 历史页里过去的每一次直播都只剩一行小结，逐条明细（哪条口令成功、哪条过期、
 * 为什么）在界面上根本打不开。用户想回看"上一场我到底领到没有"时无处可看。
 *
 * ⇒ 抽出来给**两处共用**：直播页（当前这次）与历史页（过去的每一次）。
 * 这正是"历史页复用结构化明细"这一条要求的落点。
 *
 * ⚠ 三个函数只依赖入参 `view`（结构化视图模型，见 `api.ts::toLiveRoomRunView`），
 * 所以能这样搬 —— 一旦它们碰了页面里的 ref/局部状态，就只能复制一份，
 * 而"两份长得像但会各自漂移"是这套界面里最难查的一类不一致。
 */
import type { LiveRoomRunView } from '../api'

export const gainedCard = (view: LiveRoomRunView) => (
    <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
      <div class="stat">
        <div class="stat-title">本次进账</div>
        <div class="stat-value text-2xl">
          {view.gained == null ? '—' : `${view.gained > 0 ? '+' : ''}${view.gained}`}
          <span class="ml-1 text-base font-normal opacity-70">朵</span>
        </div>
        <div class="stat-desc">
          {/* 进账 = 终值 − 起始，**由后端给**。前端把两朵花加起来会漏掉
              「首次参与惊喜奖」那几朵 —— 它不在任何一条口令的结果里。 */}
          {view.balanceBefore != null && view.balanceAfter != null
            ? `余额 ${view.balanceBefore} → ${view.balanceAfter}（终值 − 起始，后端算的）`
            : '后端这一趟没给余额读数'}
        </div>
      </div>
      <div class="stat">
        <div class="stat-title">拿去领取的口令</div>
        <div class="stat-value text-2xl">{view.attempted}</div>
        <div class="stat-desc">
          成功 {view.succeeded.length} · <b class="font-medium">本号</b>已领过{' '}
          {view.redeemed.length} · 已失效{' '}
          {view.expired.length} · 无效 {view.invalid.length} · 未分类 {view.other.length}
        </div>
      </div>
    </div>
  )

  /**
   * 一类口令一栏。
   *
   * ⚠ 这里**故意做成逐类一栏**而不是一个合并列表：把「已失效」和「无效」
   * 显示在一起、或者合并成一句"口令错误"，就会让用户去查**错的方向**
   * （抄错了口令 vs 直播已经结束了）。空的那几类**不显示** ——
   * 每类都挂一个"无"会把真正有内容的那一栏淹掉。
   *
   * ⚠⚠ 判空用**三元**（`codes.length === 0 ? null : …`），返回 `null` 就行 ——
   * lite 的 `setNodes` 走 `createNodes`，`null` / `false` 都归一成"零个节点"，
   * 在 JSX 子节点位置上放一个返回 `null` 的调用是安全的。
   *
   * （历史备注：以前这里警告过"子节点位置返回 `null` 会让渲染整块崩掉"，
   * 那是 **Vue Vapor** 的 `setNodes` 读 `null.parentNode` 的毛病，2026-09-30 拆掉
   * 兼容层之后不适用了。别照那条旧警告改代码。）
   */
export const group = (label: string, hint: JSX.Element, badge: string, codes: { code: string }[]) =>
    codes.length === 0 ? null : (
      <div class="flex flex-col gap-1">
        <div class="flex items-center gap-2">
          <span class={`badge ${badge}`}>{label}</span>
          <span class="text-xs opacity-60">{codes.length} 条</span>
        </div>
        <p class="text-xs leading-5 opacity-60">{hint}</p>
        <ul class="flex flex-col gap-0.5">
          {codes.map((c) => (
            <li key={c.code} class="font-mono text-xs break-all opacity-80">
              {c.code}
            </li>
          ))}
        </ul>
      </div>
    )

export const codeBreakdown = (view: LiveRoomRunView) => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <h2 class="card-title text-base">口令结果</h2>
        {/* 成功 */}
        {view.succeeded.length > 0 ? (
          <div class="flex flex-col gap-1">
            <div class="flex items-center gap-2">
              <span class="badge badge-soft badge-success">成功</span>
              <span class="text-xs opacity-60">{view.succeeded.length} 条</span>
            </div>
            <p class="text-xs leading-5 opacity-60">这几条换到了花。</p>
            <ul class="flex flex-col gap-0.5">
              {view.succeeded.map((c) => (
                <li key={c.code} class="font-mono text-xs break-all opacity-80">
                  {c.code}
                  {c.flowerNum != null ? `（+${c.flowerNum}）` : ''}
                  {c.prizeName ? ` · ${c.prizeName}` : ''}
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {/* 已领过（200112）—— 不是失败 */}
        {view.redeemed.length > 0 ? group(
          '本号已领过',
          <span>
            这个<b class="font-medium">账号</b>已经用这条口令领过了 —— 口令本身是对的，
            也不是"口令被别人用掉了"（口令<b class="font-medium">全场通用</b>，谁都能领）。
          </span>,
          'badge-soft',
          view.redeemed,
        ) : null}
        {/*
          ⚠⚠ 这两栏**必须分开**，不能合并成"口令错误"：
          「已失效」= 口令是对的、但本场直播结束了（下一步：等下一场）；
          「无效」  = 这条口令上游不认识（下一步：查口令从哪抄来的）。
        */}
        {view.expired.length > 0 ? group(
          '口令已失效（本场已结束）',
          <span>
            口令本身是<b class="font-medium">对的</b>，但这一场直播已经结束了 ——
            上游不再收。下一步是<b class="font-medium">等下一场</b>，
            而不是去查口令抄得对不对。
          </span>,
          'badge-soft badge-warning',
          view.expired,
        ) : null}
        {view.invalid.length > 0 ? group(
          '无效口令',
          <span>
            这条口令上游不认识，多半是从别处抄来的、或者上一场的废值。
            口令只在<b class="font-medium">当场</b>的直播里有效。
          </span>,
          'badge-soft badge-error',
          view.invalid,
        ) : null}
        {view.other.length > 0 ? group(
          '未分类',
          <span>
            上游回了一个我们还没分类的业务码。它既不是"口令抄错了"也不是"直播结束了"
            —— 排查方向是上游改了什么。
          </span>,
          'badge-soft badge-warning',
          view.other,
        ) : null}
        {view.succeeded.length === 0 &&
        view.redeemed.length === 0 &&
        view.expired.length === 0 &&
        view.invalid.length === 0 &&
        view.other.length === 0 ? (
          <p class="text-sm opacity-70">
            这一趟没有拿去领取的口令 —— 所以下面是空的。原因见上面那条结论。
          </p>
        ) : null}
      </div>
    </div>
  )

  /**
   * "没领"的那三条路。
   *
   * ⚠ 措辞**逐条不同**，来自后端 `idle_summary()`：三种"没领"的下一步
   * 完全不一样（等 / 别等 / 本来就没打算领）。混成一句"没有可领的口令"
   * 会让人去查**收集**那一步，而那一步根本没被走到过。
   */

/** 历史页/直播页共用的明细块（去掉"当前这次"的页头与空态提示）。 */
export const LiveRoomResult = (props: { view: LiveRoomRunView }) => (
  <div class="flex flex-col gap-3">
    {gainedCard(props.view)}
    {codeBreakdown(props.view)}
  </div>
)
