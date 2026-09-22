caiyun 参考项目 · 抢兑子系统与运行时层（§3 抢兑 · §4 运行时）

> 状态：与 [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md) 同一轮盘点、同一 commit（`1b2cf07`），2026-09-22 从该文件逐字拆出。要动抢兑或运行时那一层之前读这篇。

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
| 取 deviceId 失败用**硬编码兜底 deviceId**（`services/exchange_executor.go:27-29`，全服务共用同一个假设备号） | 风控负资产 ⇒ **不搬**；我们已有账号绑定的 `smid_fingerprint`/`smid_device_id`（v5 引入，schema 现为 v8） |

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
