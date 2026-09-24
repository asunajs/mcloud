// 推送配置的**出网视图**：一次整块 marshal 都不做，逐字段按下面的表放行。
//
// # 为什么不能 `writeJSON(w, 200, config)`
//
// [core.NotifyConfig] 里装着十家渠道的**凭据**（各家 token / key / corpsecret /
// SMTP 授权码），而它的字段全都带 json tag —— 整块 marshal 会把这些原样递出去，
// **编译期没有任何东西拦得住**，测试里也不会响（`SecretValue` 的 `String()` 只挡
// `%v` 打印，不挡 `encoding/json`，这是本仓库成文的旧坑）。
// 所以这一族对外只有本文件这一个出口，它只输出 [notifyChannelFields] 里登记的字段，
// 并按"这一格是什么性质"决定出什么：
//
//   - `secret`：只出"有没有填 + 字符数"，**值永不出网**；
//
//   - `keys`：自由键（`extra` / `query` / `headers` / `data` / `body`）只出**键名**，
//     值不出 —— 用户会往 `headers` 里放 `Authorization`、往 `extra` 里带第二个凭据，
//     而这两个位置我们今天**没有**任何逐家样本能判断"这里面装的是什么"；
//
//   - `plain`：普通配置值原样出（界面上要能看见并改它，掩码了等于这一栏没法编辑）。
//
// # 字段清单出成**数组**，而且十家都出（含没配的）
//
// `fields` 是数组，顺序 = 这张表的声明序，每格带三个标识键（`name` / `kind` / `type`）
// 再加该档的值。两件事都是必要的：
//
//   - **数组**：`channels` 与"每家一份字段"若出成 JSON 对象，marshal 会把键名排成字典序
//     （Go 的 map 没有顺序），于是"企业微信"跑到"pushplus"前面、一家里 `chat_id` 排在
//     `api_host` 后面，界面在改字段表时自己换顺序（看着像数据在动）。
//     家这一层仍出对象 + 一张 `order` 序表，因为界面**按名取**一家；字段那一层只是顺序遍历。
//   - **每格自带 `kind` / `type`**：否则前端只能自己抄一份字段表，而抄了就会漂移，
//     漂移有两种表现且都不报错 —— 后端新增一格界面永远画不出来；前端多写一格 ⇒ PUT 被
//     400 拦下，看起来像"我填错了"。`type`（`string` / `number` / `boolean`）尤其必需：
//     同一个 `plain` 档背后是文本框、数字框、开关三种控件，而"值在不在"分不出它们
//     （`port` 是 `*int`，没填时 JSON 里压根没有这一格）。
//
// 没配的那几家也出整张清单（走 [notifyChannelValue] 的零值分支）：空白表单也得画得出来 ——
// 新装上什么都没有时用户点的就是"填写 pushplus"，那时没有清单就等于这一家填不了。
// 每格都念成"没填"（`present:false` / `value` 为空串或 null / `count:0`），
// 而 `configured:false` 仍然说明这一家没配。
//
// # 两处刻意从严，代价写在这里
//
//   - `workWeixinBot.url` 与 `customPost[].url` / `headers` 在 core 里是**普通字符串**
//     （不是 `SecretValue`），这里仍按 `secret` / `keys` 处理：企业微信群机器人的
//     整条 webhook 地址**就是**凭据（`…/send?key=…`），而 customPost 的地址由用户自编，
//     里面放什么我们都无从判据。代价是界面上改这两处要**重填**，
//     登记成待拍板项（`docs/pending-decisions.md`）。
//   - `email.from` 同样从严：它在 core 里是 `SecretValue`，虽然那只是一个发件地址。
//     跟随 core 的分类，不在这里另判一次（两处判据会漂移）。
package server

import (
	"errors"
	"reflect"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/catlair/mcloud/internal/core"
)

// notifyFieldKind 是一个字段在**出网视图**里的表现方式。
type notifyFieldKind int

const (
	// notifySecretField 只出"有没有 + 长度"。
	notifySecretField notifyFieldKind = iota
	// notifyPlainField 原样出值。
	notifyPlainField
	// notifyKeysField 只出键名。
	notifyKeysField
)

