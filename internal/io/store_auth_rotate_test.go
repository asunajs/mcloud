package mcloudio

import (
	"context"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// 换凭据（`RotateAccountAuth`）的一组测试。
//
// 这一条窄写的**失败形状**有两种，都不报错：
//   - 写成"整行覆盖" → 刷新一次就把 `device_id` / `user_domain_id` / `smid_*`
//     一起抹掉，症状是"过几天突然要重新登录"，而重新登录才是真代价；
//   - 两条 UPDATE 不在一个事务里 → 第二条失败时留下"新过期时刻 + 旧凭据"，
//     列表页据此显示一个**没有凭据支撑**的剩余天数。
//
// 所以断言全部盯着"不该动的列有没有被动"和"失败时另一张表有没有被写进去"。

const (
	// rotateAuthV1 是库里那一套旧整串（合成 `199` 号段，见脱敏约定）。
	rotateAuthV1 = "Basic MjoxOTkwMDAwMDAwMToyMDAwLXQ="
	// rotateAuthV2 是刷新换回来的新整串。
	rotateAuthV2 = "Basic MjoxOTkwMDAwMDAwMToyMDAxLXQ="
)

// seedRotatable 建一个"登录态齐全"的账号：凭据两列、设备两列、用户域、
// 红包 token、手机号、smid 两列、余额与昵称都有值。
//
// 每样都塞**非空**值是有意的：窄写写错成整行覆盖时，那些列会变成 NULL，
// 而"NULL 还是原值"只有在原先有值时才看得出来。
func seedRotatable(t *testing.T, s *SqliteStore, id string, expire int64, when time.Time) {
	t.Helper()
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())

	a := account(id)
	a.AuthExpire = &expire
	if err := s.UpsertAccount(ctx, scope, &a); err != nil {
		t.Fatalf("建账号: %v", err)
	}

	state := core.NewAccountState(id)
	state.JWTToken = secretPtr("jwt-旧")
	state.Authorization = secretPtr(rotateAuthV1)
	state.DeviceID = secretPtr(deviceID1)
	domain := core.UserDomainID("1990000000000000001")
	state.UserDomainID = &domain
	state.RedpackToken = secretPtr("redpack-旧")
	state.Phone = phonePtr("19900000001")
	updated := when
	state.UpdatedAt = &updated
	if err := s.UpsertAccountState(ctx, scope, &state); err != nil {
		t.Fatalf("建登录态: %v", err)
	}
	if err := s.SetSmidFingerprint(ctx, scope, core.NewAccountID(id), fpV1); err != nil {
		t.Fatalf("绑特征: %v", err)
	}
	if err := s.SetSmidDeviceID(ctx, scope, core.NewAccountID(id), deviceID1); err != nil {
		t.Fatalf("写 smid deviceId: %v", err)
	}
	at := when
	if err := s.SetAccountCloud(ctx, scope, core.NewAccountID(id), 320, at); err != nil {
		t.Fatalf("写余额: %v", err)
	}
}

