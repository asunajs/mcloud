/**
 * 无头 Chrome 的公共启动参数 —— 本仓**所有** spawn chrome 的地方只有这一个来源。
 *
 * # 为什么单独一个文件
 *
 * 2026-10-07 之前 lite 还在本仓（`web/lite/`），会 spawn chrome 的有三处
 * （`lite/regress/lib.mjs`、`lite/demo/run.mjs`、`lite/bench/run.mjs`）。现在框架
 * **已剥离到 `asunajs/lite`**（见 `docs/web/lite.md`），本仓只剩 `regress/lib.mjs`
 * 这一处（页面级验收）；框架自己那两处带着它的一份副本。
 * 参数表仍然只有一份，"要加什么"仍然只有一个地方可改 ✓ —— 那条教训是实测来的：
 * 2026-10-05 三处各写一份参数表时，不同的写入线往自己那份里塞 `--no-sandbox`、又各自
 * 还原 ⇒ **一晚上打了三次架**（我打上、别线还原 ⇒ 我这边 11 条门禁红 9 条；反过来别线
 * 在我改到一半时跑 `ready` 也红过）。
 *
 * ⚠ 环境变量名两仓不同（这是有意的）：本仓是 `MCLOUD_CHROME_NO_SANDBOX`，
 * 框架仓是 `LITE_CHROME_NO_SANDBOX`。
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
