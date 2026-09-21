package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/app"
)

// legacyFixtureDir 是旧项目数据的夹具（io 层的导入用例也读它）。
const legacyFixtureDir = "../../test/fixtures/legacy"

// testPassword 是一条**测试用**口令：它只为把 argon2id 那一步跑通，
// 不属于任何真实账号，也不会被断言"等于"什么 —— 只断言它不许出现在输出里。
const testPassword = "go-cli-test-password"

func TestVersionMatchesTheSharedPayload(t *testing.T) {
	stdout, stderr, code := runCLI(t, "", "version", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d（stderr=%q）", code, stderr)
	}

	// 判据不是"看起来像 JSON"，而是"与 GET /api/version 出自同一个结构体"。
	var payload app.VersionPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("--json 的 stdout 必须是一份干净 JSON（`| jq` 要能直接接）：%v\n%s", err, stdout)
	}
	if payload != app.VersionPayloadOf() {
		t.Errorf("输出与 app.VersionPayloadOf() 不一致：%+v", payload)
	}
	if stderr != "" {
		t.Errorf("--json 时 stderr 不该有进度行：%q", stderr)
	}

	human, _, _ := runCLI(t, "", "version")
	if !strings.Contains(human, "时区") {
		t.Errorf("人看的输出该带时区那一栏，实得 %q", human)
	}
	if json.Valid([]byte(human)) {
		t.Error("不带 --json 时不该输出 JSON")
	}
}

func TestCapabilitiesSharesTheRegistryWithTheWebUI(t *testing.T) {
	stdout, stderr, code := runCLI(t, "", "capabilities", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d（stderr=%q）", code, stderr)
	}
	var payload app.CapabilitiesPayload
	if err := json.Unmarshal([]byte(stdout), &payload); err != nil {
		t.Fatalf("stdout 不是干净 JSON：%v", err)
	}
	if stderr != "" {
		t.Errorf("--json 时 stderr 不该有输出：%q", stderr)
	}
	if len(payload.All) == 0 {
		t.Fatal("注册表是空的")
	}

	human, _, _ := runCLI(t, "", "capabilities")
	if !strings.Contains(human, "通用任务列表可见") {
		t.Errorf("缺了那行可见能力清单：%q", human)
	}
	// 那一行必须是 join 出来的 `a, b` —— 直接打切片会得到 Go 的 `[a b]`，
	// 而它长得只差一个逗号，最容易在评审里滑过去。
	if got, want := visibleLine(t, human), strings.Join(payload.Visible, ", "); got != want {
		t.Errorf("可见清单渲染错了\n  期望: %q\n  实得: %q", want, got)
	}
}

// visibleLine 取出"通用任务列表可见: …"那一行的冒号后内容。
func visibleLine(t *testing.T, human string) string {
	t.Helper()
	const marker = "通用任务列表可见: "
	for _, line := range strings.Split(human, "\n") {
		if rest, found := strings.CutPrefix(line, marker); found {
			return rest
		}
	}
	t.Fatalf("输出里没有那一行：%q", human)
	return ""
}

// TestVersionFlagOnRoot：cobra 的 --version 与 `mcloud version` 是两条路，
// 但值必须来自同一个 core.Version。
func TestVersionFlagOnRoot(t *testing.T) {
	stdout, _, code := runCLI(t, "", "--version")
	if code != ExitSuccess {
		t.Fatalf("--version 应退 0，实得 %d", code)
	}
	// 逐字比，不用 Contains/HasPrefix：`mcloud version 0.1.0`（cobra 的默认模板）
	// 与 `mcloud 0.1.0`（clap 的形状）**都**满足"前缀是 mcloud"，那条弱断言
	// 当初就是绿的 —— 直到把模板改回默认才发现没人钉住它。
	if want := "mcloud " + app.VersionPayloadOf().Version + "\n"; stdout != want {
		t.Errorf("--version 输出应逐字等于 %q，实得 %q", want, stdout)
	}
}

func TestUserInitThenList(t *testing.T) {
	db := filepath.Join(t.TempDir(), "t.db")
	stdin := testPassword + "\n" + testPassword + "\n"

	stdout, stderr, code := runCLI(t, stdin, "user", "init", "--name", "admin", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("初始化该成功，退 %d（stderr=%q）", code, stderr)
	}
	if !strings.Contains(stdout, "已创建管理员 admin（角色 admin）") {
		t.Errorf("输出少了那一行，实得 %q", stdout)
	}
	if !strings.Contains(stdout, "mcloud login qr") {
		t.Errorf("少了下一步提示：%q", stdout)
	}

	// ⚠ 口令纪律：两条输入路径都不许把口令打到任何流里（终端那条由 readSecret 关回显）。
	if strings.Contains(stdout+stderr, testPassword) {
		t.Error("输出里出现了口令明文")
	}

	listOut, _, code := runCLI(t, "", "user", "list", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("list 应退 0，实得 %d", code)
	}
	if !strings.Contains(listOut, "用户 1 个：") || !strings.Contains(listOut, "admin") {
		t.Errorf("list 输出不对：%q", listOut)
	}
	// 哈希那一列绝不能出现在终端上（它就在同一个结构体里）。
	if strings.Contains(listOut, "$argon2id$") {
		t.Error("user list 把口令哈希打出来了")
	}
}

