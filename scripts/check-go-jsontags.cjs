#!/usr/bin/env node
// 结构体字段的 `json:"…"` tag 门禁。
//
// # 为什么要有这条
//
// Rust 侧的 `#[derive(Serialize)]` 默认给 **snake_case** 字段名
// （`password_hash` / `cloud_count`），而 Go 不带 tag 会输出**字段名原样**
// （`PasswordHash` / `CloudCount`）。同一个字段两个名字，而且**不报错** ——
// 只有真跑起来、另一侧读这条记录的时候才看得见。
//
// 这个坑在 mcloud 里已经犯了**三次**，分属三个不同模块：
//   - `CloudStats`（pure 那一步带过来的）；
//   - `User` / `Session`（user 那一步带过来的）；
//   - `model` 里那一批（这次迁移时逐个补上的）。
//
// 三次的共同点是：**发现它们靠的是"改坏了却不响"，不是靠测试**。
// 往返测试用什么字段名都是绿的 —— 它只证明"自己解得回来"，
// 不证明"和另一侧同名"。所以必须有一条不依赖往返的守卫。
//
// # 判据
//
//   - **混合**（有些字段有 tag、有些没有）→ **直接失败**。
//     这是无歧义的错误：一个结构体不可能"一半进 JSON、一半不进"。
//   - **全都没有 tag** → 必须在下面的白名单里，否则**失败**。
//     零 tag 有两种可能：① 它确实不进 JSON（内部类型）；
//     ② 它整个漏写了。脚本分不出来，所以交给白名单 ——
//     **新增一个无 tag 的结构体必须显式登记并写明理由**，
//     这就是"让它自己报出来"的那一下。
//
// # ⚠ 白名单不是"批准"，是"待复查"
//
// 白名单里的每一条都写了理由。**理由过期了就要删**：
// 哪天 `AuthInfo` 开始要进 JSON，它的正确做法是**加上 tag 并从这里删掉**，
// 而不是继续待在名单里。
//
// 用法：node scripts/check-go-jsontags.cjs

'use strict'

const fs = require('fs')
const path = require('path')

const ROOT = path.resolve(__dirname, '..')
const SCAN_DIRS = ['cmd', 'internal']

