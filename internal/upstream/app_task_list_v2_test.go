package upstream

// `taskListV2` 的用例：请求面钉**实拍字节**，解析面钉**实拍形状**。
//
// 两份出处分别是"多强"的判据，分开说：
//   - 请求面有**本仓夹具的两条实拍**（`test/fixtures/http/3d.json`，group `new` / `beiyong1`），
//     所以 body 逐字与 content-length 都能钉死；
//   - `appversion` / `activityid` 两枚头的**值**在夹具里是 `<REDACTED>` ⇒ 那两枚只钉
//     "发了、且恰好一枚、值是 [MarketName] 或本次的市场名"，出处是旧 TS 字面量（B 档）。
//     ⚠ 反过来"上游会不会因为 activityid 少了/错了而改变判断"**未测试** ——
//     本层只钉"发出去的这一组恰好是这些"。
//   - 响应面只拍到 `sign_in_3` 那两支 group ⇒ 邮箱市场名（`newsign_139mail`）的响应形状
//     **零样本**，本文件不据它写任何判据。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// taskListV2NewFixture 是实拍那一条**空组**响应（group `new`，逐字照抄键集合）。
func taskListV2NewFixture() map[string]any {
	return map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{}, "groupState": map[string]any{},
	}
}

// taskListV2Beiyong1Fixture 是实拍那一条**有任务**的响应。
//
// 三条的取值全部来自夹具（id / state / enable / currstep / process / marketname / limitType），
// 并**刻意保留**本层不读的那十几个键（`button` / `icon` / `steps` / `prov` / `recordid` …）：
// 上游给的比我们要的多是常态，把夹具削平就等于没测这一条。
func taskListV2Beiyong1Fixture() map[string]any {
	item := func(id int64, name string, currstep int64) map[string]any {
		return map[string]any{
			"id": id, "name": name, "description": "上传照片，生成童趣3D玩偶",
			"groupid": "beiyong1", "marketname": "sign_in_3",
			"state": TaskStateFinish, "enable": 1, "currstep": currstep, "process": 0,
			"limitType": TaskLimitEver, "currDay": 0, "sort": 1, "taskType": 0,
			"prov": "0", "hasBak": 0, "hasNda": 1, "limitCloud": 1,
			"recordid": 38782925983,
			"icon":     "https://img.mcloud.139.com//uop_prod/20260528/x.png",
			"button":   map[string]any{"app": map[string]any{"text": "已完成"}},
			"steps":    []any{}, "stepTypeSet": []any{"sendcloud", "click"},
		}
	}
	return map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"beiyong1": []any{
			item(596, "珍藏童真时刻", 4),
			item(597, "制作童趣3D玩偶", 6),
			item(598, "九宫格趣晒萌娃", 8),
		}},
		"groupState": map[string]any{
			"beiyong1": map[string]any{"taskFinishState": 2},
		},
	}
}

