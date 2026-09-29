# Review — 整个项目（qwen3.8f / 2026-09-29）

范围是**整个仓库**，不是某一天的提交。分 **一、Web** / **二、Rust** 两部分；当日 diff 的专章在 §3.5。
带 ★ = 本轮由我逐行或跑命令复现过的结论；其余来自分路审读，都给了 `文件:行号`，按同一方法可复核。
**§4 单列"被推翻的结论"**（含我自己的一次假阴性）—— 那几条不要照做。

## 0. 基数与方法

| 项 | 读数 |
|---|---|
| Rust | 8 个 crate、约 67k 行生产码 + 约 6k 行内嵌测试；**1479** 个 `#[test]`/`#[tokio::test]` ★ |
| 分布 | `upstream` 28.0k / `tasks` 20.8k / `server` 5.5k / `engine` 4.9k / `crypto` 3.0k / `app` 3.0k / `core` 2.7k / `store` 2.1k |
| Web | `web/src` 6.7k 行（29 个 ts/tsx）+ `web/lite` ≈3.1k 行框架与工具链 |
| 文档 | `docs/` 4.4k 行，`architecture.md` 已 1237 行（见 §5 拆分建议） |
| 本轮跑过的门 | `cargo check --workspace --all-targets` ✓ / `cargo clippy --workspace --all-targets -- -D warnings` ✓ / `cargo test -p store|engine|tasks` ✓ / `node scripts/gates-web.mjs` ✓ / `node web/lite/regress/all.mjs` ✓ |
| 按约束没跑 | `cargo build`、`cargo test --workspace`（都会链接并覆写 `target/debug/mcloud-server` ⇒ SIGKILL 服务 + TCC 弹框） |

⚠ **本轮没有任何"数据损坏/重复扣豆/未鉴权接口"被复现**，`auth_gate` 是 default-deny，生产路径 `.unwrap()` 只有 3 处 ★。风险集中在三类：**语义与文档分叉**、**编排层无测试保护**、**不可逆动作缺服务端门**。

---

## 一、Web

### 1.1 现实：默认运行时是自研 lite，类型门却还是 Vue 的

`web/vite.config.ts:29-33` 把 `vue` 与 `vue-jsx-vapor` **alias 到 `./lite/src/index.ts`**，当前 `web/dist` 就是 lite 产物；但 `web/tsconfig.json:10` 的 `jsxImportSource` 仍是 `vue-jsx-vapor`。

⇒ 于是出现本轮最重要的结构性问题：**`tsc` 与 `gates-web.mjs` 全绿，不能证明页面正确**，因为门审的是 Vue 语义、跑的是 lite 语义。§1.2 那条回归就是活例子。
`vite.config.probe.ts` 已经能用 lite 编译器把整个 `src` 过一遍，**但没进门禁** —— 这是最省事的一道补口。

### 1.2 阻断 ★ —— 流水线页的步骤列表增删/改序不上屏

三步事实链都可复现：

1. `pipelines-page.tsx:549` 的 `<div key={stepsRev.value}>` 是为 **Vue Vapor 的 keyed fragment** 写的（注释 `:545` 自己这么假设）；lite 在**非 `.map` 位置静默丢弃 `key`**（`web/lite/compiler.ts:293`）。
2. `ref` 是浅响应（`lite/src/signal.ts:9-16`，"不用 Proxy"），而 `addStep/removeStep/moveStep` 是 `d.steps.push/splice` 原地改（`:243-266`）⇒ 不触发任何信号。
3. 把该文件真编一遍：产物里 `stepsRev.value` **只有 5 次自增、0 次读取**；列表编成 `createFor(_n3, () => d.steps, …)`，`d.steps` 是普通属性读 ⇒ 没有可订阅依赖。

**后果**：打开编辑器后点「添加步骤/删除/上移下移」界面不动；而 `draft.value` 被整体赋值（重开编辑）会重建 ⇒ 症状时有时无。
**修法**：① `compiler.ts:293` 对非 map 位置的 `key` 抛错而不是静默丢；② `stepsRev` 改成喂进 `createFor` 的列表源，或三个函数改不可变替换；③ `regress/interact.mjs` 补"加一步/移序"（现在 45/8/10 步交互里没有它，所以全绿）。

### 1.3 重要

