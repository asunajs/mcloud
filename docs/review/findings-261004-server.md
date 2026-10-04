# findings-261004-server

- 基线: HEAD 2ca9cf7
- 范围: crates/mcloud-server/, crates/mcloud-app/, crates/mcloud-engine/
- 方法:
  - 通读 `crates/mcloud-server/src/` 的服务侧文件：`auth_gate.rs`、`routes/*`（18 个非测试模块 + `routes/*/tests.rs` 抽查）、`static_files.rs`、`logbuf.rs`、`error.rs`、`state.rs`、`lib.rs`、`main.rs`、`qr.rs`（skim）、`cli/*`（只做目录级浏览，未逐行）；
  - 通读 `crates/mcloud-engine/src/` 的 `runner.rs`（重点 120-620）、`scheduler.rs`（全文）、`pipeline.rs`、`cancel_registry.rs`、`details_ledger.rs`、`artifacts.rs`（skim）、`registry.rs`（skim）；
  - 通读 `crates/mcloud-app/src/` 的 `hub.rs`（全文）、`notify.rs`（60-390）、`push.rs`（840-969 + `build_custom_post`/`safe_reason`）、`builder.rs`（200-260）、`auth_refresh.rs`（60-210）、`smid.rs`（skim）；
  - 交叉核对 `web/src/api.ts`（2056 行，逐个 `request<>()` 调用点与 `web/src/pages/*.tsx` 的消费点）与 `routes/mod.rs` 的路由表、Rust DTO 的 serde 属性；
  - 只读命令：`grep -rn` / `sed -n` / `git rev-parse`，以及对外部依赖源码的只读检查（`rust-embed-impl-8.12.0`、`axum-0.8.9` 的 registry 源码）。
  - **没有**跑 cargo（另一条线在编译）、**没有**跑任何任务/推送、**没有**触碰 :3000 上的服务。
- 结论计数: 高 2 / 中 6 / 低 5（共 13 条）

## 摘要（≤10 行）

1. **一条真会咬人的资源泄漏**：后台多账号运行撞上并发闸时早退**不解取消登记**，`CancelRegistry` 与 `run_owners` 各漏一条且永不回收（双击「运行全部」就能触发）；界面上的「正在运行 N 个」于是只涨不跌。
2. **调度器把任务同步 `await` 在 tick 循环里**：一条跑 >60s 的任务会让同一时刻的其它定时被判「错过太久」而**整体弃掉**（只留一条 warn），用户看到的是「某天没跑」。
3. 契约上最实在的一条：后端有 `RunStatus::Cancelled`（"已中止"），**前端一个字节都不认**——用户自己点的中止在历史/总览里是红色「失败」并计入 failed。
4. 认证面：登录对**不存在的用户名**不跑 argon2（快一个数量级）⇒ 用户名存在性时序侧信道；`POST /api/session` **无限速**，每次登录都是一次 19MiB argon2；登出**不发** `Set-Cookie` 清 cookie。
5. 扫码会话表没有 TTL 清扫，未轮询到终态的会话永久占内存。
6. `customPost` 推送渠道允许任意 URL/方法/头（服务端发请求）= 盲 SSRF；客户端默认跟随重定向。
7. 静态文件路径穿越**已核实不可利用**（rust-embed 8.12 会 canonicalize 并校验前缀）；请求体有 axum 默认 2MB 上限；公开路由清单与三态门逐条核对无误；`web/src/api.ts` 里**没有**不存在的路由，字段大小写也一致（仅 `RunRecord.status` 一处缺口）。

## 发现

### sv-1 [高] 后台运行在并发闸早退时不解登记取消令牌 ⇒ CancelRegistry / run_owners 永久泄漏

