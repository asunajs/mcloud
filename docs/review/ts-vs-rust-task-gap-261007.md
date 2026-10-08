# TS 参考实现（`mcloud-sign`）× 本仓 —— 任务「能跑 / 不能跑」实测与缺口清单

> 下达时刻：2026-10-07 ≈10:39（用户：「运行一下看看哪些任务能跑，哪些不能，全部记录下来，
> 并在本项目完全实现，然后使用"移动"进行验证」）
>
> 本文只回答两件事：
> ① **`mcloud-sign` 那条 CLI 今天（2026-10-07）真跑起来，哪些任务能跑、哪些不能**（含原始报错）；
> ② **本仓（Rust）相对它的缺口清单**，以及每条缺口的处置（实现 / 保持不做的口径 + 依据）。
>
> ⚠ 本表的读数**全部是跑出来的**，不是读代码推的。复现命令见 §1。
> 与 `docs/business-matrix.md` 的关系：那份是**长期状态表**，本文是**这一次实测的快照**，
> 实施完成后要回填进那份表。

## 0. 一句话结论

TS 侧 **16 条**任务今天跑得动、**6 条**跑不动（其中 2 条是活动已下线、1 条会让整个账号
后续任务全部中断）。本仓的缺口集中在 **`appTask` 的 11 个专用执行器** 与
**click-only 那 8 条**；`hc1t` / `ai-avatar` 这两条 TS 侧自己也已经跑不动了。

## 1. 复现方式（可自己跑）

```bash
# ① TS 主流程（默认配置，1 个真实账号）
cd /Users/catlair/Documents/workspace/mcloud-sign
NO_COLOR=1 node_modules/.bin/tsx packages/cli/src/index.ts | tee /tmp/mcloud-ts-run/run-1.log
#   ⇒ exit 0，101 行，耗时 26 秒（2026-10-07 10:40:30 → 10:40:56）

# ② 单条探针（只跑一条，用来回答"这条今天还能不能跑"）
#    探针脚本在 /tmp（不进 mcloud-sign 仓库），借它的 node_modules
cd /tmp/mcloud-ts-run/probe-cfg
NO_COLOR=1 PROBE_CLEANUP=1 /Users/catlair/Documents/workspace/mcloud-sign/node_modules/.bin/tsx \
  /tmp/mcloud-ts-run/probe.ts <hc1t|ai-avatar|live-room|upload-ten|mail-session|task-585|task-1028|...>
```

⚠ 探针用的是**同一个真实账号**（`asign.json` 里那份 auth，登录有效期到 2026-10-28）。
凡"写"的动作（上传 / 发邮件 / 转换 / 抽奖）都在探针里带 `PROBE_CLEANUP=1`，
跑完立刻走 `afterTask` 把自家传的文件移进回收站。

## 2. TS 主流程逐条读数（2026-10-07 10:40，`/tmp/mcloud-ts-run/run-1.log`）

| # | TS 调用 | 结果 | 原始那一行 |
|---|---|---|---|
| 1 | `signIn` | ✅ 能跑（今天已签） | `网盘今日已签到获得3云朵` |
| 2 | `revivalRewardTask` | ✅ 能跑（本月已领） | `本月复活奖励已领取` |
| 3 | `taskExpansionTask` | ✅ 能跑 | `下月可领取128个云朵` |
| 4 | `playAiSpecialTask`（夏日拍拍） | ◐ 能跑，但**今天无事可做** | `跳过无法自动化的任务: 生成我的校园海报(33)` / `体验AI相机(34)` / `无抽奖机会` |
| 5 | `playAiLotteryTask` | ◐ 能跑，今天次数用完 | `抽奖次数已用完，跳过` |
| 6 | `cloudPhoneRedpackTask` | ✅ 能跑 | `余额: 可用3元，累计4.6元` → `已完成: 每日签到` → `去体验AI灵犀 +0.30元` |
| 7 | `msgPushOnTask` | ✅ 能跑（已开） | `已经开启 27 天` |
| 8 | `noticeTask` | ✅ 能跑（已开） | `短信通知已开启 13 天` / `邮箱通知已开启 31 天，上报通知状态` |
| 9 | `backupGiftTask` | ✅ 能跑（本月已领） | `本月已领取` |
| 10 | `appTask` | ◐ 见 §3 | `获取到 45 个任务，去重后 28 个` |
| 11 | `printPendingPrizes` | ✅ 能跑 | `待领取奖品 1 个：移动云盘100万tokens叠加包 (ID:260630012)` |
| 12 | `receiveTask` | ✅ 能跑 | `今日获得12次云朵数量共：235` |
| 13 | `afterTask` | ✅ 能跑 | `删除文件成功`（6 个临时文件） |
| — | `liveRoomKoulingTask` | ⏸ 未调度（配置默认关） | 单条探针见 §4 |
| — | `aiAvatarTask` | ⏸ 未调度（配置默认关） | 单条探针见 §4 |
| — | `hc1tTask` | ⏸ 未调度（配置默认关） | 单条探针见 §4 |
| — | `exchangeTask` / `exchange` | ⏸ 按配置，默认不跑 | 只读的 `printExchangeList` 可读，见 §4 |

