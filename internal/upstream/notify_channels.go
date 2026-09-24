// 推送渠道的**请求构造**（一次请求就能发出去的那七家）。
//
// 每个 `XxxRequest` 都是纯函数：给同一份配置与同一串正文，必须给出**逐字节相同**的
// 请求 —— 这是"能离线断言、不需要真发一条"的全部前提，也是这七家唯一被钉住的东西。
//
// # ⚠ 这一族**零样本**
//
// 本仓库所有归档（`docs/capture/`、`test/fixtures/http/`）里**没有任何一条**第三方
// 推送的请求或响应。所以这里的形状对齐的是旧 TS 的**字面量**
// （`packages/shared/src/push/index.ts` 那七个函数），而不是任何实拍。
// 换句话说：请求构造这一组测试钉的是"我们发的是我们想发的那一条"，
// **不**钉"第三方认这一条"。补样方法见 `notify.go` 抬头。
//
// # 为什么请求体收成 `map[string]any` 而不是各家一个 struct
//
// 因为旧实现每一家都有一句 `...config`（把用户配置里多出来的键并进请求体），
// 而那一句**必须**能覆盖我们已经放进去的键（用户靠它改写 `content`）。
// struct 表达不了"未知的键"，`map` 可以；代价是拼错键名不会编译失败 ——
// 已知的键全部走字段，`Extra` 只装未知的键，这条纪律在 `core/notify.go` 抬头。
package upstream

import (
	"fmt"
	"net/url"
	"sort"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// 各家基址。**逐字对齐旧实现**，只有一处例外，写在那一条上面。
const (
	// pushplusBase 是 pushplus 的下发地址。
	//
	// ⚠ **与旧实现不同**：旧版写的是 `http://www.pushplus.plus/send`（明文 HTTP），
	// 而这一家的 token 就在请求体里 —— 明文等于把令牌撒在链路上。
	// 换成 https 是**修一个明显的凭据外泄面**，不是"对齐上游"：
	// 该域名是否接受 TLS **本仓没有样本**，不接受时表现是这一家记一条 failed
	// （看得见），而不是像现在这样静默地用明文。
	pushplusBase = "https://www.pushplus.plus"
	// serverChanBase 是 Server酱（方糖）的下发地址。
	serverChanBase = "https://sctapi.ftqq.com"
	// tgBotDefaultHost 是 Telegram Bot 的默认主机（旧实现的解构默认值同名）。
	tgBotDefaultHost = "api.telegram.org"
	// barkBase 是 Bark 的下发地址（key 在路径上）。
	barkBase = "https://api.day.app"
	// twoIMBase 是回逍的下发地址（key 与 sid 在查询串上）。
	twoIMBase = "https://api.2im.cn/push"
	// dingTalkBase 是钉钉群机器人的下发地址。
	dingTalkBase = "https://oapi.dingtalk.com/robot/send"
	// workWeixinAPIBase 是企业微信开放接口根（换票与发消息共用）。
	workWeixinAPIBase = "https://qyapi.weixin.qq.com/cgi-bin"
)

// notifyBodyText 是"标题 + 正文"合成一条消息时的分隔（旧实现一律是 `\n\n`）。
func notifyBodyText(title, text string) string { return title + "\n\n" + text }

// mergeExtra 把用户多配的自由键并进请求体。
//
// 方向与旧实现一致：`{我们拼的..., ...config}` ⇒ **Extra 覆盖**同名键。
// 覆盖本身**不报错**是刻意的：用户有权改写正文。返回被覆盖的键名。⚠ 当前**没有**任何调用方把它说出去（八家都是直接丢弃）——
// 那一格留给将来：想报"content 被 Extra 覆盖了"，落点在 [NotifySender.sendOne]
// 包一层，不必给这八个构造函数改签名。之所以留着返回值而不是静默吞掉：
// 这一族"用户覆盖了正文"是最容易让人查半天的静默变形，值得一句人话。
func mergeExtra(body map[string]any, extra map[string]any) []string {
	var overridden []string
	for key, value := range extra {
		if _, exists := body[key]; exists && key != "token" {
			overridden = append(overridden, key)
		}
		body[key] = value
	}
	// 排序是**必需**的，不是整齐：`Extra` 是 map，遍历序每轮随机，
	// 不排序的话同一份配置两次的结论文案会不一样（那是最难复现的一类偶发）。
	sort.Strings(overridden)
	return overridden
}

// jsonPostRequest 拼一个 JSON POST（七家全是 JSON 体）。
//
// `content-type` 一律显式给 `application/json`：旧实现里 pushplus / dingTalk /
// serverChan 各自写了这个头（有的写 `Content-Type`、有的写小写 `content-type`），
// 其余几家靠 `ctx.fetch` 的默认头。这里统一成一处，是因为**大小写两种写法混在
// 一份代码里**会让人以为某一家要的是小写 —— 那是 HTTP 头，大小写不敏感。
func jsonPostRequest(target string, body map[string]any) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(target).WithJSONBody(body)
	if err != nil {
		return request, core.NewInternal("推送请求体序列化失败")
	}
	return request.WithHeader("content-type", "application/json"), nil
}

