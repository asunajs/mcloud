package cli

import (
	"io"
	"strconv"
	"strings"
	"testing"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"

	"github.com/catlair/mcloud/internal/capabilities"
)

// buildRoot 是测试用的命令树入口（不走 [Main]，好直接读 flag 定义）。
func buildRoot(t *testing.T) *cobra.Command {
	t.Helper()
	return newRootCommand(&Deps{Stdout: io.Discard, Stderr: io.Discard})
}

func findCommand(root *cobra.Command, name string) *cobra.Command {
	for _, sub := range root.Commands() {
		if sub.Name() == name {
			return sub
		}
	}
	return nil
}

func defaultOf(t *testing.T, fs *pflag.FlagSet, name string) string {
	t.Helper()
	flag := fs.Lookup(name)
	if flag == nil {
		t.Fatalf("没有 --%s 这个开关", name)
	}
	return flag.DefValue
}

// TestSigninParameterSurfaceIsDefinedOnce 钉住"顶层与 signin 是同一份定义"。
//
// 对位 Rust 的 `#[command(flatten)]` + `top_level_and_subcommand_spellings_agree`。
// 那三条 assert_eq 在 clap 里是天然的（同一个结构体），Go 这边是**两次注册**，
// 所以"抄了两份、其中一份的默认值漂了"这件事在 Go 侧是可能发生的 ——
// 而它的表现恰好是最难发现的那种：只在直接运行时某个默认值没生效。
// 于是这里比的不是三个字段，而是**每一个开关的名字 / 默认值 / 帮助文本 / 类型**。
func TestSigninParameterSurfaceIsDefinedOnce(t *testing.T) {
	root := buildRoot(t)
	signin := findCommand(root, "signin")
	if signin == nil {
		t.Fatal("命令树里没有 signin 子命令")
	}

	rootFlags := root.Flags()
	if got := countSigninLike(rootFlags); got < 20 {
		t.Fatalf("签到那套参数至少该有 20 个开关，顶层只数到 %d", got)
	}

	rootFlags.VisitAll(func(upper *pflag.Flag) {
		if !isSigninFlag(upper.Name) {
			return
		}
		lower := signin.Flags().Lookup(upper.Name)
		if lower == nil {
			t.Errorf("--%s 在 signin 上不存在（顶层有、子命令没有 = 两份定义漂了）", upper.Name)
			return
		}
		if upper.DefValue != lower.DefValue {
			t.Errorf("--%s 默认值不一致：顶层 %q vs signin %q", upper.Name, upper.DefValue, lower.DefValue)
		}
		if upper.Usage != lower.Usage {
			t.Errorf("--%s 帮助文本不一致：顶层 %q vs signin %q", upper.Name, upper.Usage, lower.Usage)
		}
		if upper.Value.Type() != lower.Value.Type() {
			t.Errorf("--%s 类型不一致：%s vs %s", upper.Name, upper.Value.Type(), lower.Value.Type())
		}
	})
}

// isSigninFlag 把根命令上属于签到那套的开关挑出来（根还带 cobra 自己的 --help）。
func isSigninFlag(name string) bool {
	switch name {
	case "help", "version":
		return false
	}
	return true
}

func countSigninLike(fs *pflag.FlagSet) int {
	count := 0
	fs.VisitAll(func(flag *pflag.Flag) {
		if isSigninFlag(flag.Name) {
			count++
		}
	})
	return count
}

// TestSigninDefaultsAreTheRustOnes：同一条命令在两边必须指向同一个库文件，
// 否则用户在 Rust/Go 之间切换时会"看见不同的账号清单"。
func TestSigninDefaultsAreTheRustOnes(t *testing.T) {
	root := buildRoot(t)
	flags := root.Flags()

	for _, want := range []struct{ name, def string }{
		{"db", "data/mcloud.db"},
		{"app-device-file", "data/device-app.txt"},
		{"device-short", ""},
		{"device-id", ""},
		{"cloud-id", ""},
		{"account", "0"},
		{"cloud-type", "0"},
		// 四个写开关默认必须关 —— 尤其 backup-gift / multiple 是**每月一次**的名额。
		{"receive", "false"},
		{"revive", "false"},
		{"multiple", "false"},
		{"backup-gift", "false"},
		{"force", "false"},
		{"json", "false"},
	} {
		if got := defaultOf(t, flags, want.name); got != want.def {
			t.Errorf("--%s 默认值应为 %q，实得 %q", want.name, want.def, got)
		}
	}
}

