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
//
// # 压缩与缓存头（2026-09-21 补）
//
// 后面那组 `Gzip` / `Cache` 测的是 [Server.App] 静态分支上的两层新东西。
// 它们的**失败方式各不相同**，所以一条都省不掉：
// 压坏了是"页面白屏但状态码 200"，漏了 `Vary` 是"某些客户端拿到乱码"，
// 而给首页打上一年缓存是"改版后老用户永远看到旧壳"。

import (
	"bytes"
	"compress/gzip"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
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
	// 一个**够大**的 CSS。压缩那几条要判"压完确实小了"，
	// 而 14 字节的 `app.js` 压完只会变大 —— 拿它判这件事会得到一条恒假的断言。
	css := strings.Repeat(".dock{padding-bottom:env(safe-area-inset-bottom)}", 200)
	if err := os.WriteFile(filepath.Join(dir, "assets", "index-DEttkgOQ.css"), []byte(css), 0o644); err != nil {
		t.Fatalf("写 CSS 失败: %v", err)
	}
	// 一个"已经压过"的二进制（png/woff2 就是这一类）：内容是伪随机字节，
	// 所以 gzip 压不动它 —— 这一条判的是"我们不去碰它"。
	if err := os.WriteFile(filepath.Join(dir, "assets", "logo-3f2a1b.png"), bytesFor(700), 0o644); err != nil {
		t.Fatalf("写 png 失败: %v", err)
	}
	return dir
}

// bytesFor 造 n 个**压不动**的字节（LCG，别依赖 math/rand 的全局状态）。
func bytesFor(n int) []byte {
	out := make([]byte, n)
	seed := uint32(12345)
	for i := range out {
		seed = seed*1664525 + 1013904223
		out[i] = byte(seed >> 16)
	}
	return out
}

const (
	cssAsset = "/assets/index-DEttkgOQ.css"
	pngAsset = "/assets/logo-3f2a1b.png"
)

// rawCSS 是夹具 CSS 的未压缩原文（多条断言要拿它对比）。
func rawCSS(t *testing.T) []byte {
	t.Helper()
	return []byte(strings.Repeat(".dock{padding-bottom:env(safe-area-inset-bottom)}", 200))
}

// gunzip 解压响应体，顺便判"这真的是一份 gzip 流"。
//
// ⚠ 解压后**必须比原文多判一次相等**：只判"能解压"是空断言 ——
// 一段 20 字节的空流也解得开，而那正是 304 上最容易发生的错。
func gunzip(t *testing.T, body []byte) []byte {
	t.Helper()
	r, err := gzip.NewReader(bytes.NewReader(body))
	if err != nil {
		t.Fatalf("响应体不是一份 gzip 流: %v", err)
	}
	defer r.Close()
	out, err := io.ReadAll(r)
	if err != nil {
		t.Fatalf("解压到一半断了（多半是没 Close 压缩流）: %v", err)
	}
	return out
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

// callWithHeader 带上任意请求头（`Accept-Encoding`、`Range`、`If-None-Match`）打请求。
//
// 这些头**必须**能逐条控制：压缩那组断言判的就是"客户端这么说，服务端怎么做"，
// 用一个写死 `Accept-Encoding` 的辅助函数会让"不声明就不压"那条变成恒真。
func callWithHeader(
	t *testing.T, h http.Handler, method, target string, headers map[string]string,
) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, target, nil)
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

// browserEncoding 是浏览器实际发的那个头（Chrome/Safari 今天都带 br 与 zstd）。
const browserEncoding = "gzip, deflate, br, zstd"

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

// ─────────────────────────────────────────────────────────────────────────────
// 压缩：产物要走线变小，但不能把任何一种响应压坏
// ─────────────────────────────────────────────────────────────────────────────

// TestHashedAssetsGoOutGzippedAndSmaller 是这一层的**存在理由**。
//
// 迁移过来的静态是裸 [http.FileServer]：实测那份 CSS 走线 80,213 字节，
// gzip 后 12,771。所以这一条不只判"有 gzip 头"，还判**真的变小了** ——
// 只判头的写法在"声明了却没压"（浏览器直接显示乱码）时照样绿。
// ⚠ 基准那一侧实测也不压（8787 上 `content-length: 80213`、无 `Content-Encoding`），
// 所以这一层是**对用户的改进**，不是对基准的对位 —— 静态行为本来就不在
// `routes_test.go` 那张路由表管的地方。
func TestHashedAssetsGoOutGzippedAndSmaller(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))
	raw := rawCSS(t)

	rec := callWithHeader(t, app, "GET", cssAsset, map[string]string{"Accept-Encoding": browserEncoding})
	if rec.Code != http.StatusOK {
		t.Fatalf("应 200，实得 %d", rec.Code)
	}
	if got := rec.Header().Get("Content-Encoding"); got != "gzip" {
		t.Fatalf("Content-Encoding=%q", got)
	}
	// `Vary` 漏了的话，一级缓存会把 gzip 那份发给不认它的客户端 —— 白屏。
	if got := rec.Header().Get("Vary"); !strings.Contains(strings.ToLower(got), "accept-encoding") {
		t.Fatalf("压缩过的响应必须带 Vary: Accept-Encoding，实得 %q", got)
	}
	// 压过了就不能再报未压缩的长度，否则客户端按错的长度截断。
	if got := rec.Header().Get("Content-Length"); got != "" {
		t.Fatalf("Content-Length 该被去掉（那是未压缩那份的），实得 %q", got)
	}

	got := gunzip(t, rec.Body.Bytes())
	if !bytes.Equal(got, raw) {
		t.Fatalf("解压回来与原文不一致：解压得 %d 字节，原文 %d 字节", len(got), len(raw))
	}
	if rec.Body.Len() >= len(raw)/2 {
		t.Fatalf("压了但没变小：走线 %d 字节，原文 %d 字节", rec.Body.Len(), len(raw))
	}
}

