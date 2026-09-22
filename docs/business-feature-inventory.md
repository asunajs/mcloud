# 待实现业务功能盘点（旧项目 → 本项目）

> **标题口径（2026-09-22）**：这一份最初写的是"旧项目 → **Rust** 新项目"，正文里"新项目落点""§0 表头"
> 沿用当时的说法。**Rust 那一版已经冻结并剥离为同级仓库 [`../mcloud-rust`](../../mcloud-rust)**，
> 当前在写的是 Go（本仓库 `internal/`）。所以看本文时把"落点"读成 Go 落点，
> 遇到 `mcloud-<crate>::…` 这种写法就是 Rust 基准里的旧名字（除历史引用外已逐条改成 Go 的 `pkg.Symbol`）。

> **这份文档是什么**：把旧项目 `/Users/catlair/Documents/workspace/mcloud-sign` 里**真实存在的业务功能**逐条拆到"能照着写代码"的粒度 —— 每条给：做什么、上游请求序列、判据分支、副作用等级、幂等与并发、依赖与共享状态、新项目落点。
>
> **和 `../mcloud-rust/docs/rust-rewrite-plan.md` 的分工**（该方案随 Rust 剥离住在那边）：它的 §1.2 只给了 19 项的名字与风险序、§7.1 给了实现顺序、§1.3–1.5 给了加密原语与已知缺陷。**本文补的是中间那层：每项能力内部的流程与判据**。两文冲突时以 §7.1 的顺序为准，以本文为单项实现规格。
>
> **证据口径**：`path:line` 指向旧项目文件，均为本次通读时逐条打开核对过的。**未经真实流量验证的上游路径，本文一律标注旧代码写法**，并在 §5 汇成"动代码前必须实测"清单 —— 新项目的基本原则仍是"按新抓包重写，不从旧 TS 搬"（方案 §6.3）。
>
> **凭据口径**：本文只写**形状**与**来源**，不写任何真实值；应用级白盒常量（签名盐、`DEVICE_INFO`、内置公钥）注明"非账号凭据"。

---

## 0. 一页总览

| # | 能力 | 产出物 | 副作用 | 需人工 | Rust 侧现状（基准冻结时，2026-09-19） |
| --- | --- | --- | --- | --- | --- |
| 1 | `prizes` 待领取奖品 | 只读列表 | 无 | 否 | 无 |
| 2 | `cloud-snapshot` 云朵快照 | 本地日快照 | 无（新能力，旧项目缺，见 D1/D2） | 否 | **仅注册表声明**（`capabilities::REGISTRY`） |
| 3 | `sign` 每日签到 | 云朵 | 记账 | 否 | 声明有，上游 `app_signin` 已有抓包 |
| 4 | `receive` 领取云朵 | 云朵 | 记账 | 否 | 无 |
| 5 | `revival` 复活奖励 | 云朵 | 记账，月限 2 | 否 | 无 |
| 6 | `auth-refresh` 凭证刷新 | 新 token | 记账 | 否 | 登录链已有 token 解析（`core::auth`），刷新无 |
| 7 | `after` 收尾 | 日志 + **删网盘文件** | **改用户网盘** | 否 | 无 |
| 8 | `task-expansion` 任务拓展 | 云朵 | **在用户网盘真建随机文件** | 否 | 无 |
| 9 | `app` App 任务矩阵 | 云朵/抽奖次数 | 记账 + 上传 + 建删外链 + 发邮件 | 部分任务码需人工 | 无 |
| 10 | `ai-avatar` AI 头像 | 头像 + 抽奖 | **上传文件到网盘** | 否 | 无 |
| 11 | `msg-push` 消息推送任务 | 云朵 | 记账 | 需先在 App 开启推送 | 无 |
| 12 | `notice` 公告任务 | 云朵 | 记账 | `未开启` 需人工 | 无 |
| 13 | `backup-gift` 备份礼 | 云朵/权益 | 记账（GET 却是写） | 否 | 无 |
| 14 | `hc1t` 云朵大作战 | 游戏次数/排名 | 记账 + **建立邀请关系** | 否 | 无 |
| 15 | `red-flower` 小红花 | 小红花/奖品 | 记账，口令一次性 | 否 | 无 |
| 16 | `play-ai-special` 拍拍 | 抽奖 | 记账 | 两个子任务设计内跳过 | 无 |
| 17 | `play-ai-lottery` 玩 AI 抽奖 | 抽奖 | 记账 | 否 | 无 |
| 18 | `cloud-phone-redpack` 红包派对 | **现金（分）** | **真给云机装 APK** | 三类任务需人工 | 无 |
| 19 | `kouling` 直播口令 | 小红花 | 入群足迹 + WS 长连 | 否 | 无 |
| 20 | `exchange` 抢兑 | 奖品记录 | **真扣云朵、不可回滚** | 需短信码的奖品必失败 | 仅注册表声明 |

（`cloud-snapshot` 是新项目按 D1/D2 增设的，不在旧项目 19 项里；旧项目 19 项 = 表中 1、3–20。）

> **Go 侧现状（2026-09-22 逐条 grep 核对，读的是本仓库代码）** —— 上表那一列是 Rust 基准冻结时的记录，别拿它当现状：
>
> - **已有实现且 CLI 可达**：#1 待领取奖品（`app.PendingPrizes` + `mcloud signin --prizes`）、
>   #2 云朵快照（`app.RecordCloud` / `core.NextCloudStats`）、#3 签到（`app.CloudSignIn`）、
>   #4 领取（`app.ReceiveCloud` / `ReceiveAll`，`--receive`）、#5 复活卡（`app.ReceiveRevivalReward`，`--revive`）、
>   #8 任务拓展（`app.ReceiveTaskExpansion` 一族）、#13 备份礼（`app.ClaimBackupGift`，`--backup-gift`）；
>   另有**连签倍数**（`app.ClaimMultiple`，`--multiple`），它不在旧项目那 19 项的编号里。
> - **只有声明**：#20 `exchange` 抢兑 —— 注册表里有 `Code: "exchange"`、存储里有 `exchange_config` 表，
>   上游"抢兑"那条接口在本仓库搜不到。
> - **仍无**：#6 凭证刷新（`core`/`upstream` 里只有 `RefreshToken` 这类**登录链**的痕迹，不是独立的续期能力）、
>   #7 `after` 收尾、#9 App 任务矩阵、#10 AI 头像、#11 消息推送、#12 公告、#14 云朵大作战、
>   #15 小红花、#16/17 拍拍与 AI 抽奖、#18 红包派对、#19 直播口令 —— 这些在 `internal/` `cmd/` 里按关键词全查不到。
>
> 所以下面那段"19 项业务能力：0 项可执行"已经**不再成立**（它写于 2026-09-19 的 Rust 侧）。
> 仍然成立的是它后半句：剩下那些项缺的不是单个接口，是它们共用的那一层（§3）。

**当时 Rust 侧可跑的东西**（2026-09-19 订正；这几条线如今都在 Go 侧，同值结论见上面那条 09-22 核对）：
登录（PC 短信/扫码、App 短信）、账号会话与归属、
设备指纹 smid、**App 端签到（云朵中心）全链路**、旧数据迁移、`internal/app/cloud.go` / `signin.go` 的上游封装。
**schema 现为 v7**（v4 归属 → v5 smid 两列 → v6 `account_state.phone` → v7 `Authorization` 整串存）。
> ⚠ 这一句写于 v8 之前：**Go 侧现在到 v8**（`internal/io/schema.go` 的 `SchemaVersion = 8`，
> v8 那一条引入 `run_events` 表 ⇒ 运行事件流落库）。v4→v7 那条演化线仍然有效。
**19 项业务能力：0 项可执行** —— 缺的不是接口，是它们下面那一层（§3）。

> 上面这段的上一版写的是"schema v4 / v5、`mcloud-app::cloud` / `signin` 并行工作流**未提交**"（那是 Rust 时期的写法与状态，照原文留着）。
> 那描述的是两份盘点刚写完、实现还在飞的时候；现在那些提交已经落地
> （云端 9 个 + 本地 1 个），语境消失，故订正。

**补充（2026-09-19 用户订正，对上面这张表的三处读法）** —— 表格本身按"旧项目代码怎么写"保留原样，读的时候叠加这三条：

- **第 5 行 `revival`**：表里"月限 2"是**旧代码的硬编码保险值**，上游真实月限是 **1 次**；之所以不敢收紧到 1，是因为
  **判断能否复活的接口从未抓到**。展开见 §2.2 该条与新增的 §5.8。
- **第 7 行 `after`**：它的"删网盘文件"风险**不在错删**（`batchTrash` 按唯一 `fileId` 删），而在**漏记导致云盘侧残留孤儿文件**；
  "归属到 run"指我们自己的产物清单，上游没有可查的文件归属。展开见 §2.2 `after` 第 3 条。
- **第 14 行 `hc1t`**：**该任务已过期**，不进实现顺序；整段保留只为"活动站会话 + 埋点"这一通用形态做样本。展开见 §2.4 与 §5.9。

---

## 1. 四条跨功能的公共语义（先定，否则每个能力各错一遍）

### 1.1 参数从哪来：旧项目**没有**参数 schema

`TaskModule = { name, title, description, hidden?, run(ctx, body?) }`（`packages/api/src/tasks/types.ts:9-24`）—— `run` 返回 `Promise<unknown>`，入参无 schema，全靠 `description` 文案 + 函数体里手写 `body?.X`。账号定位参数（`accounts` / `account` / `accountMode` / `mode` / `force`）与任务参数**共用同一个 body 命名空间**（`packages/api/src/index.ts:102-133`）。

三个后果，新项目必须逐项破掉：