// TestTaskListV2RequestBodyIsTheCapturedBytes ⭐ 本文件最贵的一条：请求面逐字。
//
// 实拍 body 逐字 `{"clientVersion":"12.6.0","group":"new","marketname":"sign_in_3"}`
// （夹具里 `content-length: 65`）。这里的键序不是巧合：本层的 body 是 `map[string]any`
// 经 `encoding/json` 序列化，**Go 保证按键名字典序**输出，正好与实拍一致 ——
// 所以这条断言同时钉住"没有多出来的 `client` 键"（那是 V3 的键，混进来会改变字节数）。
func TestTaskListV2RequestBodyIsTheCapturedBytes(t *testing.T) {
	request, perr := TaskListV2Request(
		"JWT-1", "https://referer.example/signin", MarketName, "new")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if request.URL != "https://m.mcloud.139.com/ycloud/signin/task/taskListV2" {
		t.Errorf("URL 应逐字为抓包那一条，实得 %q", request.URL)
	}
	const want = `{"clientVersion":"12.6.0","group":"new","marketname":"sign_in_3"}`
	if got := string(request.Body); got != want {
		t.Errorf("body 应逐字为 %s，实得 %s", want, got)
	}
	if n := len(request.Body); n != 65 {
		t.Errorf("实拍 content-length 是 65，实得 %d（body=%q）", n, request.Body)
	}
	if strings.Contains(string(request.Body), `"client"`) {
		t.Errorf("这一口不带 `client`（那是 V3 的键），实得 %s", request.Body)
	}
	// 头面：一枚 activityid、一枚带 charset 的 content-type、票在 jwttoken 上。
	if n := countHeader(request.Headers, "activityid"); n != 1 {
		t.Fatalf("activityid 应恰好一枚（两枚时哪枚生效变成传输层的事），实得 %d", n)
	}
	if got := headerValue(request.Headers, "activityid"); got != MarketName {
		t.Errorf("marketname 是 sign_in_3 时 activityid 应是 %q，实得 %q", MarketName, got)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应恰好一个，实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != H5ContentType {
		t.Errorf("content-type 应是 %q（带 charset），实得 %q", H5ContentType, got)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("jwttoken 应是合成票 JWT-1，实得 %q", got)
	}
	// 实拍这两条**都带** appversion 与 showloading（夹具里值被脱敏 ⇒ 只钉"发了"）。
	if n := countHeader(request.Headers, "appversion"); n != 1 {
		t.Errorf("实拍有 appversion 这一枚，实得 %d 个", n)
	}
	if n := countHeader(request.Headers, "showloading"); n != 1 {
		t.Errorf("实拍有 showloading 这一枚，实得 %d 个", n)
	}
}

// TestTaskListV2ActivityIDFollowsTheMarketName：`activityid` 跟着本次的市场名走。
//
// 反向改坏它的样子：漏掉"先剥再补"那两句，直接用 [WithH5Headers] 的结果 ——
// 那么 `sign_in_3` 那一支**照样全绿**（两值相同），只有邮箱这一支会发出
// "marketname 是 newsign_139mail、activityid 是 sign_in_3"这种**实拍与旧 TS 都没有**的组合。
func TestTaskListV2ActivityIDFollowsTheMarketName(t *testing.T) {
	request, perr := TaskListV2Request(
		"JWT-1", "https://referer.example/signin", TaskListV2EmailMarketName, "month")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if n := countHeader(request.Headers, "activityid"); n != 1 {
		t.Fatalf("应恰好一枚 activityid，实得 %d 枚（剥没剥干净）", n)
	}
	if got := headerValue(request.Headers, "activityid"); got != TaskListV2EmailMarketName {
		t.Errorf("activityid 应是 %q，实得 %q", TaskListV2EmailMarketName, got)
	}
	if !strings.Contains(string(request.Body), `"marketname":"`+TaskListV2EmailMarketName+`"`) {
		t.Errorf("body 里的 marketname 要同步换，实得 %s", request.Body)
	}
}

// TestTaskListV2GroupsAreTheEightFromTheOldImplementation：group 集合与**顺序**。
//
// 钉顺序的理由在 [TaskListV2Groups] 那一条注释里（合并时先来的排前面，去重保留先来的）。
// 另钉两件事：`hiddenabc` **不在**这个数组里（旧 TS 的类型里有它、请求数组里没有），
// `beiyong2` **在**（旧 TS 的类型里没有它）。
func TestTaskListV2GroupsAreTheEightFromTheOldImplementation(t *testing.T) {
	want := []string{"new", "beiyong1", "beiyong2", "cloudEmail", "time", "day", "month", "hidden"}
	if len(TaskListV2Groups) != len(want) {
		t.Fatalf("应恰好 %d 个 group，实得 %d（%v）", len(want), len(TaskListV2Groups), TaskListV2Groups)
	}
	for i := range want {
		if TaskListV2Groups[i] != want[i] {
			t.Errorf("第 %d 个 group 应是 %q，实得 %q（顺序是行为的一部分）", i, want[i], TaskListV2Groups[i])
		}
	}
	for _, g := range TaskListV2Groups {
		if g == "hiddenabc" {
			t.Error("hiddenabc 从不请求（只在旧 TS 的类型与结果文案里出现）")
		}
	}
}

func TestParseTaskListV2EmptyResultObjectIsAnEmptyGroup(t *testing.T) {
	list, perr := ParseTaskListV2(envelopeFrom(t, taskListV2NewFixture()), "new")
	if perr != nil {
		t.Fatalf("空组不是错误（实拍 group `new` 就是这条），实得 %v", perr)
	}
	if len(list.Items) != 0 {
		t.Errorf("应 0 条，实得 %d", len(list.Items))
	}
	if list.Group != "new" {
		t.Errorf("Group 应回填请求时的 group，实得 %q", list.Group)
	}
	if len(list.GroupFinishStates) != 0 {
		t.Errorf("实拍这条 groupState 是 {}，实得 %v", list.GroupFinishStates)
	}
}

// TestParseTaskListV2MissingResultKeyIsAlsoAnEmptyGroup：`result` 整个缺席 ⇒ 同样按空组。
//
// 判成解析错误的代价是"整条聚合链被一次空响应打断"（实拍只证明它**有时**是 `{}`，
// 没证明它一定在）。⚠ 反过来"上游会不会整个不给 result"**未测试**。
func TestParseTaskListV2MissingResultKeyIsAlsoAnEmptyGroup(t *testing.T) {
	list, perr := ParseTaskListV2(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
	}), "day")
	if perr != nil {
		t.Fatalf("缺 result 应降级成空组，实得 %v", perr)
	}
	if len(list.Items) != 0 {
		t.Errorf("应 0 条，实得 %d", len(list.Items))
	}
}

