// 推送配置的读写接口：`GET /api/notify/config` 与 `PUT /api/notify/config`。
//
// 出网形状在 `notify_view.go`（那张字段表是本文件写侧白名单的同源依据），
// 发送侧的装配与试发在 `notify.go`。
//
// # 写入是**逐字段合并**，不是整块覆盖
//
// 规则一句话：**这一格没填 ⇒ 保持库里那一份**；要清空**一整家** ⇒ 把它的键置 `null`。
//
// 为什么不照 `PUT /api/schedules/{id}` 那样做全量覆盖：这一族的读视图是**脱敏**的
// （各家 token 只出"有没有 + 长度"），所以界面**拿不到**原值，也就**提交不回**原值。
// 全量覆盖的话，"我只想换一下 bark 的 key"会把同一家里看不见的自由键、
// 以及别家没在表单里的字段一起洗掉 —— 而那种丢失**不报任何错**，
// 表现是过一阵"推送莫名不响了"。这正是本仓库最反感的那一类静默失效。
//
// 代价要说清：**清空单个字段做不到**（只能整家删了重填），
// 而 `customPost` 是数组、没有"同一条"的可靠身份，所以它是**整块替换**
// （改其中一条要把那几条重提交一遍，URL 得重填）。登记成待拍板项。
//
// # 不认识的键 ⇒ 400（顶层、渠道名、渠道内字段名三处都拦）
//
// 旧实现那一族最难查的现象是"配错了键名"：它不报错，只是那一格静默不上线
// （`chat_id` 写成 `chatId`，见 `internal/core/notify.go` 文件头第 2 条）。
// [core.ParseJSONTo] 与标准库一样**不**拒绝未知字段，所以这道闸只能在边界上手写。
//
// # 这张表**无归属**
//
// `notify_config` 是 `CHECK (id = 1)` 的单行表（与 `exchange_config`、`webui_config`
// 同一族），所以任何已登录用户改它都会影响**整个安装**的推送，而本服务今天
// 没有 admin 角色门槛（既成事实，不在这里新造一道）。多用户部署下的后果登记成
// 待拍板项，见 `docs/pending-decisions.md`。
package server

import (
	"encoding/json"
	"net/http"
	"sort"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// notifyTopLevelKeys 是 PUT 请求体认识的**顶层**键（与 [notifyChannelFields] 同一个用途：
// 读侧决定"什么能出网"，写侧决定"什么算打错了"）。
var notifyTopLevelKeys = []string{"enabled", "title", "onlyError", "minLevel", "channels"}

// notifyUnknownTopLevelKeyError 是"请求体顶层写了一个不认识的键"那句话。
func notifyUnknownTopLevelKeyError(key string) *core.Error {
	return core.NewBadRequest("请求体里有不认识的键：" + key +
		"（认识的是：" + strings.Join(notifyTopLevelKeys, " / ") + "）")
}

func (s *Server) getNotifyConfig(w http.ResponseWriter, r *http.Request, _ *AuthUser) {
	config, err := s.store.NotifyConfig(r.Context())
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	payload, perr := notifyPayloadOf(config)
	if perr != nil {
		// ⚠ 不把 perr 的原文带出去：它可能含 marshal 出来的**值**（本函数就是因为
		//  marshal 才失败的）。对外只说"构造失败"，与 [core.NewInternal] 那条
		// "内部错误不外泄细节"的既有纪律同口径。
		ErrDomain(core.NewInternal("推送配置视图构造失败")).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"config": payload})
}

