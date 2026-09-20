package upstream

import (
	"strings"
	"testing"
)

// ─────────────────────────────────────────────────────────────────────────────
// infoV3：抓包实拍的那份状态（它是"今天签没签"的唯一权威判据）
// ─────────────────────────────────────────────────────────────────────────────

func capturedStatus(t *testing.T) SignInStatus {
	t.Helper()
	v := loadAppSigninVectors(t)
	status, err := ParseSignInStatus(envelopeFrom(t, v.InfoV3.Raw))
	if err != nil {
		t.Fatalf("解析 infoV3 失败: %v", err)
	}
	return status
}

func TestInfoV3StatusParsesTheCapturedPayload(t *testing.T) {
	status := capturedStatus(t)

	if status.BeanTotal != 10496 {
		t.Errorf("余额（total）不对: %d", status.BeanTotal)
	}
	if status.CloudTotal == nil || *status.CloudTotal != 8027 {
		t.Errorf("结转额（beforeTotal）不对: %v", status.CloudTotal)
	}
	if status.SignInPoints != 3 || status.SignCount != 1 {
		t.Errorf("签到点数/连签天数不对: %d / %d", status.SignInPoints, status.SignCount)
	}
	if status.MonthDays != 7 {
		t.Errorf("本月已签天数不对: %d", status.MonthDays)
	}
	if status.CurMonthBackup == nil || !*status.CurMonthBackup {
		t.Errorf("curMonthBackup 应为 true: %v", status.CurMonthBackup)
	}
	if status.CurMonthBackupSignAccept == nil || !*status.CurMonthBackupSignAccept {
		t.Errorf("curMonthBackupSignAccept 应为 true: %v", status.CurMonthBackupSignAccept)
	}
	if len(status.ReceiveList) != 1 {
		t.Fatalf("receiveList 应有一条，实得 %d", len(status.ReceiveList))
	}
	if status.ReceiveList[0].CloudNum != 234 {
		t.Errorf("cloudNum 不对: %d", status.ReceiveList[0].CloudNum)
	}
	if status.ToReceive == nil || *status.ToReceive != 0 {
		t.Errorf("toReceive 不对: %v", status.ToReceive)
	}
}

// 余额是 `total` 而不是 `beforeTotal` —— 这里**记错过一次**（趋势图因此是一条直线）。
//
// 抓包自带一条自洽恒等式，可以直接当判据用：
// 兑换门槛 `canExchangeText[0].pOrder`（12000）减去 `total` 应当等于 `lackCloudNum`。
func TestBalanceIsTotalNotBeforeTotal(t *testing.T) {
	status := capturedStatus(t)

	const pOrder = 12000
	const lackCloudNum = 1504
	if got := pOrder - status.BeanTotal; got != lackCloudNum {
		t.Errorf("自洽恒等式不成立：pOrder(%d) - total(%d) = %d，而 lackCloudNum 是 %d"+
			" —— 说明取错了字段（很可能是拿 beforeTotal 当余额）",
			pOrder, status.BeanTotal, got, lackCloudNum)
	}
	if status.CloudTotal != nil && *status.CloudTotal == status.BeanTotal {
		t.Error("两个口径的数不该相等 —— 相等说明取了同一个字段两次")
	}
}

// `monthDays` 与"按 cal 数出来"是两条独立路径，相等才说明解析没走样。
func TestCountedMonthDaysAgreesWithTheServerField(t *testing.T) {
	status := capturedStatus(t)
	if got := status.CountedMonthDays(); int64(got) != status.MonthDays {
		t.Errorf("按日历数出来 %d 天，服务端报 %d 天 —— 解析走样了", got, status.MonthDays)
	}
}

func TestSignedTodayComesFromTheServerMarkedCell(t *testing.T) {
	status := capturedStatus(t)
	signed, ok := status.SignedToday()
	if !ok {
		t.Fatal("这份抓包里应当有服务端标为今天的格子")
	}
	if !signed {
		t.Error("服务端标为今天的那格应是已签")
	}
}

