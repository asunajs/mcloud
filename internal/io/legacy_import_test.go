package mcloudio

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// helper
//
// 夹具是**真实脱敏**的旧项目数据（`test/fixtures/legacy/`），Rust 侧那 1,163 行
// 测试读的也是它 ⇒ 两边从同一份输入得出同样的数字，才算真对上了。
// ─────────────────────────────────────────────────────────────────────────────

const legacyFixtures = "../../test/fixtures/legacy"

func legacyNow() time.Time {
	fixed, err := time.Parse(time.RFC3339, "2026-09-17T00:00:00Z")
	if err != nil {
		panic(err)
	}
	return fixed.UTC()
}

// 夹具里两个账号的掩码 AccountID（前 2 + `****` + 后 2）。
const (
	acc1 = "19****01" // 手机号 19900000001，昵称「账号2」
	acc2 = "19****02" // 手机号 19900000002，昵称「账号1」
)

func openStore(t *testing.T) *SqliteStore {
	t.Helper()
	store, err := OpenInMemory()
	if err != nil {
		t.Fatalf("内存库应可打开: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })
	return store
}

// ensureUser 建好首个用户 —— 账号从 v3 起有归属，而 `accounts.owner` 有外键指向
// `users`，也就是说**导入之前必须真的有一个用户**。这正是生产路径的要求
// （先 `user init` 再 `migrate`），测试里同样照办，不给自己开免检通道。
func ensureUser(t *testing.T, store *SqliteStore) core.UserID {
	t.Helper()
	hash, err := core.HashPassword("correct-horse", newSeqRandom(7))
	if err != nil {
		t.Fatalf("算口令哈希: %v", err)
	}
	user := core.NewUser("u-test", "test", hash, core.RoleAdmin, legacyNow())
	if err := store.UpsertUser(context.Background(), &user); err != nil {
		t.Fatalf("建测试用户: %v", err)
	}
	return user.ID
}

// runImport 导入的统一入口：建好首个用户，并把旧数据导到它名下。
//
// 这里刻意走 [core.ScopeUser] 而不是 [core.ScopeLocal]：两者在导入器里的差别只是
// "账号归谁"，而这些用例关心的正是归属正确 —— 所以用 User 范围覆盖它。
// Local（账号保持无主）由 DiagnoseLegacy 的预演用例覆盖。
func runImport(t *testing.T, store *SqliteStore, source LegacySource) MigrationReport {
	t.Helper()
	uid := ensureUser(t, store)
	report, err := ImportLegacy(context.Background(), store, core.ScopeUser(&uid), source, legacyNow())
	if err != nil {
		t.Fatalf("导入失败: %v", err)
	}
	return report
}

func fixtureSource() LegacySource {
	root, err := filepath.Abs(legacyFixtures)
	if err != nil {
		panic(err)
	}
	return NewLegacySource(filepath.Join(root, "asign.json"), root)
}

// copyFixtures 把夹具整份复制到临时目录，供"改坏一个文件"的场景使用。
func copyFixtures(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	names := append([]string{"asign.json"}, LEGACY_DATA_FILES...)
	for _, name := range names {
		raw, err := os.ReadFile(filepath.Join(legacyFixtures, name))
		if err != nil {
			t.Fatalf("读夹具 %s 失败: %v", name, err)
		}
		if err := os.WriteFile(filepath.Join(dir, name), raw, 0o644); err != nil {
			t.Fatalf("写夹具 %s 失败: %v", name, err)
		}
	}
	return dir
}

func sourceOf(dir string) LegacySource {
	return NewLegacySource(filepath.Join(dir, "asign.json"), dir)
}

// ─────────────────────────────────────────────────────────────────────────────
// 端到端
// ─────────────────────────────────────────────────────────────────────────────

func TestFixtureImportLandsEveryEntity(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	report := runImport(t, store, fixtureSource())
	scope := core.ScopeUser(&uid)

	if report.Accounts != 2 {
		t.Errorf("应导入 2 个账号，实得 %d", report.Accounts)
	}
	if report.AccountStates != 2 {
		t.Errorf("应导入 2 条登录态，实得 %d", report.AccountStates)
	}
	if report.Pipelines != 1 {
		t.Errorf("夹具里只有 1 条组合，实得 %d", report.Pipelines)
	}
	if report.Schedules != 0 {
		t.Errorf("夹具的 schedules.json 是空的，实得 %d", report.Schedules)
	}
	if report.Runs != 12 {
		t.Errorf("夹具里有 12 条运行历史，实得 %d", report.Runs)
	}
	if len(report.SkippedUnchanged) != 0 {
		t.Errorf("首次导入不该跳过任何文件，实际跳过 %v", report.SkippedUnchanged)
	}
	if report.HasIssues() {
		t.Errorf("干净的旧数据不该产生问题，实际：%v", report.IssueLines())
	}

	// 落库结果与报告一致 —— 报告说导了却查不到是最坏的失败方式
	ctx := context.Background()
	if got := len(mustListAccounts(t, store, scope)); got != 2 {
		t.Errorf("库里应有 2 个账号，实得 %d", got)
	}
	if got := len(mustListRuns(t, store, scope)); got != 12 {
		t.Errorf("库里应有 12 条历史，实得 %d", got)
	}
	if got := len(mustListPipelines(t, store, scope)); got != 1 {
		t.Errorf("库里应有 1 条组合，实得 %d", got)
	}
	schedules, err := store.ListSchedules(ctx, scope)
	if err != nil {
		t.Fatalf("列定时任务: %v", err)
	}
	if len(schedules) != 0 {
		t.Errorf("库里不该有定时任务，实得 %d", len(schedules))
	}
}

func TestAccountsAreKeyedByMaskNotByPhone(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	runImport(t, store, fixtureSource())
	scope := core.ScopeUser(&uid)

	accounts := mustListAccounts(t, store, scope)
	if len(accounts) != 2 {
		t.Fatalf("应有 2 个账号，实得 %d", len(accounts))
	}
	// ⚠ 列表顺序由 SQL 保证（sort_order），不是 map 顺序 —— 见 §6.1 #21
	if accounts[0].ID.String() != acc1 || accounts[1].ID.String() != acc2 {
		t.Errorf("应按配置顺序落库，实得 %v / %v", accounts[0].ID, accounts[1].ID)
	}
	for _, account := range accounts {
		if id := account.ID.String(); id == "19900000001" || id == "19900000002" {
			t.Errorf("手机号原文**不得**成为账号主键（会把可识别信息带进所有下游表）: %s", id)
		}
	}

	first, err := store.GetAccount(context.Background(), scope, core.NewAccountID(acc1))
	if err != nil || first == nil {
		t.Fatalf("取账号失败: %v", err)
	}
	if first.Nickname == nil || *first.Nickname != "账号2" {
		t.Errorf("昵称应为「账号2」，实得 %v", first.Nickname)
	}
	if first.PhoneMasked == nil || *first.PhoneMasked != acc1 {
		t.Errorf("脱敏号应是 id 本身，实得 %v", first.PhoneMasked)
	}
	if first.SortOrder != 0 {
		t.Errorf("第一个账号的 sort_order 应为 0，实得 %d", first.SortOrder)
	}
	if !first.Enabled {
		t.Error("导入的账号应默认启用")
	}
	// 夹具的 expire 是 13 位毫秒（2030-01-01），不是 0，所以应当带过来
	if first.AuthExpire == nil || *first.AuthExpire != 1_893_456_000_000 {
		t.Errorf("auth_expire 应为 1893456000000，实得 %v", first.AuthExpire)
	}
}

func TestStateKeysAreRemappedThroughTheIdentityMap(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	runImport(t, store, fixtureSource())
	scope := core.ScopeUser(&uid)

	// state.json 的键是手机号原文；迁移后必须挂到掩码 id 上
	state, err := store.GetAccountState(context.Background(), scope, core.NewAccountID(acc1))
	if err != nil || state == nil {
		t.Fatalf("取登录态失败: %v", err)
	}
	if state.JWTToken == nil {
		t.Error("凭据应随迁移一起搬过去")
	}
	if state.DeviceID == nil {
		t.Error("deviceId 应随迁移一起搬过去")
	}
	if state.UserDomainID == nil {
		t.Error("userDomainId 是纯标识，有排障价值，应当保留")
	}
	if state.RedpackToken == nil {
		t.Error("redpack token 应随迁移一起搬过去")
	}
	// 旧 state.json 没有时间戳，"导入时间"看台账
	if state.UpdatedAt != nil {
		t.Errorf("旧数据没有时间戳，不该编一个，实得 %v", state.UpdatedAt)
	}

	// 反向：拿手机号原文当 id 必须查不到东西
	orphan, err := store.GetAccountState(context.Background(), scope, core.NewAccountID("19900000001"))
	if err != nil {
		t.Fatalf("取登录态失败: %v", err)
	}
	if orphan != nil {
		t.Error("手机号原文不该成为 id")
	}
	for _, account := range mustListAccounts(t, store, scope) {
		if account.ID.String() == "19900000001" {
			t.Error("手机号原文不该出现在账号表里")
		}
	}
}

func TestRunsAttachToAccountsByDisplayName(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	runImport(t, store, fixtureSource())
	scope := core.ScopeUser(&uid)

	runs := mustListRuns(t, store, scope)
	perAccount := map[string]int{}
	attached := 0
	done, failed := 0, 0
	for _, run := range runs {
		if run.AccountID != nil {
			perAccount[run.AccountID.String()]++
			attached++
		}
		switch run.Status {
		case core.StatusDone:
			done++
		case core.StatusFailed:
			failed++
		}
	}
	if perAccount[acc1] != 6 {
		t.Errorf("「账号2」应有 6 条历史，实得 %d", perAccount[acc1])
	}
	if perAccount[acc2] != 6 {
		t.Errorf("「账号1」应有 6 条历史，实得 %d", perAccount[acc2])
	}
	if attached != len(runs) {
		t.Errorf("夹具里每条历史的展示名都应能唯一解析，实得 %d/%d", attached, len(runs))
	}
	if done != 10 {
		t.Errorf("ok=true 应映射成 Done，实得 %d 条", done)
	}
	if failed != 2 {
		t.Errorf("ok=false 应映射成 Failed，实得 %d 条", failed)
	}
	// 失败记录的 error 原文要搬到 message，否则失败原因就丢了
	foundMessage := false
	for _, run := range runs {
		if run.Message != nil && contains(run.Message, "prizeIdOrName") {
			foundMessage = true
		}
	}
	if !foundMessage {
		t.Error("失败原因（message）应当搬过来")
	}
	// 旧数据从未采过云朵净增（原项目写入被注释），编不出来就留空
	for _, run := range runs {
		if run.CloudGained != nil || run.CloudAfter != nil {
			t.Errorf("云朵净增/余额编不出来就留空，实得 %v / %v", run.CloudGained, run.CloudAfter)
		}
	}
}

func TestPipelineKeepsStepOrderAndDefaultsToSequential(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	runImport(t, store, fixtureSource())
	scope := core.ScopeUser(&uid)

	pipelines := mustListPipelines(t, store, scope)
	if len(pipelines) != 1 {
		t.Fatalf("应有 1 条组合，实得 %d", len(pipelines))
	}
	pipeline := pipelines[0]
	if pipeline.Name != "轻量巡检" {
		t.Errorf("组合名应为「轻量巡检」，实得 %q", pipeline.Name)
	}
	if len(pipeline.Steps) != 2 ||
		pipeline.Steps[0].Task != "prizes" || pipeline.Steps[1].Task != "receive" {
		t.Errorf("步骤顺序即执行顺序，不能重排，实得 %v", pipeline.Steps)
	}
	if pipeline.AccountMode != core.ModeSequential {
		t.Errorf("未指定 accountMode 应落串行，实得 %v", pipeline.AccountMode)
	}
	if !pipeline.StopOnError {
		t.Error("旧版注释写明 stopOnError 默认 true")
	}
	if len(pipeline.AccountIDs) != 0 {
		t.Errorf("旧数据没指定 accounts = 全部账号，实得 %v", pipeline.AccountIDs)
	}
	if pipeline.LastDurationMS == nil || *pipeline.LastDurationMS != 2481 {
		t.Errorf("last_duration_ms 应为 2481，实得 %v", pipeline.LastDurationMS)
	}
	// 旧 id 是 UUID，应当原样保留而不是换成兜底 id
	if pipeline.ID != "84848ee7-3b94-483b-80fe-2ee33c93c7fe" {
		t.Errorf("旧 UUID 应原样保留，实得 %q", pipeline.ID)
	}
}

func TestExchangeWindowsAndWebuiPrefsRoundTrip(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	runImport(t, store, fixtureSource())
	_ = uid

	ctx := context.Background()
	exchange, err := store.ExchangeConfig(ctx)
	if err != nil {
		t.Fatalf("取抢兑配置: %v", err)
	}
	if len(exchange.Windows) != 2 || exchange.Windows[0] != "09:59:40" || exchange.Windows[1] != "11:59:40" {
		t.Errorf("窗口应为 [09:59:40 11:59:40]，实得 %v", exchange.Windows)
	}
	if exchange.Enabled {
		t.Error("旧数据没有绑定，启用没有意义")
	}
	if len(exchange.Bindings) != 0 {
		t.Errorf("不该有绑定，实得 %v", exchange.Bindings)
	}
	if exchange.UpdatedAt == nil {
		t.Error("updated_at 应当搬过来")
	}

	webui, err := store.WebuiConfig(ctx)
	if err != nil {
		t.Fatalf("取界面偏好: %v", err)
	}
	// 旧版两个布尔字段的缺省方向相反，这里校验它被归一化成了确定值
	if !webui.RememberLastPage {
		t.Error("rememberLastPage 应原样搬过来（true）")
	}
	if webui.LogExpanded {
		t.Error("logExpanded 缺省应为 false")
	}
	if webui.BackgroundRun {
		t.Error("backgroundRun 缺省应为 false")
	}
	if webui.LastPage != "tasks" {
		t.Errorf("last_page 应为 tasks，实得 %q", webui.LastPage)
	}
}

func contains(text *string, sub string) bool {
	if text == nil {
		return false
	}
	return len(sub) > 0 && indexOf(*text, sub) >= 0
}

func indexOf(text, sub string) int {
	for i := 0; i+len(sub) <= len(text); i++ {
		if text[i:i+len(sub)] == sub {
			return i
		}
	}
	return -1
}

// ─────────────────────────────────────────────────────────────────────────────
// 列表 helper（失败即 Fatal，让用例正文只留判据）
// ─────────────────────────────────────────────────────────────────────────────

func mustListAccounts(t *testing.T, store *SqliteStore, scope core.Scope) []core.Account {
	t.Helper()
	out, err := store.ListAccounts(context.Background(), scope)
	if err != nil {
		t.Fatalf("列账号: %v", err)
	}
	return out
}

func mustListRuns(t *testing.T, store *SqliteStore, scope core.Scope) []core.RunRecord {
	t.Helper()
	out, err := store.ListRuns(context.Background(), scope, 1000)
	if err != nil {
		t.Fatalf("列历史: %v", err)
	}
	return out
}

func mustListPipelines(t *testing.T, store *SqliteStore, scope core.Scope) []core.Pipeline {
	t.Helper()
	out, err := store.ListPipelines(context.Background(), scope)
	if err != nil {
		t.Fatalf("列组合: %v", err)
	}
	return out
}

// acc1Auth 是夹具里「账号2」那条 auth 的**原文**。
//
// ⚠ 断言必须比整串：v7 起 `Authorization` 是**原样整串**存的（见铁律 2），
// 任何"拆开再拼回去"的写法都会在这里被抓到 —— 拼出来的串与原文字节不同。
const acc1Auth = "Basic MjoxOTkwMDAwMDAwMTpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr"

// TestCredentialsAreBackfilledWholeAndOnlyOnce：
// 旧 `asign.json` 里的凭据要落到 `account_state`，而这件事有四条容易走偏的地方。
func TestCredentialsAreBackfilledWholeAndOnlyOnce(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)

	first := runImport(t, store, fixtureSource())
	if first.CredentialsFilled != 2 {
		t.Fatalf("首次导入应回填 2 个账号的凭据，实得 %d", first.CredentialsFilled)
	}

	ctx := context.Background()
	state, err := store.GetAccountState(ctx, scope, core.NewAccountID(acc1))
	if err != nil || state == nil {
		t.Fatalf("取登录态失败: %v", err)
	}
	if state.Authorization == nil {
		t.Fatal("Authorization 应被回填 —— 空着会让下游每次都当成未登录，" +
			"而『看起来像没登录』正是这个项目最不想要的错误方向")
	}
	if got := string(*state.Authorization); got != acc1Auth {
		t.Errorf("Authorization 应**整串**原样落库（不拆不拼），\n实得 %q\n期望 %q", got, acc1Auth)
	}
	if state.Phone == nil || string(*state.Phone) != "19900000001" {
		t.Errorf("手机号应一并回填，实得 %v", state.Phone)
	}

	// 第二次导入：凭据已经在库里了，报告必须说 0 ——
	// 每次都报 2 会让这个数字从第二次起失去意义（"回填了" ≠ "这次新填了"）
	again := runImport(t, store, fixtureSource())
	if again.CredentialsFilled != 0 {
		t.Errorf("凭据已存在时不该再报回填，实得 %d", again.CredentialsFilled)
	}
	// 但也不能被冲刷掉：read back 一次确认
	state2, err := store.GetAccountState(ctx, scope, core.NewAccountID(acc1))
	if err != nil || state2 == nil || state2.Authorization == nil ||
		string(*state2.Authorization) != acc1Auth {
		t.Errorf("二次导入不该动已有的凭据，实得 %v", state2)
	}
}
