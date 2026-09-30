# 任务实现清单（待确认）

> 来源：`mcloud`（Go，19 条能力已全部实现）+ `mcloud-sign`（旧 TS，权威清单 `TASKS.md`）
> + `mcloud/docs/business-feature-inventory.md`（逐条实现规格，含 `path:line` 证据）。
>
> 本项目现状（2026-09-26）：**已实现 10 个任务** —— P1 签到族全部落地
> （`sign` / `receive` / `multiple` / `backup-gift` / `revival` / `prizes` /
> `cloud-snapshot` / `task-list`），端点全部换成**实拍**的
> `/ycloud/signin/page/*`（不再是 405 的占位）；
> **P4 直播小红花两条**（`live-room` / `red-flower`）已在同一天落地，
> 协议层按黄金向量**字节级**对拍（见 `test/golden/live-room-vectors.json`、
> `live-redflower-vectors.json`）。
> `/api/capabilities` 实测返回这 10 条。
>
> **增补（2026-09-28）：已实现 18 个任务** —— 上面 10 条 +
> `exchange`（兑换）+ `msg-push`（消息推送）+ `fun-ai`（趣玩AI抽奖）+ `notice`（公告任务）
> + `play-ai-special`（拍拍盘点，**只读那一半**）
> + `app-task-matrix`（App 任务矩阵，**只读那一半**）
> + `task-expansion`（任务拓展，**全链**：读 → 凑备份 → 等 → 重读 → 领 → **删自己传的那个文件**）
> + `app-task`（App 任务**做任务**，写侧：106 / 522 / beiyong1 名字门三档玩法 → 逐条打卡 → 清理）。
> ⇒ 下面 §2 的 **P2「只读查询」那一档已经全部落完**（那四条的实拍材料都拿到了）。
> 下面那张 19 条清单是**参考实现的规格表**，不是本项目的进度表；
> 本项目的进度以 `/api/capabilities`（今天 18 条）与 `README.md` 为准。
>
> ⚠ **`task-expansion` 与参考实现有一处刻意不同：本仓会删掉自己传的那个文件**
> （参考实现留在它的 P1 缺口里不删）。⚠ 但"撞同名"那一档**绝不删** ——
> 那是用户本来就有的文件。详见 `crates/mcloud-tasks/src/task_expansion.rs` 文件头。
>
> ⚠ **`app-task-matrix` 与表里的 `app-task` 不是同一条**：后者在参考实现里是
> 「**做任务**」（逐条 `task/click`，会写用户的账且不可撤销），
> 而它自己写着「今天一次真打都没有」、`RunInBatch: false`。
> 本仓只落了**只读那一半**并另起一个 code ⇒ **`app-task` 这个名字今天不存在**，
> 写的那一半仍未实现。详见 `crates/mcloud-tasks/src/app_task_matrix.rs` 文件头。
>
> ⚠ **`play-ai-special` 只落了只读那两枪**（`user/info` + `task/list`），
> 上游那一族的另外三口（上报一步 / 抽奖 / 写网盘）**故意不接**：
> ③ 卡在"埋点零实拍"、④ 卡在"判据语义未验证且不可回滚"、⑤ 卡在"真写用户网盘 + 清理链冻结"。
> 另外这一族的**请求头比仓库其余 H5 接口少两个**（没有 `appversion` / `activityid` /
> `deviceid` —— 同归档里门户那四条都带，所以是这一族真的没发）⇒ 它**不复用** `h5()`。
> 详见 `crates/mcloud-upstream/src/app_play_ai.rs` 文件头。
>
> ⚠ **`notice` 的判据是猜的，别当结论读**：`cloudStatus` 哪个值读作"未领"
> **没有正面样本**（实拍两槽读数方向分不清），`noticeNeedsManual` **恒 false**
> （"未开启"那一档今天**不可达**，因为"未开启"态的响应**零样本**），
> 而实拍里**没有** `taskListV2`（旧 TS 说有）⇒ 参考实现选择"按实拍最强的那条裁"。
> 详见 `crates/mcloud-tasks/src/notice.rs` 文件头。
>
> ⚠ **`fun-ai` 的每日上限是本工具定的**（`FUNAI_DAILY_DRAW_LIMIT = 1`），
> **不是**上游规则 —— 上游"每日最多 5 次"说的是**做任务能挣到几次**。
> 另外三个计数**跨天清不清零未测试**。
>
> ⚠ **`msg-push` 的证据分两半**：`task/status` 是实拍逐字的
> （裸 GET、零查询参数、六个字段名都见过），而 **`task/obtain` 那一枪整条零样本**
> —— 实拍那天 `secondTaskStatus=1`（进度 16/31 天），页面上根本没有领取入口。
> 所以它的**方法都还没定**（常量在
> `mcloud_upstream::app_msg_push::MSG_PUSH_OBTAIN_IS_POST`），
> 且"什么值该做什么"那层判据**仍是旧项目口径、一条都没被上游证实过**。
> 详见 `crates/mcloud-upstream/src/app_msg_push.rs` 文件头。
>
> ⚠ **直播那两条的边界**（详见 `README.md`「状态」）：
> **开播门已接**（2026-09-26，换 SSO → 换直播侧 JWT → 房间列表），
> 所以"不填 `codes` 就自动开门听弹幕收口令"这条链**在代码上通了、有端到端单测**；
> 但**真实开播窗口没抓到** ⇒ 真实 WS / 真弹幕**一次都没跑过**
> （上游那一段用假收集器替换，验的是编排而不是"对面认不认我们"）。
> 帧顺序与 `command` 取值等四条证据等级是**「原项目实现，零实拍」**。
> **不要把"测试全绿"读成"直播那条链已真机验证"。**