// String 是这个档位**出网时**的名字（视图里 `kind` 那一格，前端按它选控件）。
//
// 它存在的理由是"视图写出去的那三个字符串"与"测试判的那三个"只用一份表：
// 两处各写一遍时，改档位的人只会改到自己看见的那一处。
func (k notifyFieldKind) String() string {
	switch k {
	case notifySecretField:
		return "secret"
	case notifyKeysField:
		return "keys"
	default:
		return "plain"
	}
}

// notifyField 是"这个渠道的这一个字段叫什么、按哪一档出"。
type notifyField struct {
	name string
	kind notifyFieldKind
}

// notifyChannelFields 是九个**对象型**渠道的字段清单（`customPost` 是数组，单独一张表）。
//
// 它同时服务三处，而且这个"同一处"是刻意的：
//   - 读侧：不在表里的字段**出不了网**（新增 core 字段默认是"不出网"，
//     与 [accountPayload] 那一条纪律同一个方向）；
//   - 写侧：表就是"这个渠道认识哪些键"的白名单，写了个不认识的键 ⇒ **400**。
//     旧实现那一族最难的坑就是"把 `chat_id` 写成 `chatId`"—— 它不报错，
//     只是那一格静默不上线（`internal/core/notify.go` 文件头第 2 条记的就是这件事）。
//     在 HTTP 边界上拦住它，比在推送正文里发现它便宜得多。
//   - 界面侧：**声明序就是渲染序**（视图里的 `fields` 数组由它生成）。所以给一家加一格
//     只改这一处，界面上就会多出这一格 —— 不存在"后端加了、前端没跟上"那种漂移。
var notifyChannelFields = map[core.NotifyChannel][]notifyField{
	core.NotifyPushplus: {
		{"token", notifySecretField}, {"extra", notifyKeysField},
	},
	core.NotifyServerChan: {
		{"token", notifySecretField}, {"extra", notifyKeysField},
	},
	core.NotifyWorkWeixin: {
		{"corpid", notifySecretField}, {"corpsecret", notifySecretField},
		{"agentid", notifyPlainField}, {"touser", notifyPlainField},
		{"msgtype", notifyPlainField}, {"extra", notifyKeysField},
	},
	core.NotifyWorkWeixinBot: {
		// ⚠ url 按凭据处理（整条地址就是 key），理由见本文件抬头。
		{"url", notifySecretField}, {"msgtype", notifyPlainField}, {"extra", notifyKeysField},
	},
	core.NotifyTGBot: {
		{"api_host", notifyPlainField}, {"token", notifySecretField},
		{"chat_id", notifyPlainField},
		{"disable_web_page_preview", notifyPlainField}, {"extra", notifyKeysField},
	},
	core.NotifyBark: {
		{"key", notifySecretField}, {"level", notifyPlainField}, {"extra", notifyKeysField},
	},
	core.NotifyTwoIM: {
		{"key", notifySecretField}, {"sid", notifySecretField},
		{"msgtype", notifyPlainField}, {"query", notifyKeysField}, {"extra", notifyKeysField},
	},
	core.NotifyDingTalk: {
		{"token", notifySecretField}, {"secret", notifySecretField}, {"extra", notifyKeysField},
	},
	core.NotifyEmail: {
		{"host", notifyPlainField}, {"port", notifyPlainField},
		{"from", notifySecretField}, {"pass", notifySecretField}, {"to", notifyPlainField},
	},
}

// notifyCustomPostFields 是通用 HTTP 那一族的字段清单（每条一份）。
//
// `url` / `headers` 同上按凭据处理；`data` / `body` 是用户自己拼的请求体，
// 里面放什么都可能，所以只出键名。
var notifyCustomPostFields = []notifyField{
	{"method", notifyPlainField}, {"url", notifySecretField}, {"timeout", notifyPlainField},
	{"headers", notifyKeysField}, {"data", notifyKeysField}, {"body", notifyKeysField},
}

