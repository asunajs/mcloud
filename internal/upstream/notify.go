// 推送渠道的**发送层**：十个渠道逐个发、逐个记账，一封邮件不算失败整次。
//
// 对应旧实现 `packages/shared/src/push/index.ts`（316 行）的 `CHANNEL_MAP` + `sendNotify`。
// 请求构造与逐家判据在 `notify_channels*.go`，模型在 `internal/core/notify.go`。
//
// # 与旧实现的三处**刻意不同**（都不是"顺手优化"，各有一个失效场景）
//
//  1. **失败可见**：旧实现把每个渠道的结果全塞进 logger，`sendRequest` 一律返回
//     `undefined` —— 调用方**无从知道**是"没配渠道"还是"配的四家全挂了"。
//     这里返回逐家结果（[NotifyResult]），因为这一族将来要进 `run_events`：
//     "定时任务跑完了但一条推送都没发出去"必须是查得出来的事。
//  2. **缺必填不发**：旧实现只有 bark / tgBot / email 三家有前置检查，其余带着空
//     token 也照样 POST 出去（在第三方那里留下一次无意义请求，而我们这边只是日志）。
//     判据在 [core.NotifyChannels.Missing]，这里只负责"缺了就记一条 skipped"。
//  3. **Telegram 看 `ok`**：旧实现只看 `errcode` / `code` / `err` 三个键，
//     而 Bot API 失败时回的是 `{"ok":false,"error_code":401,...}` ——
//     `error_code` 不等于 `errcode`，`ok:false` 又不在这三键里，
//     于是**一次 401 会被判成"已发送"**。这一格不需要样本：那是 Bot API 的公开形状。
//
// # 重试：本层**不**自己重试
//
// 方案（`../mcloud-rust/docs/rust-rewrite-plan.md` §1.6）写的是"发送层重做：补重试、
// 超时、失败可见"。超时与重试的落点在 [io.Client]（`RetryPolicy` + `TimeoutMS`），
// 而它的规则是"写请求必须带幂等键才重试"—— 推送渠道**没有**幂等键这种东西。
// 所以这一层先只做"失败可见"，重试登记成待拍板项（重复推送一条 vs 漏推一条，
// 哪一个更不能接受，是用户口径不是我能替他定的）：见 `docs/pending-decisions.md`。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// NotifyOutcome 是一家渠道这一次的结论。**三档**，没有"部分成功"。
type NotifyOutcome string

const (
	// NotifySent 请求发出且按该渠道的判据算收下。
	//
	// ⚠ "按判据算收下" ≠ "用户看到了"：pushplus 回 `code:200` 只代表收到任务，
	// 微信那边到不到是它的异步事。**没有任何一栏能证明送达**，所以这一档不叫 delivered。
	NotifySent NotifyOutcome = "sent"
	// NotifySkipped 没发：缺必填、总开关没开、这一层没接实现。
	NotifySkipped NotifyOutcome = "skipped"
	// NotifyFailed 发了但判据不过（含非 2xx、含回了 JSON 但业务码非 0）。
	NotifyFailed NotifyOutcome = "failed"
)

// String 用于事件流与日志里的那一行。
func (o NotifyOutcome) String() string { return string(o) }

// NotifyResult 是一家渠道这一次的结论。
//
// ⚠ `Reason` 里**只放**：缺哪个字段、HTTP 状态码、渠道自己回的那句人话（截断）、
// 传输层错误的**类别**（[transportReason]，不含原文）。
// 绝不放请求 URL —— `serverChan` / `tgBot` / `bark` / `twoIm` 的密钥就在 URL 里，
// 而这一句将来会进 `run_events`，进了库就等于把密钥抄进了一张只读表。
type NotifyResult struct {
	Channel core.NotifyChannel
	Outcome NotifyOutcome
	Reason  string
}

// NotifyReport 是一次推送的逐家结果。
type NotifyReport []NotifyResult

// Count 返回某一档的家数。
func (r NotifyReport) Count(outcome NotifyOutcome) int {
	n := 0
	for _, one := range r {
		if one.Outcome == outcome {
			n++
		}
	}
	return n
}

// SummaryLine 是给人看的那一句（进 run 小结，不进 API 的结构化字段）。
//
// 三档全报，包括 `0`：`已发 0 / 失败 1 / 跳过 3` 比"没有可报"信息量大 ——
// 这一族最常见的现象就是"什么都没发出去"，而它的原因分布在三档里。
func (r NotifyReport) SummaryLine() string {
	if len(r) == 0 {
		return "没配任何推送渠道"
	}
	var parts []string
	for _, outcome := range []NotifyOutcome{NotifySent, NotifyFailed, NotifySkipped} {
		parts = append(parts, fmt.Sprintf("%s %d", outcomeLabel(outcome), r.Count(outcome)))
	}
	line := strings.Join(parts, " / ")
	if failed := r.Count(NotifyFailed); failed > 0 {
		line += "：" + strings.Join(r.failedChannels(), "、")
	}
	return line
}

