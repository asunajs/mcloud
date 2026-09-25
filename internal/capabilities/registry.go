// 能力声明与注册表。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-capabilities/src/lib.rs。

package capabilities

import (
	"encoding/json"

	"github.com/catlair/mcloud/internal/core"
)

// 能力 code 的**唯一**成文处。
//
// # 为什么要从注册表里提出来
//
// 同一个 code 有三个使用者：注册表本身、执行面的分派表（`internal/server` 按它
// 决定"这一条能力会不会跑"）、以及 [core.ManualStep.Capability] 那一列。
// 各自写字面量的代价是**拼错的那一处不会报错** —— 分派表里 `"sign"` 拼成 `"sign "`
// 表现成"这条能力没接上执行"（回 501），而界面上只写得下"未实现"。
//
// ⚠ 它们**是**对外契约的一部分（`runs.capability` 列、定时任务的 `target.code`、
// 界面路由的键），改任何一个都要过 `docs/cli-usage.md` 与黄金向量那两条基准。
const (
	CodeSign          = "sign"
	CodeExchange      = "exchange"
	CodeCloudSnapshot = "cloud-snapshot"
	CodeLiveRoom      = "live-room"
	CodeAuthRefresh   = "auth-refresh"

	// 下面五条是**签到页那一条会话上的五个开关**（2026-09-23 接线批）。
	//
	// 它们此前已经各自有领域层实现（`ClaimMultiple` / `ClaimBackupGift` /
	// `ReceiveAll` / `ReceiveRevivalReward` / `PendingPrizes`），只是**没有可寻址的名字**：
	// 界面上点不到、定时任务也指不过来（[core.ScheduleTarget] 的 code 必须认识）。
	// code 取值沿用 `docs/business-feature-inventory.md` 那张表里旧项目的小写名，
	// 将来对位时不必再翻一次。
	CodeReceive    = "receive"
	CodePrizes     = "prizes"
	CodeMultiple   = "multiple"
	CodeBackupGift = "backup-gift"
	CodeRevival    = "revival"

	// 下面四条是 2026-09-23「新链路批」：三条**新接上游接口** + 一条只读查询。
	//
	// code 取值同样沿用 `docs/business-feature-inventory.md` §2.3 那张表里旧项目的小写名
	// （`task-list` 是那张表里"把 `app` 拆开"的那一半：查询先落地，玩法逐条补）。
	//
	// ⚠ `notice` / `msg-push` 两条在 `internal/app` 里目前写的是**同串字面量**
	// （`noticeCapabilityCode` / `msgPushCapabilityCode`）—— 因为写那两个文件时注册表里
	// 还没有这两条。现在两条都登记了，那两个常量**指回这里**（`= capabilities.CodeNotice`），
	// 所以"两处各写一遍字面量"这件事已经不存在，不需要额外的测试去钉一致性。
	CodeTaskList = "task-list"
	CodeNotice   = "notice"
	CodeMsgPush  = "msg-push"
	CodeFunAI    = "fun-ai"

	// CodeCloudPhoneRedpack 是红包派对（#18）这一族的名字。
	//
	// 取值沿用 `docs/business-feature-inventory.md` §2.4 里旧项目那个小写名
	// （`cloud-phone-redpack`），与上面那四条同一口径。
	//
	// ⚠ 今天这一条**只对应只读那一半**（看余额、看清单、看播报位）。
	// 旧实现在这一族里还会做浏览 / 答题 / **往云机装 APK** 三类任务，
	// 那三类**没有**挂在这个 code 下面 —— 安装类属待拍板 P3
	// （`docs/pending-decisions.md`）。将来接写请求时，是"这一条多一个开关参数"
	// 还是"再登记一条 code"，得先过那一份文件，不是顺手 `RunInBatch` 打开。
	CodeCloudPhoneRedpack = "cloud-phone-redpack"

	// CodePlayAISpecial 是拍拍（#16 · 旧实现 `play-ai-special`）这一族的名字。
	//
	// 取值同样沿用 `docs/business-feature-inventory.md` §2.4 里那个小写名。
	//
	// ⚠ 今天这一条**只对应只读那两口**（`user/info` + `task/list`）。同一族另外三口
	// 各自卡在不一样的地方，**都没有**挂在这个 code 下面：
	//   - `task/step/click`（写）：旧实现把两条 `visitlog/journaling` 埋点当**门槛**，
	//     而那两条本仓零实拍 ⇒ 待拍板 **P6**；
	//   - `blindbox/lottery`（写、不可回滚）：判据那一格 `totalLotteryCount` 的**语义未验证**
	//     （"累计获得"还是"还剩"两说）⇒ 不拿读不懂的数当写请求的判据；
	//   - `aiRecognize`：`uploadToCloud` 实拍恒 `true` ⇒ **真的往用户网盘写一张图**，
	//     而清理那一族今天接口层冻结（待拍板 **P1**）。
	//
	// ⚠ 还有一格**没定**：这一页的 `jwttoken` 今天用的是**签到页那张**（AGENTS §1.12 的临时态），
	// 特有取法的实现已在 `upstream`（换票两步）但**零实拍** ⇒ 待拍板 **P5**。
	// 拍板"切"的时候改的是 `internal/app/play_ai_run.go` 里取票那一句，不是这里的声明。
	CodePlayAISpecial = "play-ai-special"

	// CodeAppTask 是「App 任务矩阵」（#9 · 旧实现 `appTask`）这一族的名字。
	//
	// 它与 [CodeTaskList] 是**两张表**：那一条打 `taskListV3`（签到页那张，只查），
	// 这一条打 `taskListV2` 按 2 市场名 × 8 group 聚合出来的那张 App 任务矩阵，
	// 并且**会发写请求**（每做一条一发 `task/click`）。
	//
	// ⚠ `docs/business-feature-inventory.md` §2.3 的意见是"`app` 不该是一个能力，
	// 应拆成 `task-list`（查询）+ 每种玩法一个能力"。今天登记成**一条**是刻意的过渡：
	// 十四种玩法里只接上两种（另外十二种缺的是端点族，不是分派逻辑），
	// 现在就按玩法拆 code 会得到十条"未实现"，界面上比一条"这一趟没接上 12 条玩法"
	// 更难读。等玩法逐条接上后再拆（拆的时候这条 code 的**历史行**要迁移，别留两份）。
	CodeAppTask = "app-task"

	// CodeAiAvatar 是「AI 头像」（#10 · 旧实现 `ai-avatar`）这一族的名字。	//
	// 取值沿用 `docs/business-feature-inventory.md` §2.3 里那个小写名。
	//
	// ⚠ 这一条是本注册表里**唯一会往用户网盘写东西**的一条（`image/upload` 一发出去，
	// 网盘里就多两个文件）。写在前面是为了让"要不要挂上调度"这一眼就看到代价：
	// 清理那一族（`recyclebin/batchTrash`）**零实拍、今天没接**（待拍板 P1），
	// 所以这一条 `RunInBatch: false` —— 每天自动跑一次等于每天攒十来个没人删的文件。
	//
	// ⚠ 这一族的 `jwttoken` 今天借的是**签到页那张**（AGENTS §1.12 的临时态）：
	// rabactive 那六口**整族零实拍**，特有取法无从得知。
	// 要切特有取法时改的是 `internal/app/ai_avatar_run.go` 取票那一句。
	CodeAiAvatar = "ai-avatar"
)

