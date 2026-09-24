package server

// `GET /api/accounts/{id}/exchange-prizes`（只读选品器）的用例。
//
// # 这一层只钉"站在 HTTP 面上才成立"的四件
//
// 判据本身在领域层（`internal/app/exchange_plan_test.go` 与 `exchange_list_test.go`
// 逐档数过枪序与"少了余额只少那一档"），本文件不重抄那些断言。这里钉的是：
//
//  1. **顺序**：分组键按**数值**排（`2` 在 `10` 前面）。实拍那九个键是
//     `0,1,2,5,7,8,9,11,15`，而 JSON 对象没有顺序承诺 —— 前端按字典序排就会把
//     `10` 排在 `2` 前面，界面上"第 10 组"跑到"第 2 组"上头，且没人报错。
//     这一格只有出网之后才看得见，所以在 server 层钉。
//  2. **一条写操作、一次账号级读数都不发**（`infoV3` 也不发 —— 那是刻意的宽）。
//  3. **归属与免登录**：别人的 / 从未存在的 / 匿名都零请求。
//  4. **失败不降级**：上游回非 0 码时报 5xx，而不是 200 + 空清单 ——
//     后者的界面文案是"今天没有可兑的奖品"，是一句假话。
//
// 外加票池那一格：连点两次只建一条会话（AGENTS §1.12"票 = 账号 × 页面"）。

import (
	"net/http"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// exchangePrizeListBody 是两份分组清单（键**故意**写成 "10" 在前、"2" 在后，
// 这样"按数值排"这条判据不会因为我先排好而自证）。
//
// 两件各挡一档：`260630012` 是 `limit=2`（本月已兑），`260630099` 是"价格 5000 而
// 这里**没有**余额读数" ⇒ 它必须落 `go`（界面上先让人看见，余额那一挡留给下单那一步）。
const exchangePrizeListBody = `{"code":0,"msg":"success","result":{` +
	`"10":[{"prizeId":260630099,"prizeName":"贵得很","pOrder":5000,"limit":0,` +
	`"count":10,"totalCount":10,"dailyCount":5,"dailyRemainderCount":5,` +
	`"minRemainderCount":5,"onLine":1}],` +
	`"2":[{"prizeId":260630012,"prizeName":"甲","POrder":50,"limit":2,` +
	`"count":0,"totalCount":1,"dailyCount":1,"dailyRemainderCount":0,` +
	`"minRemainderCount":0,"onLine":1}]}}`

// exchangePrizeRoutes 给"建会话三枪 + 清单"。
//
// ⚠ **不**配 `infoV3` 那条路由：这份脚本对没配路由的端点回 404，
// 于是"顺手多读了一次余额"会红在 404 上而不是静默通过。
func exchangePrizeRoutes(list string) *probeScript {
	return newProbeScript(append(signInSessionRoutes(),
		probeRoute{match: "/ycloud/signin/page/exchangeList", bodies: []string{list}},
	)...)
}

func exchangePrizesGet(t *testing.T, h *harness, id string) map[string]any {
	t.Helper()
	rec := h.do(t, "GET", "/api/accounts/"+id+"/exchange-prizes", "", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("选品器应 200，实得 %d：%s", rec.Code, brief(rec.Body.String()))
	}
	return decode(t, rec)
}

func TestExchangePrizesReportsListInNumericGroupOrder(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	auth := readAuth(t, h, id)
	client := exchangePrizeRoutes(exchangePrizeListBody)
	h.replaceRunWorld(t, client)

	payload := exchangePrizesGet(t, h, id)
	if payload["accountId"] != id {
		t.Errorf("accountId 应是掩码 id %q，实得 %v", id, payload["accountId"])
	}
	groups, ok := payload["groups"].([]any)
	if !ok || len(groups) != 2 || groups[0] != "2" || groups[1] != "10" {
		t.Fatalf("groups 应按**数值**升序（2 在 10 前），实得 %+v", payload["groups"])
	}
	prizes, ok := payload["prizes"].([]any)
	if !ok || len(prizes) != 2 {
		t.Fatalf("应摊平出两件，实得 %+v", payload["prizes"])
	}
	if got := numField(payload["total"]); got != "2" {
		t.Errorf("total 应是 2，实得 %s", got)
	}

	first, _ := prizes[0].(map[string]any)
	if numField(first["prizeId"]) != "260630012" || first["groupId"] != "2" {
		t.Errorf("第一件应是第 2 组那件，实得 %+v", first)
	}
	// 两拼法都要认：这一件用的是 `POrder`（实拍里同一份响应两种拼法并存）。
	if got := numField(first["price"]); got != "50" {
		t.Errorf("price 应认得 POrder 那一拼，实得 %s", got)
	}
	if first["plan"] != "exchanged_this_prize" || first["canOrder"] != false {
		t.Errorf("limit=2 应落那一档且不可下单，实得 %v / %v", first["plan"], first["canOrder"])
	}
	if reason, _ := first["reason"].(string); !strings.Contains(reason, "本月已经兑过") {
		t.Errorf("reason 该是领域层那句原文，实得 %q", reason)
	}

	second, _ := prizes[1].(map[string]any)
	if numField(second["prizeId"]) != "260630099" || second["groupId"] != "10" {
		t.Errorf("第二件应是第 10 组那件，实得 %+v", second)
	}
	// ⭐ 刻意的宽：没有余额读数 ⇒ 价格 5000 那件仍然 `go`。
	// 改窄（多打一枪 `infoV3`）会让这一条变红，而那一枪该不该打是接口形状的决定。
	if second["plan"] != "go" || second["canOrder"] != true {
		t.Errorf("没有余额读数时不该挡（宽是刻意的），实得 %v / %v", second["plan"], second["canOrder"])
	}

	// 建会话只一趟，清单只一枪，账号级与写操作一次都不发。
	if n := client.count("/ycloud/auth-service/auth/tyrzLogin"); n != 1 {
		t.Errorf("只应建一次会话，实得 %d 趟", n)
	}
	if n := client.count("/ycloud/signin/page/exchangeList"); n != 1 {
		t.Errorf("清单应恰好读一次，实得 %d 次", n)
	}
	for _, forbidden := range []string{
		"/ycloud/signin/page/infoV3", "/ycloud/signin/page/exchangeV3",
		"auth-service/slide/getSlide", "startSignIn", "/ycloud/signin/page/receiveV3",
		"taskListV3", "/liveRoomFeedback", "/ycloud/funai",
	} {
		if n := client.count(forbidden); n != 0 {
			t.Errorf("只读选品不该打 %s，实得 %d 次（全部 URL：%v）", forbidden, n, urlsOf(client))
		}
	}

	// 云盘侧 JWT、整串 auth 与完整手机号都不许出现在体里（同一纪律见探针那条用例）。
	body, err := core.MarshalJSON(payload)
	if err != nil {
		t.Fatalf("响应体重新序列化失败：%v", err)
	}
	text := string(body)
	for _, secret := range []string{probeH5JWT, "199-sso-token", auth,
		strings.TrimPrefix(auth, "Basic "), "19900000001"} {
		if secret != "" && strings.Contains(text, secret) {
			t.Errorf("响应里带着凭据或完整手机号（%q）：%s", brief(secret), brief(text))
		}
	}
	assertNoPastedAuth(t, "选品器的响应", text, auth)
}

func TestExchangePrizesReusesThePageTicketAcrossCalls(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	client := exchangePrizeRoutes(exchangePrizeListBody)
	h.replaceRunWorld(t, client)

	for range 2 {
		exchangePrizesGet(t, h, id)
	}
	if n := client.count("/ycloud/signin/page/exchangeList"); n != 2 {
		t.Errorf("两次点按应各读一次清单，实得 %d 次", n)
	}
	// ⭐ 票属于"账号 × 页面"：第二次不该再去换一张（AGENTS §1.12）。
	if n := client.count("/ycloud/auth-service/auth/tyrzLogin"); n != 1 {
		t.Errorf("连点两次只应建一条会话，实得 %d 趟", n)
	}
}

func TestExchangePrizesNeedsAnAccountInTheCallersScope(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	admin := h.cookie
	client := exchangePrizeRoutes(exchangePrizeListBody)
	h.replaceRunWorld(t, client)

	h.cookie = secondUser(t, h, "bob")
	if rec := h.do(t, "GET", "/api/accounts/"+id+"/exchange-prizes", "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("别人的账号应 404（403 会泄露 id 存在），实得 %d：%s",
			rec.Code, brief(rec.Body.String()))
	}
	h.cookie = admin
	if rec := h.do(t, "GET", "/api/accounts/19****99/exchange-prizes", "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("从未存在的 id 应 404，实得 %d", rec.Code)
	}
	h.cookie = nil
	if rec := h.do(t, "GET", "/api/accounts/"+id+"/exchange-prizes", "", nil); rec.Code != http.StatusUnauthorized {
		t.Errorf("未登录应 401，实得 %d", rec.Code)
	}
	if client.sent() != 0 {
		t.Errorf("越权 / 不存在 / 匿名三种请求一次上游都不该打，实得 %d 次", client.sent())
	}
}

