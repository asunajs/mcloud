# ai-avatar（AI 新头像）上游规格

> 这份是**只读调研**的产物（2026-09-30）。依据优先级：`mcloud-sign` TS > 真机抓包 > 推断；
> 未标「推断 / 线索」的结论都逐条给了 TS 的 `文件:行号`。Go 参考只作线索，不作结构。
>
> ⚠ 它**不是**实现，也不代表结论已定：落地前先看「未定项」，尤其是最硬的那条 ——
> **活动疑似已下线**（Go 2026-09-25 实测 `getHashLeft` / `draw` 回 404、`merge` 回 405，两个号一字不差）。
> 本文件保留原样：有新实拍就往里加，**别把没验过的改写成验过的**。

# ai-avatar（AI 新头像 / AI头像）上游规格 — 给 Lead

> 依据优先级：**mcloud-sign TS 为准** > 真机抓包 > 推断。本仓 `docs/capture/` 目录**不存在**，
> 仓内也无任何 ai-avatar 实现（全仓 grep 只有 `account.rs` 与 `task_expansion.rs:7` 的注释）。
> Go 参考（同级的 `mcloud`，另一份 Go 实现）**只作线索**，凡引用处均标「线索」。
> 未标 [推断] 的结论都逐条给了 TS 的 文件:行号。

## 1. 端点清单

TS 全部落在 `packages/core/src/tasks/ai-avatar/api.ts`（8 口）。

| # | 方法 | host + path | 请求体 / query | 性质 |
|---|---|---|---|---|
| 1 | GET | `m.mcloud.139.com` + `/market/rabactive/image/getHashLeft` | 无体、无 query（api.ts:43-45） | **只读**（读剩余抽奖次数，result:number） |
| 2 | GET | `m.mcloud.139.com` + `/market/rabactive/image/draw` | 无体、无 query（api.ts:47-49） | **写 / 有副作用**：消耗抽奖次数并发奖 |
| 3 | POST | `m.mcloud.139.com` + `/market/rabactive/ai/image/merge` | `{imageBase64, imageExt:"jpg"}`（api.ts:51-56；"jpg" 写死 :54） | **有副作用**：建立一次合成任务 |
| 4 | POST | `m.mcloud.139.com` + `/market/rabactive/ai/image/getStatus` | `{taskId}`（api.ts:58-60） | **只读** |
| 5 | POST | `m.mcloud.139.com` + `/market/rabactive/image/upload` | `{originImage, mergeImage}`（均 base64，api.ts:62-64） | **有副作用**：往用户网盘写图片文件（服务端写） |
| 6 | GET | `m.mcloud.139.com` + `/market/rabactive/image/getWorkList?type=1` | query `type`，默认 1（api.ts:66-68） | **只读** |
| 7 | GET | `m.mcloud.139.com` + `/market/rabactive/image/catalogFullPath?marketName=rabbityearpic` | query `marketName`，默认 `rabbityearpic`（api.ts:70-74） | **只读** |
| 8 | POST | `personal-kd-njs.yun.139.com` + `/hcy/file/list` | `{parentFileId, pageInfo:{pageSize:100}, imageThumbnailStyleList:["Big","Small"], orderDirection:"DESC", orderBy:"updated_at"}`（api.ts:76-95） | **只读**（列网盘活动目录） |

基址常量：`MCLOUD_URL=https://m.mcloud.139.com`（api.ts:3）、`AI_IMAGE_URL=…/market/rabactive/ai/image`（api.ts:4）、
`IMAGE_URL=…/market/rabactive/image`（api.ts:5）、`FILE_LIST_URL=https://personal-kd-njs.yun.139.com/hcy/file/list`（api.ts:6）。

### 必须带的头

- **第 1–7 口** 走「主 http 实例」（`createHttp`，base.ts:34-154），静态头 base.ts:25-32：
  `user-agent`（Android WebView UA）、`content-type: application/json;charset=UTF-8`（**GET 也发**，静态头恒发）；
  `deviceid` 静态注入（base.ts:44-46）；cookie jar 回写（base.ts:70,100-109）。动态头按 **host** 决定（base.ts:76-95）：
  - `authorization`（Basic，账号侧）：host 以 `AUTH_DOMAINS` 结尾才发（base.ts:11,79-83）—— `mcloud.139.com` **命中**。
  - `jwttoken`：host ∈ `TARGET_HOSTNAMES` 才发（base.ts:13,86-90）—— `m.mcloud.139.com` **命中**。
  - ⚠ **不发** `activityid` / `appversion` / `origin` / `referer` / `showloading`：那是 `createTaskHttp`（base.ts:165-182）那套，本族不用。
    所以这一族**没有签名头**（`x-request-id`/`x-timestamp`/`x-nonce`/`x-signature` 一个都不发）；签名只在建会话那一刻用（见 §2）。
