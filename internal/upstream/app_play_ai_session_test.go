package upstream

// 拍拍三样前置（#16）的用例：换票两步、`visitlog/journaling` 埋点、`chat/v2/add` 请求。
//
// # 本层是**纯**测试：只钉两件事
//
// ① 我们**发出**的这一组恰好是这个形状（URL / 方法 / 头集合 / body 字节）；
// ② 给一段**声明里那个形状**的响应体，本层怎么处置它。
// 不联网、不起 server、不碰 data 目录，也不打任何真实上游。
//
// # ⚠ 这一页对三样前置**零实拍**，所以测试的措辞受限
//
// 夹具 `test/fixtures/http/夏日拍拍大作战.json` 的 13 条里没有换票、没有埋点、没有 chat
// （唯一相关的一条 A 档：**同一个** `journaling` URL 在直播小红花族实拍过，见下面
// `…SameEndpointAsTheCapturedLiveRoomOne`）。于是：
//
//   - 每条测试的**名字与注释**都在说"我们发的是什么"，没有一条在说"上游要求什么"；
//   - 判据类断言（两个 `Parse*`）钉的是"我们怎么处置这一段字节"，
//     非成功码**一个实拍都没有** ⇒ 只钉"把码与原文带出来、不折成成功"（AGENTS.md §1.7）；
//   - chat 那条**没有响应侧断言**：响应是 SSE 且零样本，本层不解析，
//     所以这里只按方法名扫住"别偷偷加一个发送方法"（见最后一条）。
//
// # 隐私
//
// 手机号/`userId` 一律 `199…` 合成号；票、`Authorization`、`x-yun-tid` 都是看得见是假的的合成值。
// 实拍里那几枚头值已脱敏 ⇒ 没有任何真实凭据进本文件。

