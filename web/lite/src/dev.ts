/**
 * 是否为**开发构建**。
 *
 * ⚠ 写法必须是 `import.meta.env.DEV` 这种**能在构建期被静态替换**的形式：
 * Vite 在 `build` 时把它换成字面量 `false`，压缩器随即把 `if (DEV) { … }` 整块折掉
 * —— 于是诊断代码（长文案 + `new Error().stack` 抓栈）**不进生产产物**，
 * 但在 `vite dev` 里一条不少。
 *
 * 实测（2026-09-30，`node lite/size.mjs`）：这条值 **gzip −337 B**（2,808 → 2,471，约 −12%）。
 *
 * ⚠ 不要改成 `globalThis.__DEV__` 之类的**运行时**值：那样压缩器折不掉分支，
 * 字符串会照旧留在产物里 —— 只有执行路径被跳过，体积一分不省。
 *
 * ⚠ 各构建入口（`web/vite.config.ts`、`lite/demo`、`lite/bench`、`lite/regress`、
 * `lite/size.mjs`）都走 Vite，所以 `import.meta.env` 一定有。将来若有人用
 * 裸 esbuild/rolldown 直接打 `lite/src`，必须自己 `define` 它。
 */
export const DEV: boolean = import.meta.env.DEV
