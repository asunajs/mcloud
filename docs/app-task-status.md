# appTask（App 任务）· 实现 / 自动验证 / 手动验收

> 这张表只讲**本项目**的三件事：**实现进度**、**自动验证结论**、**你的验收**。
> ⚠ **账号侧的状态（`WAIT` / `FINISH` 之类）不是本项目的进度** —— 它是上游自己的记账，
> 只作**证据输入**，一律放在 §6 附录，不出现在主表里 ✗。
>
> 数据来自**只读**那一档：**「上游状态」页**（`GET /api/upstream/app-task-list`）现取一份
> App 任务表，不打卡、不上传 ✓。
> ⚠ 2026-10-07 之前那是一条独立任务（`app-task-matrix`）—— 用户当场否了
> （「app-task-matrix 这个任务本身就没有意义」），它现在**不是任务**了：那张表由
> **页面按需去取**（用户同一天的口径：「需要主动获取，而不是跟着其他任务一起」）✓。

## 0. 进度总览（项目口径）

| 实现状态 | 条数 | 是哪几条 |
|---|---|---|
| ✅ **已实现**（会真做：上传 / 分享 / 固定入口） | **7** | 106、113、409、434、522、615、1005 |
| ✅ **已实现**（判定门：按参考**刻意不动**） | **2** | 120（与参考一致 ✓）、1021（**与参考一致** ✓ —— 矩阵里不做，而它的真身在**另一条任务** `notice` 的邮箱那一半，2026-10-06 已落码 ✓，见 §5-2） |
| ✅ **无需实现**（上游已完成 ⇒ 门挡住即可，行为已正确） | **1** | 118 |
| ❌ **未实现** | **15** | 见 §2 主表 |
| **合计** | **25** | 活列表基数（2026-10-04 16:00:17 拉取，只读，落 `data/history.json` 的 run 93） |

⇒ **项目口径完成度：10 / 25**（有明确实现且行为可判），其余 **15 条未实现**。

## 1. 五道关（纯代码事实，与账号无关）

先过门、再判形状；顺序反了不改结果，但会把"不该动的"算进计划里 ✗。

> ⚠ **落点这一列刻意只给符号名**（2026-10-04 改）：旧版钉的是 `:行号`，而 `app_task.rs` 一直在长 ⇒ 整列系统性指错地方 ✗。要行号就现算，例如 `grep -n 'pub fn step_for' crates/mcloud-tasks/src/app_task.rs`。

| # | 关 | 判据 | 落点（`crates/mcloud-tasks/src/app_task.rs`，**给符号名**） | 它决定什么 |
|---|---|---|---|---|
| 1 | 上游已记完成 | `state == "FINISH"` | `should_click()` | 上游说这条做完了 ⇒ 不再动它 |
| 2 | 黑名单 | `SKIP_TASK_LIST`（11 个 id，逐字照抄参考实现） | `SKIP_TASK_LIST` | 这些 id 的 `task/click` **有副作用或无意义** ⇒ 一律不打 |
| 3 | 本账号 `skipTasks` | `AccountSettings::skip_tasks` | `is_configured_skip()` | 每账号自己的跳过名单 |
| 4 | 月门 | `market=sign_in_3` **且** `group=month` **且** 本月备份为 `false` **且** 今天 < 20 号 | `month_gate_skips()` | 没开备份时，20 号前不做 app 的月任务 |
| 5 | 形状判定 | `step_for` 只认「上传 / 分享 / 固定入口」；**判不出 ⇒ 一枪不发** | `step_for()` | ⇒ **这条是那 15 条未实现的直接原因**（见 §5-1；⚠ 早先写的 17 把 2 条**刻意不做**也算了进去 ✗） |

## 2. 主表（25 条，**项目口径**）

图例：实现 —— ✅ 已实现 ｜ ⛔ 刻意不做（黑名单）｜ ✅门 判定门 ｜ ❌ 未实现；
自动验证 —— ✅ 有断言 ｜ ⚠ 仅"黑名单字面量/反例"级 ｜ ❌ 无。

| taskId | 名称 | 实现 | 落点（`app_task.rs`） | 自动验证 | 手动验收 |
|---|---|---|---|---|---|
| 106 | 手动上传一个文件 | ✅ 已实现：上传·单件 | `UPLOAD_ONE` · `upload_shape_for()` | ✅ 单测 26 处；**真机闭环** ✓ —— 17:06:18 小号 `9f611bba8206ca0a` 跑出 `{taskId:106, clicked:true, uploaded:1, problems:[]}`（run 96 ✓），17:07:52 矩阵 `state=FINISH`（run 99 ✓，**这次 1.5 分钟就入账**，不像上次等了 76 分钟） | ☑ **已验收**（2026-10-04 用户实跑 ⇒ 上游认账 ✓） |
| 113 | 使用PC客户端 | ✅ 已实现：上传·单件，但**按 PC 档发**（`channelSrc=10200153` ✓） | `UPLOAD_ONE_PC`（`upload_shape_for()`，`channel: CloudChannel::Pc` ✓）；渠道头 `app_cloud_upload::cloud_file_op_headers()`（**2026-10-04 加**，见 §5-7） | ✅ 单测 2 处（形状✓）+ 运行级 1 条（**逐枚头断言** PC 档的 4 枚 + 断言安卓档那 2 枚**缺席** ✓，退回安卓档即红 ✓） | ☑ **已验收**（run 108，2026-10-04 20:07，测试号 `onlyTaskIds=113`：`clicked:true` ✓ `uploaded:1` ✓ `problems:[]` ✓ ⇒ run 109 矩阵 **`WAIT` → `FINISH`**、`currStep 0 → 3` ✓，**1 分钟就入账** ✓） |
| 118 | 调查问卷 | ✅门 判定门（上游已记完成 ⇒ 不动） | `should_click()`（判定门） | ⚠ 该 id 仅作"不在黑名单"的反例（`app_task/tests.rs:793`） | ☐ 无需人工动作 |
| 120 | 30G流量0元领 | ⛔ 刻意不做（**与参考一致** ✓） | `SKIP_TASK_LIST` | ⚠ 仅黑名单字面量（`tests.rs:785`） | ☐ 无需人工动作 |
| 409 | 从固定入口访问云朵中心 | ✅ 已实现：固定入口（**两发上报**：新 `doTaskPost` + 旧 `page/doTask`；→ 停一拍 → **两发打卡**：先 `task`（`clickTaskWithStep` 那一发）→ 补 `task2`） | `Step::Entry` → `entry_reports()` + `click_with_key()` | ✅ 单测 5 处 + 运行级 1 条（断言两发上报的市场名与两发打卡的 key ✓） | ☑ **已验收**（2026-10-10，三账号：测试 `0f6050105ab111f4` run 293 `WAIT → FINISH` ✓、移动 `b73bb07d5971e8e7` run 295 ✓；**真因见 §5-8**） |
| 431 | 去使用移动云手机 | ❌ 未实现（两边都无 runner；**参考会打卡，本仓连打卡都不发**） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 434 | 分享文件领好礼（原始 name 带 HTML） | ✅ 已实现：分享本趟传上去的第一个文件、随即删外链；**手里空着就自己传一个** ✓；**成没成按"次数涨没涨"判**（`shareBefore`/`shareAfter`/`shareCounted`） | `share_first_uploaded()`；打卡门 `should_click()`；判据 `settle_share_progress` + `share_progress_from_name`；**计划里排末尾**（`sort_by_key(task_id == 434)` ✓）（**2026-10-04 修**，见 §5-7） | ✅ 单测 18 处 + 运行级 **5 条**（打卡门正反 ✓ + 次数正反 ✓ + **顺序/补传** ✓；三条红测都验过 ✓） | ☑ **已验收**（run 104，2026-10-04 19:37，测试号 `onlyTaskIds=434`：`uploaded:1` ✓ `shared:true` ✓ `0/7 → 1/7` ✓ `shareCounted:true` ✓ `acted=1`、success、文件已收回 1/1 ✓） |
| 522 | 当月上传文件满100个 | ✅ 已实现：上传·每趟 5 件、上限 100；**跑完报进度**（`progressBefore`/`progressAfter` + 日志一行 ✓） | `ROUNDS_PER_RUN` / `STOP_AT_PROCESS`；进度 `settle_upload()`（**2026-10-04 加**，见 §5-7） | ✅ 单测 16 处 + 运行级 **2 条**（进度正常 ✓ / 差额说明 ✓，都断言了**日志行** ✓，撤掉即红 ✓） | ☑ **已验收**（run 105/106/107，2026-10-04 19:55-19:58：日志打出 `48/100 → 52/100`、`53/100 → 58/100`、`58/100 → 63/100` ✓；明细两格同步 ✓） |
| 547 | 加入一个知识库 | ✅ 已实现：**打卡**（与 431 同档 ✓）—— 入口是 `mcloud://main/aiAssistantGlobal?params=<base64>` 深链（**取不了** ✗），打卡就当趟翻 ✓（见 §5-13） | `Step::ClickOnly`（`CLICK_ONLY_TASK_IDS` ✓） | ✅ 单测 1 条（在那一串里 ✓） | ☑ **已验收**（2026-10-11：测试 run 318 `WAIT → FINISH` ✓、移动 run 322 ✓；小号 `planned=0` ⇒ 没排进计划） |
| 548 | 开启备份领好礼 | ❌ 未实现（⚠ 它在 `time` 组，**不是**月门那条） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 550 | 开启云盘APP通知领好礼 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 551 | 连续31天开启云盘APP通知 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 604 | 去云盘图书馆阅读书籍 | ✅ 已实现：与 1017 **同一档**（页面初始化上报 + 取入口页 + 打卡）—— 差别是入口那一格在 **`button.app.link`**（不是 `email` ✓）、且 `currstep = 0` ⇒ **不补第二发**打卡 ✓（见 §5-12） | `Step::PortalEntry` → `entry_reports()` + `portal_entry()` + ③-a 那道门自己那一发 | ✅ 单测 2 条（分档 ✓、**只打一发** ✓）+ 上游 1 条（入口页解析同 1017 ✓） | ☑ **已验收**（2026-10-10：测试 run 313 `WAIT → FINISH` ✓、移动 run 314 ✓；小号 run 315 早已 `FINISH` ⇒ 三档都在） |
| 607 | 下月签到领惊喜福利 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 609 | 和AI助手对话 | ✅ 已实现：**打卡不够**（run 318 `WAIT → WAIT` ✗）⇒ 接上 **AI 对话链**（派生 AI 鉴权 → 状态查询（`taskId=609`）→ SSE 对话 → 复查 ✓）—— 真机读数钉住"翻账靠对话"：对话前 `taskStatus=0 / cloudStatus=-1`，对话后 `1 / 0` ✓（见 §5-13） | `AppRunner::AiAssistant` | ✅ 单测 1 条（映射 ✓）+ 真机读数（对话前后两格对照 ✓） | ☑ **已验收**（2026-10-11：测试 run 319 `WAIT → FINISH` ✓、移动 run 320 ✓；小号 `planned=0`） |
| 615 | 记录国庆节欢乐瞬间 | ✅ 已实现：`beiyong1` 的**按钮门** ⇒ 一次传 **10 张 `.jpg`**（`process` 已走的先扣掉 ✓） | `beiyong1_button_gate()` / `UPLOAD_TEN`；形状四格 `rounds=10 / stop_at_process=10 / per_round_ms=300 / file_ext=".jpg"`；门读 `button_app_link`（`app_task_list_v2.rs` 的 `button_app_link`，**2026-10-04 加**，见 §5-7） | ✅ 单测 4 处（门 3 + 解析 1）+ 运行级 **2 条**（10 张 ✓ / 补差额 ✓；都断言了 `.jpg` 与**进度分母** ✓，撤门或撤"剩余量"即红 ✓） | ☑ **已验收**（run 110，2026-10-04 20:30，测试号 `onlyTaskIds=615`：`clicked:true` ✓ `uploaded:10` ✓ `problems:[]` ✓ `收回 10/10` ✓ ⇒ run 111 矩阵 **`WAIT` → `FINISH`**、已完成 3 → 4 ✓；**当场入账** ✓） |
| 617 | 体验智能消除 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 618 | 体验AI相机 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1004 | 给好友发邮件 | ❌ 未实现（参考 **`sendMailTask`**，TS `runners.ts:154-159`；Go 侧记在 `toolGaps`，`dispatch.go:490`；本仓 **mail139 整族未实现**） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1005 | 从固定入口进入云朵中心 | ✅ 已实现：固定入口（**两发上报**同上；打卡 key = **`task2`** —— `currstep` 恒为 1 ⇒ 上面那道"先打卡"门永远不打，所以它只发得出**一发**，那一发必须是 `task2`） | `Step::Entry` → `entry_reports()` + `click_with_key()` | ✅ 单测 5 处 + 运行级 1 条（同 409 那条一起钉 ✓） | ☑ **已验收**（2026-10-10，三账号：测试 run 294 `WAIT → FINISH` ✓、移动 run 295 ✓、小号 `9f611bba8206ca0a` run 296 ✓） |
| 1014 | 体验“PDF转换”功能 | ❌ 未实现（参考 **`pdfConvertTask`**，TS `runners.ts:168-173`；Go `MailPDF`，`dispatch.go:422`；mail139） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1017 | 登录移动云盘云朵中心 | ✅ 已实现：**页面初始化上报**（`doTaskPost` + 旧 `page/doTask`，`activityid` 走本条任务的市场名）+ **取入口页**（`button.email.link` 里的 `#ssoToken#` 换成真票）+ **打卡**（key = `task2`）—— 缺那一格上报时连着三趟 `WAIT → WAIT` ✗，补上就当趟翻 ✓（见 §5-11） | `Step::PortalEntry` → `entry_reports()` + `portal_entry()` + `click_with_key()` | ✅ 单测 2 条（分档 ✓、四发顺序与占位替换 ✓）+ 上游 2 条（`button.email.link` 解析 ✓、`signInCallback` 形状 ✓） | ☑ **已验收**（2026-10-10：测试 run 309 `WAIT → FINISH` ✓、移动 run 310 ✓；小号 run 311 早已 `FINISH` ⇒ 三档都在） |
| 1020 | 完成一次邮件转存 | ✅ 已实现：**App 实拍的三发**（① `disk:getDirectorys` 拿落点目录 → ② `mbox:SearchByFidOrCondition` 拿一枚 `mid` → ③ `disk:backupMail` 转存）。⚠ 旧 TS 那条 `POST mail.10086.cn/mailsms/sms/backup` **实测 404**（那台主机上只认 `/login/inlogin.action`）⇒ 参考实现里这一条**没有活路**，形状只能来自 139邮箱 App 的字面量（见 §5-10） | `Step::Runner(AppRunner::MailBackup)` → `mail139::{cloud_directories_request, mail_search_request, backup_mail_request}` | ✅ 单测 3 条（三发形状 + 取不到 `mid` 的硬失败 + 业务码打回的硬失败）+ 上游 5 条（形状/取值器/形状提示） | ☑ **已验收**（2026-10-10：测试 run 299 `WAIT → FINISH` ✓、移动 run 300 `WAIT → FINISH` ✓；小号那条早已 `FINISH` ⇒ 三档都有） |
| 1029 | 查看"我的账单" | ⚠ **部分实现（形状齐、读数不动 ✗）**：2026-10-11 用户在小号真机走了一趟**真的翻账了**（面板「已完成 · 第 4 步」✓）⇒ 这一族**是能做成的** ✓；照那一趟逐格复刻出 `AppRunner::BillView`（**五发**：账单页 `mobilebill.html?sendId=1000006` → `mbox:searchMessages` → `mbox:updateMessagesStatus` → `view:readMessage` → 像素 `111631/261`，外加打卡的**后半发** `visitlog/journaling` ✓），在**移动**上跑 **run 379/380/381 全 `WAIT → WAIT`** ✗ ⇒ **差的不是形状**（响应体大小逐格对得上：页面 12013 / 正文 61959 字节 ✓）。**还剩的候选**（下一轮一次只加一格）：`encryptType=1` 版 search / `getCommConfig` 带 `behaviorData=108365_66` / `disk:mailReportToMQ` / 账号侧（小号是重庆移动）。依据与实拍见 **§5-14**、**§5-16** | — | ✅ 单测（`visitlog` 2 条形状钉子 + `runners` 14 条 ✓；⚠ 旧的"断言它没有执行器"那条已随实现撤掉 ✓） | ☐ **未验收**（移动仍 `WAIT` ✓；**小号已翻 ⇒ 月限不可重试** ✗） |
| 1021 | 连续31天开启邮箱APP通知 | ⛔ 矩阵里**不做**（**与参考一致** ✓）—— 它的真身是**另一条任务** `notice` 的**邮箱那一半**（2026-10-06 已落码 ✓，见 §5-2） | 矩阵不碰它；实现在 `crates/mcloud-tasks/src/notice.rs` + `crates/mcloud-upstream/src/app_notice.rs` | ✅ `cargo test -p mcloud-tasks --lib notice`（6 条邮箱钉子）+ `cargo test -p mcloud-upstream --lib app_notice` | ☐ 跑一次 `notice`，看明细里那条 `noticeEmail`（`day` / `canReceive` / `reported`） |
| 1057 | 体验AI工作台 | ❌ 未实现（⚠ **参考侧也没有 runner**：TS 全仓 **0 命中 1057**；Go 只把它归成 `VerifyStillOpen`「需前往邮箱完成」（`task_matrix_report_test.go:123`，组 `cloudEmail`）⇒ 不是「漏做」） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |

## 3. 自动验证（四层，各自能证明什么）

| 层 | 命令 | 它证明什么 | 现在读数 |
|---|---|---|---|
| L1 单测 | `cargo test -p mcloud-tasks --lib app_task` | 判据/门/形状的**逻辑** | **57 passed / 0 failed** ✓（2026-10-04 16:0x） |
| L2 真机端到端（**写**） | `run app-task <账号> --body '{"onlyTaskIds":"409"}'` | 真的发得出去、上游认账 | 已验：106 ✓、409 / 522 / 1005 各跑过一次 ✓（**旁证** = 上游记账，见 §6） |
| L3 界面门禁 | `node scripts/gates-web.mjs` | 页面交互不回归 | ⚠ **夹具里没有 `app-task`**（只有 daily-checkin / receive / live-room / msg-push 四条假任务，`web/regress/lib.mjs:231-259`）⇒ appTask 界面自动化覆盖 **= 0** ✗ |
| L4 全量门禁 | `cargo test --workspace` / `scripts/gates-rust.sh --full` / `gates-web.mjs --full` | 全仓不回归 | 本轮**没跑** ✗ |

⚠ L1 里"该 id 出现 N 处"是**粗指标**：同一文件里也可能只是黑名单字面量或反例（118 / 120 / 1021）⇒ 别当成"逐条覆盖" ✗。

## 4. 手动验收（留给你）

**A. 5 条已实现的：跑一次，逐条核明细**

1. WebUI → 任务 → `app-task`（账号选测试号）→ 跑一次；
2. 预期摘要「**App 任务做了 4 条（判出 4 条）**」（106 若已被上游记完成，会被第 1 道门挡掉 ⇒ 那个数是 4 不是 5；顺序按 id：409 → 434 → 522 → 1005）；
3. 看运行明细：409 / 1005 `clicked:true`；522 `uploaded:5`（会真传 5 个文件，随后删掉）；434 `shared:true`。

**B. 15 条未实现的**（另有 2 条是刻意不做，见 §0）：本仓**一个请求都不发** ⇒ 跑完它们不会出现在这次运行的记录里 ✓。这是"如实不假装"，**不是 bug**；要它们动起来得先按 §5 逐条补实现。

**C. 两条纪律**

* 写运行会**真动上游**：`task/click` 与上传都不可回滚（上传的文件随后被删 / 进回收站）⇒ 用**测试号** ✓；
* 判定"有没有生效"**别急**：上游记账**最长见过 76 分钟**（106 从跑到记账隔了 76 分钟 ✓）⇒ 跑完立刻看 §6 的 `state` 会得出错的结论 ✗。

## 5. 已知差异 / 为什么未实现

1. **本仓对"判不出形状"的条目连打卡都不发** —— 参考实现的 `default:` 分支在 `TASK_LIST[task.id]` 不命中时**仍会** `clickTask`（`mcloud-sign` 的 `services/app/tasks.ts:101-105`），而本仓主循环先 `filter_map(step_for)`（`app_task.rs:602-611`）⇒ 那 17 条一枪不发 ✗。这是**结构性差异**，不是漏写一行。
2. ✅ **1021：这里原先写错了，2026-10-06 更正**（旧文写的是"参考里 `TASK_LIST[1021]` 先命中 ⇒ 会跑 `emailNotice`，两边结论相反"✗）。
   **三边实测**（`/Users/catlair/Documents/workspace/mcloud-sign` 是本机 TS 检出）：
   * 旧 TS 的 app runner 表键 = `106/107/434/113/409/522/1005/472/473/1004/1020/1014/1028/585`（`packages/core/src/tasks/app/runners.ts:39`）——**没有 1021**；
   * 旧 TS 的分派顺序确实是"runner 先、黑名单兜底"（`tasks.ts:96-104`），但 1021 **不在 runner 表里** ⇒ 落到 `if (!SKIP_TASK_LIST.includes(task.id))`（`:111` 含 1021）⇒ **一枪不发** ✓；
   * Go 参考同构（`internal/app/task_matrix_dispatch.go:519-557`：runner 表 → toolGaps → beiyong1 → `taskMatrixSkipsClick` → 盲点 click），1021 两边都不在 ⇒ `StepBlockedByRule` ✓；
   * `emailNotice` 这个符号在 **TS / Go / 本仓三边 0 命中** ✗ —— 旧文那个名字是**编的**。
   ⇒ **两边结论一致**：矩阵都不碰 1021 ✓（本仓的 `SKIP_TASK_LIST` 与判定顺序**不动** ✓）。
   **1021 的真身**是另一条任务：旧 TS 的 `noticeTask`（`packages/core/src/tasks/notice.ts:8`）**邮箱那一半**（`:32-54`）——
   查邮箱月表（`getEmailTaskList`：`POST …/signin/task/taskListV2`，body `{marketname:"newsign_139mail",clientVersion:"11.2.1",group:"month"}`，`apis/task.ts:37-46`）
   → 按 id 找 1021 → 读 `button.email.{day,canReceive}` → **只有 `Number(day) > 0 && canReceive === 1`** 才发
   `GET …/signin/page/reportAppNoticeStatus?status=1`（`:44-50`）。
   ⚠ 它**只上报状态、没有"做"的动作** ⇒ 语义上限就是"上报/领取"（证据缺口见 §6）。
   **本仓落码（2026-10-06，用户口径「跟参考」）**：`claim_email_notice`（`crates/mcloud-tasks/src/notice.rs`）
   + `email_task_list_request` / `parse_email_notice_slot`（`crates/mcloud-upstream/src/app_notice.rs`），
   接在 `NoticeTask::run` 里**短信那一半之后**，两半各自 best-effort ✓。
   ⚠ 旁证：活列表里 1021 的 `marketName / group` 正是 **`newsign_139mail` / `month`**（§6 附录那张表 ✓）——
   与上面那个 body 逐字对上 ✓。
