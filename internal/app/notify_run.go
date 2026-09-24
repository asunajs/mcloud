// 一趟运行收尾时的推送编排：**门槛判定 → 正文组装（含脱敏）→ 逐家发送 → 一句小结**。
//
// 对应旧实现两处：`packages/cli/src/index.ts` 的 `sendPushNotification`（判门槛、
// 取日志、发一次）与 `packages/shared/src/push/index.ts` 的 `formatLogsForPush`
// （按级别过滤 + 排成 `[type 时刻]正文`）。渠道侧的发送与逐家记账在
// `internal/upstream/notify.go`，配置模型在 `internal/core/notify.go`。
//
// # 为什么这一族住在 app
//
// 它同时要用**运行过程话**（只有编排层有）、**推送配置**（存储层）、**发送器**
// （上游层），而这三样在依赖方向上分别属于它的下面与旁边 —— 放 server 或 CLI
// 就得各写一份，放 upstream 则它得反过来知道"一次运行"是什么。
//
// # 与旧实现的四处不同，每处都写清代价
//
//  1. **正文脱敏**：旧实现把 logger 里的原文直接发出去。这里每条过一遍
//     [core.ScrubText]（与调度写 `last_error` 同一个入口）。代价是正文里可能出现
//     `<REDACTED>` 而不是那串真值 —— 而这一条消息要经过 pushplus / Telegram /
//     邮件服务商的服务器，**出网的那一句不该带可直接复用的凭据**。
//  2. **时刻按上海时区**（[core.LocalTZ]）：旧实现是 `toLocaleTimeString()`，
//     随运行进程的语言环境变。这里钉死一个时区，与调度 cron 用同一份
//     （否则"cron 说 09:00 跑"与"推送正文里写着 01:00 跑"会在同一条消息里打架）。
//     显示用的 `HH:MM:SS` 是 24 小时制；旧那一句在中文环境下是"上午9:05:43"，
//     **不复刻**：它随 locale 变，我们没法也不必复刻一个由浏览器/系统决定的格式。
//  3. **一家都没配时不发**：旧实现是空转一圈（`Object.entries` 没命中就什么都不做），
//     表现成"配了推送但一条没收到，日志里也看不出为什么"。这里给一条 skip 原因。
//  4. **门槛不过时给原因**：旧实现那四处 `return` 全是静默的。这一族的常见现象恰好是
//     "没收到"，而"没收到"的四种原因（开关没开 / 没有过程话 / 只推错误但这次没错 /
//     过滤完正文为空）在用户看来是同一件事 —— 必须分开说。
//
// # ⚠ 未测试的点
//
// 这一层的判据全部在**进程内**断言（假发送器 + 假配置），没有任何一条真第三方推送
// 样本（`docs/pending-decisions.md` R22 那一族）。所以"发出去了"只到
// `upstream.NotifySender` 认为各家认了为止，"用户看到了"仍无判据。
package app

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 编译期确认：上游那个真发送器满足这里的端口（改了 `SendAll` 签名就红，
// 而不是等到接进 server 时才发现两边对不上）。
var _ RunNotifySender = (*upstream.NotifySender)(nil)

// notifyMaxLines 是一次运行**攒**多少条过程话的上限。
//
// agent 侧默认（**非用户要求**，登记在 `docs/pending-decisions.md`）。
// 为什么要上限：一条直播间链路一趟能写几百条过程话（口令、送礼、断线重连各一条），
// 而推送正文是要经过第三方服务器的 —— "把整趟过程的每一句都抄给对方"
// 既没必要，也会撞上各家正文长度限制（表现是"这一家莫名失败"，查不到根因）。
// 截断时在末尾留一条说明，免得读者以为"这就是全部"。
const notifyMaxLines = 200