func outcomeLabel(outcome NotifyOutcome) string {
	switch outcome {
	case NotifySent:
		return "已发"
	case NotifyFailed:
		return "失败"
	case NotifySkipped:
		return "跳过"
	}
	return string(outcome)
}

func (r NotifyReport) failedChannels() []string {
	var names []string
	for _, one := range r {
		if one.Outcome == NotifyFailed {
			names = append(names, string(one.Channel))
		}
	}
	return names
}

// NotifySender 把一条消息发到配置里的各家。
//
// `mail` 允许是 `nil`：只有配了 email 那一家才需要它。传 `nil` 时 email
// 记一条 skipped（"本进程没接 SMTP 实现"），**不是** panic 也不是 failed ——
// CLI 那一条路不接邮件是正常用法。
type NotifySender struct {
	http  core.HTTPClient
	mail  core.MailSender
	clock core.Clock
}

// NewNotifySender 用注入的 HTTP 客户端、（可空的）邮件发送器与时钟构造。
//
// 时钟不是可选的：钉钉加签要一个时间戳，而本仓库的业务代码**禁止**直接
// `time.Now()`（判据见 [core.Clock] 的注释）。传 `nil` 会让带 secret 的
// 钉钉那一家记一条 failed（"没有时钟"），而不是偷偷拿系统时间——
// 那样测试里钉不住签名，正是这一族最容易出现"看起来对、其实每次都不一样"的地方。
func NewNotifySender(client core.HTTPClient, mail core.MailSender, clock core.Clock) *NotifySender {
	return &NotifySender{http: client, mail: mail, clock: clock}
}

// SendAll 按固定顺序逐家发。
//
// 三家失败不会中断后面的：渠道之间没有依赖，"第一家挂了就不发第二家"
// 会把"我配的钉钉坏了"表现成"我的 bark 也没收到"，那是最难查的一种连带。
// 反过来，这里也**不并发** —— 并发发送会让十家共用一个连接池去撞外网，
// 而这一族的失败绝大多数是"某一家网络不通"，串行时它只拖慢这一次，
// 并行时它会与别家的超时混成一句看不懂的话。
func (s *NotifySender) SendAll(
	ctx context.Context, channels core.NotifyChannels, title, text string,
) NotifyReport {
	var report NotifyReport
	for _, name := range channels.Names() {
		report = append(report, s.sendOne(ctx, name, channels, title, text))
	}
	return report
}

// sendOne 发一家：先过必填判据，再进各家的实现。
func (s *NotifySender) sendOne(
	ctx context.Context, name core.NotifyChannel, channels core.NotifyChannels,
	title, text string,
) NotifyResult {
	if missing := channels.Missing(name); missing != "" {
		return NotifyResult{
			Channel: name, Outcome: NotifySkipped,
			Reason: "缺少必填项：" + missing,
		}
	}
	switch name {
	case core.NotifyPushplus:
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return PushplusRequest(*channels.Pushplus, title, text)
		})
	case core.NotifyServerChan:
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return ServerChanRequest(*channels.ServerChan, title, text)
		})
	case core.NotifyWorkWeixinBot:
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return WorkWeixinBotRequest(*channels.WorkWeixinBot, title, text)
		})
	case core.NotifyTGBot:
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return TGBotRequest(*channels.TGBot, title, text)
		})
	case core.NotifyBark:
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return BarkRequest(*channels.Bark, title, text)
		})
	case core.NotifyTwoIM:
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return TwoIMRequest(*channels.TwoIM, title, text)
		})
	case core.NotifyDingTalk:
		// 加签才需要时间戳，而时刻只能来自注入的 [core.Clock]（本层不许 `time.Now()`）。
		// 没接时钟时带 secret 的那一家记 failed 而不是拿系统时间兜底 —— 兜底会让
		// "签名对不对"这件事在测试里根本没有入口。签名算法本身由 [DingTalkSignedURL] 直接断言。
		//
		// ⚠ **没配 secret 时不需要时刻**（那一枪只有 `access_token`，见 [DingTalkSignedURL]），
		// 所以那种用法不能因为"没接时钟"被判失败：只填 webhook 地址不加签是这一家的常见配法，
		// 拿"没时钟"挡它等于把正常用法挡在门外。
		signed := channels.DingTalk.Secret
		stamp, ok := notifyTimestampMS(s.clock)
		if !ok && strings.TrimSpace(signed.Value()) != "" {
			return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: "本进程没接时钟，无法加签"}
		}
		return s.sendBuilt(ctx, name, func() (core.HTTPRequest, *core.Error) {
			return DingTalkRequest(*channels.DingTalk, title, text, stamp)
		})
	case core.NotifyWorkWeixin:
		return s.sendWorkWeixin(ctx, channels.WorkWeixin, title, text)
	case core.NotifyCustomPost:
		return s.sendCustomPosts(ctx, channels.CustomPost, title, text)
	case core.NotifyEmail:
		return s.sendEmail(ctx, channels.Email, title, text)
	}
	return NotifyResult{Channel: name, Outcome: NotifySkipped, Reason: "本仓没有这一家的实现"}
}

