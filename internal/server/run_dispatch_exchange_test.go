package server

// `exchange` 那条接线的用例。
//
// 判据在领域层（`internal/app/exchange_run_test.go` 用注入点逐枪数过），本文件只钉
// **接线层独有的三件事**：
//
//  1. 分派表里那一行真的把 code 接到了兑换链上（受理 + 终局 + `runs.capability`）；
//  2. 「必填」在后端也成立 —— 空 `prizeIdOrName` 回 **400 且不落运行记录**，
//     而不是 202 之后跑出 N 条红色失败；
//  3. ⭐ 互斥键里带着**奖品与 skipWait** —— 这一格是这条能力特有的：
//     旧实现用「能力 + 账号」当锁，于是"兑 B"被"正在兑 A"挡住、或者手动触发第二条
//     时拿回第一条的记录（`docs/business-feature-inventory.md` §2.5 `exchange` 的
//     幂等缺口那一条）。这里换的是 builder，所以判据只能建在 builder 上。
//
// ⚠ 本文件**不**验"成交那一枪"：领域层之外没有解算注入点，真解算器面对合成题面必然
// 拒答 ⇒ 下单被闸住（那正是闸的行为）。链上逐枪的次数由领域层那一份测试负责。

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
)

// 兑换族在接线层的合成样本（**不是**实拍；实拍逐字在 `docs/capture/app-exchange-samples.txt`）。
// 这里的形状要求只有一个：能被上游层解析、且判据走到"可以下单"。
const (
	// 一件"此刻可兑"的商品：`limit=0`、有余量、在架、价格远低于下面那份余额。
	exchangeWiredListBody = `{"code":0,"msg":"success","result":{"0":[{` +
		`"prizeId":260630012,"prizeName":"合成奖品","groupId":0,` +
		`"pOrder":50,"limit":0,"count":100,"totalCount":200,` +
		`"dailyCount":100,"dailyRemainderCount":99,"minRemainderCount":1,"onLine":1}]}}`
	// 四个必需整数一个都不能少（少了会红在"解析失败"上，而不是判据本身）。
	exchangeWiredStatusBody = `{"code":0,"msg":"success","result":{` +
		`"cal":[],"total":10496,"signInPoints":3,"signCount":7,"monthDays":7}}`
)

// exchangeParamsOf 拼一份参数（只给这一族特有的那两栏；账号走缺省"全部启用账号"）。
func exchangeParamsOf(t *testing.T, prize string, skipWait *bool) map[string]json.RawMessage {
	t.Helper()
	raw := map[string]json.RawMessage{"prizeIdOrName": json.RawMessage(quoteJSON(prize))}
	if skipWait != nil {
		body, err := json.Marshal(*skipWait)
		if err != nil {
			t.Fatalf("编码 skipWait 失败: %v", err)
		}
		raw["skipWait"] = body
	}
	return raw
}

