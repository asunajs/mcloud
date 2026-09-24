# 待拍板清单（Agent 攒下来的、需要用户定的问题）

> 状态：**清单**（不是规范，也不含任何结论 —— 这里每一条都还是问句）
>
> 用法：这是按"先记录、跳过、后续统一处理"的口径攒出来的一张表。用户当场拍板的条目
> **就地改写成终判并从本表删掉**（结论进它该去的那份权威文档，别在这里留第二处），
> 而不是在下面叠一条"补记"。判据同 AGENTS.md §2.6。
>
> ⚠ 本表**不新增事实**：每一条都指向出处，出处那一节才是现场。这里只负责"一次看全"。

## 怎么读这三档

| 档 | 含义 | 不拍板的后果 |
| --- | --- | --- |
| **挡路** | 没有这一句，某条链就只能冻着 | 那条能力**不会**落地（不是"做得糙"，是没有） |
| **不挡路** | 已经按某个默认推进并且跑绿（夹具上） | 继续按默认走；拍板只改口径，不改结构 |
| **要样本** | 缺的是实拍，不是决定 | 要么授权去抓（= 允许真动账号），要么继续标 `未测试` |

---

## 一、挡路（六条）

| # | 问题 | 不拍板会怎样 | 现状 | 出处 |
| --- | --- | --- | --- | --- |
| P1 | **现在要不要抓一条真实上传**（往测试号网盘写一个小文件再删掉，补 `file/create` 文件型 + `PUT uploadUrl` + `file/complete` + `batchTrash` 四枪） | 上传族与 #7 `after` 继续接口层冻结：#8 任务拓展 / #9 的"凑备份"那一半**进不来**。<br>⚠ 订正（2026-09-24）：这一条原先把"#10 AI 头像"也列成进不来，那一句**不成立** —— 那条链的"写网盘"是**活动侧** `rabactive/image/upload` 完成的，不经过 `file/create` 那一族，所以它的**生成 + 找回**两半都已落地并接上执行。P1 现在管到它的那一格只剩**清理**：写进用户网盘的图今天**没人删**，而清理要的 `batchTrash` 零实拍（详见 R19）。账本 `ArtifactLedger` 仍是"已建好、没有调用点" | 三档里选①不抓 / ②只补只读 / ③补完整四枪 | [`status.md`](status.md)「上传族与 `after` 清理」第四节 |
| P2 | **兑换第一次真跑**要不要现在做（真扣豆、不可回滚；验收口径已定死为抓包 §5.4 那三条） | 这一族的"跑对了"目前**只**在夹具上成立；不真跑就永远不知道线上认不认我们这条 `exchangeV3` | 需要一件"便宜 + 不要短信码"的奖品，且有人盯着。<br>⚠ 订正（2026-09-24）：这一条原先读起来像"整族都是黑箱"，**只读那一半已经不是了** —— 本仓当天用两个真账号经新接口 `GET /api/accounts/{id}/exchange-prizes` 真打 `exchangeList` 成功（55 件 / 9 组）。所以建会话、换票、解析清单那几段已有实测；**P2 现在只管 `exchangeV3` 那一枪**（下单 + 滑块 + 余额对账） | [`status.md`](status.md)「抢兑 `exchange`」三·1、「兑换页的选品只读接口」四 |
| P3 | **`#18` 红包派对那条链真给云机装 APK**（旧实现还会"先记账后动作"）—— 迁不迁？迁的话装完删不删？ | 这一族的产出是**现金（分）**，但不装 APK 就没有任务完成 ⇒ 要么接受"真动云机"，要么这一族只做只读那一半 | **只读那一半已于 2026-09-24 落地并接上执行**（能力 `cloud-phone-redpack`、`internal/app/cloud_phone_run.go`、独立活动站票池、26 条测试）⇒ 现在界面上看得见余额与任务清单，但**一件任务都不代做**。所以这一条现在**只管写侧**：装机链上游层已按旧 TS 订正（`internal/upstream/app_cloud_phone*.go` 四份、21 个 `(u *Upstream)` 入口、**72 条测试**）**但没有调用点**；另注意 ⑤ `aiRecognize` 的 `uploadToCloud` 会**往网盘写文件** | [`business-feature-inventory.md`](business-feature-inventory.md) §2.4 `cloud-phone-redpack`、[`status.md`](status.md)「#18 只读那一半接上执行」 |
| P4 | **`#16` 拍拍这一期还活着吗**（`marketName = National_PlayAISpecial` 是夏日那一期，归档时间是 2026-06-29） | 不确认就真打 ⇒ 拿到一个非零码，而"接口下线"与"我这端点写错"**长得一模一样**（§5.9），排障会往错方向走 | **只读那一半已于 2026-09-24 接上执行**（能力 `play-ai-special`、`internal/app/play_ai_run.go`、复用签到页票池、14 条测试）⇒ 界面上已经点得到这一条，所以这一条从"要不要接"变成**"真打之前先确认入口"**：先在 App 里看一眼当期入口（**看**不算真打，不打请求）。③④⑤ 那三口的卡点各自见 P6 / P1 与 `play_ai_run.go` 文件头 | [`status.md`](status.md)「#16 拍拍只读那一半接上执行」 |
| P5 | **`#16` 拍拍这一页的 `jwttoken` 要不要按旧 TS 那两步自己换**（`GET …/userdomain/v2/querySpecToken?targetSourceId=001005` → `POST …/auth/tyrzLogin`，body 带 `marketName:"shake_shake"`、`sourceId:"1314"`、`type:"signupPlatform"`、`channel:null` ＋ 一个 `type` 头） | 不拍板就继续拿签到页那张票顶这一页用（§1.12 的**临时态**被固定下来）；拍板"换"⇒ 要多发**两个本仓零实拍**的上游请求，且第一个端点是本仓没实现过的另一个 URL | **代码侧已不再缺东西**：那两步已实现成 `app_play_ai_session.go`（`PlayAIQuerySpecTokenRequest` → `PlayAITyrzLoginRequest`，32 条测试）。⇒ 这一条现在是**纯取舍**：要不要在第一次真打前改用它。**本轮接线时明确没有切**（`play_ai_run.go` 文件头把理由与"切的时候动哪一处"都写死了：改的是 `SignInWorld.Source` 里那个 `RecipeSignInPage`，界面上这条链仍复用签到页那份池）。当前①…⑤仍收注入的 jwt，注释里"jwttoken 使用公共来源"照 §1.12 保留 | [`status.md`](status.md)「#16 三样前置落地成代码」三、本轮那一节 |
| P6 | **`#16` 的两条前置门槛接不接**：`visitlog/journaling` 埋点（旧实现里**上报失败就放弃整件任务**）与 `chat/v2/add`（SSE，"拍照问AI"那件的第 5 步）—— 两样本仓**零实拍** | 不接 ⇒ 这一族的编排**跑不到"任务完成"那一步**（旧实现的门槛在那儿摆着）；接 ⇒ 发两条形状靠旧 TS 推的请求，上游认不认**未测试** | 请求侧**已落地**（`app_play_ai_session.go` 里埋点五种 keyword + body 是字符串；`chat/v2/add` **只构造、不发送**，SSE 响应零解析）。⇒ 这一条只剩"编排接不接门槛"这一句。本轮的只读链**不碰**这两口（③ 一次都没打过，测试逐点数过），所以它不挡只读那一半 | [`business-feature-inventory.md`](business-feature-inventory.md) §2.4 `play-ai-special` "Go 侧现状" |

