package upstream

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// respondWith 用一段原始 JSON 造一个"每次都回同一份响应"的假客户端。
func respondWith(t *testing.T, raw any) (*Upstream, *scriptedClient) {
	t.Helper()
	buf, err := core.MarshalJSON(raw)
	if err != nil {
		t.Fatalf("序列化夹具失败: %v", err)
	}
	client := &scriptedClient{response: core.NewResponse(200, buf)}
	return NewUpstream(client), client
}

// ─────────────────────────────────────────────────────────────────────────────
// 建会话
// ─────────────────────────────────────────────────────────────────────────────

// TestAppH5TimestampComesFromTheServerNotTheLocalClock：签名里的 `x-timestamp`
// 必须与服务端逐字相同，所以这一层**必须**发请求去取，不能拿本地时钟顶替。
func TestAppH5TimestampComesFromTheServerNotTheLocalClock(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success", "result": "1789800128813",
	})
	got, perr := up.AppH5Timestamp(context.Background())
	if perr != nil {
		t.Fatalf("取时间戳失败: %v", perr)
	}
	if got != "1789800128813" {
		t.Errorf("应逐字交出服务端那个串，实得 %q", got)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应发一个请求，实得 %d", len(client.requests))
	}
	if !strings.HasSuffix(client.requests[0].URL, "/ycloud/api/timestamp") {
		t.Errorf("应打 /ycloud/api/timestamp，实得 %q", client.requests[0].URL)
	}
}

// TestAppQuerySpecTokenRoundTripByHand：换 SSO token 走的是**原生**那套头，
// 不是 H5 那套 —— 签到要同时说两种客户端语言，这里守的是第一段没说错。
func TestAppQuerySpecTokenRoundTripByHand(t *testing.T) {
	profile := NewAppProfile(NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"))
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"token": "SSO-TOKEN-1"},
	})
	got, perr := up.AppQuerySpecToken(
		context.Background(), profile, 1789800128813000, "mobile abc", ToSourceID)
	if perr != nil {
		t.Fatalf("换 token 失败: %v", perr)
	}
	if got != "SSO-TOKEN-1" {
		t.Errorf("应取到 SSO token，实得 %q", got)
	}
	request := client.requests[0]
	if !strings.Contains(request.URL, "/user/querySpecTokenV2") {
		t.Errorf("应打 querySpecTokenV2，实得 %q", request.URL)
	}
	if headerValue(request.Headers, "Authorization") != "mobile abc" {
		t.Errorf("应带上注入的 Authorization，实得 %q",
			headerValue(request.Headers, "Authorization"))
	}
}