func TestRotatingAuthOnlyTouchesTheCredentialColumns(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())
	old := time.Date(2026, 9, 1, 8, 0, 0, 0, time.UTC)
	seedRotatable(t, s, "acc-1", 1_700_000_000_000, old)

	newExpire := int64(1_800_000_000_000)
	at := time.Date(2026, 9, 22, 9, 30, 0, 0, time.UTC)
	if err := s.RotateAccountAuth(ctx, scope, core.NewAccountID("acc-1"),
		core.NewSecretValue(rotateAuthV2), core.NewSecretValue("jwt-新"), &newExpire, at); err != nil {
		t.Fatalf("换凭据: %v", err)
	}

	st, err := s.GetAccountState(ctx, scope, core.NewAccountID("acc-1"))
	if err != nil || st == nil {
		t.Fatalf("取登录态: %v / nil?", err)
	}
	if st.JWTToken == nil || string(*st.JWTToken) != "jwt-新" {
		t.Errorf("新 jwt_token 没落库：%v", st.JWTToken)
	}
	if st.Authorization == nil || string(*st.Authorization) != rotateAuthV2 {
		t.Errorf("新整串没落库：%v", st.Authorization)
	}
	if st.UpdatedAt == nil || !st.UpdatedAt.Equal(at) {
		t.Errorf("换过凭据**该**推进 updated_at（那是新快照）：%v", st.UpdatedAt)
	}

	// 反向改坏这里的方式：把这条窄写换成 `UpsertAccountState`（整行覆盖），
	// 下面每一列都会变成 NULL —— 症状是"刷新一次等于把设备与用户域忘掉"。
	for _, probe := range []struct {
		what string
		ok   bool
	}{
		{"device_id（换钥匙不该换设备）", st.DeviceID != nil && string(*st.DeviceID) == deviceID1},
		{"user_domain_id", st.UserDomainID != nil && st.UserDomainID.String() == "1990000000000000001"},
		{"cloud_phone_redpack_token", st.RedpackToken != nil && string(*st.RedpackToken) == "redpack-旧"},
		{"phone（完整号码）", st.Phone != nil && st.Phone.Value() == "19900000001"},
	} {
		if !probe.ok {
			t.Errorf("%s 被卷进了窄写", probe.what)
		}
	}
	smid, err := s.GetSmidState(ctx, scope, core.NewAccountID("acc-1"))
	if err != nil || smid == nil || smid.FingerprintJSON == nil ||
		string(*smid.FingerprintJSON) != fpV1 || smid.DeviceID == nil ||
		string(*smid.DeviceID) != deviceID1 {
		t.Errorf("smid 两列被卷进了窄写：%v / %v", smid, err)
	}

	got, err := s.GetAccount(ctx, scope, core.NewAccountID("acc-1"))
	if err != nil || got == nil {
		t.Fatalf("取账号: %v / nil?", err)
	}
	if got.AuthExpire == nil || *got.AuthExpire != newExpire {
		t.Errorf("auth_expire 没跟着换（新旧不一致就是下一个 bug）：%v", got.AuthExpire)
	}
	// 同一行上的其余列：昵称与余额不该被"换凭据"碰
	if got.Nickname == nil || *got.Nickname != "账号2" {
		t.Errorf("昵称被卷进来了：%v", got.Nickname)
	}
	if got.CloudCurrent == nil || *got.CloudCurrent != 320 {
		t.Errorf("余额被卷进来了：%v", got.CloudCurrent)
	}
}

// TestRotatingAuthLeavesNullExpireWhenUpstreamGaveNone 钉住"nil 不保留旧值"。
//
// 直觉上"上游没给过期信息就别动那一列"更像省事，但那一列描述的是**已被替换的旧串**：
// 留着它，列表页会显示一个属于旧 token 的剩余天数，刷新判定也跟着错。
// 写 NULL 之后读取方判"无过期信息 → 该刷新"，那才是安全的一侧。
func TestRotatingAuthLeavesNullExpireWhenUpstreamGaveNone(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())
	seedRotatable(t, s, "acc-1", 1_700_000_000_000,
		time.Date(2026, 9, 1, 8, 0, 0, 0, time.UTC))

	if err := s.RotateAccountAuth(ctx, scope, core.NewAccountID("acc-1"),
		core.NewSecretValue(rotateAuthV2), core.NewSecretValue("jwt-新"), nil,
		time.Date(2026, 9, 22, 9, 30, 0, 0, time.UTC)); err != nil {
		t.Fatalf("换凭据: %v", err)
	}

	got, err := s.GetAccount(ctx, scope, core.NewAccountID("acc-1"))
	if err != nil || got == nil {
		t.Fatalf("取账号: %v / nil?", err)
	}
	if got.AuthExpire != nil {
		t.Errorf("新串不带过期信息时该写 NULL，实得 %d", *got.AuthExpire)
	}
}

