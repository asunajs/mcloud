# `mcloud signin` 编排层规格 —— 历史段（归档）

> 状态：**归档**。2026-09-22 从 [`../signin-cli-spec.md`](../signin-cli-spec.md) 拆出，
> **内容逐字未改**。这里三段当年是"照此动手"的指导，今天已经是历史记录：
>
> - **§0 的三个落点**：其中 `unmigrated.go` 已被整文件删除，位置由
>   `internal/cli/args_test.go` 的 `TestEveryLeafActuallyRuns` 接管（见 `docs/status.md`）。
> - **§6 与 Rust 的三块职责划分**：Rust 侧已冻结并剥离为同级仓库 mcloud-rust，
>   "Go 该怎么切"已经切完 —— 剩下的价值是"当初为什么这么分"。
>   ⚠ 文中引用的 Rust 行号经抽查仍与冻结基准一致（`signin_run.rs` / `signin_one.rs` /
>   `signin_output.rs`），要看那份代码去 `../mcloud-rust/crates/mcloud-cli/src/`。
> - **§7 不确定清单**：九条里**七条已定**（#1 进度行永远走 stderr、#2 JSON 按产出条目数、
>   #3 prizes 用 typed 不带 raw、#4 三常量放编排文件、#6 deviceId 归属且 upstream 注释已更正、
>   #7 解析失败不提前拦、#8/#9 文案与 `ok` 语义各自已定），**仅 #5 `backup_1` 二次筛选
>   仍未做**（判据要一条实拍，`internal/cli/run_signin.go:1279` 那句注释就是它）。
>   每条的落点写在原文对应位置旁边 —— 没有删任何一条，因为它们记的是"当时为什么不敢定"。
> - **§8 实现落点清单**：八条全部已实现（§8.3 的三个辅助按 Go 习惯改了名与所在层：
>   `selectSigninTargets` 仍在 `run_signin.go:224`，`listAccountsWithIndex` 落到渲染层
>   `signin_render.go:406`，`signinOne` 变成方法 `(*signinRun).one:356`）。

---


## 0. 一句话结论与三个落点

`mcloud signin` 的编排层只做四件事：

1. **选账号**（`--phone` > `--account N` > 全部启用）；
2. **每个账号解析一次设备形态**（`device_id` / `device_short`，**整次运行只解析一次**，
   `signin_run.rs:46-55`）；
3. **按互斥组决定这个账号走哪条子功能**，调 `internal/app` 对应函数；
4. **汇总**：失败计数 + 退出码 + 把每个账号的结果（成功对象 / 失败对象）交给渲染层。

现在这四件事**一件都没有**：`internal/cli/run_signin.go:14` 是 `return notMigrated("signin")`，
`internal/cli/unmigrated.go:20` 登记「缺多账号编排与四态渲染」。

**编排层不该做的事**（越界即返工）：
- 不解析 HTTP 响应、不判 `code`（那是 `internal/upstream` 的）；
- 不判门槛（`cloudType` 分流 / 每月一次机会 / 备份是否开启 —— 都在 `internal/app`）；
- 不拼 `Authorization`（`app.CredentialFromAccount` / `CredentialFromPhone`）；
- 不决定"今天签没签"（`app.SignInReport.SignedToday()`）。

---

## 6. Rust 三块的职责划分，与 Go 该怎么切

### 6.1 Rust 侧切法（含它的历史偶然）

| 文件 | 职责 | 是否值得照搬 |
| --- | --- | --- |
| `signin_run.rs` | 多账号：选 targets、逐账号调 `signin_one`、汇总与退出码；`--list` 渲染 | **职责照搬**，但文件怎么切不必照搬 |
| `signin_one.rs` | 单账号：凭据、设备、profile、子功能分支（records/prizes/raw/status/signin/write-switches） | **职责照搬**，参数形状不照搬（见 6.0） |
| `signin_output.rs` | 纯渲染：`print_*`、`*_json`、`record_cloud_note`、**以及 3 个分页常量** | 渲染归渲染层；**常量归属要改**（见 6.0 第 9 条） |

