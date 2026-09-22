package mcloudio

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// **数据隔离**的集中验收：两个用户各一个账号，把所有入口交叉摸一遍。
//
// 隔离规则是逐方法写在 SQL 里的，这组测试是唯一把它们串起来的地方 ——
// 漏掉任何一处（"忘了加 `owner` 条件"）都会在这里被抓住，
// 而漏掉一处不会有任何编译期提示（这正是 `../mcloud-rust/docs/rust-rewrite-plan.md` §3.7 的要点）。

// newUser 建一个用户并落库。
func newUser(t *testing.T, s *SqliteStore, id string) core.UserID {
	t.Helper()
	u := core.NewUser(id, id, core.PasswordHash("!"), core.RoleUser, time.Now().UTC())
	if err := s.UpsertUser(context.Background(), &u); err != nil {
		t.Fatalf("建用户 %s: %v", id, err)
	}
	return u.ID
}

func TestCrossUserIsolationIsEnforcedOnEveryEntryPoint(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	alice := uid()
	bob := newUser(t, s, "bob")

	aliceAccount := account("acc-a")
	aliceAccount.Owner = &alice
	if err := s.UpsertAccount(ctx, core.ScopeUser(&alice), &aliceAccount); err != nil {
		t.Fatalf("建 alice 的账号: %v", err)
	}
	bobAccount := account("acc-b")
	bobAccount.Owner = &bob
	if err := s.UpsertAccount(ctx, core.ScopeUser(&bob), &bobAccount); err != nil {
		t.Fatalf("建 bob 的账号: %v", err)
	}

	// ① 列表与单项读取：只看得见自己的
	visibleToAlice, err := s.ListAccounts(ctx, core.ScopeUser(&alice))
	if err != nil {
		t.Fatalf("alice 列表: %v", err)
	}
	if len(visibleToAlice) != 1 || visibleToAlice[0].ID.String() != "acc-a" {
		t.Fatalf("alice 只该看见 acc-a，实得 %v", visibleToAlice)
	}
	visibleToBob, err := s.ListAccounts(ctx, core.ScopeUser(&bob))
	if err != nil {
		t.Fatalf("bob 列表: %v", err)
	}
	if len(visibleToBob) != 1 {
		t.Fatalf("bob 只该看见自己的 1 个，实得 %d", len(visibleToBob))
	}
	crossed, err := s.GetAccount(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("alice 读 acc-b: %v", err)
	}
	if crossed != nil {
		t.Fatal("alice 不该读到 bob 的账号")
	}
	crossed, err = s.GetAccount(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-a"))
	if err != nil {
		t.Fatalf("bob 读 acc-a: %v", err)
	}
	if crossed != nil {
		t.Fatal("bob 不该读到 alice 的账号")
	}

	// ② 登录态：读不到，也写不进
	bobState := core.NewAccountState("acc-b")
	assertKind(t, mustErr(t, s.UpsertAccountState(ctx, core.ScopeUser(&alice), &bobState)), core.KindNotFound)
	if err := s.UpsertAccountState(ctx, core.ScopeUser(&bob), &bobState); err != nil {
		t.Fatalf("bob 写自己的登录态: %v", err)
	}
	stateForAlice, err := s.GetAccountState(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("alice 读 acc-b 的登录态: %v", err)
	}
	if stateForAlice != nil {
		t.Fatal("alice 不该读到 bob 账号的登录态")
	}
	stateForBob, err := s.GetAccountState(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("bob 读自己的登录态: %v", err)
	}
	if stateForBob == nil {
		t.Fatal("bob 应当读得到自己的登录态")
	}

	// ②b smid 两列：同理。这里的后果比登录态更隐蔽 ——
	//      alice 若写得进 bob 的账号，就是替 bob 的账号凭空认下一台设备
	assertKind(t, mustErr(t,
		s.SetSmidFingerprint(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"), fpV1)),
		core.KindNotFound)
	assertKind(t, mustErr(t,
		s.SetSmidDeviceID(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"), deviceID1)),
		core.KindNotFound)
	if err := s.SetSmidFingerprint(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-b"), fpV1); err != nil {
		t.Fatalf("bob 写自己的 smid: %v", err)
	}
	smidForAlice, err := s.GetSmidState(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("alice 读 smid: %v", err)
	}
	if smidForAlice != nil {
		t.Fatal("alice 不该读到 bob 账号的 smid")
	}
	smidForBob, err := s.GetSmidState(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("bob 读 smid: %v", err)
	}
	if smidForBob == nil {
		t.Fatal("bob 应当读得到自己的 smid")
	}

	// ③ 云朵快照：同理
	stats := core.NextCloudStats(core.NewAccountID("acc-b"), "2026-09-15", 100, nil)
	assertKind(t, mustErr(t, s.UpsertCloudStats(ctx, core.ScopeUser(&alice), &stats)), core.KindNotFound)
	if err := s.UpsertCloudStats(ctx, core.ScopeUser(&bob), &stats); err != nil {
		t.Fatalf("bob 写自己的快照: %v", err)
	}
	statsForAlice, err := s.CloudStatsSince(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"), "2026-01-01")
	if err != nil {
		t.Fatalf("alice 读快照: %v", err)
	}
	if len(statsForAlice) != 0 {
		t.Fatal("alice 不该读到 bob 账号的快照")
	}
	statsForBob, err := s.CloudStatsSince(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-b"), "2026-01-01")
	if err != nil {
		t.Fatalf("bob 读快照: %v", err)
	}
	if len(statsForBob) != 1 {
		t.Fatalf("bob 应当读到自己的 1 条快照，实得 %d", len(statsForBob))
	}

	// ④ 运行记录：写、读、改三条路都要挡住
	run := runningRun("run-b", "acc-b")
	run.Status = core.StatusDone
	assertKind(t, mustErr(t, s.InsertRun(ctx, core.ScopeUser(&alice), &run)),
		core.KindNotFound) // 不能往别人的账号上写运行记录
	if err := s.InsertRun(ctx, core.ScopeUser(&bob), &run); err != nil {
		t.Fatalf("bob 写自己的运行记录: %v", err)
	}
	runsForAlice, err := s.ListRuns(ctx, core.ScopeUser(&alice), 100)
	if err != nil {
		t.Fatalf("alice 读运行记录: %v", err)
	}
	if len(runsForAlice) != 0 {
		t.Fatal("alice 不该看到 bob 的运行记录")
	}
	runsForBob, err := s.ListRuns(ctx, core.ScopeUser(&bob), 100)
	if err != nil {
		t.Fatalf("bob 读运行记录: %v", err)
	}
	if len(runsForBob) != 1 {
		t.Fatalf("bob 应当看到自己的 1 条，实得 %d", len(runsForBob))
	}
	assertKind(t, mustErr(t, s.UpdateRun(ctx, core.ScopeUser(&alice), &run)),
		core.KindNotFound) // 不能改别人的运行记录
	if err := s.UpdateRun(ctx, core.ScopeUser(&bob), &run); err != nil {
		t.Fatalf("bob 改自己的运行记录: %v", err)
	}

	// ⑤ 不能替别人建账号（owner 与调用者不一致就直接拒）
	forged := account("acc-a2")
	forged.Owner = &bob
	assertKind(t, mustErr(t, s.UpsertAccount(ctx, core.ScopeUser(&alice), &forged)), core.KindNotFound)

	// ⑥ 同一个 AccountId 被他人占用时不能覆盖：给出可行动的提示，
	//    而不是静默把对方的账号顶掉（AccountId 是脱敏手机号派生的，会撞）
	duplicated := account("acc-b")
	duplicated.Owner = &alice
	err = s.UpsertAccount(ctx, core.ScopeUser(&alice), &duplicated)
	assertKind(t, mustErr(t, err), core.KindBadRequest)
	if !strings.Contains(err.Error(), "已被其他用户添加") {
		t.Fatalf("错误信息应可行动，实得: %v", err)
	}
	kept, err := s.GetAccount(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("bob 读自己的账号: %v", err)
	}
	if kept == nil {
		t.Fatal("被拒之后原账号必须原样还在")
	}
	if kept.Nickname == nil || *kept.Nickname != "账号2" {
		t.Fatalf("被拒之后原账号必须原样还在，昵称实得 %v", kept.Nickname)
	}
}

// TestLocalScopeSeesAndWritesEverything：**本地范围**（CLI）看得见所有人的账号，
// 也能写任意账号的数据。
//
// 这条与上面那条隔离测试是**一对**：隔离管的是"网络上的用户"，
// 而拿着库文件的人本来就能操作所有 —— 两条都得成立，边界才说得清。
func TestLocalScopeSeesAndWritesEverything(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	alice := uid()
	bob := newUser(t, s, "bob")

	a := account("acc-a")
	a.Owner = &alice
	if err := s.UpsertAccount(ctx, core.ScopeUser(&alice), &a); err != nil {
		t.Fatalf("建 acc-a: %v", err)
	}
	b := account("acc-b")
	b.Owner = &bob
	if err := s.UpsertAccount(ctx, core.ScopeUser(&bob), &b); err != nil {
		t.Fatalf("建 acc-b: %v", err)
	}

	// ① 列表：两个都看得见（无论归谁）
	all, err := s.ListAccounts(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("本地列表: %v", err)
	}
	if len(all) != 2 {
		t.Fatalf("本地视角必须看得见所有人的账号，实得 %d", len(all))
	}

	// ② 单项：别人的账号也读得到（用户视角下这里是 nil）
	localView, err := s.GetAccount(ctx, core.ScopeLocal(), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("本地读 acc-b: %v", err)
	}
	if localView == nil {
		t.Fatal("本地视角应当读得到别人的账号")
	}
	userView, err := s.GetAccount(ctx, core.ScopeUser(&alice), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("alice 读 acc-b: %v", err)
	}
	if userView != nil {
		t.Fatal("用户视角不该读到别人的账号")
	}

	// ③ 写：本地视角可以写别人账号的登录态与快照
	state := core.NewAccountState("acc-b")
	if err := s.UpsertAccountState(ctx, core.ScopeLocal(), &state); err != nil {
		t.Fatalf("本地写登录态: %v", err)
	}
	localState, err := s.GetAccountState(ctx, core.ScopeLocal(), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("本地读登录态: %v", err)
	}
	if localState == nil {
		t.Fatal("本地视角应当读得到刚写的登录态")
	}
	stats := core.NextCloudStats(core.NewAccountID("acc-b"), "2026-09-15", 7, nil)
	if err := s.UpsertCloudStats(ctx, core.ScopeLocal(), &stats); err != nil {
		t.Fatalf("本地写快照: %v", err)
	}
	localStats, err := s.CloudStatsSince(ctx, core.ScopeLocal(), core.NewAccountID("acc-b"), "2026-01-01")
	if err != nil {
		t.Fatalf("本地读快照: %v", err)
	}
	if len(localStats) != 1 {
		t.Fatalf("本地应读到 1 条快照，实得 %d", len(localStats))
	}

	// ③b smid 两列：同理。CLI 侧的 `mcloud smid` 就是本地范围 ——
	//      拿着库文件的人本来就能改所有账号的设备长相，这与隔离不矛盾
	if err := s.SetSmidFingerprint(ctx, core.ScopeLocal(), core.NewAccountID("acc-b"), fpV1); err != nil {
		t.Fatalf("本地写指纹: %v", err)
	}
	if err := s.SetSmidDeviceID(ctx, core.ScopeLocal(), core.NewAccountID("acc-b"), deviceID1); err != nil {
		t.Fatalf("本地写 deviceId: %v", err)
	}
	smid, err := s.GetSmidState(ctx, core.ScopeLocal(), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("本地读 smid: %v", err)
	}
	if smid == nil {
		t.Fatal("本地范围读得到别人的两列")
	}
	if smid.FingerprintJSON == nil || smid.FingerprintJSON.Value() != fpV1 {
		t.Fatalf("指纹不符: %v", smid.FingerprintJSON)
	}
	if smid.DeviceID == nil || smid.DeviceID.Value() != deviceID1 {
		t.Fatalf("deviceId 不符: %v", smid.DeviceID)
	}
}

// TestLocalScopeCanCreateUnownedAccountsButUserScopeCannot：
// 本地范围能写**无主**账号 —— 这是"先 migrate / 先 login 再 init"能成立的关键。
//
// 用户范围下这条是被拒的（见 [SqliteStore.UpsertAccount] 的说明）：
// 无主账号对谁都不可见，不该由"某个用户"随手造出来。
func TestLocalScopeCanCreateUnownedAccountsButUserScopeCannot(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	alice := uid()

	orphan := core.NewAccount("acc-orphan") // Owner: nil
	if err := s.UpsertAccount(ctx, core.ScopeLocal(), &orphan); err != nil {
		t.Fatalf("本地建无主账号: %v", err)
	}

	// 无主账号对任何人都不可见，但本地视角看得见
	visible, err := s.ListAccounts(ctx, core.ScopeUser(&alice))
	if err != nil {
		t.Fatalf("alice 列表: %v", err)
	}
	if len(visible) != 0 {
		t.Fatalf("无主账号不该出现在用户的列表里，实得 %d", len(visible))
	}
	all, err := s.ListAccounts(ctx, core.ScopeLocal())
	if err != nil {
		t.Fatalf("本地列表: %v", err)
	}
	if len(all) != 1 {
		t.Fatalf("本地应看得见无主账号，实得 %d", len(all))
	}

	// 用户范围下造无主账号必须被拒
	another := core.NewAccount("acc-2")
	assertKind(t, mustErr(t, s.UpsertAccount(ctx, core.ScopeUser(&alice), &another)), core.KindNotFound)

	// 认领之后它才进入 alice 的视野
	claimed, err := s.ClaimUnowned(ctx, alice)
	if err != nil {
		t.Fatalf("认领: %v", err)
	}
	if claimed.Accounts != 1 {
		t.Fatalf("应认领 1 个账号，实得 %d", claimed.Accounts)
	}
	visible, err = s.ListAccounts(ctx, core.ScopeUser(&alice))
	if err != nil {
		t.Fatalf("alice 列表: %v", err)
	}
	if len(visible) != 1 {
		t.Fatalf("认领后 alice 应看得见 1 个，实得 %d", len(visible))
	}
}

// TestLocalScopeMayRewriteSomeoneElsesAccount：本地范围下可以覆盖别人的账号
// （运维语义），用户范围下不行。
func TestLocalScopeMayRewriteSomeoneElsesAccount(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	alice := uid()
	bob := newUser(t, s, "bob")

	b := account("acc-b")
	b.Owner = &bob
	if err := s.UpsertAccount(ctx, core.ScopeUser(&bob), &b); err != nil {
		t.Fatalf("建 acc-b: %v", err)
	}

	// 用户范围：被占用 → 拒绝（这条早已有用例，这里把它和本地范围并排钉住）
	stolen := b
	stolen.Owner = &alice
	stolen.Nickname = strPtr("顶掉")
	assertKind(t, mustErr(t, s.UpsertAccount(ctx, core.ScopeUser(&alice), &stolen)), core.KindBadRequest)

	// 本地范围：允许改写（调用方本来就拿着库文件）
	refreshed := b
	refreshed.Nickname = strPtr("运维刷新")
	if err := s.UpsertAccount(ctx, core.ScopeLocal(), &refreshed); err != nil {
		t.Fatalf("本地改写: %v", err)
	}
	back, err := s.GetAccount(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-b"))
	if err != nil {
		t.Fatalf("bob 读账号: %v", err)
	}
	if back == nil || back.Nickname == nil || *back.Nickname != "运维刷新" {
		t.Fatalf("本地改写应当生效，实得 %v", back)
	}
}

// TestLocalScopeListsOrphanRuns：本地范围看得到"匹配不到账号"的历史运行记录
// （它不属于任何人）。
func TestLocalScopeListsOrphanRuns(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	run := core.RunRecord{
		ID:         "run-orphan",
		AccountID:  nil,
		Capability: "sign",
		Status:     core.StatusDone,
		StartedAt:  time.Now().UTC(),
	}
	if err := s.InsertRun(ctx, core.ScopeLocal(), &run); err != nil {
		t.Fatalf("本地写无主运行记录: %v", err)
	}

	localRuns, err := s.ListRuns(ctx, core.ScopeLocal(), 10)
	if err != nil {
		t.Fatalf("本地列表: %v", err)
	}
	if len(localRuns) != 1 {
		t.Fatalf("本地应看到 1 条，实得 %d", len(localRuns))
	}
	userRuns, err := s.ListRuns(ctx, core.ScopeUser(uidPtr()), 10)
	if err != nil {
		t.Fatalf("用户列表: %v", err)
	}
	if len(userRuns) != 0 {
		t.Fatal("无归属的运行记录不该冒充某个人的历史")
	}
}

// TestUnownedAccountsAreClaimedExactlyOnce 覆盖认领的三条不变式：
// 未归属时谁都看不见、认领幂等、已归属的不能被抢走。
func TestUnownedAccountsAreClaimedExactlyOnce(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)

	// 模拟"从 v2 升上来、还没初始化管理员"的库：owner 为 NULL 的行。
	//
	// 这里**直接用 SQL 写**而不是走 UpsertAccount：后者现在会拒绝无主写入
	// （无主状态只该由 v3 迁移产生）。测试要模拟的正是迁移留下的那个状态，
	// 所以就得按迁移的方式写 —— 用 API 反而是在测一件不存在的事。
	for _, id := range []string{"19****01", "19****02"} {
		mustExec(t, s.db, "INSERT INTO accounts (id, enabled, sort_order, fail_streak) VALUES ('"+id+"', 1, 0, 0)")
	}

	admin := uid()
	other := newUser(t, s, "bob")

	// 未归属时**谁都看不见**：宁可账号暂时不出现，也不要出现"谁都拿得到"
	visible, err := s.ListAccounts(ctx, core.ScopeUser(&admin))
	if err != nil {
		t.Fatalf("admin 列表: %v", err)
	}
	for _, a := range visible {
		if a.Owner != nil {
			t.Fatalf("这个夹具里不该有已归属账号: %v", a.Owner)
		}
	}
	if len(visible) != 0 {
		t.Fatal("未归属的账号不该出现在任何人的列表里")
	}

	claimed, err := s.ClaimUnowned(ctx, admin)
	if err != nil {
		t.Fatalf("认领: %v", err)
	}
	if claimed.Accounts != 2 {
		t.Fatalf("应认领 2 个，实得 %d", claimed.Accounts)
	}
	if claimed.Schedules != 0 || claimed.Pipelines != 0 {
		t.Fatalf("这个夹具里只有账号；定时任务与组合由 config 的用例覆盖，实得 %+v", claimed)
	}

	again, err := s.ClaimUnowned(ctx, admin)
	if err != nil {
		t.Fatalf("重复认领: %v", err)
	}
	if again.Accounts != 0 {
		t.Fatal("认领必须幂等")
	}
	stolen, err := s.ClaimUnowned(ctx, other)
	if err != nil {
		t.Fatalf("别人认领: %v", err)
	}
	if stolen.Accounts != 0 {
		t.Fatal("已归属的账号不能被别人抢走")
	}

	visible, err = s.ListAccounts(ctx, core.ScopeUser(&admin))
	if err != nil {
		t.Fatalf("admin 列表: %v", err)
	}
	if len(visible) != 2 {
		t.Fatalf("认领后应看得见 2 个，实得 %d", len(visible))
	}
	for _, a := range visible {
		if a.Owner == nil || a.Owner.String() != admin.String() {
			t.Fatalf("归属应被认领给 admin，实得 %v", a.Owner)
		}
	}
	otherVisible, err := s.ListAccounts(ctx, core.ScopeUser(&other))
	if err != nil {
		t.Fatalf("bob 列表: %v", err)
	}
	if len(otherVisible) != 0 {
		t.Fatal("交叉可见就是越权")
	}
}
