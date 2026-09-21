package cli

import (
	"bytes"
	"strings"
	"testing"

	mcloudio "github.com/catlair/mcloud/internal/io"
)

// runCLI 跑一次完整的命令行：真实的解析、真实的分发，只有三个流是假的。
//
// # 为什么走 [Main] 而不是只调被测函数
//
// 参数面这一层的**产品**就是退出码与两个流的分派。只调 runSignin 之类的内部函数，
// 测到的是"函数返回了 error"，而用户撞到的是"退 2、stderr 有一行、stdout 是空的" ——
// 后面这三件事没有任何一件在函数返回值里。
func runCLI(t *testing.T, stdin string, args ...string) (stdout, stderr string, code int) {
	t.Helper()
	var out, errOut bytes.Buffer

	deps := &Deps{
		Stdout: &out,
		Stderr: &errOut,
		Stdin:  strings.NewReader(stdin),
		Clock:  mcloudio.SystemClock{},
		Random: mcloudio.SystemRandom{},
	}
	// ⚠ 先跑、再取两个流的内容。写成 `return out.String(), errOut.String(), Main(...)`
	// 是错的：Go 按从左到右求值返回式的三个操作数，前两个会在 Main 之前取，
	// 于是每条用例都拿到空串 —— 而"空串"会让一大类断言（Contains 之外的那些）静默通过。
	code = Main(deps, args)
	return out.String(), errOut.String(), code
}

// assertExitWith 跑一条命令并核对退出码。
//
// `want == ExitRun` 时额外要求 stderr 里有"尚未迁移"：这一条把"解析器接受了"
// 与"真的走到后端了"分开钉。少了它，一个把互斥关系写反的改动只要还能退非零，
// 用例就照样绿 —— 而那正是"看起来被断言了、其实没有"。
func assertExitWith(t *testing.T, head, argv []string, want int) {
	t.Helper()
	full := append(append([]string{}, head...), argv...)
	stdout, stderr, code := runCLI(t, "", full...)
	if code != want {
		t.Errorf("%v 应退 %d，实得 %d\n  stderr=%q", full, want, code, stderr)
	}
	if want == ExitRun && !strings.Contains(stderr, "尚未迁移") {
		t.Errorf("%v 该被解析器接受并走到后端缺口，stderr=%q", full, stderr)
	}
	if want == ExitUsage && stdout != "" {
		t.Errorf("%v 用法错误不该往 stdout 写东西，实得 %q", full, stdout)
	}
}
