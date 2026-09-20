package upstream

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 奖品记录
// ─────────────────────────────────────────────────────────────────────────────

// TestPrizeRecordsParseTheCapturedPendingPrize 用**实拍条目**钉住每一条字段。
//
// 判据来自 `app-signin-vectors.json` 的 `prizeRecords` 段（2026-09-19 账号 159
// 的「待领取」页签）。它是这一族里唯一有实拍的一条 —— 其余字段声明都靠它背书，
// 所以这里**逐字段**比，不挑几个代表性的。
func TestPrizeRecordsParseTheCapturedPendingPrize(t *testing.T) {
	vectors := loadAppSigninVectors(t)
	got, perr := ParsePrizeRecords(envelopeFrom(t, vectors.PrizeRecords.Raw))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(got.Records) != 1 {
		t.Fatalf("实拍那一页应有 1 条，实得 %d", len(got.Records))
	}
	item := got.Records[0]

	want := map[string]string{
		"prizeName":  "移动云盘1000万tokens叠加包",
		"insertTime": "2026-09-16T11:12:54.000+00:00",
		"expireTime": "2026-09-30T15:59:59.000+00:00",
		"oid":        "1270932714",
		"marketName": "sign_in_3_ex",
	}
	gotStr := map[string]*string{
		"prizeName":  item.PrizeName,
		"insertTime": item.InsertTime,
		"expireTime": item.ExpireTime,
		"oid":        item.Oid,
		"marketName": item.MarketName,
	}
	for key, expect := range want {
		actual := gotStr[key]
		if actual == nil {
			t.Errorf("%s 应取到值，实得 nil", key)
			continue
		}
		if *actual != expect {
			t.Errorf("%s 应为 %q，实得 %q", key, expect, *actual)
		}
	}

	wantNum := map[string]int64{"flag": 1, "prizeId": 260630014, "type": 1057}
	gotNum := map[string]*int64{"flag": item.Flag, "prizeId": item.PrizeID, "type": item.PrizeType}
	for key, expect := range wantNum {
		actual := gotNum[key]
		if actual == nil {
			t.Errorf("%s 应取到值，实得 nil", key)
			continue
		}
		if *actual != expect {
			t.Errorf("%s 应为 %d，实得 %d", key, expect, *actual)
		}
	}
}

// TestPrizeRecordExpireTimeIsPresentEvenThoughTheTemplateNeverRendersIt 守住
// 那条被改过一次的注释：列表项模板只渲染 `insertTime`，但 `expireTime`
// **一直在响应里**。「模板没渲染」≠「响应没有」。
//
// ⚠ 反向测试已确认：把 `ExpireTime` 去掉（改去读一个不存在的键）后本条变红 ——
// 而它变红的方式是 `*actual == nil`，不是"值不对"，这正是当年混淆时的样子。
func TestPrizeRecordExpireTimeIsPresentEvenThoughTheTemplateNeverRendersIt(t *testing.T) {
	vectors := loadAppSigninVectors(t)
	got, _ := ParsePrizeRecords(envelopeFrom(t, vectors.PrizeRecords.Raw))
	if len(got.Records) == 0 {
		t.Fatal("实拍那一页不应为空")
	}
	if got.Records[0].ExpireTime == nil {
		t.Fatal("expireTime 一直存在于响应里（中奖后 14 天）；拿不到说明字段名或取值路径错了")
	}
}

