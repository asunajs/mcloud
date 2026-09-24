// 推送（通知）渠道的**模型** —— 只有数据与判据，一次请求都不发。
//
// 对应旧实现 `packages/shared/src/push/types.ts`（122 行）+ `index.ts` 里的
// `CHANNEL_MAP`（十个键）。发送在 `internal/upstream/notify*.go`，
// 持久化在 `internal/io`（单行表 `notify_config`），正文组装在 `internal/app`。
//
// # 为什么这一族住 core 而不是 upstream
//
// 配置要被 server 读写（界面上那一格）、被 io 持久化、被 app 判"该不该推"、
// 被 upstream 拼成请求 —— 四层都要用同一个类型，而依赖方向只允许它们都指向 core。
// 放在 upstream 会让 app 之外的层拿不到它；放在 app 会让 upstream 反过来依赖 app。
//
// # 为什么每个渠道是一个**具名结构**，不是 `map[string]any`
//
// 旧实现是 `Record<string, unknown>` 加上一句 `...config` 展开 —— 于是"配错了键名"
// 这种失效**不会报错**，只是那个参数静默不上线（把 `chat_id` 写成 `chatId` 的现场
// 就是"推送莫名不发"，而日志里一句都没有）。这里把必填项收成字段，
// 剩下的自由键仍收进 `Extra` 并照旧合并到请求体（用户要靠它带 `topic` / `sound` /
// `at` 这些我们没枚举的参数），但**拼错了的必填项会在判据那一格被拦下来**。
//
// # ⚠ `Extra` 的合并顺序 = 旧实现的方向（后写覆盖先写）
//
// 旧实现 `body: {token, title, content: text, ...config}` —— 展开在**最后**，
// 所以用户Extra 里的同名键**覆盖**我们拼的那一个。这里保留这个方向
// （它让用户能改写 `content`），但覆盖时会在结果里记一条 `overridden`，
// 免得"我明明配了 topic 怎么没生效"反过来变成"配了 content 把正文换了却没人在场"。
package core

import (
	"fmt"
	"sort"
	"strings"
	"time"
)

// NotifyChannel 是渠道名。**取值逐字对齐**旧 `CHANNEL_MAP` 的键
// （大小写也要对齐：`serverChan` / `dingTalk` 是驼峰，其余是全小写或 Bot 结尾的驼峰）。
//
// 它是对外标识（库里的 JSON、界面上的选择项），所以这些字面量**不是内部细节**：
// 改一个就等于把用户已配的渠道改名 —— 需要迁移，不只是改常量。
type NotifyChannel string

const (
	NotifyPushplus      NotifyChannel = "pushplus"
	NotifyServerChan    NotifyChannel = "serverChan"
	NotifyWorkWeixin    NotifyChannel = "workWeixin"
	NotifyWorkWeixinBot NotifyChannel = "workWeixinBot"
	NotifyTGBot         NotifyChannel = "tgBot"
	NotifyBark          NotifyChannel = "bark"
	NotifyTwoIM         NotifyChannel = "twoIm"
	NotifyDingTalk      NotifyChannel = "dingTalk"
	NotifyEmail         NotifyChannel = "email"
	NotifyCustomPost    NotifyChannel = "customPost"

	notifyLevelMaxUnknown = -1
)

// NotifyChannelOrder 是**发送顺序**，也是界面上的列举顺序。
//
// ⚠ 与旧实现有一处**刻意不同**：旧版是 `Object.entries(config)` ——
// 顺序等于用户配置文件里键的书写顺序。Go 的 map 没有顺序，而"没有顺序"
// 表现成"同一份配置两次跑，日志里渠道的先后不一样"，那比固定序难查得多。
// 这里钉成旧 `CHANNEL_MAP` 的**声明序**；渠道之间没有依赖，所以顺序只影响读感，
// 不影响结果。
func NotifyChannelOrder() []NotifyChannel {
	return []NotifyChannel{
		NotifyPushplus, NotifyServerChan, NotifyWorkWeixin, NotifyWorkWeixinBot,
		NotifyTGBot, NotifyBark, NotifyTwoIM, NotifyDingTalk, NotifyEmail,
		NotifyCustomPost,
	}
}

// Valid 是否是已知渠道名。
//
// 未知名字的处置是**逐渠道跳过并记一条原因**，不是整份配置作废：
// 用户手改库里的 JSON 打错一个键，不该让另外四条配好的渠道也跟着不发。
func (c NotifyChannel) Valid() bool {
	for _, known := range NotifyChannelOrder() {
		if known == c && c != "" {
			return true
		}
	}
	return false
}

