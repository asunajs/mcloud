package upstream

// AI 头像（#10）这一族的**协议层**用例：请求形状与解析。
//
// # 为什么这一族的用例比别的族更"薄"一点
//
// `rabactive/*` 六口**零实拍**（文件头那条"证据分两半"）⇒ 这里**没有**任何一条断言
// 在钉"上游真正接受什么"，全部在钉两件可控的事：
//
//  1. **形状别串到另一条链上去** —— `/market/` 前缀 vs `/ycloud/`、
//     `hcy/file/list` **有 authorization 无 jwttoken**（这两枚头的开关按主机分，
//     写错的那一格恰好是"看着像漏写"的那一格）、这一页从不发的 `activityid` / `referer`
//     不能从头集合里渗进来。
//  2. **"没给"不能念成"给了个 0 / 空"** —— `getHashLeft` 的 `null`、`upload` 的空串 id、
//     `file/list` 的 `items` 不是数组。这三格一旦伪造成"正常值"，
//     表现是链条继续往下走、最后**在网盘里留下一张没人知道的图**。
//
// ⚠ 反过来，这里**刻意不钉**的：`status` 的档位表（3/4 是从旧实现的分支形状反推的，
// 零样本）、`408` 的语义、`drawsUsed` 那类计数关系。给一个没验证过的判据写测试，
// 等于把它钉成"以后都不能改" —— 那正是 §1.7 要避免的写法。
//
// # 凭据边界
//
// 夹具里的 `authorization` / `jwttoken` / `deviceid` 全是**合成哨兵串**（名字里带
// SENTINEL），断言只看"这枚头在不在、值是不是我传进去的那一串"，不看内容。
// `file/list` 那份实拍里 `fileId` 有被脱敏的，也有 `/synbox` 这种公开目录名 ——
// 这里只取**未脱敏且公开**的两条当形状样本。

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	aiAuthSentinel   = "Basic U1lOVEhFVElDLUFVVEg=" // base64("SYNTHETIC-AUTH")
	aiJWTSentinel    = "JWT-SENTINEL"
	aiDeviceSentinel = "DEVICEID-SENTINEL"
)

// aiAvatarEnvelope 把一份 JSON 原文走 [ParseEnvelope]。
//
// ⚠ 必须走它而不是手搓 `map[string]any`：真实路径上数字是 [json.Number]，
// 手搓的那条会让 `asInt64` 的数字分支**永远测不到**（同 `app_funai_test.go` 那条教训）。
func aiAvatarEnvelope(t *testing.T, body string) *Envelope {
	t.Helper()
	envelope, perr := ParseEnvelope([]byte(body))
	if perr != nil {
		t.Fatalf("夹具不是合法 JSON: %v", perr)
	}
	return envelope
}

// bodyKeys 把请求体解成"键 → 值"，用于钉"只有这几个键"。
func bodyKeys(t *testing.T, body []byte) map[string]any {
	t.Helper()
	var obj map[string]any
	if err := json.Unmarshal(body, &obj); err != nil {
		t.Fatalf("请求体不是合法 JSON 对象: %v（实得 %s）", err, body)
	}
	return obj
}

func sortedKeys(obj map[string]any) []string {
	keys := make([]string, 0, len(obj))
	for k := range obj {
		keys = append(keys, k)
	}
	// 手写插入排序：本文件只为断言"键集合"服务，不想为它引 sort 的语义依赖。
	for i := 1; i < len(keys); i++ {
		for j := i; j > 0 && keys[j] < keys[j-1]; j-- {
			keys[j], keys[j-1] = keys[j-1], keys[j]
		}
	}
	return keys
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求形状：rabactive 六口
// ─────────────────────────────────────────────────────────────────────────────

// TestAiAvatarRabactivePrefixIsMarketNotYcloud ⭐ 六口的路径前缀必须是 `/market/rabactive/`。
//
// 反向改坏它的样子：把 `BaseRabactive` 里的 `/market/` 删掉（或者"顺手"跟签到族一样
// 用 `/ycloud/`）—— 拼出来的 URL 会回 404 页，而 404 页在 [ParseEnvelope] 那一层
// 表现为"响应体不是合法 JSON"，看着像**解析器**坏了，不像基址写错。
// 这一条就是为了让那种误读在测试里直接现形。
func TestAiAvatarRabactivePrefixIsMarketNotYcloud(t *testing.T) {
	merge, err := AiAvatarMergeRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "aGVsbG8=")
	if err != nil {
		t.Fatalf("merge 构造失败: %v", err)
	}
	status, err := AiAvatarStatusRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "T-1")
	if err != nil {
		t.Fatalf("getStatus 构造失败: %v", err)
	}
	upload, err := AiAvatarUploadRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "a", "b")
	if err != nil {
		t.Fatalf("upload 构造失败: %v", err)
	}
	cases := []struct {
		name    string
		request core.HTTPRequest
		wantURL string
		method  core.HTTPMethod
	}{
		{"getHashLeft", AiAvatarHashLeftRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel),
			"https://m.mcloud.139.com/market/rabactive/image/getHashLeft", core.MethodGet},
		{"draw", AiAvatarDrawRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel),
			"https://m.mcloud.139.com/market/rabactive/image/draw", core.MethodGet},
		{"merge", merge,
			"https://m.mcloud.139.com/market/rabactive/ai/image/merge", core.MethodPost},
		{"getStatus", status,
			"https://m.mcloud.139.com/market/rabactive/ai/image/getStatus", core.MethodPost},
		{"upload", upload,
			"https://m.mcloud.139.com/market/rabactive/image/upload", core.MethodPost},
		{"getWorkList", AiAvatarWorkListRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, 1),
			"https://m.mcloud.139.com/market/rabactive/image/getWorkList?type=1", core.MethodGet},
		{"catalogFullPath", AiAvatarCatalogPathRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, AiAvatarCatalogMarketName),
			"https://m.mcloud.139.com/market/rabactive/image/catalogFullPath?marketName=rabbityearpic", core.MethodGet},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if c.request.Method != c.method {
				t.Errorf("方法应是 %s，实得 %s", c.method, c.request.Method)
			}
			if c.request.URL != c.wantURL {
				t.Errorf("URL 应逐字为 %q，实得 %q", c.wantURL, c.request.URL)
			}
			if strings.Contains(c.request.URL, "/ycloud/") {
				t.Errorf("这一族的基址是 /market/ 不是 /ycloud/，实得 %q", c.request.URL)
			}
			if !strings.HasPrefix(c.request.URL, BaseRabactive+"/") {
				t.Errorf("应挂在 %s 下面，实得 %q", BaseRabactive, c.request.URL)
			}
		})
	}
}