// TestAppH5LoginCarriesTheSignatureHeaders：建会话那一步必须带四个签名头，
// 少了任何一个都是 `401`（且报错不会提示"少了哪个头"）。
func TestAppH5LoginCarriesTheSignatureHeaders(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"token": "JWT-1", "mobile": "159****0000"},
	})
	sign := SignH5RequestWith("rid-1", "1789800128813", "nonce-1")
	got, perr := up.AppH5Login(context.Background(), sign, "https://referer", "SSO-TOKEN-1")
	if perr != nil {
		t.Fatalf("建会话失败: %v", perr)
	}
	if got.JWTToken != "JWT-1" {
		t.Errorf("应取到 jwt，实得 %q", got.JWTToken)
	}
	if got.Mobile == nil || *got.Mobile != "159****0000" {
		t.Errorf("应取到回显的手机号，实得 %v", got.Mobile)
	}
	request := client.requests[0]
	for _, name := range []string{"x-request-id", "x-timestamp", "x-nonce", "x-signature"} {
		if headerValue(request.Headers, name) == "" {
			t.Errorf("建会话必须带 %s，实得为空", name)
		}
	}
	// 这一步正是**为了**拿 jwt，所以还没有 jwt 可带。
	if countHeader(request.Headers, "jwttoken") != 0 {
		t.Error("建会话那一步不该带 jwttoken —— 它就是要来拿 jwt 的")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 状态 / 签到
// ─────────────────────────────────────────────────────────────────────────────

// TestAppSignInStatusUsesTheCapturedInfoV3：用黄金向量那条实拍走完整条发送链路，
// 顺带确认它**只发一个请求**（`infoV3` 是只读的，不该有副请求）。
func TestAppSignInStatusUsesTheCapturedInfoV3(t *testing.T) {
	vectors := loadAppSigninVectors(t)
	up, client := respondWith(t, vectors.InfoV3.Raw)
	got, perr := up.AppSignInStatus(context.Background(), "jwt", "https://referer")
	if perr != nil {
		t.Fatalf("查状态失败: %v", perr)
	}
	if got.BeanTotal != 10496 {
		t.Errorf("余额（total）应为 10496，实得 %d", got.BeanTotal)
	}
	if len(client.requests) != 1 {
		t.Fatalf("只读查询应只发一个请求，实得 %d", len(client.requests))
	}
	if !strings.Contains(client.requests[0].URL, "/ycloud/signin/page/infoV3") {
		t.Errorf("应打 infoV3，实得 %q", client.requests[0].URL)
	}
}

// TestAppSignInStatusRawKeepsFieldsWeDoNotKnow：原始入口存在的理由就是
// "`receiveList[]` 里还有我们不认识的字段"。它交出的必须是**上游原文**，
// 而不是被 [SignInStatus] 过滤过的那一份。
func TestAppSignInStatusRawKeepsFieldsWeDoNotKnow(t *testing.T) {
	vectors := loadAppSigninVectors(t)
	up, _ := respondWith(t, vectors.InfoV3.Raw)
	got, perr := up.AppSignInStatusRaw(context.Background(), "jwt", "https://referer")
	if perr != nil {
		t.Fatalf("取原文失败: %v", perr)
	}
	obj, ok := got.(map[string]any)
	if !ok {
		t.Fatalf("原文应是整个响应体（map），实得 %T", got)
	}
	if _, ok := obj["result"]; !ok {
		t.Errorf("原文应含 result，实得 keys=%v", obj)
	}
	result, _ := obj["result"].(map[string]any)
	if _, ok := result["cal"]; !ok {
		t.Errorf("原文应含 result.cal，实得 keys=%v", result)
	}
}

// TestAppStartSignInResponseIsTheSnapshotBeforeTheCall：
// `startSignIn` 的响应是**调用前**的快照 —— `TodaySignIn == false`
// 恰恰表示"这次把今天签上了"。所以这条断言守的是"没把它当成失败"。
func TestAppStartSignInResponseIsTheSnapshotBeforeTheCall(t *testing.T) {
	vectors := loadAppSigninVectors(t)
	up, client := respondWith(t, vectors.StartSignIn.Raw)
	got, perr := up.AppStartSignIn(context.Background(), "jwt", "https://referer", nil)
	if perr != nil {
		t.Fatalf("签到失败: %v", perr)
	}
	if !got.TodaySignIn {
		t.Error("实拍那条 todaySignIn 是 true，应原样交回；翻转它等于把快照当成了判据")
	}
	if len(client.requests) != 1 {
		t.Fatalf("startSignIn 应只发一个请求 —— 发送层不代劳回查 infoV3（那一步由调用方做），实得 %d", len(client.requests))
	}
	if !strings.Contains(client.requests[0].URL, "/ycloud/signin/page/startSignIn") {
		t.Errorf("应打 startSignIn，实得 %q", client.requests[0].URL)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 领取 / 记录 / 备份好礼
// ─────────────────────────────────────────────────────────────────────────────

// TestAppReceiveV3CarriesTheDeviceID：领云朵是这一族里唯一"不带就失败"的接口，
// 所以发送层必须把 deviceId 既放进 body 又配上 `isdeviceid` 头。
func TestAppReceiveV3CarriesTheDeviceID(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"receive": 100},
	})
	_, perr := up.AppReceiveV3(context.Background(), "jwt", "https://referer", "CLOUD-1", nil, "DEV=1")
	if perr != nil {
		t.Fatalf("领云朵失败: %v", perr)
	}
	request := client.requests[0]
	if headerValue(request.Headers, "isdeviceid") != "true" {
		t.Errorf("应带 isdeviceid 头，实得 %q", headerValue(request.Headers, "isdeviceid"))
	}
	body, ok := core.ParseJSON(string(request.Body))
	if !ok {
		t.Fatalf("请求体不是合法 JSON: %s", request.Body)
	}
	bodyObj, _ := body.(map[string]any)
	if bodyObj["deviceId"] != "DEV=1" {
		t.Errorf("body 的 deviceId 应为 DEV=1，实得 %v", bodyObj["deviceId"])
	}
	if bodyObj["cloudId"] != "CLOUD-1" {
		t.Errorf("body 的 cloudId 应为 CLOUD-1，实得 %v", bodyObj["cloudId"])
	}
}

