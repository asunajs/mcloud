// 云笔记那一族（玩法 **107「创建云笔记」**）：三口一串，**两枪是写**。
//
//	POST note.mcloud.139.com/noteServer/api/authTokenRefresh.do   → 只读**响应头** `app_auth`
//	POST mnote.caiyun.feixin.10086.cn/noteServer/api/createNote.do      （**写**：在用户笔记里造一条）
//	POST mnote.caiyun.feixin.10086.cn/noteServer/api/moveToRecycleBin.do （**写**：把它移进回收站）
//
// # 两个参考实现，而其中**只有一个真的跑过**
//
//   - 旧 TS `packages/core/src/apis/note.ts` + 调用点 `tasks/app/runners.ts:47-66`：
//     ⚠ 那一条 runner **从来没有真的跑通过** —— `apis/note.ts` 交回的是
//     `{body, headers}` 自己造的对象，而 `http.post` 在 ofetch 下**直接返回解析后的响应体**
//     （`packages/shared/src/http/base.ts:113-131` 没有设 `responseType`），于是
//     `(resp as any).headers` 是 `undefined` ⇒ 那句 `if (!noteAuth?.headers?.app_auth)`
//     恒成立 ⇒ 每次都走"获取笔记鉴权失败，跳过云笔记任务"那一支，后两枪**一枪都没发过**。
//     这一格本仓早有盘点（[`docs/business-feature-inventory.md`](../../docs/business-feature-inventory.md)
//     :383 那句"实测 bug：`107` 恒跳过"）。⇒ 本文件的请求面**不能**说"旧实现发过"。
//   - 开源 Go 参考实现 [3238614968/caiyun](https://github.com/3238614968/caiyun) 的
//     `backend/internal/core/api/caiyun_note.go`（三口）+ 调用点
//     `internal/core/tasks/tasklist.go:540`（`handleNoteTask`，注释直写"云笔记任务（107）"）
//     与 `internal/core/tasks/activity_actions.go:89`（`createTempNote`）——
//     它是**在跑的**实现，也是本文件里"响应头怎么读、往下游透传哪几枚"的**唯一**证据源。
//
// 两家的**请求面逐字相同**（同一批 URL、同一批头名与面值、同一份 createNote 体），
// 差别只在两处，本文件取**跑过的那一家**并把它记在下面：
//   - 透传哪些响应头：TS 是 `...headers`（全量摊进请求头，但它从没跑到那一行）；
//     caiyun 只把 `app_auth` 这一枚带进后两枪 ⇒ 本层**只带 `app_auth`**。
//     全量透传会把 `set-cookie`、`content-length`、`transfer-encoding` 这些**传输层头**
//     也发回上游，而那一格没有任何实现验证过。
//   - `createNote` 的 `tags`：TS 发 `[]`，caiyun 传 `nil` 时 Go 会 marshal 成 `null` ⇒
//     本层发 `[]`（与 TS 一致，且不依赖"上游把 null 当空数组"这一格未验证的宽容）。
//
// # ⚠ 三口**零 REST 实拍**
//
// 本仓 `har/` 与 `test/fixtures/` 里**没有一条** `noteServer/*`（2026-09-25 按
// `noteServer` / `mnote.caiyun` / `app_auth` / `note_token` 四个词逐份扫过那 14 份 har，
// 零命中）。[`docs/client-profiles.md`](../../docs/client-profiles.md) §3 那一行
// "UA=`mobile`、6 条请求、特有头 `app_auth app_cp app_number note_token cp_version hcy-cool-field charset`"
// 说的是**画像统计**，不是"这三口各有一次完整往返"—— 那一批原始流量已不在仓里，
// 现在能引到的只有这张表。⚠ 它也**不**读成"这一族要发 `note_token`"：那一枚出现在
// 同一组里，但没成文说是哪一口带的（要判得先拍到一条完整往返）。
//
// ⇒ 与上传族、外链族同一处置：请求面**逐字**照参考实现，测试只钉"发得出 + 形状对"，
// **不钉语义**；"上游认不认这一套"只有真跑一次才有答案（AGENTS.md §1.13）。
//
// # 鉴权那一枪**不看 `code`**，这一格是照抄而不是偷懒
//
// 两家参考实现的判据都是同一句"`app_auth` 有没有"，谁都不看它的响应体。
// 本层照它，但把**失败原因**补全：拿不到 `app_auth` 时，若响应体是 JSON 就顺带念出
// `code` / `message`（两家在这里都只报一句"未获取到 app_auth"，线上真出问题时没有线索）。
// ⚠ 那只影响错误文本，**不改判定**：`code` 非 0 而 `app_auth` 在 ⇒ 仍按两家原样继续。
package upstream