// TestClientsThatDidNotAskForGzipGetTheRawBytes：没声明 `Accept-Encoding` 就不压。
//
// 反过来才是要防的：无条件压会让"不认识 gzip 的客户端"（curl、健康检查、
// 某些代理）拿到一坨乱码，而状态码是 200，日志里什么都看不出来。
func TestClientsThatDidNotAskForGzipGetTheRawBytes(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))
	raw := rawCSS(t)

	rec := callHandler(t, app, "GET", cssAsset)
	if got := rec.Header().Get("Content-Encoding"); got != "" {
		t.Fatalf("客户端没要 gzip 却压了：Content-Encoding=%q", got)
	}
	if !bytes.Equal(rec.Body.Bytes(), raw) {
		t.Fatalf("未压缩那条路的 body 该是原文，实得 %d 字节", rec.Body.Len())
	}
	if got := rec.Header().Get("Content-Length"); got != strconv.Itoa(len(raw)) {
		t.Fatalf("Content-Length 该保持原样，实得 %q", got)
	}
	// 不压也是一次"按头而定"的应答，所以 Vary 照样要在。
	if got := rec.Header().Get("Vary"); !strings.Contains(strings.ToLower(got), "accept-encoding") {
		t.Fatalf("不压也要声明 Vary，实得 %q", got)
	}
}

// TestAlreadyCompressedFilesAreNotRegzipped：png/woff2 内部已经压过。
//
// 再套一层 gzip 常常**更大**还白烧 CPU。判据用扩展名而不是 sniff 内容，
// 所以这一条同时钉住"扩展名清单没写漏"。
func TestAlreadyCompressedFilesAreNotRegzipped(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	rec := callWithHeader(t, app, "GET", pngAsset, map[string]string{"Accept-Encoding": browserEncoding})
	if got := rec.Header().Get("Content-Encoding"); got != "" {
		t.Fatalf("二进制不该再压一层，实得 Content-Encoding=%q", got)
	}
	if rec.Body.Len() == 0 {
		t.Fatal("夹具 png 该原样送出来")
	}
}

// TestRangeRequestsStayUncompressed：带了 `Range` 就不压。
//
// 压完之后"第 0–3 字节"指的就不再是磁盘上那份的字节，而 206 的语义正是按字节切。
func TestRangeRequestsStayUncompressed(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	rec := callWithHeader(t, app, "GET", cssAsset, map[string]string{
		"Accept-Encoding": browserEncoding,
		"Range":           "bytes=0-3",
	})
	if got := rec.Header().Get("Content-Encoding"); got != "" {
		t.Fatalf("Range 请求不该压缩，实得 %q", got)
	}
	if got := rec.Body.String(); got != ".doc" {
		t.Fatalf("Range 该给出原文的前 4 字节，实得 %q", got)
	}
}

// TestHEADKeepsTheContentLength：HEAD 走未压缩那条路。
//
// HEAD 要拿的正是 `Content-Length`；而空 body 压完是一段 20 字节的空流，
// 两个错叠在一起会让人完全看不懂现象。
func TestHEADKeepsTheContentLength(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))
	raw := rawCSS(t)

	rec := callWithHeader(t, app, "HEAD", cssAsset, map[string]string{"Accept-Encoding": browserEncoding})
	if got := rec.Header().Get("Content-Encoding"); got != "" {
		t.Fatalf("HEAD 不该压缩，实得 %q", got)
	}
	if got := rec.Header().Get("Content-Length"); got != strconv.Itoa(len(raw)) {
		t.Fatalf("HEAD 的 Content-Length 该是文件真实大小，实得 %q", got)
	}
}