// Capability 是能力声明。
//
// `Params` / `Output` 用**函数**而不是常量切片，是为了让"默认值依赖配置"的情况
// （例如账号下拉选项来自当前配置）可以在调用时求值，同时避免调用方拿到一份
// 共享的可变切片 —— 后者才是重点：注册表是**进程级共享**的，
// 若 `Params` 是一个切片字段，任何一个调用方 append 进去的东西都会被下一个
// 能力看到，而那种污染是静默的（表单里多出一个没人声明的字段）。
//
// # ⚠ 本结构体**不**直接进 JSON
//
// Rust 侧它没有 derive `Serialize`：对外形状由 `mcloud-app::payload` 手工拼装
// （`runInBatch` / `needsConfirmation` 是 **camelCase**，与 serde 的 snake_case
// 默认**不一致**）。所以这里刻意不加 tag —— 加了反而会让人以为
// "`Capability` 序列化出来就是给前端的那个形状"，而它不是。
type Capability struct {
	// Code 是稳定 code，用于 API 路由 / 组合步骤 / 定时任务引用（如 `sign`）。
	Code        string
	Title       string
	Group       core.Group
	Kind        core.CapabilityKind
	Description string

	// Hidden 表示在通用任务列表与编排选择器中隐藏（独立页面运行）。
	//
	// 现有唯一用例是 `exchange`：它有自己的奖品页与定时抢兑规则，
	// 混在通用列表里既难用也危险（[core.RiskHigh]）。
	Hidden bool
	// RunInBatch 表示是否参与组合编排与批量执行。
	RunInBatch bool
	Risk       core.RiskLevel
	// DefaultAccountMode 是多账号默认执行方式。**默认串行**，因为高并发容易触发风控。
	DefaultAccountMode core.AccountMode

	// MissPolicyOnMiss 是"这一行定时任务没显式选过补跑策略时，本能力按哪一档办"。
	//
	// 为什么住在能力声明里而不是全局开关：任务有两种脾气（见 [core.MissPolicy] 的
	// 分档理由），而"它是哪种任务"由能力知道、由**用户那一行**最终拍板。
	//
	// ⚠ 类型是**指针**，而且这个设计不是随手选的：值类型的话"忘了填"会静默变成
	// 零值，而零值那一档恰好是"会多打一次真上游"的那一侧（`MissCatchUp` 是 0）。
	// 指针 ⇒ 没声明就是 nil ⇒ [ResolveMissPolicy] 走 [core.MissSkip]：
	// **补跑是"替人授权一次额外执行"，没人声明过就不做**。
	// 上面那句是兜底，不是常态 —— 每条声明都必须显式给，
	// 由 [TestEveryCapabilityDeclaresMissPolicy] 钉住。
	MissPolicyOnMiss *core.MissPolicy

	Params func() []core.FieldSpec
	Output func() []core.FieldSpec
}

// ResolveMissPolicy 定"这一次错过了要不要补跑"，按**离人更近的优先**：
//
//  1. 那一行定时任务显式选过的（[core.Schedule.MissPolicy]）；
//  2. 本能力声明的默认；
//  3. [core.MissSkip] —— 两样都没说 ⇒ 不补。
//
// ⚠ 顺序不能反：能力默认替的是"没人选过"，不是"覆盖用户选的"。
// 反过来的表现是"我在界面上勾了不补跑，它每天还是多跑一趟"。
func ResolveMissPolicy(cap Capability, schedule *core.Schedule) core.MissPolicy {
	if schedule != nil && schedule.MissPolicy != nil {
		return *schedule.MissPolicy
	}
	if cap.MissPolicyOnMiss != nil {
		return *cap.MissPolicyOnMiss
	}
	return core.MissSkip
}

// NeedsConfirmation 报告前端表单是否应要求二次确认。
func (c Capability) NeedsConfirmation() bool { return c.Risk.NeedsConfirmation() }

func noOutput() []core.FieldSpec { return nil }

func accountSelector() []core.FieldSpec {
	return []core.FieldSpec{
		{
			Key:   "accounts",
			Label: "账号",
			Kind:  core.FieldMultiSelect,
			// Options 留空：由服务端在 P5 按当前配置填充。
			Help: strPtr("留空 = 全部账号"),
		},
		{
			Key:     "accountMode",
			Label:   "执行方式",
			Kind:    core.FieldSelect,
			Default: json.RawMessage(`"sequential"`),
			Options: []core.SelectOption{
				{Label: "串行（默认，防风控）", Value: json.RawMessage(`"sequential"`)},
				{Label: "并行", Value: json.RawMessage(`"parallel"`), Help: strPtr("高并发容易触发限流")},
			},
		},
	}
}

func exchangeParams() []core.FieldSpec {
	return append(accountSelector(),
		core.NewFieldSpecText("prizeIdOrName", "奖品").
			MarkRequired().
			WithHelp("奖品 ID 或名称，来自兑换列表"),
		core.NewFieldSpecBool("skipWait", "跳过等待", true).
			WithHelp("关闭后等到整点开抢窗口"),
	)
}

func exchangeOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("cloudCost", "消耗云朵"),
		core.NewFieldSpecText("prizeName", "奖品名称"),
		core.NewFieldSpecText("result", "结果"),
	}
}

func cloudSnapshotOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("cloudCurrent", "当前云朵"),
		core.NewFieldSpecText("cloudDiff", "日增量"),
		core.NewFieldSpecText("cloudDiffWeek", "周增量"),
		core.NewFieldSpecText("updatedAt", "采集时间"),
	}
}

