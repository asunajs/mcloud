package app

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
)

// legacyFixturesDir 是真实脱敏夹具所在目录（`test/fixtures/legacy/`）。
//
// 它由本文件所在位置推导，不写死相对路径 —— 否则换个工作目录就找不到夹具，
// 而"找不到"在 [HasLegacyData] 那里是静默的 false，测试会被无声地跑成空集。
func legacyFixturesDir() string {
	return filepath.Join("..", "..", "test", "fixtures", "legacy")
}

func fixturePlan(t *testing.T) MigrationPlan {
	t.Helper()
	work := t.TempDir()
	return MigrationPlan{
		Source: mcloudio.NewLegacySource(
			filepath.Join(legacyFixturesDir(), "asign.json"),
			legacyFixturesDir(),
		),
		DBPath:    filepath.Join(work, "mcloud.db"),
		BackupDir: filepath.Join(work, "legacy-source"),
	}
}

// seedAdmin 预置管理员（`mcloud user init` 的等价物）。
//
// 账号从 v3 起必须有主人，所以"**先 init 再 migrate**"是最省事的一条路。
// 但它**不是唯一**的路：下面
// [TestMigrationWithoutAUserKeepsAccountsUnowned] 专门盯着"先 migrate 再 init"
// 那条（CLI 走的正是它）。
func seedAdmin(t *testing.T, dbPath string) core.UserID {
	t.Helper()
	store, err := mcloudio.Open(dbPath)
	if err != nil {
		t.Fatalf("打开 %s 失败: %v", dbPath, err)
	}
	defer func() { _ = store.Close() }()
	admin := core.NewUser("admin", "admin", "!", core.RoleAdmin, testClock().Now())
	if err := store.UpsertUser(context.Background(), &admin); err != nil {
		t.Fatalf("写入管理员失败: %v", err)
	}
	return admin.ID
}

func openDB(t *testing.T, path string) *mcloudio.SqliteStore {
	t.Helper()
	store, err := mcloudio.Open(path)
	if err != nil {
		t.Fatalf("打开 %s 失败: %v", path, err)
	}
	t.Cleanup(func() { _ = store.Close() })
	return store
}

// TestMigrationWithoutAUserKeepsAccountsUnowned：
// **库里一个用户都没有时，迁移要照跑**，导入的账号保持无主。
//
// 这条盯的是 CLI 那条路（它一律 [core.ScopeLocal]）："先 migrate 再 init"
// 必须走得通，否则用户第一次上手就撞一道门禁。
//
// ⚠ Rust 侧这里断言的恰恰相反（要求返回 `NoOwner`）—— 而那个行为与
// `run` 自己的注释、以及 CLI 侧的注释都矛盾。保持无主是**安全**的：
// `owner` 落 `NULL`，`user init` 的 `ClaimUnowned` 按 `owner IS NULL` 认领，
// 所以下面紧接着断言"认领能把它们收回去"。
func TestMigrationWithoutAUserKeepsAccountsUnowned(t *testing.T) {
	plan := fixturePlan(t)
	ctx := context.Background()

	outcome, err := RunMigration(ctx, plan, false, nil, testClock())
	if err != nil {
		t.Fatalf("没有用户时迁移也该跑得通（账号先保持无主，等 init 认领）: %v", err)
	}
	if outcome.Report.Accounts != 2 {
		t.Fatalf("两个账号都该导进来，实得 %d", outcome.Report.Accounts)
	}

	store := openDB(t, plan.DBPath)

	// ① 无主：`Scope::user(任何人)` 都看不见 —— 这是"还没认领"的正常状态
	nobody := core.UserID("nobody")
	visible, err := store.ListAccounts(ctx, core.ScopeUser(&nobody))
	if err != nil {
		t.Fatalf("列举账号失败: %v", err)
	}
	if len(visible) != 0 {
		t.Errorf("无主账号对具体用户不可见，实得 %d 个", len(visible))
	}

	// ② 但本地范围看得见（CLI 就是靠它干活的）
	local, err := store.ListAccounts(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("列举账号失败: %v", err)
	}
	if len(local) != 2 {
		t.Errorf("本地范围应看到 2 个账号，实得 %d", len(local))
	}

	// ③ 补建管理员时被认领 —— "先 migrate 再 init"的收尾就靠这一步。
	//
	// `owner` 上有 `REFERENCES users(id)` 外键，所以这一步**必须**先有真实的用户行；
	// 光造一个 `UserID` 值去认领会撞 FOREIGN KEY 失败
	//（那正是 `mcloud user init` 里"先建用户、再认领"的顺序）。
	admin := seedAdmin(t, plan.DBPath)
	claimed, err := store.ClaimUnowned(ctx, admin)
	if err != nil {
		t.Fatalf("认领失败: %v", err)
	}
	if claimed.Accounts != 2 {
		t.Errorf("两个账号都该被认领，实得 %d", claimed.Accounts)
	}
	owned, err := store.ListAccounts(ctx, core.ScopeUser(&admin))
	if err != nil {
		t.Fatalf("列举账号失败: %v", err)
	}
	if len(owned) != 2 {
		t.Errorf("认领后管理员应看到 2 个账号，实得 %d", len(owned))
	}
}

