package upstream

// 兑换族的**协议层**用例：`exchangeList`（只读清单）与 `exchangeV3`（下单）。
//
// 这一层要钉的四件事，全都是"看着正常、实际是另一个请求 / 另一个答案"那一类：
//
//  1. **请求形状** —— 清单是"两个查询参数、不带 deviceId 的 GET"；下单是**键序固定**的
//     POST，且额外带 `isdeviceid` 与 `origin`。少一样上游也可能回 200，
//     差别只体现在"这一笔算在哪个客户端头上"。
//  2. **`code != 0` 不是错误** —— `514` 与 `2301` 的处置完全相反（一个可重来、一个别试），
//     把它们折进错误通道就没有重试策略可言。
//  3. **异形字段** —— 价格两种拼法（`pOrder` / `POrder`）、`probability2` 是**JSON 串**、
//     串里两个字段一个是数字一个是**字符串数组**、顶层按**不连续**的 groupId 分组。
//     这四处每一个"读错"的表现都是**静默**的（拿到 nil 或 0，不报错）。
//  4. **下单体的字节数** —— 与实拍的 `content-length`（196 / 两位数偏移时 195）对上，
//     这是本仓库唯一一条能拿**字节**对拍的写请求。
//
// ⚠ 夹具是 §5.1 / §5.2 的逐字实拍**脱敏**后的形态：`deviceId` 换成定长占位（长度保留，
// 因为它进 `content-length`）、`userDomainId` 整键不进夹具。响应里没有凭据类字段。