---

## 0. 先说结论：真正的阻塞点不是任务数量

我原本以为"19 个任务要一个个写"。核对后发现**顺序被一个东西卡住**：

### ⛔ 阻塞点一：`jwttoken` 不是"一个接口"，是**每页一套配方**

我第一版把这条写成"取一次 JWT 然后复用"—— **这是实质性低估**。
核对 mcloud 的抓包后，真实形态是：

> **每个页面用自己的参数换自己的票，票与票之间逐字节不等。**

#### 实测的配方差异（来自 `mcloud/docs/capture/`）

| 页面 | `marketName` | `sourceId` | 抓到没 |
|---|---|---|---|
| 签到页 | `sign_in_3` | `1002` | ✅ 完整实拍 |
| 签到扩展 | `sign_in_3_ex` | — | ✅ |
| 备份有礼页 | `backupgift` | `1134` | ✅ **完整实拍**（明确证"不复用签到页那张票"） |
| 开启通知页 | `msgpushon` | `1062` | ✅ **完整实拍** |
| 拍拍门户页 | `National_playAI` | `1044` | ✅ 完整实拍 |
| 拍拍子页 | `shake_shake` | `1314` | ⚠️ **零实拍**（四键是旧 TS 主张） |
| 小红花页 | `National_Activity` | — | ⚠️ 未证明有特有取法 |
| 兑换页 | `sign_in_3_ex` | — | ⚠️ |
| 直播页 | `National_LiveRoomFeedback` | — | ⚠️ |

mcloud 实测的三张票长度相同（905 字符）但**哈希互不相等**，且各自被用在**不同**接口上。

#### 结构差异不只是"取值不同"

签到页那次带**三个签名头**（`x-request-id` / `x-nonce` / `x-signature`）+
一个必须带 `targetSourceId` 的 `referer`；而 `msgpushon` 那次的差异
"**只在 `marketName` 与 `sourceId`**"。也就是说：**头和体的形状也可能不同**。

#### mcloud 为此建了一整层（不是顺手写个函数）

| 文件 | 行数 | 职责 |
|---|---|---|
| `app/signin_session.go` | 284 | 会话源：三枪建票、`90001` 刷新重放 |
| `app/signin_session_pool.go` | 171 | **会话池**：按 `(账号, 配方, 凭据指纹)` 持有，跨轮复用 |
| `upstream/app_signin_session.go` | 211 | 三枪的上游构造 |
| `upstream/app_play_ai_session.go` | — | 拍拍的**另一套**建会话 |
| 测试 | 1009 | |
| **合计** | **≈ 2943** | |

**核心抽象是 `SessionRecipe`**：键不是"页面名"，而是"换票那三枪用的参数"。
理由（mcloud 原话）：

