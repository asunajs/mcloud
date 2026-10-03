# 全项目 review —— 2026-10-03

> **本报告由 Qwen3.8-Flash 提供。**
>
> 五个重点：① Rust 可能隐藏的逻辑问题；② 前端可能存在的性能问题；③ `docs/` 里老旧、容易误导的内容；
> ④ `docs/` 同一事实在多处重复且互相冲突；⑤ 注释与文档 / 代码互相矛盾的地方。
>
> 每条都带**落点（`文件:行`）**与**怎么复核**。置信度分三档，别混着读：
> **已实测**（有可复现读数）/ **读码确认**（代码里直接读到）/ **推断**（从形状推的，没量过）。

## 0. 基数与口径

| 项 | 值 |
|---|---|
| 基线 | 读码从 `HEAD 1ebfe77`（2026-10-03 00:23:54）开始；**审阅期间 HEAD 移动了三次**，本报告定稿时对 `a82748c`（08:54:22）复核过一遍。当时工作区在飞的 `Cargo.lock`、`Cross.toml`、`crates/mcloud-upstream/Cargo.toml` 已由并行线提交收口（`66cb3aa`，其中 **`Cross.toml` 被整个删除**），那三处已不再是"在飞"状态 ⇒ 原先标「⚠ 文件在飞」的条目已全部按提交后的现状改写。**另有一处在飞**：定稿时 `crates/mcloud-tasks/src/exchange.rs` 与 `exchange/tests.rs` 是未提交改动（并行线在照 §1.1 修 `WaitFn`；大小以 `git diff --stat crates/mcloud-tasks/` **现算** —— 我读到那一刻是 +82/−15，可能还在涨）⇒ **只有 §1.1 与 C6 两条受它影响**，两处都已就地写明"动手前先 `git diff`" ⚠ |
| Rust | 8 个 crate；生产行数 core 6027 / crypto 3211 / upstream 31125 / engine≈4.1k / store≈3.4k / tasks≈14k / server≈5.2k / app≈4.5k |
| Web | `web/src` **36 个文件 / 14371 行**；`web/lite` 运行时 + 编译器由本次审阅逐条读码 |
| 跑过的命令 | `cargo check` **一次都没跑**、`cargo build/test/clippy --workspace` **全部没跑**、`npm run build` 没跑、`scripts/gates-web.mjs` 没跑（都不碰 `target/debug/mcloud-server`、`web/dist` 和 :3000）。其余只读：`git log/status`、`grep`、`wc`、`sed`；实测量测三组：`web/lite/bench`（运行时耗时）、一次**临时**的 `vite build`（入口在 `web/lite/regress/.check-tmp/`，产物落 `/tmp`，跑完连目录删干净；`git status` 已确认无残留）、以及仓内常驻的 `node lite/regress/leak.mjs --fixture --rounds 5`（夹具后端 + 调试端口 48980 ⇒ 不碰 :3000、不重编二进制）。**唯一一次浏览器装置失败**（同形夹具页在 Chrome 里跑不起来，见 §2 开头的产物核对说明）没有影响结论，只是少了一组绝对毫秒数 |
| 实测装置 | `web/lite/regress/lib.mjs` 的夹具服务 + 无头 Chrome（CDP）；`web/lite/bench` 的基准页 |

一句话结论：**协议内核与任务层的"护栏式注释"密度很高，多数危险面已经被前人钉住了**；剩下最硬的三个问题是
「抢整点兑换在定时路径上一枪不发」「调度器被单个长任务占住、优雅停止要等它跑完」「链级取消在收尾判定里被漏掉」。
前端这一侧最值钱的是一条**交互级**问题：日志面板与参数块都在"每次变化拆掉整块重建"，而仓内已经有正确写法（账号页的 thunk）可以做对照。

---

## 1. Rust：可能隐藏的逻辑问题

### 1.1 [高] 抢整点兑换走定时/编排路径 ⇒ 必然"零请求"结束

**落点**：`crates/mcloud-tasks/src/exchange.rs:1845`（`wait: None` 硬编码）+ `:1257-1261`（没等待实现就早退）+ `:716-730`（窗口等待判据）+ `web/src/pages/exchange-page.tsx:373`（界面发 `skipWait=false`）

```rust
// exchange.rs:1845 —— ExchangeTask::run 里
let ran = ExchangeRun { prize: &prize, skip_wait, force, offset_ms,
    device_id: device_fn.as_deref(),
    wait: None,          // ← 生产链路恒为 None
}.run_account(&env, ctx.clock.now_ms()).await;

// exchange.rs:1257 —— 于是这一段
let Some(sleep) = opts.wait else {
    return Ok(set_error(skipped(&judge, outcome),
        "skipWait=false 但这一趟没接上等待实现（不静默跳过等待，本次没发请求）".into()));
};
```

**为什么这是"隐藏"的**：类型上完全合法，测试里也测不到 —— `wait: Some(..)` **只出现在 `tests.rs`**。
界面那侧却把它当成产品功能写给了用户（`exchange-page.tsx:345` 注释：「提前量 ≥60 秒且目标是整点时，发 `skipWait=false`，由任务内部等到那一刻再发枪」）。

**后果发生在哪个动作**：用户在「兑换」页设一个"21:00 抢 X，提前 120 秒"的定时 ⇒
cron 排在 20:58 触发（`exchange-page.tsx:374-376`）⇒ 那一刻 `into_hour = 58×60×1000 < start_ms(≈59:40)` ⇒
`plan_exchange_window_wait` 回 `needed = true` ⇒ 直接早退，**一条上游请求都不发**，历史里是一条"跳过 + 那句中文错误"。
提前量 ≥60 秒的整点目标 ⇒ 每次都是这个结局（`skipWait=false` 的定义就是"要等"）。

**怎么复核**：
```bash
grep -rn "wait_until" --include="*.rs" crates    # 只有 exchange.rs:1943 那条注释引用它，函数不存在
grep -rn "wait: " crates/mcloud-tasks/src/exchange.rs | head
grep -n "skipWait" web/src/pages/exchange-page.tsx | head
```
要坐实行为，最快的路是在**库副本**上（别碰 `data/`）设一个 `skipWait=false` 的 schedule，把 cron 排到 2 分钟后，看运行历史是不是那句"没接上等待实现"。

**修的方向**（不在本报告范围内执行）：给 `ExchangeTask::run` 注一个真的 sleep（`tokio::time::sleep` + `check_cancelled` 分段），或者把界面的"任务自己等"这条承诺撤掉、改成调度器提前到窗口内触发。两者都要连 `:1943` 那条空头注释一起改。（**HEAD 上的行号**，见下面那条状态更新。）

