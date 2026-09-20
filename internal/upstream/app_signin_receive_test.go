package upstream

import (
	"strings"
	"testing"
)

// ─────────────────────────────────────────────────────────────────────────────
// 领云朵
// ─────────────────────────────────────────────────────────────────────────────

// `cloudType` 缺省时发的是**空串**，不是 `0` —— `0` 是"当前可领取"这个合法取值。
func TestReceiveV3SendsAnEmptyStringWhenCloudTypeIsUnknown(t *testing.T) {
	bare, err := ReceiveV3Request("jwt", "https://referer", "", nil, "B_DEVICE")
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if !strings.Contains(string(bare.Body), `"cloudType":""`) {
		t.Errorf("缺省应是空串，不是 0: %s", bare.Body)
	}
	if !strings.Contains(string(bare.Body), `"deviceId":"B_DEVICE"`) {
		t.Errorf("receiveV3 必须带 deviceId: %s", bare.Body)
	}
	if got := headerValue(bare.Headers, "isdeviceid"); got != "true" {
		t.Errorf("应带 isdeviceid 头: %q", got)
	}

	// 显式给 `0` 时发的就是数字 0 —— 两种请求**不是**一回事
	zero := int64(CloudTypeNow)
	given, err := ReceiveV3Request("jwt", "https://referer", "", &zero, "B_DEVICE")
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if !strings.Contains(string(given.Body), `"cloudType":0`) {
		t.Errorf("显式给 0 时应发数字: %s", given.Body)
	}
	if string(given.Body) == string(bare.Body) {
		t.Error("空串与 0 必须产出不同的请求体")
	}
}

func TestReceiveV3ResultIsWhatLandedThisTime(t *testing.T) {
	result, err := ParseReceiveV3(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"receive": 6},
	}))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if result.Receive != 6 {
		t.Errorf("本次进账不对: %d", result.Receive)
	}
	// 失败分支由 EnsureOKWithHint 转成带提示的错误，不走这里
	if _, err := ParseReceiveV3(envelopeFrom(t, map[string]any{"code": 615, "msg": "活动太火爆"})); err == nil {
		t.Error("615 应判为失败")
	} else if !strings.Contains(err.Message(), "没带") {
		t.Errorf("应带上排查提示: %s", err.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 复活卡 / 回馈卡
// ─────────────────────────────────────────────────────────────────────────────

func TestRevivalRewardParsesTheCapturedResponse(t *testing.T) {
	v := loadAppSigninVectors(t)
	reward, err := ParseRevivalReward(envelopeFrom(t, v.ReceiveRevivalReward.Raw))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	// 实拍：cardType=1、rewardClouds=114、totalClouds=571
	if reward.CardType != 1 {
		t.Errorf("cardType 应是 1（复活卡）: %d", reward.CardType)
	}
	if reward.RewardClouds != 114 {
		t.Errorf("本次复活到的数不对: %d", reward.RewardClouds)
	}
	if reward.TotalClouds != 571 {
		t.Errorf("累计数不对: %d", reward.TotalClouds)
	}
}

// `littleFirstTime` 只影响一句文案 —— 缺了给 0，不因此判整条响应失败。
func TestRevivalRewardToleratesAMissingCosmeticField(t *testing.T) {
	reward, err := ParseRevivalReward(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"cardType": 2, "rewardClouds": 10, "totalClouds": 20},
	}))
	if err != nil {
		t.Fatalf("附属字段缺失不该判失败: %v", err)
	}
	if reward.LittleFirstTime != 0 {
		t.Errorf("缺失时应给 0: %d", reward.LittleFirstTime)
	}
	// 但主判据缺了必须报错
	if _, err := ParseRevivalReward(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"rewardClouds": 10},
	})); err == nil {
		t.Error("缺 cardType 应报错")
	}
}