// TestPrizeRecordOidAcceptsBothNumberAndString：`oid` 实拍是数字，而 H5 把它拼进
// URL 查询串 —— 上游两种形态都出现过，只收一种的表现是"某些奖品点「去领取」时 id 是空的"。
//
// ⚠ 反向测试已确认：删掉 `optIDField` 的 `json.Number` 分支后本条变红
// （数字形态拿不到值），而这条路径挂了**不会**有任何报错，只是 id 为空。
func TestPrizeRecordOidAcceptsBothNumberAndString(t *testing.T) {
	cases := []struct {
		name string
		oid  any
		want string
	}{
		{"数字形态（实拍）", float64(1270932714), "1270932714"},
		{"字符串形态", "1270932714", "1270932714"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, perr := ParsePrizeRecords(envelopeFrom(t, map[string]any{
				"code": 0, "msg": "success",
				"result": map[string]any{"records": []any{map[string]any{"oid": c.oid}}},
			}))
			if perr != nil {
				t.Fatalf("解析失败: %v", perr)
			}
			if len(got.Records) != 1 || got.Records[0].Oid == nil {
				t.Fatalf("两种形态都要能取到 oid，实得 %+v", got.Records)
			}
			if *got.Records[0].Oid != c.want {
				t.Errorf("oid 应为 %q，实得 %q", c.want, *got.Records[0].Oid)
			}
		})
	}
}

// TestPrizeStatusConstantsAreTheHardcodedTabIds：三个页签 id 是 H5 里写死的
// 字面量（`1` / `2` / `10`），**不是**从响应里读出来的。
// ⚠ 「已过期」是 `10` 不是 `3` —— 顺手"补一个 3 上去"是最容易犯的错。
func TestPrizeStatusConstantsAreTheHardcodedTabIds(t *testing.T) {
	if PrizeStatusPending != 1 || PrizeStatusReceived != 2 || PrizeStatusExpired != 10 {
		t.Errorf("页签 id 应为 1/2/10，实得 %d/%d/%d",
			PrizeStatusPending, PrizeStatusReceived, PrizeStatusExpired)
	}
	if PrizeMarketName != "sign_in_3_ex" {
		t.Errorf("marketname 应为 sign_in_3_ex，实得 %q", PrizeMarketName)
	}
}

// TestPrizeRecordRequestEncodesMarketName：GET + 四个查询参数，且 `marketname`
// 走百分号编码（它含下划线，编码后不变；这条守的是"确实调了编码函数"）。
func TestPrizeRecordRequestEncodesMarketName(t *testing.T) {
	out := PrizeRecordRequest("jwt", "https://referer", PrizeMarketName, PrizeStatusPending, 1, 10)
	if out.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", out.Method)
	}
	if len(out.Body) != 0 {
		t.Errorf("这一族没有 body，实得 %q", out.Body)
	}
	if countHeader(out.Headers, "isdeviceid") != 0 {
		t.Error("H5 没标 isDeviceId ⇒ 不该注入设备头；带上它反而是错的")
	}
	for _, want := range []string{
		"/ycloud/signin/public/record?",
		"marketname=sign_in_3_ex",
		"type=1",
		"pageNumber=1",
		"pageSize=10",
	} {
		if !strings.Contains(out.URL, want) {
			t.Errorf("URL 应含 %q，实得 %q", want, out.URL)
		}
	}
}