- **第 8 口** 同一静态头 + `authorization`（`yun.139.com` 命中）但 **不发 jwttoken**（`personal-kd-njs.yun.139.com` 不在 TARGET_HOSTNAMES）。
  另显式 4 枚**业务头**（api.ts:87-92）：`x-yun-api-version: v1`、`x-yun-app-channel: 10214200`、
  `x-yun-module-type: 100`、`x-yun-op-type: 1`。无签名头。
  [未定] `10214200` 属哪一端（仓库三档是 android 10000023 / web 10000034 / PC 10200153），TS 别处无定义。

## 2. 会话与凭据

这条链要**两个半**凭据，全部由账号初始化一次性换来并**全账号共享**：

1. **authorization（Basic，账号侧）**：`parseAuth(auth).auth`（init-account.ts:26）→ `createHttp({deviceid})`（:40）
   → `http.setAuth({type:"basic", token})`（:42）→ 按域名注入（base.ts:78-83）。第 1–8 口都带。
2. **jwttoken（H5 业务票，仅第 1–7 口）**：`getJwtToken(http, logger, toSourceId)`（init-account.ts:46）
   → `fetchJwtToken`（auth/index.ts:42-62）：
   - ① `getSsoToken`（auth/index.ts:16-37）→ `querySpecToken`（`POST user-njs.yun.139.com/user/querySpecTokenV2`，
     body `{toSourceId:"001005"}`，apis/auth.ts:54-60），**5 次重试、指数退避 1/2/4/8s**（auth/index.ts:21-33）。
   - ② `tyrzLogin` 旧接口（auth/api.ts:23-32）：`GET caiyun.feixin.10086.cn/portal/auth/v2/tyrzLogin.action?ssoToken=..&openAccount=0`，
     带 4 枚签名头 `getSignHeader(ts, saltKey=2, other="ssoToken=..&openAccount=0")`（auth/sign.ts:28-42；
     salt2=`sekaMdYYLIZfbCfm` sign.ts:5；算法 `md5(salt+id+ts+nonce+other+salt)` sign.ts:16）。
   - `http.setJwtToken`（init-account.ts:51）→ 注入（base.ts:86-90）。
3. **deviceID**：`getDeviceId()`（shared/src/smid/index.ts:370，init-account.ts:33），静态头发往所有请求。

- 复用会话抽象：整条链**共用账号级主 http 实例**，**没有为 rabactive 建专属会话**，也没有该活动页自己的取票参数 [线索：Go 也这么读]。
- 本仓对应物：`H5Client`（h5_client.rs:85）+ `SessionRecipe::SignInPage`（session.rs:75）；toSourceId `001005` 与 TS 同源。
- ⚠ **分歧**：TS 主实例用**旧** tyrzLogin（`caiyun.feixin.10086.cn/portal/auth/v2/tyrzLogin.action`，签名 GET）；
  本仓 session.rs:338 `tyrz_login_request` 实现的是**新** tyrzLogin（`m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin`，marketName/sourceId）。
  两条都可能通，但票不一定同源 —— 接线时选哪条 [未定]。
- ⚠ 本仓**没有 `sign.rs`**；所谓"签名头"就是 session.rs 的 `H5Signature` + `mcloud-crypto::generate_signature`，
  且本族实际只在换票那一步用，业务口不用。

## 3. 主流程

入口 `aiAvatarTask`（task.ts:164-169），开关 `config.AI新头像?.开启` 为假即 return（:165）。

1. **生成** `aiAvatarGenerateTask`（task.ts:78-107）
   - `count = max(0, floor(config.每日生成次数 ?? 5))`（:80-83）。默认 10 来自 config schema（schema.ts:29）；
     task.ts:7 的 `DEFAULT_GENERATE_COUNT=5` 只在整块配置缺失时兜底。
   - 每轮 `generateOnce`（task.ts:48-76）：
     造 200×200 随机噪点 PNG→base64（:38-40,49）→ `merge`（:50）；
     `code!==0` 或缺 `taskId` → 本轮失败（:52-55）；
     轮询 `getStatus` 最多 **8 次、间隔 3s**（:9-10,57-59）：`code!==0`→失败；`status===4`→失败；
     `status!==3`→继续等；`status===3`→取 `imageUrl`，**裸 `fetch` 下载**（:42-46，不带任何鉴权头）
     → `upload(originImage=本地 base64, mergeImage=下载图 base64)`（:70-72），`code===0` 时收 `result` 为 uploadId。
   - 每个 count 位最多 **2 次尝试、尝试间不等待**（:88-94）；失败**不致命**，继续下一轮；轮间 `sleep 2000ms`（:102）。
   - `uploadIds` 存入 `ctx.state.aiUploadIds`（:105）。