import (
	"context"
	"fmt"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
)

// BaseNoteMnote 是**写**那两枪的主机（与鉴权那一枪**不同一台**）。
//
// ⚠ 别与 [BaseNoteMcloud]（`note.mcloud.139.com`，只发鉴权那一枪）或 [BaseCaiyun]
// （`caiyun.feixin.10086.cn`，签到那一族）混成一个：三台都以 `feixin.10086.cn`
// 或 `mcloud.139.com` 结尾 ⇒ 在旧 TS 那张 `AUTH_DOMAINS` 表里都命中"带 authorization"，
// 但**路径与前缀完全不同**，搬错一台就是打到另一个服务上。
const BaseNoteMnote = "https://mnote.caiyun.feixin.10086.cn"

// 这一族的三枚路径（两家的字面量逐字相同）。
const (
	notePathAuthTokenRefresh = "/noteServer/api/authTokenRefresh.do"
	notePathCreate           = "/noteServer/api/createNote.do"
	notePathMoveToRecycleBin = "/noteServer/api/moveToRecycleBin.do"
)

// NoteAppAuthHeader 是鉴权那一枪**响应头**里那一枚会话凭据的名字。
//
// ⚠ 名字里是**下划线**，不是连字符（`app-auth` 是另一枚、本族没人发）。
// 而 Go 的 `http.Header` 会把它规范化成 `App_auth` ⇒ 读取必须**大小写不敏感**，
// 见 [ParseNoteAppAuth]。
const NoteAppAuthHeader = "app_auth"

// 这一族自己的四枚字面量。它们的角色与外链族那五枚一样：**素材字面量**，
// 对任何账号都发同一串 ⇒ 整串照抄，不按仓库里其它族的常量重拼。
const (
	// noteAuthCPVersion 是鉴权那一枪的 `CP_VERSION`（`3.2.0`，安卓笔记客户端的版本）。
	noteAuthCPVersion = "3.2.0"
	// noteClientCPVersion 是后两枪的 `CP_VERSION`（`7.7.1.20240115`，**PC** 笔记客户端的版本）。
	//
	// ⚠ 与上一枚**不是同一个端**：鉴权那一枪 `APP_CP: android`，写那两枪 `APP_CP: pc`。
	// 把两枚统一成一个的表现是上游按另一套客户端画像处理，而哪一枚"才对"没有样本。
	noteClientCPVersion = "7.7.1.20240115"
	// noteAuthChannelSrc 是鉴权那一枪的 `x-huawei-channelsrc`。
	//
	// ⚠ 它与 [CloudChannelWeb]（`10000034`）、[IOSChannel]（`10000020`）是**不同值**，
	// 与本族后两枪又**无关**（那两枪不带这一枚）。
	noteAuthChannelSrc = "10001400"
	// noteMobileUserAgent 是鉴权那一枪的 `User-Agent` —— 字面量就是 `mobile` 这五个字母。
	//
	// ⭐ 这一枚是"要不要照抄"的判断题，而答案是**照抄**：两家都在发它，而
	// [`docs/client-profiles.md`](../../docs/client-profiles.md) §3 里那 6 条真实请求
	// 的 UA 分组名**正是** `mobile` ⇒ 它是线上真的收到过的值，不是笔误。
	// 把它"修成"正常的安卓 WebView UA，等于发一份上游没收过这份画像的请求。
	noteMobileUserAgent = "mobile"
)

// ─────────────────────────────────────────────────────────────────────────────
// 第一口：POST note.mcloud.139.com/noteServer/api/authTokenRefresh.do（只读，但**发的是凭据**）
// ─────────────────────────────────────────────────────────────────────────────

