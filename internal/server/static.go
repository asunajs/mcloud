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
//
// # 这一层还负责压缩与缓存头（2026-09-21 补）
//
// 迁移过来的时候静态就是裸 [http.FileServer]，所以产物是**原样**发出去的：
// 实测 `web/dist` 那份 CSS 走线 80,213 字节，而它 gzip 后只有 12.7 KB
// —— 差的不是 tree shaking（产物里根本没有未用到的组件），是**没人压缩**。
// ⚠ 冻结基准那一侧实测同样不带 `Content-Encoding`（8787 上 `content-length: 80213`），
// 所以这一层是**我们主动加的**，不是"移植时漏了基准有的东西" ——
// 别把它当成对齐动作改回去，也别指望它跟基准逐字节一致。
//
// ⚠ 压缩与 `Cache-Control` 都只加在**静态分支**上，不包整个 handler。
// 理由不是"API 不需要"，而是 `/api/runs/{id}/events` 是 SSE：gzip 的块缓冲
// 会把事件攒着不放，现象是"跑任务的进度要等很久才一次性全出来"。
// 这条由 static_test.go 里"API 响应不带压缩头"钉住。

import (
	"compress/gzip"
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

		// 与 [fileExists] 同一条归一化：这样"按哪个路径判 cache 与压缩"和
		// "实际送出哪个文件"不会各判各的。
		target := path.Clean("/" + r.URL.Path)
		request, served := r, target
		if !fileExists(webDir, r.URL.Path) {
			// SPA 回退：非 `/api` 的未知路径交给首页（前端路由自己处理）。
			//
			// ⚠ 改的是**副本**：`r.URL` 是调用方（net/http）的东西，
			// 就地改它等于把这次改写泄漏给上游 —— 在真实服务器上表现为
			// 日志里记的路径变成了 `/`，而这类错误查起来毫无线索。
			//
			// ⚠ `served` 也要跟着换：判据得按**真正送出去的那个文件**，
			// 否则两头都错 —— `/assets/`（一个目录，走回退）会被按路径判成
			// "带哈希的产物"而打上一年缓存，而首页（`/` 这个路径没有扩展名）
			// 会被判成"不值得压"。
			request = r.Clone(r.Context())
			request.URL.Path = "/"
			served = "/index.html"
		}

		// 会不会按 `Accept-Encoding` 给出不同字节，就得声明 `Vary` ——
		// 不声明的话，任何一级缓存都可能把 gzip 那份发给不认它的客户端。
		// 这里**无条件**给（不压的时候也要），因为"这次没压"本身就是变体之一。
		w.Header().Set("Vary", "Accept-Encoding")
		setCacheControl(w.Header(), served)

		if !wantsGzip(r, served) {
			files.ServeHTTP(w, request)
			return
		}
		gz := &gzipResponseWriter{ResponseWriter: w}
		// 收尾必须跑：gzip 流带 CRC + 长度尾巴，不 Close 就是**截断的**，
		// 而浏览器报出来的错是"资源加载失败"，看不出跟压缩有关。
		defer gz.finish()
		files.ServeHTTP(gz, request)
	})
}

// wantsGzip 判断这一次要不要把响应压成 gzip。
//
// 三条各自挡掉一类"压了反而坏事"的情况，缺一都会留下现象很难归零的坑。
func wantsGzip(r *http.Request, served string) bool {
	// HEAD 不压：[http.ServeContent] 对 HEAD 是"只出头、body 空"，
	// 而空 body 压完会吐出一段 20 字节的空流；HEAD 要拿的正是那个 Content-Length。
	if r.Method != http.MethodGet {
		return false
	}
	// 带 Range 不压：压缩之后"第 1000–2000 字节"指的就不再是磁盘上那份的字节，
	// 而 206 的语义就是按字节切。让它走未压缩那条路，Range 保持自洽。
	if r.Header.Get("Range") != "" {
		return false
	}
	// ⚠ 判据是"提没提 gzip"，不是完整解析 `Accept-Encoding`：
	// `gzip;q=0`（宁可不压）这类写法今天没有任何浏览器会发，而漏判的代价
	// 只是多压一次。真要加解析，先想清楚谁来发那个头。
	if !strings.Contains(strings.ToLower(r.Header.Get("Accept-Encoding")), "gzip") {
		return false
	}
	return compressible(served)
}

