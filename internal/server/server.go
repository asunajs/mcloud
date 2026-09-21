package server

// 进程入口的**外部世界**与**配置**。
//
// ⚠ 与 internal/cli 的分工完全一样：参数解析与组装住在这里（可测），
// `os.Exit` 只留在 cmd/mcloud-server 那一行。
//
// # 与 internal/cli 的 Deps 为什么长得像
//
// 不是抄形状，是同一件事：两个产物都需要"把外部世界交给调用方"才能测。
// CLI 需要换掉三个流与时钟，服务端需要换掉**阻塞**那一步。

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// 退出码沿用 internal/cli 那一套（理由见那边 cli.go 的注释）：
// 只有**运行错误**才影响退出码，"用法错误"单独占 2。
//
// ⚠ 服务端没有"业务层面的 fail"这一类 —— 它不跑业务，只接请求；
// 请求失败是 HTTP 状态码的事，与进程退出码无关。
const (
	ExitSuccess = 0
	ExitRun     = 1
	ExitUsage   = 2
)

// 参数默认值。
//
// 与 Rust 侧 `Args` 的 `default_value` 逐项一致 —— 这是**边界**
// （用户与脚本看得见），不是内部实现。
//
// ⚠ `DefaultHost` 是 `127.0.0.1` 而不是 `0.0.0.0`：这是个人工具，
// 不该一上来就暴露在局域网上。
const (
	DefaultDB     = "data/mcloud.db"
	DefaultHost   = "127.0.0.1"
	DefaultPort   = 8787
	DefaultWebDir = "web/dist"
)

// Options 是命令行解析出来的那一份配置。
type Options struct {
	// DB 是 SQLite 文件（不存在就新建并建表）。
	DB string
	// Host / Port 是监听地址。
	Host string
	Port int
	// WebDir 是前端产物目录（`cd web && npm run build` 的输出）。
	WebDir string
	// SecureCookie 给会话 cookie 打 `Secure`。
	//
	// ⚠ 默认**关**：本地是 `http://127.0.0.1`，开了它浏览器根本不回传 cookie，
	// 表现是"登录接口返回成功，但每个请求都是未登录" —— 很难往 cookie 上想。
	SecureCookie bool
}

// Addr 是监听地址。
//
// 用 [net.JoinHostPort] 而不是拼 `host + ":" + port`：IPv6 字面量必须加方括号，
// 而 `--host ::1` 是合法输入，拼串会拼出 `::1:8787` 这种连不上也看不懂的东西。
func (o Options) Addr() string { return net.JoinHostPort(o.Host, strconv.Itoa(o.Port)) }

// Deps 是 HTTP 产物持有的外部世界。
type Deps struct {
	// Clock / Random 供会话（token 与到期时间）使用。
	Clock  core.Clock
	Random core.Random

	// Serve 是"把 handler 挂上 listener 并阻塞到服务结束"这一步。
	//
	// 做成字段而不是直接调 [http.Serve]：Main 会**阻塞**，而"阻塞"在测试里的
	// 表现是**挂住** —— CI 上看起来是超时，没人会去读它为什么超时。
	// 与 internal/cli 的 `Deps.HTTP` / `Deps.Waiter` 同一条理由。
	//
	// ⚠ nil 表示"用真的那个"（[Deps.serve] 惰性构造）。
	Serve func(net.Listener, http.Handler) error

	// ⚠ 这里**没有** HTTP 客户端字段。
	//
	// 上游请求目前一个调用点都没有：跑任务的那条路（`POST /api/runs`）还没落地，
	// 而其余路由全是读库。先摆一个没人读的字段，只会让"接线的缺口"看起来像
	// 已经填上了 —— 等真有调用点再加，那时它会**必须**被用上。
}

// DefaultDeps 是真实世界那一份。
func DefaultDeps() *Deps {
	return &Deps{
		Clock:  mcloudio.SystemClock{},
		Random: mcloudio.SystemRandom{},
	}
}

func (d *Deps) clock() core.Clock {
	if d.Clock != nil {
		return d.Clock
	}
	return mcloudio.SystemClock{}
}

func (d *Deps) random() core.Random {
	if d.Random != nil {
		return d.Random
	}
	return mcloudio.SystemRandom{}
}

