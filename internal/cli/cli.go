package cli

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/spf13/cobra"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// 退出码沿用 Rust 侧的口径（`main.rs` 的 §退出码约定）：
// 只有**运行错误**才影响退出码，业务层面的 fail 属 info 级。
//
// 但"用法错误"与"运行错误"必须分开：clap 对参数不合法退 **2**，
// 而 `run()` 返回 Err 退 **1**。脚本靠这两个值的差判断
// "是我的调用写错了"还是"这次真的没跑成" —— 合成一个值就把这条信息丢了。
const (
	ExitSuccess = 0
	ExitRun     = 1
	ExitUsage   = 2
)

// ErrUsage 是给"该退 2 的错"用的标记。
//
// ⚠ 为什么要有这个哨兵而不是让 RunE 直接返回 error：cobra 把 RunE 的错误
// 与它自己解析参数的错误混在同一个返回值里，而"解析期"才是 2 的原意。
// 跨层校验（`--records-all` 需要 `--records`）发生在 RunE 里，
// 但它的性质**就是**用法错误 —— 没有这个标记，它会被算成运行错误退 1。
var ErrUsage = errors.New("用法错误")

// Deps 是一次 CLI 运行的外部世界：三个流 + 时钟 + 随机源。
//
// 为什么显式传而不是直接用 os.Stdout：`--json` 与 stderr 那套约定
// 只有在"两个流都能被测试抓住"时才被测得住。Rust 侧靠 stdout 重定向测，
// Go 侧这里把流做成字段，测试就不必去碰真实描述符。
type Deps struct {
	Stdout io.Writer
	Stderr io.Writer
	Stdin  io.Reader

	Clock  core.Clock
	Random core.Random

	// line 是 Stdin 上的**共享**读取器。必须共享：每次调用新建一个 bufio.Reader
	// 会把上一次多读进缓冲区的行丢掉 —— "读两次口令"于是变成"读同一行两次"。
	line *bufio.Reader
}

// DefaultDeps 是真实世界那一份。
func DefaultDeps() *Deps {
	return &Deps{
		Stdout: os.Stdout,
		Stderr: os.Stderr,
		Stdin:  os.Stdin,
		Clock:  mcloudio.SystemClock{},
		Random: mcloudio.SystemRandom{},
	}
}

func (d *Deps) reader() *bufio.Reader {
	if d.line == nil {
		src := d.Stdin
		if src == nil {
			src = os.Stdin
		}
		d.line = bufio.NewReader(src)
	}
	return d.line
}

// Main 解析并执行，返回进程退出码。
//
// 这里**不**调用 os.Exit：让调用方能拿到返回值，整条分发链路（含退出码本身）
// 才是可测的。cmd/mcloud/main.go 是唯一的 os.Exit 落点。
func Main(deps *Deps, args []string) int {
	if deps == nil {
		deps = DefaultDeps()
	}
	if deps.Stdout == nil {
		deps.Stdout = os.Stdout
	}
	if deps.Stderr == nil {
		deps.Stderr = os.Stderr
	}

	root := newRootCommand(deps)
	root.SetArgs(args)
	root.SetOut(deps.Stdout)
	root.SetErr(deps.Stderr)

	// dispatched 把"解析期的错"与"运行期的错"分开 —— 见上面那三个退出码的注释。
	//
	// ⚠ 为什么不是挂在 PersistentPreRunE 上（看着更省事的那个写法）：
	// cobra 的 `execute()` 顺序是 ParseFlags → preRun（含 PersistentPreRunE）
	// → **ValidateRequiredFlags / ValidateFlagGroups** → RunE。
	// 也就是说 `--list --account 1` 这种互斥违规发生在 PersistentPreRunE **之后**，
	// 用它置位就会把"用法错误"算成"运行错误"（退 1 而不是 2）——
	// 而这恰好是这批参数约束最常被撞到的那一类。
	dispatched := false
	traceDispatch(root, &dispatched)

	err := root.Execute()
	switch {
	case err == nil:
		return ExitSuccess
	case errors.Is(err, ErrUsage):
		reportError(deps.Stderr, err)
		reportUsage(deps.Stderr, root)
		return ExitUsage
	case dispatched:
		reportError(deps.Stderr, err)
		return ExitRun
	default:
		// 没进 RunE 就是 cobra/pflag 的解析错误（未知参数、枚举值不对、缺值、互斥违规……）
		reportError(deps.Stderr, err)
		reportUsage(deps.Stderr, root)
		return ExitUsage
	}
}

