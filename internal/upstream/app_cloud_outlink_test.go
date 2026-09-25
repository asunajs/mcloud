package upstream

// 分享外链那一族两口的**协议层**用例（玩法 434 的前置）。
//
// # 这一族钉什么、不钉什么
//
// 两口**零 REST 实拍**（见 `app_cloud_outlink.go` 文件头那一节：仓里那三处名字像的
// 东西分别是"查最近收到的分享"、PC 客户端的 XML 版 `getOutLinkV3` 与代码里的键名）。
// 所以这里**没有**一条断言在钉"上游真的接受这些键 / 真的回 linkID"：
// 那些格子只有"旧 TS 这么发过"这一个证据源。
//
// 全部用例钉的是三类**可控**的事：
//
//  1. **字面量别在翻译中丢失** —— 十三枚头里那两枚只差结尾管道数的设备串、
//     `mcloud-version` 的 `7.14.4`（与上传那一族串里的 `7.15.3` **不是一枚**）、
//     `shareChannel:"3001"`（PC 客户端那一版是 `3003`）。"顺手统一"任何一处都静默改掉一次接口。
//  2. **两口的头集合不对称是事实，不是笔误** —— `getOutLink` 带那十三枚、
//     `delOutLink` 一枚都不带（旧 TS 那里没有第三参数）。把两边统一会发出一份
//     上游从未收到过的请求。
//  3. **"没给"与"给了零值"、"形状变了"必须是三种结果** —— 非 0 码 / `success:false`
//     是错误；载荷在而 `getOutLinkResSet` 为空是"拿不到 linkID"；
//     `data` 不是对象或 PC 那一种 `result` 包装是"形状变了"。
//     把后两档混成一档的表现是：外链建好了却没人知道怎么删，而日志说一切正常。
//
// # 凭据边界
//
// `authorization` / `deviceid` 用合成哨兵串。`account` 那一格是**手机号**
// （旧实现传 `ctx.config.phone`）⇒ 用例里只用 `199` 号段的合成值，
// 并钉"错误文本里不 echo 它"。
//
// # 反向改坏这一族时的红法（逐条对应用例）
//
// 把两口的头统一 ⇒ 第 2 类红；把 `x-deviceinfo` 与 `x-yun-client-info` 改成同一串 ⇒
// 那一枚"结尾管道数"的断言红；给 `getOutLink` 补幂等键让它可重试 ⇒ `IsRetryable` 红
// （重放一次会多建一条公开外链）；把"载荷不是对象"兜成"没有 linkID" ⇒ 第 3 类红。

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

const (
	// outlinkSynthPhone 是**合成**手机号（199 号段，与标识守卫同一条不变量）。
	outlinkSynthPhone   = "19900000000"
	outlinkSynthFileID  = "SYNTH-FILE-ID"
	outlinkSynthLinkID  = "SYNTH-LINK-ID"
	outlinkAuthSentinel = "Basic U1lOVEhFVElDLUFVVEg="
	outlinkDeviceName   = "DEVICEID-SENTINEL"
)

// outlinkInput 是一个合法的默认输入，用例只改自己关心的那一格。
func outlinkInput() CloudOutLinkInput {
	return CloudOutLinkInput{
		Account: outlinkSynthPhone,
		FileIDs: []string{outlinkSynthFileID},
	}
}

// mustOutLinkRequest 构造请求，失败即 fatal。
func mustOutLinkRequest(t *testing.T, in CloudOutLinkInput) core.HTTPRequest {
	t.Helper()
	request, perr := CloudGetOutLinkRequest(outlinkAuthSentinel, outlinkDeviceName, in)
	if perr != nil {
		t.Fatalf("构造 getOutLink 失败: %v", perr.Message())
	}
	return request
}