- 位置: `crates/mcloud-engine/src/runner.rs:403`（早退），对照 `crates/mcloud-engine/src/runner.rs:390`（另一条早退**有** release）与 `crates/mcloud-engine/src/runner.rs:513`（正常路径 release）
- 证据: 预登记发生在 `runner.rs:212`(`start`) 与 `runner.rs:264`(`start_serial`)，而 `run_impl` 有**两条**早退：

  ```rust
  // runner.rs:389-398 —— 这条自己还回去了（注释里写着"否则取消表里会留一条没人回收的条目"）
  if pre_registered.as_ref().is_some_and(|t| t.is_cancelled()) {
      self.cancel.release(run_id);
      ...
  }
  // runner.rs:402-412 —— 这条没有
  let mut inflight = self.inflight.lock().expect("inflight 锁中毒");
  if inflight.contains_key(&key) {
      return Ok(self.record(RunId::new(0), task_name, account, 0,
          TaskOutcome::skipped("同一任务正在运行中")));   // ← 没有 self.cancel.release(run_id)
  }
  ```

  泄漏链条（同一批代码）：`crates/mcloud-server/src/routes/health.rs:109` 的 `running_count` 就是 `CancelRegistry::len()`（`cancel_registry.rs:80`），前端把它显示成「正在运行 N 个」（`web/src/pages/dashboard-page.tsx:449`）；`crates/mcloud-server/src/state.rs:64` 的 `run_owners` 用 `cancel.is_running(id)` 做 `retain` ⇒ 取消表里那条不走，归属表那条也永远清不掉。
- 复现: `sed -n '386,414p;508,514p' crates/mcloud-engine/src/runner.rs`
- 影响: 生产路径唯一带预登记令牌的是 `web` 的后台多账号运行（`routes/runs.rs:330` `start_serial`，前端**总是**用 `background=true`，见 `web/src/pages/tasks-page.tsx:277`）。触发条件很日常：双击「运行全部」、或定时那一趟还在跑时手点一次。每次撞上泄漏 1 条，永不回收 ⇒ ①「正在运行 N 个」只涨不跌（重启才归零）；②`POST /api/runs/{id}/cancel` 对那些**早已结束**的运行回 `{"cancelled":true,"running":true}`（`routes/cancel.rs:62-67`）。进程跑几个月就是几百条常驻条目。
- 建议: 在 `runner.rs:403` 那条早退前也调 `self.cancel.release(run_id)`；更稳的写法是把登记—回收做成 `run_impl` 里的 guard（`Drop` 时 release），这样以后再加早退分支也不会漏。
- 状态: 仅静态 · 置信度 高

### sv-2 [高] 调度器在 tick 内同步 `await` 整个任务：一条慢任务会吞掉其它定时的触发点

- 位置: `crates/mcloud-engine/src/scheduler.rs:312`（`self.fire(...)`）→ `crates/mcloud-engine/src/scheduler.rs:446`（`self.runner.run(...).await`）→ `crates/mcloud-engine/src/runner.rs:452`（`task.run(&ctx, body).await`）
- 证据:

  ```rust
  // scheduler.rs:309-314（tick 的循环体里，串行 await）
  Decision::Due { point_ms } => {
      self.advance_watermark(&sch, point_ms, true).await?;
      let (outcome, _blocked) = self.fire(&sch, point_ms).await;   // ← 整条链跑完才返回
  }
  // scheduler.rs:444-447
  for acc in accounts {
      match self.runner.run(&sch.task, &acc, &sch.body).await {    // ← 同上
  ```

  而 `run_loop` 里 `tick_and_log().await`（`scheduler.rs:546`）是循环体唯一的推进方式 ⇒ **任何一条定时跑多久，整个调度器就停多久**。恢复后，其它定时的点已经迟到，`decide` 判 `late_by > GRACE_MS`（`scheduler.rs:128`，`GRACE_MS = 60_000`，`scheduler.rs:52`）⇒ `Decision::Missed` ⇒ `scheduler.rs:297-307` **整体弃掉**，只留一条 `tracing::warn`（"错过的触发点已整体弃掉（不逐点补跑）"）。
- 复现: `sed -n '309,318p;425,450p' crates/mcloud-engine/src/scheduler.rs; sed -n '537,546p' crates/mcloud-engine/src/scheduler.rs`
- 影响: 一条 2 分钟的定时（直播那类"等小红花"在 `mcloud-tasks` 里本来就会阻塞数分钟，`listenSeconds` 默认就是分钟级）会让同一时刻的其它定时**整天不跑**；界面只在 `lastStatus` 上留一个 `skipped`，用户不会知道少跑了一趟。这是"用户看到任务没执行"的最可能来源之一。
- 建议: 定时触发改成**不阻塞 tick** 的形态：要么用已有的 `Runner::start_serial`（后台、带 run id）并把记账挪到收尾回调，要么给每条定时 spawn 一个驱动任务 + 每账号一个并发闸；关键是 `tick()` 只做"决定 + 登记"，不等任务体。
- 状态: 仅静态 · 置信度 高

### sv-3 [中] 后端有「已中止」而前端没有：用户主动中止的运行在历史/总览里显示为「失败」

