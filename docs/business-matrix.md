# 移动云盘业务清单 —— `mcloud-sign`（TS 参考实现）× 本仓（Rust 后端 / WebUI）

> 本报告由 Qwen3.8-Flash 提供。
>
> **这份表只管一件事**：每条业务在「TS 参考实现 / 本仓 Rust 后端 / WebUI」这三层各自**有没有**、
> 有什么**证据**。单条任务的接口规格与实拍证据仍在 `docs/task-inventory.md`（那份管规格，这份管状态）。
>
> ⚠ **WebUI 的验收由用户本人做**。所以本表 WebUI 那一列只写"界面上有没有入口"，
> **一律不写"已验收"**；机器门禁绿灯也**不等于**验收 —— 留给你的核对清单在 §5.3。

## 0. 基数与口径（别信我抄的数，跑这三条）

| 数 | 现算命令 | 今天（2026-10-03，HEAD `fae794b`）|
|---|---|---|
| 本仓注册了几个任务 | `grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs` | **20**（`lib.rs:74-95`）|
| 其中几个对 WebUI 隐藏 | `grep -rn "fn hidden" crates/mcloud-tasks/src \| wc -l`（真实任务里覆写了没有；trait 默认值在 `mcloud-engine/src/registry.rs:165`，另一次命中 `:282` 是测试桩，都不算）| **0** ⇒ 20 条全可见 |
| TS 侧权威清单 | `sed -n '1,40p' /Users/catlair/Documents/workspace/mcloud-sign/TASKS.md` | 26 项「已完成」+ 14 个 appTask 执行器 + 8 条 click-only（HEAD `3bce0bb`，2026-09-12）|

**图例**：✅ 有 ／ ◐ 有但只做了一部分 ／ ✗ 没有 ／ ❓ 本次没能证实。
**证据分层**：`向量` = 拿 TS 真跑生成的黄金向量逐条对拍 ＞ `实拍` = 注释里有真机读数 ＞ `单测` = 有 `#[test]` ＞ 空 = 本清单未证。
⚠ `单测` 那一列是**模块级**（`signin.rs` 一个模块管 8 条任务，数不能摊到单条）。

## 1. 主表：本仓 20 条 × TS × WebUI

### 1.1 签到族（`crates/mcloud-tasks/src/signin.rs`，8 条一个模块；模块合算 37 个测试 / "实拍"12 处）

| 本仓任务名 | 标题 | TS 侧 | Rust 后端 | WebUI 有没有入口 |
|---|---|---|---|---|
| `sign` `:70` | 每日签到 | ✅ `signIn`（CLI 调度）| ✅ | 任务页卡片，无参数 |
| `receive` `:167` | 领取AI豆 | ✅ `receiveTask` | ✅ | 同上 |
| `multiple` `:319` | 签到翻倍 | ✅ 在 TS `sign/task.ts:101` 那段（`TASKS.md` 没单列）| ✅ | 同上 |
| `backup-gift` `:468` | 备份好礼 | ✅ `backupGiftTask` | ✅ | 同上 |
| `revival` `:393` | 复活奖励 | ✅ `revivalRewardTask`（本地放行 **2 次/月**）| ✅ **每次运行盲打一发**，靠服务端"本月已领取"做闸（理由 `signin.rs:381-388`）| 同上；⚠ 界面文案是"每月一次"，读起来和行为不同 |
| `prizes` `:544` | 待领取奖品 | ✅ `printPendingPrizes`（独立调用）| ✅ 只读 | 同上 |
| `cloud-snapshot` `:612` | AI豆快照 | — TS `TASKS.md` 无；出处是 **Go 参考实现**的新能力（差异表 D4「已确认要做」）| ✅ 只读观测 | 同上 |
| `task-list` `:663` | 任务清单 | — TS `TASKS.md` 无；出自 **Go 参考实现**那张能力表第 7 条（`task-inventory.md:194`，Query 类）| ✅ 只读观测 | 同上 |

### 1.2 领取 / 活动族

