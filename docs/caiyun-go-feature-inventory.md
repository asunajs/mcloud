# Go 参考项目（caiyun）待实现业务功能盘点

> **定位**：本文是 [`docs/business-feature-inventory.md`](./business-feature-inventory.md)（旧 TS 项目 `mcloud-sign` 盘点）的**对照篇**，
> 对象是方案 §2 里点名的参考项目 [3238614968/caiyun](https://github.com/3238614968/caiyun)（Go + Vue3），
> clone 于 `/tmp/caiyun-ref`，本文所有行号对该 commit 有效：**`1b2cf07`**。
> 体量：`backend/` 337 个 `.go` / 61,215 行；`internal/core/tasks/` 30 个任务文件；`internal/services/exchange_*.go` 25 个。
>
> **它不替代**[`docs/rust-rewrite-plan.md`](./rust-rewrite-plan.md)：方案定"做什么、按什么风险顺序做"（§7.1），
> 本文定"参考实现里每一条到底怎么跑、哪一处不能照抄"。两者的取舍基线是方案 §2.4 ——
> **不采纳 MySQL / Redis / 多用户+RBAC / K8s / 监控栈 / API+Worker 双进程**；
> 本项目形态是**本地优先、单进程、多账号、零外部服务依赖**；多账号保留且强化。
>
> **凭据口径**（与旧 TS 盘点篇一致）：只写**形状**与**来源**，不写任何真实值；
> 应用级白盒常量（签名盐、内置公钥、`appId`、`channelSrc`）注明"非账号凭据"。
>
> **一句话判据**：Go 项目里凡是**代码事实**（某个分支没判、某个 key 没人读）本文直接断言并给 `file:line`；
> 凡是**上游协议事实**（某个 URL、某个 code 的含义）本文一律写成"Go 侧写法"，
> 与我们抓包冲突时以抓包为准 —— 全部收进 §7 那张"动代码前必须实测"表。

---

## 0. 一页总览

### 0.1 三条总结论

1. **业务任务层"能跑但判定粗糙"**。24 条任务里**过半的失败分支只写日志不返回 error**，
   而它的状态机恰好是"日志里出现过 `Error` 才算失败"（§1.4）⇒ 组合效果是**天天绿灯的空转**。
   这是它相对旧 TS 项目**没有改进**的一条（旧 TS 同样全链吞错）。
2. **真正的工程量在抢兑与运行时两层**，而这两层里值得搬的是**判定内核与不变式**，
   不是 Redis/MySQL 的实现：下单响应分支表、错误分类→重试决策表、
   "非幂等 POST 默认不重试"、令牌负缓存 + 连续失败自动停号、
   "崩溃的执行不得回写更晚一次执行的结果"。
3. **参数面是我们要规避的反面案例全集**。它没有 `FieldSpec` 等价物（零参数元数据），
   于是想调"摇一摇次数"只能改 `.env` 重启 worker；默认值散在 **4~6 处**；
   `system_configs` 15 个键里 **12 个是死键**；还有两套近乎不相交的 `.env.example`
   和一整套**零 importer** 的 YAML 配置系统。⇒ 方案 D3「参数、账号定位、执行模式三者分离」被独立验证为必要。

### 0.2 任务目录总表（`backend/internal/services/task_catalog.go:31-341`，共 24 条）

顺序即批次顺序（`SortOrder` 升序，同序按 code 字典序，`:384-389`）。
**批次排尾的两条是刻意的**：`todaycloud`(165) 在所有产云朵任务之后统计、`after_task`(170) 最后清理临时文件 —— 这个"统计收尾 + 资源回收收尾"的语义要搬。

| sort | code | 名称 | 入批 | 副作用 | 判据/实现的主要缺陷（详见 §2） | 判定 |
| --- | --- | --- | --- | --- | --- | --- |
| 10 | `signin` | 每日签到 | ✅ | 写 | 每次跑都重做 28 个上游请求的门户预热；已签到返回 nil 记 success | **搬语义，预热改每账号每批一次** |
| 20 | `task_expansion_reward` | 备份翻倍奖励 | ✅ | 写 + **往网盘上传文件** | 领取步 **code 完全没检查**；单跑不清理 ⇒ 网盘留垃圾 | **先修再搬** |
| 25 | `cloud_multiple` | 云朵翻倍 | ✅ | 写 | "已领取"判定只有一条 `Contains(msg,"已领取")` ⇒ 奖品领完被吞成成功 | **先修再搬** |
| 30 | `wechat` | 微信签到 | ✅ | 读 | — | 搬 |
| 40 | `wxdraw` | 微信抽奖 | ✅ | 写 | `alreadyDrawn := 50 - surplusNumber` 硬编码 50；三处 code 判定漏 `"200103"` | **先修再搬** |
| 50 | `tasklist` | 任务中心巡检 | ✅ | 写 + 上传 + 分享 | 906 行最大；`Run` 恒返回 nil；special-handler 挡在 skip 检查前 ⇒ 8 条内置跳过配置死 7 条 | **搬（含 7 条表驱动用例当行为规格）** |
| 60 | `invitefriends` | 邀请好友看电影 | ✅ | 写 + **向数据中心上报手机号** | 无分享记录照样上报；`calcCRC` 里 `e = e & e` 是空操作 | **搬语义，默认关闭 + 匿名化** |
| 70 | `shake` | 摇一摇 | ✅ | 写 | 结果用裸 `fmt.Printf` 输出 ⇒ **不进 task_logs、不进 WS**（`core/tasks/shake.go:49,51`） | **先修再搬** |
| 80 | `receive` | 领取云朵 | ✅ | 写 | — | 搬 |
| 90 | `messagepush` | 消息推送奖励 | ✅ | 写 | 未开启推送时只 Warn（= 保持绿） | 搬（第四态见 §1.4） |
| 95 | `revivalreward` | 复活卡奖励 | ✅ | 写 | 幂等全靠服务端 + 文案双条件；**月度上限 2 在 Go 版不存在** | 搬语义 + 我们补本地额度 |
| 100 | `backupgift` | 备份礼包 | ✅ | 写 | **最明确的静默失败点**：`receiveCode!=0` 无 else、零日志、返回 nil（`backupgift.go:121-131`） | **先修再搬** |
| 110 | `garden` | 果园 | ❌ | 写 | 目录标"活动已下架"；`initTree` 不校验 code；浇水失败只记日志仍返回 nil | **不搬** |
| 120 | `redpacket` | AI红包 | ❌ | 写 | 已下架；且它是 `aicloud` 的唯一数据生产者（隐式顺序契约） | **不搬** |
| 130 | `aicloud` | AI云朵 | ❌ | 写 | 已下架；依赖 `ai_sessions`，`redpacket` 弃则它无意义 | **不搬** |
| 140 | `cloudbattle` | 云朵大作战（合成 1T） | ✅ | 写 | **签名用空 queryString 计算，实际 URL 带 query**（`cloudbattle.go:356-358`）；取服务器时间失败回退本地钟 | **先修再搬** |
| 150 | `blindbox` | 盲盒 | ❌ | 写 | 已下架；`openBox` 全分支吞掉（函数返回 void）⇒ 奖品拿不到也报成功；弃它可省掉 mail.10086.cn 四跳 SSO | **不搬** |
| 152 | `token_pk` | 算力大作战 | ✅ | 写 + 上传/建笔记/分享 | `openUrl` 只 `Sleep(1s)` 从不访问链接；`rewardStages` 解析出来丢弃；动作失败全 `Debug` ⇒ 几乎恒 success | **先修再搬** |
| 154 | `make_wish` | 许愿赢好礼 | ✅ | 写 | `ENABLE_NOTIFICATION` **只 append 进已完成列表、无真实动作 = 谎报完成**（`makewish.go:104-106`） | **先修再搬** |
| 156 | `fun_ai` | 趣玩AI抽奖 | ✅ | 写 | 6 个中文片段判"没次数" ⇒ 文案一改 error 翻 failed | 搬 |
| 158 | `poster_activity` | 校园海报活动 | ✅ | 写 | **全文件没有 receivePrize 调用** ⇒ SUCCESS 奖励永远领不走；末尾永远 `return nil` ⇒ 不可能失败 | **不搬**（期次绑定） |
| 160 | `cloudphone` | 云手机红包 | ✅ | 写 | **真实 bug：`body := map[string]interface{}{}`，taskID 从未进请求体**（`cloudphone.go:276-280`）⇒ 签到永远打在默认项 | **先修再搬**（除非不需要云手机积分） |
| 165 | `todaycloud` | 今日云朵统计 | ✅ | 读 | **唯一显式写 `CloudGained` 的任务**；用 `time.Local`（其余处一律 `FixedZone("CST",+8)`）；`code:"500"` 判成功 | **搬（它就是我们的 `cloud-snapshot`）** |
| 170 | `after_task` | 收尾清理 | ✅ | 删（网盘文件） | 清理失败则 KV 列表不清空 ⇒ 24h 后信息消失、**孤儿文件永久残留** | **搬语义，载体换表** |

另有 4 条**不是任务**的运行时件：`userid`（每批重解析 userId）、`store`（跨任务 KV）、
`shumei` 设备指纹、`sms` 远端识别 —— 见 §4。

**与旧 TS 盘点的差异一句话**：同义的有 12 条；**Go 独有**的是 `todaycloud`（独立统计任务）、`token_pk`、
`invitefriends`、`userid`、`store` 与整套抢兑子系统；**旧 TS 独有**的是 `kouling`（直播口令，WS + protobuf）与
`prizes` / `play-ai-special` / `ai-avatar` 三条 —— 这四条在 Go 项目里**没有对应实现可参照**。

> **补充（2026-09-19 用户订正，叠加到上面这张表的两行读法）**：
> ① `cloudbattle`（= 旧 TS 的 `hc1t`，同一条合成 1T）**活动已过期** —— 本表"判定"列原写"先修再搬"（签名那条缺陷是真的），
> 但过期后**不进实现顺序**，那张签名缺陷只作为"我们写活动站签名时别重演"的反例保留。
> ② `revivalreward` 的"Go 版没有本地月度上限"这条对照**要连着旧 TS 的 2 一起读**：
> **上游真实月限是 1 次**，旧 TS 的 2 是"判活接口从未抓到"逼出来的保险值（展开见
> `docs/business-feature-inventory.md` §2.2 与 §5.8）。⇒ Go 侧"全靠服务端"反而更接近真实额度，
> 但它没有任何本地兜底，服务端文案一改就翻车（`T-11`）。

---

## 1. 四条公共语义（Go 版把坑挪到了哪里）

### 1.1 参数从哪来：**目录里零参数元数据，参数全在环境变量**

`TaskDefinition{Code, Name, Description, SortOrder, DefaultEnabled, RunInBatch, Aliases, execute}`
（`services/task_catalog.go:13-22`）—— 没有 params、没有 group、没有 kind/risk、没有 output。
`execute` 闭包直接嵌在目录条目里（`:21,41-43`）。

三个后果，与旧 TS 项目**同类但不同形**：

1. **24 条任务里只有 5 条有可调参数**（shake / wxdraw / cloudbattle / backup 重试 / tasklist 跳过表），
   且**全部是进程级 env**（`CAIYUN_TASK_SHAKE_TIMES` 等，见 §5.2），**没有一处能按账号覆盖**；
2. **core 层直接 `os.Getenv`**（12 处，如 `core/tasks/tasklist.go:122`）⇒ 能力层不受注入、不可测；
3. **`Description` 里内嵌 env 提示**（`task_catalog.go:262` 写着 `CAIYUN_MAKEWISH_PRIZE`）——
   等于把参数文档写进任务名的旁边，接口仍不自描述。

⇒ 新落点不变：`mcloud-core::FieldSpec` → `mcloud-capabilities::REGISTRY`，
WebUI 表单 / API 校验 / 组合步骤参数 / CLI `--help` 同源生成（方案 §3.3、D3）。
本文 §5 是这套设计**必要性的证据**，不是要照抄的对象。

### 1.2 判据：信封比旧 TS 统一，但**内联解析有两套且其中一套是错的**

- 统一信封 `CaiyunResponse{Code interface{} …}`；`isSuccessCode` 白名单包含**空串**（`""` 也算成功），
  而 `nil` 判失败（`core/api/caiyun.go:11-14` 及周边）。
- **同一份 code 解析存在正确实现却没被复用**：`parseCaiyunCode`（`core/tasks/tasklist.go:732-747`）
  与内联的 `switch v := resp.Code.(type) { case string: if v == "0" { code = 0 } }` 并存 ——
  后者初值就是 0，于是 **`code:"500"` 判成功**。命中点：`todaycloud.go:41-50`、`backupgift.go:109-119`、
  `wechat.go` 的三处判定。这是**同一个 bug 的三个实例**，不是三件事。
- 域外仍有两套：红包派对域用 `header.status`（无 `code`）；抢兑域用 `msg EqualFold "success"`（§3.2）。
- 鉴权头**按域名自动注入**（`core/http/client.go:429-448`：Basic 全域；`jwttoken`/`jwtToken` 双小大写对
  caiyun.feixin / mrp.mcloud / m.mcloud 注入），且刻意用**小写 `jwttoken`** 绕开 Go 的头规范化。
  ⇒ 这是**端档案（`docs/client-profiles.md`）**该记的形状，不要在每个能力里重拼。

⇒ 我们要收敛成的仍是三类失败：`Transport` / `Biz(code,msg)` / `UnknownEnvelope`。
Go 侧没有 `UnknownEnvelope` 这一类 —— **它把"不认识"直接当成功**（空串、未命中的 string code），
这就是 §2 那张"缺陷"列里一半的成因。

### 1.3 幂等：**队列层有真幂等键，任务与兑换层仍然没有**

比旧 TS（"一个幂等键都没有"）前进了一格，但方向不对齐：

| 层 | 有无幂等 | 证据 |
| --- | --- | --- |
| 队列入队 | ✅ `task:queue:dedupe:<uid>:<accID>:<taskType>`，TTL **2 分钟**；键优先级 **IdempotencyKey > OperationID > 三元组**，且注释写明为何优先 IdempotencyKey（否则一个还活着的 claim 会在 TTL 内压掉合法的 `failed → queued` 重放） | `internal/queue/dedupe.go:11,12,24-26,32-54` |
| API→Worker 命令 | ✅ `operations.idempotency_key` + `uq_operations_user_idempotency` 唯一索引 | 迁移 `014`；`handlers/account_tasks.go:45` |
| 多副本防重 | ✅ `execution_token CHAR(36)` fencing，所有写带 `WHERE execution_token=?` | 迁移 `018`；`repository/exchange_task_execution_repo.go:189-238` |
| 传输层写重试 | ✅ **POST 默认不重试**，必须显式 `WithIdempotencyKey` 才重试并带 `Idempotency-Key` 头 | `core/http/client.go:141-160,492-503,553-557` |
| **日粒度任务** | ⚠️ **只在 HTTP 手动触发路径查"今日已执行"**，cron 日批路径完全不查；且粒度是**整批**不是"每任务类型" | `services/task_service.go:278-303`、`handlers/account_tasks.go:39`、`app/worker/run.go:156-163` |
| **上游写操作** | ❌ 下单是 **GET**（`services/exchange_executor.go:148`）；领云朵/领礼包/合成 1T 的 `beinvite/finish/exchange` 同样 GET —— 与旧 TS **完全相同的病** | §2、§3.1 |

⇒ 值得搬的正是两条**决策**：「非幂等默认不重试」（直接照搬进 reqwest 层）与
「重放不被去重键压掉」（我们的 `runs` 唯一索引必须留 `attempt` 维度）。
"GET 承载写"这条**无解**（上游就是 GET），只能用**客户端单次尝试 + 状态预检**补偿。

### 1.4 "需人工"：**它是用日志级别实现的**

Go 侧没有第四态，但它的机制值得单独理解，因为**它解释了为什么这套系统能天天显示绿**：

- 每个 `TaskRunner` 独占一个 logger（`services/task_service.go:107`，级别 `LevelInfo`）；
- 任务跑完后比对 `logger.Snapshot()` 与 `ErrorsSince`，**只要期间出现过 `Error()` 就把 status 从 success 翻成 failed**；
  `Warn` / `Info` / `Debug` / `Success` **一律不计数**
  （`services/task_execution.go:117-137`、`core/logger/logger.go:73-140,171-194`）。

⇒ 于是所有"这个我做不了，你去 App 里点"都写成 `Warn`，**结果是绿的**；
所有"上游报错但代码没抛"都写成 `Error` 或直接不写，**结果取决于作者是否记得调用**。
`status=="skipped"` 那条路更彻底：**不写 `task_logs`、不发 WS**
（`services/task_service_execute.go:84-86,138-140`）—— 但生产路径 `disabledTasks` 恒传 `nil`
（`task_service_execute.go:60`），**该分支根本不可达**。

⇒ 新项目的落点不变（与旧 TS 盘点篇 §1.4 同一个要求）：
`Run` 结果需要显式第四态 `Success / PartialSuccess / ManualRequired / Failed`，
且 `ManualRequired` **必须落历史、必须可推**。附带两条从 Go 学到的硬约束：
① 第四态**不能靠日志级别推导**（要显式 `RunReporter`）；
② `skipped` 也必须落一条轻量 run 行 —— 否则"今天全是 skipped"会被日幂等判成"今天没跑过"而无限重跑。

---

## 2. 能力逐条

顺序沿用方案 §7.1 的五档，此处只列 **Go 侧的增量信息**（旧 TS 侧的序列见 [`business-feature-inventory.md`](./business-feature-inventory.md) §2，不重复）。

### 2.1 档 1 · 查询型

- **`todaycloud` = 我们的 `cloud-snapshot`**。Go 的实现是
  `GET {MarketURL}/signin/public/cloudRecord?type=1&pageNumber=1&pageSize=300` 后累加"今天且 num>0"，
  并且**它是全项目唯一显式写 `TaskResult.CloudGained` 的任务** —— 与旧 TS（余额只进日志、不进状态）比，
  这一条正好补齐了方案 D1/D2 指出的缺口。**要搬的是"独立统计任务 + 批次排尾"这个形状**。
  已知缺陷：只取首页 300 条（多于 300 静默截断，和旧 TS 的"只读第 1 页 10 条"同病）；`int(num)` 截断小数；
  日界用 `time.Local`（`todaycloud.go:105`）而**其余处一律 `FixedZone("CST",+8)`** ⇒ 部署不设 `TZ` 就统计错一天。
- **`prizes` / 奖品记录在 Go 项目里没有对应实现** —— 只能回到旧 TS 的 `getUserPrizeLogPageV2`，实测确认该接口是否还在。

### 2.2 档 2 · 日常核心

| 能力 | Go 侧要点（相对旧 TS 的增量） | 必须先修的点 |
| --- | --- | --- |
| `signin` | 走**新前缀** `m.mcloud.139.com/ycloud`（旧 TS 用 `caiyun.feixin.139.com/market`）；`prepareSignInCenterSession` 先取数美 deviceId（20s 预算）→ GET 门户 H5 建 JSESSIONID → **6 条 `visitlog/journaling` 埋点**（`newsignin_index_*`）→ `infoV3` → sleep 1s → `startSignIn` → 复查 → 收尾 `infoV3`；`TodaySigned()` 是**双通道或**：`todaySignIn==true` 或 `cal[]` 里有 `{t:true,s:truthy}`（`boolFromAny` 接受 `1/"true"/"yes"`） | 埋点与门户预热在每次 `GetCloudInfo`/`SignIn` 前都重跑 ⇒ **一次签到≈28 个上游请求**；应改"每账号每批次一次" |
| `receive` | 同义；GET 承载写 | — |
| `revivalreward` | 独立成任务（旧 TS 把它当 `sign` 的子步骤）；预热(true) + `POST receiveRevivalReward` body `{}`；"已领取"判定是**双条件** `Contains("复活卡") && Contains("已领取")` | 与 `cloud_multiple` 的**单条件**判定不一致 ⇒ 两代补丁产物；"本月奖励已领取"会判失败。**Go 版没有旧 TS 的本地月度上限 2**，额度全靠服务端 |
| `cloud_multiple` | `GET {MobileMarketURL}/signin/page/multiple`；键名驼峰 `multiple`/`cloudCount` | `isCloudMultipleAlreadyClaimed = Contains(msg,"已领取")` **过宽** ⇒ "奖品已领取完"被吞成成功 |
| `task_expansion_reward` | 壳 33 行恒返回 nil，真身在 `core/tasks/tasklist.go:121-188`；若 `curMonthBackup==false` 且有重试次数 ⇒ **`UploadRandomFile`（`Name:"caiyun-backup-<unix>.txt"`、`ChannelSrc:"10200153"`、`OpType:"backup"`、sha256、auto_rename）→ sleep 20s → 重查** → `GET receiveTaskExpansion?acceptDate=<QueryEscape 上一步响应>`；人工出口是 `Warn("…需要手动开启")` | ① 领取响应 code **完全没检查**；② 单跑该任务**不清理临时文件**（清理只发生在 `TaskListTask.Run` 第 5 步 / `after_task`）⇒ 用户网盘留垃圾；③ 注释自陈上传是"简化实现"（不提交 `createSnapshot/completeSnapshot`）；④ 重试次数与等待走 env（`CAIYUN_TASK_BACKUP_RETRY_COUNT`=1 / `…_WAIT_SECONDS`=20，≤0 强回 20）⇒ 要改成参数注入 |
| `backupgift` | 走**老 `:7071` 域** `GET /backupgift/info` → 仅 `state==0` 才 `GET /backupgift/receive`。`state` 语义：`-1` 需人工 / `1` 已领 / `0` 去领 / 其他 Warn | ① `curMonth==0` 先静默跳过 ⇒ **`curMonth==0 && state==0` 会漏领**；② `receiveCode!=0` 时**无 else、零日志、返回 nil**（`backupgift.go:121-131`，本文档实测复核）⇒ 批次显示成功；③ 写操作用 GET，而 GET 层默认重试 3 次 |
| `tasklist` | 906 行最大。`fetchAllTaskItems`（`sign_in_3`×4 分组 + `newsign_139mail`，分组 `cloudEmail/time/day/month`）→ `loadSkipTaskIDs` → `runAutomaticTasks` → `receiveCompletedTaskRewards`（`status==1` → `POST /signin/task/receiveV2`）→ `checkIncompleteTasks`（日志"未完成：请前往移动云盘手动完成"）→ 清理临时文件。**真实接口**：`doTaskPost`、`GET /signin/task/click?key&id`、`POST /signin/task/receiveTask` | ① `Run` **恒返回 nil**；② `handleSpecialTask`（`:334` 的 switch 覆盖 106/107/110/113/434/522/585 共 7 个 ID）**跑在内置跳过检查之前**（`:241` vs `:245`）⇒ `taskListBuiltinSkipTaskIDs`（`:27-36`，8 条）里**7 条是死配置，只有 1021 生效**；③ `handleShareFileTask` 上传→getOutLink→**立刻 delOutLink**→删文件，POST 三步任一失败留孤儿外链；④ `handleMonthlyUploadTask` 目标 **100** vs `make_wish` 的 `UPLOAD_20` 目标 **20**，同一计数两种口径 |

> **`tasklist_test.go` 的 7 条表驱动用例就是 Rust 侧的行为规格**，建议逐条移植为黄金向量：
> 409→`[task,task2]` / `[task2]`；478→`[randomCloudTask]` / `[task]`；`stepTypeSet`→`[task]`；
> `sign_in_3` 无 click→`nil`；legacy market→`[task]`。

### 2.3 档 3-4 · 玩法与活动

**四条换活动站会话的链路**（Go 侧把它们做成了共享件，这是相对旧 TS 的最大改进）：

| # | 链路 | 用在 |
| --- | --- | --- |
| ① | SSO→`tyrzLogin`：`querySpecToken{toSourceId:"001005"}` → `mail.10086.cn/login/inlogin.action`（XML，`clientId=10804,version=9,loginType=7`）→ `smsrebuild1.mail.10086.cn/setting/s?func=umc:getArtifact`（`Cookie: RMKEY=…`）→ `GET caiyun.feixin.10086.cn/portal/auth/v2/tyrzLogin.action?marketName=&sourceId=` | `blindbox`(1005)、`cloudbattle`(1169) |
| ② | `getTyrzToken.action?sourceId=001216` → `cpactiv…/user/tokenValidate{pintype:13}` | `cloudphone` |
| ③ | `PrepareActivitySession`：GET 活动 H5 门户拿 JSESSIONID，门户映射 `yunClound/1073`、`hcyview/1075`、`huiyuanri/1170`、`yunpanpage/2073` | `fun_ai`、`make_wish`、`poster`、`token_pk` |
| ④ | 只要 mail cookie | `garden` |

**两套 query-string MD5 签名**（与 `legacy/crates/mcloud-core/src/crypto.rs:43` 的 `SIGN_SALT_MAP` 可对照）：
caiyun 域用 saltKey `1`/`2` 两把盐 + 头 `x-request-id/x-timestamp/x-nonce/x-signature`；
cpactiv 域用 `appId=12345681`（应用级白盒常量，非账号凭据）+ 常量盐 + 头 `sign/requestId/appId/token/timestamp`，
且 **token 未到手前用空 token 签名**。

逐条要点（只写 Go 的增量）：

| 能力 | Go 侧要点 | 必须先修 / 不搬的理由 |
| --- | --- | --- |
| `token_pk` | 纯函数规划器 `planTokenPKActions`：`WAIT/ING(去做)→SUCCESS(领奖励)→FINISH(跳过)`；run-key 白名单 `{backup,uploadPhoto,aiCamera,aiAssistant,createNote,shareFile,openUrl}`，未知 ⇒ `manual("暂不支持的自动化步骤")`；`manualReasons` 有 `loginPc:"需要登录PC客户端"`、`inviteFriend:"需要好友真实参与"`；`StepKey()` 平台顺序 `["android","app","other","harmony","ios"]`。序列：`PrepareActivitySession` → `GET /tokenpk/task/list?marketName=&platform=android&sortState=true` → 逐项（`receive`→`step/receivePrize`；`run`→先 `step/click` 再做真实动作；`reserve`→`step/reserve`）→ sleep 3s → 复查再领 → `autoReceiveLotteryChance`(→`rewardCount`) → `queryHome`(→`usedToken`) | ① `rewardStages` 被 `_, _, err :=` 丢弃 ⇒ **阶段奖励读得到领不了**；② `openUrl` 只 `time.Sleep(time.Second)`（`tokenpk.go:214-215`）而 `StepLink()` 注释写着"需要真实访问"却只有单测在用 ⇒ 该类任务**实际永不完成**；③ 动作失败全 `logger.Debug` ⇒ 只有 `failures>0 && claimed==0 && executed==0` 才 error ⇒ **几乎恒 success**；④ `StepKey()` 兜底**遍历 map 顺序随机**；⑤ `TokenPKGenerateInviteCode` 零调用。**规划器形态值得搬**（它是本项目 `ManualRequired` 最好的样例） |
| `make_wish` | `MakeWishHome` → 若 `Wish==nil && Config.CanEarnCodes` ⇒ `PrizeList` + `SelectMakeWishPrize(env)` + `MakeWish`；`MakeWishAutoAdvance` 两段；taskCode 分派 `AI_LINGXI`/`BACKUP_FILE`/`UPLOAD_20`（用 `(\d+)\s*$` 从描述里抠目标数）/`ENABLE_NOTIFICATION`/人工清单（`MOMENTS_9PIC`/`SHARE_ACTIVITY`/`CLOUD_EXCHANGE`，后者只有只读探测 `task/cloud-exchange/info`，因为换量子动作从未被抓到）。`appVersion` 13.0.1 | ① `UPLOAD_20` 从**文案**抠数字 ⇒ 脆；② `ENABLE_NOTIFICATION` 谎报完成（`makewish.go:104-106`）；③ prize 选择走 env ⇒ 改参数 |
| `fun_ai` | `PrepareActivitySession` → `FunaiPageModule`（err 直接 return）+ `FunaiTimedModules`（err 吞）→ 模块分派 chat(`aizhushou/lingxiluxian/zuowenzhushou`) / camera(`aixiangji/paizhaowenai`) / note(`yunbiji`→`createTempNote`) + `FunaiRegister`，每步 2s；抽奖上限 `funaiMaxLotteries=5`、间隔 1s | 6 个中文片段判"无次数" ⇒ 文案一改就把 skipped 变 failed。**最值得搬的活动任务**（与其他三条共享 ③ 号链路，摊薄预热成本） |
| `cloudbattle` | 同 blindbox 的 SSO 骨架（**代码复制未抽公共函数**）；`hecheng1T/info`→`curr` 决定局数；`opRequest.action{op:currentTimeMillis}` 取服务器时间；`beinvite?inviter=` → `rounds=(gameTime+2)/3` 次 tap（每次 `3*RandomInt(990,1010)` ms）→ `finish?flag=true` | ① **签名用空 queryString、URL 带 query**（`cloudbattle.go:356-358`）⇒ 服务端一旦校验即 403；② 取服务器时间失败回退本地钟 ⇒ 时钟漂移会伪装成业务失败 |
| `cloudphone` | `userAccountInfo`/`configTaskList`/`userSign`；`IsToday==1`、`status==0` 已签到跳过、`code==1` 成功 | **taskID 从未进请求体**（`cloudphone.go:276-280`）。旧 TS 侧这条还有"假成功"分支（`return true`），两边都要防 |
| `shake` / `wxdraw` / `wechat` | shake：`POST /market/shake-server/shake/shakeIt?flag=1` 循环，参数 env；wxdraw：`CAIYUN_TASK_WXDRAW_TIMES`=1；wechat：`GET /market/playoffic/followSignInfo?isWx=true` → `drawInfo` → 循环 `GET /market/playoffic/draw` | shake 结果用 `fmt.Printf`（`shake.go:49,51`）**绕过 logger ⇒ 不进历史、不进推送**；另有一个 `:7071` 变体 `api/caiyun_rewards.go:9-29`（同 endpoint 两种 host）；wechat 的 `alreadyDrawn := 50 - surplusNumber` 硬编码 50（`wechat.go:121`）+ 三处 code 判定漏 string 码 |
| `invitefriends` | `GetCloudRecord` 统计本月 `mark=="fxnrplus5"` ⇒ `20-已得`；`POST datacenter.mail.10086.cn/datacenter/`，body `base64(JSON)+ext=crc=`，JSON 内含**真实手机号**或 `"anonymous"` | ① 无分享记录照样上报，且 `getCloudRecord` 不校验 code、出错时**返回 20**（⇒ 配额耗尽后每天仍上报）；② `calcCRC` 的 `e = e & e` 是空操作（注释称模拟 JS 32 位溢出）；③ 首行日志 `"测试中。。。"`；④ **向数据中心上报手机号是隐私代价** ⇒ 搬则默认关闭 + 强制匿名 |
| 已下架四条 | `garden`（`initTree.do` 不校验 code；浇水失败只 Fail 日志仍返回 nil）、`redpacket`（埋点拼串畸形 `marketName=National_LanternRiddlesal_LanternRiddles`；`submitAnswered` 的中文答案未 URL 转义；25 处"与 mjs 一致"注释说明协议是从旧 JS 端口来的，**可信度较高**）、`aicloud`（依赖 `KeyAISessions`）、`blindbox`（`openBox` 返回 void 吞全分支） | **不搬**。注意 `redpacket` 是 `aicloud` 的**唯一**数据生产者 ⇒ 弃前者则后者自动无意义；弃 `blindbox` 可连带去掉 ① 号 mail.10086.cn 四跳 SSO |

### 2.4 档 5 · 抢兑

单独成节，见 §3。

### 2.5 共享动作库（`core/tasks/activity_actions.go`）—— 搬之前先读风险

这些是 `token_pk` / `fun_ai` / `make_wish` / `tasklist` 复用的"真实动作"：

| 动作 | 做法 | 风险 |
| --- | --- | --- |
| `uploadTextFile` | 内容就是 `"0"` | 低；但 `rapidUpload` 命中会短路 ⇒ 同名同内容重复上传不落盘，**可能骗不过任务计数** |
| `uploadPhotoFile` | 样本图；注释"照片类任务要求 `category=image`" | 低 |
| `createTempNote` | `app_auth` 头校验 → `createNote.do`（noteID=`RandomString(32)`、title=`RandomString(3)`）→ 2s → `moveToRecycleBin.do` | **留回收站不物删** ⇒ 长期堆积 |
| `shareNewFile` | getOutLink（`pubType:1,shareChannel:"3001",account=<真实值，形状: 11位手机号>`）；**做完把 fileID 从 `KeyTempFiles` 移除**，注释"删除文件可能导致分享失效" | **网盘长期残留 `activity_share_*.txt` 且外链有效** ⇒ 用户可见副作用，搬之前必须给用户开关 |
| `performAICamera` | `aiRecognize` + `chat/v2/add` | 低 |
| `performAlbumBackup` | 600×800 现算渐变 JPEG + `hcy/backup/createSnapshot` + `completeSnapshot` | **注释直说"服务端据此判定成功备份一次" ⇒ 污染用户相册，这批里风险最高** |

⇒ 落点：这四类动作在 Rust 里应是**能力级私有工具 + 显式风险声明**（`Capability::risk` / `needs_confirmation`），
不是 `activity_actions.go` 这样的公共静默库。

---

## 3. 抢兑子系统（Go 项目真正的重头，也是我们错误模型的对照源）

`legacy/crates/mcloud-core/src/error.rs:1-10` 已经对齐了它的两条约定（错误分类决定重试策略；对外绝不回传 `err.Error()`）。
本节给出"到底对齐到哪一步"。

### 3.1 一次下单的 11 步（顺序即护栏，不可乱）

```
0 ctx.Err() 预检
1 提交幂等锁 SetNX              exchange:submit:<actorID>:<prizeID>，TTL 5s
2 令牌桶 WaitContext            30 rps / burst 30
3 准备鉴权（TokenManager 优先，否则 basic→JWT）
4 建 session：cookiejar + 1min timeout + 随机 UA + 12s 取数美 deviceId（失败用硬编码兜底）
5 取滑块 + 远端识别（3 次，退避 attempt*200ms）
6 偏移抖动 offset + rand(-3..3)，下限 0
7 GET 下单                      ← 写操作走 GET
8 重试循环 attempt<=maxRetries，success || !retryable 则停，退避 attempt*2s
9 月度系列租约（在重试之外、执行之内）
10 fencing 落库：FinalizeOwned(token) 同事务更新计数 + 插 exchange_records
11 TaskLog + WebSocket
```

证据：`services/exchange_request_control.go:70-84`（0-2）、`services/exchange_executor.go:88-146`（3-7）、
`services/exchange_retry.go:22-53` + `services/exchange_execution_service.go:294-312`（8）、
`services/exchange_execution_service.go:279-292`（9）、`repository/exchange_task_execution_repo.go:189-238`（10）。

### 3.2 下单响应分支表（**这是要搬的内核**）

`GET https://m.mcloud.139.com/ycloud/signin/page/exchangeV2?prizeId=…&client=app&clientVersion=13.0.0&puzzleOffset=…&smsCode=`
（`services/exchange_executor.go:148,212-220`，含 `req.Host` 覆写 `:152`）。

| 条件 | success | stop | 消息模板 | 行 |
| --- | --- | --- | --- | --- |
| 读 body 失败 | false | 否 | `读取响应失败：%v` | `:163-166` |
| `status>=400` | false | 否 | `请求返回异常 \| http_status=%d \| body=%s` | `:175-177` |
| JSON 解析失败 | false | 否 | `解析响应失败：%v \| … \| body=%s` | `:179-182` |
| 无 `msg`/`message` 键 | false | 否 | `响应格式错误 \| …` | `:184-187` |
| `msg != "success"` | false | **查终端文案表** | `buildExchangeFailureMessage(...)` | `:188-191` |
| `msg=="success"` 但内层 result 是业务失败 | false | **查终端文案表** | 用内层 desc 重渲染 | `:192-200` |
| 全通过且有 `result.prizeName/name` | **true** | true | `兑换成功：<name>，<solveInfo>` | `:202-209` |
| 网络层失败 | false | 否 | `请求失败：%v` | `:157-160` |

**成功判定只看 `msg EqualFold "success"`**，商品列表那条读接口才用码白名单
`{"",0,0000,000000,200,success,ok,true}`（`services/product_service.go:108-127`）⇒ 同一个域内两套成功定义，
`""` 与 `200` 都算数。这解释了旧 TS 盘点篇 §1.2 的"三套信封"为什么必须收敛。

**终端（stop）文案白名单**（`services/exchange_executor.go:222-243`）：
`已兑完 / 已耗尽 / 奖品单日已耗尽 / 奖品已兑完 / 云朵不足 / 不足 / 已兑换 / 今日已兑换 / 本月已兑换 / 已下架 /
账号未登录 / 账号失效 / 账号已失效 / 登录失效 / 重新登录 / 认证为空 …`。
其中孤零零一条 `"不足"` 会命中"库存不足 / 参数不足 / 次数不足"一切文本（§8.4 X-B7）。

### 3.3 分类与重试决策表（12 类，**有序子串匹配**）

`services/exchange_request_control.go:297-325`；输入是**渲染后的失败消息**（含 `http_status=`/`body=`）。

| 序 | 命中任一子串（lower+Contains） | kind | 重试 |
| --- | --- | --- | --- |
| 0 | 空消息 | unknown | ✅ |
| 1 | `幂等保护` / `重复提交` | duplicate_window | ✅ |
| 2 | `已兑完` / `已耗尽` / `售罄` / `库存` / `sold` | sold_out | ❌ |
| 3 | `云朵` / `余额` / `不足` | insufficient_cloud | ❌ |
| 4 | `auth` / `token` / `jwt` / `登录` / `鉴权` | auth | ❌ |
| 5 | `timeout` / `超时` | timeout | ✅ |
| 6 | `429` / `频繁` / `限流` | rate_limited | ✅ |
| 7 | `本月` / `今日已兑换` / `已兑换` / `已领` | already_claimed | ❌ |
| 8 | `下架` / `不存在` / `prizeid` / `更新商品列表` | product_unavailable | ❌ |
| 9 | `解析响应失败` / `响应格式错误` / `body=` | invalid_response | ✅ |
| 10 | `请求失败` / `connection` / `refused` / `reset by peer` | network | ✅ |
| 11 | 其余 | other | ✅ |

不重试集合的权威定义在 `:331-342`（5 项）。**分类标签与重试性是两个正交表** ——
标签另外用于落库统计（`repository/exchange_record_repo.go:127-147`）。

**与 Rust `ErrorKind`（`legacy/crates/mcloud-core/src/error.rs:13-40`）的差集，必须逐条决定**：

| Go kind | Rust 现状 | 动作 |
| --- | --- | --- |
| `auth`/`sold_out`/`insufficient_cloud`/`timeout`/`network`/`rate_limited` | 已有，重试性一致 | 保留 Rust 的 `marks_account_unhealthy`（Go 侧不置不健康，是它的缺） |
| `already_claimed` | **缺** | 新增；不重试且必须能进 `completed`（今天已到手不是失败） |
| `product_unavailable` | **缺** | 新增；不重试 → `completed` |
| `invalid_response` | 对应 `Parse`，但 **Rust 的 `Parse` 不重试** | 改为重试（上游返回半截 JSON 时重试有利） |
| `duplicate_window` | **缺** | **不要放进上游分类器** —— Go 把"我方本地锁消息"和"上游拒绝"混在同一张子串表里，正是它的 bug 根源 |
| `other`/`unknown` | 落到 `Internal`（不重试） | **决策点**：Go 靠"默认重试"兜住未知错误，Rust 靠"默认不重试"兜安全。抢兑是花钱的写操作 ⇒ **保持 Rust 的不重试默认**，但要给"未知"加 `ManualRequired` 出口而不是静默失败 |

### 3.4 锁、令牌桶与 double-spend 面

| 层 | key | TTL | 释放 | Redis 出错时 |
| --- | --- | --- | --- | --- |
| API 命令幂等 | `operations.source_operation_id` UNIQUE + `Idempotency-Key` | 持久 | 不适用 | 不适用 |
| 提交保护（下单前） | `exchange:submit:<actorID>:<prizeID>`（`actorID = AccountID`，为 0 时回落 `ID` —— 见 X-B16） | `EXCHANGE_SUBMIT_LOCK_TTL` 默认 5s | **无释放路径** | **fail OPEN**（继续执行） |
| 无 Redis 时本地锁 | 同上字符串 | 同上 | 30s janitor 扫过期 | — |
| 月度系列租约 | `exchange:series:<user>:<rule>:<YYYY-MM>:<seriesKey>` | 到月末 +1h | 失败时 `DelIfValue`，成功时**故意不释放** | **fail CLOSED**；`lockStore==nil` 时**跳过守卫仅打日志** |
| 月卡租约 | `exchange:monthly:run:<date>` 24h / `:acc:<id>:<date>` 25h | — | 见 X-B5 | — |
| 令牌桶 | 进程内 `chan` + ticker | — | — | 非分布式 |
| 任务 running 独占 | `exchange_tasks.execution_token CHAR(36)` | 15min 后视为 stale（**硬编码无 env**） | 所有写带 `WHERE execution_token=?` | — |

**重复扣云朵的成立条件**（逐项，这是本节唯一目的）：

| 场景 | 会否重复下单 | 原因 |
| --- | --- | --- |
| 用户双击"执行" | **可能** | 前端 `operationHeaders()` **每次新生成** `crypto.randomUUID`（`frontend/src/api/operation.ts:4-8`）⇒ 这一层不防；真正防的是 DB 唯一约束 + 5s 提交锁 |
| 同一 operation 被两个 worker 抢到 | 不会 | fencing token + `TryMarkRunning` |
| worker 崩溃后重放 | **可能** | `RecoverStaleRunning`（15min）把 running 放回 pending 再打一次 GET；上游若已成功，第二次靠"已兑换"文案兜住 ⇒ **云朵是否二次扣除取决于上游幂等，本地无法保证** |
| **上游超时但实际成功** | **可能（最危险）** | 网络错误 → kind `network` → **重试** → 二次 GET |
| Redis 不可用 | **放大** | 提交锁 fail open，并发波次全放行 |

⇒ Rust 侧的落法：`run_id` + `UNIQUE(account, capability, prize_id, local_date)` 的 `runs` 表承担幂等；
提交锁用进程内 `Mutex` map（含**参数维度**，见旧 TS 盘点篇 §1.3 的 lockKey 教训）；
**超时类失败在 GET-as-write 上不得自动重试**（与 §1.3 "读重试只允许用于幂等 GET" 一致）。

### 3.5 调度：**定时开火，不是发现补货**

**全仓没有库存轮询循环**（没有 `for { check stock; sleep }`）。库存认知只有三个来源：
(a) 每天 03:00 的整表商品同步；(b) 下单瞬间上游拒绝文案（stop 白名单）；(c) `restock_cycle` 时间窗
（`repository/exchange_schedule.go:183-215`）。`products.daily_remainder_count` 存在且参与排序，
**但抢兑主流程从不读它做前置判断**（只有一处"仅打日志"的快照检查会看）。

值得搬的调度编排：触发前 30s 预热、按 `AccountID` 去重预热、并发 clamp 1..10、
`0 < delay <= 2min` 时起精确 timer（避免粗粒度轮询）、开火偏移 clamp ±5s、
**按 prizeID 分组 + 组内共享 limiter + 任一任务命中 stop 文案则整组停止**。
`claimSchedulerSlot`（`services/exchange_scheduler.go:345-368`）**零调用点，是死代码**，别搬。

⇒ 若旧 TS 侧确有"盯盘刷新"需求，**Go 无源可搬**，只能自行设计成
"下单失败 → 短窗口重投同一 prizeId"的伪轮询 —— 而那会直接撞上 §3.4 的重复下单面，**必须先有真幂等**。

### 3.6 验证码：**全自动，零人工兜底**

链路：数美设备指纹（`core/shumei/device_profile.go`，`slw.h5cmpassport.com:9090/deviceprofile/v4`，
内置 RSA 公钥包 `ep`，~60 字段 web 指纹，成功码 1100，产物 `B` 前缀 deviceId → `deviceId` 头 + `.thumbcache_*` cookie，
`sync.Once` 注入，**`CAIYUN_SHUMEI_DEVICE_ID` 可手工固定绕过**）
→ `POST getSlide`（3 次，退避 `attempt*200ms`）
→ **远端付费识别** `POST {CAIYUN_SMS_API_BASE_URL:-https://ydyp.apisky.cn}/api/sms/solve`（body `{puzzle,picture}`，返回 `data.offset`；本地无 CV；30s 超时、1MiB 响应上限）
→ `finalOffset = offset + rand(-3..3)` 进 URL。

| 事实 | 后果 |
| --- | --- |
| 失败即 `滑块验证码识别失败：%v` → kind `other` → 重试 → 耗尽 → 任务 failed | **验证码这一层没有"请你去 App 点一下"的出口**：全仓"人工/手动"字样在兑换侧只命中一处，而且是**任务级**（`repository/exchange_schedule.go:211` 的"仅一次任务已尝试过，等待人工处理"），不是识别级 ⇒ 正是 §1.4 要补的第四态 |
| 每次重试**重新拉一次滑块、重新识别** | N 次重试 = N 次付费识别；offset 不缓存 |
| 取 deviceId 失败用**硬编码兜底 deviceId**（`services/exchange_executor.go:27-29`，全服务共用同一个假设备号） | 风控负资产 ⇒ **不搬**；我们已有账号绑定的 `smid_fingerprint`/`smid_device_id`（v5 引入，schema 现为 v7） |

### 3.7 数据模型与审计

`exchange_rules`（旧名 `exchange_accounts`）/ `exchange_tasks` / `exchange_records` / `products` / `calendar_dates` / `operations`。
审计事实只有 `exchange_records`（成功失败都产，与计数同事务，`FinalizeOwned`）。
`exchange_task_history` **建表但 Go 代码零读写**（死表）；
`exchange_accounts.max_concurrency`/`retry_interval` **有列无消费者**；
`status` 枚举里的 `cancelled` 与 `task_type` 里的 `immediate` Go 侧永不写入 ⇒ 枚举与实现漂移。
PII：WebSocket 载荷用**未脱敏**的 `account_name`（= remark 或 `<真实值，形状: 11位手机号>`），
日志侧才脱敏 ⇒ 事件载荷形态不要照搬。

### 3.8 API 面（22 个端点，摘要）

`/exchange/rules`（与 `/exchange/accounts` **是同一资源的两个别名**）、`/exchange/tasks`（17 字段 + 6 个可互相替代的账号定位别名）、
`/tasks/batch`（上限一处写 100、另一处 `binding max=50` ⇒ **两套上限**）、`/tasks/:id/execute`（入队，带 `Idempotency-Key`）、
`/records`（统计窗口固定 30 天）、`/records/export`（硬上限 10000 + `escapeCSVFormula` 防公式注入 —— **这条要搬**）、
`/immediate`、`/products/search|categories|update`、`/exchange/config`（public，fail-open）、`/admin/exchange/config`（**PUT 无条件写 5 个 bool** ⇒ 单字段 PATCH 会把其他开关静默关掉，见 X-B14）。

---

## 4. 运行时层（语义高度可搬，实现全部要换载体）

### 4.1 全部 cron job

| job | schedule | 关键性 | 锁 key / TTL |
| --- | --- | --- | --- |
| `daily_task_execution` | `TASK_SCHEDULE`，默认 `0 8 * * *` | **Critical**（注册失败 ⇒ 进程拒绝启动） | `worker:cron:lock:daily_task_execution` / **12h** |
| `monthly_exchange` | 由 `system_configs.exchange_monthly_time` 拼 `%d %d * * *` | Non-critical | **无锁** ⚠️ |
| `auto_update_products` | `0 3 * * *`（常量） | Non-critical | 1h |
| `account_health_check` | `0 */6 * * *` | Non-critical | 2h + 进程内 `atomic.Bool` 二次防重入 |
| `history_archive` | `ARCHIVE_JOB_SCHEDULE` | Non-critical | `history:archive:lock` / 30min |

租约：owner = `hostname:pid:unixNano`，`SetNX` → 执行 → `defer DelIfValue`。
**崩溃后锁残留至 TTL**；daily 是 12h ⇒ 若把 `TASK_SCHEDULE` 改成一日多次，一次崩溃会吞掉后续所有轮次。
**取锁报错时 `return nil` 静默跳过本轮**（`app/worker/jobs.go:39-42`）⇒ 单机抖动 = 当天不签到，只留一行日志。
⇒ `JobCriticality`（日批注册失败即拒绝启动）**要搬**；"锁失败即静默跳过"**不要搬**。

### 4.2 "一个自然日"的确切语义（三条，逐条比旧 TS 强，但都有坑）

```go
cstZone := time.FixedZone("CST", 8*3600)        // services/task_service.go:296
today := time.Date(now.Y(), now.M(), now.D(), 0,0,0,0, cstZone)
```
→ `account_id = ? AND task_type IN (?) AND created_at >= ? AND created_at < ?`。

| 特性 | 实际语义 | 对我们 |
| --- | --- | --- |
| 粒度 | **整批**（该账号今日有任何 `DailyTaskTypes()` 日志即算已执行） | 我们要**更细**（每 capability × 每账号 × 每自然日），但要防"半途失败的账号被永久锁住" |
| 生效位置 | **只有 2 处，都在 HTTP handler**；cron 路径不查 | 我们的调度路径必须同样受日幂等约束 |
| 载体 | MySQL `COUNT` + `task_logs` | SQLite 上打 `UNIQUE(account_id, capability_code, local_date)` |
| 时区 | 边界硬编码 +8，**写入走 `CURRENT_TIMESTAMP` + DSN `loc=Local`** | ⇒ 正确性完全依赖部署设 `TZ=Asia/Shanghai`（compose/Termux 脚本侥幸设了），**零运行时断言**。Rust 侧：固定 `FixedOffset::east_opt(8*3600)` + **启动断言** |
| 全 skipped 批次 | 不落日志 ⇒ 当天可无限重跑 | ⇒ skipped 必须落轻量 run 行（§1.4） |

### 4.3 并发与熔断

- **两层独立并发**：`concurrency.TaskManager`（worker 池，默认 `TASK_CONCURRENCY=10`，`taskQueue` 缓冲 `×20`、`resultQueue` `×50`）与队列侧 `sem`（**先抢信号量再 Dequeue**，注释说明避免持有未启动的 delivery 拉长 PEL 空闲 —— 这个顺序值得记）。
- **真实缺陷**：`SubmitTask` 用 `select{…default:}`，队列满时返回**伪造的 `context.DeadlineExceeded`**（根本没有 deadline），`SubmitBatchTasks` 首个错误即 return ⇒ **该 200 账号批剩余部分静默丢弃，且整轮 `CalculateDailyStats` 不再执行**（缓冲 `10×20=200` 恰等于页大小 200）。
- **全局一个并发数，无法按任务声明并发度** ⇒ 我们的 per-capability `AccountMode` + `risk` 是净增益（"签到可并发、分享/上传类必须串行"它表达不了）。
- **熔断器状态机**（`core/http/client.go:82-128`）：closed → open（连续失败阈值 **10**）→ open 期 30s → half-open（放行 **5**）→ success 归零。**判定很重要**：只有 5xx 与传输错误记失败，**4xx 与业务错误码算 success**（`:575-589,593-597`）—— 云盘大量返回 200+业务码，若误判会把账号永久锁死。作用域是 per-client，而 `buildTaskRunner` 每账号每批 new 一个 client（`services/task_service.go:105-106`）⇒ 实际 **per-account-per-batch**，批次间不共享。
- 🔴 **确认 bug（本次逐行复核）**：`allow()` 在 half-open 递增 `halfOpenInFlight`（`:94-97`），
  但 `RequestWithContextOptions` 有 **4 条不经过 `recordSuccess`/`recordFailure` 就返回**的路径 ——
  body `json.Marshal` 失败（`:519-522`）、退避期 `ctx.Done()`（`:531-534`）、`ctx.Err()` 预检（`:541-543`）、
  `http.NewRequestWithContext` 失败（`:548-551`）；而 `recordSuccess`/`recordFailure` 只在 `:588,593,597` 被调。
  每条泄漏 1 个半开许可，**满 5 后 `allow()` 永远拒绝，且因无请求能通过 ⇒ 永不自愈**。
  ⇒ 搬熔断器必须**先修**（在 `allow()` 之后用 `defer` 或统一 `release()` 归还半开许可）。

### 4.4 令牌生命周期（6 类令牌 + 一张常量表）

| 令牌 | 存放 | 刷新触发 |
| --- | --- | --- |
| **Basic auth** | `accounts.auth`（`json:"-"`）；形状 `<真实值，形状: Basic base64(手机号:密码)>` 或裸 base64 | 提前 **5 天**；`expireAt<=0` ⇒ **恒刷新** |
| **authToken**（tellin） | `accounts.token` | `tellin/authTokenRefresh.do`，XML `<return>!=0` → `TokenRefreshError` |
| **specToken / ssoToken** | 仅内存 | 每次取 JWT 前先取 sso |
| **JWT** | `accounts.jwt_token` + 内存 | **内存寿命硬编码 `now+15min`**，与真实 `exp` 无关；头是**小写 `jwttoken`** |
| **App refreshToken（AES 会话）** | 不落库 | `user-njs.yun.139.com/user/auth/refreshToken`；⚠️ **默认 AES 密钥硬编码入仓**（`<形状: 30 字符 ASCII>`，`core/auth/app_refresh.go:17-32`），仅 env 可覆盖且**无"生产必须覆盖"校验** |
| **云笔记 app_auth** | 不落库；只用**正则**从 body 抠 `userId` 当 `user_id` | `note.mcloud.139.com/noteServer/api/authTokenRefresh.do`；🔴 **未校验 HTTP 状态码** |

`TokenManager` 常量（全硬编码，只有最后一项可 env 覆盖）：健康缓存窗口 2min、预刷新提前 5min、
**负缓存 `tokenRefreshErrorTTL=2min`**（防打爆上游，**必搬**）、连续失败阈值 3 ⇒ **自动 `is_active=false`**
并给一句面向用户的原因文案（"连续无法获取 JWT Token，账号已暂停，请重新登录后再启用任务"）、
刷新锁 30s + 续租 ttl/3 + 抢锁失败轮询 10s/500ms、预刷新扫描上限 50。

🔴 **两套重复实现**：`TokenManager` 与 `buildTaskRunner` 各自有一套 `IncrementJWTErrorCount` + "≥3 就禁用"
（`services/token_manager_refresh.go:186-190` vs `services/task_service.go:180-186`）⇒ 3 次上限可能 2 轮就触发，
且两处禁用文案不同。auth 刷新失败时 `refresh.go:132-137` **保留旧记录并返回 nil**（吞掉）⇒ 批次继续用旧 auth。

⇒ 我们的对应件：`auth_expire` 语义化闸门 + `fail_streak`。
**要补的两件事**：① 负缓存；② "连续失败自动停用 + 原因文案"（旧 TS 盘点篇 §1.4 的 `ManualRequired` 正好由它承担）。

### 4.5 事件与推送

**推送实际为 0**：`internal/notification/`（唯一文件，226 行）只有 `LogNotifier` 被装配，
`WebhookNotifier` **全仓零实例化**；无 Server酱 / Bark / Telegram / 钉钉 / 企业微信 / 邮件任务通知。
覆盖面更窄：只有**队列路径**的成功/最终失败与 `health_check` 会通知，
**cron 日批任何结果完全不通知**（`concurrency/task_manager.go:241-268` 只 `log.Printf`），
账号被自动禁用也不发 notification（只发 WS）。
⇒ 对我们是**硬需求、无源可搬**；可搬的只有接口形状（`Notifier` trait + `MultiNotifier` fan-out + lastErr 聚合）。

**WS 信封值得搬**：写库分配单调 `sequence` → 扇出 → 客户端 ack；未 ack 进 `client.pending`，
`WS_ACK_TIMEOUT/2`（默认 2.5s）触发重投，`WS_MAX_RETRIES`=3 后放弃；`markSeen` 幂等去重（容量 4096）；
重连按 `Last-Event-ID`/sequence 回放未 ack；`WS_MESSAGE_TTL` 24h；另有 SSE 兜底 `/events`
（CDN 不支持 WS 的场景，`VITE_PUSH_TRANSPORT=sse|ws|auto`）。
事件全表：`task_complete` / `task_summary` / `account_disabled` / `operation.updated` / 兑换 7 个 / `pong`。
（注意 `exchange_complete` 与 `exchange_completed` **两个并存**，命名不一致。）
🔴 **PII 缺陷**：`account.Phone` 明文进 WS `Data`，且 `SendToUser` **先持久化到 `web_socket_messages.data` 再扇出** ⇒
手机号落进 DB 且无 `deleted_at`；同一份代码日志侧却做了 `maskPhoneForLog` ⇒ **我们的事件载荷只放 `account_id` + 掩码标签**。

### 4.6 跨任务状态存储（语义必搬，建模必须重做）

- 抽象 `Storage{Get,Set,Delete}`（**无 TTL 参数、无 CAS、无 List**）+ `ScopedStorage`（prefix `account:<id>:`）；
  生产 base 是 Redis，最终键形如 `caiyun:task:account:<id>:<key>`，**`Set` 固定 24h TTL、Get 不续期、Delete 无前缀批量回收**；
  `FileStore` 是死代码（有路径穿越防护，但无 fsync / 无原子 rename）。
- 5 个键与每批重置：`user_id` / `ai_sessions` / `ai_cloud_num` **重置**（其中 **`ai_cloud_num` 从未被读写 = 纯死键**）；
  `temp_files` / `temp_share_links` **故意不重置**（留给 `after_task`）。重置粒度是"该账号本次执行"，不是"该自然日"。
- 🔴 **结构性错误：用 24h 过期的 KV 承载"待清理工作队列"**。`after_task` 被关 / 批次中途取消 / 熔断打开 ⇒
  `temp_files` 里的 fileID 留在云盘；清理失败时列表不清空、等下次重试，但 **24h 后 key 过期 ⇒ 重试信息消失 ⇒ 永久孤儿文件**。
  另外所有 `AppendStringList(...)` 返回值被 `_ =` 丢弃（`tasklist.go:209,429,460,489`；`activity_actions.go:60,84,135`）；
  `LoadAISessions` 把 Redis 故障吞成 `nil` ⇒ AI 类任务**静默 skipped**；
  `RedisStorage.Get` 把"未找到"和"Redis 挂了"统一包成 `key not found: %s` ⇒ 二者不可区分。
- ⇒ 落点：SQLite 表 `pending_cleanup(account_id, kind, ref, created_at, attempts, last_error)`，
  永不过期、由收尾能力消费、跨批重试；跨任务态用 `account_state(account_id, key, value, updated_at)`
  加**显式 `per_batch_reset` 列**，取代隐式白名单。

### 4.7 可观测性：只有两件事值得搬

❌ 不搬：Prometheus（`monitor/metrics.go` 651 行）、`/metrics`（还要求 token + 默认必须 HTTPS/loopback）、
K8s 探针五件套、OTel tracing、Grafana dashboard、logrotate、chaos/capacity drill、SBOM/签名。
✅ **搬**：① `GET :8081/status` 的 JSON 快照形状（每任务类型 total/success/failed/avgDuration + 活跃 worker + 队列深度 + 心跳）
—— 直接当我们"运行状态页"的形状；② **每 5min 打一条摘要日志**（注释原文："不接入监控系统时也能观察"）——
本地工具最划算的一条；③ 按大小轮转 + JSON 结构化日志；④ **审计中间件的"请求/响应体截断 + 脱敏 + 丢弃计数"**；
⑤ `RequestID` 贯穿（降级为"每批 `run_id` 贯穿日志"）。
⚠️ 一条纪律要搬：GORM 埋点注释明确"凭据与用户数据不入 trace"。

### 4.8 数据表清单与 SQLite 最小集

Go 侧 17+ 张表（迁移 `001` 建 17 张，23 个 `.sql`，`GET_LOCK('caiyun_schema_migrations')` 串行化）。

| 表 | 用途 | 本地要不要 |
| --- | --- | --- |
| `accounts` | 云盘账号 | ✅（去 `user_id`） |
| `task_logs` | **执行历史 = 日粒度去重的唯一真相** | ✅✅，并**加** `(account_id, task_type, local_date)` 唯一索引换真幂等 |
| `cloud_stats` | 趋势 | ✅✅（唯一键 `account+date`） |
| `task_configs` | 任务开关覆盖层 | ✅（`code,is_enabled,sort_order,run_in_batch`） |
| `system_configs` | KV 配置 | ✅（但只保留**有消费者的**键，见 §5） |
| `operations` | API→Worker 持久化命令/outbox | ⭕ **搬语义**为 `job_runs`（去 fencing token） |
| `web_socket_messages` / `_sequences` | 可靠投递信封 | ⭕ 一张 `events(seq,payload,created_at,expires_at)` + 本地已读游标 |
| `audit_logs` | 审计 | ⭕ 降级为操作日志（去 `user_id`/`ip`） |
| `calendar_dates` | 节假日/工作日（CSV 灌入） | ✅ —— **它是运行时依赖**（补货窗口/日历策略要用） |
| `login_fail_locks` | Redis 不可用时的降级防暴破 | ⭕ 搬语义（多账号导入的凭据暴破防护） |
| `task_logs_archive` / `exchange_records_archive` | 冷分层 | ⭕ 降级为定期 DELETE + 可选导出 |
| `users` / `sessions` / `refresh_sessions` / `announcements` | 多用户/会话/公告 | ❌（§2.4） |
| `products` / `exchange_rules` / `exchange_tasks` / `exchange_records` / `exchange_task_history` | 兑换子系统 | ✅（`history` 死表除外） |
| `schema_migrations` | 迁移登记 | ✅（已有） |

⇒ **最小集 = 现有 5 张 + `job_runs` + `pending_cleanup` + `events` + `calendar_dates`**。

---

## 5. 参数面全量（一张表看清"三套存储 + 一套死系统"）

### 5.1 配置存储四份

| 存储 | 内容 | 判定 |
| --- | --- | --- |
| **env** | 170 处读取，**构造期与调用期混用**，core 层完全靠现读 | 业务参数全在这一层 ⇒ **不搬这个做法** |
| `system_configs` KV 表 | 种子 15 行，**其中 12 行无人读**（`task_concurrency`/`task_schedule`/`token_cache_ttl`/`exchange_schedule_time_1|2`/`exchange_update_time`/`exchange_enable_priority`/`exchange_default_timeout`/`exchange_max_global_concurrency`/`exchange_auto_retry_failed`/`exchange_log_retention_days`/`product_categories`） | **不搬**；`exchange_enabled` 更是**只有展示消费者 = 死旋钮，且缺省 fail-open** |
| `task_configs` | 只有全局 `is_enabled`/`sort_order`/`run_in_batch` | ✅ 搬表，见 §5.4 |
| `internal/core/config/`（YAML + `FeaturesConfig` 10 个 bool） | **零 importer、仓库内零 YAML 文件**；作者已在 `pkg/config/config.go:1-6` 留注释承认"曾经造成两条分叉的配置路径"，然后**以另一形式重新长出同一个坑** | **不搬** |

另有：`.env.example` **两套近乎不相交**（根 71 键 / `backend/configs` 123 键，仅 26 键只在根、79 键只在 backend），
且约 **35 个键在 Go 源码零引用**（`ENABLE_*`、`SMS_*`、`ALERT_*`、`GRAFANA_ADMIN_*`、大写 `EXCHANGE_*`、`MYSQL_*`…）。
⇒ 我们的门禁里若有"配置键必须有消费者"的检查，这条表就是它的存在理由。

### 5.2 业务任务参数（**全部是 env、全部进程级、零按账号覆盖**）

| 参数 | 默认 | 读取处 | 影响 |
| --- | --- | --- | --- |
| `CAIYUN_TASK_SHAKE_TIMES` / `_DELAY_MS` | 15 / 1000 | `services/task_runner_daily_tasks.go:40,41` | `shake` 次数与间隔 |
| `CAIYUN_TASK_WXDRAW_TIMES` / `_DELAY_MS` | 1 / 500 | `:30,31` | `wxdraw` |
| `CAIYUN_TASK_CLOUDBATTLE_GAME_TIME` | 30 | `task_runner_extended_tasks.go:38` | `cloudbattle` 局数（**名字不带单位，秒还是分钟不可判**） |
| `CAIYUN_TASK_BACKUP_RETRY_COUNT` / `_WAIT_SECONDS` | 1 / 20（≤0 强回 20） | `core/tasks/tasklist.go:122,126` | `task_expansion_reward` 备份重试 |
| `CAIYUN_TASKLIST_SKIP_IDS` | `""` | `tasklist.go:682` | 跳过指定转存 ID |
| `CAIYUN_MAKEWISH_PRIZE` | `""` | `core/tasks/makewish.go:151` | 许愿奖品选择 |
| `CAIYUN_SHUMEI_DEVICE_ID` | `""` | `core/http/client.go:308` | **手工固定设备指纹**（与我们的"账号绑定特征"同构，可作轮换逃生口） |
| `CAIYUN_AI_USERID_KEY` | `""`，**仅 `len==16` 才生效，否则静默忽略** | `core/utils/utils.go:114` | AI userId 解密 |
| `CAIYUN_APP_REFRESH_AES_KEY` | `""` ⇒ 用硬编码默认 | `core/auth/app_refresh.go:28` | App 刷新会话 |
| `CAIYUN_SMS_API_BASE_URL` / `_TOKEN` | `https://ydyp.apisky.cn` / — | `core/sms/sms.go:286,293` | 远端滑块识别 |
| `EXCHANGE_*`（pacing/rps/burst/submit-lock-ttl/fire-offset/…） | 见 `services/exchange_request_control.go:94-96,254`、`services/exchange_scheduler.go:324-327` | 抢兑 |
| `ARCHIVE_*` / `OTEL_*` / `RATE_LIMIT_*` / `JWT_*` / `SMTP_*` / `TASK_*` / `WORKER_*` / `WS_*` / `SSE_HEARTBEAT_INTERVAL` / `DATA_ENCRYPTION_*` | — | 基础设施（多数**不搬**） |

### 5.3 默认值的真实份数：**不是三处，是 4~6 处**

例：`TASK_CONCURRENCY` = Go 常量 10（`app/worker/config.go:45`）/ compose 10 / `system_configs` 种子 10（**死键**）/ `.env.example` 5。
抢兑并发更糟，6 处：`constants.DefaultConcurrency`、`exchange_config_service.go:15,20,25,30`、
`exchange_scheduler_helpers.go:141,146,151`、`handlers/exchange_config.go:34`、SQL 种子、前端 `:max="50"`。
**UI 与 API 的区间还不一致**：并发数前端 clamp 50、后端允许 1000。

### 5.4 无按账号任务开关（确认成立，且比旧 TS 更受限）

`task_configs` DDL 无 `user_id`/`account_id`（`migrations/001_init.sql:245-256`）；
唯一写入口 DTO 只有一个 `IsEnabled bool`（`services/admin_reporting.go:232-240`）；
语义是**平台上下架**（运行时消费点 `task_execution.go:112` → `Message:"任务已下架"`）。
⇒ **普通用户关不掉任何单个签到任务**，只能停整个账号（`accounts.is_active`）。
真正的按账号配置只存在于抢兑（`exchange_rules.exchange_time_1/2`、`is_active`）与账号级 `is_active`。
⇒ 新项目的落点：`capability_overrides(account_id?, code, is_enabled, sort_order, run_in_batch)`
+ `params` 覆盖 —— **这块 Go 什么都没有，是我们必须自己立的项**（方案 §3.3 的 `default_account_mode` 同理）。

### 5.5 前端：100% 硬编码，但**契约生成值得抄**

Vue3 + Element Plus + Pinia，12 个 view。**没有 schema 驱动表单**（因为任务无参数）。
`utils/task-types.ts` 里三份硬编码清单（`taskTypeNameMap` 28 条 / `orderedTaskTypes` 26 条）
与后端 24 条目录**既不同名也不同序**，还含 4 个后端不存在的条目（`exchange`/`store`/`all`/`trigger_all`）
⇒ 正是 D3「表单与能力声明同源」要消灭的东西。
双向缺口：`run_in_batch`、全部 `CAIYUN_TASK_*`、`TASK_SCHEDULE`、`TASK_CONCURRENCY`、dead-letter 重放、
`exchange_tasks.priority/task_group/timeout_seconds/max_retries` **后端有 UI 无**；
`store/account.ts`、`store/stats.ts`、`views/AnnouncementManage.vue`（593 行）**UI 有后端/路由无**（死代码）。

**唯一强烈推荐的非业务件**：`predev`/`prebuild`/`pretypecheck`/`pree2e`/`pretest:unit` 五个 lifecycle 钩子
跑 OpenAPI/AsyncAPI → TS 客户端生成，配 `validate-dto-boundaries.py` + `validate-frontend-contract-usage.py` 进门禁
（`frontend/package.json:7` + `scripts/generate-*.py`）⇒ **正好是我们 `FieldSpec → WebUI/API/CLI` 生成链的工程护栏形态**。

---

## 6. 需要集中管理的硬编码常量

| 类 | 常量 | 位置 |
| --- | --- | --- |
| 域名 | `MarketURL`（`caiyun.feixin.10086.cn/market`）、`Market7071URL`（同主机 `:7071`）、`MobileMarketURL`（`m.mcloud.139.com/ycloud`）、`AIYunURL`（`ai.yun.139.com`） | `core/api/caiyun.go:11-14` |
| 版本 | `MarketClientVersion="12.5.4"`；抢兑里 **下单 13.0.0 / 列表 12.5.3 / UA 12.5.4 / 死代码 12.4.0** | 同上；`services/exchange_executor.go:20-21`、`core/api/get_product.go:20`、`utils/http_client.go:27,33` |
| 源/渠道码 | `MarketSourceID="1097"`；抢兑 `source/targetSource = 1097/001005`；`activityId="sign_in_3"`；`ChannelSrc="10200153"`；cpactiv `appId=12345681` | `services/exchange_executor.go:22-25`、`core/tasks/tasklist.go`、§2.3 |
| 签名盐 | saltKey `1`/`2` 两把（应用级白盒常量，非账号凭据） | 我们已有：`legacy/crates/mcloud-core/src/crypto.rs:43` `SIGN_SALT_MAP` |
| 阈值 | 熔断 10/30s/半开 5；JWT 失败自动停 3；令牌窗口 2/5/2min；`stale running` 15min；滑块重试 3；AI 抽奖 5；funai 抽奖 5 | §4.3、§4.4、§3 |
| 魔法数 | 月卡奖品 `1001`；`alreadyDrawn = 50 - surplus`；配额 `20`；`OffsetJitter=±3`；`MaxMultipartSize=8MiB` | `services/exchange_monthly_service.go:119-130`、`core/tasks/wechat.go:121`、`core/tasks/invitefriends.go`、`services/exchange_executor.go:141`、`app/api/config.go:56` |

⇒ 落点同旧 TS 盘点篇 §4（集中到 `mcloud-upstream` 的端档案 + `mcloud-core` 的常量模块），
并遵守 [`docs/device-and-source-codes.md`](./device-and-source-codes.md) 的一句话判据：
**能在同一端两个接口上取不同值的码，不许钉进端档案**（Go 侧 `clientVersion` 三个值并存就是反例）。

---

## 7. 动代码前必须实测确认的冲突

与 [`business-feature-inventory.md`](./business-feature-inventory.md) §5 同一张检查表的关系：**那张是"旧 TS 写法 vs 我们的抓包"，本节是"Go 写法 vs 我们的抓包 / vs 旧 TS"**。三源冲突时，**以我们自己的抓包为准**（方案 §6.3「不从旧实现搬」）。

| # | 冲突 | 两侧写法 | 动手前要做的事 |
| --- | --- | --- | --- |
| 1 | **签到域前缀** | Go：新前缀 `m.mcloud.139.com/ycloud/signin/...`；旧 TS：`caiyun.feixin.139.com/market/...`；我们抓包：`/ycloud/signin/...` | 按抓包定前缀；但**必须实测 Go 的 `infoV3` 与旧 TS 的 `page/startSignIn` 在同一前缀下是否都还在**（Go 写的是 `signin/page/infoV3`，且抢兑列表却是**无 `/ycloud` 前缀**的 `/market/signin/page/exchangeList`） |
| 2 | **`clientVersion` 到底几版** | Go 同时有 12.5.3 / 12.5.4 / 13.0.0 / 12.4.0 四个值 | 抓一条真实下单 + 一条列表，逐端点记版本，**不要合并成一个端常量** |
| 3 | **门户预热是否必需** | Go 每次签到前重跑 28 个请求（含 6 条 `visitlog/journaling` 埋点 + JSESSIONID）；旧 TS 无埋点 | 做**最小请求集**对照实验：只发 `infoV3 → startSignIn` 能否成功。若不能，再逐步加埋点，找出真正被服务端校验的那一条 |
| 4 | **下单是 GET** | Go 与旧 TS 一致：`GET exchangeV2` | 实测确认上游是否仍接受 GET；若提供 POST 变体则改用 POST + 幂等键（§3.4 的重复下单面会显著缩小） |
| 5 | **`exchangeList` 与 `exchangeV2` 不同前缀** | 前者 `caiyun…/market/signin/page/exchangeList`，后者 `m.mcloud…/ycloud/signin/page/exchangeV2` | 两条分别实测，不要假设同一个 base |
| 6 | **数美 deviceId 是否进原生头** | Go 发 `deviceId` 头 + `.thumbcache_*` cookie（活动/H5 域） | **沿用上文已核对的结论**：我们那批 App 抓包里原生请求头 `deviceid` 一次都没出现，带 `deviceId` 的 178 条夹具**全是 H5 / WebView**。所以这条**不是"补上 deviceid"**，而是等一条**原生**抓包再逐端点判断（详见 `docs/rust-rewrite-plan.md` §7 的 smid 段与 `docs/client-profiles.md` §5.4 ③） |
| 7 | **红包派对域无 `code`** | Go 与旧 TS 都用 `header.status` | 该域必须显式 `UnknownEnvelope` 之外的第四类判据，实测确认字段仍叫 `status` |
| 8 | **复活卡月度上限** | 旧 TS 本地 `revival.json` 记 2 次；Go 侧无本地额度、全靠服务端文案 | 实测服务端拒绝文案（Go 的双条件 `Contains("复活卡") && Contains("已领取")` 说明文案会变），决定我们是否保留本地额度 |
| 9 | **`tasklist` 接口族** | Go 用 `doTaskPost` / `GET /signin/task/click?key&id` / `POST /signin/task/receiveTask` / `POST /signin/task/receiveV2` 四件混用 | 逐条实测哪个是现役；`click` 是 GET 且**幂等**，可以按 §1.3 开读重试 |
| 10 | **口令/直播（`kouling`）** | **Go 项目完全没有** | 只能回旧 TS 实现 + 抓包；不要为了"有参考"而放弃实测 |
| 11 | **活动期次是否还活着，代码看不出来**（2026-09-19 追加） | Go 把 `cloudbattle`（合成 1T）当默认启用的入批任务；实测口径：**该活动已过期**。Go 里另有 `garden`/`redpacket`/`aicloud`/`blindbox`/`poster_activity` 五条**自己写明"已下架"** | "接口下线"与"我这端点写错"在非零码上不可区分 ⇒ 动任何一条活动前先确认当期有无活入口；已过期的（含 `cloudbattle`）不因本表列了就进实现顺序。与旧 TS 盘点篇 §5.9 是同一件事的两个来源 |

---

## 8. 搬 / 不搬 / 先修再搬

### 8.1 业务能力

| 判定 | 清单 |
| --- | --- |
| **直接搬语义** | `signin`（预热改为每账号每批一次）、`receive`、`cloud_multiple`、`messagepush`、`todaycloud`（= 我们的 `cloud-snapshot`）、`wechat`、`fun_ai`、`revivalreward`（+ 补本地月度额度）、`make_wish`（先修）、`calendar_dates` 节假日依赖、批次排尾的"统计 → 清理"两阶段 |
| **先修再搬** | `task_expansion_reward`（code 未判 + 单跑不清理）、`backupgift`（静默 nil + `curMonth/state` 漏领）、`cloud_multiple` 与 `revivalreward` 的"已领取"判定统一、`wxdraw`（硬编码 50 + string code）、`shake`（`Printf` 绕过日志）、`cloudphone`（补 taskID）、`cloudbattle`（修签名）、`token_pk`（修 `openUrl`/`rewardStages`/Debug 吞错）、`tasklist`（special/skip 顺序 + 恒返回 nil + 两种上传口径）、`invitefriends`（默认关闭 + 匿名化 + 修 `getCloudRecord` 出错返回 20）、`after_task`（清理改表） |
| **不搬** | `garden`、`redpacket`、`aicloud`、`blindbox`、`poster_activity`（四条目录自陈"活动已下架"；`redpacket` 弃 ⇒ `aicloud` 无意义；`blindbox` 弃 ⇒ 去掉 ① 号 mail.10086.cn 四跳 SSO）；`performAlbumBackup`（污染用户相册）默认不做、须显式开关 |

> **补充（2026-09-19 用户订正）**：上表按 2026-09-19 的实测口径有两处**读法调整**（原表保留不改）——
> `cloudbattle` 从"先修再搬"**移出**（活动已过期，见 §7 第 11 条），`revivalreward` 的"补本地月度额度"
> 要按**真实月限 1、旧 TS 的 2 是保险**来设计（见 §0 补充与旧 TS 盘点篇 §5.8）。

### 8.2 运行时与架构（对齐方案 §2.4）

| 不搬（及理由） | 搬语义（及换后的载体） |
| --- | --- |
| MySQL + Redis 双依赖、compose 8 服务、`docker`/`k8s`/KEDA/ExternalSecrets/Prometheus Operator | 日幂等：`runs(account_id, code, local_date)` 唯一索引 |
| API+Worker 双进程、Redis Streams 队列 + DLQ + PEL + 跨实例 pubsub、`execution_token` fencing、调度槽租约 | 持久化命令 outbox：`job_runs`（保留"失去所有权的执行不得回写、不得 ACK"的**不变式**） |
| 多用户 + RBAC + 开放注册 + CSRF 双提交 + `refresh_sessions`/`token_version` + `bootstrap-admin` | 队列去重键优先级：**IdempotencyKey > 内部 job_id > 三元组**（保住合法重放不被压掉） |
| Prometheus/Grafana/OTel/logrotate/chaos/SBOM/探针 | 熔断器（**先修半开泄漏**）、**非幂等 POST 默认不重试** + `Idempotency-Key` 才重试、指数退避 + ≤25% 抖动、令牌负缓存 + 预刷新 + 连续失败自动停号、跨任务态与 `pending_cleanup` 表、WS/SSE 可靠信封（sequence + ack + 重连回放）、`/status` 快照 + 5min 摘要、按大小轮转 + JSON 日志、审计截断脱敏、`RequestID`→`run_id`、按路由限流（验证码类 1r/s）、CSV 公式注入防护、`login_fail_locks` 语义 |
| 三套并存的配置存储、12 个死 `system_configs` 键、两套 `.env.example`、一套零 importer 的 YAML | 任务目录 `aliases` 归一化；`SyncDefinitions` 时**永不覆盖用户 `is_enabled`**（这条它对，但我们**不搬**它"软删不在目录的行"） |
| Go 侧 `logger` 兼作状态容器 | `RunReporter` 显式承载错误计数；`Warn 不计数` 的分级**保留** |
| OTel 实现 | 只搬一条纪律：**span/日志不带凭据与用户数据** |

**佐证**：它自己的统一 CLI 有 `all` 子命令**在单进程里同时跑 API + Worker**（`backend/cmd/caiyun/main.go:71-75`），
且 api/worker 共用 `bootstrap.InitSharedServices` ⇒ 双进程是**部署形态而非架构必需**，直接支撑方案 §2.4。

### 8.3 值得抄的"非业务"五件

1. **连续失败 N 次自动停号 + 面向用户的原因中文文案**（`token_manager_refresh.go:180-206`）—— 我们的 `fail_streak` 目前只记不停。
2. **删除账号的显式事务清理顺序 + 日志"先脱敏再删"**（`account_lifecycle.go:227-276`）。
3. **字段级加密 + 版本化 keyring + 离线轮换工具**（`security/field_crypto.go`、`cmd/reencrypt`、`scripts/rotate-encryption.sh`）——
   我们 `account_state` 目前明文；⚠️ 但它的 fail-open（两个 env 都空 ⇒ **不加密直接落库**，只告警）不要搬。
4. **OpenAPI/AsyncAPI → TS 客户端生成 + 契约校验进门禁**（§5.5）。
5. **`Deprecation`/`Sunset`/`Link` 头 + `/api` 与 `/api/v1` 双栈收敛策略**（`app/api/routes.go:45-56`，`Sunset: 2026-12-31`）。

### 8.4 已确认的真实缺陷汇总（编号可引用）

> 只列**代码事实**（能读出分支的那类），不列"上游可能改版"那类。全部已给 `file:line`，本次抽查复核过的标 ✅。

**T · 任务层**

| ID | 缺陷 | 证据 |
| --- | --- | --- |
| ✅ T-1 | 内联 `code` 解析对 **string 型非 "0" 值判成功**（初值 0），同一 bug 三处；正确实现 `parseCaiyunCode` 没被复用 | `core/tasks/todaycloud.go:41-50`、`backupgift.go:109-119`、`wechat.go:43,95,145`（三处里 `:43` 属 `wechat` 签到、`:95/:145` 属 `wxdraw` 抽奖）vs 正确实现 `tasklist.go:732-747` |
| ✅ T-2 | **领取失败零日志、返回 nil** ⇒ 批次显示成功 | `backupgift.go:121-131` |
| ✅ T-3 | 特殊任务处理跑在内置跳过检查之前 ⇒ 8 条 skip 配置里 **7 条死**（106/107/110/113/434/522/585 全部被 switch 覆盖），只有 1021 生效 | `tasklist.go:241` vs `:245`；switch `:334-380`；表 `:27-36` |
| ✅ T-4 | `cloudphone` 的 `userSign` 收了 `taskID` 但请求体是空 map ⇒ 永远打在服务端默认项 | `cloudphone.go:262,276,280` |
| ✅ T-5 | `cloudbattle` 签名按空 queryString 计算，实际 URL 带 `?inviter=` | `cloudbattle.go:356-358` |
| ✅ T-6 | `poster_activity` 全文件无 `receivePrize` 调用，且末尾恒 `return nil` | `poster.go`（`receivePrize` 命中数 0；`:100,112`） |
| ✅ T-7 | `make_wish` 的 `ENABLE_NOTIFICATION` 只把任务 append 进已完成列表，无真实动作 = 谎报完成 | `makewish.go:104-106` |
| ✅ T-8 | `token_pk`：`openUrl` 只 `Sleep(1s)` 从不访问链接；`rewardStages` 解析后丢弃；动作失败全 `Debug` ⇒ 几乎恒 success | `tokenpk.go:214-215`；`StepLink()` 仅单测引用 |
| ✅ T-9 | `shake` 结果用 `fmt.Printf` 绕过 logger ⇒ 不进 `task_logs`、不进 WS、不进推送 | `shake.go:49,51` |
| ✅ T-10 | `wxdraw` 硬编码 `alreadyDrawn := 50 - surplusNumber`（文件 `core/tasks/wechat.go`，但这段在 `RunDrawWithInterval` 路径上 ⇒ 归属任务码是 `wxdraw`；**同一个 `WeChatTask` 类型同时服务 `wechat` 与 `wxdraw` 两个任务码**，读代码时容易认错） | `wechat.go:121` |
| ✅ T-11 | "已领取"判定两套宽严不一致（单条件 vs 双条件） | `cloudmultiple.go` vs `revivalreward.go` |
| — T-12 | `invitefriends`：`getCloudRecord` 不校验 code、出错返回 20 ⇒ 配额耗尽后仍每天上报；`e = e & e` 空操作；首行日志 `"测试中。。。"` | `core/tasks/invitefriends.go` |
| — T-13 | `task_expansion_reward` 领取步 code 完全未判；单跑不清理临时文件（清理只挂在 `tasklist.Run` 第 5 步 / `after_task`）⇒ 网盘留垃圾 | `core/tasks/tasklist.go:121-188` |

**X · 抢兑**

| ID | 缺陷 | 证据 |
| --- | --- | --- |
| ✅ X-B1 | **重试自锁**：提交锁 5s 且不释放 ⇒ 退避累计 <5s 的那次重试（默认 `maxRetries=3` 时是第 1 次，t=2s）**必然**拿到"幂等保护"而不是真实失败原因 | 加锁无释放 `services/exchange_request_control.go:249-276`；重入 `:74-79` → `exchange_execution_service.go:305-312`；退避 `attempt*2s` `:303` |
| ✅ X-B2 | 本地锁消息与上游响应共用一张子串分类表（`幂等保护` 与上游拒绝并列） | `exchange_request_control.go:302-303`；重试性 `:331-342` |
| — X-B3 | 公共文案白名单被绕过：执行失败把含 `body=` 的上游原文透传，handler 再 `err.Error()`；正规映射器 `hdl/exchange_errors.go:18-48` **零调用 = 死代码** | `exchange_execution_service.go:361`；`hdl/exchange_tasks.go:263,286`、`hdl/exchange_rules.go:55,184,207` |
| — X-B4 | **无 Redis 时守卫静默失效**（仅打日志）⇒ 单机部署下月度系列守卫等于不存在 | `exchange_monthly_guard.go:220-223`、`exchange_monthly_service.go:209-212` |
| — X-B5 | 月卡 `keepAccountLock=true` 在 success 判定**之前**无条件设 ⇒ 一次瞬时网络失败把该号挡到 25h | `exchange_monthly_service.go:153-157`（对比正确写法 `exchange_execution_service.go:338-354`） |
| ✅ X-B6 | 用**渲染后的消息**做分类，而渲染把 `code=` / `result=`（**整个 result 值**）/ `body=`（180 字节摘要）全拼进去 ⇒ 上游任意字段含"库存/不足/登录"即误分类 | `exchange_executor.go:326-357`（`appendField("result","result")` 在 `:347`、`body=` 在 `:352-353`，摘要 `:394-404`）vs 匹配 `:297-325` |
| ✅ X-B7 | stop 白名单含单条 `"不足"` ⇒ 误伤面过大，且命中即**整组停止** | `exchange_executor.go:222-243`；组停止 `exchange_scheduler_execute.go:244-247` |
| — X-B8 | `prepareExchangeAuth` 换 JWT 失败后**吞错继续**用旧 auth | `exchange_executor.go:106-114` |
| — X-B9 | 商品同步 `UPDATE exchange_tasks … WHERE prize_name=?` / `prize_id=Memo` **不带 user/account 过滤**；按名称匹配还会串改同名不同商品，`p.id IS NULL` 那条能整表置 completed | `repository/product_repo.go:375-444` |
| — X-B10 | `clientVersion`/UA 四值互相矛盾（13.0.0 / 12.5.3 / 12.5.4 / 12.4.0） | §6 |
| — X-B11 | `ReplaceProducts` 内 `context.Background()` 丢弃调用方 ctx | `product_repo.go:287-288` |
| — X-B12 | 造 prize_id 时允许回落成 `Memo`，executor 又拒绝 memo 形态 ⇒ 建任务成功、执行必失败；另有两处"隐式纠正"实现 | `product_service.go:174-183` vs `exchange_executor.go:53-59` |
| — X-B13 | 死代码群：`respondExchangeServiceError`、`claimSchedulerSlot`、`utils.BuildExchangeURL/BuildProductListURL/IsRetryableError`（**第二套重试策略**）、多个 repo 方法、表 `exchange_task_history`、列 `max_concurrency/retry_interval` | 见各定义处，零调用点 |
| — X-B14 | 配置 `PUT` 无条件写 **4 个 bool + 1 个 int**（`fmt.Sprintf("%v", …)`，JSON 省略 ⇒ 零值 false）⇒ 单字段更新会静默关掉抢兑/月卡/立即兑换；只有两个字符串字段有 `if != ""` 保护 | `hdl/exchange_config.go:144-155` |
| — X-B15 | 月卡 cron 构造忽略 `Atoi` 错误，且调**无 ctx** 版本 ⇒ 时间畸形静默变 00:00、月卡执行不可取消 | `app/worker/jobs.go:94-137` |
| — X-B16 | 本地锁 key 用 `AccountID==0 ? ID : AccountID` ⇒ 两个不同用户恰好数值相同时**跨用户共享锁** | `exchange_request_control.go:278-284` |
| — X-B17 | 批量上限两套（100 vs `binding max=50`）；配置读取吞 repo 错误 | `hdl/exchange_tasks.go:16` vs `:330-332`；`exchange_config_service.go:12-38` |

**R · 运行时**

| ID | 缺陷 | 证据 |
| --- | --- | --- |
| ✅ R-1 | **熔断半开许可泄漏 ⇒ 永久熔断、永不自愈**（4 条早退路径） | `core/http/client.go:94-97`（递增）vs `:519-522,531-534,541-543,548-551`（早退）；记录点只有 `:588,593,597` |
| — R-2 | 日界时区耦合：边界硬编码 +8，写入走会话时区 + `loc=Local`，**零运行时断言**；部署不设 `TZ` 则日窗偏 8 小时 | `task_service.go:296-298`；`repository/task_log_repo.go:27-31`；`pkg/database/mysql.go:28` |
| ✅ R-3 | cron 日批路径不做日幂等；`skipped` 不落日志 ⇒ 全 skipped 批次当天可无限重跑 | `app/worker/run.go:156-163`；`task_service_execute.go:84-86` |
| ✅ R-4 | `skipped` 整条不可达（`disabledTasks` 生产恒传 `nil`）⇒ "部分不可用"在 UI/历史里不存在 | `task_service_execute.go:60`；判定 `task_execution.go:112-114` |
| — R-5 | `KeyAICloudNum` 死键（只定义 + 只重置，从未读写） | `core/tasks/store.go:256`；`task_execution.go:75` |
| — R-6 | 临时文件清理靠 24h 过期 KV 承载 ⇒ 云盘孤儿文件永久残留；所有 `AppendStringList` 返回值丢弃；无 CAS 的读改写在同账号并发投递下丢更新 | `cache/redis_storage.go:33-37`；`task_runner_cleanup.go:41-51`；`store.go:355-393` |
| — R-7 | `SubmitTask` 队列满返回**伪造的** `context.DeadlineExceeded` 并中断整轮 ⇒ 剩余账号静默不跑、跳过日统计 | `concurrency/task_manager.go:133-142,146-153`；`app/worker/worker.go:226-228` |
| — R-8 | `processResult` 把 `skipped` 计入 `FailedTasks` ⇒ `/status` 失败率虚高 | `task_manager.go:255-259` |
| — R-9 | Redis 故障被当作"key 不存在"；`ResetKeys` 吞掉所有 Delete 错误 | `redis_storage.go:26-28`；`store.go:267-272` |
| — R-10 | 批次锁取锁报错 ⇒ **静默跳过本轮**；daily TTL 12h 与"一日多次 schedule"无耦合校验 | `app/worker/jobs.go:39-42`；`run.go:157` |
| — R-11 | `monthly_exchange` job 无租约、无进程内防重入（兄弟 job 都有） | `jobs.go:113-133` vs `:190-199` |
| — R-12 | 云笔记鉴权**未校验 HTTP 状态码**，且用正则从 body 抠 `userId` ⇒ 错误页含 `"userId":` 即被当有效 | `core/api/caiyun_note.go:35-53`；`core/tasks/userid.go:10,31-34` |
| — R-13 | JWT 失败计数**两条路径各自自增** ⇒ 3 次上限可能 2 轮触发；两处禁用文案不同 | `token_manager_refresh.go:186-190` vs `task_service.go:180-186` |
| — R-14 | `RetryManager` 注释"指数退避"、实现**线性**；`time.Sleep` 不响应取消 | `monitor/task_monitor.go:414-417,440,443` |
| — R-15 | 明文手机号进 WS 载荷并先持久化再扇出（同库日志侧却脱敏） | `task_service_execute.go:145,160`；`ws/hub.go:396-412` vs `app/worker/jobs.go:293-299` |
| — R-16 | `Logger.Error` 的 `recordError` 在级别门内 ⇒ 若开放"日志级别可配"就会静默吞错（当前唯一构造点硬编码 `LevelInfo`，不可触发） | `core/logger/logger.go:79-83,189-194` |
| — R-17 | **任务落库失败被折算成"任务失败"** ⇒ 上游已成功却显示 failed，重跑 = 重复领取 | `task_service_execute.go:102-105,181-183` |
| — R-18 | `HasExecutedToday` 无 ctx 版吞掉 DB 错误 ⇒ 抖动 = "今日未执行" = 重复跑 | `task_service.go:285-287` |
| — R-19 | 任务/账号/批次**三级超时都不存在**；只有 HTTP 30s + 任务边界 `ctx.Err()`；`CleanupStaleTasks` 只清内存表不取消 goroutine | `core/http/client.go:196`；`task_execution.go:96-104`；`monitor/task_monitor.go:323-369` |
| — R-20 | `validateSchedule` 把 6 段表达式的前 5 段当分/时/日/月/周校验（首字段实为秒），`dow` 不校验；`?`/`L`/`#`/英文名一律拒 ⇒ 合法 robfig 表达式被误拒 | `scheduler/cron_scheduler.go:350-373,449-503` |
| — R-21 | `clearTaskDedupeKeys` 只扫 200 个 key 且不循环 ⇒ 清队列后仍有 dedupe 残留，重放被吞 | `queue/dedupe.go:104-118` |
| — R-22 | 硬编码上游 AES 密钥入仓（仅 env 可覆盖，无"生产必须覆盖"校验）——与它对 JWT 的 `GetSecretEnv` 纪律不一致 | `core/auth/app_refresh.go:17-32` vs `bootstrap/env.go:46-63` |
| — R-23 | `SyncDefinitions` **软删所有不在代码目录内的 `task_configs` 行** | `repository/task_config_repo.go:102-106` |
| — R-24 | `TASK_CONFIG_SYNC_ON_STARTUP` 默认 **false**，而批次解析优先读 DB ⇒ 种子 19 行 vs 目录 24 条：`cloud_multiple`/`token_pk`/`make_wish`/`fun_ai`/`poster_activity` **静默永不入批、无告警** | `bootstrap/core.go:56`；`task_execution.go:20-28`；`task_catalog.go:32-319` vs `migrations/001_init.sql:517-536` |
| — R-25 | 通知覆盖面只有队列路径；**cron 日批任何结果不通知**、账号自动禁用不通知；`WebhookNotifier` 零实例化 | `task_manager.go:241-268`；`notification/notifier.go:161-226`；`app/worker/run.go:132-134` |

**C · 配置与账号**

| ID | 缺陷 | 证据 |
| --- | --- | --- |
| — C-1 | 更新账号 `auth` 是 `omitempty` ⇒ 只改备注回传空 auth 会**清空凭证** | `services/account_lifecycle.go:14-25,56-81` |
| — C-2 | `maxJWTRefreshFailures=3` 与全部令牌时间常量硬编码、无 env、UI 无入口 | `services/token_manager.go:45-53` |
| — C-3 | `expire_at` 存**毫秒**，而 `envutil.Duration` 把裸数字当**秒** ⇒ 同进程两套时间单位 | `models/models.go:34`；`internal/envutil/envutil.go:71` |
| — C-4 | GORM 标签写成 `column:"jwt_token"`（`column` 不是 GORM 的 tag key）⇒ "看起来有 tag 的 no-op" | `models/models.go:29-32`；同款在 `models/exchange.go` |
| — C-5 | 软删 + `uk_user_phone(user_id,phone)`（不含 `deleted_at`）⇒ 删号后重建同号撞唯一键 | `migrations/001_init.sql:153-175`；`account_lifecycle.go:56-81` |
| — C-6 | `Account.Preload("TaskLogs")` 无分页 ⇒ 账号详情把该号全部日志载入内存 | `models/models.go:26-44` |
| — C-7 | 校验三处不一致：后端 `^1[3-9]\d{9}$`、前端 CK 表单只 `required`、短信表单用更宽的 `/^1\d{10}$/`；且错误只以 toast 平文本呈现（无字段级定位） | `pkg/validator/validator.go:249-253`；`frontend/src/views/AccountManage.vue:428-444,361-369`；`api/axios.ts:131-179` |
| — C-8 | "每天限手动执行一次"用 **429** 表达（限的是业务日额度不是请求速率），阈值不可配 | `handlers/account_tasks.go:39-57` |
| — C-9 | `ConfigService`（DB 覆盖 env）生产从未构造；`ReloadFromDB` 存裸 string ⇒ `GetInt/GetBool` 断言永不命中 = **自失效缓存** | `services/config_service.go:20,24-42,108-124` |
| — C-10 | 同一张配置表两种生效语义：`exchange_concurrency` 每次执行读、月卡 cron 注册时读一次 ⇒ 改一个立刻生效、另一个要重启，UI 无提示 | `exchange_config.go:116-164` vs `app/worker/jobs.go:94-137` |
| — C-11 | `exchange_monthly_time` 兜底 worker `10:00` vs handler `00:00`，且种子写的是另一批键名 ⇒ 全新部署"cron 在 10:00 跑、界面显示 00:00、用户改不动" | `jobs.go:97` vs `exchange_config.go:54`；`migrations/001_init.sql:502,503,505` |
| — C-12 | 伪任务 code（`health_check`/`all_tasks`/`all`）不在目录也不在前端映射 ⇒ 日志/通知页渲染裸 key | `app/worker/jobs.go:268,281`；`handlers/account_tasks.go:57`；`frontend/src/utils/task-types.ts` |

---

## 9. 把本文变成能验收的步（对方案 §7.1 / P4-P5 的增补）

1. **`run_reporter` 先落地**（`Success/PartialSuccess/ManualRequired/Failed` + 每 run 一条 `runs` 行，`skipped` 也落轻量行）——
   这是搬任何一条 Go 任务之前的前置，否则会复刻 §1.4 的"天天绿灯空转"。依据：R-3/R-4/T-2/T-8。
2. **传输层两条硬约束**：写操作默认单次尝试（非幂等不重试）、`ErrorKind` 补齐 §3.3 差集
   （新增 `AlreadyClaimed`/`ProductUnavailable`，`Parse` 改重试，`DuplicateWindow` 不进上游分类）。
   依据：§1.3、R-14、X-B2。**顺带**：Rust 侧自写熔断器时把半开许可归还放进 `defer`（避 R-1）。
3. **日界与超时**：`FixedOffset(+08:00)` + 启动断言 + 每 run 落 `local_date` 唯一索引；
   补 Go 完全缺失的**任务/账号/批次三级超时**（R-19）。
4. **`cloud-snapshot` 与批次排尾**：按 `todaycloud` 形状做统计能力 + `phase` 声明（Stats/Cleanup 强制排尾），
   并按 §2.1 修"只取一页 / `time.Local` / string code"三件事。
5. **抢兑**：先只搬 §3.2 分支表 + §3.3 决策表 + §3.5 调度编排（预热去重、精确开火、按 prizeID 分组 + 组内 stop），
   幂等锁用进程内 `Mutex`（**key 必须含参数维度**）+ `runs` 唯一索引，
   验证码链路给 **`ManualRequired` 出口**（Go 没有，我们必须补）。**在这一阶段前，先实测 §7 第 4/5 条**。
6. **跨任务态与清理**：`account_state(per_batch_reset)` + `pending_cleanup` 表，替代 KV + 24h TTL。依据：R-5/R-6。
7. **令牌层**：负缓存 + 预刷新 ticker + "连续失败自动停用 + 原因文案"，且**失败必须上浮**（不照搬 R-13/X-B8/R-12 的吞错）。
8. **参数与表单**：把 §5.2 那 11 个 env 参数逐个降级为 `FieldSpec` + 按账号覆盖；
   建门禁"**配置键必须有消费者**"与"**默认值只有一处**"（依据 §5.1/§5.3/C-9/C-10/C-11）。
9. **隐私与副作用红线**（Go 侧全部不合规，别继承）：事件载荷不放手机号（R-15）、
   `invitefriends` 的数据中心上报默认关闭 + 强制匿名（T-12）、`performAlbumBackup` 需显式开关（§2.5）、
   分享/上传类动作必须注册进 `pending_cleanup`（§2.5/§4.6）。