3. **mail139 整族未实现**（参考里是 **472 / 473 / 1004 / 1014 / 1020**）：`grep -rln mail139 crates/` = **0 命中** ⇒ 那几条的 runner 无从落。⚠ 早先把 **1021** 也算进这一族是**错的** ✗ —— 1021 在 `notice` 族，见 §5-2 ✓。
4. **113 差一个入参**：参考是 `refreshToken()` + `uploadRandomFile({channelSrc:"10200153"})`；本仓 upload **没有** `channelSrc` 这个入参（`grep -rn channelSrc crates/` 只命中 login 的头常量）。
5. ⚠⚠ **本表曾经"编过参考符号"（2026-10-06 查实，5 处已全部修掉）**：主表里「参考实现那个 runner 叫什么」有 **5 处是编的** ——
   `emailNotice`（1021 那行，见 §5-2）、`sendMailRunner`、`pdfConvertRunner`、`backupMailRunner`、`aiWorkbenchRunner`：
   这些串在 **TS / Go 两边 0 命中** ✗（真名分别是 `sendMailTask` / `pdfConvertTask` / `backupMailTask` / `aiWorkbenchTask`，
   见 `runners.ts:154/168/161/175`；而 `aiWorkbenchTask` 属于 **1028**，不是 1057 ✗）。
   ⇒ **审计办法**（以后写"参考里有 X"时照做）：把每个引用的符号拿去做 **两遍 `grep -rn <符号>`**
   （TS 检出 `/Users/catlair/Documents/workspace/mcloud-sign/packages` + Go 参考 `../mcloud-back/internal`），
   **0 命中就不许写**；只写得起"能自己跑的命令或 `file:line`"的那种引用 ✓（与 `AGENTS.md` 的依据优先级同一条）。
   ⚠ 编出来的名字比"不写"危害大：它让一行"未实现"看着像"有据可依" ✗。
5. **615 的名字门**：`beiyong1_name_gate` 要「上传」+「图/照」（符号 `beiyong1_name_gate()`），活体名字是「记录国庆节欢乐瞬间」⇒ 不过门。
6. **434 的原始 name 带 HTML**：上游给的是 `分享文件领好礼<span id='share_title'>0/7</span>` ⇒ 进记录前由 `clean_task_name()`剥掉 ✓。
7. ✅ **434 的"第 0 步"曾经被吞了 —— 已修（2026-10-04）**：`should_click` 原先把"434 只在第 22 步打卡"写成了**替代**规则（`if task_id == 434 { return step == 22 }`），而参考实现里那张 `specialMap` 是**叠加**在步号规则之上的 ——
   `if (specialMap[id] && currstep === specialMap[id]) return click; if (currstep === 0) return click;`
   ⇒ **`currstep` 为 0 / 缺格时：参考会打卡，本仓不会** ✗（同函数的注释自己写的正是"`currstep == 0` ⇒ **打**" ⇒ 代码与自己的注释相反 ✓）。
   **实证**：小号 `9f611bba8206ca0a` 的 run 96（17:06:18）里 434 是 `clicked:false`，还打了那句 `已走到第 0 步 ⇒ 按旧实现不打卡` —— 而"旧实现"在第 0 步恰恰**会**打卡 ✗；同趟 106 / 409 / 522 / 1005 都 `clicked:true` ✓。
   **改法**：`crates/mcloud-tasks/src/app_task.rs:325-331`（`if 434 && step == 22 { return true }`，其余落回 `step == 0`）+ 注释一并写清"叠加不是替代" ✓。
   **钉子两条**（⚠ 原先那条单测**把错值钉住了**，所以这次特意补了运行级）：
   * 单测 `the_share_task_clicks_at_its_special_step_and_still_at_step_zero`（22 ✓ / 0 ✓ / 缺格 ✓ / 21·23·5 ✗）
   * ⭐ 运行级 `the_share_task_clicks_when_upstream_omits_the_step` —— 断言**真的发出了一发 `task/click`**（`tests.rs` 新增 `task_json_without_step`）
   **红测**：把修复临时撤掉重跑 ⇒ 两条都红 ✓，运行级那句是 `434 该真打一发，实得 []`（旧写法**一枪不发**）✓；还原后 `cargo test -p mcloud-tasks --lib app_task` = **58 passed / 0 failed** ✓。
   ⚠ **代价（你点头过的）**：以后每条 434 会多一发**不可回滚**的 `task/click` —— 参考实现就是这么打的 ✓。
   ⚠ **同处发现的第二件 —— 也已修（2026-10-04），但中途被我理解反过一次** ✗：434 那一档原先**只在分享成功**时计 `acted`，另外两档（`Entry`、`Upload`）却是"打卡过就算" ⇒ 三处口径不一致。
   **我第一版的"改法"是错的** ✗：我读旧 TS `tasks.ts:29-36` 的 `handleTaskClick`（`if (await clickTaskWithStep(...)) { await taskRunner[id]?.(task); doingList.push(task.id) }`）得出"**打卡门一过就算**"，于是把 434 改成 `if step.clicked || step.shared { acted += 1 }`。用户当场指出：那句「分享文件成功（**分享成功不等于任务完成**）」的意思**不是**"打卡就算"，而是 **官方根本没有判断完成的接口，只能比次数** ✓。
   **真正的依据（用户指路后才看全）**：`app.ts:30` 在跑之前把 434 名字里的次数记进 `state.shareCount`（`getShareCount`，`tasks.ts:113-115`），`app.ts:56-60` 在**全部任务跑完之后 `getAllAppTaskList()` 整表再拉一遍**，`printShareProgress` 里 `if (count > state.shareCount) logger.success(...)` ⇒ **判据是 `after > before`** ✓；而 `results.ts:18-45` 的 `collectTaskResult` 对"处理过、state 仍非 `FINISH`"一律记 `failed` ⇒ 与"门过了 ≠ 完成"同向 ✓。
   **最终改法**：① 434 动手前把名字里的 `a/b` 解出来存 `share_before` ✓；② 跑完（含清理）后**整张表再拉一遍**（16 发只读 GET，同参考 ✓）解 `share_after` ✓；③ **只有 `after > before`** 才 `share_counted = true` 并 `acted += 1` ✓；没涨/读不出就**如实**写进 `problems`（`0/7 → 0/7 ⇒ 这一条不算记上` ✓）。解析按**结构**（`share_title` 段内找 `a/b`，退到整串第一个 `a/b`）而**不**照抄旧 TS 的固定前后缀 `replace`（文案一换就解出 `NaN` ⇒ 静默永不成功 ✗）。
   **钉子 5 条**：解析 3 条（活体形状 ✓ / 文案变了照样解 ✓ / 读不出回 `None` 而**不是 0** ✓）+ 运行级正反 2 条（`the_share_task_counts_only_when_the_upstream_number_moves`：106 真传文件 → 434 建删外链 → 事后表 `1/7` ⇒ `share_counted` ✓、`acted=2` ✓；`a_share_that_does_not_move_the_number_is_not_counted`：外链成了但事后仍是 `0/7` ⇒ **不算** ✓、`problems` 写明 `0/7 → 0/7` ✓）。
   **红测两条**：① 退回旧口径（`|| shared`）⇒ 反例红 ✓（"次数没动 ⇒ 不算记上"）；② 把事后读数 stub 成 `None` ⇒ 正例红 ✓ ⇒ **那趟"事后整表"是承重的** ✓。`cargo test -p mcloud-tasks --lib` = **440 passed / 0 failed** ✓；fmt ✓；clippy ✓。
   **明细新增三格**（`details.reported.steps[]`）：`shareBefore` / `shareAfter` / `shareCounted` ✓ —— 界面上能直接看出"为什么不算" ✓。
   ⭐⭐ **真机验收顺带挖出两个洞（都已修，2026-10-04 19:30-19:40）** —— 用户要求「**单独运行下 434，验证**」，这一跑把"纸面上通"和"真能做成"的差距暴露了：
   ① **顺序洞**：434 消费的是**这一趟**传上去的文件，而计划按 id 升序（`merge_pages` 的 `sort_by_key(task_id)`）⇒ **434(434) 永远排在 522(522) 前面** ✗。真机 run 103（`onlyTaskIds=522,434`）：522 传了 **5** 个文件、434 那一格却是「没有可分享的文件，跳过分享任务」✗；而**两个账号的 106 都是 `FINISH`** ⇒ 只要按 id 排，434 就**永远**分享不到东西 ✗✗。修法：`planned.sort_by_key(|(i, _)| i.task_id == Some(SHARE_TASK_ID))` ⇒ 它**只消费不生产**，排末尾不影响任何别人 ✓（`sort_by_key` 稳定 ⇒ 其余照旧 id 升序 ✓）。
   ② **空手洞**：用户口径「**没有共享文件 id 就主动上传**」✅ —— 就算排到末尾、这一趟也没人上传（只跑 434、或生产者都被跳过），434 也不该空手跳过。修法：434 那一支里 `if result.file_ids.is_empty() { upload_one_file(ctx) }`，交回的 id 进 `file_ids` ⇒ 收尾那一发会把它一起移进回收站 ✓，`step.uploaded` 如实 +1 ✓（但它**不**构成"记上"—— 判据仍然只有次数 ✓）。
   **验收读数（测试号 `0f6050105ab111f4`，`onlyTaskIds=434`）**：run 100（19:28，改之前）⇒ `uploaded:0`、`shared:false`、`0/7 → 0/7`、failed ✗ 与 run 104（19:37，改之后）⇒ **`uploaded:1` ✓ `shared:true` ✓ `0/7 → 1/7` ✓ `shareCounted:true` ✓ `acted=1`、outcome success、`收回 1/1`** ✓ 正好是一组前后对照。
   ⚠ 顺带一条**独立证据**：run 104 里 434 的 `clicked` 是 **`false`**（`currstep == 2` ⇒ 按旧实现不打卡 ✓），**次数照样从 0/7 涨到 1/7** ✓ ⇒ 更说明"打卡"和"记上"是两件事，"打卡就算成"那种口径确实站不住 ✓。
   **新增钉子 2 条（共 5 条）**：`the_share_task_runs_after_the_uploaders`（计划给 `[434, 522]` ⇒ 断言真跑成 `[522, 434]`、434 拿得到 522 刚传的文件 ✓）+ `a_lone_share_task_uploads_its_own_file_and_counts`（只跑 434 ⇒ `uploaded:1` ✓ `0/7 → 1/7` ⇒ `shareCounted` ✓ `acted=1` ✓ success ✓ 收回 1/1 ✓）。**红测又两条**：撤掉"排末尾" ⇒ 顺序那条红 ✓（实得 `steps[0]=Some(434)`）；撤掉"补传" ⇒ 独跑那条红 ✓（`uploaded` 实得 0）。`cargo test -p mcloud-tasks --lib` = **442 passed / 0 failed** ✓；fmt ✓；clippy ✓。
   ⚠ 顺带修掉一处**夹具自身的错**：垃圾箱回执我写成了 `{"code":"0","message":"success"}`，而 `parse_batch_trash` 要求 `code == "0000"` **且** `success: true` ⇒ 那几条用例里"清理"其实一直静默失败、只是没人断言它 ✗（5 处一起改对 ✓，并给新用例加上 `cleaned_up` 断言 ✓）。