// traceDispatch 给树上每个带 RunE 的命令套一层"我确实开跑了"的记录。
//
// 遍历而不是逐个手写：将来加一个子命令忘了套，它的用法违规就会被算成运行错误，
// 而这种事**没有任何测试会红**。
func traceDispatch(cmd *cobra.Command, flag *bool) {
	if inner := cmd.RunE; inner != nil {
		cmd.RunE = func(c *cobra.Command, args []string) error {
			*flag = true
			return inner(c, args)
		}
	}
	for _, sub := range cmd.Commands() {
		traceDispatch(sub, flag)
	}
}

// reportUsage 只给一行紧凑用法，不给 cobra 那一大段 help。
//
// 签到那 21 个开关的完整帮助有几十行，clap 在这种情况下也只给一行
// `Usage: mcloud [OPTIONS] [COMMAND]`；把整页 help 喷到出错路径上，
// 真正的错因就被顶出屏幕了。
func reportUsage(w io.Writer, root *cobra.Command) {
	fmt.Fprintf(w, "用法：%s [OPTIONS] [COMMAND]　（完整帮助见 %s --help）\n", root.Name(), root.Name())
}

// reportError 是 `eprintln!("错误: {err:#}")` 的对位。
//
// SilenceErrors 是刻意开的：cobra 默认把错误原样打出去，而"错误:"这个前缀
// 是 Rust 侧用户熟悉的形状（脚本有人按它 grep）。前缀由这里统一加，
// 所以业务代码只负责给出**没有前缀**的正文。
func reportError(w io.Writer, err error) {
	if err == nil {
		return
	}
	fmt.Fprintf(w, "错误: %s\n", strings.TrimPrefix(err.Error(), "错误: "))
}

// newRootCommand 建出整棵命令树。
//
// # 根命令为什么也带签到参数
//
// clap 侧是 `#[command(flatten)] signin: SigninArgs`：`mcloud --list`、
// `mcloud --status-only`、`mcloud --db X` 都成立，因为"不带子命令"就是签到。
// Go 的对位写法是**同一个 register 函数调两次**（一份定义、两处注册），
// 而不是把参数抄两遍 —— 抄两遍迟早会在"某个默认值只有一边生效"上出错，
// 而那种错只在直接运行时显现，最难被发现。
func newRootCommand(deps *Deps) *cobra.Command {
	out := &output{stdout: deps.Stdout, stderr: deps.Stderr}

	rootSignin := &SigninArgs{}
	cmdSignin := &SigninArgs{}

	root := &cobra.Command{
		Use:   "mcloud",
		Short: "移动云盘自动化工具（Go 版）",
		Long: "不带子命令时直接对**全部启用账号**跑一遍签到 —— 对齐旧项目" +
			"「直接运行、一个参数都不用给」的用法。\n" +
			"顶层与 `mcloud signin` 收**同一套参数**。",
		Version:       core.Version,
		SilenceErrors: true,
		// 出错时由 [reportUsage] 给一行紧凑用法；cobra 那一大段 help 会把错因顶出屏幕。
		SilenceUsage: true,
		RunE: func(*cobra.Command, []string) error {
			return runSignin(deps, out, rootSignin)
		},
	}
	// clap 的 `--version` 打的是 `mcloud 0.1.0`（名字 + 版本），不是 Go 模板默认那句
	// "mcloud version 0.1.0"。差一个词，但 `--version | cut -d' ' -f2` 会切出不同的东西。
	root.SetVersionTemplate("mcloud {{.Version}}\n")
	// cobra 默认会塞一个 `completion` 子命令，而 Rust 侧没有（clap 的补全走
	// clap_complete，是另一个 crate、另一条构建路径）。参数面要一致，就得关掉：
	// 多出来的那个命令没人会用到，却会让"两边命令树一样"这句话变成假的。
	root.CompletionOptions.DisableDefaultCmd = true
	registerSigninFlags(root.Flags(), rootSignin)
	applySigninGroups(root, rootSignin)

	signin := &cobra.Command{
		Use:   "signin",
		Short: "签到（云朵中心）。需要先有一次 App 端登录",
		RunE: func(*cobra.Command, []string) error {
			return runSignin(deps, out, cmdSignin)
		},
	}
	registerSigninFlags(signin.Flags(), cmdSignin)
	applySigninGroups(signin, cmdSignin)

	root.AddCommand(signin)
	root.AddCommand(newLoginCommand(deps, out))
	root.AddCommand(newUserCommand(deps, out))
	root.AddCommand(newSmidCommand(deps, out))
	root.AddCommand(newMigrateCommand(deps, out))
	root.AddCommand(newFixtureCommand(deps, out))
	root.AddCommand(newCapabilitiesCommand(deps, out))
	root.AddCommand(newVersionCommand(deps, out))

	return root
}