// receiveOutput 是"只领取"这一条的输出。
//
// ⚠ 这一族 `Output` 目前是**给前端看的形状说明**：后端把结论写在终局事件的
// `summary` 与 `runs.message` 里，没有逐字段回填（界面上那几页读的是自己的接口字段）。
// 所以改了这里**不会**改变任何实际输出 —— 别把它当契约的兑现处。
func receiveOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("receivedTotal", "本次领到（豆）"),
		core.NewFieldSpecText("failedCount", "失败笔数"),
		core.NewFieldSpecText("nextMonthCount", "下月才可领"),
	}
}

func prizeListOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("pendingCount", "待领取条数"),
		core.NewFieldSpecText("prizeNames", "奖品名（至前列三个）"),
	}
}

func claimOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("gained", "进账数量"),
		core.NewFieldSpecText("detail", "附属说明（倍数 / 卡型）"),
	}
}

// taskListOutput 是「只查不做」那一族的输出形状。
//
// 字段对应 `internal/app/task_list_run.go` 的 `TaskListCounts`（总数 / 待做 / 已做 / 状态未知）：
// `unknown` 那一格必须有，因为上游的 `state` 实拍只见过 `WAIT` / `FINISH` 两档，
// 第三档长什么样**未验证** —— 有它才能把"读到没认出的状态"和"读到 0 条待做"分开。
func taskListOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("total", "任务条数"),
		core.NewFieldSpecText("waitCount", "未完成"),
		core.NewFieldSpecText("finishedCount", "已完成"),
		core.NewFieldSpecText("unknownCount", "状态读不出"),
	}
}

// appTaskOutput 对应 `TaskRunCounts`（见 `internal/app/task_matrix_run.go`）。
//
// ⚠ 那两格"没做成"的必须**分开设**：`toolGapCount` 是**本工具的**缺口（缺端点族，
// 用户去 App 里点也没用），`manualCount` 是**要用户去做的**（未开备份）。
// 合成一格的表现是：界面喊"有 N 条要你处理"，用户去了 App 发现什么都没得点。
//
// `notAcknowledgedCount`（发了枪、复查仍未完成）是这条链最该被看见的一格 ——
// 它是"上游根本不认这种做法"的第一个信号，今天零实测。
//
// ⚠ 与 [receiveOutput] 头上那条同一件事：这一族 `Output` 今天是**形状声明**，
// 后端没有按这些 key 逐字段回填，web 侧也还没读 `capability.output`
// （2026-09-24 查过：`web/src` 里 `.output` 零命中）。
// 这一趟真正给用户看的数，走的是 `TaskMatrixRunResult.SummaryLine()` ⇒
// 终局事件的 `message` 与 `runs.summary`。改了上面六格**不会**改变任何实际输出。
//
// ⚠ 2026-09-25 起事件流里**还多了一层逐条播报**（`internal/app/task_matrix_report.go`：
// 组分节 + `成功/已完成/失败/需前往…完成` 四档行 + 每一发上传一行）。它不改上面这六格
// —— 那六格是**计数**声明，播报是**过程**；两者今天都不经 `capability.output` 渲染。
func appTaskOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("pendingCount", "待做条数"),
		core.NewFieldSpecText("clickedCount", "已上报"),
		core.NewFieldSpecText("failedCount", "上报失败"),
		core.NewFieldSpecText("notAcknowledgedCount", "上报了但未认"),
		core.NewFieldSpecText("toolGapCount", "本工具未接玩法"),
		core.NewFieldSpecText("manualCount", "需人工"),
	}
}

// noticeOutput 对应 `NoticeResult`（见 `internal/app/notice_run.go`）。
//
// ⚠ `gained` 在实拍里**恒为空** —— 上游 `reward` 的 `result` 没给出朵数
// （那是 [upstream.NoticeReward] 注释里登记的样本缺口）。这一格留着而不是删掉，
// 是为了让"领到了但没报数"在界面上显示成空而不是 0。
func noticeOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("gained", "本次进账（上游未报数时为空）"),
		core.NewFieldSpecText("taskOne", "邮箱通知槽"),
		core.NewFieldSpecText("taskTwo", "短信通知槽"),
	}
}

// msgPushOutput 对应 `MsgPushResult`（见 `internal/app/msg_push_run.go`）。
//
// ⚠ 这一族的字段名来自旧 TS 的文字描述，**尚无抓包样本** —— 所以这里只列
// "判据要看的那三个读数"，等真跑通一次再按实况增删（改之前先回那一份文件头看证据）。
func msgPushOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("pushOn", "推送开关读数"),
		core.NewFieldSpecText("secondTaskStatus", "第二次任务状态"),
		core.NewFieldSpecText("obtainCode", "领取回执"),
	}
}

// funaiOutput 对应 `FunaiResult`（见 `internal/app/funai_run.go`）。
//
// 两格各自都留空的情形不同：`prizeName` 空 = **上游没说**（≠ 没中奖，实拍 `isWin`
// 只有 `1` 一档，"没中奖长什么样"未验证）；`plan` 空 = 判据那一枪就没读成。
func funaiOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("prizeName", "奖品名（上游没说时为空）"),
		core.NewFieldSpecText("plan", "门槛判定"),
	}
}

// cloudPhoneRedpackOutput 对应 `CloudPhoneAssets`（见 `internal/app/cloud_phone_run.go`）。
//
// # 为什么钱那一格写"元"而本层不做除法
//
// 上游给的是**分**（`canAmount "210"`），除法在 `SummaryLine` 那一层做，并且**两句都留**
// （"2.10 元（210 分）"）—— 单位是这一族最容易读错的一格，除错了要能被看见。
//
// ⚠ 这一族的形状今天只有**只读那一半**：`pendingCount` 数出来的是"读成未完成的条数"，
// 而本工具**一件都不代做**（浏览 / 答题 / 装 APK 三类都没接，见 `cloud_phone_run.go` 文件头）。
// 留这一格是为了"将来接了写请求时不用改字段名"，不是"已经会做了"。
func cloudPhoneRedpackOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("canAmount", "可兑余额（元；上游没给这一位时为空）"),
		core.NewFieldSpecText("totalAmount", "累计（元）"),
		core.NewFieldSpecText("taskCounts", "任务条数（新手/每日/每月）"),
		core.NewFieldSpecText("pendingCount", "读成未完成的条数（本工具不代做）"),
	}
}