// buildExchangeSpec 直接跑 builder，返回那份 [runSpec]（或 builder 的错误）。
//
// 为什么要绕过 HTTP：第 3 件事（指纹）在 202 响应里**看不见** —— `ParamsDigest`
// 只进互斥键，而互斥键不出网。走 HTTP 只能验"受理了"，验不了"两趟会不会互斥"。
func buildExchangeSpec(
	t *testing.T, h *harness, raw map[string]json.RawMessage,
) (runSpec, *core.Error) {
	t.Helper()
	capability, found := capabilities.Get(capabilities.CodeExchange)
	if !found {
		t.Fatal("注册表里没有 exchange —— 接线用例无从判起")
	}
	return buildExchangeRun(context.Background(), h.server, core.ScopeUser(userIDOf(t, h)),
		capability, raw)
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. 受理与路由
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeRunIsAcceptedAndReachesTerminal：接上了就该受理并跑到终局。
//
// 与 `TestSignInFamilyIsAcceptedAndReachesTerminal` 同一口径：用默认那份"拒绝联网"的
// 客户端，要证的只是"点开始之后那条流会结束"。
// ⚠ 它**必须**带 `prizeIdOrName` —— 空着现在是 400（见下一条），不再是"受理了再失败"。
func TestExchangeRunIsAcceptedAndReachesTerminal(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)

	rec := h.do(t, "POST", "/api/runs",
		`{"capability":"exchange","params":{"prizeIdOrName":"260630012"}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("兑换应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	if len(rows) != 1 {
		t.Fatalf("应给出一个账号的配对，实得 %v", rows)
	}
	events := waitForRunFinished(t, h, firstRunID(t, rows))
	if !anyMessageContains(events, "拒绝联网") {
		t.Errorf("事件流里没有上游失败的原因：%v", messagesOf(events))
	}
	assertRunCapability(t, h, rows, "exchange")
}

// TestExchangeDispatchHitsItsOwnChain：兑换 code 打到的是兑换那两条只读接口。
//
// ⚠ 断言止于"没打下单那一枪"：这一层没有解算注入点，而 deviceId 也没接
// （上报端点没配路由 ⇒ 404 ⇒ 领域层那道"缺 deviceId 一枪不发"的闸生效）。
// 于是这一条同时钉住两件事：**路由对了**（只有 exchange 会打 `exchangeList`），
// 且**闸还在**（`exchangeV3` 零次）。
func TestExchangeDispatchHitsItsOwnChain(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedProbeAccount(t, h)
	client := newProbeScript(append(signInSessionRoutes(),
		probeRoute{match: "/ycloud/signin/page/exchangeList", bodies: []string{exchangeWiredListBody}},
		probeRoute{match: "/ycloud/signin/page/infoV3", bodies: []string{exchangeWiredStatusBody}})...,
	)
	h.replaceRunWorld(t, client)

	rec := h.do(t, "POST", "/api/runs",
		`{"capability":"exchange","params":{"prizeIdOrName":"260630012"}}`, nil)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("兑换应受理，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	rows := runsPairing(t, rec)
	waitRuns(t, h, rows)

	for _, endpoint := range []string{"/ycloud/signin/page/exchangeList",
		"/ycloud/signin/page/infoV3"} {
		if got := client.count(endpoint); got != 1 {
			t.Errorf("应把 %s 打 1 次，实得 %d 次（全部 URL：%v）",
				endpoint, got, urlsOf(client))
		}
	}
	// 下单那一枪 + 别的能力的端点：一次都不许出现。
	for _, forbidden := range []string{
		"/ycloud/signin/page/exchangeV3", "auth-service/slide/getSlide",
		"startSignIn", "/ycloud/signin/page/receiveV3", "/ycloud/signin/page/multiple",
		"openEmailsms", "/market/msgPushOn", "/ycloud/funai", "taskListV3",
		"/liveRoomFeedback",
	} {
		if got := client.count(forbidden); got != 0 {
			t.Errorf("不该打 %s，实得 %d 次（全部 URL：%v）",
				forbidden, got, urlsOf(client))
		}
	}
	assertRunCapability(t, h, rows, "exchange")
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 必填在后端也成立
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeWithoutPrizeIsRefusedBeforeLaunch：没点名兑哪一件 ⇒ **起跑之前** 400。
//
// ⭐ 反向改坏点：把这一栏交给领域层判（那里确实也判），这条就红 ——
// 而那时 202 已经发出、`runs` 表里已经多了一条记录，界面上是"一条红色失败历史"
// 而不是"你还没说要兑哪个"。
//
// 顺带钉住"空串 = 没给"：只填空格同样要 400，否则 `LocateExchangePrize` 会拿
// 一个裁完是空的串去比对全部商品名。
func TestExchangeWithoutPrizeIsRefusedBeforeLaunch(t *testing.T) {
	for _, tc := range []struct{ name, body string }{
		{"没给这一栏", `{"capability":"exchange","params":{}}`},
		{"给的是空串", `{"capability":"exchange","params":{"prizeIdOrName":""}}`},
		{"给的是一串空格", `{"capability":"exchange","params":{"prizeIdOrName":"   "}}`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			h := newHarness(t).initialized(t)
			seedLoginAccount(t, h, fullPhone)
			rec := h.do(t, "POST", "/api/runs", tc.body, nil)
			if rec.Code != http.StatusBadRequest {
				t.Fatalf("应 400，实得 %d：%s", rec.Code, brief(rec.Body.String()))
			}
			message, _ := decode(t, rec)["message"].(string)
			if message == "" {
				t.Error("400 该有一句人话")
			}
			if ids := storedRunIDs(t, h); len(ids) != 0 {
				t.Errorf("被拒的请求不该留下运行记录：%v", ids)
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. 互斥键带奖品（本条能力特有）
// ─────────────────────────────────────────────────────────────────────────────

// TestExchangeDigestIsNotEmptyAndCarriesThePrize：⭐ 指纹不能为空，且要随奖品分叉。
//
// 指纹为空时 [core 侧的互斥键]会退化成「能力 # 账号」—— 那正是旧实现的那一版锁，
// 表现是"兑 B 的那一趟被正在跑的兑 A 挡掉"。所以先钉"非空"，再钉"随输入分叉"。
func TestExchangeDigestIsNotEmptyAndCarriesThePrize(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)
	yes, no := true, false

	a := mustSpecDigest(t, h, exchangeParamsOf(t, "260630012", &yes))
	if a == "" {
		t.Fatal("指纹为空 ⇒ 锁退化成「能力#账号」，两件奖品会互相挡住")
	}
	if b := mustSpecDigest(t, h, exchangeParamsOf(t, "260630099", &yes)); a == b {
		t.Error("换了奖品指纹没变 ⇒ 兑 B 会被兑 A 挡住（旧实现的那个缺口）")
	}
	if b := mustSpecDigest(t, h, exchangeParamsOf(t, "260630012", &no)); a == b {
		t.Error("换了 skipWait 指纹没变 ⇒ 两趟打得不一样的运行共用了同一把锁")
	}
	// 名字与 id 是**不同**的串，因而指纹不同 —— 哪怕它们指向同一件商品。
	// ⚠ 这是**已知取舍**：同一件商品用两种方式点名会拿到两把锁，
	// 于是同一账号能并发跑两条下单链。要不要在起跑前把名字解析成 id 再进指纹
	// ⇒ 待用户拍板（登记在 `docs/status.md`；那需要多打一次 `exchangeList`）。
	if b := mustSpecDigest(t, h, exchangeParamsOf(t, "合成奖品", &yes)); a == b {
		t.Error("点名方式不同却压出同一份指纹（说明指纹没吃这一栏）")
	}
}

// TestExchangeDigestIgnoresSurroundingSpaces：首尾空白不该造出第二把锁。
//
// 界面上多打一个空格是常态。裁一次就够，但**两头都得裁**：builder 传给领域的
// 是裁过的那一份，指纹也用那一份 —— 两边同裁，"甲 "与"甲"才是同一趟。
func TestExchangeDigestIgnoresSurroundingSpaces(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)
	yes := true
	plain := mustSpecDigest(t, h, exchangeParamsOf(t, "260630012", &yes))
	padded := mustSpecDigest(t, h, exchangeParamsOf(t, "  260630012  ", &yes))
	if plain != padded {
		t.Errorf("同一件商品因空格拿到两把锁：%s vs %s", plain, padded)
	}
}

// TestExchangeDefaultSkipsWait：缺省时"不等整点"，且缺省的来源是**声明**不是字面量。
//
// 两层各判一件事：
//   - 声明里 `skipWait` 的默认值必须是 `true`（这一族刻意保守：等整点不是上游硬条件，
//     而我们没有理由让一趟运行在没人看着的时候挂一个小时）；
//   - **不给**这一栏时 builder 压出的指纹，与**显式给声明那份默认值**时一致 ——
//     不一致就说明有一处自己写了个 `true`，将来改声明不会跟着改（[runParams.take]
//     的缺省来源正是声明，这条钉的就是这条）。
//
// ⚠ 这一条**不**直接判"领域层有没有真的不等"：那要观察等待，而默认那份等待用的是
// 真实计时器 —— 接反了的表现是上面两条受理用例**卡到 10 秒超时**（`waitForRunFinished`
// 那句"等不到终局事件"），不是一条干净的红灯。看到了那句超时，先查这里。
func TestExchangeDefaultSkipsWait(t *testing.T) {
	h := newHarness(t).initialized(t)
	seedLoginAccount(t, h, fullPhone)
	capability, found := capabilities.Get(capabilities.CodeExchange)
	if !found {
		t.Fatal("注册表里没有 exchange")
	}
	var declared json.RawMessage
	for _, field := range capability.Params() {
		if field.Key == "skipWait" {
			declared = field.Default
		}
	}
	if string(declared) != "true" {
		t.Fatalf("声明里 skipWait 的默认值应是 true，实得 %s", declared)
	}
	yes := true
	if omitted, explicit := mustSpecDigest(t, h, exchangeParamsOf(t, "260630012", nil)),
		mustSpecDigest(t, h, exchangeParamsOf(t, "260630012", &yes)); omitted != explicit {
		t.Errorf("省略 skipWait 与显式给声明默认值压出两份指纹（%s / %s）⇒ 有一处没走声明的缺省",
			omitted, explicit)
	}
}

// mustSpecDigest 跑 builder 并取出那份参数指纹。
func mustSpecDigest(t *testing.T, h *harness, raw map[string]json.RawMessage) string {
	t.Helper()
	spec, perr := buildExchangeSpec(t, h, raw)
	if perr != nil {
		t.Fatalf("builder 失败: %v", perr)
	}
	return spec.plan.ParamsDigest
}
