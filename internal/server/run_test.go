package server

// 入口这一层：参数面、退出码、横幅，以及"真的能起来并响应"。
//
// # 为什么这一组要打真监听
//
// 接线的价值全在「参数 → 开库 → 监听 → 路由」这条链上。把任何一段换成假的，
// 测的就只是剩下的那段 —— 而恰恰是**没测的那段**会坏，且坏的方式通常是
// "参数拼错了监听地址"或"库根本没打开"，两者都不报错。
//
// ⚠ 所以 [Deps.Serve] 是**唯一**被替换的一环，替换的是"阻塞"本身：
// 不换掉它，Main 永远不返回。

import (
	"bytes"
	"errors"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// localClient 是**不走代理**的客户端。
//
// ⚠ 本机在跑 Clash：默认 transport 会读 `HTTP_PROXY` / `ALL_PROXY`。
// Go 的 httpproxy 对回环地址本来就不走代理，但"我的断言依赖一个我不控制的
// 环境变量"这件事不该出现 —— 显式给一个空 proxy 的 transport，
// 行为就只由代码决定。
var localClient = &http.Client{
	Timeout:   5 * time.Second,
	Transport: &http.Transport{Proxy: nil},
}

func httpGet(t *testing.T, url string) string {
	t.Helper()
	resp, err := localClient.Get(url)
	if err != nil {
		t.Fatalf("GET %s 失败：%v", url, err)
	}
	defer func() { _ = resp.Body.Close() }()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("读响应体失败：%v", err)
	}
	return string(body)
}

// ─────────────────────────────────────────────────────────────────────────────
// 参数面
// ─────────────────────────────────────────────────────────────────────────────

// TestParseArgsMirrorsTheRustFlagSurface：
// 默认值与选项名逐项对齐 Rust 侧 `Args` 的 `default_value`。
//
// ⚠ 这是**边界**：脚本与用户写的是 `--db`，而默认值一旦漂了
// （比如 host 变成 `0.0.0.0`），"只监听本机"这条性质就没了，且不报错。
func TestParseArgsMirrorsTheRustFlagSurface(t *testing.T) {
	opts, err := ParseArgs(nil, io.Discard)
	if err != nil {
		t.Fatalf("空参数应能解析：%v", err)
	}
	// 前五项抄自 Rust 侧 `Args` 的 default_value。**写成字面量**而不是引用
	// [DefaultPCDeviceFile] 那类常量：这条的作用正是"默认值漂了要变红"，
	// 拿被测的常量当期望值就等于什么都没断言。
	//
	// 后两项是本仓库自己加的（冻结基准里没有）：登录要落两份设备指纹，
	// 而它们的默认值与 CLI 那两份**故意不同名** —— 漂移会让两端抢同一台"机器身份"。
	want := Options{
		DB: "data/mcloud.db", Host: "127.0.0.1", Port: 8787, WebDir: "web/dist",
		PCDeviceFile: "data/server-device.txt", AppDeviceFile: "data/server-device-app.txt",
	}
	if opts != want {
		t.Fatalf("默认值不对：\n实得 %+v\n期望 %+v", opts, want)
	}
	if opts.Addr() != "127.0.0.1:8787" {
		t.Fatalf("监听地址不对：%s", opts.Addr())
	}

	// ⚠ 双横线写法必须认：Rust 侧是 clap，用户与脚本写的是 `--db`。
	// 标准库 flag 认 `-db` 与 `--db` 两种，这里是把它钉住，免得哪天
	// 换成别的解析器时**只有单横线能用**（而那时没人会去读 --help）。
	opts, err = ParseArgs([]string{
		"--db", "/tmp/x.db", "--host", "0.0.0.0", "--port", "9000",
		"--web-dir", "/srv/dist", "--secure-cookie",
		"--device-file", "/tmp/pc.txt", "--app-device-file", "/tmp/app.txt",
	}, io.Discard)
	if err != nil {
		t.Fatalf("解析失败：%v", err)
	}
	if opts.DB != "/tmp/x.db" || opts.Host != "0.0.0.0" || opts.Port != 9000 ||
		opts.WebDir != "/srv/dist" || !opts.SecureCookie ||
		opts.PCDeviceFile != "/tmp/pc.txt" || opts.AppDeviceFile != "/tmp/app.txt" {
		t.Fatalf("覆盖后的值不对：%+v", opts)
	}

	// 等号写法也得认（clap 的 `--db=x`）。
	if _, err := ParseArgs([]string{"--db=/tmp/y.db"}, io.Discard); err != nil {
		t.Fatalf("等号写法应被接受：%v", err)
	}

	// IPv6 字面量必须加方括号 —— 拼串会拼出 `::1:8787`。
	if got := (Options{Host: "::1", Port: 8787}).Addr(); got != "[::1]:8787" {
		t.Fatalf("IPv6 监听地址不对：%s", got)
	}
}

