package upstream

// 拍拍系列（#16）上游协议层的用例。
//
// # 本层的性质：**纯**测试
//
// 只断言两件事：① 构造出来的**请求**长什么样（URL / 方法 / 头名 / body 字节）；
// ② 给一段**实拍响应体**，解析出什么。不联网、不起 HTTP server、不碰 data 目录。
// 响应体一律走 [ParseEnvelope]（它用 `UseNumber`）而不是手搓 `map[string]any`：
// 真实路径上数字来自响应体解析，只测手搓的那条会让 [asInt64] 的 `json.Number`
// 分支挂掉而全绿（同 `app_funai_test.go` 那条说明）。
//
// # 证据只有一份
//
// `test/fixtures/http/夏日拍拍大作战.json` 的 13 条（①…⑤ 对应顺序 4/5/7/8/10/11/12）。
// 下面那些 `…Body…` 常量是**实拍原文**，只有四处动过，都在注释里就地标了：
//   - 值被脱敏的（`fileId`、prize 的 `buttonLink` 里那个 token）换成合成串；
//   - 本层不读的大块（`showData.prizeImages` 六项、任务条目的 `button` 七份端形态）
//     压成一项 / 一键 —— 保留**键本身**，因为"未知键要能放过"也是断言的一部分。
//
// ⚠ 头值（`jwttoken` / `referer` / `showloading` / `Authorization` / `x-yun-tid`）在夹具里
// **全部已脱敏** ⇒ 本文件只断言**头名**与**注入进去的合成值**，不断言任何真实凭据。
//
// # 每条测试挡的是哪一种"看着正常"
//
//  1. **URL 的字面形态** —— 这些"改对了"的表现是 404，看起来像接口下线，
//     不会指向我们自己动过的那一处（同 `app_notice_test.go` 文件头那段理由）。
//  2. **头集合** —— 这一族**刻意不复用** [WithH5Headers]：夹具里六条 `newyear/*`
//     一条都没有 `appversion` / `activityid`（`deviceid` 也没有）。"顺手复用"的
//     表现是发出一条**夹具里不存在的请求**，而上游对多余头的反应**没有样本**。
//     下面 `…NoAppVersionActivityIDDeviceID` 那条除了钉住"我们没有发"，
//     还钉住"[WithH5Headers] 确实会发" —— 后者变了，前者的说服力就要重写。
//  3. **两族信封不同构** —— 共用判据（[Envelope.EnsureOK] 走 [asInt64]，数字与字符串
//     都收）会把两族**都**判成成功，于是"把 AI 的响应喂给 newyear 的解析器"是一条
//     静默通路。各有专门一条反向用例。
//  4. **判据字段是指针** —— 实拍里为 `null` / 为 `0` / 为 `false` 的格子，
//     必须与"上游没给"分得开。
//  5. **两口写请求的字节** —— 与实拍的 `content-length`（79 / 54）对上，
//     这是本仓库少数能拿**字节**对拍的写请求；另有"不许被自动重试"那一条。
//
// ⚠ **零样本的分支不钉语义**：两族非成功码**一个实拍都没有**，所以那些用例钉的是
// "我们怎么处置它"（把码 + 上游 `msg` 原文带出来、不折成成功），
// 不是"某个码意味着 X"。

import (
	"context"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 夹具（实拍原文，脱敏位与压缩处见上面各常量抬头）
// ─────────────────────────────────────────────────────────────────────────────

// playAITestUserStatusBody 是夹具第 4 条（`user/info`，12:10:43）的响应。
//
// ⚠ `showData` 实拍是 `prizeBg` + `prizeImages`（**六项**）+ `titleImage`；这里只留一项，
// 因为本层不读它 —— 留着是为了钉"未知键不报错"。
const playAITestUserStatusBody = `{"code":0,"msg":"success","result":{` +
	`"showData":{"prizeBg":"https://img.mcloud.139.com/uop_prod/prize-bg.png",` +
	`"prizeImages":["https://img.mcloud.139.com/uop_prod/p1.png"],` +
	`"titleImage":"https://img.mcloud.139.com/uop_prod/title.png"},` +
	`"totalLotteryCount":0}}`

// playAITestTaskListBodyBefore 是夹具第 5 条（`task/list` 第一次，12:10:43）的三条任务。
//
// 实拍一条有 **21 个键**；这里每条只留本层读的十个 + `button`（压成一键）+ `sort`
// + `updateTime` —— 后三个是"读了也没用、但不许因为它在就报错"的那一类。
const playAITestTaskListBodyBefore = `{"code":0,"msg":"success","result":[` +
	`{"id":1,"name":"体验拍照问AI","flag":"拍照问AI","description":"抽奖次数+1",` +
	`"groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0,` +
	`"button":{"app":{"ext":"click"}},"sort":1,"updateTime":"2026-06-29 20:10:43"},` +
	`{"id":2,"name":"拍夏日关键词","flag":"拍夏日关键词","description":"抽奖次数+1",` +
	`"groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0,` +
	`"button":{"app":{"ext":"click"}},"sort":2,"updateTime":"2026-06-29 20:10:43"},` +
	`{"id":31,"name":"去小红书点赞","flag":"小红书","description":"抽奖次数+1",` +
	`"groupId":"popup","state":"FINISH","limitType":"EVER","recordid":"20466372","currstep":2,"process":0,` +
	`"button":{"app":{"ext":"click"}},"sort":3,"updateTime":"2026-05-30 23:15:58"}]}`

// playAITestTaskListBodyAfter 是夹具第 10 条（**同一条请求**的第二次，12:12:26）。
//
// 与上面那份的**全部**差别只在 `id:1` 那条：`WAIT`→`FINISH`、
// `recordid` 由 `null` 变**字符串** `"20674897"`、`currstep` 0→3。
// ⚠ `process` 两条都是 0（连已完成的那条也是）⇒ "进度"读不出，两格都存、不加解释。
const playAITestTaskListBodyAfter = `{"code":0,"msg":"success","result":[` +
	`{"id":1,"name":"体验拍照问AI","flag":"拍照问AI","description":"抽奖次数+1",` +
	`"groupId":"normal","state":"FINISH","limitType":"DAY","recordid":"20674897","currstep":3,"process":0,` +
	`"button":{"app":{"ext":"click"}},"sort":1,"updateTime":"2026-06-29 20:12:26"},` +
	`{"id":2,"name":"拍夏日关键词","flag":"拍夏日关键词","description":"抽奖次数+1",` +
	`"groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0,` +
	`"button":{"app":{"ext":"click"}},"sort":2,"updateTime":"2026-06-29 20:10:43"},` +
	`{"id":31,"name":"去小红书点赞","flag":"小红书","description":"抽奖次数+1",` +
	`"groupId":"popup","state":"FINISH","limitType":"EVER","recordid":"20466372","currstep":2,"process":0,` +
	`"button":{"app":{"ext":"click"}},"sort":3,"updateTime":"2026-05-30 23:15:58"}]}`

// playAITestStepClickBody 是夹具第 7 条（`task/step/click`）的成功响应。
//
// ⚠ 它**连 `result` 这个键都没有** —— 而这是一次**写**操作。
const playAITestStepClickBody = `{"code":0,"msg":"success"}`

// playAITestLotteryBody 是夹具第 12 条（`blindbox/lottery`）的响应，逐字。
//
// 两处合成：`buttonLink` 实拍查询串里带 `token=<那个值归档时已被脱敏>`（凭据型链接 ⇒ 换成合成串，
// 键与路径保留），`prizeImg` 只留占位。`prize` 里 `description`/`extInfo`/`poster`
// 三个键**实拍只有 null** ⇒ 保留在体里，但本层刻意不声明它们（见 `PlayAIPrize` 注释）。
const playAITestLotteryBody = `{"code":0,"msg":"success","result":{"prize":{` +
	`"buttonLink":"https://m.mcloud.139.com/portal/cloudItem/index.html?path=getPrize&sourceid=1090&token=SYNTHETIC-TOKEN",` +
	`"buttonTxt":"前往激活","description":null,"extInfo":null,"firstMonthGet":false,"high":false,` +
	`"oid":1248582079,"poster":null,"prizeId":260416004,` +
	`"prizeImg":"https://img.mcloud.139.com/uop_prod/prize.png",` +
	`"prizeName":"0.5元支付宝红包","prizeType":1002,` +
	`"tips":"奖品已下发到你的账号中，请在本月内前往“移动云盘APP-我的-领奖专区”激活使用，逾期失效",` +
	`"title":"恭喜你获得","type":"common"}}}`

// playAITestRecognizeBody 是夹具第 8 条（`aiRecognize`）的响应，逐字。
//
// `fileId` 实拍已脱敏 ⇒ 合成值（键保留：清理那一族要的就是它）。
// ⚠ 这一族的外层是 `{code:"0000", message, data, success}` —— 与 `newyear/*` 那族
// **不同构**：`code` 是**字符串**，"消息"那格叫 `message` 不叫 `msg`。
const playAITestRecognizeBody = `{"code":"0000","message":"请求成功","success":true,"data":{` +
	`"content":null,"fileId":"SYNTHETIC-FILE-ID","hasQuestion":null,"hasText":false,` +
	`"language":null,"recommendList":["人物戴眼镜了吗","人物穿什么上衣","背景是什么颜色"],` +
	`"taskId":"1991452320654084604"}}`

// playAITestReferer 是**合成** referer（实拍那个头已脱敏）。
const playAITestReferer = "https://referer.example/newyear-play-ai"

// ─────────────────────────────────────────────────────────────────────────────
// helper
// ─────────────────────────────────────────────────────────────────────────────

// playAITestEnvelope 走 [ParseEnvelope]（真实路径上数字是 [json.Number] 形态）。
func playAITestEnvelope(t *testing.T, body string) *Envelope {
	t.Helper()
	envelope, perr := ParseEnvelope([]byte(body))
	if perr != nil {
		t.Fatalf("夹具不是合法 JSON（必须走 ParseEnvelope）: %v", perr)
	}
	return envelope
}

// playAITestHeaderNames 返回**排序后的小写**头名串，供"恰好这一组"的断言用。
//
// 比个数不够：多一个头 + 少一个头可以个数相同，而那已经是两条不同的请求。
func playAITestHeaderNames(request core.HTTPRequest) string {
	names := make([]string, 0, len(request.Headers))
	for _, h := range request.Headers {
		names = append(names, strings.ToLower(h[0]))
	}
	sort.Strings(names)
	return strings.Join(names, ",")
}

// playAITestProfile 造一个未登录态的 App 档案（设备标识是合成值）。
func playAITestProfile() *AppProfile {
	return NewAppProfile(NewIPadAppDevice("PAD0000000000000000000000000000", "PAD0000000000"))
}

// playAITestErrMessage 要求"必须报错"，并把消息文本交回调用方继续钉内容。
func playAITestErrMessage(t *testing.T, name string, perr *core.Error) string {
	t.Helper()
	if perr == nil {
		t.Fatalf("%s 应报错，实得 nil（非成功码不许被当成成功）", name)
	}
	return perr.Message()
}

// ─────────────────────────────────────────────────────────────────────────────
// ① ②：只读 GET 的字面形态
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIUserStatusRequestIsTheCapturedBareGet 钉夹具第 4/11 条（两次逐字相同）。
//
// 反向改坏它谁会红：把 `user/info` 也补上 `source`（那是 ② 才有的键）、
// 把 `newyear` 写成 `newYear`、或换了主机 —— 都会红。而真跑时那三种都只表现为 404。
func TestPlayAIUserStatusRequestIsTheCapturedBareGet(t *testing.T) {
	request := PlayAIUserStatusRequest("JWT-ONE", playAITestReferer, "true")
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	want := "https://m.mcloud.139.com/ycloud/newyear/user/info?marketName=National_PlayAISpecial"
	if request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("这条是裸 GET，不该有 body，实得 %q", string(request.Body))
	}
	// ⚠ 实拍这一条**不带** `source`（与 ② 差一个键）。
	if strings.Contains(request.URL, "source") {
		t.Errorf("user/info 实拍没有 source 键，实得 %q", request.URL)
	}
	if got, want := playAITestHeaderNames(request), "accept,jwttoken,referer,showloading"; got != want {
		t.Errorf("本层发出的头应恰好是 [%s]（夹具里其余的 sec-fetch-*/UA/cookie 按仓库现状不发），实得 [%s]", want, got)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-ONE" {
		t.Errorf("jwttoken 应是注入的那张票，实得 %q", got)
	}
	if got := headerValue(request.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 该由调用方给（实拍值已脱敏），实得 %q", got)
	}
	if got := headerValue(request.Headers, "referer"); got != playAITestReferer {
		t.Errorf("referer 该由调用方给（实拍值已脱敏），实得 %q", got)
	}
	// 实拍 GET **没有** origin（两条 POST 才有）⇒ 这里不许出现。
	if countHeader(request.Headers, "origin") != 0 {
		t.Error("实拍 GET 不带 origin")
	}
	if countHeader(request.Headers, "content-type") != 0 {
		t.Error("无 body ⇒ 不该有 content-type")
	}
}

