package cli

import (
	"io"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

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
			// ⚠ 仍然补一个临时 `--db`：默认值是**相对路径** `data/mcloud.db`，
			// 不补就会在包目录下建出一个真实的库（已经踩过一次，残留过 139 KB 的 db）。
			full := append(append([]string{}, argv...), "--db", filepath.Join(t.TempDir(), "state.db"))

			// ① **层**的判据：这条用例声称的是"**解析器**放行了它"，而退出码只有三档、
			// **表达不了"哪一层拦下的"**（`2` 同时属于解析期的 flag-group 校验与 RunE 期
			// 手工判的用法错误）⇒ 判据要**对准层**，直接调那一层。
			// 起因：`--account <越界>` 从退 1 改成退 2（2026-09-22，坑表 #126）之后，
			// 这里原来那句 `code != ExitUsage` 当场红 —— 而它红得**像实现错了**（坑表 #127）。
			assertParsesCleanly(t, head, full)

			// ② **契约**的判据：解析期放行**不等于**"跑起来也不会被判成用法错" ——
			// 后者是**另一层**的事（`--records-all` 缺 `--records` 就在 RunE 里判）。
			// 所以这里**继续**端到端跑一遍：不然将来有人往 accepted 里塞一条 RunE 层会被
			// 判错的组合，本用例会**静默放行**（那正是把"只到解析期"当成"放宽"的代价）。
			//
			// ⚠⚠ 唯一的例外是"点了库里的第 N 个账号"：本用例的库是**空的**，
			// 任何下标都越界 ⇒ 那**就是**用法错误（#126），端到端断言在这里不成立。
			// 例外**写进条件里、并具名**，不是把整条用例的端到端断言一起砍掉。
			if !namesAnAccountIndex(argv) {
				assertAccepted(t, head, full)
			}
		}
		for _, argv := range rejected {
			assertUsageRejected(t, head, argv)
		}
	}
}

// namesAnAccountIndex 判断 argv 里有没有按**序号**点账号的 `--account`。
//
// ⚠ 它存在只为一件事：把"本用例的库是**空的** ⇒ 任何下标都越界 ⇒ 那**就是**用法错误"
// 这个**例外具名**，而不是用"整条用例都别端到端跑"来回避一条特例（坑表 #127 的教训：
// 判据要么对准层，要么把例外写清楚 —— 别用"放宽全部"换一句好看的解释）。
func namesAnAccountIndex(argv []string) bool {
	for _, a := range argv {
		if a == "--account" || strings.HasPrefix(a, "--account=") {
			return true
		}
	}
	return false
}