// outlinkReqObject 取出 body 里那一层 `getOutLinkReq`。
func outlinkReqObject(t *testing.T, body []byte) map[string]any {
	t.Helper()
	obj := bodyKeys(t, body)
	if got := strings.Join(sortedKeys(obj), ","); got != "getOutLinkReq" {
		t.Fatalf("顶层应只有一个键 getOutLinkReq，实得 %s", got)
	}
	req, ok := obj["getOutLinkReq"].(map[string]any)
	if !ok {
		t.Fatalf("getOutLinkReq 不是对象，实得 %T", obj["getOutLinkReq"])
	}
	return req
}

// ─────────────────────────────────────────────────────────────────────────────
// 第一口：getOutLink 的请求体
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudGetOutLinkBodyIsLegacyElevenKeys ⭐ 十一键齐全，且那些**没有解释的数字**在位。
//
// 反向改坏它的样子：把 `shareChannel` 改成 PC 客户端那一版的 `3003` ⇒ 红；
// 把 `period` / `periodUnit` "顺手参数化"成 7 天 ⇒ 键值红（那一格没有出处，
// 要改得先实拍，属改接口）。
func TestCloudGetOutLinkBodyIsLegacyElevenKeys(t *testing.T) {
	request := mustOutLinkRequest(t, outlinkInput())
	if request.URL != BaseYun139Orchestration+"/outlink/v1.0/getOutLink" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应为 POST，实得 %v", request.Method)
	}
	req := outlinkReqObject(t, request.Body)
	wantKeys := []string{
		"caIDLst", "coIDLst", "commonAccountInfo", "dedicatedName", "encrypt", "extInfo",
		"period", "periodUnit", "pubType", "subLinkType", "viewerLst",
	}
	if got := strings.Join(sortedKeys(req), ","); got != strings.Join(wantKeys, ",") {
		t.Errorf("键集合实得 %s", got)
	}
	// 六个数字位：逐字（旧 TS `apis/share.ts:25-37`），语义**未验证**。
	for key, want := range map[string]any{
		"subLinkType": float64(0), "encrypt": float64(1), "pubType": float64(1),
		"period": float64(1), "periodUnit": float64(1),
	} {
		if req[key] != want {
			t.Errorf("%s 应是 %v，实得 %v", key, want, req[key])
		}
	}
	// 两枚空数组**必须出现**（旧 TS 有这两格）：缺席与"给了空列表"在上游那边
	// 是不是同一回事，零样本 ⇒ 照抄形状。
	for _, key := range []string{"caIDLst", "viewerLst"} {
		list, isList := req[key].([]any)
		if !isList || len(list) != 0 {
			t.Errorf("%s 应是**出现的**空数组，实得 %v", key, req[key])
		}
	}
	if ids, ok := req["coIDLst"].([]any); !ok || len(ids) != 1 || ids[0] != outlinkSynthFileID {
		t.Errorf("coIDLst 实得 %v", req["coIDLst"])
	}
	ext, ok := req["extInfo"].(map[string]any)
	if !ok {
		t.Fatalf("extInfo 不是对象，实得 %T", req["extInfo"])
	}
	if ext["shareChannel"] != "3001" {
		t.Errorf("shareChannel 实得 %v（PC 客户端那一版是 3003，两版不许统一）", ext["shareChannel"])
	}
	if ext["isWatermark"] != float64(0) {
		t.Errorf("isWatermark 实得 %v", ext["isWatermark"])
	}
	head, ok := req["commonAccountInfo"].(map[string]any)
	if !ok {
		t.Fatalf("commonAccountInfo 不是对象，实得 %T", req["commonAccountInfo"])
	}
	if head["account"] != outlinkSynthPhone || head["accountType"] != float64(1) {
		t.Errorf("commonAccountInfo 实得 %v", head)
	}
	// `dedicatedName` 旧实现恒传空串 ⇒ 空串这一档要**出现**而不是消失。
	if name, present := req["dedicatedName"]; !present || name != "" {
		t.Errorf("dedicatedName 应是出现的空串，实得 %v / %v", name, present)
	}
}

