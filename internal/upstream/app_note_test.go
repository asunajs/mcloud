package upstream

// 云笔记那一族三口的**协议层**用例（玩法 107 的前置）。
//
// # 这一族钉什么、不钉什么
//
// 三口**零 REST 实拍**（见 `app_note.go` 文件头那一节：那 14 份 har 按四个词逐份扫过，
// 一条 `noteServer/*` 都没有；`docs/client-profiles.md` §3 那一行是画像统计、
// 不是完整往返）。所以这里**没有**一条断言在钉"上游真的接受这些键 / 真的回 `app_auth`"。
//
// 更特殊的一层：**旧 TS 那一条 runner 从没跑到过后两枪**（它读 ofetch 响应对象的
// `.headers`，而 ofetch 直接返回解析后的响应体 ⇒ 判据恒不成立）。于是本族"照抄谁"
// 是有对象的：**在跑的** Go 参考实现（caiyun `caiyun_note.go`）。下面的形状断言
// 钉的是那一家 + 两家一致的部分，不是"验过上游认"。
//
// 全部用例钉四类**可控**的事：
//
//  1. **字面量别在翻译中丢失** —— `CP_VERSION` 在两处是**两个不同值**（`3.2.0` 与
//     `7.7.1.20240115`，因为两处 `APP_CP` 也不同端）、`x-huawei-channelsrc` 的
//     `10001400`（与另外两族的 `10000034` / `10000020` 不是一枚）、
//     UA 就是 `mobile` 那五个字母。"顺手统一"任何一处都静默改掉一次接口。
//  2. **两口的头集合不对称是事实，不是笔误** —— `createNote` 带 `APP_NUMBER`、
//     `moveToRecycleBin` 不带（两家一致）。把两边统一等于发一份上游没收过的请求。
//  3. **只往下游透传 `app_auth` 一枚** —— 这是本层与旧 TS 那一句 `...headers`
//     的**唯一**形状差别（而旧 TS 从没跑到过那一行）。多带 `set-cookie` /
//     `content-length` 这类传输层头没有任何实现验证过。
//  4. **鉴权那一口不看 `code`、写那两口看** —— 上一口的判据是"响应头有没有 `app_auth`"
//     （两家同句）；后两口非 0 码 / `success:false` 都回错误（旧 TS 在那里是静默的，
//     本层不迁那份静默）。
//
// # 凭据边界
//
// `app_auth` 是**会话凭据**：断言只用合成哨兵串，且逐条钉"错误文本里不 echo 它"，
// 也不许出现 `authToken` 与手机号（与 `docs/sanitize-guide.md` 同一条纪律）。
//
// # 反向改坏这一族时的红法（逐条对应用例）
//
// 把两枚 `CP_VERSION` 统一成一个 ⇒ 第 1 类红；给删除那一口补 `APP_NUMBER` ⇒
// 第 2 类红；把鉴权响应整坨头透传下去 ⇒ 第 3 类红；给鉴权那一口加 `code` 判据 ⇒
// "code 非 0 而 `app_auth` 在仍算拿到"那条红；把 `strings.EqualFold` 换成精确匹配 ⇒
// Go 规范化后的 `App_auth` 读不到，那条红。

import (
	"context"
	"encoding/json"
	"sort"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	noteSynthPhone   = "19900000000"
	noteSynthToken   = "SYNTH-AUTHTOKEN"
	noteSynthAppAuth = "SYNTH-APP-AUTH"
	noteSynthAuth    = "Basic U1lOVEhFVElDLUFVVEg="
	noteSynthNoteID  = "synth-note-id"
	noteSynthTitle   = "abc"
	// noteSynthMillis 是一个 13 位毫秒数（合成，与任何真实时刻无关）。
	noteSynthMillis int64 = 1790000000000
)

func noteHeader(request core.HTTPRequest, name string) (string, bool) {
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1], true
		}
	}
	return "", false
}

func noteHeaderNames(request core.HTTPRequest) []string {
	names := make([]string, 0, len(request.Headers))
	for _, pair := range request.Headers {
		names = append(names, strings.ToLower(pair[0]))
	}
	sort.Strings(names)
	return names
}