// TestParseArgsRejectsAnOutOfRangePort：端口越界在**解析期**就报出来。
//
// 等 net.Listen 报的话，错误信息是 `listen tcp: address 99999: invalid port`——
// 那是"系统不认这个地址"，读的人第一反应是去查网络，而不是"我参数写错了"。
func TestParseArgsRejectsAnOutOfRangePort(t *testing.T) {
	for _, arg := range []string{"-1", "65536", "99999"} {
		if _, err := ParseArgs([]string{"--port", arg}, io.Discard); err == nil {
			t.Errorf("端口 %s 应被拒", arg)
		}
	}

	// ⚠ 0 是**合法**的：让内核挑一个空闲端口。测试就是靠它起在随机端口上的，
	// 顺手把它当越界挡掉会让这一整组测试只能去抢固定端口（于是偶发变红）。
	opts, err := ParseArgs([]string{"--port", "0"}, io.Discard)
	if err != nil || opts.Port != 0 {
		t.Fatalf("端口 0 应被接受（内核挑端口）：err=%v opts=%+v", err, opts)
	}
}

func TestParseArgsRejectsUnknownArguments(t *testing.T) {
	if _, err := ParseArgs([]string{"--nope"}, io.Discard); err == nil {
		t.Error("未知选项应被拒")
	}
	// 位置参数：clap 侧没有位置参数，多给一个就是用法错误。
	if _, err := ParseArgs([]string{"extra"}, io.Discard); err == nil {
		t.Error("多余的位置参数应被拒")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 退出码与两个流
// ─────────────────────────────────────────────────────────────────────────────

// TestHelpGoesToStdoutAndExitsZero：
// `--help` 打 **stdout**、退 **0**（clap 的口径）。
//
// ⚠ 标准库 flag 的默认行为是把用法打到**它自己的 output**（默认 stderr）上，
// 于是 `mcloud-server --help | less` 会拿到空 —— 而"零结果"是最危险的输出。
func TestHelpGoesToStdoutAndExitsZero(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if code := Main(nil, []string{"--help"}, &stdout, &stderr); code != ExitSuccess {
		t.Fatalf("--help 应退 %d，实得 %d（stderr=%q）", ExitSuccess, code, stderr.String())
	}
	if !strings.Contains(stdout.String(), "用法：mcloud-server [OPTIONS]") {
		t.Errorf("--help 应把用法打在 stdout：%q", stdout.String())
	}
	// 默认值也要在帮助里看得见（改了默认值而没人知道，是另一类静默漂移）。
	if !strings.Contains(stdout.String(), "127.0.0.1") {
		t.Errorf("帮助里应带上默认值：%q", stdout.String())
	}
	if stderr.Len() != 0 {
		t.Errorf("--help 不该往 stderr 写东西：%q", stderr.String())
	}
}

func TestVersionGoesToStdoutAndExitsZero(t *testing.T) {
	var stdout, stderr bytes.Buffer
	if code := Main(nil, []string{"--version"}, &stdout, &stderr); code != ExitSuccess {
		t.Fatalf("--version 应退 %d，实得 %d", ExitSuccess, code)
	}
	if got := strings.TrimSpace(stdout.String()); got != "mcloud-server "+core.Version {
		t.Fatalf("版本串不对：%q", got)
	}
}

// TestAUsageMistakeExitsTwo：用法错误退 2、且**不碰 stdout**。
//
// 与 internal/cli 同一条约定：脚本靠 1 与 2 的差判断
// "是我的调用写错了"还是"这次真的没跑成"。
func TestAUsageMistakeExitsTwo(t *testing.T) {
	var stdout, stderr bytes.Buffer
	code := Main(nil, []string{"--port", "99999"}, &stdout, &stderr)
	if code != ExitUsage {
		t.Fatalf("用法错误应退 %d，实得 %d", ExitUsage, code)
	}
	if stdout.Len() != 0 {
		t.Errorf("用法错误不该往 stdout 写：%q", stdout.String())
	}
	if !strings.Contains(stderr.String(), "错误: ") {
		t.Errorf("stderr 该带「错误: 」前缀（脚本按它 grep）：%q", stderr.String())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 端到端：真监听、真库、真请求
// ─────────────────────────────────────────────────────────────────────────────

// TestMainServesTheRealStack 一次把整条链跑通。
//
// `--port 0` 让内核挑端口，所以这条不会与任何东西抢 8787（也就不会偶发变红）；
// 横幅上印的是 listener **实际**绑上的地址，测试就从横幅里把它读出来。
func TestMainServesTheRealStack(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.db")
	webDir := webDirFixture(t)

	var stdout, stderr bytes.Buffer
	var health, index string

	deps := &Deps{
		Serve: func(listener net.Listener, handler http.Handler) error {
			srv := &http.Server{Handler: handler}
			go func() { _ = srv.Serve(listener) }()
			defer func() { _ = srv.Close() }()

			// 监听已经 bind 过了，所以此刻发请求不会丢：连接排在 backlog 里，
			// 等 Serve 开始 accept 就会处理掉 —— 不需要 sleep，也就不会 flaky。
			base := "http://" + listener.Addr().String()
			health = httpGet(t, base+"/api/health")
			index = httpGet(t, base+"/")
			return nil
		},
	}

	code := Main(deps,
		[]string{"--db", db, "--host", "127.0.0.1", "--port", "0", "--web-dir", webDir},
		&stdout, &stderr)
	if code != ExitSuccess {
		t.Fatalf("应退 %d，实得 %d（stderr=%q）", ExitSuccess, code, stderr.String())
	}

	if !strings.Contains(health, `"ok":true`) {
		t.Errorf("/api/health 应答不对：%q", health)
	}
	if !strings.Contains(index, "app-shell-marker") {
		t.Errorf("首页没发出来：%q", index)
	}
	if !strings.Contains(stdout.String(), "服务已启动") {
		t.Errorf("横幅没打出来：%q", stdout.String())
	}
	// ⚠ 横幅上的端口必须是**实际**绑上的那个。打 `--port 0` 时若照抄参数，
	// 会印出一个谁也没在听的端口 —— 而人看到"服务已启动"就信了。
	if strings.Contains(stdout.String(), "http://127.0.0.1:0") {
		t.Errorf("横幅打的是参数而不是实际端口：%q", stdout.String())
	}
	if _, err := os.Stat(db); err != nil {
		t.Errorf("库文件该被建出来：%v", err)
	}
}

// TestAFailureToOpenTheDatabaseNeverTouchesThePort：
// 库打不开时**先别碰端口**，且退 1（运行错误）。
//
// # 这条是怎么钉住"顺序"的
//
// 光断言"没调 Serve"是不够的 —— 把监听提到开库之前，`Serve` 照样不会被调，
// 断言照样绿（**反向测试实测**过）。所以这里**先把端口占住**：
//
//   - 顺序对了 ⇒ 报的是**库**的错（还没轮到监听）；
//   - 顺序反了 ⇒ 报的是**端口**的错（"端口被占用？"），而那是句误导 ——
//     人的第一反应是去查谁占了端口，不是去看库路径。
//
// 两个分支给出的**错误文案不同**，所以这条断言真的有判别力。
func TestAFailureToOpenTheDatabaseNeverTouchesThePort(t *testing.T) {
	// 让 `--db` 的父路径是一个**普通文件**：Open 建不出目录。
	parent := filepath.Join(t.TempDir(), "not-a-dir")
	if err := os.WriteFile(parent, []byte("x"), 0o644); err != nil {
		t.Fatalf("造夹具失败：%v", err)
	}
	badDB := filepath.Join(parent, "state.db")

	// 占住一个端口：127.0.0.1:0 让内核挑，拿到手就不放。
	occupied, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("占端口失败：%v", err)
	}
	defer func() { _ = occupied.Close() }()
	port := occupied.Addr().(*net.TCPAddr).Port

	var stdout, stderr bytes.Buffer
	served := false
	deps := &Deps{Serve: func(net.Listener, http.Handler) error { served = true; return nil }}

	code := Main(deps, []string{
		"--db", badDB, "--host", "127.0.0.1", "--port", strconv.Itoa(port),
	}, &stdout, &stderr)

	if code != ExitRun {
		t.Fatalf("库打不开应退 %d，实得 %d（stderr=%q）", ExitRun, code, stderr.String())
	}
	if served {
		t.Fatal("库都打不开，不该起来服务")
	}
	if !strings.Contains(stderr.String(), "打开数据库失败") {
		t.Errorf("错误信息该指出是库的问题：%q", stderr.String())
	}
	if strings.Contains(stderr.String(), "监听") {
		t.Errorf("报成了监听的错 —— 顺序反了（监听跑在开库之前）：%q", stderr.String())
	}
	if stdout.Len() != 0 {
		t.Errorf("失败路径不该打横幅（「已启动」是句谎）：%q", stdout.String())
	}
}

// TestAMissingWebDirIsAnnouncedNotFatal：
// 产物目录不存在**不是**错误 —— 开发期常态是先 `npm run dev`（vite 在 5173）。
// 但必须有一行说明，否则"页面 404"会被当成后端挂了。
func TestAMissingWebDirIsAnnouncedNotFatal(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.db")
	var stdout, stderr bytes.Buffer
	deps := &Deps{Serve: func(net.Listener, http.Handler) error { return nil }}

	code := Main(deps, []string{"--db", db, "--port", "0",
		"--web-dir", filepath.Join(t.TempDir(), "no-such-dist")}, &stdout, &stderr)
	if code != ExitSuccess {
		t.Fatalf("前端目录不存在不该是错误，实得 %d：%s", code, stderr.String())
	}
	if !strings.Contains(stdout.String(), "前端目录不存在") {
		t.Errorf("该给一行说明：%q", stdout.String())
	}
}

// TestAServeFailureIsARunError：服务本身失败（监听断了）算运行错误。
func TestAServeFailureIsARunError(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.db")
	var stdout, stderr bytes.Buffer
	deps := &Deps{Serve: func(net.Listener, http.Handler) error { return errors.New("连接断了") }}

	code := Main(deps, []string{"--db", db, "--port", "0"}, &stdout, &stderr)
	if code != ExitRun {
		t.Fatalf("服务失败应退 %d，实得 %d", ExitRun, code)
	}
	if !strings.Contains(stderr.String(), "连接断了") {
		t.Errorf("原因该被带出来：%q", stderr.String())
	}
}