> **状态更新（2026-10-03 12:1x）：本条已被并行写入线接手并落盘** ⚠
> 上面「修的方向」的第一条**正在工作区里改**（**未提交** —— `git status` 显示 `M crates/mcloud-tasks/src/exchange.rs` + `M crates/mcloud-tasks/src/exchange/tests.rs`；**改动还在涨**，大小自己跑 `git diff --stat crates/mcloud-tasks/`）：
> `WaitFn` 从同步 `dyn Fn(i64) -> Result<()>` 改成返回 `WaitFuture` 的 async 形式；
> `ExchangeTask::run` 里 `wait: None` 换成注入 `sleep_until(&*clock, &cancel, chain.as_ref(), until_ms)`（`WAIT_SLICE_MS = 200` 分段睡，每片查 `cancel` 与 `chain_cancel`）；
> `exchange()` 里两处 `sleep(...)` 补 `.await`；`:1943` 那条空头注释改成"之前指向一个不存在的 `wait_until`"（所以现在 `grep -rn wait_until` 仍只一处命中，但内容和行号都变了 —— 工作区里是 `:2003`）。
> ⇒ **别再照上面那份代码动手**；先 `git diff crates/mcloud-tasks/src/exchange.rs crates/mcloud-tasks/src/exchange/tests.rs` 看现状。
> ⚠ 这份改动**我没验证过**：没跑 `cargo check -p mcloud-tasks --all-targets`、没跑任何真任务。台账 #30（并行线自己记的）说它已重编 + kickstart、且**全套门禁跑过**；我 12:1x 只核到 `:3000` 现在 `/api/health` = 200 ✓，"线上那份确实含这个修法"这一条**我没有独立读数** ✗ —— 要确认看 #30 里那条负向钉子 `task_shell_injects_a_real_wait`（把修复撤掉会红）。
> 而且它动的是 `WaitFn` 的**公开类型签名**（`ExchangeOptions` 与 `ExchangeRun` 都持有它）⇒ 所有调用侧（含 `tests.rs` 里那些 `wait: Some(..)`）跟没跟上，得由动手的人跑一次那条 check 才算数 ✗ 别只看它写完了。
> 顺带一条**留给复核**的疑点：新注释说「单测里的假 `Clock` 时间不会自己走 ⇒ 每片重读时钟会死循环」，所以它用 `left -= slice` 递推。
> 递推的代价是**睡不够准**：每片只睡 `min(left, 200)`，而 `tokio::time::sleep` 本身只会晚不会早 ⇒ 唤醒/调度的迟到会**一路累加**，抢整点这种"卡在窗口边上"的场景最敏感（一小时那侧最多累计到秒级）。
> 真要修得稳，应当每片按 `until_ms - clock.now_ms()` 重算、但只在**真时钟**下这么做（假时钟下用递推兜住），或者在睡完后再校正一次。置信度：**推断**（没跑过）。

### 1.2 [高] 调度器 `tick()` 里同步 `await` 整条任务/整条编排

**落点**：`crates/mcloud-engine/src/scheduler.rs:311`、`:388`；循环形状在 `:534-545`

```rust
// scheduler.rs:311（Due 分支）
self.advance_watermark(&sch, point_ms).await?;
let (outcome, _blocked) = self.fire(&sch, point_ms).await;   // ← 整条任务在 tick 里跑完
// scheduler.rs:388（定时编排）
match self.pipelines.run(&pipeline, accounts, None).await {  // ← 整条链

// scheduler.rs:534 run_loop
tokio::select! {
    _ = interval.tick() => self.tick_and_log().await,        // ← 期间
    res = stop.changed() => { ... }                          //    stop 分支不被轮询
}
```

**后果**：一次多账号、分钟级的任务（直播听房一类要听 45 s，再乘账号数）会把整个调度循环占住。期间
① 其他到点的定时全部排队（水位不动，表现成"这一轮没跑"）；
② **优雅停止要等这一轮跑完** —— 关停窗口不够就是硬杀，可能停在半个上游请求上（签到类上游不幂等，这是真风险）。
不是理论问题：`select!` 的形状就写着"我假设 tick 很短"。

**怎么复核**：读 `:534-545`；或在库里放一条 `task` 为慢任务的 schedule，`/api/system/restart` 之前观察停止耗时（**注意**：这会重启 :3000 上的托管服务，属于要用户点头的动作，我没跑）。

### 1.3 [高] 链级取消在"收尾判定"里被漏掉 ⇒ 用户点停止 = N 条红色失败

**落点**：`crates/mcloud-engine/src/runner.rs:346`（只看单 run 令牌）vs `crates/mcloud-engine/src/lib.rs:66-77`（`check_cancelled`/`is_cancelled` **两条都看**）；`crates/mcloud-server/src/routes/pipelines.rs:201`（链令牌没登记）；`crates/mcloud-engine/src/scheduler.rs:388`（定时编排传 `None`）

```rust
// runner.rs:346
let was_cancelled = cancel_token.is_cancelled();   // cancel_token 来自 :320，单 run 那枚
// lib.rs:75  TaskContext::is_cancelled —— 口径不一样
self.cancel.is_cancelled() || self.chain_cancel.as_ref().is_some_and(|c| c.is_cancelled())
```

`runner.rs:438-440` 的 `if was_cancelled { record.status = RunStatus::Cancelled }` 因此永不覆盖链上的子步；
`record()`（`:450-468`）默认写 `Done`，而 `Err → TaskOutcome::failed`（`:341`）⇒
**状态"已完成"、业务结论"失败"**。往下游：`pipeline.rs:234` 的 `cancelled` 判据落假，`notify` 侧按 `registry.is_cancelled`（单令牌）判假 ⇒ 推送报 error。

同文件 `:463-464` 那句注释（「生命周期与业务结论分开……被中止时由调用方覆盖成 `Cancelled`」）本身没写错设计意图，
但它把希望寄托在 `:438` 那个**只看了单令牌**的判据上 —— 读起来就像"这条链已经闭环"。

`routes/pipelines.rs:201` 那句 `let cancel = mcloud_core::CancelToken::new();` 是**自造、不登记**的链令牌：
HTTP `/cancel` 只能翻每个子步的单 run 令牌，够不着它 ⇒ 手动跑的编排停不掉"进入下一步"；定时跑的编排（`:388` 传 `None`）**完全不可取消**。

**怎么复核**：`grep -n "chain_cancel" crates/mcloud-engine/src/{runner.rs,lib.rs}`、`sed -n 195,210p crates/mcloud-server/src/routes/pipelines.rs`。

### 1.4 [中] 更新定时会把累计记账清零（且与它自己的注释相反）

**落点**：`crates/mcloud-server/src/routes/schedules.rs:188-189`，对照同函数 `:159-161` 的注释「保留水位线与**记账字段**」

```rust
// schedules.rs:185-189 —— 同一个 struct 字面量里
last_fire_ms: existing.as_ref().and_then(|s| s.last_fire_ms),
last_run_ms:  existing.as_ref().and_then(|s| s.last_run_ms),
last_status:  existing.and_then(|s| s.last_status),
last_error: None,     // ← 注释说要保留记账，这里抹掉了
run_count: 0,         // ← 累计运行次数归零
```

**后果**：改一次名字或表达式（`PUT` 到已存在的 id）⇒ 界面上「跑过 N 次」变 0；而 `last_status` 留着 `error`、`last_error` 空 ⇒ 显示"error"却答不出原因。注释与代码在同八行内自相矛盾，属于「看着像 bug 但故意的」的反面 —— 它不是故意的。

**复核**：`sed -n 155,195p crates/mcloud-server/src/routes/schedules.rs`；`grep -n "run_count" crates/mcloud-store/src/json_store.rs`。

### 1.5 [中] `EventHub` 只进不出：`forget` / `unsubscribe` 生产零调用

**落点**：`crates/mcloud-app/src/hub.rs:151`（`unsubscribe`）、`:209`（`forget`）、`:132`/`:234`（`runs.entry(run_id).or_default()`）；SSE 侧 `crates/mcloud-server/src/routes/runs.rs:375`

**已实测**（grep 计数，非推断）：这两个方法的调用点**只出现在 `hub/tests.rs`**（`:429`、`:480`、`:482`）。
`forget` 的文档自己写着「只在事件已落盘之后调用」，但没人调；SSE 流被 drop（客户端断开/任务结束）也不会 `unsubscribe`。
⇒ 每个 run 的 `RunState`（含最多 `LOG_LIMIT_PER_RUN` 条 history + 全部订阅者队列）常驻，随运行次数**线性增长**。本地长跑（launchd 托管、debug 常驻）会看到 RSS 只涨不落。