1. **参数集要靠读代码枚举**（本文每张"关键参数"列就是产物），不能靠接口自描述；
2. **无默认值合并**：只有 CLI 与 `runHc1t` 走 `defaultAccountConfig → common → user` 三级深合并（`packages/cli/src/index.ts:68-78`），API 路径直接把裸账号对象当 `ctx.config`（`packages/api/src/account.ts:133-140`）⇒ 经 API 触发时 `ctx.config.云朵大作战` 常整体 `undefined`，全靠代码里 `?? 默认值` 兜底，`开启兑换` 永远 falsy；
3. **配置默认值三处打架**（schema / 代码常量 / 文档各写一份），见 §5.4。

新落点：`core.FieldSpec` → `capabilities.All()`（`internal/capabilities/registry.go`），参数、账号定位、执行模式**三者分离**，WebUI 表单 / API 校验 / 组合步骤参数 / CLI `--help` 同源生成（方案 §3.3、D3）。

### 1.2 判据：旧项目同一代码库混用**三套**信封

| 判据 | 定义位置 | 语义 |
| --- | --- | --- |
| `tryRequest` | `packages/shared/src/result.ts:69-86` | HTTP 抛错 **或** `code ∉ {0,"0000",200,"200"}` **或** 响应含 `err\|errcode\|error` 键 ⇒ 一律 Error，**业务码丢失** |
| `tryResponse` | `:93-103` | 仅 HTTP 层抛错算 Error，业务 `code` 原样透传交调用方判 |
| `header.status` | `packages/core/src/tasks/cloud-phone-redpack/task.ts:77-79` | 红包派对域**完全没有 `code` 字段**，`String(header.status)==="200"` 才算成功 |

⇒ 新项目要把它收敛成**三类失败**并分别可测：`Transport`（超时/连接/DNS）、`Biz(code,msg)`（业务码，携带原始码）、`UnknownEnvelope`（信封不认识 —— 必须显式报错，不能像旧代码那样落到"其他 code"只打日志）。方案 §7.2 的 error 分类（`Auth/RateLimited/SoldOut/InsufficientCloud/Timeout`）是它的子集，需补 `ManualRequired`（§1.4）。

### 1.3 幂等：**旧项目一个幂等键都没有**

- 写操作大量使用 **GET**：`exchangeV2`（下单）、`market/signin/page/receiveV2`（领云朵）、`market/backupgift/receive`、`hecheng1T/beinvite|finish|exchange`。
- 唯一的传输层保护是**默认零重试**（`packages/shared/src/http/base.ts:64` `retry: options.retry ?? 0`；`createTaskHttp` 硬编码 `:192 retry: 0`）。
- 并发闸只有一层且是**进程内**的：`runner` 用 `${task.name}:${accountLabel}` 作 key，**已存在 promise 就直接返回同一个结果**（`packages/api/src/runner.ts:29-33,106-115`）。key 里**不含参数** ⇒ 同账号带不同 `prizeId`/`codes` 的第二次请求，拿回的是第一次的结果。这是全套代码里最危险的语义（方案 §3.5 的"同(能力+账号)互斥"必须补上参数维度或显式拒绝）。
- 天然幂等只有两处靠服务端：`redFlower/draw` 的 `200112`（口令已兑换）、任务状态预检（`userStatus===1` 跳过）。

⇒ 新项目的落点：`runId`（uuid v4）作为幂等单元 + 写操作在能力声明里标 `CapabilityKind::Action` 并强制"状态预检 → 执行 → 复查"三段（`mcloud-exec`，P5）。**HTTP 层零自动重试**这条对写操作必须保持（读重试只允许用于幂等 GET）。

> 2026-09-22 补：这句写于迁移之前，当时的"落点"是计划。现在前两件已经有 Go 实现 ——
> `core.RunID`（幂等单元）、`exec.MutexKey`（键里带 `exec.ParamsDigest` 的参数维度，正是
> 上面批评旧 `runner` 缺的那一维）、`exec.RunRegistry.Admit`，能力性质是
> `core.KindAction` / `core.KindQuery`（`internal/core/model.go`）。
> ⚠ **"状态预检 → 执行 → 复查"三段仍未被结构性地强制** —— 它要靠各能力的编排自己写，
> 目前没有断言盯着。相反，**"写操作不许盲目重试"那一条已经被结构强制了**，
> 比原文设想的"靠约定"更强：传输层确实有自动重试（`io.Client.Send`，按 `RetryPolicy`），
> 但**两个条件必须同时成立**才重试 —— 错误类别可重试（`core.ErrorKind.IsRetryable`）
> **且**请求可重试（`core.HTTPRequest.IsRetryable`：读天然可，写必须带
> `WithIdempotencyKey`）。判据在 `internal/io/client.go`，
> 三条用例钉住（`TestReadRequestsRetryButWritesNeedAnIdempotencyKey` 等）。
> ⇒ 落地形态与旧项目相反但目标一致：旧项目靠"整个传输层不重试"，
> 本项目允许读重试、把写重试关在幂等键后面。

### 1.4 "需人工"不是一句日志

旧项目用 `logger.info("需手动完成: …")` 表达"这个任务我做不了，得你去 App 里点"，返回值仍是成功 ⇒ 前端与推送都看不到。五个能力有这类出口：

| 能力 | 需人工的情形 | 证据 |
| --- | --- | --- |
| `cloud-phone-redpack` | 无云手机 / 云机锁屏 / 未知任务码 / "需云机内实际使用"（**该分支还 `return true`** = 假成功） | `task.ts:270-284,383-384,471-472`，`TASKS.md:87` |
| `msg-push` | `pushOn === 0` ⇒ `logger.error("消息通知已关闭，请前往 APP 手动打开")` 后 return；`firstTaskStatus !== 3` ⇒ `首次奖励未领取，请前往 APP 手动完成`（**info 级，不影响成功**） | `packages/core/src/tasks/msg-push.ts:20-30` |
| `notice` | `短信通知未开启，请手动开启` / `邮箱通知未开启，请手动开启`；只有 `Number(day) > 0 && canReceive === 1` 才会上报状态 | `packages/core/src/tasks/notice.ts:20-50` |
| `backup-gift` | `state === -1` ⇒ `未开启备份，请前往 APP 手动开启`（warn，仍算成功） | `packages/core/src/tasks/backup-gift.ts:27-30` |
| `play-ai-special` | `生成我的校园海报(33)`、`体验AI相机(34)` 设计内跳过 | `packages/core/src/tasks/play-ai-special/task.ts` |

⇒ 新项目的 `Run` 结果状态需要第四态：`Success / PartialSuccess / ManualRequired / Failed`，并且 `ManualRequired` 要能进推送与历史（本文档不改方案 §3.5，属对它的补充要求）。

---

## 2. 能力逐条

顺序沿用方案 §7.1 的五档。**每档先说共同的坑，再逐条给上游序列。**

### 2.1 档 1 · 查询型低风险

#### `prizes` 待领取奖品查询

- **做什么**：列出账号名下待领取的奖品记录（抢兑、抽奖、活动奖励的最终落点），供人工去 App 领取。
- **上游**：`GET https://m.mcloud.139.com/ycloud/prizeApi/checkPrize/getUserPrizeLogPageV2`（`packages/core/src/apis/task.ts:70`），入参 `(pageNumber=1, pageSize=10)`。
- **判据**（`packages/core/src/tasks/prize-records.ts:8-30`）：`err || !data?.result?.result` ⇒ 打"无待领取奖品"；`filter(r => r.flag === 1)` ⇒ 待领取；过期时间按 `Asia/Shanghai` 格式化输出。
- **已知不足（重写要修）**：① **只读第 1 页 10 条**，多于 10 条静默截断；② `flag` 的其他取值没有任何解释；③ 双层 `result.result` 信封是本域特有形状，`tryRequest` 未必兜得住。
- **新落点**：`internal/upstream` 新增 prize 模块（抓包确认 `getUserPrizeLogPageV2` 是否还在）→ 注册表里加一条能力（**这条两侧都还没有**：Go 的 `capabilities.All()` 目前只有 sign / exchange / cloud-snapshot 三条；奖品那边现有的 `app.PendingPrizes` 与 CLI `--prizes` 是签到页的待领取奖品，不是这个），参数 `pageNumber/pageSize`，输出 `prizeName/prizeId/expireTime/flag`。

#### `cloud-snapshot` 云朵余额快照（新能力）

旧项目**没有**这个功能：`sign` 里 `ctx.logger.info("当前云朵：…")` 之后那行 `ctx.state.totalCloud = total` 被注释掉了（`packages/core/src/tasks/sign/task.ts` 内，方案 D1 记为 `sign/task.ts:45`），所以余额只进日志、不进状态 ⇒ 无法回答"这账号现在还有多少云朵、今天涨了多少"（D2）。

新项目已按此增设能力声明（Go 侧在 `internal/capabilities/registry.go` 的 `All()`；Rust 基准在
`../mcloud-rust/crates/mcloud-capabilities/src/lib.rs`，`code: "cloud-snapshot"`，`core.KindQuery`，输出含 `updatedAt`）。待补的是上游：`GET …/market/signin/page/infoV3?client=app`（旧写法，实测前缀见 §5.1）与 `…/market/signin/public/cloudRecord?type=1&pageNumber=1&pageSize=100`（`packages/core/src/tasks/after.ts:24`），落 `cloud_stats` 日快照表。

### 2.2 档 2 · 日常核心：`sign` → `receive` → `revival` → `auth-refresh` → `after`

> **本档共同的坑**：**全链吞错**。`sign` 的每个失败分支都只 `ctx.logger.error(...)` + `return`，从不抛错 ⇒ 对 `runner` 永远"成功"（`ok = 没抛异常`）。新项目里这类步骤失败必须变成 `Run` 的失败或部分失败，否则定时任务会在"天天报错"的情况下绿灯一个月。

