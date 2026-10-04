# 全项目 review —— 2026-10-04（定稿跨到 10-05 00:0x）

> **形状**：仿 [`review-qwen3.8f-261002.md`](./review-qwen3.8f-261002.md) —— 五个重点（Rust 隐藏逻辑 /
> 前端性能与交互 / `docs/` 过期误导 / 同一事实多处冲突 / 注释与代码矛盾）+ 被推翻的结论 + 没覆盖的格子。
> **作者口径**：这份出自当前 Qoder 会话的读码与量测；文件名沿用 `review-qwen3.8f-<yymmdd>` 这条既有命名，
> 别把它当"某个模型出具"的凭证引用。
> **与同日 [`review-261004.md`](./review-261004.md) 的分工**：那份是六条并行只读线的**编号清单**（77 条）+
> Lead 抽查；这份的价值在另外两头 —— ① **审那批正在飞ing 的修复本身**（六线写的时候还没有它们），
> ② 六线没覆盖的形状（落盘耐久性、调度器改造的副作用、轮询竞态的全量清点）。**结论有出入时以这份的"读数"为准**，
> 因为它每条都带我自己跑过的命令。
>
> 每条带**落点（`文件:行`）**与**怎么复核**。置信度三档别混着读：
> **已实测**（有可复现读数）/ **读码确认**（代码里直接读到）/ **推断**（从形状推的，没量过）。

## 0. 基数与口径

| 项 | 值 |
|---|---|
| 基线 | 代码 `HEAD = 2ca9cf7`（2026-10-04 22:12）**+ 工作区未提交改动**。⚠ **审阅期间树一直在动**：我 23:38 起读到的 diff 是 22 文件 / +976，00:0x 收尾时是 **44 文件 / +1905 / −493**（`git diff --stat \| tail -1` 现算）；并行线在按 `review-261004.md` 的批次清单逐条修，`docs/task-log.md` 已经排到第 89 行。⇒ **本报告的每条都要用后面的命令现算一遍再动手**，别引用我这里的行号当稳定坐标 |
| 树在飞的一个实例 | `crates/mcloud-upstream/src/app_cloud_upload.rs` 我在 **23:39** 读到的是 `exist: Some(obj.get("exist").and_then(Value::as_bool).unwrap_or(false))` —— 字段文档已经写成"三态"，代码却还是**两态**（兜底照旧，等于没修）✗；**23:39:49** 同一个文件被并行线改成 `and_then(Value::as_bool)` ✓。⇒ 见 §6 第 1 条，这条**不是缺陷**，是"文档先行、代码后到"的中间态，别照我 23:39 那一份规划工作 |
| Rust | 8 个 crate；生产行数（`find crates/*/src -name '*.rs' ! -name 'tests.rs' ! -path '*/tests.rs' \| xargs wc -l`，本轮现算）core 6715 / crypto 2656 / upstream 19005 / tasks 13597 / server 8037 / app 4521 / engine 4506 / store 1387 ⇒ **≈6.2 万行**；测试属性 **1864** 条（`grep -rh '#\[test\]\|#\[tokio::test\]' crates \| wc -l`） |
| Web | `web/src` **39 个文件 / 15478 行**；`docs/` 9807 行 |
| 我自己跑过的 | `cargo check --workspace --all-targets` ⇒ **exit 0**，1m20s（本线独占 `CARGO_TARGET_DIR=target/lines/reviewer-261004`，按 AGENTS 那条多写入线约定；起跑 00:0x）；`cd web && npm run typecheck` ⇒ **exit 0**（23:4x）；`rustc -O` 到 `target/fs-probe/` 的落盘量测（读数在 §1.3）；其余只读：`git diff` / `grep` / `sed` / `stat` / `PlistBuddy` |
| 我**没**跑的 | `cargo test --workspace`、`clippy -D warnings`、`gates-web.mjs` 两档、`npm run build`（不碰 `web/dist`）、任何 `cargo build`（不重链接托管那份 `target/debug/mcloud` ⇒ 0 次重启、0 次授权框）、任何真任务。**测试全绿那一格本轮不是我验的**：仓里最新的读数是并行线的 `target/batch2c-gates.log`（00:01，`clippy exit=0`），我没有独立复现 ✗ |
| 没碰的东西 | `data/`（只 `ls`/`stat` 读元数据，没读内容）、`:3000` 的托管服务（没 kill、没 kickstart、没另起实例）、`~/Library/LaunchAgents/*.plist`（只读 `Print`） |

