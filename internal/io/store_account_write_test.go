package mcloudio

import (
	"context"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// 账号的两条**写**接口：开关与删除。
//
// 这两条值得单独一组测试，不是因为它们复杂，而是因为它们的**失败形状**是静默的：
//   - 开关写成整行覆盖 → 只在"另一处刚改过昵称"时丢数据，平时全绿；
//   - 删除少删一张表 → 列表里那一行确实没了，凭据却还在库里能冒充登录。
//
// 所以每条断言都盯着"不该动的东西有没有被动"和"该一起没的东西有没有跟着没"。

// ── 启用开关 ────────────────────────────────────────────────────────────────

func TestTogglingEnabledOnlyTouchesThatColumn(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	putAccount(t, s, "acc-1")

	at := time.Date(2026, 9, 22, 8, 0, 0, 0, time.UTC)
	if err := s.SetAccountCloud(ctx, core.ScopeUser(uidPtr()),
		core.NewAccountID("acc-1"), 320, at); err != nil {
		t.Fatalf("写余额: %v", err)
	}

	if err := s.SetAccountEnabled(ctx, core.ScopeUser(uidPtr()),
		core.NewAccountID("acc-1"), false); err != nil {
		t.Fatalf("停用: %v", err)
	}

	got, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil || got == nil {
		t.Fatalf("取账号: %v / nil?", err)
	}
	if got.Enabled {
		t.Error("停用没生效：enabled 仍为 true")
	}
	// 反向改坏这里的方式：把 SetAccountEnabled 改成"读整行 → 改一列 → 写回去"
	// 或干脆换成 UPDATE 带上一堆列 —— 下面三条会立刻红。
	if got.Nickname == nil || *got.Nickname != "账号2" {
		t.Errorf("昵称被卷进了窄写：%v", got.Nickname)
	}
	if got.CloudCurrent == nil || *got.CloudCurrent != 320 {
		t.Errorf("余额被卷进了窄写：%v", got.CloudCurrent)
	}
	if got.CloudUpdatedAt == nil || !got.CloudUpdatedAt.Equal(at) {
		t.Errorf("余额时间被卷进了窄写：%v", got.CloudUpdatedAt)
	}

	// 再开回去：窄写要**双向**都只动那一列
	if err := s.SetAccountEnabled(ctx, core.ScopeUser(uidPtr()),
		core.NewAccountID("acc-1"), true); err != nil {
		t.Fatalf("启用: %v", err)
	}
	again, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil || again == nil {
		t.Fatalf("再取账号: %v / nil?", err)
	}
	if !again.Enabled {
		t.Error("启用没生效")
	}
}

func TestTogglingAnotherUsersAccountIsRejectedAndLeavesTheRowAlone(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	putAccount(t, s, "acc-1")

	other := core.NewUserID("other-user")
	err := s.SetAccountEnabled(ctx, core.ScopeUser(&other),
		core.NewAccountID("acc-1"), false)
	// 结论必须是"没有这个账号"，不是"你不能改"（后者把 id 空间变成可枚举的）
	assertKind(t, mustErr(t, err), core.KindNotFound)

	got, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil || got == nil {
		t.Fatalf("取账号: %v / nil?", err)
	}
	if !got.Enabled {
		t.Error("别人的停用调用把账号写进去了")
	}
}

// ── 删除 ────────────────────────────────────────────────────────────────────

// seedEverythingOn 给账号 accountID 挂上**四样**附属数据（凭据、余额快照、一次运行、
// 那次运行的事件），返回那条运行的 id。
//
// `runUUID` 必须是合法 uuid —— [core.ParseRunID] 只认这个形状，
// 而事件写入要的正是 RunID（测试里用固定串，为的是能按 id 数孤儿行）。
func seedEverythingOn(t *testing.T, s *SqliteStore, accountID, runUUID string) core.RunID {
	t.Helper()
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())
	now := time.Date(2026, 9, 22, 9, 0, 0, 0, time.UTC)

	token := core.SecretValue("jwt-" + accountID)
	authz := core.SecretValue("Basic cGM6" + accountID)
	at := now
	state := core.NewAccountState(accountID)
	state.JWTToken = &token
	state.Authorization = &authz
	state.UpdatedAt = &at
	if err := s.UpsertAccountState(ctx, scope, &state); err != nil {
		t.Fatalf("建登录态: %v", err)
	}

	if err := s.UpsertCloudStats(ctx, scope, &core.CloudStats{
		AccountID: core.NewAccountID(accountID), Date: "2026-09-22",
		CloudCount: 100, CloudDiff: i64Ptr(5), CloudDiffWeek: i64Ptr(20),
	}); err != nil {
		t.Fatalf("建云朵快照: %v", err)
	}

	putRun(t, s, runUUID, accountID)
	runID, ok := core.ParseRunID(runUUID)
	if !ok {
		t.Fatalf("测试用的 run id 必须是合法 uuid: %s", runUUID)
	}
	if err := s.AppendRunEvents(ctx, scope, runID, logs(runID, 3)); err != nil {
		t.Fatalf("建事件: %v", err)
	}
	return runID
}