8. **522 的进度要报出来**（用户口径 2026-10-04：「522 **应该是成功的**，但是当前日志**没有输出他的进度**，毕竟需要 100 个，也不急着一次做完，**进度还是很重要的**」✓）。
   **问题**：这一档一趟只传 `ROUNDS_PER_RUN=5` 个、要攒 `STOP_AT_PROCESS=100` 个 ⇒ 明细里只有 `uploaded:5`，**完全看不出走到哪儿** ✗（它自己的 `process` 那一格才是有用的读数 —— 真机 42 / 41 ✓，与 434 的"次数"是同一类"官方只给数字"的东西 ✓）。
   **改法**：跑之前把 `item.process` 记成 `progress_before`（`"42/100"`）✓ → 收尾那一趟**事后整表**（与 434 共用同一次拉表 ✓，不额外发 GET ✓）读回 `process` 存 `progress_after` ✓ → 打一行日志 + 明细两格 `progressBefore`/`progressAfter` ✓。⚠ 进度**不参与成败判定**：522 的"成"仍是"这一趟真传了文件"（那一支自己计 `acted` ✓）。
   **真机读数**（测试号，`onlyTaskIds=522`）：run 105 `48/100 → 52/100`、run 106 `53/100 → 58/100`、run 107 `58/100 → 63/100` ✓。
   ⚠⚠ **顺带查明一个"数字打架"**：run 105 我们传了 **5** 个、上游只 **+4** ✗。**不是 bug**：上游记账**慢一步**（最多差最后一个）—— run 106 的"动手前"读数正好是 **53** ✓（= 52 + 那一个），此后每趟都是准确的 `+5` ✓；与 106 那条"76 分钟才记账"**不是一回事** ✓。⇒ 于是日志里把差额**说出来**：`（这一趟 +4；我们实际传了 5 个，差的 1 个上游还没记账 ✓）`，不让两个数字自己打架 ✓。
   **钉子 2 条（都断言了日志行 ✓）**：`the_monthly_upload_reports_its_progress_before_and_after`（`42/100 → 47/100` + `这一趟 +5` ✓）+ `the_monthly_upload_explains_a_short_count`（传 5 只 +4 ⇒ 日志必须写明差额 ✓）。⚠ 为此在测试里加了 `CapturedSink`（原来的 `NullSink` **看不见** `ctx.info/success` ✗ —— 用户问的正是"日志里有没有" ✓）。**红测**：撤掉 522 那一段落地 ⇒ 正例红 ✓（`progress_after` 实得 `None`）。`cargo test -p mcloud-tasks --lib` = **444 passed / 0 failed** ✓。

9. **玩法 113「使用PC客户端」**（2026-10-04 实现并验收 ✓）。
   **真机形状**（两个账号完全一致）：`name=使用PC客户端`、`description=备份后完成任务奖励翻倍`、`groupId=month`、`marketName=sign_in_3`、`limitType=MONTH`、`currStep=0`、`process=0`、`state=WAIT` ✓ ⇒ 它是 `TASK_LIST` 里的条目 ⇒ **照常打卡 + 做 runner** ✓。
   **参考实现 `runners.ts:27-31` 逐字**：
   ```ts
   113: async () => {
     await refreshToken();
     await sleep(1000);
     await uploadRandomFile({ channelSrc: "10200153" });
   },
   ```
   **关键发现**：`channelSrc` **不是 body 字段，而是请求头**（`apis/file.ts:93-118` 的 `getFileOpHeader`）—— `x-yun-app-channel` 发渠道号；PC 档另加 `x-yun-market-source=001` / `x-yun-module-type=100` / `x-yun-svc-type=1` / `x-yun-client-info=<PC 指纹>`；**安卓档**才有的 `x-huawei-channelSrc` 与 `x-yun-sub-op-type` 在 PC 档**一枚都不发** ✓。⇒ 这条任务的**全部意义**就在渠道：它奖励"用 PC 客户端"，按安卓档传等于没做 ✗。
   **改法（3 处）**：① [`UploadShape`] 加 `channel: CloudChannel` 一格（原来 `upload_one_file` 把 `CloudChannel::Android` **写死**在体内 ✗ ⇒ 变成 `upload_one_file(ctx, channel)`，让"哪一档走哪个客户端"在**调用点看得见** ✓）；② 新增 `UPLOAD_ONE_PC`（形状与 `UPLOAD_ONE` **逐格相同**，只差 `channel` ✓）+ `upload_shape_for` 里给 113 挂上 ✓；③ 434 那句补传显式用 `CloudChannel::Android`（那是本仓自己补的一发，没有渠道要求 ✓）。⚠ 本仓 `CloudChannel::Pc` **早就有**（`app_cloud_upload.rs:101-103`，注释里甚至已经写着"旧实现里玩法 113 在上传前先 `refreshAuthToken` 再按 PC 档传" ✓）—— 缺的只是**把它接上**。
   ⚠ **TS 那句 `refreshToken()` 本仓不做** ✗⇒ 依据是 `mcloud-upstream::auth_refresh` 模块文档里的判定：TS 走的 `aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do`（XML 体、`clienttype=656`）**零抓包 ⇒ 不实现**，本仓只走实拍到的 `/user/auth/refreshToken` ✓；而"那条刷新"与"113 算不算做成"之间**没有可证的因果**，旧 TS 的调用方连返回值都不看（拿到新 token 就丢 ✗）⇒ 宁可不做，等有实拍再说 ✓（**这一条留作待拍板项：要不要在 113 前面补一发刷新**）。
   **真机验收**：run 108（2026-10-04 20:07，测试号 `onlyTaskIds=113`）⇒ `{clicked:true, uploaded:1, problems:[]}`、`acted=1`、outcome success、`网盘新增 1 个文件已移进回收站` ✓；run 109（矩阵，1 分钟后）⇒ **113 `WAIT` → `FINISH`**、`currStep 0 → 3`、全表"已完成"从 **2 → 3** ✓ ⇒ 上游认账 ✓（**1 分钟就入账**，不是 106 那种 76 分钟 ✓）。
   **钉子 2 条**：`task_113_uploads_one_file_on_the_pc_channel`（形状与 106 逐格相同、只差渠道 ✓）+ `task_113_sends_the_pc_channel_headers`（**逐枚头**断言 PC 档那 4 枚 ✓，并断言安卓档那 2 枚**缺席** ✓ —— 只看 `uploaded:1` 证明不了它按 PC 档发 ✗）。**红测**：把 113 退回安卓档 ⇒ 两条都红 ✓（形状 `Android` vs `Pc`；头实得 `10000023` vs `10200153` ✓）。`cargo test -p mcloud-tasks --lib` = **446 passed / 0 failed** ✓；fmt ✓；clippy ✓。

10. **玩法 615「记录国庆节欢乐瞬间」—— `beiyong1` 的"按钮门"**（2026-10-04 实现并验收 ✓）。
    **真机形状**（两个账号一致）：`name=记录国庆节欢乐瞬间`、`description=上传10张可领`、`groupid=beiyong1`、`marketname=sign_in_3`、`limitType=EVER`、`currStep=0`、`process=0`、`state=WAIT` ✓。
    **为什么原先没做**：本仓只实现了 `beiyong1` 的**名字门**（`name` 含"上传"+图/照 ⇒ 传 1 张 ✓），而 615 的名字里既没有"上传"也没有"图/照" ✗ ⇒ 它一直被漏在"未实现"里。
    **参考实现 `tasks.ts:66-84` 的 `beiyong1` 分支有三条路**：
    ```ts
    await handleTaskClick(task, doingList, taskRunner);                 // ① 先打卡
    if (name.includes("上传") && (name.includes("图") || name.includes("照"))) { await uploadRandomFile(); return; }  // ② 名字门 ⇒ 1 张
    if ((task.button.app as Beiyong1App)?.link?.includes("uploadView")) { await upload10PictureTask(task); return; } // ③ 按钮门 ⇒ 10 张
    ```
    ⇒ **615 走的是 ③** ✓（顺序也照抄：名字门在前 ✓）。
    **关键证据（抓包仓）**：`har/2026-09-25-通知有礼.har` 的 `taskListV3` 响应里有同族条目
    `id=614 name=上传中秋团圆照 description=上传10张可领 process=6 state=WAIT group=beiyong1`，
    其 `button.app.link = mcloud://main/uploadView` ✓ —— 615 与它是**同一族**（同 description、同 group，只是节日文案不同 ✓）。
    **形状（`UPLOAD_TEN`）**：`ai-tasks.ts:190-224` 的 `remaining = max(0, 10 - process)` + 每张 `uploadRandomFile({ ext: ".jpg" })` + `sleep(300)` ⇒ 本仓四格逐条对上：`rounds=10`、`stop_at_process=10`、`per_round_ms=300`、`file_ext=".jpg"` ✓。
    ⚠⚠ **为此给 `UploadShape` 补了两格**（原来的形状表达不了这一族）：
    ① `top_up_from_process` —— 522 的 `process` 是**循环外的快照闸**（`for(i<5){ if(process>=100) break; }`、循环里不会变 ✓），而 615 是**先算剩余再循环**（`remaining = 10 - process` ✓）⇒ 不做这一格，实拍那条 `process=6` 会**白传 6 张** ✗（真机 run 110 的离线复刻钉住了它 ✓）；
    ② `per_round_ms`（原来叫 `pace_each_round: bool`、时长借用任务的 `pace_ms`）⇒ 522 是 **500ms**、615 是 **300ms**，两族的节拍本来就不同 ✗，做成毫秒数之后差别就写在形状里 ✓。
    ③ 另给上传链加了**后缀**这一格（`random_plan_with_ext`）：这一族刻意传 `.jpg`，别处不带 `ext` 走缺省 `.bin` ✓。
    **门读哪一格**：`TaskV2Item` 新增 `button_app_link`（`app_task_list_v2.rs`，从 `button.app.link` 取 ✓；坏形状一律 `None`・不编造 ✓），**只读矩阵也把它报出来**（`buttonAppLink` ✓）—— 好让"它到底该不该传 10 张"在界面上可复核 ✓。
    **真机验收**：run 110（2026-10-04 20:30，测试号 `onlyTaskIds=615`）⇒ `{clicked:true, uploaded:10, problems:[]}`、`acted=1`、success、`网盘新增 10 个文件已全部移进回收站` ✓、耗时 15.2s（含 10×300ms 节拍 ✓）；run 111（矩阵，1 分钟后）⇒ **615 `WAIT` → `FINISH`**、全表"已完成" **3 → 4** ✓ ⇒ 上游认账 ✓。
    **钉子 6 条**：门 3（按钮门 ⇒ `UPLOAD_TEN` ✓ / 名字门优先 ✓ / 两条都不中 ⇒ 不传 & 门只在 beiyong1 生效 ✓）+ 解析 1（`button.app.link` 读出 + 四种坏形状 ⇒ `None` ✓）+ 运行级 2（10 张 `.jpg` ✓ / `process=6` ⇒ 只补 4 张 ✓）。**红测 3 条**：撤按钮门 ⇒ 三条红 ✓；关 `top_up_from_process` ⇒ 补差额那条红 ✓（实得 10/期望 4）；把进度分母写回 522 的常量 ⇒ 两条红 ✓（实得 `Some(100)`/`"6/100"` ✗）。
    ⚠⚠ **顺带认一个我自己造的 bug（真机抓到的）**：`progressBefore` 的分母我写成了 522 的常量 `STOP_AT_PROCESS` ⇒ 615 那一格里冒出 **`0/100`** ✗（run 110 明细实拍 ✓）。改成**按各档自己的 `stop_at_process`** 之后，615 显示 `0/10 → 10/10` ✓、522 仍是 `42/100` ✓（钉子在 `task_615_*` 两条里 ✓）。⚠ 615 已 FINISH ⇒ 那条修复**没法再真机复现**（钉子复刻的就是 run 110 的原值 ✓）。
    ⚠ **另记一条现场观察**：615 完成后，上游那一格的 `button.app.link` **没有了**（run 112 的 `buttonAppLink=None`，而同样 FINISH 的 106 仍带着 `mcloud://main/uploadView` ⇒ 是**那条任务自己的状态变化**、不是解析问题 ✓ —— 全表 15 条都读得出 link ✓）。影响：**615 之后再也没法用它真机复验这个门** ✗；但门在它 WAIT 期间确实命中过（run 110 那 10 张就是证据 ✓）。
    `cargo test -p mcloud-tasks --lib` = **451 passed / 0 failed** ✓；`cargo test -p mcloud-upstream --lib` = **724 passed / 0 failed** ✓；fmt ✓；clippy ✓。

