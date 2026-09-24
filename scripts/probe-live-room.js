#!/usr/bin/env node
// 开播后一键核对：把「仍未实拍」那几条逐条跑一遍 —— 能自动的自动跑，要抓包的把
// 前置条件、步骤与判据打出来。
//
// ─────────────────────────────────────────────────────────────────────────────
// # 为什么需要它
//
// 「直播口令 → 抽小红花」这条链有 **5 条事实**只在"主播开播的那几小时"里才可能拿到：
// 未开播时 `roomBeat` 回 `status:0` + `chatRoomId:null` ⇒ **WS 结构性地不存在**
// （坑表 #107：那不是"抓漏了"）。窗口一过就得再等一天，而**漏掉一条是静默的** ——
// 你只会以为核完了。这份脚本把"每条能不能自动核 / 判据是什么 / 结果写到哪里"固定下来。
//
// ⭐ **2026-09-23 的窗口用掉了**：清单里 **2 条已实拍**
// （`member-apply-headers` / `ws-frame-order`，结论在各条的 `evidence`，
// 原始证据在 `docs/capture/app-live-ws-samples.txt`），**1 条仍无样本**
// （`journaling-optkeywords` —— 它**不在** cmlive H5 的进房链路上）。
// ⚠ 已实拍的**不从清单里删**：服务端可能变，下次开播要**回归**；
// `--run` 会把这一批单独列出来，别把它当成"还没核"。
//
// # 权威性（⚠ 别在别处再抄一份）
//
// 下面的 `ITEMS` 就是**唯一**一份清单。`docs/status.md` 的「未验证清单」与
// `docs/cli-usage.md` 里都只放**指针**。理由与坑表 **#116** 同源：
// 一份**手写的清单**若声称"全都在这里"，它迟早会漏 —— 而漏了**不会报错**。
// ⇒ 改了 `ITEMS`，必须回去按结果**补记** `status.md`（脚本末尾会提醒）。
//
// # 用法
//
//   node scripts/probe-live-room.js                     # 打印清单与每条的确切命令（**不碰网络**）
//   node scripts/probe-live-room.js --run               # 跑能自动跑的两条：在播/门 + 真口令（**只读**）
//   node scripts/probe-live-room.js --wire              # 检查抓包那条路的前置条件（**不碰网络**）
//
// ⚠⚠ 没有"投口令"这一档，也**没有** `--code` 这个开关（2026-09-23 删）：
// 本脚本**只收口令并打印**，不去兑换 —— 口令是全场通用的，不存在"消耗"，
// 但"投"本身是一次兑换写操作，不该由一个"核对清单"替用户做。
//
// 可选：`--db <路径>`（默认 `data/mcloud.db`）、`--account <下标>`、`--listen <秒>`（默认 60）。
//
// 退出码（`--run` 才有意义）：
//
//   0  凡**这一趟会跑**的项都拿到了。⚠ **不等于**整份清单核完了 ——
//      要抓包的那几条本来就不在这一趟里（脚本会印出还剩几条）。
//   1  前置成立，但**有项真的试过却没拿到**（多半是"门开着、主播这一段没喊"—— 重跑即可）
//   2  **前置不成立**：现在没在播。⚠ 这不是失败 —— 但也**不是"核过了"**，
//      后几条在这个状态下**不适用**（WS 结构性地不存在，坑表 #107）。
//      把它和 1 分开，是为了让"今天白跑一趟"与"跑了但没拿到"在脚本里就分得开。
//
// ⚠⚠ 三档必须**档档可达**。原先的实现是 `process.exit(notLive ? 2 : 0)` ——
// 于是 **1 永远出不来**："门开着、主播这一段一条口令都没喊"会退 0、被记成"核完了"，
// 而它恰恰是最该重跑一次的那种处境。这是"一个退出码承载两种处境"
// （与坑表 #113 同族），代价是**那趟不会重跑**。
//
// 产出：`docs/capture/probe-live-room-<YYYY-MM-DD>.md` —— **追加**，不覆盖当天的前几次
// （与仓库的"记录只增改不删"一致；一天里可能跑好几次）。
//
// ─────────────────────────────────────────────────────────────────────────────
// # 三条纪律（别绕开）
//
// 1. **默认不碰网络。** 要真打上游必须显式 `--run`。
//    ⚠⚠ **本脚本从不投口令**（2026-09-23 定）：它只**收**口令并打出来。
//    早先这里有一条"给 `--code` 就投两次抓 `200112`"，建立在"口令一次性、会被消耗"
//    这个**未验证**的前提上 —— 口令是**全场通用**的，没有"消耗"这回事；
//    而"投"本身是一次兑换**写操作**，不该由核对清单替用户做。
// 2. **口令只在本次直播有效。** 直播时间外、以及**上一场的口令**都不能用
//    （上游 `200108` 的原话就是"口令需当场直播获取才有效哦"）。
//    ⇒ 报告里必须带上**抓到的时刻** —— 没有时刻的样本，事后分不清是
//    "口令抄错了"、"抄的时候已经过期"还是"那是上一场的口令"。
// 3. **抓包那条路要 Reqable 接管系统代理。** 脚本只**检查**前置条件并打印步骤，
//    不自己去开代理 —— 改系统代理是机器级副作用，不该由一个"核对清单"偷偷做。
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { cd } from 'zx'

