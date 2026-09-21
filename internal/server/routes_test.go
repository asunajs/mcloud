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
// `legacy/crates/mcloud-server/src/auth.rs` 与 `src/routes/*.rs` 里的 `.route(...)`
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
}
