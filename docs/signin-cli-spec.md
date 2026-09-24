# `mcloud signin` 编排层规格
> 2026-09-22 拆过一次：当年"照此动手"的四段（§0 落点、§6 与 Rust 的职责划分、
> §7 不确定清单、§8 实现落点）已逐字移入
> [`archive/signin-cli-spec-history.md`](./archive/signin-cli-spec-history.md)。
> **怎么敲命令**看 [`cli-usage.md`](./cli-usage.md)（操作口径的唯一来源），
> 本文只留**每条判据与形状**：判据/形状看这里，操作与推荐用法看那边。


> 交付物：**只读调研**的产物，用来指导 `internal/cli/run_signin.go` 的实现。
> 读完本文件应当**不需要再回头读 Rust**。
>
> 判据来源（每条结论都带 `文件:行`）：
> - Rust（对位基准，只读）：`../mcloud-rust/crates/mcloud-cli/src/signin_run.rs`（225 行）、
>   `signin_one.rs`（558 行）、`signin_output.rs`（481 行）、`args.rs:96-330`
> - Go（已就位）：`internal/app/{signin,signin_receive,signin_bonus,cloud,smid,device,app_login}.go`、
>   `internal/upstream/app_signin*.go`、`internal/cli/{args,cli,output,unmigrated,run_user}.go`
> - 抓包：`docs/capture/app-signin-samples.txt`（下文简称「抓包」）
>
> ⚠ 凡**没有实拍**的上游路径，一律显式标「旧代码写法」或「无实拍」。
>
> 范围边界：本文只覆盖**编排层**（选账号 → 调 app 层 → 把结果交给渲染层 → 定退出码）。
> 终端文案与 JSON 形状由**渲染层**（另一条线，对位 `signin_output.rs`）负责；
> 本文只写清「渲染层要消费哪些数据、哪些字段在什么情况下为 nil」。

---

## 0. 一句话结论与三个落点 —— **已移出**

> 逐字搬至 [`archive/signin-cli-spec-history.md`](./archive/signin-cli-spec-history.md)。
> 该段点名的 `unmigrated.go` 已被整文件删除，位置由 `internal/cli/args_test.go`
> 的 `TestEveryLeafActuallyRuns` 接管（`docs/status.md` 步 4 那条）。

---

## 1. 21 个 flag 逐条

`SigninArgs` 在 `internal/cli/args.go:36-59`；注册在 `registerSigninFlags`（`args.go:78-126`）。
**21 个** = Rust 的 20 个字段 + `--h5-device-id` 别名（`args.go:100`）。

互斥表 `signinExclusive`（`args.go:133-159`）、单向依赖 `signinRequires`（`args.go:167-169`）、
手工校验 `validate`（`args.go:189-196`）都已就位 —— **编排层不要再判一遍互斥**，
只调用 `a.validate()`（`run_signin.go:11`）。

### 1.1 选择类（决定"跑谁"）

| flag | 定义 | 语义 / 分支 | 落点 |
| --- | --- | --- | --- |
| `--account N` | `args.go:81-83` | 账号**下标**，从 1 开始，含**停用**账号（`signin_run.rs:189-212`）。`N == 0` → 报错「下标从 1 开始」（`signin_run.rs:191-193`）；越界 → 报错并提示 `--list`（`signin_run.rs:194-199`）；停用 → **照跑**但 stderr 提醒（`signin_run.rs:200-207`）。 | 编排层自实现（无 app 层函数）。⚠ **运行错误**，退 1（见 §5） |
| `--list` | `args.go:84-85` | 打清单后退出（`signin_run.rs:34-36` → `list_accounts_with_index`，`signin_run.rs:142-161`）。**在选账号之前**短路；**忽略 `--json`**（Rust 该分支直接 `println!` 表格，不查 `args.json`）。 | `store.ListAccounts`（`core/traits.go:650`）。互斥 `--account` / `--phone`（`args.go:134-135`） |
| `--phone` | `args.go:86-88` | 老账号逃生口。**优先级最高**，直接短路（`signin_run.rs:179-185`）：`index = 0`，`phoneOverride` 非空 → **强制**走现拼那条路。用 `core.AccountLabel(nil, phone)` 推 id（`signin_run.rs:182`，Go 对位 `core.AccountLabel`，`core/auth.go:342`）。 | `app.CredentialFromPhone`（`signin.go:121`）。跑完**只把 phone 记库**（`signin_one.rs:70-76` → `store.FillAccountCredentials`，`core/traits.go:719`），失败只警告 |

