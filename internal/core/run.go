// 运行语义：`RunID`、运行结果四态、"需人工"的结构化出口，以及运行事件流。
// 对应 Rust 侧 legacy/crates/mcloud-core/src/run.rs。
//
// # 为什么单独成模块
//
// 这四条语义（`FieldSpec` 参数 / 三类错误 / `runId` 幂等 / `ManualRequired`）
// 是所有 19 项能力的**公共判据**。旧项目把它们各自散在 6+ 个文件里，
// 结果是同一个概念有 3 份实现、且互相矛盾（详见 docs/business-feature-inventory.md §1）。
// 这里钉成唯一来源，能力层只引用、不重定义。
//
// # 与 [RunStatus] 的分工
//
// [RunStatus] 是**存储层的生命周期**：Running → Done / Failed，
// 它回答"这一行还在跑吗"。而 [RunOutcome] 是**业务层的终局判定**，
// 它回答"这一趟到底算成没成、要不要叫人"。
//
// 两者**不是**一对一的：Done 可以对应三种结果（全成 / 部分成 / 需人工），
// 把它们压成"成功"正是旧项目最要命的失真 ——
// `logger.info("需手动完成: …")` 之后仍然 `return true`，
// 于是前端与推送都以为一切正常（docs/business-feature-inventory.md §1.4）。
//
//	RunStatus.Running ──┬── Done   ── RunOutcome.{Success, PartialSuccess, ManualRequired}
//	                    └── Failed ── RunOutcome.Failed
//
// # ⚠ Go 侧多出来的一条：枚举漏态不再有编译错误
//
// Rust 用 `enum` + `match`，"新增一个态、某个分支没处理"是**编译错误**；
// Go 的 `switch` 漏一个分支不报错，会静默走 default。
//
// 本模块的四个枚举（[RunOutcome] / [ManualTarget] / [LogLevel] / [RunStatus]）
// 因此在 run_test.go 里各配一张**真值表**守卫：
// "已知态的数量"由 `String()` 对越界值返回空串数出来，
// 与表长不等就红。新增态时要么补表、要么改 String()，两者不一致就暴露。
// 这是方案 §9 里点名"最该补装 linter"的那一条，在没装 linter 之前先用表兜住。
package core

import (
	"encoding/json"
	"fmt"
	"math"
	"strings"
)

// ─────────────────────────────────────────────────────────────────────────────
// RunID
// ─────────────────────────────────────────────────────────────────────────────

// RunID 是一次运行的幂等标识（UUID v4）。
//
// # 为什么用 newtype 而不是裸 string
//
// `runId` 是要**落库当主键**、还要进 URL（`GET /api/runs/:id/events`）的东西。
// 用 newtype 挡住两种很容易犯的错：把「账号 id」或「能力 code」误传进需要 runId 的位置 ——
// 这三者在旧项目里都是字符串，编译器分不出来。
//
// ⚠ 构造只走 [NewRunID]（从注入的 [Random] 取），没有 `From<string>`：
// 从外部字符串进来的东西应当走 [ParseRunID] 做形状校验，
// 否则一个手写的 "abc" 也能当 runId 用，幂等闸就形同虚设。
type RunID string

// NewRunID 生成一个新的运行标识。
//
// 随机源从 [Random] 注入 —— 与 [NewSessionToken] 同一个理由：
// 测试里要能钉住"同一个 runId 重放"这条语义，直接取全局随机源就做不到。
func NewRunID(rng Random) RunID { return RunID(UUIDv4(rng)) }

// ParseRunID 从已存库的字符串还原。
//
// **只做形状校验，不做语义校验** —— 是否真的存在这个 run 由存储层回答。
// 形状按 UUID v4 的规范形式（36 字符、4 个连字符），
// 这样 parse 失败就意味着"这串东西不可能是我们生成的"，
// 调用方可以直接判 404，不必再去查库。
func ParseRunID(raw string) (RunID, bool) {
	if !isUUIDv4Shape(raw) {
		return "", false
	}
	return RunID(strings.ToLower(raw)), true
}

// String 返回底层字符串（实现 fmt.Stringer）。
func (r RunID) String() string { return string(r) }

// isUUIDv4Shape 校验 UUID v4 的规范字符串形状：`8-4-4-4-12` 十六进制，版本位为 `4`。
//
// 只认小写与大写混排的十六进制（比较时统一转小写），
// **不接受** braced / urn / 无连字符等变体 —— 那些我们从不生成，
// 放宽只会让"这不是我们的 id"这类判断失效。
//
// ⚠ 按**字节**比较而不是 rune，与 Rust 的 `s.as_bytes()` 同口径。
func isUUIDv4Shape(s string) bool {
	if len(s) != 36 {
		return false
	}
	for i := 0; i < 36; i++ {
		c := s[i]
		switch i {
		case 8, 13, 18, 23:
			if c != '-' {
				return false
			}
		default:
			if !isASCIIHexDigit(c) {
				return false
			}
		}
	}
	// 版本位固定为 4（与 [UUIDv4] 里 `b[6] = ..| 0x40` 一致）。
	return s[14] == '4'
}