| 位置 | 问题 |
|---|---|
| `schedules-page.tsx:494` + `scheduler.rs:389` ★ | legend 写「账号（**手机号**，逗号分隔）」，后端却把输入原样当 `AccountId`（手机号 SHA-16）比对 ⇒ 填手机号 = 匹配 0 个账号 = **定时任务静默不跑**；`:328` 又把 `s.accounts.join('、')` 直接渲染成哈希串。同时违反"界面只见掩码"的硬约定 |
| `web/src/api.ts` 全文 ★ | **全前端没有任何请求超时**（`AbortSignal`/`abort(`/`timeout` 在 `src` 下 grep 0 命中）⇒ 请求悬挂时 `busy/submitting/saving` 永不复位，只能刷页 |
| 显示链 8 份 ★ | `account-names.ts:20` 的 `pick` 未导出，`nickname \|\| label \|\| id` 在 `accounts-page.tsx:439/443/520/872`、`exchange-page.tsx:191`、`tasks-page.tsx:258`、`live-room-page.tsx:401` 各抄一遍；且后端 `label` 是**掩码优先**（`auth.rs:218`）、前端是**昵称优先** ⇒ 同一账号在日志/CLI 与界面名字不同 |
| `account-names.ts:17` + 四页 | `names` 这个 Map 非响应式，而 `refreshAccountNames()` 与各页 `load()` 并发发起 ⇒ 账号接口后到时表格不会再渲染，长期显示哈希 |
| `index.html:28` + `main.ts:16` | boot 动画没有失败兜底：chunk 404/断网/顶层抛错时**无限转圈**，把真实故障完全挡住 |
| `auth-signal.ts:40` ★ | `authEpoch` 是死状态：全 `src` 只有声明与自增两处命中，**没有任何读取方**，而注释声称"页面用它当 key/watch" |
| `scripts/gates-web.mjs` ★ | 依赖 cwd（`fs.mkdirSync('target')` + `npm run`），在 `web/` 下跑给出**假失败且零日志**，还会建出多余的 `target/` |
| `api.ts` ↔ 后端 | 契约手写、无生成。本轮逐字段对过 5 类接口，形状基本一致；缺口是**后端有 `status`（done/cancelled）前端未声明** ⇒ 界面无法显示"已中止"；SSE `RunEvent` 少后端 `Details` 变体（被静默忽略） |
| 触控目标 ≥44px | 10 处违背既有约定：`schedules-page.tsx:523/608/613`、`exchange-page.tsx:244/258/272`、`history-page.tsx:104/191`、`dashboard-page.tsx:91/174`、`live-room-page.tsx:811/834`、`tasks-page.tsx:292/348`、`settings-page.tsx:58`、`accounts-page.tsx:501` |
| `signal.ts` 框架债 | 循环更新无防护（`:96-102`，A→B→A 直接爆栈）；`watch`/`computed` 的 effect 永不释放（`:151/:168`）。现在只有 `app.tsx:117` 一处根级 `watch` ⇒ 未发作，但这是框架的债 |
| `regress/interact.mjs:297-300` | 收了 `Runtime.exceptionThrown`/`console.error` 却**不 increment bad** ⇒ lite 侧吞异常也能全绿 |
| `docs/lite-framework.md:8` vs `:226-230` | 体积宣称自相矛盾（1,892 / 1,514），本机 `node lite/size.mjs` 实测 **2,462–2,620 gzip** ★，漂移约 +30%；且 `bench/lite-side.ts` 是手写目标形态而非编译产物，"挂载快 10×"口径偏乐观 |

### 1.4 lite 语义差的全量扫描（我扫的 + 另一路独立扫的，交叉验证过）

| 坑 | 命中 | 判定 |
|---|---|---|
| 非 `.map` 位置用 `key=` | **7 处**：`pipelines-page.tsx:549`；`app.tsx:251/254/324/331/333/335/343` | 只有 pipelines 是真回归。`app.tsx` 那 6 处**没坏**：分支被编成 `_$setNodes(_n8, () => page.value === 'exchange' ? _$createComponent(ExchangePage, …) : …)`，条件一变就新建实例，且 `remove()` 里有 `disposeTree` + 跑 `onUnmounted`（`dom.ts:221-231`）⇒ 卸载语义由 `setNodes` 兜住。**但注释是错的**：`app.tsx:248` 写"`key` 不能省：没有 key 时 Vapor 可能复用实例"，在 lite 下不成立 —— 见 §2.1 的"代码对、注释错"专类 |
| `ref` 内原地变异且喂渲染 | `pipelines-page.tsx:246/253/262-263`（真 bug）；`pipelines-page.tsx:637/649/674/693/715/738` 是单向绑定的输入框，无更新依赖 ⇒ 无害；`dashboard-page.tsx:57`、`pipelines-page.tsx:271`、`api.ts:697` 是每次新建的局部数组 ⇒ 无害 ★ |
| `class={数组}` | **0 处**（18 处全是模板串/字符串）★ |
| `style={{对象}}` | **0 处**（`style=` 全仓零命中）★ |
| `computed`/`reactive`/`nextTick` | **0 处**；`watch` 1 处 ★ |
| `.map` 缺 `key`（lite 退化索引键 + 整表重建） | `exchange-page.tsx:191/281`（`control.ts:69` 有兜底，正确但整表重建） |

### 1.5 体量与结构

>500 行的"上帝组件"4 个：`accounts-page` 882、`live-room-page` 850、`pipelines-page` 841、`schedules-page` 642；`api.ts` 1145。

重复结构（各页各写一套）：① 三态加载 + 错误 alert + 重试 + 右上刷新 ≈40 行 × **8 个页面**；② 账号下拉 + "还没有已登录账号" + 单账号预选 × 3（`tasks-page.tsx:235-264`、`live-room-page.tsx:379-411`、`exchange-page.tsx:172-195`）；③ 运行 harness（start/abort/SSE/ticker/finish/onUnmounted）—— `tasks-page.tsx:86-213` 与 `live-room-page.tsx:79-343` 近乎逐行相同。