| 本仓任务名 | 标题 | TS 侧 | Rust 后端 | WebUI 有没有入口 |
|---|---|---|---|---|
| `msg-push` `msg_push.rs:331` | 消息推送 | ✅ `msgPushOnTask` | ✅ | 任务页卡片 |
| `notice` `notice.rs:649` | 开启通知领AI豆 | ✅ `noticeTask` | ✅（先查两个槽再领）| 任务页卡片 |
| `task-expansion` `task_expansion.rs:692` | 任务拓展 | ✅ `taskExpansionTask`（含文件上传）| ✅ **会写网盘**，传的文件跑完自删 | 任务页卡片 |
| `exchange` `exchange.rs:1753` | 兑换奖品 | ✅ `exchangeTask` + `exchange` + `printExchangeList` | ✅ 单测 75 / 实拍 36 / 黄金向量 3 | **专属页** `/exchange`（`nav.ts` 第 4 条）+ 任务页参数 4 个；⚠ 下单只走专属端点，见 §4 |
| `ai-avatar` `ai_avatar.rs:112` | AI 头像 | ✅ `aiAvatarTask`（按配置）| ✅ 但账号级开关 `ai_avatar_enabled` **默认 false**（`account.rs:264`）⇒ 不开就永远 `Skipped`（`ai_avatar.rs:840`）| 任务页卡片；**开关那一块 UI 当前不显示** ⇒ 见 §6-1 |
| `cloud-phone-redpack` `cloud_phone_redpack.rs:82` | 红包派对 | ✅ `cloudPhoneRedpackTask`（按配置）| ✅ 开关默认 **true**（`account.rs:266`）| 任务页卡片 |
| `play-ai-special` `play_ai.rs:205` | 拍拍盘点 | ✅ `playAiSpecialTask`（会做活动里可自动化的那几条；"生成校园海报 33 / 体验AI相机 34"是它**设计内跳过**，见 TS `TASKS.md` §运行观测）| ◐ **本仓这条只盘点不代做**（自陈见 `description`：不做任务、不抽奖、不写网盘）⇒ 与 TS 同名不同物 | 任务页卡片；开关 `play_ai_enabled` 默认 true，同样只在隐藏块里 |
| `fun-ai` `fun_ai.rs:389` | 趣玩AI抽奖 | ✅ `playAiLotteryTask`（CLI 调度）| ✅ | 任务页卡片 |
| `live-room` `live_room.rs:1727` | 直播口令小红花 | ✅ `liveRoomKoulingTask` + `getLiveKouling` + `hasActiveLiveRoom`（WS 与小红书主页**并行**后合并去重）| ✅ 两个来源**同级并发**（`collect_inner` `live_room.rs:1005-1015`），口令解析层对拍 TS 的 `koulingExtract` 36 条 + `xiaohongshuExtract` 5 条（`live_kouling/tests.rs:68-84`）；开关 `kouling_enabled` **默认 false** | **专属页** `/live-room`（口令池 + 参数 `codes`）+ 任务页；⚠ 同 §6-1 |
| `red-flower` `live_room.rs:1847` | 领取小红花 | ✅ `drawRedFlower` / `autoDrawRedFlower` / `drawRedFlowerForAccount` | ✅ 独立一条（TS 那边是 3 个函数）| 任务页，参数 `codes` |
| `app-task` `app_task.rs:791` | App 任务（做任务）| ✅ `appTask`（14 个专用执行器 + `default:` 打卡）| ◐ **只做 4 档形状**：106 / 522 / `beiyong1` 名字门 / 434（`upload_shape_for` `:105-124`、`step_for` `:203-207`）⇒ 详见 §3 | 任务页卡片 |
| `app-task-matrix` `app_task_matrix.rs:243` | App 任务矩阵（只看）| — TS 无"只看"这一条 | ✅ 只读（2 市场 × 8 组）| 任务页卡片 |

## 2. 反向缺口：TS 有、本仓没有