**复核**：
```bash
grep -rn "\.unsubscribe(\|hub\.forget" --include="*.rs" crates   # 期望：只有 tests.rs
```

### 1.6 [中] 水位写失败用 `?` 冒泡 ⇒ 整轮 tick 提前结束

**落点**：`crates/mcloud-engine/src/scheduler.rs:296`、`:310`（`self.advance_watermark(...).await?`），消费点 `:561` 只 `tracing::error!`

一轮里排在前面的定时落盘失败（磁盘满 / `JsonStore` 整文件重写失败），**本轮其后所有定时都不再判定**，而下一次 tick 才恢复。语义上"宁可少跑"是对的，但代价是**一条定时写失败会连带饿死同一轮其他定时**。

### 1.7 [中] 「错过的触发点整体弃掉」也 +1 运行次数

**落点**：`crates/mcloud-store/src/json_store.rs:1026`（`mark_schedule_fired` 里 `run_count += 1`），被 Missed（`scheduler.rs:296`）与 Due（`:310`）两条分支共用

口径混了：弃跑（一次上游都没打）与真跑记成同一个计数。界面上「跑过 N 次」在补跑弃置后会虚高。

### 1.8 [中] 删账号不级联：定时/编排里留悬空账号 id

**落点**：`crates/mcloud-store/src/json_store.rs:738-749` —— `remove_account` 只做 `db.accounts.retain` + `db.state.remove` + `db.smid.remove`

schedules / pipelines 的 `accounts` 数组不处理。运行时 `target_accounts` 会过滤掉不存在的账号（所以不会炸），但**配置面**留着一堆引用别人 id 的定时；重新导入同 id 的账号时那些定时会"复活"。影响小、属清理不彻底，`remove_schedule`(`:1016`)、`remove_pipeline`(`:1099`)、`remove_user`(`:1251`) 都做了级联，只有账号这一侧没做 —— 更像漏写。

### 1.9 [中] 登录校验在 async 处理器里同步跑 argon2；全仓无登录限速

**落点**：`crates/mcloud-server/src/routes/auth.rs:220`（`mcloud_crypto::verify_password(...)`）→ `crates/mcloud-crypto/src/password.rs:93`

**已实测**：`grep -rn spawn_blocking --include="*.rs" crates` 只命中注释，无调用 ⇒ argon2 直接在 tokio worker 上跑。
另外 `login` / `setup` 都没有失败计数、退避或锁定（`grep -rn "throttle\|429\|lockout"` 零命中）—— 单用户本机服务，爆破面主要在"服务被暴露到局域网"时；这条留给用户判断要不要处理。

### 1.10 [中] 同名请求头发两枚：`user-agent` 与 PC 设备头

**落点**：`crates/mcloud-upstream/src/client.rs:332`（先设 profile UA）+ `:334-336`（再把 `req.headers` 逐条 `header()` 上去）；`crates/mcloud-upstream/src/smid.rs:687`（指纹请求自己在 `req.headers` 里放 `user-agent`）；`client.rs:325-329`（PC 设备头无去重）

`reqwest 0.12` 的 `RequestBuilder::header` 是 **append**，不是 set ⇒ 会出现两根同名 `user-agent`。
这一族在本仓**已经踩过并被钉成红线**：`cloud_phone_redpack.rs:203`「在这儿加会是第二根」、`:1138` 有断言防它；同文件 `client.rs:299-312` 刚给 `authorization`/`jwttoken`/`deviceid` 补了 `should_inject_credential` 去重 —— **UA 与 PC 设备头是同一规则漏掉的两支**。

**怎么复核**：`grep -n "user-agent" crates/mcloud-upstream/src/client.rs crates/mcloud-upstream/src/smid.rs`。
上游/APISIX 收到两枚时取首值还是末值 —— **未证实**，需要实拍（见 §6）。

### 1.11 [中] HTTP 状态分类把 401/403/400 全折进 `Error::Network`

**落点**：`crates/mcloud-upstream/src/session.rs`（`ensure_http_ok`，非 2xx → `Error::Network`）+ `crates/mcloud-core/src/error.rs:8-57`（没有 `Http(u16)` 变体）

讽刺的是同一条链的**诊断注释**已经写清了危害（`client.rs:290-295`：双 `Authorization` 时 APISIX 回 **400 HTML**，"看起来像网络/网关问题"，与真结论"凭据过期"完全不是一回事）。分类层没把这个认知接住：上层无法据类型触发刷新/重登，排查方向会被带向代理与网络。8 处调用都吃这个分类。

### 1.12 [次要] 一片"文档规定要用、代码里从不生产"的死面

**已实测**（grep 计数）：

| 项 | 定义处 | 生产调用/构造 |
|---|---|---|
| `Error::AuthExpired` | `mcloud-core/src/error.rs:17` | **零构造点**（只有 `:70` 与 `mcloud-server/src/error.rs:27` 在 match 它） |
| `Error::is_auth_failure()` | `mcloud-core/src/error.rs:68` | **零调用**，而 `docs/conventions.md:288` 写「判断凭证失效**必须**用它」 |
| `Account::needs_refresh` | `mcloud-core/src/account.rs:140` | 零生产调用（只有 `:750` 自测），且与 app 侧 `decide()` 对 `expire<=0` 的取向相反 |
| `assert_usable()` | `mcloud-app/src/auth_refresh.rs:259` | 文档自称"落库前的兜底校验"，调用者 0 |
| `endpoints.rs` 七个常量 | `:21/:24/:28/:35/:38/:68/:104` | 零引用；实际 URL/sourceId 硬编码在 `session.rs:356`、`:96/:105/:112`（值还一样）⇒ 改一处即漂移 |
| `recover_orphans` / `running_runs` | store 侧 | 死码 |

`is_auth_failure` 里那张码表（`"1001"|"1002"|"401"|"AUTH_FAILED"|"TOKEN_EXPIRED"`）**没有出处** —— 按 AGENTS.md 的依据优先级，这类常量应当标"推断"或注明来自 `mcloud-sign`，现在读起来像实拍。

### 1.13 [次要] 按主机分 TLS 后端已成空转，注释还在讲两套

**落点**：`crates/mcloud-upstream/src/client.rs:101-106`、`:181`、`:276-277`；`crates/mcloud-upstream/Cargo.toml:25`；`live_ws_client.rs:63`+`:289`

工作区 `Cargo.toml:131`「原先的 `rustls-tls` 已移除」、`:139` 只留 `native-tls` ⇒ `inner` 与 `legacy` 两个 builder **行为相同**（`:276-277` 那个按主机分流现在等于空转），而 `:103` 仍写「两个 builder 差别只有 TLS 后端」「必须同步改」；`LEGACY_TLS_HOSTS` 上方 `:180` 那条「加之前先确认『全站真的没有现代套件』」的护栏也跟着失去了意义 —— **表再长也不改变行为，只会让人以为它在改变行为**。
`crates/mcloud-upstream/Cargo.toml:25` 那句「TLS 走 rustls（与 reqwest 同源，不引入 openssl）」现在是**三重过期**：reqwest 侧（根 `Cargo.toml:139`）已改 native-tls；WS 侧（根 `Cargo.toml:161`）也已改 native-tls ⇒ "走 rustls""与 reqwest 同源"两句都不成立；第三句"不引入 openssl"至少对 **`aarch64-unknown-linux-gnu`** 不成立 —— `66cb3aa`（2026-10-03 00:52，我读码之后由并行线提交）给该 target 加了 `openssl = { features = ["vendored"] }`（`crates/mcloud-upstream/Cargo.toml:81-82`，注释自己写明"静态链 OpenSSL、约大 2–3 MB"）。macOS/Windows 走 Security.framework/SSPI 那侧确实还没有 OpenSSL，所以这半句要按 target 读，别当成"全平台都链了 openssl"。