// TestPrizeRecordEmptyPageIsNotAnError：三个页签里总有两个是空的 ——
// "这一档没有记录"是最常见的**正常**答案，打成解析错误会让整条查询天天报错。
func TestPrizeRecordEmptyPageIsNotAnError(t *testing.T) {
	for name, raw := range map[string]any{
		"没有 result":        map[string]any{"code": 0, "msg": "success"},
		"result 无 records": map[string]any{"code": 0, "msg": "success", "result": map[string]any{}},
		"records 是空数组":     map[string]any{"code": 0, "msg": "success", "result": map[string]any{"records": []any{}}},
	} {
		t.Run(name, func(t *testing.T) {
			got, perr := ParsePrizeRecords(envelopeFrom(t, raw))
			if perr != nil {
				t.Fatalf("空页不应报错，实得 %v", perr)
			}
			if len(got.Records) != 0 {
				t.Errorf("空页应给空数组，实得 %d 条", len(got.Records))
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 豆的收支流水
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudRecordV2CarriesFourQueryParamsAndNoDevice：GET + 四个查询参数、
// **没有 body**、**没有设备头**（H5 没标 `isDeviceId`）。
func TestCloudRecordV2CarriesFourQueryParamsAndNoDevice(t *testing.T) {
	out := CloudRecordV2Request("jwt", "https://referer", RecordBookBean, RecordFilterEarned, 2, 10)
	if out.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", out.Method)
	}
	if len(out.Body) != 0 {
		t.Errorf("这条没有 body，实得 %q", out.Body)
	}
	if countHeader(out.Headers, "isdeviceid") != 0 {
		t.Error("流水那条 H5 没标 isDeviceId ⇒ 不该注入设备头")
	}
	for _, want := range []string{
		"/ycloud/signin/public/cloudRecordV2?",
		"aiDou=1",
		"type=1",
		"pageNumber=2",
		"pageSize=10",
	} {
		if !strings.Contains(out.URL, want) {
			t.Errorf("URL 应含 %q，实得 %q", want, out.URL)
		}
	}
}

// TestCloudRecordFilterSpentIsMinusOne：`type` 的「使用记录」是 **-1** 不是 2 ——
// 写错的表现是"使用记录页签显示的是获得记录"，而两者都能正常返回 200。
func TestCloudRecordFilterSpentIsMinusOne(t *testing.T) {
	if RecordFilterSpent != -1 {
		t.Errorf("「使用记录」应为 -1，实得 %d", RecordFilterSpent)
	}
	if RecordFilterAll != 0 || RecordFilterEarned != 1 {
		t.Errorf("页签应为全部 0 / 获得 1，实得 %d / %d", RecordFilterAll, RecordFilterEarned)
	}
}

// TestCloudRecordsTotalIsEntryCountNotAmount：`result.total` 是**条目总数**（分页用），
// 不是金额。判据是 H5 的翻页逻辑：`records.length + … >= total` 就判"加载完了" ——
// 能这么比的只有条数。
//
// 夹具里故意让"条数"与"金额"取不同的值，这样按金额去取会当场错。
func TestCloudRecordsTotalIsEntryCountNotAmount(t *testing.T) {
	got, perr := ParseCloudRecords(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{
			"total":   37, // 条数
			"records": []any{map[string]any{"amount": 100}},
		},
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.Total == nil || *got.Total != 37 {
		t.Errorf("total 是条数，应为 37，实得 %v", got.Total)
	}
	if len(got.Records) != 1 {
		t.Errorf("本页应 1 条，实得 %d", len(got.Records))
	}
}

// TestCloudRecordsMissingRecordsIsEmptyNotError：审计接口里"这一档没有记录"
// 是正常答案（例如改名前的账本本来就只到 2026-07-30）。
func TestCloudRecordsMissingRecordsIsEmptyNotError(t *testing.T) {
	got, perr := ParseCloudRecords(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{},
	}))
	if perr != nil {
		t.Fatalf("空账本不应报错，实得 %v", perr)
	}
	if len(got.Records) != 0 {
		t.Errorf("应给空数组，实得 %d 条", len(got.Records))
	}
	if got.Total != nil {
		t.Errorf("上游没给 total 时应是 nil（≠ 0），实得 %d", *got.Total)
	}
}

// TestCloudRecordsKeepEntriesAsIs：本仓库**没有 `records[]` 的实拍**，
// 所以条目原样交出去 —— 编字段等于把猜测变成断言。
// 这条守的是"没有因为声明字段而把它拦腰截断"。
func TestCloudRecordsKeepEntriesAsIs(t *testing.T) {
	got, _ := ParseCloudRecords(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{
			"records": []any{map[string]any{"someUnknownField": "x"}},
		},
	}))
	entry, ok := got.Records[0].(map[string]any)
	if !ok {
		t.Fatalf("条目应原样保留为对象，实得 %T", got.Records[0])
	}
	if entry["someUnknownField"] != "x" {
		t.Errorf("未声明的字段不该被丢掉，实得 %v", entry)
	}
}

// TestCloudRecordBooksAreTwoLedgersNotTwoCurrencies：`RECORD_BOOK_*` 两档分开
// 只是因为记录跨了 2026-07-31 那次 1:1 改名，不是两个池子、也不能互相换算。
func TestCloudRecordBooksAreTwoLedgersNotTwoCurrencies(t *testing.T) {
	if RecordBookCloud != 0 || RecordBookBean != 1 {
		t.Errorf("账本应为 0（改名之前）/ 1（改名之后），实得 %d / %d",
			RecordBookCloud, RecordBookBean)
	}
	// 换账本只改 `aiDou` 一个参数，其余完全一致 —— 能换算的话这里会是两条不同接口。
	before := CloudRecordV2Request("j", "r", RecordBookCloud, RecordFilterAll, 1, 10)
	after := CloudRecordV2Request("j", "r", RecordBookBean, RecordFilterAll, 1, 10)
	if !strings.Contains(before.URL, "aiDou=0") || !strings.Contains(after.URL, "aiDou=1") {
		t.Errorf("两账本应由 aiDou 区分，实得 %q / %q", before.URL, after.URL)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 备份好礼
// ─────────────────────────────────────────────────────────────────────────────

// TestBackupGiftUsesTheCaiyunHostNotMcloud：这个接口不在新 H5 的签到页里，
// 沿用旧项目那条 `caiyun.feixin.10086.cn`。换主机有实测依据（裸请求回 `90001`，
// 与 `m.mcloud.139.com` 上的接口回同一个信封 ⇒ 同一道鉴权网关后面）。
func TestBackupGiftUsesTheCaiyunHostNotMcloud(t *testing.T) {
	info := BackupGiftInfoRequest("jwt", "https://referer")
	receive := BackupGiftReceiveRequest("jwt", "https://referer")
	if !strings.HasPrefix(info.URL, BaseCaiyun) || !strings.HasPrefix(receive.URL, BaseCaiyun) {
		t.Errorf("备份好礼走彩云主机，实得 %q / %q", info.URL, receive.URL)
	}
	if strings.Contains(info.URL, BaseMcloud) || strings.Contains(receive.URL, BaseMcloud) {
		t.Errorf("备份好礼**不**在 m.mcloud.139.com 上，实得 %q / %q", info.URL, receive.URL)
	}
	if info.Method != core.MethodGet || receive.Method != core.MethodGet {
		t.Errorf("两条都是 GET，实得 %s / %s", info.Method, receive.Method)
	}
	if !strings.HasSuffix(info.URL, "/market/backupgift/info") {
		t.Errorf("info 路径错: %q", info.URL)
	}
	if !strings.HasSuffix(receive.URL, "/market/backupgift/receive") {
		t.Errorf("receive 路径错: %q", receive.URL)
	}
}

// TestBackupGiftStateThreeWay：`state` 是这条接口的主判据，三态逐字来自
// 旧项目 `backupGiftTask` 的 `switch (state)`。
// ⚠ `0` 是"可领"、`-1` 是"没开备份" —— 别按"0 = 关"的直觉去写。
func TestBackupGiftStateThreeWay(t *testing.T) {
	cases := []struct {
		state int64
		want  int64
		name  string
	}{
		{-1, BackupGiftOff, "未开启备份 ⇒ 提示去 App 开"},
		{1, BackupGiftClaimed, "本月已领过 ⇒ 不领"},
		{0, BackupGiftClaimable, "可领 ⇒ 发领取请求"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, perr := ParseBackupGift(envelopeFrom(t, map[string]any{
				"code": 0, "msg": "success",
				"result": map[string]any{"state": c.state, "curMonth": true},
			}))
			if perr != nil {
				t.Fatalf("解析失败: %v", perr)
			}
			if got.State != c.want {
				t.Errorf("state 应为 %d，实得 %d", c.want, got.State)
			}
		})
	}
	if BackupGiftOff != -1 || BackupGiftClaimed != 1 || BackupGiftClaimable != 0 {
		t.Errorf("三态常量应为 -1/1/0，实得 %d/%d/%d",
			BackupGiftOff, BackupGiftClaimed, BackupGiftClaimable)
	}
}

// TestBackupGiftStateIsRequired：`state` 缺了就无法回答"该不该领"，
// 而这正是本结构存在的唯一理由 ⇒ 必须报错，不能静默认成 0（"可领"）。
func TestBackupGiftStateIsRequired(t *testing.T) {
	_, perr := ParseBackupGift(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"curMonth": true},
	}))
	if perr == nil {
		t.Fatal("缺 state 应报错 —— 静默认成 0 等于把「没开备份」当成「可以领」")
	}
	if !strings.Contains(perr.Message(), "state") {
		t.Errorf("报错应点名 state，实得 %q", perr.Message())
	}
}