func TestDryRunNeitherCreatesADatabaseNorArchives(t *testing.T) {
	plan := fixturePlan(t)

	outcome, err := RunMigration(context.Background(), plan, true, nil, testClock())
	if err != nil {
		t.Fatalf("预演应成功: %v", err)
	}
	if !outcome.DryRun {
		t.Error("DryRun 应为 true")
	}
	if outcome.Archived != 0 {
		t.Errorf("预演不该归档，实得 %d", outcome.Archived)
	}
	if outcome.DBPath != nil {
		t.Errorf("预演不建库，DBPath 应为 nil，实得 %q", *outcome.DBPath)
	}
	if _, err := os.Stat(plan.DBPath); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("预演不该在磁盘上留下数据库，实得 err=%v", err)
	}
	if _, err := os.Stat(plan.BackupDir); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("预演不该归档，实得 err=%v", err)
	}

	// 预演的报告必须和真实导入一致 —— 这正是"共用同一条路径"的意义
	if outcome.Report.Accounts != 2 {
		t.Errorf("预演应报告 2 个账号，实得 %d", outcome.Report.Accounts)
	}
	if outcome.Report.Runs != 12 {
		t.Errorf("预演应报告 12 条历史，实得 %d", outcome.Report.Runs)
	}
	if outcome.IssueCount() != 0 {
		t.Errorf("夹具不该有问题，实得 %d", outcome.IssueCount())
	}
}

