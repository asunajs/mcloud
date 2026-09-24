package mcloudio

import (
	"context"
	"path/filepath"
	"strconv"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// 存储层单元测试的公共 helper；主题用例见 store_isolation_test.go /
// store_run_events_test.go / userstore_test.go。
//
// ⚠ 这里**没有**任何"手工建表"的代码：所有测试都走 [Open] / [OpenInMemory]，
// 也就是生产同一条路径（打开即迁移）。手工建表会让测试与迁移链分叉，
// 而分叉那天表现为"测试全绿、真库打不开"。

// testUser 是测试用户。
//
// v3 起账号必须有主人（`accounts.owner` 有外键指向 `users`，隔离也靠它过滤），
// 所以碰账号的用例都先要有一个用户 —— 这正是生产的顺序（先 `user init` 再导入）。
const testUser = "test-user"

func uid() core.UserID { return core.NewUserID(testUser) }

func store(t *testing.T) *SqliteStore {
	t.Helper()
	s, err := OpenInMemory()
	if err != nil {
		t.Fatalf("内存库应可打开: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

// storeWithUser 建好测试用户的库。
func storeWithUser(t *testing.T) *SqliteStore {
	t.Helper()
	s := store(t)
	u := core.NewUser(testUser, testUser, core.PasswordHash("!"), core.RoleAdmin, time.Now().UTC())
	if err := s.UpsertUser(context.Background(), &u); err != nil {
		t.Fatalf("建测试用户: %v", err)
	}
	return s
}

func account(id string) core.Account {
	a := core.NewAccount(id)
	nickname := "账号2"
	masked := "199****0001"
	owner := uid()
	a.Nickname = &nickname
	a.PhoneMasked = &masked
	// 统一归给测试用户；不设的话写入会被隔离校验拒掉
	a.Owner = &owner
	return a
}

// putAccount 建一个已归属的账号行。
//
// 写云朵快照 / 运行记录之前必须有这一行：隔离校验靠 `accounts.owner`，
// 账号不存在时那些写入会被正确地拒掉（这正是它该有的行为）。
func putAccount(t *testing.T, s *SqliteStore, id string) {
	t.Helper()
	a := account(id)
	if err := s.UpsertAccount(context.Background(), core.ScopeUser(uidPtr()), &a); err != nil {
		t.Fatalf("建账号 %s: %v", id, err)
	}
}

func uidPtr() *core.UserID { u := uid(); return &u }

// ── smid 两列 ───────────────────────────────────────────────────────────────
//
// 这一组的分量比看上去重：`SmidState` 与 `AccountState` 的写入契约**相反**
// （前者窄写、后者整体覆盖），而这个相反只写在注释和 SQL 里，编译器看不见。
// 下面几条就是它唯一的执行力来源 —— 尤其
// `TestUpsertAccountStateLeavesTheSmidColumnsAlone`，schema.go 的 v5 注释直接点名了它。

// fpV1 / fpV2 是探针用的设备特征文本。存储层**从不解析**它
// （`SmidState` 的"这一层只搬文本"），所以随手给一段任意 JSON 就是合法的
// —— 这本身也是那条契约的断言之一。
const (
	fpV1 = `{"ua":"UA-v1","canvas":"aaaa1111"}`
	fpV2 = `{"ua":"UA-v2","canvas":"bbbb2222"}`
)

// deviceID1 是合成 deviceId（`199` 段，见脱敏约定）。
const deviceID1 = "B1990000000000001"

// authV1 是合成整串 `Authorization`（`199` 段手机号，见脱敏约定）。
//
// 形状与真实值一致：`Basic base64("<前缀>:<手机号>:<token>")`。
// 这一列存的是**原文**，所以测试里也照原文比 —— 不做任何解析。
const authV1 = "Basic MjoxOTkwMDAwMDAwMTphdXRoLXYx"

// authV2 是另一个合成整串，专用来验证"回填不覆盖已有值"。
const authV2 = "Basic MjoxOTkwMDAwMDAwMjphdXRoLXYy"

// ── 断言小工具 ──────────────────────────────────────────────────────────────

// assertKind 断言 err 是 [core.Error] 且类别为 want。
//
// ⚠ 用 [core.KindOf] 而不是 `errors.As` + 手写比较：`*core.Error` 同时实现
// `error` 与 `Unwrap`，直接用 `errors.As` 会顺着链把包装层也解出来 ——
// 那是"想看到最外层类别"时不该有的行为。
func assertKind(t *testing.T, err error, want core.ErrorKind) {
	t.Helper()
	if err == nil {
		t.Fatalf("期望错误 %s，实得 nil", want)
	}
	got, ok := core.KindOf(err)
	if !ok {
		t.Fatalf("期望错误 %s，实得非 core.Error: %v", want, err)
	}
	if got != want {
		t.Fatalf("期望错误 %s，实得 %s: %v", want, got, err)
	}
}

func mustErr(t *testing.T, err error) error {
	t.Helper()
	if err == nil {
		t.Fatal("期望报错，实得 nil")
	}
	return err
}

func strPtr(s string) *string { return &s }

func i64Ptr(v int64) *int64 { return &v }

// nullText 把可选增量打印成"看得懂是哪一种空"。
//
// 测试里的失败分支**不能**解引用指针：那样"该为 NULL 却不是"会先 panic 再报错，
// 而 panic 的现场没有断言信息可读。
func nullText(p *int64) string {
	if p == nil {
		return "NULL"
	}
	return strconv.FormatInt(*p, 10)
}

func secretPtr(s string) *core.SecretValue { v := core.NewSecretValue(s); return &v }

func phonePtr(s string) *core.FullPhone { v := core.FullPhone(s); return &v }

func domainPtr(s string) *core.UserDomainID { v := core.UserDomainID(s); return &v }

func timePtr(t time.Time) *time.Time { return &t }

// at 解析一个 RFC 3339 时刻，解析失败直接失败。
func at(t *testing.T, rfc3339 string) time.Time {
	t.Helper()
	parsed, err := time.Parse(time.RFC3339, rfc3339)
	if err != nil {
		t.Fatalf("测试用的时刻必须合法: %v", err)
	}
	return parsed.UTC()
}

// ── accounts ────────────────────────────────────────────────────────────────

func TestSchemaVersionIsRecorded(t *testing.T) {
	s := store(t)
	v, err := s.SchemaVersion(context.Background())
	if err != nil {
		t.Fatalf("读 schema 版本: %v", err)
	}
	if v != SchemaVersion {
		t.Fatalf("库里记的版本应是 %d，实得 %d", SchemaVersion, v)
	}
}

func TestAccountRoundtrip(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	acc := account("acc-1")
	acc.CloudCurrent = i64Ptr(1286)
	acc.AuthExpire = i64Ptr(1_893_456_000_000)
	acc.Note = strPtr("云图书馆阅读月卡")

	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("写账号: %v", err)
	}

	loaded, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil {
		t.Fatalf("读账号: %v", err)
	}
	if loaded == nil {
		t.Fatal("刚写的账号应当读得到")
	}
	if loaded.Nickname == nil || *loaded.Nickname != "账号2" {
		t.Fatalf("昵称不符: %v", loaded.Nickname)
	}
	if loaded.CloudCurrent == nil || *loaded.CloudCurrent != 1286 {
		t.Fatalf("云朵数不符: %v", loaded.CloudCurrent)
	}
	if loaded.AuthExpire == nil || *loaded.AuthExpire != 1_893_456_000_000 {
		t.Fatalf("凭据到期不符: %v", loaded.AuthExpire)
	}
	if loaded.Note == nil || *loaded.Note != "云图书馆阅读月卡" {
		t.Fatalf("备注不符: %v", loaded.Note)
	}
	if !loaded.Enabled {
		t.Fatal("新账号应默认启用")
	}
}

func TestUpsertAccountUpdatesInPlace(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	acc := account("acc-1")
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("首次写入: %v", err)
	}

	updated := acc
	updated.CloudCurrent = i64Ptr(1300)
	updated.FailStreak = 2
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &updated); err != nil {
		t.Fatalf("二次写入: %v", err)
	}

	all, err := s.ListAccounts(ctx, core.ScopeUser(uidPtr()))
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(all) != 1 {
		t.Fatalf("重复写入不应产生第二条账号，实得 %d 条", len(all))
	}
	if all[0].CloudCurrent == nil || *all[0].CloudCurrent != 1300 {
		t.Fatalf("云朵数未更新: %v", all[0].CloudCurrent)
	}
	if all[0].FailStreak != 2 {
		t.Fatalf("健康度未更新: %d", all[0].FailStreak)
	}
}

// TestSetAccountCloudTouchesOnlyTheTwoCloudColumns 钉住**窄写**的意义：
// 只刷余额两列，不把"读出来时"的其余列一起写回去。
//
// 反例很具体 —— 若这里改用 [SqliteStore.UpsertAccount]（整行覆盖），
// 调用方就得"读整行 → 改余额 → 写回"，而读与写之间别人改的昵称
// 会被**静默写回旧值**：一个只在列表页显示错名字的 bug，查起来毫无头绪。
func TestSetAccountCloudTouchesOnlyTheTwoCloudColumns(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	acc := account("acc-1")
	acc.CloudCurrent = i64Ptr(1286)
	acc.FailStreak = 3
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("建账号: %v", err)
	}

	when := at(t, "2026-09-19T06:00:00Z")
	if err := s.SetAccountCloud(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"), 5392, when); err != nil {
		t.Fatalf("刷余额: %v", err)
	}

	loaded, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil {
		t.Fatalf("读账号: %v", err)
	}
	if loaded.CloudCurrent == nil || *loaded.CloudCurrent != 5392 {
		t.Fatalf("余额应为 5392，实得 %v", loaded.CloudCurrent)
	}
	if loaded.CloudUpdatedAt == nil || !loaded.CloudUpdatedAt.Equal(when) {
		t.Fatalf("余额时间应为 %s，实得 %v", when, loaded.CloudUpdatedAt)
	}
	if loaded.Nickname == nil || *loaded.Nickname != "账号2" {
		t.Fatalf("昵称不该被顺手覆盖: %v", loaded.Nickname)
	}
	if loaded.FailStreak != 3 {
		t.Fatalf("健康度不该被顺手清零: %d", loaded.FailStreak)
	}
	if !loaded.Enabled {
		t.Fatal("启用状态不该被改")
	}
	if loaded.Owner == nil || loaded.Owner.String() != testUser {
		t.Fatalf("归属不该被改: %v", loaded.Owner)
	}
}