// TestPlayAITaskListRequestPinsBothQueryKeysInCapturedOrder 钉夹具第 5/10 条。
//
// 反向改坏它谁会红：改用 `url.Values.Encode()`（字典序）后新增一个键、把顺序挪了 ——
// 本条按**整串字节**比，动了就红。实拍两次这一条的 URL 逐字相同。
func TestPlayAITaskListRequestPinsBothQueryKeysInCapturedOrder(t *testing.T) {
	request := PlayAITaskListRequest("JWT-ONE", playAITestReferer, "true")
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	want := "https://m.mcloud.139.com/ycloud/newyear/task/list?marketName=National_PlayAISpecial&source=app"
	if request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q（两个键的先后也是实拍的一部分）", want, request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("这条是裸 GET，不该有 body，实得 %q", string(request.Body))
	}
	if got, want := playAITestHeaderNames(request), "accept,jwttoken,referer,showloading"; got != want {
		t.Errorf("本层发出的头应恰好是 [%s]，实得 [%s]", want, got)
	}
}

// TestPlayAINewyearRequestsCarryNoAppVersionActivityIDDeviceID ⭐ 钉"这一族少三个头"。
//
// 核对口径（逐条回数，不是引用注释）：夹具第 4/5/10/11 条（GET）与第 7/12 条（POST）
// 的头名里**都没有** `appversion` / `activityid` / `deviceid`；同一份归档的第 0/1/2/3 条
// **都带** `appversion`、`activityid`、`deviceid` ⇒ 不是脱敏抹掉的（脱敏器只换**值**，
// 而这三个头在 `newyear/*` 六条里连**名字**都没出现）。
//
// 后半段钉"[WithH5Headers] 确实会补 `appversion`/`activityid`" —— 这条对照红了，
// 就说明"不复用它"的理由变了，要回来重写上面的取向，而不是留着一条没理由的禁令。
func TestPlayAINewyearRequestsCarryNoAppVersionActivityIDDeviceID(t *testing.T) {
	stepClick, perr := PlayAIStepClickRequest("JWT-ONE", playAITestReferer, "true", 1)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	lottery, perr := PlayAILotteryRequest("JWT-ONE", playAITestReferer, "true")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	for name, request := range map[string]core.HTTPRequest{
		"user/info":        PlayAIUserStatusRequest("JWT-ONE", playAITestReferer, "true"),
		"task/list":        PlayAITaskListRequest("JWT-ONE", playAITestReferer, "true"),
		"task/step/click":  stepClick,
		"blindbox/lottery": lottery,
	} {
		for _, forbidden := range []string{"appversion", "activityid", "deviceid"} {
			if n := countHeader(request.Headers, forbidden); n != 0 {
				t.Errorf("%s 不该带 %s（夹具那六条一条都没有，实得 %d 个）", name, forbidden, n)
			}
		}
		// 每条都**必须**带票：这四条在夹具里条条带 `jwttoken`。
		if n := countHeader(request.Headers, "jwttoken"); n != 1 {
			t.Errorf("%s 应恰好一个 jwttoken，实得 %d", name, n)
		}
	}

	jwt := "JWT-ONE"
	bare := WithH5Headers(core.NewRequestGet("https://m.mcloud.139.com/ycloud/newyear/user/info"),
		playAITestReferer, &jwt, "true")
	for _, added := range []string{"appversion", "activityid"} {
		if countHeader(bare.Headers, added) != 1 {
			t.Errorf("对照失效：[WithH5Headers] 不再补 %s ⇒ 上面那条'刻意不复用'的理由要重写", added)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ ④：两口**写**请求的字节
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIStepClickRequestBodyIsTheCapturedBytes 钉夹具第 7 条的 body 与头。
//
// 实拍 `content-length: 79`，body 逐字：
//
//	{"key":"click","marketName":"National_PlayAISpecial","source":"app","taskId":1}
//
// 反向改坏它谁会红：`taskId` 被"顺手"改成字符串、`key` 改了字面量（实拍只有 `click`
// 这一档，其它 step key 零样本）、`marketName` 换成过期期次、多一个键少一个键 ——
// 全都红在字节上。这一条是本层**唯一**能拿字节对拍的判据来源。
func TestPlayAIStepClickRequestBodyIsTheCapturedBytes(t *testing.T) {
	request, perr := PlayAIStepClickRequest("JWT-ONE", playAITestReferer, "true", 1)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if want := "https://m.mcloud.139.com/ycloud/newyear/task/step/click"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q（末段 `click` 与 body 的 `key` 同字，但两者是否必须同字**未验证**）", want, request.URL)
	}
	wantBody := `{"key":"click","marketName":"National_PlayAISpecial","source":"app","taskId":1}`
	if string(request.Body) != wantBody {
		t.Errorf("body 应逐字为 %q，实得 %q", wantBody, string(request.Body))
	}
	if len(request.Body) != 79 {
		t.Errorf("实拍 content-length 是 79，实得 %d", len(request.Body))
	}
	if strings.Contains(string(request.Body), `"taskId":"1"`) {
		t.Error("`taskId` 实拍是**数字**（无引号），不许被改成字符串")
	}
	if got, want := playAITestHeaderNames(request), "accept,content-type,jwttoken,origin,referer,showloading"; got != want {
		t.Errorf("本层发出的头应恰好是 [%s]，实得 [%s]", want, got)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一个（先剥后补），实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != H5ContentType {
		t.Errorf("这一族实拍带 charset，实得 %q", got)
	}
	if n := countHeader(request.Headers, "origin"); n != 1 {
		t.Errorf("origin 应恰好一个，实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "origin"); got != BaseMcloud {
		t.Errorf("origin 实拍是 %q，实得 %q", BaseMcloud, got)
	}
	// **只有一枪**：写请求不带幂等键 ⇒ 传输层不许自动重试（重试 = 再上报一次）。
	if request.IsRetryable() {
		t.Error("task/step/click 是写请求且无幂等键，不该被判定为可重试")
	}
}

// TestPlayAIStepClickTaskIDIsNeverHardcoded 钉 `taskId` 真的来自入参。
//
// 实拍只有一档（`taskId: 1` 对应任务表 `id: 1`）；把它写成常量在这份夹具上**不会**红，
// 所以另钉一档非 1 的值。
func TestPlayAIStepClickTaskIDIsNeverHardcoded(t *testing.T) {
	for _, taskID := range []int64{2, 31, 0} {
		body, err := core.MarshalJSON(PlayAIStepClickBody(taskID))
		if err != nil {
			t.Fatalf("序列化失败: %v", err)
		}
		want := `{"key":"click","marketName":"National_PlayAISpecial","source":"app","taskId":` +
			playAITestItoa(taskID) + `}`
		if string(body) != want {
			t.Errorf("taskId=%d 的 body 应逐字为 %q，实得 %q", taskID, want, string(body))
		}
	}
}

// TestPlayAILotteryRequestBodyIsTheCapturedBytes 钉夹具第 12 条的 body 与头。
//
// 实拍 `content-length: 54`，**只有两个键**。反向改坏它谁会红：照 ③ 那样多带一个
// `key`/`taskId`（那是另一口请求的键）⇒ 字节变了就红 —— 而"多带一个键"上游**未必**报错，
// 差别只体现在"这次抽的是哪一档"。
func TestPlayAILotteryRequestBodyIsTheCapturedBytes(t *testing.T) {
	request, perr := PlayAILotteryRequest("JWT-ONE", playAITestReferer, "true")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if want := "https://m.mcloud.139.com/ycloud/newyear/blindbox/lottery"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, request.URL)
	}
	wantBody := `{"marketName":"National_PlayAISpecial","source":"app"}`
	if string(request.Body) != wantBody {
		t.Errorf("body 应逐字为 %q，实得 %q", wantBody, string(request.Body))
	}
	if len(request.Body) != 54 {
		t.Errorf("实拍 content-length 是 54，实得 %d", len(request.Body))
	}
	for _, forbidden := range []string{"key", "taskId", "userId"} {
		if strings.Contains(string(request.Body), `"`+forbidden+`"`) {
			t.Errorf("抽奖这条没有 %q 这个键", forbidden)
		}
	}
	if got, want := playAITestHeaderNames(request), "accept,content-type,jwttoken,origin,referer,showloading"; got != want {
		t.Errorf("本层发出的头应恰好是 [%s]，实得 [%s]", want, got)
	}
	// **只有一枪**：一次调用抽一次，且不许被自动重试。
	if request.IsRetryable() {
		t.Error("blindbox/lottery 是写请求且无幂等键，不该被判定为可重试")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑤ aiRecognize：**另一套头 + 另一族信封**
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIRecognizeRequestPinsNativeHeadersAndBodyBytes 钉夹具第 8 条。
//
// 这一条与 ①…④ **不是一套东西**：它是 App 原生侧发出的，实拍**不带 `jwttoken`**。
// 三处里最容易被"顺手统一"掉的是 `Content-Type` —— 实拍**不带 charset**，
// 而 [WithAppHeaders] 末尾一定会追加带 charset 的那个 ⇒ 必须先剥后补。
// 反向改坏它谁会红：删掉那句 `dropHeader` ⇒ 发出**两个** `Content-Type`
// （上游按第一个解析，请求看着完全正常）。
//
// ⚠ 实拍 `userId` 是一串 19 位标识（未脱敏）⇒ 这里是**合成号**，不复制真值。
func TestPlayAIRecognizeRequestPinsNativeHeadersAndBodyBytes(t *testing.T) {
	params := PlayAIRecognizeParams{
		UserID:   "1990000000000000001",
		DataURL:  "data:image/jpg;base64,QUJDRA==",
		ImageExt: "jpg",
	}
	request, perr := PlayAIRecognizeRequest(playAITestProfile(), 1750000000000,
		"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if want := "https://ai.yun.139.com/api/image/aiRecognize"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q（`aiRecognize` 的驼峰段与主机都不能动）", want, request.URL)
	}
	if strings.Contains(request.URL, BaseMcloud) {
		t.Errorf("⑤ 不在 m.mcloud.139.com 上，实得 %q", request.URL)
	}
	wantBody := `{"base64":"data:image/jpg;base64,QUJDRA==","channelId":"101","imageExt":"jpg",` +
		`"recognizeType":"1","sendType":"2","timeout":30000,"uploadToCloud":true,` +
		`"userId":"1990000000000000001"}`
	if string(request.Body) != wantBody {
		t.Errorf("body 应逐字为 %q，实得 %q", wantBody, string(request.Body))
	}
	// 三个"实拍是字符串"的键：写成数字就红在字节上（值为什么是这个 ⇒ 无成文，见常量注释）。
	for _, quoted := range []string{`"channelId":"101"`, `"recognizeType":"1"`, `"sendType":"2"`} {
		if !strings.Contains(string(request.Body), quoted) {
			t.Errorf("body 里应有 %s（实拍是**带引号的**字符串）", quoted)
		}
	}
	// `timeout` 是 body 里唯一的数字字段：带引号就红。
	if !strings.Contains(string(request.Body), `"timeout":30000`) {
		t.Error("`timeout` 实拍是数字 30000")
	}
	// `uploadToCloud` 实拍恒 true（false 那一档零样本 ⇒ 不参数化）。
	if !strings.Contains(string(request.Body), `"uploadToCloud":true`) {
		t.Error("`uploadToCloud` 应是裸 true")
	}
	// `base64` 装的实拍是**整条 data URL**：不许剥前缀也不许拼前缀。
	if !strings.Contains(string(request.Body), `"base64":"data:image/jpg;base64,`) {
		t.Error("`base64` 该原样带上整条 data URL（实拍就含 `data:` 前缀）")
	}

	// 头：原生侧那一套 + 四个注入点。
	if n := countHeader(request.Headers, "jwttoken"); n != 0 {
		t.Errorf("⑤ 实拍**不带** jwttoken（它是原生侧的 Authorization 那套），实得 %d 个", n)
	}
	for _, forbidden := range []string{"appversion", "activityid", "deviceid"} {
		if countHeader(request.Headers, forbidden) != 0 {
			t.Errorf("⑤ 不该带 %s", forbidden)
		}
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应**恰好一个**（先剥后补），实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != "application/json" {
		t.Errorf("实拍不带 charset，实得 %q", got)
	}
	if n := countHeader(request.Headers, "origin"); n != 1 {
		t.Fatalf("origin 应恰好一个，实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "origin"); got != "mcloudlocal://yun.139.com" {
		t.Errorf("origin 实拍是原生侧那一个值，实得 %q（与 ①…④ 的 %q 是两个不同的值）",
			got, BaseMcloud)
	}
	if got := headerValue(request.Headers, "authorization"); got != "Basic U1lOVEhFVElDLUFVVEg=" {
		t.Errorf("Authorization 应是注入值，实得 %q", got)
	}
	if got := headerValue(request.Headers, "x-yun-tid"); got != "TID-SYNTHETIC" {
		t.Errorf("x-yun-tid 应是注入值，实得 %q", got)
	}
	if got := headerValue(request.Headers, "accept"); got != "*/*" {
		t.Errorf("Accept 实拍是 */*，实得 %q", got)
	}
	// 实拍这条确实带的两个公共设备头（值本身已脱敏 ⇒ 只点名）。
	for _, present := range []string{"x-deviceinfo", "x-yun-api-version"} {
		if countHeader(request.Headers, present) != 1 {
			t.Errorf("实拍带 %s，实得 %d 个", present, countHeader(request.Headers, present))
		}
	}
	// 它会**往云盘落一张图** ⇒ 与 ③④ 同一档：不许自动重发。
	if request.IsRetryable() {
		t.Error("aiRecognize 是写请求（落盘一张图）且无幂等键，不该被判定为可重试")
	}
}

// TestPlayAIRecognizeRequestRejectsEmptyMandatoryParams 钉三个必填的护栏。
//
// 反向改坏它谁会红：删掉任一句 guard ⇒ 发出一条**少一个键**的请求（实拍没有这种形态），
// 而 `userId` 为空那一条的后果是"打到别人的账号上"。
// 报错时返回的必须是**零值请求** —— 半条请求比没有请求更危险。
func TestPlayAIRecognizeRequestRejectsEmptyMandatoryParams(t *testing.T) {
	full := PlayAIRecognizeParams{
		UserID: "1990000000000000001", DataURL: "data:image/jpg;base64,QUJDRA==", ImageExt: "jpg",
	}
	for name, mutate := range map[string]func(*PlayAIRecognizeParams){
		"userId 为空":   func(p *PlayAIRecognizeParams) { p.UserID = "" },
		"base64 为空":   func(p *PlayAIRecognizeParams) { p.DataURL = "" },
		"imageExt 为空": func(p *PlayAIRecognizeParams) { p.ImageExt = "" },
		"三个都空":        func(p *PlayAIRecognizeParams) { *p = PlayAIRecognizeParams{} },
	} {
		t.Run(name, func(t *testing.T) {
			params := full
			mutate(&params)
			request, perr := PlayAIRecognizeRequest(playAITestProfile(), 1750000000000,
				"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
			message := playAITestErrMessage(t, name, perr)
			// ⚠ 不看 `request.Method`：[core.HTTPMethod] 是 `int`，零值就是 `MethodGet`，
			// 拿它判"空请求"会永远为假。URL 才是那条请求发不发得出去的开关。
			if request.URL != "" || len(request.Headers) != 0 || len(request.Body) != 0 {
				t.Errorf("报错时不许留下一条能发出去的请求，实得 %+v", request)
			}
			for _, key := range []string{"userId", "base64", "imageExt"} {
				if strings.Contains(name, key) && !strings.Contains(message, key) {
					t.Errorf("错误串该点名 %s，实得 %q", key, message)
				}
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 信封：两族**不同构**，各判各的
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAINewyearParsersRejectTheRecognizeEnvelope 钉"别把 AI 的响应喂给 newyear"。
//
// 这条为什么必须存在：[Envelope.EnsureOK] 的 `code` 走 [asInt64]，而那里**数字与字符串都收**
// ⇒ 用它做公共判据会把 `{"code":0}` 与 `{"code":"0000"}` **都**判成成功，
// 于是"两族混解"是一条**静默**通路（例如把 ⑤ 的响应交给 ④ 的解析器，读成"抽了、中了个空"）。
//
// 反向改坏它谁会红：把 [playAINewyearPayload] 里那句 `rawCode.(json.Number)` 换成
// [Envelope.Code] —— 四口全部变绿（错），本条立刻红。
func TestPlayAINewyearParsersRejectTheRecognizeEnvelope(t *testing.T) {
	envelope := playAITestEnvelope(t, playAITestRecognizeBody)
	if _, perr := PlayAIParseUserStatus(envelope); !strings.Contains(
		playAITestErrMessage(t, "user/info", perr), "不是数字") {
		t.Errorf("user/info 的错误该点名 code 类型，实得 %q", perr.Message())
	}
	if _, perr := PlayAIParseTaskList(envelope); !strings.Contains(
		playAITestErrMessage(t, "task/list", perr), "不是数字") {
		t.Errorf("task/list 的错误该点名 code 类型，实得 %q", perr.Message())
	}
	if perr := PlayAIParseStepClick(envelope); !strings.Contains(
		playAITestErrMessage(t, "task/step/click", perr), "不是数字") {
		t.Errorf("step/click 的错误该点名 code 类型，实得 %q", perr.Message())
	}
	if _, perr := PlayAIParseLottery(envelope); !strings.Contains(
		playAITestErrMessage(t, "blindbox/lottery", perr), "不是数字") {
		t.Errorf("lottery 的错误该点名 code 类型，实得 %q", perr.Message())
	}
}

// TestPlayAIRecognizeParserRejectsTheNewyearEnvelope 是上一条的反方向。
func TestPlayAIRecognizeParserRejectsTheNewyearEnvelope(t *testing.T) {
	envelope := playAITestEnvelope(t, playAITestUserStatusBody)
	perr := func() *core.Error {
		_, perr := PlayAIParseRecognize(envelope)
		return perr
	}()
	message := playAITestErrMessage(t, "aiRecognize", perr)
	if !strings.Contains(message, "不是字符串") {
		t.Errorf("错误该点名 code 类型（这一族是字符串 \"0000\"），实得 %q", message)
	}
}

// TestPlayAINewyearParsersCarryTheNonZeroCodeAndUpstreamText 钉**我们的处置**，不钉码义。
//
// ⚠ 两族**非成功码一个实拍都没有**，所以这里不写"某码意味着 X"，只钉三件事：
// 非 0 不许被当成功、上游那句 `msg` **原文**要带进错误（`Message` 只进日志）、
// `msg` 不是字符串时不许把 Go 的打印形态塞进去。
// 另钉 `kind`：非 0 码落成 [core.KindBadRequest]（不重试）—— 若哪天被折成
// [core.KindParse] 或 Auth，重试与"该不该刷票"的策略会跟着变。
func TestPlayAINewyearParsersCarryTheNonZeroCodeAndUpstreamText(t *testing.T) {
	envelope := playAITestEnvelope(t, `{"code":7,"msg":"活动已结束"}`)
	for name, perr := range map[string]*core.Error{
		"user/info": func() *core.Error {
			_, perr := PlayAIParseUserStatus(envelope)
			return perr
		}(),
		"task/list": func() *core.Error {
			_, perr := PlayAIParseTaskList(envelope)
			return perr
		}(),
		"task/step/click":  PlayAIParseStepClick(envelope),
		"blindbox/lottery": func() *core.Error { _, perr := PlayAIParseLottery(envelope); return perr }(),
	} {
		message := playAITestErrMessage(t, name, perr)
		if !strings.Contains(message, "code=7") {
			t.Errorf("%s 的错误该带上那个码便于排障，实得 %q", name, message)
		}
		if !strings.Contains(message, "活动已结束") {
			t.Errorf("%s 该把上游原文带出来，实得 %q", name, message)
		}
		if perr.Kind() != core.KindBadRequest {
			t.Errorf("%s 的 kind 应是 BadRequest（非 0 码不是解析失败也不是刷票），实得 %v", name, perr.Kind())
		}
	}

	// `msg` 不是字符串 ⇒ 只要码 + 一句人话，不许出现 Go 的 map 打印形态。
	odd := playAITestEnvelope(t, `{"code":7,"msg":{"text":"活动已结束"}}`)
	perr := PlayAIParseStepClick(odd)
	message := playAITestErrMessage(t, "msg 非字符串", perr)
	if strings.Contains(message, "map[") {
		t.Errorf("不该把上游那格的 Go 打印形态塞进错误，实得 %q", message)
	}
	if strings.Contains(message, "活动已结束") {
		t.Errorf("不是字符串就不该带出来，实得 %q", message)
	}

	// 没有 code ⇒ 明说"这一族必带"，不折成"code=0"。
	noCode := playAITestEnvelope(t, `{"msg":"success"}`)
	message = playAITestErrMessage(t, "缺 code", PlayAIParseStepClick(noCode))
	if !strings.Contains(message, "code") {
		t.Errorf("错误该点名 code，实得 %q", message)
	}
	if strings.Contains(message, "code=0") {
		t.Errorf("缺字段不许被读成 code=0，实得 %q", message)
	}
}

// TestPlayAINewyearNineZeroZeroOneIsSessionExpired：`90001` 是这一族**唯一**被单独立起来的非零码。
//
// 判据本身**不是**这一族抓来的（newyear 那六条实拍全是 `code=0`），而是签到族那条实测：
// `90001` = "`jwttoken` 没带上，或会话没建起来"（`docs/capture/app-signin-samples.txt`）。
// 之所以要在这一族也分出来：app 层那条"刷一次票、重放一次"的自救支路
// （[core.Error.IsSessionExpired]）**只认这个变体** —— 折成普通 BadRequest 的表现是
// 自救代码在这条链上永远不触发，池里那张过期的票会被一直打。
//
// ⚠ 本条只钉"我们把它分类成什么"，**不**主张"这一族撞 90001 时刷了票就能过"（未测试）。
func TestPlayAINewyearNineZeroZeroOneIsSessionExpired(t *testing.T) {
	envelope := playAITestEnvelope(t, `{"code":90001,"msg":"未登录"}`)
	for name, perr := range map[string]*core.Error{
		"user/info": func() *core.Error { _, perr := PlayAIParseUserStatus(envelope); return perr }(),
		"task/list": func() *core.Error { _, perr := PlayAIParseTaskList(envelope); return perr }(),
	} {
		message := playAITestErrMessage(t, name, perr)
		if !perr.IsSessionExpired() {
			t.Errorf("%s 的 90001 该是会话过期变体（否则 app 层不会刷票），实得 %q", name, message)
		}
		if !strings.Contains(message, "code=90001") || !strings.Contains(message, "未登录") {
			t.Errorf("%s 仍该带上游的码与原文，实得 %q", name, message)
		}
		if !strings.Contains(message, "jwttoken") {
			t.Errorf("%s 该带 [Diagnose] 那句排查提示，实得 %q", name, message)
		}
		// kind 不变：这个变体对外仍是 BadRequest（见 core/errors.go 那张表），
		// 否则重试策略与对外文案会跟着这次分类一起漂移。
		if perr.Kind() != core.KindBadRequest {
			t.Errorf("%s 的 kind 应仍是 BadRequest，实得 %v", name, perr.Kind())
		}
	}
}

// TestPlayAIRecognizeRequiresBothCodeStringAndSuccessTrue 钉 ⑤ 的两道判据都要收。
//
// ⚠ 唯一样本里 `code=="0000"` 与 `success==true` **同时**成立 ⇒ "只成立一个"是什么意思
// **没有样本**：所以两档都交回错误，而不是挑一个当判据。
func TestPlayAIRecognizeRequiresBothCodeStringAndSuccessTrue(t *testing.T) {
	cases := []struct {
		name     string
		body     string
		wantOK   bool
		wantKind core.ErrorKind
	}{
		{
			name:   "实拍那份",
			body:   `{"code":"0000","message":"请求成功","success":true,"data":{"hasText":false}}`,
			wantOK: true,
		},
		{
			name:     "success 是 false",
			body:     `{"code":"0000","message":"请求成功","success":false,"data":{"hasText":false}}`,
			wantKind: core.KindBadRequest,
		},
		{
			name:     "没有 success 这个键",
			body:     `{"code":"0000","message":"请求成功","data":{"hasText":false}}`,
			wantKind: core.KindBadRequest,
		},
		{
			name:     "code 是别的字符串",
			body:     `{"code":"0001","message":"识别失败","success":true,"data":{"hasText":false}}`,
			wantKind: core.KindBadRequest,
		},
		{
			name:     "code 是数字（那是 newyear 那族）",
			body:     `{"code":0,"message":"请求成功","success":true,"data":{"hasText":false}}`,
			wantKind: core.KindParse,
		},
		{
			name:     "成功但没有对象形态的 data",
			body:     `{"code":"0000","message":"请求成功","success":true}`,
			wantKind: core.KindParse,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			_, perr := PlayAIParseRecognize(playAITestEnvelope(t, c.body))
			if c.wantOK {
				if perr != nil {
					t.Fatalf("实拍那份该解析成功，实得 %v", perr)
				}
				return
			}
			if perr == nil {
				t.Fatal("这一档没有样本 ⇒ 不许被当成成功")
			}
			if perr.Kind() != c.wantKind {
				t.Errorf("kind 应是 %v，实得 %v（%q）", c.wantKind, perr.Kind(), perr.Message())
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析读数：指针与"缺 / 零值"之分
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIUserStatusDistinguishesZeroFromMissing 钉 ① 那一格。
//
// `totalLotteryCount` 实拍两条是 **0**（12:10:43）与 **1**（12:12:26 前后）：
// 0 是**合法取值**，与"上游没给"混起来，调用方就没法区分"这次抽不了"与"读不出能不能抽"。
// ⚠ 它到底是"累计已获得"还是"还剩几次"**未验证** —— 本条只钉读数，不钉语义。
func TestPlayAIUserStatusDistinguishesZeroFromMissing(t *testing.T) {
	before, perr := PlayAIParseUserStatus(playAITestEnvelope(t, playAITestUserStatusBody))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if before.TotalLotteryCount == nil || *before.TotalLotteryCount != 0 {
		t.Errorf("实拍 0 要读成 0（非 nil），实得 %v", before.TotalLotteryCount)
	}
	after, perr := PlayAIParseUserStatus(playAITestEnvelope(t,
		strings.Replace(playAITestUserStatusBody, `"totalLotteryCount":0`, `"totalLotteryCount":1`, 1)))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if after.TotalLotteryCount == nil || *after.TotalLotteryCount != 1 {
		t.Errorf("第二次读到 1，实得 %v", after.TotalLotteryCount)
	}

	// 三种"没有这一格"：缺键、给了 null —— 都读成 nil；result 整个没有 ⇒ 报错。
	for body, wantNil := range map[string]bool{
		`{"code":0,"msg":"success","result":{"showData":{}}}`:            true,
		`{"code":0,"msg":"success","result":{"totalLotteryCount":null}}`: true,
	} {
		got, perr := PlayAIParseUserStatus(playAITestEnvelope(t, body))
		if perr != nil {
			t.Fatalf("缺这一格不该报错: %v", perr)
		}
		if (got.TotalLotteryCount == nil) != wantNil {
			t.Errorf("%s ⇒ 应是 nil，实得 %v", body, got.TotalLotteryCount)
		}
	}
	_, perr = PlayAIParseUserStatus(playAITestEnvelope(t, `{"code":0,"msg":"success"}`))
	playAITestErrMessage(t, "user/info 缺 result（实拍两条都有）", perr)
	_, perr = PlayAIParseUserStatus(playAITestEnvelope(t, `{"code":0,"msg":"success","result":[]}`))
	playAITestErrMessage(t, "user/info 的 result 不是对象", perr)
}

// playAITestTaskProbe 把一条 [PlayAIUserTask] 摊成"键 → 有没有给 + 值"，供表驱动用。
func playAITestTaskProbe(task PlayAIUserTask) map[string]string {
	out := map[string]string{}
	if task.TaskID != nil {
		out["id"] = playAITestItoa(*task.TaskID)
	}
	if task.Name != nil {
		out["name"] = *task.Name
	}
	if task.Flag != nil {
		out["flag"] = *task.Flag
	}
	if task.Description != nil {
		out["description"] = *task.Description
	}
	if task.GroupID != nil {
		out["groupId"] = *task.GroupID
	}
	if task.State != nil {
		out["state"] = *task.State
	}
	if task.LimitType != nil {
		out["limitType"] = *task.LimitType
	}
	if task.RecordID != nil {
		out["recordid"] = *task.RecordID
	}
	if task.CurrStep != nil {
		out["currstep"] = playAITestItoa(*task.CurrStep)
	}
	if task.Process != nil {
		out["process"] = playAITestItoa(*task.Process)
	}
	return out
}

// TestPlayAITaskListParsesTheCapturedThreeTasks 逐字段钉夹具第 5 条。
//
// 两处最容易"读错但不报错"：
//   - `groupId` 是**驼峰**，而签到任务表那族是全小写 `groupid` —— 写成同一个键
//     会有一族静默读到 nil，而 nil 是合法的"上游没给"；
//   - `recordid` 实拍是 `null` 或**字符串**，从没数字形态 ⇒ 数字形态必须读成 nil，
//     "顺手"收数字就等于替上游宣布一条它没说过的规则。
func TestPlayAITaskListParsesTheCapturedThreeTasks(t *testing.T) {
	tasks, perr := PlayAIParseTaskList(playAITestEnvelope(t, playAITestTaskListBodyBefore))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(tasks) != 3 {
		t.Fatalf("实拍三条，实得 %d", len(tasks))
	}
	want := []map[string]string{
		{
			"id": "1", "name": "体验拍照问AI", "flag": "拍照问AI", "description": "抽奖次数+1",
			"groupId": "normal", "state": "WAIT", "limitType": "DAY",
			"currstep": "0", "process": "0",
		},
		{
			"id": "2", "name": "拍夏日关键词", "flag": "拍夏日关键词", "description": "抽奖次数+1",
			"groupId": "normal", "state": "WAIT", "limitType": "DAY",
			"currstep": "0", "process": "0",
		},
		{
			"id": "31", "name": "去小红书点赞", "flag": "小红书", "description": "抽奖次数+1",
			"groupId": "popup", "state": "FINISH", "limitType": "EVER",
			"recordid": "20466372", "currstep": "2", "process": "0",
		},
	}
	for i, expect := range want {
		got := playAITestTaskProbe(tasks[i])
		for key, value := range expect {
			if got[key] != value {
				t.Errorf("任务 %d 的 %s 应是 %q，实得 %q", i+1, key, value, got[key])
			}
		}
		// `recordid` 只在第三条出现：前两条**实拍是 null** ⇒ 必须读成"没给"。
		if _, ok := got["recordid"]; ok && expect["recordid"] == "" {
			t.Errorf("任务 %d 的 recordid 实拍是 null ⇒ 应是 nil，实得 %q", i+1, got["recordid"])
		}
		if _, ok := got["process"]; !ok {
			t.Errorf("任务 %d 的 process 实拍给了 0（不是缺键）⇒ 应是 0 而不是 nil", i+1)
		}
	}
}

// TestPlayAITaskListReadsTheSecondCaptureAfterTheStep 钉夹具第 10 条（同请求的第二次）。
//
// 它存在的理由：**同一段解析代码**要能读出两次之间的三处变化
// （`state` WAIT→FINISH、`recordid` null→字符串、`currstep` 0→3）。
// 反向改坏它谁会红：把 `recordid` 改用数字收法 ⇒ 第二条从字符串变成 nil ⇒ 红。
func TestPlayAITaskListReadsTheSecondCaptureAfterTheStep(t *testing.T) {
	tasks, perr := PlayAIParseTaskList(playAITestEnvelope(t, playAITestTaskListBodyAfter))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(tasks) != 3 {
		t.Fatalf("实拍三条，实得 %d", len(tasks))
	}
	got := playAITestTaskProbe(tasks[0])
	if got["id"] != "1" || got["state"] != "FINISH" || got["currstep"] != "3" || got["recordid"] != "20674897" {
		t.Errorf("上报后再查这一条应读到 FINISH/3/\"20674897\"，实得 %+v", got)
	}
	if got["process"] != "0" {
		t.Errorf("`process` 已完成时实拍**仍是 0** ⇒ 进度这一格不能只读 process，实得 %q", got["process"])
	}
	if second := playAITestTaskProbe(tasks[1]); second["recordid"] != "" {
		t.Errorf("第二条实拍仍是 null，实得 %q", second["recordid"])
	}
}

// TestPlayAITaskListRejectsUnexpectedShapes 钉"形状不对当场响"。
//
// 为什么这里选报错而不是"跳过读不出来那条"：任务表是本能力的判据来源，
// 一条读不出来的记录比"少一条"更危险 —— 调用方会以为那一步没做（然后再上报一次）。
func TestPlayAITaskListRejectsUnexpectedShapes(t *testing.T) {
	for name, body := range map[string]string{
		"缺 result":   `{"code":0,"msg":"success"}`,
		"result 是对象": `{"code":0,"msg":"success","result":{"id":1}}`,
		"条目不是对象":     `{"code":0,"msg":"success","result":["x"]}`,
	} {
		t.Run(name, func(t *testing.T) {
			tasks, perr := PlayAIParseTaskList(playAITestEnvelope(t, body))
			message := playAITestErrMessage(t, name, perr)
			if tasks != nil {
				t.Errorf("报错时不该交出半份结果，实得 %+v", tasks)
			}
			if !strings.Contains(message, "result") {
				t.Errorf("错误该点名 result，实得 %q", message)
			}
		})
	}
	// ⚠ 空数组是**合法**的（一条任务都没有 = 上游确实没排任务），与"没有 result 键"不同。
	tasks, perr := PlayAIParseTaskList(playAITestEnvelope(t, `{"code":0,"msg":"success","result":[]}`))
	if perr != nil {
		t.Fatalf("空数组不该报错，实得 %v", perr)
	}
	if len(tasks) != 0 {
		t.Errorf("应读出零条，实得 %d", len(tasks))
	}
}

// TestPlayAILotteryParsesTheCapturedPrize 逐字段钉夹具第 12 条的 prize。
//
// 两处值得单独说：
//   - `oid` 实拍是**数字**，本层统一成字符串带出（下一步进 URL 查询串）。
//     反向改坏它谁会红：换成"收成 int64 再格式化"或 float64 那条路 ⇒ 19 位那种会先丢精度，
//     下面另有一条 19 位的用例专门钉它。
//   - `firstMonthGet` / `high` 实拍是 `false` ⇒ 必须非 nil（false 是真实取值）。
//   - `description` / `extInfo` / `poster` 实拍**只有 null** ⇒ 本层刻意不声明：
//     给它们编一个类型就是替上游宣布"非 null 时长这样"。这里只钉"它们在场也不影响解析"。
func TestPlayAILotteryParsesTheCapturedPrize(t *testing.T) {
	got, perr := PlayAIParseLottery(playAITestEnvelope(t, playAITestLotteryBody))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.Prize == nil {
		t.Fatal("实拍有 prize，实得 nil")
	}
	prize := got.Prize
	if prize.PrizeID == nil || *prize.PrizeID != 260416004 {
		t.Errorf("prizeId 应是 260416004，实得 %v", prize.PrizeID)
	}
	if prize.PrizeType == nil || *prize.PrizeType != 1002 {
		t.Errorf("prizeType 应是 1002，实得 %v", prize.PrizeType)
	}
	if prize.PrizeName == nil || *prize.PrizeName != "0.5元支付宝红包" {
		t.Errorf("prizeName 应逐字是「0.5元支付宝红包」，实得 %v", playAITestStrOrEmpty(prize.PrizeName))
	}
	if prize.Type == nil || *prize.Type != "common" {
		t.Errorf("type 应是 common（取值集零其它样本 ⇒ 不解释），实得 %v", playAITestStrOrEmpty(prize.Type))
	}
	if prize.Oid == nil || *prize.Oid != "1248582079" {
		t.Errorf("oid 实拍是数字、要逐字转成字符串，实得 %v", playAITestStrOrEmpty(prize.Oid))
	}
	if prize.ButtonTxt == nil || *prize.ButtonTxt != "前往激活" {
		t.Errorf("buttonTxt 应是「前往激活」，实得 %v", playAITestStrOrEmpty(prize.ButtonTxt))
	}
	if prize.Title == nil || *prize.Title != "恭喜你获得" {
		t.Errorf("title 应是「恭喜你获得」，实得 %v", playAITestStrOrEmpty(prize.Title))
	}
	if prize.ButtonLink == nil || !strings.Contains(*prize.ButtonLink, "token=") {
		t.Errorf("buttonLink 要原样带出（含那个凭据型查询串，实拍值已脱敏）：实得 %v", playAITestStrOrEmpty(prize.ButtonLink))
	}
	if prize.Tips == nil || !strings.Contains(*prize.Tips, "奖品已下发到你的账号中") {
		t.Errorf("tips 是上游文案，原样存（不据此判成败）：实得 %v", playAITestStrOrEmpty(prize.Tips))
	}
	if prize.FirstMonthGet == nil || *prize.FirstMonthGet {
		t.Errorf("firstMonthGet 实拍 false ⇒ 必须是非 nil 的 false，实得 %v", prize.FirstMonthGet)
	}
	if prize.High == nil || *prize.High {
		t.Errorf("high 实拍 false ⇒ 必须是非 nil 的 false，实得 %v", prize.High)
	}

	// 19 位的 `oid`（数字形态）：本层的收法不许丢末几位。
	long, perr := PlayAIParseLottery(playAITestEnvelope(t,
		`{"code":0,"msg":"success","result":{"prize":{"oid":1234567890123456789}}}`))
	if perr != nil {
		t.Fatalf("只给一个 oid 也该解析，实得 %v", perr)
	}
	if long.Prize == nil || long.Prize.Oid == nil || *long.Prize.Oid != "1234567890123456789" {
		t.Errorf("19 位数字 oid 该逐字留住，实得 %v", playAITestStrOrEmpty(playAITestOid(long.Prize)))
	}
}

// TestPlayAILotteryMissingPrizeIsNotAParseFailure 钉"已经抽掉了"这一档不许红成失败。
//
// ⚠ `code:0 但没有 prize` **没有样本** —— 但这是**写**请求：把它报成解析错误，
// 调用方为了拿结论只能再抽一次。所以三档（缺 result / 缺 prize 键 / prize 是 null）
// 都交回"Prize == nil 且无错误"。
func TestPlayAILotteryMissingPrizeIsNotAParseFailure(t *testing.T) {
	for name, body := range map[string]string{
		"缺 result（同 ③ 的形态）":  `{"code":0,"msg":"success"}`,
		"result 里没有 prize 键": `{"code":0,"msg":"success","result":{}}`,
		"prize 是 null":       `{"code":0,"msg":"success","result":{"prize":null}}`,
	} {
		t.Run(name, func(t *testing.T) {
			got, perr := PlayAIParseLottery(playAITestEnvelope(t, body))
			if perr != nil {
				t.Fatalf("一次真成功的抽奖不许被读成失败（重跑 = 再抽一次）: %v", perr)
			}
			if got.Prize != nil {
				t.Errorf("应交回 nil 让调用方折叠，实得 %+v", got.Prize)
			}
		})
	}
	// 反方向：prize 在场但形态不是对象 ⇒ 那是"看不懂"，必须报错而不是 nil。
	if _, perr := PlayAIParseLottery(playAITestEnvelope(t,
		`{"code":0,"msg":"success","result":{"prize":[]}}`)); perr == nil {
		t.Error("prize 不是对象时报错")
	}
}

// TestPlayAIStepClickSuccessNeedsNoResultKey 钉夹具第 7 条那份**没有 result** 的成功响应。
//
// 反向改坏它谁会红：在 [playAINewyearPayload] 里把"没有 result"一律判成解析错误 ⇒
// 本条红 —— 而真跑时的表现是"一次成功的上报被读成失败"，而它是**写**操作。
func TestPlayAIStepClickSuccessNeedsNoResultKey(t *testing.T) {
	if perr := PlayAIParseStepClick(playAITestEnvelope(t, playAITestStepClickBody)); perr != nil {
		t.Fatalf("实拍的成功响应就是没有 result，实得 %v", perr)
	}
	// 非 0 码仍要红（别把"放过缺 result"顺手写成"放过一切"）。
	if perr := PlayAIParseStepClick(playAITestEnvelope(t, `{"code":3,"msg":"重复上报"}`)); perr == nil {
		t.Error("非 0 码不许被当成成功")
	}
}

// TestPlayAIRecognizeParsesTheCapturedData 逐字段钉夹具第 8 条的 data。
//
// 三处"看着正常其实读错"：`taskId` 是**字符串**形态的 19 位标识（过一遍数字类型会静默丢末几位，
// 而它将来是 `chat/v2/add` 的入参）；`hasText` 的 `false` 是真实取值；
// `content`/`hasQuestion`/`language` 实拍**只有 null** ⇒ 只能读成 nil（形态未验证）。
func TestPlayAIRecognizeParsesTheCapturedData(t *testing.T) {
	got, perr := PlayAIParseRecognize(playAITestEnvelope(t, playAITestRecognizeBody))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.TaskID == nil || *got.TaskID != "1991452320654084604" {
		t.Errorf("taskId 该按字符串逐字带出，实得 %v", playAITestStrOrEmpty(got.TaskID))
	}
	if got.FileID == nil || *got.FileID != "SYNTHETIC-FILE-ID" {
		t.Errorf("fileId 要交出去（清理那一族要的就是它），实得 %v", playAITestStrOrEmpty(got.FileID))
	}
	if got.HasText == nil || *got.HasText {
		t.Errorf("hasText 实拍 false ⇒ 非 nil 的 false，实得 %v", got.HasText)
	}
	if strings.Join(got.Recommend, "|") != "人物戴眼镜了吗|人物穿什么上衣|背景是什么颜色" {
		t.Errorf("recommendList 三条要逐字带出，实得 %+v", got.Recommend)
	}
	if got.Content != nil || got.HasQuestion != nil || got.Language != nil {
		t.Errorf("content/hasQuestion/language 实拍是 null ⇒ 读成 nil（非 null 形态零样本），实得 %+v", got)
	}
}

// TestPlayAIRecognizeNullVersusMissingAndBadListShape 钉 ⑤ 的"缺 / null / 异形"三档。
//
// ⚠ 这里钉的是**我们的收法**，不是上游的承诺：实拍只有一条，
// 里面 `hasText` 给了 `false`、`recommendList` 给了三条字符串。
func TestPlayAIRecognizeNullVersusMissingAndBadListShape(t *testing.T) {
	// 缺 `hasText` 与给 `false` 分得开。
	missing, perr := PlayAIParseRecognize(playAITestEnvelope(t,
		`{"code":"0000","message":"请求成功","success":true,"data":{}}`))
	if perr != nil {
		t.Fatalf("data 是空对象也该解析，实得 %v", perr)
	}
	if missing.HasText != nil || missing.Recommend != nil || missing.TaskID != nil {
		t.Errorf("全缺 ⇒ 全 nil，实得 %+v", missing)
	}
	// `hasText` 给数字 0 ⇒ 本层**只认真布尔**，读成 nil（不替它选方向）。
	numeric, perr := PlayAIParseRecognize(playAITestEnvelope(t,
		`{"code":"0000","message":"请求成功","success":true,"data":{"hasText":0}}`))
	if perr != nil {
		t.Fatalf("非布尔的可选格不该报错，实得 %v", perr)
	}
	if numeric.HasText != nil {
		t.Errorf("hasText 是数字 ⇒ 应是 nil，实得 %v", *numeric.HasText)
	}
	// `recommendList` 的异形：非数组、元素非字符串 ⇒ 报错（这一格会展开成数组，必须当场看见）。
	for name, body := range map[string]string{
		"不是数组":   `{"code":"0000","success":true,"data":{"recommendList":{"a":1}}}`,
		"元素非字符串": `{"code":"0000","success":true,"data":{"recommendList":[1]}}`,
	} {
		t.Run(name, func(t *testing.T) {
			_, perr := PlayAIParseRecognize(playAITestEnvelope(t, body))
			message := playAITestErrMessage(t, name, perr)
			if !strings.Contains(message, "recommendList") {
				t.Errorf("错误该点名 recommendList，实得 %q", message)
			}
		})
	}
	// 字符串形态的 `taskId` 只认字符串：数字形态读成 nil（零样本 ⇒ 不猜）。
	shapes, perr := PlayAIParseRecognize(playAITestEnvelope(t,
		`{"code":"0000","success":true,"data":{"taskId":1991452320654084604}}`))
	if perr != nil {
		t.Fatalf("数字形态的 taskId 不该报错（读不出来而已），实得 %v", perr)
	}
	if shapes.TaskID != nil {
		t.Errorf("实拍 taskId 是字符串 ⇒ 数字形态读成 nil，而不是替上游宣布形态，实得 %q", *shapes.TaskID)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 常量与夹具的字面一致性
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIConstantsMatchTheCapturedLiterals 钉那五个写死的字面量。
//
// 为什么值得单独一条：`marketName` 是**期次**的一部分（春日/夏日/秋日同一实现换期），
// 换一期时"只改常量、不改测试"这条通路必须被堵住 —— 新值进来时本条先红，
// 逼着改动的人**重新对一次夹具**，而不是在运行时带上一个过期的市场名。
func TestPlayAIConstantsMatchTheCapturedLiterals(t *testing.T) {
	for name, pair := range map[string][2]string{
		"marketName":    {PlayAIMarketName, "National_PlayAISpecial"},
		"source":        {PlayAISource, "app"},
		"key":           {PlayAIStepKeyClick, "click"},
		"channelId":     {PlayAIRecognizeChannelID, "101"},
		"recognizeType": {PlayAIRecognizeType, "1"},
		"sendType":      {PlayAIRecognizeSendType, "2"},
		"origin(⑤)":     {PlayAILocalOrigin, "mcloudlocal://yun.139.com"},
	} {
		if pair[0] != pair[1] {
			t.Errorf("%s 应逐字是 %q（夹具），实得 %q", name, pair[1], pair[0])
		}
	}
	if PlayAIRecognizeTimeoutMS != 30000 {
		t.Errorf("timeout 实拍 30000，实得 %d", PlayAIRecognizeTimeoutMS)
	}
	if !PlayAIRecognizeUploadToCloud {
		t.Error("uploadToCloud 实拍只有 true 一档（false 零样本 ⇒ 不参数化）")
	}
	if PlayAIRecognizeCodeOK != "0000" {
		t.Errorf("⑤ 的成功码是**字符串** \"0000\"，实得 %q", PlayAIRecognizeCodeOK)
	}
	// ①…④ 的主机就是 [BaseMcloud]：本文件不该另立一个同主机常量。
	if !strings.HasPrefix(PlayAIHostAI, "https://ai.yun.139.com") {
		t.Errorf("⑤ 的主机应是 ai.yun.139.com，实得 %q", PlayAIHostAI)
	}
	if strings.HasPrefix(PlayAIHostAI, BaseMcloud) {
		t.Error("⑤ 与 ①…④ 不同主机：两个常量不许被\"顺手统一\"")
	}
}

// playAITestItoa 给上面的期望字符串用（避免把 strconv 的调用散在各处）。
func playAITestItoa(value int64) string { return strconv.FormatInt(value, 10) }

// playAITestStrOrEmpty 让指针在错误信息里看得懂（nil 与空串是两回事）。
func playAITestStrOrEmpty(value *string) string {
	if value == nil {
		return "<nil>"
	}
	return *value
}

// playAITestOid 取 prize 的 oid（供上面那条打印用）。
func playAITestOid(prize *PlayAIPrize) *string {
	if prize == nil {
		return nil
	}
	return prize.Oid
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层：五口 `(u *Upstream).PlayAI*`
//
// 替身用包内既有的 [scriptedClient]（见 upstream_test.go：它把收到的请求留下来供断言，
// 不联网、不起 server）—— 与 `app_signin_test.go` / `app_funai_test.go` 同一套路。
//
// 这一节钉的是**编排边界**：本层一次调用**只发一条**，且失败**不自动重发**。
// ③④⑤ 三口是写操作（上报一步 / 抽一次 / 落一张图到云盘），
// "解析失败就再抽一次"这种兜底写进本层，表现是**真的多抽一次**，
// 而它看起来只是在"处理一个错误"。
// ─────────────────────────────────────────────────────────────────────────────

// TestPlayAIUserStatusSendsExactlyOneGetAndParsesTheReply 钉 ① 的发送层。
//
// 反向改坏它谁会红：在 `PlayAIUserStatus` 里"顺手再查一次任务表"（把编排写进本层）
// ⇒ `len(requests) != 1` 红；换主机/换 path ⇒ URL 断言红。
func TestPlayAIUserStatusSendsExactlyOneGetAndParsesTheReply(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(playAITestUserStatusBody))}
	up := NewUpstream(client)

	got, perr := up.PlayAIUserStatus(context.Background(), "JWT-ONE", playAITestReferer, "true")
	if perr != nil {
		t.Fatalf("失败: %v", perr)
	}
	if got.TotalLotteryCount == nil || *got.TotalLotteryCount != 0 {
		t.Errorf("totalLotteryCount 实拍 0 ⇒ 应是非 nil 的 0，实得 %v", got.TotalLotteryCount)
	}
	if len(client.requests) != 1 {
		t.Fatalf("本层一次调用应只发一条（编排不是这一层的事），实得 %d 条", len(client.requests))
	}
	sent := client.requests[0]
	if want := "https://m.mcloud.139.com/ycloud/newyear/user/info?marketName=National_PlayAISpecial"; sent.URL != want {
		t.Errorf("发到线上的 URL 应是 %q，实得 %q", want, sent.URL)
	}
	if sent.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", sent.Method)
	}
	if got := headerValue(sent.Headers, "jwttoken"); got != "JWT-ONE" {
		t.Errorf("票要真的带上（值是合成的），实得 %q", got)
	}
	if got, want := playAITestHeaderNames(sent), "accept,jwttoken,referer,showloading"; got != want {
		t.Errorf("发出去的头应恰好是 [%s]，实得 [%s]", want, got)
	}
}

// TestPlayAITaskListSendsExactlyOneGetAndParsesThreeTasks 钉 ② 的发送层。
func TestPlayAITaskListSendsExactlyOneGetAndParsesThreeTasks(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(playAITestTaskListBodyBefore))}
	up := NewUpstream(client)

	got, perr := up.PlayAITaskList(context.Background(), "JWT-ONE", playAITestReferer, "true")
	if perr != nil {
		t.Fatalf("失败: %v", perr)
	}
	if len(got) != 3 {
		t.Fatalf("实拍三条，实得 %d 条", len(got))
	}
	if len(client.requests) != 1 {
		t.Fatalf("应只发一条，实得 %d", len(client.requests))
	}
	if want := "https://m.mcloud.139.com/ycloud/newyear/task/list?marketName=National_PlayAISpecial&source=app"; client.requests[0].URL != want {
		t.Errorf("实得 %q", client.requests[0].URL)
	}
}

// TestPlayAIStepClickSendsOnePostWithTheGivenTaskID 钉 ③ 的发送层（**写**）。
//
// 反向改坏它谁会红：把 body 里的 taskId 写成实拍那个 `1` ⇒ 传 31 时字节断言红；
// 加一层"失败再来一次" ⇒ `len(requests) != 1` 红。
func TestPlayAIStepClickSendsOnePostWithTheGivenTaskID(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(playAITestStepClickBody))}
	up := NewUpstream(client)

	// ③ 的成功响应**连 result 都没有** ⇒ 交回 nil（不报错），这一格只能在发送层验。
	if perr := up.PlayAIStepClick(context.Background(), "JWT-ONE", playAITestReferer, "true", 31); perr != nil {
		t.Fatalf("没有 result 的成功响应不该被读成失败: %v", perr)
	}
	if len(client.requests) != 1 {
		t.Fatalf("一次调用只该上报一次，实得 %d 条", len(client.requests))
	}
	sent := client.requests[0]
	if want := `{"key":"click","marketName":"National_PlayAISpecial","source":"app","taskId":31}`; string(sent.Body) != want {
		t.Errorf("发出去的 body 应是 %s，实得 %s", want, string(sent.Body))
	}
	if n := countHeader(sent.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一个，实得 %d 个", n)
	}
	if sent.IsRetryable() {
		t.Error("写请求且无幂等键 ⇒ 不该被判定为可重试（否则传输层会替我们再上报一步）")
	}
}

// TestPlayAIStepClickBusinessFailureIsNotRetried ⭐ 非零码：一条请求 + 一个错误，**不重发**。
//
// 这条与上面那条是一对：成功与失败都必须只发一条。
// 反向改坏它谁会红：给 `PlayAIStepClick` 加"code!=0 就重试一次" ⇒ 计数变 2。
func TestPlayAIStepClickBusinessFailureIsNotRetried(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(`{"code":7,"msg":"合成失败文案"}`))}
	up := NewUpstream(client)

	perr := up.PlayAIStepClick(context.Background(), "JWT-ONE", playAITestReferer, "true", 1)
	message := playAITestErrMessage(t, "③ 非零码", perr)
	if !strings.Contains(message, "code=7") || !strings.Contains(message, "合成失败文案") {
		t.Errorf("错误里该有码 + 上游原文（这一族非零码零样本 ⇒ 只透传，不翻译），实得 %q", message)
	}
	if len(client.requests) != 1 {
		t.Errorf("失败时也只该发一条，实得 %d 条", len(client.requests))
	}
	if perr.Kind() != core.KindBadRequest {
		t.Errorf("业务失败档实得 Kind=%v", perr.Kind())
	}
}

// TestPlayAILotterySendsOnePostAndReturnsThePrize 钉 ④ 的发送层（**写**：一次抽一次）。
func TestPlayAILotterySendsOnePostAndReturnsThePrize(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(playAITestLotteryBody))}
	up := NewUpstream(client)

	got, perr := up.PlayAILottery(context.Background(), "JWT-ONE", playAITestReferer, "true")
	if perr != nil {
		t.Fatalf("失败: %v", perr)
	}
	if len(client.requests) != 1 {
		t.Fatalf("一次调用只该抽一次（循环是调用方的事），实得 %d 条", len(client.requests))
	}
	if got.Prize == nil || got.Prize.PrizeName == nil || *got.Prize.PrizeName != "0.5元支付宝红包" {
		t.Errorf("prize 应带出 prizeName，实得 prize=%v / oid=%v",
			playAITestStrOrEmpty(got.Prize.PrizeName), playAITestOid(got.Prize))
	}
	if want := `{"marketName":"National_PlayAISpecial","source":"app"}`; string(client.requests[0].Body) != want {
		t.Errorf("发出去的 body 应是 %s，实得 %s", want, string(client.requests[0].Body))
	}
	if client.requests[0].IsRetryable() {
		t.Error("抽奖不该被判定为可重试")
	}
}

// TestPlayAIRecognizeSendsOnePostWithInjectedNativeHeaders 钉 ⑤ 的发送层。
//
// 反向改坏它谁会红：`PlayAIRecognize` 里把注入的 `authorization`/`tid` 换成公共来源
// （"反正都叫票"）⇒ 那两个头的断言红；必填校验挪到发送之后 ⇒ `len(requests) != 0` 红。
func TestPlayAIRecognizeSendsOnePostWithInjectedNativeHeaders(t *testing.T) {
	params := PlayAIRecognizeParams{
		UserID: "1990000000000000001", DataURL: "data:image/jpg;base64,QUJDRA==", ImageExt: "jpg",
	}
	t.Run("发得出去的那一条", func(t *testing.T) {
		client := &scriptedClient{response: core.NewResponse(200, []byte(playAITestRecognizeBody))}
		up := NewUpstream(client)
		got, perr := up.PlayAIRecognize(context.Background(), playAITestProfile(), 1750000000000,
			"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC", params)
		if perr != nil {
			t.Fatalf("失败: %v", perr)
		}
		if got.TaskID == nil || *got.TaskID != "1991452320654084604" {
			t.Errorf("taskId 要原样带出（19 位过一遍数字会丢精度），实得 %v", playAITestStrOrEmpty(got.TaskID))
		}
		if got.FileID == nil || *got.FileID == "" {
			t.Error("fileId 要交出去：清理那一族（`hcy/file/get`）要的就是它")
		}
		if len(client.requests) != 1 {
			t.Fatalf("一次调用只该识别一次（它会落一张图），实得 %d 条", len(client.requests))
		}
		sent := client.requests[0]
		if !strings.HasPrefix(sent.URL, "https://ai.yun.139.com/") {
			t.Errorf("⑤ 与其余四条不同主机，实得 %q", sent.URL)
		}
		if got := headerValue(sent.Headers, "authorization"); got != "Basic U1lOVEhFVElDLUFVVEg=" {
			t.Errorf("Authorization 应是注入的合成值，实得 %q", got)
		}
		if got := headerValue(sent.Headers, "x-yun-tid"); got != "TID-SYNTHETIC" {
			t.Errorf("x-yun-tid 应是注入的合成值，实得 %q", got)
		}
		if !strings.Contains(string(sent.Body), `"userId":"1990000000000000001"`) {
			t.Errorf("userId 由调用方注入（填错会打到别人账号上），实得 %s", string(sent.Body))
		}
		if sent.IsRetryable() {
			t.Error("落盘一张图的请求不该被判定为可重试")
		}
	})
	t.Run("必填没过就一条都不发", func(t *testing.T) {
		client := &scriptedClient{response: core.NewResponse(200, []byte(playAITestRecognizeBody))}
		up := NewUpstream(client)
		_, perr := up.PlayAIRecognize(context.Background(), playAITestProfile(), 1750000000000,
			"Basic U1lOVEhFVElDLUFVVEg=", "TID-SYNTHETIC",
			PlayAIRecognizeParams{UserID: "", DataURL: params.DataURL, ImageExt: params.ImageExt})
		playAITestErrMessage(t, "空 userId", perr)
		if len(client.requests) != 0 {
			t.Errorf("校验没过时一条都不该发（发出去就是拿别人的账号标识去落图），实得 %d 条", len(client.requests))
		}
	})
}

// TestPlayAISendLayerSharesTheCommonStatusMapping 钉"五口都走同一个 [Upstream.Call]"。
//
// 拿 ① 验一条非 2xx：错误走公共映射，且**不带响应体原文**
// （上游错误体里常常直接回显凭据，见 [errorFromStatus] 那段理由）。
// 反向改坏它谁会红：哪一口偷偷用自己的一份 HTTP 调用、或自己解析错误体，
// 这条就会在那一口上变成"消息里有响应体原文"或 Kind 不对。
func TestPlayAISendLayerSharesTheCommonStatusMapping(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(429,
		[]byte(`{"msg":"too many","authorization":"SYNTHETIC-ECHOED-CREDENTIAL-DO-NOT-LOG"}`))}
	up := NewUpstream(client)

	_, perr := up.PlayAIUserStatus(context.Background(), "JWT-ONE", playAITestReferer, "true")
	message := playAITestErrMessage(t, "非 2xx", perr)
	if perr.Kind() != core.KindRateLimited {
		t.Errorf("实得 Kind=%v", perr.Kind())
	}
	if strings.Contains(message, "SYNTHETIC-ECHOED") {
		t.Errorf("错误详情里不该出现响应体原文，实得 %q", message)
	}
}