// NoteAuthTokenRequest 构造换笔记凭据那一发。
//
// ⚠ 请求体里的 `authToken` 与头里的 `authorization` **都是凭据**（前者就是登录 token 本身）：
// 只进请求，不进任何错误文本与日志（本文件所有错误串只说"缺 authToken"，不 echo 值）。
// 手机号也一样：它是 `APP_NUMBER` 与体里的 `userPhone`，错误文本里同样不出现。
//
// `authorization` 传空 ⇒ 不带那一枚（旧 TS 的主实例对 `*.mcloud.139.com` 会注入它，
// 但"这一口没有它认不认"零样本 ⇒ 由调用方决定，本层不猜、也不拦）。
func NoteAuthTokenRequest(authorization, authToken, phone string) (core.HTTPRequest, *core.Error) {
	if authToken == "" {
		return core.HTTPRequest{}, core.NewInternal("笔记鉴权缺 authToken：它是这一个体的唯一凭据键（错误文本刻意不带它的值）")
	}
	if phone == "" {
		return core.HTTPRequest{}, core.NewInternal("笔记鉴权缺 userPhone：两家参考实现都总是带手机号（错误文本刻意不带它的值）")
	}
	request, err := core.NewRequestPost(BaseNoteMcloud + notePathAuthTokenRefresh).
		WithJSONBody(map[string]any{"authToken": authToken, "userPhone": phone})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = request.
		WithHeader("APP_CP", "android").
		WithHeader("APP_NUMBER", phone).
		WithHeader("CP_VERSION", noteAuthCPVersion).
		WithHeader("x-huawei-channelsrc", noteAuthChannelSrc).
		WithHeader("User-Agent", noteMobileUserAgent)
	if authorization != "" {
		request = request.WithHeader("authorization", authorization)
	}
	return request, nil
}

// ParseNoteAppAuth 从鉴权那一发的**响应头**里取 `app_auth`。
//
// # 为什么这一口不走 [Envelope.EnsureOK]
//
// 两家参考实现的判据都是同一句"`app_auth` 有没有"，谁都不看它的响应体 ⇒
// 在这里加一条 `code` 判据，等于让本仓在一家**在跑的**实现之外单独改判
// （R27 同族：判据只在有出处的时候才落码）。
//
// 但失败时本层**多说一句**：拿不到那一枚 ⇒ 若响应体是 JSON，把 `code` / `message`
// 附进错误文本（两家在这里都只报"未获取到 app_auth"，线上真出问题时无从下手）。
// ⚠ 那只是**原因**，不是判定 ⇒ `code` 非 0 而 `app_auth` 在时仍按成功交回。
//
// 头名**大小写不敏感**，而且这不是防御性代码：Go 的 `net/http` 会把
// `app_auth` 规范化成 `App_auth`（首字母大写、其余小写），而 caiyun 里那句
// `resp.Header.Get("app_auth")` 之所以能命中，靠的正是 `Get` 自己再做一次规范化。
// 我们手里的是 [core.HeaderPair] 列表（本层自己摊平的），没有那层规范化 ⇒ 必须自己比。
func ParseNoteAppAuth(response core.HTTPResponse) (core.SecretValue, *core.Error) {
	for _, pair := range response.Headers {
		if !strings.EqualFold(pair[0], NoteAppAuthHeader) {
			continue
		}
		if strings.TrimSpace(pair[1]) == "" {
			// 头在而值为空白 ⇒ 判"没给"：两家的判据都是"非空才算拿到"。
			break
		}
		return core.NewSecretValue(pair[1]), nil
	}
	return "", core.NewParse("笔记鉴权没回 " + NoteAppAuthHeader + noteFailureReason(response))
}

// noteFailureReason 给"没拿到 app_auth"那一条补上响应自己说的话。
//
// ⚠ 只取 `code` 与 `message` 两格，**不取载荷**：笔记这一口的响应体形状没有任何样本，
// 而"整块塞进错误文本"在这一族等于把可能带凭据的内容写进日志（与
// [`envelope.go`](envelope.go) 里 `Detail()` **只收字符串**同一条纪律）。
// 非 JSON、或没有这两格 ⇒ 交回空串，那句失败就只说它自己知道的。
func noteFailureReason(response core.HTTPResponse) string {
	envelope, perr := ParseEnvelope(response.Body)
	if perr != nil {
		return ""
	}
	parts := make([]string, 0, 2)
	if code, ok := envelope.Code(); ok {
		parts = append(parts, fmt.Sprintf("code=%d", code))
	}
	if msg, ok := envelope.Message(); ok {
		parts = append(parts, msg)
	}
	if len(parts) == 0 {
		return ""
	}
	return "｜上游响应：" + strings.Join(parts, ": ")
}

// AppNoteAuthToken 换笔记会话凭据（只读，但把登录 token 发给了上游）。
func (u *Upstream) AppNoteAuthToken(
	ctx context.Context, authorization, authToken, phone string,
) (core.SecretValue, *core.Error) {
	request, perr := NoteAuthTokenRequest(authorization, authToken, phone)
	if perr != nil {
		return "", perr
	}
	// 走 [Raw] 而不是 [Call]：要的是**响应头**，而 `Call` 一定先把响应体喂给
	// `ParseEnvelope` —— 上游若在这一口回一个空体或非 JSON，那一句"响应不是 JSON"
	// 会掩盖真正的问题（凭据不对 ⇒ 上游根本不给 `app_auth`）。
	response, perr := u.Raw(ctx, request)
	if perr != nil {
		return "", perr
	}
	return ParseNoteAppAuth(response)
}