// TestAppPrizeRecordsWalkTheCapturedPendingPrize：奖品记录走完整链路，
// 用实拍那条钉住字段（与解析层的那条断言互补：这条证明**发送侧也接得上**）。
func TestAppPrizeRecordsWalkTheCapturedPendingPrize(t *testing.T) {
	vectors := loadAppSigninVectors(t)
	up, client := respondWith(t, vectors.PrizeRecords.Raw)
	got, perr := up.AppPrizeRecords(context.Background(),
		"jwt", "https://referer", PrizeMarketName, PrizeStatusPending, 1, 10)
	if perr != nil {
		t.Fatalf("查奖品记录失败: %v", perr)
	}
	if len(got.Records) != 1 {
		t.Fatalf("实拍那一页应有 1 条，实得 %d", len(got.Records))
	}
	if got.Records[0].PrizeName == nil ||
		*got.Records[0].PrizeName != "移动云盘1000万tokens叠加包" {
		t.Errorf("奖品名不对: %v", got.Records[0].PrizeName)
	}
	if !strings.Contains(client.requests[0].URL, "marketname=sign_in_3_ex") {
		t.Errorf("应带上 marketname，实得 %q", client.requests[0].URL)
	}
}

// TestAppBackupGiftThreeStatesThroughTheSendLayer：发送层**不判门槛** ——
// 三个 state 一律原样交回，判据留给调用方。
// ⚠ 反向测试已确认：把 `State` 静默认成 0 之后，`-1`（没开备份）会被当成"可领"。
func TestAppBackupGiftThreeStatesThroughTheSendLayer(t *testing.T) {
	cases := []struct {
		name  string
		state int64
	}{
		{"未开启备份", BackupGiftOff},
		{"本月已领过", BackupGiftClaimed},
		{"可领", BackupGiftClaimable},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			up, _ := respondWith(t, map[string]any{
				"code": 0, "msg": "success",
				"result": map[string]any{"state": c.state, "curMonth": true},
			})
			got, perr := up.AppBackupGift(context.Background(), "jwt", "https://referer")
			if perr != nil {
				t.Fatalf("查备份好礼失败: %v", perr)
			}
			if got.State != c.state {
				t.Errorf("state 应原样交回（%d），实得 %d", c.state, got.State)
			}
		})
	}
}

// TestAppReceiveBackupGiftResultIsACountNotAnObject：这条的 `result` 是豆数，
// 按对象去取的表现是"永远领到 0 豆"且不报错。
func TestAppReceiveBackupGiftResultIsACountNotAnObject(t *testing.T) {
	up, _ := respondWith(t, map[string]any{
		"code": 0, "msg": "success", "result": 500,
	})
	got, perr := up.AppReceiveBackupGift(context.Background(), "jwt", "https://referer")
	if perr != nil {
		t.Fatalf("领备份好礼失败: %v", perr)
	}
	if got.Received == nil || *got.Received != 500 {
		t.Errorf("应领到 500 豆，实得 %v", got.Received)
	}
}

// TestAppClaimMultipleSendsTheWriteRequest：签到翻倍是写请求，每月一次 ——
// 发送层照发，不替调用方判断"本月还能不能领"。
func TestAppClaimMultipleSendsTheWriteRequest(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"cloudCount": 200, "multiple": 2},
	})
	got, perr := up.AppClaimMultiple(context.Background(), "jwt", "https://referer")
	if perr != nil {
		t.Fatalf("领翻倍失败: %v", perr)
	}
	if got.CloudCount == nil || *got.CloudCount != 200 {
		t.Errorf("应领到 200 豆，实得 %v", got.CloudCount)
	}
	if got.Multiple == nil || *got.Multiple != 2 {
		t.Errorf("multiple 是**倍数**，应为 2，实得 %v", got.Multiple)
	}
	if len(client.requests) != 1 {
		t.Fatalf("应发一个请求，实得 %d", len(client.requests))
	}
}

// TestAppCloudRecordsIsReadOnly：流水是审计接口，**只读** ——
// 这一层不该出现任何写请求（用 GET 本身就是判据）。
func TestAppCloudRecordsIsReadOnly(t *testing.T) {
	up, client := respondWith(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"total": 2, "records": []any{
			map[string]any{"num": 100}, map[string]any{"num": 200},
		}},
	})
	got, perr := up.AppCloudRecords(context.Background(),
		"jwt", "https://referer", RecordBookBean, RecordFilterAll, 1, 10)
	if perr != nil {
		t.Fatalf("查流水失败: %v", perr)
	}
	if got.Total == nil || *got.Total != 2 {
		t.Errorf("total 是条数，应为 2，实得 %v", got.Total)
	}
	if len(got.Records) != 2 {
		t.Errorf("本页应 2 条，实得 %d", len(got.Records))
	}
	if client.requests[0].Method != core.MethodGet {
		t.Errorf("审计接口应是 GET，实得 %v", client.requests[0].Method)
	}
}