- 位置: `crates/mcloud-engine/src/runner.rs:452`（`Err(e) => TaskOutcome::failed(...)`）与 `crates/mcloud-engine/src/runner.rs:557`（`record.status = RunStatus::Cancelled`）；`web/src/api.ts:95`（`RunRecord` 接口**没有** `status`）；`web/src/pages/dashboard-page.tsx:151`；`web/src/pages/history-page.tsx:193`
- 证据:

  ```rust
  // runner.rs:452-455
  let outcome = match task.run(&ctx, body).await {
      Ok(o) => o,
      Err(e) => TaskOutcome::failed(e.to_string()),   // Cancelled 也走这条
  };
  // runner.rs:557-559
  if was_cancelled { record.status = RunStatus::Cancelled; }
  ```

  中止的源头是 `ctx.check_cancelled()?` ⇒ `Err(mcloud_core::Error::Cancelled)`（`crates/mcloud-core/src/cancel.rs:58`）。后端把它序列化成 `"status":"cancelled"`（`crates/mcloud-core/src/run.rs:37-46`，`rename_all="lowercase"`），而前端：

  ```ts
  // web/src/ui/history.ts:14-19 —— 只有 success/skipped/partial/failed 四档
  // web/src/pages/dashboard-page.tsx:148-152
  for (const r of runs) { ...; t[r.outcome.status]++ }   // 中止计入 failed
  ```
- 复现: `grep -n "status" web/src/api.ts | sed -n '1,12p'; sed -n '145,155p' web/src/pages/dashboard-page.tsx; sed -n '555,560p' crates/mcloud-engine/src/runner.rs`
- 影响: 用户自己点的「中止」在历史行与总览里是红徽章「失败」（`web/src/ui/history.ts:36` `failed: 'badge-soft badge-error'`）并进 `failed` 计数；后端为此专门做了区分（`RunStatus::label()` = "已中止"、`needs_attention()` 把中止排除在告警外，`notify.rs:224-227`），只有 WebUI 不认。对照：通知侧已经明确"中止不许报红"。
- 建议: 后端已经在 `RunRecord.status` 里给了真相 —— 前端把它加进类型（`status?: 'running'|'done'|'cancelled'`）并在 badge/计数里优先看它；或后端把中止的运行 `outcome` 也归到 `Skipped`。两条选一条，别两边都不做。
- 状态: 仅静态（两侧代码逐行核对，未真跑一次中止） · 置信度 高

### sv-4 [中] 登录对不存在的用户名不跑 argon2 ⇒ 用户名存在性时序侧信道

- 位置: `crates/mcloud-server/src/routes/auth.rs:196`（查用户）与 `crates/mcloud-server/src/routes/auth.rs:220`（验口令）
- 证据:

  ```rust
  let user = state.handle.store.user_by_name(&body.username).await?
      // ⚠ 用户不存在与口令错**回同一句话**：区分开来就是一个"这个用户名存不存在"的探测器。
      .ok_or_else(|| ApiError(mcloud_core::Error::Auth("用户名或口令错误".into())))?;   // ← 立即返回
  ...
  mcloud_crypto::verify_password(candidate, ...)?;   // ← 只有用户存在才走这里
  ```

  `verify_password` 是 argon2id `Argon2::default()`（`crates/mcloud-crypto/src/password.rs:93-95`），参数为 `m=19456,t=2,p=1`（该文件头 15 行写明）—— 单次十毫秒级。注释只保证了**句子相同**，没有保证**耗时相同**。
- 复现: `sed -n '196,222p' crates/mcloud-server/src/routes/auth.rs; sed -n '14,16p;93,96p' crates/mcloud-crypto/src/password.rs`
- 影响: 未认证的攻击者按响应时间就能枚举出实例上真实存在的用户名（本工具是单管理员，用户名往往就是叫 `admin` 之类的可猜值 —— 枚举出来之后就能把暴力破解集中到唯一目标上）。
- 建议: 用户不存在时也对一个固定的假 PHC 串跑一次 `verify_password`，抹平两条分支的耗时。
- 状态: 仅静态 · 置信度 高

### sv-5 [中] `POST /api/session` / `POST /api/setup` 无任何限速：未认证即可打满单核 + 每次 19MiB argon2