func TestParseTaskListV2ReadsTheCapturedGroup(t *testing.T) {
	list, perr := ParseTaskListV2(envelopeFrom(t, taskListV2Beiyong1Fixture()), "beiyong1")
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(list.Items) != 3 {
		t.Fatalf("应 3 条，实得 %d", len(list.Items))
	}
	first := list.Items[0]
	if first.TaskID == nil || *first.TaskID != 596 {
		t.Errorf("id 应解成整数 596，实得 %v", first.TaskID)
	}
	if first.Name == nil || *first.Name != "珍藏童真时刻" {
		t.Errorf("name 实得 %v", first.Name)
	}
	if first.State == nil || *first.State != TaskStateFinish {
		t.Errorf("state 实得 %v", first.State)
	}
	// 这一口**额外声明**的四个字段（V3 那份没有）。
	if first.Enable == nil || *first.Enable != 1 {
		t.Errorf("enable 应是 1（旧实现的门是 `!== 1` ⇒ 跳过），实得 %v", first.Enable)
	}
	if first.CurrStep == nil || *first.CurrStep != 4 {
		t.Errorf("currstep 实得 %v", first.CurrStep)
	}
	if first.MarketName == nil || *first.MarketName != "sign_in_3" {
		t.Errorf("marketname 实得 %v", first.MarketName)
	}
	// ⚠ 实拍三条的 process **恒为 0** 而 state 已是 FINISH ⇒ 这里钉的是"照原样交出去"，
	// 不构成"0 表示没进度"。见 TaskV2Item 那条注释。
	if first.Process == nil || *first.Process != 0 {
		t.Errorf("process 应解成数字 0（不是 nil），实得 %v", first.Process)
	}
	if list.Items[2].CurrStep == nil || *list.Items[2].CurrStep != 8 {
		t.Errorf("第三条的 currstep 应是 8（三条各不相同，钉住逐条对应关系）")
	}
	state, ok := list.GroupFinishStates["beiyong1"]
	if !ok || state == nil || *state != 2 {
		t.Errorf("groupState.beiyong1.taskFinishState 应是 2（只存不判），实得 %v", list.GroupFinishStates)
	}
}

// TestParseTaskListV2ToleratesMissingOptionalFields：上游"有时不给"那一类。
//
// 少 `enable` / `state` / `currstep` 都不该让整次解析失败 —— 少一个可选字段就把
// 一整组任务丢掉，表现是"界面上这些任务凭空消失"。
func TestParseTaskListV2ToleratesMissingOptionalFields(t *testing.T) {
	list, perr := ParseTaskListV2(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"day": []any{
			map[string]any{"id": 402, "name": "开启APP通知"},
			map[string]any{}, // 连 id 都没有：仍要出一条，交下游按 nil 处理
		}},
	}), "day")
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(list.Items) != 2 {
		t.Fatalf("应 2 条，实得 %d", len(list.Items))
	}
	if list.Items[0].Enable != nil || list.Items[0].State != nil {
		t.Errorf("缺字段应是 nil 而不是零值，实得 %+v", list.Items[0])
	}
	if list.Items[1].TaskID != nil {
		t.Errorf("没有 id 时该是 nil（不许补成 0），实得 %v", *list.Items[1].TaskID)
	}
}

