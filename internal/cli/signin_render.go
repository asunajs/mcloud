package cli

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// `mcloud signin` 的**渲染层**：一个账号这一趟的结果 → 人看的几行，或 `--json` 的那个对象。
//
// 对位 Rust 侧 ../mcloud-rust/crates/mcloud-cli/src/signin_output.rs，以及 signin_run.rs /
// signin_one.rs 里与输出有关的那几段。
//
// # 这一层只有数据进出
//
// 挑账号、调 app、写库、定退出码都在编排层；这里拿到的全是**已经算好的结果**，
// 函数一律是"入参 + [output]"，不碰库、不联网、不看时钟。于是每一条边界文案都能用
// 手搓的 [signinView] 直接测到 —— 不需要 fixture，也不需要脚本化上游。
//
// # 对齐只发生在边界上
//
// 逐字对齐的只有三样：给人看的文案、`--json` 的键名、以及"哪一行走哪个流"。
// 内部一律按 Go 的习惯来，例如 `signedDays` 用 `[]int` 而不是 `[]uint8`：
// 后者是 `[]byte` 的别名，encoding/json 会把它编成 base64 字符串 ——
// 那不是"更贴近上游"，那是另一份格式。

// signinMode 是一个账号这一趟**要做什么**。
//
// 它不参与任何判断（结果长什么样由 [signinView.report] / [signinView.status] /
// [signinView.raw] 决定），只决定"正在做什么"那行的措辞。
//
// 做成一个字段而不是从结果反推：**失败**的那一趟什么结果都没有，
// 而"正在做什么"那行本来就是发请求**之前**打的 —— 从结果反推不出来。
type signinMode int

const (
	// signinModeSignIn 是默认那条：签到。
	signinModeSignIn signinMode = iota
	// signinModeStatus 是 `--status-only`：只查状态，不发写请求。
	signinModeStatus
	// signinModeRaw 是 `--raw`：只取原始响应，不解析、不落库。
	signinModeRaw
)

// signinView 是一个账号这一趟的渲染输入。
//
// 字段一律不导出：编排层与本文件**同包**，用不上导出。顺带也就没有了
// "它会不会被当成对外 JSON 形状"这个问题 —— 它不会被序列化，
// 进 `--json` 的是 [signinStatusJSON]（形状不同，见那里的注释）。
type signinView struct {
	// account 是目标账号。标识一律取它的 ID（**已是掩码**，形如 `19****01`）——
	// 完整手机号不在这个结构里，也不该进来：一旦进来，"不打印完整号码"就只剩注释在保证。
	account core.Account
	// index 是 `--list` 打出来的下标（从 1 起）。0 = 没有下标（`--phone` 定位的账号），
	// 那时不打下标 —— 打一个 0 出来只会让人以为它是第一个账号。
	index int
	// mode 是这一趟要做什么。
	mode signinMode
	// report 是签到那一趟的报告。四态（signedToday / alreadySigned / signedNow /
	// signInPoints）**全部**从它派生，不另存一份布尔：存两份就会有对不上的那一天。
	report *app.SignInReport
	// status 是只查了状态（`--status-only`）时的那份状态。report 非 nil 时它被忽略 ——
	// 同一个量只留一个来源，那就是 report.After。
	status *upstream.SignInStatus
	// raw 是 `--raw` 那一趟的原始响应信封，**原样**打出。
	//
	// ⚠ 类型是 [json.RawMessage] 而不是 `[]byte`：后者在 `any` 里会被 encoding/json
	// 编成 base64 字符串，而这里要的是把上游那份 JSON 原封不动地嵌进去。
	raw json.RawMessage
	// failure 非 nil = 这一趟跑了但失败。
	failure *core.Error
	// skipped 非空 = 这一趟**没跑**（`--list` 模式、账号停用…），值是原因。
	//
	// 与 [signinView.failure] 是两回事，而且必须在终端上看得出区别：
	// 失败是"做了没做成"，跳过是"压根没做"。混成一个的话，定时任务连跑一个月
	// 也看不出它到底有没有在干活。
	skipped string
}