// NotifyLogLevel 是"哪些日志行进正文"的那道门槛。
//
// 语义沿用旧 `types.ts` 的注释：error 只收错误 / fail / fatal，
// warn 是 warn 及以上，info 是默认，debug 是全量。
type NotifyLogLevel string

const (
	NotifyLevelError NotifyLogLevel = "error"
	NotifyLevelWarn  NotifyLogLevel = "warn"
	NotifyLevelInfo  NotifyLogLevel = "info"
	NotifyLevelDebug NotifyLogLevel = "debug"
)

// notifyLevelCeiling 是旧 `PUSH_LEVEL_MAX` 的逐字复刻（`error:0 warn:1 info:3 debug:4`）。
//
// ⚠ 这里**没有 2 那一档**，而旧实现的过滤是 `entry.level <= max` ——
// 也就是说"级别 2"那一档在旧版就不存在（`levelMap` 里也没有映射到 2 的 type）。
// 不补、不解释成 bug：本仓的日志级别集合与它同源，改这张表要连 `internal/app`
// 的映射一起改，否则门槛会整体偏一档。
var notifyLevelCeiling = map[NotifyLogLevel]int{
	NotifyLevelError: 0,
	NotifyLevelWarn:  1,
	NotifyLevelInfo:  3,
	NotifyLevelDebug: 4,
}

// LevelCeiling 返回该门槛对应的上界；未知门槛返回 -1（调用方据此判"不认识"）。
func (l NotifyLogLevel) LevelCeiling() int {
	if v, ok := notifyLevelCeiling[l]; ok {
		return v
	}
	return notifyLevelMaxUnknown
}

// NotifyLevels 返回四个合法门槛（界面下拉框与校验共用一张表）。
func NotifyLevels() []NotifyLogLevel {
	return []NotifyLogLevel{NotifyLevelError, NotifyLevelWarn, NotifyLevelInfo, NotifyLevelDebug}
}

// Valid 是否是已知门槛。
func (l NotifyLogLevel) Valid() bool { return l.LevelCeiling() != notifyLevelMaxUnknown }

// ─────────────────────────────────────────────────────────────────────────────
// 十个渠道的配置
// ─────────────────────────────────────────────────────────────────────────────
//
// 每个结构都带一个 `Missing()`：缺必填时说得出**缺哪个字段**。
// 判据口径（与旧实现有三处不同，见各条注释）：缺必填 ⇒ 这一渠道**不发**，
// 并给出一条人能在界面上看懂的原因。旧实现只有 bark / tgBot / email 三条有这种
// 前置检查，其余几家是"带着空 token 也发出去"—— 那一次请求既不会成功，
// 又要在第三方那里留下一次记录，没有价值。

// PushplusConfig 是 pushplus（微信「pushplus 推送+」）。
type PushplusConfig struct {
	// Token 是用户令牌。旧实现拼进请求体，不进 URL。
	Token SecretValue `json:"token"`
	// Extra 会逐个并进请求体（旧实现是 `...config`），同名键**覆盖**上面的。
	Extra map[string]any `json:"extra,omitempty"`
}

// ServerChanConfig 是 Server酱（方糖）。
//
// ⚠ 旧实现把 token 拼在 URL 路径上：`https://sctapi.ftqq.com/{token}.send`。
// 所以这一家的 token **会出现在访问日志与 URL 里**，比放请求体的那几家更敏感一档。
type ServerChanConfig struct {
	Token SecretValue    `json:"token"`
	Extra map[string]any `json:"extra,omitempty"`
}

// WorkWeixinConfig 是企业微信**应用**消息（要 corpid + corpsecret 先换 access_token，
// 是本族唯一一条**两步**的渠道）。
type WorkWeixinConfig struct {
	CorpID     SecretValue    `json:"corpid"`
	CorpSecret SecretValue    `json:"corpsecret"`
	AgentID    *int64         `json:"agentid,omitempty"`
	ToUser     string         `json:"touser,omitempty"`
	MsgType    string         `json:"msgtype,omitempty"`
	Extra      map[string]any `json:"extra,omitempty"`
}

// WorkWeixinBotConfig 是企业微信**群机器人**（只有一个 webhook 地址，不换票）。
type WorkWeixinBotConfig struct {
	URL     string         `json:"url"`
	MsgType string         `json:"msgtype,omitempty"`
	Extra   map[string]any `json:"extra,omitempty"`
}