// notifyPayloadOf 拼一份完整的推送配置视图。
//
// 返回 error 只在"这个结构体 marshal 不出去"时发生（今天构造上不可能：
// 字段全是字符串、数字与已解码的 JSON 值）。**不**退化成"报一家都没填" ——
// 那种退化会把"视图坏了"念成"用户的配置没了"，而后者会让人重填一遍凭据。
func notifyPayloadOf(config core.NotifyConfig) (map[string]any, error) {
	channels, err := notifyChannelsPayload(config.Channels)
	if err != nil {
		return nil, err
	}
	levels := make([]string, 0, len(core.NotifyLevels()))
	for _, level := range core.NotifyLevels() {
		levels = append(levels, string(level))
	}
	sendable := config.ConfiguredNames()
	if sendable == nil {
		sendable = []core.NotifyChannel{}
	}
	configured := config.Channels.Names()
	if configured == nil {
		configured = []core.NotifyChannel{}
	}
	return map[string]any{
		"enabled":   config.Enabled,
		"title":     config.Title,
		"onlyError": config.OnlyError,
		// 门槛按**原样**出（包括空串）：空串是有效信息（"没填 ⇒ 跟随 info"），
		// 在这里替它填成 `info` 会让界面显示的与库里存的两样，而 PUT 是全量提交。
		"minLevel":     string(config.MinLevel),
		"levels":       levels,
		"defaultTitle": core.DefaultNotifyTitle,
		"updatedAt":    rfc3339Of(config.UpdatedAt),
		// "配了几家"与"必填齐的有几家"是两句不同的话（`ConfiguredNames` 的注释），
		// 界面上要靠这两者区分"我没配"与"我配了但缺字段"。
		// ⚠ 两处都得把 nil 裁成 `[]`：Go 的 nil slice marshal 成 `null`，
		//   而 `null` 在界面上不是"列表为空"而是"这格坏了"——
		//   实测表现是设置页整页三张卡都不出现（`configured.includes(...)` 抛 TypeError，
		//   页面停在"既不是 loading 也不是 ready"的中间态）。
		"configured": configured,
		"sendable":   sendable,
		// `order` 是给界面的**渠道渲染序**。`channels` 是 JSON 对象，出网时键名会被
		// marshal 排成字典序（Go 的 map 没有顺序）⇒ 十家的先后得另给一张表。
		// 家这一层留对象、字段那一层出数组，是因为界面**按名取**一家（点开 bark 的表单），
		// 而字段那一层只是顺序遍历 —— 两种用法各有各的形状。
		// （与 [core.NotifyChannelOrder] 钉死发送序同一条理由：顺序不稳 = 看着像数据在动。）
		"order":    core.NotifyChannelOrder(),
		"channels": channels,
	}, nil
}

// notifyFieldsPayload 把一张字段表摊成**有序数组**，每格带上"什么档位、什么值类型"。
//
// `sample` 是这一家的结构体（或零值），只用来经反射取字段的**值类型** ——
// 视图里 `plain` 那一档可能是 `string` / `*int` / `*bool`，前端得知道该摆
// 文本框、数字框还是开关。这个信息marshal 不出来：`port` 没填时 JSON 里根本没有这一格，
// 而它是 `*int`；把判据交给"值在不在"就会在空白表单上把数字格画成文本格。
func notifyFieldsPayload(fields []notifyField, sample any, asMap map[string]any) []map[string]any {
	types := notifyFieldTypes(sample, fields)
	out := make([]map[string]any, 0, len(fields))
	for _, field := range fields {
		out = append(out, notifyFieldPayload(field, types[field.name], asMap[field.name]))
	}
	return out
}

// notifyFieldTypes 反射出一张字段表里每格的 JSON 值类型（`string` / `number` / `boolean`）。
//
// 认不出（表里写了个结构体里没有的名字）时留空串：视图照常出那一格，前端回落到文本框。
// 这里**不**报错 —— 这张表与 core 的结构体由同一次改动一起改，真漏了会有别的测试红
// （写侧白名单解码时那一格解不进去，`TestNotifyConfigUnknownFieldInChannelIsRejected` 那一族会抓）。
func notifyFieldTypes(sample any, fields []notifyField) map[string]string {
	out := make(map[string]string, len(fields))
	structType := reflect.TypeOf(sample)
	if structType == nil {
		return out
	}
	if structType.Kind() == reflect.Ptr {
		structType = structType.Elem()
	}
	if structType.Kind() != reflect.Struct {
		return out
	}
	wanted := make(map[string]bool, len(fields))
	for _, field := range fields {
		wanted[field.name] = true
	}
	for index := 0; index < structType.NumField(); index++ {
		tag := strings.Split(structType.Field(index).Tag.Get("json"), ",")[0]
		if tag == "" || !wanted[tag] {
			continue
		}
		out[tag] = jsonTypeOf(structType.Field(index).Type)
	}
	return out
}