- 位置: `crates/mcloud-server/src/routes/auth.rs:184`（login）、`crates/mcloud-server/src/routes/auth.rs:126`（setup_create）；`crates/mcloud-server/src/lib.rs:26-48`（整个 Router 没有任何限速/并发层）
- 证据: `grep -rn "governor\|RateLimit\|rate_limit\|ConcurrencyLimit\|TimeoutLayer" crates/mcloud-server/src/` **零命中**（退出码 1）；而每条登录请求都会执行一次 argon2id（19 MiB × 2 轮）。这两个路由在 `crates/mcloud-server/src/auth_gate.rs:59-66` 的 `PUBLIC` 清单里 ⇒ **不需要任何凭据**。
- 复现: `grep -rn "governor\|RateLimit\|ConcurrencyLimit\|TimeoutLayer" crates/mcloud-server/src/; grep -n "PUBLIC" -A 8 crates/mcloud-server/src/auth_gate.rs`
- 影响: 一条 `for i in $(seq 100); do curl -s -X POST …/api/session & done` 就能占满这台机器的 CPU（每次请求 ~20-50ms 单核 + 瞬时 19MiB 内存），把正在跑的定时任务挤慢 —— 而 `MCLOUD_ADDR` 是**可配**的（`crates/mcloud-server/src/main.rs:59`），一旦有人按文档绑到 `0.0.0.0`，这就是一个不需要认证就可达的 DoS 面。
- 建议: 给 `/api/session`、`/api/setup` 挂一个按 IP 的令牌桶（例如 `tower_governor`），或退一步：失败计数 + 指数退避（同一用户名/IP 连续失败 N 次后延迟响应）。
- 状态: 仅静态 · 置信度 高

### sv-6 [中] 登出不清 cookie：`DELETE /api/session` 只删服务端会话，不回 `Set-Cookie Max-Age=0`

- 位置: `crates/mcloud-server/src/routes/auth.rs:260-274`（logout），对照 `crates/mcloud-server/src/routes/auth.rs:377-386`（`set_cookie_response`，只有 login/setup/change_password 用）
- 证据:

  ```rust
  pub async fn logout(State(state): State<ServerState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
      if let Some(token) = cookie_token(&headers) {
          state.handle.store.remove_session(&hash_session_token(&token)).await?;
      }
      Ok(StatusCode::NO_CONTENT)   // ← 没有 Set-Cookie
  }
  ```

  cookie 是 `HttpOnly`（`auth.rs:384`）⇒ 前端 JS 也删不掉它，只能等服务端回一个过期的 `Set-Cookie`。
- 复现: `sed -n '255,275p;377,386p' crates/mcloud-server/src/routes/auth.rs`
- 影响: 登出后令牌**仍然留在浏览器里**（服务端已失效，所以不是可用的会话），但在共用机器 / 后续任一 XSS 场景下，那是一枚"看着还在"的凭据；而 cookie 的 `Max-Age` 是登录时那份（默认按 `session_ttl_hours`，可到几十小时）。
- 建议: `logout` 回一个 `Set-Cookie: mcloud_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0` 的 204。
- 状态: 仅静态 · 置信度 高

### sv-7 [中] 扫码会话表没有任何 TTL 清扫：未轮询到终态的会话永久留在内存

- 位置: `crates/mcloud-server/src/routes/login.rs:51-53`（`LoginSessions { qr: Mutex<HashMap<String, QrSession>> }`）、插入在 `crates/mcloud-server/src/routes/login.rs:149`
- 证据: 这张表的所有触点：

  ```
  login.rs:149  put_back   ← POST /api/login/qr 插入（每出一张码一条）
  login.rs:202  take       ← 轮询时取出
  login.rs:239/255/267 put_back ← 非终态放回
  login.rs:278  remove     ← 只有显式 DELETE /api/login/qr/{id}
  ```

  **没有任何按时间的清理**（`QrLogin` 自己有 `ttl_seconds`/`started_at_ms`，见 `crates/mcloud-tasks/src/login.rs:108-109`，但那只在轮询时被 `decide` 用，不会把条目从表里摘掉）。`take` 失败（会话不在）时是直接 404（`login.rs:205`）。
- 复现: `grep -n "login_sessions\|fn remove\|fn take\|fn put_back" crates/mcloud-server/src/routes/login.rs`
- 影响: 每次「出了一张码但没轮询到终态」都留下一条常驻条目 —— 用户点开登录框看一眼就走、页面刷新、扫码超时后前端不再轮询（前端在终态才 `close`），都命中。单条不大（`QrLogin` + tid），但**只进不出**，长跑实例里单调增长；这也是"实例跑久了内存慢慢涨"最容易复现的一条。
- 建议: 在 `put_back`/`insert` 里顺手 `retain(|_, s| s.qr 未过期)`（`QrLogin` 已经把 `started_at_ms`+`ttl_seconds` 带在手里），或在 `LoginSessions` 上记 `inserted_at` 并做同样的事。
- 状态: 仅静态 · 置信度 高