func TestRealRunArchivesBeforeWritingTheDatabase(t *testing.T) {
	plan := fixturePlan(t)
	admin := seedAdmin(t, plan.DBPath)

	outcome, err := RunMigration(context.Background(), plan, false, &admin, testClock())
	if err != nil {
		t.Fatalf("迁移应成功: %v", err)
	}
	if outcome.DryRun {
		t.Error("真实导入的 DryRun 应为 false")
	}
	if outcome.Archived != 7 {
		t.Errorf("应归档 asign.json + 6 个数据文件，实得 %d", outcome.Archived)
	}
	if outcome.DBPath == nil || *outcome.DBPath != plan.DBPath {
		t.Errorf("DBPath 应是目标库，实得 %v", outcome.DBPath)
	}
	if _, err := os.Stat(plan.DBPath); err != nil {
		t.Errorf("应当建出数据库: %v", err)
	}

	// 归档内容与源逐字一致：它是"可离线重放的原始数据"，不是改写过的副本
	names := append([]string{"asign.json"}, mcloudio.LEGACY_DATA_FILES...)
	for _, name := range names {
		archived, err := os.ReadFile(filepath.Join(plan.BackupDir, name))
		if err != nil {
			t.Errorf("归档里应有 %s: %v", name, err)
			continue
		}
		original, err := os.ReadFile(filepath.Join(legacyFixturesDir(), name))
		if err != nil {
			t.Fatalf("读源 %s 失败: %v", name, err)
		}
		if string(archived) != string(original) {
			t.Errorf("%s 归档内容应逐字一致（长度 %d vs %d）", name, len(archived), len(original))
		}
	}

	// 库真的建了东西，而且**账号归属于那个管理员**（否则它们谁都看不见）
	ctx := context.Background()
	store := openDB(t, plan.DBPath)
	accounts, err := store.ListAccounts(ctx, core.ScopeUser(&admin))
	if err != nil {
		t.Fatalf("列举账号失败: %v", err)
	}
	if len(accounts) != 2 {
		t.Fatalf("管理员应看到 2 个账号，实得 %d", len(accounts))
	}
	for _, a := range accounts {
		if a.Owner == nil || *a.Owner != admin {
			t.Errorf("导入的账号必须归属于执行迁移的用户，实得 %v", a.Owner)
		}
	}
	other := core.UserID("别人")
	foreign, err := store.ListAccounts(ctx, core.ScopeUser(&other))
	if err != nil {
		t.Fatalf("列举账号失败: %v", err)
	}
	if len(foreign) != 0 {
		t.Errorf("换个用户就该一个也看不到，实得 %d 个", len(foreign))
	}
}

func TestArchivingIsIdempotent(t *testing.T) {
	plan := fixturePlan(t)
	admin := seedAdmin(t, plan.DBPath)

	if _, err := RunMigration(context.Background(), plan, false, &admin, testClock()); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}
	again, err := mcloudio.ArchiveLegacySource(plan.Source, plan.BackupDir)
	if err != nil {
		t.Fatalf("重复归档失败: %v", err)
	}
	if again != 0 {
		t.Errorf("内容相同的文件不该重复写盘，实得 %d", again)
	}
}

func TestSecondRunReportsEverythingUnchangedAndStaysConsistent(t *testing.T) {
	plan := fixturePlan(t)
	admin := seedAdmin(t, plan.DBPath)
	ctx := context.Background()

	if _, err := RunMigration(ctx, plan, false, &admin, testClock()); err != nil {
		t.Fatalf("首次迁移失败: %v", err)
	}
	second, err := RunMigration(ctx, plan, false, &admin, testClock())
	if err != nil {
		t.Fatalf("二次迁移失败: %v", err)
	}
	if second.Report.Runs != 0 {
		t.Errorf("源没变就不重复写，实得 %d", second.Report.Runs)
	}
	if second.Report.Accounts != 0 {
		t.Errorf("源没变就不重复写账号，实得 %d", second.Report.Accounts)
	}
	if len(second.Report.SkippedUnchanged) != len(mcloudio.LEGACY_DATA_FILES) {
		t.Errorf("应跳过 %d 个未变文件，实得 %v",
			len(mcloudio.LEGACY_DATA_FILES), second.Report.SkippedUnchanged)
	}
	if second.IssueCount() != 0 {
		t.Errorf("跳过不是问题，实得 %d", second.IssueCount())
	}

	// 库里不因重跑而多行
	store := openDB(t, plan.DBPath)
	runs, err := store.ListRuns(ctx, core.ScopeUser(&admin), 100)
	if err != nil {
		t.Fatalf("列举历史失败: %v", err)
	}
	if len(runs) != 12 {
		t.Errorf("重跑之后仍应是 12 条历史，实得 %d", len(runs))
	}
}