// TestRecordsAllRequiresRecords：clap 的 `requires` 是**单向**的，
// cobra 没有对应原语（RequiredTogether 会把只给 --records 也判错），
// 所以这条是 RunE 里手工判的 —— 手工判的东西必须有用例，否则迟早退化。
func TestRecordsAllRequiresRecords(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.db")

	// 单独给 `--records-all` 是**用法**错误（退 2），且 stdout 必须干净。
	assertUsageRejected(t, nil, []string{"--records-all"})

	// 两条合法的组合现在会**真的跑后端**：库是空的 ⇒ 退 0（"库里还没有账号"
	// 那句话不是错误），所以判据只能是"没被判成用法错"。手工判的那条
	// `requires` 一旦退化成"也把只给 --records 判错"，这条当场红。
	for _, argv := range [][]string{
		{"--records-all", "--records", "--db", db},
		{"--records", "--db", db},
	} {
		assertAccepted(t, nil, argv)
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
		{"qr 平台名大小写敏感（clap 默认，macOS 是用法错误）", []string{"login", "qr", "--platform", "macOS"}, ExitUsage},
		{"qr 不认的平台是用法错误", []string{"login", "qr", "--platform", "linux"}, ExitUsage},
		{"sms 缺 --phone", []string{"login", "sms"}, ExitUsage},
		{"sms --phone", []string{"login", "sms", "--phone", "19900000001"}, ExitRun},
		{"sms --client app", []string{"login", "sms", "--phone", "19900000001", "--client", "app"}, ExitRun},
		{"sms --client 未知值", []string{"login", "sms", "--phone", "19900000001", "--client", "ios"}, ExitUsage},
		{"sms --send-only 与 --code 互斥", []string{
			"login", "sms", "--phone", "19900000001", "--send-only", "--code", "123456"}, ExitUsage},
		{"sms 默认 client 是 pc", []string{"login", "sms", "--phone", "19900000001"}, ExitRun},
	} {
		// ⚠ 三条路径**必须**给临时目录：`--db` / `--device-file` 的默认值是
		// `data/…`（相对当前目录），跑参数面用例就会在包目录下建出一个 data/，
		// 而设备指纹一旦落盘，"指纹已存在"就成了跨用例的隐藏状态。
		// 后端没接上时这些文件根本不会被创建，所以这个坑是在 login 接上之后才现形的。
		dir := t.TempDir()
		argv := append([]string{}, tc.argv...)
		argv = append(argv, "--db", dir+"/state.db", "--device-file", dir+"/device.txt")
		// ⚠ `--app-device-file` 只有 `login sms` 认。给 `qr` 也加上会得到一个
		// "unknown flag" —— 用例照样红，但红的理由从"平台名不认"变成了
		// "我给了一个不存在的参数"，于是它**不再**在断言它该断言的那件事。
		if tc.argv[1] == "sms" {
			argv = append(argv, "--app-device-file", dir+"/device-app.txt")
		}

		_, stderr, code := runCLI(t, "", argv...)
		if code != tc.want {
			t.Errorf("%s：%v 应退 %d，实得 %d（stderr=%q）", tc.name, argv, tc.want, code, stderr)
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
		"auth refresh",
		"capabilities",
		"fixture extract",
		"live-room",
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

// TestEveryLeafActuallyRuns：**每个叶子都真的跑起来了**，没有"登记在缺口表里"这个中间态。
//
// # 这条用例取代了什么
//
// 它取代的是 `unmigrated.go` 那张缺口表（连同它的两条对价用例）。
// 那张表的判据是"每个叶子要么能跑、要么登记了缺哪一层"，现在**没有任何叶子可登记**，
// 于是表整个退役 —— 表自己的用例就写着"表空了就该整个删掉"。
//
// # 为什么还留着一条（而不是一起删掉）
//
// 缺口表退役之后，"某个叶子又变回没实现"这件事就**没有任何守卫**了。
// 这条用例把退役时的那个事实钉住：叶子逐个跑，**一个都不许出现"尚未迁移"**。
// 将来谁把某条 RunE 指回一个占位错误，它当场红。
//
// # 它跑的那份清单在哪
//
// 在 [leafSmokeRuns]（包级函数，不是函数内局部量）—— 因为
// [TestEveryLeafActuallyRunsIsComplete] 要拿它对着命令树查完整性。
// ⚠ 加叶子时**两处都要看**：漏一行的表现是这两条**一起绿**，
// 而"某个叶子其实没人跑"没有任何输出（2026-09-22 实测踩到过，见那条用例的说明）。
func TestEveryLeafActuallyRuns(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.db")
	// 只要求"没出现占位错误"。跑成不成功取决于库与文件系统的状态，
	// 那不是这条用例的判据（各自的叶子有各自的用例）。
	for _, argv := range leafSmokeRuns(t, db) {
		_, stderr, _ := runCLI(t, "", argv...)
		if strings.Contains(stderr, "尚未迁移") || strings.Contains(stderr, "未实现") {
			t.Errorf("`mcloud %v` 又变回占位实现了：%q", argv, stderr)
		}
	}
}

// leafSmokeRuns 返回"每个叶子跑一次"的 argv 表（`db` 是调用方给的临时库路径）。
//
// ⚠ 每条都给临时 `--db`：默认值是相对路径 `data/…`，不补会在包目录下建出真库。
// ⚠ `fixture extract` 与 `migrate` 不用 `--db`，各自补自己那个输出/来源参数。
// ⚠ 用**函数**而不是包级变量：临时路径必须按调用方各自求值；
// 写成包级变量就得在包初始化时求值，那些"临时"路径会变成进程级共享的固定路径。
func leafSmokeRuns(t *testing.T, db string) [][]string {
	t.Helper()
	return [][]string{
		{"version"},
		{"capabilities"},
		{"user", "list", "--db", db},
		{"login", "qr", "--db", db, "--device-file", filepath.Join(t.TempDir(), "dev.txt")},
		{"login", "sms", "--db", db, "--device-file", filepath.Join(t.TempDir(), "dev.txt")},
		{"smid", "show", "--db", db},
		// ⚠ `smid reset` / `smid refresh` / `auth refresh` 这三条是 2026-09-22 补进来的：
		// 在此之前清单里只有 10 条而叶子有 14 条 —— 也就是说这条用例的名字
		// （"每个叶子都真的跑起来了"）**当时是假的**，而它照样全绿。
		// 三条都实测过：空库上是"没有账号"这类正常输出，不联网、不落盘；
		// `auth refresh` 得补 `--all`，否则它自己要 `--account`/`--all` ⇒ 退 2 用法错。
		{"smid", "reset", "--db", db},
		{"smid", "refresh", "--db", db},
		{"auth", "refresh", "--all", "--db", db},
		{"signin", "--db", db},
		{"fixture", "extract", "--har", t.TempDir(), "--out", t.TempDir()},
		{"migrate", "--repo-root", t.TempDir(), "--dry-run"},
		// `live-room` **不需要**额外的临时路径：空库会在
		// `len(targets)==0` 那个分支直接返回，**早于** `LoadOrCreateAppDeviceID`
		// ⇒ 既不联网也不落盘。⚠ 反过来，如果哪天把设备解析挪到选账号之前，
		// 这条就会在包目录下建出 `data/app-device.txt`（默认值是相对路径）——
		// 那时必须在这里补 `--app-device-file`，否则它静默地污染工作树。
		{"live-room", "--db", db},
	}
}

// leafSmokeExcluded 是**故意**不进 [leafSmokeRuns] 的叶子 → 理由。
//
// ⚠ 这份白名单会被 [TestEveryLeafActuallyRunsIsComplete] **反向检查**：
// 登记了一个不是叶子的路径（写错名，或那条叶子后来被删/改名了）也会红。
// 与门禁里那几张白名单同一条纪律 —— **过期的白名单比没有白名单更坏**，
// 因为它会让下一个人以为"这里排除某样东西是有意的"。
var leafSmokeExcluded = map[string]string{
	"user init": "交互式：裸跑会打印 `设置口令: ` 并在 stdin 上等输入" +
		"（实测不给输入就是挂住）。它由 run_test.go 的 TestUserInitThenList / " +
		"TestUserInitReadsPasswordFromStdinNotFromFlags / " +
		"TestUserInitRejectsWeakOrMismatchedPasswords / TestUserInitTwiceRefusesSecondAdmin " +
		"四条覆盖（都注入 stdin 或 --password）—— 那条路径才是它的判据",
}

// TestEveryLeafActuallyRunsIsComplete：上面那份 smoke 清单**一个叶子都不漏**。
//
// # 为什么必须有这条
//
// [TestEveryLeafActuallyRuns] 的名字是"每个叶子都真的跑起来了"，但它跑的是一份
// **手写清单**。2026-09-22 实测：那份清单当时只有 10 条，而命令树上有 14 个叶子 ——
// 也就是说那个名字**当时是假的**，而它照样全绿，没有任何输出提示这件事。
// ⇒ 手写清单与命令树之间必须有机器判据，否则"加了个叶子忘了加进来"永远静默。
// 这条红的时候**不要**放宽它，去补 [leafSmokeRuns]（或写清排除理由）。
func TestEveryLeafActuallyRunsIsComplete(t *testing.T) {
	covered := map[string]bool{}
	for _, argv := range leafSmokeRuns(t, filepath.Join(t.TempDir(), "state.db")) {
		covered[commandPathOf(argv)] = true
	}

	onTree := map[string]bool{}
	for _, leaf := range leafPaths(buildRoot(t)) {
		onTree[leaf] = true
		if covered[leaf] || leafSmokeExcluded[leaf] != "" {
			continue
		}
		t.Errorf("叶子 `%s` 既不在 smoke 清单里、也没登记排除理由 —— "+
			"`TestEveryLeafActuallyRuns` 声称「每个叶子都真的跑起来了」，"+
			"而它漏了这个：要么在 leafSmokeRuns 里补一行，"+
			"要么在 leafSmokeExcluded 里写清为什么它不能跑", leaf)
	}

	// 反向：白名单里挂了命令树上不存在的路径 ⇒ 那条叶子改名或被删了，
	// 而这条理由从此再不会被任何人读到（它只会让人误以为"排除是有意的"）。
	for name, reason := range leafSmokeExcluded {
		if onTree[name] {
			continue
		}
		t.Errorf("leafSmokeExcluded 里的 `%s` 不是命令树上的叶子（改名了？删了？）：%s",
			name, reason)
	}
}

// commandPathOf 从一条 smoke argv 里取出它打的那个子命令路径（遇到第一个 flag 就停）。
//
// ⚠ 不能用"取前 N 个 token"：`login sms` 是两个、`version` 是一个，
// 而 `signin` 后面直接就跟 flag。判据只能是"flag 之前的都是路径"。
func commandPathOf(argv []string) string {
	var parts []string
	for _, tok := range argv {
		if strings.HasPrefix(tok, "-") {
			break
		}
		parts = append(parts, tok)
	}
	return strings.Join(parts, " ")
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
	want := strconv.FormatInt(int64(capabilities.QRTTL/time.Second), 10)
	if got := defaultOf(t, qr.Flags(), "ttl-seconds"); got != want {
		t.Errorf("--ttl-seconds 的默认值应来自能力注册表（%s），实得 %s\n"+
			"第二份字面量的代价是：改了那边，这个开关不跟着变，而没人会注意", want, got)
	}
}

// 早先这里有一条 `TestAnUnregisteredNotMigratedCommandSaysSo`，测的是
// `notMigrated` 的 unknown 分支（"顺手加了个子命令、把 RunE 指到 notMigrated，
// 却忘了往表里写"）。缺口表退役之后那个分支不存在了，用例一并删掉 ——
// **留下一条测不到东西的用例比没有用例更糟**：它会让"这一块有人看着"这个错觉
// 一直持续下去。那个位置现在由 [TestEveryLeafActuallyRuns] 守着。