按现有 import 边界，**最省事的拆法**：抽 `ui/account-select.tsx`（消 3 份）→ 抽 `ui/run-session.ts` 纯函数（消 2 份 harness，顺便把 live-room 的 `try/finally` 教训固化）→ `api.ts` 里 `toLiveRoomRunView` 一族（约 702-1034）挪去 `liveroom-view.ts` → accounts-page 的四种登录方式拆子组件。范本已有：`schedules-page` 用 `summaryOf/actionsOf` 让表格与卡片共用。

### 1.6 Web 侧写得好的

`api.ts` 的 code-vs-text 分工注释与 `isAuthActionFailure` 按方法分流；**SSE 生命周期是全仓最扎实的一块**（终局自动 close + 返回取消函数 + 两个页面 `onUnmounted` 都关流停 ticker + `Last-Event-ID` 断线续传两端接通）；颜色一律查表字面量（防 Tailwind 扫描失效）；原生 `<dialog>` 换来免费的 Esc/焦点/面板语义；`regress` 的双运行时逐字符 + 逐像素对比是罕见的高质量迁移验收；`control.ts` 把倒序遍历/正序成段插入的坑写成可复现依据。

---

## 二、跨端共性：代码是对的、注释/文档是错的

本轮有一类问题重复出现，单列出来，因为**它们的代价是"下一个人照注释动手"**：

| 位置 | 注释/文档怎么说 | 实际 |
|---|---|---|
| `app.tsx:248` | "`key` 不能省，没有 key 时 Vapor 可能复用实例" | lite 下 key 被丢弃，靠 `setNodes` 重跑达成同样结果 |
| `client.rs:103-105/211`、`Cargo.toml` WS 段 | "按主机选后端：只有老式主机走 native-tls" | 两边已是同一后端，实际差别只剩两个 cookie jar（★ 见 §3.5 TLS 条） |
| `live_consts.rs:25-58` | "今天打不通 / 补法缺依赖" | 已过期，`:55-57` 自相矛盾 |
| `auth-signal.ts:40` | "`authEpoch` 供页面当 key/watch" | 无人读 |
| `docs/conventions.md:288-289` | "判凭证失效必须用 `is_auth_failure()`" | 该函数**零调用者** ★ |
| `docs/task-inventory.md:32` | "`app-task` 这个名字今天不存在" | `lib.rs:81` 当天已注册 ★ |
| `docs/lite-framework.md:8/226` | 1,892 / 1,514 gzip | 实测 2,462–2,620 ★ |
| `session.rs:345-349` | "`serde_json` Map 默认按插入序输出" | `preserve_order` 未启用 ⇒ 是字典序（同仓 `auth_refresh.rs:103-112` 的说法才对） |

---

## 三、Rust

### 3.1 协议内核（core / crypto / upstream）

**装饰链是收敛的** ★：全仓 `reqwest::Client::builder()` 只有 `client.rs:205` 一处（其余是测试自建），装配分工 `SmidClient → H5Client → ReqwestClient` 清楚（`builder.rs:114-166`），凭据三枚都有"已显式给了就不补"的护栏 `should_inject_credential`（`client.rs:245-251,291-305`），理由（双同名头 ⇒ APISIX 直接 400）是实测钉出来的。

问题按代价排序：