func isASCIIHexDigit(c byte) bool {
	return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')
}

// ─────────────────────────────────────────────────────────────────────────────
// RunStatus
// ─────────────────────────────────────────────────────────────────────────────

// RunStatus 是**存储层**的生命周期三态。
//
// ⚠ 它属于 Rust 侧 `model.rs`，是**提前带过来**的 —— [RunOutcome.FromLegacyStatus]
// 与 [RunOutcome.ToLegacyStatus] 两个映射依赖它。
// `model.rs` 全量迁移时从这里取，别再写第二份。
//
// 它回答的是"这一行还在跑吗"，**不是**"这一趟成了没" ——
// 后者是 [RunOutcome] 的事，两者不要混成一个字段。
type RunStatus int

const (
	// StatusRunning 表示还在跑。
	StatusRunning RunStatus = iota
	// StatusDone 表示跑完了。**不表示成功了** ——
	// 需人工介入的那一趟确实跑完了，只是目标没达成。
	StatusDone
	// StatusFailed 表示彻底失败。
	StatusFailed
)

// String 返回落库与 JSON 用的文本（snake_case）。
//
// ⚠ 未知值返回**空串**：这是"枚举漏态守卫"数出已知态数量的手段，见包注释。
func (s RunStatus) String() string {
	switch s {
	case StatusRunning:
		return "running"
	case StatusDone:
		return "done"
	case StatusFailed:
		return "failed"
	}
	return ""
}

// ParseRunStatus 从存储里读回。未知值一律归零值（Running）——
// "还在跑"是最保守的读法：它不会把一条真实在跑的记录误判成已结束。
func ParseRunStatus(raw string) RunStatus {
	switch raw {
	case "running":
		return StatusRunning
	case "done":
		return StatusDone
	case "failed":
		return StatusFailed
	}
	return StatusRunning
}

// IsTerminal 报告这一行是否已经不再变化。只有 [StatusRunning] 还会变。
//
// ⚠ 与 [RunOutcome.IsSuccessLike] 不是一回事：这里是"还要不要再盯"，
// 那里是"盯完了算不算成"。Done 是终态，但对应的 outcome 仍有三种。
func (s RunStatus) IsTerminal() bool { return s != StatusRunning }

// ─────────────────────────────────────────────────────────────────────────────
// RunOutcome
// ─────────────────────────────────────────────────────────────────────────────

// RunOutcome 是运行的**终局判定**。
//
// 四态是业务结论，不是错误分类：[OutcomeManualRequired] 与 [OutcomePartialSuccess]
// 都不是失败。它们在旧项目里被压成"成功 + 一条 info 日志"，
// 于是没人知道该去 App 里点一下。
type RunOutcome int

const (
	// OutcomeSuccess 表示全部目标达成。
	OutcomeSuccess RunOutcome = iota
	// OutcomePartialSuccess 表示有目标没达成，但**没有需要人做的事** ——
	// 重跑或等下一轮即可。典型：一个账号成功、另一个风控退避。
	OutcomePartialSuccess
	// OutcomeManualRequired 表示上游要求**人去客户端操作**才能继续，程序无法代劳。
	//
	// 典型：`pushOn === 0`（要先去 App 打开消息通知）、`未开启备份`。
	// 这一态**必须**能进推送与历史 —— 否则"需要你动一下手"这件事就永远传不到人。
	OutcomeManualRequired
	// OutcomeFailed 表示彻底失败。与 [OutcomePartialSuccess] 的区别是"一个都没成"。
	OutcomeFailed
)

// String 返回落库与 JSON 用的文本（snake_case）。未知值返回空串。
func (o RunOutcome) String() string {
	switch o {
	case OutcomeSuccess:
		return "success"
	case OutcomePartialSuccess:
		return "partial_success"
	case OutcomeManualRequired:
		return "manual_required"
	case OutcomeFailed:
		return "failed"
	}
	return ""
}

// ParseRunOutcome 从存储里读回。未知值归零值（Success）。
func ParseRunOutcome(raw string) RunOutcome {
	switch raw {
	case "success":
		return OutcomeSuccess
	case "partial_success":
		return OutcomePartialSuccess
	case "manual_required":
		return OutcomeManualRequired
	case "failed":
		return OutcomeFailed
	}
	return OutcomeSuccess
}

// IsSuccessLike 报告是否算"没白跑"（部分成功也算有产出）。
//
// 注意 [OutcomeManualRequired] **不算成功** —— 它意味着目标尚未达成，
// 只是再跑一遍也没用。把它算成功正是旧项目的失真点。
func (o RunOutcome) IsSuccessLike() bool {
	return o == OutcomeSuccess || o == OutcomePartialSuccess
}

