// Command mcloud 是命令行产物（对应 Rust 侧的 mcloud-cli）。
//
// 退出码约定（见 internal/cli 的 Main）：0 = 跑完了；1 = 运行错误；
// 2 = 用法错误。业务层面的 fail 属 info 级，**不**影响退出码。
//
// 本文件只做两件事：把真实的流接上，把退出码交给进程。
// 分发与参数全在 internal/cli —— 那里必须是可测的，而 os.Exit 不是。
package main

import (
	"os"

	"github.com/catlair/mcloud/internal/cli"
)

func main() {
	os.Exit(cli.Main(cli.DefaultDeps(), os.Args[1:]))
}