// jsonTypeOf 把一个 Go 类型折成"JSON 里长什么样"。指针只是"可空"，不是另一种类型。
func jsonTypeOf(t reflect.Type) string {
	for t.Kind() == reflect.Ptr {
		t = t.Elem()
	}
	switch t.Kind() {
	case reflect.Bool:
		return "boolean"
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64,
		reflect.Float32, reflect.Float64:
		return "number"
	case reflect.Map, reflect.Struct:
		// 今天只有 `map[string]any` 那一族（`extra` / `query` / `headers` / `data` / `body`）
		// 走到这里，而它们的 `kind` 恒为 `keys`：这一格出 `object` 是为了让前端
		// "按 type 选控件"那段代码不必为 keys 档另开一条判断。
		return "object"
	default:
		// `SecretValue` 底层是 string，走默认这一支（它的 `kind` 是 `secret`，类型无人读）。
		return "string"
	}
}

func notifyChannelsPayload(channels core.NotifyChannels) (map[string]any, error) {
	out := make(map[string]any, len(notifyChannelFields)+1)
	for _, name := range core.NotifyChannelOrder() {
		if name == core.NotifyCustomPost {
			payload, err := notifyCustomPostPayload(channels.CustomPost, channels.Missing(name))
			if err != nil {
				return nil, err
			}
			out[string(name)] = payload
			continue
		}
		// 未配置那一家拿到的是**零值结构**（见 [notifyChannelValue]），于是字段清单照常出、
		// 每格念成"没填" —— 空白表单也得画得出来。
		value, configured := notifyChannelValue(channels, name)
		asMap, err := jsonValueMap(value)
		if err != nil {
			return nil, err
		}
		out[string(name)] = map[string]any{
			"configured": configured,
			// 未配置时**不**念"缺少必填项"：core 那一份在没配时返回的是渠道名本身
			// （"这一家没配"），显示成"缺少必填项：bark"会让人以为填坏了。
			"missing": missingOrDefault(channels.Missing(name), configured),
			"fields":  notifyFieldsPayload(notifyChannelFields[name], value, asMap),
		}
	}
	return out, nil
}

// missingOrDefault 是"没配这一家就不念缺少必填项"那一句的判断（数组型与对象型共用）。
func missingOrDefault(missing string, configured bool) string {
	if !configured {
		return ""
	}
	return missing
}

func notifyCustomPostPayload(items []core.CustomPostConfig, missing string) (map[string]any, error) {
	// 条目里每格只带**值**（清单在上面那一份里，一条一份是重复）。
	rows := make([]map[string]any, 0, len(items))
	for _, item := range items {
		asMap, err := jsonValueMap(item)
		if err != nil {
			return nil, err
		}
		rows = append(rows, map[string]any{
			"fields": notifyFieldsPayload(notifyCustomPostFields, item, asMap),
		})
	}
	configured := len(items) > 0
	return map[string]any{
		"configured": configured,
		"count":      len(items),
		// 数组里**每一条**的字段表是同一张，所以清单只出一份（按零值出档位与类型）。
		"fields": notifyFieldsPayload(
			notifyCustomPostFields, core.CustomPostConfig{}, map[string]any{}),
		// 与对象型渠道同一条口径：没配时不把"缺少必填项：customPost"念出来。
		"missing": missingOrDefault(missing, configured),
		"items":   rows,
	}, nil
}