9. ✅✅ **409 / 1005 从来"没记上"的真因：打卡 key 发错了**（**2026-10-10 修，真机读数钉死**）——
   症状是用户截图里那两行：`从固定入口访问云朵中心：本次做了，但上游状态/读数没变（状态 WAIT → WAIT）⇒ 没记上`
   （`docs/task-log.md` 第 190 行）。
   **真因**：旧 TS 里这一族是**两发打卡**，而两发的 key **不一样** ——
   * `tasks.ts:19-27` 的 `clickTaskWithStep` → `clickTask(ctx, id)` ⇒ **默认 key `task`**（"先打卡"那一发，`handleTaskClick` 打的）；
   * `runners.ts:107` 的 `await clickTask(ctx, 409, "task2")` ⇒ **`task2`**（runner 里"补的那一发"）。
   本仓原先 `click(ctx, item)` **两处共用一个 `click_key(id)`** ⇒ 409 变成 **`task2` 打两发** ✗，
   而"先打卡"那一发在参考实现里是 **`task`** ⇒ 上游没把它算成"进过固定入口" ⇒ 状态永远 `WAIT` ✗。
   （`click_key()` 的注释里本来就写着"`handleTaskClick` 那发**另算**"，只是没落到代码上 ✗。）
   **修法**：`click()` 固定走默认 key；新增 `click_with_key()`，只有 `Step::Entry` 的"补那一发"传 `click_key(id)` ✓。
   **1005 那一格**：它 `currstep` 恒为 1 ⇒ "先打卡"那道门（`currstep == 0` 才打）**永远不打** ⇒
   这一条只发得出**一发**；按 409 的读数（`task` → `task2` 才翻）外推，**完成那一发必须是 `task2`** ✓
   —— 参考实现给 1005 用的是默认 key，而它在参考实现里**也没成**（`state` 停在 `WAIT`、`currstep` = 1 ✗）
   ⇒ 这一格是**照真机读数改的**，不是照 TS 抄的 ✓。
   **真机验收（三账号，`onlyTaskIds=409,1005`）**：
   | 账号 | 409 | 1005 |
   |---|---|---|
   | 测试 `0f6050105ab111f4` | `WAIT → FINISH` ✓（run 293） | `WAIT → FINISH` ✓（run 294） |
   | 移动 `b73bb07d5971e8e7` | `WAIT → FINISH` ✓（run 295，同趟两条都翻） | 同左 ✓ |
   | 小号 `9f611bba8206ca0a` | 早已 `FINISH`（本轮不计划） | `WAIT → FINISH` ✓（run 296） |
   ⇒ 三个账号、**零失败**（修之前同一批账号连着多趟都是 `WAIT → WAIT`，run 280/281/282 ✓）。
   **顺带的两处**：① 这一族**两发上报**（新 `POST …/ycloud/signin/page/doTaskPost`（App 12.6.0 实拍）+ 旧 `GET mrp…/market/signin/page/doTask`（TS 路径，Go 探针实测回 `code 0`）），
   `activityid` 跟着**本条任务的市场名**走（409 = `sign_in_3`、1005 = `newsign_139mail`）✓；
   ⚠ 实测这两发**都不是**翻账的原因（修 key 之前发着也不翻）—— 它们是"照 App 的形状补齐"，别把功劳记在它们头上 ✓；
   ② 上报那一发**按业务码判**（`SignInEnvelope` 的 `code`）而不是只看 HTTP 状态 ——
   这一族的失败长相是 `HTTP 200 + code≠0`，而 `ensure_http_ok` 只看状态码 ⇒ 打回也当成功 ✗。
   **测试**：`cargo test -p mcloud-tasks --lib` = **489 passed / 0 failed** ✓；`mcloud-upstream --lib` = **809 passed / 0 failed** ✓
   （运行级新用例 `entry_tasks_report_the_page_on_both_paths_with_their_own_market` 断言两发上报的市场名 + 两发打卡的 key 顺序 ✓）。
10. ✅✅ **1020「完成一次邮件转存」：旧 TS 那条路今天 404，活路在 App 的字面量里**（**2026-10-10 修，真机两个账号各翻一次**）——
   **现象**：跑一趟就报 `邮箱备份请求失败：网络错误：邮箱备份（mailsms/sms/backup）失败，HTTP 404：上游回的是**网页**不是接口` ✗，
   状态恒 `WAIT → WAIT`（用户 2026-10-10 08:00 的截图就是它）。
   **这不是网络问题、也不是我们抄错了地址**：旧 TS 就是 `POST https://mail.10086.cn/mailsms/sms/backup?sid=`（`runners.ts:285-294`，空 JSON 体 ✓），
   而**抓包**证明那台 `mail.10086.cn` 今天只认 `/login/inlogin.action`（同一张会话在它上面 `autologin.action` 与 `mailsms/sms/backup` 都 404，
   在 `html5.mail.10086.cn` 上却连**写**都被认 ✓）⇒ **参考实现里这一条没有活路** ✗，照抄只会一直 404 ✓。
   **活路（App 实拍三发）**：139邮箱 App 自己的字面量里有这一族（`-[PMCaixunInterface uploadMailToCloudWithMid:directoryId:user:onFinish:]` 与 `disk:backupMail` 同簇 ✓）：
   ① `disk:getDirectorys`（落点目录）→ ② `mbox:SearchByFidOrCondition`（一枚邮件号 `mid`）→ ③ `disk:backupMail`（`mailIds` / `backupType` 空 / `channel=16861` / `directoryId`）。
   主机取 **`html5.mail.10086.cn` + `/mw2/file/disk`**（**抓包**那一档：同一台同一前缀上 `disk:surplusNum`、`disk:convertPdfFromYunFile` 都被认 ✓；App 模板里的 `%@` 是运行期路由域）
   ⇒ 这一条是**"App 字面量 + 抓包主机"的拼装**，标注在 `mail139.rs` 那一节的文档注释里 ✓。
   ⚠⚠ **② 不能带 `encryptType=1`**（App 字面量里**有**它）：真机实测带上之后响应体是 **3296 个十六进制字符**（按客户端那一套加密回的，我们没有那把钥匙）⇒ 读不出 `mid` ✗；
   去掉它同一个口回**可读体** ✓ —— 这一格是**两次真机读数**（run 297/298 失败 → 去掉后 run 299 成）钉下来的，不是推的 ✓。
   **真机验收**：测试 `0f6050105ab111f4` run **299** `WAIT → FINISH` ✓、移动 `b73bb07d5971e8e7` run **300** `WAIT → FINISH` ✓；
   小号 `9f611bba8206ca0a` 这一条早已 `FINISH`（run 301 直接跳过 ✓）⇒ **三档读数都在** ✓。
   **测试**：`cargo test -p mcloud-tasks --lib` = **491 passed / 0 failed** ✓；`mcloud-upstream --lib` = **814 passed / 0 failed** ✓
   （新增：三发形状 ✓、取不到 `mid` 时**把体的键名形状**带进失败明细 ✓、业务码打回算硬失败 ✓、`first_field` 两种形状都收且取不到不猜 ✓）。

11. ✅✅ **1017「登录移动云盘云朵中心」：缺的是"页面初始化上报"那一发**（**2026-10-10 修，真机两个账号各翻一次**）——
   **现象**：这一条在旧表里是"❌ 未实现"（它属于 §5-1 说的"判不出形状的 17 条"之一：本仓连打卡都不发 ✗），
   面板上写着「福利同步，探索更多任务」、`newsign_139mail` / `month` / 第 1 步 / `state=WAIT`。
   **先按 TS 试打卡（都不成）**：TS 的分派里它**会被打卡**（不在 runner 表、也不在 `SKIP_TASK_LIST`
   ⇒ 落到 `tasks.ts:101-103` 的 `clickTask(ctx, task.id)` ✓），于是先加进 click-only 试了两档 key：
   **默认 `task` → `WAIT → WAIT` ✗（run 302）**、**`task2` → 还是 `WAIT → WAIT` ✗（run 303）**
   ⇒ 打卡这一格不是答案（这一条与 409/1005 的"key 发错"**不是同一类** ✗）。
   **拿到原始条目之后才看清形状**（临时取证，见下）：它的 `button` 里是
   **`button.email.link`**（**不是** `button.app.link` ✗）——
   `https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin&sourceid=1179&enableShare=1&targetSourceId=001005&token=#ssoToken#`，
   且 `stepTypeSet = ["sendcloud","click","interfacecallback"]`、`currstep = 1` ⇒
   与 409/1005 同族（"进云朵中心"），但**入口是一张能直接取的 http 页**（那两条是 `mcloud://` 深链，取不了 ⇒ 只能发上报 ✓）。
   **修法（四发固定顺序）**：① **页面初始化上报**（`doTaskPost` + 旧 `page/doTask`，`activityid` = 本条任务的
   `newsign_139mail` ✓ —— 出处：H5 页面自己的 JS 里 `initTaskStatus: POST /ycloud/signin/page/doTaskPost` ✓）
   → ② 取入口页（`#ssoToken#` 换成真票 ✓）→ ③ 打卡（`currstep = 1` ⇒ 只发这一发，key = `task2` ✓）。
   **归因是分开的**（run 306/307/308 三趟只发"入口页 + 打卡"、外加一发登录回调 ⇒ **恒 `WAIT → WAIT`** ✗；
   run 309 补上那两发上报 ⇒ **当趟 `WAIT → FINISH`** ✓）⇒ 缺的就是**上报那一格** ✓，入口页与打卡**不是**翻账的原因 ✓。
   **走过的一条岔路（记着，别重复）**：`interfacecallback` 那一格先猜成
   `POST /ycloud/signin/mobileApp/signInCallback`（端点与体都出自页面 JS ✓），拿账号 SSO 票当 `taskToken`
   发一发 ⇒ 上游回 **`code=500 msg=无法获取中国移动任务信息`** ✗（那一格要的是"中国移动那条任务自己的 token"，
   不是我们手里的票）⇒ **1017 不需要它** ✓（构造器留在 `app_signin`，标注"今天没有调用方" ✓）。
   **真机验收**：测试 `0f6050105ab111f4` run **309** `WAIT → FINISH` ✓、移动 `b73bb07d5971e8e7` run **310** ✓、
   小号 `9f611bba8206ca0a` run 311 早已 `FINISH`（跳过 ✓）⇒ **三档读数都在** ✓。
   **测试**：`cargo test -p mcloud-tasks --lib` = **493 passed / 0 failed** ✓；`mcloud-upstream --lib` = **816 passed / 0 failed** ✓
   （新增：分档 ✓、四发顺序 + 占位替换 ✓、`button.email.link` 与 `button.app.link` **不许串台** ✓、`signInCallback` 形状 ✓）。
   ⚠ **口径**：`button.email.link` 这一格**别的条目也有**（同日临时取证：548/549/522/1075/1005/1004/1014/1020 都有 ✓）
   ⇒ 这一档**只认 1017**，**不许**按"有这一格"推广 ✗（别的条目各有自己的 runner ✓）。


