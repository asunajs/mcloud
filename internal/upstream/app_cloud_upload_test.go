package upstream

// 上传族四枪的**协议层**用例（#35）。
//
// # 这一族钉什么、不钉什么
//
// 四枪里只有 `file/create` 有实拍，而那两条实拍是**建目录**那一型
// （`type:"folder"`，`uploadId` / `partInfos` 都是 `null`）⇒ 它们能钉的只有
// ①URL、②信封形状（`code` 是字符串 `"0000"`、载荷在 `data`）。
// 所以这里**没有**任何一条断言在钉"上游真正接受哪些键、`exist` 到底指什么"：
// 那些格子只有"旧 TS 这么发过"这一个证据源。
//
// 全部用例钉的是三类**可控**的事：
//
//  1. **字面量别在翻译中丢失** —— `application/oct-stream`（少一个 `e`，在**请求体**里）
//     与 `application/octet-stream`（正常拼写，在 **PUT 的头**里）是两处不同的东西，
//     把它们"顺手统一"会静默改掉一次接口。
//  2. **同名头只能有一枚** —— [core.HTTPRequest.WithHeader] 是**追加**，
//     而旧 TS 是对象赋值（覆盖）。PC 档要覆盖 `User-Agent`、三档都要保证
//     `content-type` 恰好一枚 ⇒ 不摘就发不出去的东西会带着两号线。
//  3. **"没给"不能念成"给了零值"** —— `data` 缺失、`success` 那一格缺席、
//     预签名 URL 缺席，三格都得是可区分的错误，而不是"成功但空"。
//
// ⚠ 刻意**不钉**的：`exist:true` 之后该不该继续（那是编排层的判断，且零样本）、
// `uploadId` 为 null 时 complete 会怎样（未测试）、PUT 的响应形状（零样本）。
//
// # 凭据边界
//
// `authorization` / `deviceid` 用合成哨兵串；预签名 URL 一律用
// `https://synth-oss.example.invalid/…`（`.invalid` 是保留域，永远不会被解析）。

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	uploadAuthSentinel   = "Basic U1lOVEhFVElDLUFVVEg=" // base64("SYNTHETIC-AUTH")
	uploadDeviceSentinel = "DEVICEID-SENTINEL"
	uploadSynthPutURL    = "https://synth-oss.example.invalid/put/SYNTH-TOKEN"
	uploadSynthKDURL     = "https://personal-kd-njs.yun.139.com/hcy/put/SYNTH-TOKEN"
)

// uploadCreateInput 是一个合法的默认输入，用例只改自己关心的那一格。
func uploadCreateInput() CloudFileCreateInput {
	return CloudFileCreateInput{
		Name:        "asign-synth.png",
		Size:        64,
		ContentHash: strings.Repeat("a", 64),
	}
}

// mustCreateRequest 构造请求，失败即 fatal。
func mustCreateRequest(t *testing.T, in CloudFileCreateInput) core.HTTPRequest {
	t.Helper()
	request, perr := CloudFileCreateRequest(uploadAuthSentinel, uploadDeviceSentinel, in)
	if perr != nil {
		t.Fatalf("构造 file/create 失败: %v", perr.Message())
	}
	return request
}

