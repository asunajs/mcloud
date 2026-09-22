// Command mcloud-server 是 HTTP 产物（对应 Rust 侧的 mcloud-server）。
//
// ```text
// go run ./cmd/mcloud-server                        # 默认 127.0.0.1:8787
// go run ./cmd/mcloud-server --db data/mcloud.db --port 9000
// ```
//
// 前端开发时用 `cd web && npm run dev`（vite 在 5173，已配好到 8787 的代理）；
// 要看生产形态就 `npm run build`，然后本进程会直接发 `web/dist`。
//
// 本文件只做两件事：把真实的流接上，把退出码交给进程。
// 参数、组装、路由全在 internal/server —— 那里必须是可测的，而 os.Exit 不是。
// 退出码约定：0 = 正常结束（含 `--help` / `--version`）；1 = 运行错误
// （库打不开、端口被占）；2 = 用法错误。
//
// ⚠ 与 cmd/mcloud 一样，刻意不引入第三方依赖 —— 接线时也要守住这条
// （`internal/server` 只用标准库 `net/http`，见 docs/archive/go-migration-crate-notes.md §6.7）。
package main

import (
	"os"

	"github.com/catlair/mcloud/internal/server"
)

func main() {
	os.Exit(server.Main(server.DefaultDeps(), os.Args[1:], os.Stdout, os.Stderr))
}