**能跑 12 条 / 今天无事可做 3 条**。整趟 **exit 0**，没有任何异常逃逸。

## 3. `appTask` 逐条读数（同一趟）

TS 的 `appTask` 形状是「runner 表 + `default:` 打卡 + 8 条 click-only」。

| id | 名称 | 结果 | 读数 / 报错 |
|---|---|---|---|
| 434 | 分享文件 | ⏭ 跳过 | `没有可分享的文件，跳过分享任务`（`ctx.state.files` 为空） |
| 106 | 上传文件 | ✅ 成功 | `上传文件成功` |
| 522 | 当月上传文件满100个 | ❌ 失败 | 5 次 `上传文件成功` 之后仍判 `失败`（与 `TASKS.md` 2026-09-12 的观测同形） |
| 409 | 从固定入口访问云朵中心 | ✅ 成功 | `成功 从固定入口访问云朵中心  409`（该条 `state` 已是 `FINISH`） |
| 1005 | 从固定入口进入云朵中心 | ❌ 失败 | `失败 从固定入口进入云朵中心 1005`（`state` = `WAIT`、`currstep` = 1） |
| 1004 | 给好友发邮件 | ✅ 成功 | `邮件发送成功 to=15909397542@139.com` |
| 1014 | 体验"PDF转换"功能 | ✅ 成功 | `PDF转换完成` |
| 1020 | 完成一次邮件转存 | ❌ 失败 | `邮箱备份请求失败 [POST] https://mail.10086.cn/mailsms/sms/backup?sid=…: 404 Not Found` |
| 107 | 创建云笔记 | ❌ 失败（单条探针） | `获取笔记鉴权失败，跳过云笔记任务` |
| 472 / 473 | 体验 139 邮箱 | ❌ 无可见效果（单条探针） | 见 §4：`taskClickV2` 整个端点 **404** |
| 1028 | 完成 AI 工作台 | ✅ 成功（单条探针） | `chat result: events=116 sessionId=1310506255813068364 text="你好，我是灵犀。…"` |
| 585 | AI 相机 | ✅ 成功（单条探针） | `AI相机任务完成` |
| 587 族 | 上传 10 张图片 | ✅ 成功（单条探针） | `10图上传完成: 10张`，随后 `afterTask` 清掉 10 个文件 |
| 115/431/435/1009/1010/1013/1016/447 | click-only | ❔ 无独立日志 | TS 走 `default:` 分支发 `task/click`，**不发卡就没有读数**（这正是它和本仓的差别） |
| 481 / 478 | 随机云朵任务 | ❔ 今天不在列表里 | `appTask` 收尾阶段对 481(`currstep==2`) / 478(`state!=FINISH`) 补发 `clickTask(…, "randomCloudTask")` |

## 4. 单条探针读数（把开关打开 / 直接调 runner）

| 任务 | 结果 | 原始报错 |
|---|---|---|
| `aiAvatarTask`（AI新头像） | ❌ **不能跑，且会中断该账号后续任务** | `POST https://m.mcloud.139.com/market/rabactive/ai/image/merge: 405 Not Allowed`；`GET …/image/getHashLeft: 404`；`GET …/image/draw: 404` ⇒ 第三个 404 是**未捕获异常**，在 CLI 主流程里会把该账号 `try` 块里剩下的任务全部跳过 |
| `hc1tTask`（云朵大作战） | ❌ **不能跑（活动已下线）** | `GET https://caiyun.feixin.10086.cn/market/signin/hecheng1T/info: 404` |
| `liveRoomKoulingTask`（直播口令） | ⏸ **闸门正常，今天没开播** | `当前无 中国移动云盘 主播的直播中直播间` ⇒ `直播未开播，跳过口令抓取与小红花领取` |
| `mail139` session | ✅ 能拿到 | `{"sid":"00c5…","rmkey":"2f0a…"}` |
| `taskClickV2`（472/473/1004 的"推进市场步"） | ❌ **端点整体 404** | 两个 `marketname`（`sign_in_3` / `newsign_139mail`）都是 `404` |
| `upload10PictureTask`（587 族） | ✅ 成功 | `开始10图上传: 当前0/10，还需10张` → `10图上传完成: 10张` |
| `printExchangeList` | ✅ 只读可读 | 19 条奖品（含"今日已兑完"档） |

