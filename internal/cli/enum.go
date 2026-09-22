package cli

import (
	"fmt"
	"strings"
)

// enumValue 是 clap `ValueEnum` 的 Go 对位。
//
// # 为什么要在 Set 里判而不是在 RunE 里判
//
// pflag 的解析错误发生在**进 RunE 之前**，于是 [Main] 能如实把它算成用法错误
// 退 2；放进 RunE 就变成运行错误退 1，而脚本正是靠这个差值区分
// "我的参数写错了"和"这次没跑成"。
//
// 接受的大小写与 clap 一致：ValueEnum 默认**大小写敏感**，只认变体的
// kebab-case 字面量（`Macos` → `macos`）。这里若用 EqualFold 放宽，
// `--platform macOS` 在 Rust 侧退 2、在 Go 侧退 0 —— 可接受输入集是
// 冻结的对拍面，不能被"顺手更友好"改掉。
type enumValue struct {
	allowed []string
	dest    *string
}

func newEnumFlag(dest *string, allowed ...string) *enumValue {
	return &enumValue{allowed: allowed, dest: dest}
}

func (e *enumValue) String() string {
	if e.dest == nil {
		return ""
	}
	return *e.dest
}

func (e *enumValue) Set(raw string) error {
	for _, option := range e.allowed {
		if raw == option {
			*e.dest = option
			return nil
		}
	}
	return fmt.Errorf("可选值：%s（收到 %q）", strings.Join(e.allowed, " / "), raw)
}

// Type 出现在帮助文本的占位里：`--platform PLATFORM`。
func (e *enumValue) Type() string { return strings.ToUpper(strings.Join(e.allowed, "|")) }

// 两端枚举的字面量取自 Rust 侧的 ValueEnum 变体（`Macos` → "macos"）。
var (
	platformValues = []string{"windows", "macos"}
	clientValues   = []string{"pc", "app"}
)
