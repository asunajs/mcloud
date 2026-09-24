// 推送渠道里**不是一次请求**的那三家：企业微信应用（两步）、customPost（多条）、
// email（不走 HTTP）。
//
// 单独一个文件的理由：这三家的"结果"不是一个请求的成败，把它们塞进
// `notify_channels.go` 会让那个文件同时承担"纯请求构造"和"多次发送的编排"，
// 而前者是能被逐字节断言的、后者不是。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 企业微信**应用**消息：先换 access_token，再发消息
// ─────────────────────────────────────────────────────────────────────────────

// WorkWeixinTokenRequest 构造换 `access_token` 那一枪。
//
// ⚠ **形状照旧实现**（`packages/shared/src/push/index.ts:140-144`）：POST + JSON 体
// `{corpid, corpsecret}`。这一口对不对，本仓**零样本** —— 企业微信这一家的公开形状
// 是 GET + 查询串，而我**没有**仓库内的出处能证明那一句（只有"通常是 GET"的印象，
// 印象不算判据，见 AGENTS §1.10）。
// 所以这里按"接口真相 = 抓包 > 开源 Go > 旧 TS > 猜测"的第三档落，并把这一格
// 登记进 `docs/pending-decisions.md`：真发一次看 `errcode`，
// 若回非 0 就把这一枪改成 GET + 查询串再比一次（两档改动都只在这一个函数里）。
func WorkWeixinTokenRequest(cfg core.WorkWeixinConfig) (core.HTTPRequest, *core.Error) {
	body := map[string]any{"corpid": cfg.CorpID.Value(), "corpsecret": cfg.CorpSecret.Value()}
	return jsonPostRequest(workWeixinAPIBase+"/gettoken", body)
}

// WorkWeixinSendRequest 构造发消息那一枪（access_token 进查询串）。
//
// `msgtype` 默认 `text`，正文放在**与 msgtype 同名的键**下面（`{"text":{"content":…}}`）——
// 那是企业微信的形状，不是我们拼的键名。旧实现还有一句"用户配的 msgtype 决定键名"，
// 于是配 `markdown` 时正文字段变成 `{"markdown":{"content":…}}`：这一格保留，
// 但 ⚠ 我们**没有**任何一家的消息体实拍，非 `text` 那几档的字段名对不对未测试。
func WorkWeixinSendRequest(
	cfg core.WorkWeixinConfig, accessToken string, title, text string,
) (core.HTTPRequest, *core.Error) {
	msgType := cfg.MsgType
	if msgType == "" {
		msgType = "text"
	}
	toUser := cfg.ToUser
	if strings.TrimSpace(toUser) == "" {
		toUser = "@all" // 旧实现的解构默认值：不给收件人就是全员
	}
	body := map[string]any{
		"touser":  toUser,
		"msgtype": msgType,
		msgType:   map[string]any{"content": notifyBodyText(title, text)},
	}
	if cfg.AgentID != nil {
		body["agentid"] = *cfg.AgentID
	}
	mergeExtra(body, cfg.Extra)
	return jsonPostRequest(
		fmt.Sprintf("%s/message/send?access_token=%s", workWeixinAPIBase, accessToken), body)
}

// parseWorkWeixinToken 从换票响应里取 `access_token`。
//
// 非 JSON、缺字段、`errcode` 非 0 三样都要判失败 —— 尤其是"缺字段"：
// 拿着空串去发第二条，URL 会变成 `?access_token=`，而上游回的那句
// 与"corpid 错了"长得一样，排障的人会往错的方向走。
func parseWorkWeixinToken(body []byte) (string, string, bool) {
	value, ok := core.ParseJSON(string(body))
	if !ok {
		return "", "换票响应不是 JSON", false
	}
	obj, isObject := value.(map[string]any)
	if !isObject {
		return "", "换票响应不是对象", false
	}
	if code, present := obj["errcode"]; present {
		if n, isNumber := asInt64(code); isNumber && n != 0 {
			return "", fmt.Sprintf("换票回了 errcode=%d：%s", n, replyMessage(obj)), false
		}
	}
	token, _ := obj["access_token"].(string)
	if strings.TrimSpace(token) == "" {
		return "", "换票响应里没有 access_token", false
	}
	return token, "", true
}