func noteBodyObject(t *testing.T, body []byte) map[string]any {
	t.Helper()
	var obj map[string]any
	if err := json.Unmarshal(body, &obj); err != nil {
		t.Fatalf("请求体应是 JSON 对象: %v", err)
	}
	return obj
}

func noteKeys(obj map[string]any) []string {
	keys := make([]string, 0, len(obj))
	for key := range obj {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	return keys
}

// ─────────────────────────────────────────────────────────────────────────────
// 第一口：换 `app_auth`
// ─────────────────────────────────────────────────────────────────────────────

// TestNoteAuthTokenRequestShape 钉鉴权那一发的 URL、体两键与五枚头。
func TestNoteAuthTokenRequestShape(t *testing.T) {
	request, perr := NoteAuthTokenRequest(noteSynthAuth, noteSynthToken, noteSynthPhone)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if request.Method != core.MethodPost {
		t.Errorf("方法应是 POST，实得 %q", request.Method)
	}
	if request.URL != BaseNoteMcloud+notePathAuthTokenRefresh {
		t.Errorf("URL 实得 %q", request.URL)
	}
	body := noteBodyObject(t, request.Body)
	if got := strings.Join(noteKeys(body), ","); got != "authToken,userPhone" {
		t.Errorf("体里应只有那两键，实得 %s", got)
	}
	if body["authToken"] != noteSynthToken || body["userPhone"] != noteSynthPhone {
		t.Errorf("两键的值被改动过: %+v", body)
	}
	for name, want := range map[string]string{
		"APP_CP":              "android",
		"APP_NUMBER":          noteSynthPhone,
		"CP_VERSION":          noteAuthCPVersion,
		"x-huawei-channelsrc": noteAuthChannelSrc,
		"User-Agent":          noteMobileUserAgent,
	} {
		got, has := noteHeader(request, name)
		if !has {
			t.Errorf("缺头 %s（这一族的头名与面值两家逐字相同，少一枚就是换了一份画像）", name)
			continue
		}
		if got != want {
			t.Errorf("头 %s 实得 %q，期望 %q", name, got, want)
		}
	}
	if got, _ := noteHeader(request, "User-Agent"); got != "mobile" {
		t.Errorf("UA 的字面量就是 `mobile`，改成正常 UA 等于发一份上游没收过的画像，实得 %q", got)
	}
}

// TestNoteAuthTokenRequestAuthorizationIsOptional 钉"空串 ⇒ 不带那一枚，也不报错"。
//
// 这一口要不要 `authorization` **零样本**：旧 TS 的主实例按域名注入，而"这一口没有它
// 认不认"没有证据 ⇒ 由调用方决定，本层不猜也不拦。
func TestNoteAuthTokenRequestAuthorizationIsOptional(t *testing.T) {
	without, perr := NoteAuthTokenRequest("", noteSynthToken, noteSynthPhone)
	if perr != nil {
		t.Fatalf("空 authorization 不该报错，实得 %v", perr.Message())
	}
	if _, has := noteHeader(without, "authorization"); has {
		t.Error("空串时不该带 authorization 头")
	}
	with, perr := NoteAuthTokenRequest(noteSynthAuth, noteSynthToken, noteSynthPhone)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if got, _ := noteHeader(with, "authorization"); got != noteSynthAuth {
		t.Errorf("非空时应原样带上，实得 %q", got)
	}
}

// TestNoteAuthTokenRequestRejectsMissingCredentials 钉两枚缺任一都不发（一枪不发）。
func TestNoteAuthTokenRequestRejectsMissingCredentials(t *testing.T) {
	if _, perr := NoteAuthTokenRequest(noteSynthAuth, "", noteSynthPhone); perr == nil {
		t.Error("缺 authToken 应是错误")
	}
	if _, perr := NoteAuthTokenRequest(noteSynthAuth, noteSynthToken, ""); perr == nil {
		t.Error("缺手机号应是错误")
	}
}

// TestParseNoteAppAuthReadsHeaderCaseInsensitively 钉头名比对**必须**大小写不敏感。
//
// 这不是防御性代码：Go 的 `net/http` 会把 `app_auth` 规范化成 `App_auth`，
// 而本层手里的是自己摊平的 [core.HeaderPair] 列表（没有 `Header.Get` 那层规范化）。
// 反向改坏：把 `strings.EqualFold` 换成 `== "app_auth"` ⇒ 这一条红。
func TestParseNoteAppAuthReadsHeaderCaseInsensitively(t *testing.T) {
	for _, name := range []string{"app_auth", "App_auth", "APP_AUTH", "App_Auth"} {
		got, perr := ParseNoteAppAuth(core.HTTPResponse{
			Status: 200, Headers: []core.HeaderPair{{name, noteSynthAppAuth}},
		})
		if perr != nil {
			t.Fatalf("头名 %q 应能读到: %v", name, perr.Message())
		}
		if got.Value() != noteSynthAppAuth {
			t.Errorf("头名 %q 值实得 %q", name, got.Value())
		}
	}
}

// TestParseNoteAppAuthDoesNotLookAtCode 钉这一口的判据**只有**"头有没有"。
//
// 两家参考实现的判据是同一句，谁都不看响应体。给这一口补一条 `code` 判据，
// 等于让本仓在一家**在跑的**实现之外单独改判（R27 同族）。
func TestParseNoteAppAuthDoesNotLookAtCode(t *testing.T) {
	got, perr := ParseNoteAppAuth(core.HTTPResponse{
		Status:  200,
		Headers: []core.HeaderPair{{"App_auth", noteSynthAppAuth}},
		Body:    []byte(`{"code":"9999","message":"SYNTH"}`),
	})
	if perr != nil {
		t.Fatalf("code 非 0 而 app_auth 在 ⇒ 仍按两家原样继续，实得 %v", perr.Message())
	}
	if got.Value() != noteSynthAppAuth {
		t.Errorf("值实得 %q", got.Value())
	}
}

// TestParseNoteAppAuthFailure 钉"没给"的三档与**空白值**也算没给。
func TestParseNoteAppAuthFailure(t *testing.T) {
	for _, value := range []string{"", "   "} {
		got, perr := ParseNoteAppAuth(core.HTTPResponse{
			Status: 200, Headers: []core.HeaderPair{{"app_auth", value}},
		})
		if perr == nil {
			t.Errorf("空白值 %q 应判没给，实得 %q", value, got.Value())
		}
	}
	if _, perr := ParseNoteAppAuth(core.HTTPResponse{Status: 200}); perr == nil {
		t.Error("一条头都没有应是错误")
	}
}

// TestParseNoteAppAuthFailureCarriesUpstreamWords 钉失败文本带上 `code` / `message`。
//
// 两家在这里都只报一句"未获取到 app_auth"，线上真出问题时无从下手 ——
// 这是本层**只加原因、不改判定**的那一格。
func TestParseNoteAppAuthFailureCarriesUpstreamWords(t *testing.T) {
	_, perr := ParseNoteAppAuth(core.HTTPResponse{
		Status: 200, Body: []byte(`{"code":"90001","message":"token invalid"}`),
	})
	if perr == nil {
		t.Fatal("应是错误")
	}
	message := perr.Message()
	if !strings.Contains(message, "90001") || !strings.Contains(message, "token invalid") {
		t.Errorf("失败文本应带上游那两格读数，实得 %q", message)
	}
}

// TestParseNoteAppAuthFailureToleratesNonJSON 钉"响应体不是 JSON"不掩盖真正的问题。
func TestParseNoteAppAuthFailureToleratesNonJSON(t *testing.T) {
	_, perr := ParseNoteAppAuth(core.HTTPResponse{Status: 200, Body: []byte("<html>")})
	if perr == nil {
		t.Fatal("应是错误")
	}
	if !strings.Contains(perr.Message(), NoteAppAuthHeader) {
		t.Errorf("那句错误应只说它自己知道的，实得 %q", perr.Message())
	}
	if strings.Contains(perr.Message(), "<html>") {
		t.Errorf("不该把响应体整块塞进错误文本，实得 %q", perr.Message())
	}
}

// noteAuthClient 是"回一批固定响应头"的假客户端（鉴权那一口要的是头，不是体）。
type noteAuthClient struct {
	headers  []core.HeaderPair
	body     []byte
	sent     []core.HTTPRequest
	response core.HTTPResponse
}

func (c *noteAuthClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.sent = append(c.sent, request)
	response := c.response
	if response.Headers == nil {
		response.Headers = c.headers
	}
	if len(response.Body) == 0 {
		response.Body = c.body
	}
	if response.Status == 0 {
		response.Status = 200
	}
	return response, nil
}

// TestAppNoteAuthTokenGoesThroughRaw 钉这一口**不**被信封解析拦在前面。
//
// 走 [Upstream.Raw] 的理由写在 `app_note.go`：这一口要的是响应头，而 `Call` 一定先
// 把体喂给 `ParseEnvelope` ⇒ 上游回空体时会得到一句"响应不是 JSON"，掩盖真正的原因
// （凭据不对，上游根本不给 `app_auth`）。
// 反向改坏：把实现换成 `Call` ⇒ "空体也能拿到 app_auth"那一条红。
func TestAppNoteAuthTokenGoesThroughRaw(t *testing.T) {
	client := &noteAuthClient{headers: []core.HeaderPair{{"App_auth", noteSynthAppAuth}}}
	got, perr := NewUpstream(client).AppNoteAuthToken(
		context.Background(), noteSynthAuth, noteSynthToken, noteSynthPhone)
	if perr != nil {
		t.Fatalf("应成功: %v", perr.Message())
	}
	if got.Value() != noteSynthAppAuth {
		t.Errorf("实得 %q", got.Value())
	}
	if len(client.sent) != 1 {
		t.Fatalf("应恰好一发，实得 %d", len(client.sent))
	}
}

func TestAppNoteAuthTokenEmptyBodyStillReadsHeader(t *testing.T) {
	client := &noteAuthClient{headers: []core.HeaderPair{{"app_auth", noteSynthAppAuth}}}
	if _, perr := NewUpstream(client).AppNoteAuthToken(
		context.Background(), noteSynthAuth, noteSynthToken, noteSynthPhone); perr != nil {
		t.Fatalf("空响应体不该变成\"响应不是 JSON\"，实得 %v", perr.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第二口：createNote
// ─────────────────────────────────────────────────────────────────────────────

func noteCreateInput() NoteCreateInput {
	return NoteCreateInput{NoteID: noteSynthNoteID, Title: noteSynthTitle, Phone: noteSynthPhone}
}

func mustNoteCreate(t *testing.T, millis int64) core.HTTPRequest {
	t.Helper()
	request, perr := NoteCreateRequest(noteSynthAuth, core.NewSecretValue(noteSynthAppAuth),
		noteCreateInput(), millis)
	if perr != nil {
		t.Fatalf("构造 createNote 失败: %v", perr.Message())
	}
	return request
}

// TestNoteCreateRequestBodyShape 钉那二十六键**逐字**成文、类型不"顺手统一"。
//
// 这一条是本族最值钱的一条：体里空串与零值**不是**"没填"，是这一族的默认笔记形状。
func TestNoteCreateRequestBodyShape(t *testing.T) {
	body := noteBodyObject(t, mustNoteCreate(t, noteSynthMillis).Body)
	want := []string{
		"archived", "attachmentdir", "attachmentdirid", "attachments", "contentid",
		"contents", "cp", "createtime", "description", "expands", "landMark", "latlng",
		"location", "noteid", "remindtime", "remindtype", "revision", "system", "tags",
		"title", "topmost", "updatetime", "userphone", "version", "visitTime",
	}
	if got := strings.Join(noteKeys(body), ","); got != strings.Join(want, ",") {
		t.Errorf("键集合变了（少一格就是发一份上游没收过的体）\n实得 %s\n期望 %s", got, strings.Join(want, ","))
	}
	// 数字与字符串混用是原样：`revision` 是串、`remindtype` 是数、`topmost` 是串。
	for key, wantValue := range map[string]any{
		"archived":   float64(0),
		"remindtype": float64(0),
		"revision":   "1",
		"topmost":    "0",
		"noteid":     noteSynthNoteID,
		"title":      noteSynthTitle,
		"userphone":  noteSynthPhone,
	} {
		if got := body[key]; got != wantValue {
			t.Errorf("键 %s 实得 %#v，期望 %#v（类型也是线格式的一部分）", key, got, wantValue)
		}
	}
	// `tags` 发 `[]` 而不是 `null`：caiyun 传 nil 时 Go 会 marshal 成 `null`，
	// 而"上游把 null 当空数组"是一格未验证的宽容 ⇒ 取两家一致的那一侧。
	if list, ok := body["tags"].([]any); !ok || len(list) != 0 {
		t.Errorf("tags 应是空数组（不是 null），实得 %#v", body["tags"])
	}
	for _, key := range []string{"attachments", "landMark"} {
		if list, ok := body[key].([]any); !ok || len(list) != 0 {
			t.Errorf("%s 应是空数组，实得 %#v", key, body[key])
		}
	}
	expands, ok := body["expands"].(map[string]any)
	if !ok || expands["noteType"] != float64(0) {
		t.Errorf("expands 应是 {noteType:0}，实得 %#v", body["expands"])
	}
	contents, ok := body["contents"].([]any)
	if !ok || len(contents) != 1 {
		t.Fatalf("contents 应是单元素数组，实得 %#v", body["contents"])
	}
	first, ok := contents[0].(map[string]any)
	if !ok {
		t.Fatalf("contents[0] 应是对象，实得 %#v", contents[0])
	}
	if got := strings.Join(noteKeys(first), ","); got != "data,noteId,sortOrder,type" {
		t.Errorf("contents[0] 的键实得 %s", got)
	}
	if first["data"] != "<span></span>" || first["type"] != "TEXT" ||
		first["noteId"] != noteSynthNoteID || first["sortOrder"] != float64(0) {
		t.Errorf("contents[0] 面值被改动过: %+v", first)
	}
}

// TestNoteCreateRequestUsesOneTimestampForThreeKeys 钉"一个毫秒值用三处"与它由调用方注入。
//
// 反向改坏：把 `nowMillis` 在实现里换成 `time.Now()` ⇒ 这一条会红（三格不再相等），
// 而那一改同时违反本层的纯函数约定（不许读时钟）。
func TestNoteCreateRequestUsesOneTimestampForThreeKeys(t *testing.T) {
	body := noteBodyObject(t, mustNoteCreate(t, noteSynthMillis).Body)
	want := "1790000000000"
	for _, key := range []string{"createtime", "updatetime", "visitTime"} {
		if got := body[key]; got != want {
			t.Errorf("键 %s 实得 %#v，期望与另外两格同一个串 %q", key, got, want)
		}
	}
}

// TestNoteCreateRequestRejectsZeroTimestamp 钉"没给时刻"是错误，而不是发一条 1970 年的笔记。
func TestNoteCreateRequestRejectsZeroTimestamp(t *testing.T) {
	for _, millis := range []int64{0, -1} {
		if _, perr := NoteCreateRequest(noteSynthAuth,
			core.NewSecretValue(noteSynthAppAuth), noteCreateInput(), millis); perr == nil {
			t.Errorf("nowMillis=%d 应是错误（三格时间戳没有值可填）", millis)
		}
	}
}

// TestNoteCreateRequestHeaders 钉 PC 档那三枚 + `app_auth`，且**只**透传这一枚。
func TestNoteCreateRequestHeaders(t *testing.T) {
	request := mustNoteCreate(t, noteSynthMillis)
	if request.URL != BaseNoteMnote+notePathCreate {
		t.Errorf("URL 实得 %q（写那两枪在 mnote 那台，不是 note.mcloud）", request.URL)
	}
	for name, want := range map[string]string{
		"APP_CP":          "pc",
		"APP_NUMBER":      noteSynthPhone,
		"CP_VERSION":      noteClientCPVersion,
		NoteAppAuthHeader: noteSynthAppAuth,
		"authorization":   noteSynthAuth,
	} {
		if got, _ := noteHeader(request, name); got != want {
			t.Errorf("头 %s 实得 %q，期望 %q", name, got, want)
		}
	}
	// ⚠ 两枚 `CP_VERSION` **不是一枚**（两端的客户端版本）：统一成一个就是换一份画像。
	if noteAuthCPVersion == noteClientCPVersion {
		t.Error("两枚 CP_VERSION 不该相等")
	}
}

// TestNoteCreateRequestNeedsAppAuth 钉缺会话凭据时不发（它建出来的东西将没人能删）。
func TestNoteCreateRequestNeedsAppAuth(t *testing.T) {
	if _, perr := NoteCreateRequest(noteSynthAuth, core.NewSecretValue(""),
		noteCreateInput(), noteSynthMillis); perr == nil {
		t.Error("缺 app_auth 应是错误")
	}
	if _, perr := NoteCreateRequest(noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), NoteCreateInput{Title: noteSynthTitle,
			Phone: noteSynthPhone}, noteSynthMillis); perr == nil {
		t.Error("缺 noteid 应是错误：后一枪要靠它删掉这一条")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第三口：moveToRecycleBin
// ─────────────────────────────────────────────────────────────────────────────

// TestNoteMoveToRecycleBinOmitsAppNumber 钉**两口头集合不对称**是事实、不是笔误。
//
// 这一口不带 `APP_NUMBER`（TS `apis/note.ts:73-88` 与 caiyun `DeleteNote` 两家一致）。
// 反向改坏：给两边统一头集合 ⇒ 这一条红。
func TestNoteMoveToRecycleBinOmitsAppNumber(t *testing.T) {
	request, perr := NoteMoveToRecycleBinRequest(noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteSynthNoteID)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	names := strings.Join(noteHeaderNames(request), ",")
	if strings.Contains(names, "app_number") {
		t.Errorf("这一口不该带 APP_NUMBER（两家一致），头集合 %s", names)
	}
	if names != "app_auth,app_cp,authorization,content-type,cp_version" {
		t.Errorf("头集合实得 %s", names)
	}
	body := noteBodyObject(t, request.Body)
	if got := strings.Join(noteKeys(body), ","); got != "noteids" {
		t.Errorf("体里应只有 noteids，实得 %s", got)
	}
	list, ok := body["noteids"].([]any)
	if !ok || len(list) != 1 {
		t.Fatalf("noteids 应是单元素数组，实得 %#v", body["noteids"])
	}
	entry, ok := list[0].(map[string]any)
	if !ok || entry["noteid"] != noteSynthNoteID {
		t.Errorf("noteids[0] 实得 %#v", list[0])
	}
}

func TestNoteMoveToRecycleBinNeedsNoteIDAndAppAuth(t *testing.T) {
	if _, perr := NoteMoveToRecycleBinRequest(noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), ""); perr == nil {
		t.Error("空 noteid 应是错误：没有要删的东西就不发这一枪")
	}
	if _, perr := NoteMoveToRecycleBinRequest(noteSynthAuth,
		core.NewSecretValue(""), noteSynthNoteID); perr == nil {
		t.Error("缺 app_auth 应是错误")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层：两枪是写 ⇒ 各自恰好一发，且**看**响应
// ─────────────────────────────────────────────────────────────────────────────

// noteWriteClient 记录每一发并回一个可配的信封。
type noteWriteClient struct {
	responses  [][]byte
	sent       []core.HTTPRequest
	index      int
	failStatus uint16
}

func (c *noteWriteClient) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.sent = append(c.sent, request)
	body := []byte(`{"code":"0"}`)
	if c.index < len(c.responses) {
		body = c.responses[c.index]
	}
	c.index++
	if c.failStatus != 0 {
		return core.HTTPResponse{Status: c.failStatus, Body: body}, nil
	}
	return core.HTTPResponse{Status: 200, Body: body}, nil
}

// TestAppNoteCreateSendsOneRequestAndHonoursCode 钉"发一发 + 看码"。
//
// ⚠ 与旧 TS 的差别就在**看码**那一半：它用 `tryRequest` 把返回值整个丢弃 ⇒
// 建失败也照发删除那一枪。这里建失败就停在"这一条没做成"。
func TestAppNoteCreateSendsOneRequestAndHonoursCode(t *testing.T) {
	client := &noteWriteClient{}
	if perr := NewUpstream(client).AppNoteCreate(context.Background(), noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteCreateInput(), noteSynthMillis); perr != nil {
		t.Fatalf("code=0 应成功: %v", perr.Message())
	}
	if len(client.sent) != 1 || !strings.HasSuffix(client.sent[0].URL, "/createNote.do") {
		t.Fatalf("应恰好一发 createNote，实得 %+v", client.sent)
	}

	failing := &noteWriteClient{responses: [][]byte{[]byte(`{"code":"1","message":"SYNTH"}`)}}
	if perr := NewUpstream(failing).AppNoteCreate(context.Background(), noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteCreateInput(), noteSynthMillis); perr == nil {
		t.Error("非 0 码应是错误（旧实现在这里静默）")
	}

	unsuccessful := &noteWriteClient{responses: [][]byte{[]byte(`{"success":false,"message":"SYNTH"}`)}}
	if perr := NewUpstream(unsuccessful).AppNoteCreate(context.Background(), noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteCreateInput(), noteSynthMillis); perr == nil {
		t.Error("success:false 应是错误")
	}
}

// TestAppNoteCreateNeverEchoesCredentials 钉构造期的错误文本不带凭据与号码。
func TestAppNoteCreateNeverEchoesCredentials(t *testing.T) {
	client := &noteWriteClient{}
	// 时钟没给 ⇒ 走那条"缺毫秒时间戳"的错误，它不该复述任何输入值。
	_, perr := NoteCreateRequest(noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteCreateInput(), 0)
	if perr == nil {
		t.Fatal("应是错误")
	}
	for _, secret := range []string{noteSynthAppAuth, noteSynthAuth, noteSynthPhone, noteSynthNoteID} {
		if strings.Contains(perr.Message(), secret) {
			t.Errorf("错误文本里出现了 %q", secret)
		}
	}
	_ = client
}

func TestAppNoteMoveToRecycleBinSendsOneRequestAndHonoursCode(t *testing.T) {
	client := &noteWriteClient{}
	if perr := NewUpstream(client).AppNoteMoveToRecycleBin(context.Background(), noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteSynthNoteID); perr != nil {
		t.Fatalf("应成功: %v", perr.Message())
	}
	if len(client.sent) != 1 || !strings.HasSuffix(client.sent[0].URL, "/moveToRecycleBin.do") {
		t.Fatalf("应恰好一发，实得 %+v", client.sent)
	}

	failing := &noteWriteClient{responses: [][]byte{[]byte(`{"code":"9999","message":"SYNTH"}`)}}
	if perr := NewUpstream(failing).AppNoteMoveToRecycleBin(context.Background(), noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteSynthNoteID); perr == nil {
		t.Error("非 0 码应是错误（旧实现在这里也是静默）")
	}
}

// TestNoteStatusErrorIsNotParsedAsSuccess 钉 404 那一档走状态码而不是"响应不是 JSON"。
//
// 与外链族同一条落实（`errorFromStatus` 旁边那句"404 必须显式分支"）。
func TestNoteStatusErrorIsNotParsedAsSuccess(t *testing.T) {
	client := &noteWriteClient{responses: [][]byte{[]byte("<html>not found</html>")}}
	client.failStatus = 404
	if perr := NewUpstream(client).AppNoteCreate(context.Background(), noteSynthAuth,
		core.NewSecretValue(noteSynthAppAuth), noteCreateInput(), noteSynthMillis); perr == nil {
		t.Fatal("404 应是错误")
	} else if !strings.Contains(perr.Message(), "404") {
		t.Errorf("错误里应有状态码那一格，实得 %q", perr.Message())
	}
}
