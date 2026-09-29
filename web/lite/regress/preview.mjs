/**
 * 把 lite 包**真跑起来**看：静态资源 + fixture 后端，一个独立端口。
 *
 * ⚠ 它**不是**仓库那个服务（3000 端口，由 launchd 托管）：不碰它、不占它的端口、
 * 也不需要 `cargo build`。数据是 `lib.mjs` 里的 fixture（形状照 `web/src/api.ts` 写），
 * 所以点得动、有内容，但**不是真数据** —— 看的是"界面能不能用"。
 *
 * 用法：`node lite/regress/preview.mjs [端口] [ready|login|setup]`
 * （先 `npm run build`，它发的是 `web/dist`）
 */
import { serve } from './lib.mjs'

const port = Number(process.argv[2] ?? 48700)
const variant = process.argv[3] ?? 'ready'

await serve('dist', port, variant)
console.log(`预览（dist，variant=${variant}）：http://127.0.0.1:${port}/#/settings`)
console.log('页面：设置 / 账号 / 任务 / 兑换 / 直播 / 定时 / 编排 / 历史 / 总览；登录态由 variant 决定')