#### `sign` 每日签到（`packages/core/src/tasks/sign/task.ts:15-80`）

| # | 请求 | 关键键 | 意义 |
| --- | --- | --- | --- |
| 1 | `GET https://m.mcloud.139.com/market/signin/page/startSignIn?client=app`（`apis/signin.ts:24`） | — | 发起签到 |
| 2 | `GET https://caiyun.feixin.10086.cn/market/signin/page/infoV3?client=app`（`:17`） | `total,toReceive,curMonthBackup,curMonthBackupSignAccept` | 签到后详情；打印"当前云朵 / 待领取" |
| 3 | `POST …/ycloud/liveRoomFeedback/user/info`（`apis/live-room.ts`） | `flowerCnt` | 顺手打印直播小红花数 |
| 4 | 条件：`curMonthBackup && !curMonthBackupSignAccept` ⇒ `GET …/market/signin/page/multiple`（`:31`） | — | 备份签到拓展 |
| 5 | `sleep(1000)` | — | **定长 1s**，防"签到状态未落" |
| 6 | 再 `GET startSignIn` | `cal[]` 里 `currentMonth && t` 的那天 | 复查并打印「今日 N 号已签到获得 X 云朵」 |

- **判据**：`todaySignIn === true` 即"今日已签到"直接返回；复查后仍非 true ⇒ `网盘签到失败`（**只是日志**）。
- **副作用**：记账（得云朵）。幂等由服务端"今日已签"保证，可重复调。
- **实测差异**：我们抓到的 App/H5 真实流量是 `GET https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app&deviceId=…`（`docs/capture/app-signin-samples.txt:336`）—— 前缀 `/ycloud/`、**带 `deviceId` 查询参数**。见 §5.1。
- **新落点**：`internal/upstream/app_signin*.go`（已有）+ PC/H5 版 `signin` 模块；注册表里 `Code: "sign"` 那条（编排在 `app.CloudSignIn`）；云朵数写入 `cloud_stats`（替代 D1 那行注释掉的代码）。

#### `receive` 领取云朵（`sign/task.ts:121-127`）

单请求 `GET https://caiyun.feixin.10086.cn/market/signin/page/receiveV2?client=app`（`apis/signin.ts:38`）。**返回值整体丢弃**：`const [err] = await tryRequest(receive, ctx.http)` ⇒ 领到了多少云朵从未被读出来。重写时必须把 `result.receive`（或等价字段）取回来并进 `cloud_stats`，否则"领取"这一步在数据上是黑盒。

#### `revival` 复活奖励（`sign/task.ts:131-171`）

- **上游**：`GET https://m.mcloud.139.com/ycloud/signin/page/receiveRevivalReward`（`apis/signin.ts:46`）→ `result.{rewardClouds,totalClouds}`。
- **月限**：**硬编码 `maxCount = 2`**，计数存在**本地文件** `.mcloud/revival.json`，结构 `{ "YYYY-M": { "<phone>": count } }`（`tasks/sign/utils.ts:4-27`）。
- **判据**：`count >= 2` 跳过；请求失败且 `err.message.includes("本月已领取")` ⇒ 视为正常，**把本地计数直接写成 2**；其他错误只打日志、不计数；成功则 `count+1` 并落盘。
- **四处坑（重写必修）**：
  1. 计数是**本地**的，换机器/换目录即失效 ⇒ 新项目落 SQLite（`data/` 目录相对 `process.cwd()`，`utils.ts:4`，同一目录下所有状态都有这个毛病）；
  2. 判据是**中文错误消息子串匹配**，上游文案一改即失效；
  3. `getCurrentMonth()` 用**本地时区**（`utils.ts:29-32`），而调度按 `Asia/Shanghai` ⇒ 跨时区账号月界不一致；
  4. `loadRevivalStorage` 的 `catch {}` 吞掉解析错误并静默返回 `{}`，坏文件 = 静默失忆。
- **补充（2026-09-19 用户订正）**：上游真实月限是 **1 次**；上面的 `maxCount = 2` 不是"上游给两次"，而是
  **防错过的保险** —— 因为**旧项目从未抓到"是否还能复活"的判活接口**（只能盲打 + 靠文案判"本月已领取"），
  判据太紧就会整月空跑，所以本地宁多放一次。⇒ 新项目**可以尝试去抓这个判活接口**；抓到后判据应从
  "盲打 + 文案"改为"先查询再决定"，届时 2 这个保险值才有资格退役（当前**不要**按 1 收紧，那等于用一个
  没验证过的假设换掉一条实测过的保险）。
- **新落点**：能力 `revival` + `runs` 维度记录月度计数；判据改为业务码而非文案（依赖 §5.5 的实测取码）。

#### `auth-refresh` 凭证刷新（`packages/api/src/tasks/auth-refresh.ts`）

**这个任务在旧项目里实际不生效。** 它调 `refreshAuthToken(ctx)`（`packages/core/src/tasks/auth-service.ts:61-75`），而该函数只是 `authTokenRefresh` + 正则取 `<token>` 后**把新 token 当返回值交出去**；任务层 `await refreshAuthToken(ctx); return { ok: true }` —— **返回值直接丢弃，既不落配置也不改 ctx**。真正的刷新发生在 `after`（下一条）。

同名字段另有 `user-njs.yun.139.com/user/auth/refreshToken`（`apis/auth.ts:153`）与 `note.mcloud.139.com/noteServer/api/authTokenRefresh.do`（`:175`）两个不同体系的"刷新"接口 —— 全仓还有 `refreshAuth / checkAndRefreshAuth / refreshJwtToken` 三个**无人调用**的封装（死代码）。⇒ 新项目应把"续期"收敛成**一处**（`internal/app/session.go` 的生命周期，方案 §3.7），并按端区分主站 / 笔记 / 邮箱三套 token。

#### `after` 收尾（`packages/core/src/tasks/after.ts`）

三件事，风险递增：

1. **打印今日云朵**（`printTodayCloud`，`:29-56`）：`GET https://mrp.mcloud.139.com/market/signin/public/cloudRecord?type=1&pageNumber=1&pageSize=100`，按 `inserttime` 本地日期过滤 + `num > 0` 求和。**受配置 `是否打印今日云朵` 门控**。这就是 `cloud-snapshot` 的上游雏形。
2. **维护 auth**（`refreshAuthToken`，`:66-117`）：`parseAuth` 拿 `expire`，`isNeedRefresh(expire, 剩余多少天刷新token ?? DEFAULT_REFRESH_DAYS=7)` ⇒ 命中则 `authTokenRefresh.do`（`aas.caiyun.feixin.10086.cn/tellin/authTokenRefresh.do`，`apis/auth.ts:118`；**请求体是 XML 但 content-type 写 json**，响应正则取 `<token>`）⇒ 更新 `ctx.config.{auth,token,expire}` + `ctx.http.setAuth` ⇒ `updateAccountAuth(configPath, accountIndex, newAuth)` **裸 `writeFile` 落盘**（无原子替换）。
   - 两处相反语义要留意：`auth-service.ts` 那份对 `expire===0` 判"恒需刷新"，`after.ts` 这份在 `!authInfo.expire` 时**提前 return**；写回失败只 `logger.fail("请手动更新 auth")`。
   - **补充（2026-09-19 用户订正）**：`expire` **小于 0**（无有效过期时间）时**刷新不了，只能手动更新 auth** ——
     这不是"恒需刷新"能救的分支。⇒ 新项目把这种值判成 `ManualRequired` 并给出明确的人工出口，
     不要自动重试刷新（重试只会白白打 `authTokenRefresh.do`），也不要照抄 `auth-service.ts` 那半边的"恒需刷新"。
3. **删除临时文件**（`afterTask` 末段 + `utils/file.ts:113-125`）：`ctx.state.files` 非空 ⇒ `POST https://personal-kd-njs.yun.139.com/hcy/recyclebin/batchTrash`（`apis/file.ts:120`）。**这是对用户网盘的真实删除动作**（进回收站），且 `files` 是跨任务累积的共享状态 —— 一旦某能力忘了清理或错记了 ID，`after` 就会删掉不该删的东西。⇒ 新项目必须把"谁创建的文件"归属到 `run`，`after` 只清理本 run 名下且经复查确认的 ID。
   - **补充（2026-09-19 用户订正）**：上一条里"归属到 run"的**确切含义要收窄**，别读成上游能力：上游**没有**"这个文件是谁创建的"这种归属信息可查，
     最多只能在**上传时给文件名加前缀**做人为标记。同时 `batchTrash` 用的是 **`fileId`（唯一）** ⇒ 按 ID 删**不存在错删别人的文件**，
     真正的风险只有**漏记**（没进 `state.files` 的 ID = 云盘侧孤儿文件残留）。所以"归属到 run"落地成
     **我们自己的 run 产物清单 + 失败路径也清理**，不是"向上游查归属"。
     （真正会"错删"的是 §2.3 `ai-avatar` 那种**按文件名匹配**找回 ID 的写法 —— 同名即碰撞，那才是要改成一跳拿 ID 的地方。）

### 2.3 档 3 · 活动类：`task-expansion` / `app` / `ai-avatar` / `msg-push` / `notice` / `backup-gift`

#### `task-expansion` 任务拓展（`packages/core/src/tasks/task-expansion.ts:26+`）

**本档唯一会在用户网盘留下真实文件的常规能力**，流程：

