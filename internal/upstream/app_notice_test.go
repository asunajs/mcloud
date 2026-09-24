package upstream

// 公告任务族（`openEmailsms/*` 与那两口**未接链**的入账 / 上报口）的用例。
//
// 这一族值得钉的只有两件事，而且都是"改坏了不会报错"的那类：
//
//  1. **URL 的字面形态** —— 主机带端口、路径段一处全小写一处驼峰、查询串的键值。
//     这些改"对"了（去掉端口、把 `openemailsms-service` 写成驼峰、`onDuration` 拼正确）
//     的表现是 404 或 405，看起来像"接口下线了"，不会指向我们自己动过的那一处。
//  2. **缺字段的走法** —— 判据字段缺失必须落成 nil 而不是报错，
//     因为 app 层要靠 nil 与"给了某个值"分档（`PlanNotice` 的 Unknown 那一档），
//     报错会让"上游没给"与"上游坏了"混成同一句话。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// capturedNoticeTaskInfo 是 `开启通知领云朵.json` 第 3/6 条的响应原文（逐字）。
//
// 两份抓包里两次读到的**完全相同**（第 3 条 11:20:23、第 6 条 11:20:24），
// 而中间夹着一次 `reward` 之前的一次点击 —— 也就是说本仓库**没有**
// "`reward` 之后读数变了"的样本，这一点是复查那一步存在的全部理由（也说明它未验证）。
const capturedNoticeTaskInfo = `{"code":0,"msg":"success","result":{` +
	`"taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},` +
	`"taskTwoInfo":{"cloudStatus":1,"progress":31,"sum":100,"taskStatus":3}}}`

// ─────────────────────────────────────────────────────────────────────────────
// 请求形态
// ─────────────────────────────────────────────────────────────────────────────

// TestNoticeOpenEmailSmsPathsKeepTheMixedCasing 钉住两个**大小写不同**的段：
// 主机段 `openemailsms-service`（全小写）与最后一段 `openEmailsms`（驼峰）。
//
// ⚠ 反向测试已确认：把任一段"改对"成另一种写法，本条变红。
// 而真跑时的表现只是 404 —— 那看起来像"活动结束了"，不像"我们改了个字"。
func TestNoticeOpenEmailSmsPathsKeepTheMixedCasing(t *testing.T) {
	info := NoticeTaskInfoRequest("JWT-1", "https://referer")
	reward := NoticeRewardRequest("JWT-1", "https://referer")
	for name, request := range map[string]core.HTTPRequest{
		"getTaskInfo": info,
		"reward":      reward,
	} {
		if request.Method != core.MethodGet {
			t.Errorf("%s 应是 GET，实得 %s", name, request.Method)
		}
		if !strings.HasPrefix(request.URL, BaseCaiyunYcloud+"/") {
			t.Errorf("%s 应走带端口的彩云入口 %s，实得 %q", name, BaseCaiyunYcloud, request.URL)
		}
		if strings.Contains(request.URL, BaseMcloud) {
			t.Errorf("%s **不**在 m.mcloud.139.com 上，实得 %q", name, request.URL)
		}
		if !strings.Contains(request.URL, "/ycloud/openemailsms-service/openEmailsms/") {
			t.Errorf("%s 路径两段的大小写要逐字保留，实得 %q", name, request.URL)
		}
		if len(request.Body) != 0 {
			t.Errorf("%s 实拍 body 是空串，实得 %q", name, request.Body)
		}
		if countHeader(request.Headers, "isdeviceid") != 0 {
			t.Errorf("%s 不该注入 isdeviceid（H5 调用点我们没抓到，按既有约定不发）", name)
		}
		if n := countHeader(request.Headers, "jwttoken"); n != 1 {
			t.Errorf("%s 应带 1 个 jwttoken 头，实得 %d", name, n)
		}
	}
	// ⚠ 查询串：实拍那两条的完整 URL 被脱敏器压掉了，"吃不吃参数"**未验证**。
	// 这里钉的是"**当前实现不带**"，不是"上游不要"。
	if strings.Contains(info.URL, "?") || strings.Contains(reward.URL, "?") {
		t.Errorf("按描述不带查询参数，实得 %q / %q", info.URL, reward.URL)
	}
	if !strings.HasSuffix(info.URL, "/getTaskInfo") || !strings.HasSuffix(reward.URL, "/reward") {
		t.Errorf("两条的末段应是 getTaskInfo / reward，实得 %q / %q", info.URL, reward.URL)
	}
}

