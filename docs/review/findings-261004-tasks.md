# findings-261004-tasks

- 基线: HEAD 2ca9cf7
- 范围: crates/mcloud-tasks/（29 文件 / 30647 行；其中非测试源码约 1.15 万行）
- 方法: **只读**。先 `git rev-parse --short HEAD`、`find crates/mcloud-tasks -name '*.rs' | xargs wc -l`
  摸清体量；再按题面五问建索引：`grep -rn` 打 `acted` / `check_cancelled` /
  `tokio::time::sleep` / `let _ =` / `unwrap_or_default` / `\.ok()` / `contains("` /
  `split(` / `file_create_request` / `cloud_trash` / `register_cloud_file`；
  用「每个文件的循环头 vs 它的 check_cancelled 行」逐文件对齐，找出没有检查点的循环；
  然后 `read` / `sed -n` 逐个读 `app_task.rs`（全 1513 行）、`app_task_matrix.rs`(全)、
  `signin.rs`(全)、`task_expansion.rs`(关键)、`exchange.rs`(判据面)、`live_room.rs`(关键)、
  `beans.rs`(关键)、`notice.rs`(关键)、`ai_avatar.rs`(全)、`cloud_phone_redpack.rs`(关键)、
  `msg_push.rs` / `play_ai.rs` / `fun_ai.rs` / `login.rs`(关键)。
  **未跑 cargo、未跑 `mcloud run`、未碰 :3000**。所有行号均以 `grep -n`/`sed -n` 现场核对。

## 摘要（≤8 行）

1. `app-task` 的 `acted` 有两套判据：434 要求"上游次数真涨了"，而上传/埋点档只要
   "写请求发成功"就算数 —— 同一文件自己的注释记着真机 106 实测 `clicked:true`
   而上游 `state` **始终 WAIT**，它照样返回 `TaskOutcome::success`。
2. 上传产物的回收**只挂在链尾**：`check_cancelled()?` / 传输错误提前 return 就绕过它。
   `app-task`、`task-expansion`、`ai-avatar` 三条会写网盘的链**同型**；
   而 `ArtifactLedger::pending()` 是 **run 维**的（`engine/artifacts.rs:419-420`），
   没有任何 reaper 跨 run 兜底。
3. `app_task::upload_one_file` 在 `create` 成功后、`PUT`/`complete` 失败时**丢掉 file_id**
   ⇒ 那条我们造的文件永不进 `cleanup` 名单（对照 `task_expansion.rs:523` 是提前落账的）。
4. 取消检查覆盖不均：`app_task::refetch_items` 16 连发、`beans.rs` 两个分页循环
   （函数签名里**没有**令牌）、`live_room` 的"再听一轮"循环与 60 s 长收听，都没有检查点。
5. 静默失败少而集中：`cloud_phone_redpack.rs:1254` 的 `success` falsy 静默 `continue`
   且最终归因成"未找到可安装应用"；`refetch_items` 的 `if let Ok` 丢掉部分失败不留痕。
6. 固定文案/字段名依赖集中在三处：`beiyong1_name_gate`、`beiyong1_button_gate`、
   `KNOWN_ANSWERS`（15 条题库表）。前两处有注释与迁移取向，第三处会**如实**转人工。
7. **反例（说明这不是"做不到"）**：`exchange::settle_ordered`（余额差核）、
   `cloud_phone_redpack::settle`（回查 userStatus==1）都是本仓已有的硬判据写法。

## 发现

### tk-1 [高] `acted` 两套口径：434 要"上游记账"，上传/埋点只要"请求发成功"