func TestSetAccountCloudRefusesAForeignAccount(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	acc := account("acc-1")
	acc.CloudCurrent = i64Ptr(1286)
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("建账号: %v", err)
	}

	stranger := core.NewUserID("别人")
	err := s.SetAccountCloud(ctx, core.ScopeUser(&stranger),
		core.NewAccountID("acc-1"), 5392, at(t, "2026-09-19T06:00:00Z"))
	assertKind(t, mustErr(t, err), core.KindNotFound)

	loaded, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), core.NewAccountID("acc-1"))
	if err != nil {
		t.Fatalf("读账号: %v", err)
	}
	if loaded.CloudCurrent == nil || *loaded.CloudCurrent != 1286 {
		t.Fatalf("被拒的写入不该改到任何东西，实得 %v", loaded.CloudCurrent)
	}
}

func TestAccountsAreOrderedBySortOrder(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a2 := account("acc-2")
	a2.SortOrder = 2
	a1 := account("acc-1")
	a1.SortOrder = 1
	// 刻意**先写大的**：只有真的按 sort_order 排序，才会得到 1、2 的顺序
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &a2); err != nil {
		t.Fatalf("写 acc-2: %v", err)
	}
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &a1); err != nil {
		t.Fatalf("写 acc-1: %v", err)
	}

	all, err := s.ListAccounts(ctx, core.ScopeUser(uidPtr()))
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(all) != 2 {
		t.Fatalf("应有 2 个账号，实得 %d", len(all))
	}
	if all[0].ID.String() != "acc-1" || all[1].ID.String() != "acc-2" {
		t.Fatalf("应按 sort_order 升序，实得 %s, %s", all[0].ID, all[1].ID)
	}
}

