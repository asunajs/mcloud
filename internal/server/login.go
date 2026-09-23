package server

// 登录：把 CLI 那条"一次调用等到结束"的流程拆成浏览器能驱动的**多步**。
//
// # 这一层只剩 HTTP 形状
//
// 会话表、轮询协程、TTL、退避、落库全在 [app.LoginHub]；本文件只做三件事：
// 取身份（[scopeOf]）、把结论写成 JSON、把 `*core.Error` 换成状态码。
// 出现任何"轮询/等待/上游码"的字样就是放错了层。
//
// # 为什么是"前端轮询 + 后台协程"，不是 SSE
//
// 一条登录最长 120 秒，而这期间只有四个阶段值要通知。SSE 要占住一个连接，
// 还要回答"页面刷新之后谁续上"—— 那条恰好又要一张会话表。既然表已经在了，
// 轮询就是更少的机制；而且它天然可恢复（换一台设备打开界面也能接着读结论）。
//
// # 凭据边界（这一组的三条硬规则）
//
//   - **请求进、凭据不出**：`auth`/验证码/手机号只进不落响应；响应里的
//     `accountId` 是掩码手机号（[core.NewAccountID] 由掩码得来），
//     成功结论只说"哪个账号落库了"；
//   - **二维码载荷可以出**：它是"给手机扫的那串 URL"，含设备指纹但**不含凭据**
//     （凭据是扫完之后由手机侧换回来的）。终端那条路也打印它当兜底链接；
//   - **会话按归属隔离**：[app.LoginHub] 取别人的 id 一律取不到，
//     这里据此给 404 而不是 403 —— 口径与存储层一致（"看不见即不存在"）。

import (
	"net/http"
	"strings"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

// qrStartBody 是出码请求体。
//
// `platform` 可空（缺省 Windows）。它决定上游看到的 `clientType`/`cpid`/版本号，
// 所以界面想选"macOS 客户端"时必须有这个旋钮 —— 但它不是必填项，
// 因为绝大多数人只是要扫个码。
type qrStartBody struct {
	Platform string `json:"platform"`
}

// startQRLogin 出一张码，并在返回前**确认已经拿到载荷**。
//
// 用 201：这一次调用真的产生了一个会话（占一个配额、有一条后台轮询在跑），
// 不是幂等地读一份已有资源。
func (s *Server) startQRLogin(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[qrStartBody](w, r)
	if !ok {
		return
	}
	platform, perr := loginPlatformOf(body.Platform)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}

	id, payload, perr := s.hub.StartQRLogin(r.Context(), scopeOf(auth), platform)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	// `qrUrl` 与图片是**同一串**的两种出口：图片是主路径，链接是兜底
	//（终端那条路同理 —— 有的手机扫不出变形码，手输链接还能走）。
	writeJSON(w, http.StatusCreated, map[string]any{
		"sessionId": id,
		"qrUrl":     payload,
	})
}

// loginSessionView 读一次会话结论（前端每 1-2 秒问一次，直到终态）。
//
// 扫码与短信两条路共用这个出口：`phase` 的取值集合相同，前端一套分支就够。
func (s *Server) loginSessionView(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	view, ok := s.hub.View(r.PathValue("id"), scopeOf(auth))
	if !ok {
		ErrDomain(core.NewNotFound("登录会话")).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, loginSessionPayload(view))
}

// qrImage 把这张码画成 SVG。
//
// `no-store` 不是套话：这张图的内容由**会话**决定，而会话会过期与替换。
// 缓存命中的表现是"扫了一张已经作废的码，手机说无效"，而用户只会认为是码生成得不对。
func (s *Server) qrImage(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	svg, found, perr := s.hub.QRImage(r.PathValue("id"), scopeOf(auth))
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	if !found {
		ErrDomain(core.NewNotFound("登录会话")).Write(w)
		return
	}
	w.Header().Set("Content-Type", "image/svg+xml")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(svg)
}

// ─────────────────────────────────────────────────────────────────────────────
// App 端短信验证码
// ─────────────────────────────────────────────────────────────────────────────

// sendCodeBody 是发码请求体。
//
// ⚠ 手机号是**完整**的（掩码号发不出短信）。它是入参、不是出参：
// 任何响应与错误串都不带它，错误也不插值用户的输入。
//
// `platform` 只有 PC 那条读（App 端冒充的是 iPad，端档案由设备串决定，没有这个旋钮）；
// 留在同一个结构里而不是分两个 body：App 那条多带一个不认识的键是无害的，
// 而两套几乎一样的结构体会让人以为"两端的行为可以各改各的"。
type sendCodeBody struct {
	Phone    string `json:"phone"`
	Platform string `json:"platform"`
}