// notifyChannelValue 取一家渠道的配置值，**第二个返回值才是"配没配"**。
//
// 没配时返回该家的零值结构而不是 nil：视图要为它画一张空白表单，
// 而"这一家有哪些格、各是什么档位"必须与已配置那一种走同一条代码路径 ——
// 分两条路就会有一处忘更新，表现是"新装的用户填不了某一家"。
// （`any` 只在本函数与 marshal 之间出现，不外泄成响应体：出网形状始终由那张表决定。）
func notifyChannelValue(channels core.NotifyChannels, name core.NotifyChannel) (any, bool) {
	switch name {
	case core.NotifyPushplus:
		return orZero(channels.Pushplus, channels.Pushplus != nil)
	case core.NotifyServerChan:
		return orZero(channels.ServerChan, channels.ServerChan != nil)
	case core.NotifyWorkWeixin:
		return orZero(channels.WorkWeixin, channels.WorkWeixin != nil)
	case core.NotifyWorkWeixinBot:
		return orZero(channels.WorkWeixinBot, channels.WorkWeixinBot != nil)
	case core.NotifyTGBot:
		return orZero(channels.TGBot, channels.TGBot != nil)
	case core.NotifyBark:
		return orZero(channels.Bark, channels.Bark != nil)
	case core.NotifyTwoIM:
		return orZero(channels.TwoIM, channels.TwoIM != nil)
	case core.NotifyDingTalk:
		return orZero(channels.DingTalk, channels.DingTalk != nil)
	case core.NotifyEmail:
		return orZero(channels.Email, channels.Email != nil)
	}
	// 到不了这里：九个分支加 customPost 覆盖满 `NotifyChannelOrder()`，而那张序表由
	// core 的测试钉死。留着是因为"新增一家忘了在这里加分支"的表现是这一家整个从视图里
	// 消失（不是 panic、也不是报错），宁可让它拿到一张空白表单至少还能填。
	return nil, false
}

// orZero 把"没配"翻成该家的零值结构（`configured` 仍然是 false，视图另说）。
func orZero[T any](value *T, configured bool) (any, bool) {
	if configured {
		return value, true
	}
	return new(T), false
}

// notifyFieldPayload 出一格：先说"这是什么格"（`name` / `kind` / `type`），再按档位给值。
//
// 三个标识键与值在**同一个函数**里出，是为了"前端拿到的一格里标识与值一定同属一格" ——
// 拆成两处拼就会有人只改一边（比如新增档位时忘了给 `kind`，那一格在界面上是一片空白）。
func notifyFieldPayload(field notifyField, valueType string, value any) map[string]any {
	row := map[string]any{"name": field.name, "kind": field.kind.String(), "type": valueType}
	switch field.kind {
	case notifySecretField:
		text, _ := value.(string)
		// 长度按**去掉首尾空白之后**算，与"有没有填"用同一个判断
		// （core 的必填判据就是 `TrimSpace` 非空）。否则一串空格会显示成
		// "没填，但有 3 个字符"，而界面拿这两个数各说各话。
		trimmed := strings.TrimSpace(text)
		row["present"] = trimmed != ""
		row["length"] = utf8.RuneCountInString(trimmed)
	case notifyKeysField:
		asMap, _ := value.(map[string]any)
		names := make([]string, 0, len(asMap))
		for key := range asMap {
			names = append(names, key)
		}
		// 排序：Go 的 map 没有顺序，不排的话"同一份配置两次 GET 出来的键序不一样"，
		// 界面上就是一行字在刷新时自己换了顺序（与 core 里"发送顺序钉死"同一条理由）。
		sort.Strings(names)
		row["count"] = len(names)
		row["keys"] = names
	default:
		row["value"] = value
	}
	return row
}

// jsonValueMap 把一个配置结构体转成"按 json tag 取值的通用 map"。
//
// 存在的理由是**只写一份字段清单**：视图与写侧都要按字段名取值，
// 若为十个结构体各写一遍 switch 就会有两处真相（改 core 的 tag 时漏改一处，
// 表现是那一带字段永远显示"没填"）。走一遍 marshal 拿不到"忘了登记"这件事 ——
// 那张表本来就是唯一入口，未登记的字段在这里取不到、也就出不了网。
func jsonValueMap(value any) (map[string]any, error) {
	raw, err := core.MarshalJSON(value)
	if err != nil {
		return nil, err
	}
	parsed, ok := core.ParseJSON(string(raw))
	if !ok {
		return nil, errors.New("推送配置转成视图时解不回 JSON")
	}
	asMap, ok := parsed.(map[string]any)
	if !ok {
		return nil, errors.New("推送配置转成视图时不是一个对象")
	}
	return asMap, nil
}