// ─────────────────────────────────────────────────────────────────────────────
// 第一枪：create 的请求体
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudFileCreateBodyIsLegacyTenKeys ⭐ 十键齐全，且两处"看着像拼错"的字面量在位。
//
// 反向改坏它的样子：① 把 `CloudCreateContentType` 改成 `application/octet-stream`
// （"顺手修个拼写"）⇒ 这一条红；② 把 `fileRenameMode` 改成 `refuse`
// （那是**建目录**那一型的值）⇒ 这一条红；③ 少发 `parallelUpload` ⇒ 键集合断言红。
func TestCloudFileCreateBodyIsLegacyTenKeys(t *testing.T) {
	request := mustCreateRequest(t, uploadCreateInput())
	if request.URL != BasePersonalKD+"/hcy/file/create" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应为 POST，实得 %v", request.Method)
	}
	obj := bodyKeys(t, request.Body)
	wantKeys := []string{
		"contentHash", "contentHashAlgorithm", "contentType", "fileRenameMode", "name",
		"parallelUpload", "parentFileId", "partInfos", "size", "type",
	}
	if got := strings.Join(sortedKeys(obj), ","); got != strings.Join(wantKeys, ",") {
		t.Errorf("键集合实得 %s", got)
	}
	if obj["type"] != CloudFileTypeFile {
		t.Errorf("type 实得 %v", obj["type"])
	}
	if obj["fileRenameMode"] != CloudRenameModeAuto {
		t.Errorf("fileRenameMode 实得 %v", obj["fileRenameMode"])
	}
	if obj["contentHashAlgorithm"] != CloudHashAlgorithm {
		t.Errorf("contentHashAlgorithm 实得 %v", obj["contentHashAlgorithm"])
	}
	// ⚠ 这一枚在**请求体**里，拼写缺一个 `e`；PUT 那一枪的**头**里才是完整拼写。
	if obj["contentType"] != "application/oct-stream" {
		t.Errorf("体里的 contentType 应是旧 TS 那个少一个 e 的字面量，实得 %v", obj["contentType"])
	}
	if obj["contentType"] == CloudPutContentType {
		t.Error("两处的 content-type 不许统一：一处在体里（少 e），一处在 PUT 的头里")
	}
	if flag, isBool := obj["parallelUpload"].(bool); !isBool || flag {
		t.Errorf("parallelUpload 应是 false，实得 %v", obj["parallelUpload"])
	}
	// `parentFileId` 空 ⇒ 补 "/"（旧 TS `apis/file.ts:57`）。
	if obj["parentFileId"] != "/" {
		t.Errorf("缺 parentFileId 时应补根目录，实得 %v", obj["parentFileId"])
	}
	// 单分片：一项、partNumber 1、partSize 与 size 同一个值。
	parts, ok := obj["partInfos"].([]any)
	if !ok || len(parts) != 1 {
		t.Fatalf("partInfos 应是一项数组，实得 %v", obj["partInfos"])
	}
	part, _ := parts[0].(map[string]any)
	if part["partNumber"] != float64(1) || part["partSize"] != float64(64) {
		t.Errorf("分片描述实得 %v", part)
	}
	ctxField, isCtx := part["parallelHashCtx"].(map[string]any)
	if !isCtx || ctxField["partOffset"] != float64(0) {
		t.Errorf("parallelHashCtx.partOffset 实得 %v", part["parallelHashCtx"])
	}

	// 显式给了目录就**不**补根目录：见 [TestCloudFileCreateKeepsExplicitParentFolder]。
}

// TestCloudFileCreateKeepsExplicitParentFolder 钉"空 ⇒ 根、非空 ⇒ 原样"。
func TestCloudFileCreateKeepsExplicitParentFolder(t *testing.T) {
	in := uploadCreateInput()
	in.ParentFileID = "SYNTH-DIR"
	request := mustCreateRequest(t, in)
	var obj map[string]any
	if err := json.Unmarshal(request.Body, &obj); err != nil {
		t.Fatalf("体不是 JSON: %v", err)
	}
	if obj["parentFileId"] != "SYNTH-DIR" {
		t.Errorf("parentFileId 被改写了，实得 %v", obj["parentFileId"])
	}
}