// ── account_state ───────────────────────────────────────────────────────────

// TestUpsertAccountStateLeavesTheSmidColumnsAlone 是 **schema.go 的 v5 注释
// 点名的那条**：整体覆盖的登录态写入必须绕开这两列。
//
// 现实路径很平常：重新登录 → `UpsertAccountState`（新快照，`jwt_token` 之外
// 全是 nil）。如果这两列被顺手接进那条 UPDATE 的列表里，绑定就白做了 ——
// 而登录路径根本不知道它们存在，出事也不会有人怀疑到这里。
func TestUpsertAccountStateLeavesTheSmidColumnsAlone(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	id := core.NewAccountID("acc-1")
	if err := s.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), id, fpV1); err != nil {
		t.Fatalf("写指纹: %v", err)
	}
	if err := s.SetSmidDeviceID(ctx, core.ScopeUser(uidPtr()), id, deviceID1); err != nil {
		t.Fatalf("写 deviceId: %v", err)
	}

	// 重新登录：一份只带新 token 的快照，其余字段全是 nil
	state := core.NewAccountState("acc-1")
	state.JWTToken = secretPtr("JWT-2")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &state); err != nil {
		t.Fatalf("覆盖登录态: %v", err)
	}

	smid, err := s.GetSmidState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if smid == nil {
		t.Fatal("覆盖登录态之后，smid 两列必须原样还在")
	}
	if smid.FingerprintJSON == nil || smid.FingerprintJSON.Value() != fpV1 {
		t.Fatalf("指纹应原样保留，实得 %v", smid.FingerprintJSON)
	}
	if smid.DeviceID == nil || smid.DeviceID.Value() != deviceID1 {
		t.Fatalf("deviceId 应原样保留，实得 %v", smid.DeviceID)
	}

	// 顺带确认覆盖本身生效了（否则上一条断言可能只是"没写进去"的假象）
	back, err := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if back == nil {
		t.Fatal("有登录态")
	}
	if back.JWTToken == nil || back.JWTToken.Value() != "JWT-2" {
		t.Fatalf("覆盖应当生效，实得 %v", back.JWTToken)
	}
	if back.UserDomainID != nil {
		t.Fatal("整体覆盖的语义没被改坏：没给的就是 nil")
	}
}

// TestUpsertAccountStateWritesThePhone 是 **schema.go 的 v6 注释点名的那条**：
// `phone` 与 `smid_*` 相反，**必须**跟着登录态走。
//
// 这条与上一条是同一个坑的两面：把 `phone` 也做成"独立读写"，
// 重新登录（换了个手机号）之后库里就会留着**上一个号**，
// 而 `Authorization` 会被拼成 `新 token + 旧号码` —— 一个服务端只会回
// "未登录"的、看不出根因的组合。
func TestUpsertAccountStateWritesThePhone(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	id := core.NewAccountID("acc-1")

	// 首次登录：写入完整手机号
	first := core.NewAccountState("acc-1")
	first.JWTToken = secretPtr("JWT-1")
	first.Phone = phonePtr("19900000001")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &first); err != nil {
		t.Fatalf("首次登录: %v", err)
	}
	state, err := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if state.Phone == nil || state.Phone.Value() != "19900000001" {
		t.Fatalf("手机号应为 19900000001，实得 %v", state.Phone)
	}

	// 重新登录换了号：旧号码必须被覆盖掉，不能留下
	second := core.NewAccountState("acc-1")
	second.JWTToken = secretPtr("JWT-2")
	second.Phone = phonePtr("19900000002")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &second); err != nil {
		t.Fatalf("二次登录: %v", err)
	}
	state, err = s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if state.Phone == nil || state.Phone.Value() != "19900000002" {
		t.Fatalf("换号后旧号码不许留在库里，实得 %v", state.Phone)
	}

	// 快照里没给手机号 = "上游这次没给"，写成 NULL 而不是保留旧值
	third := core.NewAccountState("acc-1")
	third.JWTToken = secretPtr("JWT-3")
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &third); err != nil {
		t.Fatalf("三次登录: %v", err)
	}
	state, err = s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if state.Phone != nil {
		t.Fatalf("整体覆盖语义：没给就是 NULL，实得 %v", state.Phone)
	}
}