// NotifyLogLine 是一条要进推送正文的过程话。
//
// 它的来源就是 [RunProgress] 写出去的那一句（同一条消息、同一个级别），
// 所以界面上看到的与推送里看到的是一回事 —— 旧实现也是这个关系
// （logger 的 transport 既进终端也进推送正文）。
//
// ⚠ `Message` 里**不该**有账号标识以外的个人信息：正文出网。
// 这一条不靠自觉，收尾组装时统一过 [core.ScrubText]（见 [FormatNotifyBody]）。
type NotifyLogLine struct {
	// Level 是本仓那三档之一（[core.LevelInfo] / Warn / Error）。
	Level core.LogLevel
	// At 是写下这一句的时刻。[core.Clock] 的契约是 **UTC**，
	// 展示时由组装方换到时区（见 [FormatNotifyBody]）。
	At time.Time
	// Message 是正文（不含级别与时刻那层前缀）。
	Message string
}

// RunNotifyConfigReader 是"读一份推送配置"那一步。
//
// 只收这一个方法而不是 [core.ConfigStore] 整张接口：编排层不该拿得到
// 定时任务与流水线的写入口，测试里也就少造十个桩。
// `*mcloudio.SqliteStore` 天然满足它。
type RunNotifyConfigReader interface {
	NotifyConfig(ctx context.Context) (core.NotifyConfig, error)
}

// RunNotifySender 是"把一条消息发到各家"那一步（[upstream.NotifySender] 满足）。
//
// 刻意**不**用 `core` 里的某个现成接口：这一族的返回值（逐家结论）是上游层的类型，
// app 依赖 upstream 是合法方向，绕一圈进 core 反而把上游的记账形状塞进纯层。
type RunNotifySender interface {
	SendAll(
		ctx context.Context, channels core.NotifyChannels, title, text string,
	) upstream.NotifyReport
}

// RunNotify 是一次收尾推送的编排。
//
// 零值不可用（三个依赖都要注入）。构造见 [NewRunNotify]。
type RunNotify struct {
	configs RunNotifyConfigReader
	sender  RunNotifySender
	clock   core.Clock
	// loc 只影响正文里那串**时刻**的显示，不参与任何判定。
	// 之所以要能换：判据时刻来自 [core.Clock]（UTC），而测试要把正文钉成字面量，
	// 拿进程的本地时区会让同一份断言在两台机器上差八个字节的偏移。
	loc *time.Location
}

// NewRunNotify 组装一次收尾推送。
//
// 三个参数**都可以是 nil**，而且各自的缺法对应一条不同的 skip 原因
// （"没接配置"与"没接发送器"在界面上是两句话）。之所以不 panic：
// CLI 与测试常常只需要跑任务、不需要推，硬要它们造两个空桩不如把"没接"当一等状态。
func NewRunNotify(configs RunNotifyConfigReader, sender RunNotifySender, clock core.Clock) *RunNotify {
	return &RunNotify{configs: configs, sender: sender, clock: clock, loc: core.LocalTZ()}
}

// In 换掉正文里那串时刻的显示时区，返回自身以便链式。
//
// 只影响**显示**：门槛、发送、判定一概不随时区变（旧实现也没有把时区当判据）。
func (n *RunNotify) In(loc *time.Location) *RunNotify {
	if n != nil && loc != nil {
		n.loc = loc
	}
	return n
}

// NotifyInput 是"这一趟跑完了，要不要推"的现场。
//
// 字段刻意少：正文的全部信息量都在 `Lines` 里，而"推什么标题"由配置决定 ——
// 把标题也从调用方传进来会让 CLI 与服务端各写一份默认值（那是旧项目那三处
// 默认值互相打架的成因，见 Q18 同族）。
type NotifyInput struct {
	// Summary 是本趟运行的人话小结（[RunSummary.SummaryLine] 的产物）。
	// 非空时作为**最后一条 info** 附在正文末尾：旧实现每账号收尾都有一句
	// "执行完成"，本仓的过程话里没有"这趟成没成"（它在 [RunSummary] 里），
	// 所以在这里补一条 —— 否则推送只说过程、不说结局。
	Summary string
	// Lines 是本次运行攒下的过程话（按发生先后）。
	Lines []NotifyLogLine
}