// TestAiAvatarHeadersMatchLegacyMainInstance ⭐ 钉"第三套头集合"的四件事。
//
// 三条最容易写错、而且**都不会在报错上现形**的地方：
//   - `content-type` **恒发**（哪怕 GET 裸体）—— 旧 TS 把它放在静态头里，
//     与本仓 [WithH5Headers] 的"有 body 才补"**刻意不同**。照着 H5 那套抄 ⇒ GET 少一个头，
//     看着完全正常。
//   - POST 的 `content-type` 只有一枚 —— [core.HTTPRequest.WithHeader] 是**追加**，
//     不先 [dropHeader] 就会发出两枚，上游按第一枚解析（那是实测过的坑）。
//   - `jwttoken` 的头名是**小写**，与 H5 那套一致。
func TestAiAvatarHeadersMatchLegacyMainInstance(t *testing.T) {
	get := AiAvatarHashLeftRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel)
	if n := countHeader(get.Headers, "content-type"); n != 1 {
		t.Errorf("GET 裸体也要恰有一枚 content-type（恒发），实得 %d 枚", n)
	}
	if got := headerValue(get.Headers, "Content-Type"); got != H5ContentType {
		t.Errorf("content-type 应带 charset，逐字 %q，实得 %q", H5ContentType, got)
	}
	if got := headerOf(t, get.Headers, "User-Agent"); got != CaiyunAndroidWebUA {
		t.Errorf("user-agent 应是旧 TS 主实例那一串 Android WebView UA")
	}
	if got := headerOf(t, get.Headers, "deviceid"); got != aiDeviceSentinel {
		t.Errorf("deviceid 应逐字透传，实得 %q", got)
	}
	if got := headerOf(t, get.Headers, "Authorization"); got != aiAuthSentinel {
		t.Errorf("authorization 应逐字透传，实得 %q", got)
	}
	if got := headerOf(t, get.Headers, "jwttoken"); got != aiJWTSentinel {
		t.Errorf("jwttoken 应逐字透传，实得 %q", got)
	}

	merge, err := AiAvatarMergeRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "aGVsbG8=")
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if n := countHeader(merge.Headers, "content-type"); n != 1 {
		t.Errorf("POST 的 content-type 必须恰有一枚（先剥后补），实得 %d 枚", n)
	}
}

// TestAiAvatarHeadersOmitEachEmptySwitchSeparately 钉"三个空串各自独立"。
//
// 旧 TS 是 `if (basicToken)` / `if (jwtToken)` / `if (deviceId)` 三个**独立**开关
// （`base.ts` 的请求拦截器），少一个不会连带少另一个。合成一个"要不要发全套"的布尔
// 会让"只缺 jwttoken"这种真实现场（会话没建起来）在测试里看不出来。
func TestAiAvatarHeadersOmitEachEmptySwitchSeparately(t *testing.T) {
	none := AiAvatarHashLeftRequest("", "", "")
	for _, name := range []string{"Authorization", "jwttoken", "deviceid"} {
		if hasHeader(none.Headers, name) {
			t.Errorf("空串不应发 %s", name)
		}
	}
	if !hasHeader(none.Headers, "Content-Type") || !hasHeader(none.Headers, "User-Agent") {
		t.Error("静态那两枚与开关无关，必须仍在")
	}

	onlyJWT := AiAvatarHashLeftRequest("", aiJWTSentinel, "")
	if !hasHeader(onlyJWT.Headers, "jwttoken") {
		t.Error("只给 jwttoken 时它应在")
	}
	if hasHeader(onlyJWT.Headers, "Authorization") || hasHeader(onlyJWT.Headers, "deviceid") {
		t.Error("只给 jwttoken 时另外两枚不该被连带发出")
	}
}

// TestAiAvatarRequestsDoNotCarryH5OrAppHeaders 钉"这一页从不发的头没渗进来"。
//
// 反向改坏它的样子：为了让请求"更像正经客户端"而复用 [WithH5Headers] ——
// 于是 `activityid: sign_in_3` 被发进了一个从不认它的主机，
// `referer` 变成签到页。上游认不认多出来的头**零样本**，所以这一条守的是"别自造现场"。
func TestAiAvatarRequestsDoNotCarryH5OrAppHeaders(t *testing.T) {
	request := AiAvatarDrawRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel)
	for _, name := range []string{
		"activityid", "Referer", "referer", "showloading", "appversion",
		"isdeviceid", "Accept", "x-yun-uni", "x-yun-client-info", "x-SvcType",
	} {
		if hasHeader(request.Headers, name) {
			t.Errorf("这一族不发 %s，实得整套头 %+v", name, request.Headers)
		}
	}
}