// ── smid ────────────────────────────────────────────────────────────────────

func TestSmidStateRoundtripsAndIsAbsentBeforeFirstBind(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	id := core.NewAccountID("acc-1")

	// 没上报过的账号：整行都还不存在，读到的是 nil 而不是"空状态"
	never, err := s.GetSmidState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if never != nil {
		t.Fatal("未绑定不该伪造一个空 SmidState")
	}

	if err := s.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), id, fpV1); err != nil {
		t.Fatalf("写指纹: %v", err)
	}
	got, err := s.GetSmidState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if got == nil {
		t.Fatal("写完应读得到")
	}
	if got.AccountID.String() != "acc-1" {
		t.Fatalf("账号 id 不符: %s", got.AccountID)
	}
	if got.FingerprintJSON == nil || got.FingerprintJSON.Value() != fpV1 {
		t.Fatalf("指纹不符: %v", got.FingerprintJSON)
	}
	if got.DeviceID != nil {
		t.Fatal("刚绑定还没有 deviceId")
	}

	if err := s.SetSmidDeviceID(ctx, core.ScopeUser(uidPtr()), id, deviceID1); err != nil {
		t.Fatalf("写 deviceId: %v", err)
	}
	got, err = s.GetSmidState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if got.FingerprintJSON == nil || got.FingerprintJSON.Value() != fpV1 {
		t.Fatalf("特征不该被 deviceId 写入碰掉: %v", got.FingerprintJSON)
	}
	if got.DeviceID == nil || got.DeviceID.Value() != deviceID1 {
		t.Fatalf("deviceId 不符: %v", got.DeviceID)
	}
}

// TestRotatingTheFingerprintClearsTheStaleDeviceID：换指纹必须连带清掉旧 deviceId
// —— 旧 id 是**旧长相**在风控那边换来的。
func TestRotatingTheFingerprintClearsTheStaleDeviceID(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	id := core.NewAccountID("acc-1")
	if err := s.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), id, fpV1); err != nil {
		t.Fatalf("写指纹: %v", err)
	}
	if err := s.SetSmidDeviceID(ctx, core.ScopeUser(uidPtr()), id, deviceID1); err != nil {
		t.Fatalf("写 deviceId: %v", err)
	}

	if err := s.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), id, fpV2); err != nil {
		t.Fatalf("换指纹: %v", err)
	}
	got, err := s.GetSmidState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if got.FingerprintJSON == nil || got.FingerprintJSON.Value() != fpV2 {
		t.Fatalf("指纹应换成 fpV2，实得 %v", got.FingerprintJSON)
	}
	if got.DeviceID != nil {
		t.Fatalf("新长相配旧 id 会被风控认成同一台设备，比没有 id 更糟；实得 %v", got.DeviceID)
	}
}

// TestSmidWritesLeaveCredentialsAndUpdatedAtAlone 是窄写的另一面：
// `SetSmid*` 不能顺手清掉登录态，也不能刷 `updated_at`。
//
// `updated_at` 的语义是"登录态快照的时间"。一次指纹上报不是新快照 ——
// 把它刷成 now，会让人以为这个账号刚重新登录过。
func TestSmidWritesLeaveCredentialsAndUpdatedAtAlone(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	id := core.NewAccountID("acc-1")
	when := at(t, "2026-09-18T00:00:00Z")

	state := core.NewAccountState("acc-1")
	state.JWTToken = secretPtr("JWT-1")
	state.DeviceID = secretPtr("own-install-id-ENDIN")
	state.UserDomainID = domainPtr("domain-1")
	state.Phone = phonePtr("19900000001")
	state.Authorization = secretPtr(authV1)
	state.UpdatedAt = timePtr(when)
	if err := s.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &state); err != nil {
		t.Fatalf("写登录态: %v", err)
	}

	if err := s.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), id, fpV1); err != nil {
		t.Fatalf("写指纹: %v", err)
	}
	if err := s.SetSmidDeviceID(ctx, core.ScopeUser(uidPtr()), id, deviceID1); err != nil {
		t.Fatalf("写 deviceId: %v", err)
	}

	back, err := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if back == nil {
		t.Fatal("登录态还在")
	}
	if back.JWTToken == nil || back.JWTToken.Value() != "JWT-1" {
		t.Fatalf("jwtToken 被碰掉了: %v", back.JWTToken)
	}
	if back.DeviceID == nil || back.DeviceID.Value() != "own-install-id-ENDIN" {
		t.Fatalf("deviceId 被碰掉了: %v", back.DeviceID)
	}
	if back.UserDomainID == nil || back.UserDomainID.String() != "domain-1" {
		t.Fatalf("userDomainId 被碰掉了: %v", back.UserDomainID)
	}
	if back.Authorization == nil || back.Authorization.Value() != authV1 {
		t.Fatalf("整串凭据也得原样留着 —— 它是签到唯一需要的那个头: %v", back.Authorization)
	}
	if back.UpdatedAt == nil || !back.UpdatedAt.Equal(when) {
		t.Fatalf("smid 写入不该把登录态时间刷成 now，期望 %s 实得 %v", when, back.UpdatedAt)
	}
}

