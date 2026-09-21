package mcloudio

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// runImportErr 是 [runImport] 的"允许失败"版本：
// 安全失败那几条用例要检查**错误的具体类别**，而不是"有没有错"。
func runImportErr(t *testing.T, store *SqliteStore, source LegacySource) (MigrationReport, error) {
	t.Helper()
	uid := ensureUser(t, store)
	return ImportLegacy(context.Background(), store, core.ScopeUser(&uid), source, legacyNow())
}

func writeLegacyFile(t *testing.T, dir, name, body string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
		t.Fatalf("写 %s 失败: %v", name, err)
	}
}

func issueKinds(report MigrationReport) map[MigrationIssueKind]int {
	out := map[MigrationIssueKind]int{}
	for _, issue := range report.Issues {
		out[issue.Kind]++
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 幂等
// ─────────────────────────────────────────────────────────────────────────────

func TestSecondRunSkipsUnchangedFiles(t *testing.T) {
	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	runImport(t, store, fixtureSource())

	again := runImport(t, store, fixtureSource())
	if again.Accounts != 0 {
		t.Errorf("asign.json 没变，不重复写，实得 %d", again.Accounts)
	}
	if again.Runs != 0 {
		t.Errorf("history.json 没变，不重复写，实得 %d", again.Runs)
	}
	if again.Pipelines != 0 {
		t.Errorf("pipelines.json 没变，不重复写，实得 %d", again.Pipelines)
	}
	if again.Schedules != 0 {
		t.Errorf("空数组也不该被当成有新内容，实得 %d", again.Schedules)
	}
	if len(again.SkippedUnchanged) != len(LEGACY_DATA_FILES) {
		t.Errorf("6 个数据文件都应被跳过，实得 %v", again.SkippedUnchanged)
	}
	if again.HasIssues() {
		t.Errorf("跳过不是问题：%v", again.IssueLines())
	}

	// 库里的行数不变
	if got := len(mustListRuns(t, store, scope)); got != 12 {
		t.Errorf("库里仍应只有 12 条历史，实得 %d", got)
	}
	if got := len(mustListAccounts(t, store, scope)); got != 2 {
		t.Errorf("库里仍应只有 2 个账号，实得 %d", got)
	}
}

func TestReImportAfterTouchDoesNotDuplicateRows(t *testing.T) {
	dir := copyFixtures(t)
	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	runImport(t, store, sourceOf(dir))

	var before []string
	for _, run := range mustListRuns(t, store, scope) {
		before = append(before, run.ID)
	}

	// 改内容一个字节（末尾加换行）：sha256 变了 → 整份重跑。
	// 这一条验的才是真正的幂等：**台账失效之后**，UPSERT 主键仍然稳定。
	raw, err := os.ReadFile(filepath.Join(dir, "history.json"))
	if err != nil {
		t.Fatal(err)
	}
	writeLegacyFile(t, dir, "history.json", string(raw)+"\n")

	report := runImport(t, store, sourceOf(dir))
	if report.Runs != 12 {
		t.Errorf("重跑会重新解析 12 条，实得 %d", report.Runs)
	}
	for _, name := range report.SkippedUnchanged {
		if name == "history.json" {
			t.Error("history.json 变了，不该被跳过")
		}
	}

	var after []string
	for _, run := range mustListRuns(t, store, scope) {
		after = append(after, run.ID)
	}
	if len(after) != len(before) {
		t.Fatalf("行数不变，实得 %d → %d", len(before), len(after))
	}
	for i := range after {
		if after[i] != before[i] {
			t.Errorf("第 %d 条的主键变了：%s → %s", i, before[i], after[i])
		}
	}
	if got := len(mustListAccounts(t, store, scope)); got != 2 {
		t.Errorf("账号数不变，实得 %d", got)
	}
	if got := len(mustListPipelines(t, store, scope)); got != 1 {
		t.Errorf("组合数不变，实得 %d", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 安全失败
// ─────────────────────────────────────────────────────────────────────────────

func TestMaskCollisionAbortsBeforeWritingAnything(t *testing.T) {
	dir := copyFixtures(t)
	// 两个手机号前 2 后 2 相同 → 掩码都是 19****16 → 会塌缩成同一个 AccountID
	writeLegacyFile(t, dir, "asign.json", `{
  "caiyun": [
{ "auth": "Basic MjoxOTkwMDAwMDAxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr", "nickname": "甲" },
{ "auth": "Basic MjoxOTkwMDAwMDIxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr", "nickname": "乙" }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	_, err := runImportErr(t, store, sourceOf(dir))
	if err == nil {
		t.Fatal("掩码冲突必须让迁移失败")
	}
	var perr *LegacyImportError
	if !errors.As(err, &perr) || perr.Kind != LegacyImportIdentity {
		t.Fatalf("应当是身份映射失败，实得 %v", err)
	}

	// 关键：不是"导入了一半"，而是"什么都没动"
	if got := len(mustListAccounts(t, store, scope)); got != 0 {
		t.Errorf("中止时不该写入任何账号，实得 %d", got)
	}
	if got := len(mustListRuns(t, store, scope)); got != 0 {
		t.Errorf("中止时不该写入任何历史，实得 %d", got)
	}
	if got := len(mustListPipelines(t, store, scope)); got != 0 {
		t.Errorf("中止时不该写入任何组合，实得 %d", got)
	}
	ledger, err := store.ListImports(context.Background())
	if err != nil {
		t.Fatalf("列台账: %v", err)
	}
	if len(ledger) != 0 {
		t.Errorf("中止时也不该留下台账，否则重跑会被误判成已导入，实得 %d 条", len(ledger))
	}
}

func TestMissingFileIsReportedAndTheRestStillImports(t *testing.T) {
	dir := copyFixtures(t)
	if err := os.Remove(filepath.Join(dir, "schedules.json")); err != nil {
		t.Fatal(err)
	}
	store := openStore(t)
	report := runImport(t, store, sourceOf(dir))

	if report.Runs != 12 {
		t.Errorf("少一个文件不影响别的文件，实得 %d 条历史", report.Runs)
	}
	found := false
	for _, issue := range report.Issues {
		if issue.Kind == IssueMissingFile && strings.HasSuffix(issue.Path, "schedules.json") {
			found = true
		}
	}
	if !found {
		t.Errorf("缺失文件要报出来：%v", report.IssueLines())
	}
}

func TestCorruptJSONFailsLoudlyInsteadOfImportingHalfway(t *testing.T) {
	dir := copyFixtures(t)
	writeLegacyFile(t, dir, "history.json", "{ 这不是 JSON")

	store := openStore(t)
	_, err := runImportErr(t, store, sourceOf(dir))
	if err == nil {
		t.Fatal("整份 JSON 坏掉应当直接失败")
	}
	var perr *LegacyImportError
	if !errors.As(err, &perr) || perr.Kind != LegacyImportParse {
		t.Fatalf("应当是解析错误，实得 %v", err)
	}
}

func TestOneBadRunRecordDoesNotTakeDownTheRest(t *testing.T) {
	dir := copyFixtures(t)
	// 4 条记录，分别是：好的 / 类型错的 / 不是对象的 / 展示名认不出的
	writeLegacyFile(t, dir, "history.json", `{
  "runs": [
{ "task": "sign", "account": "账号1", "ok": true, "start": "2026-09-16T11:00:00Z", "end": "2026-09-16T11:00:01Z" },
{ "task": "sign", "account": "账号1", "ok": 1 },
"这不是对象",
{ "task": "sign", "account": "查无此人", "ok": true, "start": "2026-09-16T11:02:00Z", "end": "2026-09-16T11:02:01Z" }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	report := runImport(t, store, sourceOf(dir))

	// 能归属的那条照常落库并可读
	runs := mustListRuns(t, store, scope)
	if len(runs) != 1 {
		t.Fatalf("只有能归属的记录会出现在用户列表里，实得 %d", len(runs))
	}
	if runs[0].AccountID == nil || runs[0].AccountID.String() != acc2 {
		t.Errorf("「账号1」→ 19****02，实得 %v", runs[0].AccountID)
	}

	// 展示名认不出的那条**仍然落库**（历史不能因为归属解析失败就丢掉），
	// 只是它无法归属，所以不出现在任何用户的列表里 ——
	// 它该被看见的地方是迁移报告，不是某个人的"我的历史"。
	if report.Runs != 2 {
		t.Errorf("无归属的旧记录仍要保留，否则历史就断了，实得 %d 条", report.Runs)
	}

	lines := report.IssueLines()
	joined := strings.Join(lines, "\n")
	for _, want := range []string{"history.json 第 2 条", "第 3 条", "查无此人"} {
		if !strings.Contains(joined, want) {
			t.Errorf("下标要指得准，缺 %q：%v", want, lines)
		}
	}
}

func TestBadTimestampIsReportedAndSkipped(t *testing.T) {
	dir := copyFixtures(t)
	writeLegacyFile(t, dir, "history.json",
		`{ "runs": [ { "task": "sign", "account": "账号1", "ok": true, "start": "昨天下午" } ] }`)

	store := openStore(t)
	uid := ensureUser(t, store)
	report := runImport(t, store, sourceOf(dir))

	if report.Runs != 0 {
		t.Errorf("时间戳非法的那条应被跳过，实得 %d", report.Runs)
	}
	if issueKinds(report)[IssueBadRunTimestamp] == 0 {
		t.Errorf("应报出时间戳非法：%v", report.IssueLines())
	}
	if got := len(mustListRuns(t, store, core.ScopeUser(&uid))); got != 0 {
		t.Errorf("库里不该有这条，实得 %d", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 账号引用：绝不静默扩大范围
// ─────────────────────────────────────────────────────────────────────────────

func TestPhoneNumberReferenceIsRecoveredAndFlagged(t *testing.T) {
	dir := copyFixtures(t)
	// 旧版把纯数字串当序号 → 19900000001 越界 → 从未生效。
	// 但它显然是手机号，且能唯一匹配，所以要恢复 + 报出来让人确认。
	writeLegacyFile(t, dir, "schedules.json", `{
  "schedules": [
{ "id": "s1", "name": "早班签到", "task": "sign", "cron": "0 0 8 * * *",
  "account": "19900000001", "enabled": true }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	report := runImport(t, store, sourceOf(dir))
	if report.Schedules != 1 {
		t.Fatalf("应导入 1 条定时任务，实得 %d", report.Schedules)
	}

	schedules, err := store.ListSchedules(context.Background(), scope)
	if err != nil {
		t.Fatal(err)
	}
	schedule := schedules[0]
	if len(schedule.AccountIDs) != 1 || schedule.AccountIDs[0].String() != acc1 {
		t.Errorf("应恢复到正确的账号，实得 %v", schedule.AccountIDs)
	}
	if !schedule.Enabled {
		t.Error("恢复成功就不该停用它")
	}
	if schedule.Timezone != DefaultTZ {
		t.Errorf("旧版缺省是上海，实得 %q", schedule.Timezone)
	}
	if schedule.Target == nil || schedule.Target.Kind != core.TargetCapability || schedule.Target.Code != "sign" {
		t.Errorf("目标应是能力 sign，实得 %+v", schedule.Target)
	}
	if schedule.Cron != "0 0 8 * * *" {
		t.Errorf("cron 应原样保留，实得 %q", schedule.Cron)
	}
	if issueKinds(report)[IssueRecoveredAccountRef] == 0 {
		t.Errorf("恢复是行为变更，必须报出来：%v", report.IssueLines())
	}
}

func TestUnresolvableReferenceDisablesTheScheduleInsteadOfBroadeningIt(t *testing.T) {
	dir := copyFixtures(t)
	writeLegacyFile(t, dir, "schedules.json", `{
  "schedules": [
{ "id": "s1", "name": "幽灵账号的任务", "task": "sign", "cron": "0 0 8 * * *",
  "account": "早就删掉的账号", "enabled": true }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	report := runImport(t, store, sourceOf(dir))

	schedules, err := store.ListSchedules(context.Background(), scope)
	if err != nil {
		t.Fatal(err)
	}
	schedule := schedules[0]
	if schedule.Enabled {
		t.Error("解析不到账号必须停用 —— 回落到'全部账号'等于把'过去什么都没跑'变成'对所有人跑'")
	}
	if len(schedule.AccountIDs) != 0 {
		t.Errorf("原有账号引用不可用时不能凭空补一组账号，实得 %v", schedule.AccountIDs)
	}
	foundAction := false
	for _, issue := range report.Issues {
		if issue.Kind == IssueDeadAccountRef && issue.Action == "停用该定时任务" {
			foundAction = true
		}
	}
	if !foundAction {
		t.Errorf("应报出「停用该定时任务」：%v", report.IssueLines())
	}
}

func TestScheduleWithoutTargetOrCronIsImportedDisabled(t *testing.T) {
	dir := copyFixtures(t)
	writeLegacyFile(t, dir, "schedules.json", `{
  "schedules": [
{ "id": "s1", "name": "没有目标", "cron": "0 0 8 * * *", "enabled": true },
{ "id": "s2", "name": "没有 cron", "task": "sign", "cron": "  ", "enabled": true },
{ "id": "s3", "name": "默认关闭", "task": "sign", "cron": "0 0 8 * * *" }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	report := runImport(t, store, sourceOf(dir))
	if report.Schedules != 3 {
		t.Fatalf("三条都要留下，靠 enabled 表达处置，实得 %d", report.Schedules)
	}

	schedules, err := store.ListSchedules(context.Background(), scope)
	if err != nil {
		t.Fatal(err)
	}
	for _, schedule := range schedules {
		if schedule.Enabled {
			t.Errorf("%q 不该是启用的", schedule.Name)
		}
		if schedule.Name == "没有目标" && schedule.Target != nil {
			t.Errorf("没有目标就是没有目标，实得 %+v", schedule.Target)
		}
	}
	kinds := issueKinds(report)
	if kinds[IssueMissingTarget] == 0 {
		t.Errorf("应报出缺少目标：%v", report.IssueLines())
	}
	if kinds[IssueEmptyCron] == 0 {
		t.Errorf("应报出 cron 为空：%v", report.IssueLines())
	}
}

func TestPipelineBindingADeadAccountIsSkippedWhole(t *testing.T) {
	dir := copyFixtures(t)
	writeLegacyFile(t, dir, "pipelines.json", `{
  "pipelines": [
{ "id": "p1", "name": "好组合", "steps": [{ "task": "prizes" }] },
{ "id": "p2", "name": "坏组合", "steps": [{ "task": "prizes" }], "accounts": ["查无此人"] },
{ "id": "p3", "name": "空组合", "steps": [] },
{ "id": "p4", "name": "混合引用", "steps": [{ "task": "sign", "account": "账号1" }],
  "accounts": [2, "19900000001"], "accountMode": "parallel" }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	scope := core.ScopeUser(&uid)
	report := runImport(t, store, sourceOf(dir))

	if report.Pipelines != 2 {
		t.Fatalf("好组合与混合引用各一条，实得 %d", report.Pipelines)
	}
	var names []string
	for _, pipeline := range mustListPipelines(t, store, scope) {
		names = append(names, pipeline.Name)
	}
	if len(names) != 2 || names[0] != "好组合" || names[1] != "混合引用" {
		t.Errorf("坏组合与空组合都不入库，实得 %v", names)
	}

	mixed, err := store.GetPipeline(context.Background(), scope, "p4")
	if err != nil || mixed == nil {
		t.Fatalf("取 p4 失败: %v", err)
	}
	if len(mixed.AccountIDs) != 2 ||
		mixed.AccountIDs[0].String() != acc2 || mixed.AccountIDs[1].String() != acc1 {
		t.Errorf("序号 2 → 第 2 个账号；手机号 → 恢复成第 1 个账号，实得 %v", mixed.AccountIDs)
	}
	if mixed.AccountMode != core.ModeParallel {
		t.Errorf("accountMode 应为 parallel，实得 %v", mixed.AccountMode)
	}
	// 步骤级 account 已废弃，必须丢掉
	if mixed.Steps[0].Body != nil {
		t.Errorf("步骤级 account 已废弃，不该带过来，实得 %s", mixed.Steps[0].Body)
	}
	kinds := issueKinds(report)
	for _, kind := range []MigrationIssueKind{IssuePipelineSkipped, IssueEmptySteps, IssueDroppedStepAccount} {
		if kinds[kind] == 0 {
			t.Errorf("缺 %v：%v", kind, report.IssueLines())
		}
	}
}

func TestEmptyAccountListMeansAllAccounts(t *testing.T) {
	dir := copyFixtures(t)
	writeLegacyFile(t, dir, "pipelines.json", `{
  "pipelines": [
{ "id": "p1", "name": "空数组", "steps": [{ "task": "sign" }], "accounts": [] },
{ "id": "p2", "name": "全是空值", "steps": [{ "task": "sign" }], "accounts": [null, ""] }
  ]
}`)

	store := openStore(t)
	uid := ensureUser(t, store)
	report := runImport(t, store, sourceOf(dir))
	if report.Pipelines != 2 {
		t.Fatalf("两条都应导入，实得 %d", report.Pipelines)
	}
	for _, pipeline := range mustListPipelines(t, store, core.ScopeUser(&uid)) {
		if len(pipeline.AccountIDs) != 0 {
			t.Errorf("旧版 `if (accounts.length)` 为假 → 全部账号，实得 %v", pipeline.AccountIDs)
		}
	}
	if report.HasIssues() {
		t.Errorf("空列表是已知且正确的语义，不该报成问题：%v", report.IssueLines())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 预演与归档
// ─────────────────────────────────────────────────────────────────────────────

func TestDiagnoseRunsTheRealPathOnAThrowawayDatabase(t *testing.T) {
	report, err := DiagnoseLegacy(context.Background(), fixtureSource(), legacyNow())
	if err != nil {
		t.Fatalf("预演失败: %v", err)
	}
	// 走的是**同一条路径**，所以数字必须和真导入一模一样
	if report.Accounts != 2 || report.Runs != 12 || report.Pipelines != 1 {
		t.Errorf("预演与真导入必须同结果，实得 账号%d 历史%d 组合%d",
			report.Accounts, report.Runs, report.Pipelines)
	}
	if report.HasIssues() {
		t.Errorf("干净的旧数据不该有问题：%v", report.IssueLines())
	}
}

func TestArchiveCopiesEveryLegacyFileAndIsIdempotent(t *testing.T) {
	dest := t.TempDir()
	source := fixtureSource()

	copied, err := ArchiveLegacySource(source, dest)
	if err != nil {
		t.Fatalf("归档失败: %v", err)
	}
	if copied != len(LEGACY_DATA_FILES)+1 {
		t.Errorf("应复制 asign.json + 6 个数据文件，实得 %d", copied)
	}
	again, err := ArchiveLegacySource(source, dest)
	if err != nil {
		t.Fatalf("二次归档失败: %v", err)
	}
	if again != 0 {
		t.Errorf("内容相同的文件应被跳过，实得又复制了 %d 个", again)
	}
	// 归档是**原样**复制：一个字节都不许改
	for _, file := range source.ExistingFiles() {
		want, err := os.ReadFile(file.Path)
		if err != nil {
			t.Fatal(err)
		}
		got, err := os.ReadFile(filepath.Join(dest, file.Name))
		if err != nil {
			t.Fatalf("读归档 %s 失败: %v", file.Name, err)
		}
		if string(got) != string(want) {
			t.Errorf("%s 的归档内容与原件不同", file.Name)
		}
	}
}