⚠ **"没做"分两种**，别混着记账：**缺口** = 从没讨论过；**有口径的不做** = `docs/task-inventory.md` §3 差异表
（D1–D6）里已经拍过板。下面两列标出来了。

| TS 侧 | 它的状态 | 本仓 | 定性 / 影响 |
|---|---|---|---|
| `hc1tTask` 云朵大作战（TS 叫"AI豆大作战"）| ✅ 已实现并按配置调度（`packages/core/src/tasks/hc1t/`）| ✗ 无任务，而且**设置里那三个 `hc1t_*` 旋钮也被删了**（`account.rs:189-191`，老库残留键仍读得回：测试 `:777`）| **有口径的不做** —— `docs/task-inventory.md:355`「已过期，不实现，只作形态参考」＋差异表 D3。将来真要做得连字段一起加回来 |
| 139 邮箱族：`472/473` 体验邮箱、`1004` 发邮件、`1014` PDF 转换、`1020` 邮箱备份、`1028` AI 工作台 | ✅ appTask 执行器（靠 `auth-service loginMail` 拿 session）| ✗ `mail139` / `139邮箱` 全仓 0 命中 ⇒ 连 session 层都没有 | **缺口**（差异表里没有这一行）；**§3 那半张表的主要成分**。TS 2026-09-12 实跑里 `1020` 报 404、`473` 结果不稳定 ⇒ 这条链上游本身在动 |
| `107` 创建云笔记、`113` PC 客户端、`409` / `1005` 云朵中心页面访问 | ✅ appTask 执行器 | ✗ 无 runner（`113` 只在上传档位留了痕：`app_cloud_upload.rs:101`）| **缺口**；同上 |
| `585` AI 相机、`587` 上传 10 张图片 | ✅ 专用 runner（经 appTask）| ✗ 本仓 `app-task` 的 4 档里没有这两个 id | **缺口**；同上 |
| `afterTask` 任务后处理（清理临时文件）| ✅ 独立任务 | ✗ 无独立任务；**等价动作内联**在 `app_task.rs:751` `cleanup()`（把自家传上去的移进回收站）| **有口径的不做** —— 差异表 D1「`after` 不是独立能力（无 code）」⇒ 只是没有"单独调度它"这个入口 |
| `auth-service` 邮箱登录/刷新作为可调度能力 | ✅ | ◐ CLI 有 `auth refresh`（`cli/run.rs:116`），但那是**票**的刷新，不是邮箱 session；而 P0-2 那条 `authTokenRefresh.do` 标着「零抓包的旧 TS，**刻意不实现**」（`task-inventory.md:223`）| **有口径的不做**（那一半）＋**缺口**（邮箱 session 那一半）|
| 推送 10 条渠道（pushplus / serverChan / workWeixin / workWeixinBot / tgBot / bark / twoIm / dingTalk / email / customPost）| ✅ `shared/src/push`，10 条都发得出去（email **仅 Node**，txiki 下只 warn）| ✅ 同名 10 条都有模型（`mcloud-core/src/push.rs:385-411`）、**9 条真发**（`mcloud-app/src/push.rs:935-966`）；**email 有配置项但刻意未实现** —— 返回 `ChannelOutcome::NotImplemented` 并记一条 warn（`push.rs:905-926`，理由：不引 SMTP 依赖）| **不是缺口，是"可观测的未实现"**：配了 email 会看见 warn，不会静默"以为发了"。⚠ 而且「运行结果推送」那一块与「功能开关」是**同一个开关挡住的**（`accounts-page.tsx:1600`）⇒ 十条渠道在界面上今天**一个都配不了**，只能走那条 PUT |

**双方都不做**（TS 自己也标着"未实现/已过期"，不是本仓的账）：
`588` 出行攻略、`589`/`1069` 九宫格、`601` 端午写真、`602` 旧照片修复（TS 标 P2 未实现）；
`cloud-day` 云日、`knowledge` 知识库、`query-bill` 查账单（TS 标 P3 未迁移）；
`signInWx` / `wxDraw` / `shakeTask`（TS 标"已过期，移出待办"）。