import (
	"context"
	"encoding/json"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

// playAISessionTestReferer 是**合成** referer：本页实拍的 `referer` 头已脱敏，
// 所以换票两步注入的是这一个值（`PlayAIPageReferer()` 那一串只在埋点那条里出现）。
const playAISessionTestReferer = "https://referer.example/synthetic-play-ai"

// playAISessionTestSign 是一组**写死的**签名四件套：本层不算签名（见文件头），
// 用可预期的假值才能钉住"四枚头 = 传进来的那四格，逐字、按序"。
func playAISessionTestSign() H5Signature {
	return H5Signature{
		RequestID: "REQ-SYNTHETIC", Timestamp: "1789800128813",
		Nonce: "NONCE-SYNTHETIC", Signature: "SIG-SYNTHETIC",
	}
}

// playAISessionTestChatParams 给一份五格齐全的 chat 入参（`userId` 是合成号，
// `name` 用旧 TS 那一种 19 位形态，专门用来验"本层不做加减法"）。
func playAISessionTestChatParams() PlayAIChatParams {
	return PlayAIChatParams{
		UserID:    "1990000000000000001",
		Dialogue:  "人物戴眼镜了吗",
		InputTime: PlayAIChatInputTime(playAISessionTestWhen()),
		FileID:    "SYNTHETIC-FILE-ID",
		FileName:  "1990000000000000005.jpeg",
	}
}

// playAISessionTestWhen 是一个固定时刻（毫秒位非零、且不足一位毫秒时补零的那种）。
func playAISessionTestWhen() time.Time {
	return time.Date(2026, 6, 29, 12, 10, 43, 123000000, time.UTC)
}

// playAISessionTestBody 把 body 字节转成字符串，顺手挡掉空 body。
func playAISessionTestBody(t *testing.T, name string, request core.HTTPRequest) string {
	t.Helper()
	if len(request.Body) == 0 {
		t.Fatalf("%s 应有 body，实得空", name)
	}
	return string(request.Body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 甲·换票 ①：GET …/ycloud/api/cloud/userdomain/v2/querySpecToken
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIQuerySpecTokenRequestIsTheTwoKeyGetVerbatim 钉第①步的 URL 逐字。
//
// 反向改坏它谁会红：把两个查询键换顺序、把 `v2` 挪位、把 `hideLoading` 参数化掉、
// 或者"顺手"复用 ①…④ 那个 `playAIQuery` 之外的拼法 —— 都红在本条。
// 三种改法在真跑时的表现分别是 404 / 404 / 上游回一句看不懂的话，都不会指向我们动过的字面量。
func TestPlayAIQuerySpecTokenRequestIsTheTwoKeyGetVerbatim(t *testing.T) {
	request := PlayAIQuerySpecTokenRequest(playAISessionTestSign(), playAISessionTestReferer, "true")
	if request.Method != core.MethodGet {
		t.Errorf("第①步应是 GET（旧 TS `auth.ts:66` 用的是 `http.$get`），实得 %s", request.Method)
	}
	want := "https://m.mcloud.139.com/ycloud/api/cloud/userdomain/v2/querySpecToken" +
		"?targetSourceId=001005&hideLoading=true"
	if request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("这条不该有 body，实得 %q", string(request.Body))
	}
	// 值没有被 percent-encode 改过形（这一族的查询串走 [playAIQuery]，编码后逐字节不变）。
	if strings.Contains(request.URL, "%") {
		t.Errorf("这两个值都不该被编码变形，实得 %q", request.URL)
	}
}

// TestPlayAIQuerySpecTokenHeadersAreTheSigninFamilySetWithoutJWTToken 钉第①步**发出的头集合**。
//
// # 这一处**偏离** ①…④ 那一族，是刻意的
//
// ①…④ 按夹具裁成"少 `activityid`/`appversion`/`deviceid` 三头"（见
// `TestPlayAIUserStatusRequestIsTheCapturedBareGet`），而这一条**带**前两个 ——
// 因为这一格取的是**同端点已实跑通**的那一份（`tyrzLogin`，C 档），不是这一页的样本（零）。
// 反过来"删掉这两个头会不会也过"⇒ **未测试**。钉的是我们的取向，不是上游的要求。
//
// 反向改坏它谁会红：改用 [playAIH5Headers]（会多一枚 `jwttoken`、少两枚签名头）⇒ 本条红；
// 改用 [WithH5Headers] 但忘了补 `Origin` ⇒ 本条红。
func TestPlayAIQuerySpecTokenHeadersAreTheSigninFamilySetWithoutJWTToken(t *testing.T) {
	request := PlayAIQuerySpecTokenRequest(playAISessionTestSign(), playAISessionTestReferer, "true")
	wantNames := "accept,activityid,appversion,origin,referer,showloading," +
		"x-nonce,x-request-id,x-signature,x-timestamp"
	if got := playAITestHeaderNames(request); got != wantNames {
		t.Errorf("我们发出的头应恰好是 [%s]，实得 [%s]", wantNames, got)
	}
	// 这一步的**目的**就是拿票 ⇒ 没有票可带。带上了就是发出一条出处里不存在的请求。
	if n := countHeader(request.Headers, "jwttoken"); n != 0 {
		t.Errorf("第①步不该带 jwttoken（票还没拿到），实得 %d 个", n)
	}
	if n := countHeader(request.Headers, "content-type"); n != 0 {
		t.Errorf("GET 无 body ⇒ 不该有 content-type，实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "Referer"); got != playAISessionTestReferer {
		t.Errorf("referer 应是注入的那一串，实得 %q", got)
	}
	if got := headerValue(request.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 应原样带回注入值，实得 %q", got)
	}
	if got := headerValue(request.Headers, "activityid"); got != MarketName {
		t.Errorf("本条沿用签到族那一份头集合（activityid=%s）；这一格**不是**本页实拍", got)
	}
}

// TestPlayAIQuerySpecTokenCarriesTheInjectedSignatureVerbatim 钉签名四枚头 = 传进来的那四格。
//
// 关键点是"本层**不自己算**签名"：四件套由调用方（复用 [SignH5RequestWith]）算好后注入，
// 所以这里的值是可预期的假值，而不是 `md5(...)` 的结果。
// 反向改坏它谁会红：在 `PlayAIQuerySpecTokenRequest` 里加一句现算签名（第二份实现）⇒ 本条红，
// 而且下面那条"两步同一组签名值"也会红（现算会把两步拆成两个签名）。
func TestPlayAIQuerySpecTokenCarriesTheInjectedSignatureVerbatim(t *testing.T) {
	sign := playAISessionTestSign()
	request := PlayAIQuerySpecTokenRequest(sign, playAISessionTestReferer, "true")
	want := [][2]string{
		{"x-request-id", sign.RequestID},
		{"x-timestamp", sign.Timestamp},
		{"x-nonce", sign.Nonce},
		{"x-signature", sign.Signature},
	}
	for i, pair := range want {
		if got := headerValue(request.Headers, pair[0]); got != pair[1] {
			t.Errorf("第 %d 枚签名头 %s 应逐字为 %q，实得 %q", i, pair[0], pair[1], got)
		}
		if n := countHeader(request.Headers, pair[0]); n != 1 {
			t.Errorf("%s 应恰好一枚，实得 %d", pair[0], n)
		}
	}
	// 与**同端点已实跑通**的 `tyrzLogin` 那条共用一份头名表：两条是**同一条链**，
	// 对拍时要能看出"只差 body 与一枚 type 头"。
	step2, perr := PlayAITyrzLoginRequest(sign, playAISessionTestReferer, "SSO-SYNTHETIC", "true")
	if perr != nil {
		t.Fatalf("第②步构造失败: %v", perr)
	}
	for _, pair := range want {
		if headerValue(request.Headers, pair[0]) != headerValue(step2.Headers, pair[0]) {
			t.Errorf("两步的 %s 应同源同值（一次签名走两步），实得 %q / %q",
				pair[0], headerValue(request.Headers, pair[0]), headerValue(step2.Headers, pair[0]))
		}
	}
}

// TestPlayAISignatureReusesTheLegacySaltNumberAndOrder 把"盐值与拼法逐字比对"的**结论**跑起来。
//
// 比对的原文写在实现文件头（旧 TS `auth/sign.ts:3-6/:19` vs 本仓 `core/crypto.go:65-68/:675-689`）：
// 盐号同为 `2`、盐值同字、拼接顺序同为 `salt+id+ts+nonce+other+salt`、H5 那段 `other` 同为空串。
// 这里不重复写盐值字面量（那等于抄第二份），只钉"用的是同一枚常量、同一个算式、同一次拼接"。
//
// 反向改坏它谁会红：把 [H5SignSaltKey] 改成 `1` ⇒ 第一句红；
// 把 [core.GenerateSignature] 里 nonce 与 timestamp 对调 ⇒ 第二句红。
func TestPlayAISignatureReusesTheLegacySaltNumberAndOrder(t *testing.T) {
	if core.SignSalt(H5SignSaltKey) != core.SignSalt(2) {
		t.Errorf("H5 签名的盐号应是旧 TS 的 SALT_MAP[2]，实得编号 %d 的盐", H5SignSaltKey)
	}
	if core.SignSalt(H5SignSaltKey) == core.SignSalt(1) {
		t.Error("盐号 1 与 2 是两个不同的盐；相等说明映射被动过")
	}
	sign := playAISessionTestSign()
	want := core.MD5Hex([]byte(core.SignSalt(H5SignSaltKey) +
		sign.RequestID + sign.Timestamp + sign.Nonce + "" + core.SignSalt(H5SignSaltKey)))
	if got := SignH5RequestWith(sign.RequestID, sign.Timestamp, sign.Nonce).Signature; got != want {
		t.Errorf("拼法应是 md5(盐+id+ts+nonce+other+盐)，实得 %q 期望 %q", got, want)
	}
}

// TestPlayAIQuerySpecTokenIsNotTheNativeQuerySpecTokenV2 防"顺手统一"。
//
// 本仓**已有** [QuerySpecTokenRequest]（原生侧 `user-njs.yun.139.com/user/querySpecTokenV2`，
// POST + body + A 族头 + 要 Authorization）。这一页要的是**另一条**（H5 门户、GET、查询串、
// 签名四件套、不要 Authorization）。主机/方法/body/鉴权头**四项全不同** ——
// 把它们并成一个函数，就会有一次"两个端点里有一个悄悄不发出去了"。
func TestPlayAIQuerySpecTokenIsNotTheNativeQuerySpecTokenV2(t *testing.T) {
	ours := PlayAIQuerySpecTokenRequest(playAISessionTestSign(), playAISessionTestReferer, "true")
	theirs, perr := QuerySpecTokenRequest(playAITestProfile(), 1789800128813000,
		"Basic U1lOVEhFVElDLUFVVEg=", map[string]any{"toSourceId": ToSourceID})
	if perr != nil {
		t.Fatalf("既有那条构造失败: %v", perr)
	}
	if ours.URL == theirs.URL {
		t.Errorf("两条端点不该同 URL： ours=%q theirs=%q", ours.URL, theirs.URL)
	}
	if ours.Method == theirs.Method {
		t.Errorf("一条是 GET、一条是 POST，实得同为 %s", ours.Method)
	}
	if len(ours.Body) != 0 || len(theirs.Body) == 0 {
		t.Errorf("第①步无 body、原生那条有 body，实得 %d / %d 字节", len(ours.Body), len(theirs.Body))
	}
	if n := countHeader(ours.Headers, "Authorization"); n != 0 {
		t.Errorf("第①步不带 Authorization（H5 侧靠签名），实得 %d 个", n)
	}
	if n := countHeader(theirs.Headers, "Authorization"); n != 1 {
		t.Errorf("原生那条应带 Authorization，实得 %d 个", n)
	}
	for _, name := range []string{"x-request-id", "x-signature"} {
		if countHeader(ours.Headers, name) != 1 {
			t.Errorf("第①步应带 %s", name)
		}
		if countHeader(theirs.Headers, name) != 0 {
			t.Errorf("原生那条不该带 %s（两套签名机制）", name)
		}
	}
}

// TestPlayAISessionConstantsAreNotMergedWithTheSigninOnes 钉住"同值不等于同一格"。
//
// 三个容易合并的口子都在本文件里：`targetSourceId`（与 [ToSourceID] 同值）、
// `marketName`（与 [PlayAIMarketName] **不同**值）、`sourceId`（与 [SourceID] 不同值）。
// 合并的后果不是编译错，是"下一期只改一个值时静默错"。
func TestPlayAISessionConstantsAreNotMergedWithTheSigninOnes(t *testing.T) {
	if PlayAISpecTokenTargetSourceID != ToSourceID {
		t.Errorf("这两格当前同值（%s/%s），但它们是**两个参数**：合并前请先看两处注释",
			PlayAISpecTokenTargetSourceID, ToSourceID)
	}
	if PlayAITyrzLoginMarketName == PlayAIMarketName {
		t.Errorf("换票的 marketName 不能等于活动接口那一个（%q）—— 两个值、两处出现", PlayAIMarketName)
	}
	if PlayAITyrzLoginMarketName != "shake_shake" || PlayAITyrzLoginSourceID != "1314" {
		t.Errorf("第②步那两个值应逐字为 shake_shake/1314（旧 TS `auth.ts:90-91`），实得 %q/%q",
			PlayAITyrzLoginMarketName, PlayAITyrzLoginSourceID)
	}
	if PlayAITyrzLoginSourceID == SourceID {
		t.Errorf("换票的 sourceId 与签到那一个（%s）不同值，别统一", SourceID)
	}
	if PlayAIAuthType != "signupPlatform" {
		t.Errorf("type 那一格应逐字 signupPlatform（旧 TS body 与头两处同字面量），实得 %q", PlayAIAuthType)
	}
	if PlayAIChatSourceChannel != PlayAIRecognizeChannelID {
		t.Errorf("chat 的 sourceChannel 与 ⑤ 的 channelId 当前同值（都是 101）但是两口的两格：" +
			"⑤ 有实拍、这一条没有，合并后换期只改得动一处")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 甲·换票 ②：POST …/ycloud/auth-service/auth/tyrzLogin（这一页的参数变体）
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAITyrzLoginBodyHasExactlySixKeys 钉第②步 body 的**字节**。
//
// 六个键、`openAccount` 是**布尔 false**、`channel` 是 **null**、键序是 Go 序列化器给的字母序。
// 反向改坏它谁会红：把 `openAccount` 折成数字 `0`（签到族那一份就是 `0`）⇒ 本条红 ——
// 而这一格恰恰是"两种形态都没有本页样本"的那一个，红得有道理：逼动手的人去看注释。
func TestPlayAITyrzLoginBodyHasExactlySixKeys(t *testing.T) {
	body, err := core.MarshalJSON(PlayAITyrzLoginBody("SSO-SYNTHETIC"))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	want := `{"channel":null,"marketName":"shake_shake","openAccount":false,` +
		`"sourceId":"1314","token":"SSO-SYNTHETIC","type":"signupPlatform"}`
	if string(body) != want {
		t.Errorf("body 应逐字为 %q，实得 %q", want, string(body))
	}
	if len(body) != 129 {
		t.Errorf("body 长度登记为 129 字节，实得 %d", len(body))
	}
	var decoded map[string]any
	if err := json.Unmarshal(body, &decoded); err != nil {
		t.Fatalf("body 不是合法 JSON: %v", err)
	}
	if len(decoded) != 6 {
		t.Errorf("应恰好六个键，实得 %d：%v", len(decoded), decoded)
	}
	if _, ok := decoded["openAccount"].(bool); !ok {
		t.Errorf("openAccount 应是布尔 false（这一页的出处如此），实得 %T", decoded["openAccount"])
	}
	if v, exists := decoded["channel"]; !exists || v != nil {
		t.Errorf("channel 这一格必须是 null，实得 %#v", v)
	}
	// 第①步的产物只能出现在 token 这一格里（别把它塞进 URL 或头：那是第二处凭据出口）。
	if tok, _ := decoded["token"].(string); tok != "SSO-SYNTHETIC" {
		t.Errorf("token 应是第①步那一个值，实得 %q", tok)
	}
}

// TestPlayAITyrzLoginBodyDiffersFromSigninBodyPerKey 把"同一端点、不同参数"落成断言。
//
// §1.12 事实②那句"URL 可能相同而参数不同"的现场：两条 URL 相同、body 五处不同。
// 反向改坏它谁会红：有人"复用签到那一份 body"（少两键、三个值不同）⇒ 本条红，
// 而请求看着完全正常（上游认不认 ⇒ 未测试）。
func TestPlayAITyrzLoginBodyDiffersFromSigninBodyPerKey(t *testing.T) {
	ours := PlayAITyrzLoginBody("SSO-SYNTHETIC")
	theirs := TyrzLoginBody("SSO-SYNTHETIC")
	if len(ours) != 6 || len(theirs) != 4 {
		t.Fatalf("两条 body 的键数应分别为 6 / 4，实得 %d / %d", len(ours), len(theirs))
	}
	for _, key := range []string{"type", "channel"} {
		if _, ok := ours[key]; !ok {
			t.Errorf("这一页的 body 应有 %s（旧 TS `auth.ts:92-94`）", key)
		}
		if _, ok := theirs[key]; ok {
			t.Errorf("签到那一条不该有 %s（它没有这一格）", key)
		}
	}
	if ours["marketName"] == theirs["marketName"] || ours["sourceId"] == theirs["sourceId"] {
		t.Error("marketName / sourceId 两条各取各的值，相等说明有人统一了参数")
	}
	if _, ok := ours["openAccount"].(bool); !ok {
		t.Error("这一页的 openAccount 是布尔")
	}
	if _, ok := theirs["openAccount"].(float64); !ok {
		if _, isInt := theirs["openAccount"].(int); !isInt {
			t.Errorf("签到那一条的 openAccount 是**数字**（实拍），实得 %T", theirs["openAccount"])
		}
	}
	if ours["token"] != theirs["token"] {
		t.Error("只有 token 这一格两义相同（都是上一Step 换来的 SSO token）")
	}
}

// TestPlayAITyrzLoginRequestReusesTheSigninRequestAndAddsTypeHeader 钉第②步"不复制第二份实现"。
//
// URL / 方法 / 签名四件套 / `Origin` 全部**继承**自 [TyrzLoginRequest]；差别只有两处处：
// body 与多出来的 `type` 头。反向改坏它谁会红：在这里重写一遍头组装（少一枚或多一枚）⇒
// 下面那条"头名恰好这一组"红。
//
// ⚠ `type` 这枚头**没有实拍**：旧 TS 在 body 与头两处都写了同一个串，只发一半是**我们自己**
// 造的新形态。这里两条一起钉，钉的是"我们两处都按出处发"，不是"上游要看这枚头"。
func TestPlayAITyrzLoginRequestReusesTheSigninRequestAndAddsTypeHeader(t *testing.T) {
	sign := playAISessionTestSign()
	ours, perr := PlayAITyrzLoginRequest(sign, playAISessionTestReferer, "SSO-SYNTHETIC", "true")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	theirs, err := TyrzLoginRequest(sign, playAISessionTestReferer, TyrzLoginBody("SSO-SYNTHETIC"))
	if err != nil {
		t.Fatalf("既有那条构造失败: %v", err)
	}
	if ours.URL != theirs.URL || ours.Method != theirs.Method {
		t.Errorf("同一端点不该有两份 URL：ours=%q theirs=%q", ours.URL, theirs.URL)
	}
	if want := "https://m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin"; ours.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, ours.URL)
	}
	if got := playAISessionTestBody(t, "第②步", ours); got == string(theirs.Body) {
		t.Error("两条 body 不该逐字相同（这一页是六个键）")
	}
	wantNames := "accept,activityid,appversion,content-type,origin,referer,showloading,type," +
		"x-nonce,x-request-id,x-signature,x-timestamp"
	if got := playAITestHeaderNames(ours); got != wantNames {
		t.Errorf("我们发出的头应恰好是 [%s]，实得 [%s]", wantNames, got)
	}
	if got := headerValue(ours.Headers, "type"); got != PlayAIAuthType {
		t.Errorf("type 头应与 body 的 type 同字（旧 TS 两处复用同一串），实得 %q", got)
	}
	if n := countHeader(ours.Headers, "jwttoken"); n != 0 {
		t.Errorf("这一步正是**为了**拿票，不该带 jwttoken，实得 %d 个", n)
	}
}

// TestPlayAITyrzLoginRequestShowloadingIsReplacedNotDuplicated 钉"先剥后补"。
//
// [TyrzLoginRequest] 内部把 `showloading` 钉成 `"true"`；这一页的 `hideLoading=true` 说明它想关蒙层。
// 反向改坏它谁会红：把那句 `dropHeader` 删掉 ⇒ 发出**两枚**同名头，
// 上游按第一枚解析（本仓库实测过的坑），而"我们想关蒙层"这件事静默失效。
func TestPlayAITyrzLoginRequestShowloadingIsReplacedNotDuplicated(t *testing.T) {
	sign := playAISessionTestSign()
	for _, tc := range []struct{ in, want string }{
		{"false", "false"}, {"true", "true"}, {"", "true"},
	} {
		ours, perr := PlayAITyrzLoginRequest(sign, playAISessionTestReferer, "SSO-SYNTHETIC", tc.in)
		if perr != nil {
			t.Fatalf("showloading=%q 构造失败: %v", tc.in, perr)
		}
		if n := countHeader(ours.Headers, "showloading"); n != 1 {
			t.Errorf("showloading=%q 时应恰好一枚，实得 %d（两枚会被上游按第一枚解析）", tc.in, n)
		}
		if got := headerValue(ours.Headers, "showloading"); got != tc.want {
			t.Errorf("showloading=%q 应发出 %q，实得 %q", tc.in, tc.want, got)
		}
	}
}

// TestParsePlayAIQuerySpecTokenWantsABareStringResult 钉第①步的取值口径。
//
// 旧 TS 的声明是 `result: string`（`auth.ts:6-10`）—— **裸字符串**，不是对象。
// 三档失败各钉一条：没有 result、result 是对象、result 是空串。
// 后两条的理由不是"上游会给这种形状"（零样本），而是"我们**不替**上游兼容它没给过的形状"：
// 顺手加一句 `if obj, ok := result.(map[string]any)` 就等于宣布"它哪天改成对象我们也读得懂"。
func TestParsePlayAIQuerySpecTokenWantsABareStringResult(t *testing.T) {
	got, perr := ParsePlayAIQuerySpecToken(
		playAITestEnvelope(t, `{"code":0,"msg":"success","result":"SSO-SYNTHETIC"}`))
	if perr != nil {
		t.Fatalf("成功档不应报错: %v", perr)
	}
	if got != "SSO-SYNTHETIC" {
		t.Errorf("应逐字交出 result，实得 %q", got)
	}
	cases := []struct{ name, body, wantFragment string }{
		{"无 result", `{"code":0,"msg":"success"}`, "没有 result"},
		{"result 是对象", `{"code":0,"msg":"success","result":{"token":"SSO-SYNTHETIC"}}`, "不是字符串"},
		{"result 是空串", `{"code":0,"msg":"success","result":""}`, "空串"},
		{"result 是数字", `{"code":0,"msg":"success","result":123}`, "不是字符串"},
	}
	for _, tc := range cases {
		_, perr := ParsePlayAIQuerySpecToken(playAITestEnvelope(t, tc.body))
		msg := playAITestErrMessage(t, tc.name, perr)
		if !strings.Contains(msg, tc.wantFragment) {
			t.Errorf("%s 的报错应含 %q，实得 %q", tc.name, tc.wantFragment, msg)
		}
	}
}

// TestParsePlayAIQuerySpecTokenRejectsTheAIEnvelopeShape 是那条**跨族反向**用例。
//
// 本层第①步的判据复用 [playAINewyearPayload]（`code` 必须是**数字字面量**），而 ⑤ 那一族
// 的 `code` 是字符串 `"0000"`。两族信封不同构 ⇒ 混喂必须当场响。
// 反向改坏它谁会红：把判据换成公共的 [Envelope.EnsureOK]（数字与字符串都收）⇒ 本条红，
// 同时"把 ⑤ 的响应喂给换票解析器"变成一条静默通路。
func TestParsePlayAIQuerySpecTokenRejectsTheAIEnvelopeShape(t *testing.T) {
	aiShape := `{"code":"0000","message":"请求成功","success":true,"data":{"fileId":"SYNTHETIC-FILE-ID"}}`
	msg := playAITestErrMessage(t, "AI 族信封", func() *core.Error {
		_, perr := ParsePlayAIQuerySpecToken(playAITestEnvelope(t, aiShape))
		return perr
	}())
	if !strings.Contains(msg, "数字") {
		t.Errorf("应说清是 code 类型不对（两族不同构），实得 %q", msg)
	}
	// 非成功码不许折成成功：把码与上游 msg 原文带出来。
	msg = playAITestErrMessage(t, "非零码", func() *core.Error {
		_, perr := ParsePlayAIQuerySpecToken(playAITestEnvelope(t, `{"code":500,"msg":"SYNTHETIC-UPSTREAM-TEXT"}`))
		return perr
	}())
	if !strings.Contains(msg, "500") || !strings.Contains(msg, "SYNTHETIC-UPSTREAM-TEXT") {
		t.Errorf("应把码与上游原文带出来，实得 %q", msg)
	}
}

// TestParsePlayAITyrzLoginReadsTokenAndTwoOptionalStrings 钉第②步的产物。
//
// 三格里只有 `token` 是必需；`mobile`/`userDomainId` **可能缺失**（这一页零实拍，
// "缺了就是坏响应"我们没资格说）⇒ 指针，且只认字符串形态。
// 反向改坏它谁会红：把某个可选格改成非指针 + 缺失即报错 ⇒ 本条红。
func TestParsePlayAITyrzLoginReadsTokenAndTwoOptionalStrings(t *testing.T) {
	session, perr := ParsePlayAITyrzLogin(playAITestEnvelope(t,
		`{"code":0,"msg":"success","result":{"token":"JWT-SYNTHETIC",`+
			`"mobile":"1990000000000000002","userDomainId":"1990000000000000001"}}`))
	if perr != nil {
		t.Fatalf("成功档不应报错: %v", perr)
	}
	if session.JWTToken != "JWT-SYNTHETIC" {
		t.Errorf("JWTToken 应是 result.token，实得 %q", session.JWTToken)
	}
	if playAITestStrOrEmpty(session.Mobile) != "1990000000000000002" {
		t.Errorf("mobile 应带出来（确认这张票签给谁），实得 %q", playAITestStrOrEmpty(session.Mobile))
	}
	if playAITestStrOrEmpty(session.UserDomainID) != "1990000000000000001" {
		t.Errorf("userDomainId 应带出来（⑤ 的 userId 候选来源），实得 %q",
			playAITestStrOrEmpty(session.UserDomainID))
	}
	// 只有 token：两格缺席 ⇒ 成功，且都是 nil（不报错、不猜）。
	lean, perr := ParsePlayAITyrzLogin(playAITestEnvelope(t,
		`{"code":0,"msg":"success","result":{"token":"JWT-SYNTHETIC"}}`))
	if perr != nil {
		t.Fatalf("两个可选格缺席不该报错: %v", perr)
	}
	if lean.Mobile != nil || lean.UserDomainID != nil {
		t.Errorf("缺席应是 nil，实得 %v / %v", lean.Mobile, lean.UserDomainID)
	}
	// 非字符串形态（旧 TS 那份声明里 `userDomainId` 是 string；哪天回数字就是**另一种**响应）
	// ⇒ nil，不转换：转换等于替上游发明一条它没写过的口径。
	tricky, perr := ParsePlayAITyrzLogin(playAITestEnvelope(t,
		`{"code":0,"msg":"success","result":{"token":"JWT-SYNTHETIC","userDomainId":123,"mobile":null}}`))
	if perr != nil {
		t.Fatalf("可选格形态不认识不该报错: %v", perr)
	}
	if tricky.UserDomainID != nil || tricky.Mobile != nil {
		t.Errorf("非字符串形态应留 nil（不猜不转换），实得 %v / %v", tricky.UserDomainID, tricky.Mobile)
	}
	for _, tc := range []struct{ name, body string }{
		{"没有 result", `{"code":0,"msg":"success"}`},
		{"result 是字符串", `{"code":0,"msg":"success","result":"JWT-SYNTHETIC"}`},
		{"缺 token", `{"code":0,"msg":"success","result":{"mobile":"1990000000000000002"}}`},
	} {
		_, perr := ParsePlayAITyrzLogin(playAITestEnvelope(t, tc.body))
		if perr == nil {
			t.Errorf("%s 应报错", tc.name)
		}
	}
}

// TestParsePlayAITyrzLoginNotLoggedInIsStillTypedAsSessionExpired 钉"第②步的失败档走公共判据"。
//
// 判据用 [EnsureOKWithHint]（与 [ParseJWTToken] 同一个）而不是 [playAINewyearPayload]，
// 为的就是 `90001` 能被上层**类型化**认出来。⚠ 变体**不改** kind，所以这里不能靠 kind 区分：
// 钉的是 `IsSessionExpired()` + kind 仍为 bad_request + 提示串在正文里。
// 反向改坏它谁会红：把这一条的判据换成 `playAINewyearPayload` ⇒ `IsSessionExpired` 那一句红
// （`90001` 会落成一句没有变体的业务错，app 层就不再刷新重放）。
func TestParsePlayAITyrzLoginNotLoggedInIsStillTypedAsSessionExpired(t *testing.T) {
	_, perr := ParsePlayAITyrzLogin(playAITestEnvelope(t,
		`{"code":90001,"msg":"未登录"}`))
	if perr == nil {
		t.Fatal("90001 应判失败")
	}
	if !perr.IsSessionExpired() {
		t.Errorf("90001 必须带会话过期这个变体（换票报未登录听起来怪，但判据来自公共表）: %v", perr)
	}
	if perr.Kind() != core.KindBadRequest {
		t.Errorf("kind 应仍是 bad_request（否则对外文案跟着变）: %v", perr.Kind())
	}
	if !strings.Contains(perr.Message(), "会话没建起来") {
		t.Errorf("Diagnose 的提示应在正文里，实得 %q", perr.Message())
	}
	// 其他码**不该**被判成会话过期 —— 否则一次普通的业务失败会被刷票重放。
	_, other := ParsePlayAITyrzLogin(playAITestEnvelope(t, `{"code":500,"msg":"SYNTHETIC"}`))
	if other == nil || other.IsSessionExpired() {
		t.Errorf("500 不该判成会话过期: %v", other)
	}
}

// TestPlayAIExchangeSendStepsIssueOneRequestEach 走发送层：一次调用 = 一条请求 + 一个结论。
//
// 两步**不串成一条链**（先换票再建会话是 app 层的编排）。这里各调一次，钉"没有偷偷多发"。
func TestPlayAIExchangeSendStepsIssueOneRequestEach(t *testing.T) {
	sign := playAISessionTestSign()
	up, client := respondWith(t, map[string]any{"code": 0, "msg": "success", "result": "SSO-SYNTHETIC"})
	token, perr := up.PlayAIQuerySpecToken(context.Background(), sign, playAISessionTestReferer, "true")
	if perr != nil {
		t.Fatalf("第①步失败: %v", perr)
	}
	if token != "SSO-SYNTHETIC" {
		t.Errorf("应给出 SSO token，实得 %q", token)
	}
	if len(client.requests) != 1 {
		t.Fatalf("第①步应发一条，实得 %d", len(client.requests))
	}
	if !strings.Contains(client.requests[0].URL, "/userdomain/v2/querySpecToken") {
		t.Errorf("第①步的 URL 走样了: %q", client.requests[0].URL)
	}

	up2, client2 := respondWith(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"token": "JWT-SYNTHETIC"},
	})
	session, perr := up2.PlayAITyrzLogin(context.Background(), sign, playAISessionTestReferer, "SSO-SYNTHETIC", "false")
	if perr != nil {
		t.Fatalf("第②步失败: %v", perr)
	}
	if session.JWTToken != "JWT-SYNTHETIC" {
		t.Errorf("第②步应给出这一页自己的票，实得 %q", session.JWTToken)
	}
	if len(client2.requests) != 1 {
		t.Fatalf("第②步应发一条，实得 %d（两步各自一次，本层不串链）", len(client2.requests))
	}
	if !strings.Contains(client2.requests[0].URL, "/auth-service/auth/tyrzLogin") {
		t.Errorf("第②步的 URL 走样了: %q", client2.requests[0].URL)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 乙·埋点：POST /ycloud/visitlog/journaling
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIPageRefererIsTheLegacyPortalStringWithNoCredentials 钉那一串 Referer 逐字，
// 并且钉"里面没有任何凭据形态的东西"。
//
// 签到族那份页面 URL 会把 SSO token 拼进查询串；这一份不会（旧 TS 那一份也不含）。
// 反向改坏它谁会红：有人把票拼进 URL（"反正 Referer 也带着"）⇒ 本条红 ——
// 一个会过期的凭据进 URL，等于在日志与错误里多开一处凭据出口。
func TestPlayAIPageRefererIsTheLegacyPortalStringWithNoCredentials(t *testing.T) {
	want := "https://m.mcloud.139.com/portal/yunpanpage/index.html" +
		"?path=National_PlayAISpecial&sourceid=2019&enableShare=1"
	got := PlayAIPageReferer()
	if got != want {
		t.Errorf("referer 应逐字为 %q，实得 %q", want, got)
	}
	// 这一串里没有任何凭据形态的东西（凭据只走 `jwttoken` 头）。
	for _, fragment := range []string{"token=", "jwttoken", "Basic ", "@"} {
		if strings.Contains(strings.ToLower(got), strings.ToLower(fragment)) {
			t.Errorf("referer 里不该出现 %q（凭据只走 jwttoken 头），实得 %q", fragment, got)
		}
	}
	// `path=` 那一段是活动名，与 `marketName` 同源：两处必须同字，否则页面与接口对不上号。
	if !strings.Contains(got, "path="+PlayAIMarketName) {
		t.Errorf("referer 的 path 段应与接口那一份市场名同字，实得 %q", got)
	}
}

// TestPlayAIJournalingRequestShapeIsOurRegisteredChoice 钉一条埋点的**全部**三要素。
//
// # 措辞受限的地方
//
// `Content-Type` 选 form-urlencoded 是**我们的选择**（依据：同一 URL 在直播小红花族实拍长这样），
// 不是"这一页实拍过"—— 这一页零样本。本条只钉"我们发出的这一组恰好是这个形状"。
// 反向改坏它谁会红：body 三处顺序被动过、`module` 换名、头集合多一枚 `activityid`
// （改用 [WithH5Headers] 就会）—— 都红在本条。
func TestPlayAIJournalingRequestShapeIsOurRegisteredChoice(t *testing.T) {
	request := PlayAIJournalingRequest(
		PlayAIJournalingKeyword(PlayAIJournalingSuffixPV), PlayAIJournalingSourceIDPage,
		"JWT-SYNTHETIC", PlayAIPageReferer(), "true")
	if want := "https://m.mcloud.139.com/ycloud/visitlog/journaling"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, request.URL)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	wantBody := "module=uservisit&optkeyword=National_PlayAISpecial_pv&sourceid=2019" +
		"&marketName=National_PlayAISpecial"
	if got := playAISessionTestBody(t, "埋点", request); got != wantBody {
		t.Errorf("body 应逐字为 %q，实得 %q", wantBody, got)
	}
	if len(request.Body) != 101 {
		t.Errorf("body 长度登记为 101 字节，实得 %d", len(request.Body))
	}
	// body 是**字符串**，不是 JSON：以 `{` 开头说明有人走了 WithJSONBody。
	if strings.HasPrefix(strings.TrimSpace(string(request.Body)), "{") {
		t.Errorf("body 不该是 JSON 对象，实得 %q", string(request.Body))
	}
	wantNames := "accept,content-type,jwttoken,origin,referer,showloading"
	if got := playAITestHeaderNames(request); got != wantNames {
		t.Errorf("我们发出的头应恰好是 [%s]（夹具那族裁出来的集合），实得 [%s]", wantNames, got)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-SYNTHETIC" {
		t.Errorf("埋点也在 /ycloud 域内，应带注入的那张票，实得 %q", got)
	}
}

// TestPlayAIJournalingContentTypeMatchesTheCapturedLiveRoomOne 是这一族**唯一**的 A 档锚点。
//
// 同一个 URL 在直播小红花族实拍过（`docs/capture/app-live-redflower-samples.txt` §5.1 第 6 条），
// 那条用的是 form-urlencoded。这里钉两件事：①两条 URL 相同；②这一层选的头**与实拍那条同字**。
//
// ⚠ 它**不**证明"上游对这一页也认 form"（旧 TS 这一页发的是 JSON 头配字符串 body，两种形态
// 都没有本页样本）。反向改坏它谁会红：把 [PlayAIJournalingContentType] 改成 JSON 那一个 ⇒ 本条红，
// 而红得有意义：那是**离开唯一实拍**的动作，得先去看注释再决定。
func TestPlayAIJournalingContentTypeMatchesTheCapturedLiveRoomOne(t *testing.T) {
	ours := PlayAIJournalingRequest(
		PlayAIJournalingKeyword(PlayAIJournalingSuffixPV), PlayAIJournalingSourceIDPage,
		"JWT-SYNTHETIC", PlayAIPageReferer(), "true")
	theirs := LiveRoomJournalingRequest("National_LiveRoomFeedback_pv", "JWT-SYNTHETIC")
	if ours.URL != theirs.URL {
		t.Errorf("两条应打同一个 journaling 端点：ours=%q theirs=%q", ours.URL, theirs.URL)
	}
	if got, want := headerValue(ours.Headers, "content-type"), headerValue(theirs.Headers, "content-type"); got != want {
		t.Errorf("content-type 应与唯一实拍那一条同字（A 档），实得 %q vs %q", got, want)
	}
	if n := countHeader(ours.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应**恰好一枚**（先剥后补），实得 %d 个：两枚时上游按第一枚解析", n)
	}
	// 两条的**差别**只有三格取值：把这条断言留着，防止有人把两族埋点合并成一个函数。
	if strings.Contains(string(theirs.Body), PlayAIMarketName) ||
		strings.Contains(string(ours.Body), LiveRoomMarketName) {
		t.Error("两族埋点各用各的 marketName，谁也不该出现另一个的名字")
	}
}

// TestPlayAIJournalingRequestOmitsAppVersionActivityIDDeviceID 钉这一族"少三个头"的取向也适用于埋点。
//
// 与 `TestPlayAIUserStatusRequestIsTheCapturedBareGet` 同一理由，但这里是**外推**：
// 六个实拍只证明 `newyear/*` 那四条不发，没证明 journaling 不发（本页零样本）。
// 之所以跟着本族走：埋点与 ①…④ 是同一个页面、同一套前端发的。
// 同时钉住"[WithH5Headers] 确实会发这两个头" —— 它哪天变了，上面那句外推的说服力就要重写。
func TestPlayAIJournalingRequestOmitsAppVersionActivityIDDeviceID(t *testing.T) {
	request := PlayAIJournalingRequest(
		PlayAIJournalingKeyword(PlayAIJournalingSuffixPV), PlayAIJournalingSourceIDPage,
		"JWT-SYNTHETIC", PlayAIPageReferer(), "true")
	for _, forbidden := range []string{"appversion", "activityid", "deviceid"} {
		if n := countHeader(request.Headers, forbidden); n != 0 {
			t.Errorf("埋点不该带 %s（与 ①…④ 同一取向），实得 %d 个", forbidden, n)
		}
	}
	if got := playAITestHeaderNames(WithH5Headers(
		core.NewRequestGet(BaseMcloud), playAISessionTestReferer, nil, "true")); !strings.Contains(got, "activityid") {
		t.Errorf("对照组失效：WithH5Headers 应会发 activityid，实得 [%s]（上面那条外推的理由需要重写）", got)
	}
}

// TestPlayAIJournalingBodyLeavesInputsUnencoded 钉"我们**不**编码"这一选择。
//
// 出处能给的只有 `[A-Za-z0-9_]` 那些正常值（编码后逐字节不变），所以"要不要编码"没有证据。
// 这里刻意不调 [PercentEncode] —— 编码会把意外进来的字符吃掉，"看不出被改过"比"当场变样"更糟。
// 反向改坏它谁会红：加一句 `PercentEncode(optkeyword)` ⇒ 本条红，逼动手的人先去找实拍。
func TestPlayAIJournalingBodyLeavesInputsUnencoded(t *testing.T) {
	if got := PlayAIJournalingBody("a b&c", "2019"); got !=
		"module=uservisit&optkeyword=a b&c&sourceid=2019&marketName=National_PlayAISpecial" {
		t.Errorf("入参应原样出现（本层不编码，理由见注释），实得 %q", got)
	}
	// 键序与固定段：`module` 第一、`marketName` 最后，两枚固定值逐字。
	if got := PlayAIJournalingBody("k", "1"); !strings.HasPrefix(got, "module=uservisit&optkeyword=k&sourceid=1&marketName=") {
		t.Errorf("body 模板走样: %q", got)
	}
	if PlayAIJournalingModule != "uservisit" {
		t.Errorf("module 那一格的值应与唯一实拍同字，实得 %q", PlayAIJournalingModule)
	}
}

// TestPlayAIJournalingKeywordBuildersMatchTheirLegacySources 钉四组 optkeyword 的拼法与条数。
//
// 前缀恒为市场名 + 一个下划线（这是从旧 TS 那十余条串里逐条数出来的规律，B 档）。
// `complete_task` 那**四条**是旧实现在**猜格式**（两种下划线 × 有无 `_app`）：
// 本层把四种原样交出去并标明它们是猜法，不"顺手只留一种"（那等于替旧实现宣布它猜对了哪一种）。
// 反向改坏它谁会红：把 `_app` 那两条当成"同一条的两种写法"删掉 ⇒ 条数与顺序红；
// 把 `click_task_%d` 与 `click_task_%d_app` 合成一个函数 ⇒ 两条都红（它们分属两个调用点）。
func TestPlayAIJournalingKeywordBuildersMatchTheirLegacySources(t *testing.T) {
	if got := PlayAIJournalingKeyword("pv"); got != "National_PlayAISpecial_pv" {
		t.Errorf("前缀应取活动市场名，实得 %q", got)
	}
	if got := PlayAIJournalingClickTaskAppKeyword(7); got != "National_PlayAISpecial_click_task_7_app" {
		t.Errorf("aiCamera 第二条（task.ts:118）应为 …_app 形态，实得 %q", got)
	}
	if got := PlayAIJournalingClickTaskKeyword(7); got != "National_PlayAISpecial_click_task_7" {
		t.Errorf("任务前第三条（task.ts:388）**没有** _app 尾，实得 %q", got)
	}
	wantFour := []string{
		"National_PlayAISpecial_complete_task31",
		"National_PlayAISpecial_complete_task31_app",
		"National_PlayAISpecial_complete_task_31",
		"National_PlayAISpecial_complete_task_31_app",
	}
	gotFour := PlayAIJournalingCompleteTaskKeywords(31)
	if len(gotFour) != len(wantFour) {
		t.Fatalf("小红书那一条旧 TS 发了 %d 条（四种猜法），实得 %d", len(wantFour), len(gotFour))
	}
	for i := range wantFour {
		if gotFour[i] != wantFour[i] {
			t.Errorf("第 %d 条应为 %q，实得 %q", i+1, wantFour[i], gotFour[i])
		}
	}
	// 四条**互不相同**：两种下划线写法只差一个字符，合并成一类就是替上游做了决定。
	seen := map[string]bool{}
	for _, keyword := range gotFour {
		if seen[keyword] {
			t.Errorf("四条里有重复（%s）—— 重复说明有人在替旧实现挑一种猜法", keyword)
		}
		seen[keyword] = true
	}
}

// TestPlayAIJournalingSourceIDConstantsStayThreeApart 钉三档 sourceid 不许"顺手统一"。
//
// 最刺眼的一处事实（B 档）：`aiCamera` 那一个 handler 里连着两条埋点用了**两个**不同的
// `sourceid`（`task.ts:92` 用 2008、`:118` 用 2019），差别为什么存在没有成文。
// 反向改坏它谁会红：把 `…Camera` 与 `…Page` 合成一个常量 ⇒ 本条红（两条事件当场变成同一条）。
func TestPlayAIJournalingSourceIDConstantsStayThreeApart(t *testing.T) {
	trio := []string{
		PlayAIJournalingSourceIDPage, PlayAIJournalingSourceIDCamera, PlayAIJournalingSourceIDRedbook,
	}
	seen := map[string]bool{}
	for _, id := range trio {
		if seen[id] {
			t.Errorf("三档 sourceid 有两个同值（%v）—— 它们分属不同事件分组，别统一", trio)
		}
		seen[id] = true
	}
	if trio[0] != PlayAIPageSourceID {
		t.Errorf("页面级埋点那档应与页面 Referer 的 sourceid 同值（2019），实得 %q vs %q",
			trio[0], PlayAIPageSourceID)
	}
	// 同一条事件的两写法：`client` 这一后缀在入口（2019）与 aiCamera（2008）各出现一次。
	if PlayAIJournalingKeyword(PlayAIJournalingSuffixClient) != "National_PlayAISpecial_client" {
		t.Error("client 那一枚后缀在两处同字，改它会静默改掉两条事件")
	}
}

// TestParsePlayAIJournalingJudgesOnlyTheNumericCode 钉"回没回认账"的处置。
//
// 判据**不是这一页的**：A 档那条实拍回 `{"code":0,"msg":"success"}`，这一页零样本。
// 于是只钉得住两件事：数字 0 ⇒ 交回 nil；非 0 ⇒ 把码与原文带出来（**不**折成成功）。
// 旧 TS 把这一步当**门槛**（上报失败 ⇒ 整件任务 return false）—— 要不要同样当门槛是 app 层的
// 决定，本层不表态，所以这里也不断言"埋点失败必须中止"。
func TestParsePlayAIJournalingJudgesOnlyTheNumericCode(t *testing.T) {
	if perr := ParsePlayAIJournaling(playAITestEnvelope(t, `{"code":0,"msg":"success"}`)); perr != nil {
		t.Errorf("数字 0 应判认账，实得 %v", perr)
	}
	msg := playAITestErrMessage(t, "非零码", ParsePlayAIJournaling(
		playAITestEnvelope(t, `{"code":7,"msg":"SYNTHETIC-UPSTREAM-TEXT"}`)))
	if !strings.Contains(msg, "7") || !strings.Contains(msg, "SYNTHETIC-UPSTREAM-TEXT") {
		t.Errorf("应把码与上游原文带出来，实得 %q", msg)
	}
	msg = playAITestErrMessage(t, "字符串码", ParsePlayAIJournaling(
		playAITestEnvelope(t, `{"code":"0","msg":"success"}`)))
	if !strings.Contains(msg, "数字") {
		t.Errorf("`code` 是字符串 ⇒ 不是这一族的信封，应说清，实得 %q", msg)
	}
	msg = playAITestErrMessage(t, "无 code", ParsePlayAIJournaling(playAITestEnvelope(t, `{"msg":"x"}`)))
	if !strings.Contains(msg, "code") {
		t.Errorf("缺 code 应报错，实得 %q", msg)
	}
}

// TestPlayAIJournalingSendLayerSendsOneEventPerCall 钉"一次调用 = 一条事件"。
//
// 旧 TS 那几处循环（入口三条、小红书四条）是**多条事件**，循环归调用方 —— 本层不内置"一次发一串"。
// 反向改坏它谁会红：在 `PlayAIJournaling` 里加个 for 把四条 complete 都发出去 ⇒ 条数红。
func TestPlayAIJournalingSendLayerSendsOneEventPerCall(t *testing.T) {
	up, client := respondWith(t, map[string]any{"code": 0, "msg": "success"})
	if perr := up.PlayAIJournaling(context.Background(),
		PlayAIJournalingKeyword(PlayAIJournalingSuffixPV), PlayAIJournalingSourceIDPage,
		"JWT-SYNTHETIC", PlayAIPageReferer(), "true"); perr != nil {
		t.Fatalf("埋点应判认账: %v", perr)
	}
	if len(client.requests) != 1 {
		t.Fatalf("一次调用应发一条，实得 %d", len(client.requests))
	}
	sent := client.requests[0]
	if !strings.Contains(string(sent.Body), "optkeyword=National_PlayAISpecial_pv&sourceid=2019") {
		t.Errorf("发出去的那一条 body 走样: %q", string(sent.Body))
	}
	if n := countHeader(sent.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一枚，实得 %d", n)
	}
	// 上游回非 0 时结论必须传出去（旧 TS 用它当门槛；本层只交结论）。
	up2, _ := respondWith(t, map[string]any{"code": 3, "msg": "SYNTHETIC"})
	if perr := up2.PlayAIJournaling(context.Background(), "k", "2019",
		"JWT-SYNTHETIC", PlayAIPageReferer(), "true"); perr == nil {
		t.Error("code=3 不该被当成认账")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 丙·chat/v2/add：**只构造请求**（响应是 SSE，零样本 ⇒ 不猜形状）
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIChatBodyIsTheByteForByteShapeWeRegister 钉 chat 请求体的**字节**。
//
// 三处形状最容易被"顺手整理"掉，都在本条按字节钉着：`sessionId`/`applicationId` 是**空串**
// （不是缺席、不是 null）；`extInfo` 是字符串 "{}" 而 `sortInfo` 是**对象** {}；
// `attachmentTypeList` 是 `[3]` 而 `fileList` 只有一个元素。
// 反向改坏它谁会红：把 `extInfo` 改成对象（"更合理"）⇒ 本条红；
// 把两个空串省掉（omitempty）⇒ 本条红；动七枚开关里任意一枚 ⇒ 本条红，
// 而**这一整段零实拍** —— 红就是目的：逼动手的人先去找一条实拍。
func TestPlayAIChatBodyIsTheByteForByteShapeWeRegister(t *testing.T) {
	request, perr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
		"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", playAISessionTestChatParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if want := "https://ai.yun.139.com/api/outer/assistant/chat/v2/add"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q（`outer/assistant` 那两段是外站服务的形状）", want, request.URL)
	}
	wantBody := `{"applicationId":"","applicationType":"chat","dialogueInput":{` +
		`"attachment":{"attachmentTypeList":[3],"fileList":[{"fileId":"SYNTHETIC-FILE-ID","name":"1990000000000000005.jpeg"}]},` +
		`"command":{"command":"036","subCommand":"036006"},"dialogue":"人物戴眼镜了吗",` +
		`"enableAllNetworkSearch":false,"enableForceLlm":false,"enableForceNetworkSearch":true,` +
		`"enableKnowledgeAndNetworkSearch":false,"enableModelThinking":false,"enableRegenerate":false,` +
		`"extInfo":"{}","inputTime":"2026-06-29T12:10:43.123+08:00","prompt":"","sortInfo":{},` +
		`"toolSetting":{"imageToolSetting":{"enableLlmDescribe":true}},` +
		`"versionInfo":{"h5Version":"1.0.0"}},` +
		`"sessionId":"","sourceChannel":"101","userId":"1990000000000000001"}`
	if got := playAISessionTestBody(t, "chat", request); got != wantBody {
		t.Errorf("body 应逐字为 %q，实得 %q", wantBody, got)
	}
	if len(request.Body) != 696 {
		t.Errorf("body 长度登记为 696 字节，实得 %d", len(request.Body))
	}
	// 两格"长得不一样不是笔误"：一格里层是字符串、一格是空对象。
	for _, quoted := range []string{`"extInfo":"{}"`, `"sessionId":""`, `"applicationId":""`} {
		if !strings.Contains(string(request.Body), quoted) {
			t.Errorf("body 里应有 %s（旧 TS 那三格就是这个形态）", quoted)
		}
	}
	if !strings.Contains(string(request.Body), `"sortInfo":{}`) {
		t.Error("`sortInfo` 应是**对象**空值，不是字符串")
	}
}

// TestPlayAIChatRequestSendsFileNameUntouched 钉"本层不做那段算术"。
//
// 旧 TS 那一格是 `parseInt(String(taskId),10)+1`（`task.ts:196`）：19 位标识过一遍 JS 的 double
// 会**丢末几位**。本层既不复刻也不提供"从 taskId 推导文件名"的函数 —— 调用方给什么就发什么。
// 反向改坏它谁会红：加一句"顺手把 taskId +1 当文件名"⇒ 本条红（`…084605` 会变成 `…084600` 那一类），
// 而那种值在日志里看着完全正常，只有钉字节的测试挡得住。
func TestPlayAIChatRequestSendsFileNameUntouched(t *testing.T) {
	params := playAISessionTestChatParams()
	request, perr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
		"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	body := string(request.Body)
	if !strings.Contains(body, `"name":"1990000000000000005.jpeg"`) {
		t.Errorf("name 应原样发出，实得 %q", body)
	}
	// double 存不下 19 位：那段算术一旦进来，末三位会被抹平（`…005` → `…000`）。
	if strings.Contains(body, "1990000000000000000") {
		t.Errorf("body 里出现了被数值化过的尾号形态（本层不做 parseInt+1）: %q", body)
	}
	// `fileId` 与 `name` 是**两个**值：⑤ 回的 fileId 原样进 fileId，不许拿它去推导 name。
	if !strings.Contains(body, `"fileId":"SYNTHETIC-FILE-ID"`) {
		t.Error("fileId 应原样是 ⑤ 的 data.fileId")
	}
	// 换成别的 name 也只动那一格（证明这里没有任何"由 taskId 算出"的耦合）。
	params.FileName = "SYNTHETIC-NAME.jpg"
	other, perr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
		"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if !strings.Contains(string(other.Body), `"name":"SYNTHETIC-NAME.jpg"`) {
		t.Error("name 换了入参就该只换那一格")
	}
	if strings.Contains(string(other.Body), `"name":"1990000000000000005.jpeg"`) {
		t.Error("两格 name 不该同时出现")
	}
}

// TestPlayAIChatRequestReplacesAcceptWithSSEAndSendsNoJWTOrTingyun 钉 chat 的头集合里三件"我们的选择"。
//
// ① `Accept` 恰好一枚且为 `text/event-stream`（先剥后补）；② 不带 `jwttoken`（原生侧那套）；
// ③ **不发** `X-Tingyun` —— 旧 TS 三处都带，本仓该族夹具一条没有，且那串是写死的、含疑似
// 某次会话的标识。③ 钉的是"我们选择不发"，**不是**"上游不需要"（反向断言同样没有样本）。
// 反向改坏它谁会红：删掉那句 `dropHeader(...,"accept")` ⇒ 发出两枚 Accept，内容协商按第一枚走，
// 本条红；有人照抄那串写死的 APM 标识 ⇒ `x-tingyun` 那一问红。
func TestPlayAIChatRequestReplacesAcceptWithSSEAndSendsNoJWTOrTingyun(t *testing.T) {
	request, perr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
		"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", playAISessionTestChatParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if n := countHeader(request.Headers, "accept"); n != 1 {
		t.Fatalf("Accept 应**恰好一枚**（先剥后补），实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "accept"); got != "text/event-stream" {
		t.Errorf("Accept 应是流式那一个（旧 TS `task.ts:204`），实得 %q", got)
	}
	if got := headerValue(request.Headers, "content-type"); got != PlayAIPlainJSONContentType {
		t.Errorf("content-type 应与 ⑤ 同选不带 charset 的那一个（**我们的选择**，这一条零实拍），实得 %q", got)
	}
	if got := headerValue(request.Headers, "origin"); got != PlayAILocalOrigin {
		t.Errorf("origin 应与 ⑤ 同字（原生侧那一个），实得 %q", got)
	}
	if n := countHeader(request.Headers, "jwttoken"); n != 0 {
		t.Errorf("chat 是原生侧那一套（Authorization），不该带 jwttoken，实得 %d 个", n)
	}
	if n := countHeader(request.Headers, "x-tingyun"); n != 0 {
		t.Errorf("X-Tingyun 本层**不发**（理由见文件头三条）；这一条钉的是我们的选择，不是上游不需要")
	}
	// 与 ⑤ 的**已知偏离**点名登记：画像与渠道号沿用仓库那一套，不复制旧 TS 写死的 iPhone 串。
	if got := headerValue(request.Headers, "x-yun-app-channel"); got != IOSChannel {
		t.Errorf("x-yun-app-channel 应来自 [AppProfile]（=%s）；旧 TS 这一条给的是 \"101\"，"+
			"这一处是本文件里最可能在真打时先出问题的一格，复核顺序排第一", IOSChannel)
	}
	if got := headerValue(request.Headers, "x-yun-tid"); got != "TID-SYNTHETIC" {
		t.Errorf("x-yun-tid 应是注入值，实得 %q", got)
	}
	// 与埋点那族（H5）的对照：chat 不该带 showloading/jwttoken 那套 H5 头。
	for _, forbidden := range []string{"showloading", "activityid", "appversion"} {
		if n := countHeader(request.Headers, forbidden); n != 0 {
			t.Errorf("chat 走原生侧头，不该带 %s，实得 %d 个", forbidden, n)
		}
	}
}

// TestPlayAIChatRequestGuardsEachRequiredField 五格各一条：少一格就当场拒。
//
// 理由同 ⑤：这一条**整段**零实拍，可自由填的口子越大，能发出"出处里不存在的那一条"的面就越大；
// `userId` 那一条的后果是**以别人的账号去问**。
// 反向改坏它谁会红：把某一句 guard 删掉（"反正上游会拒"）⇒ 那一格变成发出去一条空值请求，本条红。
func TestPlayAIChatRequestGuardsEachRequiredField(t *testing.T) {
	type mutator struct {
		name     string
		field    func(params *PlayAIChatParams)
		fragment string
	}
	cases := []mutator{
		{"userId", func(p *PlayAIChatParams) { p.UserID = "" }, "userId"},
		{"dialogue", func(p *PlayAIChatParams) { p.Dialogue = "" }, "dialogue"},
		{"inputTime", func(p *PlayAIChatParams) { p.InputTime = "" }, "inputTime"},
		{"fileId", func(p *PlayAIChatParams) { p.FileID = "" }, "fileId"},
		{"name", func(p *PlayAIChatParams) { p.FileName = "" }, "name"},
	}
	for _, tc := range cases {
		params := playAISessionTestChatParams()
		tc.field(&params)
		request, perr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
			"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
		msg := playAITestErrMessage(t, "chat 缺 "+tc.name, perr)
		if !strings.Contains(msg, tc.fragment) {
			t.Errorf("报错应点名 %s，实得 %q", tc.fragment, msg)
		}
		if len(request.Body) != 0 || len(request.Headers) != 0 {
			t.Errorf("拒掉时不该留下半条请求（%s）", tc.name)
		}
	}
}

// TestPlayAIChatInputTimeReadsTheWallClockWithALiteralOffset 钉那串文本的两件事。
//
// ① 同样的入参出同样的串（构造函数必须是纯函数，否则没法逐字段断言）；
// ② 取**挂钟分量**而 `+08:00` 是**字面量** —— 这正是旧 JS `formatTime` 的语义
// （`getFullYear()/getHours()` 用本地分量，后缀硬编码）。
// ⚠ 这里**不**钉"上游要东八区"：旧 TS 跑在东八区机器上，两种写法在它的运行环境里逐字节相同，
// 差别在哪儿没有样本。所以本层不"修正"成 `t.In(+08:00)`（那是替上游宣布它比对时区）。
// 反向改坏它谁会红：换成 `t.Format(time.RFC3339)` ⇒ 毫秒三枚与后缀形状红；
// 改成 `t.In(shanghai)` ⇒ 传入 UTC 时的那一条红。
func TestPlayAIChatInputTimeReadsTheWallClockWithALiteralOffset(t *testing.T) {
	if got := PlayAIChatInputTime(playAISessionTestWhen()); got != "2026-06-29T12:10:43.123+08:00" {
		t.Errorf("应逐字为 2026-06-29T12:10:43.123+08:00，实得 %q", got)
	}
	// 挂钟分量取自入参本身（不做时区换算）：UTC 的那个时刻进来，得到同一串挂钟值 + 字面量后缀。
	utc := time.Date(2026, 1, 2, 3, 4, 5, 6000000, time.UTC)
	if got := PlayAIChatInputTime(utc); got != "2026-01-02T03:04:05.006+08:00" {
		t.Errorf("月/日/时/分/秒应零补齐、毫秒恒三枚，实得 %q", got)
	}
	// 纯函数：同一入参两次调用逐字相同。
	if PlayAIChatInputTime(utc) != PlayAIChatInputTime(utc) {
		t.Error("同一入参两次结果不同，构造函数不再是纯函数")
	}
}

// TestPlayAIChatConstantsMatchTheirLegacyLines 把 chat body 里每一格的**出处行号**跑到断言上。
//
// 为什么单独一条：五枚 false + 两枚 true 这种组合读起来像"随手填的"，而它是旧实现唯一跑过的那一档。
// 常量测试先红，逼"顺手把 enableForceLlm 打开"的人先去看这一格有没有样本（答案是没有）。
func TestPlayAIChatConstantsMatchTheirLegacyLines(t *testing.T) {
	pinned := []struct {
		name string
		got  any
		want any
	}{
		{"applicationType", PlayAIChatApplicationType, "chat"},
		{"sourceChannel", PlayAIChatSourceChannel, "101"},
		{"command", PlayAIChatCommand, "036"},
		{"subCommand", PlayAIChatSubCommand, "036006"},
		{"h5Version", PlayAIChatH5Version, "1.0.0"},
		{"extInfo", PlayAIChatExtInfo, "{}"},
		{"enableForceLlm", PlayAIChatEnableForceLlm, false},
		{"enableForceNetworkSearch", PlayAIChatEnableForceNetworkSearch, true},
		{"enableModelThinking", PlayAIChatEnableModelThinking, false},
		{"enableAllNetworkSearch", PlayAIChatEnableAllNetworkSearch, false},
		{"enableKnowledgeAndNetworkSearch", PlayAIChatEnableKnowledgeAndNetSearch, false},
		{"enableRegenerate", PlayAIChatEnableRegenerate, false},
		{"enableLlmDescribe", PlayAIChatEnableLlmDescribe, true},
		{"attachmentTypeList[0]", PlayAIChatAttachmentTypeImage, int64(3)},
	}
	for _, tc := range pinned {
		if tc.got != tc.want {
			t.Errorf("%s 应逐字为 %v（旧 TS 的那一档），实得 %v", tc.name, tc.want, tc.got)
		}
	}
	// `h5Version` 与仓库那两个 App 版本号**都不同值**：它是旧 TS 写死的"H5 版本号"。
	if PlayAIChatH5Version == H5AppVersion || PlayAIChatH5Version == IOSAppVersion {
		t.Error("h5Version 不该等于任何原生 App 版本号（三处三值，统一必错一处）")
	}
	// `PlayAIChatSSEAccept` 是本层"响应是流式文本"的唯一成文标记。
	if PlayAIChatSSEAccept != "text/event-stream" {
		t.Errorf("那一枚 Accept 就是这条链的形状声明，实得 %q", PlayAIChatSSEAccept)
	}
	// body 里布尔的**数量**（5 枚 false + 2 枚 true）：动任意一枚开关这条先红，
	// 比 body 那一条更早指向"哪一格被动过"，而这一整段零实拍。
	encoded, err := core.MarshalJSON(PlayAIChatBody(playAISessionTestChatParams()))
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	if n := strings.Count(string(encoded), "false"); n != 5 {
		t.Errorf("body 里应恰有 5 枚 false，实得 %d 枚（body=%s）", n, encoded)
	}
	if n := strings.Count(string(encoded), "true"); n != 2 {
		t.Errorf("body 里应恰有 2 枚 true（enableForceNetworkSearch 与 enableLlmDescribe），实得 %d", n)
	}
}

// TestPlayAIUpstreamHasNoChatSendMethodUntilOneCaptureExists 守住"响应零样本 ⇒ 不猜形状"这个口子。
//
// 本层对 chat **只构造请求**：[Upstream.Call] 一律按 JSON 信封解析，而这一条回的是 SSE；
// 与其猜事件名，不如留一个空口。反射扫的是**方法名**——它红了就是提醒：
// 加 `(u *Upstream).PlayAIChat…` 之前，先补一条这一页的 SSE 实拍。
// ⚠ 这条测试挡不住"有人用 [Upstream.Raw] 手解一段没见过的流"，那需要实拍才能判对错。
func TestPlayAIUpstreamHasNoChatSendMethodUntilOneCaptureExists(t *testing.T) {
	methods := reflect.TypeOf(&Upstream{})
	for i := 0; i < methods.NumMethod(); i++ {
		name := methods.Method(i).Name
		if strings.HasPrefix(name, "PlayAIChat") {
			t.Errorf("出现了 chat 的发送方法 %s：这一条的响应体零样本，解析层应等实拍（见文件头那节）", name)
		}
	}
	// 反过来：埋点与换票那三条**有**发送方法（它们回的是这一族已登记过形状的信封）。
	// 缺席说明有人把整个发送层删了 —— 那会让 app 层只能自己去拼请求。
	for _, want := range []string{"PlayAIQuerySpecToken", "PlayAITyrzLogin", "PlayAIJournaling"} {
		if _, ok := methods.MethodByName(want); !ok {
			t.Errorf("发送层应有 %s", want)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 跨三样：写请求不许自动重试
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIWriteRequestsAreNotAutoRetryable 钉 [core.HTTPRequest.IsRetryable] 这一格。
//
// 三条写请求（换票第②步、一条埋点、chat）都**没有**幂等键 ⇒ 传输层不许自动重试；
// 第①步是 GET，可重试。为什么值得单列：本层的失败结论会被 app 层拿去决定"要不要再来一次"，
// 而"再上报一次埋点""再建一条会话""再问一句"在服务器端**都是**留痕的。
// 反向改坏它谁会红：给埋点加一句 `WithIdempotencyKey`（"这样能重试"）⇒ 本条红，
// 而那个键的取值没有任何出处 —— 上游认不认它 ⇒ 未测试。
func TestPlayAIWriteRequestsAreNotAutoRetryable(t *testing.T) {
	sign := playAISessionTestSign()
	step2, perr := PlayAITyrzLoginRequest(sign, playAISessionTestReferer, "SSO-SYNTHETIC", "true")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	chat, perr := PlayAIChatRequest(playAITestProfile(), 1789800128813000,
		"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", playAISessionTestChatParams())
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	journaling := PlayAIJournalingRequest(
		PlayAIJournalingKeyword(PlayAIJournalingSuffixPV), PlayAIJournalingSourceIDPage,
		"JWT-SYNTHETIC", PlayAIPageReferer(), "true")
	for _, request := range []core.HTTPRequest{step2, chat, journaling} {
		if request.IsRetryable() {
			t.Errorf("%s 这条写请求不该可自动重试（无幂等键，且服务端留痕）", request.URL)
		}
	}
	step1 := PlayAIQuerySpecTokenRequest(sign, playAISessionTestReferer, "true")
	if !step1.IsRetryable() {
		t.Error("第①步是 GET（只读），应可重试")
	}
}