// TestParseTaskListV2ItemsAcrossGroupsAreFlattenedDeterministically：
// 一次响应里 `result` 带多个键时，摊平顺序**每次一样**。
//
// 反向改坏它的样子：把 [sortedResultGroupNames] 换成直接 `range` map ——
// 那么这一条会在**约一半的运行**里红（Go 的 map 遍历随机），而聚合层"按 name 去重
// 保留先来的那条"就变成每次不一样。跑 20 次把偶发变成必现。
func TestParseTaskListV2ItemsAcrossGroupsAreFlattenedDeterministically(t *testing.T) {
	envelope := envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{
			"time":       []any{map[string]any{"id": 1, "name": "T-time"}},
			"cloudEmail": []any{map[string]any{"id": 2, "name": "T-email"}},
			"day":        []any{map[string]any{"id": 3, "name": "T-day"}},
		},
	})
	var want []string
	for i := 0; i < 20; i++ {
		list, perr := ParseTaskListV2(envelope, "time")
		if perr != nil {
			t.Fatalf("解析失败: %v", perr)
		}
		var names []string
		for _, item := range list.Items {
			if item.Name == nil {
				t.Fatal("夹具没成立：name 不该是 nil")
			}
			names = append(names, *item.Name)
		}
		if want == nil {
			want = names
			continue
		}
		if strings.Join(names, ",") != strings.Join(want, ",") {
			t.Fatalf("第 %d 次的顺序变了（%v vs %v）：map 遍历随机，必须排序", i, names, want)
		}
	}
	if got := strings.Join(want, ","); got != "T-email,T-day,T-time" {
		t.Errorf("应按键名字典序摊平（cloudEmail < day < time），实得 %s", got)
	}
}

// TestParseTaskListV2ExpectsAnObjectResult：`result` 是数组 ⇒ 立刻响。
//
// 这一条与 `app_task_list_test.go` 里那条"V3 的 result 必须是数组"是一对：
// 两口的形状**不同构**，混解会把形状变化藏起来。
func TestParseTaskListV2ExpectsAnObjectResult(t *testing.T) {
	_, perr := ParseTaskListV2(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": []any{map[string]any{"id": 1}},
	}), "new")
	if perr == nil || !strings.Contains(perr.Error(), "不是按 group 分组的对象") {
		t.Errorf("result 是数组时应点名形状，实得 %v", perr)
	}
}

// TestParseTaskListV2GroupValueMustBeArray：`result.<group>` 不是数组 ⇒ 响。
func TestParseTaskListV2GroupValueMustBeArray(t *testing.T) {
	_, perr := ParseTaskListV2(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"new": map[string]any{"id": 1}},
	}), "new")
	if perr == nil || !strings.Contains(perr.Error(), "result.new 不是数组") {
		t.Errorf("应点名是哪一个 group 的形状不对，实得 %v", perr)
	}
}

// TestParseTaskListV2BusinessCode：非 0 码不解析 result；`90001` 走会话过期那一支。
//
// ⚠ 与拍拍那一族不同，这里的 `90001` 判据**有实测出处**：同一套信封、同一个 H5 会话
// （`EnsureOKWithHint` 那一条注释记的就是签到族的实测），所以不另立样本缺口。
func TestParseTaskListV2BusinessCode(t *testing.T) {
	_, perr := ParseTaskListV2(envelopeFrom(t, map[string]any{
		"code": 1, "msg": "系统繁忙",
	}), "new")
	if perr == nil {
		t.Fatal("code=1 应报错")
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("上游原文要一路带到错误里，实得 %q", perr.Error())
	}
	if perr.IsSessionExpired() {
		t.Errorf("code=1 不是会话过期（刷票救不了它），实得 %v", perr)
	}

	expired, perr := ParseTaskListV2(envelopeFrom(t, map[string]any{
		"code": CodeNotLoggedIn, "msg": "未登录",
	}), "new")
	if perr == nil {
		t.Fatal("90001 应报错")
	}
	if !perr.IsSessionExpired() {
		t.Errorf("90001 应分类成会话过期（app 层那条刷一次重放一次的自救只认这个变体），实得 %v", perr)
	}
	if expired.Items != nil {
		t.Errorf("失败时不该给出条目，实得 %d 条", len(expired.Items))
	}
}