> 真正决定一条会话长什么样的是**换票那三枪用的参数**（`toSourceId`、referer 那个页面、
> `activityid` 那一族），不是页面上有个什么按钮。

还有三条设计决定值得直接抄：

1. **作用域是"页面"不是"一轮"** —— 用户短窗口连点三次签到，真实客户端是
   "页面还开着，票就用到底"，所以三次共用一张票。键必须比"一轮"活得久。
2. **不设 TTL** —— 抓包只给了"2 小时即废"，**没给**"该在哪个时刻算它过期"的依据。
   所以失效判据只有服务端回的 `90001`，不自造时钟。代价是自然过期后第一次点击
   多一次往返（刷新+重放），不是失败。
3. **键里要带登录凭据指纹** —— 否则 `Authorization` 轮换后，旧会话源再建票必然失败，
   而失败会被误读成"这张票刷不动"。

#### 这对本项目意味着

`crates/mcloud-upstream/src/client.rs` 现在只有一个 `jwt_token: Option<String>` 字段 ——
**装不下这个模型**。需要的是：

- `SessionRecipe` 枚举/配置（每页一组换票参数）
- 会话池（键 = 账号 × 配方 × 凭据指纹）
- `90001` → 刷新 → **重放那一条**（不是重跑整个任务）
- 每页的建会话请求构造

> ⚠️ 关键：**重放的是"那一条请求"**，不是整轮。mcloud 为这个专门写了测试
> （`TestRedFlowerChainRefreshesAndReplaysTheFailedStep`），因为"重放整轮"
> 在余额上看不出、日志里不报错 —— 只有把**请求序列**钉住才抓得到。

### ⛔ 阻塞点二：`sign` 的端点本身是错的

```rust
// crates/mcloud-tasks/src/sign.rs
HttpRequest::post("https://m.mcloud.139.com/ycloud/api/sign/query")   // 405
HttpRequest::post("https://m.mcloud.139.com/ycloud/api/sign/doSign")  // 405
```

mcloud 的实拍结论（`business-feature-inventory.md` §5.1）明确写着：

> 真实抓包是 `https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app&deviceId=…`、
> `POST …/ycloud/signin/task/taskListV3`、`POST …/ycloud/signin/page/doTaskPost`

即：**前缀 `/market/` vs `/ycloud/`、`taskListV2` vs `V3`、`doTask` vs `doTaskPost` 都要按端逐条实测**。

---

## 1. 19 条能力全清单（按 mcloud 注册表）

**关键字段说明**：
- **批量** = `RunInBatch`，能否进编排（`false` 多有副作用理由）
- **隐藏** = `Hidden`，不出现在通用任务列表
- **错过** = 定时任务错过触发点时的兜底（`catch-up` 补跑 / `skip` 放弃）

| # | code | 标题 | 类型 | 批量 | 隐藏 | 错过 | 副作用 | 依赖 |
|---|---|---|---|---|---|---|---|---|
| 1 | `sign` | 每日签到 | Action | ✅ | | catch-up | 记账 | jwt |
| 2 | `receive` | 领取AI豆 | Action | ✅ | | catch-up | 记账 | jwt |
| 3 | `multiple` | 签到翻倍 | Action | ✅ | | catch-up | 记账 | jwt |
| 4 | `backup-gift` | 备份好礼 | Action | ✅ | | catch-up | 记账（**GET 却是写**） | jwt |
| 5 | `revival` | 用复活卡/回馈卡 | Action | ❌ | | skip | **不可逆** | jwt |
| 6 | `prizes` | 待领取奖品 | Query | ❌ | | catch-up | 无 | jwt |
| 7 | `task-list` | 任务清单 | Query | ❌ | | catch-up | 无 | jwt |
| 8 | `notice` | 开启通知领AI豆 | Action | ✅ | | catch-up | 记账 | jwt |
| 9 | `app-task` | App 任务（做任务） | Action | ❌ | | skip | **上传/建删外链/发邮件** | jwt |
| 10 | `fun-ai` | 趣玩AI抽奖 | Action | ✅ | | catch-up | 记账 | jwt |
| 11 | `msg-push` | 消息推送任务 | Action | ❌ | | skip | 记账 | jwt |
| 12 | `cloud-phone-redpack` | 红包派对盘点 | Query | ❌ | | catch-up | 无（**只读那一半**；写入侧 7 条待接） | 活动站票（`querySpecToken` → `tokenValidate`，**与 jwt 无关**） |
| 13 | `play-ai-special` | 拍拍盘点 | Query | ❌ | | catch-up | 无（**只读那两口**） | jwt + 活动会话 |
| 14 | `ai-avatar` | AI 头像生成与抽奖 | Action | ❌ | | skip | **往网盘写文件** | jwt |
| 15 | `task-expansion` | 任务拓展 | Action | ❌ | | skip | **往网盘写文件** | jwt |
| 16 | `live-room` | 直播口令 → 小红花 | Action | ❌ | | skip | 入群足迹 + **WS 长连** | jwt + WebSocket |
| 17 | `exchange` | 兑换奖品 | Action | ❌ | ✅ | skip | **真扣AI豆、不可回滚** | jwt + 滑块 |
| 18 | `cloud-snapshot` | AI豆余额快照 | Query | ❌ | | catch-up | 无 | jwt |
| 19 | `auth-refresh` | 刷新 auth 凭据 | Action | ❌ | ✅ | catch-up | 换 token | basic（**唯一不需要 jwt 的写操作**） |

