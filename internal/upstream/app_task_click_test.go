package upstream

// 打卡与入口上报这一族（#9 写侧地基）的用例。
//
// 三条"抄一遍就会漂"的事，各配一条反向改坏：
//
//  1. **域与路径**：App 任务矩阵那一发打在 `m.mcloud.139.com/ycloud/signin/task/click`
//     （实拍两条：id 597 / 547）。反向改坏 = 复用 `app_notice.go` 那个 `:7071` 的构造器
//     ⇒ 请求"看起来一模一样、也能回 code 0"，但打到另一个入口上（夹具里那两条同路径
//     不同域的样本就是这件事的证据），而这一族没有任何对账能发现。
//  2. **`key` 是调用方给的**：写死成 `task` 的话，将来 409 那一发（旧实现给 `task2`）
//     会静默发错。
//  3. **mrp 那一发的 `deviceId` 必须过 [PercentEncode]**：它是 base64，
//     `+` 不编码到查询串里表示**空格** —— 服务端解出另一个值且不报错。
//
// ⚠ 本文件判的是"我们发出去的是不是那一条"，以及"响应怎么读"。
// 2026-09-26 第廿四趟起，这一族**有过真打**（走生产入口 `RunAccount`，两个测试号各一趟：
// 打卡各若干发 + 入口上报各一发，全部 HTTP 200 且 `code` 位读成 0），逐字读数见
// `docs/capture/app-task-run-account-samples.txt`。那趟还单独量了 `code` 的**类型**：
// 入口上报那一发是**数字** `0`（打卡那一族同趟打出的是修复前的两可形态 ⇒ 类型仍未定，
// 但 [asInt64] 两条分支都认，所以不影响判定）。
// 但"回了 0"**不等于**"上游认了这条任务"—— 当天回读任务表显示一条都没翻（R24），
// 所以这里仍不判任何 id/key 的语义。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// taskClickReferer 是合成 referer（值本身不参与判据，只要每发都带上）。
const taskClickReferer = "https://referer.example/newsignin"

// TestTaskClickRequestIsTheCapturedLine 逐字钉实拍那一条（`3d.json` 的 id 597）。
func TestTaskClickRequestIsTheCapturedLine(t *testing.T) {
	request, perr := TaskClickRequest("JWT-1", taskClickReferer, TaskClickKeyDefault, 597)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	const want = "https://m.mcloud.139.com/ycloud/signin/task/click?key=task&id=597"
	if request.URL != want {
		t.Errorf("URL 应逐字为 %s，实得 %s", want, request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("GET 不该带 body，实得 %q", request.Body)
	}
	// 头面与实拍同一套 H5 头（夹具里 deviceid/jwttoken/showloading/activityid/appversion 都发出去了）。
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("jwttoken 应是合成票，实得 %q", got)
	}
	if got := headerValue(request.Headers, "activityid"); got != MarketName {
		t.Errorf("activityid 应是 %q，实得 %q", MarketName, got)
	}
	for _, name := range []string{"appversion", "showloading", "referer"} {
		if n := countHeader(request.Headers, name); n != 1 {
			t.Errorf("%s 应恰好一枚，实得 %d", name, n)
		}
	}
	// ⚠ 反向对照：这一族**不**打 `:7071`（那是 notice 那条链的域）。
	if strings.Contains(request.URL, "feixin") {
		t.Errorf("App 任务矩阵的打卡不打彩云域（实拍那两条在 m.mcloud.139.com），实得 %s", request.URL)
	}
}

// TestTaskClickKeyComesFromTheCaller：`task2` 那一发要发得出去，且只改 key 这一段。
//
// 反向改坏：把 key 写成常量 ⇒ 这一条红，而 409 那条 runner 会静默带错 key。
func TestTaskClickKeyComesFromTheCaller(t *testing.T) {
	request, perr := TaskClickRequest("JWT-1", taskClickReferer, TaskClickKeySecond, 409)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	const want = "https://m.mcloud.139.com/ycloud/signin/task/click?key=task2&id=409"
	if request.URL != want {
		t.Errorf("实得 %s，应是 %s", request.URL, want)
	}
}

// TestTaskClickRefusesNonPositiveID：id ≤ 0 在构造层就拦，不回一个"能发的"请求。
//
// 为什么值得拦：这一发是**写请求**，`id=0` 是拿用户的账去试探上游的容错，
// 而旧实现里没有这种调用点（它的 id 全部来自任务表）。
func TestTaskClickRefusesNonPositiveID(t *testing.T) {
	for _, id := range []int64{0, -1} {
		if _, perr := TaskClickRequest("JWT-1", taskClickReferer, TaskClickKeyDefault, id); perr == nil {
			t.Errorf("id=%d 应被拒绝（不发试探请求）", id)
		}
	}
}

// TestParseTaskClickAcceptsTheCapturedEnvelope 实拍那一条：只有 code 与 msg。
func TestParseTaskClickAcceptsTheCapturedEnvelope(t *testing.T) {
	click, perr := ParseTaskClick(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
	}))
	if perr != nil {
		t.Fatalf("实拍形状应判成功，实得 %v", perr)
	}
	if click.Message == nil || *click.Message != "success" {
		t.Errorf("顶层 msg 应交出去，实得 %v", click.Message)
	}
	if click.Notice != nil {
		t.Errorf("实拍没有 result.msg，这一格应是 nil，实得 %q", *click.Notice)
	}
}