## 5. 本仓缺口清单（要"完全实现"的部分）

图例：✅ 本仓已有 ／ ⛔ 缺口（本次要补） ／ ⏭ 有口径不做（附依据）。

### 5.1 `appTask` 执行器

| id | TS 行为 | 本仓现状 | 处置 |
|---|---|---|---|
| 106 / 113 / 522 | 上传（113 走 PC 渠道头） | ✅ | — |
| 409 / 1005 | 埋点 `page/doTask` + 补一发 `task/click` | ✅（`Step::Entry`） | — |
| 434 | 建外链 → 删外链 | ✅（`Step::Share`） | — |
| `beiyong1` 名字门 / 按钮门 | 传 1 张 / 补满 10 张 | ✅ | — |
| **107** | 笔记鉴权 → 建笔记 → 删笔记 | ⛔ | 实现（上游鉴权今天已失效，**如实报**） |
| **472 / 473** | mail139 session → `autologin` → `taskClickV2` | ⛔ | 实现（`taskClickV2` 已 404，**如实报**） |
| **1004** | session → `getComposeId` → `composeMail` → PE 上报 → `taskClickV2` | ⛔ | 实现（今天真能发出去 ✓） |
| **1014** | 传一个 PDF → `disk:convertPdfFromYunFile` | ⛔ | 实现（今天真能转换 ✓） |
| **1020** | `POST /mailsms/sms/backup` | ⛔ | 实现（上游 404，**如实报**） |
| **1028** | `queryUserToken` → DES 解手机号 → `cloudTaskStatus` → SSE 对话 → 复查 | ⛔ | 实现（今天真能对话 ✓） |
| **585** | `aiRecognize` → SSE 对话 | ⛔ | 实现（今天真能完成 ✓） |
| **587 族** | `beiyong1` + `button.app.link` 含 `uploadView` ⇒ 补满 10 张 | ◐ 形状已有（`UPLOAD_TEN`），但 `button` 那一格**本仓没解析** | 补 `button_app_link` 解析 + 接上 |
| **115/431/435/1009/1010/1013/1016/447** | 框架自动 `task/click` | ⛔ 完全不发 | 实现（同一串黑名单照抄） |
| **481 / 478** | 收尾补发 `clickTask(…, "randomCloudTask")` | ⛔ | 实现（`click_key` 里那两格**早就留好了**） |
| 117/437/991/406/385/480/1022/1018/603/1021/120 | 刻意跳过 | ✅（逐字照抄） | — |

### 5.2 独立任务

| TS | 本仓现状 | 处置 |
|---|---|---|
| `hc1tTask` | ⛔ 无任务，且三个 `hc1t_*` 旋钮已删 | ⏭ **保持不做**：本次实测 `404`，活动确已下线 ⇒ 与既有口径 D3「已过期，不实现」一致；**不写死代码** |
| `aiAvatarTask` | ✅ 已有（`ai_avatar.rs`） | — ⚠ 但 TS 侧已跑不动（405/404），本仓的对应接口**未被本次实测覆盖** |
| `playAiSpecialTask` | ◐ 本仓**只盘点不代做** | ⏭ 见 §6 的口径说明（今天 TS 侧的实际产出也是"无事可做"） |
| `afterTask` | ⛔ 无独立任务（等价动作内联在 `app_task::cleanup`） | ⏭ 有口径不做（差异表 D1） |
| `auth-service`（邮箱 session） | ⛔ | **本次实现**（1004/1014/1020/1028 都要它） |
| `drawRedFlower` / `autoDrawRedFlower` | ✅ 已并入 `live-room` 的「只用手工口令」 | — |
| 推送 10 条渠道 | ✅ 9 条真发，email 如实报未实现 | — |

## 6. 两条**保持不做**的口径（本次实测给它们补上了证据）

1. **`hc1t`（云朵大作战）**：`GET /market/signin/hecheng1T/info` → **404**。
   活动已下线 ⇒ 实现它等于写一段永远跑不通的代码。维持 `docs/task-inventory.md` 差异表 D3。