const (
	runForAcc1 = "550e8400-e29b-41d4-a716-446655440101"
	runForAcc2 = "550e8400-e29b-41d4-a716-446655440102"
)

// countRows 直接数库里的行数，**绕开**类型化的读接口。
//
// # 为什么这里必须绕开 API
//
// `GetAccountState` / `CloudStatsSince` / `ListRuns` 全都带归属过滤
// （account_state 是 `JOIN accounts` 一条语句里做的，见 [SqliteStore.GetAccountState]）。
// 于是"账号行删了、凭据行留着"在 API 上表现为**三样都读不到** ——
// 拿 API 断言"删干净了"会恒真，而这恰恰是这次改动最不该假绿的一条。
// 反向验证过：把 `DELETE FROM account_state` / `cloud_stats` 两行删掉，
// 只有这些裸计数会红。
func countRows(t *testing.T, s *SqliteStore, query string, args ...any) int {
	t.Helper()
	var n int
	if err := s.db.QueryRowContext(context.Background(), query, args...).Scan(&n); err != nil {
		t.Fatalf("数 %s: %v", query, err)
	}
	return n
}

func TestDeletingAnAccountTakesItsCredentialsAndHistoryWithIt(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	putAccount(t, s, "acc-1")
	runID := seedEverythingOn(t, s, "acc-1", runForAcc1)

	// 先确认"删之前真的都有"：否则下面那些"都没了"是恒真断言
	if n := countRows(t, s, "SELECT count(*) FROM account_state WHERE account_id = ?", "acc-1"); n != 1 {
		t.Fatalf("前置条件：登录态应该有 1 行，实得 %d", n)
	}
	if n := countRows(t, s, "SELECT count(*) FROM run_events WHERE run_id = ?", runID.String()); n != 3 {
		t.Fatalf("前置条件：事件应该有 3 行，实得 %d", n)
	}

	if err := s.DeleteAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1")); err != nil {
		t.Fatalf("删除: %v", err)
	}

	scope := core.ScopeUser(uidPtr())
	id := core.NewAccountID("acc-1")
	if got, err := s.GetAccount(ctx, scope, id); err != nil || got != nil {
		t.Errorf("账号行还在：%v / %v", got, err)
	}

	// 凭据留在库里 = 任何拿到库文件的人仍能冒充那个手机号登录
	for _, probe := range []struct {
		table, what string
	}{
		{"account_state", "登录态（含凭据）"},
		{"cloud_stats", "云朵快照"},
		{"runs", "运行记录"},
	} {
		if n := countRows(t, s, "SELECT count(*) FROM "+probe.table+" WHERE account_id = ?", "acc-1"); n != 0 {
			t.Errorf("%s 没跟着删：%s 里留着 %d 行", probe.what, probe.table, n)
		}
	}
	if n := countRows(t, s, "SELECT count(*) FROM run_events WHERE run_id = ?", runID.String()); n != 0 {
		t.Errorf("事件没跟着删：run_events 里留着 %d 条孤儿行", n)
	}

	// run 行确实没了 → 事件续传那条查询给 NotFound 而不是空列表。
	// 两种结果都要断言：空列表意味着"run 还在、只是事件没了"，那不是这里的语义。
	_, runErr := s.RunEventsAfter(ctx, scope, runID, nil, 100)
	assertKind(t, mustErr(t, runErr), core.KindNotFound)
}