// TestParseTaskClickReadsResultMsgWhenPresent 钉"上游给了 result"那一档。
//
// ⚠ 这一档**零实拍**（四条样本全无 `result`），形状出自旧实现读 `result.result.msg`。
// 带着它只为"给了就交出去给人看"，不参与任何判据。
func TestParseTaskClickReadsResultMsgWhenPresent(t *testing.T) {
	click, perr := ParseTaskClick(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"msg": "任务已完成"},
	}))
	if perr != nil {
		t.Fatalf("应判成功: %v", perr)
	}
	if click.Notice == nil || *click.Notice != "任务已完成" {
		t.Errorf("result.msg 应读出来，实得 %v", click.Notice)
	}
}

// TestParseTaskClickFailsOnNonZeroCode：非 0 一律失败，不读成语义码。
func TestParseTaskClickFailsOnNonZeroCode(t *testing.T) {
	_, perr := ParseTaskClick(envelopeFrom(t, map[string]any{
		"code": 1, "msg": "系统繁忙",
	}))
	if perr == nil {
		t.Fatal("code=1 必须判失败")
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("原文要留在错误里便于排障，实得 %v", perr)
	}
}

// TestMrpDoTaskRequestEncodesTheDeviceID 钉这一发的三段：域、`client=app`、deviceId 编码。
//
// ⚠ **整条链零实拍**（文件头第二节）。这里钉的是"我们发出去的形状"，
// 不是"上游会接受"。反向改坏：把 `PercentEncode` 换成原样拼接 ⇒
// 含 `+` 的 base64 设备号会被服务端解成空格，而且**不报错**。
func TestMrpDoTaskRequestEncodesTheDeviceID(t *testing.T) {
	request, perr := MrpDoTaskRequest("JWT-1", taskClickReferer, "ab+cd/ef==")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	const want = "https://mrp.mcloud.139.com/market/signin/page/doTask" +
		"?client=app&deviceId=ab%2Bcd%2Fef%3D%3D"
	if request.URL != want {
		t.Errorf("实得 %s，应是 %s", request.URL, want)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("票要带在 jwttoken 上（AGENTS §1.12：这一族用签到页那张），实得 %q", got)
	}
}

// TestMrpDoTaskRequestRefusesEmptyDeviceID：空设备号不发（旧实现会发，我们不跟这一步）。
func TestMrpDoTaskRequestRefusesEmptyDeviceID(t *testing.T) {
	if _, perr := MrpDoTaskRequest("JWT-1", taskClickReferer, ""); perr == nil {
		t.Fatal("空 deviceId 应被拒绝")
	}
}

// mrpDoTask 响应判定的三档。夹具里那一份 `{"code":0,"msg":"success"}` **逐字**取自
// 2026-09-26 第廿四趟真账号整趟真跑（两个号各一趟，回的就是这一串）——
// 这一族从前零样本、只判 `success`，那份实拍就是"要回来补判定"的那一句的兑现。
func TestMrpDoTaskAcceptsTheCapturedBody(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(`{"code":0,"msg":"success"}`))}
	if perr := NewUpstream(client).AppMrpDoTask(
		context.Background(), "JWT-1", taskClickReferer, "BSYNTH-DEVICE",
	); perr != nil {
		t.Fatalf("实拍那一档应判成功，实得 %v", perr)
	}
}

// TestMrpDoTaskFailsOnNonZeroCode 钉"补上的那一半判定"。
//
// ⚠ 这一档的**码值本身零样本**（今天只有 `0` 一份实拍）：这里要的是"非 0 会被交回去"，
// 不是"1 就是系统繁忙"。反向改坏：把 `EnsureOKWithHint` 换回从前那句只判 `success`
// ⇒ 这一条红。
func TestMrpDoTaskFailsOnNonZeroCode(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200,
		[]byte(`{"code":1,"msg":"合成失败"}`))}
	perr := NewUpstream(client).AppMrpDoTask(
		context.Background(), "JWT-1", taskClickReferer, "BSYNTH-DEVICE")
	if perr == nil {
		t.Fatal("非 0 必须判失败（旧实现连错误都吞）")
	}
	if !strings.Contains(perr.Error(), "合成失败") {
		t.Errorf("上游原话要留在错误里，实得 %v", perr)
	}
}

// TestMrpDoTaskWithoutCodeIsNotAFailure ⭐ 缺 `code` 那一格**不**判失败。
//
// 为什么单独钉：这一口只有一次实拍（带回 `code`），没有"上游必然带这一格"的证据，
// 而旧实现压根不读它的响应。把缺字段判成失败，等于用一份样本否掉一条今天能跑通的枪
// —— 它挂在 409 / 1005 两条 runner 上，误报会把整条玩法带红。
// 判据落在 [Envelope.EnsureOK] 的 `!hasCode || code == 0`：走的是同一份实现，
// 但这条用例保证将来没人"顺手收紧"。
//
// ⚠ "上游会不会回不带 `code` 的体"本身**未测试**（零样本）。
func TestMrpDoTaskWithoutCodeIsNotAFailure(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200, []byte(`{"msg":"success"}`))}
	if perr := NewUpstream(client).AppMrpDoTask(
		context.Background(), "JWT-1", taskClickReferer, "BSYNTH-DEVICE",
	); perr != nil {
		t.Errorf("缺 code 应沿『没有信封判定』放行，实得 %v", perr)
	}
}

// TestMrpDoTaskRejectsSuccessFalse：`success:false` 优先于 code（与信封那一族同一条纪律）。
func TestMrpDoTaskRejectsSuccessFalse(t *testing.T) {
	client := &scriptedClient{response: core.NewResponse(200,
		[]byte(`{"code":0,"success":false,"msg":"合成拒绝"}`))}
	if perr := NewUpstream(client).AppMrpDoTask(
		context.Background(), "JWT-1", taskClickReferer, "BSYNTH-DEVICE",
	); perr == nil {
		t.Fatal("success=false 必须判失败，且优先于 code=0")
	}
}