- 位置: `crates/mcloud-tasks/src/app_task.rs:1108`、`:1203`、`:947`、`:1383`
- 证据: 三条分支三种判据 ——

  ```rust
  // Entry（409/1005）：只要 click 那一枪没报错
  1108:            if step.clicked {
  1109:                result.acted += 1;
  // Upload（106/522/113/beiyong1）：传上去了**或**打过卡
  1203:        if step.uploaded > 0 || step.clicked {
  1204:            result.acted += 1;
  // 434：唯一判据是"上游次数涨了"（share_counted，由 settle_share 给）
  ```

  同一文件 `:1383-1384` 自己记着真机读数：

  ```text
  2026-10-04 用户实测 106 —— 那趟记录是 `{uploaded:1, clicked:true, problems:[]}`，
  可上游的 `state` 始终是 `WAIT` ✗。
  ```

  而 `click()`（`:1398-1408`）只做 `ensure_http_ok` + `parse_task_click`；后者
  只 `env.ensure_ok(...)`（`mcloud-upstream/src/app_task_click.rs:81`），实拍四发
  全是 `{"code":0,"msg":"success"}`（同文件 `:68-70`）—— **没有任何一格携带"记上了"**。
  于是 `:947` 的 `planned > 0 && acted == 0` 一旦为假，`:953` 就
  `TaskOutcome::success(summary)`，小结念"App 任务做了 N 条"。
- 复现: `grep -n "if step.clicked {\|if step.uploaded > 0 || step.clicked {\|TaskOutcome::success(summary)\|state\` 始终是 \`WAIT" crates/mcloud-tasks/src/app_task.rs`
- 影响: 上游没记账的一趟被报成 success（历史页绿、`onlyError` 推送不触发、用户以为做完）。
  与 434 档"次数没动就报失败"的口径**互相矛盾**，也与本仓两处硬判据
  （`exchange.rs:1449` 的余额差、`cloud_phone_redpack.rs:935` 的回查 `userStatus==1`）不一致。
- 建议: 上传档复用收尾那次整表读数（`settle_progress` 已经在拉），把该条目的
  `state == FINISH` 或 `process` 上涨当第二判据；至少在 `details` 里分出
  `clickedButNotCredited`，并把 `acted` 的文档从"真的推进了"改成"两档判据不同"。
- 状态: 仅静态 · 置信度 高

### tk-2 [高] 上传产物的回收只挂在链尾 ⇒ 取消 / 异常早退就漏（三文件同型）

- 位置: `crates/mcloud-tasks/src/app_task.rs:1218`、`:1045`、`:1181`；
  `crates/mcloud-tasks/src/task_expansion.rs:430`、`:392`；
  `crates/mcloud-tasks/src/ai_avatar.rs:928`、`:922`、`:893`
- 证据: 三条会写网盘的链**都是**"清理只在最后一行调用" ——

  ```rust
  // app-task
  1218:    cleanup(ctx, &mut result).await;
  // 而前面两处 ? 会直接返回：
  1045:        ctx.check_cancelled()?;      // 逐条循环头
  1181:            ctx.check_cancelled()?;  // 上传循环头；此刻 file_ids 可能已非空

  // task-expansion（:523 已落账 upload_file_id，但）
  392:        let second = read_once(ctx, device_id).await?;   // 失败 ⇒ 直接 return
  430:    cleanup_uploaded(ctx, &mut result).await;            // 走不到

  // ai-avatar
  893:                        Err(e) if is_transport(&e) => return Err(e),  // 前几轮已生成的丢
  922:        ctx.check_cancelled()?;
  927:        ctx.check_cancelled()?;
  928:        collect_and_clean(...)   // ← 清点与清理都在这一步，早退就没有它
  ```

  `ai-avatar` 更重一层：它的产物**只在 `collect_and_clean` 里登记**（`ai_avatar.rs:729`
  `ctx.register_cloud_file(...)`），所以早退时**账上也没有**；而 `pending()` 是
  run 维的（`crates/mcloud-engine/src/artifacts.rs:419-420` 调 `pending_for(self.run_id, …)`），
  全仓 `grep` 找不到任何跨 run 的 reaper（`pending_for` 只出现在该文件与它的测试里）。
- 复现: `grep -n "cleanup(ctx, &mut result)\|cleanup_uploaded(ctx, &mut result)\|collect_and_clean(\|ctx.check_cancelled()?\|read_once(ctx, device_id).await?" crates/mcloud-tasks/src/app_task.rs crates/mcloud-tasks/src/task_expansion.rs crates/mcloud-tasks/src/ai_avatar.rs`
- 影响: 用户网盘根目录/活动目录里留下随机名废弃文件；取消得越晚留得越多；
  读数里 `fileIds=0 / cleanedUp=0`，小结反而显得干净。`ai_avatar` 的还会
  在**下一趟**被记成 `pre_existing`（基线里已有）⇒ 以后也不会被清。
