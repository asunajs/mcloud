package upstream

// 任务拓展**读口**（`GET /ycloud/signin/page/taskExpansion`）的用例。
//
// 四条"抄一遍就会漂"的事，各配一条反向改坏：
//
//  1. **域与路径**：夹具实拍那两条打在 `m.mcloud.139.com/ycloud/signin/page/taskExpansion`，
//     而旧实现在 `mrp.mcloud.139.com/market/signin/page/taskExpansion`。
//     反向改坏 = 复用旧 TS 那一口 ⇒ 请求"看起来也是 taskExpansion"，但打到另一个入口，
//     而这一族没有任何对账能发现（坑表 #151：对齐只借形状、不借事实）。
//  2. **不带查询串**：夹具的 URL 逐字没有 `?client=`，旧 TS 那句也是裸 URL。
//     反向改坏 = "顺手对齐同族的 `infoV3?client=app`" ⇒ 发出的是**另一个请求**。
//  3. **实拍响应逐字**：那一条给的是 `curMonthBackup:true` + `curMonthTaskRecordCount:0`
//     + `nextMonthTaskRecordCount:358`，**没有** `acceptDate` ⇒ 五格读到、一格为 nil。
//  4. ⭐ **nil ≠ false**：上游没给 `curMonthBackup` 时必须得到 nil，不能是 `false`。
//     这一条不是洁癖：编排层拿它决定"要不要往用户网盘传一个文件凑备份"，
//     而那是不可回滚的动作。反向改坏 = 把缺字段折成 `false` ⇒ 本文件那条红。
//
// ⚠ 本文件**不**判"上游认不认这个票 / 这个 deviceid"—— 一次真打都没有（真跑在
// `internal/app` 那侧的活体探针）。钉的全是"我们发出去的是不是那一条"、
// "读回来的六格是不是上游说的那六格"。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// taskExpansionCapturedBody 是夹具 `test/fixtures/http/3d.json` 里那两条的响应体，
// **逐字**照抄（两条内容相同）。它是本仓关于这个端点的唯一实拍读数。
const taskExpansionCapturedBody = `{"code":0,"msg":"success","result":` +
	`{"curMonthBackup":true,"curMonthBackupTaskAccept":false,` +
	`"curMonthTaskRecordCount":0,"nextMonthTaskRecordCount":358,"preMonthBackup":false}}`

func TestTaskExpansionRequestIsTheCapturedLine(t *testing.T) {
	id := "SYNTH_DEVICE_ID"
	request := TaskExpansionRequest("JWT-1", "https://referer.example/newsignin", &id)

	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	// ⚠ 逐字：域是 m.mcloud、路径是 /ycloud/signin/page/、**没有查询串**。
	const want = "https://m.mcloud.139.com/ycloud/signin/page/taskExpansion"
	if request.URL != want {
		t.Errorf("URL 应逐字为 %s，实得 %s", want, request.URL)
	}
	if strings.Contains(request.URL, "?") {
		t.Errorf("实拍那两条不带任何查询参数，多发一个是**另一个请求**: %s", request.URL)
	}
	if strings.Contains(request.URL, "client=") {
		t.Errorf("别顺手对齐同族的 infoV3?client=app: %s", request.URL)
	}
	// ⚠ 反向对照：不走旧 TS 那一口（mrp 域 / market 路径）。
	if strings.Contains(request.URL, "mrp.mcloud.139.com") || strings.Contains(request.URL, "/market/") {
		t.Errorf("应打实拍那一条的域与路径，实得 %s", request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("GET 不该带 body，实得 %q", request.Body)
	}

	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("jwttoken 应是合成票，实得 %q", got)
	}
	if got := headerValue(request.Headers, "activityid"); got != MarketName {
		t.Errorf("activityid 应是 %q，实得 %q", MarketName, got)
	}
	// 实拍四条：showloading / activityid / appversion / referer 都发出去了。
	for _, name := range []string{"appversion", "referer", "deviceid"} {
		if n := countHeader(request.Headers, name); n != 1 {
			t.Errorf("%s 应恰好一枚，实得 %d", name, n)
		}
	}
	if got := headerValue(request.Headers, "deviceid"); got != "SYNTH_DEVICE_ID" {
		t.Errorf("deviceid 头应是给进去的那个值，实得 %q", got)
	}
	if got := headerValue(request.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 应是 true（同页那几条几乎都是 true）: %q", got)
	}
	// ⚠ 这一口**不**带 isdeviceid：实拍那两条的头里没有它，而 `receiveV3` 才有。
	if countHeader(request.Headers, "isdeviceid") != 0 {
		t.Error("taskExpansion 不该带 isdeviceid 头（别照 receiveV3 抄）")
	}
}