// signinState 是这一趟要展示的状态；没有可展示的状态时返回 nil。
func (v signinView) signinState() *upstream.SignInStatus {
	if v.report != nil {
		return &v.report.After
	}
	return v.status
}

// signinSummary 是整批的汇总。
//
// 计的是**结局**不是账号：一个账号只落进一档（failure / skipped / 四态之一）。
type signinSummary struct {
	total         int
	signedNow     int
	alreadySigned int
	// unknown 是"既没签上、也没有「今天已签」的凭据"那一档。正常不该出现，
	// 但它一旦出现，把它并进别的任何一档都是在替上游撒谎。
	unknown int
	failed  int
	skipped int
}

// summarizeSignin 数一遍结局。
//
// 编排层判退出码用的就是它（`failed`）—— 汇总行与退出码**必须**同源，
// 否则会出现"屏幕上写着失败 2 个、退出码却是 0"。
func summarizeSignin(views []signinView) signinSummary {
	summary := signinSummary{total: len(views)}
	for _, view := range views {
		switch {
		case view.failure != nil:
			summary.failed++
		case view.skipped != "":
			summary.skipped++
		case view.report == nil:
			// 只查了状态（`--status-only` / `--raw`）：没有"签没签上"这回事，
			// 只计入总数。
		case view.report.SignedNow():
			summary.signedNow++
		case view.report.WasAlreadySigned():
			summary.alreadySigned++
		default:
			summary.unknown++
		}
	}
	return summary
}

// ─────────────────────────────────────────────────────────────────────────────
// 账号标识
// ─────────────────────────────────────────────────────────────────────────────

// signinAccountID 是账号在输出里的标识。
//
// 一律取 [core.Account.ID]（掩码手机号，或昵称派生的稳定串），**不**取
// `PhoneMasked`、也不用昵称去顶替：账号列与"账号 N"前缀打的都是它。
// 换一个来源，同一行在两处就会显示不同的东西。
func signinAccountID(account core.Account) string {
	return account.ID.String()
}

// signinWho 是"账号 2 (19****01)"那一段；没有下标时只给账号。
func signinWho(view signinView) string {
	if view.index == 0 {
		return "账号 " + signinAccountID(view.account)
	}
	return fmt.Sprintf("账号 %d (%s)", view.index, signinAccountID(view.account))
}

// signinProgressLine 是"正在做什么"那行。
//
// 单独成一个函数而不是直接写进 [renderSigninResult]：编排层在**发请求之前**就该能打它，
// 那才是这行的用处（等待期间看得见进度）；等结果出来再打就晚了。
func signinProgressLine(view signinView) string {
	what := "签到"
	switch view.mode {
	case signinModeRaw:
		what = "取 infoV3 原始响应"
	case signinModeStatus:
		what = "查询签到状态"
	}
	return fmt.Sprintf("%s：以 iOS App 身份%s", signinWho(view), what)
}

// ─────────────────────────────────────────────────────────────────────────────
// 人看的：一个账号
// ─────────────────────────────────────────────────────────────────────────────

// renderSigninResult 是一个账号"拿到了东西"时的那几行（签到结论 / 状态 / 原始响应）。
func renderSigninResult(out *output, view signinView) error {
	if len(view.raw) > 0 {
		// `--raw` 是排障路径：整份原样打出，不解析。上游那份 JSON 长什么样，
		// 这里就是什么样 —— 替它重排字段只会让"看到的"和"抓到的"对不上。
		return out.rawJSON(view.raw)
	}
	if view.report != nil {
		renderSigninConclusion(out, view.report)
	}
	if status := view.signinState(); status != nil {
		renderSigninStatus(out, status)
	}
	return nil
}