1. `GET https://mrp.mcloud.139.com/market/signin/page/taskExpansion`（`apis/signin.ts:56`）取任务与 `acceptDate`；
2. **建随机文件凑备份**：`POST personal-kd-njs.yun.139.com/hcy/file/create`（`apis/file.ts:64`，入参含 `contentHash`+`SHA256`、`fileRenameMode:"auto_rename"`，返回 `{fileId, uploadId, rapidUpload, partInfos[].uploadUrl, exist}`）→ `PUT <uploadUrl>` → `POST hcy/file/complete`（`:96`）。**"像备份来的"靠的是安卓渠道那组头**：`opType==="backup"` ⇒ `x-yun-op-type: 2` + `x-yun-sub-op-type: 200`（`file.ts:32-41`，`sub = op*100`；`channelSrc` 默认 `10000023`），而 `upload` / PC / Web 分支拿到的是 `1`。
3. `sleep(backupWaitTime || 20)` 秒，等备份被识别；
4. 复查 + `GET https://mrp.mcloud.139.com/market/signin/page/receiveTaskExpansion?acceptDate=${acceptDate}`（`apis/signin.ts:65`，**未 percent-encode**，且我们抓包样本里**没有出现 `acceptDate` 参数** —— §5.5）；
5. 留下的文件 ID 写进 `ctx.state.files`，由 `after` 删除。

> ⚠ 副作用分级：`file/create + complete` 是**写用户网盘**。任一步失败都可能留下孤儿文件（`after` 只删 `state.files` 里记着的）。新实现必须有"本 run 创建了什么"的事务式清单（`mcloud-exec` 的 run 维度产物），并在失败路径也执行清理。
>
> 2026-09-22 补：这一条**在 Go 侧仍是缺口** —— `internal/exec` 当前只有并发闸
> （`Concurrency`）、互斥键（`MutexKey` + `ParamsDigest`）、准入（`RunRegistry`）与事件总线
> （`EventBus`），**没有** "run 内产物清单" 这一层，也没有失败路径的补偿钩子。

#### `app` App 任务矩阵（`packages/core/src/tasks/app/`）—— 体量最大的一项

不是"一个任务"，而是**一个读取任务列表 + 按任务码分发**的执行器：

- 列表：`GET https://m.mcloud.139.com/ycloud/signin/task/taskListV2`（`tasks/app/helpers.ts:50`）与 `GET https://caiyun.feixin.10086.cn:7071/ycloud/signin/task/taskListV2`（`apis/task.ts:39`）两处封装，**8 组 marketName × 2 = 16 次**串行拉取；点击上报 `…/ycloud/signin/task/click`（`helpers.ts:10`）与 `:7071/…/taskClickV2`（`runners.ts:223`）；通用完成 `GET https://mrp.mcloud.139.com/market/signin/page/doTask?client=app&deviceId=${encodeURIComponent(deviceId)}`（`runners.ts:103,125`）。
- **任务码分发表**（每个码是一种独立玩法，各带专属实现）：`106` 上传文件、`107` 云笔记、`434` 建外链后立即删、`113` PC `authTokenRefresh`、`409/1005` `page/doTask?client=app&deviceId=`、`522` 循环上传、`472/473/1004/1014/1020/1028` 走 **139 邮箱**链路。
- **139 邮箱子链**（`runners.ts:193,287` + `apis/mail139.ts`）：`querySpecTokenV2(toSourceId 001003)` → `mail.10086.cn/login/inlogin.action`（**XML 请求**）→ 缓存 `sid/rmkey` → `RmWeb/mail`（XML，判 `S_OK`）→ PE/gzip 日志上报 → `disk:convertPdfFromYunFile` → DES 解 account + RSA 解手机号 → `getUserTaskAndCloudStatusExternal` → AI 对话 SSE。这是全项目最长的一条依赖链，跨 4 个域。
- **`SKIP_TASK_LIST`**：`[117,437,991,406,385,480,1022,1018,603,1021,120]`（明确不做）。
- **结果四象限**：完成 / 失败 / 无需做 / **需前往云盘或邮箱完成**（§1.4）。
- **实测 bug**：`107`（云笔记）**恒跳过** —— 该 runner 读的是 `http` 返回值形状，与 `apis/note.ts` 实际返回不符，条件永远不成立。重写时先补一条"107 能被选中"的断言再动逻辑。
- **新落点**：`app` 不该是一个能力，应拆成 `task-list`（查询）+ 每种玩法一个能力（`upload-file` / `create-note` / `create-and-delete-link` / `mail-read` …），由**组合流水线**（§3.3）拼回旧行为。这样每种玩法各自可测、可控风险、可单独禁用。

#### `ai-avatar` AI 头像（`packages/core/src/tasks/ai-avatar/`）

链路：本地生成 **200×200 随机噪点 PNG**（`utils/png-generator.ts`，base64 去掉 data 前缀）→ `rabactive/ai/image/merge` 得 `taskId` → 轮询 `getStatus`（**最多 8 次 × 3s**；`code!==0` 立即放弃、`status===4` 失败、`status!==3` 继续等、`3` ⇒ 下载 `imageUrl` 后 `image/upload`，**上传时同时带原噪点图与合成图**）→ `hcy/file/list` 按**文件名**匹配出 fileId → `getHashLeft` 查剩余抽奖次数（取不到则兜底 `MAX_DRAW_FALLBACK = 5` 次；`0` 直接结束）→ 循环 `draw`（`code===408` break、`code!==0` continue）。生成阶段每轮**最多重试 2 次**，失败的轮次不入 `uploadIds`。

- **副作用**：向用户网盘**上传**头像图，随后靠"文件名匹配"找回它 —— 同名碰撞即错删/错用（`file/create` 本身返回 `fileId`，旧代码这里没用它）。改成"上传响应里拿 ID"。
- **参数**：`AI新头像.{开启=false, 每日生成次数}` 有三份不一致的默认值（schema 10 / 代码 5 / 文档 5 且标 1-20），§5.4；`开启` 这个门同样只在 CLI 生效。

#### `msg-push` / `notice` / `backup-gift`（同构：查状态 → 领）

| 能力 | 查 | 做 / 领 | 判据与人工点 |
| --- | --- | --- | --- |
| `msg-push` 消息推送 | `GET caiyun.feixin.10086.cn/market/msgPushOn/task/status`（`apis/task.ts:14`）→ `result.{pushOn,onDuaration,firstTaskStatus,secondTaskStatus}` | **仅当 `secondTaskStatus === 2`** ⇒ `…/msgPushOn/task/obtain`，body `{type:2}`（`:18`），成功判据是 `result.obtainCode === 0`（`tasks/msg-push.ts:32-40`） | `pushOn===0`、`firstTaskStatus!==3` ⇒ 需人工（用户得先在 App 开推送）。上游字段拼错为 `onDuaration`（`types/task.ts:10`），反序列化**必须按错的键名**收 |
| `notice` 公告 | `…:7071/ycloud/openemailsms-service/openEmailsms/getTaskInfo`（`:27`） | `…/openEmailsms/reward`（`:33`）+ `taskListV2(clientVersion 11.2.1)` 找 id `1021` + `GET …:7071/ycloud/signin/page/reportAppNoticeStatus?status=1`（`:50`） | `未开启` ⇒ 需人工 |
| `backup-gift` 备份礼 | `GET caiyun.feixin.10086.cn/market/backupgift/info`（`:57`）→ `result.{state,curMonth}`；**`!curMonth` 直接"无可领"返回** | 仅 `state === 0` ⇒ `…/market/backupgift/receive`（`:62`，**GET 却是写操作**），成功时 `result` 就是**云朵数（数字）** | `state`：`-1` 未开启备份 ⇒ `warn("请前往 APP 手动开启")`、`1` 本月已领取、`0` 去领、**其他 ⇒ `未知状态` 只打日志**（`tasks/backup-gift.ts:27-44`） |

⇒ 这三个适合做**新能力的样板**：一条查询 + 一条动作 + 三态判据，正好用来验证 `FieldSpec` / `CapabilityKind` / `RiskLevel` 这套声明式形态跑不跑得通。

### 2.4 档 4 · 复杂活动：`hc1t` / `red-flower` / `play-ai-special` / `play-ai-lottery` / `cloud-phone-redpack`

#### `hc1t` 云朵大作战（合成 1T）

> **补充（2026-09-19 用户订正）：该任务已过期** —— 活动期次已结束，上游入口不再可用。
> 下面整段因此**不作为待实现能力**排进实现顺序，只保留两点价值：① 它是"换端建独立会话 + 每局十余条埋点"
> 这类**活动站通用形态**的最完整样本（`querySpecToken → tyrzLogin → 专用 HTTP 实例`，本节下面的
> `play-ai-special` / `cloud-phone-redpack` 走同一套骨架）；② `x-signature` 的拼法与"签名不绑请求体"
> 这个结论对**其他仍然活着的**活动接口同样成立。要动这一族之前，先按 §5 实测确认当前哪个期次是活的，
> 别照这一段去补一个已经关张的入口。