// playAISpecialOutput 对应 `PlayAIBoard`（见 `internal/app/play_ai_run.go`）。
//
// ⚠ 与 `cloudPhoneRedpackOutput` 同一条纪律：这一族是**给前端看的形状说明**，
// 后端今天只出 `SummaryLine` 那一句人话，没有逐字段回填。
//
// `lotteryCount` 那一格的说明**故意写成两问**（"累计还是剩余未验证"）：
// 界面如果只写"抽奖次数"，用户会按"还剩几次"理解它，而那一档**没有样本**。
func playAISpecialOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("lotteryCount", "抽奖次数那一格读数（累计还是剩余，未验证）"),
		core.NewFieldSpecText("taskTotal", "任务条数"),
		core.NewFieldSpecText("taskWaiting", "读成未完成的条数（本工具不代做）"),
		core.NewFieldSpecText("taskUnknown", "状态是这一族没见过的值的条数"),
	}
}

// aiAvatarOutput 对应 `AiAvatarResult`（见 `internal/app/ai_avatar_run.go`）。
//
// # `tempFileCount` 为什么是"个数"而不是那一串 id
//
// 界面上摆一串网盘对象 id 没有任何可读性，而这一格真正要传达的是
// "你的网盘里因为这一趟多了几个待清理的东西"。id 本身在 `SummaryLine` 那句里也不出现
// （它只数个数）—— 要看具体是哪几个文件，得回事件流。
//
// ⚠ `drawPrizes` 空**不等于**没中奖：上游两格名字（`contentName`/`prizeName`）
// 在零样本的族里谁必填都不知道，两格都没有时这一族只报"抽了几次"。
func aiAvatarOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("generated", "生成成功的张数（计划 5 张）"),
		core.NewFieldSpecText("drawCount", "抽中的次数"),
		core.NewFieldSpecText("drawPhase", "抽奖停在哪一档（抽完/被叫停/没抽/被拦）"),
		core.NewFieldSpecText("drawPrizes", "奖品名（上游没报名字时为空）"),
		core.NewFieldSpecText("tempFileCount", "写进网盘、等清理的文件个数（今天不删）"),
	}
}

// liveRoomParams 是「直播口令 → 小红花」的参数。
//
// # 为什么口令做成可填的
//
// 自动收口令要**听满一段时间**的弹幕，而它只有在主播开播的那几小时里才有意义。
// 手工填口令让这条链在"没开播、但口令已经从别处拿到了"的情况下也能跑
// —— 而那时它退化成一条**纯六步链**，与直播毫无关系。
//
// ⚠ 但手工口令有一条硬限制：实拍那句
// `口令需当场直播获取才有效哦` 说明**从别处抄来的口令一律不通**。
// 所以这一栏的用途是"把刚在弹幕里看到的那一个填进来"，
// 而不是"攒一批口令离线投"。
func liveRoomParams() []core.FieldSpec {
	return append(accountSelector(),
		core.NewFieldSpecText("codes", "口令").
			WithHelp("留空 = 自动从弹幕与小红书收集；多个用换行或逗号分隔"),
		core.NewFieldSpecText("listenSeconds", "听弹幕时长（秒）").
			// ⚠ 默认值写在**声明**里，而不是让服务端自己记一份：界面上那一栏
			// 就是照这份声明渲染的，声明里的默认值与后端用的缺省不一致时，
			// 表现是"界面显示 60、实际听了别的数"。
			// CLI 那一侧的缺省是同一个人填的另一个数字（`--listen-seconds`，
			// 见 docs/cli-usage.md），改这里要一并想到它。
			WithDefault(json.RawMessage(`60`)).
			WithHelp("默认 60。只在自动收集时生效"),
		core.NewFieldSpecBool("koulingOnly", "只听不投", false).
			// ⚠ 措辞是"不投"，**不是**"不消耗"：口令全场通用，不存在消耗
			// （见 `internal/upstream/live_flower.go` 文件头五条事实②）。
			WithHelp("只收口令并打印，**一个口令都不投给上游**"),
		core.NewFieldSpecBool("skipKouling", "只用给定口令", false).
			WithHelp("跳过收集，直接投 `口令` 那一栏填的内容"),
	)
}

func liveRoomOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("koulingCodes", "收到的口令"),
		core.NewFieldSpecText("balanceBefore", "投前小红花"),
		core.NewFieldSpecText("balanceAfter", "投后小红花"),
		core.NewFieldSpecText("gained", "本次进账"),
		core.NewFieldSpecText("succeeded", "成功口令数"),
		core.NewFieldSpecText("invalidCodes", "无效口令"),
		core.NewFieldSpecText("revivalFlowers", "复活花"),
	}
}

// authRefreshParams 是「刷新 auth 凭据」的参数。
//
// ⚠ 参数面里**没有** `force`，这一条是刻意的，不是漏了：
// `force` 的语义是"人已经拍板现在就换"（越过提前量那道闸），而定时任务里**没有人**。
// 把它开成参数等于给定时任务开一个"每个触发点都硬换一次 token"的开关 ——
// 换来换去都是同一套凭据在轮换，而轮换对旧串的影响至今未实测（见 docs/auth-refresh.md §8）。
// 手动那一枚按钮走的是另一条路由（`POST /api/accounts/{id}/refresh-auth`，恒 force），
// 与这条链不共用参数面。
//
// 同理也**没有** `leadDays`：提前量是"这台机器替所有人定的那一个数"，
// 它今天来自 CLI 的 `--lead-days` 与后端缺省，不是每行任务各调一份的旋钮。
// 真要逐行可调，那是先改 [core.Schedule] 的契约、再改这里，不是顺手加个参数。
func authRefreshParams() []core.FieldSpec { return accountSelector() }

func authRefreshOutput() []core.FieldSpec {
	return []core.FieldSpec{
		core.NewFieldSpecText("end", "端别").
			WithHelp("pc / mobile，取库里那串的第一段"),
		core.NewFieldSpecText("decision", "判定"),
		core.NewFieldSpecText("rotated", "是否换成"),
		core.NewFieldSpecText("afterExpire", "换后到期时刻"),
	}
}