一句话结论：**这批在飞的修复方向都是对的，且比六线报告更收敛（`exist` 走三态、产物提前落账、并发闸补 `release`）**；
真正值得马上处理的三件事是 —— ① `sv-2` 改成 spawn 之后**关停不再等在途任务、同一条定时也不再互斥**（§1.1）；
② `co-1` 的耐久性代价是**每次写盘**都付，包括 2 s 一拍的小日志文件，而"判旧文件是否为空"用的是**把整个旧文件读进内存**（§1.3，已量）；
③ 空文件从"静默按空库启动"改成"报错退出"之后，在 `KeepAlive=true` 的托管下会变成 **≥10 s 一次的循环重启、WebUI 起不来**（§1.2，取向对、缺一档兜底，要用户拍板）。

---

## 1. Rust：逻辑问题（含对**在飞修复**的复核）

### 1.1 [中] `sv-2` 用 spawn 解掉"占住一拍"，同时解掉了"关停等在途"和"同一条定时互斥"

**落点**：`crates/mcloud-engine/src/scheduler.rs:343-357`（`Some(this)` 分支里 `tokio::spawn`）、`:447`（`fire_pipeline` 直接 `self.pipelines.run(&pipeline, accounts, None)`）、`crates/mcloud-server/src/main.rs:130-131`（`stop_scheduler.stop(); let _ = ticker.await;`）、`crates/mcloud-engine/src/pipeline.rs:183-196`（`run()` 开头只有 `preflight`，**没有**"这条编排已经在跑"的判据）

```rust
// scheduler.rs:343 —— 守护循环那一支
Some(this) => {
    tokio::spawn(async move {                       // ← 没人持有这个 JoinHandle
        let (outcome, _blocked) = this.fire(&sch_for_task, point_ms).await;
        tracing::info!(…);
    });
    results.push((sch.id.clone(), FireOutcome::Spawned));
}
```

**两件事跟着变了，而注释只说了一半**：

1. **优雅停止不再等在途定时。** `main.rs:131` 只 `await` `ticker`（= `run_loop` 本体），`run_loop` 一 `break` 就返回；
   那些 spawn 出去的 `fire` 是**脱离体**。runtime 一退 ⇒ 任务被直接丢掉，
   表现正是 261002 §1.2 担心的另一半："停在半个上游请求上 / 跑完了但没记账"（`record()` 在 fire 尾部，丢的就是它）。
   旧写法是"等它跑完"（太慢），新写法是"根本不等"（太快）—— **两个极端，中间那档没做**。
2. **同一条定时不再串行。** 旧形状里 tick 被占住，物理上不可能叠加；现在一条 >30 s 的定时会在下一拍再起一份。
   任务分支还有 `Runner` 的 `task:account` 闸兜住（撞闸记 `SkippedBusy` ✓），
   **而编排分支不经过 `Runner`** ⇒ 一条"每分钟跑、单步 90 s"的定时编排可以叠三份，
   每份都真的往上游打（扣豆、传文件、兑换）。这是**不可撤的副作用**方向。

**怎么复核**：
```bash
sed -n 340,360p crates/mcloud-engine/src/scheduler.rs      # spawn，无人持有
sed -n 445,462p crates/mcloud-engine/src/scheduler.rs      # fire_pipeline 不经 Runner
grep -n 'ticker.await\|stop_scheduler.stop' crates/mcloud-server/src/main.rs
grep -n 'pub async fn run' -A 8 crates/mcloud-engine/src/pipeline.rs   # 只有 preflight
```
**要坐实行为**（我没跑，属于"要用户点头 + 库副本"那一格）：副本上建一条**每分钟**的编排定时、单步故意拖 >90 s，
看历史里同一分钟是否出现多份 run。

**修的方向**（不在本报告范围）：`Arc<tokio::task::JoinSet>` 收住句柄 + 停止时**有界** drain（例如等 20 s 再放弃并 `warn`），
外加一条**按 schedule id 的 inflight 判据**（`SkippedBusy` 那个语义现成的）。置信度：**读码确认**（形状）／**推断**（叠加与截断的实际后果）。

### 1.2 [中] 空文件"报错退出"接在 `KeepAlive` 托管上 ⇒ 循环重启、界面起不来（取向对，兜底缺一档）

**落点**：`crates/mcloud-store/src/json_store.rs:402-424`（`read_json`：0 字节 / 全空白 ⇒ `Err(Storage(…))`，附恢复提示）+ `~/Library/LaunchAgents/com.catlair.mcloud-webui.plist`

