package server

// 进程入口：解析 → 开库 → 监听 → 印横幅 → 阻塞服务。
//
// ⚠ `os.Exit` **不在这里** —— 与 internal/cli 同一条理由：
// 让调用方拿到返回值，整条链路（含退出码本身）才是可测的。
// cmd/mcloud-server/main.go 是唯一的 os.Exit 落点。

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"sync"

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

	srv := New(store, deps.clock(), deps.random(),
		Config{SecureCookie: opts.SecureCookie},
		deps.loginHub(store, opts), deps.refresher(store, opts), deps.runWorld(opts),
		// 推送：WebUI 手动跑与调度到点跑**两条路**都在收尾推（同一个 Runner，见
		// [Server.launchRun]）。默认**不响**，因为 `notify_config.enabled` 的默认值是关。
		WithNotifyWorld(deps.notifyWorld(store)))

	// 横幅打的是 listener **实际**绑上的地址，不是 `--host:--port`：
	// `--port 0` 是合法的（让内核挑一个），打参数会印出一个谁也没在听的端口。
	fmt.Fprintf(stdout, "%s 服务已启动：http://%s\n", core.ProductName, listener.Addr())
	fmt.Fprintf(stdout, "  数据库：%s\n", opts.DB)
	fmt.Fprintf(stdout, "  前端：%s\n", opts.WebDir)
	if !isDir(opts.WebDir) {
		fmt.Fprintln(stdout, "  （前端目录不存在，只提供 API；开发时用 cd web && npm run dev）")
	}

	// 调度循环随进程起停（为什么搬进服务进程、以及"一份库只跑一个进程"那条
	// 约束，见 [startScheduler] 的文件头）。
	ctx, cancel := context.WithCancel(context.Background())
	var loop sync.WaitGroup
	loop.Add(1)
	go func() {
		defer loop.Done()
		// 循环那一份问题只能打在这里：一轮读库失败时连一行 `schedules` 都没读到，
		// 错误无处可写，界面上会表现成"什么都没发生"。
		srv.startScheduler(ctx, func(problem string) {
			fmt.Fprintln(stdout, "  调度："+problem)
		})
	}()

	err = deps.serve(listener, srv.App(opts.WebDir))
	cancel()
	// 等循环退出**再**返回：`defer store.Close()` 就在返回那一刻执行，不等的话
	// 最后一轮 tick 可能正要把水位写进一个已经关掉的库。
	// ⚠ 这里等的只是"判点与推水位"那一步（几十毫秒）。已经起跑的任务**不**等 ——
	// 它在 [launchRun] 那个脱离调用方上下文的 goroutine 里，与"客户端关了页面、
	// 口令照样投完"是同一条设计。
	loop.Wait()
	return err
}