### sv-8 [中] 推送 `customPost` 接受任意 URL / 方法 / 头 ⇒ 服务端盲 SSRF（且默认跟随重定向）

- 位置: `crates/mcloud-app/src/push.rs:864`（`send_custom_post`）与 `build_custom_post` 里的 URL 直接取用（`crates/mcloud-app/src/push.rs`，见证据）；实际发请求在 `crates/mcloud-app/src/notify.rs:83-99`
- 证据:

  ```rust
  // push.rs（build_custom_post）：url 只判空，不判 scheme/host
  if api.url.is_empty() { return Ok(None); }
  let method = api.method.as_deref().map(str::to_uppercase).unwrap_or_else(|| "POST".into());
  let url = api.url.replacen("{title}", ...).replacen("{text}", ...);
  ```

  `url` 是**用户配置里的整条地址**，不只有 `customPost`：企业微信机器人同样直接用它——`crates/mcloud-core/src/push.rs:242`(`WorkWeixinBotConfig.url`) → `crates/mcloud-app/src/push.rs:397`(`build_post(config.url.clone(), ...)`)；TG 还多一个 `api_host`（`crates/mcloud-core/src/push.rs:258`）也由用户给。配置来源是每账号设置（`PUT /api/accounts/{id}/settings`，`crates/mcloud-server/src/routes/accounts.rs:528`），客户端是**专用** reqwest（`crates/mcloud-app/src/builder.rs:220-226`，注释写着"云盘凭据一个都不能给"），重定向没设策略 ⇒ 用 reqwest 默认（跟随 ≤10 跳，`crates/mcloud-upstream/src/client.rs:205-209` 的 builder 只设了 timeout/cookie/gzip）。
- 复现: `grep -n "fn build_custom_post" -A 12 crates/mcloud-app/src/push.rs; sed -n '388,398p' crates/mcloud-app/src/push.rs; sed -n '218,226p' crates/mcloud-app/src/builder.rs`
- 影响: 有权改某个账号设置的人可以让**服务进程**向任意地址发任意方法的请求（`http://127.0.0.1:3000/api/system/restart`、`http://169.254.169.254/…`、内网段探测），请求自带任意头；响应体不回给调用方，所以是**盲** SSRF（能拿到状态码，状态码进推送日志）。单用户/本机部署下这只是"管理员自伤"，但本仓的归属模型明确支持多用户，且 `MCLOUD_ADDR` 可绑到 LAN ⇒ 那时第二个用户就能把它当内网探针用。另外 serverchan / day.app / 2im 这类渠道把 token 放在 **URL query** 里，跟随重定向时会把这枚 token 交给跳转目标。
- 建议: 在 `AccountSettingsDto::from_wire`（或 `notify` 投递前）对 `customPost.url` / `workWeixinBot.url` / `tgBot.apiHost` 这类用户可控 URL 做一次校验：只允许 `http(s)`、拒绝解析到 loopback/私网/链路本地/元数据地址，并把 reqwest 的 `redirect::Policy::none()` 用上。
- 状态: 仅静态 · 置信度 中（可达性高；实际危害取决于部署拓扑）

### sv-9 [低] `/api/runs/{id}/logs` 与 `/events` 的可见性口径不一致（且该前端函数是死代码）

- 位置: `crates/mcloud-server/src/routes/logs.rs:38`，对照 `crates/mcloud-server/src/routes/runs.rs:140`（`run_access`）
- 证据:

  ```rust
  // routes/logs.rs:38 —— 只认存储层
  if state.handle.store.run_for(&cur.0, run_id).await?.is_none() {
      return Err(ApiError(mcloud_core::Error::NotFound(format!("运行记录不存在：{id}"))));
  }
  ```

  而记录是**运行结束**才落盘的（`runner.rs:561`），同一时刻 `/api/runs/{id}/events` 走 `run_access` 的三来源（服务端登记表 / 存储 / 内存 `Started` 事件，`runs.rs:137-139`）→ 200。
