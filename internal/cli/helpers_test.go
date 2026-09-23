package cli

import (
	"bytes"
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// refusingClient 对任何请求都直接报错。
//
// # 为什么必须有它
//
// `login qr` / `login sms` 接上后端之后，参数面的用例就**真的会打上游**了。
// 而"打上游"在测试里的失败形态不是变红，是**卡住**：先等 DNS、再等超时，
// 而扫码那条循环一轮询就是两分钟 —— 于是整包测试表现为 `panic: test timed out`，
// 且没有任何一行指出是哪条命令。实际踩到过一次（20 秒超时只跑到第 6 条用例）。
//
// 所以 [runCLI] 默认装这个客户端：**这个包里的测试一律不许联网**。
// 需要脚本化上游的用例自己传一个进去（见 run_login_test.go 的 [loginScript]）。
type refusingClient struct{}

var errRefused = core.NewInternal("测试客户端拒绝联网：这条命令不该在测试里打上游")

func (refusingClient) Send(context.Context, core.HTTPRequest) (core.HTTPResponse, error) {
	return core.HTTPResponse{}, errRefused
}

// instantWaiter 是"立刻返回"的等待器，并记下被要求等了几次。
//
// 用它而不是 [app.RealWaiter]：轮询那条循环一旦真睡，
// 一条"轮询到成功"的用例就要等好几个真实间隔，而**挂住**与**红**在 CI 里是两回事。
type instantWaiter struct{ calls int }

func (w *instantWaiter) Wait(context.Context, time.Duration) *core.Error {
	w.calls++
	return nil
}

// 编译期钉住它确实实现了 [app.Waiter]：接口改签名时这里先红，
// 而不是等到某条用例忽然开始真睡才被发现。
var _ app.Waiter = (*instantWaiter)(nil)

// TestRunCLINeverDialsUpstream 钉住 [runCLI] 那两道闸还在。
//
// `login qr` 是这个包里唯一真打上游的叶子，所以它是最合适的探针：
// 探针必须打在"最可能被联网"的那条路上，否则守卫被拆掉时它会一直绿。
func TestRunCLINeverDialsUpstream(t *testing.T) {
	dir := t.TempDir()
	_, stderr, code := runCLI(t, "", "login", "qr",
		"--db", dir+"/state.db", "--device-file", dir+"/device.txt")
	if code != ExitRun {
		t.Fatalf("拒绝联网应表现为运行错误（退 %d），实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "拒绝联网") {
		t.Errorf("应是被测试客户端挡下的，而不是连到了真实上游: %q", stderr)
	}
}

// runCLI 跑一次完整的命令行：真实的解析、真实的分发，只有三个流是假的。
//
// # 为什么走 [Main] 而不是只调被测函数
//
// 参数面这一层的**产品**就是退出码与两个流的分派。只调 runSignin 之类的内部函数，
// 测到的是"函数返回了 error"，而用户撞到的是"退 2、stderr 有一行、stdout 是空的" ——
// 后面这三件事没有任何一件在函数返回值里。
//
// ⚠ 两个"不许真的发生"的外部副作用在这里被掐掉：联网（[refusingClient]）与
// 真睡（[instantWaiter]）。缺了它们，参数面用例会在后端补齐的那天集体变成
// 联网 + 休眠的慢用例 —— 而那一天它不会报错，只是变慢，然后偶尔超时。
func runCLI(t *testing.T, stdin string, args ...string) (stdout, stderr string, code int) {
	t.Helper()
	return runCLIWithDeps(t, &Deps{
		Clock:  mcloudio.SystemClock{},
		Random: mcloudio.SystemRandom{},
	}, stdin, args...)
}

// runCLIWithDeps 跑一次完整命令行，外部世界由调用方给（[Deps] 里没设的字段在这里补齐）。
//
// 它是 [runCLI] 与 run_login_test.go 那条 `runLoginCLI` 的共同底座 ——
// 两处各自拼一遍 Deps 的话，"其中一边忘了装等待器"这种差别不会报错，
// 只会让那半边用例忽然开始真睡。
func runCLIWithDeps(
	t *testing.T, deps *Deps, stdin string, args ...string,
) (stdout, stderr string, code int) {
	t.Helper()
	var out, errOut bytes.Buffer
	deps.Stdout = &out
	deps.Stderr = &errOut
	deps.Stdin = strings.NewReader(stdin)
	// 两道闸：不联网、不真睡。调用方**显式**给过就尊重它
	// （脚本化上游的用例自己传 [core.HTTPClient] 进来）。
	if deps.HTTP == nil {
		deps.HTTP = refusingClient{}
	}
	if deps.Waiter == nil {
		deps.Waiter = &instantWaiter{}
	}
	// ⚠ 先跑、再取两个流的内容。写成 `return out.String(), errOut.String(), Main(...)`
	// 是错的：Go 按从左到右求值返回式的三个操作数，前两个会在 Main 之前取，
	// 于是每条用例都拿到空串 —— 而"空串"会让一大类断言（Contains 之外的那些）静默通过。
	code = Main(deps, args)
	return out.String(), errOut.String(), code
}

// assertAccepted 跑一条**参数合法**的命令，判据是"它没被判成用法错（退 2）"。
//
// ⚠⚠ **只在"这条命令真的会跑起来、且它跑起来之后不可能因为别的理由退 2"时才用它**
// （2026-09-22 补）。它用的代理是"退出码不是 2"，而退出码**只有三档**、
// 表达不了"是**哪一层**拦下的"：`--account <越界>` 与互斥违规**同码不同层**
// （前者是 RunE 里手工判的用法错误，后者是 cobra 在 RunE **之前**拦的）。
// ⇒ 只要用例里出现"会被 RunE 判成用法错"的输入，这个代理就不再可靠 ——
// 那时要钉的是**层**，用 [assertParsesCleanly]。
//
// ⚠ 它**故意**不断言退出码等于某个具体值。这条命令现在真的会跑后端，
// 跑起来成不成功取决于库里的状态（空库退 0），而"参数合法"不等于"这次跑成了"。
//
// 它取代了早先的 `assertExitWith(..., ExitRun)` —— 那个版本还要求 stderr 里有
// "尚未迁移"，而全部叶子落地之后这句话不会再出现了。
func assertAccepted(t *testing.T, head, argv []string) {
	t.Helper()
	full := append(append([]string{}, head...), argv...)
	_, stderr, code := runCLI(t, "", full...)
	if code == ExitUsage {
		t.Errorf("%v 参数合法，却被判成用法错误：stderr=%q", full, stderr)
	}
}

// assertParsesCleanly 只验**解析期**：pflag 解析 + cobra 的 flag-group 校验，**不进 RunE**。
//
// ⚠ 为什么需要它（2026-09-22）：互斥表那张用例原来的判据是"退出码不是 2"，而它
// 把两个层**混成了一个码** —— `--account 3`（空库、越界）从这一版起**也是**退 2
// （用法错误：重试不会变好，只能改调用），于是"不是 2"不再等于"解析器放行了它"。
// ⚠⚠ 这不是把断言放松，而是把它**对准它一直声称的那一层**：用例的判据原文就是
// "解析器放行了它"，而退出码表达不了"哪一层拦下的" ⇒ **要钉层就直接调那一层**。
//
// ⚠ 它**不**覆盖"命令能跑起来"这件事 —— 需要那个的用例继续用 [assertAccepted]。
func assertParsesCleanly(t *testing.T, head, argv []string) {
	t.Helper()

	root := buildRoot(t)
	cmd := root
	if len(head) > 0 {
		sub := findCommand(root, head[0])
		if sub == nil {
			t.Fatalf("命令树里没有 %q 这个子命令 —— 用例会静默变成空跑", head[0])
		}
		cmd = sub
	}

	// ⚠ 两处都要：`ParseFlags` 只跑 pflag，互斥/成组校验是 `ValidateFlagGroups`
	// 做的，而 cobra 是在 `execute()` 里、ParseFlags **之后**才调它。
	if err := cmd.ParseFlags(argv); err != nil {
		t.Errorf("%v 在解析期就该放行，实得 %v", argv, err)
		return
	}
	if err := cmd.ValidateFlagGroups(); err != nil {
		t.Errorf("%v 不该被判成互斥违规，实得 %v", argv, err)
	}
}

// assertUsageRejected 跑一条**参数非法**的命令：退 2，且 stdout 干净。
func assertUsageRejected(t *testing.T, head, argv []string) {
	t.Helper()
	full := append(append([]string{}, head...), argv...)
	stdout, stderr, code := runCLI(t, "", full...)
	if code != ExitUsage {
		t.Errorf("%v 应退 %d，实得 %d\n  stderr=%q", full, ExitUsage, code, stderr)
	}
	if stdout != "" {
		t.Errorf("%v 用法错误不该往 stdout 写东西，实得 %q", full, stdout)
	}
}
