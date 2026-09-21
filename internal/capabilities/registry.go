// 能力声明与注册表。
//
// 对应 Rust 侧的 ../mcloud-rust/crates/mcloud-capabilities/src/lib.rs。

package capabilities

import (
	"encoding/json"

	"github.com/catlair/mcloud/internal/core"
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

	Params func() []core.FieldSpec
	Output func() []core.FieldSpec
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

// registry 是能力注册表。
//
// 用显式列表而不是"注册函数 + init()" —— 可 grep、可 diff，新增能力只改这一处。
// P4 会补齐全部 19 项；这里先登记 3 项以验证声明式形态成立。
var registry = []Capability{
	{
		Code:               "sign",
		Title:              "每日签到",
		Group:              core.GroupDaily,
		Kind:               core.KindAction,
		Description:        "移动云盘每日签到并领取奖励",
		RunInBatch:         true,
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		Params:             accountSelector,
		Output:             noOutput,
	},
	{
		Code:               "exchange",
		Title:              "兑换奖品",
		Group:              core.GroupExchange,
		Kind:               core.KindAction,
		Description:        "消耗云朵兑换奖品（独立页面运行）",
		Hidden:             true,
		Risk:               core.RiskHigh, // 不可逆消耗云朵
		DefaultAccountMode: core.ModeSequential,
		Params:             exchangeParams,
		Output:             exchangeOutput,
	},
	{
		Code:               "cloud-snapshot",
		Title:              "云朵余额快照",
		Group:              core.GroupQuery,
		Kind:               core.KindQuery, // 无副作用，可反复执行
		Description:        "采集账号云朵余额并写入日快照",
		Risk:               core.RiskLow,
		DefaultAccountMode: core.ModeSequential,
		Params:             accountSelector,
		Output:             cloudSnapshotOutput,
	},
}

// All 返回全部能力声明的**副本**。
//
// ⚠ 返回副本而不是切片本身 / 指针，是 Go 侧新长出来的一条约束：
// Rust 的 `REGISTRY` 是 `LazyLock<Vec<Capability>>`，取出来只能读
// （`&'static Capability`），而 Go 的切片与指针**都可写** ——
// 把注册表本身交出去，等于允许任何一个调用方改写"签到这个能力的声明"，
// 而下一个读到它的能力不会报错，只是拿到被改过的那一份。
// 注册表是**两个产物共用的唯一契约来源**，它必须只能被本包改。
func All() []Capability {
	out := make([]Capability, len(registry))
	copy(out, registry)
	return out
}

// Get 按 code 查找能力。
func Get(code string) (Capability, bool) {
	for _, c := range registry {
		if c.Code == code {
			return c, true
		}
	}
	return Capability{}, false
}

// ListVisible 返回可在通用任务列表中展示的能力（过滤掉 `Hidden`）。
func ListVisible() []Capability {
	out := make([]Capability, 0, len(registry))
	for _, c := range registry {
		if !c.Hidden {
			out = append(out, c)
		}
	}
	return out
}

// ListBatchable 返回可参与组合编排的能力。
func ListBatchable() []Capability {
	out := make([]Capability, 0, len(registry))
	for _, c := range registry {
		if c.RunInBatch && !c.Hidden {
			out = append(out, c)
		}
	}
	return out
}

func strPtr(s string) *string { return &s }