| 级别 | 位置 | 问题 |
|---|---|---|
| ⭐ 重要 | `mcloud-app/src/auth_refresh.rs:173-180` ★ | **刷新成功后过期时刻永远不推进**。这里把**明文** `"platform:phone:token"` 喂给 `parse_auth`，而 `parse_auth` 的入参契约是 **base64 整串**（`auth.rs:45-122`：先剥 `Basic `、再 `node_like_b64_decode`，且**丢掉字母表以外的字符** —— `:` 就在被丢之列）。⇒ 解不出或解出错位，`parsed.expire > 0` 恒不成立，`updated.auth.expire` 保留**旧值**并落库（`:182`），`rotate()` 还把旧值当新值返回（`:191`），Web 的 `expire` 列照它显示（`routes/accounts.rs:65`）。**后果**：`decide()` 下次仍判"该刷" ⇒ `--all` 一轮又一轮真轮换凭据（不可逆）。对照同族正确写法：`mcloud-tasks/src/login.rs:493-500` 把整串交给 `parse_auth`。`auth_refresh/tests.rs` 覆盖了三档判据却**没有一条覆盖 `rotate()` 的 expire 再推导** ⇒ 不在任何门内 |
| 重要 | `client.rs:316-320` ★ + `login.rs:263-293` | PC 形态账号的 `pc.business_headers()`（`x-yun-client-info`/`x-DeviceInfo`/`x-MM-Source`/`x-huawei-channelSrc`/`x-yun-app-channel`）是**无条件 append**，没走 `should_inject_credential`；而 `app_cloud_outlink.rs:58`、`cloud_trash.rs`、`app_cloud_upload.rs:259-271`、`app_login.rs:315-326` 自己也带同名头 ⇒ 同一发请求**两根 `x-yun-client-info`、值还不同**。`app_cloud_upload.rs:52` 自己写着"唯一承重的是 `x-yun-client-info` 的值"。**上游取哪根：未证实**（本仓无这一类样本），但这正是"任务跑得通、某个按钮永远 401"的形状 |
| 重要 | `error.rs:68-81` ★ + `mcloud-core/src/error.rs:17` | "需要重新登录"这一档**实际产不出来**：`Error::AuthExpired` 全仓**零构造点**（只有两处 match 臂）；上游真实信号 `05050009 非法的token！`、`90001` 都不映射过去 —— `90001` 变成 `Error::Other("会话被拒：code=…")` 字符串（`session_source.rs:230-238`），非 2xx 一律 `Error::Network`（`session.rs:667-676`）。而唯一判据 `is_auth_failure()` **零调用者**，它匹配的码值 `"1001"|"1002"|"401"|"AUTH_FAILED"|"TOKEN_EXPIRED"` 在文档里找不到抓包依据（`docs/` 里的 1001/1002 全是 `sourceId`/`activityid`）⇒ **码表未证实**，与 `conventions.md:288` 的要求脱节 |
| 重要 | `app_task.rs:511/537/568`、`task_expansion.rs:447/509/565` | `parse_file_create/parse_file_complete/parse_batch_trash` **不过 `ensure_http_ok`**，而 `session_source`/`live_*` 39 处都过 ⇒ HTTP 500 的 HTML 被报成"JSON 解析错"，一个上游故障被归类成代码 bug |
| 重要 | `live_ws_client.rs:318` | WS **握手 `connect_async(request).await` 没有超时**：外层 `listen_for` 只覆盖"听"的循环（`:365-366`、`:405-408`），TCP/TLS 挂起时该 run 无限等。engine 侧也没有任务级超时（`tokio::time::timeout` 只出现在测试与 `runner.rs:799`） |
| 重要 | `session_source.rs:158-177` | 唯一能让非幂等写**双发**的路径：`90001` 刷新后重放那一条，判据只是"顶层 `code == 90001`"。重放安全的前提是"回 90001 ⇒ 上游没执行过"，而本仓 `describe()` 自己说 90001 是"被挡下"不是"未执行"（`:200-209`）⇒ **未证实**，属需要实拍的一格 |
| 重要 | `builder.rs:166-179` ★ | 账号客户端构建失败就退化到 `ClientProfile::default()`（无凭据、`timeout_secs: 1`、无 H5/Smid 装饰）的裸客户端，只在构建那一刻 `tracing::error!` 一条 ⇒ 此后该账号所有请求恒定鉴权失败，而任务侧只看到"网络错误"。且 `:177` 的 `.expect()` 在工厂闭包内，release 是 `panic="abort"` ⇒ 真在这里 panic 是整个服务没掉 |
| 次要 | `account.rs:136-143` vs `auth_refresh.rs:62-75` ★ | 两套**相反**的阈值且一套是死码：`needs_refresh()` 对 `expire <= 0`（未知）判"保守不刷"，`RefreshDecision::ManualOnly` 对同一情形硬拒绝；`needs_refresh` 与 `AccountSettings.refresh_token_days = 10` **零生产调用者**，实际生效的是 `DEFAULT_LEAD_DAYS = 7` |
| 次要 | `event.rs:188-199` vs 调用点 ★ | `redact()` 注释说"事件会被写入磁盘并展示在浏览器里"，但它**只有一个调用点**（`mcloud-server/src/error.rs:35`）；`RunEvent::Log{message}` 与 `Finished` 都不过它。而 `ensure_http_ok`/`apply_refresh` 会把响应体前 200 字符塞进错误 ⇒ 上游 5xx 若回显请求头就会被带出来：**未证实** |
| 次要 | `mcloud-app/src/smid.rs:331-361` + `mcloud-upstream/src/smid.rs:699-740` | 换设备号失败时**吞掉错误、照常发不带 `deviceid` 的请求**；而底层 `smid_device_id` 返回 `Err`、旧 TS 返回 `""` ⇒ 两层合起来把"没有设备号"变成静默行为差异 |
| 次要 | `session_source.rs:85-114`、`mcloud-app/src/smid.rs:224-284` | 取票 get→build→put **没有 single-flight** ⇒ 同账号同配方并发时重复打三枪（多做功、不写坏数据）；`SmidService` 首次使用也无互斥 |
| 次要 | `endpoints.rs:27`、`crypto/sign.rs:89-98` | TS 有的 legacy `tyrzLogin.action` + `sign_headers` 在本仓是**挂着但无生产调用者** |
| 次要 | `tn.rs:15` vs `:37-39/:61` | 注释说"数组也递归"，实现对数组返回空串；TS 会把元素值拼进去。当前特征体没有数组字段 ⇒ 无现症，潜在偏差 |
| 次要 | `live_consts.rs:58-67` | `LEGACY_TLS_HOSTS` 只列 `v.cmcc-cs.cn`，IM 侧另有 `videoim.cmcc-cs.cn:20103/20102`；那台若同样只给 CBC 套件就漏了 —— **未证实**（无实测） |

**smid / 设备指纹**：与 `mcloud-sign` 逐条对得上（`ep` RSA-PKCS#1v1.5、`priId = md5(uid)[0..16]`、DES 25 条规则、`tn` 必须在 `tn` 仍为空时算、gzip 后把下标 9 的 OS 字节改 19、AES-CBC 固定 IV），每次现掷的 5 个随机字段与 TS 一致（**是 parity 不是偏差**）；稳定身份由落库的 `SmFingerprint` 承担。两处刻意偏差（东八区时间串、`smid` 失败返 `Err`）都写了理由。
**重试**：本仓无自动网络重试（与 TS `retry: 0` 一致），三处例外各自成策；**写侧的"零重试"是有测试兜住的**（`exchange/tests.rs:1874-1881`、`:1910-1943`、`:1450-1464`）—— 这是本轮看到最好的一处纪律。反向缺口：TS 换 SSO 票有 5 次指数退避（`packages/core/src/auth/index.ts:14-37`），本仓三枪各打一次。