cd(new URL('..', import.meta.url).pathname)

// ─────────────────────────────────────────────────────────────────────────────
// 参数
// ─────────────────────────────────────────────────────────────────────────────

const argv = process.argv.slice(2)
const flag = (name, fallback) => {
  const i = argv.indexOf(name)
  return i >= 0 && i + 1 < argv.length ? argv[i + 1] : fallback
}
const DB = flag('--db', 'data/mcloud.db')
const ACCOUNT = flag('--account', '')
const LISTEN = flag('--listen', '60')
// ⚠ 这里**没有** `--code`（2026-09-23 删）：本脚本只收口令、不投口令，
//   见上面「三条纪律」第 1 条。要给口令去兑换请用 `mcloud live-room --codes …`
//   本身 —— 那是**用户自己**决定要投的时候。
const RUN = argv.includes('--run')
const WIRE = argv.includes('--wire')

const CLI = 'target/probe-mcloud'
const REPORT = `docs/capture/probe-live-room-${new Date().toISOString().slice(0, 10)}.md`

const accountArgs = ACCOUNT ? ['--account', ACCOUNT] : []

// ─────────────────────────────────────────────────────────────────────────────
// 跑 CLI
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 编一次二进制，后面反复用。
 *
 * 为什么不在每次调用时 `go run`：这条链要打 3–4 次上游，每次都重编一遍会让
 * "门→收→投"之间隔上好几秒 —— 而**口令的有效期是"当场"**，中间隔得越久越可疑。
 * 编译产物落在 `target/`（已 gitignore）。
 */
function build() {
  const r = spawnSync('go', ['build', '-o', CLI, './cmd/mcloud'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8',
  })
  if (r.status !== 0) {
    console.error('编译失败：\n' + (r.stderr || r.stdout))
    process.exit(1)
  }
}

/**
 * 跑一次 `mcloud live-room`，返回 { code, stdout, stderr, argv }。
 *
 * ⚠ 用 `spawnSync` 而不是 zx 的 `$`：要**同时**拿到退出码与两条流，且这条命令
 * 会打印中文（zx 的 `$` 在非 TTY 下的输出处理会多一层）。参数数组直传，
 * 不经过 shell —— 口令里可能有空格或全角逗号，过一遍 shell 就多一处转义面。
 */
function runLiveRoom(args) {
  const full = [CLI, 'live-room', '--db', DB, ...accountArgs, ...args]
  const r = spawnSync(full[0], full.slice(1), {
    stdio: ['ignore', 'pipe', 'pipe'],
    encoding: 'utf8',
  })
  return { code: r.status, stdout: r.stdout ?? '', stderr: r.stderr ?? '', argv: full.slice(1) }
}