> **隐藏的只有两条**：`exchange`（有自己的奖品页与抢兑规则）与 `auth-refresh`（只给调度指向）。
> 其余 17 条都在通用任务列表里可见。已逐条核对源码，不是解析推断。

**mcloud 侧 19 条全部有实现**（`grep` 无 `ErrNotImplemented` 残留）。
但注意：**"实现"不等于"实测跑通"** —— 很多分支标注"零实拍"。

---

## 2. 按依赖分档（这是建议的实现顺序）

### 🅿️ P0 — 地基（不做完，后面全是空的）

| 项 | 内容 | 判据 |
|---|---|---|
| P0-1 | ~~**会话层**：`SessionRecipe` + 会话池 + `90001` 刷新重放~~ | ✅ **已完成**（`mcloud-upstream::session` / `session_source` / `h5_client`）|
| P0-2 | **`auth-refresh`**：`authTokenRefresh.do`（XML 体、正则取 `<token>`）+ 三档判据 | ✅ **已完成**，但**换了端点**：实拍走 `user/auth/refreshToken` JSON 体；`authTokenRefresh.do` 是零抓包的旧 TS，**刻意不实现**。`expire<0` ⇒ `ManualOnly`，不重试 |
| P0-3 | ~~**端点校正**：按真实抓包逐条核对 `/market/` vs `/ycloud/`~~ | ✅ **已完成**（P1 八条的端点全部换成实拍 `/ycloud/signin/page/*`）|
| P0-4 | **产物账本**：run 名下文件清单 + 失败也清理 | ✅ **账本层已完成**（`mcloud-engine::artifacts`，34 条用例）。✅ **`batchTrash` 协议层已完成**（`mcloud-upstream::cloud_trash`，10 条用例，2026-09-28）。**仍缺**：真正**调用**它那一枪 —— 要等第一条写网盘的链（P3）落地才接得上，见下 |