func TestHumanLinesMentionTheDryRunAndTheIssues(t *testing.T) {
	plan := fixturePlan(t)

	outcome, err := RunMigration(context.Background(), plan, true, nil, testClock())
	if err != nil {
		t.Fatalf("预演失败: %v", err)
	}
	text := strings.Join(outcome.HumanLines(), "\n")
	for _, want := range []string{"预演", "账号 2", "没有需要人工确认的问题"} {
		if !strings.Contains(text, want) {
			t.Errorf("预演输出应包含 %q，实得:\n%s", want, text)
		}
	}

	admin := seedAdmin(t, plan.DBPath)
	real, err := RunMigration(context.Background(), plan, false, &admin, testClock())
	if err != nil {
		t.Fatalf("迁移失败: %v", err)
	}
	text = strings.Join(real.HumanLines(), "\n")
	for _, want := range []string{"迁移完成", "已归档 7 个源文件", "数据库:"} {
		if !strings.Contains(text, want) {
			t.Errorf("真实导入输出应包含 %q，实得:\n%s", want, text)
		}
	}
}

func TestHasLegacyDataIsTrueWhenAtLeastOneFileExists(t *testing.T) {
	dir := t.TempDir()
	source := mcloudio.NewLegacySource(filepath.Join(dir, "asign.json"), dir)
	if HasLegacyData(source) {
		t.Error("空目录不算有旧数据")
	}

	// 只有数据文件、没有 asign.json 也算 —— 否则整次迁移会被静默跳过
	if err := os.WriteFile(filepath.Join(dir, "history.json"), []byte("{}"), 0o644); err != nil {
		t.Fatal(err)
	}
	if !HasLegacyData(source) {
		t.Error("只要有一个数据文件就算有旧数据")
	}
}

func TestOutcomeJSONIsMachineConsumable(t *testing.T) {
	plan := fixturePlan(t)
	outcome, err := RunMigration(context.Background(), plan, true, nil, testClock())
	if err != nil {
		t.Fatalf("预演失败: %v", err)
	}
	raw, err := OutcomeToJSON(outcome)
	if err != nil {
		t.Fatalf("序列化失败: %v", err)
	}
	payload, ok := core.ParseJSONTo[map[string]any](string(raw))
	if !ok {
		t.Fatalf("输出的不是 JSON 对象: %s", raw)
	}
	if got := asText(payload["dryRun"]); got != "true" {
		t.Errorf("dryRun 应为 true，实得 %s", got)
	}
	if payload["dbPath"] != nil {
		t.Errorf("预演的 dbPath 应是 JSON 的 null，实得 %v", payload["dbPath"])
	}
	report, _ := payload["report"].(map[string]any)
	if report == nil {
		t.Fatalf("输出应含 report，实得 %s", raw)
	}
	if got := asText(report["runs"]); got != "12" {
		t.Errorf("report.runs 应为 12，实得 %s", got)
	}
	if _, ok := report["summary"].(string); !ok {
		t.Errorf("report.summary 应是字符串，实得 %T", report["summary"])
	}
}

// asText 把 JSON 里的值转成文本再比。
//
// ⚠ 这是 §6.1 #34 的老规矩：[core.ParseJSON] 为"19 位标识不丢精度"返回
// `json.Number`，直接跟 `float64(12)` 比会得到"实得 12，期望 12"这种
// 看不出哪里不同的报错。
func asText(value any) string { return fmt.Sprintf("%v", value) }