// NeedsNotification 报告是否需要通知到人（进推送队列）。
//
// PartialSuccess 也返回 true：部分失败同样值得人知道，
// 但**不需要人动手** —— 用 [RunOutcome.NeedsHumanAction] 区分这两件事。
func (o RunOutcome) NeedsNotification() bool { return o != OutcomeSuccess }

// NeedsHumanAction 报告是否必须由人去客户端操作才能推进。
func (o RunOutcome) NeedsHumanAction() bool { return o == OutcomeManualRequired }

// CombineOutcomes 把**多个账号**的结果合成一次运行的总判定。
//
// 优先级从"最需要人管"往下排：
//
//  1. 只要有一个账号要人动手 → [OutcomeManualRequired]
//     —— 再跑一遍也没用，必须先有人去客户端；
//  2. 全部成功 → [OutcomeSuccess]；
//  3. 一个都没成 → [OutcomeFailed]；
//  4. 其余 → [OutcomePartialSuccess]。
//
// # 两个刻意的选择
//
//   - **ManualRequired 优先于 Failed**：两种都有时返回前者。
//     后者重跑可能就好了，前者重跑一定不好 —— 报告里更该突出"要人动手"。
//     具体哪些账号失败没有丢：逐账号的结果在调用方手里（RunReport 切片）。
//   - **空集合是 Failed**：什么都没跑成不是成功。这也让"目标账号筛出来是空的"
//     这种情况**在总判定里就露出来**，而不是安静地报成功。
func CombineOutcomes(reports []RunReport) RunOutcome {
	for _, r := range reports {
		if r.Outcome.NeedsHumanAction() {
			return OutcomeManualRequired
		}
	}
	succeeded := 0
	for _, r := range reports {
		if r.Outcome.IsSuccessLike() {
			succeeded++
		}
	}
	if succeeded == 0 {
		return OutcomeFailed
	}
	if succeeded == len(reports) {
		return OutcomeSuccess
	}
	return OutcomePartialSuccess
}

// FromLegacyStatus 从 [RunStatus] 回推 —— 供存储层在只有 status 一列时的**退路**使用。
//
// 这是**有损**的：Done 无法区分三种成功态，所以只用于读**旧数据**。
// 新写入一律带完整 outcome，不要走这里。
func FromLegacyStatus(status RunStatus) RunOutcome {
	if status == StatusFailed {
		return OutcomeFailed
	}
	return OutcomeSuccess
}

// ToLegacyStatus 写回 [RunStatus] —— 存储层的生命周期列。
//
// [StatusDone] 表示"跑完了"，**不表示"成了"**，所以只有 [OutcomeFailed]
// 映射到 [StatusFailed]：需人工介入的那一趟确实跑完了，只是目标没达成 ——
// 把它写成 Failed 会让历史里出现一堆"失败"，而它们其实只是在等用户。
func (o RunOutcome) ToLegacyStatus() RunStatus {
	if o == OutcomeFailed {
		return StatusFailed
	}
	return StatusDone
}

// ─────────────────────────────────────────────────────────────────────────────
// 需人工
// ─────────────────────────────────────────────────────────────────────────────

// ManualTarget 是需要人工介入的地方。
//
// 单独成枚举而不是塞进文案：以后 WebUI 要按这个分出"去 App 的引导"与
// "去网页的引导"，靠 instruction 里的字符串做前缀匹配太脆。
type ManualTarget int

const (
	// TargetMobileApp 是移动云盘 App。
	TargetMobileApp ManualTarget = iota
	// TargetWeb 是移动云盘网页端。
	TargetWeb
	// TargetWebui 是本系统的 WebUI（如"去设置里补手机号"）。
	TargetWebui
	// TargetCli 是本系统的 CLI。
	TargetCli
	// TargetUnspecified 表示上游未指明，或我们没归类出来。
	//
	// **刻意保留**：抓包与旧文案里总会有归不进去的情形，
	// 强行归类会把"我们没搞清"伪装成"我们知道"。
	TargetUnspecified
)

// String 返回落库与 JSON 用的文本（snake_case）。未知值返回空串。
func (t ManualTarget) String() string {
	switch t {
	case TargetMobileApp:
		return "mobile_app"
	case TargetWeb:
		return "web"
	case TargetWebui:
		return "webui"
	case TargetCli:
		return "cli"
	case TargetUnspecified:
		return "unspecified"
	}
	return ""
}

// ParseManualTarget 从存储里读回。未知值归 [TargetUnspecified] ——
// 与枚举里那条"刻意保留"同源：读不出来的就承认读不出来，不要伪装成别的。
func ParseManualTarget(raw string) ManualTarget {
	switch raw {
	case "mobile_app":
		return TargetMobileApp
	case "web":
		return TargetWeb
	case "webui":
		return TargetWebui
	case "cli":
		return TargetCli
	}
	return TargetUnspecified
}