### 3.2 引擎 / 存储 / 任务

| 级别 | 位置 | 问题 |
|---|---|---|
| 重要 | `scheduler.rs:273` ★ | **"本文件最重要的一行顺序"没有任何回归保护**。全仓无测试调用 `tick()`/`fire()`（grep 复核）；变异复现：注释掉 `advance_watermark(...)` ⇒ `cargo test -p mcloud-engine` **103 passed 全绿**（该代理已还原，我核过 `git status` 干净 ★）。建议补两条：同触发点连跑两次 `tick()` 只 fire 一次；fire 返回 Err 后水位不回退。顺带把 `tick()` 内的 `.await?`（`:259`/`:273`）改成记录并 continue，避免一个坏 schedule 中止本轮其余定时 |
| 重要 | `json_store.rs:513-524` ★ | `remove_account` 只级联 `state` + `smid`，**不动 schedules/pipelines** ⇒ 已删账号仍被定时器引用，下次 tick 对不存在的账号 fire。`json_store/tests.rs` 恰好只断言了 state+smid 被清，掩盖了这个面 |
| 重要 | `exchange.rs:1471/1493`、`lib.rs:49-62`、`routes/runs.rs:81` ★ | 不可逆任务**没有服务端门**：exchange 今天花不出豆的**唯一原因是 `device_id: None`（硬编）= 缺功能**，而不是门；`POST /api/runs` 只要任务名在注册表就放行（`/api/capabilities` 走 `registry.list()`，含隐藏任务）⇒ 绕过专用页也能触发。`app-task`/`app-task-matrix` 连 `params()` 都不覆写（无 dryRun），却做 `task/click`（写账号、不可撤销）与外链分享。另：**代码里没有任何任务覆写 `fn hidden`** ★（只有 trait 默认实现与测试夹具），而 `task-inventory.md` 已把 exchange 标为 Hidden —— 机制其实是通的（`registry.rs:99-107` + `tasks-page.tsx:76`），只是没人用 |
| 重要 | `routes/schedules.rs:69`、`routes/pipelines.rs:27` + `scheduler.rs:386` ★ | `deny_unknown_fields` 全仓 **0 处** ★，而这两处 `accounts` 是 `#[serde(default)] Vec<String>`、空值语义 = **全部账号** ⇒ 字段名打错一个字母（`acounts`）就静默把爆炸半径扩到所有账号，其中含真扣豆的 `exchange` |
| 次要 | `json_store.rs:281-307` | `write_atomic` 有 temp+rename + `.bak` + 0600/0700，但**无 `f.sync_all()`、无父目录 fsync** ⇒ SIGKILL 安全（rename 原子），断电可能新文件未落盘只剩 `.bak` |
| 次要 | `json_store.rs:662-688` | `append_logs` 持写锁取快照后**释放锁**再写盘 ⇒ 同 run 并发 append 有 lost-update 面；目前靠 hub 单 flusher 兜（脆弱契约） |
| 次要 | `json_store.rs:60` | **无 schema 版本、无迁移器**：`f3230e1` 的主键改动只发生在代码层 + 手工改文件，开库不校验、手工编辑 JSON 不会被发现 |
| 次要 | `json_store.rs:377-393`、`:647` | 生产路径从不写 `RunStatus::Running` ⇒ 进行中的 run 不在 history.json，`recover_orphans` 与 `running_runs()` 实为死码（安慰剂） |
| 次要 | `json_store.rs:205-232` | `harden_existing_permissions` 只覆盖 accounts/runs/schedules/pipelines/state，不含 smid/users/sessions/pc_device |
| 次要 | `signin.rs:378-427` | `revival` 是幂等判据里唯一的"盲发"（靠上游"本月已领取"错误码去重，注释已标为过渡态）；其余一律以上游读为准（`signin.rs` 用 infoV3 判活，因为 `startSignIn` 会骗人）⇒ **偏差方向明确偏保守，这是对的** |

**调度语义**（核过，符合注释）：只接受 `FixedOffset`、坏时区显式报错；过期窗口整体弃掉不逐点补跑（`GRACE_MS=60s`，"宁可漏跑不可重跑"）；定时器与手动共用 `task:account` 闸、忙则**跳过不排队**；`start()` 在 spawn **之前**同步注册取消 token（`runner.rs:89-117`）+ `routes/runs.rs:173-175` 三源存在性检查 ⇒ SSE 404/重连竞态**已闭环**。

**任务盘点（18 个，`lib.rs:63-88` ★ 与文档"18 条"对得上）**：只读 2（`cloud-snapshot`、`task-list`）；可逆 12（`sign`/`receive`/`multiple`/`backup-gift`/`revival`/`prizes`/`live-room`/`msg-push`/`fun-ai`/`notice`/`play-ai`/`task-expansion`）；**不可逆 4**（`red-flower` 消耗抽奖次数——**只有它有 dryRun**；`exchange`；`app-task`；`app-task-matrix`）。