- **换端建会话**：`GET …/ycloud/api/cloud/userdomain/v2/querySpecToken?targetSourceId=001005`（带 `x-signature` 四头，saltKey=2）→ `POST …/ycloud/auth-service/auth/tyrzLogin`（`marketName:"hecheng1T"`, `sourceId:"1169"`, `type:"signupPlatform"`）→ **新建独立 HTTP 实例**，referer `https://caiyun.feixin.10086.cn:7071/portal/synthesisonet/index.html`，`setAuth/clearAuth/setJwtToken` 在这个实例上是 **no-op**（`packages/shared/src/http/base.ts:236-238`）。
- **状态**：`GET caiyun.feixin.10086.cn/market/signin/hecheng1T/info` → `result.info{invite,exchange,succ,lastSucc,curr}` + `result.history`（**对象**，`"0"` 本月、`"-1"` 上月；rank 可能是 `"1000+"`）。
- **每局**：6 条埋点 `POST …/ycloud/visitlog/journaling?...&sourceid=1005&marketName=hecheng1T`（`synthesisonet_pv / _cookie / _cookie_notApp / _inviterUserPlayGame / _playGame / _playGame_isOts`，每条之间 `sleep(randomNumber(50,200))`）→ `POST …/portal/ajax/tools/opRequest.action {op:"currentTimeMillis"}` 取**服务器时间** → `GET hecheng1T/beinvite?inviter=<手机号>` 开局 → `ceil(游戏时间/3)` 次 `optkeyword=synthesisonet_game_tap` 埋点（每次 `sleep(3*randomNumber(990,1010))` ⇒ 挂机时长 ≈ 配置秒数）→ `_finish_gameSuc` → 再取服务器时间 → `GET hecheng1T/finish?flag=true` → 复查 info。
- **兑换**：`GET hecheng1T/exchange?num=<time>`，条件 `time>0 && rank >= 目标排名 && 开启兑换`（**判定顺序**：先看排名再看开关 ⇒ 排名够好时连"未开启"都不打印）。
- **签名**：`x-signature = md5(salt + id + ts + nonce + other + salt)`，`id/nonce` 每次随机 ⇒ **签名不绑定请求体**，无防重放价值；`beinvite/finish` 的 `timestamp` 形参在函数体内**未被使用**，只通过 header 生效（`tasks/hc1t/api.ts:22-50`）。
- **风险要点**：`inviter` 是**别人/自己的手机号**，配置允许写 `UserContext<base64>` 形态并解码后**直接字符串拼接进 URL，未 encode**（`api.ts:29`）⇒ 新项目要用类型化参数 + 编码；`exchange` 会在服务端建立"被邀请"关系。埋点总量（每局 9 + N≈100 条）是本能力的主要风控暴露面。
- **参数（旧项目全在账号配置里，键名即中文）**：`云朵大作战.{开启=false, 邀请用户=[], 游戏时间=300, 目标排名=500, 开启兑换=false}`（`packages/shared/src/config/schema.ts:13-21`）。

#### `red-flower` 口令抽小红花

六个请求，顺序固定（`packages/core/src/tasks/red-flower/standalone.ts:23-96`，base `https://m.mcloud.139.com/ycloud`）：

| # | 请求 | 判据 |
| --- | --- | --- |
| 1 | `POST liveRoomFeedback/user/info` | `code!==0` ⇒ **整账号放弃**（返回 `{0,[]}`） |
| 2 | `POST visitlog/journaling` × 5（`National_LiveRoomFeedback_{pv,client,client_isApp,login,flowerBtn}`，`sourceid=1007`，`marketName=National_LiveRoomFeedback`） | 串行、无并发 |
| 3 | `POST liveRoomFeedback/firstParticipate/give` | `0` 成功；`200110` = 无首次资格（静默） |
| 4 | `POST liveRoomFeedback/redFlower/draw?code=<encode>` × N | `0`+result 成功；`200112` **已兑换（不当无效、不从共享缓存剔除）**；其他数字 ⇒ **无效口令**；响应无法解析 ⇒ 只 warn、**不判无效以免误伤** |
| 5 | `GET liveRoomFeedback/redFlower/queryRevivalFlowers` | 只读余额 |
| 6 | 再 `POST user/info` | 终值核对 |

- **头**：`activityid: sign_in_3`、`appversion: 12.6.0.0`、referer `…/portal/yunClound/index.html?path=National_LiveRoomFeedback`、iPad UA。
- **⚠ 上下文可变状态**：调用前 `ctx.http.clearAuth()`（`packages/api/src/tasks/red-flower.ts:19`），**该入口没有 `restoreAuth`**（`kouling` 那条链用 `finally` 恢复了）；因每次 run 新建 ctx 才没外溢。新项目 HTTP 客户端**不应持有可变的凭据**（方案 §3.2 的"凭据随请求传入"）。
- **真实 bug**：`:20-21` 把 `DrawRedFlowerResult`（`{total,invalidCodes}`）当数字赋给 `gained`。
- **参数**：`codes: string[]`；缺省则自动走 `kouling` 的抓取链（§2.5），**跨账号进程内缓存 5h**（`live-room-kouling.ts:454-455`）。

#### `play-ai-special` 拍拍系列 / `play-ai-lottery` 玩 AI 抽奖

- **`play-ai-special`**：活动站会话（`querySpecToken` → `tyrzLogin(marketName shake_shake, sourceId 1314)`）+ **专用 HTTP 客户端**（同样 `setAuth` 空实现）+ 十余条 `visitlog/journaling` 埋点 + `newyear/task/step/click` + `ai.yun.139.com/api/image/aiRecognize` 与 `chat/v2/add`（**SSE**）+ `blindbox/lottery`。
  - ⚠ 三处设备画像互相矛盾：iPad UA、`x-DeviceInfo` 写 iPhone、另一套 client-info —— 真机上到底哪个生效必须实测（§5.6）。
  - 春日/夏日/秋日拍拍是**同一实现的不同期次**，不是三个能力（方案 §1.2 注）。
- **`play-ai-lottery`**：`POST auth-service/auth/getAccount {marketName:"National_playAI", sourceId:"1108"}` → `funai/api/page/getmodule`（读 `drawTimes`）→ `page/lottery`（**只抽一次**，不看剩余次数循环）。
  - ⚠ 用 `err.message.includes("500")` 判"次数用完" ⇒ 真故障会被误吞成正常结束。新实现要有明确业务码。

#### `cloud-phone-redpack` 红包派对（**产出是钱**）

- **产出**：`userAccountInfo.{canAmount,totalAmount}`，单位**分**（代码 `/100` 显示元）。
- **统一信封**：`POST https://cpactiv.buy.139.com/cloudphone-market/<path>`，body `{version:"SYS_CONFIG_Y", ...data}`；判据 `String(header.status)==="200"`；错误文案优先级 `data.errorMsg → header.errMsg → header.respMsg`。
- **鉴权头每次重算**：`sign = md5(requestId + APP_ID + token + SIGN_SALT + timestamp)`，`requestId = YYYYMMDDHHmmss + Date.now() + random36(8)`（**不是 UUID**），`x-DeviceInfo` 是一串拼出来的指纹（iPad UA + `<ts><random>` + `375X667`）。`APP_ID="12345681"`、`CHANNEL_SRC="red-mcloudapp-001"`、`SIGN_SALT` 是应用级常量（`md5("123456")`），`VERSION="SYS_CONFIG_Y"`。
- **登录**：`querySpecToken?targetSourceId=001216`（签名 saltKey=2）→ `POST /user/tokenValidate {pintype:13, token, deviceId, loginConfig:"redpacketLoginConfig"}` → 活动站 token 存 `ctx.state.cloudPhoneRedpackToken`。**首行 `if (getToken(ctx)) return true`** ⇒ 持久化的 token **永不校验有效期**，过期后全线失败且只能手删 state（新项目要把活动站 token 纳入会话生命周期，方案 §3.7）。
- **主流程**：`configTaskLoginList`（四张列表）+ `userAccountInfo`（前后各一次）→ `userSign` 签到 + `userToastInfo` 播报金额 → 逐任务 `handleTask`：`userCompleteTask` → 按 `taskCode` 分发：
  - `BROWSE_TASKS{NOVICE_2,NOVICE_3,MONTHLY_1}`：`sleep(15000)` 后 `userBrowse {taskCode}`；
  - 安装类：`thirdQueryUserRelationshipV2`（`ACTIVE`+`PENDING` 两查聚合，兼容 `data.data` 双层嵌套）列云机 → 锁屏 `lockStatus==="Y"` 优先跳过 → `configAppList` → `gethwToken` → `instanceApkInstallStatus`（已装则 `userInstallApp{existed:1}`）→ **`apkInstall` 真往云机装 APK** → `appInstallTaskStatus` 轮询 ≤60×1s → `userInstallApp`；
  - 答题类：`configTopicList` 取 `[0]`，查**内置 15 条 FAQ 题库**（`KNOWN_ANSWERS`，改版即静默失效）→ `userTopicAnswer {answer:"A"|"B"|"C"|"D"}`（最多重试 15×150ms 直到题目在库里）；
  - `DAILY_1`：`getRcsToken` 返回值丢弃 + `info("需手动完成 … (需云机内实际使用)")` + **`return true`**（假成功，§1.4）；
  - 未知码 ⇒ `info("需手动完成: <name> (<reason>)")`。
- **状态码语义**（`statusText`，`:85-91`）：`0` 未完成 / `1` 已完成跳过 / `3` 奖品已兑完跳过 / `4` 明天再来跳过 / **`2` 未定义但会进入执行分支**；`completeTask` 的失败判据是 `!isNil(data.data?.status) && status!==1` ⇒ **status 缺省视为成功**（宽松）。
- ⚠ **先记账后动作**：`installApp/completeCloudUse/answerTopic` 都先 `userCompleteTask` 再执行实际动作 ⇒ 后续失败时任务已标记完成。新实现要么先做后记，要么把"登记完成"当作显式的最后一步。

### 2.5 档 5 · 最高风险：`kouling` / `exchange`

#### `kouling` 直播口令（WS + 小红书双源）

门（`hasActiveLiveRoom`）：`user-njs.yun.139.com/user/querySpecTokenV2 {toSourceId:"017027"}` → `v.cmcc-cs.cn/cmlive/api/user/auth/cmpassLoginByCmToken` → `GET …/api/channel/channel/tabContentList?tabId=127&contentType=0&pageNo=1&channelId=134` → 过滤 `room.status===1`（`1=直播中, 6=已结束`）且 **`anchor === "中国移动云盘"` 精确相等**（主播改名即失效）。
**SSO/登录失败时 `return true` 放行**（`:421-422`，"避免因临时故障漏掉口令"）；但真的拉到列表且**目标房间数 0** ⇒ `return false` ⇒ **整个任务跳过**（不抓、不领）。