## 3. appTask 逐条对照（**本清单最大的一块缺口**）

TS 的形状是「runner 表 + `default:` 分支打卡」；本仓是「过三道门 → 只挑得出 4 档形状的条目 → 打卡 → 做那一件事」。
关键差别在**没形状的那些条目**：本仓主循环先 `filter_map(|i| step_for(i))`（`app_task.rs:543-549`），
所以判不出形状的条目**连 `task/click` 都不发** —— TS 的 `default:` 分支会对它们打卡（黑名单 `SKIP_TASK_LIST` 那 11 个 id 除外，本仓照抄：`app_task.rs:157`）。

| 玩法 id | TS 怎么做 | 本仓 |
|---|---|---|
| 106 上传文件 | runner：文件上传 | ✅ `UPLOAD_ONE` |
| 522 上传 100 个 | runner：分批循环 | ✅ `{5, 100, pace}` 档 |
| 434 分享文件 | runner：建并删外链 | ✅ `Step::Share`，且只在第 22 步打卡（`SPECIAL_CLICK_STEP`）|
| `beiyong1` 分组 | `switch (task.groupid)` | ✅ 名字门 → `UPLOAD_ONE` |
| 115 / 431 / 435 / 1009 / 1010 / 1013 / 1016 / 447 | **click-only**（框架自动打卡）| ✗ **不发卡**（不在 `planned` 里）|
| 107 / 113 / 409 / 1005 / 472 / 473 / 1004 / 1014 / 1020 / 1028 / 585 / 587 | 12 个专用 runner | ✗ 全部没有 |
| 117 / 437 / 991 / 406 / 385 / 480 / 1022 / 1018 / 603 / 1021 / 120 | TS 刻意跳过 | ✅ 同一串黑名单，逐字照抄 |

⚠ 这一栏给的是"有没有"，不是"该不该"。`task/click` **不可回滚**，
所以"要不要给 click-only 那批发卡"是一个需要用户点头的产品口径，不是可以顺手补的实现。
（另外：`docs/task-inventory.md` 里那句"已接的有 106/107/…/587"**说的旧项目**，
不是本仓 —— 已在那份文件里就地加了读法说明；它对应的差异登记在差异表 **D6**「`app` 登记成一条（刻意过渡）」。）

## 4. 四条入口 × 可达性（`exchange` 这一格最容易看错）

| 入口 | 走哪段代码 | 参数怎么给 | 能下单吗（`exchange`）|
|---|---|---|---|
| 兑换页「兑换」按钮 / 定时抢兑 | `POST /api/accounts/{id}/exchange`（`routes/exchange.rs:547-560`）| 页面表单 | ✅ **唯一**会注入 `deviceId` 的一条（取 `smid.device_id(...)`，注释明说"不是 `account.device_id`"）|
| 任务页跑一条 / `POST /api/runs` | `routes/runs.rs::create` → `runner.run` | `params()` 声明的那几个字段 | ✗ 拿不到 `deviceId` ⇒ 如实 `device_id_failed`、**一枪不发** |
| 定时任务 | `scheduler.rs:446` `runner.run(&sch.task, &acc, &sch.body)` | 排期里存的 JSON | ✗ 同上（除非 body 里手填 `deviceId`，而它是设备指纹换来的）|
| 编排 / CLI `mcloud run … --body` | `pipeline.rs` / `cli/run.rs:154-161` | 步骤 body / `--body` JSON | CLI 手填 `deviceId` 才能发；不填就是安全阀那条结论 |

⇒ 判据是 `exchange.rs:1804-1817`：**`deviceId` 从请求体来，没有 ⇒ 一枪不发**（这是刻意的安全阀，不是 bug）。
其余 19 条任务不依赖注入点，四条入口都等价。

## 5. 验收情况

### 5.1 Rust 侧证据（现算：`grep -c '#\[test\]\|#\[tokio::test\]' <模块>`）