// renderSigninConclusion 是签到结论那一行。
//
// 三种结局必须**分开报** —— 它们在日志里长得一样，含义却完全不同：
// 定时任务连跑一个月，只有分开才知道它到底有没有在干活。
func renderSigninConclusion(out *output, report *app.SignInReport) {
	switch {
	case report.SignedNow():
		// `SignedNow` 蕴含 `Result != nil`，所以点数一定拿得到 ——
		// Rust 侧那条 `None => "签到成功"` 到不了，不搬。
		out.human("签到成功：+%d 豆", *report.SignInPoints())
	case report.WasAlreadySigned() && report.Result != nil:
		// `Force`（协议自检）会在已签时也发一次 `startSignIn`：`Result` 非 nil、
		// 回查也是已签，但**不是**"这次签上的"。
		out.human("今天本来就已签；按 --force 重发了一次 startSignIn，上游接受（协议自检通过）。")
	case report.WasAlreadySigned():
		out.human("今天已经签过了，本次没有重复提交。")
	default:
		// 既不是"刚签上"、也不是"本来就签过"，却还能走到这里 —— 不该发生。
		// 宁可打一句明确的异常，也不要静默按"成功"糊过去。
		out.human("⚠ 本次既未签上、也没有「今天已签」的凭据，请检查 infoV3 的返回值。")
	}
}

// renderSigninStatus 是状态那几行。
//
// **不把整份 `cal` 倒出来**：35 个格子对排障没有帮助，而"今天签没签"与
// "本月已签几天"才是要看的两件事。
//
// 单位一律叫「豆」不叫「朵」：云朵与 AI豆是**同一个东西的两个名字**
// （2026-07-31 按 1:1 改名），界面（H5）上写的就是「豆」，输出跟着界面走 ——
// 用户对着 App 核数时才不会以为差了东西。
func renderSigninStatus(out *output, status *upstream.SignInStatus) {
	today := "未知（服务端没标出今天是哪一格）"
	if signed, ok := status.SignedToday(); ok {
		if signed {
			today = "已签"
		} else {
			today = "未签"
		}
	}
	out.human("  今天：%s", today)
	out.human("  豆余额：%d", status.BeanTotal)
	// 两个口径不一致时**把另一个也说出来** —— 不说的话，看到 `--raw` 的人会以为
	// 我们漏读了一个字段。`beforeTotal` 是改名时结转进来的那笔（一张冻结的快照），
	// 永远不变、也永远不等于余额。
	//
	// 上游没给（nil）就**不提**：那是"没说"，不是"是 0"。
	if status.CloudTotal != nil && *status.CloudTotal != status.BeanTotal {
		out.human("  （另有一个数 %d：改名时结转进来的那笔，不是余额 —— 见 `--raw` 的 beforeTotal）",
			*status.CloudTotal)
	}
	out.human("  连续签到：%d 天", status.SignCount)
	out.human("  本月已签：%d 天（服务端报 %d）", status.CountedMonthDays(), status.MonthDays)
	out.human("  本次可得：%d 豆", status.SignInPoints)
	// 有才打这一行 —— 天天打一句"没有"是纯噪音。
	if len(status.ReceiveList) > 0 {
		items := make([]string, 0, len(status.ReceiveList))
		for _, item := range status.ReceiveList {
			items = append(items, fmt.Sprintf("%s %d 豆（cloudType=%d）",
				cloudTypeLabel(item.CloudType), item.CloudNum, item.CloudType))
		}
		out.human("  可领取账目：%s", strings.Join(items, "，"))
	}
	out.human("  自动备份：%s", backupLine(status))
}

