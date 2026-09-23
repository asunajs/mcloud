caiyun 参考项目 · 参数面与取舍（§5 参数面 · §6 硬编码 · §7 冲突 · §8 搬不搬 · §9 验收步）

> 状态：与 [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md) 同一轮盘点、同一 commit（`1b2cf07`），2026-09-22 从该文件逐字拆出。决定"搬哪一条"之前读这篇。

> **本篇 2026-09-22 拆成三篇**（内容逐字未改，只按主题分段）：
> [① 总览 · 四条公共语义 · 能力逐条（§0 ~ §2）](./caiyun-go-feature-inventory.md) ｜ [② 抢兑子系统 · 运行时层（§3 ~ §4）](./caiyun-go-exchange-and-runtime.md) ｜ [③ 参数面 · 硬编码 · 冲突 · 搬不搬 · 验收步（§5 ~ §9）](./caiyun-go-parameters-and-decisions.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§0 ~ §9 连续），所以「§3.2」「§7」这类引用仍然有效，
> 只是落在不同的文件里。
> ⚠ 本文的**"Go 侧"指参考项目 caiyun**，不是本仓库；说本仓库时会明写"本仓库"。

---

## 5. 参数面全量（一张表看清"三套存储 + 一套死系统"）

### 5.1 配置存储四份

| 存储 | 内容 | 判定 |
| --- | --- | --- |
| **env** | 170 处读取，**构造期与调用期混用**，core 层完全靠现读 | 业务参数全在这一层 ⇒ **不搬这个做法** |
| `system_configs` KV 表 | 种子 15 行，**其中 12 行无人读**（`task_concurrency`/`task_schedule`/`token_cache_ttl`/`exchange_schedule_time_1\|2`/`exchange_update_time`/`exchange_enable_priority`/`exchange_default_timeout`/`exchange_max_global_concurrency`/`exchange_auto_retry_failed`/`exchange_log_retention_days`/`product_categories`） | **不搬**；`exchange_enabled` 更是**只有展示消费者 = 死旋钮，且缺省 fail-open** |
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
| 签名盐 | saltKey `1`/`2` 两把（应用级白盒常量，非账号凭据） | 我们已有：`../mcloud-rust/crates/mcloud-core/src/crypto.rs:43` `SIGN_SALT_MAP` |
| 阈值 | 熔断 10/30s/半开 5；JWT 失败自动停 3；令牌窗口 2/5/2min；`stale running` 15min；滑块重试 3；AI 抽奖 5；funai 抽奖 5 | §4.3、§4.4、§3 |
| 魔法数 | 月卡奖品 `1001`；`alreadyDrawn = 50 - surplus`；配额 `20`；`OffsetJitter=±3`；`MaxMultipartSize=8MiB` | `services/exchange_monthly_service.go:119-130`、`core/tasks/wechat.go:121`、`core/tasks/invitefriends.go`、`services/exchange_executor.go:141`、`app/api/config.go:56` |

⇒ 落点同旧 TS 盘点篇 §4（集中到端档案层与纯计算层。⚠ 本文的"Go 侧"指的是**参考项目 caiyun**，
所以这里明写"本仓库"：`internal/upstream` 的 `ClientProfile` / `AppProfile`
+ `internal/core` 的常量模块，见 [`client-profiles.md`](./client-profiles.md)），
并遵守 [`docs/device-and-source-codes.md`](./device-and-source-codes.md) 的一句话判据：
**能在同一端两个接口上取不同值的码，不许钉进端档案**（caiyun 的 `clientVersion` 三个值并存就是反例）。

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
| 6 | **数美 deviceId 是否进原生头** | Go 发 `deviceId` 头 + `.thumbcache_*` cookie（活动/H5 域） | **沿用上文已核对的结论**：我们那批 App 抓包里原生请求头 `deviceid` 一次都没出现，带 `deviceId` 的 178 条夹具**全是 H5 / WebView**。所以这条**不是"补上 deviceid"**，而是等一条**原生**抓包再逐端点判断（详见 `../mcloud-rust/docs/rust-rewrite-plan.md` §7 的 smid 段与 `docs/client-profiles-fingerprint-and-landing.md` §5.4 ③） |
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
   ⭐ **2026-09-22 补记（本项目真机实测）**：验证码这一层的**设计空间比 §3.6 记的宽**。
   §3.6 记的是参考项目的**二元**选择（远端付费识别 / 识别失败即 `failed`），
   而本项目已实测出**第三条路：本地自研解算** —— Go 侧两个互相独立的算法在 4 张真实题面上
   彼此一致（证据图上前 5 名得分**逐字复现**原项目 `matchSlide`），且**真机上过了滑块闸**
   （`0` 偏差的提交回的是**业务码** `407`，不再是 `514`）。
   ⇒ 本条"给 `ManualRequired` 出口"**仍然要做**（它是**兜底**，见 §3.6 第一行那张表），
   但它**不再是唯一出路**。⚠ 两条约束：① 参考项目**本地无 CV** ⇒ 这块**没有可"搬"的实现**，
   是**自研**（§3.9 第 3 行）；② 解算器的**容忍度约 ±4~9px**（`0` 过 / `+10` 拒）⇒
   **不要加抖**。判据与实拍见 [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt) §5.3.1。
6. **跨任务态与清理**：`account_state(per_batch_reset)` + `pending_cleanup` 表，替代 KV + 24h TTL。依据：R-5/R-6。
7. **令牌层**：负缓存 + 预刷新 ticker + "连续失败自动停用 + 原因文案"，且**失败必须上浮**（不照搬 R-13/X-B8/R-12 的吞错）。
8. **参数与表单**：把 §5.2 那 11 个 env 参数逐个降级为 `FieldSpec` + 按账号覆盖；
   建门禁"**配置键必须有消费者**"与"**默认值只有一处**"（依据 §5.1/§5.3/C-9/C-10/C-11）。
9. **隐私与副作用红线**（Go 侧全部不合规，别继承）：事件载荷不放手机号（R-15）、
   `invitefriends` 的数据中心上报默认关闭 + 强制匿名（T-12）、`performAlbumBackup` 需显式开关（§2.5）、
   分享/上传类动作必须注册进 `pending_cleanup`（§2.5/§4.6）。