// compressible 按**扩展名**判断值不值得压。
//
// 为什么不 sniff `Content-Type`：这里已经知道是哪个文件，而扩展名同时给出
// "该压"和"别压"两张清单。别压那半张更要紧 —— png/jpg/woff2 内部**已经**压过，
// 再套一层 gzip 往往更大且白烧 CPU，产物里的字体与图标正落在这一类。
//
// `.map` 是 sourcemap：整份产物里最大的文本，能压掉八成。
func compressible(p string) bool {
	switch strings.ToLower(path.Ext(p)) {
	case ".html", ".htm", ".css", ".js", ".mjs", ".cjs", ".json", ".map", ".svg", ".txt", ".webmanifest":
		return true
	default:
		return false
	}
}

// setCacheControl 给静态响应打缓存头。
//
// 分界线是 Vite 的产物布局：`assets/` 下的文件名**带内容哈希**
// （`index-DEttkgOQ.css`），内容一变文件名就变，所以那份可以放心缓存一年；
// 其余（`index.html`、`favicon.ico`、以及走 SPA 回退送出的首页）名字固定，
// 只能每次回源问一次。
//
// ⚠ `no-cache` 不是 `no-store`：前者允许存下来但**必须带 ETag 回源校验**
// （[http.ServeContent] 会答 304），后者等于每次全量重下。要的就是前者 ——
// 首页只有 1 KB 出头，但它是每次导航都要拿的东西。
//
// ⚠ 这条规则的前提是"`assets/` 下全是哈希名"。往 `web/public/assets/`
// 放一个固定名文件时，它会拿到一年缓存，改内容后老用户取不到新的；
// 真有那种文件就放回 `public/` 根下，别放进 `assets/`。
func setCacheControl(h http.Header, served string) {
	if strings.HasPrefix(served, "/assets/") {
		h.Set("Cache-Control", "public, max-age=31536000, immutable")
		return
	}
	h.Set("Cache-Control", "no-cache")
}

// gzipResponseWriter 把写出去的那份字节换成 gzip 流。
//
// 它存在的理由是顺序问题：[http.ServeContent] 先填好 `Content-Length`、
// `Content-Type`、`ETag`，**再**调 `WriteHeader`，最后才写 body。
// 所以改头的时机只有 `WriteHeader` 这一个口子。
type gzipResponseWriter struct {
	http.ResponseWriter
	gz   *gzip.Writer
	head bool
}

// WriteHeader 在这里决定这一次到底压不压。
//
// ⚠ 只有 200 才建压缩流：304 是 [http.ServeContent] 对条件请求的答复，
// 它不写 body。那时若建了流，[gzipResponseWriter.finish] 就会吐出一段
// 20 字节的**空** gzip 流 —— 现象是"304 带了一个 body"。
func (g *gzipResponseWriter) WriteHeader(code int) {
	if !g.head {
		g.head = true
		if code == http.StatusOK {
			g.gz = gzip.NewWriter(g.ResponseWriter)
			h := g.Header()
			h.Set("Content-Encoding", "gzip")
			// 那个长度是**未压缩**那份的，留着它等于让客户端按错的长度截断。
			h.Del("Content-Length")
			// ⚠ ETag 不动。说实话：[http.ServeContent] **根本不造** ETag
			// （它只尊重调用方预设的那个），所以回源校验走的是 `Last-Modified`，
			// 这里既没有"压完后 ETag 与字节不再对应"的问题，也没有可弱化的东西。
			// 真要在外层自己加 ETag，记得同时把它弱化（`W/`）—— 那时两个变体
			// 共用一个强 ETag 才是要防的那件事。
		}
	}
	g.ResponseWriter.WriteHeader(code)
}

func (g *gzipResponseWriter) Write(b []byte) (int, error) {
	// 没显式 WriteHeader 就 Write：net/http 会推断 200，这里跟着推断走。
	// 漏掉这一步的后果是写出压缩字节却没声明 `Content-Encoding`，
	// 浏览器把 gzip 的乱码当文本渲染，而响应码还是好看的 200。
	if !g.head {
		g.WriteHeader(http.StatusOK)
	}
	if g.gz == nil {
		return g.ResponseWriter.Write(b)
	}
	return g.gz.Write(b)
}

// finish 收尾压缩流。非 200（没建流）时什么都不做。
func (g *gzipResponseWriter) finish() {
	if g.gz != nil {
		// 错误没去处：头已经发出去了，这时才发现写不下去，
		// 能做的只有让连接断掉，而断掉的 gzip 流本来就是无效响应。
		_ = g.gz.Close()
	}
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