### 1.2 设备类（决定"以哪台设备"）

| flag | 定义 | 语义 / 分支 | 落点 |
| --- | --- | --- | --- |
| `--db` | `args.go:89` | SQLite 路径 | `openLocal`（`run_user.go:18`）。别忘 `defer store.Close()` |
| `--app-device-file` | `args.go:90-91` | App 端 `deviceId` 存放位置。**必须与登录时同一个**，漂移的表现是"凭据都在、却回 90001"（`signin_one.rs:33-34`） | `app.LoadOrCreateAppDeviceID`（`device.go:140`）。**整次运行只解析一次**（`signin_run.rs:46-51`） |
| `--device-short` | `args.go:92-93` | 固定设备串第 7 段。给了就 `ToUpper`；不给则现生成（`signin_run.rs:52-55`） | `app.NewAppDeviceShort`（`device.go:130`）。同样**整次运行一次** |
| `--device-id` | `args.go:94-96` | H5 那个 `deviceId`（`B` + 88 字符 base64）。显式给的优先；否则从库里取、没有就现上报一次（`smid_cmd.rs:108-120`） | `app.DeviceID`（`smid.go:197`）。⚠ 解析失败**不致命**：降级成"不带"+ stderr 警告（`signin_one.rs:104-108`）。⚠ `--status-only` / `--raw` / `--records` **跳过解析**（`signin_one.rs:89-90`，理由：查状态不该顺手打一次第三方风控） |
| `--h5-device-id` | `args.go:100` | `--device-id` 的别名（Rust `visible_alias`），绑**同一个变量**，两者等价、后者覆盖前者 | 同上。**不是**第 21 个独立语义 |

### 1.3 只读查询类

| flag | 定义 | 语义 / 分支 | 落点 |
| --- | --- | --- | --- |
| `--status-only` | `args.go:112` | 只查状态不签到（`signin_one.rs:254-272`）。⚠ **会落库余额**，且记的是 **`bean_total`（上游 `total`）**，不是 `beforeTotal`（`signin_one.rs:260-263`） | `app.SignInStatus`（`signin_receive.go:20`）+ `app.RecordCloud`（`cloud.go:67`） |
| `--raw` | `args.go:113-114` | `infoV3` 原始信封，**不解析、不落库**（`signin_one.rs:234-252`）。不落库是有意的：原始响应没被解析成余额，记一条"余额未知"只会往趋势图塞噪声 | `app.SignInStatusRaw`（`signin_receive.go:39`） |
| `--records` | `args.go:115-116` | 豆的收支流水，**两个账本各一页**（`signin_one.rs:143-203`）。账本顺序固定：`RecordBookCloud`（改名前的云朵）在前、`RecordBookBean`（改名后的 AI 豆）在后 | `app.CloudRecords`（`signin_receive.go:77`），每账本一次。常量 `RecordBookCloud/Bean`（`upstream/app_signin_records.go:31,33`）、`RecordFilterAll`（同文件 `:39`） |
| `--records-all` | `args.go:117-118` | 只改**分页循环**：一页页翻到底。**单独给是用法错误退 2**（`args.go:167-169` + `validate`，`args.go:189-195`），**不是**运行错误 | 编排层的分页循环（见 §2.3）。⚠ Rust 把页大小常量放在**输出文件**里（`signin_output.rs:49,54`），见 §6.0 第 9 条 |
| `--prizes` | `args.go:119-120` | 待领取奖品，**只取一页**（`signin_one.rs:205-232`）。只读、不落库 | `app.PendingPrizes`（`signin_receive.go:105`）。页大小 `PRIZE_PAGE_SIZE = 10`（`signin_output.rs:102`）。常量 `PrizeMarketName` / `PrizeStatusPending`（`upstream/app_signin_records.go:137,146`）由 app 层内部使用 |

### 1.4 写请求类（全部"失败不抛，进 extras"）

⚠ 这一族在 Rust 里**一律不用 `?` 抛出**：用途恰恰是"看上游到底回什么码"，
抛出去就只剩一句错误信息（`signin_one.rs:303-305, 388-390, 449-451, 483-485`）。
Go 的 app 层已把这套约定做进返回值（`MultipleOutcomeFailed` / `BackupGiftOutcomeFailed` /
`ReceiveOutcomeFailed` 都**不返回 error**，见 `signin_bonus.go:96-100, 215-217`、`signin_receive.go:354-357`）。
**编排层照抄**：这几个开关失败时**不要**让整个账号失败。