// ManualStep 是一条"需要人去客户端做的事"。
//
// # 为什么是一等类型而不是一句日志
//
// 旧项目用 `logger.info("需手动完成: …")` / `logger.error("…请前往 APP 手动打开")`
// 表达这件事，返回值仍是成功。后果有三层：
// ① 前端看不到；② 推送看不到（而推送正是用来喊人的）；③ 历史记录里查不出
// "我到底积了多少件没去做的事"。
//
// 所以这里把**结构化**的部分与**给人看**的部分分开：
// 前者供程序判断（Capability 可去重、Account 可归属），
// 后者供渲染（Instruction 是完整句子，Target 是"去哪做"）。
type ManualStep struct {
	// Capability 是能力 code（如 `msg-push`）—— 用于按能力去重与聚合。
	Capability string `json:"capability"`
	// Account 是账号 id（脱敏标识，**不是**凭据）。无账号上下文的任务为 nil。
	Account *AccountID `json:"account"`
	// Instruction 是一句话说明**要人去做什么**，完整句子，可直接进推送正文。
	Instruction string `json:"instruction"`
	// Target 是去哪做。旧文案里散落的"请前往 APP"归到这里，便于以后换端。
	Target ManualTarget `json:"target"`
}

// NewManualStep 构造一条待办。
func NewManualStep(capability string, account *AccountID, instruction string, target ManualTarget) ManualStep {
	return ManualStep{Capability: capability, Account: account, Instruction: instruction, Target: target}
}

// RunReport 是一次运行的完整结果。
//
// 把 [RunOutcome] 与它携带的细节打包，让"结论"和"依据"一起流动 ——
// 否则调用方拿到一个 ManualRequired 却不知道要人去做什么（旧项目的日志就是这个下场）。
type RunReport struct {
	Outcome RunOutcome `json:"outcome"`
	// ManualSteps 是需要人做的事（可能多件）。非 ManualRequired 时通常为空，
	// 但**允许非空** —— PartialSuccess 也可能夹带一件小事，强制清空会丢掉信息。
	//
	// ⚠ 空的时候是 `[]` 而不是 `null`：Rust 侧的 `Vec::new()` 序列化成 `[]`，
	// 而 Go 的 nil slice 会变成 `null` —— 前端 `steps.map(...)` 在 null 上直接炸。
	// 构造函数一律给非 nil，别用字面量绕开。
	ManualSteps []ManualStep `json:"manual_steps"`
	// Summary 是给人看的一句话小结。nil 表示"没有值得说的话"。
	//
	// ⚠ 不能用 `omitempty`：Rust 的 `Option<String>` 在 `None` 时输出 **`null`**
	// 而不是省略，两侧形状必须一致 —— 而 `omitempty` 会把 nil 变成"字段消失"，
	// 于是前端 `data.summary === null` 与 `!( 'summary' in data)` 分道扬镳。
	Summary *string `json:"summary"`
}

// NewRunReportSuccess 是单纯的成功，没有附带信息。
func NewRunReportSuccess() RunReport {
	return RunReport{Outcome: OutcomeSuccess, ManualSteps: []ManualStep{}, Summary: nil}
}

// NewRunReportFailed 是一次彻底的失败。
func NewRunReportFailed(summary string) RunReport {
	return RunReport{Outcome: OutcomeFailed, ManualSteps: []ManualStep{}, Summary: &summary}
}

// NewRunReportManualRequired 造一个"需人工"的结果。
//
// `steps` 为空时**降级为 [OutcomePartialSuccess]**：一个"需人工"却不说什么事的
// 结果和旧项目那句没人看的日志是同一个东西，不能让它进库。
// 这是构造函数里的守卫，不是运行期断言 —— 把不可能的状态挡在构造之外。
func NewRunReportManualRequired(steps []ManualStep, summary string) RunReport {
	if len(steps) == 0 {
		return RunReport{Outcome: OutcomePartialSuccess, ManualSteps: []ManualStep{}, Summary: &summary}
	}
	return RunReport{Outcome: OutcomeManualRequired, ManualSteps: steps, Summary: &summary}
}

// MarshalJSON 走 [marshalJSON] 而不是默认的 `json.Marshal`。
//
// 同 [RunEvent.MarshalJSON] 的HTML 转义那条理由：RunReport 是要进
// `run_events.payload` 与推送正文的，默认转义会把 `<` `>` `&` 变成 `\u003c` 等，
// 而 Rust 的 `serde_json` 不转 —— 于是两侧的形状不一致。
//
// ⚠ 用 `type plain` 脱掉方法：直接在自己身上调 marshalJSON 会无限递归。
func (r RunReport) MarshalJSON() ([]byte, error) {
	type plain RunReport
	return marshalJSON(plain(r))
}

// Normalize 复核"只要有一条人工步骤，结论就不能是成功"。
//
// 这个不变式由 [NewRunReportManualRequired] 保证，这里再做一次组装后的复核 ——
// 因为 RunReport 的字段是导出的，可以被字面量直接构造绕过构造函数。
// 存储层与推送层在发送前调它，避免"自造的 Success + 人工步骤"漏出去。
func (r RunReport) Normalize() RunReport {
	if len(r.ManualSteps) > 0 && r.Outcome == OutcomeSuccess {
		r.Outcome = OutcomeManualRequired
	}
	return r
}

