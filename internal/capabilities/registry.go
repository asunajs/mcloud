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
// P4 会补齐全部 19 项；这里先登记 3 项以验证声明式形态成立。
//
// ⚠ 上面那句"先登记 3 项"是**迁移当时**的原文，现在共 **5** 项 —— 2026-09-22：
// 前 3 项（`sign` / `exchange` / `cloud-snapshot`）是**迁移对位**那一组，
// 与冻结基准的 code 集合 `diff` 为空；后 2 项是**新增能力**，
// **不是**迁移缺口 —— 它们那条链在原项目里是 **TS 实现**，
// `legacy/` 里没有对应 crate，**无基准可对**。别把 5 当成"基准里多出来了什么"。
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