// TestAiAvatarMergeBodyIsTwoKeysAndCarriesNoDataPrefix ⭐
//
// 两件事：body **只有** `imageBase64` + `imageExt:"jpg"` 两键；
// 并且本层**原样透传**调用方给的串（去 `data:image/png;base64,` 前缀是 app 层的活，
// 见旧 TS `task.ts:38-40` 那条正则）。
//
// ⚠ 不钉**字节序**：这一口零实拍，"逐字节对齐旧 TS"没有对照物，
// 而 JSON 对象的键序对语义无影响（钉死只会把 Go 序列化器的排序变成对拍面）。
func TestAiAvatarMergeBodyIsTwoKeysAndCarriesNoDataPrefix(t *testing.T) {
	raw := "iVBORw0KGgoAAAANSUhEUg=="
	request, err := AiAvatarMergeRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, raw)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	obj := bodyKeys(t, request.Body)
	if keys := sortedKeys(obj); strings.Join(keys, ",") != "imageBase64,imageExt" {
		t.Errorf("应恰有两键，实得 %v", keys)
	}
	if obj["imageExt"] != AiAvatarImageExt || AiAvatarImageExt != "jpg" {
		t.Errorf("imageExt 必须逐字是 \"jpg\"（旧实现写死那一个），实得 %v", obj["imageExt"])
	}
	if obj["imageBase64"] != raw {
		t.Errorf("base64 应逐字透传，实得 %v", obj["imageBase64"])
	}
	// 带前缀进来就带前缀出去：本层不"顺手修好"，否则去掉前缀那一步坏掉时无人知晓。
	prefixed := "data:image/png;base64," + raw
	withPrefix, err := AiAvatarMergeRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, prefixed)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if got := bodyKeys(t, withPrefix.Body)["imageBase64"]; got != prefixed {
		t.Errorf("本层不该改这一格，实得 %v", got)
	}
}

// TestAiAvatarStatusAndUploadBodiesAreTheLegacyKeys 钉另外两口 POST 的键集合。
//
// `upload` 是这一族**唯一写用户网盘**的口：两枚图片缺一枚，上游回什么**没有样本**，
// 而"只传合成图"这种"看起来更省"的改法会静默少传一张原图。
func TestAiAvatarStatusAndUploadBodiesAreTheLegacyKeys(t *testing.T) {
	status, err := AiAvatarStatusRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "T-1")
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	obj := bodyKeys(t, status.Body)
	if keys := sortedKeys(obj); strings.Join(keys, ",") != "taskId" {
		t.Errorf("getStatus 只带 taskId，实得 %v", keys)
	}
	if obj["taskId"] != "T-1" {
		t.Errorf("taskId 应逐字透传，实得 %v", obj["taskId"])
	}

	upload, err := AiAvatarUploadRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "ORIGIN", "MERGE")
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	up := bodyKeys(t, upload.Body)
	if keys := sortedKeys(up); strings.Join(keys, ",") != "mergeImage,originImage" {
		t.Errorf("upload 应恰有两枚图片，实得 %v", keys)
	}
	if up["originImage"] != "ORIGIN" || up["mergeImage"] != "MERGE" {
		t.Errorf("两张图各归各的键，实得 %v / %v", up["originImage"], up["mergeImage"])
	}
}