12. ✅✅ **604「去云盘图书馆阅读书籍」：与 1017 同一档，但"入口那一格"和"打几发"都不同**（**2026-10-10 修，真机两个账号各翻一次**）——
   **现象**：面板上直接把它给的链接印出来了
   （`https://h.139.com/content/cloudClass?columnId=56&level=1&channel=400&token=#ssoToken#`），
   而本仓**一枪都没发**（它在"判不出形状"那一档里 ✗）；旧 TS 里也没有 runner（全仓 0 命中 604 ✓）。
   **真机原始条目**（`sign_in_3` / `month`）给的三格：
   `button.app.link = 上面那条`、`currstep = 0`、`stepTypeSet = ["sendcloud","click"]`（**没有** `interfacecallback` ✓）。
   **与 1017 的两处差别，都按读数处理**：
   ① 入口那一格在 **`button.app`**（1017 在 `button.email` ✓）⇒ `portal_entry()` 两格都收 ✓；
   ② `currstep = 0` ⇒ **③-a 那道门自己会打一发**（默认 key `task` ✓）⇒ 这一档**不许再补第二发** ✗
   （旧 TS 里 604 没有 runner ⇒ 全趟只有那一发；而 409 是"门那一发 + runner 补的 `task2`"两发 ⇒ **形状不同** ✓）。
   **修法**：`step_for` 把 604 与 1017 一起收进 [`Step::PortalEntry`] ✓；那一档的打卡改成
   **只在③-a 没打过的时候才补**（`if !step.clicked` ✓）—— 1017 `currstep = 1` ⇒ 门不打 ⇒ 补 ✓；
   604 `currstep = 0` ⇒ 门打过 ⇒ 不补 ✓。两条都有测试钉住（604 那条断言**只发一发** ✓，红测：去掉那个 `if` ⇒ 变两发 ⇒ 红 ✓）。
   **真机验收**：测试 run **313** `WAIT → FINISH` ✓、移动 run **314** ✓、小号 run 315 早已 `FINISH`（跳过 ✓）。
   ⚠ **归因没有分开**（与 1017 那次不同）：604 原先**一枪不发** ⇒ "整链（上报 + 入口页 + 门那一发）"是**一起上的** ✓，
   "到底哪一格让它翻"**没有单独验过** ✗ —— 想分离得在别的账号上做（月限任务翻过不能重试 ✓）。
   **测试**：`cargo test -p mcloud-tasks --lib` = **495 passed / 0 failed** ✓；`mcloud-upstream --lib` = **816 passed / 0 failed** ✓。


13. ✅✅ **547 / 609：同一族的 `mcloud://` 深链两条，一条打卡就翻、一条必须真对话**（**2026-10-11 修，各两个账号验收**）——
   两条都在"判不出形状"那一档里（本仓原先**一枪不发** ✗），旧 TS 里也**都没有 runner**（全仓 0 命中 ✓
   ⇒ 旧 TS 只会在 `default:` 分支给它们打卡 ✓）。
   **真机原始条目**（都是 `sign_in_3` / `currstep = 0`，入口都是 **`mcloud://` 深链** ⇒ **取不了** ✗，
   与 1017 / 604 那种 http 入口是两回事 ✓）：
   * **547「加入一个知识库」**（`groupid = time`、`+10豆`）：`button.app.link = mcloud://main/aiAssistantGlobal?params=<base64>`，
     `params` 解出来 = `{"params":{"jumpConfig":{"tab":"squareList","targetUrl":"/pages/knowledgeBase/squareList?from=app"},"sourceId":"101151000"}}`；
     `stepTypeSet = ["sendcloud","click"]` ✓。
   * **609「和AI助手对话」**（`groupid = time`、`+20豆`）：同一个深链前缀，`params` 解出来 =
     `{"params":{"adId":"66197","adVt":"47535","sourceId":"101151000"}}`；
     `stepTypeSet = ["cloudtasklogstep","sendcloud","processbindtask","click","interfacecallback"]`（**多三格** ✓）。
   **先按旧 TS 的 `default:` 口径试打卡**（同族的 431 就是打卡翻的 ✓）：真机 run 318 一趟跑两条 ——
   **547 `WAIT → FINISH` ✓**、**609 `WAIT → WAIT` ✗**（同趟、同账号、同一发打卡 ⇒ 这一对**互为对照** ✓）。
   还看到一格细节：609 那一趟打卡**确实动了上游**（`currstep` 从 0 推到 3 ✓），只是**不算完成** ✗
   ⇒ "打卡没反应"和"打卡不算数"是两件事 ✓。
   **609 的修法**：接 AI 那一族（1028 的整套件复用 ✓）—— 派生 AI 域鉴权 → 状态查询
   （**用本条任务自己的 id**：那一口是**按 `taskId` 作答**的 ✓，抓包仓那张对照表写着 1028 → `501 任务不存在`、
   585 / 106 / 1020 → 真实读数 ✓）→ **SSE 对话** → `sleep 5s` 复查 ✓。
   ⚠ **不抄 1028 的"前置已满足就跳过对话"那道门**：609 在旧 TS 里没有 runner ⇒ 那道门**没有出处**，
   而这一条的名字就是"对话" ✓ ⇒ 每趟都发；状态读数只当诊断写进 notes ✓。
   **⭐ 归因是钉死的**（这一条比 1017 / 604 干净）：run 319 里打卡**根本没发**（`currstep = 3` ⇒ 门不打 ✓），
   而**对话前后**的状态读数变了：**对话前 `code=0 taskStatus=0 cloudStatus=-1`** ⇒
   **对话后 `taskStatus=1 cloudStatus=0`** ✓ ⇒ **翻账的就是那一发对话** ✓（不是打卡 ✓）。
   **真机验收**：547 —— 测试 run 318 ✓ / 移动 run 322 ✓；609 —— 测试 run 319 ✓ / 移动 run 320 ✓；
   小号两个 id 都是 `planned = 0`（没排进计划 ⇒ 上游那边不是"待做" ✓）。
   **测试**：`cargo test -p mcloud-tasks --lib` = **495 passed / 0 failed** ✓（映射 1 条 ✓、click-only 串 1 条 ✓）。
   ⚠ **AI 那条链没有运行级单测**（1028 也没有 ⇒ 本仓现状 ✓）：运行级要真登邮箱 + 派生 AI 鉴权 + 打 SSE，
   成本高；这一条的验收靠**真机读数**（对话前后两格 ✓）✓ —— 想补的话按 `runners/tests.rs` 那套 scripted 回复接 ✓。