- 复现: `grep -n "run_for\|run_access" crates/mcloud-server/src/routes/logs.rs crates/mcloud-server/src/routes/runs.rs | head`
- 影响: 运行中的那次运行，`/logs` 一律 404「运行记录不存在」，而 SSE 是 200 —— 口径不一致。今天**没有用户可见影响**，因为 `web/src/api.ts:1026` 的 `getRunLogs` 在整个 `web/src/` 里**零调用点**（`grep -rn "getRunLogs" web/src/` 只命中定义行）；一旦有人接上（补历史页的按需拉日志），就会得到"日志接口偶发 404"。
- 建议: 让 `/logs` 复用 `run_access`（Denied/Unknown → 404，Allowed → 再回日志），顺手删掉或接上 `getRunLogs`。
- 状态: 已实测（`grep` 证明零调用点） · 置信度 高

### sv-10 [低] 事件中枢：对"已被回收的终态 run"订阅会新建一个**永不回收**的非终态条目

- 位置: `crates/mcloud-app/src/hub.rs:156`（`subscribe` 的 `entry(run_id).or_default()`），回收条件在 `crates/mcloud-app/src/hub.rs:324-339`
- 证据:

  ```rust
  pub fn subscribe(&self, run_id: RunId, since: Seq) -> u64 {
      ...
      let state = inner.runs.entry(run_id).or_default();   // ← 没有就建一个新的
  ```
  ```rust
  // evict_terminal_runs 的三个条件
  .filter(|(id, s)| s.terminal && s.subscribers.is_empty() && pending…)
  ```

  新建出来的 `RunState` 的 `terminal` 恒为 `false`（只有 `publish(Finished)` 才会置位，`hub.rs:274-277`）⇒ 永远不满足回收条件。触发路径是现成的：`run_access` 因为"存储里有记录"而判 `Allowed`（`runs.rs:157-162`）但对内存里已被 `MAX_TERMINAL_RUNS_KEPT = 8` 挤掉的旧 run，`subscribe` 会凭空造一条。
- 复现: `grep -n "or_default()" crates/mcloud-app/src/hub.rs; sed -n '333,344p' crates/mcloud-app/src/hub.rs`
- 影响: 每条这种订阅留下一个永久表项（表项本身很小：空 `Vec` + 空 `HashMap` + 计数器），且 `flush()` 每 2 秒遍历一次 `runs` 时都要多看它一眼。属于"阈值很低但会一直涨"的一类；今天前端只对**活**运行订阅，所以量很小。
- 建议: `evict_terminal_runs` 的候选条件里加上"非终态且**没有任何订阅者**且已存在超过 N 秒"，或在 `subscribe` 里对"库里已终结的 run"不建内存态（改为一次性的历史回放）。
- 状态: 仅静态 · 置信度 中

### sv-11 [低] 任务体 panic 会把 `inflight` 与取消登记**永久**留在表里（该 task:account 之后每次都被跳过）

- 位置: `crates/mcloud-engine/src/runner.rs:413`（`inflight.insert`）→ `crates/mcloud-engine/src/runner.rs:512`（`remove`）之间没有 `Drop` 守卫
- 证据: `run_impl` 的清理只在函数**正常走到底**时执行：

  ```rust
  let outcome = match task.run(&ctx, body).await {   // runner.rs:452 —— panic 会在这里 unwind
      Ok(o) => o,
      Err(e) => TaskOutcome::failed(e.to_string()),  // 只处理 Err，不处理 panic
  };
  ...
  self.inflight.lock()...remove(&key);               // runner.rs:512
  self.cancel.release(run_id);                       // runner.rs:513
  ```

  `task.run` 是 `mcloud-tasks` 里成百上千行的业务代码（`unwrap/expect` 不罕见），而 spawn 出去的 future 一旦 panic，`tokio` 只会把 JoinHandle 标成 cancelled，**不会**替我们跑清理。
- 复现: `sed -n '450,456p;508,514p' crates/mcloud-engine/src/runner.rs`
- 影响: 该 `task:account` 之后**每一次**运行都会撞上 `runner.rs:403` 那条并发闸、返回 `Skipped("同一任务正在运行中")` —— 表现为"这个任务从此再也跑不起来"，且 `Finished` 事件永不发出 ⇒ 正在看的 SSE 连接会一直挂着（只有 keep-alive），前端永远停在「执行中」。与 sv-1 叠加：那条泄漏的登记还会让「正在运行」计数也跟着错。
- 建议: 用 guard 结构（`struct InflightGuard<'a>`，`Drop` 里 `remove` + `cancel.release`），或 `tokio::spawn` 里套 `AssertUnwindSafe(fut).catch_unwind()` 并在 catch 分支补发 `Finished(Failed)` + 清理。
- 状态: 仅静态 · 置信度 中