// sendBuilt 把"构造 + 发送 + 记账"收成一步：构造失败也算这一家的 failed，
// 而不是让整次推送炸掉 —— 一家配置里的 URL 拼不出来，不该连带另外三家不发。
func (s *NotifySender) sendBuilt(
	ctx context.Context, name core.NotifyChannel,
	build func() (core.HTTPRequest, *core.Error),
) NotifyResult {
	request, perr := build()
	if perr != nil {
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: "请求构造失败"}
	}
	return s.sendRequest(ctx, name, request)
}

// sendRequest 发一个请求并按渠道判据记账。
func (s *NotifySender) sendRequest(
	ctx context.Context, name core.NotifyChannel, request core.HTTPRequest,
) NotifyResult {
	response, err := s.http.Send(ctx, request)
	if err != nil {
		if response.Status != 0 {
			// 请求发出去了、状态码非 2xx：`io.Client` 会把这两种情况一起返回
			// （它按"非 2xx 即错误"口径给 err），这里要说的是状态码，
			// 不是"未发出"。
			return NotifyResult{Channel: name, Outcome: NotifyFailed,
				Reason: fmt.Sprintf("HTTP %d", response.Status)}
		}
		// 传输层错误**只留类别**：`io.Client` 那句里带着 `Get "https://…"` 的完整 URL，
		// 而 serverChan / bark / tgBot / twoIm 四家的密钥就在 URL 里 —— 这一句将来进
		// `run_events`（一张只读表），照抄错误串等于把密钥抄进去。
		// 代价是排障时少一句原文；那是日志该管的事，不是这一栏。
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: transportReason(err)}
	}
	if !response.IsSuccess() {
		return NotifyResult{
			Channel: name, Outcome: NotifyFailed,
			Reason: fmt.Sprintf("HTTP %d", response.Status),
		}
	}
	reason, ok := classifyChannelReply(response.Body)
	if !ok {
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: reason}
	}
	return NotifyResult{Channel: name, Outcome: NotifySent, Reason: reason}
}

// ─────────────────────────────────────────────────────────────────────────────
// 渠道响应的公共判据
// ─────────────────────────────────────────────────────────────────────────────