- 建议: 把"已造产物"的登记提前到 `create` 返回 `exist==false` 的那一刻；再让
  `Task::run` 的错误/取消分支走一次 best-effort 回收（`Result` 包一层，或
  在 runner 侧按同一个 ledger 兜底），而不是只在链尾一行。
- 状态: 仅静态 · 置信度 高

### tk-3 [中高] `upload_one_file` 在 `create` 成功后丢掉 file_id（PUT/complete 失败即漏一个文件）

- 位置: `crates/mcloud-tasks/src/app_task.rs:1308-1330`
- 证据: file_id 只在**整条链都成**之后才交回调用方；中间任何一枪失败都把它丢掉 ——

  ```rust
  1308:    let Some(file_id) = created.file_id.clone() else {
  1312:        let Some(url) = created.upload_url.clone() else {
  1319:        upload::ensure_put_ok(&resp)?;            // ← Err 上抛，file_id 丢弃
  1320:        let req = upload::file_complete_request(
  1328:        upload::parse_file_complete(&resp.body)?;  // ← 同上
  1330:    Ok(Some(file_id))
  ```

  调用点 `:1184-1194` 只把 `Ok(Some(id))` 推入 `result.file_ids`：

  ```rust
  1185:                Ok(Some(id)) => {
  1186:                    result.file_ids.push(id);
  ```

  对照 `task_expansion.rs:523`（`result.upload_file_id = created.file_id.clone();`
  在 PUT **之前**、`:506-515` 已明说"create 发出去了 ⇒ 网盘里可能已经占坑"），
  同一仓库里两种取向并存 —— 也就是说这个坑在本仓**已经被识别过一次**。
  `ai_avatar` 走第三条路（`generate_once` 只回成功 id，同样丢中间态）。
- 复现: `sed -n '1284,1331p' crates/mcloud-tasks/src/app_task.rs` 与 `sed -n '517,540p' crates/mcloud-tasks/src/task_expansion.rs`
- 影响: 每次 `PUT`/`complete` 失败都在用户根目录留一个 0 字节或半截的随机名文件，
  永久不被回收、也不在任何读数里。
- 建议: 在 `create` 判定 `!exist` 之后立刻把 id 交给调用方（改成回调/`&mut Vec<String>`
  入参，或返回一个"已造 id + 结果"的结构），让 `cleanup` 覆盖中间失败态。
- 状态: 仅静态 · 置信度 高

### tk-4 [中] `refetch_items` 的 16 连发分页循环没有取消检查

- 位置: `crates/mcloud-tasks/src/app_task.rs:525-531`
- 证据: 同一文件里**开跑前**那次整表拉取有检查，**收尾这次没有** ——

  ```rust
  // 开跑前：
  985:    for market in APP_TASK_MARKETS {
  986:        for group in ...TASK_LIST_V2_GROUPS {
  987:            ctx.check_cancelled()?;
  // 收尾重拉（无 check_cancelled）：
  525:    for market in app_task_matrix::APP_TASK_MARKETS {
  526:        for group in mcloud_upstream::app_task_list_v2::TASK_LIST_V2_GROUPS {
  527:            // 单组失败不炸整件事（与只读矩阵同口径 ✓），只是少一页证据。
  528:            if let Ok(p) = app_task_matrix::fetch_one(ctx, market, group).await {
  ```

  量级由 `:522` 注释给出（"实测整表读一次约 1.4 s"）⇒ 最长约 20 s 不可中断，
  且它发生在**写操作之后**。
- 复现: `sed -n '523,539p' crates/mcloud-tasks/src/app_task.rs`
- 影响: 收尾阶段点"中止"要等一整趟整表读取；期间没有回执说明"文件已经传完、正在读数"。
- 建议: 在 `:526` 内层循环头补 `ctx.check_cancelled()?;`（与 `:987` 对齐）。
- 状态: 仅静态 · 置信度 高

### tk-5 [中] `beans.rs` 两个分页循环结构上拿不到取消令牌