14. ❌ **1029「查看"我的账单"」：今天没做成 —— 把试过什么、还剩什么候选如实记下来**（**2026-10-11**）——
   **现象**：面板印着 `https://y.10086.cn/x/wdzd`（短链 ✓）、`newsign_139mail` · 第 0 步 ·
   「一键掌握话费及流量使用情况」· 状态 `WAIT` ✓；本仓原先**一枪不发** ✗（不在任何表里 ✓）。
   ⚠ 这一条**只在移动 / 小号两张表里**（测试那张**没有**它 ✓）⇒ 两个账号都能迭代（没翻就不算用掉 ✓）。
   **真机原始条目**（两张表逐字相同）：`currstep = 0`、`groupid = month`、`content_display2 = "+15豆"`、
   `button.app.link` 与 `button.email.link` **两格同一条短链** `https://y.10086.cn/x/wdzd`（**没有** `#ssoToken#` ✓）、
   `stepTypeSet = ["cloudtasklogstep","sendcloud","sendsms","email"]` ✓、
   `show = {"limitMoveNet":"1"}` ✓。
   **短链落点**（实拍，`curl -L` ✓）：
   `https://y.10086.cn/x/wdzd` →（浏览器档 UA）`https://html5.mail.10086.cn/pe/PE_html/open_download.html?aid=wdzd&name=MyBillFolder&channel=4013001&thingId=50668` ✓
   —— 也就是说这个 thing 是**邮箱里的一个文件夹**（`name=MyBillFolder` ✓），
   而那一页是"装 App / 用微信打开"的**落地页** ✓。
   **那一页自己会发的两个像素**（内联 JS 逐字 ✓，体在 `peCommonFun.js` 的 `addBehavior` 里 ✓）：
   `clickDownload → addBehavior(111949, thingId)`（下载 App ✓）、
   `clickOpen → addBehavior(111948, thingId)`（在 App 里打开 ✓），
   端点 = `https://smsrebuild1.mail.10086.cn/weather/weather?func=user:logBehaviorAction&action=…&thingId=…&version=html5&cguid=…` ✓
   （**空 200**，fire-and-forget ✓）。
   **试过的四档，全是 `WAIT → WAIT` ✗**：
   | # | 试了什么 | run | 读数 |
   |---|---|---|---|
   | ① | 上报 + 取入口页 + 打卡（入口页那一档的口径 ✓） | 327 | ✗ |
   | ② | 像素 `action=111949`（不带 cookie） | 328 | ✗ |
   | ③ | 像素 `action=111949` + **邮箱 cookie**（`RMKEY`/`Os_Sso_Sid` ✓） | 329 | ✗ |
   | ④ | 像素 `action=111948`（"在 App 里打开"那一格 ✓）+ cookie | 330 | ✗ |
   **另有一个读探针的读数**（找"账单文件夹"的 `fid`）：`mbox:getAllFolders`（体照同族拼 ✓）
   回 `{"code":"S_OK","var":""}` ✓ —— **路由通、会话可用**，但**我的体形状要不到清单** ✗
   （探针用完已删 ✗，读数留在抓包仓 §12.3 ✓）。

   ---
   ### 2026-10-11 上午续查（第二轮，读数更全了，**仍然没做成** ✗）
   **用户口径（关键）**：「**移动号才有账单任务**」 ✓ ⇒ 这一条是**话费账单**（10086 侧 ✓），
   不是"邮箱里那个文件夹" ✗ —— 顺带把那条岔路走死了 ✓。
   **① 列目录那一口的**正确形状**拿到了**（webmail 自己的 JS：`/html5/js/T.mvc.pack.js` 的 `getFolderList` ✓）：
   `POST {HTML5_MAIL_HOST}/s?func=mbox:getAllFolders&sid=…`，体 **`stats=1&type=0`** ✓
   （不是我先前拼的 `<object><int name=…>` ✗）。实测移动那个号回 **2326 字节** 的清单 ✓：
   收件箱(1) / 服务邮件(8) / 草稿箱(2) / 订阅邮件(9) / 已发送(3) / 已删除(4) / 垃圾邮件(5) ✓
   ⇒ **这个号根本没有"账单"文件夹** ✗（那条 JS 里 `"我的帐单" == list[i].name` 那一句
   对应的是**有账单的号** ✓）⇒ **候选①（读账单文件夹）在这号上不成立** ✓。
   **② 找到账单页本体**（App 字面量：`https://html5.mail.10086.cn/html/mobilebill.html?sid=` ✓，
   `mobilebill` = 手机账单 ✓）⇒ 扒它自己的包 `/js/mobilebill.html.pack.js` ✓，里面：
   * **账单数据口**：`POST {HTML5_MAIL_HOST}/setting/s?func=bill:getBatterypitcherBill&sid=…`，
     体 **`<null></null>`**（`T.json2xml(null)` 的产物 ✓，逐字照抄 ✓）—— 实测回
     `{"code":"S_OK","summary":"","billDates":[]}` ✓（**路由通、会话可用**，该号**没有可补投月份** ✓）；
   * **页面加载那一发像素**（这一条最像"看了账单" ✓）：模型里那张表写着
     `CONFIG_INFO: { common:{}, 1000006:{ loadCount:[108365, 66] } }` ✓，
     而 `initConfigData` 里就是 `T.addBehavior.apply(T, CONFIG_INFO[sendId].loadCount)` ✓
     ⇒ 打开账单页 = **`addBehavior(108365, 66)`** ✓。
   **③ 同一发像素有两台主机**（各读到一处 ✓）：PE 落地页那版发
   `//smsrebuild1.mail.10086.cn/weather/weather`（带 `cguid` ✓）；
   webmail 框架那版（`T.mvc.pack.js` 的 `T.addBehaviorAction` ✓）发**相对**路径 `/mw2/weather/weather`
   ⇒ 落在 `html5.mail.10086.cn`（**不带 `cguid`** ✓）—— **两版都试过** ✗。
   **这一轮试的（全 `WAIT → WAIT` ✗）**：
   | # | 发了什么 | 读数 |
   |---|---|---|
   | ⑤ | webmail 那台主机上的像素（`action=111948` + 邮箱 cookie） | ✗ |
   | ⑥ | 账单数据口 `bill:getBatterypitcherBill` + 像素 | ✗ |
   | ⑦ | **页面加载那一发**（`action=108365 thingId=66`，webmail 主机 + cookie） | ✗ |
   ⚠ 那一发像素**上游什么都不回**（0 字节空 200 ✓，fire-and-forget ✓）⇒ 从回应里看不出对错 ✓。
   **下一轮该往哪走**（都还没有读数支持 ✓）：① 10086 侧（`happy.mail.10086.cn/jsp/cn/billsubscibe/sso.do`
   那一发"账单订阅 SSO" ✓ —— 账单页里 `mobile.billAccount` 就指它 ✓）；
   ② 账单页的**主数据**（那一页是"按标签列账单邮件" ✓：`noMailTip` = "该标签下没有邮件哦" ✓
   ⇒ 也许是 `mbox:searchMessages` 按某个 `sendId` 标签读 ✓）；
   ③ PE 活跃上报（`func=user:activeAction` ✓）。
   **结论（已排除 / 还剩什么）**：
   * 已排除：打卡（run 318 那一族的读法）、取落地页、两个行为像素（带/不带 cookie ✓）。
   * 还剩的候选（**都没有读数支持**，下一个接手的人从这里挑 ✓）：
     ① **邮箱侧读"账单文件夹"** —— `stepTypeSet` 里有 `email` ✓、thing 名字就叫 `MyBillFolder` ✓
        ⇒ 先拿对 `mbox:getAllFolders` 的体（或别的列目录口），再 `mbox:SearchByFidOrCondition` 换 `fid` 读它 ✓；
     ② **PE 活跃上报**（`func=user:activeAction` ✓ —— 旧 TS 那句注释写着"用于推进 mail139 邮箱活跃度及**各类任务进度**" ✓，
        1004 用它发 `eventType="3"` ✓；1029 该用哪一格**未知** ✗）；
     ③ 云盘 App 侧的市场任务通知（`/marketPlat/api/marketTaskNotify.action` 之类 ✓ —— 只在 App 表面文档里见过路径 ✗，形状未知）。
   **代码现状**：**已撤回**（半成品不留 ✗）—— 只留下 `mail139::log_behavior_action_request` +
   `BEHAVIOR_ACTION_URL` / `BILL_VIEW_ACTION` / `BILL_THING_ID` 三个常量，**明确标注"今天没有调用方"** ✓
   （端点存在、形状长这样、上游回空 200 这三格已经是读数 ✓）；
   测试里加了一条 `AppRunner::for_task(1029) == None` ✓，**防止半成品被当成能用** ✓。
   ⚠ 所以这一条现在的状态就是 **❌ 未实现** ✓ —— 宁可写这个，也不写"记着要做" ✗。


   ---
   ### 2026-10-11 上午第三轮：**把能想到的 16 条全试了，一条都不翻** ✗（按用户口径「你先测试所有可能性」✓）
   这一轮改法：写了个**临时探针**（`MCLOUD_BILL_PROBE=<n>` 选一条 ✓，**一趟只发一条** ⇒ 万一哪条翻了，
   立刻知道是哪条 ✓ —— 这一条月限，翻过就不能重试 ✓）；试完**探针已撤** ✗。
   | # | 候选（依据） | 结果 |
   |---|---|---|
   | ① | `addBehavior(108372, 3)` —— 包里底部菜单那格 `{action:"108372", thing:"3", menuName:"查话费"}` ✓ | ✗ |
   | ② | `mbox:searchMessages` 按 5 条 `sendId` 条件搜账单邮件（包里 `getMailsList` ✓） | ✗（但**真回了 12886 字节的账单邮件列表** ✓） |
   | ③ | `GET mobilebill.html?sid=`（App 字面量 ✓） | ✗ |
   | ④ | `GET /?id=bill`（App 字面量 ✓） | ✗ |
   | ⑤ | `GET happy.mail.10086.cn/jsp/cn/billsubscibe/sso.do`（包里 `mobile.billAccount` ✓） | **502**（这条路径今天不在 ✗） |
   | ⑥ | ② → `disk:backupAllBillMail` **转存账单邮件**（真转存 ✓ 回 31 字节） | ✗ |
   | ⑦ | PE `user:activeAction`（`eventType="1"`） | ✗ |
   | ⑧ | `addBehavior(108372, 2)`（"139云朵中心"那格 ✓） | ✗ |
   | ⑨ | `setting:queryServiceNumbers&type=2&comefrom=2001` | **502** |
   | ⑩ | `addBehavior(110542, 6)` —— 包里 `case "readMail"` 里 `1==read && 1000006==sendId` 才发的那一格 ✓ | ✗ |
   | ⑪ | ② → `updateMessagesStatus`（**把第一封账单邮件标已读** ✓，真发 ✓ 回 23 字节） | ✗ |
   | ⑬ | ② → `view:readMessage`（**真读邮件正文** ✓，回 **27816 字节** ✓） | ✗ |
   | ⑮ | `click?key=task2`（1017 当初就是换 key 才翻的 ✓） | ✗ |
   | ⑯ | `click?key=randomCloudTask` | ✗ |
   **这一轮新拿到的读数**（都在抓包仓 §12.5 ✓）：
   * **账单邮件列表的壳**（`mbox:searchMessages` ✓，12886 字节 ✓）：`{'code':'S_OK','var':[{'mid':"…",'fid':1,'imapId':…,'size':…,'sendId':…,'billType':…,'billFlag':…,'sendNewNum':…}]}`
     —— ⚠ **键是单引号、值是双引号**（`'mid':"d506…"` ✓）⇒ `serde_json` 直接失败 ✗，
     本仓的 `first_field` 也只认两种壳 ✗ ⇒ 补了**第三种**（值不带引号那一支旁边又加引号壳 ✓）。
   * 邮件列表里**确实有账单邮件**（`billType` / `billFlag` / `sendNewNum` 这些键 ✓）⇒ 搜索条件是对的 ✓。
   * `logBehaviorAction` 那一发**上游什么都不回**（0 字节空 200 ✓，fire-and-forget ✓）⇒ 从回应看不出对错 ✓。
   **结论**：邮箱侧能做的**读**（列表/正文/标已读/转存/像素/页面/换 key）**都试遍了** ✗ ⇒
   这一条的读数**多半不在邮箱侧** ✓。**用户已点头下一步抓包** ✓ —— 抓的时候最有用的是：
   **在 App 里点这条任务、一直点到它显示完成**，以及**看它有没有跳到 10086/账单页之外的什么地方** ✓
   （云盘侧还有一族 `/marketPlat/api/{marketTaskNotify,marketTaskQuery,marketGetPrize}.action` ✓
   只在 App 表面文档里见过**路径名**、形状未知 ✗，那是最可能的"上报口" ✓）。

   ---
   ### 2026-10-11 抓包准备（**已就绪并实拍验证** ✓ —— 用户口径「你做好抓包准备」）
   **Mac 侧（我做完了 ✓）**：
   * Reqable GUI 已起（`/Applications/Reqable.app` ✓，PID 见 `pgrep -fl "Reqable.app/Contents/MacOS/Reqable"` ✓）；
   * 系统代理已接管：`networksetup -getsecurewebproxy Wi-Fi` = `Enabled: Yes / 127.0.0.1:9000` ✓
     —— **原状是 `Enabled: No / 127.0.0.1:7897`** ✓（回滚就把它设回这个 ✓）；
   * 录制 `active` ✓、记录已清空 ✓、`9000` 在 LISTEN ✓；
   * **实拍验证过"抓了真有数据"** ✓：从 `python3 urllib`（它会走系统代理 ✓）打一发
     `https://html5.mail.10086.cn/html/mobilebill.html` ⇒ `reqable-capture.py summary` 回
     **`总记录 1 条`（进程 `python3.14`）** ✓ —— 那一发 python 自己报
     `CERTIFICATE_VERIFY_FAILED` 是**正常的**（它不认 Reqable 的 CA ✓，iOS App 认 ✓）。
   **手机侧（要你动手 ✓）**：Wi-Fi → 代理 → 手动 → 服务器 **`192.168.3.187`**（本机 `en0` ✓）、端口 **`9000`**；
   HTTPS 要能被解密才看得到内容（Reqable 的 CA 已在信任列表里就够 ✓ —— 若记录里只有 `CONNECT` 隧道、
   没有明文请求，就是 CA 那一步没成 ✓）。
   **要抓的那一趟（按顺序 ✓）**：
   ① 先跟我说一声「开始了」⇒ 我在本仓 `clear` 并记下时刻 ✓；
   ② 在 **139 邮箱 App** 里走这一条：**我的账单** → 点「去完成」→ **它跳到哪就跟着走**
      （跳到 10086 / 浏览器 / 微信也照走 ✓ —— 手机级代理把这些都看得见 ✓）；
   ③ 一直到它显示 **已完成 / 已领取** 为止 ✓，**中途别回退重来**（这一条月限，重来就多花一次 ✓）；
   ④ 抓完**立刻**说一声 ⇒ 我马上导出 HAR（记录按体积滚，晚一步可能就没了 ✓）：
      `python3 scripts/reqable-capture.py har har/2026-10-11-1029-bill.har --all` ✓。
   **顺手记两个读数（我没法从包里推 ✓）**：点之前卡片上写的字（`去完成` ✓）、点之后写的字（`已完成`？✓）、
   以及**有没有弹提示**（"任务已完成"/"明天再来"这类 ✓）。