### 1.14 [次要] 其他四条

- `crates/mcloud-upstream/src/live_ws_client.rs:318`：`connect_async(request).await` **无超时**（同文件 HTTP 侧有 30 s）；而 `Cargo.toml:150-156` 刚论证过这台老主机的握手敏感 ⇒ 挂起会吃掉整轮听房。
- `crates/mcloud-upstream/src/session.rs:346` 注释「`serde_json` 的 Map 默认按**插入序**输出」与同仓 `mcloud-upstream/src/auth_refresh.rs:106-112`「默认按 BTreeMap 字典序（除非开 `preserve_order`）」**直接冲突**；`Cargo.toml:74 serde_json = "1"` 未开该特性 ⇒ 后者对。当前签名恰好可用，只因键名写得已按字典序 —— 这是"靠运气对"的那类注释误导。
- `crates/mcloud-crypto/src/tn.rs:61` 对数组走 `out.push_str(&get_tn(v))`，而 `get_tn` 对非对象 `return String::new()` ⇒ 数组贡献空串；同文件 `:15` 的规则 3 写的是「object（含数组）→ 递归」。当前 `build_sm_data` 产物无数组字段（潜伏）。
- `crates/mcloud-core/src/event.rs:191` `redact()` 未覆盖 `RunEvent::Log`(`:56`) 与 `Finished`(`:68`)，生产只有 3 处调它（server/error.rs、app/push.rs ×2）⇒ engine/hub/store 落盘与 SSE 广播不过它。日志里带口令、带票的可能性没有被这道闸兜住（`feedback-no-real-phone-numbers` 那条口径在成文侧成立，代码侧没闸）。

### 1.15 [次要] CLI 的三处"教用户做错事"

**落点**：`crates/mcloud-server/src/bin/cli.rs:42`、`:192`、`:195`

- `:42` 注释「现在有 **6** 个命令」，顶层分派有 **7** 个 arm（`tasks`45 / `pending`57 / `login`77 / `cron`98 / `pipeline`101 / `auth`105 / `run`122）。
- `:192` `print_usage` 的 login 模式列 `qr / sms / app / password`，**漏 `device`**（`:86` 会分派它）—— 用户照帮助文本打不出那条命令。
- `:195` 让用户跑 `mcloud serve`，**没有这个子命令**（grep 零命中）。这条最实在：照着说就是 `unknown command`。

复核：`grep -n '^\s*"'\'' crates/mcloud-server/src/bin/cli.rs | head -30`；`./target/debug/mcloud 2>&1 | head`（帮助文本，不启动服务，安全）。

### 1.16 未证实但值得留名的两条

- **90001 重放可能双扣**：`session_source.rs:158-177 with_session` 撞 `90001` 后**原样重放那一条**，不区分幂等性。如果上游在**已落单之后**才回 90001，`exchangeV3`（真扣豆，`mcloud-tasks/src/exchange.rs:1129`）就是双扣。**没有抓包证据**说明 90001 可能出现在落单之后 —— 记为待实拍，不作为缺陷。
- **同名不同 id 的打卡合并**：`mcloud-tasks/src/app_task_matrix.rs:145` 按 **name** 去重，而 `app_task.rs:739` 按 **task_id**。若上游同批给两条同名不同 id 的任务，其中一条会永不打卡。是否有这种形态 —— 未证实。

### 1.17 [中] 地址写错时进程已经跑过一轮调度 —— `bind` 排在所有副作用之后

这条是用户 2026-10-03 **12:11** 实跑 release 包时撞出来的（`~/Downloads/mcloud-v3.0.0-rc.1-aarch64-apple-darwin`，`MCLOUD_ADDR=3010`）⇒ 补进 §1。使用者写错地址是表象，**"报错前已经把副作用跑完"是本仓自己的形状**。

**落点**：`crates/mcloud-server/src/main.rs:18`（取 `MCLOUD_ADDR`）、`:30`（`seed_run_ids`）、`:58`（spawn 调度器）、`:73-75`（`bind`）+ `crates/mcloud-engine/src/scheduler.rs:537-538`（「立刻跑一次」）

顺序是：**组装应用 → 把 run id 续号写盘 → spawn 调度器（它第一个 tick 不等间隔，立刻 `tick_and_log()`）→ spawn 日志攒批 → 最后才 `bind`**。
于是 `MCLOUD_ADDR` 写成 `3010` 这种非法值时，报错退出**之前**该进程已经：① 写过 `data/` 的续号；② 完整跑过一轮调度 —— 而那一轮里到点的定时任务是真的会被派发的（兑换 / 红花 / app-task **不可逆**）。
主线程从 `:75` 的 `?` 直接返回 `Err`，`:88-94` 那段优雅收尾（停调度器、`await` ticker、把日志落盘）**一行都不会执行** ⇒ 表现是"启动失败 = 硬切"，与文件头 :78-80 那段"别让它掉出去，否则在途的被拦腰截断"的告诫自相矛盾。

**为什么 `3010` 不等于"端口 3010"**：`main.rs:18` 拿到的串**原样**进 `TcpListener::bind`，仓内没有"只给端口就补 `127.0.0.1`"那层便利；而 std 的 `ToSocketAddrs for str` **要求 `host:port`**，没有冒号就在**解析阶段**直接失败。
12:1x 用一段独立小程序实测（`rustc` 到 `/tmp`，不碰工作区 `target/`）：
```
bind("3010")          => invalid socket address | kind=InvalidInput | raw_os_error=None
bind("127.0.0.1:3010")=> OK
bind(":3010")         => failed to lookup address information…  ← ⚠ 光给端口号写 `:3010` 在 macOS 上也不行
bind("0.0.11.194:0")  => Can't assign requested address (os error 49)
```
`raw_os_error=None` 就是"根本没走到 syscall"的读数 —— 与用户看到的原话逐字吻合。
⚠ 我起初把成因写成"macOS 的 `getaddrinfo` 把 `3010` 按旧式数字 IPv4 解成 `0.0.11.194:0`"，那是拿 **python** 试出来的路径，**在这条链上不成立**（真按那个地址 bind 会报 os error 49，不是 `invalid socket address`）⇒ 已就地改正，见 §6 第 7 条。

**正确用法**（使用者侧，不改代码就行）：`export MCLOUD_ADDR=127.0.0.1:3010`；要局域网访问用 `0.0.0.0:3010`。

**怎么复核**：`sed -n 17,31p crates/mcloud-server/src/main.rs`、`sed -n 58,75p crates/mcloud-server/src/main.rs`、`sed -n 532,542p crates/mcloud-engine/src/scheduler.rs`；解析那三条把上面那段 `try_bind` 存成 `/tmp/m.rs` 用 `$HOME/.cargo/bin/rustc -O /tmp/m.rs -o /tmp/m && /tmp/m` 跑（**别在工作区里 cargo build**，AGENTS.md 那条 TCC/重启坑）。
**修的方向**：把 `bind` 提到 `:22` 之前（只依赖 `addr`，不依赖 `app`），失败就在**任何副作用之前**退出；顺手把 `invalid socket address` 换成人话（"要 `ip:port`，例如 `127.0.0.1:3000`"）—— `addr` 已经在 `with_context` 里了，加一句提示零成本。置信度：**读码确认**（顺序与收尾跳过是代码直读）／**已实测**（那四种地址字符串各自的报错，`raw_os_error` 有无是判据）。