- 位置: `crates/mcloud-tasks/src/beans.rs:262`、`:350`
- 证据: 整文件 **0 个** `check_cancelled`，而两个循环各最多翻 `MAX_PAGES = 20` 页：

  ```rust
  61:pub const MAX_PAGES: usize = 20;
  262:    for page in 1..=MAX_PAGES {          // read_day
  350:    for page in 1..=MAX_PAGES {          // read_records
  ```

  两个函数的签名收的是 `http: &dyn HttpClient`（不是 `TaskContext`）⇒ 不是"漏写一行"，
  是**要改签名**才接得进令牌。
- 复现: `grep -c check_cancelled crates/mcloud-tasks/src/beans.rs`（期望 0）与
  `grep -n "for page in 1..=MAX_PAGES\|pub async fn read_day\|pub async fn read_records" crates/mcloud-tasks/src/beans.rs`
- 影响: 「今日AI豆」明细最多 20 次串行请求，中途不可取消（慢网下像"中止没反应"）。
- 建议: 给这两个函数加 `cancel: Option<&CancelToken>` 参数，循环头 `check()`；
  服务器路由把该次请求的令牌传进来。
- 状态: 仅静态 · 置信度 高

### tk-6 [中] `live-room` 的"再听一轮"循环与 60 s 长收听没有取消检查

- 位置: `crates/mcloud-tasks/src/live_room.rs:1499`、`:1524`、`:268`
- 证据: 该 `while` 体里**没有任何** `check_cancelled`（文件里那两处检查在 `:1582`
  的 `run_chain` 里，是另一条循环）：

  ```rust
  1499:        while !params.skip_kouling && rounds < MAX_LISTEN_ROUNDS {
  ...
  1524:                .collect(ctx, jwt, &room, params.listen_for(), &exclude)
  ```

  `listen_for()` 就是 `Duration::from_secs(self.listen_seconds)`（`:268-269`，
  默认 60 s；`:1291` 注释写明"默认 60s ⇒ 3 间就是 3 分钟"）。
- 复现: `sed -n '1496,1530p' crates/mcloud-tasks/src/live_room.rs` 与
  `grep -n "fn listen_for" -A 3 crates/mcloud-tasks/src/live_room.rs`
- 影响: 用户点中止后最长等一整轮收听（默认 60 s，可配更大）才断开；多房间成倍。
- 建议: 循环头加 `ctx.check_cancelled()?`；把令牌传进 `KoulingCollector::collect`，
  让 WS 收听内部也能被提前掐断（只听 `listen_for` 到点自然结束是被动的）。
- 状态: 仅静态 · 置信度 中（未读 `LiveKoulingCollector` 的 WS 实现，仅看签名与时长）

### tk-7 [中] `apkInstall` 的 `success` 为假 ⇒ 静默 `continue`，最终归因成"未找到可安装应用"

- 位置: `crates/mcloud-tasks/src/cloud_phone_redpack.rs:1254`、`:1277`
- 证据:

  ```rust
  1254:        if !js_truthy(install.get("success").unwrap_or(&Value::Null)) {
  1255:            continue;          // ← 不 board.note、不 board.fail，什么都不留
 1256:        }
  ...
  1277:    board.fail(format!("{title}：未找到可安装应用"));
  ```

  即：只要上游把这一格改名/改成字符串/在该分支不回它，`continue` 到下一个 app，
  循环走完落进 `:1277`，报出的原因是**错的**（应用是找到了的，是 `success` 读不出）。
  同一函数里其它早退支路都留了 `board.note`（`:1212`/`:1216`/`:1232`/`:1250`），
  只有这一支没有 —— 说明这是漏的，不是风格。
- 复现: `sed -n '1234,1280p' crates/mcloud-tasks/src/cloud_phone_redpack.rs`
- 影响: 上游字段一改，用户看到"未找到可安装应用"并去查应用列表，而真因是解析；
  该任务会**长期静默失败**且每次归因都错。
- 建议: 该支补 `board.fail(format!("{title}：apkInstall 未返回 success（原样：{install}）"))`
  或至少 `board.note`，让 `:1277` 的兜底原因不再被这一支冒充。
- 状态: 仅静态 · 置信度 高