### sv-12 [低] `dropped` 的说法与实现不符：前端收不到这个信号，且同一订阅者会反复告警

- 位置: `crates/mcloud-app/src/hub.rs:90`（`dropped: usize`，注释写着"累计且不自动清零"）与 `crates/mcloud-server/src/routes/runs.rs:402-410`
- 证据:

  ```rust
  // routes/runs.rs:402-410
  if delivery.dropped > 0 {
      // 前端看到这条提示应重新订阅（带上已收到的最大序号）。
      tracing::warn!(run = %st.run_id, dropped = delivery.dropped, "SSE 订阅者落后，已丢弃部分事件");
  }
  ```

  这只是一条服务端 `tracing::warn`（进 `logbuf` 环，见 `logbuf.rs:113-129`），SSE 上并没有对应事件 —— 前端 `RunEvent` 联合类型里没有任何 dropped/error 档（`web/src/api.ts:181-199`），"前端看到会重新订阅"这件事**代码里不存在**。同时 `Subscriber.dropped` 累计不清零，`drain` 每次都把它回给调用方 ⇒ 一旦掉过队，之后每一次唤醒都会再打一条同样的 warn（`hub.rs:190-197`）。
- 复现: `grep -n "dropped" crates/mcloud-app/src/hub.rs crates/mcloud-server/src/routes/runs.rs | head; sed -n '181,199p' web/src/api.ts`
- 影响: ①真正的丢事件对用户**完全不可见**（只在服务端日志里），用户看到的是"日志少了一截"却没有任何提示；②日志环会被同一批重复 warn 冲刷，把真正有用的近因挤出 2000 行的窗口。
- 建议: 要么按注释把 dropped 做成一条真实 SSE 事件（`event: dropped`）并让前端重订阅，要么把注释改成事实（纯服务端观测），并把 `dropped` 在 `drain` 后清零、避免重复告警。
- 状态: 仅静态 · 置信度 中

### sv-13 [低] 死代码：几处导出/接口在生产路径与前端都没有调用点

- 位置: `crates/mcloud-engine/src/pipeline.rs:307`（`single_step_pipeline`）、`crates/mcloud-engine/src/cancel_registry.rs:85`（`running()`，只有它自己第 155 行的单测命中）、`crates/mcloud-app/src/hub.rs:207`（`is_tracked`，只有 `hub/tests.rs` 命中）、`crates/mcloud-server/src/routes/pipelines.rs:249-250`（`_assert_send_sync`，自己带着 `#[allow(dead_code)]`）、`web/src/api.ts:1026`（`getRunLogs`）
- 证据:

  ```
  $ grep -rn "single_step_pipeline\|\.running()\|is_tracked\|getRunLogs" crates/ web/src --include=*.rs --include=*.ts --include=*.tsx
  crates/mcloud-engine/src/pipeline.rs:307:pub fn single_step_pipeline(...)      ← 只有定义
  crates/mcloud-engine/src/cancel_registry.rs:155:  assert!(reg.running().is_empty());  ← 生产路径零调用，只有它自己的单测
  crates/mcloud-app/src/hub.rs:207:pub fn is_tracked(...)   ← 只有 hub/tests.rs 命中
  web/src/api.ts:1026:export const getRunLogs = ...         ← 只有定义（见 sv-9）
  ```

  另外 `Runner::start`（`runner.rs:196`）在生产路径**没有**调用点（`start_serial` 才是服务在用的那条，`routes/runs.rs:330`），只有 `crates/mcloud-app/tests/run_id_seed.rs:90` 用。
- 复现: `grep -rn "single_step_pipeline\|\.running()\|is_tracked\|getRunLogs" crates/ web/src --include=*.rs --include=*.ts`
- 影响: 都是"看起来是能力、其实是摆设"的接口：`single_step_pipeline` 全仓**零命中**（连单测都没有），`running()`/`is_tracked()`/`getRunLogs` 只有单测或只有定义 —— 尤其 `single_step_pipeline` 容易被当成"只跑一步"的入口去接线。属于维护成本，不是运行期缺陷。
- 建议: 删掉，或在文档/测试里给出唯一用途（`Runner::start` 若只服务测试，就在文档里写明"生产走 `start_serial`"）。
- 状态: 已实测（grep 全仓） · 置信度 高