---

## 2. 前端：性能与交互开销

判定口径先钉住（这是本仓自研 `lite`，不是 Vue/React）：JSX 里**动态子节点编译成一个槽 effect**，槽重跑 = `remove(旧) + createNodes(新) + insert`；`createFor` **只有 `row.item === item`（对象身份）才复用行**。
⇒ "少 key 就整表重建"在这里**不自动成立**；真正贵的是"把整个组件包在一个会重跑的槽里"。

编译产物核对（本次我自己写了一份与线上同形的夹具，入口在 `.check-tmp/`、产物在 `/tmp`，均已删除；
**编译成功 ⇒ 下面那段就是真实产物**，但那个页面在无头 Chrome 里没跑起来 ⇒ 只核到"形状"，没量到绝对毫秒数）：

```js
O(e, () => z.value.length > 0 ? j(B, { get lines() { return z.value } }) : null, n)
```

—— 槽订阅了 `lines`，且每轮**重新调用组件工厂** `j(B, …)` ⇒ 整个面板（含全部行）被拆掉重造。

### 2.1 [高] 运行日志：每条 SSE 事件拆掉整块面板重建，且日志不截断

**落点**：`web/src/pages/tasks-page.tsx:667`、`web/src/pages/live-room-page.tsx:910`（同一形状）；`web/src/ui/log-panel.tsx:27`（行渲染）；`tasks-page.tsx:132-134`、`live-room-page.tsx:220`（`push`）

```tsx
{lines.value.length > 0 ? <LogPanel lines={lines.value} /> : null}   // 判空条件包住了整个组件
lines.value = [...lines.value, { kind, text, at: Date.now() }]        // 每条事件换新数组、无上限
```

**代价发生在**：任务运行期间每一条日志事件 ⇒ 已显示的 L 行连同 `card` 壳与每行 class effect 全部重造 ⇒ 一次运行累计 **O(L²)**。
多账号运行时是 N 条 SSE 流灌同一个数组（`tasks-page.tsx` 的 `run()` 每个 run id 一条 `subscribeRun`），L 涨得更快。
`h-64 overflow-y-auto` 那个滚动容器每条事件被换掉 ⇒ **用户往上翻日志会被弹回底部**（这条是症状推断，但由"容器节点被重建"直接得出）。

**怎么复核**：Chrome Performance 录一次真任务，看 Node 增删计数是否随事件数**超线性**；或灌 1000 条看是否掉帧。
修法方向：判空挪进面板内部（`lines.length === 0` 由 `createFor` 自己处理空表），让行按对象身份复用；`push` 加截断（仓内已有正解：`toast.ts:22/43 MAX_TOASTS`、`dashboard-page RECENT_LIMIT`）。

### 2.2 [高] 参数块每次按键整块重建 ⇒ 焦点/光标丢失

**落点**：`web/src/ui/param-field.tsx:26-27`（组件体顶层 `const spec = props.spec; const cur = props.value`）；调用点 `web/src/pages/task-config-page.tsx:143-146`（`value={values.value[s.name]}`，一次按键换新 `values` 对象）；`web/src/pages/pipelines-page.tsx:753-756`（每次 `onInput` 用 `{ ...x, body }` 造新 step 对象 ⇒ `row.item !== item` ⇒ 整张步骤卡含任务下拉一起重建）

**对照着看更清楚**：`web/src/pages/accounts-page.tsx:1192-1199` 用 `get: () => string` **thunk** 把值传下去，`:889-897` 把列表项做成**下标** —— 注释里写明这就是为了"一次按键只动一个 input、保住焦点"。**同一个问题在本仓已有正解**，这两处是没跟上。

**怎么复核**：在任务配置页连打两个字，看第二个字是否落进同一个 `<input>`（Elements 面板里看节点 identity）。

### 2.3 [高] 桌面 / 移动两套列表同时进 DOM

**落点**：`history-page.tsx:331`/`:378`、`schedules-page.tsx:449`/`:461`、`pipelines-page.tsx:609`/`:643`（`hidden … md:block` 与 `grid … md:hidden` 各 `.map` 一遍）

隐藏那一套**同样建节点**：节点 ×2；每行 `fmtEpochMs` ×2（`web/src/ui/time.ts:23-26` 每次调用隐式 `new Intl.DateTimeFormat`）；直播记录每条 `toLiveRoomRunView`（`api.ts` 里若干正则 + `indexOf` 扫描）×2。历史页 100 条时进入即数千节点。

**复核**：Elements 面板数 `tr` / `.card` ≈ 记录数 ×2；窄屏验收（`scripts/gates-web.mjs` 那条）不受影响，因为它只看可见那一套。

### 2.4 [中] 兑换页：分组每轮现算现造新对象 ⇒ 点一次 tab 重建整张清单

**落点**：`exchange-page.tsx:1327`（`groupViews(...).filter(...).map((g) => <details key={g.key} …>`）、`:1314`（同一次渲染里 `groupTabs` 又算一遍）、`:271-295`（`groupPrizes`；比较器里调 `hasOpen = g.items.some(...)` ⇒ 额外 O(g·log g × 件数)）、`:784-788`（注释已承认"清单每读一次就重建这一批节点"）、`:1482/1484/1488/1490/1512`（同一个槽里 `buildPlan(planAt, planLead)` 调 **5** 次）、`:215`（`browserTz()` 每次 `new Intl.DateTimeFormat()`，位于 `:1491` 那个槽内）、`:917-921`（`isSubscribed` 每行 `some`、`liveOf` 每行 `find`）

组对象每轮全新 ⇒ 所有组块连同组内每个 `PrizeRow` 子树整批重建。触发点：点分组 tab、刷新清单、兑完一件（后者本来就要重读，代价可接受；点 tab 那次是纯白付）。

### 2.5 [中] 一次 `load()` 连写多个 ref，全仓 0 处 `batch`

**落点**：`dashboard-page.tsx:306-324`（顺序写 status/runs/schedules/accounts/beans/state），`<StatRow items={metrics()} />`（`:806`）+ `ui/stat-row.tsx:35` 顶层读 ⇒ `metrics()`（两趟 ≤100 条 runs）与四张卡**每写一次信号重建一遍**，一次进页 4~6 遍。`settings-page.tsx:110-118` 同形。

**已实测**（`web/lite/bench`，本机，1000 行页面）：不合批的 3 次写 0.20 ms，包进 `batch` 0.30 ms —— 在这个形状上合批**没有**免费收益（`batch` 自身有成本），所以这条的绝对量级很小；写它是因为**白付的性质**（同一个 UI 被重造 5 遍），不是为了那点毫秒。置信度：读码确认（绝对量 已实测且很小）。

⚠ **改之前先读 `dashboard-page.tsx:40-45` 那段"写 `load()` 的顺序不能动"**：它要求五个信号先写完、`state` 最后写。
这条约束与 `batch` **不冲突**（`batch` 只把 flush 推迟到末尾，写序不变，而 `state` 仍在块内最后），但它解释了为什么这里不能"随手改成先置 `ready`"。别把两件事混着改。

### 2.6 [中·已实测] 列表更新是 O(总行数)/次 ⇒ 逐条追加就是平方

**已实测**（临时探针，`/tmp/lite-bench.html`，跑完删除）：从 1000 行起，每次只追加 **1** 行：

| 行数 | 100 次单行追加 | 每次 |
|---|---|---|
| 1100 | 12.6 ms | 0.126 ms |
| 1800 | 15.2 ms | 0.152 ms |

同样 800 行、分 **8** 批（每批 100 行）= **10.3 ms**。⇒ 单次更新要过整个表两遍，**逐条追加的累积代价是平方**。