// sendWorkWeixin 走那两步。任一步失败都只记这一家的结论，不影响别家。
func (s *NotifySender) sendWorkWeixin(
	ctx context.Context, cfg *core.WorkWeixinConfig, title, text string,
) NotifyResult {
	const name = core.NotifyWorkWeixin
	tokenRequest, perr := WorkWeixinTokenRequest(*cfg)
	if perr != nil {
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: "换票请求构造失败"}
	}
	tokenResponse, err := s.http.Send(ctx, tokenRequest)
	if err != nil {
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: transportReason(err)}
	}
	if !tokenResponse.IsSuccess() {
		return NotifyResult{Channel: name, Outcome: NotifyFailed,
			Reason: fmt.Sprintf("换票 HTTP %d", tokenResponse.Status)}
	}
	token, reason, ok := parseWorkWeixinToken(tokenResponse.Body)
	if !ok {
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: reason}
	}
	sendRequest, perr := WorkWeixinSendRequest(*cfg, token, title, text)
	if perr != nil {
		return NotifyResult{Channel: name, Outcome: NotifyFailed, Reason: "发消息请求构造失败"}
	}
	return s.sendRequest(ctx, name, sendRequest)
}

// ─────────────────────────────────────────────────────────────────────────────
// customPost：一家多条
// ─────────────────────────────────────────────────────────────────────────────

// CustomPostRequest 构造通用 webhook 的一条。
//
// # 占位符替换分两个位置，而且**编码方式不同**（旧实现就是这样，各有理由）
//
//   - URL 里的 `{title}` / `{text}` 走 `encodeURIComponent` ⇒ 这里对应 [encodeURIComponent]。
//     不编码的话正文里一个 `&` 就会把查询串改写掉，那是**注入**而不是截断。
//   - 请求体里的占位符是"整串 JSON 文本上替换再重新解析"（旧实现
//     `JSON.stringify(payload).replace(/{title}/g, title)`）⇒ 放在引号里就是普通字符串值，
//     但正文里若带 `"` 会让重新解析失败。这里把那次失败如实报成这一条 failed，
//     而不是替用户转义 —— 转义了就和旧实现不是同一件事了。
//
// ⚠ 与旧实现有一处不同：它替换 URL 时**没有** `/g`，所以同一 URL 里第二次出现
// `{title}` 不会被替换。那是笔误级别的差异，这里全部替换。
func CustomPostRequest(cfg core.CustomPostConfig, title, text string) (core.HTTPRequest, *core.Error) {
	if strings.TrimSpace(cfg.URL) == "" {
		return core.HTTPRequest{}, core.NewBadRequest("customPost 缺少 url")
	}
	target := strings.NewReplacer("{title}", encodeURIComponent(title), "{text}", encodeURIComponent(text)).
		Replace(cfg.URL)

	kind, ok := customPostMethod(cfg.Method)
	if !ok {
		return core.HTTPRequest{}, core.NewBadRequest("customPost 的方法只认 GET / POST / PUT / DELETE")
	}
	request := core.HTTPRequest{Method: kind, URL: target, Headers: []core.HeaderPair{}}
	for key, value := range cfg.Headers {
		request = request.WithHeader(key, value)
	}
	if cfg.Timeout != nil && *cfg.Timeout > 0 {
		timeout := uint64(*cfg.Timeout)
		request.TimeoutMS = &timeout
	}

	payload := cfg.Data
	if payload == nil {
		payload = cfg.Body
	}
	if len(payload) == 0 {
		return request, nil
	}
	// 与旧实现同口径：先序列化、再在**文本上**替换、再解析回来。
	serialized, err := json.Marshal(payload)
	if err != nil {
		return request, core.NewInternal("customPost 请求体序列化失败")
	}
	replaced := strings.NewReplacer("{title}", title, "{text}", text).Replace(string(serialized))
	var body any
	if err := json.Unmarshal([]byte(replaced), &body); err != nil {
		return request, core.NewBadRequest("customPost 请求体里的正文不是合法 JSON（含引号或反斜杠）")
	}
	withBody, berr := request.WithJSONBody(body)
	if berr != nil {
		return request, core.NewInternal("customPost 请求体序列化失败")
	}
	return withBody, nil
}

// customPostMethod 把配置里那个**大写串**折成 [core.HTTPMethod]。
//
// 只认这四个（与旧实现的 `PushMethod` 同一个集合），空串按 POST：
// 旧实现那句是 `apiTemplate.method?.toUpperCase() || "POST"`。
// 收到第五种写法（`PATCH`、`head` 小写）时**判失败而不是兜成 POST** ——
// 用错方法发出去的 webhook 有可能真的改数据，那一次不是"推送通知"。
func customPostMethod(method string) (core.HTTPMethod, bool) {
	switch strings.ToUpper(strings.TrimSpace(method)) {
	case "", "POST":
		return core.MethodPost, true
	case "GET":
		return core.MethodGet, true
	case "PUT":
		return core.MethodPut, true
	case "DELETE":
		return core.MethodDelete, true
	}
	return core.MethodPost, false
}

// encodeURIComponent 对齐 JS 的 `encodeURIComponent`。
//
// `url.QueryEscape` 与它的差别只有空格（`+` vs `%20`）—— 而在**查询串**里
// 两种写法服务端通常都认，所以不追求逐字节；空格按 `%20` 是为了与旧实现一致，
// 少一处"我们和参考实现对不上"的解释成本。
func encodeURIComponent(text string) string {
	return strings.ReplaceAll(url.QueryEscape(text), "+", "%20")
}