// ─────────────────────────────────────────────────────────────────────────────
// pushplus
// ─────────────────────────────────────────────────────────────────────────────

// PushplusRequest 构造 pushplus 那一枪：`POST /send`，token 在**请求体**里。
func PushplusRequest(cfg core.PushplusConfig, title, text string) (core.HTTPRequest, *core.Error) {
	body := map[string]any{"token": cfg.Token.Value(), "title": title, "content": text}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(pushplusBase+"/send", body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Server酱
// ─────────────────────────────────────────────────────────────────────────────

// ServerChanRequest 构造 Server酱那一枪。
//
// ⚠ token 在**URL 路径**上（`/{token}.send`），不在请求体：这一家的密钥会出现在
// 任何记录 URL 的地方（代理日志、我们自己的错误串）。所以 `NotifyResult.Reason`
// 那条"绝不放 URL"的纪律在这一家不是洁癖，是**唯一**的防线。
//
// 正文换行按旧实现翻倍（`\n` → `\n\n`）：方糖把正文按 markdown 渲染，
// 单个换行会被并成一行。
func ServerChanRequest(cfg core.ServerChanConfig, title, text string) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"text":  title,
		"title": title,
		"desp":  strings.ReplaceAll(text, "\n", "\n\n"),
	}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(
		fmt.Sprintf("%s/%s.send", serverChanBase, cfg.Token.Value()), body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 企业微信群机器人（一步，webhook 地址即凭据）
// ─────────────────────────────────────────────────────────────────────────────

// WorkWeixinBotRequest 构造企业微信群机器人那一枪。
//
// 这里的 `url` 由用户提供，且**它就是凭据**（`key=` 在查询串里）。
// ⚠ 因此这一家不做"URL 不合法就不发"的判断：合法的自建网关地址形态太多，
// 拦错的代价是推送不发，而放过的代价只是一次失败 —— 后者看得见。
func WorkWeixinBotRequest(cfg core.WorkWeixinBotConfig, title, text string) (core.HTTPRequest, *core.Error) {
	msgType := cfg.MsgType
	if msgType == "" {
		msgType = "text"
	}
	body := map[string]any{"msgtype": msgType, msgType: map[string]any{"content": notifyBodyText(title, text)}}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(cfg.URL, body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Telegram Bot
// ─────────────────────────────────────────────────────────────────────────────

// TGBotRequest 构造 Telegram 那一枪：`POST /bot{token}/sendMessage`。
//
// `disable_web_page_preview` 的默认值是 **true**（旧实现的解构默认值），
// 不是 false —— 反向改坏它的样子是"照 Go 习惯给零值"，用户会看到每条推送
// 都带一张链接预览卡。
func TGBotRequest(cfg core.TGBotConfig, title, text string) (core.HTTPRequest, *core.Error) {
	host := cfg.APIHost
	if strings.TrimSpace(host) == "" {
		host = tgBotDefaultHost
	}
	preview := true
	if cfg.DisableWebPagePreview != nil {
		preview = *cfg.DisableWebPagePreview
	}
	body := map[string]any{
		"disable_web_page_preview": preview,
		"chat_id":                  cfg.ChatID,
		"text":                     notifyBodyText(title, text),
	}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(fmt.Sprintf("https://%s/bot%s/sendMessage", host, cfg.Token.Value()), body)
}

// ─────────────────────────────────────────────────────────────────────────────
// Bark
// ─────────────────────────────────────────────────────────────────────────────

// BarkRequest 构造 Bark 那一枪。
//
// Bark 的字段名与直觉相反：`title` 是标题、**`body` 才是正文**。
// 旧实现正是这么写的；这里把它写成一条注释，是因为下一个改这一族的人
// 最容易"顺手"把正文挪到 `content`（那样 iOS 上会收到一条只有标题的通知，
// 而 HTTP 侧回 200 —— 判据完全看不出来）。
func BarkRequest(cfg core.BarkConfig, title, text string) (core.HTTPRequest, *core.Error) {
	level := cfg.Level
	if level == "" {
		level = "passive"
	}
	body := map[string]any{"title": title, "body": text, "level": level}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(fmt.Sprintf("%s/%s", barkBase, cfg.Key.Value()), body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 回逍（2Im）
// ─────────────────────────────────────────────────────────────────────────────

// TwoIMRequest 构造回逍那一枪。
//
// 位置分工与旧实现一致：`key` / `sid` 与 `Query` 里的自由键进**查询串**，
// `msgtype` / `content` / `title` 与 `Extra` 进**请求体**。
// 旧实现用 `URLSearchParams`（键序 = append 顺序、空格编成 `+`）；
// 这里用 `url.Values.Encode()`，它按**键名排序**、空格也编成 `+`。
// 差异只在查询串里键的先后 —— 对 HTTP 服务端而言键序不构成身份，
// 但这一格要说清，否则"每次都不一样"会被当成偶发故障查。
func TwoIMRequest(cfg core.TwoIMConfig, title, text string) (core.HTTPRequest, *core.Error) {
	query := url.Values{}
	query.Set("key", cfg.Key.Value())
	query.Set("sid", cfg.SID.Value())
	for key, value := range cfg.Query {
		query.Add(key, fmt.Sprint(value))
	}
	msgType := cfg.MsgType
	if msgType == "" {
		msgType = "text"
	}
	body := map[string]any{"msgtype": msgType, "content": text, "title": title}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(twoIMBase+"?"+query.Encode(), body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 钉钉（唯一带签名的一家）
// ─────────────────────────────────────────────────────────────────────────────

// DingTalkRequest 构造钉钉那一枪。
//
// `timestampMS` 由调用方给（本层不带时钟，理由见 `notify.go` 里那一格）。
// 正文按旧实现合成 `title + "\n\n" + text`，但**不带**它那句里的前导空格
// （`" ${title}\n\n${text}"`）：那一个空格在任何一家文档里都没有作用，
// 而它会出现在用户收到的每一条消息开头 —— 那是旧实现的笔误，不是协议。
func DingTalkRequest(
	cfg core.DingTalkConfig, title, text string, timestampMS int64,
) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"msgtype": "text",
		"text":    map[string]any{"content": notifyBodyText(title, text)},
	}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(DingTalkSignedURL(cfg.Token, cfg.Secret, timestampMS), body)
}

// DingTalkSignedURL 拼钉钉的下发地址；`secret` 为空时不加签（与旧实现同口径：
// 只有 `if (secret)` 那一支才往查询串里追加 `timestamp` 与 `sign`）。
//
// # 签名算法（与旧实现逐步对齐）
//
// 旧版是 crypto-js：`HmacSHA256(timestamp + "\n" + secret, secret)` 再 base64，
// 最后 `encodeURIComponent`。crypto-js 的参数序是 **(message, key)**，
// 而本仓的 [core.HmacSHA256Base64] 是 **(key, message)** —— 两个顺序反过来
// 会得到一个**完全不同**的签名，而它只会在真发的时候才暴露，
// 所以这一格写死在下面那条例断里（`TestDingTalkSignedURLMatchesTheReferenceVector`）。
func DingTalkSignedURL(token, secret core.SecretValue, timestampMS int64) string {
	target := fmt.Sprintf("%s?access_token=%s", dingTalkBase, token.Value())
	if strings.TrimSpace(secret.Value()) == "" {
		return target
	}
	message := fmt.Sprintf("%d\n%s", timestampMS, secret.Value())
	sign := core.HmacSHA256Base64([]byte(secret.Value()), []byte(message))
	return target + fmt.Sprintf("&timestamp=%d&sign=%s", timestampMS, url.QueryEscape(sign))
}

// notifyTimestampMS 从注入的时钟取毫秒时间戳。第二档 false = 没接时钟。
func notifyTimestampMS(clock core.Clock) (int64, bool) {
	if clock == nil {
		return 0, false
	}
	return clock.Now().UnixMilli(), true
}