// TestAiAvatarWorkListTypeAndCatalogEncoding 钉查询串的两处细节。
//
// `catalogFullPath` 的 `marketName` **过 [PercentEncode]**：默认值 `rabbityearpic`
// 全字母，编码前后一模一样 ⇒ 光测默认值测不出"有没有编码"，所以另传一个含
// `+` / 空格 / `/` 的值来钉（`+` 在查询串里表示空格，不编码=换了个值）。
func TestAiAvatarWorkListTypeAndCatalogEncoding(t *testing.T) {
	list := AiAvatarWorkListRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, 2)
	if !strings.HasSuffix(list.URL, "/image/getWorkList?type=2") {
		t.Errorf("type 应进查询串，实得 %q", list.URL)
	}
	if len(list.Body) != 0 {
		t.Errorf("这一口是 GET 且无 body，实得 %q", list.Body)
	}
	path := AiAvatarCatalogPathRequest(aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "a b+c/d")
	if !strings.HasSuffix(path.URL, "?marketName=a%20b%2Bc%2Fd") {
		t.Errorf("marketName 必须百分号编码，实得 %q", path.URL)
	}
	if strings.Contains(path.URL, "+") {
		t.Errorf("编码后的串里不该还有裸的 +（它会解成空格），实得 %q", path.URL)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求形状：hcy/file/list（本文件唯一有实拍的口）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudFileListHasAuthorizationButNoJwtToken ⭐⭐ 钉住"两台主机、两枚头不对称"。
//
// 旧 TS 主实例的 `jwttoken` 只对 `caiyun.feixin.10086.cn` / `mrp.mcloud.139.com` /
// `m.mcloud.139.com` 三台发（`base.ts:14`），`personal-kd-njs.yun.139.com` **不在名单里**；
// 而 `authorization` 的名单按后缀匹配，这台**命中**。
//
// 反向改坏它的样子：把这一口也接进 [WithH5Headers] 那一路（"活动接口当然都带票"）——
// 请求看着更整齐，实测**未验证**是否会被拒；更糟的是它把"这台主机不收票"这一格
// 抓包事实抹掉了，将来查为什么网盘侧不认某个会话时会往错的方向查。
func TestCloudFileListHasAuthorizationButNoJwtToken(t *testing.T) {
	request, err := CloudFileListRequest(appProfile(), 1893456000000, aiAuthSentinel, "/AI空间", 100)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if request.URL != "https://personal-kd-njs.yun.139.com/hcy/file/list" {
		t.Errorf("应打实拍那一台，实得 %q", request.URL)
	}
	if got := headerOf(t, request.Headers, "Authorization"); got != aiAuthSentinel {
		t.Errorf("这一口有 authorization，实得 %q", got)
	}
	if hasHeader(request.Headers, "jwttoken") {
		t.Errorf("这一口**不发** jwttoken（那台主机不在旧 TS 名单里）")
	}
	if got := headerOf(t, request.Headers, "x-yun-module-type"); got != "100" {
		t.Errorf("x-yun-module-type 应是 100，实得 %q", got)
	}
	if got := headerOf(t, request.Headers, "x-yun-op-type"); got != "1" {
		t.Errorf("x-yun-op-type 应是 1，实得 %q", got)
	}
	// 走的是 App 原生那套头（x-yun-* 全家），不是 H5 那套。
	if !hasHeader(request.Headers, "x-yun-client-info") {
		t.Error("缺 x-yun-client-info ⇒ 没走 WithAppHeaders")
	}
	if hasHeader(request.Headers, "activityid") || hasHeader(request.Headers, "showloading") {
		t.Error("H5 专属头不该出现在网盘侧")
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰一枚，实得 %d 枚", n)
	}
}

// TestCloudFileListBodyFollowsLegacyBuilderNotTheCapture 钉"按旧实现发"这一取向（R16）。
//
// 实拍那一份（App 真机 okhttp）另有 `fields` / `ownerId` / `parentFilePath` / `type` /
// `workSpaceType` / `pageInfo.needTotalCount` / `pageInfo.pageCursor`。
// 这里断言的正是**这些键都不在**：取向按本仓这条链的旧 builder 发（它曾经可用），
// 实拍只用来钉 URL、信封形状与 `items[].fileId/name` 确实存在。
func TestCloudFileListBodyFollowsLegacyBuilderNotTheCapture(t *testing.T) {
	request, err := CloudFileListRequest(appProfile(), 1893456000000, aiAuthSentinel, "/synbox", 100)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	obj := bodyKeys(t, request.Body)
	want := "imageThumbnailStyleList,orderBy,orderDirection,pageInfo,parentFileId"
	if keys := sortedKeys(obj); strings.Join(keys, ",") != want {
		t.Errorf("应恰有旧 builder 那五键（%s），实得 %v", want, keys)
	}
	if obj["parentFileId"] != "/synbox" {
		t.Errorf("parentFileId 应逐字进来，实得 %v", obj["parentFileId"])
	}
	page, ok := obj["pageInfo"].(map[string]any)
	if !ok {
		t.Fatalf("pageInfo 应是对象，实得 %T", obj["pageInfo"])
	}
	if len(page) != 1 {
		t.Errorf("pageInfo 只有 pageSize 一键（实拍那份多两键，见 R16），实得 %v", sortedKeys(page))
	}
	if got, isNum := asInt64(page["pageSize"]); !isNum || got != 100 {
		t.Errorf("pageSize 应是 100，实得 %v", page["pageSize"])
	}
	if styles, ok := obj["imageThumbnailStyleList"].([]any); !ok || len(styles) != 2 {
		t.Errorf("缩略图档位应是数组，实得 %v", obj["imageThumbnailStyleList"])
	}
}

// aiStrPtr / aiDeref 是本文件的两个小工具（只为表格用例的可读性服务）。
func aiStrPtr(text string) *string { return &text }

func aiDeref(text *string) string {
	if text == nil {
		return "<nil>"
	}
	return *text
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析：读判据那一半
// ─────────────────────────────────────────────────────────────────────────────

// TestParseAiAvatarHashLeftNullIsNotZero ⭐ 钉"`nil` ≠ `0`"。
//
// 旧 TS 用 `typeof === "number"` 挡这一手（`task.ts:114`）。把"没给"读成 0
// 的表现是**少做一次**（今天还能抽却说没次数），而它比"多做一次"更难发现 ——
// 什么都不报错，只是当天什么都没发生。
func TestParseAiAvatarHashLeftNullIsNotZero(t *testing.T) {
	left, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, `{"code":0,"msg":"success","result":3}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if left == nil || *left != 3 {
		t.Fatalf("应解出 3，实得 %v", left)
	}

	zero, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, `{"code":0,"result":0}`))
	if perr != nil {
		t.Fatalf("0 是合法读数，不该报错: %v", perr)
	}
	if zero == nil || *zero != 0 {
		t.Errorf("应解出 0（且与 nil 可区分），实得 %v", zero)
	}

	nullPayload, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, `{"code":0,"result":null}`))
	if perr != nil {
		t.Fatalf("`result: null` 不该报错: %v", perr)
	}
	if nullPayload != nil {
		t.Errorf("null 应是 nil（没给），实得 %d", *nullPayload)
	}

	absent, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, `{"code":0}`))
	if perr != nil || absent != nil {
		t.Errorf("没有 result 也应是 nil 且不报错，实得 %v / %v", absent, perr)
	}

	// 字符串数字：[asInt64] 认（活动接口常把数字写成串），这里显式收下这一格行为。
	text, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, `{"code":0,"result":"5"}`))
	if perr != nil || text == nil || *text != 5 {
		t.Errorf("字符串数字应认，实得 %v / %v", text, perr)
	}

	for _, body := range []string{`{"code":0,"result":{"n":1}}`, `{"code":0,"result":"三次"}`} {
		if got, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, body)); perr == nil {
			t.Errorf("%s 应判解析失败（不是数字），实得 %v", body, got)
		}
	}

	if _, perr := ParseAiAvatarHashLeft(aiAvatarEnvelope(t, `{"code":90001,"msg":"未登录"}`)); perr == nil {
		t.Error("非 0 码应交回错误")
	}
}

// TestParseAiAvatarPrizeDoesNotRequireResult 钉"没有 result 不判失败"。
//
// 与 [ParseFunaiLottery] 同一口径：成败已由 `code` 说过一次，这里再判一次
// 会把一句上游原话换成一句我们编的解析错误。
func TestParseAiAvatarPrizeDoesNotRequireResult(t *testing.T) {
	prize, perr := ParseAiAvatarPrize(aiAvatarEnvelope(t,
		`{"code":0,"result":{"prizeName":"头像框","contentName":"限定头像"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if prize.PrizeName == nil || *prize.PrizeName != "头像框" {
		t.Errorf("prizeName 实得 %v", prize.PrizeName)
	}
	if prize.ContentName == nil || *prize.ContentName != "限定头像" {
		t.Errorf("contentName 实得 %v", prize.ContentName)
	}

	missing, perr := ParseAiAvatarPrize(aiAvatarEnvelope(t, `{"code":0}`))
	if perr != nil {
		t.Fatalf("没有 result 不该报错: %v", perr)
	}
	if missing.PrizeName != nil || missing.ContentName != nil {
		t.Errorf("两格都应是 nil，实得 %+v", missing)
	}

	onlyOne, perr := ParseAiAvatarPrize(aiAvatarEnvelope(t, `{"code":0,"result":{"prizeName":"云朵"}}`))
	if perr != nil || onlyOne.PrizeName == nil || onlyOne.ContentName != nil {
		t.Errorf("只有一格时应各归各，实得 %+v / %v", onlyOne, perr)
	}
}

// TestParseAiAvatarDrawDistinguishesStoppedFromFailed ⭐ 三态分得出来，且不靠错误串。
//
// `408` 走 [AiAvatarDrawResult].Stopped（旧实现 `break`），其余非 0 交回错误
// （旧实现 `continue`）。反向改坏它：删掉这一格 ⇒ 调用方只能对中文错误消息做子串匹配，
// 改一个字就会把"停止"念成"重试"，对已拒绝的账号连打五枪。
func TestParseAiAvatarDrawDistinguishesStoppedFromFailed(t *testing.T) {
	got, perr := ParseAiAvatarDraw(aiAvatarEnvelope(t, `{"code":408,"msg":"今日次数已用完"}`))
	if perr != nil {
		t.Fatalf("408 不是错误，是停止信号: %v", perr)
	}
	if !got.Stopped {
		t.Error("应落在 Stopped 那一态")
	}
	if got.Prize.PrizeName != nil || got.Prize.ContentName != nil {
		t.Errorf("408 那一档的响应体形状零样本，Prize 应留空，实得 %+v", got.Prize)
	}

	ok, perr := ParseAiAvatarDraw(aiAvatarEnvelope(t, `{"code":0,"result":{"prizeName":"云朵"}}`))
	if perr != nil || ok.Stopped || ok.Prize.PrizeName == nil {
		t.Errorf("code 0 应落在抽中那一态，实得 %+v / %v", ok, perr)
	}

	if _, perr := ParseAiAvatarDraw(aiAvatarEnvelope(t, `{"code":500,"msg":"其他失败"}`)); perr == nil {
		t.Error("其余非 0 码应交回错误（旧实现 continue 下一次）")
	}
}

// TestParseAiAvatarTaskServesBothShapes 钉"同一段代码吃 merge 与 getStatus 两种形状"。
//
// merge 只有 `taskId`、getStatus 才有 `status` / `imageUrl` ⇒ 三格全是指针。
// ⚠ 这里**不**断言 3/4 的语义（档位表零样本），只断言"解得出来、缺了是 nil"。
func TestParseAiAvatarTaskServesBothShapes(t *testing.T) {
	merge, perr := ParseAiAvatarTask(aiAvatarEnvelope(t, `{"code":0,"result":{"taskId":"T-9"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if merge.TaskID == nil || *merge.TaskID != "T-9" {
		t.Errorf("taskId 实得 %v", merge.TaskID)
	}
	if merge.Status != nil || merge.ImageURL != nil {
		t.Errorf("merge 那一枪不该凭空有 status / imageUrl，实得 %+v", merge)
	}

	status, perr := ParseAiAvatarTask(aiAvatarEnvelope(t,
		`{"code":0,"result":{"taskId":"T-9","status":3,"imageUrl":"https://x/y.png"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if status.Status == nil || *status.Status != AiAvatarStatusSuccess {
		t.Errorf("status 应解出 3，实得 %v", status.Status)
	}
	if status.ImageURL == nil || *status.ImageURL != "https://x/y.png" {
		t.Errorf("imageUrl 实得 %v", status.ImageURL)
	}

	pending, perr := ParseAiAvatarTask(aiAvatarEnvelope(t, `{"code":0,"result":{"status":null}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if pending.Status != nil || pending.TaskID != nil {
		t.Errorf("null 与缺失都应是 nil，实得 %+v", pending)
	}

	// 形状变了（result 是串）⇒ 三格 nil 而不是编一条解析错误：调用方按"没有 taskId"处置。
	weird, perr := ParseAiAvatarTask(aiAvatarEnvelope(t, `{"code":0,"result":"ok"}`))
	if perr != nil {
		t.Fatalf("不报错是设计: %v", perr)
	}
	if weird.TaskID != nil || weird.Status != nil || weird.ImageURL != nil {
		t.Errorf("实得 %+v", weird)
	}
}

// TestParseAiAvatarUploadIDRejectsEmptyString ⭐ 空串必须判失败。
//
// 旧实现只检查 `code === 0` 就收下（`task.ts:72`）。拿着空 id 往下走，
// 后续那三枪"按文件名找文件"会**静默匹配不到任何东西** ——
// 表现是"网盘里多了张图，没人知道"，而不是任何一处报错。
func TestParseAiAvatarUploadIDRejectsEmptyString(t *testing.T) {
	id, perr := ParseAiAvatarUploadID(aiAvatarEnvelope(t, `{"code":0,"result":"FFa1b2"}`))
	if perr != nil || id != "FFa1b2" {
		t.Errorf("实得 %q / %v", id, perr)
	}
	if id, perr := ParseAiAvatarUploadID(aiAvatarEnvelope(t, `{"code":0,"result":""}`)); perr == nil {
		t.Errorf("空串应判失败，实得 %q", id)
	}
	if _, perr := ParseAiAvatarUploadID(aiAvatarEnvelope(t, `{"code":0,"result":123}`)); perr == nil {
		t.Error("数字形态没有样本，不猜 ⇒ 判解析失败")
	}
	if _, perr := ParseAiAvatarUploadID(aiAvatarEnvelope(t, `{"code":0}`)); perr == nil {
		t.Error("没有 result 也应判失败（这一口的 result 就是那个 id）")
	}
	if _, perr := ParseAiAvatarUploadID(aiAvatarEnvelope(t, `{"code":1,"msg":"失败"}`)); perr == nil {
		t.Error("非 0 码应交回错误")
	}
}

// TestParseAiAvatarWorkListShape 钉"形状变了不能念成'这个号没有作品'"。
//
// 旧 TS 是 `result || []`：`result` 是对象时兜成空数组，界面读出来是"干净"。
// 这里改成解析失败，并把"数字 id 也认"钉住（活动接口常把 id 写成数字）。
func TestParseAiAvatarWorkListShape(t *testing.T) {
	works, perr := ParseAiAvatarWorkList(aiAvatarEnvelope(t, `{"code":0,"result":[
		{"id":"W-1","imgUrl":"https://x/a.png","qrImgUrl":"https://x/q.png"},
		{"id":3228,"imgUrl":null},
		{"imgUrl":"https://x/no-id.png"},
		"不是对象的一项"
	]}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(works) != 2 {
		t.Fatalf("无 id 的项与非对象项应跳过，实得 %d 条: %+v", len(works), works)
	}
	if works[0].WorkID != "W-1" || works[0].ImgURL == nil || works[0].QrImgURL == nil {
		t.Errorf("第一条实得 %+v", works[0])
	}
	if works[1].WorkID != "3228" {
		t.Errorf("数字 id 应转成串，实得 %q", works[1].WorkID)
	}
	if works[1].ImgURL != nil {
		t.Errorf("imgUrl 是 null ⇒ nil，实得 %v", *works[1].ImgURL)
	}

	if got, perr := ParseAiAvatarWorkList(aiAvatarEnvelope(t, `{"code":0,"result":null}`)); perr != nil || got != nil {
		t.Errorf("null 应是 (nil, nil)，实得 %v / %v", got, perr)
	}
	if _, perr := ParseAiAvatarWorkList(aiAvatarEnvelope(t, `{"code":0,"result":{"list":[]}}`)); perr == nil {
		t.Error("result 不是数组应判解析失败，不能兜成空列表")
	}
	if _, perr := ParseAiAvatarWorkList(aiAvatarEnvelope(t, `{"code":0,"success":false}`)); perr == nil {
		t.Error("success:false 优先判失败")
	}
}

// TestParseAiAvatarCatalogParentFileIDTakesLastNonEmptySegment 钉切法逐字对齐旧实现。
//
// `fullPath_sub.split("/").filter(Boolean).pop()` —— 按 `/` 切、**丢空段**、取末段。
// 丢空段这一格最容易被"顺手简化成 strings.Split 取最后一个"替代：
// 末尾带斜杠时那一句会交出空串，而空 `parentFileId` 在 file/list 里
// 是**另一个目录**（根），于是那一枪列的是根目录。
func TestParseAiAvatarCatalogParentFileIDTakesLastNonEmptySegment(t *testing.T) {
	cases := []struct {
		name string
		body string
		want *string
	}{
		{"普通路径", `{"code":0,"result":{"fullPath_sub":"/AI空间/FFa1b2"}}`, aiStrPtr("FFa1b2")},
		{"末尾带斜杠", `{"code":0,"result":{"fullPath_sub":"/AI空间/FFa1b2/"}}`, aiStrPtr("FFa1b2")},
		{"连续斜杠", `{"code":0,"result":{"fullPath_sub":"//a///FFa1b2//"}}`, aiStrPtr("FFa1b2")},
		{"无斜杠", `{"code":0,"result":{"fullPath_sub":"FFa1b2"}}`, aiStrPtr("FFa1b2")},
		{"只有根", `{"code":0,"result":{"fullPath_sub":"/"}}`, nil},
		{"全斜杠", `{"code":0,"result":{"fullPath_sub":"///"}}`, nil},
		{"空串", `{"code":0,"result":{"fullPath_sub":""}}`, nil},
		{"缺键", `{"code":0,"result":{}}`, nil},
		{"没有 result", `{"code":0}`, nil},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, perr := ParseAiAvatarCatalogParentFileID(aiAvatarEnvelope(t, c.body))
			if perr != nil {
				t.Fatalf("不该报错: %v", perr)
			}
			switch {
			case c.want == nil && got != nil:
				t.Errorf("应是 nil，实得 %q", *got)
			case c.want != nil && got == nil:
				t.Errorf("应是 %q，实得 nil", *c.want)
			case c.want != nil && *got != *c.want:
				t.Errorf("期望 %q，实得 %q", *c.want, *got)
			}
		})
	}

	if _, perr := ParseAiAvatarCatalogParentFileID(aiAvatarEnvelope(t, `{"code":0,"result":["/a/b"]}`)); perr == nil {
		t.Error("result 不是对象应判解析失败")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析：hcy/file/list（唯一有实拍的一份）
// ─────────────────────────────────────────────────────────────────────────────

// capturedFileListBody 是 `test/fixtures/http/ref.json` 里
// `POST personal-kd-njs.yun.139.com/hcy/file/list` 响应的**形状样本**：
// 顶层四键逐字保留，`items` 只留本族要用的两格（其余十余格与账号标识都不进夹具），
// 且只挑未脱敏、公开的那两条目录（另外几条的 `fileId` 在归档里已是 `<REDACTED>`）。
const capturedFileListBody = `{"code":"0000","message":"请求成功","success":true,"data":{` +
	`"items":[` +
	`{"fileId":"/synbox","name":"同步盘","category":"folder","type":"folder","createdAt":"2024-02-17T12:47:27.000+08:00"},` +
	`{"fileId":"/myfaverapp","name":"我的应用收藏","category":"folder","type":"folder"}` +
	`],"nextPageCursor":null,"parentFilePathInfo":null}}`

// TestParseCloudFileListFromTheCapturedEnvelope ⭐ 实拍那份信封解得开。
//
// 两处"看着会错"的地方：`code` 是**字符串** `"0000"`（[asInt64] 认字符串数字 ⇒
// [Envelope.EnsureOK] 判成功），载荷在 `data` 而不是 `result`（[Envelope.Payload] 先认 `data`）。
func TestParseCloudFileListFromTheCapturedEnvelope(t *testing.T) {
	page, perr := ParseCloudFileList(aiAvatarEnvelope(t, capturedFileListBody))
	if perr != nil {
		t.Fatalf("实拍信封解不开: %v", perr)
	}
	if len(page.Items) != 2 {
		t.Fatalf("实得 %+v", page.Items)
	}
	if page.Items[0].FileID != "/synbox" || page.Items[0].Name != "同步盘" {
		t.Errorf("第一条实得 %+v", page.Items[0])
	}
	if page.NextPageCursor != nil {
		t.Errorf("实拍这一格是 null ⇒ nil，实得 %q", *page.NextPageCursor)
	}
}

// TestParseCloudFileListShapeFailures 钉三档形状处置，方向各不相同。
//
//   - `success:false` ⇒ 失败（优先于 `code`，实拍两条都有这一格）
//   - `data.items` **不是数组** ⇒ 解析失败（旧实现兜成 `[]`，会把"形状变了"念成"目录是空的"，
//     而清理那一族拿它判"没有临时文件要删"）
//   - `data` 里**没有** `items` 这一格 ⇒ **空列表**（实拍两条都有这一格，所以"没有"
//     更可能是"这一页确实没文件"；这一口只读，匹配不到就是匹配不到，不会多删）
func TestParseCloudFileListShapeFailures(t *testing.T) {
	if _, perr := ParseCloudFileList(aiAvatarEnvelope(t,
		`{"code":"0000","success":false,"message":"失败","data":{}}`)); perr == nil {
		t.Error("success:false 应判失败")
	}
	if _, perr := ParseCloudFileList(aiAvatarEnvelope(t,
		`{"code":"5001","message":"失败","data":{}}`)); perr == nil {
		t.Error("非 0 码应判失败")
	}
	if _, perr := ParseCloudFileList(aiAvatarEnvelope(t,
		`{"code":"0000","success":true,"data":{"items":{}}}`)); perr == nil {
		t.Error("items 不是数组应判解析失败")
	}
	if _, perr := ParseCloudFileList(aiAvatarEnvelope(t,
		`{"code":"0000","success":true,"data":[]}`)); perr == nil {
		t.Error("data 不是对象应判解析失败")
	}
	page, perr := ParseCloudFileList(aiAvatarEnvelope(t,
		`{"code":"0000","success":true,"data":{}}`))
	if perr != nil {
		t.Fatalf("没有 items 不该报错: %v", perr)
	}
	if page.Items == nil || len(page.Items) != 0 {
		t.Errorf("应是**非 nil** 的空列表，实得 %#v", page.Items)
	}
}

// TestParseCloudFileListSkipsEntriesWithoutBothKeys 钉条目级筛选。
//
// `fileId` 与 `name` **都要有**才收：清理那一族是"按名字匹配、拿 id 去删/去用"，
// 只有名字没有 id 的条目一旦收进来，匹配上之后就没有可用的目标。
func TestParseCloudFileListSkipsEntriesWithoutBothKeys(t *testing.T) {
	page, perr := ParseCloudFileList(aiAvatarEnvelope(t, `{"code":"0000","success":true,"data":{
		"items":[
			{"fileId":"FFa1","name":"a.png"},
			{"fileId":"FFa2"},
			{"name":"没有id.png"},
			"不是对象",
			{"fileId":"","name":""}
		],"nextPageCursor":"11"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(page.Items) != 2 {
		t.Fatalf("只应收两条（含空串那条：本层不判空，交给按名字匹配），实得 %+v", page.Items)
	}
	if page.Items[1].FileID != "" || page.Items[1].Name != "" {
		t.Errorf("空串条目原样交出，实得 %+v", page.Items[1])
	}
	if page.NextPageCursor == nil || *page.NextPageCursor != "11" {
		t.Errorf("游标是串时应解出（本族不翻页，只带出来），实得 %v", page.NextPageCursor)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层：走一遍 [Upstream.Call]
// ─────────────────────────────────────────────────────────────────────────────

// TestAppAiAvatarRoundTripsSendOneRequestEach 钉发送层"一枪一发、解析接得上"。
//
// 只测这一层能测的：请求发出去了、URL 与头没在中途被换掉、响应按各自的形状解。
// 业务节奏（轮询几次、每天几次）在 app 层，不在这张表里。
func TestAppAiAvatarRoundTripsSendOneRequestEach(t *testing.T) {
	ctx := context.Background()

	t.Run("getHashLeft", func(t *testing.T) {
		up, client := respondWith(t, map[string]any{"code": 0, "result": 2})
		got, perr := up.AppAiAvatarHashLeft(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel)
		if perr != nil || got == nil || *got != 2 {
			t.Fatalf("实得 %v / %v", got, perr)
		}
		if len(client.requests) != 1 || !strings.HasSuffix(client.requests[0].URL, "/market/rabactive/image/getHashLeft") {
			t.Fatalf("应恰一发且打在 /market/ 上，实得 %+v", client.requests)
		}
	})

	t.Run("draw 三态", func(t *testing.T) {
		up, _ := respondWith(t, map[string]any{"code": 408, "msg": "今日到此为止"})
		got, perr := up.AppAiAvatarDraw(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel)
		if perr != nil || !got.Stopped {
			t.Fatalf("实得 %+v / %v", got, perr)
		}
	})

	t.Run("merge", func(t *testing.T) {
		up, client := respondWith(t, map[string]any{"code": 0, "result": map[string]any{"taskId": "T-7"}})
		got, perr := up.AppAiAvatarMerge(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "aGVsbG8=")
		if perr != nil || got.TaskID == nil || *got.TaskID != "T-7" {
			t.Fatalf("实得 %+v / %v", got, perr)
		}
		if !strings.Contains(string(client.requests[0].Body), `"imageExt":"jpg"`) {
			t.Errorf("body 实得 %s", client.requests[0].Body)
		}
	})

	t.Run("getStatus", func(t *testing.T) {
		up, client := respondWith(t, map[string]any{"code": 0, "result": map[string]any{
			"taskId": "T-7", "status": 4, "imageUrl": nil}})
		got, perr := up.AppAiAvatarStatus(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "T-7")
		if perr != nil || got.Status == nil || *got.Status != AiAvatarStatusFailed {
			t.Fatalf("实得 %+v / %v", got, perr)
		}
		if !strings.Contains(string(client.requests[0].Body), `"taskId":"T-7"`) {
			t.Errorf("body 实得 %s", client.requests[0].Body)
		}
	})

	t.Run("upload", func(t *testing.T) {
		up, client := respondWith(t, map[string]any{"code": 0, "result": "FFa1"})
		got, perr := up.AppAiAvatarUpload(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, "O", "M")
		if perr != nil || got != "FFa1" {
			t.Fatalf("实得 %q / %v", got, perr)
		}
		body := string(client.requests[0].Body)
		if !strings.Contains(body, `"originImage":"O"`) || !strings.Contains(body, `"mergeImage":"M"`) {
			t.Errorf("两张图都要发出去，实得 %s", body)
		}
	})

	t.Run("getWorkList", func(t *testing.T) {
		up, _ := respondWith(t, map[string]any{"code": 0, "result": []any{
			map[string]any{"id": "W-1", "imgUrl": "https://x/a.png"}}})
		got, perr := up.AppAiAvatarWorkList(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel, 1)
		if perr != nil || len(got) != 1 || got[0].WorkID != "W-1" {
			t.Fatalf("实得 %+v / %v", got, perr)
		}
	})

	t.Run("catalogFullPath", func(t *testing.T) {
		up, client := respondWith(t, map[string]any{"code": 0, "result": map[string]any{
			"fullPath_sub": "/AI空间/FFa1b2"}})
		got, perr := up.AppAiAvatarCatalogParentFileID(ctx, aiAuthSentinel, aiJWTSentinel, aiDeviceSentinel,
			AiAvatarCatalogMarketName)
		if perr != nil || got == nil || *got != "FFa1b2" {
			t.Fatalf("实得 %v / %v", aiDeref(got), perr)
		}
		if !strings.HasSuffix(client.requests[0].URL, "?marketName=rabbityearpic") {
			t.Errorf("URL 实得 %s", client.requests[0].URL)
		}
	})

	t.Run("file/list", func(t *testing.T) {
		up, client := respondWith(t, map[string]any{
			"code": "0000", "success": true, "message": "请求成功",
			"data": map[string]any{
				"items":          []any{map[string]any{"fileId": "FFa1", "name": "AI头像.png"}},
				"nextPageCursor": nil, "parentFilePathInfo": nil,
			},
		})
		page, perr := up.AppCloudFileList(ctx, appProfile(), 1893456000000, aiAuthSentinel, "/", 100)
		if perr != nil || len(page.Items) != 1 || page.Items[0].Name != "AI头像.png" {
			t.Fatalf("实得 %+v / %v", page, perr)
		}
		request := client.requests[0]
		if hasHeader(request.Headers, "jwttoken") {
			t.Error("发送层也不该给这一口补上票")
		}
		if !hasHeader(request.Headers, "Authorization") {
			t.Error("发送层不该把 authorization 弄丢")
		}
	})
}

// TestAiAvatarTimeIndependentConstantsAreTheLiteralBytes 钉三枚字面量**逐字**没错：
// `"jpg"`、`rabbityearpic`、`408`。它们是这一族仅有的"抄错一个字符就换了一条链"的常量，
// 而**不**碰档位表（3/4 的语义未验证，这里只钉它俩与旧实现字面一致，不解释含义）。
func TestAiAvatarTimeIndependentConstantsAreTheLiteralBytes(t *testing.T) {
	if AiAvatarImageExt != "jpg" {
		t.Errorf("imageExt 实得 %q", AiAvatarImageExt)
	}
	if AiAvatarCatalogMarketName != "rabbityearpic" {
		t.Errorf("marketName 实得 %q", AiAvatarCatalogMarketName)
	}
	if AiAvatarDrawCodeStopped != 408 {
		t.Errorf("停止码实得 %d", AiAvatarDrawCodeStopped)
	}
	if BaseRabactive != "https://m.mcloud.139.com/market/rabactive" ||
		BasePersonalKD != "https://personal-kd-njs.yun.139.com" {
		t.Errorf("基址实得 %q / %q", BaseRabactive, BasePersonalKD)
	}
}