// cloudTypeLabel 是 `receiveList` 里那条 `cloudType` 的人话 —— 由它决定"现在能不能领"。
//
//	0    当前可领取（直接生效）
//	1    月度任务奖励（上月攒下的），需开启自动备份
//	2    下月才可领（官方 H5 也只弹一句提示，**不发请求**）
//	其它 当前可领取 —— H5 的点击分流里 `0` 与"其它"走的是同一条路（`else` 分支）
//
// ⚠ 写"可领取"之前先看是不是 `2`：曾经一律写成"可领取：cloudType=2 × 154 朵"，
// 而 `2` 恰恰是**下月可领取** —— 一句话把"领不了"说成了"能领 154 朵"。
func cloudTypeLabel(cloudType int64) string {
	switch cloudType {
	case upstream.CloudTypeDeferred:
		return "月度可领取（需开启备份）"
	case upstream.CloudTypeNextMonth:
		return "下月可领取"
	default:
		return "可领取（立即生效）"
	}
}

// backupLine 是「自动备份」那行的措辞。
//
// 三态要分得开：开了 / 没开 / **上游没说**。混成一个 bool 的话，"判不了"会被读成
// "没开"，然后有人去追一个根本不存在的备份问题。
func backupLine(status *upstream.SignInStatus) string {
	if status.CurMonthBackup == nil {
		return "上游没给 `curMonthBackup`，判不了"
	}
	if !*status.CurMonthBackup {
		return "本月未开启 —— 翻倍奖励与月度奖励都拿不到"
	}
	// 已开启：翻倍机会领没领是第二个门槛（每月只有一次）。
	if status.CurMonthBackupSignAccept == nil {
		return "本月已开启（翻倍机会领没领，上游没说）"
	}
	if *status.CurMonthBackupSignAccept {
		return "本月已开启（翻倍机会本月已领过）"
	}
	return "本月已开启（翻倍机会还没领，可 `--multiple`）"
}

// ─────────────────────────────────────────────────────────────────────────────
// 人看的：整批
// ─────────────────────────────────────────────────────────────────────────────

// renderSigninBatch 是整批签到的渲染入口。
//
// asJSON 决定通道，其余照 Rust 的边界走：
//
//   - `--json`：**stdout 上只有那一份 JSON**（单账号给对象、多账号给数组）；
//     进度行、分隔行、失败行一律走 stderr —— 否则 `mcloud signin --json | jq`
//     当场语法错，而人在终端上看不出任何不对。
//   - 人看的：逐账号几行，多账号时带分隔行与汇总。
//
// 一个账号失败**不影响**其余：失败只打一行、记一笔，循环继续。
func renderSigninBatch(out *output, views []signinView, asJSON bool) error {
	multi := len(views) > 1
	for _, view := range views {
		if multi {
			separator := fmt.Sprintf("------ %s ------", signinWho(view))
			if asJSON {
				out.note("%s", separator)
			} else {
				out.human("%s", separator)
			}
		}
		// ⚠ "正在做什么"那行**不在这里打**。
		//
		// 它的用处是"等待期间看得见进度"，所以必须在**发请求之前**打 —— 由编排层
		// 用 [signinProgressLine] + `out.note` 完成。渲染函数是拿到结果之后才被调用的，
		// 在这里再打一遍就是**同一个账号两行**，而且第二行还出现在结果之后，
		// 读起来像"它又要做一次"。
		//
		// 顺带：`out.note` 走 stderr，与"进度行永远 stderr"的约定一致
		// （见 output.go 的注释）；`out.human` 会把它写进 stdout，
		// 于是 `mcloud signin > log.txt` 的日志里混进一行与结果无关的进度。
		if view.failure != nil {
			// 失败**永远**走 stderr：它不是这一趟的结果，而是"这一趟没做成"。
			out.note("✗ %s 失败：%s", signinWho(view), view.failure.Error())
			continue
		}
		if view.skipped != "" {
			// 跳过与失败是两回事，而且在终端上必须看得出来：失败是 `✗`（stderr），
			// 跳过是"压根没做"（人看时走 stdout，`--json` 时退到 stderr 免得污染 stdout）。
			if asJSON {
				out.note("%s 跳过：%s", signinWho(view), view.skipped)
			} else {
				out.human("%s 跳过：%s", signinWho(view), view.skipped)
			}
			continue
		}
		if asJSON {
			continue
		}
		if err := renderSigninResult(out, view); err != nil {
			return err
		}
	}

	if asJSON {
		return renderSigninJSON(out, views)
	}
	if multi && summaryApplies(views) {
		renderSigninSummary(out, views)
	}
	return nil
}

