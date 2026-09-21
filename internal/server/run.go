package server

// 进程入口：解析 → 开库 → 监听 → 印横幅 → 阻塞服务。
//
// ⚠ `os.Exit` **不在这里** —— 与 internal/cli 同一条理由：
// 让调用方拿到返回值，整条链路（含退出码本身）才是可测的。
// cmd/mcloud-server/main.go 是唯一的 os.Exit 落点。

import (
	"errors"
	"fmt"
	"io"
	"net"
	"os"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// Main 解析并执行，返回进程退出码。
func Main(deps *Deps, args []string, stdout, stderr io.Writer) int {
	if deps == nil {
		deps = DefaultDeps()
	}
	if stdout == nil {
		stdout = os.Stdout
	}
	if stderr == nil {
		stderr = os.Stderr
	}

	opts, err := ParseArgs(args, stdout)
	switch {
	case errors.Is(err, errDone):
		// 帮助 / 版本已经打过了，正常退出。
		return ExitSuccess
	case err != nil:
		fmt.Fprintf(stderr, "错误: %s\n", err)
		fmt.Fprintf(stderr, "用法：mcloud-server [OPTIONS]　（完整帮助见 mcloud-server --help）\n")
		return ExitUsage
	}

	if err := Run(deps, opts, stdout); err != nil {
		fmt.Fprintf(stderr, "错误: %s\n", err)
		return ExitRun
	}
	return ExitSuccess
}

// Run 打开库、起监听、印横幅，然后阻塞到服务结束。
//
// # 顺序：先开库，再监听
//
// 与 Rust 侧一致，而这个顺序**不是**随手排的：反过来的话，库打不开时端口
// 已经被占用过一下，而"端口被占用"是最容易被误读的失败 ——
// 人的第一反应是去查谁占了端口，不是去看库路径。
// ⇒ 这条顺序由 run_test.go 里"库打不开时不许碰端口"那条断言钉住。
func Run(deps *Deps, opts Options, stdout io.Writer) error {
	if deps == nil {
		deps = DefaultDeps()
	}
	if stdout == nil {
		stdout = os.Stdout
	}

	store, err := mcloudio.Open(opts.DB)
	if err != nil {
		return fmt.Errorf("打开数据库失败：%s：%w", opts.DB, err)
	}
	defer func() { _ = store.Close() }()

	listener, err := net.Listen("tcp", opts.Addr())
	if err != nil {
		return fmt.Errorf("监听 %s 失败（端口被占用？）：%w", opts.Addr(), err)
	}
	defer func() { _ = listener.Close() }()

	srv := New(store, deps.clock(), deps.random(), Config{SecureCookie: opts.SecureCookie})

	// 横幅打的是 listener **实际**绑上的地址，不是 `--host:--port`：
	// `--port 0` 是合法的（让内核挑一个），打参数会印出一个谁也没在听的端口。
	fmt.Fprintf(stdout, "%s 服务已启动：http://%s\n", core.ProductName, listener.Addr())
	fmt.Fprintf(stdout, "  数据库：%s\n", opts.DB)
	fmt.Fprintf(stdout, "  前端：%s\n", opts.WebDir)
	if !isDir(opts.WebDir) {
		fmt.Fprintln(stdout, "  （前端目录不存在，只提供 API；开发时用 cd web && npm run dev）")
	}

	return deps.serve(listener, srv.App(opts.WebDir))
}