func (s *Server) putNotifyConfig(w http.ResponseWriter, r *http.Request, _ *AuthUser) {
	body, ok := decodeJSON[map[string]json.RawMessage](w, r)
	if !ok {
		return
	}
	ctx := r.Context()
	current, err := s.store.NotifyConfig(ctx)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	// 从库里那一份出发做**增量**合并（理由见本文件抬头）。
	merged := current

	keys := make([]string, 0, len(body))
	for key := range body {
		keys = append(keys, key)
	}
	// 按字典序过一遍：一个体里有两处错误时报哪一处是**确定的**
	// （map 的遍历顺序随机，否则同一份请求两次可能给出两句话）。
	sort.Strings(keys)
	for _, key := range keys {
		raw := body[key]
		switch key {
		case "enabled":
			value, perr := decodeNotifyTopLevel[bool]("enabled", raw)
			if perr != nil {
				ErrDomain(perr).Write(w)
				return
			}
			merged.Enabled = value
		case "onlyError":
			value, perr := decodeNotifyTopLevel[bool]("onlyError", raw)
			if perr != nil {
				ErrDomain(perr).Write(w)
				return
			}
			merged.OnlyError = value
		case "title":
			// `null` 与"没有这个键" ⇒ 保持；`""`（只有空白也算）⇒ **清空**，回到默认标题。
			//
			// 与凭据那一族相反，这里空串是**有信息**的：留空不损失任何东西 ——
			// [core.NotifyConfig.TitleOrDefault] 本来就把空串折成默认标题，
			// 所以"清空"与"忘了填"落进库里是同一个结果，不必像 token 那样
			// 为了保住已配好的那一份而把空白当"没填"。
			value, perr := decodeNotifyTopLevel[*string]("title", raw)
			if perr != nil {
				ErrDomain(perr).Write(w)
				return
			}
			if value != nil {
				if strings.TrimSpace(*value) == "" {
					merged.Title = nil
				} else {
					merged.Title = value
				}
			}
		case "minLevel":
			value, perr := decodeNotifyTopLevel[*string]("minLevel", raw)
			if perr != nil {
				ErrDomain(perr).Write(w)
				return
			}
			if value != nil {
				merged.MinLevel = core.NotifyLogLevel(*value)
			}
		case "channels":
			updates, perr := decodeNotifyTopLevel[map[string]json.RawMessage]("channels", raw)
			if perr != nil {
				ErrDomain(perr).Write(w)
				return
			}
			// `"channels": null` ⇒ 一个渠道都不动（上面那句解出的是 nil map，
			// 遍历零次），与"整块清空"不同：后者要逐家置 null。
			if perr := mergeNotifyChannels(&merged.Channels, updates); perr != nil {
				ErrDomain(perr).Write(w)
				return
			}
		default:
			ErrDomain(notifyUnknownTopLevelKeyError(key)).Write(w)
			return
		}
	}

	// 结构校验走 core 那一份（门槛合法、配了的那家必填齐），**不在这里另写判据**。
	if perr := merged.Validate(); perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	now := s.Now()
	merged.UpdatedAt = &now
	if err := s.store.PutNotifyConfig(ctx, &merged); err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}

	// 回读一遍再出视图：**不**直接把 `merged` 画出去。
	// 存储层在读回时会丢掉它不认识的渠道键（`io/configstore.go` 那条测试记着这个口径），
	// 回读让界面显示的就是"下一次运行真正会用到的那一份"。
	saved, err := s.store.NotifyConfig(ctx)
	if err != nil {
		ErrDomain(asDomain(err)).Write(w)
		return
	}
	payload, perr := notifyPayloadOf(saved)
	if perr != nil {
		ErrDomain(core.NewInternal("推送配置视图构造失败")).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"config": payload})
}

// decodeNotifyTopLevel 解顶层的一格，形状不对时报出**是哪一格**。
//
// 用 [core.ParseJSONTo] 而不是 `json.Unmarshal`：与 [decodeJSON] 同一条纪律
// （数字口径只有一份）。
func decodeNotifyTopLevel[T any](name string, raw json.RawMessage) (T, *core.Error) {
	var zero T
	value, ok := core.ParseJSONTo[T](string(raw))
	if !ok {
		return zero, core.NewBadRequest("推送配置的 " + name + " 那一格形状不对")
	}
	return value, nil
}

// mergeNotifyChannels 把 `updates` 里的每一家并进 `existing`。
//
// 渠道名按 [core.NotifyChannelOrder] 处理而不是按 map 顺序：一处体里同时写坏两家时，
// 报出来的那一句要稳定。
func mergeNotifyChannels(existing *core.NotifyChannels, updates map[string]json.RawMessage) *core.Error {
	var unknown []string
	for key := range updates {
		if !core.NotifyChannel(key).Valid() {
			unknown = append(unknown, key)
		}
	}
	if len(unknown) > 0 {
		sort.Strings(unknown)
		return notifyUnknownChannelError(unknown)
	}
	for _, name := range core.NotifyChannelOrder() {
		raw, present := updates[string(name)]
		if !present {
			continue
		}
		if string(raw) == "null" {
			clearNotifyChannel(existing, name)
			continue
		}
		if perr := mergeNotifyChannel(existing, name, raw); perr != nil {
			return perr
		}
	}
	return nil
}