// RunNotifyResult 是一次收尾推送的结论。
//
// 三态由 `Skipped` 是否为空分开：
// **没发**（门槛没过 / 依赖没接 / 配置读不出来）与**发了但某几家失败**
// 是两件不同的事，混成"成功/失败"会让"我配的钉钉坏了"表现成"整个推送没跑"。
type RunNotifyResult struct {
	// Skipped 非空 ⇒ 这一次**一条对外请求都没发**，值是人能看懂的原因。
	Skipped string
	// Body 是实际发出去的正文（未发时为空串）。
	Body string
	// Title 是实际发出去的标题（未发时为空串）。
	Title string
	// Report 是逐家结论（未发时长度 0）。
	Report upstream.NotifyReport
	// Err 是**读配置**那一步的错误（发送失败不算：那已经在 Report 里逐家记了）。
	// 只用于测试断言与日志；对外文案走 [RunNotifyResult.SummaryLine]。
	Err error
}

// Sent 报告这一次**有没有**对外发过请求。
//
// ⚠ 它不等于"发成功了" —— 发出去了且三家全失败时它也是 true，
// 那要看 [RunNotifyResult.Report]。
func (r RunNotifyResult) Sent() bool { return r.Skipped == "" }

// String 让 `%v` 打出来就是那句小结（与 [core.MailMessage.String] 同一条纪律：
// 正文只在这里出现，而它**已经**过脱敏）。
func (r RunNotifyResult) String() string { return r.SummaryLine() }

// SummaryLine 是给人看的那一句（进调度记账、CLI 输出，不进 API 的结构化字段）。
//
// 未发时把原因带上：这一族最常见的现象就是"什么都没收到"，
// 而它的原因有五种，五种都得能从这一句里读出来。
func (r RunNotifyResult) SummaryLine() string {
	if r.Skipped != "" {
		return "推送没发：" + r.Skipped
	}
	if len(r.Report) == 0 {
		// 发过了但一家都没记结论 —— 只有自定义 sender 会走到这里。
		// 空句子比"看起来一切正常"诚实。
		return "推送：没有一家给出结论"
	}
	return "推送：" + r.Report.SummaryLine()
}

// Dispatch 判门槛、组正文、逐家发。
//
// **永不返回 error**：推送是运行的**附属品**，它失败不该让一趟已经往上游投了口令的
// 运行变成"运行失败"（更不该让 `Runner.Run` 回错、让界面把一次成功的签到标红）。
// 结论一律在 [RunNotifyResult] 里，调用方爱怎么记账是它的事。
//
// 门槛的顺序照旧实现（`sendPushNotification`），且顺序**有后果**：
// "开关没开"要排在"没有过程话"之前 —— 否则关掉推送且一趟什么都没写时，
// 用户看到的是"没有过程话"，而真正的原因是开关。
func (n *RunNotify) Dispatch(ctx context.Context, in NotifyInput) RunNotifyResult {
	if n == nil || n.configs == nil {
		return RunNotifyResult{Skipped: "本进程没接推送配置"}
	}
	if n.sender == nil {
		return RunNotifyResult{Skipped: "本进程没接推送发送器"}
	}
	config, err := n.configs.NotifyConfig(ctx)
	if err != nil {
		// 错误原文先脱敏再截断：这一句会进调度记账与日志。
		return RunNotifyResult{
			Skipped: "推送配置读不出来：" + clipNotifyText(core.ScrubText(err.Error()), 120),
			Err:     err,
		}
	}
	if !config.Enabled {
		return RunNotifyResult{Skipped: "总开关没开"}
	}
	lines := in.Lines
	if in.Summary != "" {
		lines = append(append([]NotifyLogLine(nil), lines...), NotifyLogLine{
			Level:   core.LevelInfo,
			At:      n.now(),
			Message: in.Summary,
		})
	}
	if len(lines) == 0 {
		return RunNotifyResult{Skipped: "这次运行没有过程话"}
	}
	// 旧口径：`onlyError && !logs.some(type==='error')`，且这一句看的是**过滤之前**
	// 的全量日志 —— 门槛设成 error 以上时，一条 warn 不该被当成"有错误"，
	// 反过来一条 error 也不该因为门槛设得更高而被漏掉。
	if config.OnlyError && !hasErrorLine(lines) {
		return RunNotifyResult{Skipped: "设了只推错误，而这次没有 error"}
	}
	// 一家都没配 ⇒ 不发（旧实现是空转一圈，什么也不说）。
	// 与"配了但必填缺"（`SendAll` 会逐家记 skipped）刻意分开：
	// 前者是"你没配"，后者是"配了但这一家发不出去"，处置完全不同。
	if len(config.Channels.Names()) == 0 {
		return RunNotifyResult{Skipped: "一家推送渠道都没配"}
	}
	body := FormatNotifyBody(lines, config.MinLevel, n.location())
	if strings.TrimSpace(body) == "" {
		return RunNotifyResult{Skipped: fmt.Sprintf(
			"门槛为 %s 时正文为空", notifyLevelWord(config.MinLevel))}
	}
	title := config.TitleOrDefault()
	return RunNotifyResult{
		Body:   body,
		Title:  title,
		Report: n.sender.SendAll(ctx, config.Channels, title, body),
	}
}