func TestExchangePrizesFailsLoudlyInsteadOfReturningAnEmptyList(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	client := exchangePrizeRoutes(`{"code":610,"msg":"合成失败"}`)
	h.replaceRunWorld(t, client)

	rec := h.do(t, "GET", "/api/accounts/"+id+"/exchange-prizes", "", nil)
	if rec.Code == http.StatusOK {
		t.Fatalf("上游回非 0 码时不该 200（那会显示成\"今天没有可兑的\"）：%s",
			brief(rec.Body.String()))
	}
	if strings.Contains(rec.Body.String(), "\"prizes\"") {
		t.Errorf("失败响应里不该带一份清单：%s", brief(rec.Body.String()))
	}
	if n := client.count("/ycloud/signin/page/exchangeList"); n != 1 {
		t.Errorf("非会话类失败不该重放，实得 %d 次", n)
	}
}

func TestExchangePrizesWithoutARunWorldAndWithoutPost(t *testing.T) {
	h := newHarness(t).initialized(t)
	id := seedProbeAccount(t, h)
	h.server.runs = nil
	if rec := h.do(t, "GET", "/api/accounts/"+id+"/exchange-prizes", "", nil); rec.Code != http.StatusInternalServerError {
		t.Errorf("缺执行面应 500（而不是 200 带空清单），实得 %d", rec.Code)
	}

	// 只读是这条接口的**契约**：挂上写方法就等于给界面开一条"看着像刷新、其实会下单"的路。
	h2 := newHarness(t).initialized(t)
	id2 := seedProbeAccount(t, h2)
	if rec := h2.do(t, "POST", "/api/accounts/"+id2+"/exchange-prizes", "", nil); rec.Code != http.StatusNotFound {
		t.Errorf("POST 落在只读路径上应 404，实得 %d", rec.Code)
	}
}