/** 解析 `--json` 的那一份输出（拿不到就 null）。 */
function jsonOf(result) {
  try {
    return JSON.parse(result.stdout)
  } catch {
    return null
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// 三个自动探针
// ─────────────────────────────────────────────────────────────────────────────

/**
 * ① 前置：门开着，且有「中国移动云盘」的直播间在播。
 *
 * ⚠ 听 **1 秒**就够了：这一步只问"门开不开"，不是去收口令。
 * 门没开时 CLI 自己会早退（不打 WS），所以这一步的代价是一次 HTTP 往返。
 */
function probeLiveNow() {
  const r = runLiveRoom(['--kouling-only', '--listen-seconds', '1', '--json'])
  const payload = jsonOf(r)
  const room = payload?.liveRoom?.[0]
  if (r.code !== 0 || !room) {
    return { status: 'missing', detail: `退出码 ${r.code}；stderr=${r.stderr.trim()}`, raw: r }
  }
  if (!room.gateOpen) {
    return {
      status: 'missing',
      detail: `门没开：liveCount=${room.liveCount}（0 = 谁都没在播；>0 = 别的主播在播）`,
      raw: r,
    }
  }
  return { status: 'ok', detail: `门开着，在播 ${room.liveCount} 间`, raw: r }
}

/**
 * ② 一条**真口令**（当场从弹幕或小红书收到）。
 *
 * ⚠ 这是**只读**的（`--kouling-only` 不投），所以可以放心跑满 `--listen`。
 */
function probeRealKouling() {
  const r = runLiveRoom(['--kouling-only', '--listen-seconds', LISTEN, '--json'])
  const payload = jsonOf(r)
  const room = payload?.liveRoom?.[0]
  const codes = room?.kouling ?? []
  if (r.code !== 0 || !room) {
    return { status: 'missing', detail: `退出码 ${r.code}；stderr=${r.stderr.trim()}`, raw: r }
  }
  if (codes.length === 0) {
    return {
      status: 'missing',
      detail:
        `听了 ${LISTEN} 秒，两个来源一条都没收到` +
        `（门${room.gateOpen ? '开着' : '没开'} —— 门开着却没口令才是"主播没喊"）`,
      raw: r,
    }
  }
  return { status: 'ok', detail: `收到 ${codes.length} 条：${codes.join(' / ')}`, codes, raw: r }
}

/**
 * ⚠⚠ 这里**原本**有一条「③ `draw` 的 `200112`（已兑换过）」—— **已删除**（2026-09-23）。
 *
 * 它当时的做法是"把同一条口令投**两次**，第二次抓 `200112`"，注释还写着
 * "第一次投就会把这个口令**消耗掉**（不可逆）"。两个前提都**没有实拍支撑**，
 * 用户 2026-09-23 逐条纠正：
 *
 *   ① **口令全场通用**：所有用户共用同一批口令 ⇒ **不存在"消耗口令"这回事**，
 *      也不存在"我投了别人就没得投"。
 *   ② `200112` **零实拍**：连它出自哪个接口都没验证 —— 语义"已兑换"更像兑换那一步
 *      （`liveRoomFeedback/check?prizeId=`，那边 `200103` 才是"红花数不够"），
 *      不是 `draw`。拿一个没验证归属的码去断言 `draw` 的行为，正是"张冠李戴"。
 *   ③ 本脚本的定位是"**只收口令并打印**" ⇒ 它**根本不该去投口令**。
 *
 * ⇒ 该条同时从 `ITEMS` 删掉（清单 **6 条 → 5 条**），`--code` 开关一并删掉。
 * 留这一段只有一个理由：下次谁再想加"投一次看看"，先读上面这三个否定的前提。
 *
 * 现在的判据写在 `internal/upstream/live_flower.go` 顶上的「口令的五条事实」。
 */

// ─────────────────────────────────────────────────────────────────────────────
// 抓包那条路（只检查前置条件，不动系统代理）
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 检查"现在能不能抓包"，并返回一组可打印的结论。
 *
 * ⚠ **不自己去开关代理**：那是机器级副作用（会改 Wi-Fi 的代理设置），
 * 一个"核对清单"不该偷偷做。这里只报状态，让人自己决定。
 *
 * ⚠⚠ **判据必须落在 Reqable 的 GUI 那个二进制上**，不能用 `pgrep -f Reqable`：
 * 后者会命中 `/Applications/Reqable.app/Contents/Helpers/mcp-server`，
 * 而那个 helper 是 `scripts/reqable-capture.py` **自己拉起来的**（`MCP_BIN`）⇒
 * 只要抓包脚本跑过一次，这条判据就**恒真**（与坑表 #93「守卫扫自己」同一族）。
 */
function wirePreconditions() {
  const out = []
  // 1) Reqable 的 **GUI** 在跑吗
  const ps = spawnSync('pgrep', ['-fl', 'Reqable.app/Contents/MacOS/Reqable'], { encoding: 'utf8' })
  out.push({
    ok: ps.status === 0,
    what: 'Reqable 的 **GUI** 在跑（判据是 `Contents/MacOS/Reqable` 那个二进制）',
    how:
      '打开 Reqable。⚠ 别用 `pgrep -f Reqable` —— 它连抓包脚本自己拉起的 ' +
      '`Contents/Helpers/mcp-server` 一起命中，于是**恒真**',
  })
  // 2) 系统代理被接管了吗（顺带把端口打出来 —— 下面那条说明要用到它）
  const proxy = spawnSync('networksetup', ['-getsecurewebproxy', 'Wi-Fi'], { encoding: 'utf8' })
  const text = proxy.stdout ?? ''
  const server = (/Server:\s*(\S+)/.exec(text) ?? [])[1] ?? ''
  const port = (/Port:\s*(\d+)/.exec(text) ?? [])[1] ?? ''
  out.push({
    ok: /Enabled:\s*Yes/.test(text) && server !== '' && port !== '',
    what: `系统代理已被 Reqable 接管（${server || '?'}:${port || '?'}）`,
    how: 'Reqable 里的"系统代理"开关',
  })
  // 3) ⚠⚠ 关键：**取证面是 App，不是 CLI** —— 理由见下面那段 `WHY_NOT_THE_CLI`。
  const app = spawnSync('ls', ['-d', '/Applications/mCloud.app'], { encoding: 'utf8' })
  out.push({
    ok: app.status === 0,
    what: '目标 App `/Applications/mCloud.app` 在（**它**才是取证面）',
    how: '装好移动端 App（iOS App 跑在 Apple Silicon 上）；抓的时候它得**真的在播间里**',
  })
  return out
}

/**
 * ⚠⚠ **为什么不能"跑一遍 CLI 让 Reqable 抓"** —— 这条是实测出来的，别再想当然。
 *
 * Reqable 是**代理**，只能看见"愿意走代理"的流量。而本项目：
 *
 *   - `internal/io/client.go` 的 `http.Transport` **没设 `Proxy` 字段** ——
 *     在 Go 里那是"**不走代理**"的意思，它**不会**回落到 `http.ProxyFromEnvironment`
 *     （所以 shell 里那个 `HTTPS_PROXY` 对上游调用**毫无作用**）。
 *     ⚠ 旁证：`internal/server/run_test.go` 里专门写了一个
 *     `&http.Transport{Proxy: nil}` 的客户端，注释写明"默认 transport 会读 `HTTP_PROXY`"
 *     —— 即**默认** transport 才读，本项目那个显式构造的不读。
 *   - WS 是手写的直连（`internal/io/websocket.go`），同样不经代理。
 *
 * ⇒ 跑 CLI 时 Reqable 里**一条记录都不会有**，而这个现象最容易被读成
 * "**CLI 没发请求**" —— 与坑表 #102 / #107 同一族（把"当前状态"当成"这件事的性质"）。
 *
 * ⭐ 更根本的一层：这三条问的是"**真实客户端**怎么做"，
 * 而抓我们自己 CLI 的帧是**循环论证** —— 它只能证明"我发了我以为的东西"。
 * ⇒ 取证面一律是 **App**，与既有样本（`docs/capture/app-*-samples.txt`）同一路。
 */
const WHY_NOT_THE_CLI =
  '⚠ 取证面是 **App**，不是 CLI：`internal/io/client.go` 的 `http.Transport` 没设 ' +
  '`Proxy`（Go 里 = 不走代理、不会回落到 `ProxyFromEnvironment`），WS 也是手写直连 ' +
  '⇒ 跑 CLI 时 Reqable 里**一条记录都不会有**（而那个现象最容易被读成"CLI 没发请求"）。' +
  '⭐ 更根本：这几条问的是**真实客户端**怎么做，抓自己的 CLI 是循环论证。'

const WIRE_STEPS = [
  {
    id: 'journaling-optkeywords',
    title: '`visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认',
    evidence:
      '⚠ 2026-09-23 实拍**零样本**：抓包 201 条里 `visitlog` / `journaling` / `_flowerBtn` ' +
      '**全部零命中**，而**进房链路完整**抓到了 ⇒ **它不在 cmlive H5 的进房链路上**。' +
      '⭐ 触发面是**活动站入口**或**点小红花按钮** —— 从 cmlive 直播间里找它**永远找不到**。',
    lookFor:
      'App 抓包里 `POST m.mcloud.139.com/ycloud/visitlog/journaling` 的**每一条**：' +
      '`optkeyword` 是什么、响应体的 `code` 是什么。' +
      '⭐ **先数条数**：如果 App 只发 `National_LiveRoomFeedback_flowerBtn` 一条，' +
      '那"我们多发 4 条"这个取舍就该重新评估（那 4 条是照原项目抄的，理由写在 ' +
      '`upstream.LiveRoomJournalingEvents`）。',
    why:
      '⚠ CLI **刻意不解析**这个响应（`app/live_room.go` 的 `StepJournaling`：' +
      '"它是到场证据，回什么码都不改变后面的步骤"）⇒ 光跑 CLI 看不出服务端认不认，' +
      '只能看**响应体**。⚠ 而 CLI 的流量也抓不到（见上），所以这一条**只能**从 App 侧取证。',
  },
  {
    id: 'member-apply-headers',
    title: '`member/apply` 的签名头与 body（⚠ WS 升级请求**不带** `systemmd5` / `tokenmd5`）',
    verified: '2026-09-23',
    evidence:
      '✅ 实拍：`Version: v23.0.2`（原先标着"未实拍"）、body 里 **`message` 在**（空串）、' +
      '`deviceInfo: other/unknown`（**非 hex**）。' +
      '⭐⭐ 签名算式**实拍验证通过**：`tokenMd5 = md5("Bearer " + t[:10] + ts10 + t[10:])`、' +
      '`systemMd5 = md5(**deviceInfo 头值字符串** + ts10)` —— 两条记录都逐字命中。' +
      '⚠⚠ **WS 升级请求里没有 `systemmd5` / `tokenmd5`** —— 它们只属于 `/cmlive/api/*`。' +
      '⭐⭐ **这一格已收口**：反证实验（`systemMd5` 与 `tokenMd5` **两个头都打成 32 个 0**）' +
      '照样回 `code: 0`「接口调用成功」⇒ **服务端根本不校验这两个头**（capture 文档 §8.6，坑表 **#136**）。',
    lookFor:
      '✅ **不必再抓** —— 它没有可核的闸门了。⚠ 只有两种情况会把它重新打开：' +
      '① `Version` 变了（换版本时把 §8.6 那个反证实验重跑一次，复现命令只有一行）；' +
      '② 想核 body 的**键序**（09-23 实拍是 `roomId` / `message` / `userName` / `invoker`）—— ' +
      '⚠ 但键序**不是**服务端会看的判据（JSON 对象本身无序），别为它改实现。',
    why:
      '⚠⚠ 这里早先写着"算式已与 `apis/live-room-chat.ts` 逐字对拍过 —— 但**服务端认不认**只有实拍能答"，' +
      '**那句是错的**（坑表 **#136**）：它把**客户端算法**当成了**服务端契约**。' +
      '真去问服务端的结果是它**一个签名都不看**（§8.6）⇒ 这一格**没有可核的闸门**，' +
      '`SystemMD5` 也因此**不需要改**（它与实拍客户端的差异是真的，但没有功能后果）。' +
      '⭐ 这一条确实判下来一件事：原项目**漏了** body 里的 `message`（Go 补上了），实拍 body 确认该补。',
  },
  {
    id: 'ws-frame-order',
    title: 'WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」',
    verified: '2026-09-23',
    evidence:
      '✅ 实拍（133 帧 / 73.6 s）：`o` → C(`cmd=3`) → S(`cmd=3`) → C(`cmd=1`) → S(`cmd=1`) ' +
      '→ S(`cmd=2`) → C(`cmd=0`) → S(`cmd=0`) …；`command`：' +
      '**0 = ack / 1 = 心跳 / 2 = 推送（弹幕）/ 3 = PROVISION**。' +
      '⚠⚠ **`cmd=1` 是每 ~15 s 一次的周期心跳**（+608 / +15537 / +30665 / +45561 / +60583 ms），' +
      '**不是**"PROVISION 之后补发一次"。' +
      '⚠ **`h` 全程没出现**（SockJS 要静默 25 s 才发）⇒ "心跳回什么"**仍未判定**。',
    lookFor:
      'App 抓包里 `wss://videoim.cmcc-cs.cn:20102/ws/<serverId>/<sessionId>/websocket` 上' +
      '**双向**帧的次序：`"o"` → PROVISION(`command=3`?) → 客户端是否补了 UNREAD(`command=1`)；' +
      '服务端推送的前缀是不是 `a["`、客户端发的是不是 `["`；心跳回的是什么。',
    why:
      '⚠⚠ **这一条抓 CLI 是循环论证** —— 我们自己的帧只能证明"我发了我以为的东西"。' +
      '而原项目里有**两份互相矛盾的实现**（坑表 #117）：Go 跟的是被任务调用的 ' +
      '`tasks/live-room-kouling.ts`（心跳回 `"h"`、`command===3` 之后补 UNREAD），' +
      '而 `dist/…/live-room/client.js` 那另一份（心跳回 `[""]`、UNREAD 走 500 ms 定时器）' +
      '**没有被任何代码调用**。App 的实拍是**唯一**能判定"哪一份对"的东西。',
  },
]

// ─────────────────────────────────────────────────────────────────────────────
// 清单
// ─────────────────────────────────────────────────────────────────────────────

const ITEMS = [
  {
    id: 'live-now',
    title: '前置：门开着，且有「中国移动云盘」的直播间在播',
    probe: probeLiveNow,
  },
  {
    id: 'real-kouling',
    title: '一条**真口令**（当场从弹幕 / 小红书收到）',
    probe: probeRealKouling,
  },
  {
    id: 'journaling-optkeywords',
    title: '`visitlog/journaling` 其余 4 个 `optkeyword` 服务端认不认',
    probe: null,
    // ⚠ 2026-09-23 实拍：这一条**仍然没有答案**，而且**抓包答不了它**。
    evidence:
      '⚠ 2026-09-23 实拍**零样本**：抓包 201 条里 `visitlog` / `journaling` / `_flowerBtn` ' +
      '**全部零命中**，而**进房链路完整**抓到了（`cmpassLoginByCmToken` → `roomBeat` → ' +
      '`ws/info` → WS 升级 → `member/apply`）⇒ **它不在 cmlive H5 的进房链路上**。' +
      '⭐ 触发面是**活动站入口**或**点小红花按钮**；下次要造样本就从这两条路走。',
  },
  {
    id: 'member-apply-headers',
    title: '`member/apply` 的签名头与 body（⚠ WS 升级请求**不带** `systemmd5` / `tokenmd5`）',
    probe: null,
    verified: '2026-09-23',
    evidence:
      '✅ 实拍：`Version: v23.0.2`（原先标着"未实拍"）、body 里 **`message` 在**（空串）、' +
      '`deviceInfo: other/unknown`（**非 hex**）。' +
      '⭐⭐ 签名算式**实拍验证通过**（两条记录都逐字命中）：' +
      '`tokenMd5 = md5("Bearer " + t[:10] + ts10 + t[10:])`、' +
      '`systemMd5 = md5(**deviceInfo 头值字符串** + ts10)` —— ' +
      '⚠ 而 `SystemMD5` 强制 hex 解码，喂实拍值会当场报错（见 capture 文档 §8.4）。' +
      '⚠⚠ **WS 升级请求里没有 `systemmd5` / `tokenmd5`** —— 它们只属于 `/cmlive/api/*` 这一路。',
  },
  {
    id: 'ws-frame-order',
    title: 'WS 帧的真实顺序 / `command` 取值 / 「ack 之后必须补 UNREAD」',
    probe: null,
    verified: '2026-09-23',
    evidence:
      '✅ 实拍（133 帧 / 73.6 s）：`o` → C(`cmd=3`) → S(`cmd=3`) → C(`cmd=1`) → S(`cmd=1`) ' +
      '→ S(`cmd=2`) → C(`cmd=0`) → S(`cmd=0`) …；`command` 取值：' +
      '**0 = ack / 1 = 心跳 / 2 = 推送（弹幕）/ 3 = PROVISION**；' +
      '弹幕文本在 `MSync.9.2.6.4.2`（与 Go 的 `ExtractDanmakuText(cmd=2)` 路径吻合）。' +
      '⚠⚠ **`cmd=1` 是每 ~15 s 一次的周期心跳**（+608 / +15537 / +30665 / +45561 / +60583 ms），' +
      '**不是**"PROVISION 之后补发一次"；Go 只发一次（见 capture 文档 §七）。' +
      '⚠ **`h` 全程没出现**（SockJS 要静默 25 s 才发）⇒ "心跳回什么"**仍未判定**。',
  },
]

// ─────────────────────────────────────────────────────────────────────────────
// 报告
// ─────────────────────────────────────────────────────────────────────────────

const now = () => new Date().toLocaleString('sv-SE') // `YYYY-MM-DD HH:MM:SS`，本地时区

/** 把一次运行的原始输出与结论追加进报告（**追加**，不覆盖当天前几次）。 */
function appendReport(lines) {
  const header = fs.existsSync(REPORT)
    ? ''
    : '# 直播口令 / 小红花：开播后核对记录\n\n' +
      '> 由 `node scripts/probe-live-room.js` 生成，**追加**式。\n' +
      '> ⚠ 清单的权威版本在**脚本里**（`ITEMS`），这里只是**记录**。\n' +
      '> ⚠ 口令**当场有效**，所以每次记录都带时刻 —— 没有时刻的样本事后没法判"是不是过期了"。\n'
  fs.mkdirSync(path.dirname(REPORT), { recursive: true })
  fs.appendFileSync(REPORT, header + '\n' + lines.join('\n') + '\n')
}

/** 把一次 CLI 调用的原始输出收进详情块（报告里要留**原文**，不是我的转述）。 */
function rawBlock(result) {
  if (!result) return []
  return [
    '',
    '```',
    `$ mcloud ${result.argv.join(' ')}`,
    // ⚠ 写明**主体**：同一份报告里还会出现**脚本自己**的档位（每节开头的
    // 「脚本退出码」）。两个数含义不同 —— CLI 的 `0` 只表示"它正常跑完了"
    // （"今天没开播"也是 0），与脚本的 0/1/2 **不是一回事**。
    `CLI 退出码 ${result.code}（**CLI 的**，不是脚本的档位；CLI 的 0 = "正常跑完"）`,
    '--- stdout ---',
    result.stdout.trim(),
    '--- stderr ---',
    result.stderr.trim(),
    '```',
  ]
}

// ─────────────────────────────────────────────────────────────────────────────
// 主流程
// ─────────────────────────────────────────────────────────────────────────────

function printChecklist() {
  const verified = ITEMS.filter((item) => item.verified)
  console.log('开播后要核的清单（共 %d 条）：\n', ITEMS.length)
  for (const [i, item] of ITEMS.entries()) {
    console.log(`  ${i + 1}. ${item.title}`)
    // ⭐ `verified` = **已实拍**（结论在 `evidence`，原始证据在 `docs/capture/`）。
    // ⚠ 它**不从清单里删** —— 服务端可能变，下次开播仍要**回归**一次。
    const mark = item.verified
      ? `✅ 已实拍（${item.verified}）—— 留作回归`
      : item.probe
        ? '✅ 可自动核'
        : '🔌 要抓包（见 --wire）'
    console.log(`     ${mark}`)
    if (item.evidence) console.log(`     ${item.evidence}`)
  }
  if (verified.length > 0) {
    console.log(
      `\nℹ 其中 ${verified.length} 条已于 ${verified[0].verified} 实拍` +
        '（结论见上，原始证据在 `docs/capture/app-live-ws-samples.txt`）' +
        '—— ⚠ 仍建议**回归**一次，服务端可能变。',
    )
  }
  console.log('\n自动核：node scripts/probe-live-room.js --run')
  console.log('抓包步骤：node scripts/probe-live-room.js --wire')
  console.log(`\n报告落盘：${REPORT}`)
}

if (!RUN && !WIRE) {
  printChecklist()
  process.exit(0)
}

if (WIRE) {
  console.log('抓包前置条件：\n')
  const checks = wirePreconditions()
  for (const c of checks) {
    console.log(`  ${c.ok ? '✅' : '❌'} ${c.what}`)
    if (!c.ok) console.log(`      → ${c.how}`)
  }
  console.log('\n要核的三条（每条的判据与"为什么 CLI 答不了"）：\n')
  for (const step of WIRE_STEPS) {
    const mark = step.verified ? `✅ 已实拍（${step.verified}）` : '🔌 待核'
    console.log(`  · ${step.title}  ${mark}`)
    if (step.evidence) console.log(`      实拍结论：${step.evidence}`)
    console.log(`      看什么：${step.lookFor}`)
    console.log(`      为什么：${step.why}\n`)
  }
  console.log(WHY_NOT_THE_CLI + '\n')
  console.log('抓包步骤（取证面 = App）：')
  console.log('  1. Reqable 里开抓包（或 `scripts/reqable-capture.py on`）')
  console.log('  2. 打开 `/Applications/mCloud.app`，登录，**进直播间并待够一段时间**')
  console.log('     （要它自己喊口令 / 自己握手 WS，别去动 Go 那侧）')
  console.log('  3. 停抓包并导出：scripts/reqable-capture.py har har/live-room-<日期>.har')
  console.log('     ⚠ 不加 `--all`：默认就按 `--app mCloud` 过滤，正是要的那一份')
  console.log('  4. 抽夹具：go run ./cmd/mcloud fixture extract \\')
  console.log('       --har har/live-room-<日期>.har --out test/fixtures/http')
  process.exit(0)
}

// ── `--run`：跑能自动跑的那几条 ──────────────────────────────────────────────

build()

const results = []
for (const item of ITEMS) {
  if (!item.probe) {
    results.push({ ...item, status: 'wire' })
    continue
  }
  console.log(`\n── ${item.title}`)
  const r = item.probe()
  console.log(`   ${r.status === 'ok' ? '✅' : '❌'} ${r.detail}`)
  results.push({ ...item, ...r })
}

// 前置不成立 ⇒ 后面那些"只在开播时才有意义"的项**不是失败，是不适用**。
// ⚠ 这个区分很重要：把"没开播"记成"核过、没拿到"会让下一次真的开播时以为已经核完了。
const notLive = results.find((r) => r.id === 'live-now')?.status !== 'ok'

console.log('\n══ 汇总 ══')
for (const r of results) {
  const mark = r.status === 'ok' ? '✅' : r.status === 'wire' ? '🔌' : '❌'
  console.log(`  ${mark} ${r.title}`)
}
if (notLive) {
  console.log('\n⚠ 前置不成立（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。')
}

// 退出码三档（判据见文件头）。⚠ 这里的 `attempted` 只数 **`missing`**：
// `wire`（要抓包的那三条）**不是"没拿到"，是"这一趟不跑"** ——
// 把它们算成失败的话，`--run` 单独跑就永远到不了 0，三档立刻退化成两档。
//
// ⚠⚠ 这几行必须在 `appendReport` **之前**：报告是这一趟唯一的留档，里面若没有
// 脚本自己的档位，将来读者只看得到"每项拿到没拿到"、看不出"这趟整体算不算过了"。
// 而 raw 块里还有一个**CLI 的**退出码，两个数混在一起更分不清。
const attempted = results.filter((r) => r.status === 'missing')
const exitCode = notLive ? 2 : attempted.length > 0 ? 1 : 0
const exitWhy = notLive
  ? '前置不成立（现在没在播）⇒ 后几条**不适用**，不是"核过了没拿到"'
  : attempted.length > 0
    ? `前置成立，但有 ${attempted.length} 条**试过却没拿到**（多半是主播这一段没喊 ⇒ 重跑即可）`
    : '这一趟会跑的项都拿到了（⚠ 要抓包的几条本来就不在这一趟）'

const reportLines = [
  `## 运行记录 —— ${now()}`,
  '',
  `参数：\`--db ${DB}\`${ACCOUNT ? ` \`--account ${ACCOUNT}\`` : ''} \`--listen ${LISTEN}\``,
  '',
  // ⚠ 与下面 raw 块里那个「CLI 退出码」**不是一回事**（主体不同），所以这里写明主体。
  `**脚本退出码：${exitCode}** —— ${exitWhy}`,
  '',
  '| 项 | 结果 | 说明 |',
  '| --- | --- | --- |',
  ...results.map((r) => {
    const mark =
      r.status === 'ok'
        ? '✅ 拿到'
        : r.status === 'wire'
          ? r.verified
            ? '✅ 已实拍'
            : '🔌 要抓包'
          : '❌ 没拿到'
    // `evidence` 兜在 `detail` 之后：抓包那几条没有 `detail`，但**有实拍结论** ——
    // 报告里若只写"（见 --wire）"，将来读这份留档的人就看不出它其实已经核过了。
    return `| ${r.title} | ${mark} | ${r.detail ?? r.evidence ?? '（见 --wire）'} |`
  }),
  '',
  notLive ? '⚠ **前置不成立**（现在没在播）⇒ 后两条**不适用**，不是"核过了没拿到"。\n' : '',
  ...results.flatMap((r) => (r.raw ? [`### ${r.title}`, ...rawBlock(r.raw), ''] : [])),
]
appendReport(reportLines)

console.log(`\n报告已追加：${REPORT}`)
console.log(
  '⚠ 拿到结果后回去**补记** `docs/status.md` 的「未验证清单」—— 清单在脚本里，' +
    '状态在文档里，两边都要动（坑表 #116：清单漏了不会报错）。',
)

// ⚠ 这两个块都只在**前置成立**时才打。前置不成立时那两条 `missing` 是**预期的**
// （"门没开 ⇒ 当然收不到口令"），上面那句「后两条不适用」已经说清了；
// 再打一遍「❌ 试过却没拿到的有 2 条」会**自相矛盾**，把"今天没开播"渲染成失败。
if (!notLive) {
  const pending = results.filter((r) => r.status === 'wire' && !r.verified)
  const verified = results.filter((r) => r.status === 'wire' && r.verified)
  if (pending.length > 0) {
    console.log(
      `\nℹ 这一趟不跑的还有 ${pending.length} 条（都要抓包，见 \`--wire\`）` +
        '⇒ 退 0 只代表**这一趟会跑的**都拿到了，**不等于**整份清单核完了。',
    )
  }
  if (verified.length > 0) {
    console.log(
      `\n✅ 另有 ${verified.length} 条**已实拍**（${verified[0].verified}，` +
        '见 `docs/capture/app-live-ws-samples.txt`）—— ⚠ 仍建议下次开播**回归**一次。',
    )
  }
  if (attempted.length > 0) {
    console.log(`\n❌ 试过却没拿到的有 ${attempted.length} 条：`)
    for (const r of attempted) console.log(`   · ${r.title} —— ${r.detail}`)
  }
}

// 退出码连**理由**一起打：三档里有两档（0 / 1）**只在开播时才会出现**，
// 所以到那天必须能一眼看出"是哪一档、以及它是不是该出现的档"。
console.log(`\n退出码 ${exitCode}：${exitWhy}`)
process.exit(exitCode)