| 模块 | `#[test]` 数 | "实拍"注释 | 黄金向量 |
|---|---|---|---|
| `signin.rs`（管 8 条）| 37 | 12 | — |
| `live_room.rs`（管 `live-room` + `red-flower`）| 58 | 18 | 口令解析层对拍 TS 36+5 条（`live_kouling/tests.rs`）|
| `exchange.rs` | 75 | 36 | 3 处 |
| `app_task.rs` | 36 | 2 | — |
| `notice.rs` | 30 | 29 | 1 |
| `msg_push.rs` / `ai_avatar.rs` | 各 23 | 12 / **0** | — / 1 |
| `task_expansion.rs` | 22 | 4 | — |
| `app_task_matrix.rs` | 18 | 4 | — |
| `play_ai.rs` | 17 | 14 | — |
| `cloud_phone_redpack.rs` | 15 | 1 | 1 |

⚠ 这张表要说清它**不**说明什么：单测多≠真账号跑通过。本清单**没有**逐条证实"哪条任务今天真在账号上跑成功过"
—— TS 侧那次 2 账号完整跑通（`TASKS.md` §运行观测，2026-09-12）是 **TS 的**记录，不能记到本仓账上。
`ai-avatar` 那一格"实拍 0"值得单独留意：它的模块注释里没有真机读数。

### 5.2 机器门禁断言了什么、**没**断言什么

`node scripts/gates-web.mjs --full` 六条：typecheck、build、lite 编译器 13 条负例、
窄屏布局（320/375 × 9 路由溢出 + 1440 版式）、真 DOM 交互（静态步骤 132 条）、泄漏量测。

⚠ **它没断言任务面**：夹具 `/api/capabilities` 只回 **4 条**假任务
（`daily-checkin` / `receive` / `live-room` / `msg-push`，见 `web/lite/regress/lib.mjs:231-259`）
⇒ 20 条里**只有 3 条真名**（`receive` / `live-room` / `msg-push`）被脚本碰过，**其余 17 条的参数表单与卡片没有任何脚本跑过**，
而且那 4 条里有一格 id 是假的（`daily-checkin` ≠ 真的 `sign`）。
所以"门禁全绿"覆盖的是**框架与布局**，不是**业务清单**。

### 5.3 留给你的 WebUI 验收清单（我不打勾）

下面每条都是"看哪页 → 点什么 → 预期看到什么"。前两条是我读码推断出的**一定会卡住**的地方，最能一次验出真相。

| # | 页面 | 动作 | 预期（按代码应当看到）|
|---|---|---|---|
| 1 | 任务 | 选账号点 `ai-avatar`「运行」| 结论 `Skipped`、理由含「AI 头像已在账号设置里关掉」⇒ 若如此，去第 2 条 |
| 2 | 账号 → 打开某个账号的设置 | 找「功能开关」这一块 | **当前找不到**（`accounts-page.tsx:143` 那块不显示）⇒ 4 个开关在界面上无入口，只能用 `PUT /api/accounts/{id}/settings` 改 |
| 3 | 直播 | 不填 `codes` 直接起一场 | 看它是否自己去抓小红书主页（§6-4 说代码里两源都做）；抓不到时页面怎么陈述 |
| 4 | 兑换 | 点一件奖品的「兑换」并确认 | 真扣豆（**不可逆**，建议先看余额截图 → 兑 → 再看余额）；结果卡上有没有 `ordered`/`already-exchanged` 这类结论 |
| 5 | 兑换 | 把「跳过等待」关掉、目标设在整点 | 任务自己等到整点才发枪（2026-10-03 review §1.1 修的就是这一条，需要真账号才能闭环）|
| 6 | 任务 | 在任务页把 `exchange` 当普通任务跑 | 应当**不发单**、结论 `device_id_failed`（§4 那一格）|
| 7 | 任务 | `app-task` 跑一趟 | 步骤里只应出现 106 / 522 / `beiyong1` / 434 这几档；矩阵里其他条目**不动**（§3）|
| 8 | 定时 | 建一条 30 秒内的排期 | 到点真触发、日志页能看到进度；重启服务后 `run_count` 不被清零 |
| 9 | 编排 | 建两步链，第一步故意失败 | 「失败继续 / 中止」两种配置表现不同；`preflight` 说人话 |
| 10 | 历史 | 翻 `live-room` / `exchange` 的旧记录 | 明细能展开（口令逐条落点 / 兑换结论），不是只剩一行小结 |
| 11 | 窄屏 375 | 任一条任务跑起来后往上翻日志 | 不该被弹回底部（review §2.1 改的就是这个）|