**已实测**（只读 `PlistBuddy`）：
```
KeepAlive: true
ThrottleInterval: （没有这一项 ⇒ launchd 默认 10 s）
ProgramArguments → /Users/catlair/mobile/workspace/mcloud/target/debug/mcloud
```
⇒ 一旦某张表真是 0 字节（断电残骸、或有人手工 `>data/accounts.json`），`JsonStore::open` 直接失败 ⇒
进程退出 ⇒ `KeepAlive` 每 ≥10 s 再拉一次 ⇒ **WebUI 一直起不来**，而那句人话提示只落在 stderr 日志里。

**这一条我不当缺陷写**：把"静默丢账号"换成"停下并说清楚"是对的（模块头那条「不试图自动恢复」的理由站得住）。
但它现在把**只有你能做的决定**变成了"用户面对一台打不开的界面"。要不要加一档
「**响亮地**自动从同名 `.bak` 兜一次、同时把 0 字节残骸改名留证（`accounts.json.empty-<ts>`）」—— 要，我按你的口径写；
不要也请明确一句，好让下一轮别自作主张。（需要用户点头 ✓）

**复核**：`grep -n '是空文件' -B 6 -A 6 crates/mcloud-store/src/json_store.rs`；`/usr/libexec/PlistBuddy -c 'Print :KeepAlive' ~/Library/LaunchAgents/com.catlair.mcloud-webui.plist`。

### 1.3 [中·已实测] `co-1` 的耐久性成本落在**每一次**写盘，包括 2 s 一拍的日志；而"判空"用的是整读

`write_atomic`（`crates/mcloud-store/src/json_store.rs:347-397`）现在每次写都做：
**整读旧文件**（只为判 `is_empty`）→ `copy` 出 `.bak` → 写 tmp → `sync_all` → `rename` → 开目录 `sync_all`。

**量测**（同一份卷：`/Users/catlair/mobile` = APFS / USB SSD / journaled；装置 `target/fs-probe/probe.rs`，
`rustc -O` 单独编，20 次一组，**不碰 `data/`**）：

| 载荷 | 改前（copy+write+rename） | 改后（read+copy+fsync+目录 fsync） |
|---|---|---|
| 276 287 B（= 真 `history.json` 的体量，实测 `stat -f '%z' data/history.json` ⇒ 276275） | med **2.33** / p90 4.82 / max 5.04 ms | med **9.87** / p90 36.61 / max 117.25 ms |
| 2 343 B（= 真 `accounts.json`） | med 5.41 / p90 28.38 / max 162.15 ms | med 22.29 / p90 98.23 / max 160.24 ms |
| 单独量「开目录 + `sync_all`」一步 | — | med **0.02** / max 1.75 ms |

三个可读结论：
1. **目录那道几乎免费**（0.02 ms），代价全在**文件的 `sync_all`** ⇒ 别为了省时间把它砍掉，要省就**按表分档**。
2. 小文件那一组**改前改后都在 100 ms 级抖**（USB SSD 上这份卷的抖动本来就大）⇒ 别拿"小文件写得起"当依据；
   能确定的是**中位数从 5 ms 涨到 22 ms**，以及**每次写都多一整份 `.bak`**（实测 `data/history.bak` = **259 239 B** ⇒ 一次 `save_run` 写 ≈0.5 MB）。
3. `read_json` 那条"0 字节 ⇒ 报错"要的是**长度**，现在却把整个旧文件读进内存（276 KB 那份每次进 store 都读一遍）——
   **改成 `std::fs::metadata(path).map(|m| m.len() == 0)` 就等价，且省掉一整读** ✓（这条最便宜，建议直接做）。

**为什么值得动**：这些同步系统调用跑在 `write_gate`（**`std::sync::Mutex`**）里，调用方是 `async fn`
（`json_store.rs:620-640` 的 `persist_*`、`:980-1006` 的 `append_logs`），而 `append_logs` 正是
`main.rs:106` 那**每 2 秒一拍**的攒批落盘 ⇒ 阻塞的是 tokio worker，同卷还叠着 `.bak` 的整份 copy。
置信度：**已实测**（上表）／**读码确认**（锁与 async 的形状）。
⚠ 我没有"线上因此卡顿"的读数 ✗ —— 那是另一格（要量就量 `/api/health` 的 p99 或一次 200 条日志刷屏的 run）。