## 二、不挡路（已按默认推进，拍板只改口径）

| # | 问题 | agent 侧当前默认（**非用户要求**） | 出处 |
| --- | --- | --- | --- |
| Q1 | `420` / `421`（用户级每日 / 每月配额）零样本，要不要为它们补一发探针 | 落 `default` 档：结论 `Failed`、**不重发**；不为它们特判 | [`status.md`](status.md)「抢兑」三·2 |
| Q2 | `limit == 3` 两处文案冲突，取哪一种 | 取 `ExchangePlanLimitThree` 里那一句（另一句留在注释里不删） | 同上 三·3 |
| Q3 | `skipWait=false` 的等待做到什么程度（旧实现有 `:59:40` 预热 + 2 条埋点） | 只"等到下一个整点前 20 秒"；不做预热窗口 | 同上 三·4 |
| Q4 | `offset` 要不要回到参数面（旧项目有，本项目刻意不收） | 不收，写在 `PlanExchangeWait` 的注释里 | 同上「Go 侧落点」三处刻意不同 |
| Q5 | 兑换要不要命令行面（`mcloud signin --exchange`） | 不做。入口只有"这一页 + 定时任务 + `POST /api/runs`"。补就要同时改**冻结对拍面** `docs/cli-usage.md` | 同上 三·10 |
| Q6 | 同一账号并发跑两条下单要不要收成"一次只跑一条"（这一族没有幂等键） | 不额外收：锁键含奖品摘要 ⇒ 不同奖品各一把锁 | 同上 三·7① |
| Q7 | 名字与 id 指向同一件商品却是两把锁，要不要起跑前解析成 id 再进指纹 | 不解析（解析要多打一次 `exchangeList`） | 同上 三·7② |
| Q8 | `Hidden` 的能力被 schedule 指到、无人值守真扣豆，要不要额外挡一道 | 不挡（今天能指到）。界面侧的闸门是二次确认那一句 | 同上 三·8 |
| Q9 | 下单前那两条 `visitlog/journaling` 埋点补不补 | 不补（本仓库只有一份不同形的 `journaling`，且缺了会怎样**没有样本**） | 同上 三·5 |
| Q10 | `revival` 的本地月额度表建不建（上游没有"还剩几张卡"的查询口） | 不建，靠上游回码 | [`status.md`](status.md)「签到页那一族接上执行」三·1 |
| Q11 | `prizes` 只取一页（`pageSize=10`）多于十条静默截断，要不要翻页 | 照搬旧项目：不翻 | 同上 三·3 |
| Q12 | `notice` 领云朵另三口（`taskListV2` 找 id、`reportAppNoticeStatus`）接不接 | 不接，只走 `getTaskInfo → reward → 复查` | [`status.md`](status.md)「新链路批」三·2 |
| Q13 | `task-list` 的"做任务"那一半（`doTaskPost` / `task/click` / `receiveTask` 三条**有实现、没接链**）做不做 | 暂不接（先只查）。三条链的写副作用与 #9 那 16 次拉取一起考虑 | 同上 三·5 |
| Q14 | `clientVersion` 三个取值（11.2.1 / 11.4.0 / 12.6.0）各在哪些请求上 | 一律 12.6.0（`CaiyunAppVersion`） | 同上 三·5 |
| Q15 | 仓库里剩下的两个 `.py` 脚本（`applog-capture.py` / `probe-hcy-cool-flag.py`）：补 x 位还是按 AGENTS §3.5 收口成 zx | 不动（只把文档口径统一成 `python3 …`） | [`status.md`](status.md)「抓包工具与使用说明」三 |
| Q16 | `docs/status.md` 已 2447 行，什么时候搬历史（拆出 `status-log.md`） | 不搬，等两族链子收口；方案已写好 | [`status.md`](status.md)「run 产物账本」五 |
| Q17 | #9 那份"16 次拉全"的聚合（`internal/app/task_matrix.go`）要不要登记成**一条能力**并接 WebUI | **不登记**（agent 侧默认，非用户要求）。理由是它今天只有读、没有对应的"这一页显示什么"设计：现成的 `task-list` 能力打的是 `taskListV3`（签到页那张表），两张表都叫"任务列表"但**不同形不同链** ⇒ 要么加成 `task-list` 的第二页、要么另起一条码，两样都改界面。`AppTaskMatrix` 与 `TaskMatrixNeverClick` 因此**都没有调用点** | [`status.md`](status.md)「#9 的 `taskListV2`」那一节的四·1 |
| Q18 | #10 AI 头像"生成几张"这件事的**默认值口径**：schema 写 10、旧代码写 5、旧文档写 5（且标 1–20），本仓钉成常量 5 | 取**旧代码实际跑的那个值**（5），不取 schema。⚠ 这一条与下面 Q19 是两件事：Q18 管"默认值取哪一份"，Q19 管"要不要开成参数"。（原 Q18 问的是"要不要下一轮登记成能力"，那一问已由 2026-09-24 那轮落地回答，就地改写不叠补记） | [`business-feature-inventory.md`](business-feature-inventory.md) §2.3 `ai-avatar`、[`status.md`](status.md)「#10 AI 头像编排层落地」四·2 |
| Q19 | **每日生成几张要不要开成参数面**（`aiAvatarDailyGenerateCount = 5` 今天是常量，界面上改不了） | 不开，钉成常量（agent 侧默认，**非用户要求**）。要开就是 `Params` 多一格 + 冻结对拍面 `docs/cli-usage.md` 的编排段跟着改；而旧项目那三处默认值互相打架，先定口径（Q18）再谈参数。副作用侧的后果明确：每多一张就多往网盘写两张文件，而清理还没接（P1） | 同上四·2、`internal/app/ai_avatar_run.go`（`AiAvatarDailyGenerateCount` 那条常量注） |
| Q20 | **合成图 URL 要不要收白名单**：`imageUrl` 由上游响应给，本仓拿它**裸 GET**（不带任何头、不校验主机） | 不校验，照旧实现（agent 侧默认，**非用户要求**）。登记这一格是因为它的**失效方式安静**：上游给一个内网地址或带凭据的查询串时，我们照打不误，而日志里只会看到"下载失败"。要收就是一份主机白名单（`img.*` 那一档）+ 拒绝带查询串的 URL，两样都得先知道真实 `imageUrl` 长什么样（R17 的样本缺口） | 同上四·3、`internal/app/ai_avatar_run.go`（`downloadAiAvatarImage` 头注） |
| Q21 | **`drawAll` 的"发不出去就停手"**是本仓刻意不同于旧实现（旧 `code!==0` 是 `continue`）—— 要不要改回去 | 保持停手（agent 侧默认，**非用户要求**）。代价写死在注释里：上游给了额度而我们**少抽**。改回去的前置条件是协议层先把"发不出去"分型（网络/超时 vs 上游回了非 0），否则一次抖动会让后面几次全不做；分型那一格本仓零样本（R4 同源） | 同上四·4、`internal/app/ai_avatar_run.go`（`drawAll` 头注） |
| Q22 | **推送要不要重试**（以及要不要一条持久化队列）：方案文档 §1.6 写的是「发送层重做：补重试、超时、失败可见」，而超时与重试的落点在 `io.Client`，它的规则是「写请求必须带幂等键才重试」—— 推送渠道**没有**幂等键这种东西 | 本层**不重试**：一趟跑完一家一条结论。不重试 ⇒ 某一家网络抖一下这一次就少一家收；重试 ⇒ 用户可能收到**两条一模一样的推送**（pushplus / 钉钉没有去重键），哪一边更不能接受是用户口径。真要持久化队列是另一档工程量（表 + 状态位 + 重投时机） | [`internal/upstream/notify.go`](../internal/upstream/notify.go) 文件头「重试：本层**不**自己重试」 |
| Q23 | **推送在哪些点触发**：2026-09-24 已按默认接上 **run 收尾那一处**（`internal/app/notify_run.go` + `Runner.Run` 的尾巴，一趟一条），剩下的问句只有两条，已分拆成下面的 Q26 / Q27 | agent 侧默认（**非用户要求**）：一趟一条、接在跨账号收口处。接错方向的两种表现都不报错 —— 只接 CLI ⇒ 后台定时跑完永远不推（用户以为坏了）；每家每次一推 ⇒ 十个号 × 三家 = 三十条轰炸 | [`internal/app/notify_run.go`](../internal/app/notify_run.go) 文件头、[`internal/app/run.go`](../internal/app/run.go)（`RunSummary.Notify` 那条字段注） |
| Q24 | **邮件这一家的 SMTP 出口接在哪**：2026-09-24 已接进 server 的组装根（`Deps.mail()` 惰性构造 `io.NewSMTPSender()`，与 `Deps.http()` 同一条口径），`NotifyWorld` 从它取 —— 界面上那一路（手动跑、调度跑、试发）今天**都带得到**发送器 | 剩下的只有两处：① CLI 那一路仍不接（与 Q26 同一格，不重复拍）；② 测试里**必须**注入假的，漏注入的表现不是变红而是真往库里填的那台 SMTP 连出去并交出授权码（`Deps.Mail` 的字段注与 `internal/server/notify_test.go` 抬头那两条就是为此）。风险面：SMTP 出站端口（465 / 25 / 587）在部分网络里根本不通，那时报的是「连不上」，看起来像配置问题 | [`internal/server/server.go`](../internal/server/server.go)（`Deps.Mail` / `mail()`）、[`internal/server/notify_test.go`](../internal/server/notify_test.go) 抬头、[`internal/io/mail.go`](../internal/io/mail.go) 文件头 |
| Q25 | **明文连接上拒绝送出授权码**这一条要不要保持：本仓在「非 465 端口 + 服务端不给 STARTTLS + 填了授权码」时**直接判失败**，而旧 nodemailer 那种写法是照登（它只有 `secure: port===465` 一句判断） | 保持 ⇒ 少数只提供明文 SMTP 的内网中继配不上（表现是一句看得懂的失败）；放开 ⇒ 授权码在网线上裸奔，而它同时也是**云盘账号那把钥匙的二次防线**。少发一封是可见的失败，码被掐走是不可见的损失 | [`internal/io/mail.go`](../internal/io/mail.go) 文件头「一处刻意不同于旧实现」+ `TestPlainServerWithPasswordRefusesToSendCredentials` |
| Q26 | **CLI 那一路（`mcloud signin`）推不推**：旧项目的推送恰恰只在 CLI 那一趟跑完发生，而本仓的推送接在 [app.Runner] 的收尾 —— CLI 两条路**都不经过**它（它们各自把结果直接渲染到 stdout，也没有带级别的过程话可攒） | 不接 ⇒ 用 CLI 的人配了推送也不会收到（与本仓「CLI 面是冻结对拍面」一致：加开关要连 `docs/cli-usage.md` 一起改）；接 ⇒ 要么给 CLI 造一份 level 化的过程话，要么在收尾处读库里的 `notify_config` 拼一份「只含小结」的正文，两样都不等于旧实现那句「把 logger 全抄进正文」 | [`internal/cli/run_signin.go`](../internal/cli/run_signin.go)（文件头那张「这一层不做什么」的表）、[`docs/cli-usage.md`](cli-usage.md) |
| Q27 | **推送结论要不要持久化、持久化在哪**：今天它只在 `RunSummary.Notify` 里（内存），跑完就没了 —— 界面上「这次到底推没推出去」查不到 | 现状的失效方式：某一家长期失败（token 过期）而没人发现，因为每趟只在小结里过一下。要持久化得先决定写在哪：`run_events` 是**按单账号那一趟**组织的（主键 `(run_id, seq)`），一次跨账号推送不属于任何一条 run，写进任意一条都会让那趟的事件流含进别人的事；五类事件又是**与 Rust 侧逐字一致的冻结契约**，不为一族新增一类 ⇒ 剩下的候选是调度记账（`schedules` 那几列）或一张新表 | [`internal/app/run.go`](../internal/app/run.go)（`RunSummary.Notify` 字段注）、[`internal/core/run.go`](../internal/core/run.go)（`RunProgress` 头注那条「不为了传数据新增一类」） |
| Q29 | **推送配置是单行表、无归属，而本服务今天没有任何 admin 角色门槛**（`notify_config` 与 `exchange_config` / `webui_config` 同族，`CHECK (id = 1)`）：任何一个已登录身份改它，影响的是**整个安装**的推送 | agent 侧默认（**非用户要求**）：照既成事实接三条鉴权接口（只要求"已登录"），不在这一族里新造一道角色闸。后果：多用户部署下，`bob` 能把 `admin` 配的那家渠道整块删掉、或把密钥换成自己那台服务 —— 而**推送正文里带着账号标识与运行过程话**，那等于把别人的运行摘要引到自己的收件箱。要收就是一次成型的角色门槛（`RoleAdmin` 已有模型字段，缺的是 HTTP 面那一层），它会同时改变 `exchange_config` / `webui_config` 那一族的行为，不该由推送这一族单方面定。现状由 `TestNotifyConfigIsOneRowSharedByEveryUser` 钉成**既成事实**（不是"应当如此"）：加那道闸时这一条一定会红 | [`internal/server/notify_config.go`](../internal/server/notify_config.go) 文件头「这张表**无归属**」、[`internal/server/notify_config_test.go`](../internal/server/notify_config_test.go) 那条用例 |
| Q30 | **写侧是逐字段合并 ⇒ 清空单个字段做不到**（空串与"忘了填"在请求体里同形，只能保住已配好的那一份）；`customPost` 是数组、"同一条"没有可靠身份 ⇒ **整块替换**（改其中一条要把那几条重提交，URL 得重填） | 保持 ⇒ 界面上"我把这个 token 删掉"这件事只能用「删掉这一家」表达，而清空一格与忘填在只有一枚输入框的表单里无从区分。要另开一条口子只有两种形状：① 每格一个「清除」按钮（前端多一层状态，且**每个**字段都要有）；② 空串一律算清空（那就变成"表单里哪个框没填就洗掉哪个"，而这一族的读视图是脱敏的、界面**拿不到**原值 —— 那正是本条要防的静默丢失）。按下标对齐合并已被排除：删掉第一条会让后面每条错位读到上一条的凭据 | [`internal/server/notify_config.go`](../internal/server/notify_config.go) 文件头「写入是**逐字段合并**」+ `TestNotifyConfigBlankSecretKeepsTheStoredOne` / `TestNotifyConfigCustomPostIsReplacedWholesale` |
| Q31 | **出网视图里有三格按凭据处理，而 core 里它们只是普通字符串**：`workWeixinBot.url`、`customPost[].url`、`customPost[].headers`（后者只出键名）。`email.from` 则**跟随** core 的 `SecretValue` 分类（它只是一个发件地址，但不在视图里另判一次） | 从严 ⇒ 这两处地址在界面上只看得见"填了没 + 几个字符"，改一次要**重填**整条地址，而用户没法回头核对自配错没错（放开的代价是"少一次核对"，不报错）；不从严 ⇒ 企业微信群机器人的整条 webhook 地址**就是**凭据（`…/send?key=…`），一次"顺手把 config 整个 marshal 出去"等于把密钥交给任何能看到这个响应的人，而 `SecretValue.String()` 只挡打印、不挡 `encoding/json`（旧坑）。要改的话得先决定"地址要不要显示末段"，那是界面口径不是安全口径 | [`internal/server/notify_view.go`](../internal/server/notify_view.go) 文件头「两处刻意从严」、`TestNotifyConfigResponseNeverCarriesAnyCredential` 里 `workWeixinBot.url` / customPost 那两格 |