// `cal` 里混着上月的 31 号格 —— 按 `d` 裸查会命中错的月份。
func TestSignedOnIgnoresCellsFromOtherMonths(t *testing.T) {
	status := capturedStatus(t)
	// 抓包里 `d:31` 那格是上月的补位（currentMonth=0）
	if _, ok := status.SignedOn(31); ok {
		t.Error("本月没有 31 号格时应返回「找不到」，而不是拿上月的补位格来充数")
	}
	if signed, ok := status.SignedOn(19); !ok || !signed {
		t.Errorf("本月 19 号应是已签: %v / %v", signed, ok)
	}
	if signed, ok := status.SignedOn(3); !ok || signed {
		t.Errorf("本月 3 号应是未签: %v / %v", signed, ok)
	}
	if _, ok := status.SignedOn(0); ok {
		t.Error("0 号是无效输入，应返回找不到")
	}
	if _, ok := status.SignedOn(32); ok {
		t.Error("32 号是无效输入，应返回找不到")
	}
}

// `cloudType` 决定这一笔现在能不能领 —— 名字里的 "receive" 不等于"现在能领"。
func TestReceiveItemCloudTypeDecidesWhatIsReceivable(t *testing.T) {
	status := capturedStatus(t)
	item := status.ReceiveList[0]
	if item.CloudType != CloudTypeNextMonth {
		t.Fatalf("这份抓包里那一条应是 cloudType=2，实得 %d", item.CloudType)
	}
	if item.CanReceiveNow() {
		t.Error("cloudType=2 是「下月可领取」，不是现在能领")
	}
	if !item.IsNextMonth() {
		t.Error("cloudType=2 应判为下月")
	}
	if item.IsDeferred() {
		t.Error("cloudType=2 不是延期领取那条线")
	}

	// 实测另一条账目 `[{2,240},{0,6}]`：只有 `0` 那条现在能领
	now := ReceiveItem{CloudType: CloudTypeNow, CloudNum: 6}
	if !now.CanReceiveNow() || now.IsNextMonth() || now.IsDeferred() {
		t.Error("cloudType=0 应判为现在可领，且不属于另两类")
	}
	deferred := ReceiveItem{CloudType: CloudTypeDeferred, CloudNum: 240}
	if !deferred.IsDeferred() || deferred.CanReceiveNow() {
		t.Error("cloudType=1 应走延期领取那条线")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// startSignIn：它的响应是**调用前**的快照，不是成败判据
// ─────────────────────────────────────────────────────────────────────────────

func TestStartSignInResultIsTheBeforeSnapshot(t *testing.T) {
	v := loadAppSigninVectors(t)
	result, err := ParseStartSignIn(envelopeFrom(t, v.StartSignIn.Raw))
	if err != nil {
		t.Fatalf("解析 startSignIn 失败: %v", err)
	}
	// 这份抓包是"调用前就已经签过了"的那一次
	if !result.TodaySignIn {
		t.Error("这份抓包的 todaySignIn 应为 true")
	}
	if result.SignInPoints != 3 || result.SignCount != 1 {
		t.Errorf("点数/天数不对: %d / %d", result.SignInPoints, result.SignCount)
	}
	if result.ServerTimeMS == nil || *result.ServerTimeMS != 1789799929696 {
		t.Errorf("服务端时间不对: %v", result.ServerTimeMS)
	}
	// ⚠ 这条断言是"不要用它判成败"的落点：字段解析出来了，
	// 但它代表的是**调用前**的状态 —— 判成败要回查 infoV3。
}

func TestStartSignInRejectsANonBooleanTodaySignIn(t *testing.T) {
	// 上游把 `todaySignIn` 变成 `1` / `0` 时必须立刻响，
	// 而不是"顺手当成 true" —— 那会让"今天签没签"这个判断悄悄反掉。
	if _, err := ParseStartSignIn(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"todaySignIn": 1, "signInPoints": 3, "signCount": 1},
	})); err == nil {
		t.Error("非布尔的 todaySignIn 应被拒")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求形状
// ─────────────────────────────────────────────────────────────────────────────

func TestInfoV3RequestCarriesTheJWTToken(t *testing.T) {
	out := InfoV3Request("jwt-905", "https://referer")
	if out.URL != "https://m.mcloud.139.com/ycloud/signin/page/infoV3?client=app" {
		t.Errorf("URL 不对（client 是字面量 app，不是数字）: %s", out.URL)
	}
	if got := headerValue(out.Headers, "jwttoken"); got != "jwt-905" {
		t.Errorf("jwttoken 头不对: %q", got)
	}
	if got := headerValue(out.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 不对: %q", got)
	}
}

// `deviceId` 是 base64，含 `+` / `/` `=` —— 不编码的话 `+` 会被服务端解成空格。
func TestStartSignInPercentEncodesTheDeviceID(t *testing.T) {
	bare := StartSignInRequest("jwt", "https://referer", nil)
	if strings.Contains(bare.URL, "deviceId") {
		t.Error("不给 deviceID 时不该出现 deviceId 查询参数")
	}
	if countHeader(bare.Headers, "isdeviceid") != 0 {
		t.Error("不给 deviceID 时不该带 isdeviceid 头 —— 两者必须成对")
	}

	// 这个值里三个特殊字符都有
	id := "Babcd+/="
	out := StartSignInRequest("jwt", "https://referer", &id)
	if !strings.Contains(out.URL, "deviceId=Babcd%2B%2F%3D") {
		t.Errorf("deviceId 应百分号编码: %s", out.URL)
	}
	if strings.Contains(out.URL, "+/=") {
		t.Error("原始字符不应出现在查询串里")
	}
	if got := headerValue(out.Headers, "isdeviceid"); got != "true" {
		t.Errorf("带 deviceID 时应同时补 isdeviceid 头: %q", got)
	}
	if got := headerValue(out.Headers, "showloading"); got != "false" {
		t.Errorf("startSignIn 的 showloading 是 false（与 infoV3 不同）: %q", got)
	}
}

func TestTaskListV3AndDoTaskPostBodies(t *testing.T) {
	task, err := TaskListV3Request("jwt", "https://referer", 1)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	body := string(task.Body)
	// `client` 是**数字** 1（不是 infoV3 那个字面量 "app"）
	if !strings.Contains(body, `"client":1`) {
		t.Errorf("taskListV3 的 client 是数字: %s", body)
	}
	for _, want := range []string{`"marketname":"` + MarketName + `"`, `"clientVersion":"` + TaskClientVersion + `"`} {
		if !strings.Contains(body, want) {
			t.Errorf("请求体应含 %s: %s", want, body)
		}
	}

	// doTaskPost 的 deviceId 是**必填**（签名里没有 Option）
	post, err := DoTaskPostRequest("jwt", "https://referer", "B_DEVICE_ID")
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if !strings.Contains(string(post.Body), `"deviceId":"B_DEVICE_ID"`) {
		t.Errorf("doTaskPost 应带 deviceId: %s", post.Body)
	}
	if got := headerValue(post.Headers, "isdeviceid"); got != "true" {
		t.Errorf("doTaskPost 应带 isdeviceid 头: %q", got)
	}
	if n := countHeader(post.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一个，实得 %d", n)
	}
}

// `taskListV3` 的 `result` 是**数组**，不是对象 —— 解析错的话这里会立刻响。
func TestParseTaskListExpectsAnArray(t *testing.T) {
	items, err := ParseTaskList(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": []any{map[string]any{"taskId": "1"}},
	}))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if len(items) != 1 {
		t.Errorf("应有一条任务，实得 %d", len(items))
	}
	if _, err := ParseTaskList(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"taskId": "1"},
	})); err == nil {
		t.Error("result 是对象时应报错")
	}
}

// 缺 `cal` 必须报错 —— 它是"今天签没签"的唯一依据，静默给个空日历等于假装没签。
func TestParseSignInStatusRequiresTheCalendar(t *testing.T) {
	_, err := ParseSignInStatus(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"total": 1, "signInPoints": 3, "signCount": 1, "monthDays": 0},
	}))
	if err == nil {
		t.Error("缺 cal 应报错")
	}
}