// FormatNotifyBody 把过程话按门槛排成推送正文。
//
// 对应旧 `formatLogsForPush`：`logs.filter(l => l.level <= max)` 再
// `map(m => \`[${m.type} ${时间}]\${m.msg}\`)`，`join('\n')`。
//
// 三处口径要说清：
//   - **未知门槛按 info 走**：旧实现是 `PUSH_LEVEL_MAX[minLevel] ?? 3`，
//     库里的文本被人手改坏时不该让推送整体罢工（与 [core.NotifyConfig.Validate]
//     的"空串跟随 info"同一条方向）。
//   - **每条过 [core.ScrubText]**：见本文件抬头第 1 条。
//   - **末行不留空行**：与旧实现 `join('\n')` 同形状。
//
// 导出是为了让 server 侧的"试发"（`POST /api/notify/test`）与测试能用**同一个**
// 组装函数 —— 试发出来的与真发的不是同一形状，是这类界面最常见的误导。
func FormatNotifyBody(
	lines []NotifyLogLine, minLevel core.NotifyLogLevel, loc *time.Location,
) string {
	ceiling := notifyCeiling(minLevel)
	var out strings.Builder
	for _, line := range lines {
		if notifyLevelNumber(line.Level) > ceiling {
			continue
		}
		if out.Len() > 0 {
			out.WriteByte('\n')
		}
		out.WriteString(fmt.Sprintf("[%s %s]%s",
			line.Level.String(), formatNotifyTime(line.At, loc),
			core.ScrubText(line.Message)))
	}
	return out.String()
}

// notifyCeiling 把门槛文本折成"能收进来的最高数值档"。
//
// 未知（含空串 ⇒ [core.LevelCeiling] 给 -1）落回 info 那一档（3），
// 与旧实现 `?? 3` 同口径。
func notifyCeiling(minLevel core.NotifyLogLevel) int {
	if ceiling := minLevel.LevelCeiling(); ceiling >= 0 {
		return ceiling
	}
	return core.NotifyLevelInfo.LevelCeiling()
}

// notifyLevelNumber 把本仓三档映射到旧 `PUSH_LEVEL_MAX` 的数值档。
//
// 本仓的枚举序是"越严重越大"（info 0 / warn 1 / error 2），旧那套是
// "越严重越小"（error 0 / warn 1 / info 3 / debug 4）—— 两套方向相反，
// 所以这一格映射**必须**集中在此处：散着写的表现是"门槛整体偏一档"
// （`core/notify.go` 里那条"没有 2 那一档"的注释说的就是这件事）。
//
// 本仓没有 debug 档，故 2 那一格空着，与旧表同形。
func notifyLevelNumber(level core.LogLevel) int {
	switch level {
	case core.LevelError:
		return 0
	case core.LevelWarn:
		return 1
	case core.LevelInfo:
		return 3
	}
	// 越界值（枚举漏态由 `run_test.go` 的真值表守，这里是第二道）：按最严的那一档收，
	// 判成"只有 error 门槛才收" ⇒ 宁可少一行噪声，也不把未知级别的句子塞进出网正文。
	return 0
}

// notifyLevelWord 给人看的门槛名（未知值也要说得出"实得什么"）。
func notifyLevelWord(minLevel core.NotifyLogLevel) string {
	if minLevel == "" {
		return "info（没填，跟随默认）"
	}
	return string(minLevel)
}