// TestSmidWritesRequireAnOwnedAccount：没有归属账号就没有 smid 状态 ——
// 写入被拒，而不是凭空造一行登录态。
func TestSmidWritesRequireAnOwnedAccount(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	missing := core.NewAccountID("no-such-account")

	err := s.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), missing, fpV1)
	assertKind(t, mustErr(t, err), core.KindNotFound)

	err = s.SetSmidDeviceID(ctx, core.ScopeUser(uidPtr()), missing, deviceID1)
	assertKind(t, mustErr(t, err), core.KindNotFound) // 往不存在的账号上挂 deviceId = 给未来那个同名账号预定设备

	got, err := s.GetSmidState(ctx, core.ScopeUser(uidPtr()), missing)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if got != nil {
		t.Fatal("不该有 smid 状态")
	}
}

// ── 回填 ────────────────────────────────────────────────────────────────────

// TestFillAccountCredentialsOnlyFillsTheBlanks：回填是**只填空缺** ——
// 已有值的列原样保留，哪怕调用方递了一个新值上来。
//
// 这条是回填能"每次 `migrate` 都跑"的前提。语义写成"有值就覆盖"也不会报错，
// 只会在某次迁移时把用户重登后的新凭据换成 `asign.json` 里的旧串 ——
// 而那时没有任何线索指向这里。
func TestFillAccountCredentialsOnlyFillsTheBlanks(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	id := core.NewAccountID("acc-1")

	// ① 行都不存在时：两列都填上（历史账号可能压根没登录态行）
	if err := s.FillAccountCredentials(ctx, core.ScopeUser(uidPtr()), id, strPtr(authV1), strPtr("19900000001")); err != nil {
		t.Fatalf("回填: %v", err)
	}
	state, err := s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if state == nil {
		t.Fatal("回填应当把行造出来")
	}
	if state.Authorization == nil || state.Authorization.Value() != authV1 {
		t.Fatalf("凭据应为 authV1，实得 %v", state.Authorization)
	}
	if state.Phone == nil || state.Phone.Value() != "19900000001" {
		t.Fatalf("手机号应为 19900000001，实得 %v", state.Phone)
	}
	if state.JWTToken != nil {
		t.Fatal("回填不碰别的列")
	}
	if state.UpdatedAt != nil {
		t.Fatal("补缺口不是新快照：不该写 updated_at")
	}

	// ② 已有值时：递新值上去也必须**不动**它
	if err := s.FillAccountCredentials(ctx, core.ScopeUser(uidPtr()), id, strPtr(authV2), strPtr("19900000002")); err != nil {
		t.Fatalf("二次回填: %v", err)
	}
	state, err = s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if state.Authorization == nil || state.Authorization.Value() != authV1 {
		t.Fatalf("已有凭据必须原样保留 —— 覆盖它就是把新登录换成旧的；实得 %v", state.Authorization)
	}
	if state.Phone == nil || state.Phone.Value() != "19900000001" {
		t.Fatalf("已有手机号必须原样保留，实得 %v", state.Phone)
	}

	// ③ nil = "这一列我没有可填的"，不该把已有值清成 NULL
	if err := s.FillAccountCredentials(ctx, core.ScopeUser(uidPtr()), id, nil, nil); err != nil {
		t.Fatalf("三次回填: %v", err)
	}
	state, err = s.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if state.Authorization == nil || state.Authorization.Value() != authV1 {
		t.Fatalf("nil 不该清空已有凭据，实得 %v", state.Authorization)
	}
	if state.Phone == nil || state.Phone.Value() != "19900000001" {
		t.Fatalf("nil 不该清空已有手机号，实得 %v", state.Phone)
	}
}

// TestFillAccountCredentialsRequiresAnOwnedAccount：回填的归属检查 ——
// 不能拿本地范围以外的手去补别人的凭据。
func TestFillAccountCredentialsRequiresAnOwnedAccount(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	alice := uidPtr()
	bob := core.NewUserID("someone-else")

	// 别人的账号：拒
	err := s.FillAccountCredentials(ctx, core.ScopeUser(&bob), core.NewAccountID("acc-1"), strPtr(authV1), nil)
	assertKind(t, mustErr(t, err), core.KindNotFound)

	// 不存在的账号：也拒（否则就是"给未来那个同名账号预定凭据"）
	err = s.FillAccountCredentials(ctx, core.ScopeUser(alice), core.NewAccountID("no-such"), strPtr(authV1), nil)
	assertKind(t, mustErr(t, err), core.KindNotFound)
}

// ── cloud_stats ─────────────────────────────────────────────────────────────

func TestCloudStatsUpsertIsKeyedByAccountAndDate(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	id := core.NewAccountID("acc-1")
	// 快照依附于账号：账号行必须先存在且归属于调用者
	putAccount(t, s, "acc-1")

	day1 := core.NextCloudStats(id, "2026-09-15", 1278, nil)
	if err := s.UpsertCloudStats(ctx, core.ScopeUser(uidPtr()), &day1); err != nil {
		t.Fatalf("写快照: %v", err)
	}

	// 同日重复采集应覆盖而不是新增
	day1Again := core.NextCloudStats(id, "2026-09-15", 1280, nil)
	if err := s.UpsertCloudStats(ctx, core.ScopeUser(uidPtr()), &day1Again); err != nil {
		t.Fatalf("重写快照: %v", err)
	}

	stats, err := s.CloudStatsSince(ctx, core.ScopeUser(uidPtr()), id, "2026-09-01")
	if err != nil {
		t.Fatalf("读快照: %v", err)
	}
	if len(stats) != 1 {
		t.Fatalf("同日应只有一条，实得 %d", len(stats))
	}
	if stats[0].CloudCount != 1280 {
		t.Fatalf("应被覆盖成 1280，实得 %d", stats[0].CloudCount)
	}
}