根因两处（`web/lite`，属运行时侧、改框架的秤要按 AGENTS.md 那条来比）：
- `web/lite/src/control.ts` `createFor`：每轮先对 `rows.values()` **全行做一次"还活着吗"扫描**（用于父槽已被摘掉时提前退出），再走一遍全量 diff ⇒ 每次更新 2×O(n)。
- `web/lite/src/dom.ts:70-87` `own()`：每次登记都 `list.filter((e) => !e.disposed)` + push + set ⇒ 同父兄弟越多，每次登记 O(兄弟数)。

**这条与 §2.1 是同一条链**：日志面板那个形状把"每次更新的 O(总行数)"乘在了"每条事件重建整块"上，是最坏用法。

### 2.7 [次要] 请求形状

- `exchange-page.tsx:924-937`：`await loadAccounts()` → `loadPrizes()` → `loadSubscriptions()` **串行**，进页至少 3 个 RTT；账号与订阅互不依赖，可并（`Promise.all`，账号页 `load()` 就是这么写的）。
- `web/src/api.ts` `listRuns()` **无分页**：`dashboard-page.tsx:296`、`history-page.tsx:171` 各拿 100 条**完整**记录（含 `outcome/details`，直播那族还含口令全文）。后端 `store.runs(100)` 有上限，所以量级可控，但字段面可以裁。
- `accounts-page.tsx:436-441`：二维码限流后按**写死的 1000 ms** 重启轮询（`retry_after_ms` 只用于第一次延时），与同函数 `:388`「轮询间隔由服务端给：客户端写死一个数会和状态机分叉」相反。而且这一段**没有总时限** —— 出码时拿到的 `ttl_seconds` 只进了 `:387` 那句文案（全文件 `ttl_seconds` 仅此一处），所以码过期后仍在按秒问，直到上游回终态或组件卸载（`:1708`）。
- 反面做得对的（别改）：定时页 60 s 静默刷 + 卸载清理（`schedules-page.tsx:321/325`）、实时日志走 SSE 而非轮询、编排 202 后不摆"会一直转的进度"（`pipelines-page.tsx:347-352`）。

### 2.8 切页遗留：**本报告初稿这一条写错了，现按仓内量测改写**

组件 effect 作用域那条根因（`createComponent` 不给 `effect` 建作用域）已由 `4b208ca` 修掉 ——
`web/lite/src/component.ts` 现在用 `ownedEffects(...)` 包住组件体，`dom.ts` 的 `setNodes` 也把槽 effect 挂到片段首节点上、随 `remove(cur)` 销毁。

**用仓内那条常驻泄漏门复测**（**已实测**：`cd web && node lite/regress/leak.mjs --fixture --rounds 5`；夹具后端 + 固定调试端口 48980，不碰 :3000、不重建 `dist`）：

```
每页访问 5 次（每次停留 2500ms，已预热），Δ 为 GC 之后的净增长：
accounts +0  tasks +0  exchange +0  live-room +0  schedules +0  pipelines +0  history +0  settings +0   ✓
全路由 5 轮：节点 4199 → 4072，监听 89 → 91，堆 1.90 → 2.31 MB（文档里实际只有 336 个元素）
✅ 都在预算内
```

⇒ **每页遗留 0 个节点**，且全路由跑完节点总数**下降** ⇒ 修后没有可测的切页泄漏。

⚠ 我在这条上先给过一个**错读数**：早前一版探针量到"每次访问遗留 48–176 个节点"，那是**停留时间不够**——
那个探针双 rAF 就读数，把"异步数据到达之后才铺的那一层"算进了"遗留"。
`leak.mjs:23-25` 早就警告过：「每页停留 `DWELL_MS`（默认 2500）。停太短测的是'请求还在路上'的中间态 ——
那是另一个现象，会把它误判成泄漏 ✗」。⇒ **量泄漏前先回查仓内既有量测的口径，别自造装置**（同一个坑见 §6 第 4 条）。

**还剩一条读码层面的暴露（不构成当前的泄漏证据）**：`web/lite/src/dom.ts` 的 `flushMounted` / `flushSlots`
都是"有界 8 轮"，8 轮之后**没进文档的条目不丢弃**，留在模块级 `mounts` / `pending` 里 ⇒ 每条拽着 1 个节点 + 1 个闭包。
当前没有任何读数说明它有贡献（上面那组 Δ=0 说明正常情况下都被后续插入接走了），
所以它只是"哪天某个页面的槽深过 8 轮就会沉淀"的结构性口子。**置信度**：读码确认（暴露存在）/ 无读数（不作缺陷）。


## 3. `docs/`：过期、会误导的内容（按危险序）

每条都给了**当前事实**与**复核命令**。前八条我都自己跑过 grep/读码复核，不是转述。

| # | 落点 | 文档现在说什么 | 实际是什么 | 复核 |
|---|---|---|---|---|
| D1 | `docs/usage.md:3-9`（`:8` 那行表格）、`:10`、**`:95`**、`:106` | 「整包**两个**可执行文件…`mcloud-cli`」；产物名 `mcloud-v0.1.0-*.tar.xz`；`:95` 还教人 `xattr -dr com.apple.quarantine ./mcloud-server ./mcloud-cli` | `Cargo.toml:15` 是 `3.0.0-rc.1`；`.github/workflows/release.yml:10-13` 用户口径写明「**只发 `mcloud-server`，不发 `mcloud-cli`**」「unix 一律 `tar.gz`（不再同时给 xz），Windows 用 `zip`」 | `grep -n "只发\|tar.gz" .github/workflows/release.yml \| head` |
| D2 | `docs/task-inventory.md:25-30`、`:39` | 「exchange 已摘掉、共 19 条」「app-task 这个名字今天不存在」 | `crates/mcloud-tasks/src/lib.rs` 的 `register_all` vec 有 **20** 项，含 `Arc::new(exchange::ExchangeTask)`(`:86`)、`AppTaskTask::new()`(`:94`)、`AiAvatarTask::production()`(`:91`) | `grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs`（现算，别背数） |
| D3 | `docs/architecture.md:4`、`:930-947`、`:982` | 「前端 **Vue** Vapor」；列 vue 依赖；`stores/` 目录；「代码分割跑不通」 | 自研 `web/lite`，**零 vue 依赖**（`web/package.json` 连 runtime dependencies 都没有）；无 `stores/`；`app.tsx` 已按页 `import()`，`web/dist/assets` 现在 **20 个 chunk** | `grep -rn "vue" web/package.json`（应 0 命中）；`ls web/dist/assets \| wc -l` |
| D4 | `docs/ai-avatar-spec.md:13-14`、`:146` | 「仓内也无任何 ai-avatar 实现」「本仓**没有** ai-avatar task」 | `crates/mcloud-upstream/src/ai_avatar.rs` 存在且已注册 | `ls crates/mcloud-upstream/src/ai_avatar.rs` |
| D5 | `docs/web/lite-framework.md:498`（§8）、`:419-431` | 「代码分割不做、单 chunk 是刻意的」；「`compiler.ts` 仍然 `import ts from 'typescript'`，构建依赖它」 | 与同文件头部/§5 相反（已按页拆）；`compiler.ts` 只 import `./ast.ts`，`package.json` 里**没有** `typescript`（解析器已换 `oxc-parser`） | `grep -n "^import" web/lite/compiler.ts` |
| D6 | `docs/architecture.md:583`（§7 任务表）、`:204-215` | 任务表列了不存在的名（`app`/`play-ai-lottery`/`hc1t`/`kouling`/`auth-refresh`），漏 `multiple`/`cloud-snapshot`/`task-list`/`app-task`，写"19 个"；工厂函数 `create_http(HttpOptions)`/`create_task_http` | 名单以 `register_all` 现算为准（20）；那两个函数**全仓零命中**，真实装配在 `mcloud-app/src/builder.rs`（`ClientProfile` + `ReqwestClient` + `H5Client` + `SmidClient`） | `grep -rn "create_http\|HttpOptions" crates --include="*.rs"` |
| D7 | `docs/design/cli-webui-design.md:199-201` | 「`GET /api/beans/today` … 端点未落地」 | `crates/mcloud-server/src/routes/beans.rs:40` 已实现、`routes/mod.rs:62` 已 merge | `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3000/api/beans/today`（未登录会 401，401 就说明路由在） |
| D8 | `docs/conventions.md:308`/`:248`、`:26`、`:580`/`:589` | 引 `mcloud-tasks/src/sign.rs`（真名 `signin.rs`）；「58 条测试通过」；门禁「≈100 s / ≈56 s」 | 文件不存在；测试属性有 **1780** 行 `#[test]`/`#[tokio::test]`；`scripts/gates-web.mjs:15-16` 自己写的是 **≈33 s / ≈77 s** | `ls crates/mcloud-tasks/src/signin.rs`；`grep -rh "#\[test\]\|#\[tokio::test\]" crates \| wc -l`；`sed -n 10,20p scripts/gates-web.mjs` |