### tk-8 [低中] `refetch_items` 用 `if let Ok` 丢掉"哪几组没读到"

- 位置: `crates/mcloud-tasks/src/app_task.rs:528`
- 证据: 与开跑前"逐条登记 failures"（`:991` `Err(e) => result.failures.push(...)`）
  不同，收尾那次只留成功页；只有"16 发全挂"才有信息（`:533-536` ⇒ `:391` 记一句）：

  ```rust
  528:            if let Ok(p) = app_task_matrix::fetch_one(ctx, market, group).await {
  529:                pages.push(p);
  530:            }
  ```

- 复现: `sed -n '523,539p' crates/mcloud-tasks/src/app_task.rs`
- 影响: 434/522 的收尾读数建立在"不完整的一张表"上而**明细里看不出**；
  条目恰好落在没读到的那组时会被判成"表里没这条"（`:414`），归因指向错方向。
- 建议: `Err` 也登记进 `result.failures`，并在 `settle_progress` 里带上"这次只拉到 N/16 组"。
- 状态: 仅静态 · 置信度 高

### tk-9 [低] `cloud_phone_redpack` 的 `verdict` 只在有硬失败时才不是 `Done`

- 位置: `crates/mcloud-tasks/src/cloud_phone_redpack.rs:494`、`:498-501`
- 证据:

  ```rust
  494:            verdict: RedpackVerdict::Done,     // ← 先无条件置 Done
  ...
  498:        if !result.board.failures.is_empty() {
  499:            result.verdict = RedpackVerdict::Failed;
  ```

  ⇒ 整趟每条都落 `Blocked`/`需手动`/`Unknown`（一条都没做成、一个写请求都没发）
  也算 `Done` → `TaskOutcome::success`（`:388`）。小结本身是诚实的
  （`:344-382` 会念"未完成 N、已完成 0"），但 `runs` 状态是绿的。
  注意同文件的 `RedpackVerdict` 零值刻意是 `Failed`（`:325-328`），
  所以这是"显式置 Done 覆盖了保守零值"。
- 复现: `sed -n '486,505p' crates/mcloud-tasks/src/cloud_phone_redpack.rs`
- 影响: 一条都没推进的那趟不进失败计数、不触发 onlyError；用户要靠读小结才能发现。
- 建议: 加一档"做了 0 条"的判定（`counts.done == 0 && writes 为空` ⇒ `Skipped`/`Partial`），
  或让 `verdict` 保持零值除非真有 `done` 计数上涨。
- 状态: 仅静态 · 置信度 高

### tk-10 [低] `signin::receive` 把**算出来**的数当"余额"念

- 位置: `crates/mcloud-tasks/src/signin.rs:388`
- 证据: 该任务的判据是 `infoV3`（读到的），但小结里的"余额"是**上一枪读数 + 本次份数**：

  ```rust
  386:        let mut msg = if total_gained > 0 {
  387:            format!(
  388:                "共领取 {total_gained} AI豆，余额 {}",
  389:                st.total + total_gained
  ```

  `st` 是**领之前**那次 `query_status`（`:260`），此后没有再读一次余额。
- 复现: `sed -n '258,262p;386,393p' crates/mcloud-tasks/src/signin.rs`
- 影响: 上游延迟入账/份数口径与 `total` 不同源时，用户看到一个余额数字，
  而它从未被上游确认过；对照 `CloudSnapshotTask` 刻意只取 `total`（`:728-733`），
  同一文件里两种口径。
- 建议: 把那一句改成"本次领取 N（余额未回读）"，或在末了补一次 `query_status` 取真余额。
- 状态: 仅静态 · 置信度 中

### tk-11 [低] 固定文案/字段名依赖（三处，均已如实上报，登记备查）

- 位置: `crates/mcloud-tasks/src/app_task.rs:245`、`:158`；
  `crates/mcloud-tasks/src/cloud_phone_redpack.rs:102`、`:1038-1042`
