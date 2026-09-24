package upstream

// 趣玩AI抽奖族的**协议层**用例：请求形状与解析。
//
// 这一层只有两件事值得钉，而且都是"看起来正常、实际是另一个请求"的那类：
//
//  1. **请求形状** —— `page/lottery` 的 body 只有一个键，且值来自上一枪的响应。
//     body 少一个键、多一个键、或把 moduleId 写死，上游**都可能回 200**，
//     只有账目与"抽的是哪一期"会不对（判据因此在字节上，不在返回码里）。
//  2. **必填 vs 可选** —— `moduleId` 必填（缺了发不出 POST），三个计数可选
//     （`timed/getmodule` 那一份里它们全是 `null`，把 `null` 读成 `0` 会判成"没次数"）。
//
// ⚠ 夹具里的值全是合成/实拍脱敏后的形态，不含任何凭据：这两条请求的响应里
// 没有 token 类字段，唯一的账号标识（`phone`）已经排除在结构之外。

import (
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// funaiCapturedModule 是 `page/getmodule` 的**实拍形状**（顺序 2 / 7），
// 只留本层读的那几个键：`taskIds[]` 那 16 项里带着站内链接与 token 参数，
// 不进夹具、也不进结构。
const funaiCapturedModule = `{"code":0,"msg":"success","result":{` +
	`"drawTimes":1,"drawsUsed":0,"todayGetLotteryCount":1,` +
	`"isDraw":null,"drawTime":null,"drawResult":null,"flag":1,` +
	`"moduleId":"1-250730","subtitle":"趣玩AI赢好礼",` +
	`"startTime":"2025-05-28T00:00:00","endTime":"2026-12-31T23:59:59",` +
	`"pullLink":"https://m.mcloud.139.com/portal/cloudCircle/v1/index.html?path=National_playAI"}}`

// funaiModuleEnvelope 把上面那份 JSON 走一遍 [ParseEnvelope]。
//
// ⚠ 必须走 `ParseEnvelope`（它用 `json.Number`）而不是直接 hand-build `map[string]any`：
// 真实路径上数字来自响应体解析，只测手搓的那条会让 `asInt64` 的数字分支挂掉而全绿。
func funaiModuleEnvelope(t *testing.T, body string) *Envelope {
	t.Helper()
	envelope, perr := ParseEnvelope([]byte(body))
	if perr != nil {
		t.Fatalf("夹具不是合法 JSON: %v", perr)
	}
	return envelope
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求形状
// ─────────────────────────────────────────────────────────────────────────────

// TestFunaiModuleRequestIsABareGetLikeTheCapture ⭐ 钉住实拍（顺序 2、7）的四件事：
// 裸 GET（**无查询串**）、**空 body**、**无 `isdeviceid`**、**无 `content-type`**。
//
// 反向改坏它的样子：照 [ReceiveV3Request] 抄一句 `.WithHeader("isdeviceid","true")` ——
// 请求看着更像"正经接口"，但已经不是抓包里那一条了。
func TestFunaiModuleRequestIsABareGetLikeTheCapture(t *testing.T) {
	request := FunaiModuleRequest("JWT-1", "https://referer.example/funai", nil)
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	if request.URL != "https://m.mcloud.139.com/ycloud/funai/api/page/getmodule" {
		t.Errorf("URL 应逐字为抓包那一条（无查询串），实得 %q", request.URL)
	}
	if strings.Contains(request.URL, "?") {
		t.Errorf("这条没有查询参数，实得 %q", request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("请求体应是空的（实拍 body 为 ''），实得 %q", request.Body)
	}
	if n := countHeader(request.Headers, "isdeviceid"); n != 0 {
		t.Errorf("实拍没有 isdeviceid，实得 %d 个", n)
	}
	if n := countHeader(request.Headers, "content-type"); n != 0 {
		t.Errorf("GET 无 body ⇒ 不该有 content-type（实拍也没有），实得 %d 个", n)
	}
	if n := countHeader(request.Headers, "jwttoken"); n != 1 {
		t.Errorf("jwttoken 应恰好一个，实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "showloading"); got != "true" {
		t.Errorf("showloading 应是 true，实得 %q", got)
	}
	// ⚠ 实拍这个头的值**已被归档脱敏**，看不出这一页带的是哪个市场名。
	// 这里钉的是"与仓库其余 H5 接口同一个公共值"，不是"与实拍逐字相同"。
	if got := headerValue(request.Headers, "activityid"); got != MarketName {
		t.Errorf("activityid 应是公共值 %q，实得 %q", MarketName, got)
	}
}

// TestFunaiModuleDeviceIDHeaderIsOptionalEitherWay：给了就**恰好一个** `deviceid`，
// 不给就一个都没有 —— 两条都要钉。
//
// 反向改坏它的样子：写成 `WithHeader("deviceid", "")`（"没值也发个头"）——
// 那是**第三种**请求，实拍里不存在，上游回什么也没有样本。
func TestFunaiModuleDeviceIDHeaderIsOptionalEitherWay(t *testing.T) {
	absent := FunaiModuleRequest("JWT-1", "https://referer.example/funai", nil)
	if n := countHeader(absent.Headers, "deviceid"); n != 0 {
		t.Errorf("缺省不该发 deviceid，实得 %d 个", n)
	}
	id := "JWT-DEV-VALUE"
	present := FunaiModuleRequest("JWT-1", "https://referer.example/funai", &id)
	if n := countHeader(present.Headers, "deviceid"); n != 1 {
		t.Fatalf("给了就该发一个，实得 %d 个", n)
	}
	if got := headerValue(present.Headers, "deviceid"); got != id {
		t.Errorf("deviceid 应是给的那个值，实得 %q", got)
	}
	// 同名头不许叠加：追加两个的表现是上游按第一个解析，看起来"完全正常"。
	if n := countHeader(present.Headers, "jwttoken"); n != 1 {
		t.Errorf("补头不该把 jwttoken 挤成两个，实得 %d 个", n)
	}
}

// TestFunaiLotteryRequestBodyIsTheCapturedBytes ⭐ 这一条是本文件最贵的断言。
//
// 实拍：`POST …/funai/api/page/lottery`，**无查询串**，
// body 逐字 `{"moduleId":"1-250730"}`（`content-length: 23`），
// `content-type: application/json;charset=UTF-8`（带 charset 的那个），
// **没有** `isdeviceid`。
//
// 为什么连字节数都钉：body 多一个键（比如"顺手加上 marketName"）或少一个键，
// 上游都可能回 200 —— 而它已经**不是抓包里那条请求**了。
func TestFunaiLotteryRequestBodyIsTheCapturedBytes(t *testing.T) {
	request, perr := FunaiLotteryRequest("JWT-1", "https://referer.example/funai", "1-250730", nil)
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if request.URL != "https://m.mcloud.139.com/ycloud/funai/api/page/lottery" {
		t.Errorf("URL 应逐字为抓包那一条，实得 %q", request.URL)
	}
	if got := string(request.Body); got != `{"moduleId":"1-250730"}` {
		t.Errorf("body 应逐字为 %s，实得 %s", `{"moduleId":"1-250730"}`, got)
	}
	if n := len(request.Body); n != 23 {
		t.Errorf("content-length 实拍是 23，实得 %d（body=%q）", n, request.Body)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应恰好一个（两个时上游按第一个解析），实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != H5ContentType {
		t.Errorf("content-type 应是 %q（带 charset），实得 %q", H5ContentType, got)
	}
	if n := countHeader(request.Headers, "isdeviceid"); n != 0 {
		t.Errorf("实拍这条也没有 isdeviceid，实得 %d 个", n)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("jwttoken 应是合成票 JWT-1，实得 %q", got)
	}
}

// TestFunaiLotteryModuleIDIsNeverHardcoded：换一个模块 id 就换一份 body。
//
// 反向改坏它的样子：把 body 里的值写成常量 `1-250730`（实拍那一期）——
// 换一期活动之后，本函数仍然**编译得过、测试也可能还绿**（如果只钉实拍的 body），
// 但抽的是过期模块。
func TestFunaiLotteryModuleIDIsNeverHardcoded(t *testing.T) {
	for _, moduleID := range []string{"1-250730", "2-260324-1", "9-999999"} {
		request, perr := FunaiLotteryRequest("JWT-1", "https://referer.example/funai", moduleID, nil)
		if perr != nil {
			t.Fatalf("构造失败: %v", perr)
		}
		want := `{"moduleId":"` + moduleID + `"}`
		if got := string(request.Body); got != want {
			t.Errorf("body 应为 %s，实得 %s", want, got)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 解析：必填 vs 可选
// ─────────────────────────────────────────────────────────────────────────────

// TestParseFunaiModuleParsesTheCapturedShape 用实拍那份钉每个字段。
func TestParseFunaiModuleParsesTheCapturedShape(t *testing.T) {
	got, perr := ParseFunaiModule(funaiModuleEnvelope(t, funaiCapturedModule))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.ModuleID != "1-250730" {
		t.Errorf("moduleId 应是 1-250730，实得 %q", got.ModuleID)
	}
	if got.DrawTimes == nil || *got.DrawTimes != 1 {
		t.Errorf("drawTimes 应是 1，实得 %v", got.DrawTimes)
	}
	if got.DrawsUsed == nil || *got.DrawsUsed != 0 {
		t.Errorf("drawsUsed 应是 0（注意：nil 与 0 是两回事），实得 %v", got.DrawsUsed)
	}
	if got.TodayGetLotteryCount == nil || *got.TodayGetLotteryCount != 1 {
		t.Errorf("todayGetLotteryCount 应是 1，实得 %v", got.TodayGetLotteryCount)
	}
	// 起止时刻：**按东八区解释**（实拍那两个串没有时区）。
	if got.StartTime == nil || got.StartTime.UTC().Format(time.RFC3339) != "2025-05-27T16:00:00Z" {
		t.Errorf("startTime 应折成 2025-05-27T16:00:00Z（= 东八区 05-28 00:00），实得 %v", got.StartTime)
	}
	if got.EndTime == nil || got.EndTime.UTC().Format(time.RFC3339) != "2026-12-31T15:59:59Z" {
		t.Errorf("endTime 应折成 2026-12-31T15:59:59Z（= 东八区 12-31 23:59:59），实得 %v", got.EndTime)
	}
}

// TestFunaiActivityTimeLayoutIsTheCapturedOne：那两个串**不是** RFC 3339，
// 也没有 `Z` / `+08:00` 后缀（实拍逐字 `2026-12-31T23:59:59`）。
//
// 反向改坏它的样子：用 `time.RFC3339` 去解 —— 它会**解不出来**，
// 于是活动期永远"判不了"，链上一步都走不到抽奖。
func TestFunaiActivityTimeLayoutIsTheCapturedOne(t *testing.T) {
	if _, err := time.Parse(time.RFC3339, "2026-12-31T23:59:59"); err == nil {
		t.Error("前提变了：实拍那个串按 RFC 3339 是能解的，本注释与判据要一起复核")
	}
	if _, err := time.Parse(FunaiActivityTimeLayout, "2026-12-31T23:59:59"); err != nil {
		t.Errorf("layout 应能解实拍那个串: %v", err)
	}
	// 归档里 `startTime` 晚于 `endTime` 这种坏形状**没有样本** ⇒ 交给 app 层那一档"判不了"，
	// 本层只负责"解不出来就给 nil"。
}

// TestParseFunaiModuleUnknownTimeShapeIsNilNotError：形态不认识 ⇒ `nil`（判不了），
// 不报错。报错会把一次**能抽**的机会变成解析失败，而"上游换了时间格式"
// 真正该表现为"判不了 ⇒ 不发请求"。
func TestParseFunaiModuleUnknownTimeShapeIsNilNotError(t *testing.T) {
	for name, body := range map[string]string{
		"epoch 数字": `{"code":0,"msg":"success","result":{"moduleId":"1-1","startTime":1769932800}}`,
		"带 Z 的串":   `{"code":0,"msg":"success","result":{"moduleId":"1-1","endTime":"2026-12-31T23:59:59Z"}}`,
		"日期无时刻":    `{"code":0,"msg":"success","result":{"moduleId":"1-1","endTime":"2026-12-31"}}`,
		"空串":       `{"code":0,"msg":"success","result":{"moduleId":"1-1","endTime":""}}`,
	} {
		t.Run(name, func(t *testing.T) {
			got, perr := ParseFunaiModule(funaiModuleEnvelope(t, body))
			if perr != nil {
				t.Fatalf("形态不认识不该报错，实得 %v", perr)
			}
			if got.StartTime != nil || got.EndTime != nil {
				t.Errorf("应给 nil（判不了），实得 %v / %v", got.StartTime, got.EndTime)
			}
		})
	}
}

// TestParseFunaiModuleNullCountsAreNilNotZero ⭐ `timed/getmodule` 那份（顺序 3、8）
// 的**每一个**模块都是 `drawTimes:null, drawsUsed:null, todayGetLotteryCount:null`。
//
// 把 `null` 读成 `0` 的表现：判据算出"剩余 0 次"，于是**天天报"没有次数"**，
// 而这个形状压根不是判据的来源（判据只问 `page/getmodule`）。
func TestParseFunaiModuleNullCountsAreNilNotZero(t *testing.T) {
	got, perr := ParseFunaiModule(funaiModuleEnvelope(t,
		`{"code":0,"msg":"success","result":{"moduleId":"2-260324-1",`+
			`"drawTimes":null,"drawsUsed":null,"todayGetLotteryCount":null,`+
			`"isDraw":null,"drawTime":null,"drawResult":null}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.DrawTimes != nil {
		t.Errorf("drawTimes 是 null，应给 nil（≠ 0），实得 %d", *got.DrawTimes)
	}
	if got.DrawsUsed != nil {
		t.Errorf("drawsUsed 是 null，应给 nil，实得 %d", *got.DrawsUsed)
	}
	if got.TodayGetLotteryCount != nil {
		t.Errorf("todayGetLotteryCount 是 null，应给 nil，实得 %d", *got.TodayGetLotteryCount)
	}
	// 三个 null 的"已抽过"字段**不参与解析**：结构里压根没有它们（见 FunaiModule 的注释）。
	if got.ModuleID != "2-260324-1" {
		t.Errorf("moduleId 仍要取到，实得 %q", got.ModuleID)
	}
	if got.StartTime != nil || got.EndTime != nil {
		t.Errorf("上游没给起止时间时应是两个 nil，实得 %v / %v", got.StartTime, got.EndTime)
	}
}

// TestParseFunaiModuleRequiresModuleID：`moduleId` 是这条链上唯一的必填 ——
// 缺了它就发不出 POST，所以宁可在这里立刻报错，也不要带着空 id 去抽。
func TestParseFunaiModuleRequiresModuleID(t *testing.T) {
	for name, body := range map[string]string{
		"没有 moduleId":  `{"code":0,"msg":"success","result":{"drawTimes":1}}`,
		"moduleId 是数字": `{"code":0,"msg":"success","result":{"moduleId":1250730}}`,
		"moduleId 是空串": `{"code":0,"msg":"success","result":{"moduleId":""}}`,
		"没有 result":    `{"code":0,"msg":"success"}`,
	} {
		t.Run(name, func(t *testing.T) {
			_, perr := ParseFunaiModule(funaiModuleEnvelope(t, body))
			if perr == nil {
				t.Fatal("取不到可用的 moduleId 必须报错 —— 静默往下走会发一条空 id 的 POST")
			}
			if !strings.Contains(perr.Message(), "moduleId") {
				t.Errorf("报错该点名 moduleId，实得 %q", perr.Message())
			}
		})
	}
}

// TestParseFunaiModuleRejectsTheArrayShape：`timed/getmodule` 的 `result` 是**数组**，
// 与 `page/getmodule` 不是一个形状 ⇒ 同一条"判不了就报错"的防线。
//
// ⚠ 这条不是在说"timed 那条接口坏了"，而是在钉"别把两份响应喂进同一个解析函数"。
func TestParseFunaiModuleRejectsTheArrayShape(t *testing.T) {
	_, perr := ParseFunaiModule(funaiModuleEnvelope(t,
		`{"code":0,"msg":"success","result":[{"moduleId":"2-260324-1","drawTimes":null}]}`))
	if perr == nil {
		t.Fatal("数组形状不该被当成模块对象解析出来")
	}
}

// TestParseFunaiModuleBusinessFailureIsAnError：`code != 0` 交回错误（走 app 层的错误那条路）。
func TestParseFunaiModuleBusinessFailureIsAnError(t *testing.T) {
	_, perr := ParseFunaiModule(funaiModuleEnvelope(t, `{"code":1,"msg":"系统繁忙"}`))
	if perr == nil {
		t.Fatal("业务失败该报错")
	}
	if !strings.Contains(perr.Message(), "系统繁忙") {
		t.Errorf("错误里该带上游那句话，实得 %q", perr.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 抽奖响应
// ─────────────────────────────────────────────────────────────────────────────

// TestParseFunaiLotteryParsesTheCapturedDraw 用实拍那一档（顺序 11）钉字段。
func TestParseFunaiLotteryParsesTheCapturedDraw(t *testing.T) {
	got, perr := ParseFunaiLottery(funaiModuleEnvelope(t,
		`{"code":0,"msg":"success","result":{"id":3228298,"isWin":1,`+
			`"lotteryDate":"20260629","lotteryTime":"2026-06-29T20:13:47.890625379",`+
			`"marketName":"National_playAI","moduleId":"1-250730",`+
			`"prizeId":260107009,"prizeName":"30云朵","prizeType":2,`+
			`"prizeUrl":"https://img.mcloud.139.com/x.png","type":"1"}}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.PrizeName == nil || *got.PrizeName != "30云朵" {
		t.Errorf("prizeName 应是「30云朵」，实得 %v", got.PrizeName)
	}
	if got.PrizeID == nil || *got.PrizeID != 260107009 {
		t.Errorf("prizeId 应是 260107009，实得 %v", got.PrizeID)
	}
	if got.PrizeType == nil || *got.PrizeType != 2 {
		t.Errorf("prizeType 应是 2，实得 %v", got.PrizeType)
	}
	if got.IsWin == nil || *got.IsWin != 1 {
		t.Errorf("isWin 应是 1（只有这一档实拍，不据此分支），实得 %v", got.IsWin)
	}
	if got.ModuleID == nil || *got.ModuleID != "1-250730" {
		t.Errorf("moduleId 要带出去与请求对账，实得 %v", got.ModuleID)
	}
	if got.MarketName == nil || *got.MarketName != FunaiMarketName {
		t.Errorf("marketName 应是 %q，实得 %v", FunaiMarketName, got.MarketName)
	}
}

// TestParseFunaiLotteryMissingFieldsAreNilNotFailure：这条**只有一档成功样本**，
// 没有任何字段是"缺了就说响应坏了"的依据 ⇒ 全可选。
//
// 硬要求某个字段的表现是把一次真成功的抽奖报成解析错误 ——
// 而它偏偏是**写请求**，重跑等于再抽一次（与 [ParseReceiveTaskExpansion] 同一教训）。
func TestParseFunaiLotteryMissingFieldsAreNilNotFailure(t *testing.T) {
	for name, body := range map[string]string{
		"没有 result":   `{"code":0,"msg":"success"}`,
		"result 是空对象": `{"code":0,"msg":"success","result":{}}`,
		"只有奖品名":       `{"code":0,"msg":"success","result":{"prizeName":"支付宝红包"}}`,
	} {
		t.Run(name, func(t *testing.T) {
			got, perr := ParseFunaiLottery(funaiModuleEnvelope(t, body))
			if perr != nil {
				t.Fatalf("缺字段不该报错，实得 %v", perr)
			}
			if name == "只有奖品名" && (got.PrizeName == nil || *got.PrizeName != "支付宝红包") {
				t.Errorf("给了就要取到，实得 %v", got.PrizeName)
			}
		})
	}
}

// TestParseFunaiLotteryBusinessFailureIsAnError ⚠ "没有次数"那一类**尚无样本**
// （既不知道码值、也不知道 message），所以这里只钉"非 0 一律交回错误"这一条通用口径，
// 不猜具体码值。要补它：拿一个当天次数已用完的号真抽一次，把响应对拍进本文件。
func TestParseFunaiLotteryBusinessFailureIsAnError(t *testing.T) {
	_, perr := ParseFunaiLottery(funaiModuleEnvelope(t, `{"code":1,"msg":"合成失败文案"}`))
	if perr == nil {
		t.Fatal("非 0 该报错")
	}
}