> **P0-4 的现状要说准**（2026-09-26）：
>
> **已完成**的是**账本本身**：`ArtifactLedger`（记账 / 销账 / 按账号切清单 /
> 交回未清项）+ `ArtifactRecorder`（任务侧窄句柄，`run_id` 与 `account` 自动填）
> + 执行器收尾（把剩余清单写进 `RunRecord.details.artifacts` 并发 warn）。
> 它修掉的正是旧项目那个缺口：**只念"留了 N 个"却不知道是哪 N 个**。
>
> **仍缺两件**，都不是这一层能单独补的：
>
> 1. **真去删那一枪**（`hcy/recyclebin/batchTrash`）：
>    **协议层已落在本仓**（`mcloud-upstream::cloud_trash`，2026-09-28）——
>    请求构造（空列表**不发**）、web 档头画像、回执解析都在，逐条对着
>    `mcloud/docs/capture/app-cloud-recyclebin-samples.txt` 与参考实现落码。
>    **仍缺的是"谁在链尾调它"**：本层是零 IO 层，而今天**一条写网盘的链都没有**，
>    所以它还没有调用点。等 P3 第一条链落地时接上。
>    ⚠ 协议层带来的两条口径要先说清（都写进了模块头）：
>    ① `success` **缺席算失败**，不是"没失败就是成功" —— 反着写会把
>    "上游换了回执形状"洗白成删除成功，产物还在网盘里而账本已销账；
>    ② 重复点名同一枚 `fileId` 今天**没有**"多记一条"的读数，但那**不是幂等保证**
>    （只测过三型对象、只测过当天），**不能**拿它省掉记账。
> 2. **编排链内的跨步骤累积接不上**：账本以 `RunId` 为键，而一次 pipeline
>    产出 N 条独立 run。刻意**不自造**"编排执行 id"——选错键的代价是
>    **分组分错 ⇒ 看起来像没删掉**，而那种错误不会报错。
>    上游参考实现把同一格登记为待拍板，这里沿用同一处置。
>
> ⚠ 另有一条**只到内存**的边界：账本活不过进程重启。留痕在运行记录的
> `details.artifacts` 里，所以"查历史"走 `/api/runs`，"看当下还剩什么"走
> `mcloud-gost pending`。

> **你需要提供的**：真实抓包（HAR 或样本）。没有它，P0-3 只能猜，而猜错的代价是
> "签到请求打到已下线的端点"——返回非零码，与"我写错了"完全无法区分。

### 🅿️ P1 — 签到族（共用一条会话，一次打通 8 条）✅ **已完成（2026-09-26）**

`sign` → `receive` → `multiple` → `backup-gift` → `revival` → `prizes` → `cloud-snapshot` → `task-list`

**为什么这批一起做**：它们**共用签到页那一条会话**，mcloud 侧就是 `run_dispatch_signin.go`
一层装配（§0 表格里"2026-09-23 那一批接线"）。打通一条，其余是加请求。

**共同坑（mcloud 明确记录）**：旧实现**全链吞错** —— 失败只打日志、从不抛错，
于是对 runner 永远"成功"。新实现必须把失败变成 `Run` 的失败。

> **落地位置**（分层同其余任务）：
>
> | 层 | 文件 | 职责 |
> |---|---|---|
> | 协议 | `crates/mcloud-upstream/src/app_signin.rs` | 请求构造与响应解析，**纯函数、零 IO** |
> | 编排 | `crates/mcloud-tasks/src/signin.rs` | 串步骤、判分支、把结论说成一句人话 |
> | 消费 | `test/golden/app-signin-vectors.json` | 实拍信封与常量，**权威** |
>
> **两处实测纠正**（都有抓包支撑，不是推断）：
>
> 1. **路径是 `/ycloud/signin/page/*`，不是 `/market/signin/page/*`**。
> 2. **判据是回查 `infoV3`，不是 `startSignIn` 的响应**。实测 `startSignIn`
>    响应里的 `cal` / `todaySignIn` 会骗人 —— 它说没签，而紧接着 `infoV3`
>    已经是 `s:true`。只信提交响应会把一次**成功**报成失败，用户看到失败
>    再点一次就是重复请求。

**其中 `revival` 有特殊风险**：判活接口**从未抓到**，旧实现是"盲打 + 中文文案匹配
`本月已领取` + 本地硬编码 2 次"。mcloud 明确说：**抓到判活接口前不要按 1 收紧**，
那是用一个没验证的假设换掉一条实测过的保险。
→ 本仓**没有收紧**：仍按文案判活，但把匹配面**收窄**成双条件
（先有"已领取/已领过"，再有"复活卡/回馈卡/奖励"三种主体词之一）。
裸一句"本月已领取"**刻意不认** —— 漏认一档只是报失败、人来判，
误认一片却会把真故障洗白。命中时判 `skipped`（跳过，不是失败也不是成功），
因为"本月没得领"下个月之前都不会自己好，天天标红只会训练人忽略告警。