> ⚠ **D8 的时间读数要带条件看**：`gates-web.mjs` 的 mtime 是 2026-10-02 22:55，比 `AGENTS.md`（同日 17:25）和 `docs/conventions.md`（10-01 12:20）都新 ⇒ 现在**以脚本头部自述为准**；但那个数是作者机器上的耗时，换机就变 —— 更稳的做法是文档里**不写秒数**，只写"默认只跑 `ready`，`--full` 跑三个认证态"。

## 4. `docs/`：同一事实写了好几处、且互相冲突

判据是「**权威源应当是代码，文档只写怎么现算**」。

| 事实 | 现有出处（互相冲突） | 应当的权威源 |
|---|---|---|
| 任务条数 | `architecture.md` §7「19」/P9「19」、`usage.md:47`「18」、`task-inventory.md`「19」、`docs/web/lite-framework.md:397`「18」 | `grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs`（现算 = 20） |
| 门禁耗时 | `AGENTS.md:253`「≈100 / ≈56」、`conventions.md` §9.2 同、`scripts/gates-web.mjs:15-16`「≈33 / ≈77」 | 脚本头部（或直接删掉秒数，只留口径区别） |
| lite 体积 | `architecture.md` §9.3「154433 B 单 JS」、`Cargo.toml` 注释「132846 → 19277」、`docs/web/lite-framework.md` 头 L12 / §5 / §5.1 三个向 | `docs/web/lite-framework.md` 头部（2586 / 5971） |
| CLI 命令数 | `docs/design/cli-webui-design.md`「5 个，不多不少」、`architecture.md:1283`「六件事」/`:1309`「5 件事」、`bin/cli.rs:42`「6 个」 | `bin/cli.rs` 顶层分派 arm 数（现 = 7） |
| 测试条数 | `conventions.md:26`「58」、`docs/design/cli-webui-design.md`「605」、`build-perf.md`「1740」 | `grep -rh` 现算（1780）或 `cargo test` 输出 |

**不要顺手删的"刻意冗余"**：`AGENTS.md` 里的凭据处理、TCC/可移除宗卷那一大段、"什么才需要先问用户"那张表 ——
它们**故意在每个 agent 的上下文里重复出现**，删了会直接退化成"反复弹确认 / 反复误杀服务"。这类重复是护栏，不是冗余。

**体量与归位建议**（本报告**没有执行**）：`architecture.md` 1320 行里 §9.3/§10 的前端历史已被 `docs/web/lite-framework.md`/`lite-guide` 覆盖且过期 ⇒ 抽进 lite 归档、`architecture.md` 留后端；`review-*`、`*-verdict`、`lite-postmortem` 这类快照文件在 docs 索引里统一标注「历史快照，不作当前事实」。

---

## 5. 注释与文档 / 代码矛盾（汇总一遍，方便逐条改）

| # | 落点 | 矛盾 | 状态 |
|---|---|---|---|
| C1 | `mcloud-core/src/kouling.rs:247`、`:318` vs `:398-401` | 字段/方法注释说「按 `schedule_anchor` 筛」「`None` ⇒ 退回不筛、界面照旧有东西显示」，实现是**硬编码比 `KOULING_ANCHOR`**，且 `:355-364` 已删掉退回全量的兜底。`schedule_anchor` 生产**只写不读**（写：`live_room.rs:1370`；读：0） | **已实测**（grep 计数）：按文档理解会以为换个主播就能复用池子，实际永远只认「中国移动云盘」 |
| C2 | `mcloud-tasks/src/lib.rs` `register_all` 上方（约 `:109-119`） | 旧的「⭐ `exchange` **不得**出现在 `register_all` 里…用户点了永远什么都不会发生」没删，**下面直接叠了一条相反的新哨兵**（"必须出现，2026-10-02 起"） | 违反「错记录就地修」：该改写而不是叠着放 |
| C3 | `web/vite.config.ts:47-53` | 「本产物是**单 chunk**（HTML 里 `<link rel=modulepreload>` 数量为 0）」—— 现在 `dist/index.html` 有 1 条 modulepreload（`assets/api-*.js`）、`dist/assets` 20 个 chunk。而注释自己写着「真要做按需拆包时记得回来复核这一条」 ⇒ **触发条件已经满足** | 读码 + 产物核对 |
| C4 | `docs/conventions.md:288` vs `mcloud-core/src/error.rs:68` | 文档规定「判断凭证失效必须用 `is_auth_failure()`」，该函数**零调用**、`AuthExpired` **零构造** | **已实测** |
| C5 | `web/src/api.ts:635`、`dashboard-page.tsx:259-261` vs `routes/beans.rs:40` | 代码注释说「后端还没有这个端点」「它一直是 null、是 404」，实际已实现并挂载 ⇒ 会把下一个审阅者引向"有一发白发的请求" | **已实测** |
| C6 | `mcloud-tasks/src/exchange.rs:1943` | 「生产路径要注入真的 sleep（见 [`wait_until`]）」 —— `wait_until` **不存在**，且生产仍传 `wait: None`（= §1.1 的成因） | **已实测**（grep 只这一处命中）；⚠ **并行写入线已在未提交的工作区里把这条改掉**（`WaitFn` 改 async + 注入 `sleep_until`，注释也改成实话，见 §1.1 的状态更新）⇒ 动手前先 `git diff` 一眼，别重复修 |
| C7 | `bin/cli.rs:42`/`:192`/`:195` | 命令数错、login 模式漏 `device`、教用户跑不存在的 `mcloud serve` | 见 §1.15 |
| C8 | `upstream/Cargo.toml:25`、`client.rs:101-106` | TLS 后端叙述过期（rustls 已被移除；arm64 还要链 OpenSSL） | ⚠ 该 Cargo.toml **正在被并行线修改** |
| C9 | `web/lite/src/dom.ts:150-159` | 注释「合批之后同一串插入只收尾一次」与实现不符：`inserting` 只包住**一次** `insertBefore` 循环，兄弟插入仍逐次 flush（真正的节省来自 `pending.length`/`mounts.length` 那两个空判） | 读码确认 |
| C10 | `session.rs:346` vs `auth_refresh.rs:106-112` | 同仓两份注释对 `serde_json` Map 的输出顺序给出**相反**结论（前者错） | 读码确认 |
| C11 | `mcloud-tasks/src/exchange.rs`（`ExchangeTask` 文档） | 「界面上的开关会等」这一族文案 vs 生产 `wait: None` | 与 §1.1 同源 |