// ─────────────────────────────────────────────────────────────────────────────
// 第二口：createNote.do（**写**：在用户笔记里造一条真笔记）
// ─────────────────────────────────────────────────────────────────────────────

// NoteCreateInput 是"造一条笔记"的调用方输入。
//
// 做成结构体与 [CloudOutLinkInput] 同一条理由：这一口的体里有**二十六个键**，
// 连排的位置参数会让"传错顺序"变成一个能编译通过的 bug。
type NoteCreateInput struct {
	// NoteID 同时进**三处**：顶层 `noteid`、`contents[0].noteId`，以及后一枪
	// `moveToRecycleBin` 的 `noteids[0].noteid` —— 三处都是同一个值。
	//
	// ⚠ 键名大小写**不一致**是原样：顶层全小写，`contents` 里是 camelCase。
	NoteID string
	// Title 是笔记标题（两家都用随机短串）。
	Title string
	// Phone 是 `userphone` 与 `APP_NUMBER`（手机号明文进体，同外链族那一格）。
	Phone string
}

// NoteCreateRequest 构造"建一条笔记"那一发（**写**，不可回滚 —— 只能移进回收站）。
//
// 体里那二十六键逐字来自两家（TS `apis/note.ts:33-70`、caiyun `caiyun_note.go:78-106`
// 完全一致）。那些空串与零值**不是**"没填"：`archived:0` / `topmost:"0"` /
// `remindtype:0` / `revision:"1"` / `expands:{noteType:0}` 是这一族的**默认笔记形状**，
// 少一格或改成 null 就是发一份上游没收过的体。数字与字符串的混用（`revision` 是串、
// `remindtype` 是数、`topmost` 是串）也**原样保留**，不"顺手统一类型"。
//
// `nowMillis` 是 `createtime` / `updatetime` / `visitTime` 三格共用的毫秒数，
// **由调用方注入**（本层是纯层：请求构造函数不许读时钟，与
// [`app_signin.go`](app_signin.go) 里 `timestamp int64` 同一族口径）。
// 三格取**同一个值**是照 caiyun：它算一次 `UnixMilli()` 复用三份，而旧 TS 是三次
// `Date.now()`（理论上会跨毫秒）。差别只影响"上游看到的是不是同一时刻"，
// 它到底比不比这一格**未验证**（R32）。
//
// ⚠ 传 0 或负数 ⇒ 报错而不是发出去：零值时间戳的含义是"调用方没给时刻"，
// 那时正确的作用是改调用方，而不是让上游去猜。
func NoteCreateRequest(
	authorization string, appAuth core.SecretValue, in NoteCreateInput, nowMillis int64,
) (core.HTTPRequest, *core.Error) {
	if in.NoteID == "" {
		return core.HTTPRequest{}, core.NewInternal("createNote 缺 noteid：后一枪要靠它删掉这一条，缺了就会留下垃圾笔记")
	}
	if appAuth.Value() == "" {
		return core.HTTPRequest{}, core.NewInternal("createNote 缺 app_auth：它是这一族唯一的会话凭据")
	}
	if in.Phone == "" {
		return core.HTTPRequest{}, core.NewInternal("createNote 缺 userphone：两家都总是带（错误文本刻意不带它的值）")
	}
	if nowMillis <= 0 {
		return core.HTTPRequest{}, core.NewInternal("createNote 缺毫秒时间戳：由调用方注入时钟，本层不读 `time.Now()`")
	}
	// 三格共用同一个值：TS 是三次 `Date.now()`（理论上会跨毫秒），caiyun 明确算一次复用。
	// 这里取 caiyun 那一档 —— **一个值用三处**（R32）。
	now := strconv.FormatInt(nowMillis, 10)
	request, err := core.NewRequestPost(BaseNoteMnote + notePathCreate).WithJSONBody(map[string]any{
		"archived":        0,
		"attachmentdir":   "",
		"attachmentdirid": "",
		"attachments":     []any{},
		"contentid":       "",
		"contents": []any{
			map[string]any{"data": "<span></span>", "noteId": in.NoteID, "sortOrder": 0, "type": "TEXT"},
		},
		"cp":          "",
		"createtime":  now,
		"description": "",
		"expands":     map[string]any{"noteType": 0},
		"landMark":    []any{},
		"latlng":      "",
		"location":    "",
		"noteid":      in.NoteID,
		"remindtime":  "",
		"remindtype":  0,
		"revision":    "1",
		"system":      "",
		"tags":        []any{},
		"title":       in.Title,
		"topmost":     "0",
		"updatetime":  now,
		"userphone":   in.Phone,
		"version":     "",
		"visitTime":   now,
	})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = request.
		WithHeader("APP_CP", "pc").
		WithHeader("APP_NUMBER", in.Phone).
		WithHeader("CP_VERSION", noteClientCPVersion).
		WithHeader(NoteAppAuthHeader, appAuth.Value())
	if authorization != "" {
		request = request.WithHeader("authorization", authorization)
	}
	return request, nil
}