### 3.3 服务端 / CLI / 安全面

**没有"能改状态却无鉴权"的接口** ★：`auth_gate` 是 default-deny（非 `/api/` 才放行，`/api/` 除 6 条公开白名单外一律先查会话），实测 `curl /api/accounts` 未登录 → 401。三态（503/401/放行）+ **失败关闭**（`auth_gate.rs:86-96`：读不出用户数就 500 而不是放行）+ 6 条单元测试锁死"方法必须匹配""末尾斜杠不能绕过"。

| 级别 | 位置 | 问题 |
|---|---|---|
| 重要 | `mcloud-app/src/hub.rs:151/209` ★ | **EventHub 的内存态与 SSE 订阅者永不回收**：`forget`/`unsubscribe` 在生产路径**零调用点** ★（grep 只命中 `hub/tests.rs`；`runner.rs:278` 那个 `artifacts.forget` 是同名不同物）。每跑一趟常驻最多 2000 条事件（`hub.rs:44`）、每个订阅者最多 1024 条队列（`:49`），而服务由 launchd 长期不重启 ⇒ 单调增长 |
| 重要 | `routes/auth.rs:154` + `crypto/password.rs:36` | 在 `async fn` 里**同步**调 `verify_password`（argon2id `m=19456KiB,t=2`，参数是为与 Go/TS 金标互认、非 OWASP 强度），且**无速率限制/锁定** ⇒ 暴破没有节流、并发登录吃掉 worker 线程。建议 `spawn_blocking` + 退避 |
| 重要 | `routes/auth.rs:253-276` ★ | `require_session` **不校验 `SessionKind`**（只看哈希与过期）。`user.rs:100-111` 里 `expires_at_ms == 0` = 永不过期（CLI 用）⇒ 一旦将来签发 CLI 会话就是长效旁路。**当前全仓没有签发 `SessionKind::Cli` 的代码** ★，所以这是推断的放大面、不是现症 |
| 重要 | 三处入口 ★ | 不可逆动作**无二次确认**：`POST /api/accounts/{id}/exchange`（`routes/exchange.rs:362`）、`POST /api/runs(task=exchange)`、CLI `run exchange`（`bin/cli.rs:110-144`，无 `--yes`、**且 CLI 完全不经过登录闸**）。而 `web/src/ui/nav.ts:92` 写着"点之前会再确认一次" —— UI 有、服务端没有 |
| 重要 | `json_store.rs:207-221` + `:283` | `users.json`/`sessions.json` 不在权限收紧清单（老目录里 argon2 哈希与 token 哈希可能仍是 0644）；`write_atomic` 每次留 `.bak` ⇒ **凭据明文常驻第二份** |
| 重要 | `routes/login.rs:50-75` | 扫码会话表只在轮询到终态或显式 `DELETE` 时移除 ⇒ 客户端中途放弃则条目永久留在 `HashMap` |
| 次要 | `cli/login.rs:156/246` | 完整手机号 `println!` 回显到 stdout（crontab 重定向即留存）。★ 全仓日志侧未发现把 `auth.raw`/`token`/完整 `phone` 写进 tracing 的点，标识一律走 `mask_phone`/`account_label`/16 位哈希 |
| 次要 | `error.rs:35` + `client.rs:143/257` | `redact` 键表不含 phone；reqwest 的 `Display`（含 URL）被拼进 `Error::Network` ⇒ 将来若某 URL 的 query 带手机号会进 502 响应体（**当前没有**这种构造，已 grep） |
| 次要 | `routes/*` 测试面 | 写路由（PUT schedules、POST exchange、DELETE accounts）**没有"未登录应 401"的用例**，`tests/routes.rs:805-812` 只覆盖 4 个 GET |
| 次要 | `routes/pipelines.rs:146` | 自造的 `CancelToken` 未登记到取消表 ⇒ 从 `/api/runs/{id}/cancel` 停不掉整条链 |
| 次要 | `routes/health.rs:15` | `/api/version` 公开返回产品名+版本；cookie 刻意无 `Secure`（本地 http，有据），但 `MCLOUD_ADDR` 设成非回环时**没有任何告警** |
| 次要 | SSE 越权面 | 事件流**没有按账号/用户过滤**（`runs.rs:173-186` 只判 run 是否存在，run id 是自增整数）。单管理员模型下不构成越权，多用户化后必须补 |

**依赖**：`cargo audit` 未安装（出不了漏洞结论）。`Cargo.lock` 271 包、单源；**rustls/ring/webpki-roots 确已退出** ★（只剩 reqwest 拉的 `rustls-pki-types` 类型壳）。重复版本集中在 crypto：`getrandom 0.2+0.4`、`rand 0.8+0.10`、`digest 0.10+0.11`、`sha1/sha2` 两代、`base64 0.22+0.23`。

### 3.4 全项目视角的纪律