import (
	"sort"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// exchangePrizeJSON 是 §5.1 里 `group 0` 第一件的逐字（只删掉 imageUrl / memo 的长值：
// 那两个键本层不读，留着会让夹具里出现与判据无关的站内链接）。
const exchangePrizeJSON = `{"POrder":50,"allowInside":0,"areaCode":"0","canEdit":0,"count":399291,` +
	`"dailyCount":5000,"dailyRemainderCount":4998,"endTime":"2026-12-31 23-59-59",` +
	`"groupId":0,"marketName":"sign_in_3_ex","oid":260630012,"onLine":1,"prizeId":260630012,` +
	`"prizeName":"移动云盘100万tokens叠加包","prizeSectionStatus":1,` +
	`"probability2":"{\"multi\":true,\"limitMonthQuantity\":3}",` +
	`"receiveQuantity":0,"sort":1,"startTime":"2026-07-01 09-30-00","totalCount":400000,` +
	`"type":1057,"updateTime":"2026-09-22 09-46-31","limit":0,"minRemainderCount":4998,` +
	`"pOrder":50,"nextTimeStatus":0}`

// exchangeListBody 把若干件按**不连续**的 groupId 拼成一份 §5.1 形状的信封。
//
// 这里只需要"拼出来的串是合法 JSON"，键的顺序不影响任何判据 ——
// 清单那一份的**输出**顺序由 [ExchangeList.Order] 负责，下面有专门一条钉它。
func exchangeListBody(groups map[string][]string) string {
	keys := make([]string, 0, len(groups))
	for key := range groups {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	parts := make([]string, 0, len(keys))
	for _, key := range keys {
		parts = append(parts, `"`+key+`":[`+strings.Join(groups[key], ",")+`]`)
	}
	return `{"code":0,"msg":"success","result":{` + strings.Join(parts, ",") + `}}`
}

func exchangeEnvelope(t *testing.T, body string) *Envelope {
	t.Helper()
	envelope, perr := ParseEnvelope([]byte(body))
	if perr != nil {
		t.Fatalf("夹具不是合法 JSON（必须走 ParseEnvelope：真实路径上数字是 json.Number）: %v", perr)
	}
	return envelope
}

// ─────────────────────────────────────────────────────────────────────────────
// 清单：请求形状
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeListRequestMatchesCapture ⭐ 钉 §5.1 那四条：GET、两个查询参数、
// **不带 deviceId**、无 body ⇒ 无 content-type。
//
// 反向改坏它的样子：照 `exchangeV3` 那样补一个 `deviceId` 参数或 `isdeviceid` 头 ——
// H5 那条只标了 `showLoading`，多带的那一样都会把这一枪变成"另一个客户端"。
func TestExchangeListRequestMatchesCapture(t *testing.T) {
	request := ExchangeListRequest("JWT-1", "https://referer.example/signin")
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	if want := "https://m.mcloud.139.com/ycloud/signin/page/exchangeList" +
		"?client=app&clientVersion=12.6.0"; request.URL != want {
		t.Errorf("URL 应逐字为 %q，实得 %q", want, request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("清单是只读 GET，不该有 body，实得 %q", request.Body)
	}
	if n := countHeader(request.Headers, "content-type"); n != 0 {
		t.Errorf("无 body ⇒ 不该有 content-type（实拍也没有），实得 %d 个", n)
	}
	if n := countHeader(request.Headers, "isdeviceid"); n != 0 {
		t.Errorf("实拍这条没有 isdeviceid，实得 %d 个", n)
	}
	if strings.Contains(request.URL, "deviceId") {
		t.Errorf("查询串里不该出现 deviceId（H5 未标 isDeviceId），实得 %q", request.URL)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("jwttoken 应是那一张票，实得 %q", got)
	}
	// `clientVersion`（三段）与 `appversion` 头（四段）在**同一个请求**里同时出现且不同值。
	if got := headerValue(request.Headers, "appversion"); got != "12.6.0.0" {
		t.Errorf("appversion 头应是 12.6.0.0（四段），实得 %q", got)
	}
	if !strings.HasSuffix(request.URL, "clientVersion=12.6.0") {
		t.Errorf("查询串里的 clientVersion 应是 12.6.0（三段），实得 %q", request.URL)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 清单：解析
// ─────────────────────────────────────────────────────────────────────────────

// TestParseExchangeListReadsCapturedPrize ⭐ 把 §5.1 那一件的每个判据字段都对一遍。
//
// 它挡的是三种"读错但不报错"：价格只读一种拼法、`probability2` 当对象读、
// 余量缺省被当成 0。
func TestParseExchangeListReadsCapturedPrize(t *testing.T) {
	list, perr := ParseExchangeList(exchangeEnvelope(t, exchangeListBody(map[string][]string{
		"0": {exchangePrizeJSON},
	})))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	item, found := list.Find(260630012)
	if !found {
		t.Fatalf("按 prizeId 找不到那一件，实得清单 %+v", list)
	}
	if item.PrizeID != 260630012 {
		t.Errorf("prizeId 应是 260630012（实拍是数字不是串），实得 %d", item.PrizeID)
	}
	if item.Price == nil || *item.Price != 50 {
		t.Errorf("价格应是 50（两种拼法同为 50），实得 %v", item.Price)
	}
	if item.Name == nil || *item.Name != "移动云盘100万tokens叠加包" {
		t.Errorf("prizeName 不符，实得 %v", item.Name)
	}
	if item.MarketName == nil || *item.MarketName != "sign_in_3_ex" {
		t.Errorf("marketName 应是带 _ex 后缀那一个（与签到的 sign_in_3 不同值），实得 %v", item.MarketName)
	}
	if item.MinRemainderCount == nil || *item.MinRemainderCount != 4998 {
		t.Errorf("minRemainderCount 应是 4998，实得 %v", item.MinRemainderCount)
	}
	if item.Count == nil || *item.Count != 399291 ||
		item.TotalCount == nil || *item.TotalCount != 400000 {
		t.Errorf("count/totalCount 应是 399291/400000，实得 %v / %v", item.Count, item.TotalCount)
	}
	if item.DailyCount == nil || *item.DailyCount != 5000 {
		t.Errorf("dailyCount 应是 5000，实得 %v", item.DailyCount)
	}
	if item.Type == nil || *item.Type != 1057 {
		t.Errorf("type 应是 1057，实得 %v", item.Type)
	}
	if item.GroupID == nil || *item.GroupID != 0 {
		t.Errorf("groupId 应是 0（**给了 0** 与没给必须分得开），实得 %v", item.GroupID)
	}
	if item.StartTime == nil || *item.StartTime != "2026-07-01 09-30-00" {
		t.Errorf("startTime 应逐字保留，实得 %v", item.StartTime)
	}
	// 窗口串**自己**要能解出来（解不出只是"判不了"、不报错，所以在这里钉一次）。
	started, ok := ParseExchangeTime(*item.StartTime)
	if !ok || started.Year() != 2026 || started.Month() != 7 || started.Day() != 1 ||
		started.Hour() != 9 || started.Minute() != 30 {
		t.Errorf("startTime 应解成 2026-07-01 09:30，实得 %v（ok=%v）", started, ok)
	}
	if _, ok := ParseExchangeTime("2026-07-01 09:30:00"); ok {
		t.Error("反向：写成冒号形态**不该**解得出来 —— 解出来说明用的不是实拍那个 layout")
	}
	// 月限：`limitMonthQuantity` 是数字，这一件没有 `monthGroupLimit`。
	if item.Quota == nil {
		t.Fatal("probability2 应解出月限那一层")
	}
	if item.Quota.MonthQuantity == nil || *item.Quota.MonthQuantity != 3 {
		t.Errorf("limitMonthQuantity 应是 3，实得 %v", item.Quota.MonthQuantity)
	}
	if item.Quota.GroupPrizeIDs != nil {
		t.Errorf("没有 monthGroupLimit 时该格应是 nil（≠ 空数组），实得 %v", item.Quota.GroupPrizeIDs)
	}
	// H5 的按钮逻辑：`limit == 0` 且 `minRemainderCount != 0` ⇒ 可兑。
	if !item.CanExchangeNow() {
		t.Error("这一件按实拍应判成可兑（limit=0 且余量非 0）")
	}
}

// TestParseExchangeListGroupLimitTypesDiffer ⭐ 坑表 #105 那一坑的**反向断言**。
//
// 实拍 §5.6：`limitMonthQuantity` 是**数字**、`monthGroupLimit` 是**字符串数组**。
// 若按"都是数字"读那个数组，它会**静默归零**（不报错），于是"本月组内名额已用掉"
// 这一档解释不了 —— 而更糟的是判据本身在链上（它决定发不发那一枪真扣豆的请求）。
func TestParseExchangeListGroupLimitTypesDiffer(t *testing.T) {
	body := exchangeListBody(map[string][]string{"2": {
		`{"prizeId":251230051,"limit":4,"minRemainderCount":49,` +
			`"probability2":"{\"limitMonthQuantity\":1,\"monthGroupLimit\":` +
			`[\"251230051\",\"251230052\",\"251230053\",\"251230057\"]}"}`,
	}})
	list, perr := ParseExchangeList(exchangeEnvelope(t, body))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	item, _ := list.Find(251230051)
	if item.Quota == nil || len(item.Quota.GroupPrizeIDs) != 4 {
		t.Fatalf("monthGroupLimit 四项应逐个读到，实得 %+v", item.Quota)
	}
	if item.Quota.GroupPrizeIDs[3] != "251230057" {
		t.Errorf("组内 id 应保持字符串形态，实得 %q", item.Quota.GroupPrizeIDs[3])
	}
	if item.Quota.MonthQuantity == nil || *item.Quota.MonthQuantity != 1 {
		t.Errorf("同一个串里的数字字段也要读到（两类型不冲突），实得 %v", item.Quota.MonthQuantity)
	}
	if item.CanExchangeNow() {
		t.Error("limit=4（同组别的件本月已兑）这一档不该判成可兑")
	}
}

// TestParseExchangeListPriceFallsBackToCapitalSpelling ⭐ 两键只剩一个时也要拿到价格。
//
// 实拍是 `pOrder` 与 `POrder` **同时**存在（同值），所以"只读一个键"的写法在上面
// 那条用例里**不会红** —— 必须有这条"只有一边"的，才真把回退分支钉住。
func TestParseExchangeListPriceFallsBackToCapitalSpelling(t *testing.T) {
	list, perr := ParseExchangeList(exchangeEnvelope(t, exchangeListBody(map[string][]string{
		"0": {`{"prizeId":1,"POrder":1800}`},
		"1": {`{"prizeId":2,"pOrder":50}`},
	})))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	capital, _ := list.Find(1)
	small, _ := list.Find(2)
	if capital.Price == nil || *capital.Price != 1800 {
		t.Errorf("只有 POrder 时也该拿到价格，实得 %v", capital.Price)
	}
	if small.Price == nil || *small.Price != 50 {
		t.Errorf("只有 pOrder 时也该拿到价格，实得 %v", small.Price)
	}
	// 两键都没有 ⇒ nil（**不是 0**）："0 豆"与"没给价格"必须分得开 ——
	// 后者若被判成 0，等于"任何余额都买得起"。
	none, perr := ParseExchangeList(exchangeEnvelope(t, exchangeListBody(map[string][]string{
		"0": {`{"prizeId":3}`},
	})))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	prize, _ := none.Find(3)
	if prize.Price != nil {
		t.Errorf("两个键都没有时价格应是 nil，实得 %v", prize.Price)
	}
	if prize.Limit != nil || prize.MinRemainderCount != nil || prize.CanExchangeNow() {
		t.Error("上游没给的判据必须全是 nil，且 CanExchangeNow 判 false（判不了 ⇒ 不发）")
	}
}

// TestExchangeListGroupKeysAreNotContiguous ⭐ 顶层键**不连续**，且输出顺序是数值序。
//
// 反向改坏它：把 `Order` 改成按下标 `0..n-1` 遍历（会漏掉一半组），
// 或按 map 键的字典序输出（会把 `11`、`15` 排到 `2` 前面）。
func TestExchangeListGroupKeysAreNotContiguous(t *testing.T) {
	one := `{"prizeId":7}`
	list, perr := ParseExchangeList(exchangeEnvelope(t, exchangeListBody(map[string][]string{
		"0": {one}, "1": {one}, "2": {one}, "5": {one},
		"7": {one}, "8": {one}, "9": {one}, "11": {one}, "15": {one},
	})))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	want := []string{"0", "1", "2", "5", "7", "8", "9", "11", "15"}
	if strings.Join(list.Order, ",") != strings.Join(want, ",") {
		t.Errorf("Order 应是数值升序的实拍那九个键 %v，实得 %v", want, list.Order)
	}
	if len(list.All()) != 9 {
		t.Errorf("九组各一件，摊平后应 9 件，实得 %d", len(list.All()))
	}
}

// TestParseExchangeListRejectsShapeChange ⭐ 形状一变就**响**，不"顺手兼容"。
//
// 三条都是"看着像兼容、实际把问题藏起来"的方向：
//   - `result` 是数组（那是另一份接口，不是我们读的那一条）；
//   - 某一组的值不是数组（**跳过那一组**表现成"查无此件"，比报错难发现）；
//   - 某一件缺 `prizeId`（静默当 0 ⇒ 下单打到另一件商品上，**真扣豆**）。
func TestParseExchangeListRejectsShapeChange(t *testing.T) {
	for name, body := range map[string]string{
		"result 是数组":   `{"code":0,"msg":"success","result":[]}`,
		"某一组不是数组":      `{"code":0,"msg":"success","result":{"5":{"prizeId":1}}}`,
		"某一件缺 prizeId": `{"code":0,"msg":"success","result":{"0":[{"limit":0}]}}`,
	} {
		if _, perr := ParseExchangeList(exchangeEnvelope(t, body)); perr == nil {
			t.Errorf("%s：应报错而不是给一份残缺清单", name)
		}
	}
	// 反向的**正向**：空的 result 对象是合法的"今天一件都没有"。
	if _, perr := ParseExchangeList(
		exchangeEnvelope(t, `{"code":0,"msg":"success","result":{}}`)); perr != nil {
		t.Errorf("空清单不该报错，实得 %v", perr)
	}
}

// TestParseExchangeListNonZeroCodeIsError：清单这一条**没有**"码是数据"的档。
//
// 它与 [ParseExchangeResult] 的分别待遇是刻意的：下单的码要参与分流
// （能不能再来一次），只读查询失败就是失败，交回错误让编排层记一次失败。
func TestParseExchangeListNonZeroCodeIsError(t *testing.T) {
	_, perr := ParseExchangeList(exchangeEnvelope(t, `{"code":90001,"msg":"未登录"}`))
	if perr == nil {
		t.Fatal("非 0 码应回错误")
	}
	if !strings.Contains(perr.Message(), "90001") {
		t.Errorf("错误串里应带上那个码便于排障，实得 %q", perr.Message())
	}
}

// TestParseMonthQuotaTolerances ⭐ `probability2` 的三态。
//
// 没给键 / 空串 ⇒ nil（"上游没配这一层"，不报错）；给了但解不出 JSON ⇒ **报错**。
// 后者不能静默成 nil：那一层是 `limit == 4` 的字段依据，读成"没给"会把
// "本月组内名额已用掉"判成可兑，而可兑的后果是**发一笔真扣豆的请求**。
//
// ⚠ "空串 ⇒ 没给"这一档**没有样本**（实拍四件都有值），取的是"最保守可用信息"。
func TestParseMonthQuotaTolerances(t *testing.T) {
	for _, c := range []struct {
		name        string
		probability string // 直接拼进夹具的那一截（含键名）
		wantErr     bool
	}{
		{"没有这个键", `"limit":0`, false},
		{"空串", `"probability2":""`, false},
		{"空对象", `"probability2":"{}"`, false},
		{"少一个闭括号", `"probability2":"{\"a\":1"`, true},
		{"解出来是数组", `"probability2":"[1,2]"`, true},
	} {
		_, perr := ParseExchangeList(exchangeEnvelope(t, exchangeListBody(map[string][]string{
			"0": {`{"prizeId":1,` + c.probability + `}`},
		})))
		if (perr != nil) != c.wantErr {
			t.Errorf("%s：期望报错=%v，实得 %v", c.name, c.wantErr, perr)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 下单：请求形状（**唯一能按字节对拍的写请求**）
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeV3RequestBodyIsByteIdenticalToCapture ⭐⭐ 连 `content-length` 都对上。
//
// 实拍（§5.2，4 条只有 `puzzleOffset` 不同）：
//
//	{"prizeId":251230053,"client":"app","clientVersion":"12.6.0","puzzleOffset":373,"smsCode":"","deviceId":"<89 字符>"}
//
// 文档记的是 `content-length: 196`，并补了一句"`puzzleOffset` 为两位数时 195"。
// ⇒ 这一条把**键序**钉在字节上：`WithJSONBody` 那种按字母序重排的构造法在这里会直接红
// （它会把 `client` 排到第一个）。
func TestExchangeV3RequestBodyIsByteIdenticalToCapture(t *testing.T) {
	deviceID := strings.Repeat("B", 89)
	threeDigit := int64(373)
	three := ExchangeV3Request("JWT-1", "https://referer.example/signin", deviceID, 251230053, &threeDigit)
	if got, want := string(three.Body),
		`{"prizeId":251230053,"client":"app","clientVersion":"12.6.0","puzzleOffset":373,`+
			`"smsCode":"","deviceId":"`+deviceID+`"}`; got != want {
		t.Errorf("请求体应逐字等于实拍\n实得 %s\n应为 %s", got, want)
	}
	if len(three.Body) != 196 {
		t.Errorf("实拍 content-length 是 196，实得 %d", len(three.Body))
	}
	twoDigit := int64(99)
	if got := len(ExchangeV3Request("JWT-1", "r", deviceID, 251230053, &twoDigit).Body); got != 195 {
		t.Errorf("两位数偏移时实拍是 195，实得 %d", got)
	}
}

// TestExchangeV3NullOffsetIsLiteralNull ⭐ `puzzleOffset` 的"没算出来"那一档。
//
// 实测：`null` 与空串 `""` 回的是**逐字相同**的 `{"code":501,...}`（§5.2 末），
// 所以这里只发 `null`，不留"发空串"的第二条路 —— 那不是另一个取值，只是同一种意思的
// 另一种写法，多一条分支就多一处"哪天上游开始区分"的隐患。
func TestExchangeV3NullOffsetIsLiteralNull(t *testing.T) {
	body := string(ExchangeV3Request("JWT-1", "r", "B", 1, nil).Body)
	if !strings.Contains(body, `"puzzleOffset":null`) {
		t.Errorf("nil 偏移应编成 null，实得 %s", body)
	}
	if strings.Contains(body, `"puzzleOffset":""`) {
		t.Errorf("不该发空串那一种：%s", body)
	}
}

// TestExchangeV3RequestHeadersMatchCapture 钉住 `WithH5Headers` **不管**的那两个头，
// 以及"`content-type` 只有一个"。
//
// ⚠ `origin` 用的是站点基址而不是签到页地址：实拍那一行就是 `https://m.mcloud.139.com`。
func TestExchangeV3RequestHeadersMatchCapture(t *testing.T) {
	offset := int64(144)
	request := ExchangeV3Request("JWT-1", "https://referer.example/signin", "B", 251230053, &offset)
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if request.URL != "https://m.mcloud.139.com/ycloud/signin/page/exchangeV3" {
		t.Errorf("URL 实得 %q", request.URL)
	}
	if got := headerValue(request.Headers, "isdeviceid"); got != "true" {
		t.Errorf("下单必须带 isdeviceid: true（H5 的 isDeviceId:!0），实得 %q", got)
	}
	if got := headerValue(request.Headers, "origin"); got != "https://m.mcloud.139.com" {
		t.Errorf("origin 应是站点基址，实得 %q", got)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 应恰好一个，实得 %d 个（WithHeader 是追加，重复不会报错）", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != H5ContentType {
		t.Errorf("content-type 应是 %q，实得 %q", H5ContentType, got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 下单：解析（**码是数据不是错误**）
// ─────────────────────────────────────────────────────────────────────────────

// TestParseExchangeResultSuccessReadsCapturedOrder ⭐ §5.2 记录 13071 的逐字。
func TestParseExchangeResultSuccessReadsCapturedOrder(t *testing.T) {
	const captured = `{"code":0,"msg":"success","result":{"oid":1272221744,"prizeId":251230053,` +
		`"prizeName":"网易云音乐VIP会员月卡","flag":1,"insertTime":"2026-09-22T04:16:25.453+00:00",` +
		`"linkUrl":"251230053","type":1035,"uorder":1800,"reasons":"251230053","memo":"",` +
		`"updateTime":"2026-09-22T04:16:25.453+00:00","marketName":"sign_in_3_ex","sourceId":"",` +
		`"orderType":0,"expireTime":"2026-09-30T15:59:59.453+00:00","source":0,"device":0,` +
		`"belonger":0,"groupId":2,"prizeSectionStatus":1}}`
	result, perr := ParseExchangeResult(exchangeEnvelope(t, captured))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if !result.OK() {
		t.Fatalf("code 0 应判成功，实得 %+v", result)
	}
	order := result.Order
	if order == nil {
		t.Fatal("成功必须有订单（oid 是事后对账的唯一线索）")
	}
	if order.OrderID != 1272221744 || order.PrizeID != 251230053 {
		t.Errorf("oid/prizeId 应是 1272221744/251230053，实得 %d/%d", order.OrderID, order.PrizeID)
	}
	if order.Cost == nil || *order.Cost != 1800 {
		t.Errorf("uorder 应是 1800（那一件的价格），实得 %v", order.Cost)
	}
	if order.PrizeName == nil || *order.PrizeName == "" {
		t.Errorf("prizeName 应读到，实得 %v", order.PrizeName)
	}
	if order.MarketName == nil || *order.MarketName != "sign_in_3_ex" {
		t.Errorf("marketName 应是 sign_in_3_ex，实得 %v", order.MarketName)
	}
	// linkUrl 实拍是 prizeId 的**字符串形态**，不是 URL —— 别按 URL 用（注释里已写，这条钉它）。
	if order.LinkURL == nil || *order.LinkURL != "251230053" {
		t.Errorf("linkUrl 应逐字保留，实得 %v", order.LinkURL)
	}
}

// TestParseExchangeResultMissingPrizeNameIsNotFilled ⭐ 缺名**不补空串**。
//
// 原项目把"`code == 0` 但缺 prizeName"记为它自己的一处隐性 bug；我们保留 nil，
// 让领域层能区分"上游没给名字"与"名字是空串"。
func TestParseExchangeResultMissingPrizeNameIsNotFilled(t *testing.T) {
	result, perr := ParseExchangeResult(exchangeEnvelope(t,
		`{"code":0,"msg":"success","result":{"oid":1,"prizeId":2}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if result.Order.PrizeName != nil || result.Order.Cost != nil || result.Order.LinkURL != nil {
		t.Errorf("缺的字段应保持 nil，实得 %+v", result.Order)
	}
}

// TestParseExchangeResultKeepsBusinessCodes ⭐⭐ 失败码**不**回错误。
//
// 反向改坏它：这里改用 [Envelope.EnsureOK] —— 编译照过、上面那条成功用例照绿，
// 而链上的表现是"滑块答错（该重来，代价为零）与重复兑奖（别试了）都变成一次上游错误"，
// 于是**该重试的没重试、不该重试的试了三次**。
func TestParseExchangeResultKeepsBusinessCodes(t *testing.T) {
	for _, c := range []struct {
		body string
		code int64
		msg  string
	}{
		{`{"code":514,"msg":"验证失败"}`, ExchangeCodeSliderFailed, "验证失败"},
		{`{"code":407,"msg":"非移动用户不可领奖"}`, ExchangeCodeNotTargetUser, "非移动用户不可领奖"},
		{`{"code":415,"msg":"奖品单日已耗尽"}`, ExchangeCodePrizeExhaustedToday, "奖品单日已耗尽"},
		{`{"code":2301,"msg":"重复兑奖"}`, ExchangeCodeDuplicated, "重复兑奖"},
		{`{"code":501,"msg":"滑块验证参数不能为空"}`, ExchangeCodeSliderParamMissing, "滑块验证参数不能为空"},
	} {
		result, perr := ParseExchangeResult(exchangeEnvelope(t, c.body))
		if perr != nil {
			t.Errorf("%d 那一档应作为数据交回，实得错误 %v", c.code, perr)
			continue
		}
		if result.Code != c.code || result.Message != c.msg || result.OK() {
			t.Errorf("应得 code=%d msg=%q，实得 %+v", c.code, c.msg, result)
		}
		if result.Order != nil {
			t.Errorf("%d 这一档没有 result，不该凭空造一张订单", c.code)
		}
	}
}

// TestParseExchangeResultRefusesToAssumeSuccess ⭐ 五种"判不了成交"都必须响。
//
// 这一步花的是真豆，所以**绝不默认成功**：没有 code / code 0 却无 result /
// 缺 oid（钱扣了、订单号没有 ⇒ 事后无从对账）/ 缺 prizeId / result 不是对象。
func TestParseExchangeResultRefusesToAssumeSuccess(t *testing.T) {
	for name, body := range map[string]string{
		"没有 code":      `{"msg":"success"}`,
		"成功但无 result":  `{"code":0,"msg":"success"}`,
		"成功但缺 oid":     `{"code":0,"msg":"success","result":{"prizeId":1}}`,
		"成功但缺 prizeId": `{"code":0,"msg":"success","result":{"oid":1}}`,
		"result 不是对象":  `{"code":0,"msg":"success","result":"x"}`,
	} {
		if _, perr := ParseExchangeResult(exchangeEnvelope(t, body)); perr == nil {
			t.Errorf("%s：应报错，绝不能判成成功", name)
		}
	}
}