## 三、要样本（缺的是实拍，不是决定）

| # | 缺哪一格 | 判反了会怎样 | 怎么补 | 出处 |
| --- | --- | --- | --- | --- |
| R1 | `notice` 的 `cloudStatus` 哪个值是"未领" | **漏领**（不是白发写请求） | 拿一个从没开过通知的号打一次 `getTaskInfo` | [`status.md`](status.md)「新链路批」三·1 |
| R2 | `msg-push` 全族（`obtain` 的方法与 `obtainCode` 非 0 例文、`pushOn` 取值） | 未知 —— 现在只是"手工点一次拿第一个样本" | 手工点一次（已进执行面，`RunInBatch:false`） | 同上 三·3 |
| R3 | `fun-ai` 的 `isWin` 只有"中"那一档 | "没中奖"长什么样无答案 ⇒ 不据此分支 | 跑一次未中奖（需要次数没用完的号） | 同上 三·4 |
| R4 | 写请求重放是否幂等（`receiveV3` / `draw` 没有任何样本；`startSignIn` 有一条旁证） | 重放可能**做两次** | 只能真跑时看（或先不重放） | [`status.md`](status.md)「一轮一个 `jwttoken`」三·1 |
| R5 | 各活动页的 `jwttoken` **特有取法**（现在一律走公共来源 `RecipeSignInPage`） | 上游若认来源 ⇒ 那些请求会莫名失败。<br>⭐ 2026-09-24 有**一格反证**：兑换页那条 `exchangeList` 拿签到页这张公共票**被上游认了**（两个真账号各 55 件、`code 0`）⇒ "被挑来源"在这一口上不成立；**其余页面仍无证据**，别把这一格外推成"公共票到处都认" | 按页面各抓一次换票那一步 | [`status.md`](status.md) 各轮遗留里的"公共来源是临时态" + AGENTS.md §1.12 |
| R6 | 跨主机带 referer（`:7071` / `m.mcloud.139.com` 互带）上游认不认 | 无样本 ⇒ 只能说"现在这么发" | 与 R5 同一次抓包顺手带上 | [`status.md`](status.md)「新链路批」三·6 |
| R7 | `batchTrash` 对**非空目录**的行为 | 可能删不掉（回到漏记），也可能连目录带内容清掉（用户侧后果不明） | 同一测试号上：建目录 → 往里放一个 → 删它 | [`status.md`](status.md)「run 产物账本」四·2 |
| R8 | 拍拍 / 红包派对两族的活动站会话与埋点是否必需（文档说"要"，夹具里一条都没有） | 真打上游时非 0，第一个要复核的就是这里 | 两族各自第一次真跑时按请求逐条比对 | `internal/upstream/app_play_ai.go` 文件头（"登记，不是漏掉"那段） |
| R9 | **红包派对装机链的响应侧整族零实拍**（`subscribeDetails` 取层、`instanceList[0].installStatus`、`success`/`taskId`、`taskStatus` 的 2/3 两档） | 取错层 ⇒ 编排把"锁屏机"当"没锁的机"送去装 APK（订正前正是这一格，已在四档测试里钉住形状）；`installStatus` 判反 ⇒ **真重复发一次安装** | 只能真装机时按请求逐条比对（有副作用：往云机写 APK） | [`status.md`](status.md)「#18 装机链按旧 TS 订正」一·五 |
| R10 | **红包派对换票第②枪（`tokenValidate`）我们这条客户端发出去认不认**：`token` 头位填**空串**这一格只对齐到"与旧 TS 字面量一致" | 不认 ⇒ 整族跑不通（换不到票，三枪读数都出不来），而第一个报错点看起来像"清单挂了" | 真打一次只读那一半（本仓在这一条能力上**只发**两枪换票 + 三枪查询，没有装机 / 记账 / 写网盘那几口），看第②枪的 `header.status`；回非 200 时先换成填 `ssoToken` 再比 | [`status.md`](status.md)「#18 只读那一半接上执行」四·2 |
| R11 | **拍拍这一族撞 `90001` 时"刷一次票、重放一次"到底救不救得回来**。本轮把 `90001` 在 `newyear/*` 信封里也分类成"会话过期"，判据是**借**签到族那条实测（`90001` = `jwttoken` 没了或过期），而 newyear 那六条实拍**全是 `code=0`** ⇒ 这一族到底会不会回这一码、回了以后刷票认不认，**两个都没有样本** | 分错方向有两种：不该分而分了 ⇒ 一次参数级失败被重放一次（只读链上无副作用，代价是一次请求）；该分而没分 ⇒ 票池里那张过期的票被一直打，这一条能力从此每趟都失败，而界面上只看得到"上游回 90001" | 真跑一次这一条只读链，中途让票过期（或手工把池里那一条改脏），看它是"刷一次就成"还是"重放仍 90001" | `internal/upstream/app_play_ai.go`（`playAINewyearPayload` 里那一支）、`TestPlayAINewyearNineZeroZeroOneIsSessionExpired` |
| R12 | **"票借签到页、referer 用这一页自己那份"这个组合**上游认不认（本轮刻意不把 referer 也借过去，见 `play_ai_run.go` 文件头"Referer 不借"） | 真打回非零码时，第一个可疑点就是这一处 —— 而它报错的样子与"这一期活动结束了"（P4）**一模一样** ⇒ 先确认 P4 再动这里 | 第一次真打若非 0：把 `callPlayAIStatus` / `callPlayAITaskList` 那两处的 `upstream.PlayAIPageReferer()` 换成会话自带的 `s.referer` 再打一次对比（这两处是唯一写法，改完两枪一起生效） | `internal/app/play_ai_run.go` 文件头、`TestPlayAIRunBorrowsTheSignInTicketButNotItsPage` |
| R13 | **`activityid` 跟着 `marketname` 变**这一格上游认不认（旧 TS 逐字这么带，本仓一次真打都没有） | 不认 ⇒ 十六次里 `newsign_139mail` 那八次可能被当签到页那张表回（读数看着"对"，其实是**另一张表**），界面上不会报错 | 第一次真打时按请求逐条比对：两个市场名各拿一组响应，看条目与 `marketname` 字段是否成两批 | `internal/upstream/app_task_list_v2.go` 文件头"取值出处"、`TestTaskListV2ActivityIDFollowsTheMarketName` |
| R14 | **`newsign_139mail` 那一支的响应零样本**：夹具里两条 `taskListV2` 的 `marketname` **全是** `sign_in_3` ⇒ 八次邮箱侧拉取"回什么形状"没有一条实拍 | 若那一支回空 `result` ⇒ 聚合表只有云盘那半边（不会报错，只会少任务）；若回的是同形数据 ⇒ 现在这套解析直接能用 | 同一测试号打一次 `marketname=newsign_139mail` / `group=month`（只读） | 同上文件头、`TaskV2Item.MarketName` 那条注释 |
| R15 | **`groupState.<group>.taskFinishState` 与 `process` 两格的语义**：实拍一次空对象、一次是 `2`，而同一份响应三条**全是** `FINISH` ⇒ 它不是"完成/未完成"那种二值口径 | 拿它当"这一组做完了没" ⇒ 会把已经做完的一组再送去执行；拿 `process` 当进度 ⇒ 实拍三条恒 `0`，界面上会念成"一步都没走" | 只能等接写侧后在同一账号上跑一次"未完成"的组，看这两个数怎么变（今天这一族**没有**任何未完成的样本） | `internal/upstream/app_task_list_v2.go`（`GroupFinishStates`、`TaskV2Item.Process`）、`internal/app/task_matrix.go`（`SummaryLine` 里"不报额度"那一句） |
| R16 | **`hcy/file/list` 的请求体键集合**：本仓按旧 TS 那**五键**发（`parentFileId` / `pageInfo.pageSize` / `imageThumbnailStyleList` / `orderBy` / `orderDirection`），而实拍那一份是**另一个客户端**（okhttp + 完整 `x-yun-*`）发的十余键（另有 `fields` / `ownerId` / `parentFilePath` / `type` / `workSpaceType` / `pageInfo.needTotalCount` / `pageInfo.pageCursor`） | 这一口**只读** ⇒ 最坏表现是"列不出文件"（清理那一族按名字找不到目标），不会误删；但"少发那些键会不会被拒、会不会回空 `items`"**零样本** | 接上编排后在测试号上真打一次，比对 `items` 与真机 App 里看到的目录内容 | [`internal/upstream/app_ai_avatar.go`](../internal/upstream/app_ai_avatar.go)（`CloudFileListRequest` 头注）、`test/fixtures/http/ref.json` 那两条实拍 |
| R17 | **`rabactive/*` 六口整族零实拍**：本仓所有归档里 `grep rabactive` **一条都没有** ⇒ 六口的 URL、参数、信封形状只对齐到"与旧 TS 字面量一致" | 路径或参数错时上游可能回 **404 页**，而 404 页在 `ParseEnvelope` 那层表现为"响应体不是合法 JSON" ⇒ 看着像**解析器**的错，不像接口写错。所以请求形状那一组测试只钉"发得出去、形状不串台"，不钉"上游认" | 这一页在活动站 H5 里，**要活动在线**才补得了样：抓一次页面加载链（含 `getHashLeft` 与 `draw` 各一条） | 同上文件头「证据分两半」那一段 |
| R18 | **`getStatus.status` 的档位表（`3` 成功 / `4` 失败）与 `draw` 的 `408` 语义**：档位是从旧实现的分支形状**反推**的，两格都零实拍 | 把"还在合成"念成"失败"⇒ 提前放弃这一轮；把 `408` 念成"可重试"⇒ 对已拒绝的账号连打五枪（`ParseAiAvatarDraw` 做成三态，正是为了这一格**不靠错误串**判） | 真跑一次生成，逐次记录 `getStatus` 的 `status`；再抽到次数用完那一次，把响应体逐字记进 `docs/capture/` | 同上（`AiAvatarStatusSuccess` / `AiAvatarStatusFailed` / `AiAvatarDrawCodeStopped` 三条常量注） |
| R19 | **`upload` 写进用户网盘那张图的找回路径**：靠 `getWorkList` + `catalogFullPath` + `file/list` 三枪**按文件名**匹配。找回这三枪**已于 2026-09-24 接线**（`internal/app/ai_avatar_files.go`，编排里每次都打），但仍未验的两格是：**网盘里已有同名文件时找回哪一份**，以及**"活动目录 `fullPath_sub` 的末段就是 `fileId`**" | 匹配错 ⇒ 用错图（界面上把别人的图当这次产物念出来），或者将来接清理时**删掉用户自己那张同名图** —— 用户侧表现是"网盘里莫名少一张图" | 在测试号上验一次：先手工放两份**同名**文件再跑生成链，看 `collectAiAvatarFileIDs` 交出哪一个；同时把 `catalogFullPath` 的响应体逐字记进 `docs/capture/`（本仓零实拍）。⚠ 真正的解法不是补样而是**换路**：`file/create` 的响应里本来就有 `fileId`，拿到那一口的实拍后应当改成"直接用上传响应给的 id"，把这三枪整个绕开 —— 那一口在 P1 里 | [`internal/app/ai_avatar_files.go`](../internal/app/ai_avatar_files.go) 文件头（"同名即错配"那一段）、[`status.md`](status.md)「#10 AI 头像编排层落地」一 · 四·5 |
| R20 | **`AiAvatarResult.TempFileIDs` 出不了 app 层**：找回那一族把网盘侧 `fileId` 收进了结果结构体，但**离开这一层的只有一句话** —— `SummaryLine()` 里"网盘里留了 N 个待清理文件"只报**个数**，那一串 id 本身没有任何消费者（`internal/exec` 的产物账本以 `core.RunID` 为键，而 `AiAvatarRun.RunAccount(ctx, account)` 拿不到 runId） | 不是"跑错"，是**跑对了也没人收尾**：那两张图永远留在用户网盘里，且界面上只看得到"留了 10 个"而**不知道是哪 10 个** —— 将来手工清理时无从下手 | 先拍"清理接不接"（P1）。接的时候两档：① `RunAccount` 多收一个 `core.RunID`（改签名 ⇒ 这一族 30 条用例的调用点全要动）；② 让 `RunReport` 把 `TempFileIDs` 带出去、由 server 在 run 收尾时记账（app 层不动）。两档都**没做**，也都没验 | [`status.md`](status.md)「#10 AI 头像编排层落地」四·5、`internal/app/ai_avatar_run.go`（`SummaryLine` 头注"那一串 id 不进这句话"）、`internal/exec/artifacts.go` |
| R21 | **企业微信换 `access_token` 那一枪的方法与参数位置**：本仓照旧 TS 发 **POST + JSON 体** `{corpid, corpsecret}`；「公开形状是 GET + 查询串」这一句在仓库内**没有出处**（只有印象，而印象不算判据 —— AGENTS §1.10） | 形状不对 ⇒ 这一家从来发不出去，而报错的长相是「换票回了 errcode=…」，看不出是自己发的方法错了 | 真发一次看 `errcode`；回非 0 就改成 GET + 查询串再比一次。**两档改动都只在那一个函数里**（`WorkWeixinTokenRequest`），形状用例随之改 | [`internal/upstream/notify_two_step.go`](../internal/upstream/notify_two_step.go)（该函数头注） |
| R22 | **十家渠道一家响应体都没有**：`classifyChannelReply` 那三键（`errcode` / `err` / `code`）是十家**共用**的判据，对某一家必然判松或判紧。已知一处**比旧实现松**：`{"code":"200"}`（字符串数字）这里算已发，旧实现那句 `code === c` 严格相等判失败 | 判松 ⇒ 渠道坏了而小结写「已发」；判紧 ⇒ 一次真成功被记成失败，用户以为是自己配错了 | 每家手工配一次、真发一条，把响应体逐字记进 `docs/capture/`（那是**第三方**服务，不碰云盘账号，风险面只有那一条消息） | [`internal/upstream/notify.go`](../internal/upstream/notify.go)（`classifyChannelReply` 头注那三条边界）、`TestClassifyChannelReplyFollowsTheOldThreeKeys` 里「字符串码」那一格 |
| R23 | **邮件这一家没有一台真 SMTP 服务商的实拍**：`internal/io/mail.go` 的协议步骤（隐式 TLS / STARTTLS 升级 / PLAIN 与 LOGIN 挑选 / DATA 点终结 / 5xx 落进错误文本）**已全部在进程内假服务器上逐字对拍**（`internal/io/mail_test.go` 二十余条），缺的是「真服务商声明什么」这一格 —— 各家 AUTH 机制集合、465 上证书链是否完整、`550` / `535` 那几句人话长什么样、「QQ / 163 要授权码而不是登录口令」这句话本身也仍无仓库内出处 | 真发时最可能的表现是「连上了但被拒」，而我们的错误文本此时只有服务器那句 —— 判错方向的代价是把「该填授权码」念成「密码错了要改密码」 | 拿一个**测试邮箱**（不是云盘账号）手工配一次、真发一封，把 `EHLO` 能力行与每一次 5xx 的**服务器原文**逐字记进 `docs/capture/`（协议侧不需要脚本，一次手工发送即可） | [`internal/io/mail.go`](../internal/io/mail.go) 文件头「这一族零实拍」、`internal/io/mail_fakeserver_test.go` 抬头「能证明什么、不能证明什么」 |

---

## 四、已经拍过、别重问（就近记几行，免得下一轮又问一遍）

- `fun_ai` 每日抽奖 **1 次**（对齐旧 TS）—— 用户 2026-09 拍板。
- 兑换的 `smsCode` 何时非空：**有意搁置**，不列入待核销项 —— 用户 2026-09-22。
- 真跑的口径：**"旧项目怎么样就怎么样"** —— 用户拍板；但"有副作用且已实测过的不重测，其余实测"同属用户口径。
- `GET /api/exchange/prizes` 免登录：**既不是刻意设计也不算缺陷**，当普通鉴权缺口补齐，不再展开论证 —— 用户 2026-09-19。
  ⭐ 现状（2026-09-24）：Go 侧没有照搬那条全局路径，建成**按账号**的 `GET /api/accounts/{id}/exchange-prizes`，
  挂在 `s.authed` 里（旧路径那个"免登录"面因此不存在）。按账号不是审美：`limit` 本来就是各号自己的状态。
- 迁移优先级：**先按两个参考项目把业务逻辑迁完**，之后再慢慢抓包分析更新；无抓包时**接口层冻结、业务层继续**。