// sendAppLoginCode 发一次 App 端验证码。
//
// 需要滑块兜底时**也是 201**（`captchaUrl` 非空、会话仍登记着）：
// 那一支的语义是"去这个地址拖一下，再回来填码"，不是失败。
// 若把它编成 4xx，前端就会丢掉 sessionId，第二步没有对象可填。
func (s *Server) sendAppLoginCode(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[sendCodeBody](w, r)
	if !ok {
		return
	}
	id, view, perr := s.hub.SendAppCode(r.Context(), scopeOf(auth), body.Phone)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	writeSessionCreated(w, id, view)
}

// verifyCodeBody 是验证码。
type verifyCodeBody struct {
	Code string `json:"code"`
}

// verifyAppLoginCode 用验证码换登录并落库。
//
// 同一个会话重复提交是**幂等**的（[app.LoginHub.VerifyAppCode] 挡住第二次落库），
// 所以这里用 `POST` 而不担心重试：前端超时重发不会多打一次上游。
func (s *Server) verifyAppLoginCode(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[verifyCodeBody](w, r)
	if !ok {
		return
	}
	view, perr := s.hub.VerifyAppCode(r.Context(), scopeOf(auth), r.PathValue("id"), body.Code)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, loginSessionPayload(view))
}

// ─────────────────────────────────────────────────────────────────────────────
// PC 端短信验证码
// ─────────────────────────────────────────────────────────────────────────────

// sendPcLoginCode 发一次 PC 端验证码。
//
// 与 App 那条的分工是**上游决定的**，不是我们摆的造型：PC 端要 `clientType`（Windows /
// macOS 两份端档案），滑块由 [app.SmsSendCode] 在内部自动解掉，所以这一支不会给
// `captchaUrl`；解不动就直接失败。CLI 的 `mcloud login sms --client PC` 走的是同一条驱动。
func (s *Server) sendPcLoginCode(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[sendCodeBody](w, r)
	if !ok {
		return
	}
	platform, perr := loginPlatformOf(body.Platform)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	id, view, perr := s.hub.SendPcCode(r.Context(), scopeOf(auth), platform, body.Phone)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	writeSessionCreated(w, id, view)
}

// verifyPcLoginCode 用 PC 端验证码换登录并落库。幂等口径同 [verifyAppLoginCode]。
func (s *Server) verifyPcLoginCode(w http.ResponseWriter, r *http.Request, auth *AuthUser) {
	body, ok := decodeJSON[verifyCodeBody](w, r)
	if !ok {
		return
	}
	view, perr := s.hub.VerifyPcCode(r.Context(), scopeOf(auth), r.PathValue("id"), body.Code)
	if perr != nil {
		ErrDomain(perr).Write(w)
		return
	}
	writeJSON(w, http.StatusOK, loginSessionPayload(view))
}

// ─────────────────────────────────────────────────────────────────────────────
// 形状与参数
// ─────────────────────────────────────────────────────────────────────────────

// writeSessionCreated 是"发码成功"那一支的响应：201 + 会话 id + 会话结论。
//
// 两端（App / PC）共用一个函数而不是各写五遍：这条响应的**形状是契约**，
// 前端只有一套分支。分开写迟早会有一边少给 `remainGetTimes`，
// 而那种缺字段的表现是"配额那行字不显示"，没人会怀疑到出参上。
//
// 出参里的手机号只留掩码：`accountId` 就是掩码号（见包注释的凭据边界），
// 界面上"已发到 199****2716"这一句由它渲染，不再回显入参。
func writeSessionCreated(w http.ResponseWriter, id string, view app.LoginSessionView) {
	payload := loginSessionPayload(view)
	payload["sessionId"] = id
	writeJSON(w, http.StatusCreated, payload)
}

// loginSessionPayload 是会话结论的对外形状。
//
// ⚠ 手写而不是给 [app.LoginSessionView] 加 JSON tag：那个结构体将来会加字段，
// 而"加了就自动出网"正是凭据泄漏最常见的路径（同 [accountPayload] 那条理由）。
func loginSessionPayload(view app.LoginSessionView) map[string]any {
	return map[string]any{
		"phase":     string(view.Phase),
		"accountId": view.AccountID,
		"message":   view.Message,
		// 只在发码那条路上有值；null 与 0 要分得开（null = 上游没给配额）
		"remainGetTimes": view.RemainGetTimes,
		"solvedSlider":   view.SolvedSlider,
		"captchaUrl":     view.CaptchaURL,
	}
}

// loginPlatformOf 把界面上的端名翻成端档案，空值给 Windows。
//
// 只认两个值并且**显式拒绝**别的：`clientType` 写错上游会直接 401，
// 而 401 在界面上长得和"口令不对"一样 —— 在这里挡下来才给得出人话。
func loginPlatformOf(value string) (upstream.PcPlatform, *core.Error) {
	switch strings.ToLower(strings.TrimSpace(value)) {
	case "":
		return upstream.PlatformWindows, nil
	case "windows":
		return upstream.PlatformWindows, nil
	case "macos", "mac", "osx":
		return upstream.PlatformMacOS, nil
	}
	return upstream.PlatformWindows, core.NewBadRequest(
		"platform 只支持 windows 与 macos（留空即 windows）")
}
