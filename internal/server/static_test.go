package server

// 前端产物这一层：静态文件、SPA 回退、`/api` 不被吞掉。
//
// # 这一组的核心断言只有一条
//
// **静态资源不需要会话。** 它是 [Server.App] 存在的唯一理由 ——
// 把静态挂进 [Server.Handler] 的 mux 里（看着更省事的写法）会让首屏 401，
// 现象是浏览器白屏，而除了这里，**没有任何一条测试会变红**。
//
// 其余几条（SPA 回退 / 不列目录 / 目录穿越 / `/api` 不落静态）是这条的边界：
// 它们保证"为了免会话而把静态挂到外层"没有顺手把别的东西也放出去。

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// indexHTML 是夹具首页的正文。
//
// 用一个**独一无二**的串：好几条断言要判"这一份响应是不是首页"，
// 拿 `<html>` 之类的通用串去 `Contains` 会让断言在别人改了夹具之后依然绿。
const indexHTML = "<!doctype html><title>app-shell-marker</title>"

// webDirFixture 造一个像样的前端产物目录：一个首页 + 一个 assets 下的资源。
//
// ⚠ 用 t.TempDir() 而不是仓库里的 web/dist：产物目录**每次构建都会变**
// （文件名带哈希），拿它当夹具等于把断言挂在一个随时会动的东西上。
func webDirFixture(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "assets"), 0o755); err != nil {
		t.Fatalf("造 assets 目录失败: %v", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "index.html"), []byte(indexHTML), 0o644); err != nil {
		t.Fatalf("写首页失败: %v", err)
	}
	if err := os.WriteFile(filepath.Join(dir, "assets", "app.js"), []byte("console.log(1)"), 0o644); err != nil {
		t.Fatalf("写资源失败: %v", err)
	}
	return dir
}

// callHandler 直接打一个请求，返回 recorder。
//
// 与 harness.do 的差别：那个固定走 `Handler()`，这个要能换 handler ——
// 本文件测的正是"App 与 Handler 的组合"，而组合本身才是被测对象。
func callHandler(t *testing.T, h http.Handler, method, target string) *httptest.ResponseRecorder {
	t.Helper()
	return callHandlerAs(t, h, method, target, nil)
}