func TestDeletingAnAccountLeavesOtherAccountsRowsAlone(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	putAccount(t, s, "acc-1")
	putAccount(t, s, "acc-2")
	keepRunID := seedEverythingOn(t, s, "acc-2", runForAcc2)
	seedEverythingOn(t, s, "acc-1", runForAcc1)

	if err := s.DeleteAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1")); err != nil {
		t.Fatalf("删除: %v", err)
	}

	scope := core.ScopeUser(uidPtr())
	if got, err := s.GetAccount(ctx, scope, core.NewAccountID("acc-2")); err != nil || got == nil {
		t.Fatalf("别人的账号被牵连了：%v / %v", got, err)
	}
	if st, err := s.GetAccountState(ctx, scope, core.NewAccountID("acc-2")); err != nil || st == nil {
		t.Fatalf("别人的登录态被牵连了：%v / %v", st, err)
	}
	stats, err := s.CloudStatsSince(ctx, scope, core.NewAccountID("acc-2"), "2000-01-01")
	if err != nil || len(stats) != 1 {
		t.Fatalf("别人的云朵快照被牵连了：%v / %v", stats, err)
	}
	events, err := s.RunEventsAfter(ctx, scope, keepRunID, nil, 100)
	if err != nil || len(events) != 3 {
		t.Fatalf("别人的运行事件被牵连了：%v / %v", events, err)
	}
}

func TestDeletingAnotherUsersAccountIsRejected(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	putAccount(t, s, "acc-1")
	seedEverythingOn(t, s, "acc-1", runForAcc1)

	other := core.NewUserID("other-user")
	err := s.DeleteAccount(ctx, core.ScopeUser(&other), core.NewAccountID("acc-1"))
	assertKind(t, mustErr(t, err), core.KindNotFound)

	if got, _ := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1")); got == nil {
		t.Error("别人的删除调用把账号删了")
	}
	if st, _ := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1")); st == nil {
		t.Error("别人的删除调用把凭据删了")
	}
}

func TestDeletingAnUnknownAccountIsNotFound(t *testing.T) {
	s := storeWithUser(t)
	err := s.DeleteAccount(context.Background(), core.ScopeUser(uidPtr()),
		core.NewAccountID("199****0999"))
	assertKind(t, mustErr(t, err), core.KindNotFound)
}

// TestDeletingAnAccountLeavesScheduleTargetsAlone 钉住一个**刻意**的不作为。
//
// `schedules.account_ids` 的"空数组 = 全部账号"意味着：删掉数组里最后一个 id
// 会把那条任务从"只跑这一个账号"静默变成"跑全部账号"。
// 所以这里断言的是"**没被改**"—— 哪天有人来"顺手清理悬空 id"，这条会红，
// 逼他先读到上面那段理由，而不是在 review 里凭印象判断。
func TestDeletingAnAccountLeavesScheduleTargetsAlone(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())
	putAccount(t, s, "acc-1")

	schedule := core.NewSchedule("sch-1", "只跑 acc-1", "0 9 * * *", "Asia/Shanghai")
	owner := uid()
	schedule.Owner = &owner
	schedule.AccountIDs = []core.AccountID{core.NewAccountID("acc-1")}
	if err := s.UpsertSchedule(ctx, scope, &schedule); err != nil {
		t.Fatalf("建定时任务: %v", err)
	}

	if err := s.DeleteAccount(ctx, scope, core.NewAccountID("acc-1")); err != nil {
		t.Fatalf("删除: %v", err)
	}

	got, err := s.GetSchedule(ctx, scope, "sch-1")
	if err != nil || got == nil {
		t.Fatalf("取定时任务: %v / nil?", err)
	}
	if len(got.AccountIDs) != 1 || got.AccountIDs[0].String() != "acc-1" {
		t.Errorf("删账号时动了任务的目标（悬空 id 要原样留着）：%v", got.AccountIDs)
	}
}