// TestCloudFileCreateRefusesEmptyNameOrHash 钉"缺两格之一 ⇒ 不发"。
//
// 旧 TS 没有这一道闸（它总是先 sha256、先起名），所以这两格在实拍里从不为空。
// 在这里挡住是因为**返回错误比发一个空名请求更便宜**：后者会在用户网盘里
// 留下一个没有名字的文件位，而清理那一族靠 fileId 认它。
func TestCloudFileCreateRefusesEmptyNameOrHash(t *testing.T) {
	nameless := uploadCreateInput()
	nameless.Name = ""
	if _, perr := CloudFileCreateRequest(uploadAuthSentinel, uploadDeviceSentinel, nameless); perr == nil {
		t.Error("空 name 应当拒发")
	}
	hashless := uploadCreateInput()
	hashless.ContentHash = ""
	if _, perr := CloudFileCreateRequest(uploadAuthSentinel, uploadDeviceSentinel, hashless); perr == nil {
		t.Error("空 contentHash 应当拒发")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 三档头集合
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudFileOpHeadersPerChannel ⭐ 三档各自的头面，且每档都只有该有那些。
//
// 反向改坏它的样子：把 android 档的 `x-yun-sub-op-type` 从 `op*100` 改成固定 `100`
// ⇒ backup 那一条红；把 PC 档的 `User-Agent` 忘了摘 ⇒ "恰好一枚" 红。
func TestCloudFileOpHeadersPerChannel(t *testing.T) {
	type want map[string]string

	cases := []struct {
		name    string
		channel CloudChannel
		op      CloudOpType
		must    want
		mustNot []string
	}{
		{
			name: "android/upload", channel: CloudChannelAndroid, op: CloudOpUpload,
			must: want{
				"x-yun-api-version":   "v1",
				"x-yun-app-channel":   "10000023",
				"x-huawei-channelSrc": "10000023",
				"x-yun-op-type":       "1",
				"x-yun-sub-op-type":   "100",
				"x-yun-client-info":   cloudClientInfoAndroid,
				"User-Agent":          CaiyunAndroidWebUA,
			},
			// android 档**没有** PC 那四枚，也没有 web 的 device-id。
			mustNot: []string{"x-yun-market-source", "x-yun-module-type", "x-yun-svc-type", "x-yun-device-id"},
		},
		{
			name: "android/backup", channel: CloudChannelAndroid, op: CloudOpBackup,
			must: want{"x-yun-op-type": "2", "x-yun-sub-op-type": "200"},
		},
		{
			name: "pc", channel: CloudChannelPC, op: CloudOpBackup,
			must: want{
				"x-yun-app-channel":   "10200153",
				"x-yun-op-type":       "1",
				"x-yun-market-source": "001",
				"x-yun-module-type":   "100",
				"x-yun-svc-type":      "1",
				"x-yun-client-info":   cloudClientInfoPC,
				"User-Agent":          "Mozilla/5.0",
			},
			// ⚠ PC 档**不读** opType ⇒ backup 与 upload 的头**一模一样**（这是从旧代码
			// 形状读出来的，不是实测）；sub-op-type 也不属于这一档。
			mustNot: []string{"x-yun-sub-op-type", "x-huawei-channelSrc"},
		},
		{
			name: "web", channel: CloudChannelWeb, op: CloudOpUpload,
			must: want{
				"x-yun-app-channel":   "10000034",
				"x-huawei-channelSrc": "10000034",
				"x-yun-client-info":   cloudClientInfoWeb,
				// 旧 TS 把设备串塞进了 device-id（`apis/file.ts:41`）：照抄，别"修对"。
				"x-yun-device-id": cloudClientInfoWeb,
				"User-Agent":      CaiyunAndroidWebUA,
			},
			mustNot: []string{"x-yun-sub-op-type", "x-yun-market-source"},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			request := CloudFileOpHeaders(
				core.NewRequestPost("https://example.invalid/x"),
				uploadAuthSentinel, uploadDeviceSentinel, tc.channel, tc.op)
			for name, value := range tc.must {
				if got := headerValue(request.Headers, name); got != value {
					t.Errorf("%s 实得 %q，期望 %q", name, got, value)
				}
			}
			for _, name := range tc.mustNot {
				if countHeader(request.Headers, name) != 0 {
					t.Errorf("%s 不该出现在这一档里", name)
				}
			}
			// **任何**同名头都不许出现两枚：[core.HTTPRequest.WithHeader] 是追加，
			// 而旧 TS 的 `{...base, k:v}` 是覆盖 ⇒ 翻译时漏掉"先摘再设"的那一处，
			// 就会变成两根线（上游读哪一根未测试）。
			//
			// ⚠ 本文件一律用 [countHeader]（大小写不敏感）而不是包里的 `hasHeader`
			// —— 后者只比字面量，而这一族的头名**两种拼法都有**（`Content-Type` /
			// `User-Agent` / `Authorization` 来自 [AiAvatarHeaders]，其余是小写）。
			// 用错那个 helper 会让"这枚头不存在"的断言**永远通过**。
			seen := map[string]int{}
			for _, h := range request.Headers {
				seen[strings.ToLower(h[0])]++
			}
			for name, n := range seen {
				if n > 1 {
					t.Errorf("%s 发出 %d 枚（应为覆盖语义的 1 枚）", name, n)
				}
			}
			for _, name := range []string{"user-agent", "content-type", "authorization", "deviceid"} {
				if n := countHeader(request.Headers, name); n != 1 {
					t.Errorf("%s 应恰好一枚，实得 %d", name, n)
				}
			}
			// 这一族的 MIME 由主实例那枚 charset 版决定（`getFileOpHeader` 不发 content-type）。
			if got := headerValue(request.Headers, "content-type"); got != H5ContentType {
				t.Errorf("content-type 实得 %q，期望主实例那一枚 %q", got, H5ContentType)
			}
		})
	}
}

// TestCloudFileOpHeadersCarryNoJWTToken ⭐ 这台主机从来收不到 jwttoken（不是漏写）。
//
// 反向改坏它的样子：图省事改用 [WithH5Headers]（签到那一套）⇒ 这一条红，
// 而且会同时把 `activityid` / 签到页 `referer` 渗进来 —— 那正是这一族**从不发**的头。
func TestCloudFileOpHeadersCarryNoJWTToken(t *testing.T) {
	request := CloudFileOpHeaders(
		core.NewRequestPost("https://example.invalid/x"),
		uploadAuthSentinel, uploadDeviceSentinel, CloudChannelAndroid, CloudOpUpload)
	for _, name := range []string{"jwttoken", "activityid", "referer", "appversion", "showloading"} {
		if countHeader(request.Headers, name) != 0 {
			t.Errorf("%s 不该由这一族发出（主 http 实例只对三台特定主机带 jwttoken，且从不带签到那几枚）", name)
		}
	}
	// 两个开关各自独立：空串 ⇒ 不发那一枚，另一枚照发。
	anonymous := CloudFileOpHeaders(
		core.NewRequestPost("https://example.invalid/x"), "", "", CloudChannelAndroid, CloudOpUpload)
	for _, name := range []string{"authorization", "deviceid"} {
		if countHeader(anonymous.Headers, name) != 0 {
			t.Errorf("空值应当是\"不发那一枚\"，%s 却出现了", name)
		}
	}
	if countHeader(anonymous.Headers, "content-type") != 1 {
		t.Error("content-type 是恒发的那一枚，空值不该影响它")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第二枪：PUT（预签名直传）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudUploadPutRequestShape 钉方法、三枚头、裸字节 body 与"不重试"。
//
// 反向改坏它的样子：给 PUT 补一个幂等键（"写请求都该可重试"）⇒ `IsRetryable` 那条红 ——
// 而那条红是有意义的：重放 PUT 的语义零样本，代价是用户网盘里多一段坏文件。
func TestCloudUploadPutRequestShape(t *testing.T) {
	content := []byte("0123456789abcdef0123456789abcdef")
	request := CloudUploadPutRequest(uploadSynthPutURL, content, uploadAuthSentinel, uploadDeviceSentinel)
	if request.Method != core.MethodPut {
		t.Errorf("应是 PUT，实得 %v", request.Method)
	}
	if string(request.Body) != string(content) {
		t.Error("body 必须是原始字节，不能被 JSON 化")
	}
	if got := headerValue(request.Headers, "content-type"); got != CloudPutContentType {
		t.Errorf("PUT 的头里应是完整拼写，实得 %q", got)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一枚，实得 %d", n)
	}
	if got := headerValue(request.Headers, "origin"); got != "https://yun.139.com" {
		t.Errorf("origin 实得 %q", got)
	}
	if got := headerValue(request.Headers, "referer"); got != "https://yun.139.com/" {
		t.Errorf("referer 实得 %q（旧 TS 带结尾斜杠）", got)
	}
	// 这一枪**不**带 hcy 那三档文件操作头（旧 TS 的 putFile 只发那三枚 + 实例静态头）。
	for _, name := range []string{"x-yun-app-channel", "x-yun-op-type", "x-yun-client-info"} {
		if countHeader(request.Headers, name) != 0 {
			t.Errorf("%s 不该出现在 PUT 那一枪", name)
		}
	}
	if request.IsRetryable() {
		t.Error("PUT 不带幂等键 ⇒ 不该可重试")
	}
	if countHeader(request.Headers, "x-yun-api-version") != 0 {
		t.Error("PUT 那一枪不套 getFileOpHeader")
	}
}

// TestCloudUploadPutSendsAuthorizationOnlyToKnownHosts ⭐ 凭据边界：按主机后缀决定带不带。
//
// 四段后缀来自旧 TS `base.ts:10`，**规则**有字面出处；这一枪真正会发到哪台主机
// **零样本**（预签名 URL 从没被抓到过），所以这里钉的是"规则的两侧各自怎么走"，
// 不是"上游要哪一侧"。
func TestCloudUploadPutSendsAuthorizationOnlyToKnownHosts(t *testing.T) {
	content := []byte("SYNTH")
	cases := []struct {
		name          string
		url           string
		authorization string
		wantAuth      bool
	}{
		{"外站预签名 URL 不带凭据", uploadSynthPutURL, uploadAuthSentinel, false},
		{"命中 yun.139.com 后缀", uploadSynthKDURL, uploadAuthSentinel, true},
		{"给了空凭据就不带", uploadSynthKDURL, "", false},
		{"坏 URL 一律不带（宁可缺凭据，不可错发）", "::not a url::", uploadAuthSentinel, false},
		{"相对路径没有主机 ⇒ 不带", "/hcy/put/SYNTH", uploadAuthSentinel, false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			request := CloudUploadPutRequest(tc.url, content, tc.authorization, uploadDeviceSentinel)
			present := countHeader(request.Headers, "Authorization") == 1
			if present != tc.wantAuth {
				t.Errorf("Authorization 在不在：实得 %v，期望 %v", present, tc.wantAuth)
			}
			if present && headerValue(request.Headers, "Authorization") != uploadAuthSentinel {
				t.Error("Authorization 的值被改写了")
			}
		})
	}
}

// TestAppCloudUploadPutSendsNothingOnSuccess 钉发送层走 Raw：不解析信封、不看 body。
//
// 这一条的用意是"预签名响应哪怕不是 JSON 也算成功"。反向改坏：把发送层换成
// [Upstream.Call] ⇒ 这一条红（因为 body 是空串，`ParseEnvelope` 会报"不是 JSON"）。
func TestAppCloudUploadPutSendsNothingOnSuccess(t *testing.T) {
	client := &scriptedClient{response: core.HTTPResponse{Status: 200}}
	up := NewUpstream(client)
	if perr := up.AppCloudUploadPut(
		context.Background(), uploadSynthPutURL, []byte("SYNTH"), uploadAuthSentinel, uploadDeviceSentinel,
	); perr != nil {
		t.Fatalf("2xx 应当算成功，实得 %v", perr.Message())
	}
	if len(client.requests) != 1 || client.requests[0].Method != core.MethodPut {
		t.Fatalf("应恰好一发 PUT，实得 %+v", client.requests)
	}
	// 非 2xx ⇒ 错误，而且**错误文本里没有那个 URL**（它是带签名的写入入口）。
	bad := &scriptedClient{response: core.HTTPResponse{Status: 403}}
	perr := NewUpstream(bad).AppCloudUploadPut(
		context.Background(), uploadSynthPutURL, []byte("SYNTH"), "", "")
	if perr == nil {
		t.Fatal("403 应当是错误")
	}
	if strings.Contains(perr.Message(), "SYNTH-TOKEN") {
		t.Error("错误文本里出现了预签名 URL 的那一段 ⇒ 凭据边界破了")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析：create / complete / batchTrash
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudFileCreateReadsDataPayload ⭐ 载荷在 `data`、`code` 是字符串 `"0000"`。
//
// 夹具是**合成**的（形状取自 3d.json 那两条 folder 型的字段集合，值一律 SYNTH-*）：
// 实拍那两条的 `fileId` 是 `<REDACTED>`，不许复制进仓库。
func TestParseCloudFileCreateReadsDataPayload(t *testing.T) {
	envelope := envelopeOf(t, `{"code":"0000","message":"请求成功","success":true,
		"data":{"exist":false,"fileId":"SYNTH-FID","fileName":"asign-synth.png","formInfo":null,
		"parentFileId":"/","partInfos":[{"uploadUrl":"https://synth-oss.example.invalid/put"}],
		"rapidUpload":false,"transferToken":null,"type":"file","uploadId":"SYNTH-UID"}}`)
	created, perr := ParseCloudFileCreate(envelope)
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if created.FileID == nil || *created.FileID != "SYNTH-FID" {
		t.Errorf("fileId 实得 %v", created.FileID)
	}
	if created.UploadID == nil || *created.UploadID != "SYNTH-UID" {
		t.Errorf("uploadId 实得 %v", created.UploadID)
	}
	if created.UploadURL == nil || *created.UploadURL != "https://synth-oss.example.invalid/put" {
		t.Errorf("uploadUrl 实得 %v", created.UploadURL)
	}
	if created.Exist || created.RapidUpload {
		t.Errorf("两个布尔位应为 false，实得 %+v", created)
	}
}

// TestParseCloudFileCreateHandlesFolderShape 钉"实拍那两条的形状交回来是什么"。
//
// 那两条是**建目录**：`exist:true`、`uploadId:null`、`partInfos:null`。
// 这一条的价值在于"以后有人拿 file/create 复用来建目录"时，本层不会 panic，
// 也不会把 null 编造成空串。
func TestParseCloudFileCreateHandlesFolderShape(t *testing.T) {
	envelope := envelopeOf(t, `{"code":"0000","success":true,"message":"请求成功",
		"data":{"exist":true,"fileId":"SYNTH-DIR","formInfo":null,"parentFileId":"/",
		"partInfos":null,"rapidUpload":false,"type":"folder","uploadId":null}}`)
	created, perr := ParseCloudFileCreate(envelope)
	if perr != nil {
		t.Fatalf("folder 型不该报错: %v", perr.Message())
	}
	if !created.Exist {
		t.Error("exist:true 该读出来")
	}
	if created.UploadID != nil || created.UploadURL != nil {
		t.Errorf("null 不该被编造成空串：%+v", created)
	}
}

// TestParseCloudFileCreateDistinguishesMissingFromZero 钉三格"没有"的处置。
func TestParseCloudFileCreateDistinguishesMissingFromZero(t *testing.T) {
	// `data` 整个缺失 ⇒ 解析错误（旧实现是 `createResult.data` 直接解构会抛）。
	if _, perr := ParseCloudFileCreate(envelopeOf(t, `{"code":"0000","success":true}`)); perr == nil {
		t.Error("没有 data 应当报错，而不是交回一个全空结构")
	}
	// `data` 不是对象。
	if _, perr := ParseCloudFileCreate(envelopeOf(t, `{"code":"0000","success":true,"data":[]}`)); perr == nil {
		t.Error("data 是数组应当报错")
	}
	// `partInfos` 是空数组 ⇒ UploadURL 为 nil，而不是空串。
	created, perr := ParseCloudFileCreate(
		envelopeOf(t, `{"code":"0000","success":true,"data":{"partInfos":[],"fileId":"SYNTH"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if created.UploadURL != nil {
		t.Errorf("空数组该给 nil，实得 %q", *created.UploadURL)
	}
	// 业务失败（success:false）优先于载荷。
	if _, perr := ParseCloudFileCreate(
		envelopeOf(t, `{"code":"0000","success":false,"message":"容量不足","data":{"fileId":"SYNTH"}}`)); perr == nil {
		t.Error("success:false 必须是错误")
	}
	// `exist` 是数字 ⇒ 读成 false（见 [cloudFlag]：形状变了要可断言，不当真值）。
	weird, perr := ParseCloudFileCreate(
		envelopeOf(t, `{"code":"0000","success":true,"data":{"exist":1}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if weird.Exist {
		t.Error("exist:1 不该被念成 true")
	}
}

// TestCloudCreateNonZeroCodeIsError 钉非 "0000" 的码走错误分支。
//
// ⚠ `"0000"` 与 `0` 都判成功是**这条链今天的行为**（[asInt64] 认字符串数字），
// 不是上游承诺：这一族的失败码长什么样零样本 ⇒ 不枚举。
func TestCloudCreateNonZeroCodeIsError(t *testing.T) {
	if _, perr := ParseCloudFileCreate(
		envelopeOf(t, `{"code":"1","success":true,"message":"失败"}`)); perr == nil {
		t.Error(`code:"1" 应是失败`)
	} else if !strings.Contains(perr.Message(), "1") {
		t.Errorf("错误里该带着那个码，实得 %q", perr.Message())
	}
}

// TestParseCloudFileCompleteOnlyJudgesEnvelope 钉"这一族不声明 complete 的字段"。
func TestParseCloudFileCompleteOnlyJudgesEnvelope(t *testing.T) {
	if perr := ParseCloudFileComplete(envelopeOf(t, `{"code":"0000","success":true}`)); perr != nil {
		t.Errorf("成功应回 nil，实得 %v", perr.Message())
	}
	if perr := ParseCloudFileComplete(
		envelopeOf(t, `{"code":"0000","success":false,"message":"校验失败"}`)); perr == nil {
		t.Error("success:false 应是错误")
	}
}

// TestParseCloudFileBatchTrashRequiresTopLevelSuccess ⭐ "没失败"不等于"成功了"。
//
// 旧实现读的是**顶层** `success`（那个 `result` 变量是整个响应对象，不是信封的 `result`
// 字段），读不到就当删除失败。反向改坏：把这里改成"EnsureOK 过了就算成功"⇒
// 缺 `success` 那一格的红会转成"删除成功"，而那意味着**用户网盘里留下删不掉的文件**。
func TestParseCloudFileBatchTrashRequiresTopLevelSuccess(t *testing.T) {
	if perr := ParseCloudFileBatchTrash(
		envelopeOf(t, `{"code":"0000","message":"请求成功","success":true,"result":{}}`)); perr != nil {
		t.Errorf("顶层 success:true 应算成功，实得 %v", perr.Message())
	}
	if perr := ParseCloudFileBatchTrash(envelopeOf(t, `{"code":"0000","success":true}`)); perr != nil {
		t.Errorf("没有 result 也应算成功（判据是顶层 success），实得 %v", perr.Message())
	}
	if perr := ParseCloudFileBatchTrash(envelopeOf(t, `{"code":"0000","result":{}}`)); perr == nil {
		t.Error("顶层没有 success 那一格 ⇒ 不该念成成功")
	}
	if perr := ParseCloudFileBatchTrash(
		envelopeOf(t, `{"code":"0000","success":false,"message":"回收站已满"}`)); perr == nil {
		t.Error("success:false 应是错误")
	}
}

// TestCloudBatchTrashRefusesEmptyListAndUsesWebChannel 钉两件事。
//
// ① 空列表不发（旧实现在调用侧挡：`after.ts:133`）；② 删除固定 web 档，
// 与上传那一枪用的档位**无关**（`apis/file.ts:114`）—— 也就是说旧实现里
// "PC 档上传、web 档删除"是常态，别"顺手统一"。
func TestCloudBatchTrashRefusesEmptyListAndUsesWebChannel(t *testing.T) {
	if _, perr := CloudBatchTrashRequest(uploadAuthSentinel, uploadDeviceSentinel, nil); perr == nil {
		t.Error("空列表不该发请求")
	}
	request, perr := CloudBatchTrashRequest(
		uploadAuthSentinel, uploadDeviceSentinel, []string{"SYNTH-A", "SYNTH-B"})
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if request.URL != BasePersonalKD+"/hcy/recyclebin/batchTrash" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	obj := bodyKeys(t, request.Body)
	ids, ok := obj["fileIds"].([]any)
	if !ok || len(ids) != 2 || ids[0] != "SYNTH-A" {
		t.Fatalf("fileIds 实得 %v", obj["fileIds"])
	}
	if len(obj) != 1 {
		t.Errorf("这一枪只发 fileIds 一键，实得键集合 %v", sortedKeys(obj))
	}
	if got := headerValue(request.Headers, "x-yun-app-channel"); got != string(CloudChannelWeb) {
		t.Errorf("删除应固定 web 档，实得 %q", got)
	}
	if got := headerValue(request.Headers, "x-yun-device-id"); got != cloudClientInfoWeb {
		t.Error("删除没走 web 档那一套头")
	}
}

// TestCloudFileCompleteRequestSendsFourKeys 钉 complete 的体与"必须同一枚 hash"。
func TestCloudFileCompleteRequestSendsFourKeys(t *testing.T) {
	hash := strings.Repeat("b", 64)
	request, perr := CloudFileCompleteRequest(
		uploadAuthSentinel, uploadDeviceSentinel, "SYNTH-FID", "SYNTH-UID", hash,
		CloudChannelAndroid, CloudOpUpload)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr.Message())
	}
	if request.URL != BasePersonalKD+"/hcy/file/complete" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	obj := bodyKeys(t, request.Body)
	if got := strings.Join(sortedKeys(obj), ","); got != "contentHash,contentHashAlgorithm,fileId,uploadId" {
		t.Errorf("键集合实得 %s", got)
	}
	if obj["contentHash"] != hash {
		t.Error("complete 的 contentHash 必须与 create 同一枚")
	}
	// 缺 fileId 或 hash ⇒ 不发（这两个是这一枪的全部依据）。
	if _, perr := CloudFileCompleteRequest(
		uploadAuthSentinel, uploadDeviceSentinel, "", "SYNTH-UID", hash, CloudChannelAndroid, CloudOpUpload); perr == nil {
		t.Error("空 fileId 应拒发")
	}
	// ⚠ `uploadId` 为空**照样发**：旧实现就是这么做的（它不判空）。这一格不钉成断言，
	// 只在测试里留一个反向用例说明"今天这样"，将来实拍到 null 的处置时再改。
	request, perr = CloudFileCompleteRequest(
		uploadAuthSentinel, uploadDeviceSentinel, "SYNTH-FID", "", hash, CloudChannelAndroid, CloudOpUpload)
	if perr != nil {
		t.Fatalf("空 uploadId 今天不该拒发: %v", perr.Message())
	}
	if obj := bodyKeys(t, request.Body); obj["uploadId"] != "" {
		t.Errorf("uploadId 应原样发出空串，实得 %v", obj["uploadId"])
	}
}

// TestChannelOrDefaultAndBackupFolding 钉两个默认值与 opType 的折法。
func TestChannelOrDefaultAndBackupFolding(t *testing.T) {
	in := uploadCreateInput()
	if got := in.channelOrDefault(); got != CloudChannelAndroid {
		t.Errorf("零值渠道应落到旧 TS 的默认档 android，实得 %q", got)
	}
	in.Channel = CloudChannelWeb
	if got := in.channelOrDefault(); got != CloudChannelWeb {
		t.Errorf("显式档位不该被覆盖，实得 %q", got)
	}
	if opTypeIsBackup(CloudOpUpload) || opTypeIsBackup(CloudOpType("")) {
		t.Error(`只有字面量 "backup" 才是备份档（空串在旧 TS 里就是 upload）`)
	}
	if !opTypeIsBackup(CloudOpBackup) {
		t.Error("backup 没被认出来")
	}
}

// TestCloudFileCreateRoundTripsThroughSendLayer 是这一族的**最小闭环**：
// 假客户端按实拍形状回 `data`，发送层把四格交回来。
//
// 它不证明上游接受这一套（四枪零真打），只证明"从构造到解析这一串接线是通的"——
// 也就是"能编译的实现"与"第一次真跑就会散架的实现"之间的那道区别。
func TestCloudFileCreateRoundTripsThroughSendLayer(t *testing.T) {
	client := &scriptedClient{response: core.HTTPResponse{
		Status: 200,
		Body: core.Body([]byte(
			`{"code":"0000","success":true,"message":"请求成功","data":{"exist":false,` +
				`"fileId":"SYNTH-FID","uploadId":"SYNTH-UID","rapidUpload":false,` +
				`"partInfos":[{"uploadUrl":"https://synth-oss.example.invalid/put"}]}}`)),
	}}
	created, perr := NewUpstream(client).AppCloudFileCreate(
		context.Background(), uploadAuthSentinel, uploadDeviceSentinel, uploadCreateInput())
	if perr != nil {
		t.Fatalf("闭环失败: %v", perr.Message())
	}
	if created.FileID == nil || created.UploadURL == nil {
		t.Fatalf("四格没交回来: %+v", created)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应恰好一发，实得 %d", len(client.requests))
	}
	// 错误路径：非 2xx ⇒ 错误，且不外泄响应体原文。
	bad := &scriptedClient{response: core.HTTPResponse{Status: 500, Body: core.Body([]byte(`{"code":"9"}`))}}
	if _, perr := NewUpstream(bad).AppCloudFileCreate(
		context.Background(), uploadAuthSentinel, "", uploadCreateInput()); perr == nil {
		t.Error("500 应是错误")
	}
}
