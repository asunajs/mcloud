// Command mcloud-server 是 HTTP 产物（对应 Rust 侧的 mcloud-server）。
//
// # 现状：**只有入口还是骨架**
//
// ⚠ 别按这行注释去判断进度：`internal/server` **已经落地**（路由 / 会话 cookie /
// SSE / 领域错误到状态码的映射，**26 条测试**，用 `httptest` 起真服务器验过）。
// 缺的只是**把 main 接上去**：读配置 → 组装 `server.Deps` → 注册路由 → 挂静态资源。
// 也就是说这里退 2 不是"后端没写"，而是"没接线"。
//
// ⚠ 与 cmd/mcloud 一样，刻意不引入第三方依赖 —— 接线时也要守住这条
// （`internal/server` 用的是标准库 `net/http` + `chi`，见 docs/go-migration-plan.md §6.7）。
package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Fprintln(os.Stderr, "mcloud-server: Go 侧骨架，业务尚未迁移（见 docs/go-migration-plan.md）")
	os.Exit(2)
}