// summaryApplies 报告这一批要不要打汇总。
//
// 只有**真的在签到**的批次才打：`--status-only` / `--raw` 那两批没有"签没签上"这回事，
// 报一句"签到成功 0 个"是把一件没发生的事说成了结果。
func summaryApplies(views []signinView) bool {
	for _, view := range views {
		if view.mode == signinModeSignIn {
			return true
		}
	}
	return false
}

// renderSigninSummary 是多账号的汇总。
//
// ⚠ Rust 侧**没有**这一行（它只在不是全成功时用退出码报"有几个失败"）。这里加上它，
// 是因为"一共跑了几个、其中几个真签上了"是定时任务日志里最该一眼看到的东西：
// 失败数靠退出码，而"今天已签过 3 个、签到成功 0 个"只有这里说得出来。
func renderSigninSummary(out *output, views []signinView) {
	summary := summarizeSignin(views)
	line := fmt.Sprintf("共 %d 个账号：签到成功 %d 个，今天已签过 %d 个",
		summary.total, summary.signedNow, summary.alreadySigned)
	if summary.unknown > 0 {
		line += fmt.Sprintf("，状态异常 %d 个", summary.unknown)
	}
	line += fmt.Sprintf("，失败 %d 个", summary.failed)
	if summary.skipped > 0 {
		line += fmt.Sprintf("，跳过 %d 个", summary.skipped)
	}
	out.human("%s", line)
}

// ─────────────────────────────────────────────────────────────────────────────
// `--list`
// ─────────────────────────────────────────────────────────────────────────────

// renderSigninAccounts 是 `--list` 的表。
//
// 存在的理由只有一个：`--account` 收的是**下标**，而"下标 3 是哪个账号"必须有个地方
// 能查到。顺序与 `store.ListAccounts` 一致（`sort_order ASC, id ASC`），所以它是稳定的。
//
// ⚠ 表头是**逐字抄** Rust 那一行手写的字面量，因此它和下面的行格式**对不齐**
// （Rust 那边本来就对不齐）。对齐目标是与冻结的 Rust 输出逐字可 diff，
// 不是"排得好看" —— 顺手"修"一下，两边的 `--list` 就再也 diff 不起来了。
func renderSigninAccounts(out *output, accounts []core.Account) {
	if len(accounts) == 0 {
		// 空库不是错误（用户可能刚 init），但**必须说清楚下一步做什么** ——
		// 直接"跑完、什么都没打"会让人以为签到成功了。
		out.human("库里还没有账号。先登录一个：`mcloud login sms --client app`")
		return
	}
	out.human("下标  账号          昵称            登录端   状态")
	for position, account := range accounts {
		nickname := "-"
		if account.Nickname != nil {
			nickname = *account.Nickname
		}
		platform := "-"
		if account.Platform != nil {
			platform = *account.Platform
		}
		state := "停用"
		if account.Enabled {
			state = "启用"
		}
		out.human("%4d  %-12s  %-14s  %-6s  %s",
			position+1, signinAccountID(account), nickname, platform, state)
	}
	out.human("\n不带 `--account` 直接跑，就是对**全部启用**的账号各签一次。")
}

// ─────────────────────────────────────────────────────────────────────────────
// `--json`
// ─────────────────────────────────────────────────────────────────────────────