双源 `Promise.all`，单源失败各自吞成 `[]`：

- **源 A · WebSocket**：重跑一遍门那三个请求（**未复用**）→ 每房间 `getCurrentUserInfoNew` → `POST api/user/customer/userToken`（→ `access_token`，约 66 字符 `A03` 前缀）→ `POST api/room/front/roomBeat {roomId, channelId:134, sourceApp:0}`（→ `chatRoomId`，缺失回退 `String(roomId)`）→ `POST https://videoim.cmcc-cs.cn:20103/v3/room/member/apply {roomId:chatRoomId, userName/invoker:"cmlive_c_<phone>"}`，头带 `timestamp`（完整 ms 串）、`deviceinfo`（40B 的 hex 形态）、`appkey: cmos/ngvlcs`、`authorization: Bearer <token>`、`version: v23.0.2`，再加两个签名头（`apis/live-room-chat.ts:96-116`）：**`systemmd5 = md5(deviceInfoBytes ‖ utf8(ts 末 10 位))`**、**`tokenmd5 = md5("Bearer " + token[0:10] + ts 末 10 位 + token[10:])`** → **`wss://videoim.cmcc-cs.cn:20102/ws/<serverId(0-999)>/<sessionId base36-13>/websocket`**（`User-Agent` Android Chrome / `Origin: https://v.cmcc-cs.cn`）。
  - **SockJS 文本帧**：`"o"` ⇒ 发 PROVISION；`"h"` ⇒ **回 `"h"`**（被动心跳）；非 `a["` 前缀 ⇒ debug 忽略；数据帧 `a["<base64 MSync>"]`。
  - **MSync**（`packages/core/src/live-room/proto.ts`，proto3 **内联字符串、无 .proto 文件、无 codegen**）：`version=MSYNC_V1`、`guid=JID{appKey:"cmos#ngvlcs", name:"cmlive_c_<phone>", domain, clientResource:"js_<ts>"}`、`auth="$t$"+accessToken`、`crypto=0`、`userAgent` iPad、`command`、`deviceId=0`、`encryptType=[0]`、`timestamp` 8B 大端、`identification` 随机 16B、`deviceInfo` 固定 40B hex（应用级常量）；PROVISION 的 payload = `Provision{osType:7 OS_JS, version:"23.0.2", networkType:1 WIFI, appSign:"cmlive_cmos_js_sdk", compressType:[0], encryptType:[0], deviceUuid:String(Date.now()), resource, auth:"$t$"+token, deviceName:"webim", province_code:""}`；发送形态 `["<base64>"]`。
  - **关键时序**：`command===3`（PROVISION 响应）后**必须再发 UNREAD(1)**，服务器才开始推弹幕（`:176` 注释）。之后只处理 `SYNC(0)` / `NOTICE(2)`：`SYNC → CommSyncUL.meta.payload → MessageBody.contents[0].text`；`NOTICE → NoticePayload.meta.payload → MessageBody`；**每层解不开就直接把 payload 当 `MessageBody` 再解一次**（双兜底），再失败则 debug 打印前 64 字节 hex。
  - 口令解析（`live-room/kouling.ts`）：文件头注释给了 **5 个样例形态**，但实现是 **4 个分支、按顺序累加**（`:31-82`）—— ① `小红花口令[：:]\s*(.+)`；② 按行扫：`(?:复制)?口令\s*\d*\s*[：:]$` 结尾的行 ⇒ 取**下一行**（前提该行不以 `口令` 开头、不含 `无随机`），**同一行内的多个口令**再用带前瞻的正则切分（前瞻到下一个 `口令N：` / `无随机` / 行尾），允许前面带"今日口令更新～"这类描述；③ `口令是(.+)`；④ **纯数字编号 `1:X 2:Y 无随机`** —— 仅当①②③**一个都没命中且文本含"无随机"**时兜底。最后 `[...new Set]` 去重 + `isQuestion` 过滤。**跨帧拼接**：整条弹幕以 `isKoulingHead` 结尾 ⇒ 把**下一帧整条 trim** 当口令（前提不含 `口令`/`无随机`）。**提问过滤**（`isQuestion`）：含 `?`/`？` ⇒ 丢弃；去掉句尾语气词（`呀啊吧呢嘛哦`）后整条等于 `什么/啥/多少/几/哪个/哪些` 之一 ⇒ 丢弃。旧项目 `__tests__` 里 **8 条口令单测**可直接当黄金用例。`timeout=0` ⇒ 收到一个即返回；上限硬收敛到 **180000ms**。
- **源 B · 小红书**：`GET https://www.xiaohongshu.com/user/profile/<24 位 hex>`（**硬编码主播主页，无凭据**，UA/referer 伪装桌面 Chrome）→ 四级候选抽取（内嵌 JSON `displayTitle|title|desc` → 服务端渲染标签 → `<meta content>` → 全文降级），**按页面顺序第一条命中即返回**（笔记按时间倒序，避免重复投历史口令）。HTML 改版即静默返回 `[]`（只有 debug 日志）。

领取：`clearAuth()` → `drawRedFlowerForAccount`（复用 §2.4 的 6 步判据）→ `finally restoreAuth`（若 `parseAuth` 失败则 ctx 停在无 authorization 状态，**污染同 ctx 的后续任务**）。有 `invalidCodes` ⇒ `restoreAuth` → `getLiveKouling(60000)` 重听一次 → 从共享缓存剔除无效 → **只投新口令**（双向排除）。
共享状态：`sharedKouling = {codes, at}` 进程内、TTL 5h、跨账号复用（首个账号抓到即喂后续账号），`merged 为空则不更新、旧值保留`。
**新落点**：`prost` 需先把内联 `PROTO_DEF` 落成 `.proto`（方案 §3.1 的 `live_room_proto.rs`）；WS 走 `tokio-tungstenite`；`MemberApply` 的两个 md5 与固定 40B deviceInfo 属**可黄金对拍**部分，先写对拍再写链路。

#### `exchange` 抢兑（并发 + 幂等，最难的一项是**正确性**而非协议）

`packages/api/src/tasks/exchange.ts`（3 个参数）+ `packages/core/src/tasks/exchange/task.ts`（下单）+ `utils.ts`（时间窗）+ `packages/api/src/exchange-schedule.ts`（配置→cron 投影）。

- **参数**：`prizeIdOrName`（必填，不校验存在性/类型/库存）、`offset`（默认 10，**非 number 一律回落 10，负数被接受**；语义"整点前提前 N 毫秒"）、`skipWait`（严格 `=== true`，字符串 `"true"` 不生效 ⇒ 仅供测试，误用会在非开抢时刻打空且仍返回 `{ok:true}`）。
- **序列**：（仅当入参是**字符串**时）`GET …/ycloud/signin/page/exchangeList?client=app&clientVersion=12.6.0` 做名称→ID 解析 + `limit` 预检 → `POST …/ycloud/visitlog/journaling`（`newsignin_index_to_exchange`）→ `sleep(500)` → 第二条埋点（`newsignin_index_exchange_times`），`sourceid=1002`、`marketName=sign_in_3` → `POST …/ycloud/auth-service/slide/getSlide {}` → 本地图像匹配算 `puzzleOffset` → **`GET …/ycloud/signin/page/exchangeV2?prizeId=&client=app&clientVersion=&puzzleOffset=&smsCode=`**（GET 下单）→ `code===514` 时重取滑块 + **再下单一次**。
- **判据**：`code===0 && result.prizeName` ⇒ 成功；`code===0` 但缺 `prizeName` ⇒ **记为失败**（隐性 bug）；`514` ⇒ 滑块失败重试；**其他所有码（售罄/库存不足/风控/积分不足/已兑换）一律同一行日志**，无分类。HTTP 层抛错时 `result` 为 `undefined`，走"其他码"分支打出 `code=undefined`，**不抛错 ⇒ runner 记 `ok:true`**。
- **`limit` 预检**（仅字符串入参）：`0/1` 可兑、`2` 本月已兑、`3` 暂不可用、`4` 本月本组已兑 ⇒ 后三者**静默跳过**。库存判据（`count<=0` 已兑完 / `dailyRemainderCount<=0` 今日已兑完）**只在列表展示层推导，兑换路径完全不查**；且需要 `verifycode` 的奖品因 `smsCode` 恒空**必然失败**（失败原因落在"其他码"里看不出来）。风控字段 `riskUserEnable/riskEnable/riskErrorNum/exchangeCode(2031=已兑换)` **全仓无读取点**。
- **时间窗**：`getMsToNextExchangeTime` ⇒ 有效窗口 `hh:59:40.000 ~ (hh+1):00:15.000`（`PREHEAT_SECOND=40`）；`getMsToNextHour(now, offset)` ⇒ `nowMs<20000` 时不再等待，否则等到 `hh+1:00:00 − offset`。组合：cron 触发 → 等 `:59:40` → 预热（2 埋点 + 1 滑块 + 图像匹配）→ 等整点前 10ms → 下单。**若预热越过整点**（大图匹配可能 >20s），目标退到**下一个整点**，期间锁一直被持有。
- **幂等缺口（本能力最该在设计层解决）**：请求里**没有任何幂等键/客户端单号**，`oid` 由服务端生成；514 重试隐含"校验未通过 = 未成交"，若服务端已扣云朵而响应丢包（30s 超时）即**二次成交**；同账号绑两个奖品同一窗口 ⇒ 第二条 schedule 被 `isRunning` **跳过（整个兑换不发生）**；手动 HTTP 触发第二条 ⇒ 拿到的是第一条的 `RunRecord`（**lockKey 不含 prizeId**）。⇒ 新方案要求：锁 key 含参数摘要、`runId` 幂等、写操作零自动重试（§1.3）、514 之后**先查 `prizes` 再决定是否重试**。
- **埋点容错两条路径不一致**：`task.ts:86/88` 裸 `await` ⇒ 埋点失败**中断整个兑换**；`standalone.ts:154/156` 用 `tryRequest` ⇒ 吞掉继续。`doExchange` 与 `executeExchange` 是近乎逐行重复的两份实现（`getSlide` 一处 `tryResponse` 一处 `tryRequest`）。
- **配置双轨**：`exchange` 注册在通用表里但 `hidden`（`GET /api/tasks` 不列，`POST /api/tasks/exchange/run` **不检查 hidden 仍可直跑**）；另有独立轨 `GET /api/exchange/prizes`（**免登录**、走 `ofetch` 绕过项目 HTTP 层）+ `GET/PUT /api/exchange/schedule`。`data/exchange.json` = `{enabled, windows[], bindings[{account,prizeId,prizeName}], scheduleIds[], updatedAt}`；保存语义是**先按 `scheduleIds` 全删再按 (账号 × 窗口) 全建**（`windowToCron("HH:mm:ss") → "ss mm HH * * *"`，时区 `Asia/Shanghai`；`HH:mm` 自动补 `:00`；去重后字典序；空则报错），**用户手工建的 exchange schedule 不归它管**（不在 `scheduleIds` 里）。⚠ 每次保存重写全部 cron ⇒ `createdAt/lastRunAt` 丢失；且因 body 传的是**数字 prizeId**，**定时抢兑路径跳过 limit 预检与库存判据**。
  - **补充（2026-09-19 用户订正）**：`GET /api/exchange/prizes` 那个"**免登录**"**既不是刻意设计也不算缺陷** ——
    本地单机工具，免不免登录在效果上一样，当时只是懒得写。⇒ 新项目把它当**普通待补的鉴权缺口**处理即可
    （按服务端的统一会话鉴权补齐；Go 侧已是这个口径 —— `internal/server` 白名单之外一律要会话），
    不必为它保留例外，也**不必**在上游"接口是否公开"这层继续展开论证。