| flag | 定义 | 分支（关键） | 落点 |
| --- | --- | --- | --- |
| `--receive` | `args.go:101-102` | **两条子路**，由 `--cloud-type` 是否**显式给出**决定：给了 → 显式模式（只发这一笔，自检用）；没给 → 自动分流（日常） | 显式 → `app.ReceiveCloud`（`signin_receive.go:164`）；自动 → `app.ReceiveAll`（`signin_receive.go:377`）。分流纯函数 `app.PlanReceive`（`signin_receive.go:284`）。互斥 `--status-only`/`--raw`/`--records`/`--prizes` |
| `--cloud-id` | `args.go:103-104` | **只在显式模式下有用**。⚠ 抓包里 `receiveList[]` **没有 `recordId`**，所以 H5 的 `e.recordId ?? ""` 实际是空串（抓包 §3.6.2；`upstream/app_signin_status.go:56-61`） | `app.ReceiveTarget.CloudID`（`signin_receive.go:135`） |
| `--cloud-type` | `args.go:105-107` | **显式模式的开关本身**。⚠⚠ `0` 是**合法取值**（"当前可领取"），所以**必须用 `a.given("cloud-type")` 判**（`args.go:62-64`），**不能**用 `a.CloudType != 0` | `app.ReceiveTarget.CloudType *int64`（`signin_receive.go:142`，注释明写"不要缺省成 0"） |
| `--revive` | `args.go:108-109` | ⚠⚠ **不可逆**，H5 原话「关闭弹窗视为放弃机会，无法补发」（抓包 §8.3）。**deviceID 只取显式 `--device-id`，故意不回落库**（`signin_one.rs:394-397`；理由：H5 定义里没有 `isDeviceId`，默认一个头都不带） | `app.ReceiveRevivalReward`（`signin_receive.go:194`），传 `*string`：未给 `--device-id` 时给 **nil**（不是 `&""`） |
| `--multiple` | `args.go:110-111` | 领签到翻倍，**每月一次**。门槛由 app 层自己判（`curMonthBackup && !curMonthBackupSignAccept`），不成立就**一个请求都不发** | `app.ClaimMultiple`（`signin_bonus.go:112`）；门槛纯函数 `app.PlanMultiple`（`signin_bonus.go:58`） |
| `--backup-gift` | `args.go:121-122` | 领备份好礼，**每月一次**。门槛 `curMonth` 前置、再看 `state` | `app.ClaimBackupGift`（`signin_bonus.go:223`）；门槛纯函数 `app.PlanBackupGift`（`signin_bonus.go:179`） |

### 1.5 行为开关

| flag | 定义 | 语义 / 分支 | 落点 |
| --- | --- | --- | --- |
| `--force` | `args.go:123-124` | 今天已签也发一次 `startSignIn`。**唯一用途是协议自检**（`signin.go:365-372`）。⚠ 日常别开：会让"今天已经签过"这条捷径失效，多打一次写请求 | `app.SignInOptions.Force`（`signin.go:361-373`） |
| `--json` | `args.go:125` | 输出形状（单账号对象 / 多账号数组）+ **进度行进 stderr**（`signin_run.rs:63-73`、`signin_one.rs:132-141`） | 渲染层。⚠ Go 侧的流约定见 §6.0 第 7 条 |

### 1.6 分支优先级（互斥表管不到的，靠顺序）

互斥表允许以下组合**同时出现**，Rust 靠 `signin_one` 里的 **if 顺序**决定谁生效
（`signin_one.rs:143 → 205 → 234 → 254 → 274`）：

```
--records  >  --prizes  >  --raw  >  --status-only  >  签到主路
```

即 `--records --status-only` 走 records；`--raw --status-only` 走 raw；
`--prizes --status-only` 走 prizes；`--force --status-only` 走 status-only（`--force` 被忽略）。
**编排层必须照抄这个顺序**，否则行为静默分叉。

写开关（`--receive` / `--revive` / `--multiple` / `--backup-gift`）排在**签到主路之后**，
且它们之间**可以共存**（例如 `--receive --multiple`，`args.go:130-132`）。

---

## 2. 请求时序

### 2.1 四段链路与 app 层函数对照