- `unsafe_code = "forbid"` 与 lint 基线：**逐 crate 核过有没有漏写 `[lints] workspace = true`**（本轮 clippy `-D warnings` 全绿 ⇒ 继承有效）。
- 生产路径 `.unwrap()` 共 **3 处** ★，全是 `Mutex::lock().unwrap()`（`routes/login.rs:60/65/73`），**无一来自 HTTP 请求或上游响应**；`expect` 41 处多为锁中毒与常量正则。统计口径：排除 `tests.rs`/`tests/`、并截断到 `#[cfg(test)]` 之前。
- 但 `builder.rs:177` 那个 `.expect()` 在**工厂闭包**里，与 `panic="abort"` 组合会带走整个进程（§3.1）。

### 3.5 当日 diff 专章（54 commits / 68 文件 / +7655 −257）

这一节保留自上一版 review，是 §3.1–§3.4 的局部放大：

1. **`app_task.rs:400-412` + `:297-307` ★**：`share_first_uploaded` 失败只 `problems.push(...)`，不涨 `acted`/`failures`，而 `task_outcome()` 只看 `planned/acted` ⇒ 同趟有别的成功就报 `success`。最坏情形（"建了却删不掉的外链"）不影响结局位。
2. **`app_task.rs:416` + `:441` ★**：循环里 `check_cancelled()?` 跳过 `cleanup` ⇒ 最多 5×N 个文件留网盘；且全文件不碰 `ctx.artifacts`（grep 0 命中）⇒ 遗留文件不进账本。
3. **`app_task_matrix.rs:130-152` vs `app_task.rs:548` ★**：`merge_pages` 按 **name** 去重、写侧按 **task_id** 打卡 ⇒ 去重键 ≠ 写入键；同 id 异名会双发不可回滚的 click，且 `WAIT` 那份绕过状态门。**上游会不会给同 id 异名：未证实**。另 `planned` 是 16 次拉取开头的快照，522 档踱步数分钟期间看不见上游改状态。
4. **`app_task.rs:292` vs `:340` ★**：`failures` 装的是"读取 `{market}/{group}`"这种拉取失败，summary 却说「{N} **条**有失败」；step 级真失败一句不进。
5. **TLS 装置空转 ★**（`client.rs:101-105/263-269`）：`base()` 与 `base().use_native_tls()` 现在是同一后端（根 `Cargo.toml:140` 只编 native-tls；`cargo tree -e features` 无 rustls ★），实际差别只剩两个互不相通的 cookie jar。四处 rustls 旧说法未跟改（`live_ws_client.rs:63/289`、`mcloud-upstream/Cargo.toml:25`、`live_consts.rs:25-58`）。
6. **`release.yml`（275 行，从未跑过）**：`:38` `tags: ['v*']` 无校验（带斜杠的 tag 会毁掉 `:141` 的包名）；`:42-43` 顶层只写 `contents: write` ⇒ 未列 scope 变 none、且 `web`/`build` job 白拿写权限；Linux 在 `ubuntu-latest`(glibc 2.39) 编译且**动态链 OpenSSL**，`usage.md:67-72` 只提 `libssl3` 没写 glibc 下限 ⇒ 与"拷一个二进制就能跑"冲突。
7. **`ci.yml:37-39`**：砍掉 push→main 后，main 上只剩打 tag 时那条 `cargo build --release`（不跑 fmt/clippy/test），而 `AGENTS.md` 要求的 `cargo test --workspace` 恰是覆写二进制那条链 ⇒ "提交前必须全绿"**事实上无人强制**。
8. **`panic="abort"` 的代价比注释多一项 ★**：0 处 `catch_unwind`/`should_panic`/锁中毒处理，但 `main.rs:44/47`、`runner.rs:108`、`routes/pipelines.rs:145` 4 处 fire-and-forget `tokio::spawn` 的语义是"这个 run 挂了只留一条日志" ⇒ 任一后台任务 panic = 整个服务没；`strip=true` 又让 `AGENTS.md` 依赖的 `.ips` 崩溃报告只剩地址。
9. **`cli/auth.rs:206-207` ★**：把 `a.id`（16 位哈希）当显示列，与"主键只当键"自相矛盾（`--account` 确实需要 id，正确形状是 id 照打 + 旁边补掩码）。
10. **静态资源**：`static_files.rs:106-108` 的"末段带点 ⇒ 404"对本仓安全（hash 路由 + `web/dist` 只有 `assets/` 与 `index.html`），注释也写了前提；但前提变了（history 路由 / 带点深链接）就会误 404，且 404 无 `Cache-Control`。
11. **测试空白**：`run_app_task` 执行层零测试（删掉 `app_task.rs:350-354` 那行 filter 全套仍绿）；434 的失败面无一条目；`cleanup` 只测了字符串长相没测提交的 id 集合；`app_task/tests.rs:86` 把分组写死 6 个而 `TASK_LIST_V2_GROUPS` 是 8 个。
12. **`docs/usage.md`**：核过一致的是 `MCLOUD_ADDR=127.0.0.1:3000`、`MCLOUD_DATA_DIR=data`、口令下限 8 位、包名形状；不一致的是 `:41` 日志样例 `data_dir="data"`（实际输出 `data_dir=data`）、`:96` 漏 `login device`、`:27-28` 的快速开始少了 Windows zip 那层 `cd`。`vite.config.ts` 的 dev 代理写死 38123 而 usage 教的是 3000 ⇒ 照 usage 起服务再开 dev 会静默连不上。

### 3.6 Rust 侧写得好的