---

## 3. 业务能力之下的运行时层（缺了它们，19 项一个都跑不起来）

| 层 | 旧项目实际做法 | 新项目落点 / 缺口 |
| --- | --- | --- |
| 执行与并发 | `runner.ts`：`running` Map 的 **join 语义**（第二个请求复用第一个的 promise）、`force` 旁路、历史截断 200 条、`asyncRun` 仅内存上限 100（D4） | `mcloud-exec`：目前**只有** `Concurrency::effective_parallelism` 这一个纯函数。缺 runId、锁（要含参数维度）、`runs`/`run_events` 落库、SSE 事件流 |
| 调度 | `scheduler.ts`：croner，`protect:true`（**跳过不排队**）、5 或 6 段 cron、时区默认 `Asia/Shanghai`、**无错过补偿**、部分失败也记 `error` | Go 侧拆两处：`core.ParseCron` / `core.NextFire`（`internal/core/cron.go`）+ `schedules` 表读写（`SqliteStore` 的 `UpsertSchedule` / `ListSchedules`，`internal/io/configstore.go`，v4 已有归属字段，schema 现为 v8）；**按时触发的循环还没有**（`NextFire` 唯一的非测试消费者是 `routes.go`，只用来显示下次执行时间）。"错过不补"要显式保留还是改，属待拍板项 |
| 组合流水线 | `pipelines.ts`：步骤 ≤30、`stopOnError` 默认 true、**步骤间不传数据**（只共享 `state.json` 与进程全局）、stopped 后仍输出 `skipped` 占位、`ok = ≥1 步执行且全 ok`、账号默认串行防风控 | `mcloud-capabilities` + `mcloud-exec`。旧模型"步骤间不传数据"是 `app` 那种大杂烩的根因；新方案要允许显式的 run 内产物传递（`after` 删文件依赖它） |
| 账号状态存储 | `storage.ts`：`DATA_DIR` **相对 cwd**、读失败静默回落 `{}`、写 tmp+rename、**无锁**；`account.ts`：`accountKey = phone‖nickname‖"account"`、label 掩码 `前2****后2`、`findAccount` 序号**从 1 起**、state 合并方向 `fresh 覆盖 saved`、`writeConfigFile` 带 `.bak` | `internal/io/store.go` 的 `SqliteStore`（SQLite）已完成（schema 现为 v8）；`AccountState` 是**整行覆盖**写，`SmidState` 是**窄写**（两者契约相反，改动时别串台） |
| 推送 10 渠道 | `shared/src/push/index.ts:104-281`：`pushplus`(明文 http) / `serverChan`(token 进 URL) / `workWeixin`(两步取 token，**token 失败仍继续拼 URL**) / `workWeixinBot` / `tgBot`(缺 token/chat_id 主动跳过；`proxy` 配置**从不生效**) / `bark` / `twoIm`(key/sid 平铺进 query) / `dingTalk`(可选 secret 加签；content 加**前导空格**绕关键词；动态 `import("crypto-js")` 失败会冒泡) / `email`(nodemailer，tjs 运行时直接跳过；缺 pass/from/host **静默 return**) / `customPost`(url/body 占位符**字符串替换后 JSON.parse**，可配数组)。所有渠道失败**一律吞成日志**；**只有 CLI 主流程结束才推送，API / 定时路径从不推送** | **Go 侧没有**（Rust 基准同样没有；只有 `core.HmacSHA256Base64` 是给钉钉加签留的原语）（方案 §1.6 定"渠道保留、发送层重做"）。必须补的三件事：**定时路径也要能推送**、失败可见（进 `run_events`）、**正文脱敏** —— 现正文是全量日志，存在手机号/邮箱明文外泄路径（`auth-service.ts:118`、`app/runners.ts:270`） |
| CLI | `packages/cli/src/index.ts`：**只有一个"跑全流程"入口**，任务顺序固定，无参数解析、无交互（D9）；账号配置三级合并在这里才做 | `mcloud-cli`：已有 `user / login / smid / capabilities / version / fixture extract`。缺：按能力跑单任务、组合、调度管理、推送测试 |
| WebUI | 7 页；`WEBUI_PAGES` 白名单只有 6 项（`tasks/pipelines/schedules/accounts/history/config`）**漏了 exchange** ⇒ 停在兑换页时 `normalize` 把 `lastPage` 回落成 `tasks`，"记住上次页面"对该页失效（`api/src/webui-config.ts:16-24,55-62`）；`data/webui.json` 存偏好而非 localStorage；路由在 `index.ts` 平铺注册、**无鉴权、无限流、无全局错误中间件**（D7） | `mcloud-server`：目前只有 `/api/health`、`/api/setup`、`/api/session`（+ 404 兜底）。缺：capabilities / runs / schedules / pipelines / exchange / debug 全部业务路由与 OpenAPI |

> **2026-09-22 逐条核对：这张表的"新项目落点 / 缺口"一列写于迁移之前，好几个"缺"字已经过期**：
>
> - **执行与并发**：`internal/exec` 已不止一个纯函数 —— `core.RunID`、`exec.MutexKey`
>   （键里含 `exec.ParamsDigest`，正是这里点名的"参数维度"）、`exec.RunRegistry.Admit`
>   （join / force 的准入判定）、`exec.EventBus`（SSE 的进程内那一半）；
>   `runs` 与 `run_events` 都已落库（`SqliteStore.InsertRun` / `AppendRunEvents`，
>   schema **v8** 引入后者），`GET /api/runs/{id}/events` 是真接口。
>   ⚠ 仍缺的是**编排接入**与下面那行的按时触发循环。
> - **调度**：那半句"按时触发的循环还没有"**本次复测仍然成立** ——
>   `core.NextFire` 的非测试消费者只有 `internal/server/routes.go`，用途是显示下次执行时间。
> - **组合流水线 / 推送 / WebUI** 三行**未逐条复测**，别当现状读。

---

## 4. 需要集中管理的硬编码常量

旧项目这些值散在 6+ 个文件里，且同一语义有多份（改名/换期时极易漏改）。新项目落 `mcloud-upstream` 的端档案层（方案 `client.rs` / `app_client.rs` 的既有思路），**一处定义、按端取值**：

| 类别 | 值 |
| --- | --- |
| sourceId / targetSourceId | `017027`（直播 SSO）、`001005`（合成云朵）、`001003`（139 邮箱）、`001216`（红包派对）、`1314`（拍拍）、`1108`（玩 AI）、`1169`（hecheng1T tyranny）、`1007`（红花埋点）、`1002`（签到埋点） |
| 直播间定位 | `tabId=127`、`channelId=134`、`anchor="中国移动云盘"`、固定小红书 profile URL（公开主页，形状 `<24 位 hex>`） |
| 端与版本 | `clienttype 656`、`clientType 412/414`、`appversion` 四份并存（`12.6.0.0` / `12.6.0` / `11.2.1` / `23.0.2`）、`x-yun-app-channel` 三值 |
| 活动与市场名 | `activityid sign_in_3`、`marketName {sign_in_3, National_LiveRoomFeedback, National_playAI, hecheng1T, shake_shake}`、`CHANNEL_SRC`（`web 10000034 / pc 10200153 / android 10000023`，另 `red-mcloudapp-001`） |
| 签名 | 两个 saltKey 盐（`1` → hc1t 游戏接口，`2` → `querySpecTokenNew`；`core/src/auth/sign.ts:3-6`）、红包派对 `APP_ID/VERSION/SIGN_SALT`（应用级白盒常量，非账号凭据） |
| 口令解析 | `PROTO_DEF`（proto3 文本）、`DEVICE_INFO`（固定 40B）、appkey `cmos`/`ngvlcs`、`cmlive_cmos_js_sdk`、WS 上限 180s、`sharedKouling` TTL 5h |
| 限额 | 复活月限 2、历史 200、asyncRun 100、流水线步骤 30、`DEFAULT_REFRESH_DAYS 7`（与 schema 的 10 冲突，§5.4）、抢兑窗口 `:59:40`/`+15s`、offset 10ms |
| 已过期不搬 | `signInWx`、`wxDraw`、`shakeTask`（方案 §1.2 注） |