2. **抽奖** `aiAvatarDraw`（task.ts:109-137）
   - `getHashLeft`（:113）拿剩余次数；异常/非数 → 兜底 **5**（:8,111-119）；为 0 → 直接返回（:121-124）。
   - 循环 drawCount 次：`draw`（:127）；`code===408`→break（:128）；其它非 0→记日志 continue（:129-132）；
     `code===0`→取 `contentName||prizeName` 报奖（:133-134）；轮间 `sleep 1000ms`（:135）。
3. **收集临时文件** `collectTemporaryFiles`（task.ts:139-162）
   - uploadIds 空 → 不做（:140）。
   - 并行 `getCatalogFullPath` + `getWorkList`（:142-145）→ `parentFileId` = `fullPath_sub` 按 `/` 切后**最后一个非空段**（:146）。
   - `getFileList(parentFileId, pageSize=100)`（:149）。
   - `collectAiAvatarFileIds`（task.ts:18-36）：只看 `work.id ∈ 本次 uploadIds` 的 work，取其 `imgUrl`/`qrImgUrl` 文件名
     （`extractImageFileName` :12-16，只认 `.jpg/.jpeg/.png`），与 fileList 条目**按 name 匹配**，收集 fileId
     → 并入 `ctx.state.files`（去重，:157）。全链异常吞掉只 debug（:159-161）。
4. **清理（不在本族内）**：`afterTask`（after.ts:125-138）对 `ctx.state.files` 调 `deleteFiles`（utils/file.ts:113-125）
   → `batchTrash`（apis/file.ts:118-121，`POST personal-kd-njs.yun.139.com/hcy/recyclebin/batchTrash`，body `{fileIds}`）。
   整账号任务链尾统一执行（cli/src/index.ts:131 起排）。

## 4. 判定与幂等

- **每天次数上限**：生成侧**不看上游计数** —— `每日生成次数` 纯粹是**本地循环次数**；上游真实余量只在抽奖侧由
  `getHashLeft` 读。本仓 `ai_avatar_daily_limit`（`account.rs:161`，默认 10，与 TS schema 默认一致）语义应对齐
  「生成轮数」，**不是**「上游返回的余量」。
- **已完成 / 已领奖**：TS 里**没有**这类判定。能不能抽完全由 `getHashLeft` 决定（=0 跳过，>0 照抽）；
  没有"今天生成过就跳过"的标志位。
- **重复跑**：**会重复**。第二次进链照样 merge（≤count 次）、照样按 `getHashLeft` 抽。若 merge 计入额度，
  本地上限不防重（无 last-run 标记）⇒ 幂等性由上游计数兜底，不由本链兜底。
- **唯一去重**：`collectAiAvatarFileIds` 与 `state.files` 用 Set（task.ts:23,33-35,157），只防同一次运行内列重。
- `draw` 的 `408` 当"停止"（task.ts:128），语义未定（次数用完 vs 限流）[未定]。

## 5. 写侧与不可逆动作（供 Lead 定护栏）

| 动作 | 处 | 会不会改用户东西 | 可撤？ |
|---|---|---|---|
| `merge` | api.ts:51 | 建一次合成任务（疑扣当天额度 [未定]） | 否（无撤销口） |
| `upload` | api.ts:62 | **往用户网盘活动目录写图片**（服务端写 originImage/mergeImage） | 否（只能事后删） |
| `draw` | api.ts:47 | **消耗抽奖次数 + 发奖** | 否 |
| `batchTrash` | utils/file.ts:113 | **删除**，但是**移进回收站**（可捞，见 :111 注释） | 是（回收站） |

- **误删风险（重点）**：收集靠「**文件名匹配**」而非「本次返回的 fileId」。若活动目录里本就有与本次生成图**同名**的文件，
  会被一起送进回收站。TS 无 pre-existing 判据。本仓 `task_expansion.rs:16-18` 已有"撞同名（exist）就绝不删"的先例可援。
- 无本地磁盘写入（图片全程内存 base64）；无覆盖写、无本地删除；无账号级不可逆动作（不兑换、不注销）。
- `upload` 不是本仓 `app_cloud_upload` 那条本地三枪直传，而是把 base64 交给活动接口 —— 写侧的粒度落在**上游**。

## 6. 本仓可直接复用

