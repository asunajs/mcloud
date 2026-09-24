package upstream

// 消息推送族（`msgPushOn/*`）的用例。
//
// ⚠⚠ 这一族**零实拍样本**（文件头把三方核对的结果列全了），所以这些用例
// **不能**证明我们的 URL / 方法 / 字段是对的 —— 它们钉的是另外两件事：
//
//  1. **不许悄悄漂移**。描述里那些"看着像笔误"的东西（`onDuaration` 的拼写、
//     不带端口的 host、只有一个方法的 `obtain`）必须有一个红起来才会提醒人的地方，
//     否则下一个人"顺手改对"之后，表现只是某个字段永远读不到、且**一句错都不报**。
//  2. **缺字段不判成有字段**。这一族所有判据都是指针：`nil` 与 `0` 分成两档，
//     `nil` 走"什么都不发"，`0` 才是"上游说了 0"。
//
// 真出问题时怎么定位：先按 `docs/capture-tools.md` 抓一条真实的 `task/status`，
// 与这里的常量逐字比 host / 路径 / 键名 —— 三个都可能不对，而今天哪个都不许改成"更合理"的写法。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// TestMsgPushStatusReadsTheMisspelledDurationKey 钉 [MsgPushDurationKey]。
//
// ⭐ 这是"别把拼写改对"那根钉子：**同时**给 `onDuration`（正确拼法，值 7）与
// `onDuaration`（上游的错拼，值 41），断言读出来的是 **41**。
//
// 判据出处：旧项目 `types/task.ts:10` 记着反序列化**必须按错的键名**收
// （`docs/business-feature-inventory.md` §2.3 `msg-push` 那一行末了半句）。
// 改成正确拼法的表现是"这个字段永远是 nil/0"，**不报错** —— 只有这条断言会红。
func TestMsgPushStatusReadsTheMisspelledDurationKey(t *testing.T) {
	if MsgPushDurationKey != "onDuaration" {
		t.Fatalf("键名本身被改对了：应为上游的错拼 onDuaration，实得 %q", MsgPushDurationKey)
	}
	got, perr := ParseMsgPushStatus(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success",
		"result": map[string]any{
			"pushOn": 1, "onDuration": 7, "onDuaration": 41,
			"firstTaskStatus": 3, "secondTaskStatus": 2,
		},
	}))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if got.OnDuaration == nil || *got.OnDuaration != 41 {
		t.Errorf("应读**错拼**那个键（41），实得 %v", got.OnDuaration)
	}
	// 正确拼法的那个 7 不该出现在任何字段里 —— 本结构**刻意不**为它声明字段。
	// （声明了就等于假装"上游哪天会补上正确键名"，而那是猜的。）
	if got.OnDuaration != nil && *got.OnDuaration == 7 {
		t.Error("读到的是正确拼法那个值，说明取键取错了")
	}
}

// TestMsgPushStatusFieldsAreAllOptional：四个判据全可缺。
//
// 缺 `pushOn` 不许判成"通知关了"（那会凭空造一条待办），
// 缺 `secondTaskStatus` 不许判成 0（app 层据此决定**发不发**写请求）。
func TestMsgPushStatusFieldsAreAllOptional(t *testing.T) {
	cases := []struct {
		name  string
		raw   any
		check func(t *testing.T, got MsgPushStatus)
	}{
		{"没有 result", map[string]any{"code": 0, "msg": "success"}, func(t *testing.T, got MsgPushStatus) {
			t.Helper()
			if got.PushOn != nil || got.SecondTaskStatus != nil {
				t.Errorf("上游什么都没给时四个读数都该是 nil，实得 %+v", got)
			}
		}},
		{"result 是空对象", map[string]any{"code": 0, "msg": "success", "result": map[string]any{}},
			func(t *testing.T, got MsgPushStatus) {
				t.Helper()
				if got.PushOn != nil {
					t.Errorf("缺 pushOn 该是 nil（≠ 0 = 关了），实得 %d", *got.PushOn)
				}
			}},
		{"只给 pushOn=0", map[string]any{"code": 0, "msg": "success", "result": map[string]any{
			"pushOn": 0,
		}}, func(t *testing.T, got MsgPushStatus) {
			t.Helper()
			if got.PushOn == nil || *got.PushOn != 0 {
				t.Errorf("给了 0 就要收到 0，实得 %v", got.PushOn)
			}
		}},
		{"值是字符串形态", map[string]any{"code": 0, "msg": "success", "result": map[string]any{
			"secondTaskStatus": "2",
		}}, func(t *testing.T, got MsgPushStatus) {
			t.Helper()
			// 上游这一族**没有实拍**，所以两种形态都得收：只认数字时，
			// "给了但读成没给"会让链子静默停在 Unknown 那一档。
			if got.SecondTaskStatus == nil || *got.SecondTaskStatus != 2 {
				t.Errorf(`"2" 该按整数收，实得 %v`, got.SecondTaskStatus)
			}
		}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, perr := ParseMsgPushStatus(envelopeFrom(t, c.raw))
			if perr != nil {
				t.Fatalf("缺字段不该报错，实得 %v", perr)
			}
			c.check(t, got)
		})
	}
}