// 白名单：`相对路径:结构体名` → 为什么不进 JSON。
//
// ⚠ 加条目时必须写理由。没有理由的条目等于"shenme 都没说"，
// 下一次有人看到它时无法判断该不该继续留着。
// ⚠ 只有"**有导出字段**、但一个 tag 都没有"的才需要登记。
// 全是非导出字段的结构体（`Error` / `enumJSON` / `phcParams` / `cronField` /
// `CronSchedule`）脚本会**直接跳过** —— `encoding/json` 本来就看不见它们，
// 登记进去反而会让白名单越来越长、越来越没人看。
const UNTAGGED_ALLOWED = {
  // 这两条确实不进 JSON：`QrFailure` 实现了 error（对外走 `Error()` 文案），
  // `QrProgress` 是内存里的推进结果（`UserInfo any` 已经是解开的 JSON 值）。
  //
  // ⚠ 后者那条理由**带一个前提**：一旦哪天要把扫码进度直接推给前端
  // （比如 WebUI 想原样展示"已扫码，请在手机上确认"），正确做法是
  // 在 app 层**另写一个带 tag 的响应体**，而不是给 `QrProgress` 补 tag ——
  // `UserInfo any` 的形状由上游决定，把它钉进响应体等于把上游的字段约定
  // 写进我们自己的契约。
  'internal/capabilities/login.go:QrFailure':
    '实现 error 接口，对外走 `Error()` 文案，不整块 marshal',
  'internal/capabilities/login.go:QrProgress':
    '扫码登录的**内存**推进结果（`UserInfo any` 已是解开的 JSON 值），本身不是响应体',
  'internal/upstream/app_signin_status.go:SignInDay':
    'infoV3 日历格里的一天（`cal[]`），内存解析结果；Rust 侧 derive `Debug, Clone, Copy, PartialEq, Eq`，**不** Serialize',
  'internal/upstream/app_signin_status.go:ReceiveItem':
    '账目里的一笔（`receiveList[]`），内存解析结果；它要进 JSON 时走 `mcloud-app` 手工拼的响应体，不整块 marshal',
  'internal/upstream/app_signin_status.go:SignInStatus':
    'infoV3 的**整份解析结果**（余额 / 日历 / 账目 / 两个翻倍门槛），同上：Rust 侧不 Serialize，对外由 app 层另拼',
  'internal/upstream/app_signin_signin.go:SignInResult':
    'startSignIn 的解析结果（**调用前**的快照），同上；⚠ 它**不是**成败判据，别把它当成对外结论序列化出去',
  'internal/upstream/app_signin_session.go:H5Signature':
    '一次 H5 请求的**四个签名头**（request-id / timestamp / nonce / signature），是内存里的计算结果，不落库也不出响应',
  'internal/upstream/app_signin_session.go:H5Login':
    'tyrzLogin 换回的**会话凭据**（jwt + 回显手机号），进程内存里用；Rust 侧 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  // ── AI 工作台那一族（玩法 1028，2026-09-26 登记）──
  // 四枪的**入参集 / 解析结果**全部不面向网络：请求体一律手工拼 `map[string]any`
  // （键名与顺序按旧 TS 逐字，见 `app_ai_workbench.go` 的 `AIWorkbenchChatBody`），
  // 响应侧是手解 `map[string]any` 后摊成这几个结构体。
  // ⚠ 其中 `AIWorkbenchDomainAuth` 装的是**本族现换的凭据本体**（DES 解出的手机号 +
  //   ①′ 的 token + 派生 Basic 整串，三格都是 `core.SecretValue`）——
  //   而 `SecretValue.String()` 只挡打印、**不挡 `encoding/json`**（`internal/core/model.go`）。
  //   给它补 tag = 将来谁"顺手把 auth 写进日志 / 响应"就把整串凭据原样带出去，
  //   这正是本轮成文里那条"凭据只进不出"的最后一道闸，**不许**为了过门禁而加 tag。
  'internal/upstream/app_ai_workbench.go:AIWorkbenchDomainAuth':
    '①′ 派生出来的**凭据三格**（手机号 / token / Basic 整串，均 `core.SecretValue`），只在内存里传给 ②′③′；⚠ 刻意不 marshal（`String()` 挡不住 JSON，见 `internal/core/model.go`）',
  'internal/upstream/app_ai_workbench.go:AIWorkbenchTaskStatus':
    '②′④′ 的**解析结果**（`code` / 有没有 `result` / 两枚状态 / 那道门），供编排层分档念；对外那份是事件流里 `workbenchStatusLine` 拼出的一行人话，不是 JSON',
  'internal/upstream/app_ai_workbench.go:AIWorkbenchChatParams':
    '③′ 的**入参集**（`userId` / 问话 / `inputTime`），请求体由 `AIWorkbenchChatBody` 手工拼 `map[string]any`（键名与顺序按旧 TS 逐字）⇒ 这个结构体本身不进 JSON',
  'internal/upstream/app_ai_workbench.go:AIWorkbenchChatRead':
    '③′ SSE 的**解析结果**（事件数 / 末条 `code` / `message` / `sessionId` / 累出的回话），出网只出现在 `workbenchReadLine` 那一行播报里（且 `sessionId` **只念长度**，见 R38）',
  'internal/upstream/smid.go:SmUpload':
    '一次上报的**时间与上一次的 deviceId**。时间由调用方给（东八区），旧 deviceId 只在本次请求里用；Rust 侧同样是纯入参结构，不 Serialize',
  'internal/upstream/smid.go:SmRule':
    'DES 规则表的**一条**（密钥 + 混淆后的键名），是常量表的元素不是数据模型；Rust 侧这条 rule 表也是 `const` 不是 Serialize',
  'internal/app/smid.go:UploadContext':
    '一次上报的时间上下文（14 位东八区时间 + nowMillis + 上一次的 deviceId），**只**喂给上游算法；Rust 侧 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  'internal/app/login_hub.go:LoginSessionView':
    '登录会话对外的**结论**（阶段 / 掩码账号 id / 人话失败原因 / 验证码余量），内存里在组装根之间传递；出网那一步由 `internal/server/login.go` 的 `loginSessionPayload` **逐字段**抄成响应体 —— 那里写明不给它补 tag 的理由：这个结构体将来会加字段，而"加了就自动出网"正是凭据泄漏最常见的路径',
  // ── 刷新 auth 这一条链路（2026-09-22 登记）──
  // 这六个都是**内存里的入参 / 中间结论**，而刷新恰恰是"轮换真凭据"的操作，
  // 所以对外形状刻意**不**由结构体自动 marshal：
  //  - `RefreshReport` 出网走 `internal/server/auth_refresh.go` 的 `refreshAuthPayload`
  //    （逐字段列白的 map），理由与上面 `LoginSessionView` 同一条；
  //  - `RefreshedCredential` **就是那套新凭据本体**（token + 整串 + 到期时刻）——
  //    它一旦能被 marshal，离泄漏只差一次"顺手把 report 整个写出去"；
  //  - 其余四个（入参集 / 状态机中间结论 / CLI 参数 / 上游响应解析结果）根本不面向网络。
  'internal/app/auth_refresh.go:RefreshDeps':
    '一次刷新的**入参集**（上游 / 库 / scope / 时钟 / 随机 / 端档案），不是数据模型；Rust 侧这里是六个独立参数',
  'internal/app/auth_refresh.go:RefreshReport':
    '一次刷新的**结论**（端别 / 判到哪档 / 是否换掉 / 前后到期时刻），出网由 `refreshAuthPayload` 逐字段抄；⚠ 里面**没有**凭据，补 tag 也不会漏，但那条纪律只有一处实现',
  'internal/capabilities/auth_refresh.go:RefreshOutcome':
    '状态机的**中间结论**（判据 + 换到的新凭据 / 失败原因），内存里交给 app 层落库；Rust 侧是 `enum`，不 Serialize',
  'internal/capabilities/auth_refresh.go:RefreshedCredential':
    '**新凭据本体**（整串 `Authorization` + token + 到期毫秒），两个字段是 `core.SecretValue` —— 而它 `String()` 只挡打印、**不挡 `encoding/json`**（见 `internal/core/model.go` 那句"JSON 序列化不受影响"）。所以给它补 tag = 一旦哪个 handler 整个 marshal 出去，新 token 就**原样出网**；落库走 `RotateAccountAuth` 窄写，这条路不经过 JSON',
  'internal/cli/args_auth.go:AuthRefreshArgs':
    '`auth refresh` 的参数（`--account` / `--all` / `--lead-days` / `--platform` 等）；Rust 侧是 `#[derive(Args)]`，进 JSON 的只有 `--json` 那支另构造的输出',
  'internal/upstream/auth_refresh.go:RefreshTokenResult':
    'refreshToken 响应的**解析结果**：`Token` 就是新 token 本体（同上，`string` 而不是 `SecretValue`，因为它一出这道门就会被包进 `RefreshedCredential`），`ExpireSeconds` 是"还能用多久"。两者都只在内存里换算，出网只出现在上面那个逐字段列白的 map 里',
  // ── 编排入口与受理配对（2026-09-23 登记，调度循环那一步带来的）──
  // 这一族是"跑一个账号"的**入参集 / 内存结论**：字段里装着库句柄、scope、
  // 上游客户端与登录态，一旦能被整体 marshal，离泄漏只差一次"顺手把结果写出去"。
  // 对外形状一律在 server 层逐字段列白（见 `runPairings`、`refreshAuthPayload`）。
  'internal/app/signin_run.go:SignInRun':
    '签到那一趟的**入参集**（库 / scope / 上游 / 设备档案 / 时钟 / 随机 / 是否领取 / 进度回调），不是数据模型；Rust 侧这里是 `run_signin` 的一组独立参数',
  'internal/app/signin_run.go:SignInAccountResult':
    '一个账号这一趟的**内存产出**（签到结论 + 逐条领取 + 余额快照），外层交给编排层换算成 `core.RunReport` 与事件流；出网的那份是另构造的响应体，不整块 marshal',
  'internal/app/auth_refresh_run.go:RefreshRun':
    '刷新那一趟的**入参集**（同 `SignInRun` 一族），字段里有 `core.StateStore` 与 `core.Scope`：给它们补 tag 无意义，而"整个结构体能序列化"这件事本身会被误用；`LeadDays` 与 `PCPlatform` 是**这台机器定的数**，刻意不对外',
  'internal/server/run_dispatch.go:acceptedRun':
    '一次受理的**内存配对**（账号 ↔ runId）。对外那份由 `internal/server/runs.go` 的 `runPairings` 逐字段抄成 `{"account":…,"runId":…}` —— 理由同 `LoginSessionView`：这个结构体会加字段（比如内部序号、重试次数），而"加了就自动出网"正是契约漂移最常见的路径',
  // ── 签到页那一族（2026-09-23 登记，`receive` / 三条开关 / `prizes` / 快照接上执行带来的）──
  // 同一族规矩：`SignInWorld` 是**入参集**（装着库句柄与 scope），另外四个是**内存结论**
  // （外层换算成 `core.RunReport` 与事件流才出网，且对外只出现小结那一句人话）。
  // ⚠ `ClaimResult.Gained` 一类字段将来要进界面时，走 server 层逐字段列白的响应体，
  // 而不是在这里补 tag —— 与上面 `LoginSessionView` 那条同一条理由。
  'internal/app/signin_world.go:SignInWorld':
    '跑签到页那一趟的**入参集**（库 / scope / 上游 / 设备档案 / 时钟 / 随机 / 票池 / 进度回调），字段里全是句柄；Rust 侧这里是一组独立参数，不 Serialize',
  // ── 推送（通知）那一族（2026-09-24 登记，`internal/core/mail.go` 带来的）──
  // ⚠ 同一族里登记的都是**不面向网络**的那几个：`NotifyConfig` 与十个渠道配置
  //（库里那一行 `notify_config` 存的就是它）带 tag 进 JSON，所以"凭据不整体出网"
  // 那条纪律落在 server 层：对外走逐字段列白的**脱敏视图**（落在
  // `internal/server/notify_config.go`，随该文件落地），并要有一条测试钉
  // "响应体里不许出现任何已配的密钥"。
  'internal/core/mail.go:MailMessage':
    '一封要交给 SMTP 实现的**内存信**（`Username` / `Password` 是授权码本体）。它一旦能被整体 marshal，"顺手把消息写进日志或响应"就等于把授权码出网；`SecretValue.String()` 只挡打印、不挡 `encoding/json`（见 `internal/core/model.go`），所以这里靠"没有 tag"挡。推送正文对外只有小结那一句，走 app 层的字符串，不经过这个结构',
  'internal/upstream/notify.go:NotifyResult':
    '一家渠道这一次的**内存结论**（渠道名 + 三档 + 一句人话）。`Reason` 里放的是"缺哪个字段 / HTTP 状态码 / 渠道回的那句 / 传输层错误的**类别**"（见 `transportReason`），本身不含凭据；但不给 tag 是为了让"整个 report 被顺手 marshal 出去"这条路走不通 —— 对外那一族形状将来在 server 层逐字段列白（同 `LoginSessionView` 的理由：结构体会加字段，而"加了就自动出网"是契约漂移最常见的那条路）',
  // ⚠ 上面那条"将来在 server 层逐字段列白"今天**已落地**：`internal/server/notify.go`
  // 的试发响应与 `notify_view.go` 的脱敏视图都是逐字段构造的 map，这四个结构体
  // 一个都没有被整块 marshal 过。
  'internal/app/notify_run.go:NotifyLogLine':
    '一条要进推送正文的**过程话**（档位 + 时刻 + 文本）。它是 `FormatNotifyBody` 的入参，出网的是**拼好的那一段字符串**（正文），不是这一条的形状；补 tag 会把"正文里每行长什么样"（`[info 14:00:00]…`）变成两份真相',
  'internal/app/notify_run.go:NotifyInput':
    '一次收尾推送的**入参集**（这一趟的结论 + 过程话），字段里装着 `core.RunReport` 与切片；Rust 侧这里是 `dispatch_notify` 的一组独立参数，不 Serialize',
  'internal/app/notify_run.go:RunNotifyResult':
    '一次收尾推送的**内存结论**（跳过原因 / 逐家结果 / 家数）。`Report` 就是上面那个 `NotifyResult` 的切片（同样不 marshal）；对外只有 `SummaryLine()` 那**一句人话**，走 `RunSummary` 的字符串，所以这一族刻意没有 JSON 形状 —— 界面要逐家展示时走 server 层列白，不在这里补 tag',
  'internal/io/mail.go:SMTPSender':
    'SMTP **发送器本身**（组装根持有的句柄）：`TLSConfig` 里可能装着客户端证书私钥，`dial` 是函数（marshal 到它直接报错）。它是依赖不是数据模型，同 `SignInRun` / `RefreshRun` 那一族',
  'internal/app/signin_claims_run.go:ClaimRun':
    '三条月度开关共用的**入参 + 档位**（`Kind` 决定发哪个请求），不是数据模型；分派表把它建出来后只调用 `RunAccount`',
  'internal/app/signin_claims_run.go:ClaimResult':
    '一次领取的**内存结论**（档位 / 领到多少 / 原因），出网那份是 `core.RunReport` 里逐字段构造的 summary 与待办；⚠ 它带 `Account`，整体 marshal 等于把账号标识塞进每条响应',
  'internal/app/receive_run.go:ReceiveResult':
    '一次领取的**内存结论**（逐条实况 + deviceId 失败原因），同上只经 `RunReport` 出网；`Outcomes` 里装着上游原文，不是对外契约',
  'internal/app/cloud_snapshot_run.go:CloudSnapshotResult':
    '一次余额采集的**内存结论**（余额原值 + 写进去的那条日快照）。落库走 `RecordCloud` 的窄写，出网走 `RunReport` 那句话；给 `Stats` 补 tag 会把 `core.CloudStats` 的形状间接变成契约',
  'internal/app/prizes_run.go:PrizesResult':
    '一次奖品查询的**内存结论**（`[]upstream.PrizeRecordItem`）—— 字段形状由上游决定，整块 marshal 等于把上游的字段约定写进我们自己的契约（同 `QrProgress` 那条理由）',
  // ── 新链路批（2026-09-23 登记：`task-list` / `notice` / `msg-push` / `fun-ai`）──
  // 同一族规矩，但**分两类**登记（别合并成一句"都是内存里的"）：
  //
  //  · `internal/upstream/app_*.go` 那八个是**上游响应的解析结果**：字段名与形状
  //    由上游决定，而且它们大量用指针表达"上游没给"（≠ 给了 0）。补 tag 会先把
  //    "上游的字段约定"写成"我们的对外契约"，而这两件事在本仓库是分开的。
  //  · `internal/app/*_run.go` 那八个是**一次运行的内存结论**：对外只经
  //    `core.RunReport`（小结那一句人话 + 待办）出网；`Error` / `Reason` 两格
  //    装的是**上游原文**，整块 marshal 等于把上游文案当结构化字段承诺给前端。
  //
  // ⚠ 界面将来要逐字段展示某一格时，走 server 层**逐字段列白**的响应体
  //（同 `LoginSessionView` / `runPairings` 那条理由），不要在这里补 tag。
  'internal/upstream/app_task_list.go:TaskItem':
    '`taskListV3` 的**一条**任务（内存解析结果）。实拍里 `state`/`limitType` 的取值集只见过两三种，补 tag 会把这份未穷举的枚举变成契约',
  'internal/upstream/app_task_list.go:TaskList':
    '`taskListV3` 的整份解析结果（条目 + 顶层 `taskFinishState`）。后者语义**未验证**（只存不判），更不能对外承诺它的形状',
  'internal/upstream/app_task_list_v2.go:TaskV2Item':
    '`taskListV2` 的一条任务（嵌 `app_task_list.go:TaskItem` 那六个共用字段 + 这一口才读的 `enable`/`currstep`/`process`/`marketname`）。⚠ 嵌进来的那份 tag 归外层结构体管，所以它必须与 `TaskItem` 同进同退；且 `process` 在夹具三条里恒 0 而 `state` 已是 `FINISH`（2026-09-25 真号读到过 `0/9/59/64/69/73/74/100`、同一趟内会涨、传 5 个文件之后涨 4 还是 5 要看读哪个市场名，涨到 100 那条当天是 `FINISH` ⇒ "进度是几分之几"仍只是相关性读法，不能承诺给前端',
  'internal/upstream/app_task_list_v2.go:TaskListV2':
    '`taskListV2` **一个 group** 的解析结果。`GroupFinishStates` 是"只存不判"那格（夹具一次 `{}`、一次给 `2` 而同批条目全 `FINISH`；真号读到 `0`/`1`/`2` 三档、与"组内是否全 `FINISH`"零例外同向 ⇒ 那是相关性、不是因果，仍不判）；`Group` 是请求时回填的键名，不属于响应字段',
  'internal/app/task_matrix.go:TaskMatrixFailure':
    '一次拉取的失败记录，`Err` 是 `*core.Error`（其对外文案走白名单、原文只进日志）⇒ 整块 marshal 等于把上游原文当结构化字段承诺给前端',
  'internal/app/task_matrix.go:TaskMatrix':
    '十六次拉取聚合出的内存结论。它的消费者是 app 层的执行链（`task_matrix_run.go` 两趟读表：待做集合与复查），聚合结果本身不出接口 ⇒ 对外形状由 `TaskMatrixRunResult` 那一格负责；`Items` 的语义是"按 name 去重、保留 id 小的那条"，去重取向是刻意不同③，补 tag 会把它变成对外契约',
  'internal/app/task_matrix.go:TaskMatrixMarketCount':
    '上面那条 [TaskMatrix] 的**一半**读数（按市场名分开的"读到几次 / 拿到几条"），随之外层一起不进 JSON。`FieldValues` 装的是**上游响应里 `marketname` 出现过什么取值**（照抄上游字面量，用于 R13 的排障），把它钉成对外字段等于让前端依赖一份未穷举的上游枚举',
  'internal/app/task_matrix.go:TaskMatrixGroupCount':
    '同外层的第二半读数：**逐次拉取**的组级面值（`taskFinishState` 与条目 `process`），只为把 R15 那一问念成一行而存在。⚠ 2026-09-25 真跑读出的仍是**相关性**（`0`/`1`/`2` 与"组内是否全 `FINISH`"同日 18 次读数零例外同向；`process` 会涨但没有"共几步"可对照）⇒ 语义未验证、也不参与判定，钉成 JSON 就是替一份未穷举的上游枚举建契约；对外形状仍由 `TaskMatrixRunResult` 那一格负责',
  'internal/app/task_matrix.go:TaskMatrixStateEntry':
    '[TaskMatrixGroupCount] 里的那一枚键值对（组名 + 面值），跟着外层一起不进 JSON。键名是**上游给的**（2026-09-25 十六次读数里每次都恰好等于请求组名，但"只给一枚同名键"是观察、不是契约，反例有没有仍无样本），补 tag 会把这份来源不可控的键集合变成前端可依赖的形状',
  'internal/app/task_matrix_report.go:TaskReportLine':
    '一行**播报**（级别 + 已渲染好的人话）。它是纯内存形状：落库的是渲染后的 `run_events.message` 文本与 `level`，事件 JSON 形状已冻结（`kind=log` 那一份由 `core` 侧构造），所以这一格一旦补 tag 就等于新造一条"半结构化"的对外契约，而前端读的仍是那一句文本',
  'internal/upstream/app_notice.go:NoticeTaskSlot':
    '公告任务的一个槽（`cloudStatus`/`sum`/`progress`/`taskStatus`）。四个字段全是指针，"上游没给"与"给了 0"必须分得开；`cloudStatus` 的语义仍是**猜的对应关系**',
  'internal/upstream/app_notice.go:NoticeTaskInfo':
    '`getTaskInfo` 的整份解析结果（两个槽）。槽本身也是指针 ⇒ marshal 出去会得到 `null`，而前端无法区分"没给这一槽"与"给了但空"',
  'internal/upstream/app_notice.go:NoticeReward':
    '`reward` 的解析结果 —— 实拍响应**连 `result` 都没有**，所以这一格唯一确定的事实是"它常常是 nil"。把它变成对外字段会教前端把空当 0 显示',
  'internal/upstream/app_notice.go:ReceiveTask':
    '`receiveTask` 的解析结果。**本链尚未接入这一口**（零样本），登记它是为了"以后接的时候不用再讨论要不要 tag"',
  'internal/upstream/app_msg_push.go:MsgPushStatus':
    '`msgPushOn/task/status` 的解析结果。⚠ 这一族**零抓包样本**，字段名来自旧项目的文字描述；此时补 tag 等于给一个未经证实的形状加契约',
  'internal/upstream/app_msg_push.go:MsgPushObtain':
    '`task/obtain` 的解析结果（只有 `obtainCode`）。同上：零样本；而且"缺它不等于成功"这条判据只在内存里成立',
  'internal/upstream/app_funai.go:FunaiModule':
    '`funai/api/page/getmodule` 的解析结果（模块 id + 三个计数 + 活动起止）。`drawTimes`/`drawsUsed` 的语义**未验证**，起止时刻的时区解释也是我们的选择，都不能写成对外字段',
  'internal/upstream/app_funai.go:FunaiLottery':
    '`funai/api/page/lottery` 的解析结果（奖品名 / `isWin` / 上游认下的 moduleId）。`prizeType` 的取值表没有成文，`isWin` 只有一档样本 ⇒ 本链不据此分支，更不对外解释它',
  'internal/app/task_list_run.go:TaskListResult':
    '一次任务清单查询的**内存结论**（条目 + 顶层读数），出网那份是 `SummaryLine()` 的一句话；条目形状由上游决定（同 `PrizesResult` 那条理由）',
  'internal/app/task_list_run.go:TaskListCounts':
    '从 `TaskListResult` **算出来**的四个计数（含"状态读不出"那一格）。它是小结的中间量，不是数据模型；"未知"那一格的存在恰恰说明它不该被当成枚举契约',
  'internal/app/notice_run.go:NoticeOutcome':
    '公告任务链的**中间结论**（档位 + 领之前/复查之后的两槽读数 + 上游原文），由 `NoticeRun` 投影成 `NoticeResult`；两代结论都 marshal 会让"复查没跑成"与"复查没给数"在 JSON 里长得一样',
  'internal/app/notice_run.go:NoticeResult':
    '一个账号这一趟的**内存结论**，出网只经 `RunReport`（小结一句 + 待办）。`Gained` 实拍**恒 nil**，补 tag 会让前端把它显示成 0 —— 而那正好是这条链刻意不解释的东西',
  'internal/app/msg_push_run.go:MsgPushOutcome':
    '消息推送链的**中间结论**（档位 + 三个读数 + 上游原文），由 `MsgPushRun` 投影成 `MsgPushResult`；零样本的一族更不该把它的形状钉成契约',
  'internal/app/msg_push_run.go:MsgPushResult':
    '一个账号这一趟的**内存结论**，出网只经 `RunReport`。`Reason` / `Error` 两格装上游原文（同 `NoticeResult` 那条理由）',
  'internal/app/funai_run.go:FunaiOutcome':
    '抽奖链的**中间结论**（档位 + 判据 + 模块 id + 奖品名），由 `FunaiRun` 投影成 `FunaiResult`；`ModuleID` 是排障对账用的，不是对外字段',
  'internal/app/funai_run.go:FunaiReadings':
    '抽奖判据的**原值**（三个计数 + 活动期两界），只经 `Line()` 念进小结那一串文本，从不出网。刻意不 marshal：这几格的**语义未验证**（哪个是"还能抽"、哪个是"累计获得"），钉成 JSON 形状就是替未验证的猜测建契约（同 `NoticeResult` / `MsgPushResult` 那两条理由）',
  'internal/app/funai_run.go:FunaiResult':
    '一个账号这一趟的**内存结论**，出网只经 `RunReport`。⚠ `PrizeName` 为 nil **不等于**"没中奖"（实拍只有一档 `isWin`），这个区别在 JSON 里会被抹平，所以刻意不 marshal',
  // ── 兑换族（2026-09-23 登记，`exchange` 上游层落地带来的）──
  // 同一族规矩，但这一族多一条**只对它成立**的理由：它的字段形状里大量是
  // "上游没给"（指针）与"取值集没穷举"（`limit` 五档里两档零样本、`probability2`
  // 里三个开关语义未验证），而对外契约一旦成文，前端就会把"没给"显示成 0、
  // 把没见过的档当成不存在 —— 这两件事在这条链上都是**会扣豆**的误判。
  'internal/upstream/app_exchange.go:ExchangeMonthQuota':
    '`probability2` 的二次解析结果（本月名额 + 同组奖品 id）。⚠ 那两个字段一个是**数字**、一个是**字符串数组**（坑表 #105 的静默归零就是按"都是数字"读它），补 tag 会把这份异形约定写成对外契约',
  'internal/upstream/app_exchange.go:ExchangePrize':
    '`exchangeList` 里的**一件**奖品（内存解析结果）。除 `prizeId` 外全是指针，且 `limit` 的 `1`/`3` 两档**零样本或两处文案冲突** —— 它的形状目前只够内部判定，不够对外承诺',
  'internal/upstream/app_exchange.go:ExchangeList':
    '`exchangeList` 的整份解析结果（按 groupId 分组的映射 + 排序后的键）。实拍键**不连续**（`0,1,2,5,7,8,9,11,15`），marshal 出去会让前端按"下标数组"理解它',
  'internal/upstream/app_exchange.go:ExchangeOrder':
    '`exchangeV3` 成交那一条的订单（`oid` / 花费 / 两个时间 / 链接）。`insertTime` 是**带时区**的 ISO 串而列表里那两个是裸串，两种时间口径混在一个对外结构里迟早出事；出网走 `RunReport` 的小结那一句',
  'internal/upstream/app_exchange.go:ExchangeResult':
    '`exchangeV3` 的解析结果（业务码 + 文案 + 订单）。⚠ **码是数据不是错误**：`514` 可以重来而 `2301` 不该再试，整块 marshal 等于把"读 code 分流"这件事推给前端',
  'internal/upstream/app_exchange_slide.go:SlidePuzzle':
    '`getSlide` 的题面（两张 PNG 原文 + 三个尺寸读数）。它是**喂给解算器的中间量**：两张图各 ~200KB，一旦能被整体 marshal，一次误打日志就是几百 KB 的题面；且 `puzzleWidth`（96）与像素包围盒宽（95）差 1，这个偏差只在内存里判过，不该被当成对外字段',
  // ── 兑换族·领域层（2026-09-23 登记，`exchange` 判定与链落地带来的）──
  // 与上游层同一族规矩，多一条：**这条链的字段里"没给"与"没做"都比 0 有意义**
  // （余额差没核到 ≠ 核到 0；一枪没发 ≠ 发了没成），marshal 出去一定会被显示层读成 0。
  'internal/app/exchange_plan.go:ExchangeWait':
    '整点等待的**判定结果**（要不要等 + 等到什么时候）。它是 `skipWait=false` 那条路的中间量，出网只有小结里那一句；把 `Until` 写成对外字段等于承诺一个"上游几点开抢"的口径 —— 而我们**只有一个反例**（自己的成功样本发生在 12:16）',
  'internal/app/exchange_run.go:ExchangeOptions':
    '一次兑换的**入参与三个注入点**（`DeviceID` / `Wait` / `Solve` 都是函数）。marshal 它要么静默丢掉三个注入点、要么炸 —— 两种都比"不 marshal"差',
  'internal/app/exchange_run.go:ExchangeOutcome':
    '兑换链的**中间结论**（档位 + 判据快照 + 前后两次余额 + 轮数/枪数 + 上游码与原文），由 `ExchangeRun` 投影成 `ExchangeResult`。⚠ `Status` 的零值是 `NotAttempted`，序列化后前端读不到这一格就会按"没字段"处理，而这条链上"没判成"与"没成交"是两件事',
  'internal/app/exchange_run.go:ExchangeRun':
    '一条能力的**装配体**（内嵌 `SignInWorld`，带库句柄、票池与出站 HTTP），与 `FunaiRun` / `ClaimRun` 同规：它压根不是数据',
  'internal/app/exchange_run.go:ExchangeResult':
    '一个账号这一趟的**内存结论**，出网只经 `RunReport`（小结一句 + 待办）。⚠ 真扣豆那条链的"花了多少豆"要靠**两次余额读数之差**才算得出（判据链第 2 条），整体 marshal 会让前端拿 `Balance` 当花费',
  // ── run 产物账本（2026-09-24 登记，`internal/exec` 那一层带来的）──
  // 它是**内存里的账**：谁在哪个账号名下造了哪个上游对象、清掉了没有。
  // 对外形状将来落在 app/server 层的"待清理清单"响应体里（逐字段列白），不在这里。
  'internal/exec/artifacts.go:Artifact':
    '一条产物的登记（类型 + 账号 + 上游 ID），纯内存账本的键；⚠ 它会被拼进日志与 run 事件的诊断文本，' +
    '所以"能整体 marshal"这件事本身是风险 —— 补 tag 只会让它更容易被顺手写进响应体，' +
    '而清理清单出网时必须连"用谁的凭据删"一起表达，那是响应体的事，不是这个结构体的事',
  // ── 红包派对族（#18，2026-09-24 登记，`internal/upstream/app_cloud_phone*.go` 落地带来的）──
  // 与签到/exchange 那一族同一族规矩（上游解析结果 + 请求侧入参集），但这一族
  // 有**两个只对它成立**的加重理由：
  //  ① `CloudPhoneAuth.Token` 与 `CloudPhoneTokenValidate.Token` **就是活动站凭据本体**
  //     （夹具里那两格本来就是 `<REDACTED>`），能整体 marshal = 离泄漏只差一次误打日志，
  //     与 `RefreshedCredential` 同一条纪律；
  //  ② 这一族**至今零接线、零真打**（12 个端点只有 `test/fixtures/http/红包派对.json`
  //     一份归档作证据），字段形状里"取值集没穷举"的比例远高于其它族 ⇒
  //     现在补 tag 等于把一个没被任何调用方用过一次的形状写成对外契约。
  //
  // ⚠ 请求侧那几个（`CloudPhoneAuth` / `CloudPhoneBilogFields`）**从不**被 marshal：
  // body 一律走 [CloudPhoneRequest] / [CloudPhoneRawRequest] 现拼 map，
  // 键集合随"这一条实拍有没有给那个键"而变 —— 给结构体补 tag 会造出一条
  // "看起来能发、实际多发了两格空串"的旁路，而多一格键的请求**夹具里不存在**。
  'internal/upstream/app_cloud_phone.go:CloudPhoneAuth':
    '一次请求的**签名入参集**（requestId / timestamp / token / 两个设备位），只喂给签名函数与头部构造。⚠ `Token` 是活动站凭据本体（夹具里那格是 `<REDACTED>`），补 tag = 开一条"顺手 marshal 整个 auth 进日志"的路；`Timestamp` 刻意是 string 而不是 int64，因为**单位（秒/毫秒）未核实**，收成整数就是替调用方赌一个单位',
  'internal/upstream/app_cloud_phone.go:CloudPhoneHeader':
    '信封 `header` 的解析结果，五个字段全是指针 —— 这一族的判据是"上游给没给这一位"（`status` 缺失与给了 `"200"` 是两条不同的结论）。⚠ `RespMsg` **零样本**（夹具里连键都没出现过），留着只为"哪天给了别丢"，更不该写成对外字段',
  'internal/upstream/app_cloud_phone.go:CloudPhoneTokenValidate':
    '换活动站 token 那一条的解析结果。⚠ `Token` 同上，是凭据本体；`ExpireTime` 在**同一份夹具的另一条**上是 13 位毫秒时间戳、在这一条上是 `3600` —— 同名两义且哪一义都没成文说过，所以它**不许**当 TTL 用（`AGENTS.md` §1.12），补 tag 会诱使下游拿它算到期',
  'internal/upstream/app_cloud_phone.go:CloudPhoneAccountInfo':
    '红包账户余额（**产出是钱**）。`CanAmount` 用 `*int64`：0 是"确实没有可兑余额"这一条结论，nil 是"上游没给、我们不知道" —— marshal 出去两者都长得像没字段或 0，而这一族上把"不知道"显示成"没有"是**会让用户以为余额被清零**的误判。单位是分，`/100` 归展示层',
  'internal/upstream/app_cloud_phone.go:CloudPhoneToastInfo':
    '`isShowCloud` 一个格子，语义**未验证**（一条样本都没有对照组，名字与成文那句"播报金额"也对不上）。它唯一合法的用途是"读出来、不解释"，进 JSON 一定会被下游当成某个开关',
  'internal/upstream/app_cloud_phone_install.go:CloudPhoneInstance':
    '云手机实例列表里的一条。声明出来的只有 `InstanceId`/`LockStatus` 两格（键名出处是**旧 TS** `task.ts:274-283`，不是实拍 —— 该端点响应体在夹具里根本没有条目），其余全在 `Raw map[string]any` 里。marshal 它会得到一份"两格是推的、`Raw` 里是什么没人负责"的对外形状',
  'internal/upstream/app_cloud_phone_install.go:CloudPhoneApp':
    '`configAppList` 的一条应用。四个键（`packageName`/`id`/`packageId`/`apkName`）的出处同样是旧 TS，**零实拍**；⚠ `ID` 与 `PackageID` 是**两个不同的值**（装 APK 用后者、记账用前者），类型连旧实现自己都是 `as number` 硬转 ⇒ 收成 `any` 原样交出去。这种"两格容易用反、类型未知"的形状不该对外承诺',
  'internal/upstream/app_cloud_phone_install.go:CloudPhoneInstallSubmit':
    '`apkInstall` 的回执 `{Success, TaskID}`。⚠ `Success` 是**三态**：`nil` = 上游没给这一格、`false` = 给了且是假 —— 旧实现把两者合并成"跳过这个应用"，我们刻意分开（"读不到"与"读到否"是两句话）。过一遍 JSON 之后 `nil` 与 `false` 在前端那里又会合回去，而这一位决定要不要往云机发第二枪',
  'internal/upstream/app_cloud_phone_install.go:CloudPhoneInstanceList':
    '实例列表 + 一个 `Nested` 布尔（本次是从 `data.data` 那层取到的）。`Nested` 是**接线期诊断位**："取错层"与"列表真的为空"在只交 `Instances` 的版本里长得一模一样；它描述的是我们的读法，不是上游数据，不该出网',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneTaskItem':
    '任务表里的一条（十个可选位 + 四个额度读数）。⚠ `prizeType` 实拍**全是** `1`、`configTaskBuyList` 实拍**恒是空数组**、`userCanCompleteNum = periodCanNum − userCompleteNum` 只在 9 个条目上成立（是**观察**不是承诺）—— 这一族的取值集远未穷举，形状现在只够内部分发（按 `taskCode`），不够对外承诺',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneSignDay':
    '签到日历里的一格（实拍 44 格）。六个字段全是裸指针/整数而**不带档位语义**：这一格的 `status` 与任务条目那个 `userStatus` **是两本字典**（实拍 `0` 出现在今天与过去全部日子、`2` 出现在响应日期之后的七天），刻意不复用 [CloudPhoneUserStatus] 去"顺手对齐"；`1`/`3`/`4` 在本样本里一件都没有 ⇒ 补 tag 会让前端把它当成一份已穷举的枚举显示出去',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneTaskList':
    '任务表整份解析结果（四张表 + 日历 + 三个顶层读数）。`Buy` 的**元素形状零样本**（按"同一张表的结构"解，只有键名 `configTaskBuyList` 有实拍）；`SecondConfirmStatus` **只有** `configTaskLoginList` 那一条口给（另一条口连这个键都没有），补 tag 会让前端以为两条口都有这一位、缺失即 0。⚠ 另有四个键（`aiAppLink`/`aiH5Link`/`iconLink`/`iconUrl`）刻意**不声明**：前两个是带 token 的第三方跳转地址，不该成文更不该出网',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneCompleteTask':
    '登记一次任务完成的解析结果（只有 `status`）。⚠ 本层**有意偏离**旧实现：旧实现把"缺这一位"当成功（`isNil` 那半句），这里保持 nil —— marshal 出去"缺"会变成 `null`，前端读成 0 就是失败、读成"无字段"又是成功，两种都把我们刻意保留的那个区别抹掉了',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneRuleText':
    '规则文案（`code`/`title`/`ruleText`）。⚠ `ruleText` 实拍是**HTML 片段**（带 `<div class="rule_name">` 这类标签），本层原样交出不清洗 —— 进 JSON 会变成"前端直接 innerHTML 也行"的暗示，而转义责任在展示层',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneAnnouncement':
    '公告：内嵌 [CloudPhoneRuleText] 再加四个读数。⚠ 内嵌是**为了少写四行赋值**，上游那一层是**平铺**的；一旦 marshal，Go 的嵌入语义会把字段摊平 —— 于是"我们的组合方式"看起来像上游的字段约定（正是 §1.4 要防的那类混淆）',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneRuleConfigList':
    '公告列表。`data.list` 不是数组（含缺失）⇒ `Items` 为 **nil**，给了空数组 ⇒ **空切片**：这个区别只在内存里有意义（调用方 `len()` 两者都是 0，但"形状变了"要能看出来），一旦 marshal，nil 与空都成 `null`/`[]` 并被前端当成同一个结论',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneBilogFields':
    '一条埋点的三个**可选**位（`action` 必填，两个 extra 可缺）。请求 body 由 `CloudPhoneBilogUploadRequest` 现拼 map，"缺就不发这一格"是实拍行为；给结构体补 tag 等于造一条会多发 `"extra1":""` 的旁路请求，而**带空串的这条夹具里不存在**。⚠ `Extra2` 实拍那一枚是账号标识，成文与测试一律用掩码形态',
  'internal/upstream/app_cloud_phone_task.go:CloudPhoneBilogRet':
    '`bilog/upload` 的解析结果（只有 `ret`，实拍两条都是 `1`）。它**是不是**成功判据未验证（没有非 1 的对照样本，成文也没提这一位）⇒ 一条埋点的读数更不该成为对外字段',
  // ── 红包派对族的**app 编排层**（2026-09-24 接上 `cloud-phone-redpack` 只读那一半）──
  // 与 `SignInWorld` / `CloudPhoneAssets` 上游层同一族规矩；这一处多一条只对它成立的：
  // 这一族**没有票失效判据**（零样本），对外只有 `SummaryLine()` 那一句人话，
  // 把中间量摊成字段等于把"这一格上游没给"与"这一格是 0"混给前端显示。
  'internal/app/cloud_phone_run.go:CloudPhoneWorld':
    '跑红包派对那一趟的**入参集**（库 / scope / 上游 / 设备档案 / 时钟 / 随机 / 活动站票池 / 进度回调），字段里全是句柄；与 `SignInWorld` 同规，Rust 侧这里是一组独立参数、不 Serialize',
  'internal/app/cloud_phone_run.go:CloudPhoneAssets':
    '一次只读盘点的**三格内存读数**（余额 / 清单 / 播报）。前两格是上游解析结果（形状由上游决定，同 `PrizesResult` 那条理由），第三格**刻意为指针**："这一枪没打或打挂了"与"打了、播报位是空的"在内存里分得开，marshal 出去两者都是 `null`；`Toast` 里还含 `isShowCloud` 那个语义未验证的读数',
  'internal/app/cloud_phone_run.go:CloudPhoneResult':
    '一个账号这一趟的**内存结论**（账号 + 档位 + 读数 + 错误原文），出网只经 `core.RunReport`（小结一句 + 待办）。⚠ `Status` 零值是 `CloudPhoneDone`，序列化时前端拿不到这一格就会按"成功"理解；`Error` 装的是上游文案，整块 marshal 等于把它当结构化字段承诺出去（同 `NoticeResult` 那条理由）',
  // ── 拍拍族（#16，2026-09-24 登记，`internal/upstream/app_play_ai.go` 落地带来的）──
  // 同一族规矩，两个只对它成立的加重理由：
  //  ① 这一族的**证据只有一份**（`test/fixtures/http/夏日拍拍大作战.json` 13 条），
  //     而它和 `docs/business-feature-inventory.md` §2.4 写的是**两套机制**
  //     （文档说有 `querySpecToken`/`tyrzLogin` 换会话，夹具里一条都没有）⇒ 形状未定；
  //  ② `PlayAIRecognizeParams.DataURL` 是**整张图的 base64**（几百 KB 起），
  //     与 `SlidePuzzle` 那两条 PNG 是同一条纪律。
  'internal/upstream/app_play_ai.go:PlayAIUserStatus':
    '`newyear/user/info` 的解析结果（只有 `totalLotteryCount`，指针）。⚠ 0 是"抽过了"、nil 是"上游没给这一格"，而**抽奖次数就是这一页唯一的动作判据** —— marshal 出去前端把两者都读成"没有次数"，正好是最坏的那种误判。`result` 整体缺失是**报错**而不是 nil（实拍两条都有），所以这一层已经没有"缺"这个形态可承诺了',
  'internal/upstream/app_play_ai.go:PlayAIUserTask':
    '`newyear/task/list` 的一条任务。键名是**混着**的：`groupId` 驼峰而 `recordid`/`currstep` 全小写（签到任务表那族是全小写 `groupid`），且 `recordid` 实拍是 `null` 或**字符串**、从没数字形态。测试是把它摊成"键 → 有没有给"来逐字段核对的（见 `playAITestTaskProbe`），补 tag 不会让这些区别消失，只会把它们写进契约',
  'internal/upstream/app_play_ai.go:PlayAIPrize':
    '一次中奖的奖品（14 个可选位，**全是指针** —— 一次真成功的抽奖不该因为缺某一格被报成解析错误）。⚠ `prizeType`/`type`/`firstMonthGet`/`high` 的取值表**没有成文**、只有一条样本；`buttonLink` 的查询串里**带一次性 token**；`oid` 上游两个形态（数字 / 字符串）都出现过。这三件事任何一件进 JSON 都会变成对外承诺',
  'internal/upstream/app_play_ai.go:PlayAILotteryResult':
    '`blindbox/lottery` 的解析结果（`Prize` 一格）。`Prize` 为 nil **不等于**"没中奖"（实拍只有一档样本，"没中奖长什么样"仍无答案），这个区别进 JSON 一定会被读成"奖品为空"',
  'internal/upstream/app_play_ai.go:PlayAIRecognizeParams':
    '图片识别的**请求侧入参集**（三个都必填）：`UserID` 是 19 位账号标识（本仓不存真值，测试用 `199…` 合成号）、`DataURL` 是**整条 data URL**（实拍这个键装的就带 `data:image/jpg;base64,` 前缀，原样收、不拼不剥）、`ImageExt` 与它的前缀实拍一致但"必须一致"**未验证** ⇒ 不自动推也不校验。body 由 `PlayAIRecognizeBody` 现拼八键 map，从不整体 marshal 这个结构体 —— ⚠ 补 tag 的代价与 `SlidePuzzle` 同一条：一次误打日志就是几百 KB 图像数据，里面还带着账号标识',
  'internal/upstream/app_play_ai.go:PlayAIRecognizeResult':
    '识别结果的解析。`hasText` 实拍 `false`（指针：折成"没给"就等于丢掉一个真实取值）；`content`/`hasQuestion`/`language` 三格**只有 null 样本** ⇒ 形态未验证；`fileId` 指向这次写进云盘的那个文件，是清理那一族（`hcy/file/get`，接口层冻结中）的入参来源 —— 它出网要连"用谁的凭据删"一起表达，那是响应体的事不是这个结构体的事',
  // ── 拍拍族的**换票与埋点层**（`app_play_ai_session.go`，2026-09-24 登记）──
  // 与上面那一条族级前言同一口径，两条各自成立的理由：
  //  ① 这一层的形状出处**只有旧 TS**（`apis/auth.ts:45-97`、`tasks/play-ai-special/*`），
  //     夹具 13 条里连一次都没出现过换票、`journaling`、`chat/v2/add` ⇒ 无实拍；
  //  ② 它的产物是**凭据**（`jwttoken`）与"以谁的身份问一句"（`userId`），
  //     两者一旦带上 tag，`encoding/json` 就会绕过 `core.SecretValue.String()`
  //     那层只挡打印的守卫（见 `docs/sanitize-guide.md`）。
  'internal/upstream/app_play_ai_session.go:PlayAISession':
    '换票成功后的会话凭据。`JWTToken` 是**凭据本体**：整结构体不给 tag，因为 `SecretValue` 那种"只挡打印"的守卫对 `encoding/json` 无效，一旦出 tag 就会随任意一次结果序列化泄漏。`Mobile` 是回显手机号（只用于"确认签给谁"，展示层要自己掩码）。⚠ 它同时带 `UserDomainID` 与 `JWTToken`，正是最不该被顺手 marshal 的组合',
  'internal/upstream/app_play_ai_session.go:PlayAIChatParams':
    'SSE 对话那一枪的入参（**请求侧**，本层不解析 SSE 响应：零样本）。`UserID` 是账号标识不是凭据，填错等于以别人身份提问；`FileName` 刻意不由 `taskId` 推导（旧 TS 那一格过 JS double 会丢 19 位 id 的末几位，见字段注释）⇒ 收成调用方给的字符串。这三格没有一行是"对外契约"，出网只会让人误以为本页 chat 的 body 已经对拍过',
  // ── 拍拍族的**app 编排层**（2026-09-24 接上 `play-ai-special` 只读那一半）──
  // 与上面 `cloud_phone_run.go` 那一族同规：出网只经 `core.RunReport`（小结一句 + 待办），
  // 中间量摊成字段就等于把"上游没给这一格"与"这一格是 0"混给前端显示。
  // 只对它成立的一条加重理由：这一族**只有只读那一半**，而 `Board` 里两格都是
  // **未对拍形状的上游解析结果**（归档 13 条、零实拍链路）⇒ 一旦带 tag 就成了对外承诺。
  // ── 585 AI 相机那一发的 **SSE 读数**（`app_play_ai_chat_sse.go`）──
  // ⚠ 本条登记在 2026-09-26 补进：那条结构体随 585 那一轮（commit 29a1e08）进了仓，
  // 而它的豁免**没跟着进那一笔提交** ⇒ HEAD 那棵树单独看是红在 jsontags 上的。
  // 这一族的 SSE 形状**已有两条真机实拍**（`docs/capture/app-play-ai-samples.txt` §8，
  // 判据收口见 `docs/pending-decisions.md` R37③）—— 不写"零实拍"。
  'internal/upstream/app_play_ai_chat_sse.go:PlayAIChatSSERead':
    '一次 SSE 响应的**内存读数**（命中哪一档 / 那一行原文 / 判据 / `code` 与文案 / data 行条数）。`Value any` 装的是上游原文解出来的通用值，逐行形状随档变 ⇒ 带 tag 就等于把"上游的 SSE 事件长什么样"钉成对外契约，而本仓对它只做过**读数**、没做过 schema。对外出网的是 app 层那句一行播报，不是这个形状；要展示按仓库规矩在 server 层逐字段列白',
  'internal/app/play_ai_run.go:PlayAIBoard':
    '只读两枪的**内存读数**（① 状态 + ② 任务表）。①那一格唯一的值 `totalLotteryCount` 语义未验证（是累计获得还是还剩），②那一格 `state` 只支撑两个实拍值 ⇒ marshal 出去等于承诺一份我们没能对拍的上游形状。真实动作判据（"该不该抽"）本来也不在这一层',
  'internal/app/play_ai_run.go:PlayAITaskCounts':
    '从任务表**算出来**的四个计数（内存中间量，仅供 `SummaryLine` 与测试读）。⚠ `Unknown` 那一档的存在本身就是判据的一部分："上游给了这一族没见过的值"不能折进"未完成"；一旦出 tag，前端只会拿 `Total-Done` 当剩余，那一档就静默消失了',
  'internal/app/play_ai_run.go:PlayAIResult':
    '一个账号这一趟的**内存结论**（账号 + 档位 + 读数 + 错误原文），对外只有 `RunReport()` 那一句人话。⚠ `Status` 零值是 `PlayAIReadDone`，序列化时前端拿不到这一格就按"成功"理解；`Error` 装上游文案（同 `NoticeResult` 那条理由）；`Board` 里还嵌着上一条那个不该承诺的上游形状',
  // ── AI 头像族（#10，2026-09-24 登记，生成链 + 抽奖链接上执行带来的）──
  // 与 `play_ai_run.go` / `exchange_run.go` 同一族规矩（装配体不是数据、结论只经
  // `RunReport` 出网），两条只对它成立的加重理由：
  //  ① 这一族**会往用户网盘写文件**，而 `TempFileIDs` 是"该删哪几个"的唯一依据 ——
  //     它出网时必须连"用谁的凭据删"一起表达，而今天**没有**清理调用点
  //     （`recyclebin/batchTrash` 零实拍，P1），所以这份 id 清单是内部账不是契约；
  //  ② `rabactive` 那六口**零实拍**（`docs/capture/` 里 `grep rabactive` 一条没有），
  //     `Prizes` / `Note` 里的文案是上游原文，钉成字段等于承诺一份没对拍过的形状。
  'internal/app/signin_world.go:SignInOpening':
    '跑一条链要**同时**拿在手里的三样（App 档案 / 登录态凭据 / 会话来源），字段全是句柄与凭据：`Credential` 里的 `Authorization` 是 `core.SecretValue`，而它的 `String()` **只挡打印不挡 `encoding/json`**（见 `internal/core/model.go`）⇒ 一次整体 marshal 就把整串凭据递出去。对外形状一律在 server 层逐字段列白（同 `LoginSessionView` / `runPairings`）',
  'internal/app/ai_avatar_run.go:AiAvatarRun':
    '一条能力的**装配体**（内嵌 `SignInWorld` 再加一个 `Wait` 注入点），与 `ExchangeRun` / `ClaimRun` 同规：装着库句柄与出站 HTTP，压根不是数据',
  'internal/app/ai_avatar_run.go:AiAvatarResult':
    '一个账号这一趟的**内存结论**（张数 / 抽奖档位与次数 / 奖品名 / 待清理文件 id / 备注），出网只经 `core.RunReport` 的 `SummaryLine()` 那一句人话（那句话刻意**只报个数**、不报 id 串）。⚠ `Status` 零值是 `AiAvatarDone`（"有产出"），前端拿不到这一格就会把"走完了但零产出"显示成成功；`DrawPhase` 的四档与 `AiAvatarFailed` 是同一件事的两个视角，摊成字段会被读成两个独立状态',
  // ── CLI 的参数结构体 ──
  // 它们全是 clap 的 `#[derive(Args)]`（解析命令行用），**没有** serde 的 Serialize，
  // 也从不被序列化 —— 进 JSON 的只有 `--json` 那一支的输出（在 output.go 里单独构造）。
  'internal/cli/args.go:SigninArgs':
    '签到那一大套参数（`--list` / `--status-only` / `--db` …）；Rust 侧是 `#[derive(Args)]` + `#[command(flatten)]`',
  'internal/cli/args_login.go:QrArgs': '`login qr` 的参数；Rust 侧是 `#[derive(Args)]` 的 `QrArgs`',
  'internal/cli/args_login.go:SmsArgs':
    '`login sms` 的参数；Rust 侧是 `#[derive(Args)]` 的 `SmsArgs`',
  'internal/cli/args_other.go:MigrateArgs':
    '`migrate` 的参数（`--dry-run` 等）；Rust 侧是 `#[derive(Args)]` 的 `MigrateArgs`',
  'internal/cli/args_live.go:LiveRoomArgs':
    '`live-room` 的参数（`--kouling-only` / `--listen-seconds` …）；与上面几条同理 —— ' +
    '它只被 pflag 绑进 flagset，从不序列化。⚠ `--json` 那一支的输出是 ' +
    '`run_live_room.go` 里**单独构造**的 map（键名 snake_case 且刻意不用这个结构体），' +
    '理由与 `QrProgress` 那条同源：把 CLI 参数结构体当响应体用，' +
    '会让"加一个内部开关"顺手改掉外部契约',
  'internal/cli/cli.go:Deps':
    '一次 CLI 运行的**外部世界**（三个流 + 时钟 + 随机源），不是数据模型 —— Rust 侧这里直接读 `std::io::stdout()`',
  'internal/app/smid.go:FingerprintBinding':
    '一次绑定的**结果**（特征 + 库里两列 + 是否新绑），内存里用；Rust 侧同样只 derive `Debug, Clone, PartialEq, Eq`',
  'internal/app/user.go:InitOutcome':
    '一次初始化的**结果**（建出来的用户 + 认领到多少行），喂给 CLI 打印；Rust 侧是手写 Debug 的 struct，**不** Serialize',
  'internal/app/migration.go:MigrationPlan':
    '一次迁移的**入参**（从哪读 / 写到哪 / 归档到哪），由 CLI 或启动自检填；Rust 侧只 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  'internal/app/migration.go:MigrationOutcome':
    '迁移的**结论**，机器可读形态由 `OutcomeToJSON` 另行拼出（键名是 camelCase 的外部契约），结构体本身不进 JSON；Rust 侧 `report` 虽是 `Value`，但它是 `MigrationReport::to_json()` 的产物，同样不是这个结构体的序列化',
  'internal/app/run.go:RunPlan':
    '一次跨账号运行的**入参集**（能力 / 账号列表 / 参数摘要），不是数据模型；Rust 侧是 `run_many` 的四个参数',
  'internal/app/run.go:AccountRun':
    '单个账号那趟的**结果**（账号 + 报告 + 被谁挡下 + 是否跳过），内存里汇总用；Rust 侧只 derive `Debug, Clone, PartialEq, Eq`',
  'internal/app/run.go:RunSummary':
    '整批运行的**汇总**（每账号一条 + 计数），由它渲染摘要行与 JSON；Rust 侧同样只 derive `Debug, Clone, PartialEq, Eq`',
  // ── 直播口令 / 小红花这一条链（2026-09-22 登记，同日补 `live_room_task.go` 三个、
  //    同日再补 `live_room_probe.go` 一个 ⇒ ④ 那类现在是九个）──
  // 23 个结构体，分四类，理由各不相同（别合并成一句"都是内存里的"）：
  //
  //  ① **MSync 报文体**（`live_msync.go` 五个）：它们的外部契约是 **wire 字节**
  //     （由 `EncodeMSync` / `EncodeProvision` 产出，逐字节与 protobufjs 对拍过）。
  //     补 `json` tag 会造出一套**没人用、也没人核**的第二序列化，而它一旦被误用
  //     （某处顺手 `json.Marshal` 打日志），字段名与 protobuf 字段号对不上 ——
  //     "看起来结构对了、实际全错"，正是最难查的一类。
  //  ② **上游解析结果**（`live_flower.go` / `live_room.go` 八个）：里面**含凭据**
  //     （`LiveUserToken.AccessToken` 是 WS 凭据、`ApplyRoomResult.AppKey` 是进房凭据），
  //     且 `RedFlowerDrawOutcome.Message` 存的是**上游原文**。能自动 marshal 就离
  //     "顺手把整个结构写进日志/响应"只差一步 —— 与 `RefreshedCredential` 同一条纪律。
  //  ③ **状态机返回值**（`FrameActions`）：一帧处理完的动作集合，只在内存里被驱动层消费。
  //  ④ **编排层结论**（`app` 侧八个）：出网走 CLI 渲染层 / WebUI 的**另拼响应体**
  //     （理由与 `LoginSessionView` 逐字相同：这个结构体会加字段，而"加了就自动出网"
  //     正是凭据泄漏最常见的路径）。其中 `LiveRoomGate.JWT` **就是凭据本体**、
  //     `LiveRoomDriver` 含 `core.Random` / `core.Clock`（根本不可序列化）。
  'internal/upstream/live_msync.go:JID':
    '① MSync 的 `JID`（appKey / name / domain / clientResource）；外部契约是 wire 字节，不是 JSON',
  'internal/upstream/live_msync.go:MSyncMessage':
    '① MSync 报文**本体**（`MSYNC_V1`），逐字节对拍 protobufjs 产出；字段号才是契约',
  'internal/upstream/live_msync.go:Provision':
    '① PROVISION 的 payload（osType / appSign / compressType…）；同上，wire 字节才是契约',
  'internal/upstream/live_msync.go:MessageContent':
    '① `MessageBody.Content` 的一条（type + text），入站解析的中间结果',
  'internal/upstream/live_msync.go:MessageBody':
    '① 弹幕文本所在的那一层（`contents[0].text`），入站三级兜底的最终落点',
  'internal/upstream/live_flower.go:RedFlowerBalance':
    '② `user/info` 的解析结果（余额的唯一判据）；对外只出**数字**，不出这个结构',
  'internal/upstream/live_flower.go:FirstParticipateOutcome':
    '② `firstParticipate/give` 的结论；`Message` 存**上游原文**，整块 marshal 等于把上游回显自动带出网',
  'internal/upstream/live_flower.go:RedFlowerDrawResult':
    '② 一次 `draw` 成功时的 `result`（prizeId / flowerNum / prizeName）；内存里换算成报告字段',
  'internal/upstream/live_flower.go:RedFlowerDrawOutcome':
    '② 一次 `draw` 的判定（含原始码与上游原文 `Message`）；同上，不整块出网',
  'internal/upstream/live_room.go:LiveUserToken':
    '② `userToken` 的解析结果 —— `AccessToken` **就是 WS 凭据**，是这条链上最不能自动 marshal 的一个',
  'internal/upstream/live_room.go:LiveRoom':
    '② `tabContentList` 里的一间房（只留判据用得到的字段）；出网的是**门的结论**，不是房间列表',
  'internal/upstream/live_room.go:RoomBeat':
    '② `roomBeat` 的解析结果；`ChatRoomID` 用指针表达"没有聊天室"（坑表 #107 那条结构性事实）',
  'internal/upstream/live_room.go:ApplyRoomResult':
    '② `member/apply` 的解析结果 —— `AppKey` 是进房凭据；同上，不整块出网',
  'internal/upstream/live_ws.go:FrameActions':
    '③ 一帧处理完之后的**动作集合**（要回发的帧 + 弹幕 + 新口令 + 归类），状态机返回值',
  'internal/app/live_room.go:KoulingGate':
    '④ 门的判定（是否开 / 房间 / LiveCount 的两种"没有"）；出网走渲染层另拼',
  'internal/app/live_room.go:RedFlowerDrawRecord':
    '④ 一个口令的抽取记录（口令 + 判定）；同上，出网由渲染层逐字段拼',
  'internal/app/live_room.go:RedFlowerReport':
    '④ 六步链的**结论**（前后余额 / 首次参与 / 逐口令结果 / 无效与未分类口令）；⚠ 它将来会加字段（比如"这次用的凭据"），而"加了就自动出网"正是泄漏最常见的路径',
  'internal/app/live_room_drive.go:LiveRoomGate':
    '④ 门的结论 + **直播侧 JWT 本体**（`JWT` 字段）—— 这条链上最不该能被 marshal 的一个',
  'internal/app/live_room_drive.go:LiveRoomDriver':
    '④ 注入依赖集（上游 / 拨号器 / 随机源 / 时钟 / 手机号）；含接口与时钟，根本不可序列化',
  'internal/app/live_room_task.go:LiveRoomParams':
    '④ 一趟运行里**由使用者决定**的那几件事（`Codes` / `ListenSeconds` / 两个开关）；' +
    '外部契约是**能力声明**（`capabilities.liveRoomParams` 的那几个 key）与 CLI 的 flag 名，' +
    '两者都由 `internal/server/runs.go` 按声明逐键取值、不由这个结构体自动 marshal —— ' +
    '补 tag 等于给"字段名 = 契约"造出第二个定义处，而漂移的表现是界面上填了、后端没收成',
  'internal/app/live_room_task.go:LiveRoomRun':
    '④ 跑一个账号需要的**世界**（库 / scope / 驱动 / 设备 / 参数 / 进度回调）；' +
    '含 `core.StateStore`、`upstream.AppDevice` 与函数字段 `Progress`，根本不可序列化',
  'internal/app/live_room_task.go:LiveRoomAccountResult':
    '④ 一个账号这一趟的**结论**（门的判定 / 口令 / 六步链报告 / 是否重听过）；' +
    '出网由 CLI 的 `liveRoomJSON` 与 `runs.message`（`RunReport()` 那句话）**分别另拼**。' +
    '⚠ 它带 `Gate`（房间列表）与 `Codes`，而 `HasJWT` 刻意只存"有没有"——' +
    '整个 marshal 出去会把"凭据缺席"这件事变成一次随手可得的结构导出',
  'internal/app/live_room_probe.go:LiveRoomProbe':
    '④ 只读探针的**结论**（门的判定 + 那个主播的房间日程 + 余额），出网由 ' +
    '`internal/server/live_room_probe.go` 的 `liveRoomProbePayload` 逐字段另拼。' +
    '给它补 tag 尤其危险：它内嵌 `KoulingGate`（含房间列表），而它读的那条链上' +
    '到处是凭据 —— "加了字段就自动出网"正是这里要挡的那一类',
  'internal/server/auth.go:Config':
    '服务端的**启动参数**（cookie 是否带 Secure / 会话 TTL …），由 main 组装时填，不进 JSON',
  'internal/server/auth.go:AuthUser':
    '已认证身份（User + Session），存在 `context` 里；出网的形状是 `userPayload` 逐字段列白的 map，不是这个结构体',
  'internal/app/signin.go:SignInReport':
    '一次签到的**结论**（调用前 / 调用后 / 本次快照），喂给 CLI 与汇报层；Rust 侧只 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  'internal/app/signin.go:SignInOptions':
    '签到的两个开关（`force` / `deviceId`），**函数入参**不是数据模型；Rust 侧是 `Option<&str>` 两个参数',
  'internal/app/signin_receive.go:RecordsQuery':
    '流水查询的**入参集**（分页 + 区间 + 类型），Rust 侧是四个独立参数不是结构体',
  'internal/app/signin_receive.go:ReceiveTarget':
    '一条待领账目的**中间形态**（协议层解析结果 + 计划动作），内存里用',
  'internal/app/signin_receive.go:ReceiveAction':
    '一条账目的**计划动作**（走哪条接口 + 带什么参数），内存里用；Rust 侧是 `enum`',
  'internal/app/signin_receive.go:ReceiveOutcome':
    '一条账目的**领取结果**（结局 + 数量 / 失败原因），喂给 CLI；Rust 侧是 `enum`',
  'internal/app/signin_bonus.go:MultipleOutcome':
    '「签到翻倍」的**结果**（结局 + 倍数与合计 / 失败原因），喂给 CLI；Rust 侧是 `enum`',
  'internal/app/signin_bonus.go:BackupGiftOutcome':
    '「备份好礼」的**结果**（结局 + 数量 / 失败原因），喂给 CLI；Rust 侧是 `enum`',
  // ── 滑块题面解算（`internal/core/slide.go`）──
  // 这四个都是**一次解算的中间物 / 报告**：像素平面、包围盒、候选、解算报告。
  // 它们**不进 JSON** —— 对外只有一个整数 `puzzleOffset` 进 `exchangeV3` 的请求体，
  // 而那个 body 是调用方**按实拍键序手工拼**的字符串（键序也是契约的一部分，
  // 走结构体 marshal 会按字母序重排）。解算报告只进 CLI 打印与日志。
  // ⚠ 真要把它出网时，正确做法是**另写一个带 tag 的响应体**，
  //   而不是给这四个补 tag —— 补了就等于把**内部量度的约定**写进对外契约
  //   （`Metric` 一律"越小越好"、`Gray8` 在调色板图下其实是**索引**）。
  'internal/core/slide.go:SlideImage':
    '一张已解码的题面图的**像素平面**（浮点灰度 / 整数灰度或调色板索引 / alpha），纯计算中间物',
  'internal/core/slide.go:SlideBox': '拼图块在拼图图里的**包围盒**（四个整数），纯计算中间物',
  'internal/core/slide.go:SlideCandidate':
    '一个**候选落点与它的度量**（`Metric` 一律"越小越好"），纯计算中间物',
  'internal/core/slide.go:SlideSolution':
    '一次解算的**报告**（两个口径各自的最优 + 自检量 + 共识值），喂给 CLI 打印与日志',
  'internal/io/client.go:RetryPolicy':
    'HTTP 重试策略（次数 / 退避基数 / 上限），**构造参数**不是数据模型；Rust 侧 derive `Debug, Clone, Copy`，**不** Serialize',
  'internal/upstream/app_login.go:AppSendSmsOutcome':
    '一次发码的**判定结果**（分类 + 配额余量 / captchaType / code+msg），进程内存里用；Rust 侧是 `enum`，**不** Serialize',
  'internal/upstream/app_login.go:AppLoginOutcome':
    '一次 App 登录的**判定结果**（分类 + `data` 密文原文 / code+msg），进程内存里用；Rust 侧是 `enum`，**不** Serialize',
  // 领取族与流水族的**解析结果**：由 `ParseXxx(envelope)` 手工从泛型 payload 里取值
  // 构造（`RequireInt64(payload, "receive")` / `payload["records"]`），
  // **不是** `json.Unmarshal` 的目标 —— 字段名因此不参与任何解析，
  // 加 tag 只会让人以为"字段名是这里的解析键"。
  //
  // ⚠ 理由的边界：若哪天要把它们**序列化**给前端（比如 CLI `--json` 直接吐），
  // 正确做法是补上 tag 并从这里删掉 —— 而不是继续待在名单里。
  // 那时字段名就成了对外契约，要由 `test/golden/enum-json-vectors.json` 那类
  // serde 产出的向量来定，不能凭手感写。
  'internal/upstream/app_signin_receive.go:ReceiveResult':
    'receiveV3 的解析结果（`RequireInt64(payload, "receive")`），手工构造，不做反序列化',
  'internal/upstream/app_signin_receive.go:RevivalReward':
    'receiveRevivalReward 的解析结果（用掉一张卡换回的云朵），同上',
  'internal/upstream/app_signin_receive.go:ReceiveTaskExpansion':
    'receiveTaskExpansion 的解析结果（月度任务奖励），同上',
  'internal/upstream/app_signin_receive.go:MultipleResult':
    'multiple 的解析结果（本次额外领到的豆），同上',
  'internal/upstream/app_signin_task_expansion.go:TaskExpansion':
    'taskExpansion **读口**的解析结果（六格全指针）。出网的是 app 层那句小结，不是这份形状；⚠ 钉 tag 会把"上游没给这一格"和"这一格是 `false` / `0`"混成同一个读数，而编排层正是拿这个区别决定**要不要往用户网盘写一个文件**去凑备份（不可回滚）。实拍只有一条、且**不含** `acceptDate` ⇒ 更没有可承诺的形状',
  'internal/upstream/app_signin_records.go:CloudRecords':
    'cloudRecordV2 的解析结果（一页流水 + 分页总数），手工构造，不做反序列化',
  'internal/upstream/app_signin_records.go:PrizeRecordItem': '奖品流水里的一条，同上',
  'internal/upstream/app_signin_records.go:PrizeRecords':
    '奖品流水的整份解析结果（条目 + 分页），同上',
  'internal/upstream/app_signin_records.go:BackupGift': '备用礼包的状态解析结果，同上',
  'internal/upstream/app_signin_records.go:BackupGiftReceive': '领取备用礼包的结果解析结果，同上',
  'internal/core/auth.go:AuthInfo':
    'Rust 侧只 derive `Clone, PartialEq`——**刻意不** Serialize（里面是 token）',
  'internal/core/crypto.go:SignSaltPair':
    '签名所需的 (sign, salt) 对，由 crypto 的返回值组装，不整块落库',
  'internal/core/stage.go:StageError':
    'Rust 侧 derive `Debug, Clone, PartialEq, Eq, thiserror::Error`，**不** Serialize；对外文案走 `Error()`，不整块 marshal',
  'internal/core/legacy.go:LegacyError':
    'Rust 侧 derive `Debug, Clone, PartialEq, Eq, thiserror::Error`，**不** Serialize；对外文案走 `Error()`',
  'internal/core/legacy.go:ParseBad': '解析失败的**报告条目**（下标 + 信息），进迁移报告而非 JSON',
  'internal/core/legacy.go:LegacyAccountView':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq` —— 迁移期的**中间派生结果**，不落库也不出响应',
  'internal/core/legacy.go:LegacyTarget':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq` —— 描述"旧版会怎么做"的中间结果',
  'internal/core/legacy.go:ReferenceAdvice':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq` —— 迁移诊断，交给报告而不是 JSON',
  'internal/capabilities/registry.go:Capability':
    'Rust 侧**没有** derive `Serialize`：对外形状由 `mcloud-app::payload` 手工拼装（camelCase，与 serde 的 snake_case 默认不同）。刻意不加 tag —— 加了会让人以为它序列化出来就是给前端的那个形状',
  'internal/exec/exec.go:Concurrency': '并发闸**配置**（全局上限），进程内存里用，不进 JSON',
  'internal/exec/exec.go:Admission':
    '一次 run 准入的**判定结果**（Kind + 重复的 runId / 被挡的键与占用者），进程内存里用，不进 JSON；Rust 侧是 enum，无 Serialize',
  'internal/exec/exec.go:Delivery': '事件总线的一次**投递记录**，进程内存里用，不进 JSON',
  'internal/upstream/login.go:SliderPuzzle':
    '一次滑块挑战的**内存表示**（两张图 + 加密的缺口坐标）；Rust 侧 derive `Debug, Clone, PartialEq, Eq`，**不** Serialize',
  'internal/upstream/login.go:PollOutcome':
    '一次扫码轮询的**判定结果**（Go 无 sum type，用"类别 + 字段"近似），进程内存里用，不进 JSON',
  'internal/upstream/login.go:SendSmsOutcome':
    '一次发码响应的**判定结果**，同上；Rust 侧是 enum，无 Serialize',
  'internal/app/app.go:Deps': '组装根的**依赖集**，不是数据模型',
  'internal/server/server.go:Deps': '组装根的**依赖集**，不是数据模型',
  'internal/server/server.go:Options':
    '`mcloud-server` 的**启动参数**（库路径 / 监听地址 / 前端目录 / cookie 是否带 Secure），由 flag 填、由 [Run] 读，不进 JSON；Rust 侧是 clap 的 `#[derive(Parser)]`，**没有** serde 的 Serialize',
  // `imports` 表的**一行**。字段名对应 SQL 列名，不是 JSON 键 ——
  // 它由 `scanImport` 逐列读出来，从不经过 `json.Unmarshal`。
  // 对外展示走 `Summary()`（刻意不含 `Raw`，那段可能带凭据）。
  // ⚠ 若哪天要把导入台账吐给前端（比如 CLI `--json` 列导入历史），
  // 正确做法是**另写一个带 tag 的响应体**并从这里删掉 ——
  // 它现在是"数据库的形状"，与"响应的形状"是两件事。
  'internal/io/configstore.go:ImportRecord':
    'Rust 侧只 derive `Debug, Clone, PartialEq, Eq`（**不** Serialize）—— 它是 imports 表的行，字段名对应 SQL 列名',

  // ── legacy_import：Rust 侧同样**不** Serialize，一律 derive Debug/Clone/PartialEq ──
  'internal/io/legacy_import.go:LegacySource':
    '旧数据的**两个路径**（asign.json 与数据目录），进程内存里用；Rust 侧 derive `Debug, Clone, PartialEq, Eq`',
  'internal/io/legacy_import.go:LegacyFile':
    '归档清单里的**一项**（文件名 + 绝对路径）；Rust 侧 `(String, String)` 元组，不是结构体',
  'internal/io/legacy_import.go:LegacyImportError':
    '实现了 error，对外走 `Error()` 文案；Rust 侧 thiserror 的 `#[error(...)]`，不进 JSON',
  'internal/io/legacy_import.go:MigrationIssue':
    '导入报告的**一条问题**（类别 + 文件 + 下标 + 原因），对外走 [MigrationIssue.String]；Rust 侧同样不 Serialize',
  'internal/io/legacy_import.go:MigrationReport':
    '导入报告本身，对外走 [MigrationReport.ToJSON] 的**手拼**结构 —— 那串键由它自己定，不给结构体加 tag 才不会有两份契约',

  // ── app::login：Rust 侧一律只 derive `Debug, Clone[, Copy]`，**不** Serialize ──
  'internal/app/login.go:LoginOutcome':
    '登录成功的**内存**结果（账号 id + 已解密的 userInfo），对外只走 `Summary()` 那句掩码文本；⚠ `UserInfo any` 的形状由**上游**决定，给它加 tag 等于把上游的字段约定写成我们自己的契约（同 `QrProgress` 那条理由）',
  'internal/app/login.go:LoginContext':
    '一次登录的**入参打包**（scope / 平台 / 设备指纹 / TTL），是调用点防"摆错顺序"的手段不是数据模型；Rust 侧 derive `Debug, Clone, Copy`，**不** Serialize',
  'internal/app/login.go:SmsCodeSent':
    '一次发码的**结果**（配额余量 + 是否过了滑块），喂给前端渲染一句提示；Rust 侧 derive `Debug, Clone, Copy, PartialEq, Eq`，**不** Serialize',
  'internal/app/login.go:SmsCredential':
    '短信登录的**入参**（手机号 + **6 位明文验证码**）。⚠⚠ 它**绝不能**进 JSON —— 一旦可序列化，验证码就会顺着日志与响应体出去；Rust 侧 derive `Debug, Clone, Copy`，**不** Serialize',
  'internal/app/app_login.go:AppCodeOutcome':
    '一次发码的**结果**（Go 无 sum type，用"类别 + 字段"近似），喂给前端渲染一句提示或一条 H5 兜底地址；Rust 侧是 `enum AppCodeOutcome`，**不** Serialize',
  'internal/app/app_login.go:AppLoginContext':
    '一次 App 登录的**入参打包**（scope / 设备档案 / 凭据），与 `SmsCredential` 同理：**里面装着明文验证码**，一旦可序列化就会顺着日志出去；Rust 侧 derive `Debug, Clone, Copy`，**不** Serialize',
  'internal/cli/run_fixture.go:ExtractReport':
    '一次抽取的**结论**（各类计数 + 写出的文件 + 自检残留），只喂给 `renderExtractReport` 打进终端；Rust 侧这个函数是逐行 `println!` 的计数，没有 serde。⚠ 它**不该**进 JSON —— 计数是给人看的报告，不是对外契约',

  // ── AI 头像族（#10，2026-09-24 登记：`rabactive/*` 六口 + `hcy/file/list` 上游层落地带来的）──
  // 这一族有一条**只对它成立**的理由：`rabactive/*` 六口**零实拍**（文件头那条"证据分两半"）。
  // 别的族是"实拍有，但档位表/语义没穷举"，这一族是**连一份响应样本都没有** ——
  // 此时补 tag 等于把"从旧 TS 的类型声明反推出来的字段名"钉成对外契约，
  // 而它唯一的支持证据是一份**没被上游确认过**的 TypeScript interface。
  // `hcy/file/list` 那一头虽有实拍，但它的 `items[]` 有十余格、我们只取两格，
  // 补 tag 会让"只取两格"这件事看起来像"上游只有两格"。
  'internal/upstream/app_ai_avatar.go:AiAvatarPrize':
    '`image/draw` 抽出的一格奖品名（`prizeName` / `contentName` 两枚指针）。两格**谁才是奖品名**未验证（零样本），旧实现按 `contentName || prizeName` 的优先序拼文案 —— 那个优先序只在内存里成立；marshal 出去会被读成"两个字段各有一个确定含义"',
  'internal/upstream/app_ai_avatar.go:AiAvatarDrawResult':
    '抽一次的**三态**结论（抽中 / 被 `408` 叫停 / 真失败）。三态里两态是"空值 + 一个布尔"，marshal 之后 `Stopped:false` 与"这一枪没打"在 JSON 里长得一样；而这一格存在的理由恰恰是让调用方**不必**读错误串就能分档（见 `ParseAiAvatarDraw` 的头注）',
  'internal/upstream/app_ai_avatar.go:AiAvatarTask':
    '`ai/image/merge` 与 `ai/image/getStatus` **共用**的解析结果（`taskId` / `status` / `imageUrl` 三枚指针）。⚠ `status` 的档位表（`3` 成功、`4` 失败）是从旧实现的分支形状**反推**的、零实拍 ⇒ 补 tag 会把一份没穷举的枚举写成契约；共用一个结构也是刻意的（同一段代码要吃两种形状），它不是"上游的字段约定"',
  'internal/upstream/app_ai_avatar.go:AiAvatarWork':
    '`image/getWorkList` 的一条作品（`id` + 两枚图片 URL）。`id` 在旧 TS 声明成 string 而活动接口常回数字，本层把两种都折成串 —— 这个折法是内部处置，写进契约就变成了"这一格是字符串"的承诺',
  'internal/upstream/app_ai_avatar.go:CloudFile':
    '`hcy/file/list` 条目里本族要用的**两格**（`fileId` / `name`）。实拍那一条有十余格（`category`/`type`/`createdAt`/两套审计…），刻意只取两格 ⇒ marshal 会让"我们只看这两格"看起来像"上游只有这两格"（与 `PrizesResult` 那条理由同族，但这里的裁剪比例更大）',
  'internal/upstream/app_ai_avatar.go:CloudFileList':
    '`hcy/file/list` 的一页（条目 + `nextPageCursor`）。⚠ 本族**不翻页**（旧实现固定 `pageSize:100` 一次），多于 100 个文件时后面那些**静默匹配不到** —— 带着一个"从不使用"的游标出网，等于对外承诺一个我们这边**永远不会翻**的分页协议；另外 `nil` 与空列表这一对在 JSON 里也分不开（同 `CloudPhoneRuleConfigList` 那条理由）',

  // ── App 任务矩阵写侧（#9 那一族，2026-09-24 登记：`task/click` + `mrp doTask` + 分派/执行层）──
  // 这一族**共用**一条理由：它是"决定 → 打枪 → 回查"的内存中间态，对外只有
  // `core.RunReport` 的 `Summary` 那一句人话（`internal/server/run_dispatch_app_task_test.go`
  // 钉的就是"小结能在事件流里看见"）。给它们补 tag 的实际代价是：
  // 那几格的**分档语义**会被写成字段名，而其中几档至今零实测。
  'internal/upstream/app_task_click.go:TaskClick':
    '`signin/task/click` 的解析结果（`Notice` = `result.msg`、`Message` = 顶层 `msg`，两枚指针）。⚠ **四发实拍没有一条带回 `result`** ⇒ `Notice` 今天恒为 nil，它是"旧实现读所以我们留着"的那一格；把它 marshal 出去会让一个**从未观测到**的字段看起来像契约的一部分（判据只有 `code==0` 那一句，写在 `ParseTaskClick` 头注）',
  'internal/app/task_matrix_dispatch.go:TaskStepPlan':
    '分派层交回的**决定**（走哪一态、带哪枚 `key`、等不等）。它是纯内存的一次函数返回值，`Kind` / `Paced` 这些档位出网会被读成"上游认识的字段"',
  'internal/app/task_matrix_dispatch.go:taskStepInput':
    '分派要的外部事实（备份读数 + 本地日历日）。⚠ `BackupKnown` 与 `BackupOn` **必须分两格**：旧实现那句是严格 `=== false`，"没读到"与"读到 false"是两回事 —— 而 marshal 之后 `BackupOn:false` 与"没读到"在 JSON 里长得一模一样，正是这一对要防的误读',
  'internal/app/task_matrix_dispatch.go:taskRunnerSpec':
    '专属玩法常量表的一条（名字 + 要不要发入口上报 + 最后一发带哪枚 `key`）。它是**常量表的元素**不是数据模型（与 `upstream.SmRule` 同一条理由）；且那枚 `FinalKey` 的两档之一**零实拍**',
  'internal/app/task_matrix_dispatch.go:taskUploadShape':
    '一条玩法"要不要往网盘传文件、传几轮、什么形状"的**规格常量**（与 `taskRunnerSpec` 同一条理由：它是表的元素，不是数据模型）。三格语义全都依赖零实拍的那一族：`StopAtProcess` 读的 `process` 语义未验证（R15），`Channel` / `Op` / `Ext` 决定的是 `file/create` 文件型那一发的请求体，而那一发**本仓一条样本都没有**（R27）⇒ 出网会让"照旧实现落下来的形状"看起来像已被上游确认的协议',
  'internal/app/task_matrix_run.go:TaskMatrixRun':
    '这一条链的**入口装配**（内嵌 `SignInWorld` + `Wait` 注入点 + `SkipVerify`）。⚠ 它内嵌的 `SignInWorld` 带着 `Store` 与换到的会话，一旦可序列化就顺着日志出去（同 `SmsCredential` / `AppLoginInput` 那两条：里面装凭据的入参打包永不补 tag）',
  'internal/app/task_matrix_run.go:TaskStepRecord':
    '对**一条任务**做过什么的记录（处置类别 / runner 名 / 理由 / 失败原文）。`Kind` 的中文语义由 `String()` 给人看；`Err` 是**上游原文**，把它结构化出网等于把上游的错误文案写成契约',
  'internal/app/task_matrix_run.go:TaskVerifyRecord':
    '复查趟对一条任务的读数（四档 + 该去哪个 App + 补打卡说明）。⚠ "补打卡"那一发用的 `key` **零实拍**，说明文本里就写着这件事 ⇒ 出网会让一个未验证的写法看起来是既定协议',
  'internal/app/task_matrix_run.go:TaskRunCounts':
    '一次运行的计数（读 / 待做 / 打卡 / 玩法 / 失败 / 缺口 / 规则跳过 / 需人工 + 复查四格）。它是 `SummaryLine()` 那一句人话的**原料**，界面上今天没有任何一格按 key 读它（`capability.output` 在 `web/src` 里零命中）⇒ 补 tag 只会造出一份没人兑现、也没人测试的"字段契约"',
  'internal/app/task_matrix_run.go:TaskMatrixRunResult':
    '一个账号这一趟的产出（`Steps` / `Verified` / `Counts` / `Notes` / `CloudFileIDs` / `CloudProductFiles`）。对外只经 `RunReport()` 的 `Summary`；`Steps[].Err` 与 `Notes[]` 里是上游原文与人话，marshal 会把"给人看的那一句"拆成一份没人对齐的结构。⚠ `CloudFileIDs` 里**只有**网盘对象 id，刻意不带预签名 `uploadUrl`（那条纪律在 `internal/upstream/app_cloud_upload.go` 文件头）；它就是链尾那一发 `batchTrash` 的输入（#37 已接，结局进 `Cleanup`）。`CloudProductFiles` 是这份名单里"上游另存的产物"那一半的**枚数**，只给小结那句话用、不参与判定，补 tag 等于把一份播报口径升成对外字段',

  // ── 网盘上传族（#35，2026-09-24 登记：`file/create` + `PUT` + `complete` + `batchTrash`）──
  // 这一族有本仓**最硬**的一条不补 tag 理由：`CloudFileCreated.UploadURL` 是**预签名上传地址**
  // （带一次性凭据），与 §"凭据边界"里 `auth` 整串同一级 —— 只进不出。下面逐条写各自成立的那一格。
  'internal/upstream/app_cloud_upload.go:CloudFileCreateInput':
    '`file/create` 的**请求侧入参集**（父目录 / 名字 / size / contentHash / 档位）。body 由 `CloudFileCreateRequest` 现拼那**十键** map（与旧 TS 逐字对齐），从不整体 marshal 这个结构体 ⇒ 补 tag 等于造一条会多发键的旁路请求，而"上游认不认多出来的键"零样本。默认值也刻意留在这里而不是 tag 里：`ParentFileID:""` ⇒ `/`、`Channel:""` ⇒ android，那是**旧实现的默认**，不是 JSON 的默认',
  'internal/upstream/app_cloud_upload.go:CloudFileCreated':
    '`file/create` 的解析结果。⚠⚠ `UploadURL` 是**预签名直传地址**（URL 里就是一次性 token），与 `auth` 整串同一条纪律"只进不出" —— 这个结构体一旦被 marshal，离泄漏只差一次"顺手把 create 的响应写进日志/响应体"，而 `core.SecretValue` 那层保护**不在这条路上**（它是裸 `*string`，因为上游给的本来就是明文串，包一层反而遮住"这里曾是 URL"）。另外三格 `*string` 与两个布尔的区别也必须留在内存：`Exist` / `RapidUpload` 是编排层**三态结局**的唯一判据（见 `app/cloud_upload.go:CloudUploadOutcome`），而本口**文件型**零实拍（唯一样本是建目录那一型）⇒ 字段名与"会不会回这一格"都还没被上游确认过',
  'internal/app/cloud_upload.go:CloudUploadReceipt':
    '一次上传的**内存收据**（结局 + fileId + 实际发出去的名字）。`Outcome` 是 int 枚举且**零值刻意是"未上传"**：marshal 出去前端拿到 `0` 既读不成"未上传"也读不成"已上传"，而这一格的正误决定**要不要往清理那一族交 id**（交错就是删用户自己的文件）。调用点今天有了（app 层 106 / 522 与 beiyong1 那三条玩法），但它仍**不出接口**：对外只有小结里"网盘新增 N 个文件（已移进回收站 / 本次保留 / 清理失败）"那一句括号 ⇒ 补 tag 等于把一份零实拍的清理协议写成形状',
  'internal/app/cloud_upload.go:CloudUploadOptions':
    '一次上传的**可调参数**，零值 = 旧实现 `uploadRandomFile` 那一档。默认值由 `channelOrDefault` / `extensionOrDefault` / 上游层那两处**依次**补齐，补 tag 会造出第二个"默认值真相"；且 `Op` 只有 android 档读（web/PC 档忽略它），这个"档位决定哪一格生效"的关系不是字段能表达的',
  'internal/app/cloud_upload.go:CloudUploader':
    '一次上传的**注入点**（上游句柄 + 随机源 + `Auth` + `DeviceID`）。⚠ `Auth` 是 `Authorization` 头的**整串**（真凭据），一旦可序列化就顺着日志出去（同 `SmsCredential` / `AppLoginInput` 那两条：里面装凭据的入参打包永不补 tag）；`Up` 里还装着 HTTP 客户端，marshal 到它直接报错',
  // ── 回收站那一族（2026-09-26 第廿七趟：`recyclebin/list` 落码 + 第一次真打）──
  // 与同族 `CloudFileCreated` 同规：解析结果是**内存形状**，本层手工从 `map[string]any` 读，
  // 从不 marshal / unmarshal 这些结构体 ⇒ 补 tag 不会让解析多认一格，只会把
  // "这些字段名是对外契约"假成文（AGENTS §1.4：JSON 字段名要跟黄金向量比对过才算数）。
  'internal/upstream/app_cloud_recyclebin.go:CloudRecycleBinEntry':
    '`recyclebin/list` 一条条目的解析结果（七格全指针）。"上游没给这一格"与"这一格是 0/空"的区别必须留在内存里，而本层是**手工读 map**（`optStrField` / `asInt64`），这些 tag 一行都不会被用到 ⇒ 补上只是把一份零消费者的形状写成对外契约。`TrashedDirStatistics` 那型可能整体为 `null`（file 型），这个"按 `type` 换形状"的关系不是 tag 能表达的',
  'internal/upstream/app_cloud_recyclebin.go:CloudRecycleBinDirStats':
    'folder 型条目上 `trashedDirStatistics` 的三格。⚠ 它的**语义今天没判据**（实拍那枚目录删前有 1 个文件、这一格却 0/0/0 ⇒ 三种解释分不开，见 `docs/capture/app-cloud-recyclebin-samples.txt` §10），钉成 JSON 形状就是让前端把"0"念成"这个目录是空的"',
  'internal/upstream/app_cloud_recyclebin.go:CloudRecycleBinPage':
    '一页回收站读数。`HasCursorField` 是**本层自己造的第三态格**（区分"键在但为 `null`"与"键缺席"，线路上根本没有这一枚键，#197），给它任何 tag 都是撒谎；`Items` / `NextPageCursor` 由 `ParseCloudRecycleBinPage` 手工填。且这一口**今天没有生产调用点**（消费者只有探针），出网面为零',
  // ── 外链那一族（#53，2026-09-25 随玩法 434 落地）──
  'internal/upstream/app_cloud_outlink.go:CloudOutLinkInput':
    '`getOutLink` 的**请求侧入参集**（`Account` + `FileIDs` + `DedicatedName`）。⚠ `Account` 是**完整手机号** —— 与 `CloudUploader.Auth` 同一条"只进不出"：这一旦可序列化，离把号码写进日志/响应体只差一次"顺手 marshal 一下入参"。body 由 `CloudGetOutLinkRequest` 现拼那**十一键** map（与旧 TS 逐字对齐，含 `subLinkType:0`、`encrypt:1` 这些**不能缺**的档位）⇒ 补 tag 会造出第二条旁路，而"这一口认不认多出来的键"零实拍',
  'internal/upstream/app_cloud_outlink.go:CloudOutLink':
    '`getOutLink` 的解析结果（只有 `LinkID` 那一格）。它是编排层**"能不能删"的唯一判据**：`nil` 与"给了空串"都要被上层播成失败（拿不到 id ⇒ 那条公开外链留在用户网盘里，见 `runShare` 函数头那三处刻意不同）。本口**零实拍** ⇒ 字段名与"会不会回 `getOutLinkResSet`"都还没被上游确认过，补 tag 等于把一份未证实的形状写成对外契约',
  // ── 云笔记那一族（#66，2026-09-25 随玩法 107 落地）──
  'internal/upstream/app_note.go:NoteCreateInput':
    '`createNote.do` 的**请求侧入参集**（`NoteID` / `Title` / `Phone`）。⚠ `Phone` 是**完整手机号** —— 与上面 `CloudOutLinkInput.Account`、`CloudUploader.Auth` 同一条"只进不出"：这一旦可序列化，离把号码写进日志只差一次"顺手 marshal 一下入参"。而 body 由 `NoteCreateRequest` 现拼那**二十六键** map（与在跑的 caiyun 逐字对齐，`tags: []`、`contents[]` 那一格是嵌套形状）⇒ 补 tag 等于造第二条会多发键的旁路请求，而"这一口认不认多出来的键"零实拍（本族三口今日仍无一条真实反馈，见 `docs/pending-decisions.md` R32）',
  // ── 139 邮箱会话那一族（#67，2026-09-25 随玩法 472/473 落地）──
  'internal/upstream/app_mail139.go:MailSession':
    '两张字段（`SID` / `RMKey`）**都是会话凭据**，类型是 `core.SecretValue`：它的 `String()` 只挡打印，**不挡 `encoding/json`** ⇒ 一旦补上 tag，"顺手 marshal 一下这张会话"就会把 sid/rmkey 原文写进日志、错误文本或 fixture（三者都是本仓明令禁止的落点，见 `docs/sanitize-guide.md`）。这一族的③那一发更是把两枚直接拼进 URL、而传输层错误原样嵌 URL（`internal/io/client.go` 的 `mapTransportError`）⇒ 不给 tag 是"只进不出"这条边界在类型上的那一份，不是漏写。响应形状零实拍（② 那条只拍到请求面）⇒ 也没有任何外部契约要钉',
  // ── 139 邮箱资产侧那一族（#71 落地，本轮 2026-09-26 加 ⑥ 的回执）──
  'internal/upstream/app_mail139_assets.go:MailConvertReceipt':
    '⑥ 认下那一发时上游交代的**两格**（`summary` 原话 + `var.directoryId`），是手解 `map[string]any` 之后摊出来的**内存解析结果**：对外只有编排层拼的那一句人话（`runMailPDF` 的失败串与 warn 行），没有任何读者要这一族的 JSON 形状 ⇒ 没有契约要钉。⚠ 反向同样不许为了过门禁补 tag：`DirectoryID` 是**用户网盘里那一枚目录的真实 fileId**（本仓纪律："真跑读数里的真实 fileId 不进任何成文"），而 `Summary` 里那句"已成功转存至：…"带用户可见的路径 —— 一旦可 marshal，"顺手把回执整个打出去"就会把两者写进日志或响应体',
  // ── 任务拓展那一族（#81，2026-09-26 随 `task-expansion` 落地）──
  'internal/app/task_expansion_run.go:TaskExpansionRun':
    '一条链的**注入点**，不是数据：内嵌的 `SignInWorld` 里装着 `*upstream.Upstream`（HTTP 客户端）、`*SessionPool`（进程内**所有账号**的票池）与 `core.StateStore`（库句柄），marshal 到它们要么直接报错、要么把别账号的凭据带出进程；`Wait`/`BackupWait` 两格的正误只服务于测试压小等待，没有任何外部读者。对外形状另有出口：`RunReport` 与 `SummaryLine`（`internal/exec`）⇒ 补 tag 只会造出第二条**没人读**的形状',
  'internal/app/task_expansion_run.go:TaskExpansionResult':
    '一个账号这一趟的**内存结论**。三处区别只有类型能保住、tag 会抹掉：① `CloudCount *int64` 的 `nil` 与 `0` 是两档（上游没报 ≠ 报了 0 朵，同 §3.6.7 的口径），补 tag 出去前端只能看到其中一档；② `UploadFileID` / `UploadName` 是**用户网盘里的真实 fileId**，本仓纪律是"真跑日志里的真实 fileId 不进任何成文" ⇒ 这一旦可序列化，离把它写进响应体/日志只差一次"顺手 marshal"；③ `BackupStillOff` 与 `Status` 是**独立两位**（可以同时成立），档位取舍由 `RunReport` 现拼，字段平铺出去会被读成互斥。今天没有任何前端或协议读它 ⇒ 没有契约要钉',
}