- 证据:

  ```rust
  // ① beiyong1 名字门（旧 TS tasks.ts:68-77 逐字）——上游改文案 ⇒ 这条永远不上传
  245:    name.contains("上传") && (name.contains("图") || name.contains("照"))
  // ② beiyong1 按钮门（tasks.ts:78-81）——靠 mcloud://…/uploadView 那一串
  158:    link.is_some_and(|l| l.contains("uploadView"))
  // ③ 红包派对题库：15 条"问题原文 → 答案"的硬表
  102:const KNOWN_ANSWERS: [(&str, &str); 15] = [
  1038:        answer_text = KNOWN_ANSWERS
  1039:            .iter()
  1040:            .find(|(known, _)| *known == question.as_str())
  ```
- 复现: `grep -n "name.contains(\"上传\")\|uploadView" crates/mcloud-tasks/src/app_task.rs` 与
  `sed -n '102,110p;1036,1052p' crates/mcloud-tasks/src/cloud_phone_redpack.rs`
- 影响: 与题面第 5 条同型，但**三处的失败表现不同**：
  ① 一换文案就静默"不传"（无任何提示 —— 最坏的一档）；
  ② 同上，但只影响补满 10 张那一族，且 `buttonAppLink` 已进只读明细可复核（`app_task_matrix.rs:352`）；
  ③ 未知题目会走 `answer_text.is_empty()` 分支 ⇒ `board.manual("需手动完成（未知题目: …）")`
  （`:1060-1067`）**如实转人工**，不猜 —— 这一处是正例。
  另外 `app_task.rs:319` 的 `share_progress_from_name` 刻意**不**按固定前后缀解
  （注释 `:308-318` 点名批评旧 TS 那种写法），是本问里的正面样板。
- 建议: ①/② 至少在"命中 beiyong1 分组但两条门都没中"时往 `problems` 里写一句
  "无形状匹配"（现在完全静默，`step_for` 回 `None`）；③ 保持现状。
- 状态: 仅静态 · 置信度 高

### tk-12 [低] `login::logout` 的 `let _ =`（刻意为之，仅登记）

- 位置: `crates/mcloud-tasks/src/login.rs:414`
- 证据: 全 crate 非测试代码里**唯一**的 `let _ =`：

  ```rust
  402:/// 实测客户端是"先清本地 token，再发 logout"，服务端因此常回
  403:/// `01000001 用户token不能为空`。那不是失败，是正常顺序的结果。
  413:    // 尽力而为：网络错误也不上报（本地凭据已经清了）
  414:    let _ = http.request(req).await;
  ```
- 复现: `grep -rn "let _ =" crates/mcloud-tasks/src/ --include=*.rs | grep -v tests`
- 影响: 无实际危害；但"服务端 token 是否真的撤销"在任何读数里都不可见。
- 建议: 行为保持，改成 `if let Err(e) = … { tracing::debug!(…) }` 让它留痕。
- 状态: 仅静态 · 置信度 高（**这是刻意的，不算缺陷**）

## 未覆盖范围（明确声明）

- **没读**：`exchange/tests.rs`(3044)、`live_room/tests.rs`(2717)、`app_task/tests.rs`(1939)、
  `signin/tests.rs`、`ai_avatar/tests.rs`、`notice/tests.rs`、`login.rs` 的非登录段、
  `exchange.rs` 的 1-1089 与 1519-1800、`live_room.rs` 的 1-1490 大部分（只读关键段）、
  `task_expansion.rs` 的 1-360/700-795、`cloud_phone_redpack.rs` 的 1-300/1400-1623。
  ⇒ **测试文件里可能藏着对本报告结论的反例**（例如"取消后确实清理了"的用例）。
- **没跑**：cargo / 任何 `mcloud` 子命令 / 任何真实账号位操作；本报告**全部是静态结论**，
  唯一带实测性质的证据是**代码注释里已记录的真机读数**（tk-1 的 `WAIT` 那条），
  我把它当"仓库自述"引用，未复核。
- **没验证**：`mcloud-upstream` / `mcloud-engine` 的行为只读到支撑本报告所需的程度
  （`app_task_click.rs`、`artifacts.rs`），未做它们自己的审查。
- **live_room 的 WS 实现**（`LiveKoulingCollector::listen_danmaku_with`）未读 ⇒
  tk-6 只给了"循环层"建议，没断言 WS 内部能否被中断。
