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