// 拿不到 deviceID 的账号是**存在**的（设备档案那一步失败时）⇒ 这一口仍该能发出去。
// 做成必填会让"读一眼"这种只读动作被一个可选的头卡死。
func TestTaskExpansionWithoutDeviceIDOmitsTheHeader(t *testing.T) {
	request := TaskExpansionRequest("JWT-1", "https://referer.example/newsignin", nil)
	if countHeader(request.Headers, "deviceid") != 0 {
		t.Error("不给 deviceID 时不该带 deviceid 头")
	}
	// 其余那套 H5 头不受影响。
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("少了 deviceid 不该连带少掉票: %q", got)
	}
}

// ⭐ 实拍那一条**逐字**解析：五格读到、`acceptDate` 为 nil。
//
// 这一条同时钉住"这一族在真账号上的读数形状"：`curMonthBackup` 是**真布尔**、
// 两个计数是**数字**。编排层"只认真布尔"那条判据的依据就在这里。
func TestParseTaskExpansionTheCapturedResponse(t *testing.T) {
	envelope, perr := ParseEnvelope([]byte(taskExpansionCapturedBody))
	if perr != nil {
		t.Fatalf("夹具原文解不开（那说明常量抄错了）: %v", perr)
	}
	got, perr := ParseTaskExpansion(envelope)
	if perr != nil {
		t.Fatalf("实拍那一条必须解析成功: %v", perr)
	}
	if got.CurMonthBackup == nil || *got.CurMonthBackup != true {
		t.Errorf("curMonthBackup 应是 true（这个号本月开着备份）: %v", got.CurMonthBackup)
	}
	if got.PreMonthBackup == nil || *got.PreMonthBackup != false {
		t.Errorf("preMonthBackup 应是 false: %v", got.PreMonthBackup)
	}
	if got.CurMonthBackupTaskAccept == nil || *got.CurMonthBackupTaskAccept != false {
		t.Errorf("curMonthBackupTaskAccept 应是 false: %v", got.CurMonthBackupTaskAccept)
	}
	if got.CurMonthTaskRecordCount == nil || *got.CurMonthTaskRecordCount != 0 {
		t.Errorf("curMonthTaskRecordCount 应是 0（本月没记录 ⇒ 不去领）: %v", got.CurMonthTaskRecordCount)
	}
	if got.NextMonthTaskRecordCount == nil || *got.NextMonthTaskRecordCount != 358 {
		t.Errorf("nextMonthTaskRecordCount 应是 358: %v", got.NextMonthTaskRecordCount)
	}
	// ⚠ 实拍**没有** acceptDate ⇒ nil。这一格直接决定领取那一发是裸 GET 还是带日期。
	if got.AcceptDate != nil {
		t.Errorf("实拍那条没有 acceptDate，应为 nil，实得 %q", *got.AcceptDate)
	}

	// 0 与"没读到"必须可分：编排层判的是 `> 0` 才去领。
	if got.CurMonthTaskRecordCount == nil {
		t.Error("读到 0 必须是指向 0 的指针，不是 nil")
	}
}

// 没有 `result` **不算失败**：这一口是只读的，报红会把一条"上游没说"变成
// 每天一条失败历史 + 一次 `onlyError` 推送。
func TestParseTaskExpansionToleratesAMissingResult(t *testing.T) {
	got, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
	}))
	if perr != nil {
		t.Fatalf("没有 result 不该判失败: %v", perr)
	}
	if got.CurMonthBackup != nil || got.CurMonthTaskRecordCount != nil || got.AcceptDate != nil {
		t.Error("没给时应全是 nil")
	}
	// result 不是对象（比如是个数组 / 字符串）也走同一条路：交回"什么都没读到"。
	if _, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": []any{1, 2},
	})); perr != nil {
		t.Errorf("result 形状意外不该判失败: %v", perr)
	}
}