func (d *Deps) serve(listener net.Listener, handler http.Handler) error {
	if d.Serve != nil {
		return d.Serve(listener, handler)
	}
	return http.Serve(listener, handler)
}

// errDone 表示"该做的事已经做完了"（打印了帮助或版本），正常退出。
//
// ⚠ 不复用 [flag.ErrHelp]：`--version` 也走这条，而把它叫成"帮助"会让
// 下一个读代码的人以为版本那条路是错的。
var errDone = errors.New("已完成")

// ParseArgs 解析 `mcloud-server` 的参数。
//
// # 为什么用标准库 flag 而不是 cobra
//
// 这是一条命令 + 五个选项，框架在这里只带来一层间接。CLI 用 cobra 是因为
// 它有十来个子命令与一棵要遍历的树，两者不是同一个问题。
// ⚠ flag 接受 `--db x` / `--db=x` / `-db x`，是 clap 的**超集**，
// 所以调用方原来怎么写都不用改。
//
// # 刻意**不**抄的两处
//
//   - `--help` 的**排版**：抄 clap 的格式等于把 clap 的排版搬过来，
//     而没有任何脚本会去解析它。走 Go 的 `flag` 排版。
//   - 帮助里每条选项的**说明文字**：那是给读的人看的，不是契约。
//
// ⚠ 但"打在哪、退几"是边界，必须一致：`--help` 与 `--version` 都走
// **stdout** 且退 **0**（clap 的口径）；参数写错走 stderr 退 2。
func ParseArgs(args []string, stdout io.Writer) (Options, error) {
	opts := Options{
		DB:     DefaultDB,
		Host:   DefaultHost,
		Port:   DefaultPort,
		WebDir: DefaultWebDir,
	}

	fs := flag.NewFlagSet("mcloud-server", flag.ContinueOnError)
	// 自己排版错误：flag 默认会把帮助正文一起喷到出错路径上，
	// 而 clap 给的是「一行 error + 一行 usage」。错因被顶出屏幕是这里唯一的代价。
	fs.SetOutput(io.Discard)

	fs.StringVar(&opts.DB, "db", opts.DB, "SQLite 文件（不存在就新建并建表）")
	fs.StringVar(&opts.Host, "host", opts.Host, "监听地址。默认**只监听本机**")
	fs.IntVar(&opts.Port, "port", opts.Port, "监听端口")
	fs.StringVar(&opts.WebDir, "web-dir", opts.WebDir,
		"前端产物目录（cd web && npm run build 的输出）")
	fs.BoolVar(&opts.SecureCookie, "secure-cookie", false,
		"给会话 cookie 打 Secure。**只有放在 HTTPS 反代后面才开**")
	showVersion := fs.Bool("version", false, "打印版本后退出")

	if err := fs.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			printUsage(stdout, fs)
			return opts, errDone
		}
		return opts, err
	}
	if fs.NArg() > 0 {
		return opts, fmt.Errorf("不认识的参数：%s", strings.Join(fs.Args(), " "))
	}
	if *showVersion {
		fmt.Fprintf(stdout, "mcloud-server %s\n", core.Version)
		return opts, errDone
	}
	// 端口越界要在**解析期**报出来（退 2），而不是等 net.Listen 报一个
	// 与"你参数写错了"毫无关系的系统错误。
	//
	// ⚠ 0 是**合法**的（让内核挑一个空闲端口），别顺手把它当越界挡掉 ——
	// 测试就是靠它起在随机端口上的。
	if opts.Port < 0 || opts.Port > 65535 {
		return opts, fmt.Errorf("端口越界：%d（合法范围 0-65535）", opts.Port)
	}
	return opts, nil
}

// printUsage 打一份用法。
//
// 与 internal/cli 的 [reportUsage] 同一条取舍：只给一行紧凑用法 + 选项表，
// 不给 cobra/clap 那种带说明段落的整页帮助。
func printUsage(w io.Writer, fs *flag.FlagSet) {
	fmt.Fprintf(w, "mcloud-server —— 移动云盘助手 · WebUI 后端\n\n用法：mcloud-server [OPTIONS]\n\n")
	fs.SetOutput(w)
	fs.PrintDefaults()
}