// mergeNotifyChannel 并一家：先按那张字段表白名单解码，再把**填了的**字段盖上去。
func mergeNotifyChannel(existing *core.NotifyChannels, name core.NotifyChannel, raw json.RawMessage) *core.Error {
	fields := notifyChannelFields[name]
	switch name {
	case core.NotifyPushplus:
		incoming, perr := decodeNotifyObject[core.PushplusConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.Pushplus)
		out.Token = keepSecretValue(out.Token, incoming.Token)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.Pushplus = &out
	case core.NotifyServerChan:
		incoming, perr := decodeNotifyObject[core.ServerChanConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.ServerChan)
		out.Token = keepSecretValue(out.Token, incoming.Token)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.ServerChan = &out
	case core.NotifyWorkWeixin:
		incoming, perr := decodeNotifyObject[core.WorkWeixinConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.WorkWeixin)
		out.CorpID = keepSecretValue(out.CorpID, incoming.CorpID)
		out.CorpSecret = keepSecretValue(out.CorpSecret, incoming.CorpSecret)
		out.AgentID = keepPointer(out.AgentID, incoming.AgentID)
		out.ToUser = keepString(out.ToUser, incoming.ToUser)
		out.MsgType = keepString(out.MsgType, incoming.MsgType)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.WorkWeixin = &out
	case core.NotifyWorkWeixinBot:
		incoming, perr := decodeNotifyObject[core.WorkWeixinBotConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.WorkWeixinBot)
		out.URL = keepString(out.URL, incoming.URL)
		out.MsgType = keepString(out.MsgType, incoming.MsgType)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.WorkWeixinBot = &out
	case core.NotifyTGBot:
		incoming, perr := decodeNotifyObject[core.TGBotConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.TGBot)
		out.APIHost = keepString(out.APIHost, incoming.APIHost)
		out.Token = keepSecretValue(out.Token, incoming.Token)
		out.ChatID = keepString(out.ChatID, incoming.ChatID)
		out.DisableWebPagePreview = keepPointer(out.DisableWebPagePreview, incoming.DisableWebPagePreview)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.TGBot = &out
	case core.NotifyBark:
		incoming, perr := decodeNotifyObject[core.BarkConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.Bark)
		out.Key = keepSecretValue(out.Key, incoming.Key)
		out.Level = keepString(out.Level, incoming.Level)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.Bark = &out
	case core.NotifyTwoIM:
		incoming, perr := decodeNotifyObject[core.TwoIMConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.TwoIM)
		out.Key = keepSecretValue(out.Key, incoming.Key)
		out.SID = keepSecretValue(out.SID, incoming.SID)
		out.MsgType = keepString(out.MsgType, incoming.MsgType)
		out.Query = keepMap(out.Query, incoming.Query)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.TwoIM = &out
	case core.NotifyDingTalk:
		incoming, perr := decodeNotifyObject[core.DingTalkConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.DingTalk)
		out.Token = keepSecretValue(out.Token, incoming.Token)
		out.Secret = keepSecretValue(out.Secret, incoming.Secret)
		out.Extra = keepMap(out.Extra, incoming.Extra)
		existing.DingTalk = &out
	case core.NotifyEmail:
		incoming, perr := decodeNotifyObject[core.EmailConfig](name, raw, fields, false)
		if perr != nil {
			return perr
		}
		out := *keepExisting(existing.Email)
		out.Host = keepString(out.Host, incoming.Host)
		out.Port = keepPointer(out.Port, incoming.Port)
		out.From = keepSecretValue(out.From, incoming.From)
		out.Pass = keepSecretValue(out.Pass, incoming.Pass)
		out.To = keepString(out.To, incoming.To)
		existing.Email = &out
	case core.NotifyCustomPost:
		// **整块替换**（不是逐条合并）：数组里的"同一条"没有可靠身份
		// —— 按**下标**对齐会让"删掉第一条"把后面每条都错位读到上一条的凭据，
		// 那比要求重填更糟。代价写在本文件抬头。
		incoming, perr := decodeNotifyObject[[]core.CustomPostConfig](name, raw, notifyCustomPostFields, true)
		if perr != nil {
			return perr
		}
		existing.CustomPost = incoming
	}
	return nil
}

// decodeNotifyObject 解一家的值：先过字段白名单，再解进目标类型。
//
// `customPost` 传 `asArray = true`：那一家的值是数组，所以白名单要逐条查，
// 而"这一格该长什么样"的那句话也得跟着换（对数组说"要的是一个对象"是句看不懂的话）。
//
// 白名单那一遍**必须**先跑：[core.ParseJSONTo] 与标准库一样不拒绝未知字段，
// 于是打错的键会被静默丢掉（见本文件抬头）。
func decodeNotifyObject[T any](
	name core.NotifyChannel, raw json.RawMessage, allowed []notifyField, asArray bool,
) (T, *core.Error) {
	var zero T
	label := "推送渠道 " + string(name)
	if asArray {
		rows, ok := core.ParseJSONTo[[]map[string]json.RawMessage](string(raw))
		if !ok {
			return zero, core.NewBadRequest(label + " 那一格要的是一个数组，每条是一个对象")
		}
		for index, row := range rows {
			if unknown := notifyUnknownIn(row, allowed); len(unknown) > 0 {
				return zero, notifyUnknownKeysError(
					label+" 的第 "+strconv.Itoa(index+1)+" 条", allowed, unknown)
			}
		}
	} else {
		shape, ok := core.ParseJSONTo[map[string]json.RawMessage](string(raw))
		if !ok {
			return zero, core.NewBadRequest(label + " 那一格要的是一个对象")
		}
		if unknown := notifyUnknownIn(shape, allowed); len(unknown) > 0 {
			return zero, notifyUnknownKeysError(label, allowed, unknown)
		}
	}
	value, ok := core.ParseJSONTo[T](string(raw))
	if !ok {
		return zero, core.NewBadRequest(label + " 的字段形状不对")
	}
	return value, nil
}