func TestReceiveRevivalRewardSendsAnEmptyObjectBody(t *testing.T) {
	out, err := ReceiveRevivalRewardRequest("jwt", "https://referer", nil)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	// body 是 `"{}"`（与抓包的 content-length: 2 对齐），不是"没有 body"
	if got := strings.TrimSpace(string(out.Body)); got != "{}" {
		t.Errorf("body 应是空对象: %q", got)
	}
	if got := headerValue(out.Headers, "showloading"); got != "false" {
		t.Errorf("这一条的 showloading 是 false（同页其余几乎都是 true）: %q", got)
	}
	// ⚠ 这里**不加** isdeviceid（老抓包里也没有）—— 别照 receiveV3 抄
	if countHeader(out.Headers, "isdeviceid") != 0 {
		t.Error("receiveRevivalReward 不该带 isdeviceid 头")
	}
	if countHeader(out.Headers, "deviceid") != 0 {
		t.Error("不给 deviceID 时不带 deviceid 头")
	}

	// 老抓包那条路：给了才补一个 `deviceid` **头**（不是查询参数、不是 isdeviceid）
	id := "B_OLD"
	withID, err := ReceiveRevivalRewardRequest("jwt", "https://referer", &id)
	if err != nil {
		t.Fatalf("构造失败: %v", err)
	}
	if got := headerValue(withID.Headers, "deviceid"); got != "B_OLD" {
		t.Errorf("deviceid 头不对: %q", got)
	}
	if countHeader(withID.Headers, "isdeviceid") != 0 {
		t.Error("补 deviceid 时也不该带 isdeviceid")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 延期领取
// ─────────────────────────────────────────────────────────────────────────────

// 真实客户端发的是**裸 GET**：`acceptDate` 取不到时 axios 会**整个键丢掉**，
// 而不是发 `?acceptDate=`。
func TestReceiveTaskExpansionSendsABareGETWhenAcceptDateIsUnknown(t *testing.T) {
	bare := ReceiveTaskExpansionRequest("jwt", "https://referer", nil)
	if strings.Contains(bare.URL, "?") {
		t.Errorf("缺省应是裸 GET（多一个空参数是**另一个请求**）: %s", bare.URL)
	}
	if len(bare.Body) != 0 {
		t.Error("这一条没有 body")
	}
	if countHeader(bare.Headers, "isdeviceid") != 0 {
		t.Error("H5 没标 isDeviceId，别照 receiveV3 抄")
	}

	date := "2026-09-01 00:00:00"
	withDate := ReceiveTaskExpansionRequest("jwt", "https://referer", &date)
	if !strings.Contains(withDate.URL, "?acceptDate=2026-09-01%2000%3A00%3A00") {
		t.Errorf("acceptDate 应百分号编码: %s", withDate.URL)
	}
}

// 没有 `result` **不算失败** —— 客户端只拿 `code` 判成败。
func TestReceiveTaskExpansionToleratesAMissingResult(t *testing.T) {
	ok, err := ParseReceiveTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
	}))
	if err != nil {
		t.Fatalf("没有 result 不该判失败（否则会盖掉 1302 这类上游原话）: %v", err)
	}
	if ok.CloudCount != nil {
		t.Errorf("没给时应是 nil，不是 0: %v", ok.CloudCount)
	}
	// `1302` 未开启自动备份：这是这条线最该看见的一句上游原话
	if _, err := ParseReceiveTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 1302, "msg": "开启自动备份按钮才可领取哦～",
	})); err == nil {
		t.Error("1302 应判为失败")
	}

	got, err := ParseReceiveTaskExpansion(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"cloudCount": 240},
	}))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if got.CloudCount == nil || *got.CloudCount != 240 {
		t.Errorf("cloudCount 不对: %v", got.CloudCount)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 签到翻倍
// ─────────────────────────────────────────────────────────────────────────────

func TestMultipleRequestIsABareGET(t *testing.T) {
	out := MultipleRequest("jwt", "https://referer")
	if out.URL != "https://m.mcloud.139.com/ycloud/signin/page/multiple" {
		t.Errorf("URL 不应带查询串: %s", out.URL)
	}
	if got := headerValue(out.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 应是 true: %q", got)
	}
}

// 倍数与合计是**两个量**：只报 cloudCount 会丢掉"翻了几倍"。
func TestMultipleResultKeepsBothTheCountAndTheMultiplier(t *testing.T) {
	result, err := ParseMultiple(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"cloudCount": 6, "multiple": 2},
	}))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if result.CloudCount == nil || *result.CloudCount != 6 {
		t.Errorf("cloudCount 不对: %v", result.CloudCount)
	}
	if result.Multiple == nil || *result.Multiple != 2 {
		t.Errorf("multiple 不对: %v", result.Multiple)
	}

	// 没有 result 不算失败（与延期领取同一约定）
	bare, err := ParseMultiple(envelopeFrom(t, map[string]any{"code": 0, "msg": "success"}))
	if err != nil {
		t.Fatalf("没有 result 不该判失败: %v", err)
	}
	if bare.CloudCount != nil || bare.Multiple != nil {
		t.Error("没给时应是 nil")
	}
}

// ⚠ 倍数**不是**合计：这两个字段挨得很近，很容易互相赋错。
func TestMultipleIsNotTheSameQuantityAsCloudCount(t *testing.T) {
	result, err := ParseMultiple(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{"cloudCount": 50, "multiple": 10},
	}))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if result.Multiple != nil && result.CloudCount != nil && *result.Multiple == *result.CloudCount {
		t.Error("倍数与合计相等说明取了同一个字段两次")
	}
}