// TestReceiveTaskPinsTheQueryAndBareNumberResult 钉 `receiveTask?taskId=1021`
// （`完成通知领云朵.json` 第 8 条）。
//
// ⚠ 它是本族**唯一报了到账数**的入账口（`result` 是裸数字 `100`），
// 而链路里那口 `reward` 什么都不报 —— 所以这个 `taskId` 与那个 `100` 都要逐字留住。
func TestReceiveTaskPinsTheQueryAndBareNumberResult(t *testing.T) {
	request := ReceiveTaskRequest("JWT-1", "https://referer", 1021)
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	if !strings.HasSuffix(request.URL, "/ycloud/signin/page/receiveTask?taskId=1021") {
		t.Errorf("URL 应逐字以该串结尾，实得 %q", request.URL)
	}
	if !strings.HasPrefix(request.URL, BaseCaiyunYcloud) {
		t.Errorf("应走 :7071 那个入口，实得 %q", request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("这条没有 body，实得 %q", request.Body)
	}

	got, perr := ParseReceiveTask(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": 100,
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.CloudCount == nil || *got.CloudCount != 100 {
		t.Errorf("result 是**裸数字** 100，实得 %v", got.CloudCount)
	}
}

// TestReceiveTaskMissingResultIsNilNotZero：`result` 缺席只该读成"上游没说"。
// 当成 0 的表现是"这一趟领到 0 朵"，而那是**另一件事实**（上游真发过 0 也长这样）。
func TestReceiveTaskMissingResultIsNilNotZero(t *testing.T) {
	got, perr := ParseReceiveTask(envelopeFrom(t, map[string]any{"code": 0, "msg": "success"}))
	if perr != nil {
		t.Fatalf("没有 result 不该报错，实得 %v", perr)
	}
	if got.CloudCount != nil {
		t.Errorf("应是 nil，实得 %d", *got.CloudCount)
	}
}

// TestReportAppNoticeStatusPinsStatusQuery 钉 `reportAppNoticeStatus?status=1`
// （`mobile-events-…json` 第 71/87 条）。
//
// ⚠ 那两条来自 **Android WebView**（`x-requested-with: cn.cj.pe`），与 iOS 那套
// `openEmailsms` 流量**不是同一个客户端** —— 所以这条只钉形态，不代表它该被链路调用。
func TestReportAppNoticeStatusPinsStatusQuery(t *testing.T) {
	request := ReportAppNoticeStatusRequest("JWT-1", "https://referer", AppNoticeStatusOn)
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	if !strings.HasSuffix(request.URL, "/ycloud/signin/page/reportAppNoticeStatus?status=1") {
		t.Errorf("URL 应逐字以该串结尾，实得 %q", request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("这条没有 body，实得 %q", request.Body)
	}
	// 实拍响应没有 result ⇒ 只要 code:0 就算成，缺 result **不**判失败。
	envelope, perr := ParseEnvelope([]byte(`{"code":0,"msg":"success"}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if _, perr := EnsureOKWithHint(envelope); perr != nil {
		t.Errorf("这条的判据只有 code，实得 %v", perr)
	}
}

// TestSigninTaskClickPinsKeyAndID 钉 `signin/task/click?key=task&id=1022`
// （`开启通知领云朵.json` 第 0 条）。
//
// ⚠ 两个 task id（这里的 `1022` 与入账那口的 `taskId=1021`）**只差 1 但不是同一个数**，
// 而"为什么差 1"没有任何成文依据 —— 所以两边都按参数传，不合并成一个常量。
func TestSigninTaskClickPinsKeyAndID(t *testing.T) {
	request := SigninTaskClickRequest("JWT-1", "https://referer", 1022)
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	if !strings.HasSuffix(request.URL, "/ycloud/signin/task/click?key=task&id=1022") {
		t.Errorf("URL 应逐字以该串结尾，实得 %q", request.URL)
	}
	if SigninTaskClickKey != "task" {
		t.Errorf("key 参数逐字是 task，实得 %q", SigninTaskClickKey)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析
// ─────────────────────────────────────────────────────────────────────────────

// TestNoticeTaskInfoParsesTheCapturedTwoSlots 用**实拍响应原文**逐字段钉。
//
// 这四个读数是本仓库关于这一族**全部**的事实：`taskOne{cloudStatus:2, sum:200,
// taskStatus:3}`、`taskTwo{cloudStatus:1, progress:31, sum:100, taskStatus:3}`。
// ⚠ 这里**只比读数、不比语义** —— `cloudStatus` 的 1/2 哪个是"未领"是待验证的映射，
// 语义归 `internal/app` 那侧的判定用例钉（那边也标明是猜的）。
func TestNoticeTaskInfoParsesTheCapturedTwoSlots(t *testing.T) {
	envelope, perr := ParseEnvelope([]byte(capturedNoticeTaskInfo))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	got, perr := ParseNoticeTaskInfo(envelope)
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	one, two := got.TaskOneInfo, got.TaskTwoInfo
	if one == nil || two == nil {
		t.Fatalf("两槽都该解析出来，实得 %+v / %+v", one, two)
	}
	want := []struct {
		name         string
		slot         *NoticeTaskSlot
		cloud, sum   int64
		progress, ts *int64
	}{
		{"taskOneInfo", one, 2, 200, nil, int64p(3)},
		{"taskTwoInfo", two, 1, 100, int64p(31), int64p(3)},
	}
	for _, c := range want {
		if got, ok := optInt64FieldOf(c.slot.CloudStatus); !ok || got != c.cloud {
			t.Errorf("%s cloudStatus 应为 %d，实得 %v", c.name, c.cloud, got)
		}
		if got, ok := optInt64FieldOf(c.slot.Sum); !ok || got != c.sum {
			t.Errorf("%s sum 应为 %d，实得 %v", c.name, c.sum, got)
		}
		if got, ok := optInt64FieldOf(c.slot.TaskStatus); !ok || got != *c.ts {
			t.Errorf("%s taskStatus 应为 %d，实得 %v", c.name, *c.ts, got)
		}
		if c.progress == nil {
			// ⚠ `taskOneInfo` 里**没有** `progress` 这个键（实拍）。
			// 落成 0 的表现是"槽一进度 0%"，那是一句上游没说过的话。
			if got, ok := optInt64FieldOf(c.slot.Progress); ok {
				t.Errorf("%s 的 progress 应缺席（nil 而不是 0），实得 %d", c.name, got)
			}
			continue
		}
		if got, ok := optInt64FieldOf(c.slot.Progress); !ok || got != *c.progress {
			t.Errorf("%s progress 应为 %d，实得 %v", c.name, *c.progress, got)
		}
	}
}

// TestNoticeTaskInfoToleratesMissingEverything 钉"缺字段不报错"这条纪律。
//
// 三种缺失都要落成**能判就说、不能判就 nil**，而不是报错：
// app 层要靠 nil 走 `NoticePlanUnknown`（**一个请求都不发**）。
// 报错那条路的表现是"接口坏了"，而实况只是"我们读不出判据"。
func TestNoticeTaskInfoToleratesMissingEverything(t *testing.T) {
	cases := []struct {
		name      string
		raw       any
		wantOne   bool
		wantTwo   bool
		wantField bool // 槽在的时候：taskStatus 是否给了
	}{
		{name: "没有 result", raw: map[string]any{"code": 0, "msg": "success"}},
		{
			name: "result 是空对象",
			raw:  map[string]any{"code": 0, "msg": "success", "result": map[string]any{}},
		},
		{
			name: "只有 taskOneInfo，且只给 cloudStatus",
			raw: map[string]any{"code": 0, "msg": "success", "result": map[string]any{
				"taskOneInfo": map[string]any{"cloudStatus": 2},
			}},
			wantOne: true,
		},
		{
			name: "槽不是对象",
			raw: map[string]any{"code": 0, "msg": "success", "result": map[string]any{
				"taskOneInfo": "null",
			}},
		},
		{
			name: "result 是数组",
			raw:  map[string]any{"code": 0, "msg": "success", "result": []any{}},
		},
		{
			name: "两槽字段给全",
			raw: map[string]any{"code": 0, "msg": "success", "result": map[string]any{
				"taskOneInfo": map[string]any{"cloudStatus": 2, "taskStatus": 3},
			}},
			wantOne:   true,
			wantField: true,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, perr := ParseNoticeTaskInfo(envelopeFrom(t, c.raw))
			if perr != nil {
				t.Fatalf("缺字段不该报错，实得 %v", perr)
			}
			if (got.TaskOneInfo != nil) != c.wantOne {
				t.Errorf("TaskOneInfo 在不在应为 %v，实得 %v", c.wantOne, got.TaskOneInfo != nil)
			}
			if (got.TaskTwoInfo != nil) != c.wantTwo {
				t.Errorf("TaskTwoInfo 在不在应为 %v，实得 %v", c.wantTwo, got.TaskTwoInfo != nil)
			}
			if slot := got.TaskOneInfo; slot != nil {
				_, ok := optInt64FieldOf(slot.TaskStatus)
				if ok != c.wantField {
					// 上游没给的键落成 0 = 凭空替上游说了一句话，
					// 而那句话恰好会把 app 层的"判不了"变成"判定了"。
					t.Errorf("taskStatus 在不在应为 %v，实得 %v", c.wantField, ok)
				}
			}
		})
	}
}

// TestNoticeRewardReportsNoNumber 钉那条**没有 result** 的实拍响应。
//
// `{"code":0,"msg":"success"}` —— 所以"领到多少朵"这句话眼下**没有来源**，
// 而链路因此要在领完之后再查一次（`internal/app/notice_run_test.go` 钉那段）。
func TestNoticeRewardReportsNoNumber(t *testing.T) {
	envelope, perr := ParseEnvelope([]byte(`{"code":0,"msg":"success"}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	got, perr := ParseNoticeReward(envelope)
	if perr != nil {
		t.Fatalf("没有 result 不该报错（实拍就没有），实得 %v", perr)
	}
	if got.CloudCount != nil {
		t.Errorf("这条报不出数，实得 %d", *got.CloudCount)
	}
}

// TestNoticeParsersFailOnNonZeroCode：非 0 码一律走错误那条路。
//
// ⚠ 本仓库**没有**任何一条非 0 码的实拍（"未开启"回什么码**未测试**），
// 所以这条钉的是**我们自己**的处置口径：`code != 0` 不许被当成"上游没给判据"，
// 也不许被折成"没得领" —— 那是两回事，混起来的代价见 `internal/app/notice_run.go`
// 里 `noticeNeedsManual` 那段。
func TestNoticeParsersFailOnNonZeroCode(t *testing.T) {
	raw := `{"code":1,"msg":"系统繁忙，请稍后重试"}`
	envelope, perr := ParseEnvelope([]byte(raw))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if _, perr := ParseNoticeTaskInfo(envelope); perr == nil {
		t.Error("getTaskInfo 非 0 码应报错")
	}
	if _, perr := ParseNoticeReward(envelope); perr == nil {
		t.Error("reward 非 0 码应报错")
	}
	if _, perr := ParseReceiveTask(envelope); perr == nil {
		t.Error("receiveTask 非 0 码应报错")
	}
}

// int64p 造一个 `*int64`（表驱动用例里要区分"给了 0"与"没给"）。
func int64p(v int64) *int64 { return &v }

// optInt64FieldOf 把结构体里的 `*int64` 还原成「值 + 有没有给」，供上面的表用。
func optInt64FieldOf(value *int64) (int64, bool) {
	if value == nil {
		return 0, false
	}
	return *value, true
}