// registry 是能力注册表。
//
// 用显式列表而不是"注册函数 + init()" —— 可 grep、可 diff，新增能力只改这一处。
//
// ⚠ 上面那句"这里先登记 3 项"是**迁移当时**的原文，现在共 **17** 项，分五组：
//
//   - **迁移对位**（与冻结基准的 code 集合 `diff` 为空）：`sign` / `exchange` /
//     `cloud-snapshot`；
//   - **新增能力**（那条链在原项目里是 **TS 实现**，`legacy/` 里没有对应 crate，
//     **无基准可对**）：`live-room` / `auth-refresh`；
//   - **签到页会话上的五个开关**（2026-09-23 接线批）：`receive` / `multiple` /
//     `backup-gift` / `revival` / `prizes` —— 这一组**不新增上游接口**，
//     只是把早就存在的领域层实现挂上可寻址的名字。
//   - **新接口批**（2026-09-23 起）：`task-list` / `notice` / `msg-push` / `fun-ai` /
//     `cloud-phone-redpack` / `play-ai-special` / `ai-avatar` —— 这一组**各自都带来了新的上游接口**，
//     逐条的证据与"哪些仍是待验证"写在它们各自那一段注释与
//     `internal/upstream/app_*.go` 的文件头（`cloud-phone-redpack` 与 `play-ai-special`
//     **都只有只读那一半**，为什么各自没接上写请求分别看 `cloud_phone_run.go` 与
//     `play_ai_run.go` 文件头；`ai-avatar` 相反 —— 它接的**是**写请求，而且产物会留在
//     用户网盘上今天没人清，所以它 `RunInBatch: false`，理由见 [CodeAiAvatar]）。
//
// 别把 17 当成"基准里多出来了什么"。
//
//   - `live-room`：直播口令 → 小红花。
//   - `auth-refresh`：把"到期换凭据"登记成能力，**唯一用途**是让定时任务能指向它
//     （[core.ScheduleTarget] 的 code 必须是注册表里认得的）。它**不是**又写了一遍
//     刷新实现 —— 那条链已经在 [app.RefreshAccountAuth]，能力层在
//     [RefreshAuth]，这里只是给调度一个可寻址的名字。
var registry = []Capability{
	{
		Code:               CodeSign,
		Title:              "每日签到",
		Group:              core.GroupDaily,
		Kind:               core.KindAction,
		Description:        "移动云盘每日签到并领取奖励",
		RunInBatch:         true,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		// 日型：绑的是"今天这一趟"。服务停到下午才起来，补跑仍然达成目的。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           noOutput,
	},
	{
		Code:               CodeExchange,
		Title:              "兑换奖品",
		Group:              core.GroupExchange,
		Kind:               core.KindAction,
		Description:        "消耗云朵兑换奖品（独立页面运行）",
		Hidden:             true,
		Risk:               core.RiskHigh, // 不可逆消耗云朵
		DefaultAccountMode: core.ModeSequential,
		// 准点型：窗口就是那个时刻本身，过了再打只会白扣云朵。
		MissPolicyOnMiss: missPolicyPtr(core.MissSkip),
		Params:           exchangeParams,
		Output:           exchangeOutput,
	},
	{
		Code:               CodeCloudSnapshot,
		Title:              "云朵余额快照",
		Group:              core.GroupQuery,
		Kind:               core.KindQuery, // 无副作用，可反复执行
		Description:        "采集账号云朵余额并写入日快照",
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		// 日型，而且是**查询**：补跑的代价只是多一次读，不消耗任何东西。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           cloudSnapshotOutput,
	},
	{
		Code:        CodeLiveRoom,
		Title:       "直播口令 → 小红花",
		Group:       core.GroupActivity,
		Kind:        core.KindAction,
		Description: "进直播间收口令并抽小红花（仅在主播开播时有口令可用）",
		// ⚠ **不进组合编排**：这条链的可用窗口是"主播开播的那一小时"，
		// 而组合编排是"每天固定时间跑一遍"的语义。把它塞进日常编排的结果
		// 是每天白连一次 WS、听 60 秒、一个口令都收不到 —— 而日志里
		// 看起来像"今天也没开播"，看不出是编排本身就不该包含它。
		RunInBatch: false,
		// ⚠ `draw` 是**不可逆**的写操作（口令投出去就没了），
		// 且它的失败模式是"投错口令白跑一趟"。要求二次确认。
		Risk:               core.RiskHigh,
		DefaultAccountMode: core.ModeSequential,
		// 准点型：主播那一场结束了就是结束了，补跑等于白连一次 WS。
		MissPolicyOnMiss: missPolicyPtr(core.MissSkip),
		Params:           liveRoomParams,
		Output:           liveRoomOutput,
	},
	{
		Code:        CodeAuthRefresh,
		Title:       "刷新 auth 凭据",
		Group:       core.GroupMaintenance,
		Kind:        core.KindAction,
		Description: "凭据快到提前量时换新 token（到点才换，检查本身零上游请求）",
		// 从通用任务列表里**隐藏**，理由与 `exchange` 同形（它有自己的入口：
		// 账号页那一枚按钮 + `mcloud auth refresh`），但这里更具体一层：
		// 通用列表那枚"立即执行"在用户心里是"**现在就换**"，而这条链对外承诺的
		// 恰好相反 —— **到点才换，没到点一行上游都不打**。同一枚按钮上两个语义，
		// 而其中一个是用户不想要的（他按了却没换，只看到"还没到提前量"）。
		//
		// ⚠ 隐藏 ≠ 不可寻址：定时任务的 `target.code` 走 [Get]，不看 `Hidden`。
		Hidden: true,
		// ⚠ `RunInBatch: false` 与上面 `live-room` 是同一条理由的另一面：
		// 组合编排是"每天固定跑一遍"的语义，把"到期才换"塞进日常编排，
		// 表现是绝大多数日子它在空跑（判 `not-yet` 就走），偶尔换一次 ——
		// 而换的那一次是不是需要，编排本身不知道。它该是**一行定时任务**，
		// 不是组合里的一步。
		RunInBatch: false,
		// ⚠ **Medium 而不是 High**，判据是"要不要二次确认"这一条（[core.RiskLevel.NeedsConfirmation]
		// 只对 High 为真）：High 在现有四族里给的是**花掉不可逆额度**的那两件事
		// （兑换扣云朵、口令投出去）。刷新不消耗额度，最坏结果是那套凭据要重新登录一次
		// —— 可恢复。
		//
		// ⚠ 但这**不等于**它的后果已知："换下去的旧串还能不能用、会不会影响别的端
		// 正在用的会话"至今零次真上游实测（`docs/auth-refresh.md` §8）。
		// 处置只能是"不写成断言 + 登记成待验证"，⛔ 不是"给个 High 让界面弹一次框" ——
		// 弹框挡不住一个没人实测过的行为；也⛔ 不许把它读成"这一枪别打"（旧项目把刷新
		// 挂在每轮任务收尾自动做，见 `docs/auth-refresh.md` §1）。
		Risk:               core.RiskMedium,
		DefaultAccountMode: core.ModeSequential,
		// 日型：它绑的是"这次凭据到期"，晚几小时换仍然达成目的。
		// 补跑会"在没人看着的时刻换 token"，而这正是旧项目的常态（每轮收尾自动刷，
		// 本来没人在场）。另一条选 catch_up 的理由是"到期不换会更糟"—— ⚠ 那半句
		// "凭据真过期 ⇒ 那个账号所有能力都跑不动、只能人工重登"是**选型时的推断**，
		// 上游到期后回什么零实测样本（`docs/auth-refresh.md` §8），按推断对待。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           authRefreshParams,
		Output:           authRefreshOutput,
	},
	// ── 签到页那一条会话上的五个开关（2026-09-23 接线批）──────────────────────
	//
	// 五条都**不新增上游接口**：判据、请求与解析早已在 `internal/app` 与
	// `internal/upstream` 里，这里给的是"界面上点得到、定时任务指得来"的那张门牌。
	{
		Code:        CodeReceive,
		Title:       "领取云朵",
		Group:       core.GroupDaily,
		Kind:        core.KindAction,
		Description: "只把账上挂着的云朵领掉，不签到（签到已附带领取，这一条用于补漏）",
		// 领的是**当期挂在账上的**东西，不是不可逆额度：错过一次不会少一块钱，
		// 下一趟照领 ⇒ 可进日常编排，也让"签到那趟领取回 615"能被单独补一次。
		RunInBatch:         true,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		// 日型：绑的是"今天账上该领的"，晚几小时领仍然达成目的。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           receiveOutput,
	},
	{
		Code:        CodeMultiple,
		Title:       "签到翻倍",
		Group:       core.GroupDaily,
		Kind:        core.KindAction,
		Description: "领每月的签到翻倍机会（需本月已开启自动备份，门槛不成立时一个请求都不发）",
		// 机会是**每月一次**，但"这一趟该不该发"由 `infoV3` 的两个门槛字段判
		// （见 [MultiplePlan]），所以每天跑一次是安全的：不成立就什么都不发。
		// 正因如此它才**进**编排 —— 用户不必自己记得"哪天去点"。
		RunInBatch: true,
		// ⚠ **Medium 而不是 High**：High 的判据是"花掉不可逆额度 ⇒ 必须弹框"，
		// 而这一条在**门槛不成立时根本不发请求**，弹框挡不住任何事；
		// 门槛成立时领它就是目的本身。
		// 但它确实**一次用掉一个月的机会**，所以也不给 Low。
		// ⚠ "先签到再领翻倍"的先后顺序影响领到的倍数 —— 该顺序**未实测**。
		Risk:               core.RiskMedium,
		DefaultAccountMode: core.ModeSequential,
		// 日型（补跑仍达成目的）：机会本月内都有效，晚几小时不是错过。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           claimOutput,
	},
	{
		Code:               CodeBackupGift,
		Title:              "备份好礼",
		Group:              core.GroupDaily,
		Kind:               core.KindAction,
		Description:        "领每月的备份好礼（与签到翻倍互相独立，同样需本月开启自动备份）",
		RunInBatch:         true,
		Risk:               core.RiskMedium,
		DefaultAccountMode: core.ModeSequential,
		MissPolicyOnMiss:   missPolicyPtr(core.MissCatchUp),
		Params:             accountSelector,
		Output:             claimOutput,
	},
	{
		Code:        CodeRevival,
		Title:       "用复活卡 / 回馈卡",
		Group:       core.GroupActivity,
		Kind:        core.KindAction,
		Description: "用掉一张卡换回失效云朵（**卡用掉就没了，无法补发**）",
		// ⛔ 不进编排：上游**没有**"还剩几张卡"的查询口，所以这一条**没有门槛可查** ——
		// 每天自动跑一次 = 每天用掉一张卡。旧项目与参考实现都是在本地记当月额度，
		// 而本地那张表**还没建**（要动 schema 与 StateStore，已登记成待用户拍板的问题）。
		// 所以现在的唯一防线就是"只有人手工点、且点之前弹一次框"。
		RunInBatch:         false,
		Risk:               core.RiskHigh,
		DefaultAccountMode: core.ModeSequential,
		// 准点型（同 `exchange` 的取态）：一次不可逆消耗，没人在场时不替人授权第二次。
		MissPolicyOnMiss: missPolicyPtr(core.MissSkip),
		Params:           accountSelector,
		Output:           claimOutput,
	},
	{
		Code:        CodePrizes,
		Title:       "待领取奖品",
		Group:       core.GroupQuery,
		Kind:        core.KindQuery, // 只读：列出来，不领
		Description: "查有没有待领取的奖品（奖品会过期；本工具**只列不领**）",
		// 查询型一律不进编排（与 `cloud-snapshot` 同一取态）。
		RunInBatch:         false,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		MissPolicyOnMiss:   missPolicyPtr(core.MissCatchUp),
		Params:             accountSelector,
		Output:             prizeListOutput,
	},
	// ── 新链路批（2026-09-23）：四条各自带来新的上游接口 ──────────────────────
	//
	// 与上面那一组相反，这四条**各自都带来了新的上游接口**（`taskListV3` /
	// `openEmailsms/*` / `msgPushOn/*` / `funai/api/*`），证据与"哪些仍是待验证"
	// 逐条写在 `internal/upstream/app_*.go` 的文件头。
	{
		Code:        CodeTaskList,
		Title:       "任务清单",
		Group:       core.GroupQuery,
		Kind:        core.KindQuery, // 只读：列出来，不做任务
		Description: "看这个账号有哪些任务、做没做（本工具**只查不做**，做任务要逐条接线）",
		// 查询型不进编排（与 `prizes` / `cloud-snapshot` 同一取态）：它不改变任何状态，
		// 自动跑它只是每天多打一次读请求。
		RunInBatch:         false,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		MissPolicyOnMiss:   missPolicyPtr(core.MissCatchUp),
		Params:             accountSelector,
		Output:             taskListOutput,
	},
	{
		Code:  CodeAppTask,
		Title: "App 任务（做任务）",
		Group: core.GroupDaily,
		Kind:  core.KindAction,
		Description: "把 App 任务矩阵里能做的做掉：逐条 `task/click` 上报，" +
			"缺端点族的那些**明说不做**（会写用户的账，且不可撤销）",
		// ⛔ 不进编排（`RunInBatch: false`）—— 这一条与 `notice` 那条的区别在这一点：
		// `notice` 的写请求由查询读数**判出来**（不该领时一个请求都不发），
		// 而这一条只要表里有待做任务就**必然**发写请求，且今天一次真打都没有
		// （`task/click` 有实拍、`mrp doTask` 那一发零实拍）。
		// 让它每天自动跑，等于拿用户的账号替一个未验证的节奏打卡。
		// 手工点一次是拿到第一个实拍的最快路径，与 `msg-push` 同一处置。
		RunInBatch: false,
		// High：它一次跑动可以发十几枪写请求，而"这条任务上游认不认"我们只能靠
		// 第二趟复查读数推断（`currstep` / `process` 的口径至今未验证）。
		Risk:               core.RiskHigh,
		DefaultAccountMode: core.ModeSequential,
		MissPolicyOnMiss:   missPolicyPtr(core.MissSkip),
		Params:             accountSelector,
		Output:             appTaskOutput,
	},
	{
		Code:        CodeNotice,
		Title:       "开启通知领云朵",
		Group:       core.GroupDaily,
		Kind:        core.KindAction,
		Description: "查「开启邮箱/短信通知」两档任务的状态，该领时领掉云朵（领完回查一次）",
		// 进编排是安全的：**发不发写请求由查询读数判**，两槽都已领时一个请求都不发。
		// ⚠ 但那条映射（`cloudStatus` 哪个值是"未领"）是**猜的**，两档实拍没给方向
		// （`notice_run.go` 里那两个常量旁边写清了怎么定它）—— 猜反的表现是**漏领**，
		// 不是"白发一次写请求"，所以不因此收回 `RunInBatch`。
		RunInBatch: true,
		// Medium：确实会发一笔进账请求，但额度是"开通知"这一件一次性的事，
		// 判据不成立时根本不发。
		Risk:               core.RiskMedium,
		DefaultAccountMode: core.ModeSequential,
		// 日型：云朵挂在账上不会因晚几小时就没了。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           noticeOutput,
	},
	{
		Code:        CodeMsgPush,
		Title:       "消息推送任务",
		Group:       core.GroupActivity,
		Kind:        core.KindAction,
		Description: "查消息推送任务的第二次奖励，`secondTaskStatus == 2` 才领（**旧接口，尚无抓包样本**）",
		// ⛔ 不进编排：这一族的 URL / 方法 / 字段名**全部**来自
		// `docs/business-feature-inventory.md` §2.3 对旧 TS 的文字描述，
		// `test/fixtures/http/` 里一条 `msgPushOn` 都没有 ⇒ 接口层按"未验证"对待。
		// 挂在编排里等于每天替一个可能已下线的端点产生一条失败历史；
		// 手工点一次就能看见真相（回 404 / 90001 与回 `code:0` 是两种明确答案）。
		RunInBatch: false,
		// ⚠ 这一条的 `Risk` 与"进不进编排"都是 **agent 侧默认（非用户要求）**：
		// 判据只有"零样本 ⇒ 不自动跑"这一条，等真跑过一次之后再定。
		Risk:               core.RiskMedium,
		DefaultAccountMode: core.ModeSequential,
		MissPolicyOnMiss:   missPolicyPtr(core.MissSkip),
		Params:             accountSelector,
		Output:             msgPushOutput,
	},
	{
		Code:        CodeFunAI,
		Title:       "趣玩AI抽奖",
		Group:       core.GroupActivity,
		Kind:        core.KindAction,
		Description: "趣玩AI活动抽一次（本工具**每天只抽 1 次**，与旧项目一致；模块 id 现取不写死）",
		// 进编排：要不要抽由 `page/getmodule` 的读数判（活动期、剩余次数、今天有没有抽过），
		// 判不了就**一个写请求都不发** ⇒ 每天挂一次不会花掉不该花的额度。
		RunInBatch: true,
		// Low：抽的是活动**白送**的机会，不消耗云朵、不可逆消耗的是"今天这一次"，
		// 而上限 1 次由 `internal/app.FunaiDailyDrawLimit` 钉住（用户拍板：对齐旧 TS，不去试探上游额度）。
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		// 日型：次数按自然日算，晚几小时抽仍然达成目的。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           funaiOutput,
	},
	// ── 红包派对（2026-09-24）：这一条**只有只读那一半** ──────────────────────
	//
	// 与上面那四条不同，它带来的不是"一个新端点"而是**一整族**（七条口 + 一套自己的
	// 签名鉴权），而今天挂在这个 code 下面的只有其中三条查询。
	// 剩下那四条（浏览 / 答题 / 装 APK / 上传）各自的代价见 `cloud_phone_run.go` 文件头；
	// 安装类属**待拍板 P3**（`docs/pending-decisions.md`），不是"还没来得及接"。
	{
		Code:        CodeCloudPhoneRedpack,
		Title:       "红包派对盘点",
		Group:       core.GroupQuery,
		Kind:        core.KindQuery, // 只读：看钱和清单，一件任务都不代做
		Description: "看这个号在红包派对里有多少可兑现金、还有哪些任务（**只盘点不代做**，装 APK 那一类要另行拍板）",
		// 查询型不进编排（与 `prizes` / `task-list` / `cloud-snapshot` 同一取态）。
		// 这一条还多一层理由：活动站那张票**没有失效判据**（"票过期了上游回什么"零样本，
		// 见 `cloud_phone_session.go` 文件头），所以挂上调度后遇到过期只能攒失败历史 ——
		// 签到那一条有 `90001` 可以刷一次重放，这一族今天没有等价物。
		RunInBatch:         false,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		// 日型：晚几小时看一眼仍然达成"看一眼"的目的，没有窗口概念。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           cloudPhoneRedpackOutput,
	},
	// ── 拍拍（2026-09-24）：同样**只有只读那一半** ─────────────────────────────
	//
	// 上面 `CodePlayAISpecial` 那段把"另外三口各自卡在哪"逐条列了，这里不重复。
	// 这一条与 `cloud-phone-redpack` 的**一处不同**值得写在读声明的人眼前：
	// 它借的是**签到页那张 `jwttoken`**（§1.12 临时态，待拍板 P5），
	// 所以它的会话失效自救走的是 `90001` 那一条已经被实拍证明的路 ——
	// 而红包派对那一族今天**没有**任何失效判据。
	{
		Code:        CodePlayAISpecial,
		Title:       "拍拍盘点",
		Group:       core.GroupQuery,
		Kind:        core.KindQuery, // 只读：看次数读数与任务表，抽奖与拍照都不做
		Description: "看这个号在拍拍活动里的任务表与抽奖次数读数（**只盘点不代做**：不做任务、不抽奖、不写网盘）",
		// 查询型不进编排（与 `prizes` / `task-list` / `cloud-phone-redpack` 同一取态）。
		// 这一条还多一层：这一期活动**还活着吗**没有证据（`marketName` 是夏日那一期，
		// 归档时间 2026-06-29），挂上调度只会每天产一条"可能已经下线的端点"的失败历史。
		RunInBatch:         false,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		// 日型：晚几小时看一眼仍然达成"看一眼"的目的。
		MissPolicyOnMiss: missPolicyPtr(core.MissCatchUp),
		Params:           accountSelector,
		Output:           playAISpecialOutput,
	},
	// ── AI 头像（2026-09-24）：这一条**接的是写请求** ──────────────────────────
	//
	// 与上面那条 `play-ai-special` 的对照：那一条只接查询是因为**门槛本身读不懂**
	// （`totalLotteryCount` 是"累计"还是"剩余"两说）；这一条接写，是因为它的门槛
	// 是上游直接报的一个次数读数 —— 而 ⚠ **那个读数的语义同样只来自旧代码的读法**，
	// 本仓零样本（见 `internal/upstream/app_ai_avatar.go` 文件头）。
	//
	// 代价也直接：`image/upload` 会在用户网盘里落两个文件，而清理那一族
	// （`recyclebin/batchTrash`）零实拍、没接（待拍板 P1）。所以：
	//   - `RunInBatch: false` —— 挂上调度等于每天往网盘攒十来个没人删的文件；
	//   - 手工点一次是本条唯一的使用方式，也是拿到**这一族第一个响应样本**的路径。
	{
		Code:               CodeAiAvatar,
		Title:              "AI 头像生成与抽奖",
		Group:              core.GroupActivity,
		Kind:               core.KindAction,
		Description:        "生成 5 张 AI 头像并抽完当天的次数（**会往你的网盘写文件**，清理那一族还没接，产物先留在网盘里）",
		RunInBatch:         false,
		Risk:               core.RiskMedium,
		DefaultAccountMode: core.ModeSequential,
		// 日型，但**不补跑**：这一条的产物落在用户网盘上、抽奖消耗的又是当天的机会，
		// 错过就让它过去（补跑是"替人授权一次额外执行"，见 [ResolveMissPolicy]）。
		MissPolicyOnMiss: missPolicyPtr(core.MissSkip),
		Params:           accountSelector,
		Output:           aiAvatarOutput,
	},
}