---

## 6. 被推翻的结论（写下来，免得下次照它动手）

1. **「修后每次切页仍遗留 48–176 个节点」—— 这条是**我**在写本报告时造的错读数，已就地删掉**（原 §2.8 的那张八行表）。
   用仓内常驻的 `lite/regress/leak.mjs --fixture --rounds 5` 复测：八页**全部 Δ=0**，全路由 5 轮节点 **4199 → 4072**。
   错因：我的临时探针双 rAF 就读数，把"异步数据到达后才铺的那层"算成了遗留 —— 而 `leak.mjs:23-25` 恰好把这条写成警告。
   现在的结论：切页泄漏**已被 `4b208ca` 修净**（组件体走 `ownedEffects`、`setNodes` 的槽 effect 挂到片段首节点随 `remove(cur)` 销毁），**不要再按"还有残留"规划工作量**。
2. **「`vite.config.ts` 说单 chunk ⇒ 代码分割没做」** —— 现在 `app.tsx` 每页动态 `import()` + hover/focus 预取，`dist` 20 个 chunk。相关三条文档（D3/D5/C3）都是被这条推翻的。
3. **「文档内元素 301→314，证明在累积泄漏」** —— 是我自己的误读：那是**页面大小差异**。
   改判后的最终口径（以 §2.8 那次复测为准）：**文档内元素稳定在 300 出头，切页遗留 Δ=0** —— 既没有"文档内累积"，也没有"游离但可达"的残留。
4. **`MutationObserver` 的「每次切页 +20/−18 个节点」被当成泄漏证据** —— 该计数只统计**直接**增删的顶层节点，天生看不到子树，不能当泄漏判据。判据要用 `Memory.getDOMCounters` + 强制 GC。
5. **上一轮 review 的「run_app_task 执行层零测试」** —— 已过期：`app_task/tests.rs` 现在有 7+ 处直接调 `run_app_task`（黑名单门、`skip_tasks` 等）。
6. **上一轮的「双 `authorization`/`jwttoken`/`deviceid`」** —— 已修（`client.rs:299-312` + `should_inject_credential`，并有回归测试 `:550-568`）。**但同规则的 UA 与 PC 设备头没跟上**（§1.10），别以为这一族收干净了。
7. **我自己 12:1x 在 §1.17 里造的第二条错读数** —— 把 `MCLOUD_ADDR=3010` 失败的原因写成「macOS 的 `getaddrinfo` 按旧式数字 IPv4 把它解成 `0.0.11.194:0`，bind 到一个不属于本机的地址」。那是我拿 **python** 的 `socket.getaddrinfo` 试出来的路径。用 Rust 自己跑一遍就否掉了：`std::net::TcpListener::bind("3010")` 报 `invalid socket address`、`kind=InvalidInput`、**`raw_os_error=None`** ⇒ **连 syscall 都没到**，是 std 的 `ToSocketAddrs for str` 在解析阶段就要求 `host:port`。反证也齐：真按 `0.0.11.194:0` 去 bind 报的是 `Can't assign requested address (os error 49)`，与用户终端上的原话**不是同一条**。⇒ 教训：**跨语言的"解析行为"不能互相顶替**（python 那条走 libc 的宽松规则，Rust 在它前面还拦了一层严格解析）；量到哪一层就说到哪一层。

## 7. 本次没能覆盖 / 需要用户点头的格子

- **没跑运行期门禁**：`cargo test --workspace`、`clippy -D warnings`、`gates-web.mjs --full` 全部没跑（会重链接 `target/debug/mcloud-server`、触发 TCC 授权框、和并行写入线抢 `target/` 独占锁）。所以**本报告不含"编译/测试还全绿"这一条**。§1 的判断都是读码 + 静态计数。
- **`web/dist` 没重建**，:3000 上的托管服务一次都没重启、没 kill。
- **没触发任何真任务**（兑换、红花、app-task 都不可逆），所以 §1.1 的"零请求"、§1.16 的"90001 双扣"停在读码 + grep 层。要闭环就在**库副本**上跑，别写 `data/`。
- **Rust 协议内核的实拍缺口**：重复 `user-agent`/`x-yun-*` 到了 APISIX 取首值还是末值；`tn` 里数组的真实语义（黄金向量不含数组）。
- **切页泄漏这一格已经闭环**（§2.8：仓内常驻门 `leak.mjs --fixture --rounds 5` 量到每页 Δ=0、✅ 在预算内），**没有待归因的残留**。唯一留下的是 `flushMounted` / `flushSlots` "有界 8 轮之后不丢弃"那个**结构性口子** —— 它**没有读数支撑**，所以本报告不把它列为缺陷，也不给"该改成什么"的建议；要闭环得先造一个能压过 8 轮的页面形状。

## 8. 建议处理顺序（按"做错了能不能撤" + 影响面）

1. **§1.1 + C6**（抢整点兑换空跑 + 注释空头）—— 用户看得见的功能没生效，且界面文案是明确承诺；要么注入真 sleep，要么撤承诺。
2. **§1.2 + §1.3**（调度器被长任务占住、链取消丢判定）—— 影响关停安全与历史正确性。
3. **§1.4 + §1.7 + §1.17**（`PUT` 清零记账、弃跑也算一次、`bind` 提到副作用之前）—— 都是小改动、可回滚；§1.17 那条尤其只是**挪几行位置**，却把"地址写错也先跑了一轮不可逆任务"这个窗口关掉。
4. **§3 的 D1–D8 + §5 的 C1–C3**（过期文档与反向注释）—— 纯文本，先清掉最会误导人的那几条（Vue、任务数、端点未落地、`mcloud serve`、schedule_anchor）。
5. **§2.1 + §2.2**（日志面板、参数块）—— 前端"交互级"两条，顺带把丢焦点那个体验问题一起修；仓内有现成正解（`accounts-page.tsx:1192-1199` 的 thunk 形状、`RECENT_LIMIT`/`MAX_TOASTS` 的截断形状）。
6. **§1.5 + §1.9 + §1.10/§1.11**（hub 回收、argon2 出 worker、双头与状态分类）—— 长跑质量与安全面。
7. **§2.6 lite 那两处 O(n)**（`createFor` 存活扫描、`own()` 每次 filter）—— 改框架要上秤（AGENTS.md 那条口径）：这两处只对**大列表**用户付钱，而本仓最大的列表也就 100~2000 行；先把 §2.1/§2.2 的页面形状改掉，很可能就不用动运行时。

---

### 一句话自查

- 想确认这份报告**有没有凭错觉写东西**：§1 的每条我都用 `grep`/`sed` 复跑过一遍（含被 zsh 通配符坑了之后重跑的 `deny_unknown_fields`/`is_auth_failure`/`EventHub` 三组）；§2 的运行时耗时来自仓内常驻的 `web/lite/bench`（跑通）与两个临时探针（其中一个的夹具页在 Chrome 里跑不起来 ⇒ 那组绝对毫秒数**没有**，只剩产物层证据，见 §0 与 §2 开头；探针产物已删、`git status` 干净）；§2.8 的切页遗留用的是仓内常驻门 `node lite/regress/leak.mjs --fixture --rounds 5`（跑通、每页 Δ=0）；§3/§4 的每条都有对照表里的现算命令。
- 想确认**哪些还不能直接照做**：§7 整节 + 所有标「推断」「未证实」的格子。