// TestAConditionalHitStaysABodyless304：
// 条件请求命中时**不能**因为"这次要压"而漏出一段空 gzip 流。
//
// 这是本次改动里最容易自己咬自己的一条：压缩流是在 `WriteHeader` 里建的，
// 而 304 没有 body —— 建了流就得 Close，Close 就是 20 个字节。
// 现象是"304 带了一个 body"，而它只在**第二次**访问时出现。
//
// ⚠ 用 `Last-Modified` 而不是 `If-None-Match`：[http.ServeContent] 只**尊重**
// 调用方预设的 ETag，不会自己造一个，所以这里根本没有 ETag 可用。
func TestAConditionalHitStaysABodyless304(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	first := callWithHeader(t, app, "GET", cssAsset, map[string]string{"Accept-Encoding": browserEncoding})
	modified := first.Header().Get("Last-Modified")
	if modified == "" {
		t.Fatal("夹具响应该带 Last-Modified，否则这条测试什么都没判")
	}

	second := callWithHeader(t, app, "GET", cssAsset, map[string]string{
		"Accept-Encoding":   browserEncoding,
		"If-Modified-Since": modified,
	})
	if second.Code != http.StatusNotModified {
		t.Fatalf("条件请求应 304，实得 %d：%q", second.Code, second.Body.String())
	}
	if second.Body.Len() != 0 {
		t.Fatalf("304 带了一个 %d 字节的 body —— 压缩流在不该建的时候被建了", second.Body.Len())
	}
	if got := second.Header().Get("Content-Encoding"); got != "" {
		t.Fatalf("304 没有 body，不该声明 Content-Encoding，实得 %q", got)
	}
}

// TestAPIResponsesCarryNoCompressionHeaders：压缩层只包静态，不包整个 handler。
//
// ⚠ 这条钉的是一个**很贵**的选错：把 gzip 中间件套在 [Server.Handler] 外面
// 看着更省事（一处搞定），而 `GET /api/runs/{id}/events` 是 SSE ——
// gzip 的块缓冲会把事件攒着，现象变成"跑任务的进度要等很久才一次性全出来"。
// 用 `/api/health` 判这件事，是因为它是那条最外层中间件都碰得到的最小 API 响应。
func TestAPIResponsesCarryNoCompressionHeaders(t *testing.T) {
	h := newHarness(t).initialized(t)
	app := h.server.App(webDirFixture(t))

	rec := callWithHeader(t, app, "GET", "/api/health", map[string]string{"Accept-Encoding": browserEncoding})
	if rec.Code != http.StatusOK {
		t.Fatalf("API 应照常服务，实得 %d：%s", rec.Code, rec.Body.String())
	}
	for _, hdr := range []string{"Content-Encoding", "Vary", "Cache-Control"} {
		if got := rec.Header().Get(hdr); got != "" {
			t.Errorf("%s 不该出现在 API 响应上（实得 %q）—— 静态那两层漏到了 API 那边", hdr, got)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 缓存头：分界线是"文件名带不带内容哈希"
// ─────────────────────────────────────────────────────────────────────────────

// TestTheIndexIsNeverCachedButAssetsAre：固定名的东西每次回源，带哈希的存一年。
//
// 首页那份 HTML 是**唯一指向新产物**的东西：给它长缓存等于让改版后的老用户
// 永远看不到新文件。反过来给 `assets/*` 短缓存则纯属浪费往返。
func TestTheIndexIsNeverCachedButAssetsAre(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	asset := callHandler(t, app, "GET", cssAsset)
	if got := asset.Header().Get("Cache-Control"); !strings.Contains(got, "immutable") || !strings.Contains(got, "max-age=31536000") {
		t.Fatalf("带哈希的产物该缓存一年，实得 %q", got)
	}

	for _, target := range []string{"/", "/index.html", "/runs/42"} {
		rec := callHandler(t, app, "GET", target)
		if got := rec.Header().Get("Cache-Control"); got != "no-cache" {
			t.Errorf("%s 的 Cache-Control=%q，应为 no-cache（存下来但必须回源校验）", target, got)
		}
	}
}

// TestTheAssetsDirectoryFallbackIsNotTreatedAsAnAsset：
// 缓存判据要看**真正送出去的文件**，不是看请求路径。
//
// `/assets/` 是个目录，所以走 SPA 回退送出的是首页 —— 按路径判会给它打上
// 一年 `immutable`，而首页被缓存一年正是上面那条最不想看到的事。
func TestTheAssetsDirectoryFallbackIsNotTreatedAsAnAsset(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	rec := callHandler(t, app, "GET", "/assets/")
	if !strings.Contains(rec.Body.String(), "app-shell-marker") {
		t.Fatalf("应回退到首页，实得 %d：%q", rec.Code, rec.Body.String())
	}
	if got := rec.Header().Get("Cache-Control"); got != "no-cache" {
		t.Fatalf("回退送出的首页被打上了产物缓存：%q", got)
	}
}

// TestIndexHTMLIsGzippedToo：首页也压（`.html` 在清单里）。
//
// 单独一条是因为它的**收益**不在体积（1 KB 出头），而在"每次导航都要拿"：
// 只要别把 `no-cache` 和压缩写成互斥的两层就行。
func TestIndexHTMLIsGzippedToo(t *testing.T) {
	h := newHarness(t)
	app := h.server.App(webDirFixture(t))

	rec := callWithHeader(t, app, "GET", "/", map[string]string{"Accept-Encoding": browserEncoding})
	if got := rec.Header().Get("Content-Encoding"); got != "gzip" {
		t.Fatalf("首页应压，实得 %q", got)
	}
	if body := string(gunzip(t, rec.Body.Bytes())); !strings.Contains(body, "app-shell-marker") {
		t.Fatalf("解压后不是首页：%q", body)
	}
}