// TGBotConfig 是 Telegram Bot。
type TGBotConfig struct {
	// APIHost 允许自建反代；缺省 `api.telegram.org`（与旧实现同一个默认值）。
	APIHost string      `json:"api_host,omitempty"`
	Token   SecretValue `json:"token"`
	ChatID  string      `json:"chat_id"`
	// DisableWebPagePreview 缺省 **true** —— 旧实现是 `= true` 的解构默认值，
	// 不是"用户没填所以 false"。
	DisableWebPagePreview *bool          `json:"disable_web_page_preview,omitempty"`
	Extra                 map[string]any `json:"extra,omitempty"`
}

// BarkConfig 是 Bark（iOS）。
type BarkConfig struct {
	Key   SecretValue    `json:"key"`
	Level string         `json:"level,omitempty"`
	Extra map[string]any `json:"extra,omitempty"`
}

// TwoIMConfig 是回逍（2Im）。
//
// 旧实现把 `key` / `sid` 拼进 **URL 查询串**，`query` 里的键再逐个 append 上去；
// 其余字段进请求体。两个位置都要保留，否则上游认不出这一条。
type TwoIMConfig struct {
	Key     SecretValue    `json:"key"`
	SID     SecretValue    `json:"sid"`
	MsgType string         `json:"msgtype,omitempty"`
	Query   map[string]any `json:"query,omitempty"`
	Extra   map[string]any `json:"extra,omitempty"`
}

// DingTalkConfig 是钉钉群机器人。
//
// ⚠ `Secret` 非空时要**加签**（HmacSHA256 + base64 + urlencode，时间戳进查询串）。
// 这一家是本族唯一有签名的一步。
type DingTalkConfig struct {
	Token  SecretValue    `json:"token"`
	Secret SecretValue    `json:"secret,omitempty"`
	Extra  map[string]any `json:"extra,omitempty"`
}

// EmailConfig 是 SMTP 邮件。
//
// 旧实现走 nodemailer（Node 专属），`port` 缺省 465 且 `secure = (port == 465)`。
// 本仓用标准库自己发（见 `internal/io/mail.go`），那条"465 才 TLS"的口径**照抄**，
// 因为它是旧版跑得通的那一份配置的形状；587（STARTTLS）这一档**未测试**。
type EmailConfig struct {
	Host string      `json:"host"`
	Port *int        `json:"port,omitempty"`
	From SecretValue `json:"from"`
	Pass SecretValue `json:"pass"`
	To   string      `json:"to,omitempty"`
}

// CustomPostConfig 是通用 HTTP 渠道（`{title}` / `{text}` 占位符替换）。
//
// 与旧实现同口径：它可以是**一个对象也可以是数组**，数组就逐条发。
// 所以这里天然是切片。
type CustomPostConfig struct {
	Method  string            `json:"method,omitempty"`
	URL     string            `json:"url"`
	Headers map[string]string `json:"headers,omitempty"`
	Timeout *int64            `json:"timeout,omitempty"`
	Data    map[string]any    `json:"data,omitempty"`
	Body    map[string]any    `json:"body,omitempty"`
}

// ─────────────────────────────────────────────────────────────────────────────
// 一份完整的推送配置
// ─────────────────────────────────────────────────────────────────────────────

// NotifyChannels 是"配了哪几家"。字段名与渠道名一一对应；`nil` = 没配这一家。
//
// # 为什么不用 `map[NotifyChannel]any`
//
// map 的 value 只能是 `any`，于是"配错了键名"这件事重新变成静默的
// （本文件抬头说的第二个坑就是这么来的），而且界面拿不到字段清单。
type NotifyChannels struct {
	Pushplus      *PushplusConfig      `json:"pushplus,omitempty"`
	ServerChan    *ServerChanConfig    `json:"serverChan,omitempty"`
	WorkWeixin    *WorkWeixinConfig    `json:"workWeixin,omitempty"`
	WorkWeixinBot *WorkWeixinBotConfig `json:"workWeixinBot,omitempty"`
	TGBot         *TGBotConfig         `json:"tgBot,omitempty"`
	Bark          *BarkConfig          `json:"bark,omitempty"`
	TwoIM         *TwoIMConfig         `json:"twoIm,omitempty"`
	DingTalk      *DingTalkConfig      `json:"dingTalk,omitempty"`
	Email         *EmailConfig         `json:"email,omitempty"`
	CustomPost    []CustomPostConfig   `json:"customPost,omitempty"`
}