// AppNoteCreate 建一条云笔记（**写**）。
//
// 判据走 [Envelope.EnsureOK]（非 0 即失败），与 caiyun 那句
// `code` 不是 `""` 也不是 `"0"` 就报错同向；比旧 TS 好的地方是**这里真的看它** ——
// 旧 TS 用 `tryRequest` 把返回的元组整个丢弃，建失败也照发删除那一枪。
//
// ⚠ 本层**不读任何载荷字段**：`createNote` 回什么形状零样本（连"在跑的那家"也只读 `code`），
// 所以"笔记 id 是不是就是我们发过去的那一枚"没有上游确认 —— 删除那一枪靠的是**我们生成的** id。
func (u *Upstream) AppNoteCreate(
	ctx context.Context, authorization string, appAuth core.SecretValue,
	in NoteCreateInput, nowMillis int64,
) *core.Error {
	request, perr := NoteCreateRequest(authorization, appAuth, in, nowMillis)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = envelope.EnsureOK()
	return perr
}

// ─────────────────────────────────────────────────────────────────────────────
// 第三口：moveToRecycleBin.do（**写**：移进回收站，**不物删**）
// ─────────────────────────────────────────────────────────────────────────────

// NoteMoveToRecycleBinRequest 构造"把笔记移进回收站"那一发（**写**）。
//
// ⚠ 它是**回收站**，不是删除：两家都到此为止，笔记会留在用户的回收站里直到上游自己清。
// 本仓没有"物删回收站笔记"那一口（零实现、零样本）⇒ 这一族的产物**进不了**
// `app.TaskMatrixRunResult.CloudFileIDs` 那条网盘文件清理链（它清的是网盘文件，另一族）。
// 这一格是**已知残留**，登记在 `docs/pending-decisions.md` R32。
//
// ⚠ 与上一口**差一枚头**：这一口不带 `APP_NUMBER`（TS `apis/note.ts:73-88` 与
// caiyun `DeleteNote` 两家一致）。看着像漏写，但上游收到过的就是"一个带、一个不带"⇒
// 统一两处等于改一次接口，而那一格零样本（同外链族那处不对称）。
func NoteMoveToRecycleBinRequest(
	authorization string, appAuth core.SecretValue, noteID string,
) (core.HTTPRequest, *core.Error) {
	if noteID == "" {
		return core.HTTPRequest{}, core.NewInternal("moveToRecycleBin 收到空 noteid：没有要删的东西就不发这一枪")
	}
	if appAuth.Value() == "" {
		return core.HTTPRequest{}, core.NewInternal("moveToRecycleBin 缺 app_auth：没有它就删不掉刚建的那一条")
	}
	request, err := core.NewRequestPost(BaseNoteMnote + notePathMoveToRecycleBin).
		WithJSONBody(map[string]any{
			"noteids": []any{map[string]any{"noteid": noteID}},
		})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = request.
		WithHeader("APP_CP", "pc").
		WithHeader("CP_VERSION", noteClientCPVersion).
		WithHeader(NoteAppAuthHeader, appAuth.Value())
	if authorization != "" {
		request = request.WithHeader("authorization", authorization)
	}
	return request, nil
}

// AppNoteMoveToRecycleBin 把一条云笔记移进回收站（**写**，不可回滚）。
//
// 与 [Upstream.AppCloudDelOutLink] 同一条纪律：**不看它就不能静默**。
// 旧 TS 在这里也是 `tryRequest` 丢弃返回值 ⇒ 删除失败时用户笔记里多一条随机标题的笔记，
// 而日志一个字都没有。这里 [Envelope.EnsureOK] 非 0 码与 `success:false` 都回错误。
func (u *Upstream) AppNoteMoveToRecycleBin(
	ctx context.Context, authorization string, appAuth core.SecretValue, noteID string,
) *core.Error {
	request, perr := NoteMoveToRecycleBinRequest(authorization, appAuth, noteID)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = envelope.EnsureOK()
	return perr
}