// TestCloudStatsFirstSnapshotRoundTripsAsNull 钉"没有基准"能穿过 SQLite 走一个来回。
//
// v11 之前这两列是 `NOT NULL`，"无基准"只能落成 0；改成可空之后会漏的地方有三处，
// 而且**只有一处会自己响**：
//   - 写入侧把 `*int64` 原样交给 `database/sql` ⇒ 运行期报"unsupported type"（会响）；
//   - 读取侧用裸 `int64` 扫 NULL ⇒ Scan 报错（会响）；
//   - UPSERT 的 `DO UPDATE SET` 少列 ⇒ 第二次采集把旧值留在行上，**不报错**，
//     只有读数错（该变 0 的那格仍是 NULL，或反过来）。
//
// 所以这条测试刻意写三次采集把第三种漏法夹住：同日重写必须**跟着变**，
// 隔天采集必须把两个数都写进去。
func TestCloudStatsFirstSnapshotRoundTripsAsNull(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	id := core.NewAccountID("acc-1")
	putAccount(t, s, "acc-1")
	scope := core.ScopeUser(uidPtr())

	// 第一次采集：没有历史 ⇒ 两个差值都是 nil（不是 0）
	day1 := core.NextCloudStats(id, "2026-09-24", 8708, nil)
	if err := s.UpsertCloudStats(ctx, scope, &day1); err != nil {
		t.Fatalf("写首次快照: %v", err)
	}
	// 同日再采一次（余额变了）：基准仍是"今天之前"，所以依旧没有基准
	day1Again := core.NextCloudStats(id, "2026-09-24", 8711, nil)
	if err := s.UpsertCloudStats(ctx, scope, &day1Again); err != nil {
		t.Fatalf("重写首次快照: %v", err)
	}

	stats, err := s.CloudStatsSince(ctx, scope, id, "2026-09-01")
	if err != nil {
		t.Fatalf("读快照: %v", err)
	}
	if len(stats) != 1 {
		t.Fatalf("同日应只有一条，实得 %d", len(stats))
	}
	if stats[0].CloudCount != 8711 {
		t.Fatalf("同日重写要把余额覆盖掉，实得 %d", stats[0].CloudCount)
	}
	if stats[0].CloudDiff != nil || stats[0].CloudDiffWeek != nil {
		t.Fatalf("首次采集的两个差值必须是 NULL，实得 diff=%s week=%s",
			nullText(stats[0].CloudDiff), nullText(stats[0].CloudDiffWeek))
	}

	// 第二天：有了基准 ⇒ 两个数都必须落库并读回来（少写一列会在这里红）
	day2 := core.NextCloudStats(id, "2026-09-25", 8714, []core.CloudStats{stats[0]})
	if err := s.UpsertCloudStats(ctx, scope, &day2); err != nil {
		t.Fatalf("写次日快照: %v", err)
	}
	after, err := s.CloudStatsSince(ctx, scope, id, "2026-09-01")
	if err != nil {
		t.Fatalf("读快照（次日）: %v", err)
	}
	if len(after) != 2 {
		t.Fatalf("应有两条，实得 %d", len(after))
	}
	if after[1].CloudDiff == nil || *after[1].CloudDiff != 3 {
		t.Fatalf("次日的日增量应是 3，实得 %s", nullText(after[1].CloudDiff))
	}
	// 周增量这里同样是 3：历史只有一条，"退化到最早一条"与"日增量的基准"落到同一行。
	// ⚠ 刻意不写成 6（那是第一次写进去的 8708 与 8714 之差）—— 基准必须是**读回来的那一行**，
	// 同日重写的覆盖到底生效没有，正是靠这个数区分。
	if after[1].CloudDiffWeek == nil || *after[1].CloudDiffWeek != 3 {
		t.Fatalf("次日的周增量应是 3，实得 %s", nullText(after[1].CloudDiffWeek))
	}
}

func TestCloudStatsSinceFiltersAndOrders(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	id := core.NewAccountID("acc-1")
	putAccount(t, s, "acc-1")
	var history []core.CloudStats

	// 刻意**乱序**写入：只有查询真的按 date 排序，才会得到 09-10、09-15 的顺序
	for _, row := range []struct {
		date  string
		count int64
	}{
		{"2026-09-15", 1278},
		{"2026-08-01", 900},
		{"2026-09-10", 1200},
	} {
		stats := core.NextCloudStats(id, row.date, row.count, history)
		if err := s.UpsertCloudStats(ctx, core.ScopeUser(uidPtr()), &stats); err != nil {
			t.Fatalf("写快照 %s: %v", row.date, err)
		}
		history = append(history, stats)
	}

	stats, err := s.CloudStatsSince(ctx, core.ScopeUser(uidPtr()), id, "2026-09-01")
	if err != nil {
		t.Fatalf("读快照: %v", err)
	}
	if len(stats) != 2 {
		t.Fatalf("应筛出 2 条，实得 %d", len(stats))
	}
	if stats[0].Date != "2026-09-10" || stats[1].Date != "2026-09-15" {
		t.Fatalf("应按日期升序，实得 %s, %s", stats[0].Date, stats[1].Date)
	}
}