// TestH5DeviceIDIsAnAliasOfDeviceID：clap 侧是 `visible_alias`。
//
// ⚠ 这条断言的形状：别名与本体**绑同一个变量**，所以设别名、读本体。
// 用 `--device-id` 读是读不到的（那会变成"两个各存一份"也照样绿）。
func TestH5DeviceIDIsAnAliasOfDeviceID(t *testing.T) {
	root := buildRoot(t)
	if err := root.Flags().Set("h5-device-id", "B-alias-value"); err != nil {
		t.Fatalf("--h5-device-id 应当可用：%v", err)
	}
	if got := root.Flags().Lookup("device-id").Value.String(); got != "B-alias-value" {
		t.Fatalf("别名没写进本体那个变量，实得 %q", got)
	}
}

// TestExclusivePairsMirrorTheRustTable 逐字搬 `write_and_read_switches_are_mutually_exclusive`
// 与 `signin_list_conflicts_with_account_and_phone` 的用例。
//
// 为什么两边写法都要跑：Rust 那条测试的理由就是"顶层能用、子命令不能用"
// 这种不一致会留一条口子。Go 侧两次注册让这个风险**更高**，所以照抄并加倍。
func TestExclusivePairsMirrorTheRustTable(t *testing.T) {
	accepted := [][]string{
		{"--receive", "--multiple"},
		{"--receive", "--backup-gift"},
		{"--multiple", "--backup-gift"},
		{"--receive", "--revive", "--multiple", "--backup-gift"},
		{"--list"},
		{"--status-only"},
		{"--json"},
		{"--db", "/tmp/other.db"},
		{"--account", "3", "--status-only", "--json"},
	}
	rejected := [][]string{
		{"--prizes", "--receive"},
		{"--prizes", "--backup-gift"},
		{"--prizes", "--raw"},
		{"--prizes", "--records"},
		{"--raw", "--backup-gift"},
		{"--records", "--backup-gift"},
		{"--status-only", "--backup-gift"},
		{"--status-only", "--receive"},
		{"--status-only", "--revive"},
		{"--status-only", "--multiple"},
		{"--raw", "--receive"},
		{"--raw", "--revive"},
		{"--raw", "--multiple"},
		{"--raw", "--prizes"},
		{"--records", "--receive"},
		{"--records", "--revive"},
		{"--records", "--multiple"},
		{"--records", "--raw"},
		{"--records", "--prizes"},
		{"--list", "--account", "1"},
		{"--list", "--phone", "19900000001"},
	}

	heads := [][]string{{}, {"signin"}}
	for _, head := range heads {
		for _, argv := range accepted {
			// 被接受的那批会走到"后端未迁移"，所以期望退 1（运行错误）而不是 2。
			assertExitWith(t, head, argv, ExitRun)
		}
		for _, argv := range rejected {
			assertExitWith(t, head, argv, ExitUsage)
		}
	}
}

// TestRecordsAllRequiresRecords：clap 的 `requires` 是**单向**的，
// cobra 没有对应原语（RequiredTogether 会把只给 --records 也判错），
// 所以这条是 RunE 里手工判的 —— 手工判的东西必须有用例，否则迟早退化。
func TestRecordsAllRequiresRecords(t *testing.T) {
	cases := []struct {
		argv []string
		want int
	}{
		{[]string{"--records-all"}, ExitUsage},
		{[]string{"--records-all", "--records"}, ExitRun},
		{[]string{"--records"}, ExitRun},
	}
	for _, tc := range cases {
		stdout, stderr, code := runCLI(t, "", tc.argv...)
		if code != tc.want {
			t.Errorf("%v 应退 %d，实得 %d（stderr=%q）", tc.argv, tc.want, code, stderr)
		}
		if stdout != "" {
			t.Errorf("%v 不该往 stdout 写东西，实得 %q", tc.argv, stdout)
		}
	}
}