> **补充（2026-09-19 用户订正）**：上表"限额"行里的**复活月限 2 是旧代码的保险值，不是上游额度**（真实 1 次，见 §2.2）；
> "已过期不搬"行同样**遗漏了 `hc1t` 云朵大作战** —— 该活动已过期，应与这三条同列（原行保留不改，读的时候并进来）。
> 也就是说：搬常量清单时，`hecheng1T` 一族（`sourceId 001005/1169`、`marketName hecheng1T`、saltKey `1` 的那把盐）
> 属"除非实测确认仍有活的期次，否则不进实现顺序"。

---

## 5. 动代码前必须实测确认的冲突

这几条是"照着旧 TS 写就会错"的地方，每条都要先在真机/HAR 里落一个结论，再写接口（流程见方案 §7.1）。

| # | 冲突 | 旧代码 | 我们的证据 / 结论 |
| --- | --- | --- | --- |
| **5.1** | **签到域的主机与路径前缀有两套** | `caiyun.feixin.10086.cn/market/signin/…`（`infoV3/multiple/receiveV2/taskListV2`，部分带 `:7071`）、`m.mcloud.139.com/market/signin/page/startSignIn`、`mrp.mcloud.139.com/market/signin/…` | 真实抓包是 **`https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app&deviceId=…`**、**`POST …/ycloud/signin/task/taskListV3`**、**`POST …/ycloud/signin/page/doTaskPost`**（`docs/capture/app-signin-samples.txt:336,368,405`）。⇒ 前缀 `/market/` vs `/ycloud/`、`taskListV2` vs `V3`、`doTask` vs `doTaskPost` **都要按端逐条实测**，不能沿用旧 TS；哪些是"端不同"、哪些是"旧接口已下线"，未确认前不写代码 |
| **5.2** | `deviceid` 请求头的**注入范围** | `base.ts:166-181` 无条件给每个请求带 `deviceid`（值来自 smid 上报） | 我们的 App 签到抓包里原生 `deviceid` **一次都没出现**；`test/fixtures/http/` 711 条中带它的 178 条**全是 H5/WebView**。⇒ 值确认来自 smid，注入点未确认：**不塞进公共头，按端点显式带**（已记 `docs/client-profiles.md` §5.4 ③/§7） |
| **5.3** | `startSignIn` 是否带 `deviceId` 查询参数 | 旧 `apis/signin.ts:24` 只带 `client=app` | 抓包带 `&deviceId=<base64>`。两者形状不同（header vs query）⇒ 需要确认这个 `deviceId` 是不是同一个东西 |
| **5.4** | **配置默认值多份并存** | `AI新头像.每日生成次数`：schema 默认 **10**（`shared/src/config/schema.ts:29`）vs 代码 `DEFAULT_GENERATE_COUNT = 5`（`tasks/ai-avatar/task.ts:7`）vs `docs/CONFIG_SCHEMA.md:70` 写 5 且标范围 1-20；`剩余多少天刷新token`：schema 10 vs `after.ts:8 DEFAULT_REFRESH_DAYS=7`；`docs/CONFIG_SCHEMA.md:74-80` 声称游戏时间 60-600、目标排名 ≥1、邀请用户有手机号正则 `/^1[3-9]\d{9}$/` —— **代码里这些约束一个都没有**（`schema.ts` 只有 `v.optional(v.number())` / `v.array(v.string())`） | 以**代码内 `?? 默认` 为准**逐条实测；文档里那些范围/正则属"写了但没实现"。新方案的 `FieldSpec` 默认值与校验必须是**唯一**来源（D3/D8） |
| **5.5** | `receiveTaskExpansion` 的 `acceptDate` | `apis/signin.ts:65` 字符串拼接、**未 percent-encode** | 抓包样本里**没有** `acceptDate` 参数 ⇒ 该参数是否仍需要、取值来源都要确认 |
| **5.6** | 拍拍系列三套互相矛盾的**设备画像** | iPad UA / `x-DeviceInfo` 写 iPhone / 另一套 client-info 同时发 | 未实测。真机取哪套决定 `play-ai-special` 能不能过，抓包时优先看这块 |
| **5.7** | App 端滑块的自动化边界 | — | **App 自身的滑块未实现自动化**；当前代码里 App 登录过风控是**借 PC 端那套**取题与编码（围魏救赵）。真机走的仍是开 WebView 由 H5 发码那条（`docs/capture/app-sms-login-samples.txt` §2.2 的澄清、`docs/client-profiles.md`）。⇒ 任何"App 滑块已可自动"的表述都是错的 |
| **5.8** | **复活奖励的判活接口从未抓到**（2026-09-19 追加） | 旧项目只有"盲打 `receiveRevivalReward` + 中文文案匹配 `本月已领取` + 本地计数"，硬编码 `maxCount = 2` 当保险（真实月限 **1 次**） | 未抓到 ⇒ 结论只能是"暂时沿用保险式写法"。**新项目可以尝试去抓这个判活接口**（方向：`/ycloud/signin/page/` 与 `market/signin/` 两族里带 `revival` 字样的 `info`/`status`/`query*` 只读口，参考 §2.2 `revival` 与 Go 篇里 `revivalreward` 的查-领同构形态）。抓到前**不要**把 2 改成 1，也**不要**把"文案匹配"当成已验证判据 |
| **5.9** | **活动期次是否还活着，代码里看不出来**（2026-09-19 追加） | 旧 TS 把 `hc1t`（合成 1T）当成常规能力写死；实测口径：**该任务已过期**。同一形态的还有 `signInWx`/`wxDraw`/`shakeTask`（§4"已过期不搬"）与拍拍的春/夏/秋期次 | 抓包前无法区分"接口下线"与"我这端点写错"（两者都表现为非零码）。⇒ 动 §2.4 任何一条活动前，先确认**当期是否仍有活的入口**；已确认过期的（`hc1t`）**不因本表列了它就进实现顺序**，见 §6 第 6 步的补充 |

---

## 6. 建议的 P4 拆解（把上表变成能验收的步）

方案 §7.1 定了能力顺序，这里补"每一项做完算完"的判据：

1. **先把 §1 的四条语义钉成代码**（`FieldSpec` 参数 / 三类错误 / runId 幂等 / `ManualRequired`）—— 否则 19 项会各写一套判据，重演旧项目。
2. **样板三件套**：`backup-gift`（一查一领、三态）→ `prizes`（只读、双层信封）→ `cloud-snapshot`（写 `cloud_stats`，顺手补 D1/D2）。用这三条把 `FieldSpec → WebUI 表单 → API 校验 → 夹具回放 → Run 事件` 全链打通。
3. **签到主线**：`sign` / `receive` / `revival` / `after`（+ `auth-refresh` 收敛进会话生命周期）。**先按 §5.1 实测确认前缀**再写上游。验收要包含"云朵数能从 `cloud_stats` 读回"。
   - 补充（2026-09-19）：`revival` 在这一批里额外带一条抓包任务 —— **判活接口从未抓到**（§5.8），抓不到就保留"2 次保险 + 文案判据"的原写法并显式标注未验证，**不要**顺手收紧成 1；`after` 的续期分支要预留 `expire < 0 ⇒ 只能手动更新 auth` 的人工出口（§2.2）。
4. **写操作护栏**：`task-expansion` / `ai-avatar` 是第一批**会碰用户网盘**的能力，实现前先把"run 名下产物清单 + 失败也清理"做出来，再实现能力本身。
5. **`app` 拆分**：不做成一个能力，拆成 `task-list`（查询）+ 每种玩法一个能力，用流水线拼回；`107` 的"恒跳过"先写断言复现再修。
6. **活动类**：`hc1t` → `red-flower` → `play-ai-special` → `play-ai-lottery` → `cloud-phone-redpack`。前四条依赖 `visitlog/journaling` 与 `querySpecToken/tyrzLogin` 的**活动站会话**抽象（旧项目每个活动各建一个"setAuth 是 no-op"的 HTTP 实例，这块该收敛）。红包派对单列，因为**它产出现金且会装 APK**，需要独立的确认交互。
   - 补充（2026-09-19 用户订正）：这一串里 **`hc1t` 已过期**，实际顺序是 `red-flower` → `play-ai-special` → `play-ai-lottery` → `cloud-phone-redpack`；`hc1t` 只作"活动站会话 + 埋点"的**形态参考**（§2.4 那段保留就是为这个）。原来"前四条依赖活动站会话抽象"改为"后三条依赖"，但那条抽象本身**照做** —— 它是 `red-flower` / 拍拍 / 红包派对共用的那一层。动任一条前先按 §5.9 确认当期是否有活入口。
7. **最后两档**：`kouling`（先 prost 对拍 → 再 WS → 再口令解析，三层各自可测）与 `exchange`（先幂等与窗口设计评审 → 再链路）。
8. **并行推进 §3 的运行时层**：没有 run/事件/SSE 与调度，能力就只能"在 CLI 里手点一次"。