func TestCloudStatsAreIsolatedPerAccount(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	a := core.NewAccountID("acc-1")
	b := core.NewAccountID("acc-2")
	putAccount(t, s, "acc-1")
	putAccount(t, s, "acc-2")

	statsA := core.NextCloudStats(a, "2026-09-15", 100, nil)
	if err := s.UpsertCloudStats(ctx, core.ScopeUser(uidPtr()), &statsA); err != nil {
		t.Fatalf("写 a: %v", err)
	}
	statsB := core.NextCloudStats(b, "2026-09-15", 200, nil)
	if err := s.UpsertCloudStats(ctx, core.ScopeUser(uidPtr()), &statsB); err != nil {
		t.Fatalf("写 b: %v", err)
	}

	gotA, err := s.CloudStatsSince(ctx, core.ScopeUser(uidPtr()), a, "2026-01-01")
	if err != nil {
		t.Fatalf("读 a: %v", err)
	}
	gotB, err := s.CloudStatsSince(ctx, core.ScopeUser(uidPtr()), b, "2026-01-01")
	if err != nil {
		t.Fatalf("读 b: %v", err)
	}
	if len(gotA) != 1 || gotA[0].CloudCount != 100 {
		t.Fatalf("a 的快照应为 100，实得 %v", gotA)
	}
	if len(gotB) != 1 || gotB[0].CloudCount != 200 {
		t.Fatalf("b 的快照应为 200，实得 %v", gotB)
	}
}

// ── reopen ──────────────────────────────────────────────────────────────────

// openAt 打开磁盘上的库（而不是内存库），用于"重启后还在"这一组。
func openAt(t *testing.T, path string) *SqliteStore {
	t.Helper()
	s, err := Open(path)
	if err != nil {
		t.Fatalf("打开 %s: %v", path, err)
	}
	return s
}

func TestHistorySurvivesReopen(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "mcloud.db")

	first := openAt(t, path)
	u := core.NewUser(testUser, testUser, core.PasswordHash("!"), core.RoleAdmin, time.Now().UTC())
	if err := first.UpsertUser(ctx, &u); err != nil {
		t.Fatalf("建用户: %v", err)
	}
	acc := account("acc-1")
	if err := first.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("建账号: %v", err)
	}
	if err := first.Close(); err != nil {
		t.Fatalf("关库: %v", err)
	}

	second := openAt(t, path)
	t.Cleanup(func() { _ = second.Close() })
	all, err := second.ListAccounts(ctx, core.ScopeUser(uidPtr()))
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(all) != 1 {
		t.Fatalf("重启后数据必须仍在（原 TS 版运行历史仅存内存），实得 %d 条", len(all))
	}
}

// TestAccountStatePhoneSurvivesReopen：手机号要活过重启 ——
// 它只在登录时写一次，丢了就得重登才能拿回来。
func TestAccountStatePhoneSurvivesReopen(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "mcloud.db")
	id := core.NewAccountID("acc-1")

	first := openAt(t, path)
	u := core.NewUser(testUser, testUser, core.PasswordHash("!"), core.RoleAdmin, time.Now().UTC())
	if err := first.UpsertUser(ctx, &u); err != nil {
		t.Fatalf("建用户: %v", err)
	}
	acc := account("acc-1")
	if err := first.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("建账号: %v", err)
	}
	state := core.NewAccountState("acc-1")
	state.JWTToken = secretPtr("JWT-1")
	state.Phone = phonePtr("19900000001")
	if err := first.UpsertAccountState(ctx, core.ScopeUser(uidPtr()), &state); err != nil {
		t.Fatalf("写登录态: %v", err)
	}
	if err := first.Close(); err != nil {
		t.Fatalf("关库: %v", err)
	}

	second := openAt(t, path)
	t.Cleanup(func() { _ = second.Close() })
	got, err := second.GetAccountState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读登录态: %v", err)
	}
	if got == nil {
		t.Fatal("重启后仍有登录态")
	}
	if got.Phone == nil || got.Phone.Value() != "19900000001" {
		t.Fatalf("手机号没活过重启: %v", got.Phone)
	}
}

// TestSmidStateSurvivesReopen：两列都要活过重启 ——
// 绑定是一次性的事，丢了就等于悄悄换了台设备。
func TestSmidStateSurvivesReopen(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "mcloud.db")
	id := core.NewAccountID("acc-1")

	first := openAt(t, path)
	u := core.NewUser(testUser, testUser, core.PasswordHash("!"), core.RoleAdmin, time.Now().UTC())
	if err := first.UpsertUser(ctx, &u); err != nil {
		t.Fatalf("建用户: %v", err)
	}
	acc := account("acc-1")
	if err := first.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("建账号: %v", err)
	}
	if err := first.SetSmidFingerprint(ctx, core.ScopeUser(uidPtr()), id, fpV1); err != nil {
		t.Fatalf("写指纹: %v", err)
	}
	if err := first.SetSmidDeviceID(ctx, core.ScopeUser(uidPtr()), id, deviceID1); err != nil {
		t.Fatalf("写 deviceId: %v", err)
	}
	if err := first.Close(); err != nil {
		t.Fatalf("关库: %v", err)
	}

	second := openAt(t, path)
	t.Cleanup(func() { _ = second.Close() })
	got, err := second.GetSmidState(ctx, core.ScopeUser(uidPtr()), id)
	if err != nil {
		t.Fatalf("读 smid: %v", err)
	}
	if got == nil {
		t.Fatal("重启后两列都还在")
	}
	if got.FingerprintJSON == nil || got.FingerprintJSON.Value() != fpV1 {
		t.Fatalf("指纹没活过重启: %v", got.FingerprintJSON)
	}
	if got.DeviceID == nil || got.DeviceID.Value() != deviceID1 {
		t.Fatalf("deviceId 没活过重启: %v", got.DeviceID)
	}
}

