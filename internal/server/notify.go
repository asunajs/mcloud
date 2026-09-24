// 推送的**发送侧**装配：进程级一份 [NotifyWorld]，加上"试发"那一条接口。
//
// 配置的读写在 `notify_config.go`，出网视图在 `notify_view.go`。
//
// # 为什么"试发"与"运行收尾那一推"共用一份装配
//
// 两者要打的是同样那十家服务，而"测试里漏注入假客户端"的后果在两条路上是**同一种**
// 失效：真出门、真发消息。分两处装配就会有两份出站面孔，那时"哪一条路被顶掉了"
// 只能靠读代码判断。所以这里只构造一次（[Deps.notifyWorld]），两条路都从它走。
//
// # 试发**不看**总开关，但**看**门槛
//
// 分开的理由是这两格各自在回答不同的问题：
//   - `enabled`（总开关）管的是"自动推不推"。人在界面上点了一下"试发"就是
//     一次显式动作，再拿"你还没开自动推送"挡它，等于让这一栏只有在开了之后才能测 ——
//     而"先测通再开"正是它唯一的用法。
//   - `minLevel`（正文门槛）**不**绕过：试发走的是与真发**同一个**
//     [app.FormatNotifyBody]，绕过的话"试发收到了、正式跑却不响"就成了查不出来的现象
//     （这一族最常见的两种失败恰好就是这一对）。
//   - `onlyError` 同样不照搬：它的判据是"这一次运行里有没有 error 那一档"，
//     而试发正文里**固定带一条 error 行**（见下），所以它天然过闸，
//     顺带也验证了"只推错误时我配的那一家响不响"。
package server