- **凭据注入三件套落地处**：`crates/mcloud-upstream/src/client.rs` — `ClientProfile`(:14)、`ReqwestClient`(:98,:254)、
  `should_inject_credential`(:245)。authorization/jwttoken/deviceid 的"发不发、发哪根"全在这里。
- **主机判据**：`crates/mcloud-upstream/src/endpoints.rs` — `AUTH_DOMAINS`(:6)、`JWT_HOSTS`(:14)、
  `accepts_basic`(:107)、`accepts_jwt`(:114)、`PERSONAL_KD`(:95)、`MCLOUD_HOST`(:65)。
- **会话与签名**：`crates/mcloud-upstream/src/session.rs` — `SessionRecipe`(:71；`SignInPage` :75)、
  `spec_token_request`(:314)、`tyrz_login_request`(:338)、`h5_headers`(:376)、`H5Signature::with_ids`(:255,:267)、
  `basic_authorization`(:645)。⚠ `h5_headers` 那套（activityid/appversion/referer/showloading）**不该**套到 rabactive 上「TS 从不发它们」。
- **会话装饰器**：`crates/mcloud-upstream/src/h5_client.rs` — `H5Client`(:85)、`needs_session`(:34)、`recipe_for`(:61)。
  `m.mcloud.139.com` 会走建会话；`personal-kd-njs.yun.139.com` 是 passthrough（:259）—— 与 TS 行为一致。
- **网盘文件操作头**：`crates/mcloud-upstream/src/app_cloud_upload.rs` — `cloud_file_op_headers`(:250)、`sends_authorization_to`(:316)。
  ⚠ TS 的 `hcy/file/list` 只发 4 枚（无 `x-yun-client-info`，渠道号 10214200），与本函数的 android/web 档都不同 ⇒ **不能直接套**，
  要按 TS 字面另写（或加一档）。
- **清理**：`crates/mcloud-upstream/src/cloud_trash.rs`（batchTrash）+ `crates/mcloud-engine/src/artifacts.rs`
  （临时文件登记/清理，:84,:223,:389）—— 这是 TS `ctx.state.files` 在本仓的对应物。
- **设置位**：`crates/mcloud-core/src/account.rs:160-161`（`ai_avatar_enabled` 默认 false、`ai_avatar_daily_limit` 默认 10）。
- **状态**：本仓**没有** ai-avatar task 实现；`crates/mcloud-tasks/src/task_expansion.rs:7` 只是在注释里把它列为"写网盘的链"之一。

## 7. 未定项

1. **活动是否还在**：Go 2026-09-25 真跑时读余量与抽奖两口回 **HTTP 404**、合成回 **405**，两个账号一字不差
   [线索：Go `app_ai_avatar.go:17-22`]。若那是"活动下线"而非"拼错"，整条链今天跑不通。分辨仍需人工在活动站打开该页抓一次加载链。
2. **status 档位表**：3=成功 / 4=失败 / 其余继续等 —— **零实拍**，从 TS 分支形状反推。
3. **draw 的 408**：次数用完 vs 限流，未定；TS 只 break。
4. **merge / upload 是否扣「每日生成次数」**：未定（决定本地 count 与上游额度是否同一物）。
5. **uploadId 与 workList 的 work.id 是否同一物**：TS 直接用相等判（task.ts:26），无实拍 [推断：同物]。
6. **marketName=`rabbityearpic` 与 fullPath_sub → parentFileId 的取段法**：零实拍，未定。
7. **getWorkList 的 type=1** 含义：未定。
8. **hcy/file/list 为何只发 4 枚头、缺 x-yun-client-info**：未定（本仓 app_cloud_upload.rs:45-53 记"同主机两套画像都通、唯一承重的是 client-info 的值"，而 TS 这一口恰恰不发它）。
9. **沿用旧还是新 tyrzLogin**（TS 的 caiyun.feixin 签名 GET vs 本仓 session.rs:338）：未定。
10. **清理匹配策略**（按文件名 vs 按时窗/只删本次 id）：TS 按名；本仓要不要收紧未定。

## 附：与 Go 参考的差异（只作线索，不作结构）

- Go 把这一族拆成 `AiAvatarHeaders` + 每口一个 Request 构造函数；本仓按 **TS 字面**落码即可，**不照搬** Go 的类型/分层/表结构。
- Go 的 `hcy/file/list` 走 iOS profile（整族 `x-yun-*`），TS 只发 4 枚 ⇒ **以 TS 为准**。
- Go 那两枚 404/405 是真读数，可当"活动可能已下线"的线索；但**不拿它当门槛**卡 TS 的结论。
