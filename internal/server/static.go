package server

// 前端产物：静态文件 + SPA 回退。
//
// # 为什么必须在**会话中间件的外层**
//
// [Server.requireSession] 的默认是"除了白名单一律要会话"，而静态资源
// （`/index.html`、`/assets/*.js`）**在用户登录之前就得能拿到** ——
// 挂进同一个 mux 会让首屏直接 401，而现象是"页面白屏"，没人会想到 cookie。
//
// Rust 侧靠 `router.fallback_service(...)` 挂在中间件外解决；这里同一条思路，
// 但 Go 的 `ServeMux` 没有 fallback 这个概念，所以改成**先按路径分流**：
// `/api` 走 API（含它自己的 404），其余走静态。
//
// ⚠ 这个顺序是**承重**的，不是风格问题 —— 见 static_test.go 里那条
// "静态资源不需要会话"的断言，它是 [Server.App] 存在的唯一理由。

import (
	"net/http"
	"os"
	"path"
	"path/filepath"
	"strings"
)

// App 在 API 路由之上挂前端产物。
//
// ⚠ `webDir` 不存在时**只挂 API**，而不是报错：开发期常见的情况是先
// `cd web && npm run dev`（vite 在 5173，已配好到 8787 的代理），
// 这时后端没有 `dist` 也该能起来。横幅里会多一行说明（见 [Run]）。
func (s *Server) App(webDir string) http.Handler {
	api := s.Handler()
	if !isDir(webDir) {
		return api
	}
	files := http.FileServer(http.Dir(webDir))

	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if isAPIPath(r.URL.Path) {
			// `/api` 下不存在的路径由 [Server.apiNotFound] 明确报 404，
			// 绝不能落到 index.html —— 那会让"路径写错"表现成"返回了一段 HTML"，
			// 前端解析失败且毫无线索。
			api.ServeHTTP(w, r)
			return
		}
		if fileExists(webDir, r.URL.Path) {
			files.ServeHTTP(w, r)
			return
		}
		// SPA 回退：非 `/api` 的未知路径交给 index.html（前端路由自己处理）。
		//
		// ⚠ 改的是**副本**：`r.URL` 是调用方（net/http）的东西，
		// 就地改它等于把这次改写泄漏给上游 —— 在真实服务器上表现为
		// 日志里记的路径变成了 `/`，而这类错误查起来毫无线索。
		index := r.Clone(r.Context())
		index.URL.Path = "/"
		files.ServeHTTP(w, index)
	})
}

// isAPIPath 判断一个路径是否属于 API。
//
// ⚠ `/api` 本身也要算：Go 的 `ServeMux` 对 `"/api/"` 这个子树模式会给
// `/api` 一个 301 到 `/api/`。落到静态那边的话，请求 `/api` 会拿到首页 HTML，
// 而那是这里最不该发生的一件事。
func isAPIPath(p string) bool {
	return p == "/api" || strings.HasPrefix(p, "/api/")
}

func isDir(p string) bool {
	info, err := os.Stat(p)
	return err == nil && info.IsDir()
}

// fileExists 判断 `urlPath` 在 `webDir` 下是不是一个**真文件**。
//
// 目录不算：`http.FileServer` 对目录会列出内容，而 Rust 侧的 `ServeDir`
// 不列 —— 列出来等于把产物目录的清单白送出去。不算真文件就走 SPA 回退。
//
// # 目录穿越靠哪一步挡住的
//
// 靠 [path.Clean] 把 `..` 折掉：`/../secret.txt` → `/secret.txt`。
// 前导的那个 `/` 是让结果**锚在根上** —— 没有它，`Clean("../x")` 会原样
// 留着 `..`，而 [filepath.Join] 是会跟着 `..` 往上走的。
//
// ⚠ 说实话：HTTP 的路径**总是**以 `/` 开头，所以单靠 `path.Clean(urlPath)`
// 也能挡住今天的所有输入。那个 `"/" +` 是**不依赖调用方**的那一手 ——
// 它让"传进来的到底是不是绝对路径"不再重要，而这个函数将来可能被
// 非 HTTP 的调用方用到（那时没人会记得这条）。
// 与 [http.Dir] 内部的做法一致：不是另发明一套，是**同一条**。
func fileExists(webDir, urlPath string) bool {
	rel := path.Clean("/" + urlPath)
	info, err := os.Stat(filepath.Join(webDir, filepath.FromSlash(rel)))
	return err == nil && !info.IsDir()
}