// TestLoginParameterSurface：枚举值、别名、必填与 send-only/code 互斥。
func TestLoginParameterSurface(t *testing.T) {
	for _, tc := range []struct {
		name string
		argv []string
		want int
	}{
		{"qr 默认平台", []string{"login", "qr"}, ExitRun},
		{"qr --platform macos", []string{"login", "qr", "--platform", "macos"}, ExitRun},
		{"qr 平台名大小写都认", []string{"login", "qr", "--platform", "macOS"}, ExitRun},
		{"qr 不认的平台是用法错误", []string{"login", "qr", "--platform", "linux"}, ExitUsage},
		{"sms 缺 --phone", []string{"login", "sms"}, ExitUsage},
		{"sms --phone", []string{"login", "sms", "--phone", "19900000001"}, ExitRun},
		{"sms --client app", []string{"login", "sms", "--phone", "19900000001", "--client", "app"}, ExitRun},
		{"sms --client 未知值", []string{"login", "sms", "--phone", "19900000001", "--client", "ios"}, ExitUsage},
		{"sms --send-only 与 --code 互斥", []string{
			"login", "sms", "--phone", "19900000001", "--send-only", "--code", "123456"}, ExitUsage},
		{"sms 默认 client 是 pc", []string{"login", "sms", "--phone", "19900000001"}, ExitRun},
	} {
		_, stderr, code := runCLI(t, "", tc.argv...)
		if code != tc.want {
			t.Errorf("%s：%v 应退 %d，实得 %d（stderr=%q）", tc.name, tc.argv, tc.want, code, stderr)
		}
	}
}

// TestCommandTreeMatchesTheDocumentedSurface 钉住整棵命令树。
//
// 判据来自 docs/cli-usage.md 的子命令一览表。为什么用"叶子路径"而不是只数命令个数：
// 只数个数的话，`login sms` 换成 `login message` 照样绿 —— 而那是用户会撞到的改动。
func TestCommandTreeMatchesTheDocumentedSurface(t *testing.T) {
	root := buildRoot(t)
	want := []string{
		"capabilities",
		"fixture extract",
		"login qr",
		"login sms",
		"migrate",
		"signin",
		"smid refresh",
		"smid reset",
		"smid show",
		"user init",
		"user list",
		"version",
	}
	got := leafPaths(root)

	if strings.Join(got, "|") != strings.Join(want, "|") {
		t.Errorf("命令树与文档不一致\n  期望: %v\n  实得: %v", want, got)
	}
}

// TestCompletionSubcommandStaysOff：cobra 默认会加 `completion`，clap 侧没有。
//
// ⚠ 这条**不能**用 [leafPaths] 走：cobra 是在 `ExecuteC` 里才补上那个命令的
// （InitDefaultCompletionCmd），只 build 不跑的树上看不到它 —— 反向测试确认过：
// 把 DisableDefaultCmd 那行删掉，命令树那两条用例照样绿。
// 所以这里走完整的一次运行，判据是"用户敲 `mcloud completion bash` 得到未知命令"。
func TestCompletionSubcommandStaysOff(t *testing.T) {
	_, stderr, code := runCLI(t, "", "completion", "bash")
	if code != ExitUsage {
		t.Fatalf("`completion` 不该存在（Rust 侧没有这条），实得退 %d", code)
	}
	if !strings.Contains(stderr, "completion") {
		t.Errorf("错误信息该点名 completion，实得 %q", stderr)
	}
}

// TestNoLeafIsSilentlyUnimplemented：每个叶子要么真的能跑，要么在缺口表里登记了。
//
// 这条是 [notMigrated] 那张表的对价：加了子命令忘了登记，它当场红。
// 反过来也管住"后端补上了却忘了把表里那行删掉" —— 那时它会以另一种方式红。
func TestNoLeafIsSilentlyUnimplemented(t *testing.T) {
	implemented := map[string]bool{
		"capabilities": true,
		"version":      true,
		"user init":    true,
		"user list":    true,
		"migrate":      true,
	}
	gaps := UnmigratedBackends()

	for _, path := range leafPaths(buildRoot(t)) {
		_, listed := gaps[path]
		switch {
		case implemented[path] && listed:
			t.Errorf("`mcloud %s` 既能跑又还挂在缺口表里 —— 该删那一行了", path)
		case !implemented[path] && !listed:
			t.Errorf("`mcloud %s` 既没实现也没登记进缺口表 —— 见 unmigrated.go", path)
		}
	}
}