// ─────────────────────────────────────────────────────────────────────────────
// Seq
// ─────────────────────────────────────────────────────────────────────────────

// Seq 是运行事件的序号。
//
// 从 **1** 起单调递增，**0 是哨兵**：表示"这个 run 还没有发过任何事件"。
// 因此 0 永远不会作为一条事件的序号出现，这带来两个便利：
// 落库侧可以拿 0 当"没有下界"（不用为 NULL 单写一条 SQL），
// SSE 的 `Last-Event-ID` 缺失时也可以安心地按 0 处理。
//
// # 为什么要 newtype，而不是直接用 uint64
//
// 因为**"已看到"与"下一个要取"是两个不同的值**，而它们都是 uint64，
// 编译器分不出来。这个仓库里真的踩过：客户端报 3，
// 服务端照着"下一个要取"去查，于是从 5 开始发，**4 号被静默跳过**。
//
// 所以两个构造函数的名字就是为区分这件事而存在的：
//
//	| 想表达                    | 用哪个            | 内部值 |
//	| ---                      | ---              | ---   |
//	| "我从头开始"              | [SeqStart]       | 0     |
//	| "我已经看到 3 了"（要 4、5、…）| [SeqSeen](3)     | 3     |
//	| "我要取 4 之后的"（要 5、6、…） | [SeqResumeAfter](3) | 4  |
//
// ⚠ 注意后两行：SeqSeen(3) 与 SeqResumeAfter(3) 的**参数一样、结果差 1**。
// 凡是准备把 Seq 传给"从某处开始取"的接口，先想清楚手里这个是
// "客户端已经看到的值"还是"我们已经发到的值"，再选构造函数。
//
// # 饱和而不是回绕
//
// [Seq.Next] 在 [math.MaxUint64] 处停住而不是绕回 0。
// 绕回会让序号**变小**，于是续传的 `seq > after` 条件永远为假，
// 这个 run 从此再也发不出任何事件 —— 一个到不了、但代价是彻底静默的分支。
type Seq uint64

// SeqStart 表示"还没发过任何事件" —— 内部值 0。传它等于传"从头来"。
func SeqStart() Seq { return 0 }

// SeqSeen 表示"客户端说**我已经看到** seen 了"，下一个要给它的是 seen + 1。
//
// 这是 SSE 的 `Last-Event-ID` 那种语义，也是 `run_events_after` 的
// `after` 参数那种语义（SQL 里就是 `seq > seen`）。
func SeqSeen(seen uint64) Seq { return Seq(seen) }

// SeqResumeAfter 表示"我们已经发到 last，**下一个要取的**是 last + 1"。
//
// ⚠ 与 [SeqSeen] 差 1。传错会**静默跳过一条事件** ——
// 前端不会报错，只是进度条少一步；排查时几乎没人会怀疑到这里。
//
// 内部值饱和在 [math.MaxUint64]：SeqResumeAfter(MaxUint64) 与 SeqSeen(MaxUint64)
// 得到同一个值，也就是"已经到头了"。
func SeqResumeAfter(last uint64) Seq {
	if last == math.MaxUint64 {
		return Seq(last)
	}
	return Seq(last + 1)
}

// SeqFromStored 从库里存着的整数原样还原。
//
// 与 [SeqResumeAfter] 的区别就体现在这里：**存的是序号本身**，
// 不是"上一条"或"下一条"。所以直接按原值还原。
//
// 负数与 0 按 0 处理（SQLite 的 INTEGER 是有符号的，理论上能存进负数）——
// 落到 0 意味着"从头来"，是安全的退路；断言 panic 会让一条脏数据把整次回放打断。
func SeqFromStored(stored int64) Seq {
	if stored <= 0 {
		return 0
	}
	return Seq(stored)
}

// Next 取下一个序号并前进。见类型文档的"饱和而不是回绕"。
//
// ⚠ 是指针接收者：它要**推进自身**，不是只返回一个新值。
// 写成值接收者会让调用方以为自己在前进，实际一直拿到 1。
func (s *Seq) Next() Seq {
	if *s < math.MaxUint64 {
		*s++
	}
	return *s
}

// Value 返回内部值。落库、比较、拼 SQL 都用它。
func (s Seq) Value() uint64 { return uint64(s) }

// ─────────────────────────────────────────────────────────────────────────────
// LogLevel
// ─────────────────────────────────────────────────────────────────────────────

// LogLevel 是日志级别。
//
// 只有三档，且**不含** Debug/Trace：这套日志是给**用户**看的
// （前端面板、推送正文），不是给开发者调试的。多出来的档位只会
// 诱使调用方把内部细节写进去，而那些东西既不该进前端也不该进推送。
//
// ⚠ 变体顺序即严重程度，零值必须是 [LevelInfo] —— 与 [ErrorKind] 的排列规则相同。
type LogLevel int