// TestBackupGiftCurMonthIsTruthyAcrossShapes：`curMonth` 在旧项目里当布尔用，
// 所以布尔 / 数字 / 字符串三种形态都认，缺了当 `false`。
//
// ⚠ 反向测试已确认：只收 `bool` 之后数字与字符串两档变红 ——
// 而这两档挂了的表现是"本月明明有备份好礼，却提示无礼物可领"。
func TestBackupGiftCurMonthIsTruthyAcrossShapes(t *testing.T) {
	cases := []struct {
		name  string
		value any
		want  bool
	}{
		{"布尔 true", true, true},
		{"布尔 false", false, false},
		{"数字 1", float64(1), true},
		{"数字 0", float64(0), false},
		{"字符串 1", "1", true},
		{"字符串 0", "0", false},
		{"字符串 false", "false", false},
		{"空串", "", false},
		{"缺失", nil, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			result := map[string]any{"state": 0}
			if c.value != nil {
				result["curMonth"] = c.value
			}
			got, perr := ParseBackupGift(envelopeFrom(t, map[string]any{
				"code": 0, "msg": "success", "result": result,
			}))
			if perr != nil {
				t.Fatalf("解析失败: %v", perr)
			}
			if got.CurMonth != c.want {
				t.Errorf("curMonth 应为 %v，实得 %v", c.want, got.CurMonth)
			}
		})
	}
}