`auth_gate` 的失败关闭；凭据边界是**结构性**的（`AccountSummary` DTO + `summary_never_leaks_credentials`、`PasswordHash` 手写 Debug 脱敏而 serde 保持真值、`EventHub`/`JsonStore`/`SessionCache` 全部手写 Debug）；会话只存 `sha256(token)`、令牌 32 字节 `OsRng`；用户名枚举用"同一句话"堵死并有测试；`redact` 对非 ASCII 下标错位有专测；存储层临时文件**创建时即 0600**（不是先写再 chmod）、写锁中毒返回错误而不是 panic；`account_id_for` 的测试**先断言"掩码确实撞"**再断言哈希不撞；`exchange` 的 17 档计划纯函数 + `a_rejected_plan_sends_nothing` 这类"不发请求"断言；写侧"超时那一笔可能已成交 ⇒ 绝不重发"有测试钉住；`explain_send` 把 reqwest 只印最外层的 `source()` 链走全；`static_files.rs:209-221` 诚实标注"这条测不到装配"。

---

## 四、被推翻 / 修正的结论（不要照这些动手）

| 结论 | 复核结果 |
|---|---|
| ~~`release.yml:157` 的 Linux/macOS 冒烟几乎必红~~ | **不实**。那两步没写 `shell:`，Actions 默认是 `bash -e {0}`（**不带 pipefail**），管道取右侧 `grep` 状态 ⇒ 退出 0。我本地两种 shell 各跑一遍：默认 `bash -e` 打出 `冒烟 ok / step exit=0`，显式 `shell: bash` 才中断 ⇒ 工作流 `:176` 那句注释是**对的**。降级为：哪天补 `shell: bash` 就会红 |
| ~~`release.yml:202` 把 `sha256sum` 拼成 `sha25sum`~~ | **不实**，全仓 grep 无 `sha25sum` |
| ~~"生产路径 panic 会带走进程"是现症~~ | 生产 `.unwrap()` 只有 3 处且输入不来自外部 ★ ⇒ 只 `builder.rs:177` 那处 `expect` 是真敞口 |
| ~~`SessionKind` 不校验 = 越权已发生~~ | 是**推断的放大面**：当前无任何代码签发 Cli 会话 ★ |
| 我本轮自己的假阴性 | 第一次扫 `key=` 用了 `key={` 模式，**漏掉 `app.tsx:251/254` 的字符串形态 `key="setup"`**。⇒ 非 map 位置用 key 是 7 处不是 6 处（结论不变，那两处同样无害）。教训：按语法形态扫描要先枚举引号/花括号/展开三种写法 |

---

## 五、建议顺序（按"做错了能不能撤"排）

1. **补门，再改代码**（否则"删掉关键一行仍全绿"会继续骗人）：把 `vite.config.probe.ts` 编进 `gates-web.mjs` 第三条；tsconfig 的类型来源对齐 lite；`regress/interact.mjs` 改成"异常即判负"；`compiler.ts:293` 非 map 位置的 `key` 改抛错；`gates-web.mjs` 断言 cwd。
2. **两条用户已能看见的坏**：pipelines 步骤列表（§1.2）、schedules 的手机号文案 + 哈希显示（§1.3）。
3. **不可逆动作的账**：`auth_refresh` 的 expire 再推导（现在会反复真轮换凭据）★；`leftover_link_ids` 强制 `failed`；`cleanup` 无条件跑 + `ctx.artifacts` 记账；给 ③ 类任务加服务端 `confirm` 硬门 + 每账号每日上限；`remove_account` 级联 schedules/pipelines；`accounts` 字段加 `deny_unknown_fields`。
4. **资源与鲁棒性**：`EventHub` 的 `forget`/`unsubscribe` 接上；WS 握手包超时；`verify_password` 走 `spawn_blocking` + 登录节流；前端 `request` 加 `AbortSignal.timeout()`；`write_atomic` 补 `sync_all`。
5. **发版前**：tag 校验 + `permissions` 收窄 + Linux 产物可移植性；决定 `panic="abort"` 的隔离代价是否接受。
6. **可延后**：显示链收敛成一处、`app_task_matrix` 的 name/id 去重口径（先确认上游会不会给）、§1.5 的组件拆分、§2 那张"注释与实现对不上"的清单逐条改写。
7. **文档体量**：`docs/architecture.md` 1237 行、`lite-framework.md` 682 + `lite-guide.md` 667 有重叠。建议按"框架设计 / 上手教程 / 决策记录"三份归位，`architecture.md` 拆出 §10（构建与产物）独立成篇。

## 六、未证实的格子（本轮拿不到读数，别当结论用）

双同名头与 `90001` 的上游实际行为；`jwttoken` 是否被某接口要求为 cookie；`videoim.cmcc-cs.cn` 的 TLS 套件；`is_auth_failure` 那五个码值；上游 5xx 页是否回显请求头；`mrp.mcloud.10086.cn`（`endpoints.rs:16` 列了但全仓无请求）；`merge_pages` 的同 id 异名样本；`f3230e1` 对 `schedules`/`pipelines` 存量的影响（本机 `data/` 无这两个文件）；pipelines 回归的活体点击（推论来自编译产物 + 源码语义）；`release.yml` 全部问题（该工作流从未跑过）；依赖漏洞（`cargo audit` 未安装）。