15. ⚠⚠ **原生 App 的行为上报走 `logreport/s`，不是 H5 那套 `logBehaviorAction` 像素**（**2026-10-11 抓包实拍**）——
   **出处**：`../mcloud-capture/docs/capture/app-mail139-notice-samples.txt` ✓
   （139邮箱 App 11.4.1 真机抓包 ✓，原始素材 `har/2026-10-11-1029-login.har` ✓）。
   **形状**（`POST https://pesupport.mail.10086.cn/logreport/s?func=user:logDatas&cguid=&sid=&timestamp=&ver=` ✓）：

   ```json
   {"head": {"bindAccounts":"…","comeFrom":"…","netType":"2","version":"11.4.1","eMode":"1",
             "osVersion":"26.6","os":"IOS","appCode":"…","installPack":"…ipa",
             "manufacturer":"Apple","userAccount":"…","model":"iPad8,6"},
    "body": {"logDatas": [ {"eventTime":"2026-10-11 08:28:34","thingId":"2","actionId":"110635"} ] }}
   → {"code":"S_OK","summary":"S_OK"}
   ```

   ⇒ 键是 **`actionId` + `thingId`**（可带 `ext1` ✓）—— **与 H5 页那一套 `addBehavior(action, thingId)`
   是同一个编号体系** ✓，但**通道不同** ✓：H5 打的是 `logBehaviorAction` 像素 ✗。
   **实拍到的编号**（都在 `thingId` 1..9 上 ✓，含义这一趟**没验** ✗）：
   `110296` / `110625` / `110630` / `110633` / **`110635`（= 用户点「开启139邮箱通知」那一下 ✓，有口述佐证）** /
   `110640` / `110644` / `110652` ✓。
   同族还有 `func=user:activeAction`（`{"body":{"logDatas":[{"startType":"1","eventType":"1","eventTime":"…"}]}}` ✓）
   与 `func=user:saveAuthorizeUser`（`<object>…confirmType=1|3…forceConfirm=0|1…</object>` ✓ = **授权/通知同意的落账** ✓）。

   **为什么记在这里**（三条都还没有定论 ✓）：
   * **对本节的 1029** ✓：16 条候选里我**一次都没用过这条通道** ✗（只用了 `logBehaviorAction` 像素与
     `user:activeAction` ✓）⇒ 如果账单那一趟走原生上报，它就会以 `actionId` 的形式出现 ✓
     —— **下一趟抓包要盯的就是这一格** ✓。
   * **对 550「开启云盘APP通知领好礼」/ 551「连续31天开启云盘APP通知」**（本表里都还是 ❌ 未实现 ✓）：
     "通知已开"这件事要用这条通道报 ✓（云盘侧的同类口这一趟没拿到 ✗ —— `yun.mcloud.139.com` 全 TLS 被拒 ✓）。
   * **对 1021「连续31天开启邮箱APP通知」**（走 `notice.rs` ✓）：这条是**邮箱侧**的同类动作 ✓
     ⇒ 若 `notice.rs` 现在打的是别的口，值得拿这份形状对一遍 ✓。
   同趟还拿到 **推送注册那一族**（`pnsmsg-svr.mail.10086.cn:8443/rsv/{deviceBind, unDeviceBind,
   getUserConfig, setUserConfig, clearNotifyCnt}` ✓ —— 响应是 `<responseData><resultCode>…` ✓ 不是 JSON ✓，
   `configId` 是"哪一类通知"那一格 ✓ 只见 `11` ✗）与 **139PushMail 别名**（`139pe.mail.10086.cn:9443/scanWeb/QueryAliasService` ✓，**XML 体** ✓）✓
   —— 都在那份样本里 ✓。

16. ⚑ **`behaviorData` —— 账单页每一发都挂的"行为标签"（明文 ✓，2026-10-11 实拍）**：
   形状 `<action>_<thingId>`（可 `|` 串多对 ✓），挂在查询串上 ✓。实拍那一趟（`html5.mail.10086.cn` ✓）：
   `getCommConfig` → **`108365_66`** ✓ / `logBehaviorAction` → **`110542_6`** ✓ /
   `GetUserAddrJsonDataV2` → `110542_2|30129_82` ✓ / `getTrustedAuthState` → `110511_46` ✓。
   ⇒ 读法是"**我这一发属于哪个行为**" ✓ —— 和像素自己的 `action`/`thingId` 是**两回事** ✓
   （同一发像素上是 `action=111631&thingId=261` ✓ 而 `behaviorData=110542_6` ✓）。
   ⚠ **本仓此前一发都没带** ✗（所以"发过像素但不翻账"未必是像素没用 ✓，可能是**标签缺了** ✓）；
   ⚠ 也**别再把它当密文** ✗ —— 它是明文，可照抄 ✓。
   **连带纠正**：本仓常量 `BILL_VIEW_ACTION=108365` / `BILL_THING_ID=66` 是从页面包
   `CONFIG_INFO[1000006].loadCount` **推**的 ✗ —— 实拍里它**只作为 `behaviorData` 标签出现** ✓，
   **没作为像素的 `action` 发过** ✗；那一趟真正发的像素只有 **`action=111631&thingId=261`** ✓。
   出处：`../mcloud-capture/docs/capture/app-task-run-account-samples.txt` **§12.7/§12.8** ✓。


17. ✅ **1029「查看"我的账单"」—— 从"16 条候选全败"到翻账（2026-10-11）**
   * **现象**：`WAIT → WAIT` ✗，前后**四轮**都没动（候选 16 条 + 真机复刻三趟 ✓）。
   * **真因**：缺 **`disk:mailReportToMQ`** —— 账单页在"打开那一封信"之后连发的**三发**
     （`actionType=1003` ×2 + `1017` ×1 ✓，体是 **JSON** ✗ 而头写 `application/xml` ✓ 实拍如此 ✓）。
     **形状**：`POST {HTML5_MAIL_HOST}/mw2/file/disk?func=disk:mailReportToMQ&sid=&rnd=<0.xxx>&cguid=&comefrom=166`
     ＋ `Content-Type: application/xml` ＋ `Referer: …/html/readmail.html` ✓
     ＋ 体 `{"actionType":"1003","clientType":"2","imapId":"<mid>","clientVersion":"h5","channel":""}` ✓
     → `{"code":"S_OK","summary":"successed to process!"}` ✓。
   * **修法**（最小 ✓）：`crates/mcloud-upstream/src/mail139.rs` 加
     `mail_report_to_mq_request` + `mail_report_body` ✓；`AppRunner::BillView` 在"读信"之后照实拍发三发 ✓。
     ⚠ 顺带补齐的同族（都在同一条链上、都有实拍出处 ✓）：`read_message_request`（`comefrom=166` ✓）、
     `bill_page_request` 补 `sendId=1000006` ✓、打卡后半发 `visitlog/journaling` ✓、像素 `111631/261` ✓。
   * **归因**（一次只加一格 ✓）：run **379/380/381**（页面/列表/标已读/读信/像素/`behaviorData` ✓）
     全 `WAIT → WAIT` ✗ ⇒ run **382** **只多**了三发 MQ 上报 ⇒ `WAIT → **FINISH**` ✓
     ⇒ **归因干净：缺的就是它** ✓（三发里具体哪一发必要**未再细分** ✗ —— 用户口径「试出来再停」✓）。
   * **真机验收**：**移动** `b73bb07d5971e8e7` run **382** `WAIT → FINISH` ✓（`上游已确认` ✓）。
   * **测试数**：`cargo test -p mcloud-upstream --lib mail_report` **2** ✓、
     `-p mcloud-tasks --lib runners::` **14** ✓、`visitlog` **2** ✓。
   * ⚠ **两条老教训**（这次都用上了 ✓）：① 归因靠"同一串只多一格" ✓，不靠"改了两处就一起记功" ✗；
     ② **读状态前先确定账号** ✓（见本表 §5-15 那一节与 `../mcloud-capture/docs/capture-guidelines.md` §1.4 ✓）。


## 6. 附录：账号侧读数（**不是本项目进度**）

这一节是**上游/账号自己的记账**，只用来回答两件事：① 哪些条目现在就不必做（已记完成 ⇒ 第 1 道门会挡）；② 已实现的那几条**上游认不认**（旁证）。

| 项 | 值 |
|---|---|
| 命令（**只读**） | 浏览器开 `/upstream` 点「拉取」；或 `curl -s 'http://127.0.0.1:3000/api/upstream/app-task-list?account=0f6050105ab111f4'`（要带会话 cookie ✓） |
| 时刻 | 2026-10-04 16:00:17（北京时间） |
| 落点 | `data/history.json` 的 **run 93**（`details.reported.items`） |
| 规模 | 16 次拉取（2 market × 8 group）⇒ 原始 35 条 ⇒ 去重 **25 条** |
| 账号 | `0f6050105ab111f4`（测试号）；`settings.skip_tasks = []` |
| 月门输入 | `data/state.json` 里**没有** `cur_month_backup` ⇒ 第 4 道门本轮**不生效**（保守口径） |
| 另一次读数（供 106 验收） | 小号 `9f611bba8206ca0a`，17:07:52（run 99）：**28 条**（比测试号多 1029 / 1078 / 1079 ⇒ 条目集合是**按账号**变的 ✓），未完成 26 / 已完成 2（**106**、118）✓ |

逐条 `state`（**账号侧数据**；变化只说明上游怎么记账，不改变 §0 的完成度）：

| taskId | state | market | group | limitType |
|---|---|---|---|---|
| 106 | FINISH | sign_in_3 | day | DAY |
| 113 | WAIT | sign_in_3 | month | MONTH |
| 118 | FINISH | sign_in_3 | hidden | EVER |
| 120 | WAIT | sign_in_3 | time | EVER |
| 409 | WAIT | sign_in_3 | month | MONTH |
| 431 | WAIT | sign_in_3 | cloudEmail | MONTH |
| 434 | WAIT | sign_in_3 | month | MONTH |
| 522 | WAIT | sign_in_3 | time | MONTH |
| 547 | WAIT | sign_in_3 | time | MONTH |
| 548 | WAIT | sign_in_3 | time | EVER |
| 550 | WAIT | sign_in_3 | time | EVER |
| 551 | WAIT | sign_in_3 | time | MONTH |
| 604 | WAIT | sign_in_3 | month | MONTH |
| 607 | WAIT | sign_in_3 | month | MONTH |
| 609 | WAIT | sign_in_3 | time | MONTH |
| 615 | WAIT | sign_in_3 | beiyong1 | EVER |
| 617 | WAIT | sign_in_3 | time | EVER |
| 618 | WAIT | sign_in_3 | time | EVER |
| 1004 | WAIT | newsign_139mail | month | MONTH |
| 1005 | WAIT | newsign_139mail | time | MONTH |
| 1014 | WAIT | newsign_139mail | month | MONTH |
| 1017 | WAIT | newsign_139mail | month | MONTH |
| 1020 | WAIT | newsign_139mail | month | MONTH |
| 1021 | WAIT | newsign_139mail | month | EVER |
| 1057 | WAIT | newsign_139mail | time | MONTH |

复跑（CLI 只打汇总，逐条明细在运行记录里）：

```bash
cd <repo>
# 开 /upstream 点「拉取」（数据落在响应里，**不落运行历史** ✓）
python3 - <<'PY'
import json
h = json.load(open('data/history.json'))
# ⚠ 不能按数组顺序取"最后一条"：要按 run_id 取最大（踩过）
r = max((x for x in h if x.get('task') == 'app-task-matrix'), key=lambda x: x['run_id'])
rep = r['details']['reported']
print('run', r['run_id'], rep['stateCounts'], 'dedup', rep['dedupedCount'])
for i in sorted(rep['items'], key=lambda i: i['taskId']):
    print(f"{i['taskId']:5} {i['state']:7} {i.get('marketName'):16} {i.get('groupId'):12} {i['name']}")
PY
```

⚠ 「上游状态」页那一格是**只读**的；`run app-task` 是**写**的（会打卡、会传文件）✗ —— 别拿后者当"验证"。

## 7. 这张表**不**声称的东西

* **不拿账号状态当进度**：`state` 是上游的账 ⇒ 只在 §6 出现；
* **只覆盖一个账号**（`0f6050105ab111f4`）：别账号条目集合可能不同（黑名单与月门是代码级的、一致；`skip_tasks` 可能不同）；
* L4 全量门禁本轮未跑 ✗。

---

**更新方式**：改了实现 ⇒ 更新 §0 总览与 §2 对应行的「实现 / 自动验证」；重跑 §6 两条命令 ⇒ 只更新 §6（**不动** §0/§2 的进度）；**「手动验收」那一列只有你能勾** ✓。