// TestUnmigratedEntriesNameTheMissingLayer：表里每一行都要指出**缺哪一层**。
//
// 断言"含 .rs"是为了挡住"未实现"这种到哪都查不出下一步的写法。
func TestUnmigratedEntriesNameTheMissingLayer(t *testing.T) {
	entries := UnmigratedBackends()
	if len(entries) == 0 {
		t.Fatal("缺口表是空的：那意味着全部子命令都已迁移，这张表该整个删掉")
	}
	for _, command := range sortedUnmigratedCommands() {
		missing := entries[command]
		if !strings.Contains(missing, ".rs") {
			t.Errorf("`mcloud %s` 的缺口说明没点出缺哪一层：%q", command, missing)
		}
	}
}

// leafPaths 把命令树摊平成 "login qr" 这样的叶子路径。
//
// 路径用 string 拼接而不是往共享切片里 append：后者会让兄弟命令共用底层数组，
// 产出 `login qr qr` 这种只在测试里出现、却把断言整个废掉的东西。
func leafPaths(root *cobra.Command) []string {
	var paths []string
	var walk func(cmd *cobra.Command, path string)
	walk = func(cmd *cobra.Command, path string) {
		children := cmd.Commands()
		if len(children) == 0 {
			// 根命令本身（不带子命令 = 签到）不是一条"路径"，它由 signin 那行覆盖。
			if cmd.Runnable() && path != "" {
				paths = append(paths, path)
			}
			return
		}
		for _, sub := range children {
			if sub.Name() == "help" {
				continue
			}
			child := sub.Name()
			if path != "" {
				child = path + " " + child
			}
			walk(sub, child)
		}
	}
	walk(root, "")
	return paths
}

// TestQrTTLDefaultComesFromTheRegistryNotFromASecondLiteral：
// `--ttl-seconds` 的默认值必须取自能力注册表，**不能在 CLI 再写一个 120**。
//
// ⭐ 反向测试先暴露了这里是个缺口：把 `capabilities.QRTTLSeconds` 改成 121，
// **一条测试都没变红** —— 因为 [TestSigninParameterSurfaceIsDefinedOnce]
// 比的是顶层与 signin 的**相对**一致，两边都走同一个函数，恒等；
// 于是「CLI 硬编码了一份 120」这种写法完全查不出来，而它的表现是
// 「改了注册表那边的码有效期，CLI 这个开关没跟着变」。
//
// ⇒ 判据要读 flag 默认值的**文本**并与注册表常量比：这样改源头会让期望值
// 跟着走，硬编码的那份才会现形。
func TestQrTTLDefaultComesFromTheRegistryNotFromASecondLiteral(t *testing.T) {
	login := findCommand(buildRoot(t), "login")
	if login == nil {
		t.Fatal("命令树里没有 login")
	}
	qr := findCommand(login, "qr")
	if qr == nil {
		t.Fatal("login 下没有 qr")
	}
	want := strconv.FormatInt(capabilities.QRTTLSeconds, 10)
	if got := defaultOf(t, qr.Flags(), "ttl-seconds"); got != want {
		t.Errorf("--ttl-seconds 的默认值应来自能力注册表（%s），实得 %s\n"+
			"第二份字面量的代价是：改了那边，这个开关不跟着变，而没人会注意", want, got)
	}
}

// TestAnUnregisteredNotMigratedCommandSaysSo：
// 表外的命令必须**点名自己没登记**，而不是含糊地说「尚未迁移」。
//
// ⭐ 反向测试先暴露了这里是个缺口：把 [notMigrated] 的 unknown 分支改成
// 静默给个"（未登记）"占位，**一条测试都没变红** —— 因为现有用例走的都是
// 表里**已有**的那几条，unknown 分支从来没被执行过。
//
// 而这条分支存在的全部理由就是挡住"顺手加了个子命令、把 RunE 指到
// notMigrated，却忘了往表里写" —— 一个从没被执行过的分支，等于没有。
// ⇒ 它是纯函数，直接调就行，不必造一棵带野子命令的命令树。
func TestAnUnregisteredNotMigratedCommandSaysSo(t *testing.T) {
	err := notMigrated("totally-made-up")
	if err == nil {
		t.Fatal("表外的命令也该报错 —— 静默通过等于允许一个新子命令没有任何后端说明")
	}
	if !strings.Contains(err.Error(), "没有登记") {
		t.Errorf("该点名「没有登记缺哪一层」，实得 %q", err.Error())
	}
}