**会话池已经有了**（此前这一格写的是"仍未做"，是错的）：
`mcloud_upstream::SessionCache` 按 `(账号, 配方, 凭据指纹)` 缓存 `H5Session`，
在 `mcloud-app` 的组装根里**一个进程一份**（`builder.rs` 的 `sessions`），
由 `H5Client` 在每条需要票的请求上取用、在 `90001` 时失效重取。
只有内存、**不落地**：票 2 小时即废，而库里那一列装的是登录凭据（同名不同物），
落一张两小时就死的票换来的是"重启后读到一张过期票"。

⇒ 八条任务**共用同一条票**，不是各建各的。


### 🅿️ P2 — 只读查询（低风险，可先做）

`notice`（其实会写）· `msg-push` · `fun-ai` · `play-ai-special`（只读那两口）

### 🅿️ P3 — 写网盘（需要 P0-4 护栏）

`task-expansion` · `ai-avatar` · `app-task`

`app-task` 是个**矩阵**：mcloud 记录了 14 种玩法，已接的有 106/107/113/409/434/522/1005/
472/473/1004/1020/1014/1028/585/587，另有 8 个 click-only（115/431/435/1009/1010/1013/1016/447）。
**它不是一个任务，是一个子系统**。

### 🅿️ P4 — 活动类（需要活动站会话抽象）

`red-flower` → `play-ai-special`（写）→ `cloud-phone-redpack` → `live-room`（WS）

前提：活动站会话抽象（旧项目每个活动各建一个"setAuth 是 no-op"的实例，这块要收敛）；
`live-room` 还要先做 protobuf 对拍 → WS → 口令解析三层。

> ✅ **`live-room` / `red-flower` 已完成（2026-09-26）**，三层都做了：
> protobuf 手写编解码（字节级对拍）、WS 连接层（离线假服务端）、口令抽取（41 条向量）。
> 落地位置：
>
> | 层 | 文件 |
> |---|---|
> | 常量（三套响应约定的单一来源） | `mcloud-upstream::live_consts` |
> | 口令提取（弹幕 + 小红书） | `live_kouling` |
> | MSync 手写 proto3 编解码 | `live_msync` |
> | `/cmlive` + 房间发现 + `member/apply` 签名 | `live_room` |
> | SockJS 帧状态机（纯层） | `live_ws` |
> | 真实 WS 连接（tokio-tungstenite + rustls） | `live_ws_client` |
> | 六步链（纯函数） | `live_flower` |
> | 编排 + registry 注册 | `mcloud-tasks::live_room` |
>
> ✅ **门的接线已完成**（2026-09-26）：`session::live_gate_spec_token_request`
> （门第①步：`querySpecTokenV2`，**`toSourceId=017027`** ← 直播侧）
> → `live_room::cmpass_login_request` → 直播侧 JWT → `tabContentList`（127+129 两页签）。
> ⇒ **不填 `codes` 就会自动开门听弹幕收口令**，再用收到的口令领小红花
> （用户要求的"自动获取口令并使用口令领取"）。
>
> ⚠ **门 ≠ WS**：门三步**有实证**（`docs/capture/app-live-redflower-samples.txt` §5.3
> 用只读探针走通过，因为第③步是 GET、不需要开播），
> 而 **WS / 真弹幕仍结构性抓不到**（未开播，`chatRoomId: null`）——
> 自动收集那条链的最后一环在本仓只用**假收集器**验过编排。两者别混为一谈。
>
> ⚠ 门失败时**如实上抛**，**不报**"今天没有直播"（"我们凭据坏了"与"今天没开播"
> 是两件事，后者是假结论）。有 `codes` 时跳过开门（省两个请求，且避免门偶发失败
> 把一趟本来能跑完的领取拖黄）。
>
> `play-ai-special` 仍未做；`cloud-phone-redpack` 的**只读那一半**（会话两段 + 任务表 + 余额）已落，写入侧 7 条待接。

> ⚠️ `hc1t`（AI豆大作战）**已过期**，不实现，只作形态参考。

### 🅿️ P5 — 高风险收尾

`exchange`：**真扣AI豆、不可回滚**，需要幂等与抢兑窗口设计评审 + 滑块。

---

## 3. 两个项目对不上的地方（需要你拍板）