// notifyUnknownIn 挑出这一格里**白名单之外**的键名（排序后返回，报错才稳定）。
func notifyUnknownIn(shape map[string]json.RawMessage, allowed []notifyField) []string {
	known := make(map[string]bool, len(allowed))
	for _, field := range allowed {
		known[field.name] = true
	}
	var unknown []string
	for key := range shape {
		if !known[key] {
			unknown = append(unknown, key)
		}
	}
	sort.Strings(unknown)
	return unknown
}

// notifyUnknownKeysError 是"有不认识的键"那句话。
//
// 它把**认识的清单**一起打出来：这一类错误的成因就是"键名记错了"，
// 只说"不认识 chatId"会让人去查值、查类型，而不是立刻看到 `chat_id`。
func notifyUnknownKeysError(what string, allowed []notifyField, unknown []string) *core.Error {
	known := make([]string, 0, len(allowed))
	for _, field := range allowed {
		known = append(known, field.name)
	}
	return core.NewBadRequest(what + " 里有不认识的键：" + strings.Join(unknown, " / ") +
		"（认识的是：" + strings.Join(known, " / ") + "）")
}

// notifyUnknownChannelError 是"写了个不认识的渠道名"那句话。
//
// 与字段级那句分开写，是因为这里的"认识的清单"由 core 那一张序表决定
// （[core.NotifyChannelOrder]），而不是本文件的字段表 —— 两个清单各有权威，
// 合并成一个函数就会有人去改错的那一份。
func notifyUnknownChannelError(unknown []string) *core.Error {
	known := make([]string, 0, len(core.NotifyChannelOrder()))
	for _, name := range core.NotifyChannelOrder() {
		known = append(known, string(name))
	}
	return core.NewBadRequest("有不认识的推送渠道：" + strings.Join(unknown, " / ") +
		"（认识的是：" + strings.Join(known, " / ") + "）")
}

// clearNotifyChannel 把一家整块去掉（请求体里那一格写 `null` 的语义）。
func clearNotifyChannel(existing *core.NotifyChannels, name core.NotifyChannel) {
	switch name {
	case core.NotifyPushplus:
		existing.Pushplus = nil
	case core.NotifyServerChan:
		existing.ServerChan = nil
	case core.NotifyWorkWeixin:
		existing.WorkWeixin = nil
	case core.NotifyWorkWeixinBot:
		existing.WorkWeixinBot = nil
	case core.NotifyTGBot:
		existing.TGBot = nil
	case core.NotifyBark:
		existing.Bark = nil
	case core.NotifyTwoIM:
		existing.TwoIM = nil
	case core.NotifyDingTalk:
		existing.DingTalk = nil
	case core.NotifyEmail:
		existing.Email = nil
	case core.NotifyCustomPost:
		existing.CustomPost = nil
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// "没填就保持"的四个判断（合并在这一处，规则只有一份）
// ─────────────────────────────────────────────────────────────────────────────

// keepExisting 把"这一家还没配"翻成一个可写的零值结构。
func keepExisting[T any](current *T) *T {
	if current != nil {
		return current
	}
	return new(T)
}

// keepSecretValue / keepString：空（含只有空白）算"没填"。
//
// ⚠ 这意味着**单个凭据清空不了** —— 与本仓库"清空 = 整家置 null"是同一条设计，
// 而不是漏写：清空一格 token 与"忘了填 token"在请求体里**长得一模一样**，
// 两者无法区分时，宁可保住已配好的那一份。
func keepSecretValue(old, incoming core.SecretValue) core.SecretValue {
	if strings.TrimSpace(incoming.Value()) == "" {
		return old
	}
	return incoming
}

func keepString(old, incoming string) string {
	if strings.TrimSpace(incoming) == "" {
		return old
	}
	return incoming
}

// keepPointer 用 nil 表达"没填"（`*int` 的 0 与 `*bool` 的 false 都是**有效值**，
// 所以这三样必须是指针 —— core 里已经是了，这里不加第二种判据）。
func keepPointer[T any](old, incoming *T) *T {
	if incoming == nil {
		return old
	}
	return incoming
}

// keepMap 同理：`null` 与"没有这个键"都解成 nil ⇒ 保持；
// 要清空一份自由键就提交 `"extra": {}`（非 nil 的空 map ⇒ 覆盖）。
func keepMap(old, incoming map[string]any) map[string]any {
	if incoming == nil {
		return old
	}
	return incoming
}