// TestUserInitReadsPasswordFromStdinNotFromFlags：`--password` 这个开关**不该存在**。
//
// 它以前不在，将来也不该被"顺手加一个参数方便点"带进来：参数会进 shell 历史与 `ps`。
// 所以这条断言的方向是**反向**的 —— 它要求解析失败。
func TestUserInitReadsPasswordFromStdinNotFromFlags(t *testing.T) {
	db := filepath.Join(t.TempDir(), "t.db")
	_, stderr, code := runCLI(t, "", "user", "init", "--db", db, "--password", testPassword)
	if code != ExitUsage {
		t.Errorf("--password 必须是未知参数（口令只能走标准输入），实得退 %d", code)
	}
	if !strings.Contains(stderr, "--password") {
		t.Errorf("错误信息该点名那个不存在的开关，实得 %q", stderr)
	}
}

func TestUserInitRejectsWeakOrMismatchedPasswords(t *testing.T) {
	cases := []struct {
		name   string
		stdin  string
		wants  []string
		reason string
	}{
		{
			name:   "口令太短",
			stdin:  "abc\nabc\n",
			wants:  []string{"至少"},
			reason: "策略在 core.HashPassword 那侧，CLI 只是转述",
		},
		{
			name:   "两次不一致",
			stdin:  testPassword + "\n" + testPassword + "9\n",
			wants:  []string{"两次输入的口令不一致"},
			reason: "这条判据只在 CLI 有，所以必须自己钉",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			db := filepath.Join(t.TempDir(), "t.db")
			_, stderr, code := runCLI(t, tc.stdin, "user", "init", "--db", db)
			if code != ExitRun {
				t.Fatalf("应作为运行错误退 1，实得 %d（stderr=%q）", code, stderr)
			}
			for _, want := range tc.wants {
				if !strings.Contains(stderr, want) {
					t.Errorf("错误里该有 %q，实得 %q", want, stderr)
				}
			}
			if strings.Contains(stderr, testPassword) {
				t.Error("失败信息里出现了口令明文")
			}
			// 失败不能留下半成品：库里一个用户都没有，list 仍说"还没有用户"。
			listOut, _, _ := runCLI(t, "", "user", "list", "--db", db)
			if !strings.Contains(listOut, "还没有用户") {
				t.Errorf("初始化失败后不该留下用户：%q（%s）", listOut, tc.reason)
			}
		})
	}
}

func TestUserInitTwiceRefusesSecondAdmin(t *testing.T) {
	db := filepath.Join(t.TempDir(), "t.db")
	stdin := testPassword + "\n" + testPassword + "\n"

	if _, _, code := runCLI(t, stdin, "user", "init", "--db", db); code != ExitSuccess {
		t.Fatal("第一次初始化该成功")
	}
	_, stderr, code := runCLI(t, stdin, "user", "init", "--db", db)
	if code != ExitRun {
		t.Fatalf("第二次该失败（初始化只管从零到一），实得 %d", code)
	}
	if !strings.Contains(stderr, "已经有") {
		t.Errorf("错误该说清为什么拒绝，实得 %q", stderr)
	}
}

// TestMigrateDryRunWritesNothing：预演那半句承诺是"不写任何文件"。
//
// 为什么值得单独一条：`--dry-run` 写库的后果是**不可逆的**（旧数据被归档、
// 账号落进库里），而它看起来完全像在预演。
func TestMigrateDryRunWritesNothing(t *testing.T) {
	dir := t.TempDir()
	db := filepath.Join(dir, "t.db")
	backup := filepath.Join(dir, "backup")

	stdout, stderr, code := runCLI(t, "", "migrate", "--dry-run",
		"--config", filepath.Join(legacyFixtureDir, "asign.json"),
		"--data-dir", legacyFixtureDir,
		"--db", db, "--backup-dir", backup, "--json")
	if code != ExitSuccess {
		t.Fatalf("预演该成功，退 %d（stderr=%q）", code, stderr)
	}
	var report map[string]any
	if err := json.Unmarshal([]byte(stdout), &report); err != nil {
		t.Fatalf("--json 的预演报告不是干净 JSON：%v\n%s", err, stdout)
	}
	value, found := report["dbPath"]
	if !found {
		t.Errorf("报告里该有 dbPath 这一项：%v", report)
	} else if value != nil {
		t.Errorf("预演的 dbPath 该是 null（脚本靠它区分「没写库」与「写到空串」），实得 %v", value)
	}

	if _, err := os.Stat(db); !os.IsNotExist(err) {
		t.Error("--dry-run 建出了目标库")
	}
	if entries, err := os.ReadDir(backup); err == nil && len(entries) > 0 {
		t.Error("--dry-run 把旧数据归档出去了")
	}
	// 预演压根不打开库，所以那条"库里还没有用户"的提示不该出现。
	if strings.Contains(stderr, "库里还没有用户") {
		t.Errorf("预演不该报无主提示：%q", stderr)
	}
}