// classifyChannelReply 判一次渠道响应算不算"被收下"，并给出人能看的一句原因。
//
// # 判据的来源与它的边界
//
// 旧实现那一支是 `const {errcode, code, err} = data` 加
// `if (errcode || err || ![0,200,undefined].some(c => code === c))`。
// 这一支是**十个渠道共用**的，所以它必然对某一家判松、对某一家判紧：
//
//   - 判松的那几家：只回了 `{"message":"ok"}` 之类的服务，这里会算成功。
//     这是旧实现的行为，也是"没有样本时不敢收紧"的保守侧。
//   - 判紧的那一家：Telegram 的 `{"ok":false,"error_code":401}` —— 见本文件抬头第 3 条。
//   - ⚠ **比旧实现更松的一格**：`code` 是**字符串数字**时（`{"code":"200"}`、`{"code":"0"}`）
//     这里算成功，而旧实现那句 `code === c` 是严格相等 ⇒ 判失败。方向是"少报一次失败"，
//     与上面 `success=false` 那类"把失败当成功"不对称的代价并不相同（推送没有幂等，
//     重放的代价是用户多收一条），所以先按"第三方回字符串码很常见"这一侧落，
//     登记进 `docs/pending-decisions.md`（要样本那一档）：拿到任一家真响应体后回看这一格。
//
// ⚠ **十家一家样本都没有**（本仓所有归档里没有任何第三方推送响应）。
// 所以这套判据只钉到"与旧实现同口径 + 明显判反的那一格修掉"，
// 没钉到"某家真回什么"。要补：每家手工配一次、真发一条，把响应体逐字记进
// `docs/capture/`（那是**第三方**服务，不碰云盘账号，风险面只有那一条消息）。
//
// 返回值第二项是"算不算成功"；第一项是给人看的一句原因（成功时是渠道原话，
// 常常为空串 —— 空串不是错误）。
func classifyChannelReply(body []byte) (string, bool) {
	value, ok := core.ParseJSON(string(body))
	if !ok {
		// 非 JSON：可能是网关页、可能是 HTML。判失败，而不是"没有码所以算成功"——
		// 这一格判反的表现是"渠道全坏了但小结写着已发"。
		return "响应不是 JSON", false
	}
	obj, isObject := value.(map[string]any)
	if !isObject {
		return "响应不是对象", false
	}
	// Telegram / 部分自建网关用 `ok`。旧实现漏了这一键。
	if flag, isBool := obj["ok"].(bool); isBool {
		if !flag {
			return "渠道回了 ok=false：" + replyMessage(obj), false
		}
		return replyMessage(obj), true
	}
	if code, present := obj["errcode"]; present {
		if n, isNumber := asInt64(code); isNumber && n != 0 {
			return fmt.Sprintf("渠道回了 errcode=%d：%s", n, replyMessage(obj)), false
		}
	}
	if errField, present := obj["err"]; present && truthy(errField) {
		return "渠道回了 err：" + replyMessage(obj), false
	}
	if code, present := obj["code"]; present {
		// 旧口径：`[0, 200, undefined].includes(code)`。`null` 不在其中 ⇒ 判失败。
		if n, isNumber := asInt64(code); !isNumber || (n != 0 && n != 200) {
			return fmt.Sprintf("渠道回了 code=%v", code) + "：" + replyMessage(obj), false
		}
	}
	return replyMessage(obj), true
}

// replyMessage 从各家不同的键名里挑一句人话出来。截断到 120 个字符：
// 这一句要进 `run_events`，而某些网关会把整页 HTML 塞进 `msg`。
func replyMessage(obj map[string]any) string {
	// 键序按"哪一家先回得出人话"排：企业微信用 errmsg、钉钉/多数网关用 msg 或 message、
	// Telegram 用 description、自建网关用 error / err。
	for _, key := range []string{"errmsg", "msg", "message", "error_description", "description", "error", "err"} {
		if text, ok := obj[key].(string); ok && strings.TrimSpace(text) != "" {
			return clipRunes(text, 120)
		}
	}
	return ""
}

// truthy 是 JS 那几个判断在 Go 里的对应物：`null` / `false` / `""` / `0` 算假。
//
// ⚠ 空**对象** `{}` 与空数组在 JS 里是**真**，这里同口径（只把上面四种收成假），
// 因为旧实现就是 `if (err)` —— 收紧这条会让某些渠道从"成功"变"失败"，
// 而那正是我们没有样本的地方。
func truthy(value any) bool {
	switch v := value.(type) {
	case nil:
		return false
	case bool:
		return v
	case string:
		return v != ""
	case json.Number:
		n, err := v.Int64()
		return err != nil || n != 0
	}
	return true
}

// transportReason 把传输层错误折成"一句不含 URL 的话"。
//
// 只用 [core.KindOf] 的类别，不抄原文：`io.Client` 的那句里带着
// `Get "https://…"` 的完整地址，而四家的密钥就在地址里（见 [NotifyResult] 的字段注）。
// 非本仓的 error（第三方客户端直接返回的裸 error）走"未分类"那一句 ——
// 不默认成 Internal，因为那会把"我们没判出来"念成"内部错误已定位"。
func transportReason(err error) string {
	kind, ok := core.KindOf(err)
	if !ok {
		return "请求未发出：未分类的传输错误"
	}
	return "请求未发出：" + kind.PublicMessage()
}

// redactSecrets 把文本里出现的这几个凭据原样替换成占位符。
//
// ⚠ 只在文本**已经**含该凭据时才替换：空凭据拿去 `ReplaceAll` 会把整句拆得全是占位符，
// 而空凭据本来也不可能泄漏。
func redactSecrets(text string, secrets ...core.SecretValue) string {
	for _, secret := range secrets {
		value := secret.Value()
		if value == "" {
			continue
		}
		text = strings.ReplaceAll(text, value, "‹已脱敏›")
	}
	return text
}

// clipRunes 按**字符**截断（不是字节）：这一句里几乎一定有中文，
// 按字节切会把一个汉字切成两半，界面拿到的是乱码尾巴。
func clipRunes(text string, max int) string {
	runes := []rune(text)
	if len(runes) <= max {
		return text
	}
	return string(runes[:max]) + "…"
}