| 段 | 上游接口 | app 层 | upstream 层 | 抓包依据 |
| --- | --- | --- | --- | --- |
| ① | `POST /user/querySpecTokenV2`（原生 A 族头 + `Authorization`） | `app.OpenSignInSession` 第一段（`signin.go:289-293`） | `AppQuerySpecToken`（`app_signin_send.go:22`）；`ToSourceID = "001005"`（`app_signin.go:83`） | 抓包 §2.1.1 |
| ② | `GET /ycloud/api/timestamp` | 同函数第二段（`signin.go:297-302`） | `AppH5Timestamp`（`app_signin_send.go:45`）、`SignH5Request`（`app_signin_session.go:106`） | 抓包 §2.2 |
| ③ | `POST /ycloud/auth-service/auth/tyrzLogin` | 同函数第三段（`signin.go:304-309`） | `AppH5Login`（`app_signin_send.go:54`）；`SigninPageURL`（`app_signin_session.go:29`） | 抓包 §2.3 |
| ④ | `GET /ycloud/signin/page/infoV3?client=app` 与 `GET /ycloud/signin/page/startSignIn?client=app` | `CloudSignIn`（`signin.go:405-440`） | `AppSignInStatus`（`app_signin_send.go:69`）、`AppStartSignIn`（`:99`） | 抓包 §3.1 / §3.2 |

`app.OpenSignInSession`（`signin.go:280-310`）把 ①②③ 抽了出来，返回 `(referer, jwtToken)`。
**它现在只有一个调用方** —— `app.SignInSessionSource.buildLocked`（`signin_session.go`）：
每个子功能不再各自建会话，而是向**这一轮那一条**会话要票
（`SignInStatus` / `CloudRecords` / `ReceiveCloud` / `ReceiveAll` / `ClaimMultiple` /
`ClaimBackupGift` / `CloudSignIn` 一律收 `src *SignInSessionSource`）。
⇒ 上面那张"账单"里的 ①②③ 是**每轮一次**、不是**每个子功能一次**：
`--receive` 与签到主路同跑一趟，只有三枪建会话，业务请求仍各算各的。

三条口径与"不做的事"成文在 `internal/app/signin_session.go` 文件头；
中途撞 `90001` 时由 source **刷一次、重放那一条请求一次**（读写一视同仁），
仍失败就交回调用方按任务失败处理。

⚠ 上面这句"每轮一次"是**CLI 这一路**的口径：一次 `mcloud signin` 对每个账号只跑一轮，
票就活在这一轮里（`internal/cli/run_signin.go` 里刻意不接池，理由写在那处注释）。
服务端不是这样 —— 那里的作用域是**「账号 × 页面」跨轮复用**（界面上连点三次签到共用一张票，
不频繁取新票），由 `app.SessionPool` 持有；键为什么是"配方"而不是页面名、为什么不设 TTL，
见 `internal/app/signin_session_pool.go` 头注，使用口径见 `AGENTS.md` §1.12 的"作用域口径"。

⚠ 三个不可省的细节（`signin.go:43-51` 顶部三条判据）：
1. `x-yun-user-timestamp` 是**微秒**（`UserTimestamp`，`signin.go:51`）；写错量级的表现是 401。
2. `x-timestamp` 必须取自**服务端**（第 ② 段），不能用本地时钟。
3. `Authorization` **拼不出来就不要猜**（`loadCredential` 三道闸，`signin.go:163/193/209`）。

### 2.2 各子功能的请求账单

| 子功能 | 请求序列 | 次数 |
| --- | --- | --- |
| `--list` | 无网络（纯查库） | 0 |
| 签到主路（未签） | ① ② ③ → `infoV3` → `startSignIn` → `infoV3`（**回查**） | **6** |
| 签到主路（今天已签且未 `--force`） | ① ② ③ → `infoV3`（**不发** `startSignIn`，`signin.go:412-415`） | **4** |
| `--force`（今天已签） | ① ② ③ → `infoV3` → `startSignIn` → `infoV3` | 6 |
| `--status-only` | ① ② ③ → `infoV3`（+ 本地落库） | 4 |
| `--raw` | ① ② ③ → `infoV3`（原始，不落库） | 4 |
| `--records` | ① ② ③ → `cloudRecordV2` × 2 账本 × N 页 | 3 + 2N（N=1，或 `--records-all` 时 1..=20） |
| `--prizes` | ① ② ③ → `record` × 1 页 | 4 |
| `--receive`（自动） | ① ② ③ → `infoV3` → 每条动作一个请求（`receiveTaskExpansion` / `receiveV3`；`cloudType=2` 不发） | 4 + 动作数 |
| `--receive`（显式） | ① ② ③ → `receiveV3` | 4 |
| `--revive` | ① ② ③ → `receiveRevivalReward` | 4 |
| `--multiple` | ① ② ③ → `infoV3` →（门槛成立才）`multiple` | 4 或 5 |
| `--backup-gift` | ① ② ③ → `backupgift/info` →（门槛成立才）`backupgift/receive` | 4 或 5 |
| 签到 + `--receive` | **一轮一条会话**（① ② ③ 只打一次）→ 主路 `infoV3` → `startSignIn` → `infoV3` → `--receive` 的 `infoV3` → 每条动作一个请求 | 未签 **7 + 动作数**；今天已签 **5 + 动作数**（旧口径"会话建两次 / 8+"自 2026-09-23 起作废，见上面 §2.1 那段） |

