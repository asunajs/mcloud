caiyun 参考项目 · 抢兑子系统与运行时层（§3 抢兑 · §4 运行时）

> 状态：与 [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md) 同一轮盘点、同一 commit（`1b2cf07`），2026-09-22 从该文件逐字拆出。要动抢兑或运行时那一层之前读这篇。
>
> ⚠ **2026-09-22 追加了 §3.9**（本项目实拍 × 参考项目做法：逐条对照）。
> 那一节是**新写的**，**只做对照、不下"搬/不搬"的结论** —— 那类判断的唯一权威在
> [`caiyun-go-parameters-and-decisions.md`](./caiyun-go-parameters-and-decisions.md) §8；
> §3.1 ~ §3.8 与 §4 全部**逐字未改**。

> **本篇 2026-09-22 拆成三篇**（内容逐字未改，只按主题分段）：
> [① 总览 · 四条公共语义 · 能力逐条（§0 ~ §2）](./caiyun-go-feature-inventory.md) ｜ [② 抢兑子系统 · 运行时层（§3 ~ §4）](./caiyun-go-exchange-and-runtime.md) ｜ [③ 参数面 · 硬编码 · 冲突 · 搬不搬 · 验收步（§5 ~ §9）](./caiyun-go-parameters-and-decisions.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§0 ~ §9 连续），所以「§3.2」「§7」这类引用仍然有效，
> 只是落在不同的文件里。
> ⚠ 本文的**"Go 侧"指参考项目 caiyun**，不是本仓库；说本仓库时会明写"本仓库"。

---

## 3. 抢兑子系统（Go 项目真正的重头，也是我们错误模型的对照源）

`../mcloud-rust/crates/mcloud-core/src/error.rs:1-10` 已经对齐了它的两条约定（错误分类决定重试策略；对外绝不回传 `err.Error()`）。
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

**与 Rust `ErrorKind`（`../mcloud-rust/crates/mcloud-core/src/error.rs:13-40`）的差集，必须逐条决定**：

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
| 取 deviceId 失败用**硬编码兜底 deviceId**（`services/exchange_executor.go:27-29`，全服务共用同一个假设备号） | 风控负资产 ⇒ **不搬**；我们已有账号绑定的 `smid_fingerprint`/`smid_device_id`（v5 引入，schema 现为 v9） |

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

### 3.9 本项目实拍 × 参考项目做法：逐条对照

> **2026-09-22 新加的一节**，正文 §3.1 ~ §3.8 **逐字未改**。
>
> **先把三个角色分清**（本节与 `capture/app-exchange-samples.txt` §6.1 都按这套叫法）：
>
> | 角色 | 是谁 | 它的盘点在哪 |
> | --- | --- | --- |
> | **原项目** | `mcloud-sign`（**TypeScript**）—— 本项目的前身 | [`business-feature-inventory.md`](./business-feature-inventory.md)（文中简称"旧 TS 盘点篇"） |
> | **参考项目** | **caiyun**（**Go** + Vue3，`3238614968/caiyun`，commit `1b2cf07`）—— 一个**外部**的、上游同域的实现 | 本篇 + `caiyun-go-*.md` 另三篇 |
> | **本项目** | mcloud（**Go**）—— ⚠ 转 Go 的动机是**编译体验**（`cargo check --workspace` 实测 `real 118.71`，见 [`go-migration-plan.md`](./go-migration-plan.md) §1），**与参考项目用什么语言无关** | 抓包在 [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt)（⚠ 本文别处也叫"本仓库"，同一个东西） |
>
> ⚠⚠ **"本项目也是 Go"不构成与参考项目的任何关系** —— 不是同源、不是同族、
> 也不因为语言相同就"更好搬"。⚠ 同理，本文里的"**搬**"指的是
> **参考项目（caiyun）的做法要不要采用**（§8 的题），**不是**本项目 Rust → Go 的那次迁移。
> 可对照的只有**上游协议边界**（线上字节 / code / 字段名 / 时序），与实现语言无关。
>
> 本项目在真机 App 上抓到了 4 轮兑换（3 错 1 成），逐字实拍与判据链在
> [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt) §5.2 / §5.3 / §5.4。
> 本节**只做对照、只摆事实** —— 即"**它怎么做 / 我们实测是什么 / 差在哪**"。
> ⚠ **"搬 / 不搬 / 先修再搬"的结论不在本节下**，那类判断的唯一权威是
> [`caiyun-go-parameters-and-decisions.md`](./caiyun-go-parameters-and-decisions.md) §8；
> 本节与它冲突时以它为准。**别把这里的事实读成裁决。**
> ⚠ 本文的"Go 侧"仍指参考项目 caiyun，不是本项目。

| # | 参考项目（`exchangeV2`）怎么做 | 本项目实拍（`exchangeV3`） | 对照结论 |
| --- | --- | --- | --- |
| 1 | `GET …/exchangeV2?prizeId=…&puzzleOffset=…&smsCode=` —— **写操作走 GET**（§3.1 第 7 步） | `POST …/exchangeV3` + JSON body（`content-type: application/json;charset=UTF-8`） | 两代接口的**方法就不同**：`V2` = GET + query，`V3` = POST + JSON。⚠ 这不只是写法差别 —— §3.4 那张"重复扣云朵成立条件"表里，**"上游超时但实际成功"那一行是长在 GET 上的**；本仓库是 POST，**前提不同** |
| 2 | 成功判定**只看 `msg EqualFold "success"`**（§3.2）；且抢兑域与商品列表域是**两套**成功定义（§3.2）。⚠ 原项目那边更乱：同一代码库混用**三套**信封（旧 TS 盘点篇 §1.2） | 成功 `{"code":0,"msg":"success"}`；失败 `{"code":514,"msg":"验证失败"}` | 两边判据**不同源**：它判 `msg`，本项目 H5 调用点判 **`code`**（逐字见 `capture/app-exchange-samples.txt` §1.3 的 code 表）。⚠ 本次实拍两者**恰好同向**（成功时 msg 也是 `success`）—— 那是巧合，**不是等价** |
| 3 | 滑块**本地无 CV**：取题后把 `{puzzle,picture}` POST 给**远端付费识别**（`{CAIYUN_SMS_API_BASE_URL:-https://ydyp.apisky.cn}/api/sms/solve`，取返回的 `data.offset`）（§3.6） | `puzzleOffset` 实测 = **拼图块左缘在底图 680×400 里的 x**；**本地图像算法算得出来**（两个独立探针在 4 张图上**互相一致**；⚠ "与真值差 4px"**只对唯一成功那笔成立** —— 量出 `140`、提交 `144`。另外三条的"真值"无从得知，它们只能证明两个探针**彼此**一致） | 它给了一条"**不写 CV 也能长期跑**"的现成路子（代价是外部依赖 + 每次重试重付费）；本仓库实测则说明"**自己算**"也可行。⚠ 两条路的**精度口径不一样**：它拿的是第三方给的 offset，我们是自己算的。⚠⚠ **2026-09-22 补记（纠正本行原文）**：本行原写"我们的 4px 恰好压在容忍度下界（`[4, 41)`）上"—— **那个 `[4, 41)` 已被真机实测推翻**（`0` 偏差也通过、`+10` 被拒 ⇒ 容忍度约 **±4~9px**），见 [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt) §5.3.1。⭐ 另有一条**结构性**事实：**它本地没有任何 CV** ⇒ 本项目那个 Go 侧解算器**在参考项目里没有可对位的实现**（§8 若决定落地成正式能力，是**自研**，不是"搬"） |
| 4 | `finalOffset = offset + rand(-3..3)` 抖动（§3.6） | 唯一成功那笔：提交 `144`、量出 `140`（差 4） | 两边**落在同一个量级**（个位数像素）；差 4px 那笔**过了** ⇒ 这个量级**能过**。⚠ 但只有**一条**样本，别把它读成"安全区间"。⚠ 它加抖动是为**规避风控**（让每次提交的值不同），**不是精度补偿** —— 别把它当成容错机制来理解。⭐⭐ **2026-09-22 补记：两个独立来源互相印证。** 它抖 `rand(-3..3)`（幅度 ≤3px），本项目真机实测的容忍窗口是 **±4~9px** ⇒ **它的抖动幅度落在窗口之内**。两边是**独立来源**（它的线上经验 vs 我们的真机实测），**不是**谁从对方推出来的 ⇒ 这是本节里少见的"两边真能对上"的对照。⚠ 但"落在窗口内"**不等于**它是容错机制：本项目实测 `0` 偏差**直接过**、`+10` 才被拒 ⇒ **我们不需要抖**（§5.3.1 ③） |
| 5 | 它的分类器吃的是**渲染后的失败消息文本**（含 `http_status=` / `body=`），**不是码**；本仓库实拍那句 `验证失败` 在该表的 12 类子串里没有对应项 ⇒ 按文本落 `other` ⇒ **默认重试**；且"每次重试重新拉一次滑块、重新识别"（§3.3 / §3.6） | `514` = `验证失败`；**三条 `514` 期间余额一直是 10505 ⇒ 不扣豆**；H5 侧 `setTimeout(oe, 500)` 是**重新取题** | ⭐ **两边"取题 → 发 → 答错 → 重取题"这个循环是一致的**（本项目 4 组实拍的记录号交替，见 `capture/app-exchange-samples.txt` §1.7）。⚠ 但**取题的主路径是"兑换前"**（它第 5 步在下单第 7 步之前；本项目每条 `exchangeV3` 之前都有一条 `getSlide`）—— 重试路径上再取一次是**附加的**，不是取题的**唯一入口**。再加上本仓库实测 **`514` 不扣豆** ⇒ 这一档重试的**代价只有一次请求**。⚠ 但"安全"**仅对 `514` 成立** —— §3.4 表里"上游超时但实际成功"那条路，本仓库**没有实证** |
| 6 | 重复兑换靠**上游文案**兜（stop 白名单 `已兑完 / 已兑换 / 今日已兑换 / 本月已兑换 …`，§3.2） | 服务端侧有**可查的状态**：同一件商品成功后 `exchangeList` 的 `limit` 由 `0` → `2`（已兑换），`updateTime` 随之刷新 | 它**只能**靠文案（因为它那条线没有可查的个人状态）；本仓库实测**多一条可查的服务端状态**（`limit`）。⇒ 事实层面：它 §3.4 表里"上游超时但实际成功"那一行**缺的判据**，本仓库这条线**有**（它没有，不代表我们该怎么用 —— 用法不在本节） |
| 7 | `clientVersion` 在**参考项目自己那里**就**四值互相矛盾**（`13.0.0` / `12.5.3` / `12.5.4` / `12.4.0`，§6 与 X-B10） | 实拍恒为 **`12.6.0`**（= `upstream.CaiyunAppVersion`），而 `appversion` 头是另一个值 `12.6.0.0`（四段） | 同一个字段名，**两边取值不同**（`13.0.0` vs `12.6.0`），且参考项目内部就四值矛盾 ⇒ **两边没有任何可对位的关系**（它内部那份矛盾见 X-B10，那是它自己的缺陷） |
| 8 | 数美设备指纹 → `B` 前缀 `deviceId` + `.thumbcache_*` cookie（§3.6） | 抓包里 `exchangeV3` 带 `deviceId`（`B` + 88 字符），且 cookie 里**确实有** `.thumbcache_<md5(organization)>`（形状与 [`capture/app-signin-samples.txt`](./capture/app-signin-samples.txt) 记的一致；具体值已按 [`sanitize-guide.md`](./sanitize-guide.md) 换成形状） | ⭐ **两边是同一套**（交叉印证）。⚠ 它"取不到就用**硬编码兜底 deviceId**"（§3.6）那半句是它自认的风控负资产，读的时候别把它当成"可行做法" |
| 9 | 验证码这层**没有"请你去 App 点一下"的出口**（§3.6 表第一行；第四态缺失见 §1.4） | 本仓库的 `core.RiskLevel` / `Capability.NeedsConfirmation()` 已就位 | 它那边"算不出来就只能 failed"（全仓"人工/手动"字样在兑换侧只命中一处、且是**任务级**的）；本仓库**已有第四态的载体**，它**没有** |

⭐ **2026-09-22 第四轮补一条实测**（**只摆事实**，不下"搬 / 不搬"的结论）：

参考项目 §3.2 的 stop 白名单里有 `已兑换 / 今日已兑换 / 本月已兑换`；
而本项目实测**"同一账号 + 同一 `prizeId` 再发一次"**回的是

```json
{"code":2301,"msg":"重复兑奖"}
```

（逐字样本见 [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt) §5.5）

⚠ **两边的"文案"对不上** —— 而它的分类器吃的是**渲染后的失败消息文本**（§3.3 / X-B6）
⇒ `重复兑奖` 在它那 12 类子串里**一个都不命中** ⇒ 落 `other` ⇒ **默认重试**。
⇒ 事实层面：**"靠上游文案兜"这条路，在换了一代接口之后文案就会变**；
而它的分类器**没有码可依据**（本项目的 `code` 是**数值**、可判等）。
⚠ 这一条**不构成**"我们该怎么做"的结论 —— 那类判断的唯一权威是
[`caiyun-go-parameters-and-decisions.md`](./caiyun-go-parameters-and-decisions.md) §8。

⭐ **2026-09-22 第五轮再补一条**（同一族，仍只摆事实）：

本项目那 4 件"已兑"的奖品里，`limit` 有两种取值 —— `2`（**这件**已兑）与 `4`（**同组别的件**已兑）；
而 `4` 的**字段依据**是 `probability2` 里的 `monthGroupLimit`（**数组** —— 列出"每月只能兑其中一件"
的那组 prizeId）+ `limitMonthQuantity`（逐字样本见 [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt) **§5.6**）。

⚠ 与上面那条同一个形态：**"已兑"这个状态，上游在列表侧（`limit`，数值）与下单侧（`2301`，四位码）
用了两套表达，且两边都不带文案** ⇒ 谁照参考项目那样"吃渲染后的失败文案"，换一代接口就会失配。
⚠ 仍然**不下"搬 / 不搬"的结论**。

**读它代码时顺手记下的一族坑**：X-B1（重试自锁）、X-B2（本地锁消息与上游拒绝共用一张子串表）、
X-B6（拿**渲染后的消息**做分类）、X-B7（stop 白名单里孤零零一条 `"不足"`）、
X-B16（锁 key 用 `AccountID==0 ? ID : AccountID` ⇒ 跨用户共享锁）。
逐条证据在 [`caiyun-go-parameters-and-decisions.md`](./caiyun-go-parameters-and-decisions.md) §8.4 的 X 段（**那里是权威，这里不重复**）。
⇒ 值得记的是它们的**共同形态**：「**把不同来源的东西塞进同一个判据**」——
与 [`migration-pitfalls.md`](./migration-pitfalls.md) **#101**（判据在退化的输入上不报错、只给错答案）**同族**。

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