2. **`play-ai-special` 的"代做"那一半**：TS 今天在同一账号上的实际动作是
   `跳过无法自动化的任务: 33 / 34` + `无抽奖机会` ⇒ **产出为零**，与本仓"只盘点"的可见结果相同。
   ⇒ 本仓维持只读盘点；**这不是缺口，是今天两边结果等价的两种形状**。
   （真要对齐，需要接 `task/step/click` / `blindbox/lottery` / `aiRecognize` 三个零实拍的口子，
   见 `crates/mcloud-tasks/src/play_ai.rs` 文件头那三条理由。）

## 7. 本次实施（落点）

### 7.1 新增协议层（`crates/mcloud-upstream/src/`）

| 文件 | 内容 | 依据 |
|---|---|---|
| `mail139.rs` | 邮箱登录（`inlogin`）、`autologin`、市场打卡 `taskClickV2`、`getComposeId` / `compose`、PE 活跃上报（gzip + MD5 鉴权）、`disk:convertPdfFromYunFile`、`mailsms/sms/backup`、`disk:queryUserToken` | 旧 TS `apis/auth.ts:126-144`、`apis/mail139.ts`、`tasks/auth-service.ts` |
| `ai_cloud.rs` | 585 的 `aiRecognize` + `chat/v2/add`（取最后一条 `data:` 行）、1028 的 `queryUserToken` → DES → Basic、`getUserTaskAndCloudStatusExternal`、SSE 流解析 | 旧 TS `tasks/ai-tasks.ts:56-188`、`apis/ai-workbench.ts`、`tasks/app/runners.ts:345-448` |
| `note.rs` | 笔记鉴权 `authTokenRefresh.do`、`createNote.do`、`moveToRecycleBin.do` | 旧 TS `apis/note.ts` |

### 7.2 任务层（`crates/mcloud-tasks/src/app_task/`）

| 落点 | 内容 |
|---|---|
| `app_task.rs` 的 `Step` | 新增 `Step::Runner(AppRunner)`（107/472/473/1004/1014/1020/1028/585）与 `Step::ClickOnly`（115/431/435/1009/1010/1013/1016/447） |
| `app_task.rs` 的 `step_for` | 三条新分支（执行器表 → click-only 表 → 形状表） |
| `app_task.rs` 的 `settle_random_cloud_clicks` | 收尾那两条特例打卡（481 `currstep==2` / 478 `state!=FINISH`，key = `randomCloudTask`） |
| `app_task.rs` 的 `upload_bytes` | 从 `upload_one_file` 抽出"给定名字与内容的 create/PUT/complete"（1014 要传一份确定的 PDF） |
| `app_task/runners.rs` | 七个执行器的编排 + 邮箱 session 一次运行内复用 |
| `mcloud-core` 的 `format_with_offset` / `beijing_format` / `utc_format` | 时间格式化收口（`chrono` 只有 `mcloud-core` 引得到） |

⚠ **与旧 TS 的两处刻意差异**（都写在代码注释里）：
1. 585 的 `userDomainId` 取**账号上落着的那一份**（登录响应里的 `userInfo.userDomainId`），
   而不是现解 `jwttoken` —— 任务层拿不到那张票，而两者是同一个值；
2. 收尾那两条特例打卡**不无条件**重拉整表（只在第一趟的表里真出现过 478/481 时才拉），
   这是本仓 2026-10-07 那条"已做满的档不必再拉表"优化的延续。

### 7.3 **没做**的两条（附本次实测证据）

1. **`hc1t`（云朵大作战）** —— 实测 `GET /market/signin/hecheng1T/info` **404**，
   活动已下线 ⇒ 维持差异表 D3「已过期，不实现」。**写死代码是负收益**。
2. **`play-ai-special` 的"代做"那一半** —— TS 今天在同一账号上的实际动作是
   "跳过 33 / 34 + 无抽奖机会"（产出为零），与本仓"只盘点"的可见结果相同；
   要真对齐得接三个**零实拍**的写口（`task/step/click` / `blindbox/lottery` /
   `aiRecognize`），见 `crates/mcloud-tasks/src/play_ai.rs` 文件头那三条理由。

## 8. 真机验证读数（Rust 侧，2026-10-07）

### 8.1 怎么跑的（可自己复现）

```bash
# ① 用**独立 target 目录**编二进制（不碰 launchd 托管的那份 ⇒ 0 次 TCC 弹框 ✓）
export CARGO_HOME="$PWD/.cargo-home"; export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$PWD/target/lines/lead"
cargo build -p mcloud-server --bin mcloud

# ② 用一个 **data 的副本**（不污染线上那份 history.json ✓）
cp -R data /tmp/mcloud-verify/data

# ③ 在「移动」账号（b73bb07d5971e8e7）上只跑这次新接的那几条
MCLOUD_DATA_DIR=/tmp/mcloud-verify/data ./target/lines/lead/debug/mcloud run app-task \
  b73bb07d5971e8e7 --body '{"onlyTaskIds":"431,618,1004,1014,1020,1057"}'
```

