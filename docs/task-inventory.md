# 任务实现清单（待确认）

> 来源：`mcloud`（Go，19 条能力已全部实现）+ `mcloud-sign`（旧 TS，权威清单 `TASKS.md`）
> + `mcloud/docs/business-feature-inventory.md`（逐条实现规格，含 `path:line` 证据）。
>
> 本项目现状：**只有 1 个任务**（`sign`），且端点是**占位**的（实测 HTTP 405）。

---

## 0. 先说结论：真正的阻塞点不是任务数量

我原本以为"19 个任务要一个个写"。核对后发现**顺序被一个东西卡住**：

### ⛔ 阻塞点一：`jwttoken` 拿不到（挡住几乎全部任务）

本项目 `crates/mcloud-app/src/builder.rs:60` 传的是：

```rust
jwt_token: None,   // ← 所有需要 jwt 的任务到这里就断了
```

而 mcloud 侧**每一个**业务上传文件的凭证需求是这样的：

| 上游文件 | jwt 用量 | 说明 |
|---|---|---|
| `app_play_ai.go` | 29 处 | 拍拍 |
| `app_signin_send.go` | 26 处 | 签到提交 |
| `app_notice.go` | 20 处 | 通知 |
| `live_room.go` / `live_flower.go` | 各 17 处 | 直播口令 / 小红花 |
| `app_ai_avatar.go` | 37 处 | AI 头像 |
| `app_funai.go` / `app_msg_push.go` / `app_exchange.go` | 8–10 处 | 抽奖 / 推送 / 兑换 |
| **`app_ai_workbench.go` / `app_note.go` / `app_mail139_assets.go`** | **0 处** | ← **只有这几个走 basic** |

**结论**：`jwttoken` 是"取一次、后续全程复用"的会话凭证。它不通，19 个任务里绝大多数
连第一个请求都发不出去。这不是"先写哪个任务"的问题，是**地基**。

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
| 2 | `receive` | 领取云朵 | Action | ✅ | | catch-up | 记账 | jwt |
| 3 | `multiple` | 签到翻倍 | Action | ✅ | | catch-up | 记账 | jwt |
| 4 | `backup-gift` | 备份好礼 | Action | ✅ | | catch-up | 记账（**GET 却是写**） | jwt |
| 5 | `revival` | 用复活卡/回馈卡 | Action | ❌ | | skip | **不可逆** | jwt |
| 6 | `prizes` | 待领取奖品 | Query | ❌ | | catch-up | 无 | jwt |
| 7 | `task-list` | 任务清单 | Query | ❌ | | catch-up | 无 | jwt |
| 8 | `notice` | 开启通知领云朵 | Action | ✅ | | catch-up | 记账 | jwt |
| 9 | `app-task` | App 任务（做任务） | Action | ❌ | | skip | **上传/建删外链/发邮件** | jwt |
| 10 | `fun-ai` | 趣玩AI抽奖 | Action | ✅ | | catch-up | 记账 | jwt |
| 11 | `msg-push` | 消息推送任务 | Action | ❌ | | skip | 记账 | jwt |
| 12 | `cloud-phone-redpack` | 红包派对盘点 | Query | ❌ | | catch-up | 无（**只读那一半**） | jwt + 云机会话 |
| 13 | `play-ai-special` | 拍拍盘点 | Query | ❌ | | catch-up | 无（**只读那两口**） | jwt + 活动会话 |
| 14 | `ai-avatar` | AI 头像生成与抽奖 | Action | ❌ | | skip | **往网盘写文件** | jwt |
| 15 | `task-expansion` | 任务拓展 | Action | ❌ | | skip | **往网盘写文件** | jwt |
| 16 | `live-room` | 直播口令 → 小红花 | Action | ❌ | | skip | 入群足迹 + **WS 长连** | jwt + WebSocket |
| 17 | `exchange` | 兑换奖品 | Action | ❌ | ✅ | skip | **真扣云朵、不可回滚** | jwt + 滑块 |
| 18 | `cloud-snapshot` | 云朵余额快照 | Query | ❌ | | catch-up | 无 | jwt |
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
| P0-1 | **JWT 会话链**：`querySpecTokenV2` → `tyrzLogin` → 缓存到 `account_state` | 拿到 jwt 并落盘，第二次运行复用不重取 |
| P0-2 | **`auth-refresh`**：`authTokenRefresh.do`（XML 体、正则取 `<token>`）+ 三档判据 | `expire<0` ⇒ `ManualRequired`，不重试 |
| P0-3 | **端点校正**：按真实抓包逐条核对 `/market/` vs `/ycloud/` | §5.1 那三组差异有结论 |
| P0-4 | **产物账本**：run 名下文件清单 + 失败也清理 | `after`/`app-task` 的安全前提 |

> **你需要提供的**：真实抓包（HAR 或样本）。没有它，P0-3 只能猜，而猜错的代价是
> "签到请求打到已下线的端点"——返回非零码，与"我写错了"完全无法区分。

### 🅿️ P1 — 签到族（共用一条会话，一次打通 8 条）

`sign` → `receive` → `multiple` → `backup-gift` → `revival` → `prizes` → `cloud-snapshot` → `task-list`

**为什么这批一起做**：它们**共用签到页那一条会话**，mcloud 侧就是 `run_dispatch_signin.go`
一层装配（§0 表格里"2026-09-23 那一批接线"）。打通一条，其余是加请求。

**共同坑（mcloud 明确记录）**：旧实现**全链吞错** —— 失败只打日志、从不抛错，
于是对 runner 永远"成功"。新实现必须把失败变成 `Run` 的失败。

**其中 `revival` 有特殊风险**：判活接口**从未抓到**，旧实现是"盲打 + 中文文案匹配
`本月已领取` + 本地硬编码 2 次"。mcloud 明确说：**抓到判活接口前不要按 1 收紧**，
那是用一个没验证的假设换掉一条实测过的保险。

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

> ⚠️ `hc1t`（云朵大作战）**已过期**，不实现，只作形态参考。

### 🅿️ P5 — 高风险收尾

`exchange`：**真扣云朵、不可回滚**，需要幂等与抢兑窗口设计评审 + 滑块。

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

**只做 P0-1 + P0-2（JWT 会话链 + auth 刷新）**，理由：

1. 它是**唯一不依赖真实抓包**就能做对的部分 —— `querySpecTokenV2` /
   `tyrzLogin` / `authTokenRefresh.do` 三个端点在 mcloud 侧都有**实测结论**
   （不是"零实拍"），且本项目 `endpoints.rs` 里三个常量**已经在了**。
2. 做完它，`sign` 才能从 405 变成真能跑 —— 这是当前唯一能让项目"落地"的一步。
3. 它顺手把 `account_state` 用起来（目前**零任务在用**，是设计好但空着的存储）。

**请你确认**：
- [ ] P0-1/P0-2 是否可以开始
- [ ] 抓包材料能给到哪一档（HAR 全量 / 只看几个端点 / 暂时没有）
- [ ] 上面 D1–D6 六个差异怎么定
- [ ] 是否接受"P1 八条一起做"而不是一条条来

我**不会**在没有抓包的情况下猜端点。猜测的产物是"能编译、能过测试、
但打到一个已下线的地址"，这种错误比不写更糟。