**顺带两条同族、都验过现状的**：
* `write_atomic` 的失败路径**不清理 `*.tmp<pid>`**（`:353-372`，`?` 直接冒出去）⇒ 残骸留在 `data/`，
  没有清扫逻辑。实测本机现在**没有** tmp 残骸（`ls data | grep tmp` 空 ✓），所以这是"哪天 fsync 真失败一次就会留"的口子。**读码确认**。
* ~~两个并发写撞同一个 tmp 名~~ —— **证否**：`JsonStore` 有专门的 `write_gate: Mutex<()>`（`:205-209`、`persist_*` 逐个持锁），
  同进程串行落盘 ✓。别把它当缺陷报（我一开始怀疑过，读完加锁形状就撤了）。

### 1.4 [中] `sv-3` 只治了运行记录那侧，**定时页仍把"我自己按的中止"报成红色失败**

**落点**：`crates/mcloud-engine/src/scheduler.rs:517-536`（记账只有 `"ok"` / `"error"` / `"skipped"` 三档，判据是 `rec.outcome.is_failure()`）
vs `crates/mcloud-core/src/run.rs:238-242`（`needs_attention()` 明确把 `RunStatus::Cancelled` 排除在"要人管"之外）
vs `web/src/api.ts:947`（`lastStatus: 'ok' \| 'error' \| 'skipped' \| null` —— **值域里没有 cancelled**）
vs `web/src/pages/schedules-page.tsx:88`（页面自己那份 `statusBadge(lastStatus)`，与 `ui/history.ts` 的同名函数**不是同一个**，值域也不同）

⇒ 一次被中止的定时：运行历史里现在 correctly 显示「已中止」（§2 那批改动 ✓），
但**定时页那一列**显示「失败」（红），且 `last_error` 里挂的是那句取消文本。
后端与前端两侧都没有这一档，所以这不是漏改一处 UI，而是**这条口径没贯穿**。

**怎么复核**：
```bash
sed -n 515,540p crates/mcloud-engine/src/scheduler.rs
grep -n "lastStatus" web/src/api.ts
grep -n "const statusBadge" -A 10 web/src/pages/schedules-page.tsx
grep -n "fn needs_attention" -A 4 crates/mcloud-core/src/run.rs
```
**修的方向**：`Schedule::last_status` 加一档（或在 fire 里把"这一趟被中止"与"失败"分开记），
值域一旦扩，`api.ts:947` 与 `schedules-page.tsx:88` 两处都要跟上 ⇒ 属"同一事实多处"，见 §4。
置信度：**读码确认**（三处值域是代码直读）／无实测（没构造过一次被中止的定时）。

### 1.5 复核过、**结论是"改对了"**的四处（列出来免得重复劳动）

| 编号 | 落点（本轮现算的行号） | 复核到的是 |
|---|---|---|
| `sv-1` | `runner.rs:401-412` 并发闸早退补了 `self.cancel.release(run_id)` ✓；紧邻那条早退（`:389-398`）本来就有 ✓ | 两处形状一致；新增钉子 `the_gate_early_return_releases_the_cancel_token` **必须走 `Runner::start`** —— 作者自己写明"第一版直调 `run` 测不出来（撤修复照样绿）"✓，这正是该做的写法 |
| `tk-2`/`tk-3` | `app_task.rs:982`（`cancelled: Option<Error>`）+ `:1049`、`:1197` 两处 `break` 替代 `?` + `:1241-1245` 收尾后才报取消；`upload_one_file` 签名多一个 `owned: &mut Vec<String>`，`:1349` **在 PUT/complete 之前** `owned.push` | 我把整段 `run_app_task` 的 `?` 数了一遍：上传开始**之后**只剩 `:1156` 那一处，而它是 `match` 不是 `?` ✓ ⇒ 这条链的早退口全堵上了 |
| `up-1` | `app_cloud_upload.rs:715` `exist: obj.get("exist").and_then(Value::as_bool)` ✓；两个消费侧都是 `match` 三态、`None` 走"不清理"（`task_expansion.rs:524-539`、`app_task.rs:1330-1341`） | `task_expansion.rs:160-170` 新增 `BackupUpload::ExistUnknown` 一档且带文案 ✓ —— "不删"的方向一致，且把"形状变了"露在明细里 |
| `up-2` | `live_ws_client.rs:288-300`（`HANDSHAKE_TIMEOUT = 30 s`）+ `:345` `tokio::time::timeout(handshake, connect_async(…))` | 另开 `connect_live_ws_within` 只为让单测用毫秒级超时（作者注明"否则那条用例自己跑 30 s"）✓；生产入口仍是 `connect_live_ws` 一个 ✓ |