// ⭐⭐ "没读到"不许折成 `false` —— 那是"去用户网盘写一个文件"的开关。
//
// 上游把布尔改成 `1` / `"true"` 这类形状时，本层得到 nil，编排层因此**不发**那一发
// 上传。反向改坏：在这里加一句 `if v, ok := obj["curMonthBackup"].(string); ok { … }`
// 把它认成 true/false ⇒ 这一条红。
func TestParseTaskExpansionKeepsUnknownApartFromBackupOff(t *testing.T) {
	for _, shape := range []any{"false", 0, 1, "true", nil} {
		got, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
			"code": 0, "msg": "success",
			"result": map[string]any{"curMonthBackup": shape},
		}))
		if perr != nil {
			t.Fatalf("非布尔的 curMonthBackup=%v 不该判失败: %v", shape, perr)
		}
		if got.CurMonthBackup != nil {
			t.Errorf("curMonthBackup=%v 不是 JSON 布尔，应读成 nil（未知 ⇒ 不写网盘），实得 %v",
				shape, *got.CurMonthBackup)
		}
	}
	// 对照：真布尔两档都要读到，且 0 / false 各自不能被折成 nil。
	off, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"curMonthBackup": false},
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if off.CurMonthBackup == nil || *off.CurMonthBackup != false {
		t.Error("false 必须读成 false（这才是「凑备份」那一支的入口）")
	}
}

// 字符串型计数不认：与上一条同理，"本月记录数"是**要不要发那一发领取**的判据。
func TestParseTaskExpansionCountsOnlyAcceptNumbers(t *testing.T) {
	got, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{
			"curMonthTaskRecordCount":  "12",
			"nextMonthTaskRecordCount": 358,
		},
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.CurMonthTaskRecordCount != nil {
		t.Errorf("字符串计数应读成 nil，实得 %d", *got.CurMonthTaskRecordCount)
	}
	if got.NextMonthTaskRecordCount == nil || *got.NextMonthTaskRecordCount != 358 {
		t.Errorf("数字计数应正常读出: %v", got.NextMonthTaskRecordCount)
	}
}

// `acceptDate`：给了就带、给空串当没给。
//
// 反向改坏：把空串原样带出去 ⇒ 编排层会发 `?acceptDate=`，而那是**另一个请求**
// （[ReceiveTaskExpansionRequest] 头注那条被实拍推翻过两次的认识）。
func TestParseTaskExpansionTreatsAnEmptyAcceptDateAsMissing(t *testing.T) {
	empty, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"acceptDate": ""},
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if empty.AcceptDate != nil {
		t.Error("空串应收成 nil（?acceptDate= 与不带参数是两个请求）")
	}

	given, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"acceptDate": "2026-08-01 00:00:00"},
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if given.AcceptDate == nil || *given.AcceptDate != "2026-08-01 00:00:00" {
		t.Errorf("给了就该原样带回来: %v", given.AcceptDate)
	}
}

// 失败分支只有一种：信封不 OK。其中 `90001` 必须仍是"会话过期"这一**类型**
// —— 编排层据此刷票重放，而不是当成"这个号没这个任务"。
func TestParseTaskExpansionFailedEnvelopeKeepsTheSessionExpiredType(t *testing.T) {
	_, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 90001, "msg": "用户登录失效",
	}))
	if perr == nil {
		t.Fatal("90001 应判为失败")
	}
	if !perr.IsSessionExpired() {
		t.Errorf("90001 应是会话过期那一型: %s", perr.Message())
	}

	if _, perr := ParseTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 500, "msg": "系统繁忙",
	})); perr == nil {
		t.Error("非 0 业务码应判为失败")
	} else if perr.IsSessionExpired() {
		t.Errorf("500 不该被折成会话过期: %s", perr.Message())
	}
}
