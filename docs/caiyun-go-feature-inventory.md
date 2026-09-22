# Go 参考项目（caiyun）待实现业务功能盘点（§0 一页总览 · §1 四条公共语义 · §2 能力逐条）

> **定位**：本文是 [`docs/business-feature-inventory.md`](./business-feature-inventory.md)（旧 TS 项目 `mcloud-sign` 盘点）的**对照篇**，
> 对象是方案 §2 里点名的参考项目 [3238614968/caiyun](https://github.com/3238614968/caiyun)（Go + Vue3），
> clone 于 `/tmp/caiyun-ref`，本文所有行号对该 commit 有效：**`1b2cf07`**。
> 体量：`backend/` 337 个 `.go` / 61,215 行；`internal/core/tasks/` 30 个任务文件；`internal/services/exchange_*.go` 25 个。
>
> **它不替代**[`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md)：方案定"做什么、按什么风险顺序做"（§7.1），
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

> **本篇 2026-09-22 拆成三篇**（内容逐字未改，只按主题分段）：
> [① 总览 · 四条公共语义 · 能力逐条（§0 ~ §2）](./caiyun-go-feature-inventory.md) ｜ [② 抢兑子系统 · 运行时层（§3 ~ §4）](./caiyun-go-exchange-and-runtime.md) ｜ [③ 参数面 · 硬编码 · 冲突 · 搬不搬 · 验收步（§5 ~ §9）](./caiyun-go-parameters-and-decisions.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§0 ~ §9 连续），所以「§3.2」「§7」这类引用仍然有效，
> 只是落在不同的文件里。
> ⚠ 本文的**"Go 侧"指参考项目 caiyun**，不是本仓库；说本仓库时会明写"本仓库"。

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

⇒ 新落点不变：`core.FieldSpec` → `capabilities.All()`（`internal/capabilities/registry.go`），
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
  ⇒ 这是**端档案（`docs/client-profiles-fingerprint-and-landing.md`，设备串字段表在 §5.2 / §5.3）**该记的形状，不要在每个能力里重拼。

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

**两套 query-string MD5 签名**（与 `../mcloud-rust/crates/mcloud-core/src/crypto.rs:43` 的 `SIGN_SALT_MAP` 可对照）：
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

> 2026-09-22 补：本仓库（不是 caiyun）这个"应是"已经有具体形状 —— 风险声明是 `core.RiskLevel`
> （不可逆消耗那一档叫 `core.RiskHigh`），是否要二次确认由
> `capabilities.Capability.NeedsConfirmation()` 从风险派生（`internal/core/model.go` 的
> `RiskLevel.NeedsConfirmation`），前端据此弹确认并显示影响面。
> ⚠ 上面那句批评**还没被验证过、也还没被推翻**：本仓库当前**没有**这批动作的任何实现
> （全仓 grep `createSnapshot` / `shareNewFile` / `createTempNote` 只命中无关的枚举名与
> 签到请求头），它们所服务的活动任务（`hc1t` / `play-ai-*` / `kouling` 等，
> 见 [`business-feature-inventory.md`](./business-feature-inventory.md) #14–#19）也仍是"无"。
> ⇒ 真正搬这一批时，按 §2.5 表里"风险"那一列逐条给开关，别先把公共静默库抄过来。