// signinStatusJSON 是 `--json` 里那份状态对象的形状。
//
// # 字段顺序是**按 JSON 键名排的**
//
// Rust 那边的 `serde_json` 没开 `preserve_order`（键表是 BTreeMap），所以它吐出来的
// 键是**字典序**。这里照同一个顺序声明，`--json` 才能与冻结的 Rust 输出逐字对拍；
// 按语义分组会更好读，但那样两边 diff 起来就只剩"每个键都对、顺序全不同"。
//
// # `signedDays` 与 `monthDays` 是同一个数的两条独立路径
//
// 前者自己数 `cal`、后者是服务端报的。两者不一致就是上游数据有问题 ——
// 摆在同一个对象里比塞进一句自然语言里更容易被脚本发现。
type signinStatusJSON struct {
	// AlreadySigned 只有拿到报告时才有：来的时候今天就已经签过了。
	AlreadySigned *bool `json:"alreadySigned,omitempty"`
	// BeanBefore 是签到**前**的余额。叫 `beanBefore` 而不是 `cloudBefore` 是有意的：
	// 后者那个名字以前装的是上游 `beforeTotal`（不随领取变动的数），
	// 叫"before"会让人以为它是"签到前的余额"。改名比留一个静默错的键好。
	BeanBefore *int64 `json:"beanBefore,omitempty"`
	// BeanTotal 才是**余额**（上游 `total`）。
	BeanTotal int64 `json:"beanTotal"`
	// CloudTotal 装的是上游 `beforeTotal` —— 改名时结转进来的那笔，
	// **不随领取变化**。保留这个键是因为它一直在这儿；别拿它画趋势。
	CloudTotal *int64 `json:"cloudTotal"`
	// CountedMonthDays 是自己数 `cal` 数出来的本月已签天数。
	CountedMonthDays int `json:"countedMonthDays"`
	// CurMonthBackup / CurMonthBackupSignAccept 是「自动备份」的两条门槛：
	// `--multiple` 与月度奖励（`cloudType=1`）能不能领的前提。
	// **不给的话，"为什么没领到"就只能靠猜。**
	//
	// nil = 上游没给这个字段，**不是 false** —— 两者对 `--multiple` 的处置完全不同。
	CurMonthBackup           *bool `json:"curMonthBackup"`
	CurMonthBackupSignAccept *bool `json:"curMonthBackupSignAccept"`
	// MonthDays 是服务端报的本月已签天数。
	MonthDays int64 `json:"monthDays"`
	// ReceiveList 是 `--receive` 自动分流的取值来源。`recordId` 实拍里没有，
	// 所以如实给 null 而不是编一个空串；`acceptDate` 只有 `cloudType=1` 才用得上，
	// 但**要打出来** —— 少了它就只能干瞪眼（请求发不出去），而"为什么发不出去"必须看得见。
	ReceiveList []signinReceiveJSON `json:"receiveList"`
	// SignCount 是连续签到天数。
	SignCount int64 `json:"signCount"`
	// SignInPoints 是本次签到可得数（服务端 `signInPoints`）。
	SignInPoints int64 `json:"signInPoints"`
	// SignedDays 是本月已签的日号。
	//
	// ⚠ 用 `[]int` 而不是 `[]uint8`：后者是 `[]byte` 的别名，encoding/json 会把它
	// 编成 base64 字符串 —— 而这里要的是 `[1,2,3]`。
	SignedDays []int `json:"signedDays"`
	// SignedNow 只有拿到报告时才有：这次调用**真的**把今天从"未签"变成了"已签"。
	// 与 `alreadySigned` 成对：只有分开给，"定时任务到底有没有在干活"才判得出来。
	SignedNow *bool `json:"signedNow,omitempty"`
	// SignedToday 是"今天（调用之后）是否已签"。**这是唯一的成败判据。**
	// nil = 服务端没把任何一格标成"今天"（那时**不能猜**：`cal` 里混着上/下月的补位格）。
	SignedToday *bool `json:"signedToday"`
	// ToReceive 是**现在就能领**的合计（不含月度那条线）。上游有时不给 → null。
	ToReceive *int64 `json:"toReceive"`
}