// TestMsgPushStatusNonZeroCodeIsAnError：外层 `code` 非 0 仍报错误。
//
// ⚠ 与 [MsgPushObtain] 那条分开：这里判的是"这一枪打没打上"，
// 而 `obtainCode` 判的是"打上之后成没成" —— 合成一件事会让"发过但没成"消失。
func TestMsgPushStatusNonZeroCodeIsAnError(t *testing.T) {
	envelope, perr := ParseEnvelope([]byte(`{"code":90001,"msg":"未登录"}`))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if _, perr := ParseMsgPushStatus(envelope); perr == nil {
		t.Error("非 0 码应报错，不该被当成「上游没给判据」")
	}
}

// TestMsgPushObtainCodeIsSeparateFromEnvelopeCode 钉 `obtainCode` 的三种读数。
func TestMsgPushObtainCodeIsSeparateFromEnvelopeCode(t *testing.T) {
	cases := []struct {
		name string
		raw  string
		want *int64
	}{
		{"成功：obtainCode=0", `{"code":0,"msg":"success","result":{"obtainCode":0}}`, int64p(0)},
		{"非 0：obtainCode=2", `{"code":0,"msg":"success","result":{"obtainCode":2}}`, int64p(2)},
		// `code:0` 而 result 里没有 obtainCode —— 上游说"打上了"，但没说"成了"。
		{"缺 obtainCode", `{"code":0,"msg":"success","result":{}}`, nil},
		{"没有 result", `{"code":0,"msg":"success"}`, nil},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			envelope, perr := ParseEnvelope([]byte(c.raw))
			if perr != nil {
				t.Fatalf("解析失败: %v", perr)
			}
			got, perr := ParseMsgPushObtain(envelope)
			if perr != nil {
				t.Fatalf("四条都该解析成功（成败由 app 层判），实得 %v", perr)
			}
			switch {
			case c.want == nil && got.ObtainCode != nil:
				t.Errorf("应读成 nil（上游没说），实得 %d", *got.ObtainCode)
			case c.want != nil && got.ObtainCode == nil:
				t.Errorf("应读成 %d，实得 nil", *c.want)
			case c.want != nil && *got.ObtainCode != *c.want:
				t.Errorf("应读成 %d，实得 %d", *c.want, *got.ObtainCode)
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 请求形态
// ─────────────────────────────────────────────────────────────────────────────

// TestMsgPushAlignsItsHostWithTheFixtureBackedMarketPath 钉 host 选择。
//
// 本仓库**有实拍**的 `/market/…` 只有备份好礼那两条，它们走不带端口的
// [BaseCaiyun]（裸请求回 `90001`，见 [BackupGiftInfoRequest] 那段）。
// ⇒ 同前缀就跟着它对齐；[BaseCaiyunYcloud]（`:7071`）**只用过 `ycloud/*`** 路径。
//
// ⚠ 这仍是**对齐、不是证实** —— `msgPushOn` 在不在同一入口上**未验证**。
// 真跑回 404 时第一个要换的就是这一处。
func TestMsgPushAlignsItsHostWithTheFixtureBackedMarketPath(t *testing.T) {
	status := MsgPushStatusRequest("JWT-1", "https://referer")
	gift := BackupGiftInfoRequest("JWT-1", "https://referer")
	if !strings.HasPrefix(status.URL, BaseCaiyun+"/market/") {
		t.Errorf("应走不带端口的彩云 /market/，实得 %q", status.URL)
	}
	if strings.HasPrefix(status.URL, BaseCaiyunYcloud) {
		t.Errorf("不该带 :7071（那只用过 ycloud/* 路径），实得 %q", status.URL)
	}
	if strings.Split(status.URL, "/market/")[0] != strings.Split(gift.URL, "/market/")[0] {
		t.Errorf("host 段应与备份好礼逐字相同，实得 %q vs %q", status.URL, gift.URL)
	}
}

// TestMsgPushStatusRequestShape 钉 GET + 裸路径（一个查询参数都不带）。
//
// ⚠ "不带参数"是**照旧项目的调用点**写的，而那边也只是描述、**没有实拍**。
func TestMsgPushStatusRequestShape(t *testing.T) {
	request := MsgPushStatusRequest("JWT-1", "https://referer")
	if request.Method != core.MethodGet {
		t.Errorf("应是 GET，实得 %s", request.Method)
	}
	if !strings.HasSuffix(request.URL, "/market/msgPushOn/task/status") {
		t.Errorf("URL 应逐字以该串结尾，实得 %q", request.URL)
	}
	if strings.Contains(request.URL, "?") {
		t.Errorf("按描述不带查询参数，实得 %q", request.URL)
	}
	if len(request.Body) != 0 {
		t.Errorf("这条没有 body，实得 %q", request.Body)
	}
	if n := countHeader(request.Headers, "jwttoken"); n != 1 {
		t.Errorf("应带 1 个 jwttoken 头，实得 %d", n)
	}
	if countHeader(request.Headers, "isdeviceid") != 0 {
		t.Error("H5 调用点没抓到 ⇒ 按既有约定不注入 isdeviceid")
	}
}

// TestMsgPushObtainMethodIsASingleSwitch 钉住"方法只由一个常量决定、且只发一种"。
//
// ⭐ 三条断言各有其害，都要挡住：
//   - **同时发 GET 与 POST** ⇒ 向真实账号发两次同一个写请求，而上游是否幂等**零样本**；
//   - 方法散在两个构造点 ⇒ 改一处留一处，出现"status 用 GET、obtain 用 POST、
//     另一处又用 GET"那种没人能解释的组合；
//   - "顺手去掉 body" ⇒ 描述里 body 是 `{type:2}`，去掉之后上游多半什么都没领到，
//     而报错不会提示"body 空了"。
func TestMsgPushObtainMethodIsASingleSwitch(t *testing.T) {
	if msgPushObtainMethod != core.MethodGet {
		t.Errorf("缺省钉成 GET（描述未定，A/B 前先按一个走），实得 %s", msgPushObtainMethod)
	}
	request, perr := MsgPushObtainRequest("JWT-1", "https://referer")
	if perr != nil {
		t.Fatalf("构造失败: %v", perr)
	}
	if request.Method != msgPushObtainMethod {
		t.Errorf("方法应逐字取自那**一个**常量：实得 %s，常量是 %s", request.Method, msgPushObtainMethod)
	}
	if !strings.HasSuffix(request.URL, "/market/msgPushOn/task/obtain") {
		t.Errorf("URL 应逐字以该串结尾，实得 %q", request.URL)
	}
	if string(request.Body) != `{"type":2}` {
		t.Errorf("body 应逐字 {\"type\":2}，实得 %q", request.Body)
	}
	if MsgPushObtainBodyType != 2 {
		t.Errorf("body 的 type 必须与触发判据同一个 2，实得 %d", MsgPushObtainBodyType)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Errorf("content-type 只能有一个，实得 %d", n)
	}
	if n := countHeader(request.Headers, "jwttoken"); n != 1 {
		t.Errorf("应带 1 个 jwttoken 头，实得 %d", n)
	}
}