### 2.3 `--records` 分页循环（编排层职责）

Rust 实现在 `signin_one.rs:143-203`，**注意它不在输出文件里**。逐字规则：

```
collected = []; total = nil
for page_number in 1..=RECORD_MAX_PAGES (20):
    page = cloud_records(aiDou=book, filter=0, page_number, page_size=50)
    got = len(page.records)
    total = page.total OR total          # ⚠ 保留第一个非 nil 的 total
    collected += page.records
    if !records_all || got == 0: break    # ⚠ 三个退出条件
    if total != nil && len(collected) >= total: break
```

- `RECORD_PAGE_SIZE = 50`（`signin_output.rs:49`）、`RECORD_MAX_PAGES = 20`（`:54`）——
  ⚠ **Go 侧目前不存在这两个常量**（`internal/` 全域零命中），归属见 §6.0 第 9 条。
- 三个退出条件**缺一不可**：`total` 缺失时唯一的终止依据就是"这一页是空的"。
- `total` 用 `OR`（保留先到的）而不是覆盖（`signin_one.rs:174`）。
- 条目**原样**交给渲染层：Go 的 `upstream.CloudRecords.Records` 是 `[]any`
  （`app_signin_records.go:79-88`），**不做字段解析**（理由：`records[]` 字段名**没有实拍**，
  见 `app_signin_records.go:71-78`）。
- `PRIZE_PAGE_SIZE = 10`（`signin_output.rs:102`）—— 同样不存在于 Go 侧。

---

## 3. 多账号选择规则

实现在 `signin_run.rs:172-225`（`select_signin_targets`）。三条路，优先级从高到低：

| 条件 | targets | `index` | `phoneOverride` |
| --- | --- | --- | --- |
| 给了 `--phone` | 恰好 1 个（**不查库**） | **0**（无意义） | 非空 → 强制现拼 |
| 给了 `--account N` | 恰好 1 个 | `N` | 空 |
| 都不给 | 全部 `enabled == true` | **在完整列表里的位置 + 1** | 空 |

⚠ 三条关键约束：

1. **下标是"完整列表里的位置"，含停用账号**（`signin_run.rs:189-212`）。
   理由逐字在 `signin_run.rs:170-171`：一旦下标随启用状态变动，同一个数字在不同时刻
   会指到不同账号上。⇒ `--account N` 取的是 `accounts[N-1]`，**不先过滤 enabled**。
2. **`--phone` 那条路不查库**（`signin_run.rs:179-185`）：它靠 `core.AccountLabel(nil, phone)`
   推 id，与登录落库**同源**，所以"登录时存进去的"与"签到时要读的"必然是同一行。