// All 返回全部能力声明的**副本**。
//
// ⚠ 返回副本而不是切片本身 / 指针，是 Go 侧新长出来的第一条约束：
// Rust 的 `REGISTRY` 是 `LazyLock<Vec<Capability>>`，取出来只能读
// （`&'static Capability`），而 Go 的切片与指针**都可写** ——
// 把注册表本身交出去，等于允许任何一个调用方改写"签到这个能力的声明"，
// 而下一个读到它的能力不会报错，只是拿到被改过的那一份。
// 注册表是**两个产物共用的唯一契约来源**，它必须只能被本包改。
//
// ⚠ `copy` 只挡得住**字段**，挡不住 [Capability.MissPolicyOnMiss] 这种指针字段 ——
// 浅拷贝之后两份共享同一个 `core.MissPolicy`，`*got.MissPolicyOnMiss = …`
// 改的就是注册表本体，而且比改字段更隐蔽（字段改完 `All()` 下一次还是旧值，
// 指针改完连本包自己 `Get` 出来都是新的）。所以逐条走 [declarationCopy]。
func All() []Capability {
	out := make([]Capability, 0, len(registry))
	for _, c := range registry {
		out = append(out, declarationCopy(c))
	}
	return out
}

// declarationCopy 是"交出去之前把指针字段各自拆一份"的唯一一处。
//
// ⚠ 以后给 [Capability] 加**任何**指针字段，都要在这里补一行。
// 漏了不会编译报错，也不会当场红 —— 表现是"某个能力的声明被别的调用方改了"，
// 那正是 [TestAllCannotBeUsedToMutateTheRegistry] 要挡的那类事故。
func declarationCopy(c Capability) Capability {
	if c.MissPolicyOnMiss != nil {
		c.MissPolicyOnMiss = missPolicyPtr(*c.MissPolicyOnMiss)
	}
	return c
}

