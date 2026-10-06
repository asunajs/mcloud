# appTask（App 任务）· 实现 / 自动验证 / 手动验收

> 这张表只讲**本项目**的三件事：**实现进度**、**自动验证结论**、**你的验收**。
> ⚠ **账号侧的状态（`WAIT` / `FINISH` 之类）不是本项目的进度** —— 它是上游自己的记账，
> 只作**证据输入**，一律放在 §6 附录，不出现在主表里 ✗。
>
> 数据来自**只读**那一档（`app-task-matrix`）：不打卡、不上传 ✓。

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
| 409 | 从固定入口访问云朵中心 | ✅ 已实现：固定入口（埋点 → 停一拍 → 补一发打卡，key `task2`） | `Step::Entry` → `entry_do_task()` | ✅ 单测 4 处；真机 `{clicked:true, problems:[]}` ✓ | ☐ 跑一次看两发打卡与埋点 |
| 431 | 去使用移动云手机 | ❌ 未实现（两边都无 runner；**参考会打卡，本仓连打卡都不发**） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 434 | 分享文件领好礼（原始 name 带 HTML） | ✅ 已实现：分享本趟传上去的第一个文件、随即删外链；**手里空着就自己传一个** ✓；**成没成按"次数涨没涨"判**（`shareBefore`/`shareAfter`/`shareCounted`） | `share_first_uploaded()`；打卡门 `should_click()`；判据 `settle_share_progress` + `share_progress_from_name`；**计划里排末尾**（`sort_by_key(task_id == 434)` ✓）（**2026-10-04 修**，见 §5-7） | ✅ 单测 18 处 + 运行级 **5 条**（打卡门正反 ✓ + 次数正反 ✓ + **顺序/补传** ✓；三条红测都验过 ✓） | ☑ **已验收**（run 104，2026-10-04 19:37，测试号 `onlyTaskIds=434`：`uploaded:1` ✓ `shared:true` ✓ `0/7 → 1/7` ✓ `shareCounted:true` ✓ `acted=1`、success、文件已收回 1/1 ✓） |
| 522 | 当月上传文件满100个 | ✅ 已实现：上传·每趟 5 件、上限 100；**跑完报进度**（`progressBefore`/`progressAfter` + 日志一行 ✓） | `ROUNDS_PER_RUN` / `STOP_AT_PROCESS`；进度 `settle_upload()`（**2026-10-04 加**，见 §5-7） | ✅ 单测 16 处 + 运行级 **2 条**（进度正常 ✓ / 差额说明 ✓，都断言了**日志行** ✓，撤掉即红 ✓） | ☑ **已验收**（run 105/106/107，2026-10-04 19:55-19:58：日志打出 `48/100 → 52/100`、`53/100 → 58/100`、`58/100 → 63/100` ✓；明细两格同步 ✓） |
| 547 | 加入一个知识库 | ❌ 未实现（同 431） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 548 | 开启备份领好礼 | ❌ 未实现（⚠ 它在 `time` 组，**不是**月门那条） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 550 | 开启云盘APP通知领好礼 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 551 | 连续31天开启云盘APP通知 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 604 | 去云盘图书馆阅读书籍 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 607 | 下月签到领惊喜福利 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 609 | 和AI助手对话 | ❌ 未实现（参考亦无 runner） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 615 | 记录国庆节欢乐瞬间 | ✅ 已实现：`beiyong1` 的**按钮门** ⇒ 一次传 **10 张 `.jpg`**（`process` 已走的先扣掉 ✓） | `beiyong1_button_gate()` / `UPLOAD_TEN`；形状四格 `rounds=10 / stop_at_process=10 / per_round_ms=300 / file_ext=".jpg"`；门读 `button_app_link`（`app_task_list_v2.rs` 的 `button_app_link`，**2026-10-04 加**，见 §5-7） | ✅ 单测 4 处（门 3 + 解析 1）+ 运行级 **2 条**（10 张 ✓ / 补差额 ✓；都断言了 `.jpg` 与**进度分母** ✓，撤门或撤"剩余量"即红 ✓） | ☑ **已验收**（run 110，2026-10-04 20:30，测试号 `onlyTaskIds=615`：`clicked:true` ✓ `uploaded:10` ✓ `problems:[]` ✓ `收回 10/10` ✓ ⇒ run 111 矩阵 **`WAIT` → `FINISH`**、已完成 3 → 4 ✓；**当场入账** ✓） |
| 617 | 体验智能消除 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 618 | 体验AI相机 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1004 | 给好友发邮件 | ❌ 未实现（参考 **`sendMailTask`**，TS `runners.ts:154-159`；Go 侧记在 `toolGaps`，`dispatch.go:490`；本仓 **mail139 整族未实现**） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1005 | 从固定入口进入云朵中心 | ✅ 已实现：固定入口（打卡 key 用默认 `task`） | `Step::Entry` → `entry_do_task()` | ✅ 单测 4 处；真机 `{clicked:true}` ✓ | ☐ 跑一次看明细 |
| 1014 | 体验“PDF转换”功能 | ❌ 未实现（参考 **`pdfConvertTask`**，TS `runners.ts:168-173`；Go `MailPDF`，`dispatch.go:422`；mail139） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1017 | 登录移动云盘云朵中心 | ❌ 未实现 | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1020 | 完成一次邮件转存 | ❌ 未实现（参考 **`backupMailTask`**，TS `runners.ts:161-166`；Go `MailBackup`，`dispatch.go:426`；mail139） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |
| 1021 | 连续31天开启邮箱APP通知 | ⛔ 矩阵里**不做**（**与参考一致** ✓）—— 它的真身是**另一条任务** `notice` 的**邮箱那一半**（2026-10-06 已落码 ✓，见 §5-2） | 矩阵不碰它；实现在 `crates/mcloud-tasks/src/notice.rs` + `crates/mcloud-upstream/src/app_notice.rs` | ✅ `cargo test -p mcloud-tasks --lib notice`（6 条邮箱钉子）+ `cargo test -p mcloud-upstream --lib app_notice` | ☐ 跑一次 `notice`，看明细里那条 `noticeEmail`（`day` / `canReceive` / `reported`） |
| 1057 | 体验AI工作台 | ❌ 未实现（⚠ **参考侧也没有 runner**：TS 全仓 **0 命中 1057**；Go 只把它归成 `VerifyStillOpen`「需前往邮箱完成」（`task_matrix_report_test.go:123`，组 `cloudEmail`）⇒ 不是「漏做」） | — | ❌ 单测 0 处 | ☐ 待实现后验收 |