// TestBackupGiftReceiveResultIsANumberNotAnObject：这条的 `result` **不是对象**，
// 直接是个豆数。按对象去取字段的表现是"永远领到 0 豆"且不报错。
func TestBackupGiftReceiveResultIsANumberNotAnObject(t *testing.T) {
	got, perr := ParseBackupGiftReceive(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": 500,
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.Received == nil || *got.Received != 500 {
		t.Errorf("应领到 500 豆，实得 %v", got.Received)
	}
}

// TestBackupGiftReceiveMissingResultIsNilNotZero：`result` 缺失**不判失败**
// （成败由 `code` 决定），但要与"领到 0"区分开 —— `nil` ≠ `0`。
func TestBackupGiftReceiveMissingResultIsNilNotZero(t *testing.T) {
	got, perr := ParseBackupGiftReceive(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{},
	}))
	if perr != nil {
		t.Fatalf("result 不是数字不应算失败，实得 %v", perr)
	}
	if got.Received != nil {
		t.Errorf("上游没给数字时应是 nil（≠ 0），实得 %d", *got.Received)
	}
}

// TestRecordsFamilySendsNoDeviceHeader：这一族三条**都不带** `isdeviceid` ——
// 与 `receiveV3`（必带）正好相反。混用会让"该带的没带 / 不该带的带了"，
// 而后者的报错不会提示"设备头多余"。
func TestRecordsFamilySendsNoDeviceHeader(t *testing.T) {
	family := map[string]core.HTTPRequest{
		"cloudRecordV2":      CloudRecordV2Request("j", "r", RecordBookBean, RecordFilterAll, 1, 10),
		"record":             PrizeRecordRequest("j", "r", PrizeMarketName, PrizeStatusPending, 1, 10),
		"backupgift/info":    BackupGiftInfoRequest("j", "r"),
		"backupgift/receive": BackupGiftReceiveRequest("j", "r"),
	}
	for name, request := range family {
		if countHeader(request.Headers, "isdeviceid") != 0 {
			t.Errorf("%s 不该带 isdeviceid（H5 没标），带上是错的", name)
		}
	}
}