// Get 按 code 查找能力。
//
// ⚠ 同样要过 [declarationCopy]：它是**调度与 HTTP 两面共用的取声明的入口**，
// 直接返回 `registry` 里那一份的话，调用方拿到的是指向注册表本体的指针。
func Get(code string) (Capability, bool) {
	for _, c := range registry {
		if c.Code == code {
			return declarationCopy(c), true
		}
	}
	return Capability{}, false
}

// ListVisible 返回可在通用任务列表中展示的能力（过滤掉 `Hidden`）。
func ListVisible() []Capability {
	out := make([]Capability, 0, len(registry))
	for _, c := range registry {
		if !c.Hidden {
			out = append(out, declarationCopy(c))
		}
	}
	return out
}

// ListBatchable 返回可参与组合编排的能力。
func ListBatchable() []Capability {
	out := make([]Capability, 0, len(registry))
	for _, c := range registry {
		if c.RunInBatch && !c.Hidden {
			out = append(out, declarationCopy(c))
		}
	}
	return out
}

func strPtr(s string) *string { return &s }

// missPolicyPtr 给注册表那条声明取指针（字段类型的原因见 [Capability.MissPolicyOnMiss]）。
//
// ⚠ 必须是**每次新建一个变量**：写成 `var shared = core.MissCatchUp` 然后
// 各处 `&shared`，会让"改一个能力的补跑策略"变成改全体 —— 而 [All] 返回的
// 是结构体浅拷贝，指针指向的那份仍是共享的。
func missPolicyPtr(p core.MissPolicy) *core.MissPolicy { return &p }