| # | 差异 | mcloud | mcloud-sign TASKS.md | 需要你定 |
|---|---|---|---|---|
| D1 | `after` 收尾 | **不是独立能力**（无 code） | 是一个任务 | 是否登记成能力？ |
| D2 | `red-flower` | **并入 `live-room`** | 独立的 3 个函数 | 保持合并还是拆开？ |
| D3 | `hc1t` | code 都没登记 | 标注**已过期** | 确认不实现 |
| D4 | `cloud-snapshot` | 新能力（旧项目没有） | 无 | 已确认要做 |
| D5 | 签到页开关 | 拆成 5 条 code（receive/prizes/multiple/backup-gift/revival） | 混在 `sign` 里 | 跟随 mcloud 的拆分 |
| D6 | `app` | 登记成**一条**（刻意过渡） | 一个 `appTask` | 跟随 mcloud |

---

## 4. 我建议的下一步（等你确认）

> **进展（2026-09-26）**：P0-1 / P0-2 / P0-3 已完成，P0-4 的账本层已完成，
> **605 个测试全绿**，clippy 0 告警。
> 证据：占位端点 `/ycloud/api/sign/query` 实测 **405**，实拍端点
> `/ycloud/signin/page/infoV3` 实测 **200**。
> 仍未做：P0-4 的两条尾巴（`batchTrash` 那一枪、编排链内跨步骤累积）——
> 两条的接法与理由都写在上面 P0-4 那个格子里。

**下一步：P1 签到族**（P0 的地基已经够用），理由：

1. **签到页那一条配方是完整实拍的** —— `toSourceId=001005` → `/ycloud/api/timestamp`
   → `tyrzLogin`（`marketName=sign_in_3`、`sourceId=1002`），三个端点 mcloud 都有实测结论，
   本项目 `endpoints.rs` 里三个常量也**已经在了**。**这一条不需要新抓包。**
2. 但**必须一次就按"配方 + 池"的模型写**，不能先写死签到页那一套。
   理由：mcloud 自己就吃过这个亏 —— 它先是把换票三参数**写死**在
   `OpenSignInSession` 里，后来发现 `msgpushon` / `backupgift` 各有自己的配方，
   切的时候"动的是**共享会话层**"，于是 `pending-decisions.md` Q42 只能
   "先登记、等第二条页面一起改"。
   **我们先按配方建模，就不欠这笔债。**
3. 做完它，`sign` 才能从 405 变成真能跑。
4. 顺手把 `account_state` 用起来（目前**零任务在用**，是设计好但空着的存储）。

**P0-1 的验收判据**（照 mcloud 的测试抄）：
- 同账号同配方跑两轮 ⇒ `tyrzLogin` **只发一次**
- 撞 `90001` ⇒ 刷新 + **只重放那一条**请求（不是整轮）
- 不同配方 ⇒ 各自建票，互不顶替
- 凭据轮换 ⇒ 旧指纹的池条目被清掉

**请你确认**（前两条已落地，就地标记；剩下的仍然待定）：
- [x] ~~**P0-1 按"配方 + 池"建模**~~ —— ✅ 已按此落地（`SessionRecipe` + 会话池）
- [x] ~~P0-2（auth 刷新）是否同批做~~ —— ✅ 已做（`mcloud-upstream::auth_refresh`）
- [ ] **抓包材料**：签到页那条已有实拍，但 `msgpushon` / `backupgift` / 拍拍子页 /
      小红花页的配方，要不要现在就一起支持？
      - 若"先只支持签到页" ⇒ 那些任务先标"jwttoken 使用公共来源"（mcloud 也是这么过渡的）
      - 若"一次到位" ⇒ 需要你把对应抓包给到（`mcloud/docs/capture/` 里已有 6 份，
        可以直接拿来用，我不需要你重新抓 —— **只要你说可以用**）
- [ ] 上面 D1–D6 六个差异怎么定
- [ ] 是否接受"P1 八条一起做"而不是一条条来
- [ ] **写网盘那批（P3）落地时**：产物的"真去删"那一枪接在哪
      （`batchTrash` 协议 + 谁在链尾发）—— 见上面 P0-4 格子里那两条尾巴

我**不会**在没有实拍依据的情况下猜端点。猜测的产物是"能编译、能过测试、
但打到一个已下线的地址"—— 返回非零码，和"我写错了"完全无法区分，
这种错误比不写更糟。