// hasErrorLine 判这批过程话里有没有 error 档（`onlyError` 的判据）。
func hasErrorLine(lines []NotifyLogLine) bool {
	for _, line := range lines {
		if line.Level == core.LevelError {
			return true
		}
	}
	return false
}

// formatNotifyTime 把 UTC 时刻换成"这一时区的 HH:MM:SS"。
func formatNotifyTime(at time.Time, loc *time.Location) string {
	if loc != nil {
		at = at.In(loc)
	}
	return at.Format("15:04:05")
}

// clipNotifyText 按**字符**截断（不是字节）：这一句里通常有中文，
// 按字节切会把汉字切成两半，界面拿到的是乱码尾巴。
func clipNotifyText(text string, max int) string {
	runes := []rune(text)
	if max <= 0 || len(runes) <= max {
		return text
	}
	return string(runes[:max]) + "…"
}

func (n *RunNotify) now() time.Time {
	if n == nil || n.clock == nil {
		// 没有时钟时**不**取系统时间（本仓业务代码禁止 `time.Now()`，判据见
		// [core.Clock]）。这一句只用于"小结那一条"的时刻，给零值让界面看到
		// `00:00:00` —— 看着像坏了，比悄悄拿系统时间好。
		return time.Time{}
	}
	return n.clock.Now()
}

func (n *RunNotify) location() *time.Location {
	if n == nil || n.loc == nil {
		return core.LocalTZ()
	}
	return n.loc
}

// ─────────────────────────────────────────────────────────────────────────────
// 过程话的攒取（只有接了推送的运行器会走这一段）
// ─────────────────────────────────────────────────────────────────────────────

// runLogRecorder 是"把一句过程话攒进收尾那条推送"的入口（`nil` = 不攒）。
//
// 为什么是闭包而不是把 sink 直接传下去：时刻要从注入的 [core.Clock] 取、
// 账号前缀要知道本次是第几个账号、"没接推送"要让整条链一次分配都不发生 ——
// 三样都属于调用点（`Runner.Run` 的循环体），而写进 `runOne` 就得把参数表撑长。
type runLogRecorder func(level core.LogLevel, message string)

// record 攒一条。nil 接收者合法（不接推送时传的就是 nil）。
func (f runLogRecorder) record(level core.LogLevel, message string) {
	if f != nil {
		f(level, message)
	}
}

// runLogSink 攒下本次运行的过程话，供收尾那条推送用。
//
// **只在接了推送时创建**（见 [Runner.notify]）：一条直播链路一趟能写几百条过程话，
// 不推送时把它们全攒在内存里是白付的钱。
//
// 不带锁：本仓的运行是**串行**的（`Runner.Run` 的注释里那条"v1 不做并行"），
// 攒取只发生在单个 goroutine 里。将来接并行时这一处要连注释一起改。
type runLogSink struct {
	lines   []NotifyLogLine
	dropped int
}

// add 记一条，超过 [notifyMaxLines] 后只计数并静默丢弃。
func (s *runLogSink) add(level core.LogLevel, at time.Time, message string) {
	if s == nil {
		return
	}
	if len(s.lines) >= notifyMaxLines {
		s.dropped++
		return
	}
	s.lines = append(s.lines, NotifyLogLine{Level: level, At: at, Message: message})
}

// collect 取出攒到的行，并在末尾补上"中间省略了多少"那一条。
//
// 省略说明**必须**进正文：否则读者以为"这一趟只做了这些"，
// 而它恰恰是最该警觉的时候（过程话多到要截断，通常意味着某个环节在刷错误）。
func (s *runLogSink) collect(at time.Time) []NotifyLogLine {
	if s == nil {
		return nil
	}
	lines := s.lines
	if s.dropped > 0 {
		lines = append(lines, NotifyLogLine{
			Level:   core.LevelWarn,
			At:      at,
			Message: "过程话超过 " + strconv.Itoa(notifyMaxLines) + " 条，其余 " + strconv.Itoa(s.dropped) + " 条省略",
		})
	}
	return lines
}