## 3. 自动验证（四层，各自能证明什么）

| 层 | 命令 | 它证明什么 | 现在读数 |
|---|---|---|---|
| L1 单测 | `cargo test -p mcloud-tasks --lib app_task` | 判据/门/形状的**逻辑** | **57 passed / 0 failed** ✓（2026-10-04 16:0x） |
| L2 真机端到端（**写**） | `run app-task <账号> --body '{"onlyTaskIds":"409"}'` | 真的发得出去、上游认账 | 已验：106 ✓、409 / 522 / 1005 各跑过一次 ✓（**旁证** = 上游记账，见 §6） |
| L3 界面门禁 | `node scripts/gates-web.mjs` | 页面交互不回归 | ⚠ **夹具里没有 `app-task`**（只有 daily-checkin / receive / live-room / msg-push 四条假任务，`web/lite/regress/lib.mjs:231-259`）⇒ appTask 界面自动化覆盖 **= 0** ✗ |
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

## 6. 附录：账号侧读数（**不是本项目进度**）

这一节是**上游/账号自己的记账**，只用来回答两件事：① 哪些条目现在就不必做（已记完成 ⇒ 第 1 道门会挡）；② 已实现的那几条**上游认不认**（旁证）。

| 项 | 值 |
|---|---|
| 命令（**只读**） | `MCLOUD_DATA_DIR=data ./target/debug/mcloud run app-task-matrix 0f6050105ab111f4` |
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
MCLOUD_DATA_DIR=data ./target/debug/mcloud run app-task-matrix 0f6050105ab111f4
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

⚠ `app-task-matrix` 是**只读**那一档；`run app-task` 是**写**的（会打卡、会传文件）✗ —— 别拿后者当"验证"。

## 7. 这张表**不**声称的东西

* **不拿账号状态当进度**：`state` 是上游的账 ⇒ 只在 §6 出现；
* **只覆盖一个账号**（`0f6050105ab111f4`）：别账号条目集合可能不同（黑名单与月门是代码级的、一致；`skip_tasks` 可能不同）；
* L4 全量门禁本轮未跑 ✗。

---

**更新方式**：改了实现 ⇒ 更新 §0 总览与 §2 对应行的「实现 / 自动验证」；重跑 §6 两条命令 ⇒ 只更新 §6（**不动** §0/§2 的进度）；**「手动验收」那一列只有你能勾** ✓。
