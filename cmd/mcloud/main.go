// Command mcloud 是命令行产物（对应 Rust 侧的 mcloud-cli）。
//
// 现状：**骨架**。业务实现待迁移，见 docs/go-migration-plan.md。
//
// ⚠ 本文件刻意不引入任何第三方依赖（cobra 等），好让 `go build ./...`
// 在零下载的情况下就能跑通 —— 骨架的第一条退出判据是"能编译、门禁能跑"，
// 不是"功能可用"。引入依赖是迁移的第一步实现动作，不是骨架的事。
package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Fprintln(os.Stderr, "mcloud: Go 侧骨架，业务尚未迁移（见 docs/go-migration-plan.md）")
	os.Exit(2)
}