const (
	// LevelInfo 是正常进展，用户看着安心用的。
	LevelInfo LogLevel = iota
	// LevelWarn 是没成功但不算失败，以及"要人去做点什么"的提示。
	LevelWarn
	// LevelError 是这一条出错了。注意"某一条出错"未必让整个 run 失败。
	LevelError
)

// String 返回稳定的小写名字。落库、进 JSON、进前端 class 名都用它。
// 未知值返回空串。
func (l LogLevel) String() string {
	switch l {
	case LevelInfo:
		return "info"
	case LevelWarn:
		return "warn"
	case LevelError:
		return "error"
	}
	return ""
}

// ParseLogLevel 从存储里读回。未知值归 [LevelInfo]。
func ParseLogLevel(raw string) LogLevel {
	switch raw {
	case "info":
		return LevelInfo
	case "warn":
		return LevelWarn
	case "error":
		return LevelError
	}
	return LevelInfo
}

// ─────────────────────────────────────────────────────────────────────────────
// RunEvent
// ─────────────────────────────────────────────────────────────────────────────

// RunEventKind 是运行事件的五类。
type RunEventKind int

const (
	// EventStarted 是开跑。
	EventStarted RunEventKind = iota
	// EventLog 是一条日志。
	EventLog
	// EventProgress 是进度。
	EventProgress
	// EventFinished 是终局之一：正常结算（**不等于成功**）。
	EventFinished
	// EventErrored 是终局之二：执行器自己崩了（还没到能力层做判定的时候）。
	//
	// 与 [EventFinished] 的区别是**没有 outcome** ——
	// 上游错误、panic 恢复、超时这些都没跑到"这一步成没成"的判断，
	// 硬塞一个 [OutcomeFailed] 会假装我们知道得比实际多。
	EventErrored
)

// String 返回 JSON 里的 `type` 标签，与库里的 `kind` 列**同源**。
// 未知值返回空串。
func (k RunEventKind) String() string {
	switch k {
	case EventStarted:
		return "started"
	case EventLog:
		return "log"
	case EventProgress:
		return "progress"
	case EventFinished:
		return "finished"
	case EventErrored:
		return "errored"
	}
	return ""
}

// RunEvent 是运行过程中的一条事件。
//
// # ⚠ Go 侧的形状取舍
//
// Rust 侧是 `#[serde(tag = "type")]` 的枚举 —— 五类事件的字段差别很大，
// 用一个结构体塞全部字段的话，大部分字段对大部分行是无意义的，
// 于是"Progress 一定有 total"这种不变式就没人保证了。
//
// Go 没有 sum type，这里**只能**用"结构体 + Kind 字段"来近似。
// 代价是编译期不再保证"只有 Progress 能带 total"。补法有两条，都用上了：
//  1. 构造只走 [NewEventStarted] / [NewEventLog] / … 这些具名函数，
//     每个只填自己那一类该有的字段；
//  2. JSON 按 Kind 分派（见 [RunEvent.MarshalJSON]），
//     所以对外形状**仍然**是 tag 式枚举，与 Rust 侧逐字一致 ——
//     **外部契约没有因为换了语言而变形**。
//
// # 字段名 `runId` 为什么是驼峰
//
// 其余字段是 snake_case，`runId` 保持旧项目的驼峰 —— 因为它已经出现在
// 前端的 API 契约与库里的历史数据里，改名会让"新旧数据混读"多一条分支。
// 这一条**不受**仓库的文件命名规则影响（那只约束文件名，见 docs/repo-layout.md）。
type RunEvent struct {
	Kind  RunEventKind `json:"-"`
	RunID RunID        `json:"-"`

	// Capability 是能力 code（如 `sign` / `msg-push`）。仅 [EventStarted] 用。
	Capability string `json:"-"`
	// Level 是日志级别。仅 [EventLog] 用。
	Level LogLevel `json:"-"`
	// Message 是日志正文或错误原因。[EventLog] 与 [EventErrored] 用。
	Message string `json:"-"`
	// Done / Total 是进度。仅 [EventProgress] 用。
	Done  uint32 `json:"-"`
	Total uint32 `json:"-"`
	// Outcome 与 ManualSteps / Summary 是终局判定。仅 [EventFinished] 用。
	Outcome     RunOutcome   `json:"-"`
	ManualSteps []ManualStep `json:"-"`
	Summary     *string      `json:"-"`
}

// NewEventStarted 构造"开跑"事件。
func NewEventStarted(runID RunID, capability string) RunEvent {
	return RunEvent{Kind: EventStarted, RunID: runID, Capability: capability}
}

// NewEventLog 构造"日志"事件。
func NewEventLog(runID RunID, level LogLevel, message string) RunEvent {
	return RunEvent{Kind: EventLog, RunID: runID, Level: level, Message: message}
}