⚠ 用的是**真实账号**（`移动` = 17823734979），不是夹具。跑完 `cleanup` 把自家传上去的
1 个文件移进了回收站 ✓（`uploadedFiles: 1 / cleanedUp: 1`）。

### 8.2 「移动」账号 —— run 158 逐条读数

| id | 名称 | 结论 | 原始那一行 |
|---|---|---|---|
| 431 | 去使用移动云手机 | ✅ 打卡成功（**click-only**，本次新接的那一档） | `clicked: true` |
| 618 | 体验AI相机 | ❌ 首次：`AI相机任务失败: 缺少用户信息（账号没记 userDomainId）` | 见 §8.3（**改完复跑成功了** ✓） |
| 1004 | 给好友发邮件 | ✅ **真发出去了** | `邮件发送成功 to=17823734979@139.com` |
| — | （同上，市场打卡） | ❌ 上游 404（与 TS 同） | `邮箱市场打卡（taskClickV2）失败，HTTP 404：{"code":404,"msg":"访问资源不存在"}` |
| 1014 | 体验"PDF转换"功能 | ✅ **真转换完成** | `PDF转换完成` |
| 1020 | 完成一次邮件转存 | ❌ 上游 404（**进 `failures`** ✓） | `邮箱备份（mailsms/sms/backup）失败，HTTP 404`（回的是登录页 HTML）|
| 1057 | 体验AI工作台 | ✅ **真对话成功** | `AI 对话成功（events=29，回复：你好！我是灵犀，有什么可以帮你的吗？…）` |

整趟：`planned=6 / acted=4 / uploaded=1 / cleaned=1 / failures=1`，结论 `partial` ✓
（"有成功也有失败 ⇒ 部分成功"，没有把 1020 那个 404 说成成功 ✓）。

### 8.3 618（AI 相机）那一格：**三次真机对拍**才做对

它是本次唯一一个"改了三轮"的地方，三处都记在这里（都是**跑出来的**，不是读代码推的）：

| 轮次 | 读数 | 原因 | 改法 |
|---|---|---|---|
| 1（run 159/162） | `{"success":false,"code":"01000002","message":"鉴权失败"}` | 本仓给的是**裸 token**；旧 TS 的实发头是**完整 auth**（`Basic …`）—— 它的 `ctx.http` 会按域名再注入一次 authorization（抓包抓到的就是那一串 ✓） | 改传 `account.auth.raw` |
| 2 | 同上 | 还差 `deviceid` 与 `accept: application/json` —— 旧 TS 的 http 层按 `TARGET_HOSTNAMES` 注入，本仓这几台主机不走那层注入 | 协议层显式带上（真机对拍：`raw + deviceid` ⇒ `success:true`，`raw` 单独 ⇒ `鉴权失败` ✓） |
| 3 | `AI相机任务失败: 缺少用户信息（账号没记 userDomainId）` | 「移动」是 **PC 端登录**的账号，没有 `user_domain_id` 那一格（`测试`/`小号` 有 ✓） | 加兜底：走 **1028 已经在用的** `disk:queryUserToken` → `userDomainId` ✓ |

**最终读数（run 166，同一个「移动」账号）**：

```
体验AI相机：AI相机任务完成
App 任务做了 1 条（判出 1 条）
```

⇒ 618 那条**真的走完了**（识图 + 对话），不是"请求发出去了"而已 ✓。

### 8.4 已知没验到的

* **`107` / `472` / `473`**：三个真机账号的任务表里**都没有这三个 id** ⇒ 真机上到不了。
  实现按旧 TS 照抄（单测钉住"拿不到 `app_auth` 就一枪不发" ✓），**但今天没有活体读数** ✗。
* **`585` / `1028` 这两个旧 id**：两张表里也没有（今天叫 `618` / `1057`）⇒
  活体验的是 `618` / `1057` ✓，旧 id 只由单测钉住 ✓。
* **`481` / `478`**：两个账号的表里都没有这两条 ⇒ 那两条收尾打卡**没有活体读数** ✗（单测覆盖）。
* **`618` 在「移动」上第二次跑不了**：第一次（run 158）的**打卡门**已经把它标成
  `FINISH` 了 ⇒ 之后只能靠 `currstep != 0` 那条路（run 166 正是这样跑通的 ✓）。