三个文件的第一行都写着「由 main.rs 拆出」—— 即这套切法是 `main.rs` 变长之后的**历史偶然**，
不是设计。

### 6.2 §6.0 铁律：用 Go 写这个项目，不是用 Go 把 Rust 重写一遍

**边界（必须逐字对齐，一处都不能动）**：
线上字节（header 名与顺序、`percent-encode`、请求体字段名）、错误码与文案、
JSON 键名（`signedToday` / `beanTotal` / `signedNow` / `alreadySigned` / `beanBefore` …，
见 `signin_output.rs:428-481`）、退出码（0/1/2）、终端输出形状（含分隔线、
「签到成功：+N 豆」这类文案）、`--json` 单对象/多数组。

**内部（按 Go 习惯，别照搬）**：

1. **文件怎么切**：Rust 的 `run/one/output` 三块是历史偶然。Go 侧已定：
   编排进 `run_signin.go`，渲染由**另一条线**负责（可能落 `signin_output.go`）。
   编排层**不要**为了"对齐 Rust 三分法"再造一个 `signin_one.go` ——
   Go 的 `run_signin.go` 一个文件装得下。
2. **9 个位置参数**：Rust `signin_one` 有 7 个参数并显式挂了
   `#[allow(clippy::too_many_arguments)]`（`signin_one.rs:20`）。Go 侧应把它收成一个
   小结构体（例如 `signinRun{store, scope, args, up, deviceID, deviceShort}`），
   用方法而不是长参数表。**别把 9 个参数平铺成 Go 函数签名** ——
   那正是"摆错顺序编译器不吭声"的来源（app 层已经用结构体避开了这一类，
   见 `SignInOptions` `signin.go:361`、`ReceiveTarget` `signin_receive.go:131`）。
3. **为 trait 造接口**：Rust 靠 `impl Random` / `impl Clock` / `impl HttpClient` 泛型传依赖。
   Go 侧已经有 `core.Random` / `core.Clock` / `core.HTTPClient` 接口 + `Deps`
   （`cli.go:43-75`）。编排层直接用 `deps.Clock` / `deps.Random` / `deps.http()`，
   **不要**再造一层"签到专用的依赖接口"。
4. **整型宽度**：Rust 的 `usize` / `i64` / `u8` 不要直译。`--account` 是 `int`（下标），
   `--cloud-type` 是 `int64`（`args.go:46`，对齐上游 `i64`）。
   **不要**把 `usize` 翻成 `uint` —— `--account -1` 会因此静默变成天文数字。
5. **`Option<T>` 的零值陷阱**：Rust 用 `Option` 在**类型层面**表达"没给"。
   Go 只有 `pflag.FlagSet.Changed`，所以凡"零值有意义"的字段**必须**走
   `a.given(name)`（`args.go:62-64`）或 `a.accountRequested()`（`args.go:67-72`）。
   已确认的三个雷：`--account 0`（`args.go:67-72`）、`--cloud-type 0`（§1.4）、
   `--device-short ""`。**别给零值编第二种语义。**
6. **错误**：Rust 是 `anyhow::Result` + `bail!` + `{err:#}` 的链式上下文。
   Go 侧是 `*core.Error`（app/upstream 层）+ `ErrUsage` 哨兵（`cli.go:36`）。
   用法错误必须 `fmt.Errorf("…: %w", ErrUsage)`（`args.go:192`），
   否则退码从 2 掉到 1。**别照搬 `{err:#}` 的格式**，文案由 `reportError` 统一加前缀
   （`cli.go:195-200`）。
7. **输出流**：Rust 只在 `--json` 时把进度行 `eprintln` 到 stderr（`signin_run.rs:68-72`、
   `signin_one.rs:137-141`）。Go 侧 `output` 的约定是**进度行永远走 stderr**
   （`output.go:17-22` 的注释是"那样这条性质只在半边被测到"）。
   ⚠ 这是**刻意的行为差异**：非 `--json` 时 `mcloud --account 1 > log.txt` 的进度行
   不在 log 里，与 Rust 不同。见 §7 不确定 #1。
