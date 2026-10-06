/**
 * 日志行的**对象身份**复用 —— 让每 2 s 的轮询不再把视口里那几十行整批重建。
 *
 * # 为什么需要它（台账第 96 行 ② 那条线索）
 *
 * 这一页的取数是"每拍把整份列表换掉"（`logs-page.tsx` 的 `load()` ✓），而
 * `lite` 的 `.map()` 编成 `createFor(list, render, key)`，`key` 判的是 item 的
 * **对象身份** ✓（`web/lite/compiler.ts` 的说明 ✓）。`/api/logs` 每拍回来的是
 * **刚 `JSON.parse` 出来的新对象** ⇒ 就算这一行**一个字都没变**，身份也变了 ⇒
 * 视口里那几十行每 2 s 被拆掉重建一次 ✗。
 *
 * 两个后果，第二个才是用户能看见的那个：
 * 1. 每拍无谓地造/丢一批节点（DevTools 性能监视器那个读数会来回跳 ✓）；
 * 2. **正在选中的日志文字每 2 秒被清掉** ✗ —— 选中状态挂在节点上，节点换了就没了。
 *    而这一页的用途恰恰是"一边跑一边看、顺手复制一行去查" ✓。
 *
 * # 判据：**逐字段相同**才算同一行
 *
 * 键就是这行渲染出来的全部依据（时刻 / 级别 / 来源 / 正文 ✓）⇒ 键相同 ⇒
 * 两行**完全等价** ⇒ 交回上一拍那个对象是安全的 ✓（这些对象此后不再被改写：
 * 它们只在 `load()` 里整体替换，没有一处就地改字段 ✓）。
 *
 * ⚠ 字段少一个都不行：`at_ms` 不同是两行（同一句话打了两遍 ✓），
 * `level` 不同是两行（同一句话一次 info 一次 warn ✓）。漏掉任何一个都会把
 * **不同**的两行判成同一行 ⇒ 界面上少一行 ✗（比"多重建几次"严重得多 ✓）。
 */

/** 判"是不是同一行"要用的那几格 —— 与 `/api/logs` 的返回字段一一对应 ✓。 */
export interface LogIdentity {
  level: string
  target: string
  message: string
  at_ms: number
}

/**
 * 一行的身份键。
 *
 * ⚠ 用 `JSON.stringify` 而不是拼分隔符：正文里出现分隔符（`\t`、`\u0000`）不是不可能，
 * 而**键撞了就是少一行** ✗。JSON 的转义让"哪一格是哪个"无歧义 ✓，
 * 代价是每拍多 2000 次小数组序列化（微秒级 ✓）。
 */
const keyOf = (l: LogIdentity): string => JSON.stringify([l.at_ms, l.level, l.target, l.message])

/**
 * 把 `next` 里"与 `prev` 逐字段相同"的行换成 `prev` 里那个**对象**，
 * 其余照原样留下 —— 返回的**数组本身是新的**（信号要变才会重渲染 ✓）。
 *
 * ⚠ `next` 必须是一份**新数组**（每拍刚从网络解析出来的那份 ✓）：
 * 传同一个数组进来等于"信号没变"，界面不会更新 ✗。
 */
export function reuseLogLines<T extends LogIdentity>(prev: readonly T[], next: readonly T[]): T[] {
  // 首拍（或清空后）没有可复用的 ⇒ 原样交回 ✓（省掉整趟建表）
  if (prev.length === 0) return next as T[]
  /**
   * 同键多行要**按出现次数**各取一个 ⇒ 用桶（数组）而不是单值。
   *
   * 为什么真会发生：同一毫秒、同一来源、同一句话连打两遍（重试日志就这么写 ✓）。
   * 单值映射会让两行拿到**同一个对象** ⇒ `createFor` 收到两个相同的 key
   * （那不是"复用"，是"两行claim同一行" ✗）。
   */
  const pool = new Map<string, T[]>()
  for (const l of prev) {
    const k = keyOf(l)
    const bucket = pool.get(k)
    if (bucket) bucket.push(l)
    else pool.set(k, [l])
  }
  return next.map((l) => {
    const bucket = pool.get(keyOf(l))
    // 命中 ⇒ 这一行与上一拍逐字段相同 ⇒ 沿用上一拍那个对象（身份不变 ⇒ 行节点被复用 ✓）
    return bucket && bucket.length > 0 ? (bucket.pop() as T) : l
  })
}