3. **空 targets 不是错误**（`signin_run.rs:39-44`）：打印
   「库里还没有账号。先登录一个：\`mcloud login sms --client app\`」然后退 **0**。
   ⚠ 该消息在"有账号但全停用"时**也会**打出来（targets 同样为空）—— 语义不准，
   见 §7 不确定 #8。

## 4. 失败隔离与汇总

| 规则 | 依据 |
| --- | --- |
| **一个账号失败不影响其余**：逐个跑，失败记 stderr，继续下一个 | `signin_run.rs:74-103`（Rust 注释 `:25-28` 说明"旧项目对每个账号各自 try/catch"） |
| 多账号（`len(targets) > 1`）时**每个账号前**打一行 `------ 账号 {index} ({account_id}) ------`；`--json` 时走 stderr | `signin_run.rs:61-73` |
| 失败计数 `failed`；`failed > 0` → 最后**抛运行错误**，消息 `{failed} 个账号失败（本次共 {targets.len()} 个）` | `signin_run.rs:117-121` |
| `--json` 下失败账号也 push 一个对象：`{"account","index","ok":false,"error"}` | `signin_run.rs:93-100` |
| `--json` 输出形状：`len(results) == 1` → 对象，否则数组 | `signin_run.rs:105-115`（见 §7 不确定 #2） |
| 人看的模式：每个账号的输出由渲染层负责；编排层只负责分隔线与汇总 | `signin_one.rs:528-557` |

---

## 5. 四态在编排层的落点

四态定义在 `internal/app/signin.go:320-352`。**编排层只读不填** ——
填它们的是 `CloudSignIn`（`signin.go:394-438`）那一次调用。

| 态 | 由哪一步填 | 什么情况下为假 |
| --- | --- | --- |
| `SignedToday()`（`signin.go:321-324`） | 第 ④ 段**回查**的 `infoV3`（`After`）。走"今天已签"捷径时 `After == Before`（`signin.go:414`） | ⚠ **成功返回的 report 里它恒为真**：两条路径都要求 `After` 已签（捷径那条 `After = Before` 且 `Before` 已签；另一条在 `signin.go:431-435` 强制校验）。它为假只出现在**错误路径**（回查缺失/为 false → `core.NewInternal`） |
| `WasAlreadySigned()`（`signin.go:331-334`） | 第 ④ 段**第一次** `infoV3`（`Before`） | `Before` 里没有 `t:true` 的格子（`ok == false`），或该格 `s == false`。⚠ 与 `SignedNow` **互斥**，但日志里长得像 —— 必须分开报 |
| `SignedNow()`（`signin.go:341-343`） | `Result != nil`（**发了** `startSignIn`）**且** `!WasAlreadySigned()` **且** `SignedToday()` | 三种：① 走捷径（`Result == nil`）；② `--force` 在已签状态下重发（`Result != nil` 但 `WasAlreadySigned`）；③ 未签成功（真） |
| `SignInPoints()`（`signin.go:346-352`） | `Result.SignInPoints`（`startSignIn` 里的 `signInPoints`） | `Result == nil` → **返回 nil**（不是 0）。走捷径时必然为 nil |

⚠ 三条踩过的坑（`signin.go:17-24`，逐条对应实现落点）：

1. **成败一律回查 `infoV3`**，不看 `startSignIn` 的 `todaySignIn`。
   `todaySignIn` 是**调用前**的快照：实测 `todaySignIn:false` 的那次恰恰把今天签上了
   （抓包 §3.2、§6）。Go 侧由 `SignInResult.TodaySignIn` 的注释钉住
   （`app_signin_signin.go:83-91`），而 `CloudSignIn` 在 `signin.go:426-435` 明确
   **不使用它**。⇒ **编排层也不许读 `report.Result.TodaySignIn`**。
2. **`x-timestamp` 必须取自服务端**（第 ② 段），不能用本地时钟 —— 本地偏一点就是 401，
   而 401 在日志里和"盐抄错了"长得一模一样。
3. **`Authorization` 拼不出来就不要猜**（`signin.go:79-80`）：不许拿掩码 id 去凑
   （`signin.go:219-225` 的报错文案里逐字写着「⚠ 不要拿掩码 id 去凑」）。

另一条与四态相邻的约定：**`MonthDays()`**（`signin.go:355`）取自 `After.MonthDays`，
与 `CountedMonthDays()`（`app_signin_status.go:215`，自己数 `cal`）是**两条独立路径**算同一个数，
两者不一致就是上游数据有问题 —— 渲染层要把两个都摆出来。

---


> ⚠ §3 / §4 这两条在 [`cli-usage.md`](./cli-usage.md)「签到 —— 七个要记住的」里
> 有同一件事的**操作口径**说法，两边实测一致（2026-09-22 逐条对过代码：
> `internal/cli/run_signin.go:224-283` 选择顺序、`:156-162` 与 `:1084-1088` 失败隔离与汇总）。
> 那边是"怎么敲"，这里是"规则本身怎么定"。

## 6 / §7 / §8 —— **已移出**

> 逐字搬至 [`archive/signin-cli-spec-history.md`](./archive/signin-cli-spec-history.md)
> （§6 Rust 三块职责划分、§7 不确定清单、§8 实现落点清单）。拆分理由见该文件抬头。