## 已核实为「不是问题」的项（不作为发现，供对照）

- **静态文件路径穿越**：`WebAssets::get(path)` 走的是 rust-embed 生成的代码，它在 8.12.0 里**先 canonicalize 再校验前缀**（`rust-embed-impl-8.12.0/src/lib.rs` 的 `if !canonical_file_path.starts_with(#canonical_folder_path) { … return None }`），`/../…` 拿不到 `web/dist` 之外的文件；`assets/` 未命中回 404（`static_files.rs:137-140`）也正确。唯一的缺口是"指向外部的**符号链接**会被放行"（上游自己标注的 TODO，需本地写权限，未计入）。
- **请求体大小**：axum 默认给每个请求体套 2 MiB 上限（`axum-core-0.5.6/src/ext_traits/request.rs:319` 的 `const DEFAULT_LIMIT: usize = 2_097_152;`，第 326 行在"没有 `DefaultBodyLimit` 扩展"时就是走这个值），本项目没有 `DefaultBodyLimit::disable`（`crates/mcloud-server/src/lib.rs:26-48` 只挂了 compression / 登录闸 / Trace），所以没有无上限的 body 面。<br>复现: `sed -n '316,328p' ~/.cargo/registry/src/index.crates.io-*/axum-core-0.5.6/src/ext_traits/request.rs`
- **CSRF**：会话 cookie 是 `HttpOnly; SameSite=Strict; Path=/`（`auth.rs:383-384`），无 CORS 层；写接口全在 `/api/` 且方法非 GET ⇒ 跨站表单/`fetch` 都被 SameSite + 预检挡住。唯一缺口是 Host 头未校验（DNS rebinding 场景），本次未找到可利用的完整链，故未列为发现。
- **路由/字段契约**：逐个核对 `web/src/api.ts` 的 40 余个 `request<>()` 调用点与 `routes/mod.rs` + 各 `routes/*.rs` 的 `route()`：**没有不存在的路由**，方法也都对得上；`RunRecord`/`AccountSummary`/`DeviceResponse` 是故意 snake_case，前端也按 snake_case 读；`ServiceStatus`/`Schedule`/`SecurityView`/`PrizeView`/`SubscriptionView` 的 `rename_all="camelCase"` 与 TS 接口逐字段一致。唯一缺口是 sv-3 的 `status`。
- **推送错误文案**：`safe_reason = strip_urls(redact(raw))`（`crates/mcloud-app/src/push.rs`），`reqwest::Error` 里带的 URL（可能含 `access_token=`）会被剥掉 ⇒ 推送 token 不会随失败日志外泄。已核。

## 未覆盖

- **没有跑任何编译/测试**（遵守"别抢 CPU"）：所有结论都是静态阅读 + `grep`/`sed`；`cargo check -p mcloud-server -p mcloud-app -p mcloud-engine` 未执行，所以"能编译"这件事本报告不作断言。
- **没有实测任何一条**（不跑任务、不发通知、不碰 :3000）：sv-1/sv-2 的行为链是逐行读出来的，若要钉死需要一次"故意撞车"的运行 —— 那会产生上游副作用，按纪律没做。
- `crates/mcloud-app/src/push.rs` 只在 840-969 与 `build_custom_post`/`safe_reason` 两处细读，各渠道的**载荷形状**（pushplus/tg/bark/dingtalk 的字段映射）没有逐条与 TS 权威实现比对。
- `smid.rs`（854 行）只做了结构级浏览 + `grep`，没有核对指纹计算的正确性；`login.rs`（325 行）、`sink.rs`、`registry.rs`、`qr.rs` 的编码矩阵、`routes/beans.rs` 的上游读取链都没细读。
- `crates/mcloud-server/src/cli/*`（5 个文件）不属于"服务/应用/引擎"三块的核心，未审。
- 前端只审了"契约"这一层（路由/字段/状态码），`web/src/pages/*` 的交互与状态机未审。
- 多用户越权只做了**代码级**核对（`accounts_for`/`account_for`/`run_for`/`schedules_for`/`pipeline_for`/`visible_to` 的调用点都走归属），没有构造双用户实例实测；`GET /api/status` 的 `runningCount`（全局量，`health.rs:109`）在多用户下会泄露"别人在跑几个任务"，量级很小，未单列成发现。