// TestRotatingAuthWithoutACredentialRowWritesNothing 是"两条 UPDATE 必须同事务"
// 的**唯一**证据：影响 0 行时如果继续往下走，`accounts.auth_expire` 就被写了新值。
//
// 反例造法：把 `RotateAccountAuth` 改成不用事务（或把 accounts 那条挪到事务外）。
func TestRotatingAuthWithoutACredentialRowWritesNothing(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())

	// 只建账号行、**不**建登录态行：真库里的"从没登录过却被人手工加了个账号"就是这个形状
	a := account("acc-1")
	expire := int64(1_700_000_000_000)
	a.AuthExpire = &expire
	if err := s.UpsertAccount(ctx, scope, &a); err != nil {
		t.Fatalf("建账号: %v", err)
	}

	newExpire := int64(1_800_000_000_000)
	err := s.RotateAccountAuth(ctx, scope, core.NewAccountID("acc-1"),
		core.NewSecretValue(rotateAuthV2), core.NewSecretValue("jwt-新"), &newExpire,
		time.Date(2026, 9, 22, 9, 30, 0, 0, time.UTC))
	assertKind(t, mustErr(t, err), core.KindNotFound)

	got, err := s.GetAccount(ctx, scope, core.NewAccountID("acc-1"))
	if err != nil || got == nil {
		t.Fatalf("取账号: %v / nil?", err)
	}
	if got.AuthExpire == nil || *got.AuthExpire != expire {
		t.Errorf("凭据没写成，过期时刻却先换了 —— 事务漏了：%v", got.AuthExpire)
	}
	if n := countRows(t, s, "SELECT count(*) FROM account_state WHERE account_id = ?", "acc-1"); n != 0 {
		t.Errorf("窄写不该凭空建登录态行，实得 %d 行", n)
	}
}

func TestRotatingAnotherUsersAuthIsRejectedAndLeavesTheRowAlone(t *testing.T) {
	s := storeWithUser(t)
	ctx := context.Background()
	scope := core.ScopeUser(uidPtr())
	seedRotatable(t, s, "acc-1", 1_700_000_000_000,
		time.Date(2026, 9, 1, 8, 0, 0, 0, time.UTC))

	other := core.NewUserID("other-user")
	newExpire := int64(1_800_000_000_000)
	err := s.RotateAccountAuth(ctx, core.ScopeUser(&other), core.NewAccountID("acc-1"),
		core.NewSecretValue(rotateAuthV2), core.NewSecretValue("jwt-新"), &newExpire,
		time.Date(2026, 9, 22, 9, 30, 0, 0, time.UTC))
	// 与读取路径同一个结论（"没有这个账号"），否则账号 id 空间变成可枚举的
	assertKind(t, mustErr(t, err), core.KindNotFound)

	st, err := s.GetAccountState(ctx, scope, core.NewAccountID("acc-1"))
	if err != nil || st == nil || st.Authorization == nil ||
		string(*st.Authorization) != rotateAuthV1 {
		t.Errorf("别人的换凭据调用把整串写进去了：%v / %v", st, err)
	}
}

func TestRotatingAnUnknownAccountIsNotFound(t *testing.T) {
	s := storeWithUser(t)
	err := s.RotateAccountAuth(context.Background(), core.ScopeUser(uidPtr()),
		core.NewAccountID("199****0999"),
		core.NewSecretValue(rotateAuthV2), core.NewSecretValue("jwt-新"), nil,
		time.Date(2026, 9, 22, 9, 30, 0, 0, time.UTC))
	assertKind(t, mustErr(t, err), core.KindNotFound)
}

// TestLocalScopeCanRotateAnUnownedAccount 确认本地范围**不**约束归属：
// CLI 那条路径（`mcloud auth refresh`）手上没有 user id，靠的就是这一条。
func TestLocalScopeCanRotateAnUnownedAccount(t *testing.T) {
	s := store(t)
	ctx := context.Background()
	a := core.NewAccount("orphan-1")
	if err := s.UpsertAccount(ctx, core.ScopeLocal(), &a); err != nil {
		t.Fatalf("建无主账号: %v", err)
	}
	state := core.NewAccountState("orphan-1")
	state.Authorization = secretPtr(rotateAuthV1)
	if err := s.UpsertAccountState(ctx, core.ScopeLocal(), &state); err != nil {
		t.Fatalf("建登录态: %v", err)
	}

	if err := s.RotateAccountAuth(ctx, core.ScopeLocal(), core.NewAccountID("orphan-1"),
		core.NewSecretValue(rotateAuthV2), core.NewSecretValue("jwt-新"), nil,
		time.Date(2026, 9, 22, 9, 30, 0, 0, time.UTC)); err != nil {
		t.Fatalf("本地范围换凭据: %v", err)
	}
	st, err := s.GetAccountState(ctx, core.ScopeLocal(), core.NewAccountID("orphan-1"))
	if err != nil || st == nil || st.Authorization == nil ||
		string(*st.Authorization) != rotateAuthV2 {
		t.Fatalf("没换成：%v / %v", st, err)
	}
}