import (
	"net/http"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// NotifyWorld 是"推送"那一份常驻装配。
//
// 住在 [Server] 上而不是在处理器里现拼，理由与 [Server.hub] 同一条，而且这里更硬：
// 它持有的发送器带**出站 HTTP 客户端**与**SMTP 出口**，本包自己不造真的 ——
// 那样"测试里漏注入"的表现是真往第三方发消息（用户手机上就多一条），不是变红。
//
// 它同时被两条路用：`run` 收尾那一推（[Server.launchRun] 经 [NotifyWorld.RunNotify]）
// 与界面上的"试发"（`POST /api/notify/test`）。
type NotifyWorld struct {
	sender *upstream.NotifySender
	run    *app.RunNotify
	clock  core.Clock
	// loc 只影响试发正文里那串时刻的显示，与运行收尾那一路同一个来源
	// （[app.RunNotify] 内部用的是 [core.LocalTZ]，这里对齐，两条路才拿得到同一形状）。
	loc *time.Location
}

// NewNotifyWorld 装配推送的发送侧。
//
// `mail` 允许 nil：只有配了 email 那一家才用得到 SMTP 出口，
// nil 时那一格由上游层记成"本进程没接 SMTP 实现"的 skipped（不是 panic）。
// `client` 与 `clock` **不该**为 nil —— 前者决定消不消息出网，后者决定
// 钉钉加签那个时间戳（没有它，带 secret 的那一家会记 failed）。
func NewNotifyWorld(
	configs app.RunNotifyConfigReader, client core.HTTPClient,
	mail core.MailSender, clock core.Clock,
) *NotifyWorld {
	sender := upstream.NewNotifySender(client, mail, clock)
	return &NotifyWorld{
		sender: sender,
		run:    app.NewRunNotify(configs, sender, clock),
		clock:  clock,
		loc:    core.LocalTZ(),
	}
}

// RunNotify 是给运行收尾用的那一份（[app.WithRunNotify] 的参数）。
func (w *NotifyWorld) RunNotify() *app.RunNotify {
	if w == nil {
		return nil
	}
	return w.run
}

// notifyTestBody 是试发的请求体（三格都可选，空体 `{}` 就是"按默认试一条"）。
type notifyTestBody struct {
	// Title 覆盖配置里的标题（`nil` = 用配置那一份）。
	Title *string `json:"title"`
	// Message 是 info 那一行的正文（`nil` 或空 = 用一句固定的话）。
	Message *string `json:"message"`
	// Channels 只发这几家（空 = 配置里必填齐的那几家）。
	//
	// ⚠ 这里是**收窄**而不是扩面：写了没配的渠道会 400，不会因此凭空多打一家 ——
	// 一个界面输入框就能让这台机器往任意 URL 发请求，那是 SSRF 的形状。
	Channels []string `json:"channels"`
}

func (s *Server) testNotify(w http.ResponseWriter, r *http.Request, _ *AuthUser) {
	if s.notify == nil || s.notify.sender == nil {
		// 装配漏了一行，不是"用户配错了"：报 500，不去猜"要不要跳过这一格"。
		ErrDomain(core.NewInternal("本进程没接推送发送器")).Write(w)
		return
	}
	body := notifyTestBody{}
	// 没有请求体是合法用法（界面上那枚按钮不填任何东西）。
	// ⚠ 只认 `ContentLength == 0`：读过的流不能二次解码，所以这里**不**用"解失败就当空"。
	if r.ContentLength != 0 {
		decoded, ok := decodeJSON[notifyTestBody](w, r)
		if !ok {
			return
		}
		body = decoded
	}
	ctx := r.Context()
	config, err := s.store.NotifyConfig(ctx)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	targets, perr := notifyTestTargets(config, body.Channels)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	lines := notifyTestLines(s.notify.clock, body.Message)
	text := app.FormatNotifyBody(lines, config.MinLevel, s.notify.location())
	if text == "" {
		// 与运行收尾那一路同一句判据（那里是 skip 原因，这里没有"跳过"可言，
		// 就直接把原因回给人看）。
		//
		// ⚠ **这一格未被断言，而且今天测不到**：门槛最高只到 `error`（见
		// [core.NotifyLevels]），而 [notifyTestLines] 固定带一条 error 行 ⇒ 正文
		// 永不为空。留着它的理由是"将来若给试发加档位下拉框（只发 info）",
		// 这里会第一次响。要断言它得先让 `notifyTestLines` 的行可配 —— 那正是
		// 上面那个假设的功能，不该为了覆盖率先造它。
		ErrDomain(core.NewBadRequest(
			"当前门槛下试发的正文为空（门槛：" + string(config.MinLevel) + "）")).Write(w)
		return
	}
	title := config.TitleOrDefault()
	if body.Title != nil {
		title = *body.Title
	}
	report := s.notify.sender.SendAll(ctx, targets, title, text)
	rows := make([]map[string]any, 0, len(report))
	for _, one := range report {
		// `Reason` 由上游层保证不含 URL 与凭据（见 `internal/upstream/notify.go` 的
		// [upstream.NotifyResult] 字段注），所以这一栏可以原样出网。
		rows = append(rows, map[string]any{
			"channel": string(one.Channel),
			"outcome": one.Outcome.String(),
			"reason":  one.Reason,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"title":   title,
		"body":    text,
		"results": rows,
		"summary": report.SummaryLine(),
		// 让人看得见这一条**没有**经过的那些闸：试发成功不等于自动推会成功。
		"gates": map[string]any{
			"enabled":   config.Enabled,
			"onlyError": config.OnlyError,
			"minLevel":  string(config.MinLevel),
		},
	})
}

// notifyTestTargets 决定"这一枪打给哪几家"，并顺手判掉两种说不通的写法。
//
// 缺必填的渠道**保留**在目标里：让它进 `SendAll` 才会得到那句"缺少必填项：token"，
// 而这一族最常见的现场恰恰是"配了但没填全" —— 在边界上先滤掉，界面上就只剩
// "什么都没发生"。
func notifyTestTargets(config core.NotifyConfig, requested []string) (core.NotifyChannels, *core.Error) {
	configured := map[core.NotifyChannel]bool{}
	for _, name := range config.Channels.Names() {
		configured[name] = true
	}
	picked := make([]core.NotifyChannel, 0, len(configured))
	if len(requested) == 0 {
		picked = append(picked, config.Channels.Names()...)
	} else {
		seen := map[core.NotifyChannel]bool{}
		for _, raw := range requested {
			name := core.NotifyChannel(raw)
			if !name.Valid() {
				return core.NotifyChannels{}, core.NewBadRequest("没有这个推送渠道: " + raw)
			}
			if !configured[name] {
				return core.NotifyChannels{}, core.NewBadRequest("这一家没有配置，试发不到它: " + raw)
			}
			if !seen[name] {
				seen[name] = true
				picked = append(picked, name)
			}
		}
		// 按固定序排：一次点下去十家的先后不该随请求体里数组的写法变
		// （与 [core.NotifyChannelOrder] 钉死顺序同一条理由）。
		picked = orderNotifyChannels(picked)
	}
	if len(picked) == 0 {
		return core.NotifyChannels{}, core.NewBadRequest("一家推送渠道都没配，没有可试发的目标")
	}
	return pickNotifyChannels(config.Channels, picked), nil
}

func orderNotifyChannels(names []core.NotifyChannel) []core.NotifyChannel {
	seen := make(map[core.NotifyChannel]bool, len(names))
	for _, name := range names {
		seen[name] = true
	}
	out := make([]core.NotifyChannel, 0, len(seen))
	for _, name := range core.NotifyChannelOrder() {
		if seen[name] {
			out = append(out, name)
		}
	}
	return out
}

// pickNotifyChannels 只留下 `keep` 里的那些家（其余指针留 nil）。
//
// 之所以不能直接把 `config.Channels` 整个递进去：那等于"试发其中一家"
// 变成"十家全发"，而用户点的那一枚按钮说的是前者。
func pickNotifyChannels(all core.NotifyChannels, keep []core.NotifyChannel) core.NotifyChannels {
	wanted := make(map[core.NotifyChannel]bool, len(keep))
	for _, name := range keep {
		wanted[name] = true
	}
	var out core.NotifyChannels
	if wanted[core.NotifyPushplus] {
		out.Pushplus = all.Pushplus
	}
	if wanted[core.NotifyServerChan] {
		out.ServerChan = all.ServerChan
	}
	if wanted[core.NotifyWorkWeixin] {
		out.WorkWeixin = all.WorkWeixin
	}
	if wanted[core.NotifyWorkWeixinBot] {
		out.WorkWeixinBot = all.WorkWeixinBot
	}
	if wanted[core.NotifyTGBot] {
		out.TGBot = all.TGBot
	}
	if wanted[core.NotifyBark] {
		out.Bark = all.Bark
	}
	if wanted[core.NotifyTwoIM] {
		out.TwoIM = all.TwoIM
	}
	if wanted[core.NotifyDingTalk] {
		out.DingTalk = all.DingTalk
	}
	if wanted[core.NotifyEmail] {
		out.Email = all.Email
	}
	if wanted[core.NotifyCustomPost] {
		out.CustomPost = all.CustomPost
	}
	return out
}

// notifyTestLines 是试发那三行正文。
//
// 三行的**档**是刻意选的：info 那一条是"我写的这句话"，warn 那一条用来看门槛
// 到底卡在哪一档（设 warn 时正文里应该只剩两行），error 那一条是给 `onlyError`
// 与"门槛设成 error"那两种配置留一条能收到的样本。
func notifyTestLines(clock core.Clock, message *string) []app.NotifyLogLine {
	now := time.Time{}
	if clock != nil {
		now = clock.Now()
	}
	text := "这是一条测试推送。"
	if message != nil {
		if trimmed := strings.TrimSpace(*message); trimmed != "" {
			text = trimmed
		}
	}
	return []app.NotifyLogLine{
		{Level: core.LevelInfo, At: now, Message: text},
		{Level: core.LevelWarn, At: now, Message: "（试发）这一条是 warn 档，用来核对正文门槛"},
		{Level: core.LevelError, At: now, Message: "（试发）这一条是 error 档，只推错误时也应当收到它"},
	}
}

func (w *NotifyWorld) location() *time.Location {
	if w == nil || w.loc == nil {
		return core.LocalTZ()
	}
	return w.loc
}
