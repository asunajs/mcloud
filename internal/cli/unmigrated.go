package cli

import (
	"fmt"
	"sort"
)

// 这一步交付的是**参数面 + 分发 + 输出约定**，不是全部子命令都能跑。
//
// 把"还没迁的后端"写成一张表而不是散在注释里，理由有三条：
//  1. 用户撞到它时要知道**缺哪一层**，而不是看到"未实现"三个字；
//  2. 它让"这条线做到哪了"变成一个可断言的事实 —— 见 args_test.go 里
//     对 [UnmigratedBackends] 的核对，后端补齐时那个测试会**当场**要求更新，
//     而不是让注释烂在那里；
//  3. 反过来也挡住"顺手加一个子命令、忘了它还没有后端"。
//
// ⚠ 表里的 missing 写的是**具体缺的那一层**（含 Rust 侧的文件名），
// 不写"业务未迁移"这种到哪都查不出下一步的话。
var unmigrated = map[string]string{
	"signin": "缺多账号编排与四态渲染：Rust 侧 signin_run.rs / signin_one.rs / " +
		"signin_output.rs（约 1,264 行）。app 层的 CloudSignIn / OpenSignInSession 已在",
	"login qr": "缺 PC 端登录会话与终端二维码绘制：Rust 侧 login_cmd.rs 的 qr 分支" +
		"（upstream::login 已迁，capabilities.QRTTLSeconds 也在这边）",
	"login sms": "缺发码 → 验证码 → 落库的命令行编排与滑块人工出口：Rust 侧 login_cmd.rs 的 sms 分支",
	"smid show": "缺按账号列出 SmidState 的读取与摘要渲染：Rust 侧 smid_cmd.rs" +
		"（app.EnsureFingerprint / RotateFingerprint / ReportDeviceID 已在）",
	"smid reset":   "缺 reset 的落库与可选立即上报编排：Rust 侧 smid_cmd.rs",
	"smid refresh": "缺 refresh 的单次上报编排：Rust 侧 smid_cmd.rs",
	"fixture extract": "缺 HAR → 脱敏 fixture 的抽取器：Rust 侧 fixture_extract.rs（769 行）。" +
		"io 层只有**回放**侧（FixtureClient），脱敏黑名单那半在 core.Sanitize",
}

// UnmigratedBackends 返回"参数面已就位、后端还没迁"的子命令及各缺哪一层。
//
// 导出它是为了让文档与测试都能读同一份真相，而不是各写一遍。
func UnmigratedBackends() map[string]string {
	snapshot := make(map[string]string, len(unmigrated))
	for key, value := range unmigrated {
		snapshot[key] = value
	}
	return snapshot
}

// notMigrated 造那条错误。
//
// 这是**运行错误**（退出码 1）而不是用法错误：参数是完全正确的，
// 只是这台二进制还没带上那段实现。退 2 会让调用方以为是自己写错了命令。
func notMigrated(command string) error {
	missing, known := unmigrated[command]
	if !known {
		// 走到这里说明有人加了子命令、把 RunE 指到 notMigrated，却没往表里写。
		// 那正是要当场响的情况：表就是"还剩哪些没迁"的唯一清单。
		return fmt.Errorf("`mcloud %s` 尚未迁移（且没有登记缺哪一层）", command)
	}
	return fmt.Errorf("`mcloud %s` 的参数已就位，但后端尚未迁移：%s。"+
		"进度见 docs/go-migration-plan.md §7 步 4", command, missing)
}

// sortedUnmigratedCommands 给测试与文档用（map 遍历顺序不定，不能直接打印）。
func sortedUnmigratedCommands() []string {
	commands := make([]string, 0, len(unmigrated))
	for command := range unmigrated {
		commands = append(commands, command)
	}
	sort.Strings(commands)
	return commands
}