// NewEventProgress 构造"进度"事件。
//
// 不做 `Done > Total` 的校验：调用方可能是"总步数后来才确定"的场景
// （先报到 3 再到 20）。真正要守的是 [RunEvent.Percent] 里的除零。
func NewEventProgress(runID RunID, done, total uint32) RunEvent {
	return RunEvent{Kind: EventProgress, RunID: runID, Done: done, Total: total}
}

// NewEventFinished 构造"正常结算"事件。
//
// 带 outcome 与完整 manual_steps：丢掉待办的话，
// 回看历史就只剩一句"需人工"，而"要人去做什么"恰恰是这条结论的全部价值。
func NewEventFinished(runID RunID, outcome RunOutcome, steps []ManualStep, summary *string) RunEvent {
	// 同 [RunReport.ManualSteps]：空是 `[]` 不是 null。
	if steps == nil {
		steps = []ManualStep{}
	}
	return RunEvent{
		Kind: EventFinished, RunID: runID, Outcome: outcome,
		ManualSteps: steps, Summary: summary,
	}
}

// NewEventErrored 构造"执行器出错"事件。
func NewEventErrored(runID RunID, message string) RunEvent {
	return RunEvent{Kind: EventErrored, RunID: runID, Message: message}
}

// EventKind 返回稳定的小写名字，与 JSON 里的 `type` 标签、库里的 `kind` 列**同源**。
//
// 库里的 `kind` 列由它决定（见 mcloud-io 的 `event_kind`），两者冗余但不漂移。
// 单独存一列是为了让"只看 errored"能走 SQL 的 `WHERE kind = ?`，不必把每个 payload 都解一遍。
func (e RunEvent) EventKind() string { return e.Kind.String() }

// IsTerminal 报告这是否是"这条 run 已经结束了"的事件。
//
// [EventErrored] 也算终局：执行器崩了，这个 run 不会再有后续事件。
func (e RunEvent) IsTerminal() bool {
	return e.Kind == EventFinished || e.Kind == EventErrored
}

// IsStarted 报告这是否是开场那条。
//
// 单独给一个方法而不是让调用方去 switch：前端第一次连上时
// 要区分"这个 run 还没开始"与"开始了但还没发日志"，两处都用它。
func (e RunEvent) IsStarted() bool { return e.Kind == EventStarted }

// EventOutcome 返回终局事件的业务判定。只有 [EventFinished] 有 ——
// [EventErrored] 返回 ok=false，理由见该变体的文档。
func (e RunEvent) EventOutcome() (RunOutcome, bool) {
	if e.Kind == EventFinished {
		return e.Outcome, true
	}
	return OutcomeSuccess, false
}

// Percent 返回进度百分比（0–100），只对 [EventProgress] 有值。
//
// `Total == 0` 返回 ok=false 而不是 0 或 100：
// 一个"总共 0 步"的进度条画出来是**满的**（任何比例都无意义，
// 前端多半会除零拿到 NaN，然后渲染成一个空条或全条）。
// 返回 false 让前端有机会写"暂无法估算"，比给它一个假数字好。
//
// 用整数算而不是浮点：百分比要进 JSON，`33.333333333333336` 这种值
// 在前端显示出来很难看，而且同样的输入在两端可能因为浮点细节不一致。
func (e RunEvent) Percent() (uint32, bool) {
	if e.Kind != EventProgress || e.Total == 0 {
		return 0, false
	}
	// 先乘后除，且在 uint64 里算：done 最大 MaxUint32，乘 100 后仍装得下。
	return uint32(uint64(e.Done) * 100 / uint64(e.Total)), true
}

// MarshalJSON 按 Kind 分派，产出与 Rust 的 `tag = "type"` **逐字相同**的 JSON：
//
//	{"type":"started","runId":"…","capability":"sign"}
//	{"type":"log","runId":"…","level":"warn","message":"…"}
//	{"type":"progress","runId":"…","done":3,"total":20}
//	{"type":"finished","runId":"…","outcome":"…","manual_steps":[…],"summary":…}
//	{"type":"errored","runId":"…","message":"…"}
//
// ⚠ 这个形状是**外部契约**：进 `run_events.payload`、进 SSE 的 `data:`、
// 前端直接 `switch (ev.type)`。所以每个变体只输出自己那一类该有的字段，
// **不能**用 `omitempty` 去近似 —— 那会让零值字段（`done:0`、
// `level:"info"`、`outcome:"success"`）从 payload 里消失。
//
// ⚠ 走 [marshalJSON] 而不是 `json.Marshal`：后者默认转义 HTML，
// 而 message 里可能有中文与引号。
func (e RunEvent) MarshalJSON() ([]byte, error) {
	switch e.Kind {
	case EventStarted:
		return marshalJSON(struct {
			Type       string `json:"type"`
			RunID      RunID  `json:"runId"`
			Capability string `json:"capability"`
		}{e.Kind.String(), e.RunID, e.Capability})
	case EventLog:
		return marshalJSON(struct {
			Type    string   `json:"type"`
			RunID   RunID    `json:"runId"`
			Level   LogLevel `json:"level"`
			Message string   `json:"message"`
		}{e.Kind.String(), e.RunID, e.Level, e.Message})
	case EventProgress:
		return marshalJSON(struct {
			Type  string `json:"type"`
			RunID RunID  `json:"runId"`
			Done  uint32 `json:"done"`
			Total uint32 `json:"total"`
		}{e.Kind.String(), e.RunID, e.Done, e.Total})
	case EventFinished:
		return marshalJSON(struct {
			Type        string       `json:"type"`
			RunID       RunID        `json:"runId"`
			Outcome     RunOutcome   `json:"outcome"`
			ManualSteps []ManualStep `json:"manual_steps"`
			Summary     *string      `json:"summary"`
		}{e.Kind.String(), e.RunID, e.Outcome, e.ManualSteps, e.Summary})
	case EventErrored:
		return marshalJSON(struct {
			Type    string `json:"type"`
			RunID   RunID  `json:"runId"`
			Message string `json:"message"`
		}{e.Kind.String(), e.RunID, e.Message})
	}
	return nil, fmt.Errorf("未知的事件类型 %d", int(e.Kind))
}