## 6. 五条"看着在、实际另有一番样子"（都带落点，可自己核）

1. **4 个账号级开关在界面上没有入口**。判据：`account.rs:264-268` 默认 `ai_avatar=false`、`kouling=false`；
   开关块在 `accounts-page.tsx:143` 被 `SHOW_ACCOUNT_SETTINGS_BLOCKS = false` 挡掉，用在 `:1600`。
   ⇒ 点 `ai-avatar` / `live-room` 只会得 `Skipped`（`ai_avatar.rs:840`、`live_room.rs:1771`），
   而"打开它"这件事今天只能 `curl PUT /api/accounts/{id}/settings`（`routes/accounts.rs:528`）。
   ⚠ 那条 PUT 是**整体替换**：DTO 带 `deny_unknown_fields` 且**没有字段级默认**，
   少给一个字段是 **400**（`account.rs:413-414`），**不会**把漏掉的那个静默重置成默认 ⇒
   正确用法是"先 `GET` 整份 → 改开关 → `PUT` 整份"。Web 端 `putAccountSettings`（`api.ts:1339-1343`）
   发的就是整份，所以**从界面改别的设置不会把开关清零** ✓。
2. **`play-ai-special` 同名不同物**。TS 那条会**代做**活动任务；本仓的 `description` 自己写着
   「只盘点不代做：不做任务、不抽奖、不写网盘」（`play_ai.rs:212`）。⇒ 拿 TS 的清单核对时会误以为已对齐。
3. **click-only 那 8 条不打卡**（§3）。界面上 `app-task-matrix` 看得到它们，`app-task` 却一条都不动。
4. **注释与实现相反的一处**：`live_room.rs:18` 与 `cli/run.rs:151` 都说"小红书那条口令没法自动获取、
   `codes` 是唯一入口"，但同文件 `:992-1015` 真的会 GET 小红书主页并提取口令，且与弹幕**同级并发**。
   两处出自同一个提交（`c4f7f00`）。⇒ 谁对？要靠一场真直播验（§5.3 第 3 条），别先改代码。
5. **email 推送渠道能配不能发**（本仓**刻意**留空且如实报）。`PushConfig` 里它有字段、界面块里有它，
   但 `send_email` 只记一条 warn 并返回 `ChannelOutcome::NotImplemented`（`mcloud-app/src/push.rs:905-926`）；
   理由写在那段注释里（不引 SMTP 依赖）。⇒ 这一条**不是**"静默不发"那种最坏形状，
   但**别把它当已实现**：TS 那边 email 在 Node 下是真发的。

## 7. 这份清单**没**覆盖的（别照它下结论）

- 没有逐条做真账号活体核验（哪些今天真能跑通，本表**不下判断**）。
- 登录族（扫码 / 短信 / 账号密码 / 设备）本次没查。
  ⚠ 倒是顺手核掉了一条：那条整体替换的 PUT **不会**把没给的字段静默重置成默认（少字段是 400），见 §6-1 ✓
- `docs/task-inventory.md` 开头那段"2026-09-26 已实现 10 个任务"是**当日快照**（今天 20 条）⇒ 本次只在它上面加了
  **分工与现算命令**，§3 那句"已接的有…"也加了"说的是旧项目"的读法说明；**它那份接口规格与 `path:line` 证据仍然有效**，
  本次**没有**动它的实质内容（状态看本表，规格看那份）。
