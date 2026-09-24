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
// ⚠ 本文件**不**判"上游收不收这个 key / 这个 id"—— 一次真打都没有。
// 钉的全是"我们发出去的是不是那一条"。

import (
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