// signinReceiveJSON 是 `receiveList` 里的一笔（键名同样按字典序）。
type signinReceiveJSON struct {
	AcceptDate *string `json:"acceptDate"`
	CloudNum   int64   `json:"cloudNum"`
	CloudType  int64   `json:"cloudType"`
	RecordID   *string `json:"recordId"`
}

// signinFailureJSON 是 `--json` 里失败那一档的形状（键名同样按字典序）。
//
// `ok` 恒为 false：只有失败这一档会走到这里。保留这个键是因为脚本按它分流 ——
// 成功的那些条目没有它，脚本不能靠"有没有 error"来判断。
type signinFailureJSON struct {
	Account string `json:"account"`
	Error   string `json:"error"`
	Index   int    `json:"index"`
	OK      bool   `json:"ok"`
}

// renderSigninJSON 是 `--json` 的那一份：**单账号给对象、多账号给数组**。
//
// 这条规则是刻意不统一的：脚本里最常见的是"盯一个账号"，给那种用法套一层数组，
// 等于逼所有老脚本改一遍；而多账号时不套数组，多个对象的字段会互相盖掉 ——
// 那才是真危险。
func renderSigninJSON(out *output, views []signinView) error {
	entries := make([]any, 0, len(views))
	for _, view := range views {
		switch {
		case view.failure != nil:
			entries = append(entries, signinFailureJSON{
				Account: signinAccountID(view.account),
				Error:   view.failure.Error(),
				Index:   view.index,
			})
		case view.skipped != "":
			// 被跳过的账号在 JSON 里**没有条目**：Rust 侧它们根本不在 targets 里，
			// 于是结果数组里也没有它们。要看"跳过了谁"，看人看的输出。
		case len(view.raw) > 0:
			entries = append(entries, view.raw)
		default:
			entries = append(entries, signinStatusPayload(view))
		}
	}
	// 判据是**产出条目的个数**，不是目标账号个数 —— 跳过的那些不算（它们没有结果）。
	if len(entries) == 1 {
		return out.json(entries[0])
	}
	return out.json(entries)
}

// signinStatusPayload 把状态（与报告）拼成 `--json` 的那个对象。
func signinStatusPayload(view signinView) signinStatusJSON {
	status := view.signinState()
	payload := signinStatusJSON{
		BeanTotal:    status.BeanTotal,
		CloudTotal:   status.CloudTotal,
		SignCount:    status.SignCount,
		SignInPoints: status.SignInPoints,
		MonthDays:    status.MonthDays,
		// 两个切片都**先建空**再填：nil 切片会被编成 `null`，而上游没给时
		// Rust 那边给的是 `[]` —— "一条都没有"与"这个字段不存在"是两件事。
		CountedMonthDays:         status.CountedMonthDays(),
		SignedDays:               make([]int, 0, len(status.Days)),
		ReceiveList:              make([]signinReceiveJSON, 0, len(status.ReceiveList)),
		ToReceive:                status.ToReceive,
		CurMonthBackup:           status.CurMonthBackup,
		CurMonthBackupSignAccept: status.CurMonthBackupSignAccept,
	}
	if signed, ok := status.SignedToday(); ok {
		payload.SignedToday = &signed
	}
	for _, day := range status.Days {
		if day.CurrentMonth && day.Signed {
			payload.SignedDays = append(payload.SignedDays, int(day.Day))
		}
	}
	for _, item := range status.ReceiveList {
		payload.ReceiveList = append(payload.ReceiveList, signinReceiveJSON{
			CloudType:  item.CloudType,
			CloudNum:   item.CloudNum,
			RecordID:   item.RecordID,
			AcceptDate: item.AcceptDate,
		})
	}
	if report := view.report; report != nil {
		signedNow := report.SignedNow()
		alreadySigned := report.WasAlreadySigned()
		beanBefore := report.Before.BeanTotal
		payload.SignedNow = &signedNow
		payload.AlreadySigned = &alreadySigned
		payload.BeanBefore = &beanBefore
	}
	return payload
}