// Names 返回已配置（非 nil）的渠道名，按 [NotifyChannelOrder] 的顺序。
func (c NotifyChannels) Names() []NotifyChannel {
	var names []NotifyChannel
	for _, name := range NotifyChannelOrder() {
		if c.present(name) {
			names = append(names, name)
		}
	}
	return names
}

// present 判一家配没配。单独一个函数是为了让"新增渠道忘了加分支"这件事
// 集中在一处可见的 switch 上，而不是散进两个调用点。
func (c NotifyChannels) present(name NotifyChannel) bool {
	switch name {
	case NotifyPushplus:
		return c.Pushplus != nil
	case NotifyServerChan:
		return c.ServerChan != nil
	case NotifyWorkWeixin:
		return c.WorkWeixin != nil
	case NotifyWorkWeixinBot:
		return c.WorkWeixinBot != nil
	case NotifyTGBot:
		return c.TGBot != nil
	case NotifyBark:
		return c.Bark != nil
	case NotifyTwoIM:
		return c.TwoIM != nil
	case NotifyDingTalk:
		return c.DingTalk != nil
	case NotifyEmail:
		return c.Email != nil
	case NotifyCustomPost:
		return len(c.CustomPost) > 0
	}
	return false
}

// Missing 返回该渠道**缺少的必填项**（`""` = 齐了）。
//
// customPost 的必填只有"有 URL"，与旧实现一致（`if (!apiTemplate?.url) return;`）；
// 其余几家缺 token / key / 地址就是发不出去的，所以拦在发送前。
func (c NotifyChannels) Missing(name NotifyChannel) string {
	switch name {
	case NotifyPushplus:
		if c.Pushplus == nil {
			return "pushplus"
		}
		return fieldOf(c.Pushplus.Token.Value(), "token")
	case NotifyServerChan:
		if c.ServerChan == nil {
			return "serverChan"
		}
		return fieldOf(c.ServerChan.Token.Value(), "token")
	case NotifyWorkWeixin:
		if c.WorkWeixin == nil {
			return "workWeixin"
		}
		return firstMissing(
			fieldOf(c.WorkWeixin.CorpID.Value(), "corpid"),
			fieldOf(c.WorkWeixin.CorpSecret.Value(), "corpsecret"),
		)
	case NotifyWorkWeixinBot:
		if c.WorkWeixinBot == nil {
			return "workWeixinBot"
		}
		return fieldOf(c.WorkWeixinBot.URL, "url")
	case NotifyTGBot:
		if c.TGBot == nil {
			return "tgBot"
		}
		return firstMissing(
			fieldOf(c.TGBot.Token.Value(), "token"),
			fieldOf(c.TGBot.ChatID, "chat_id"),
		)
	case NotifyBark:
		if c.Bark == nil {
			return "bark"
		}
		return fieldOf(c.Bark.Key.Value(), "key")
	case NotifyTwoIM:
		if c.TwoIM == nil {
			return "twoIm"
		}
		return firstMissing(
			fieldOf(c.TwoIM.Key.Value(), "key"),
			fieldOf(c.TwoIM.SID.Value(), "sid"),
		)
	case NotifyDingTalk:
		if c.DingTalk == nil {
			return "dingTalk"
		}
		return fieldOf(c.DingTalk.Token.Value(), "token")
	case NotifyEmail:
		if c.Email == nil {
			return "email"
		}
		// 旧 email.node.ts：`if (!pass || !from || !host) return;`
		return firstMissing(
			fieldOf(c.Email.Host, "host"),
			fieldOf(c.Email.From.Value(), "from"),
			fieldOf(c.Email.Pass.Value(), "pass"),
		)
	case NotifyCustomPost:
		if len(c.CustomPost) == 0 {
			return "customPost"
		}
		for _, item := range c.CustomPost {
			if strings.TrimSpace(item.URL) == "" {
				return "url"
			}
		}
	}
	return ""
}