func TestIdentityFailureLeavesNoDatabaseBehind(t *testing.T) {
	dir := t.TempDir()
	// 两个手机号掩码都是 19****16 → 身份映射必须失败
	//（`19900000016` 与 `19900000216`：都取自 199 合成号段，掩码故意撞车）
	if err := os.WriteFile(filepath.Join(dir, "asign.json"), []byte(`{ "caiyun": [
		{ "auth": "Basic MjoxOTkwMDAwMDAxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr" },
		{ "auth": "Basic MjoxOTkwMDAwMDIxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr" }
	] }`), 0o644); err != nil {
		t.Fatal(err)
	}
	plan := MigrationPlan{
		Source:    mcloudio.NewLegacySource(filepath.Join(dir, "asign.json"), dir),
		DBPath:    filepath.Join(dir, "mcloud.db"),
		BackupDir: filepath.Join(dir, "backup"),
	}

	// 先有用户：这样失败点才是身份映射，而不是"没有归属人"。
	//（两者的顺序都发生在任何写入之前，但测的是哪个错必须说清楚。）
	admin := seedAdmin(t, plan.DBPath)

	_, err := RunMigration(context.Background(), plan, false, &admin, testClock())
	if err == nil {
		t.Fatal("掩码冲突必须失败")
	}
	var perr *mcloudio.LegacyImportError
	if !errors.As(err, &perr) || perr.Kind != mcloudio.LegacyImportIdentity {
		t.Fatalf("应是身份映射失败，实得 %T %v", err, err)
	}

	// 归档**已经完成**：顺序是"先归档、再导入"，所以哪怕导入一步没走，
	// 原始数据也已经在新项目里有一份可追溯的副本。
	if _, err := os.Stat(filepath.Join(plan.BackupDir, "asign.json")); err != nil {
		t.Errorf("身份映射失败之前归档就该完成 —— 归档先于导入，实得 err=%v", err)
	}

	// 但**库里不许有账号**
	ctx := context.Background()
	store := openDB(t, plan.DBPath)
	accounts, err := store.ListAccounts(ctx, core.ScopeUser(&admin))
	if err != nil {
		t.Fatalf("列举账号失败: %v", err)
	}
	if len(accounts) != 0 {
		t.Errorf("身份映射失败后库里不该有任何账号，实得 %d", len(accounts))
	}
}

// TestThePlanDerivesBothReadPathsFromTheRepoRoot：
// 两个读取位置**不在一起**（账号配置在根、数据在 `packages/api/data/`），
// 分开推导而不是只给一个根 —— 免得将来有人把 `asign.json` 也挪进 `data/`
// 之后这里静默读不到。
func TestThePlanDerivesBothReadPathsFromTheRepoRoot(t *testing.T) {
	plan := MigrationPlanFromRepoRoot("/old", "/new/mcloud.db", "/new/legacy-source")
	if plan.Source.ConfigPath != filepath.Join("/old", "asign.json") {
		t.Errorf("配置路径应为 <root>/asign.json，实得 %q", plan.Source.ConfigPath)
	}
	if plan.Source.DataDir != filepath.Join("/old", "packages", "api", "data") {
		t.Errorf("数据目录应为 <root>/packages/api/data，实得 %q", plan.Source.DataDir)
	}
	// 写入位置**不**从仓库根推导 —— 旧仓库自始至终只读
	if plan.DBPath != "/new/mcloud.db" || plan.BackupDir != "/new/legacy-source" {
		t.Errorf("写入位置应原样保留，实得 %q / %q", plan.DBPath, plan.BackupDir)
	}
}

// TestArchivingHappensEvenWhenTheDatabaseCannotBeOpened：
// 归档**在建库之前**，所以库打不开时源数据也已经落地。
//
// 这条是"先归档"这条顺序**唯一**能被测到的地方：正常情况下归档与建库都成功，
// 谁先谁后看不出来（把两行换位置，其余九条测试一条都不变红 —— 已实测）。
// 只有让建库失败、再看归档有没有发生，顺序才成为一个可观测的事实。
func TestArchivingHappensEvenWhenTheDatabaseCannotBeOpened(t *testing.T) {
	plan := fixturePlan(t)
	// 一个**目录**当库路径：SQLite 打不开它
	plan.DBPath = t.TempDir()

	_, err := RunMigration(context.Background(), plan, false, nil, testClock())
	if err == nil {
		t.Fatal("库打不开时应当报错")
	}

	// 归档不依赖库，所以这一步必须已经完成
	for _, name := range append([]string{"asign.json"}, mcloudio.LEGACY_DATA_FILES...) {
		if _, err := os.Stat(filepath.Join(plan.BackupDir, name)); err != nil {
			t.Errorf("归档应先于建库完成，缺 %s（err=%v）", name, err)
		}
	}
}