// sendCustomPosts 逐条发这一家配的所有 webhook，合成**一条**结论。
//
// 为什么合成一条而不是一枪一条：渠道那一列在界面与 `run_events` 里是"一家一行"，
// 三家各两条时会变成六行，读者要先做加法才知道这一家成没成。
// 每条的详情拼在 `Reason` 里（截断）。
//
// ⚠ 判据比别家**松**：只看 HTTP 状态，不套 [classifyChannelReply] 那三键。
// 通用 webhook 的响应形状是用户自己定的，回一段 HTML 的网关、回 `{"code":"OK"}`
// 的自建服务都在这一家出现，套那三键会把"其实发成功了"判成失败。
// 旧实现连状态都不看（`ctx.fetch` 不抛就算成功），这里至少看状态码。
func (s *NotifySender) sendCustomPosts(
	ctx context.Context, items []core.CustomPostConfig, title, text string,
) NotifyResult {
	const name = core.NotifyCustomPost
	var (
		details  []string
		failures int
	)
	for i, item := range items {
		request, perr := CustomPostRequest(item, title, text)
		if perr != nil {
			failures++
			details = append(details, fmt.Sprintf("第 %d 条构造失败", i+1))
			continue
		}
		response, err := s.http.Send(ctx, request)
		if err != nil {
			failures++
			if response.Status != 0 {
				details = append(details, fmt.Sprintf("第 %d 条 HTTP %d", i+1, response.Status))
				continue
			}
			details = append(details, fmt.Sprintf("第 %d 条未发出", i+1))
			continue
		}
		if !response.IsSuccess() {
			failures++
			details = append(details, fmt.Sprintf("第 %d 条 HTTP %d", i+1, response.Status))
			continue
		}
		details = append(details, fmt.Sprintf("第 %d 条已发", i+1))
	}
	outcome := NotifySent
	if failures > 0 {
		outcome = NotifyFailed
	}
	return NotifyResult{
		Channel: name, Outcome: outcome,
		Reason: clipRunes(strings.Join(details, "；"), 200),
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// email：不走 HTTP，走注入的 [core.MailSender]
// ─────────────────────────────────────────────────────────────────────────────

// EmailMessage 把渠道配置 + 标题正文折成一封 [core.MailMessage]。
//
// 导出是为了让"这一封长什么样"能被断言 —— 邮件是本族唯一没有 HTTP 请求可看的一家，
// 不导出就只能连着一台 SMTP 服务器一起测（那是最贵的一条路，见 `core/mail.go`）。
//
// `From` 的显示名拼法照旧实现（`"标题 <地址>"`）：那一句会让收件人**在列表页**
// 先看到标题而不是发件人。看着别扭，但它是旧版跑得通的那一份形状。
func EmailMessage(cfg core.EmailConfig, title, text string) core.MailMessage {
	port := 0
	if cfg.Port != nil {
		port = *cfg.Port
	}
	return core.MailMessage{
		Host:     cfg.Host,
		Port:     port,
		Username: cfg.From,
		Password: cfg.Pass,
		From:     fmt.Sprintf("%s <%s>", title, cfg.From.Value()),
		To:       cfg.To,
		Subject:  title,
		Text:     text,
	}
}

// sendEmail 发一封邮件。`s.mail == nil` 算 skipped，不算 failed：
// 这一家需要用户在**服务端**另有 SMTP 出口，CLI 与预览实例刻意没接它，
// 那种情况下"没接"是正常态，报 failed 会让人以为配错了。
func (s *NotifySender) sendEmail(
	ctx context.Context, cfg *core.EmailConfig, title, text string,
) NotifyResult {
	const name = core.NotifyEmail
	if s.mail == nil {
		return NotifyResult{Channel: name, Outcome: NotifySkipped, Reason: "本进程没接 SMTP 发送器"}
	}
	if len(core.MailRecipients(cfg.To)) == 0 {
		return NotifyResult{Channel: name, Outcome: NotifySkipped, Reason: "没填收件人"}
	}
	if err := s.mail.SendMail(ctx, EmailMessage(*cfg, title, text)); err != nil {
		// 邮件这一句照原文留（它是这一族最常见的排障信号：`535` 授权码错、`550` 没这个收件人），
		// 但先把授权码从文本里摘掉：个别 SMTP 服务会把最后一条命令回显在错误应答里。
		return NotifyResult{Channel: name, Outcome: NotifyFailed,
			Reason: clipRunes(redactSecrets(err.Error(), cfg.Pass), 120)}
	}
	return NotifyResult{Channel: name, Outcome: NotifySent}
}