// callHandlerAs 带上会话 cookie。
//
// ⚠ 这一层不是可有可无的：`/api` 下的未知路径**先过会话闸**，
// 没 cookie 时拿到的是 401 —— 于是"应 404"那条断言会被悄悄变成
// "反正不是首页"。要判 404 就必须真的登录进去。
func callHandlerAs(
	t *testing.T, h http.Handler, method, target string, cookie *http.Cookie,
) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, target, nil)
	if cookie != nil {
		req.AddCookie(cookie)
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

// ─────────────────────────────────────────────────────────────────────────────
// 核心：静态资源免会话
// ─────────────────────────────────────────────────────────────────────────────

// TestStaticAssetsAreServedWithoutASession 是 [Server.App] 存在的**唯一理由**。
//
// ⚠ 夹具用的是**未初始化**的库：所以这一条同时钉住"静态资源既不过会话闸、
// 也不过初始化闸" —— 那两道闸在 [Server.Handler] 里是包在最外层的，
// 静态一旦挂进去，这里拿到的会是 401 或 503。
func TestStaticAssetsAreServedWithoutASession(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	asset := callHandler(t, app, "GET", "/assets/app.js")
	if asset.Code != http.StatusOK {
		t.Fatalf("静态资源不该要会话，实得 %d：%s", asset.Code, asset.Body.String())
	}
	if got := asset.Body.String(); got != "console.log(1)" {
		t.Fatalf("静态资源内容不对：%q", got)
	}

	// 首页同理 —— 它是"还没登录时唯一该看到的东西"。
	index := callHandler(t, app, "GET", "/")
	if index.Code != http.StatusOK || !strings.Contains(index.Body.String(), "app-shell-marker") {
		t.Fatalf("首页应免会话可读，实得 %d：%q", index.Code, index.Body.String())
	}
}

// TestUnknownFrontendPathFallsBackToTheIndex：
// 前端路由（`/runs/42` 这类）在服务端没有对应文件，必须回退到首页，
// 否则用户刷新任何一个非根路径都会看到 404。
func TestUnknownFrontendPathFallsBackToTheIndex(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	rec := callHandler(t, app, "GET", "/runs/42")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "app-shell-marker") {
		t.Fatalf("前端路由应回退到首页，实得 %d：%q", rec.Code, rec.Body.String())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 边界：免会话不等于"什么都放出去"
// ─────────────────────────────────────────────────────────────────────────────

// TestAPIPathsAreNeverAnsweredWithTheIndexPage：
// `/api` 下的未知路径必须走 API 自己的 404，**绝不能**落到首页。
//
// 否则"接口路径写错"表现成"返回了一段 HTML"：前端 `res.json()` 抛一个
// 与真正原因毫无关系的解析错误，而查日志只会看到 200。
func TestAPIPathsAreNeverAnsweredWithTheIndexPage(t *testing.T) {
	// 夹具要**初始化过**：没初始化时 `/api` 下全是 503，
	// 那样测出来的是"不是首页"，而不是"是 API 的 404" —— 少了一半判据。
	h := newHarness(t).initialized(t)
	app := h.server.App(webDirFixture(t))

	for _, target := range []string{"/api/nope", "/api/nope/deeper", "/api"} {
		rec := callHandlerAs(t, app, "GET", target, h.cookie)
		if strings.Contains(rec.Body.String(), "app-shell-marker") {
			t.Errorf("%s 拿到了首页 —— 那会让「路径写错」表现成「返回了一段 HTML」", target)
		}
		if rec.Code != http.StatusNotFound {
			t.Errorf("%s 应 404，实得 %d：%s", target, rec.Code, rec.Body.String())
		}
	}

	// 而且形状要是我们那一个（`code` 是给前端 switch 用的稳定标识）。
	body := decode(t, callHandlerAs(t, app, "GET", "/api/nope", h.cookie))
	if body["code"] != "not_found" {
		t.Fatalf("404 的 code 不对：%v", body["code"])
	}
}

// TestAMissingWebDirLeavesOnlyTheAPI：
// 产物目录不存在**不是**错误（开发期常态是先 `npm run dev`），此时只挂 API。
//
// ⚠ 非 `/api` 的路径这时会拿到 **401**，不是 404 —— 这是对的，别改成 404：
// 没有静态层时，那个请求落在 [Server.requireSession] 的默认分支上
// （"除白名单外一律要会话"），而 Rust 侧 `app()` 的 `web_dir.filter(is_dir)`
// 给出 None 时也是同一条路。多一层"反正没静态就放个 404"反而是新行为。
//
// ⚠ 夹具要**初始化过**：没初始化时 [Server.requireSetup] 在最外层，
// 拿到的会是 503 —— 那样测出来的是"没初始化"，不是"落在会话闸上"。
func TestAMissingWebDirLeavesOnlyTheAPI(t *testing.T) {
	h := newHarness(t).initialized(t)
	app := h.server.App(filepath.Join(t.TempDir(), "no-such-dist"))

	if rec := callHandler(t, app, "GET", "/api/health"); rec.Code != http.StatusOK {
		t.Fatalf("API 应照常服务，实得 %d：%s", rec.Code, rec.Body.String())
	}
	rec := callHandler(t, app, "GET", "/assets/app.js")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("没有静态层时应落在会话闸上（401），实得 %d：%s", rec.Code, rec.Body.String())
	}
}

// TestTheFallbackCannotReadOutsideTheWebDir：目录穿越。
//
// 秘密文件放在产物目录**之外**：穿越一旦成功，它就会出现在响应里。
func TestTheFallbackCannotReadOutsideTheWebDir(t *testing.T) {
	dir := webDirFixture(t)
	// 夹具布局是 <tmp>/<TestXxx>/001/，所以上一层是这条测试自己的临时目录。
	if err := os.WriteFile(filepath.Join(filepath.Dir(dir), "secret.txt"),
		[]byte("TOP-SECRET-MARKER"), 0o644); err != nil {
		t.Fatalf("造穿越目标失败: %v", err)
	}

	h := newHarness(t)
	app := h.server.App(dir)

	// ⚠ 直接构造请求，不走 http.Client：客户端会在发出去之前把 `..` 归一化，
	// 于是这条断言永远测不到服务端的那道防线。
	for _, target := range []string{"/../secret.txt", "/assets/../../secret.txt"} {
		rec := callHandler(t, app, "GET", target)
		if strings.Contains(rec.Body.String(), "TOP-SECRET-MARKER") {
			t.Fatalf("%s 读到了产物目录之外的文件 —— path.Clean 那一步没了", target)
		}
		// 折回来之后是个不存在的路径，所以应当走 SPA 回退。
		if !strings.Contains(rec.Body.String(), "app-shell-marker") {
			t.Fatalf("%s 应回退到首页，实得 %d：%q", target, rec.Code, rec.Body.String())
		}
	}
}

// TestADirectoryIsNeverListed：产物目录的清单不该白送出去。
//
// `http.FileServer` 对目录会列出内容，Rust 侧的 `ServeDir` 不列 ——
// 这里跟着不列（[fileExists] 把目录判成"不是真文件"）。
func TestADirectoryIsNeverListed(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	rec := callHandler(t, app, "GET", "/assets/")
	if strings.Contains(rec.Body.String(), "app.js") {
		t.Fatalf("产物目录被列了出来：%q", rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), "app-shell-marker") {
		t.Fatalf("应回退到首页，实得 %d：%q", rec.Code, rec.Body.String())
	}
}

// TestTheSPAFallbackDoesNotRewriteTheCallersRequest：
// 回退改的是 URL 的**副本**。
//
// 就地改 `r.URL.Path` 会把这个改写泄漏给上游 —— 在真实服务器上表现为
// access log 里所有前端路由都记成 `/`，而这类错误查起来毫无线索。
func TestTheSPAFallbackDoesNotRewriteTheCallersRequest(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	req := httptest.NewRequest("GET", "/runs/42", nil)
	app.ServeHTTP(httptest.NewRecorder(), req)
	if req.URL.Path != "/runs/42" {
		t.Fatalf("调用方的 URL 被改掉了：%q", req.URL.Path)
	}
}