8. **`bail!` 与"业务 fail"**：Rust 里"一个账号失败"是 `Err`（影响退码），
   而"领取失败"是**数据**（`ok:false`，不影响退码）。Go 侧已按这个区分落了类型
   （`MultipleOutcomeFailed` 等）。**编排层不要**把写开关的失败也算进 `failed`。
9. **常量归属**：Rust 把 `RECORD_PAGE_SIZE` / `RECORD_MAX_PAGES` / `PRIZE_PAGE_SIZE`
   定义在**输出文件**（`signin_output.rs:49,54,102`）而**消费**在编排文件
   （`signin_one.rs:157,167,218`）—— 这是错位的切法。Go 侧应把它们放在
   **编排文件**（分页循环是编排的事），别等渲染层先落地。
10. **`--phone` 的语义**：它是"**强制现拼**"的开关，不是"先查库、查不到再拼"。
    Go 对位是 `app.CredentialFromPhone`（`signin.go:121`），**不是** `CredentialFromAccount`
    加一个 fallback。别顺手把两条路合成一条。
11. **`SigninTarget.phone_override` 这个字段本身**：Rust 用一个 `Option<String>` 表达
    "强制现拼"。Go 侧不需要在 target 结构里放这个字段 —— 选账号时直接决定调哪个
    credential 函数即可。**边界不变（仍走现拼），内部少一个字段。**

---

## 7. 不确定清单（要人拍板，别猜）

| # | 事项 | 依据 / 冲突点 |
| --- | --- | --- |
| 1 | **进度行在非 `--json` 时走 stdout 还是 stderr** | Rust：非 json 走 **stdout**（`signin_run.rs:71`、`signin_one.rs:140`）；Go `output.go:17-22` 的既有约定是**永远 stderr**。两者在人看的路径上不同，且 `human`（stdout）/ `note`（stderr）是两个方法（`output.go:29,38`），选错就是"输出形状不对齐" |
| 2 | **`--json` 单对象/多数组的判据用 `results.len()` 还是 `targets.len()`** | Rust 用 `results.len() == 1`（`signin_run.rs:109`）。后果：**2 个账号里 1 个失败时**，`results` 只有那一个失败对象 → 打出**对象**（而非数组），形状静默变化。照搬 = 继承这个怪癖；改判据 = 与 Rust 分叉 |
| 3 | **`--prizes --json` 的 `records` 能不能"原样带出"** | Rust 带 `item.raw`（原始 JSON，`signin_output.rs:227`）。Go 的 `upstream.PrizeRecordItem`（`app_signin_records.go:201-232`）**只有 typed 字段、没有 raw**，且**主动丢掉了 `userDomainId`**（安全上有意，见 `:198-200`）。要么给 upstream 加 `Raw`（会把账号标识带出来），要么改用 typed 结构（键名/顺序/缺字段的 `null` 都与 Rust 不同）。**两条路都不等价，必须选一条** |
| 4 | **三个分页常量的归属与取值** | `RECORD_PAGE_SIZE=50` / `RECORD_MAX_PAGES=20` / `PRIZE_PAGE_SIZE=10`（`signin_output.rs:49,54,102`）在 Go 侧**不存在**。建议放编排文件（§6.0 第 9 条），但"渲染层会不会也想要"没人定 |
| 5 | **`--records` 里「备份翻倍奖励」的二次筛选** | Rust 按 `record.type == "backup_1"` 再列一遍（`signin_output.rs:84-94`）。但 `records[]` 的**字段名没有任何实拍**（`app_signin_records.go:71-78` 明写"也没人抓过"）—— 这个 `type` 判据的来源是 `docs/capture/app-signin-samples.txt` §3.7 的**反推**，不是抓包直读。编排层只需把 raw 原样交给渲染层，筛选归渲染层；但该判据本身仍是"待实拍确认" |
| 6 | **`deviceId` 到底是 `smid` 产出还是"另一个 blob"** | `internal/upstream/app_signin_receive.go:32-33` 的注释说"这里要的是 H5 那个 65 字节 blob，**不是** `smid` 产出的 `B<uuid>`"；而 `internal/app/signin.go:382-387` 与抓包 §4.1 / §9.1 说"**它就是** `DeviceID` 换回来的那个值"，且 §9.1 记了 A/B 已闭环（Rust 产出的 id 打 `receiveV3` 真领到了）。**两处注释互相矛盾**。编排层按 app 层走（`app.DeviceID`，`smid.go:197`），但 upstream 那句注释该改 |
| 7 | **`deviceId` 解析失败时是否提前拦下 `--receive`** | Rust 降级成空串继续跑（`signin_one.rs:104-108, 306`），结果是 `receiveV3` 回 `615`（抓包 §4.2）。Go 的 `ReceiveV3Request` 不校验空串（`app_signin_receive.go:39-47`）。是否在编排层提前报一句更清楚的错（"先去 `smid refresh`"）未定 |
| 8 | **"有账号但全停用"时的提示文案** | Rust 打的是「库里还没有账号。先登录一个…」（`signin_run.rs:42`），与真实情况（有账号、只是停用）不符。照搬 = 继承一句误导文案 |
| 9 | **`--receive` 的 `ok` 语义** | Rust 的 `ok` = "这次调用没出错"，**不等于"领到了"**（`signin_one.rs:457-459`）；领没领到看 `claimed` / `items`。Go 侧要照搬这个两字段约定（`signin_output.rs:295-323` 的 JSON 形状）。⚠ 别把 `ok` 实现成"领到了" |