func TestMigrateRealNotesUnownedAccounts(t *testing.T) {
	dir := t.TempDir()
	db := filepath.Join(dir, "t.db")

	stdout, stderr, code := runCLI(t, "", "migrate",
		"--config", filepath.Join(legacyFixtureDir, "asign.json"),
		"--data-dir", legacyFixtureDir,
		"--db", db, "--backup-dir", filepath.Join(dir, "backup"))
	if code != ExitSuccess {
		t.Fatalf("迁移该成功（有问题只提示，不影响退出码），退 %d（stderr=%q）", code, stderr)
	}
	if _, err := os.Stat(db); err != nil {
		t.Fatalf("真跑之后目标库该存在：%v", err)
	}
	// 空库 → 无主是**正常中间态**，但必须说一声（见 run_migrate.go）。
	if !strings.Contains(stderr, "user init") {
		t.Errorf("少了那条无主提示：%q", stderr)
	}
	if stdout == "" {
		t.Error("人看的报告一行都没有")
	}
}

// TestMigrateOverridePathsBeatRepoRootDerivation：--config / --data-dir 覆盖推导结果。
//
// 判据取"给了一个空的 --repo-root 但仍能跑成"：覆盖没生效时这条路会失败，
// 所以这条断言真的在读那两个开关。
func TestMigrateOverridePathsBeatRepoRootDerivation(t *testing.T) {
	dir := t.TempDir()
	stdout, stderr, code := runCLI(t, "", "migrate", "--dry-run", "--json",
		"--repo-root", dir,
		"--config", filepath.Join(legacyFixtureDir, "asign.json"),
		"--data-dir", legacyFixtureDir)
	if code != ExitSuccess {
		t.Fatalf("覆盖生效时该成功，退 %d（stderr=%q）", code, stderr)
	}
	var report map[string]any
	if err := json.Unmarshal([]byte(stdout), &report); err != nil {
		t.Fatalf("覆盖没生效时这里会失败（空仓库根推导不出任何旧数据）：%v\n%s", err, stdout)
	}
}

// TestUnknownFlagIsUsageError：未知参数退 2 且 stdout 干净。
func TestUnknownFlagIsUsageError(t *testing.T) {
	stdout, stderr, code := runCLI(t, "", "--not-a-real-flag")
	if code != ExitUsage {
		t.Fatalf("应退 2，实得 %d", code)
	}
	if stdout != "" {
		t.Errorf("用法错误不该写 stdout：%q", stdout)
	}
	if !strings.Contains(stderr, "用法") {
		t.Errorf("该有一行紧凑用法，实得 %q", stderr)
	}
}

// TestZeroArgSigninOnAnEmptyDatabaseIsNotAnError：零参数签到撞上空库**不是错误**。
//
// 这条取代了 `TestNotMigratedNamesTheMissingLayer`（它钉的是"缺口信息里要点名缺哪一层"，
// 而缺口表已经随全部叶子落地退役了）。同一个位置换成一条**行为**判据：
//
// 空库时 `mcloud signin` 该打印"库里还没有账号"并**退 0** —— 不是退 1。
// 一个没有账号的库是**正常状态**（刚装好就是这样），把它报成运行错误会让
// 定时任务/CI 把"还没登录"当成"签到挂了"。
//
// ⚠ `--db` 必须显式给：默认值是相对路径 `data/…`，不给就会在包目录下建出真库。
func TestZeroArgSigninOnAnEmptyDatabaseIsNotAnError(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.db")
	stdout, stderr, code := runCLI(t, "", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("空库不是错误，应退 0，实得 %d（stderr=%q）", code, stderr)
	}
	if !strings.Contains(stdout, "还没有账号") {
		t.Errorf("该说明库里还没有账号，实得 stdout=%q", stdout)
	}
}