// UnmarshalJSON 按 `type` 分派，是 [RunEvent.MarshalJSON] 的逆。
//
// ⚠ `summary` 缺失与 `null` **都**落到 nil —— 与 Rust 的 `Option<String>` 同口径
// （serde 对 Option 字段允许缺失）。
func (e *RunEvent) UnmarshalJSON(data []byte) error {
	var probe struct {
		Type string `json:"type"`
	}
	if err := json.Unmarshal(data, &probe); err != nil {
		return err
	}

	switch probe.Type {
	case "started":
		var v struct {
			RunID      RunID  `json:"runId"`
			Capability string `json:"capability"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		e.Kind, e.RunID, e.Capability = EventStarted, v.RunID, v.Capability
		return nil
	case "log":
		var v struct {
			RunID   RunID    `json:"runId"`
			Level   LogLevel `json:"level"`
			Message string   `json:"message"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		e.Kind, e.RunID, e.Level, e.Message = EventLog, v.RunID, v.Level, v.Message
		return nil
	case "progress":
		var v struct {
			RunID RunID  `json:"runId"`
			Done  uint32 `json:"done"`
			Total uint32 `json:"total"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		e.Kind, e.RunID, e.Done, e.Total = EventProgress, v.RunID, v.Done, v.Total
		return nil
	case "finished":
		var v struct {
			RunID       RunID        `json:"runId"`
			Outcome     RunOutcome   `json:"outcome"`
			ManualSteps []ManualStep `json:"manual_steps"`
			Summary     *string      `json:"summary"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		e.Kind, e.RunID, e.Outcome, e.ManualSteps, e.Summary =
			EventFinished, v.RunID, v.Outcome, v.ManualSteps, v.Summary
		return nil
	case "errored":
		var v struct {
			RunID   RunID  `json:"runId"`
			Message string `json:"message"`
		}
		if err := json.Unmarshal(data, &v); err != nil {
			return err
		}
		e.Kind, e.RunID, e.Message = EventErrored, v.RunID, v.Message
		return nil
	}
	return fmt.Errorf("未知的事件类型 %q", probe.Type)
}

// ─────────────────────────────────────────────────────────────────────────────
// Sequenced
// ─────────────────────────────────────────────────────────────────────────────

// Sequenced 是带序号的事件 —— 落库与投递的最小单位。
//
// # 为什么把 Seq 与 RunEvent 绑在一起
//
// 因为它们必须**同批次确定**：序号由发布侧（EventBus）分配，
// 而分配之后到真正写库之间，进程可能崩。重启后重放同一批时，
// 如果序号是"写的时候再算"，重放就会得到**新的**序号 ——
// 于是同一个事件在库里出现两次、序号不同，客户端会看到重复进度。
//
// 绑在一起之后，"重放同一批"是幂等的：`(run_id, seq)` 是主键，
// 落库侧 `ON CONFLICT DO NOTHING` 就把它挡住了。
// 这不是纸上谈兵 —— 崩在"写完库、还没告诉总线"确实是**正常路径**。
type Sequenced struct {
	Seq   Seq      `json:"seq"`
	Event RunEvent `json:"event"`
}

// NewSequenced 绑定序号与事件。
func NewSequenced(seq Seq, event RunEvent) Sequenced { return Sequenced{Seq: seq, Event: event} }

// MarshalJSON 走 [marshalJSON]，理由同 [RunReport.MarshalJSON]。
//
// ⚠ 它是 SSE 的 `data:` 直接载荷，默认的 HTML 转义会让前端 `JSON.parse`
// 之后拿到一串 `\u003c` —— 解析回来是对的，但人眼与文本扫描都对不上。
func (s Sequenced) MarshalJSON() ([]byte, error) {
	type plain Sequenced
	return marshalJSON(plain(s))
}