**明确不标不确定的**（已核实为事实，别再犹豫）：
`toSourceId = 001005`（抓包 §2.1.1 + `app_signin.go:83`）；
`cloudType` 三态分流（抓包 §3.6.2 + `app_signin_receive.go:8-10`）；
`cloudType=1` 那条线的**响应体无实拍**（抓包 §3.6.3 明写"至今没有实拍"）；
`multiple` 的 `Claim` 分支与 `backupgift` 的 `state=0` 分支**都无实拍**（抓包 §3.9.5、§3.10.3）
—— 这三个是"实现了但没跑通过"的路径，**不是**"实现有疑问"，编排层照 app 层写即可。

---

## 8. 实现落点清单（照此动手）

1. `internal/cli/run_signin.go`：把 `notMigrated("signin")` 换成真正的编排。
   - 保留 `a.validate()` 在**最前**（`run_signin.go:9-13` 的注释：用法错误不能被运行错误盖掉）。
   - `store, err := openLocal(a.DB)`（`run_user.go:18`）+ `defer store.Close()`。
   - `owner, err := preferredOwner(ctx, store)`（`run_user.go:30`）→
     `scope := scopeForNewAccounts(owner)`（`run_login.go:493`）。
   - `up := upstream.NewUpstream(deps.http())`（对位 `run_login.go:110`）。
2. `internal/cli/unmigrated.go`：**删掉** `"signin"` 那条登记（`unmigrated.go:20`）——
   `args_test.go` 里对 `UnmigratedBackends()` 的核对会当场要求这一步。
3. 需要新增的编排辅助（建议放 `run_signin.go` 内，不新开文件）：
   - `selectSigninTargets(accounts []core.Account, a *SigninArgs) ([]signinTarget, error)`
   - `listAccountsWithIndex(out *output, accounts []core.Account)`
   - `signinOne(...)`（单账号那一趟，参数收成结构体）
4. **不要**改 `internal/io/`（另一会话在打）；**不要**改 `args.go` 的参数面
   （21 个 flag、互斥表、`validate` 都已就位且被测试钉住）。
5. 退出码不必自己算：`Main` 已经把 `ErrUsage` → 2、`dispatched` → 1 映射好了
   （`cli.go:146-162`）。编排层只需：用法错返回 `%w ErrUsage` 的错误、
   运行错返回**不带** `ErrUsage` 的错误、成功返回 nil。