### 1.6 六线里我复算过、**仍然开着**的（不动手，只报现状）

| 编号 | 一句话 | 本轮复核命令与读数 |
|---|---|---|
| up-3 | 30 s 是 reqwest 的**总** deadline，仍套在预签名 PUT 整段上传上 | `grep -n 'total deadline\|\.timeout(' crates/mcloud-upstream/src/client.rs` ⇒ `:200` 注释、`:227`/`:307` 仍是同一个 `.timeout` |
| up-4 | `CancelToken` **没进网络层**，取消只在步骤边界生效 | `grep -rn cancel crates/mcloud-upstream/src/*.rs \| grep -v tests \| grep -c CancelToken` ⇒ **0**（与六线一致，仍未接） |
| co-2 | `RunRecord.logs` 进 `history.json` ⇒ 整库重写式写放大 | 本轮量到更硬的数：`data/history.json` 276 275 B、`data/history.bak` 259 239 B ⇒ **一次 `save_run` ≈0.5 MB 落盘**（§1.3 那张表） |

---

## 2. 前端：轮询竞态（把"同型"数完，而不是只修最响的那三处）

代次号这一族已经落地三处，**形状是对的**（先判代次再写信号，连失败支一起拦）：

| 已闭 | 落点 | 做法 |
|---|---|---|
| `wb-1` 日志页 | `logs-page.tsx:58 :97 :125 :131` | 模块级 `loadSeq`，`await` 回来对不上就整份丢（成功与 `catch` 两支都拦 ✓） |
| `wb-2` 仪表盘 AI 豆明细 | `dashboard-page.tsx:664-687` | 请求键 = `账号 + 档位`，回来比对当前值 |
| `wb-3` 账号页扫码 | `accounts-page.tsx:407 :454` | 归属校验 `sid !== qrSessionId.value ⇒ return`（成功与 404 两支都拦 ✓） |

**同型还差这些**（`grep -rn "setInterval" web/src` + 每页 `load()` 调用点计数，本轮现算）：

| 页 | 并发来源 | 现状 | 严重度 |
|---|---|---|---|
| `schedules-page.tsx:165-193`、`:323` | **60 s 的 `refresh()`**（无条件 `items.value = await listSchedules()`）+ 手动 `load()` + 删/建之后的 `await refresh()` | `refresh` 与 `load` 两个写手，无代次 ⇒ 慢的那拍回来会把新的一份盖掉，并把 `state` 打回 `'error'` | 中（**这条是唯一还在周期轮询的**） |
| `settings-page.tsx:100-123` | `load()` 被 `onMounted` / `setPageRefresh` / 动作后共 **4** 处调用；`Promise.all` 五发，回来无条件写 6 个信号 | 无代次；连点两下刷新即可构造"旧的把新的盖掉 + 红屏盖掉好页面" | 低 |
| `dashboard-page.tsx:353-390` | `load()` **3** 处调用（`onMounted` + 刷新按钮 + 动作后）；`:408` 那个 30 s `setInterval` **只推时钟、不碰网络** ✓ | 同型未拦。⚠ 别照六线那句"仪表盘轮询 × 手动"抄 —— 它没有网络轮询，是**手动/动作并发**，量级小一档 | 低 |
| `pipelines-page.tsx:213` | `load()` **5** 处调用 | 同型未拦 | 低 |
| `history-page.tsx` | `load()` **3** 处调用 | 同型未拦 | 低 |

**建议**（一句）：这个模式已经成型了，值得**收成一个工具**（例如 `web/src/ui/freshness.ts` 里一个 `withSeq()` /
`latestOnly(fn)`），而不是每页手抄一遍 `if (mine !== loadSeq) return` —— 现在四处手抄的写法已经开始分叉
（日志页用模块级整数、仪表盘比请求键、账号页比会话 id）。按 AGENTS 那条秤：这是**页面侧第 5、6 次**出现同一个绕法 ⇒
"该抽一层"的证据已经够了，而它不动 lite 框架本体 ⇒ 全站零字节代价。

**正面（别改坏）**：`sv-3` 的收口做在一处真相 ✓ —— `web/src/ui/history.ts:33` 的 `runStatusOf` 是唯一判据，
`history/dashboard/settings` 三页的徽章全部改走它（`grep -rn "statusBadge(" web/src` 现算），
`cancelled` 用**中性**徽章（`:64`）且历史页单开一档筛选（`history-page.tsx:296`）；
`api.ts:70-84` 把"生命周期 vs 业务结论"的分工写进了类型文档 —— 这正是 261002 §5 那一族"注释骗人"的反面。