// ── runs ────────────────────────────────────────────────────────────────────

// runningRun 造一条"刚开跑"的运行记录。
func runningRun(id, accountID string) core.RunRecord {
	return core.RunRecord{
		ID:          id,
		AccountID:   accountIDPtr(accountID),
		Capability:  "sign",
		Status:      core.StatusRunning,
		StartedAt:   time.Now().UTC(),
		FinishedAt:  nil,
		CloudGained: nil,
		CloudAfter:  nil,
		Message:     nil,
	}
}

func accountIDPtr(id string) *core.AccountID { a := core.NewAccountID(id); return &a }

func TestRunLifecycleInsertThenUpdate(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	run := runningRun("run-1", "acc-1")
	if err := s.InsertRun(ctx, core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("插入: %v", err)
	}

	run.Status = core.StatusDone
	run.FinishedAt = timePtr(time.Now().UTC())
	run.CloudGained = i64Ptr(8)
	run.CloudAfter = i64Ptr(1286)
	run.Message = strPtr("签到成功")
	if err := s.UpdateRun(ctx, core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("更新: %v", err)
	}

	runs, err := s.ListRuns(ctx, core.ScopeUser(uidPtr()), 10)
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(runs) != 1 {
		t.Fatalf("应只有 1 条，实得 %d", len(runs))
	}
	if runs[0].Status != core.StatusDone {
		t.Fatalf("状态应为 Done，实得 %s", runs[0].Status)
	}
	if runs[0].CloudGained == nil || *runs[0].CloudGained != 8 {
		t.Fatalf("云朵增量应为 8，实得 %v", runs[0].CloudGained)
	}
	if runs[0].CloudAfter == nil || *runs[0].CloudAfter != 1286 {
		t.Fatalf("云朵余额应为 1286，实得 %v", runs[0].CloudAfter)
	}
	if runs[0].FinishedAt == nil {
		t.Fatal("结束时间应当写进去了")
	}
}

func TestDuplicateRunIDDoesNotCreateASecondRow(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	run := runningRun("run-1", "acc-1")
	if err := s.InsertRun(ctx, core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("首次插入: %v", err)
	}
	if err := s.InsertRun(ctx, core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("重复插入不该报错: %v", err)
	}
	runs, err := s.ListRuns(ctx, core.ScopeUser(uidPtr()), 10)
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(runs) != 1 {
		t.Fatalf("重复 id 不该产生第二条，实得 %d", len(runs))
	}
}

// TestUpdatingAMissingOrForeignRunIsAnError：三种情况必须给出**同一个**结论，
// 否则"报什么错"本身就泄漏了这条记录存在。
func TestUpdatingAMissingOrForeignRunIsAnError(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	putAccount(t, s, "acc-1")
	run := runningRun("run-1", "acc-1")
	run.Status = core.StatusFailed

	// ① 记录不存在
	assertKind(t, mustErr(t, s.UpdateRun(ctx, core.ScopeUser(uidPtr()), &run)), core.KindNotFound)

	// ② 记录存在、但调用者不是它账号的主人
	if err := s.InsertRun(ctx, core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("插入: %v", err)
	}
	other := core.NewUserID("other-user")
	otherUser := core.NewUser("other-user", "other-user", core.PasswordHash("!"), core.RoleUser, time.Now().UTC())
	if err := s.UpsertUser(ctx, &otherUser); err != nil {
		t.Fatalf("建别人: %v", err)
	}
	assertKind(t, mustErr(t, s.UpdateRun(ctx, core.ScopeUser(&other), &run)), core.KindNotFound)

	// ③ 而主人自己仍然能更新 —— 确认这不是"谁都改不了"
	if err := s.UpdateRun(ctx, core.ScopeUser(uidPtr()), &run); err != nil {
		t.Fatalf("主人应当能更新: %v", err)
	}
}

// TestOwnerSurvivesAnAccountUpsert：覆盖写账号时把 owner 一起写丢，
// 是"改个昵称就丢了账号"这类 bug 的常见来源。
func TestOwnerSurvivesAnAccountUpsert(t *testing.T) {
	ctx := context.Background()
	s := storeWithUser(t)
	acc := core.NewAccount("19****01")
	owner := uid()
	acc.Owner = &owner
	acc.Nickname = strPtr("旧昵称")
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("首次写入: %v", err)
	}

	acc.Nickname = strPtr("新昵称")
	if err := s.UpsertAccount(ctx, core.ScopeUser(uidPtr()), &acc); err != nil {
		t.Fatalf("二次写入: %v", err)
	}

	back, err := s.GetAccount(ctx, core.ScopeUser(uidPtr()), acc.ID)
	if err != nil {
		t.Fatalf("读账号: %v", err)
	}
	if back == nil {
		t.Fatal("账号应当还在")
	}
	if back.Nickname == nil || *back.Nickname != "新昵称" {
		t.Fatalf("昵称应为新昵称，实得 %v", back.Nickname)
	}
	if back.Owner == nil || back.Owner.String() != testUser {
		t.Fatalf("owner 不能被覆盖丢掉: %v", back.Owner)
	}
}