// NotifyConfig 是一份推送配置（单行表 `notify_config`）。
type NotifyConfig struct {
	// Enabled 是**总开关**。默认关闭：这一族会把运行正文发到第三方
	// （以及用户自己的邮箱），默认开启等于"配置一迁过来就往外发"。
	Enabled bool `json:"enabled"`
	// Title 是推送标题；`nil` = 用默认标题（[DefaultNotifyTitle]）。
	Title *string `json:"title,omitempty"`
	// OnlyError 复刻旧 `onlyError`：一条 error 都没有就**整次不发**。
	OnlyError bool `json:"onlyError"`
	// MinLevel 是正文收哪些级别的日志。空串按 [NotifyLevelInfo] 走
	// （旧实现的默认值），校验时不认第三种写法。
	MinLevel NotifyLogLevel `json:"minLevel"`
	// Channels 是"配了哪几家"。
	Channels NotifyChannels `json:"channels"`
	// UpdatedAt 由存储层维护，业务不读它。
	UpdatedAt *time.Time `json:"updated_at,omitempty"`
}

// DefaultNotifyTitle 是没配标题时的正文标题。
//
// 旧实现是 `"mcloud-v2 运行推送"`（CLI 那一处）。这里的默认值取
// "mcloud 运行推送"：`v2` 是旧项目的版本号，留着会让人以为这份 Go 实现
// 还挂在同一个版本上 —— 那是**误导**，不是保真。旧值记在这里供对照。
const DefaultNotifyTitle = "mcloud 运行推送"

// DefaultNotifyConfig 返回默认配置：关闭、门槛 info、只收错误时不发、一家都没配。
func DefaultNotifyConfig() NotifyConfig {
	return NotifyConfig{MinLevel: NotifyLevelInfo}
}

// TitleOrDefault 返回实际要用的标题（空白串也算"没填"）。
func (c NotifyConfig) TitleOrDefault() string {
	if c.Title != nil {
		if trimmed := strings.TrimSpace(*c.Title); trimmed != "" {
			return trimmed
		}
	}
	return DefaultNotifyTitle
}

// Validate 判这份配置能不能用，返回第一处问题的原因。
//
// 只做**结构**检查（门槛合法、配了的那家必填齐）。
// "该不该发"（总开关、onlyError）不在这里判 —— 那是每次运行时的判定，
// 混进校验会让界面上"保存失败"和这次"没推送"两件事共用一句话。
func (c NotifyConfig) Validate() *Error {
	// 空门槛 = 没填 ⇒ 按 info 走（旧实现的默认值），所以**不**在这里判非法；
	// 但渠道的必填检查照旧要跑 —— 早退一次就会把"配了却没填全"放过去。
	if c.MinLevel != "" && !c.MinLevel.Valid() {
		return NewBadRequest(fmt.Sprintf(
			"推送门槛只能是 %s 之一，实得 %q", joinLevels(), string(c.MinLevel)))
	}
	for _, name := range c.Channels.Names() {
		if missing := c.Channels.Missing(name); missing != "" {
			return NewBadRequest(fmt.Sprintf("推送渠道 %s 缺少必填项：%s", name, missing))
		}
	}
	return nil
}

// ConfiguredNames 返回**必填项齐全**的渠道名（按固定序）。
//
// 与 [NotifyChannels.Names] 的区别就是这一格：界面上"配了 4 家"与
// "能发 3 家"是两句不同的话，混成一句会让人以为少的那一家被程序吞了。
func (c NotifyConfig) ConfiguredNames() []NotifyChannel {
	var names []NotifyChannel
	for _, name := range c.Channels.Names() {
		if c.Channels.Missing(name) == "" {
			names = append(names, name)
		}
	}
	return names
}

// ─────────────────────────────────────────────────────────────────────────────
// 小工具（本文件内共用；导出不外泄，免得别处以为有第二份判据）
// ─────────────────────────────────────────────────────────────────────────────

// fieldOf 把"空白也算没填"收成一次判断：返回**字段名**表示缺，返回空串表示齐。
//
// ⚠ 用 `Value()` 而不是 `string(...)` 之外的方式取原文：[SecretValue] 的 `String()`
// 是**脱敏**的（"有没有 + 多长"），拿它做判据会永远判成"填了"。
func fieldOf(value, name string) string {
	if strings.TrimSpace(value) == "" {
		return name
	}
	return ""
}

func firstMissing(fields ...string) string {
	for _, f := range fields {
		if f != "" {
			return f
		}
	}
	return ""
}

func joinLevels() string {
	names := make([]string, 0, len(notifyLevelCeiling))
	for level := range notifyLevelCeiling {
		names = append(names, string(level))
	}
	sort.Strings(names)
	joined := ""
	for i, name := range names {
		if i > 0 {
			joined += " / "
		}
		joined += name
	}
	return joined
}