---

## 3. `docs/`：本轮**实测已闭环**的（别再按六线那份去改）

| 编号 | 六线当时说什么 | 本轮现算读数 | 结论 |
|---|---|---|---|
| dc-1 | `docs/usage.md` 通篇是废弃 bin 名（16 处） | `grep -c 'mcloud-server\|mcloud-cli' docs/usage.md` ⇒ **3**，逐条读：`:9` 是"2026-10-03 起两者合并"的说明句，`:129`/`:132` 引用的是 **crate 路径** `crates/mcloud-server/src/cli/run.rs`（真存在） | **已闭** ✓。判残留要分开"历史说明 / crate 路径 / 旧 bin 用法"，命令见下面 ⚠ |
| dc-3 | README 说 exchange「已摘掉」是反的、条数 19 应为 20 | `sed -n 26,31p README.md` ⇒ 这一段现在是**文档索引表**，还新加了「全库审查结论（77 条）」一行 | **已闭** ✓（那行链的是 `review-261004.md`，见 §4 最后一条） |
| dc-5 | `app-task-status.md` 的 `path:line` 整列系统性指错（`step_for` 写 `:251`，实际 `:621`） | `grep -n step_for docs/app-task-status.md` ⇒ 该列已改成**只给符号名**，并注明"要行号就现算" ✓ | **已闭** ✓，而且是按"权威源=代码"这条正的写法改的 |
| dc-9 | `lite-framework.md:19` 门禁行仍是单档 | `sed -n 19p` ⇒ 已写成 `--commit`（默认 6 条）/ `--push` 10 条，并且**删掉了写死的秒数**（"耗时随机器变，别写死"） | **已闭** ✓ |
| dc-12 | README 前端仍写 Vue Vapor + daisyUI | `sed -n 3p README.md` ⇒ 已是"自动化平台"的开篇；`:150-155` 是两档门禁与钩子 | **已闭** ✓ |
| dc-16 | `task-log.md` 状态列 9 行越出自定的四种 | 文件头 `:10-15` 新加了一段"没有上线物的行也要从这四种里挑"，历史行改写为 `已落盘 ✓（当时的写法是"已完成"…）` | **已闭** ✓（改写方式保留了当时的说法，没删证据 ✓） |
| dc-17 | `dev-reload.sh` 的 trap 早于锁检查 ⇒ 早退也会 sign+bootstrap | `sed -n 60,100p scripts/dev-reload.sh` ⇒ `restore_on_exit` 现在**只在 `STOPPED == 1` 时**拉服务，而 `STOPPED` 在 `bootout` 之后才置 1 ✓；锁检查在停服务之前并明说"不动服务" ✓ | **已闭** ✓ |
| dc-4 | `AGENTS.md` 称 `../mcloud/` 是 Go 检出 | AGENTS.md 本轮被并行线改掉（现在是"本机那份 Go 侧检出是 `../mcloud-back/`"，并给了 `git show go-legacy^:…` 的取素材路） | **已闭** ✓（我读到的就是改后的版本） |

⚠ **判"旧 bin 名残留"的命令要分开三类**（本轮实测，别再用裸 `grep -c mcloud-server`）：
```bash
# 真·旧 bin 用法（会照抄出错的形态）—— 本轮 10 处命中，全部落在"带日期的旧口径"里（AGENTS.md:65-66/173-184 是解释句，
# task-log 的 #13/#14/#28/#31 是历史台账行）⇒ 功能面 0 处 ✗ 别去改历史台账
grep -rn 'target/\(debug\|release\)/mcloud-\(server\|cli\)\|\./mcloud-server\|\./mcloud-cli\|--bin mcloud-\(server\|cli\)' \
  --include='*.md' README.md AGENTS.md docs | grep -v docs/review/
```

---

## 4. 同一事实写了好几处 / 且互相冲突（本轮新增）

