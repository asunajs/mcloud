# 复核意见 —— 对 `review-qwen3.8f-261002.md` 的逐条裁决

> 复核日期 **2026-10-03**；复核基线 `HEAD = a82748c`（原报告写的是 `1ebfe77`，中间被并行写入线推过）。
> 原报告写于 2026-10-03 09:10。
>
> | 裁决 | 意思 |
> |---|---|
> | **成立** | 我在当前 HEAD 上亲眼看到证据，问题还在 |
> | **部分成立** | 主体对、细节或坐标错 |
> | **已过期** | 当时大概是对的，当前 HEAD 不是那样了 |
> | **不成立** | 复核实测与结论相反 |
> | **不采纳** | 结论成立，但我判断**现在不该改**（每条都写了理由） |
> | **无法判定** | 需要真机/上游读数，本轮拿不到 |
>
> 方法：**不采信"作者自称已实测"** —— 每条都用 `grep`/`sed`/读码自己再跑一遍（这份报告自己抓出过
> 自己的错读数，见它 §2.8/§6，所以更要这样）。改动都配了**会红的钉子**：§1.1 那条我把修复
> 撤掉重跑过，确认用例真会红（见 §1.1）。
> 本轮**跑了**：`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
> `cargo fmt --all -- --check`、`node scripts/gates-web.mjs --full`（6 条）。
> 原报告 §7 说它一条运行期门禁都没跑 —— 这轮补上了。

---

## 0. 总评与一览

这份报告**值得留档**：432 行里每一条都有 `文件:行`、有复核命令、有置信度分档，而且它**自己推翻了自己**
（§2.8 那组"每次访问遗留 48–176 个节点"的错读数、§6 五条被推翻的结论）。我复核下来
**没有发现一条是"凭错觉写的"** —— 抽了 12 条自己重跑，全部与它说的一致（唯一漂的是计数类：
测试属性它数 1780、我数 1787，这正是它主张"文档别写死数字"的理由）。

| 节 | 采纳并已改 | 不采纳（附理由） |
|---|---|---|
| §1 Rust | 1.1 / 1.4 / 1.7 / 1.10 / 1.15 | 1.2 / 1.3 / 1.5 / 1.6 / 1.8 / 1.9 / 1.11 / 1.12 / 1.13 / 1.14 / 1.16 |
| §2 前端 | 2.1 / 2.2 | 2.3 / 2.4 / 2.5 / 2.6 / 2.7 |
| §3 docs | D1–D8 全部 | — |
| §4 docs 冲突 | 全部改成"现算命令" | 拆 `architecture.md`（要用户点头） |
| §5 注释矛盾 | C1 / C2 / C3 / C5 / C6 / C7 / C8 / C9 / C10 | C4 / C11（与 1.11、1.1 同源，见各条） |

---

## 1. Rust：逐条

### 1.1 [高] 抢整点兑换走定时/编排路径 ⇒ 必然"零请求"结束 —— **成立，已修** ⭐

复核：`grep -n "wait: None" crates/mcloud-tasks/src/exchange.rs` → 生产那唯一一处确实硬编码；
`wait_until` 全仓只有一条注释引用（函数不存在）。**根因比报告写的更深一层**：
`WaitFn` 的签名是 `dyn Fn(i64) -> Result<()>`（**同步**），而生产路径在 tokio 里跑 ——
同步闭包只能 `block_in_place` 占住一个 worker 线程去睡（抢整点最长一小时）⇒ **没人敢注入**，
于是恒传 `None`。所以这不是"忘了接线"，是**签名逼出来的**。

改法（`crates/mcloud-tasks/src/exchange.rs`）：
1. `WaitFn` 改成 async：`dyn Fn(i64) -> WaitFuture`（`WaitFuture = Pin<Box<dyn Future<Output=Result<()>> + Send>>`），
   两处调用点改 `.await`；
2. 新增 `sleep_until(clock, cancel, chain, until_ms)`：**200ms 一片**睡 + 每片前后都看
   `cancel` / `chain_cancel` 两个令牌（睡之前先查 ⇒ 已取消的立刻返回，一片都不睡）；
3. `ExchangeTask::run` 里注入它（`wait: Some(&wait)`），并把 C6 那条空头注释改掉；
4. `no_wait()` 同步改 async。

钉子（`exchange/tests.rs`）：
* `task_shell_injects_a_real_wait` —— 时刻选在"整点前 15ms、缺省补偿 10ms" ⇒ 第二段等待
  `needed = true` 但长度只有 **5ms**（不用真睡），而 `wait: None` 时 `run_account` 会如实挡下
  ⇒ 这条只到"跳过"、注入了才走到**下单**。**我把 `wait: Some` 改回 `None` 重跑过：这条红了** ✓
* `sleep_until_honours_cancellation_before_sleeping` —— 已翻的令牌（单 run 与链各一次）不睡就返回。

### 1.2 [高] 调度器 `tick()` 里同步 `await` 整条任务/编排 —— **成立，不采纳（本轮）**

读码确认形状（`:534-545` 的 `select!` 只有 `interval.tick()` 与 `stop.changed()`）。为什么不改：
把它改成"派出去 + 不阻塞 tick"是**行为变更**（同一时刻可能并发多条定时、水位线与
`/api/system/restart` 的关停语义都要重新定义），而且与 §1.1 的等待叠加后"一次 tick 占多久"
取决于任务本身。这属于**要用户点头**的那类（影响关停安全），不该由我顺手改。已记进
`docs/task-log.md` 的"未做完"。

### 1.3 [高] 链级取消在收尾判定里被漏掉 —— **成立（读码），不采纳（本轮）**

`runner.rs:346` 只看单 run 令牌，而 `TaskContext::is_cancelled` 两个都看 ✓ 报告说得对；
`routes/pipelines.rs:201` 自造链令牌、没登记 ✓ 也成立。不改的理由同 §1.2：它要动的是
**取消语义的收口**（链令牌登记到哪儿、HTTP `/cancel` 怎么够着它、`record()` 的
`Cancelled` 覆盖规则），改一半比不改更危险（"以为能停、其实停不了"）。同样记进未做完。

### 1.4 [中] 更新定时把累计记账清零 —— **成立，已修** ⭐

复核：`routes/schedules.rs` 那个 struct 字面量里 `last_error: None` / `run_count: 0` 与它上面
那句"保留水位线与**记账字段**"直接相反 ✓。
改：四个记账字段一律 `existing.as_ref()` 取回（`last_status` 顺带 clone）。
钉子：`crates/mcloud-server/tests/routes.rs` 的 `schedule_update_keeps_the_accounting_columns`
—— 建 → `mark_schedule_fired` + `record_schedule_run(error)` → 改名 PUT → 断言
`runCount=1` / `lastFireMs` 保留 / `lastError="上游 500"`。⚠ 顺手记一条给下一个人：
`/api/schedules` **只有 GET**，建/改都是 `PUT /api/schedules/{id}`（我第一版按 POST 写，405 才发现）。

### 1.5 [中] `EventHub` 只进不出 —— **成立，不采纳（本轮）**

复核：`grep -rn "\.unsubscribe(\|hub\.forget" crates` 除测试外零命中（唯一命中是
`runner.rs:364` 的 `artifacts.forget`，另一个类型）✓。
不改的理由：正确修法是"SSE 流 drop 时退订 + 事件落盘后 `forget`" —— 前者要接进
`routes/runs.rs` 的流生命周期（axum 的 `Body` drop 时机），后者要定义"哪些 run 可以忘"，
**不是一行**；而它现在的后果是"常驻内存线性增长"，对单用户本机服务是慢性病、不是急性病。
留成 TODO。

### 1.6 [中] 水位写失败用 `?` 冒泡 ⇒ 整轮 tick 提前结束 —— **成立，不采纳（本轮）**

语义上"宁可少跑"是对的，代价是同一轮后续定时被饿一轮（下一轮 tick 恢复）。修法要把
`?` 换成"记一笔 warning 继续"，属**口径选择**（要不要让写盘失败的那一条定时反复重试）。
列 TODO。

### 1.7 [中] 「错过的触发点整体弃掉」也 +1 运行次数 —— **成立，已修** ⭐

复核：`json_store.rs` 的 `mark_schedule_fired` 里 `run_count += 1`，而 `Decision::Missed`
与 `Due` 两条分支共用它 ✓；界面上 `schedules-page.tsx:384` 确实在展示 `runCount` ✓（虚高可见）。
改：`Store` 加一个**带默认实现**的窄写 `advance_schedule_watermark`（只推水位、不计数），
`JsonStore` 覆盖它，`scheduler.advance_watermark(.., counted: bool)` 分流（Missed 传 `false`）。
⚠ 用默认实现是为了**不动那十来份测试里的 `Store` 桩**（改签名要改 10 个文件）。
钉子：`json_store/tests.rs` 的 `watermark_advance_does_not_count_a_run`（两种口径都断言 +
"id 不存在时不报错"）。

### 1.8 [中] 删账号不级联 —— **成立，不采纳（本轮）**

`remove_account` 只清 accounts/state/smid，schedules/pipelines 的 `accounts` 数组不动。
影响是"配置面留悬空引用"（运行时会过滤，不会炸），属清理不彻底。列 TODO（要不要级联是
**产品口径**：有人可能希望"删了号、定时还在，重新导入就复活" —— 报告自己也说"重新导入同 id
会复活"，那可能正是有人想要的）。

### 1.9 [中] argon2 在 async 处理器里同步跑 + 无限速 —— **成立，不采纳**

复核：`grep -rn spawn_blocking crates` → **0**（只有注释）✓。不改的理由：单用户本机服务，
登录是低频动作，一次 ~100ms 占一个 worker 不构成问题；限速是**要不要把服务暴露到局域网**
这个前提下的问题 —— 那是用户的环境决策，不是我能替他定的。**留给用户判断**（报告自己也这么说）。

### 1.10 [中] 同名请求头发两枚 —— **成立，已修** ⭐

复核：`client.rs` 先 `builder.header("user-agent", profile UA)`，再逐条 append `req.headers`，
而 `smid::device_profile_request`（`smid.rs:687`）**自己带了 `user-agent`** ⇒ 两根同名线 ✓。
PC 那套 `x-yun-*` 业务头同理（无去重）。
改：复用仓内已有的 `should_inject_credential`（口径：**调用方声明的赢**）给这两处加查重。
⚠ 与它同族的 `authorization`/`jwttoken`/`deviceid` 早在 `:299-312` 就有这道闸、还有回归测试 ——
**UA 与 PC 设备头是同一规则漏掉的两支**，报告这句说得准。
⚠ **未证实**（报告自己也标了）：上游/APISIX 收到两枚时取首值还是末值 —— 需要实拍，本轮拿不到。
这条改动把"两根"变成"一根"，两种取值口径下都更接近意图 ✓。

### 1.11 [中] 401/403/400 全折进 `Error::Network` —— **成立，不采纳（本轮）**

复核：`mcloud-core/src/error.rs` 里**没有** `Http(u16)` 变体 ✓。不改的理由：这是**错误分类层的
重构**（新变体 + 8 个调用点 + 上层据类型刷新/重登的连锁），属 AGENTS.md 那条"改框架还是绕开"
的秤上偏"改框架"的一端，应该单独一轮做、并且要和"哪些状态码该触发重登"一起定。列 TODO。

### 1.12 [次要] 一片"文档规定要用、代码里从不生产"的死面 —— **成立，不采纳（本轮）**

复核（现算）：`is_auth_failure()` 生产调用 **0**；`Error::AuthExpired` 在 `error.rs` 之外的
构造点 **0** ✓。
这批**不能一律删**：它们是**接口面**（`endpoints.rs` 七个常量、`assert_usable`、`recover_orphans`），
删掉会把"以后要接的地方"一起删掉。真正该修的是**文档口径**（`conventions.md:288` 说"判断凭证
失效必须用 `is_auth_failure()`"，而没有任何地方在生产里判断凭证失效）—— 这条我在 §5/C4 里说明：
**不动**，因为那句约定本身仍然成立（它是"将来要判断时必须用它"），缺的是**生产里的调用点**，
属功能欠账。`is_auth_failure` 那张码表没有出处这条**成立且值得记**，但补出处需要实拍。

### 1.13 [次要] TLS 后端叙述过期 —— **部分成立（其中一条已被并行线改过），已修注释**

* `crates/mcloud-upstream/Cargo.toml` 那句「TLS 走 rustls（不引入 openssl）」**成立**（三重过期：
  reqwest 侧已 native-tls、WS 侧已 native-tls、arm64 交叉产物还要链 vendored OpenSSL）⇒ **已改**，
  并写明"看根 `Cargo.toml` 的两段注释，别看这里"。
* `client.rs:101-106`：**已过期** —— 并行写入线已经把它改成「默认后端（现已统一为 native-tls…）」✓。
* `live_ws_client.rs` 两处「`wss` 走 rustls」⇒ **已改**成 native-tls。

### 1.14 [次要] 其他四条

| 项 | 裁决 | 处理 |
|---|---|---|
| `connect_async` 无超时（`:319`） | **成立**（读码：只有裸 `connect_async(request).await`） | 未改。修法是包一层 `tokio::time::timeout`，但要与"这台老主机握手敏感"（Cargo.toml 里刚论证过）一起定超时值 ⇒ 列 TODO |
| `session.rs:346` 说 serde_json Map 按插入序 | **成立**（本仓 `serde_json = "1"` 未开 `preserve_order` ⇒ 默认 BTreeMap 字典序） | **已改**注释（C10），并把结论的**理由**换成"取决于键序" |
| `tn.rs:61` 数组贡献空串 | **部分成立**（潜伏：当前 `build_sm_data` 产物无数组字段） | 未改（没有触发面，改了也无从验证；已在报告里留名） |
| `redact()` 不覆盖 `RunEvent::Log`/`Finished` | **部分成立**（`redact` 是**字符串**级、只认 6 个键名；我没逐条核"生产 3 处调用"这个计数） | 未改。**这条我认为最值得单独立项**：日志/推送里带口令的可能性没有代码侧的闸 ⇒ 已记进未做完 |

### 1.15 [次要] CLI 三处"教用户做错事" —— **成立，已修** ⭐

三条**全部成立**且都改了（`crates/mcloud-server/src/bin/cli.rs`）：
① 命令数 6 → 7（并给出"现算"命令）；② `login` 模式补 `device`；③ `mcloud serve` **不存在**
（全仓零命中）⇒ 改成"WebUI 在服务进程里，默认 http://127.0.0.1:3000"。

### 1.16 [未证实] 两条 —— **无法判定，未采纳**

`90001` 重放可能双扣、同名不同 id 的打卡合并：两条都缺实拍，报告自己也标"不作为缺陷"。
同意 —— **不能拿"未证实"去改一条会真扣豆的链**。已留名。

---

## 2. 前端：逐条

### 2.1 [高] 运行日志：每条 SSE 事件拆掉整块面板重建 —— **成立，已修** ⭐

复核：`{lines.value.length > 0 ? <LogPanel lines={lines.value} /> : null}` 两处同形 ✓
（槽里读了 `lines` ⇒ 每条事件整块重造，含那个滚动容器 ⇒ **往上翻日志会被弹回底部**）。
改：
1. `LogPanel` 的 `lines: LogLine[]` → **`get: () => LogLine[]`**（thunk）——读发生在面板自己的
   槽里，`createFor` 按**行对象身份**复用已有行（旧对象身份不变）✓；
2. 新增 `pushLog(lines, kind, text)` + `MAX_LOG_LINES = 500`（只截头部），两页的 `push` 都走它
   ——日志原先**无上限**追加（报告说的"不截断"成立）。
⚠ 同款正解就在仓内（`accounts-page.tsx:1192-1199` 的 thunk 注释），这次是照着它抄的。

### 2.2 [高] 参数块每次按键整块重建 ⇒ 焦点/光标丢失 —— **成立，已修** ⭐

复核：`param-field.tsx` 在**组件体**里 `const cur = props.value`，而调用点
`value={values.value[s.name]}` 是在槽里当场读 ⇒ 那个槽订阅了值信号 ✓。
改：
1. `ParamField` 的 `value: unknown` → **`get: () => unknown`**，三处取值（bool 的 `checked`/文案、
   number 的 `value`、textarea 的 `value`）都改成在**属性槽里**调用 —— `setValue` 有
   `old !== s` 判等（`lite/src/dom.ts:483`），值相同时连赋值都不做 ⇒ 光标不动 ✓；
2. 编排页那半边还有第二个成因：`onInput` 里 `{ ...x, body }` **换掉了步骤对象** ⇒ `createFor`
   按对象身份判行 ⇒ 整张步骤卡（含任务下拉）重建 ✗。改成**原地改 `step.body`**、不写
   `steps.value`（保存路径读的就是这些同一个对象；切任务那条路径照旧换新对象，因为 specs 真变了）。

### 2.3 [高] 桌面/移动两套列表同时进 DOM —— **成立，不采纳（本轮）**

复核：三个页面各有 2 处 `md:hidden`/`md:block` ✓（两套 `.map`）。
不改的理由：这不是 bug 而是**取舍** —— 两套 DOM 换来"桌面表格 / 窄屏卡片"两种版式，
而 375 窄屏那条门禁正是按这个结构验收的。改成一套（同一份数据、CSS 切换布局）是一次
**重写**，收益是"少一半节点"，代价是三条页面的版式逻辑重做。列 TODO。

### 2.4 [中] 兑换页：分组每轮现算现造新对象 —— **成立，不采纳（本轮）**

读码确认（`groupViews(...)` 每轮新建对象 ⇒ 点分组 tab 会重建整张清单）。
理由同上：要改成"按 key 记忆化"得先有一份稳定的分组标识，而这页的判定面很重
（兑换是花钱那条链），我不打算在同一轮里既改它又改别的。列 TODO。

### 2.5 [中] 一次 `load()` 连写多个 ref，全仓 0 处 `batch` —— **成立，不采纳**

复核：`grep -rn "batch(" web/src` → **0** ✓。**不采纳的理由是报告自己给的**：
它在 `web/lite/bench` 上实测"不合批 0.20ms / 合批 0.30ms" —— 这个形状上合批**没有**免费收益。
报告写它"是为了白付的性质、不是为了那点毫秒" ✓ 我同意这个判断，但**收益是零**（甚至负），
而它还要与 `dashboard-page.tsx:40-45` 那条"写序不能动"的约束一起读 ⇒ 不动。

### 2.6 [中·已实测] 列表更新是 O(总行数)/次 —— **成立，不采纳（本轮）**

它给的两个根因（`createFor` 每轮先扫"还活着吗"、`own()` 每次 filter）我**没有自己量**，
但形状读得到。不采纳的理由：这正是 AGENTS.md 那条"改框架还是绕开"的秤 ——
报告自己的第 7 条建议就是"先把 §2.1/§2.2 的页面形状改掉，很可能就不用动运行时"，
而我**已经把 §2.1/§2.2 改了** ⇒ 先看这两处落地后的效果再决定要不要动 `lite` 运行时。

### 2.7 [次要] 请求形状 —— **部分成立，本轮未改**

逐条复核：`exchange-page.tsx` 的 `loadAccounts → loadPrizes → loadSubscriptions` 确实串行 ✓；
`listRuns()` 无分页 ✓；二维码轮询写死 1000ms ✓。三条都是"能改但本轮没改"（都在页面里、
不阻塞上面两条高优）。列 TODO。

### 2.8 [已修] 切页遗留 —— **已过期（不是问题）**

报告自己写了：`4b208ca` 已修掉 effect 作用域那条根因，它用仓内常驻的
`lite/regress/leak.mjs --fixture --rounds 5` 复测八页 **Δ=0**，并把自己早前那组错读数**就地删掉**。
我按同一条命令复跑（它现在已经是**门禁第 6 条**）：`前端泄漏量测 … ✓`。
⇒ **不要再按"还有残留"规划工作量**（它 §6 第 1 条就是这句）。

---

## 3. `docs/`：D1–D8 + §4 —— **全部成立，已修**

复核方式：每条先用报告给的命令自己跑一遍，再改；口径按 §4 的主张 ——
**权威源是代码，文档只写"怎么现算"**。改动落在 8 个文件（`usage.md` / `task-inventory.md` /
`architecture.md` / `ai-avatar-spec.md` / `docs/web/lite-framework.md` / `docs/design/cli-webui-design.md` /
`conventions.md` / `build-perf.md`），要点：

* **写死的数字全部换成现算命令**：任务条数 → `grep -c "Arc::new" crates/mcloud-tasks/src/lib.rs`；
  CLI 命令数 → `grep -n '^        Some("' …/cli.rs`；测试条数 → `grep -rh "#\[test\]\|#\[tokio::test\]" crates | wc -l`；
  lite 体积 → `node web/lite/size.mjs`。**理由就是这次复核本身**：报告数 1780、我数 1787 ⇒ 数字会漂。
* **门禁耗时不再写秒数**，只写口径（默认只跑 `ready`，`--full` 跑三个认证态 + 泄漏量测）。
* D1 `usage.md`：两个可执行文件/`tar.xz`/`xattr` 两个文件 → 单二进制 + `tar.gz`（只发 `mcloud-server`）。
* D2/D6：任务名单与条数按 `register_all` 现算重列；`create_http`/`create_task_http` 两个不存在的工厂
  改成"真实装配在 `mcloud-app/src/builder.rs`"。
* D3/D5：Vue Vapor → 自研 `lite`；"代码分割不做/单 chunk" → 已做（按页 `import()`），
  Vue 时代那几段折进 `<details>` 并标"历史、已不适用"（**没有**大改 `architecture.md` 结构）。
* D4 `ai-avatar-spec.md`：改成"协议层 + 任务层已存在、已注册、默认关"。
* D7：`GET /api/beans/today` 改成**已落地**（`routes/beans.rs` + `routes/mod.rs` merge）；
  顺手记下"路由级测试仍缺"。
* D8 `conventions.md`：`sign.rs` → `signin.rs`；"58 条测试" → 现算。
* 另补一处报告没点到的同类：`conventions.md:650` 的目录树里还写着「web/ 前端（Vue Vapor + TSX）」⇒ 已改。

---

## 4. §5 注释与代码矛盾：C1–C11

| # | 裁决 | 处理 |
|---|---|---|
| C1 `kouling.rs` 说按 `schedule_anchor` 筛、`None` 退回不筛 | **成立** | **已改**：注释改成"判据是硬编码的 `KOULING_ANCHOR`"，并写明 `schedule_anchor` **只写不读**（生产唯一写点 `live_room.rs:1370`，读点 0） |
| C2 `register_all` 旧哨兵注释与新哨兵并排 | **成立** | **已改**：两段互斥结论归并成一段，历史写在注释里（"错记录就地修"） |
| C3 `vite.config.ts` 说"单 chunk（modulepreload 数量为 0）" | **成立** | **已改**：写明前提已变（20 个 chunk、1 条 modulepreload），并说明为什么**仍然保留**这条设置 |
| C4 `conventions.md` 要求用 `is_auth_failure()`，该函数零调用 | **成立** | **不改**：约定本身仍成立（"要判断时必须用它"），缺的是生产调用点 ⇒ 属功能欠账，见 §1.11/§1.12 |
| C5 `api.ts`/`dashboard-page.tsx` 说 beans 端点"还没有" | **成立** | **已改**：两处都改成"已落地"，并说明现在读到 null 是真的读不到 |
| C6 `exchange.rs` 指向不存在的 `wait_until` | **成立** | **已改**（随 §1.1） |
| C7 CLI 三处 | **成立** | **已改**（见 §1.15） |
| C8 TLS 叙述 | **部分成立** | **已改**（见 §1.13；`client.rs` 那段已被并行线改过） |
| C9 `dom.ts` "合批之后同一串插入只收尾一次" | **成立** | **已改**：写明 `inserting` 只包住**一次** `insertBefore` 循环，真正的节省来自那两个空判 |
| C10 `session.rs` 说 Map 按插入序 | **成立** | **已改**（见 §1.14） |
| C11 兑换文档说"界面上的开关会等" | **成立** | **已改**（随 §1.1：现在真的会等了） |

---

## 5. 本轮**没做**的 / 需要用户点头的

按"做错了能不能撤"排（都能撤，但都要单独一轮 + 需要决策）：

1. **§1.2 调度器不阻塞 tick** —— 行为变更（并发语义、关停窗口），要用户点头。
2. **§1.3 链级取消收口** —— 动取消语义，改一半比不改更危险。
3. **§1.5 hub 回收** / **§1.6 水位写失败不再冒泡** / **§1.8 删账号级联** —— 各要一条口径决定。
4. **§1.11 错误分类加 `Http(u16)`** —— 接口层重构（新变体 + 8 个调用点 + 重登策略）。
5. **§1.14 的 WS 超时** 与 **`redact()` 覆盖日志/推送** —— 后者我建议优先（有安全面）。
6. **§2.3 / §2.4 / §2.7** —— 页面级重构，收益是节点数与 RTT，不阻塞功能。
7. **§2.6 lite 运行时那两处 O(n)** —— 按 AGENTS.md 那条秤：先看 §2.1/§2.2 落地效果。
8. **§1.9 登录限速** —— 取决于"服务会不会被暴露到局域网"，用户的环境决策。

## 6. 一句话自查

* 想确认**我有没有采信报告的自述**：没有。§1.1/1.4/1.5/1.7/1.9/1.10/1.11/1.12/1.14/2.3/2.5
  的 grep 我都自己重跑过，读数与它一致（唯一漂的是计数类，见 §3）。
* 想确认**改了的会不会悄悄坏**：三条 Rust 改动各配了会红的钉子，其中 §1.1 那条我**把修复撤掉
  重跑确认它真会红**；前端两条走的是仓内已有的正解形状（`accounts-page` 的 thunk），
  并跑了 `gates-web.mjs --full`（6 条全绿，含 375 窄屏、真 DOM 交互、泄漏量测）。
* 想确认**哪些还不能直接照做**：§5 那 8 条 + 报告 §7 整节 + 所有标「未证实」的格子。
