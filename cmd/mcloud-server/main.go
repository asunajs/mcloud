// Command mcloud-server 是 HTTP 产物（对应 Rust 侧的 mcloud-server）。
//
// 现状：**骨架**。路由与静态资源待迁移，见 docs/go-migration-plan.md。
//
// ⚠ 与 cmd/mcloud 一样，刻意不引入第三方依赖。
package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Fprintln(os.Stderr, "mcloud-server: Go 侧骨架，业务尚未迁移（见 docs/go-migration-plan.md）")
	os.Exit(2)
}