// TestCloudGetOutLinkKeepsGivenNameAndMultipleIDs 钉"这一层不替调用方决定分享什么"。
func TestCloudGetOutLinkKeepsGivenNameAndMultipleIDs(t *testing.T) {
	in := CloudOutLinkInput{
		Account: outlinkSynthPhone, DedicatedName: "SYNTH-NAME",
		FileIDs: []string{"A", "B"},
	}
	req := outlinkReqObject(t, mustOutLinkRequest(t, in).Body)
	if req["dedicatedName"] != "SYNTH-NAME" {
		t.Errorf("dedicatedName 实得 %v", req["dedicatedName"])
	}
	if ids, _ := req["coIDLst"].([]any); len(ids) != 2 {
		t.Errorf("这一层不裁剪 coIDLst（裁剪是编排层的事），实得 %v", ids)
	}
}

// TestCloudGetOutLinkRejectsEmptyInputs 钉两格前置：空文件列表与空 account ⇒ 不发。
//
// 反向改坏它的样子：把这两格兜成"照发" ⇒ 上游会收到一条 `coIDLst` 为空的分享请求
// 或一条定位不到账号的请求；旧实现两者**都不发**（前者在调用侧挡，后者根本不会出现）。
func TestCloudGetOutLinkRejectsEmptyInputs(t *testing.T) {
	cases := []struct {
		name string
		in   CloudOutLinkInput
	}{
		{"空文件列表", CloudOutLinkInput{Account: outlinkSynthPhone}},
		{"空 account", CloudOutLinkInput{FileIDs: []string{outlinkSynthFileID}}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, perr := CloudGetOutLinkRequest(outlinkAuthSentinel, outlinkDeviceName, tc.in)
			if perr == nil {
				t.Fatal("应当回错误且不发请求")
			}
			// 凭据边界：错误文本里不出现手机号，也不出现哨兵凭据。
			if strings.Contains(perr.Message(), outlinkSynthPhone) {
				t.Error("错误文本 echo 了 account（手机号）")
			}
			if strings.Contains(perr.Message(), outlinkAuthSentinel) {
				t.Error("错误文本 echo 了 authorization")
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第一口：getOutLink 的请求头
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudGetOutLinkHeadersAreLegacyThirteen ⭐ 十三枚逐字，且每枚恰好一根线。
func TestCloudGetOutLinkHeadersAreLegacyThirteen(t *testing.T) {
	request := mustOutLinkRequest(t, outlinkInput())
	want := map[string]string{
		"caller":               OutLinkWebCaller,
		"cms-device":           "default",
		"mcloud-channel":       OutLinkMcloudChannel,
		"mcloud-version":       OutLinkMcloudVersion,
		"x-deviceinfo":         OutLinkDeviceInfo,
		"x-huawei-channelsrc":  string(CloudChannelWeb),
		"x-svctype":            "1",
		"x-yun-api-version":    "v1",
		"x-yun-app-channel":    string(CloudChannelWeb),
		"x-yun-channel-source": string(CloudChannelWeb),
		"x-yun-client-info":    OutLinkClientInfo,
		"x-yun-module-type":    "100",
		"x-yun-svc-type":       "1",
	}
	if len(want) != 13 {
		t.Fatalf("这张表本身就是断言：旧 TS 那十三枚，实得 %d 枚", len(want))
	}
	for name, value := range want {
		if n := countHeader(request.Headers, name); n != 1 {
			t.Errorf("%s 应恰好一枚，实得 %d", name, n)
		}
		if got := headerValue(request.Headers, name); got != value {
			t.Errorf("%s 实得 %q，期望 %q", name, got, value)
		}
	}
}

// TestOutLinkDeviceStringsDifferOnlyInTrailingPipes ⭐ 那两枚"看着一样"的串**不许统一**。
//
// 这一条的存在理由：两枚只差结尾，肉眼读源码很容易"顺手合并成一个常量"。
// 反向改坏：把 [OutLinkClientInfo] 改成 [OutLinkDeviceInfo] ⇒ 红。
func TestOutLinkDeviceStringsDifferOnlyInTrailingPipes(t *testing.T) {
	if OutLinkDeviceInfo == OutLinkClientInfo {
		t.Fatal("两枚设备串被统一了：旧 TS 里它们结尾的管道数不同")
	}
	if !strings.HasPrefix(OutLinkDeviceInfo, "||9|7.14.4|edge") ||
		!strings.HasPrefix(OutLinkClientInfo, "||9|7.14.4|edge") {
		t.Error("两枚的前缀应当相同（只差结尾），前缀变了说明其中一枚被改写")
	}
	if !strings.HasSuffix(OutLinkDeviceInfo, "zh-CN|||") ||
		!strings.HasSuffix(OutLinkClientInfo, "zh-CN|||||") {
		t.Errorf("结尾实得 %q / %q", OutLinkDeviceInfo, OutLinkClientInfo)
	}
}

// TestCloudGetOutLinkStaticHeadersAndNoJWT 钉"这一台有 Basic、没有 jwttoken"。
//
// 出处是旧 TS `base.ts` 的两张名单：`AUTH_DOMAINS` 里有 `yun.139.com`（⇒ 带），
// `TARGET_HOSTNAMES` 里没有它（⇒ 不带 jwttoken）。反向改坏：给这一族补一枚
// `jwttoken`（"反正另一个族带了"）⇒ 红，而那是一枚上游从未在这一口收到过的头。
func TestCloudGetOutLinkStaticHeadersAndNoJWT(t *testing.T) {
	request := mustOutLinkRequest(t, outlinkInput())
	if got := headerValue(request.Headers, "Authorization"); got != outlinkAuthSentinel {
		t.Errorf("Authorization 实得 %q", got)
	}
	if got := headerValue(request.Headers, "deviceid"); got != outlinkDeviceName {
		t.Errorf("deviceid 实得 %q", got)
	}
	if n := countHeader(request.Headers, "Content-Type"); n != 1 {
		t.Errorf("Content-Type 应恰好一枚，实得 %d", n)
	}
	if got := headerValue(request.Headers, "Content-Type"); got != H5ContentType {
		t.Errorf("Content-Type 实得 %q", got)
	}
	if countHeader(request.Headers, "jwttoken") != 0 {
		t.Error("这一族不带 jwttoken（yun.139.com 不在旧 TS 的 TARGET_HOSTNAMES 里）")
	}
	if n := countHeader(request.Headers, "User-Agent"); n != 1 {
		t.Errorf("User-Agent 应恰好一枚，实得 %d", n)
	}
}

// TestCloudGetOutLinkIsNotRetryable 钉"这一枪发一次就够"。
//
// 它是**写**：重放一次会多建一条公开外链。反向改坏：给它补幂等键 ⇒ 红。
func TestCloudGetOutLinkIsNotRetryable(t *testing.T) {
	if mustOutLinkRequest(t, outlinkInput()).IsRetryable() {
		t.Error("getOutLink 不该可重试")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 第二口：delOutLink
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudDelOutLinkOmitsLegacyHeaderSet ⭐ 两口的头集合**不对称**，照抄不统一。
//
// 这一条是这一族里最容易被"顺手修正"的地方：给第二口补上第一口那十三枚看起来
// 更像整洁代码，而上游在 `delOutLink` 这一口**从没收到过**它们（零样本）。
func TestCloudDelOutLinkOmitsLegacyHeaderSet(t *testing.T) {
	request, perr := CloudDelOutLinkRequest(
		outlinkAuthSentinel, outlinkDeviceName, outlinkSynthPhone, []string{outlinkSynthLinkID})
	if perr != nil {
		t.Fatalf("构造 delOutLink 失败: %v", perr.Message())
	}
	if request.URL != BaseYun139Orchestration+"/outlink/v1.0/delOutLink" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	for _, name := range []string{
		"caller", "cms-device", "mcloud-channel", "mcloud-version", "x-deviceinfo",
		"x-huawei-channelsrc", "x-svctype", "x-yun-api-version", "x-yun-app-channel",
		"x-yun-channel-source", "x-yun-client-info", "x-yun-module-type", "x-yun-svc-type",
	} {
		if countHeader(request.Headers, name) != 0 {
			t.Errorf("%s 不该出现在 delOutLink（旧 TS 那一口没有第三参数）", name)
		}
	}
	// 静态头仍然在：这一口只是**不带那十三枚**，不是裸请求。
	if headerValue(request.Headers, "Authorization") != outlinkAuthSentinel {
		t.Error("delOutLink 应带 Authorization")
	}
	if headerValue(request.Headers, "deviceid") != outlinkDeviceName {
		t.Error("delOutLink 应带 deviceid")
	}
	obj := bodyKeys(t, request.Body)
	if got := strings.Join(sortedKeys(obj), ","); got != "delOutLinkReq" {
		t.Errorf("顶层应只有一个键 delOutLinkReq，实得 %s", got)
	}
	req, _ := obj["delOutLinkReq"].(map[string]any)
	if got := strings.Join(sortedKeys(req), ","); got != "commonAccountInfo,linkIDs" {
		t.Errorf("delOutLinkReq 的键集合实得 %s", got)
	}
	if ids, _ := req["linkIDs"].([]any); len(ids) != 1 || ids[0] != outlinkSynthLinkID {
		t.Errorf("linkIDs 实得 %v", req["linkIDs"])
	}
}

// TestCloudDelOutLinkRejectsEmptyInputs 与第一口同一条纪律（空列表 / 空 account ⇒ 不发）。
func TestCloudDelOutLinkRejectsEmptyInputs(t *testing.T) {
	if _, perr := CloudDelOutLinkRequest(
		outlinkAuthSentinel, outlinkDeviceName, outlinkSynthPhone, nil); perr == nil {
		t.Error("空 linkIDs 应当不发")
	}
	_, perr := CloudDelOutLinkRequest(outlinkAuthSentinel, outlinkDeviceName, "", []string{"SYNTH"})
	if perr == nil {
		t.Fatal("空 account 应当不发")
	}
	if strings.Contains(perr.Message(), outlinkSynthPhone) {
		t.Error("错误文本 echo 了手机号")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析
// ─────────────────────────────────────────────────────────────────────────────

// TestParseCloudOutLinkReadsLinkID ⭐ 载荷在 `data`，键名是**大写 ID** 的 `linkID`。
//
// 夹具是合成的（形状取自旧 TS 的 `GetOutLinkResponse` 类型声明，值一律 SYNTH-*）。
func TestParseCloudOutLinkReadsLinkID(t *testing.T) {
	envelope := envelopeOf(t, `{"code":"0","data":{"getOutLinkRes":{
		"getOutLinkResSet":[{"linkID":"SYNTH-LINK-ID","linkURL":"https://synth.example.invalid/s/1"}]}}}`)
	got, perr := ParseCloudOutLink(envelope)
	if perr != nil {
		t.Fatalf("解析失败: %v", perr.Message())
	}
	if got.LinkID == nil || *got.LinkID != outlinkSynthLinkID {
		t.Fatalf("linkID 实得 %v", got.LinkID)
	}
}

// TestParseCloudOutLinkAcceptsBothZeroSpellings ⭐ 那处"刻意不一致"就钉在这里。
//
// 旧 runner 判的是 `code === "0"`（字符串字面相等），而本层走 [Envelope.EnsureOK]
// ⇒ `"0"`、`"0000"`、数字 `0` 三档都算成功。为什么这一侧更安全，见
// `app_cloud_outlink.go` 文件头那一段（少删一次外链留下的是**公开链接**）。
// 反向改坏：把这里改成字面相等 ⇒ `"0000"` 那一档红。
func TestParseCloudOutLinkAcceptsBothZeroSpellings(t *testing.T) {
	for _, code := range []string{`"0"`, `"0000"`, `0`} {
		envelope := envelopeOf(t, `{"code":`+code+`,"data":{"getOutLinkRes":{
			"getOutLinkResSet":[{"linkID":"SYNTH-LINK-ID"}]}}}`)
		got, perr := ParseCloudOutLink(envelope)
		if perr != nil {
			t.Errorf("code=%s 应判成功，实得 %v", code, perr.Message())
			continue
		}
		if got.LinkID == nil {
			t.Errorf("code=%s 应读出 linkID", code)
		}
	}
}

// TestParseCloudOutLinkRejectsNonZeroCode 钉"上游说没成 ⇒ 错误"，不看载荷。
func TestParseCloudOutLinkRejectsNonZeroCode(t *testing.T) {
	envelope := envelopeOf(t, `{"code":"9470","message":"SYNTH-MSG","data":null}`)
	if _, perr := ParseCloudOutLink(envelope); perr == nil {
		t.Fatal("非 0 码应是错误")
	}
}

// TestParseCloudOutLinkShapeCases ⭐ 三档"没拿到 linkID"要能分开。
//
//	非 0 码            → 错误（上游说没成）
//	data 不是对象      → 错误（这一族的形状变了）
//	没有 getOutLinkRes → 错误（PC 客户端那一版多一层 result 包装，读法不同）
//	set 为空 / 缺席     → 不是错误，但 LinkID 为空 ⇒ 上层必须播成失败
func TestParseCloudOutLinkShapeCases(t *testing.T) {
	cases := []struct {
		name     string
		body     string
		wantErr  bool
		wantLink bool
	}{
		{"载荷整块缺失", `{"code":"0"}`, true, false},
		{"载荷不是对象", `{"code":"0","data":"SYNTH"}`, true, false},
		{"没有 getOutLinkRes", `{"code":"0","data":{"other":1}}`, true, false},
		{"PC 那一版的 result 包装", `{"code":"S_OK","data":{"result":{"getOutLinkRes":{
			"getOutLinkResSet":{"getOutLinkResOne":{"linkID":"SYNTH"}}}}}}`, true, false},
		{"set 是空数组", `{"code":"0","data":{"getOutLinkRes":{"getOutLinkResSet":[]}}}`, false, false},
		{"set 缺席", `{"code":"0","data":{"getOutLinkRes":{}}}`, false, false},
		{"第一项没有 linkID", `{"code":"0","data":{"getOutLinkRes":{
			"getOutLinkResSet":[{"linkURL":"https://synth.example.invalid"}]}}}`, false, false},
		{"多条只读第一条", `{"code":"0","data":{"getOutLinkRes":{"getOutLinkResSet":[
			{"linkID":"SYNTH-A"},{"linkID":"SYNTH-B"}]}}}`, false, true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, perr := ParseCloudOutLink(envelopeOf(t, tc.body))
			if (perr != nil) != tc.wantErr {
				t.Fatalf("错误与否：实得 %v（%v），期望 %v",
					perr == nil, perr, tc.wantErr)
			}
			if perr != nil {
				return
			}
			if (got.LinkID != nil) != tc.wantLink {
				t.Errorf("linkID 有无：实得 %v，期望有的=%v", got.LinkID, tc.wantLink)
			}
		})
	}
}

// TestParseCloudDelOutLinkIgnoresPayloadButHonoursFailure 钉"不看字段，但看成败"。
//
// 旧实现**完全不看**这一口的响应（那句 `tryRequest` 的返回值被丢弃）⇒ 一次失败的
// 取消分享在旧实现里是静默的。这里不复制那份静默：非 0 码与 `success:false` 都回错误。
func TestParseCloudDelOutLinkIgnoresPayloadButHonoursFailure(t *testing.T) {
	if perr := ParseCloudDelOutLink(envelopeOf(t, `{"code":"0000","success":true}`)); perr != nil {
		t.Errorf("成功不该要求任何字段，实得 %v", perr.Message())
	}
	if perr := ParseCloudDelOutLink(envelopeOf(t, `{"success":false,"message":"SYNTH"}`)); perr == nil {
		t.Error("success:false 应是错误（旧实现把这一档念成成功）")
	}
	if perr := ParseCloudDelOutLink(envelopeOf(t, `{"code":"200000768","message":"SYNTH"}`)); perr == nil {
		t.Error("非 0 码应是错误")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// TestAppCloudOutLinkSendsOneRequestEach 钉两口各自"恰好一发 + 走信封解析"。
func TestAppCloudOutLinkSendsOneRequestEach(t *testing.T) {
	get := &scriptedClient{response: core.HTTPResponse{Status: 200, Body: jsonOf(t, map[string]any{
		"code": "0", "data": map[string]any{"getOutLinkRes": map[string]any{
			"getOutLinkResSet": []any{map[string]any{"linkID": outlinkSynthLinkID}}}}})}}
	got, perr := NewUpstream(get).AppCloudGetOutLink(context.Background(),
		outlinkAuthSentinel, outlinkDeviceName, outlinkInput())
	if perr != nil {
		t.Fatalf("发送层解析失败: %v", perr.Message())
	}
	if got.LinkID == nil || *got.LinkID != outlinkSynthLinkID {
		t.Errorf("linkID 实得 %v", got.LinkID)
	}
	if len(get.requests) != 1 || !strings.HasSuffix(get.requests[0].URL, "/getOutLink") {
		t.Fatalf("应恰好一发 getOutLink，实得 %+v", get.requests)
	}

	del := &scriptedClient{response: core.HTTPResponse{Status: 200, Body: []byte(`{"code":"0"}`)}}
	if perr := NewUpstream(del).AppCloudDelOutLink(context.Background(),
		outlinkAuthSentinel, outlinkDeviceName, outlinkSynthPhone, []string{outlinkSynthLinkID}); perr != nil {
		t.Fatalf("delOutLink 应成功，实得 %v", perr.Message())
	}
	if len(del.requests) != 1 || !strings.HasSuffix(del.requests[0].URL, "/delOutLink") {
		t.Fatalf("应恰好一发 delOutLink，实得 %+v", del.requests)
	}
}

// TestAppCloudOutLinkStatusErrorIsNotParsedAsSuccess 钉 404 那一档走状态码而不是"响应不是 JSON"。
//
// 这一条是 2026-09-25 那次真跑（ai-avatar 整族撞 404/405）留下的教训在新一族的落实：
// 见 [errorFromStatus] 旁边那条"404 必须显式分支"。
func TestAppCloudOutLinkStatusErrorIsNotParsedAsSuccess(t *testing.T) {
	client := &scriptedClient{response: core.HTTPResponse{Status: 404, Body: []byte("<html>not found</html>")}}
	_, perr := NewUpstream(client).AppCloudGetOutLink(context.Background(),
		outlinkAuthSentinel, outlinkDeviceName, outlinkInput())
	if perr == nil {
		t.Fatal("404 应是错误")
	}
	if !strings.Contains(perr.Message(), "HTTP 404") {
		t.Errorf("错误里应有状态码那一格，实得 %q", perr.Message())
	}
	if strings.Contains(perr.Message(), "not found<") {
		t.Error("错误文本里不该带上游响应体原文")
	}
}

// jsonOf 序列化测试夹具（写错 JSON 时编译期就能发现一半）。
func jsonOf(t *testing.T, value any) []byte {
	t.Helper()
	body, err := json.Marshal(value)
	if err != nil {
		t.Fatalf("夹具序列化失败: %v", err)
	}
	return body
}