| 事实 | 现有出处 | 该当的权威源 |
|---|---|---|
| **中止怎么显示**（本轮 §1.4 的根因） | `ui/history.ts:33`+`api.ts:70-84`（有 `cancelled` 档，中性色 ✓）／`schedules-page.tsx:88`（自己的三档，**无 cancelled**）／`scheduler.rs:524`（只写 ok/error） | 一处：`RunStatus` + `ui/history.ts`；`last_status` 要么扩值域、要么别再自称"结果状态" |
| 同名函数 `statusBadge` | `ui/history.ts:68`（返回 class 串）与 `schedules-page.tsx:88`（返回 `{cls,text}`）——**两个值域、两种返回**，名字一样 | 页面那份改名 `lastStatusBadge`；否则下一个改 `ui/history.ts` 的人会以为两处都跟着改了 |
| 写盘耐久性口径 | `json_store.rs` 模块头 `:24-30`、`write_atomic` 文档 `:324-345`、`read_json` 文档 `:405-412` 三处 | **这是刻意冗余**（三处口径我逐条比对，一致 ✓）⇒ 别"去重"成一处，这类是护栏 |
| **本轮新增**：全库审查结论有两份 | `review-261004.md`（六线 77 条 + 批次清单，README 索引链的是它）与这份 | 分工写清：那份是**编号台账**，这份是**在飞修复的复核 + 实测量测**；`review-261004.md` 的 §4 批次状态请**以这份的 §6 为准再刷一次**（它写"批次 1 已落盘"时 sv-2 还处于注释先行的状态） |

---

## 5. 注释与文档 / 代码矛盾（本轮只留**还成立**的）

| # | 落点 | 矛盾 | 状态 |
|---|---|---|---|
| C1 | `scheduler.rs:288-299`（`tick_inner` 的说明）vs `main.rs:130-131` | 注释把改造的收益讲全了（"不再占住一拍"），但**没讲**代价（"不再等在途、不再互斥"）⇒ 读起来像纯收益 | 读码确认；建议随 §1.1 的修法一起补一句 |
| C2 | `web/src/api.ts:81`「界面必须先看 `status`，别直接拿 `outcome.status` 染红」 | 运行记录侧照做了 ✓，**定时侧**（`lastStatus`）根本没有这一格可看 ⇒ 这条纪律在那一页无法执行 | 读码确认（= §1.4） |
| C3 | `json_store.rs:205-209`「`write_gate` 让**落盘顺序**与内存修改顺序一致」 | 与 `co-5`（六线：落盘顺序可与内存倒挂）是**同一处**的两面：注释讲的是"持锁串行"，而 `append_logs` 在 `:985` 先改内存、`:1000` 才取锁 ⇒ 两个任务交错时锁内顺序仍可能反 | 读码确认（**这条六线已经列过**，本轮确认仍在：改法是把 `write_gate` 提到改内存之前一路持有） |
| C4 | `write_atomic` 里 `:347-352` 的注释「备份只从**非空**的旧文件来」 | 注释说的判据是"长度"，代码是**整读**（`std::fs::read`）⇒ 语义对、手段贵 | 已实测（§1.3 的量）；一行改 `metadata().len()` |

（六线 §5 那批反向注释 —— `wait_until` 空头、`schedule_anchor`、serde_json Map 顺序那几条 —— 本轮**没有再发现新的**；
它们要么已改、要么被并行线的文档批次覆盖，我不重复列。）

---

## 6. 被推翻 / 更正的结论（写下来，免得下一轮照它动手）

1. **"up-1 没修成"** —— 我 23:39 读到的 `exist` 行确实是"文档三态、代码兜 false"，但那是**改造还没写完**的中间态；
   23:39:49 已到位 ✓。**别按我这份行号动手**，也别据此说"批次 1 报成功而其实没修" —— 它修了。
2. **"co-3 权限清单漏 4 个"已闭**：`json_store.rs:250-275` 现在 13 张表全覆盖（与 `mod files` 的常量数一致 ✓）。
   顺带查了一格六线没说的：清单**不含 `*.bak`** —— 实测本机 14 个 `.bak`（含 `data/logs/1.bak` 等 3 个）
   **全是 `-rw-------`** ✓（`std::fs::copy` 继承源权限），所以**现在不泄漏**；风险只在"老目录里升级前就存在的宽松 `.bak`"，
   那属于 [低] 收尾项，**不要当高危报**。
3. **"`data/logs/*.bak` 是这次改动新长出来的"** —— **证否**：`HEAD` 的 `write_atomic` 本来就 copy，
   本次只加了"非空才 copy"。实测 `data/logs` 72 个文件、其中 3 个 `.bak`（167–782 B）⇒ 与改动无关。
   真正值得单独拍的是「**日志表要不要 `.bak`**」—— 一次 run 的日志是**可弃遥测**，
   给每次 2 s 攒批都留一份孪生既没用又翻倍文件数（**推断**，没量过磁盘占用是否在意）。
