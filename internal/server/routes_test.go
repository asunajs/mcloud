package server

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// TestTheRouteTableMatchesTheFrozenBaseline 逐条对位冻结基准的路由表。
//
// # 判据的来源
//
// 期望值**不是**从 Go 侧实现反推的（那样只能证明"写过的都写了"），而是逐条抄
// `../mcloud-rust/crates/mcloud-server/src/auth.rs` 与 `src/routes/*.rs` 里的 `.route(...)`
// —— 冻结基准一共 **15 条**（另有 `/api/{*rest}` 一条兜底）。
//
// # 为什么必须在裸路由表上测
//
// 从 HTTP 面**测不出来**，这一点是实测的：
//
//   - 会话闸包在整个 mux **外层** ⇒ 未登录时**所有** `/api` 路径都是 401，
//     包括不存在的那些；
//   - 兜底模式 `/api/` 不带方法 ⇒ 它把"方法不匹配"与"路径不存在"
//     **一并吞成 404**（实测全程没有 405、没有 `Allow` 头）。
//
// 于是 `GET /api/runs/1/events`（路由在、资源不在）与 `/api/nope`（路由不在）
// 在 HTTP 面上**长得一模一样**。只有问 [http.ServeMux] "这条请求命中了哪个模式"
// 才能把两者分开。
//
// # 判别器
//
// 命中的模式必须是**带方法的**（Go 写作 `"METHOD /path"`）。
// 方法不对或路径没注册时，命中的都是兜底模式 `/api/` —— 它不带方法。
//
// ⇒ 删掉任何一条路由、或把它的方法改错，这条都会红。
func TestTheRouteTableMatchesTheFrozenBaseline(t *testing.T) {
	h := newHarness(t).initialized(t)
	mux := h.server.routes()

	// 抄自冻结基准。`{id}` 在这里写成具体值 —— 模式匹配需要一条真实请求路径。
	baseline := []struct{ method, path string }{
		{"GET", "/api/health"},
		{"GET", "/api/setup"},
		{"POST", "/api/setup"},
		{"POST", "/api/session"},
		{"GET", "/api/session"},
		{"DELETE", "/api/session"},
		{"GET", "/api/version"},
		{"GET", "/api/capabilities"},
		{"GET", "/api/accounts"},
		{"GET", "/api/accounts/1/cloud-stats"},
		{"GET", "/api/runs"},
		{"GET", "/api/runs/1/events"},
		{"GET", "/api/schedules"},
		{"PUT", "/api/schedules/1"},
		{"DELETE", "/api/schedules/1"},
	}

	for _, rt := range baseline {
		req := httptest.NewRequest(rt.method, rt.path, nil)
		_, pattern := mux.Handler(req)

		// 兜底模式不带方法，所以"没有空格"就是"没命中真正的路由"。
		// 这一条同时排掉 `/api/` 与裸 `/api` 两种兜底。
		if !strings.Contains(pattern, " ") {
			t.Errorf("%s %s 没有命中带方法的路由（命中 %q）—— "+
				"冻结基准里有这一条，要么被删了，要么方法写错了",
				rt.method, rt.path, pattern)
			continue
		}
		if !strings.HasPrefix(pattern, rt.method+" ") {
			t.Errorf("%s %s 命中了 %q —— 方法对不上", rt.method, rt.path, pattern)
		}
	}

	// 对照组：不存在的路径**必须**落到兜底。
	//
	// 少了这一条，上面那圈断言就可能是"只要注册了任何贪婪模式就全过" ——
	// 而贪婪模式恰恰会让"某条路由被删掉"这件事变得看不出来。
	req := httptest.NewRequest(http.MethodGet, "/api/nope", nil)
	if _, pattern := mux.Handler(req); strings.Contains(pattern, " ") {
		t.Errorf("不存在的路径命中了带方法的路由 %q —— "+
			"注册了过于贪婪的模式，上面对位断言会因此失去判别力", pattern)
	}

	// Go 侧**新增**的写面：不在冻结基准里，所以上面那张表一条都不动。
	//
	// 单列一张表是为了让"这是谁定的形状"一眼可分辨：冻结那 15 条改了要过对拍，
	// 这几条是本项目自己定的 HTTP 面，改它只需要改前端与文档。
	// 但它们同样需要"删掉就变红"的守卫 —— 路由被误删时 HTTP 面看不出来（见上）。
	goAdded := []struct{ method, path string }{
		{"POST", "/api/accounts"},
		{"PUT", "/api/accounts/1/enabled"},
		{"DELETE", "/api/accounts/1"},
		// 2026-09-22 加：第四条账号写面（换 token）。它同样要"删掉就变红"——
		// 这条路径被误删的表现是前端点了按钮拿到 404，而 404 在界面上像"账号没了"。
		{"POST", "/api/accounts/1/refresh-auth"},
		// 触发执行。它在冻结基准里**没有**（那边 `routes/mod.rs` 自己写着"要等 P4"），
		// 所以进的是这张表：删掉它的表现是"点了开始什么也没发生（404）"，
		// 而 HTTP 面上 404 与"路径不存在"完全同形（见本用例开头的实测记录）。
		{"POST", "/api/runs"},
		// 2026-09-22 加：直播小红花页的只读探针（余额 + 那个主播的房间日程）。
		// 它是**读**路由却进这张表而不是冻结那张：形状由本项目自己定（那边基准没有）。
		// 删掉它的表现与刷新那条一样是 404，而界面上 404 会显示成"这个账号没有直播数据"——
		// 一个假结论，所以"路由在不在"这件事必须有守卫。
		{"GET", "/api/accounts/1/live-room"},
		// 2026-09-24 加：兑换页的只读选品器。同一条理由（本项目自己定的 HTTP 面，
		// 删掉表现为 404，而界面上会把 404 念成"今天没有可兑的奖品"）。
		{"GET", "/api/accounts/1/exchange-prizes"},
		{"POST", "/api/login/qr"},
		{"GET", "/api/login/qr/1"},
		{"GET", "/api/login/qr/1/image.svg"},
		{"POST", "/api/login/sms/app"},
		{"POST", "/api/login/sms/app/1/verify"},
		{"POST", "/api/login/sms/pc"},
		{"POST", "/api/login/sms/pc/1/verify"},
		// 2026-09-25 加：任务组合（预设运行列表）那四条。它们全是本项目自己定的形状
		// （冻结基准里没有），所以进这张表而不是那张。删掉任一条的表现都是 404，
		// 而界面上 404 会念成"你没有存过预设"—— 一个假结论。
		{"GET", "/api/pipelines"},
		{"PUT", "/api/pipelines/1"},
		{"DELETE", "/api/pipelines/1"},
		{"POST", "/api/pipelines/1/run"},
	}
	for _, rt := range goAdded {
		req := httptest.NewRequest(rt.method, rt.path, nil)
		_, pattern := mux.Handler(req)
		if !strings.Contains(pattern, " ") {
			t.Errorf("%s %s 没有命中带方法的路由（命中 %q）—— 这条写面被删了",
				rt.method, rt.path, pattern)
			continue
		}
		if !strings.HasPrefix(pattern, rt.method+" ") {
			t.Errorf("%s %s 命中了 %q —— 方法对不上", rt.method, rt.path, pattern)
		}
	}

	// 写面的方法必须是**收窄**的：`GET /api/accounts/1/enabled` 不该命中任何东西
	// （命中兜底 = 404，命中 PUT = 把"读一个开关"变成"改一个开关"）。
	getWrongMethod := httptest.NewRequest(http.MethodGet, "/api/accounts/1/enabled", nil)
	if _, pattern := mux.Handler(getWrongMethod); pattern != "/api/" {
		t.Errorf("GET /api/accounts/1/enabled 命中了 %q，期望兜底 %q —— "+
			"读操作会改到状态", pattern, "/api/")
	}
}
