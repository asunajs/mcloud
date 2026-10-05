/**
 * 无头 Chrome 的公共启动参数 —— **三处 spawn 只有这一个来源**。
 *
 * # 为什么单独一个文件
 *
 * 会 spawn chrome 的有三处：`lite/regress/lib.mjs`、`lite/demo/run.mjs`、
 * `lite/bench/run.mjs`。以前三处**各写一份参数表**，于是 2026-10-05 为了在本机沙箱里
 * 跑门禁，不同的写入线往自己那份里塞 `--no-sandbox`、又各自还原 ⇒
 * **一晚上打了三次架**（我打上、别线还原 ⇒ 我这边 11 条门禁红 9 条；反过来别线在我
 * 改到一半时跑 `ready` 也红过）。参数表只有一份，"要加什么"就只有一个地方可改 ✓。
 *
 * # ⚠ `--no-sandbox` 只能是**显式开关**，不能是默认
 *
 * 关掉 Chromium 的沙箱是**降级**（渲染进程不再与内核隔离）。"某台机器上必须加"
 * （本仓的 DSH 沙箱里 `chrome-headless-shell` 不带它直接 `Target crashed` /
 * `CDP 调用超时：Page.enable`）**不等于**所有人都该关掉 ✗ —— 开发机默认不需要它。
 * 所以：**默认不加**，要加就显式给一个环境变量：
 *
 * ```bash
 * MCLOUD_CHROME_NO_SANDBOX=1 node scripts/gates-web.mjs --push
 * ```
 *
 * ⚠ 那个变量会**穿过 git 钩子**（`pre-commit` / `pre-push` 里的 `node scripts/gates-web.mjs`
 * 继承当前环境）⇒ 钩子里跑的门禁同样生效，不需要为了过钩子去改文件 ✓。
 */

/** 环境变量"算不算打开"：空 / `0` / `false` / `no` 都当没开（免得 `=0` 被当成真）。 */
const noSandbox = () => {
  const v = (process.env.MCLOUD_CHROME_NO_SANDBOX ?? '').trim().toLowerCase()
  return v !== '' && v !== '0' && v !== 'false' && v !== 'no'
}

/**
 * 公共前缀参数（`--headless` + 可选的 `--no-sandbox` + `--disable-gpu`）。
 *
 * 各自的私有参数（`--hide-scrollbars` / `--window-size` / `--virtual-time-budget` /
 * `--dump-dom` / URL）由调用方自己往后接 —— 那部分三处本来就不一样，没必要统一。
 */
export const headlessFlags = () => ['--headless', ...(noSandbox() ? ['--no-sandbox'] : []), '--disable-gpu']