4. **"两个并发写会撞同一个 `tmp<pid>`"** —— **证否**，`write_gate` 串行化落盘（见 §1.3 末）。
5. **review-261004 §4 那句"批次 1 已落盘、5 条红测全部验过"** —— 只对**它那一刻的树**成立；
   同一批里 sv-2 在 23:39 还是注释先行。⇒ 台账口径要从"已落盘"降到**「已落盘（并行线持续在改，逐条现算）」**，
   并且**仍未上线**这一条没变：`:3000` 那份还是旧二进制（我没重启、也没验线上版本 —— 要验就 `curl -s :3000/api/version`）。
6. **"仪表盘 30 s 网络轮询"**（六线 wb-7 的读法之一）—— 不成立：`dashboard-page.tsx:408` 那个 `setInterval`
   **只推 `nowTick`，不碰网络** ✓（注释自己写明"且不碰网络（不是轮询）"）。⇒ §2 表里我把它降成"低·手动并发"。

---

## 7. 本轮没覆盖 / 需要用户点头的格子

* **没跑** `cargo test --workspace` 与两档前端门 ⇒ 本报告**不含**"测试还全绿"这一条自证（原因见 §0：不重链接托管二进制、不抢 `target/` 独占锁）。我复跑的只有 `cargo check --workspace --all-targets`（exit 0，隔离目录）与 `npm run typecheck`（exit 0）。
* **没跑浏览器门** ⇒ §2 那五处竞态都停在读码层，没在真浏览器构造（`narrow`/`interact`/`leak` 全未跑）。
* **没触发任何真任务**：§1.1 的"定时编排叠加"、§1.4 的"中止在定时页报红"都**没有活体读数** ✗。要闭环就在**库副本**上做，别写 `data/`。
* **§1.2 是策略问题不是代码问题**：空库残骸要不要"响亮地自动从 `.bak` 兜一次" —— 需要你拍一句。
* **量测装置留在 `target/fs-probe/`**（`probe.rs` + 编译出的 `probe` + `data/` 三份探针文件）：在 `.gitignore` 的 `target/` 里 ⇒ `git status` 干净 ✓；**要我删掉说一声**（我没自己清是想留着让你复跑）。
* `web/dist` **没重建**，`data/` 只读了元数据。

## 8. 建议处理顺序（按"做错了能不能撤" + 影响面）

1. **§1.1**（spawn 之后的 drain 与 per-schedule inflight）—— 这一条**方向不可撤**（叠加的编排会真扣豆、真传文件），
   而改动局限在 `scheduler.rs` + `main.rs` 两处；比"先做便宜的"优先。
2. **§1.3 最后那一行**（`metadata().len()` 替掉整读）—— 最便宜的一改，且是**纯收益**；顺手一起决定"日志表要不要 `.bak`"。
3. **§1.2** —— 先要你一句话（自动兜 `\.bak` 还是保持"停下并报路径"），然后按答案写文案与用例。
4. **§2 的 `schedules-page`**（唯一还在周期轮询的未拦点）+ 把代次号**收成一个 `latestOnly`** ⇒ 五处手抄不再分叉。
5. **§1.4 + C2**（`last_status` 加一档，两处 UI 跟上）—— 用户看得见的误读。
6. **§1.6 的 up-3 / up-4**（上传吃总 deadline、取消不进网络层）—— 六线已列，本轮确认仍在。
7. **C3 / 残骸清理那两条 [低]**（`write_gate` 提前持有、失败路径 `remove_file(tmp)`）—— 攒一批再动，别单开一轮。

---

### 一句话自查

* 想确认我**没凭错觉写东西**：§1.1/§1.4/§1.5/§2/§3/§4/§5 每一条的行号都是本轮 `sed -n`/`grep -n` 现算的（树在动 ⇒ 复跑为准）；
  §1.3 那张表是我自己 `rustc` 编的探针在同卷上跑出来的（命令在 §0），不是引用别人；
  §3 那 8 条"已闭"是**逐条读改后的正文**得出的，不是看提交信息。
* 想确认**哪些还不能直接照做**：§7 整节 + 所有标「推断」「读码确认」的格子；
  以及 §1.5 那张表 —— 它说的是"这四处我复核过、改对了"，**不等于**它们已上线（`:3000` 那份还是旧二进制）。

*本轮读数取自 2026-10-04 23:38 → 2026-10-05 00:0x 的工作区（`HEAD 2ca9cf7` + 44 个未提交文件）。*