function walk(dir, out = []) {
  const abs = path.join(ROOT, dir)
  if (!fs.existsSync(abs)) return out
  for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
    const rel = path.join(dir, entry.name)
    if (entry.isDirectory()) {
      // ⚠ 只走 Go 源码目录：别进 testdata / target 之类。
      walk(rel, out)
    } else if (entry.name.endsWith('.go') && !entry.name.endsWith('_test.go')) {
      out.push(rel)
    }
  }
  return out
}

// 抓 `type X struct {` … 到行首的 `}` 为止。
//
// ⚠ 只认"行首的 `}`"：Go 的 struct 声明结束时 `}` 一定在第 0 列。
// 内联的匿名 `struct{…}{…}`（marshalJSON 里那些）不以 `type` 开头，不会被抓到。
const STRUCT_RE = /^type ([A-Za-z_]\w*) struct \{$/

function structsOf(src) {
  const lines = src.split('\n')
  const found = []
  for (let i = 0; i < lines.length; i++) {
    const m = STRUCT_RE.exec(lines[i])
    if (!m) continue
    const fields = []
    for (let j = i + 1; j < lines.length; j++) {
      if (lines[j] === '}') break
      const body = lines[j].trim()
      if (!body || body.startsWith('//')) continue
      fields.push(body)
    }
    found.push({ name: m[1], fields })
  }
  return found
}

// 一个"字段行"的粗略判定：行首是大写字母 ⇒ 导出字段。
//
// ⚠ 排除只有**一个** token 的行 —— 那是嵌入字段（`Base`）或类型名，
// 不是"名字 + 类型"。
function isExportedField(line) {
  const head = line.split(/\s+/)[0]
  if (!head) return false
  if (!/^[A-Z]/.test(head)) return false
  return /\s/.test(line)
}

function main() {
  const files = SCAN_DIRS.flatMap((d) => walk(d))
  const mixed = []
  const untaggedNotAllowed = []
  const allowedSeen = new Set()
  let tagged = 0

  for (const rel of files) {
    const src = fs.readFileSync(path.join(ROOT, rel), 'utf8')
    for (const st of structsOf(src)) {
      const exported = st.fields.filter(isExportedField)
      if (exported.length === 0) continue // 全是非导出字段 ⇒ 不进 JSON，跳过
      const withTag = exported.filter((l) => l.includes('json:"'))
      const key = `${rel}:${st.name}`

      if (withTag.length === exported.length) {
        tagged++
        continue
      }
      if (withTag.length > 0) {
        const missing = exported.filter((l) => !l.includes('json:"'))
        mixed.push({ key, missing })
        continue
      }
      if (UNTAGGED_ALLOWED[key]) {
        allowedSeen.add(key)
      } else {
        untaggedNotAllowed.push(key)
      }
    }
  }

  let failed = false

  if (mixed.length > 0) {
    failed = true
    console.error('✗ 以下结构体**部分**字段缺 `json` tag —— 不可能"一半进 JSON"：\n')
    for (const m of mixed) {
      console.error(`  ${m.key}`)
      for (const f of m.missing) console.error(`      缺: ${f}`)
    }
    console.error('')
  }

  if (untaggedNotAllowed.length > 0) {
    failed = true
    console.error('✗ 以下结构体**整个**没有 `json` tag，且不在白名单里：\n')
    for (const k of untaggedNotAllowed) console.error(`  ${k}`)
    console.error(
      '\n  若它确实不进 JSON，把它加进 scripts/check-go-jsontags.cjs 的' +
        ' UNTAGGED_ALLOWED 并**写明理由**；\n' +
        '  若它要进 JSON，就给它加 snake_case 的 tag —— ' +
        '字段名是外部契约，由 test/golden/enum-json-vectors.json（serde 产出）比对。\n',
    )
  }

  // 白名单里已经不存在的条目 = 该复查了（结构体改名 / 加上了 tag）。
  const stale = Object.keys(UNTAGGED_ALLOWED).filter((k) => !allowedSeen.has(k))
  if (stale.length > 0) {
    failed = true
    console.error('✗ 白名单里有已经对不上的条目（结构体改名或已加上 tag）—— 请删掉：\n')
    for (const k of stale) console.error(`  ${k}`)
    console.error('')
  }

  if (failed) process.exit(1)

  console.log(`jsontags 通过：${tagged} 个结构体字段齐全，${allowedSeen.size} 个按白名单不进 JSON`)
}

main()
