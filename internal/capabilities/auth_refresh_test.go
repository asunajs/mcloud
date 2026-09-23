package capabilities

import (
	"context"
	"encoding/base64"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 刷新这一组测三件事：
//
//   - **判据只有一档会打上游**：另两档必须一次都不碰网络，也不碰库。
//     反过来（"没到点"也打一次）的表现是每天白打上游 + 把正常账号记进失败计数。
//   - **新串的端别与手机号来自旧串**，不来自参数：互换在当场没有任何症状，
//     只有几天后 401 才看得见。
//   - **错误串里不许出现凭据**：这一层的错误会走到 CLI 与响应体，
//     而输入正是整串凭据。
//
// 上游怎么发、响应长什么样在 internal/upstream 那组测；这里只给假 `RefreshSender`。

// rotatableStore 是刷新链路会用到的两个存储入口。
//
// 与 [recordingStore] 同样的写法：内嵌接口，没重写的方法一旦被调用就 panic ——
// 刷新不该改昵称、不该改设备特征，碰了就当场响。
type rotatableStore struct {
	core.StateStore
	state     *core.AccountState
	rotations []rotationCall
	rotateErr error
}

type rotationCall struct {
	scope  core.Scope
	id     core.AccountID
	auth   string
	jwt    string
	expire *int64
	at     time.Time
}

func (s *rotatableStore) GetAccountState(
	_ context.Context, _ core.Scope, _ core.AccountID,
) (*core.AccountState, error) {
	return s.state, nil
}

func (s *rotatableStore) RotateAccountAuth(
	_ context.Context, scope core.Scope, id core.AccountID,
	authorization, jwtToken core.SecretValue, authExpire *int64, at time.Time,
) error {
	if s.rotateErr != nil {
		return s.rotateErr
	}
	s.rotations = append(s.rotations, rotationCall{
		scope: scope, id: id,
		auth: string(authorization), jwt: string(jwtToken), expire: authExpire, at: at,
	})
	return nil
}

// authOf 造一份整串（三段形状与客户端逐字一致）。
func authOf(prefix, phone, token string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte(prefix+":"+phone+":"+token))
}

// millisAt 把"从现在起 N 天"换成 epoch 毫秒，供 token 第 4 段用。
func millisAt(offset time.Duration) string {
	return strconv.FormatInt(now().Add(offset).UnixMilli(), 10)
}

func stateWith(t *testing.T, auth string, userDomainID string) *rotatableStore {
	t.Helper()
	full := core.FullPhone("19900000000")
	authorization := core.SecretValue(auth)
	store := &rotatableStore{state: &core.AccountState{
		AccountID:     core.AccountIDFor("19900000000", nil),
		Authorization: &authorization,
		Phone:         &full,
	}}
	if userDomainID != "" {
		domain := core.UserDomainID(userDomainID)
		store.state.UserDomainID = &domain
	}
	return store
}

// okSender 回放一份"上游刷新成功"的结果，并记下拿到的 `userDomainId`。
func okSender(token string, seen *string) RefreshSender {
	return func(_ context.Context, userDomainID string) (upstream.RefreshTokenResult, *core.Error) {
		*seen = userDomainID
		return upstream.RefreshTokenResult{Token: token}, nil
	}
}

func TestDecideRefreshThreeTiers(t *testing.T) {
	const lead = 7.0
	cases := []struct {
		name  string
		token string
		want  RefreshDecision
	}{
		// 第 4 段是**epoch 毫秒**：3 天后 → 落在 7 天提前量里。
		{"还剩 3 天 → 该刷", "S|" + "1" + "|R|" + millisAt(3*24*time.Hour) + "|X", RefreshNeeded},
		// 30 天后 → 没到提前量，**不发请求**。
		{"还剩 30 天 → 不用刷", "S|" + "1" + "|R|" + millisAt(30*24*time.Hour) + "|X", RefreshNotYet},
		// 没有第 4 段（`Expire` 归零）→ 判该刷，与 core 那侧"宁可多刷一次"同向。
		{"无过期信息 → 该刷", "S|" + "1" + "|R", RefreshNeeded},
		// 负数 → 上游刷不了，只能人工换 auth。
		{"过期时刻为负 → 只能人工", "S|" + "1" + "|R|-1|X", RefreshManualOnly},
		// 已过期（1 天前）→ 仍然该刷：提前量的判据是"剩余 < N 天"，负剩余也算。
		{"已过期 → 该刷", "S|" + "1" + "|R|" + millisAt(-24*time.Hour) + "|X", RefreshNeeded},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			decision, err := DecideRefresh(authOf(PCAuthPrefix, "19900000000", c.token), now(), lead)
			if err != nil {
				t.Fatalf("判定失败: %v", err)
			}
			if decision != c.want {
				t.Errorf("实得 %v，应为 %v", decision, c.want)
			}
			// 三档都要有中文名（CLI 直接把它打给用户）
			if decision.String() == "" {
				t.Error("结论没有中文名")
			}
		})
	}
}

func TestDecideRefreshRejectsUnparsableAuthWithoutEchoingIt(t *testing.T) {
	// 这串不是 auth（base64 解不出三段形状）。归不进任何一档：
	// 判"该刷"会白打上游，判"不用刷"会把一个坏账号说成好的。
	bad := "Basic !!!not-base64!!!"
	decision, err := DecideRefresh(bad, now(), 7.0)
	if err == nil {
		t.Fatalf("应报错，实得结论 %v", decision)
	}
	if err.Kind() != core.KindBadRequest {
		t.Errorf("应为 BadRequest，实得 %v", err.Kind())
	}
	// 错误串会进 CLI 与响应体：不能把输入回显出去
	if strings.Contains(err.Message(), "not-base64") {
		t.Errorf("错误串回显了输入: %s", err.Message())
	}
}

func TestOnlyNeededReachesTheSenderAndStore(t *testing.T) {
	cases := []struct {
		name     string
		token    string
		wantCall int
	}{
		{"没到提前量不打上游", "S|1|R|" + millisAt(30*24*time.Hour) + "|X", 0},
		{"只能人工时不打上游", "S|1|R|-1|X", 0},
		{"该刷时打一次", "S|1|R|" + millisAt(3*24*time.Hour) + "|X", 1},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			scope := scopeForTest()
			store := stateWith(t, authOf(PCAuthPrefix, "19900000000", c.token), "1990000000000000001")
			calls := 0
			var seen string
			sender := func(_ context.Context, userDomainID string) (upstream.RefreshTokenResult, *core.Error) {
				calls++
				seen = userDomainID
				return upstream.RefreshTokenResult{Token: "NEW|1|R|" + millisAt(30*24*time.Hour) + "|X"}, nil
			}
			outcome, err := RefreshAuth(context.Background(), store, scope,
				store.state.AccountID, now(), 7.0, false, sender)
			if err != nil {
				t.Fatalf("刷新失败: %v", err)
			}
			if calls != c.wantCall {
				t.Errorf("上游调用 %d 次，应为 %d 次", calls, c.wantCall)
			}
			if len(store.rotations) != c.wantCall {
				t.Errorf("写库 %d 次，应为 %d 次", len(store.rotations), c.wantCall)
			}
			if outcome.Rotated != (c.wantCall == 1) {
				t.Errorf("Rotated=%v，与是否刷过不符", outcome.Rotated)
			}
			if c.wantCall == 1 && seen != "1990000000000000001" {
				t.Errorf("发出去的 userDomainId 是 %q", seen)
			}
			if c.wantCall == 1 && outcome.Credential.AuthToken == "" {
				t.Error("刷成功了却没带回新凭据（调用方要拿它报过期时刻）")
			}
			if c.wantCall == 0 && outcome.Credential.Authorization != "" {
				t.Error("没打上游却给出了新凭据")
			}
		})
	}
}

func TestForceCrossesOnlyTheLeadWindow(t *testing.T) {
	// 与 [TestOnlyNeededReachesTheSenderAndStore] 同一批 token，唯一差别是 force=true。
	// 两档要分开钉：force 越过的**只有**"还没到提前量"那一档 —— 把它做成
	// "永远打上游"，`manual-only`（库里那串读不出可用过期时刻）就会变成一次
	// 注定失败的请求，而那一档的正确出口是"重新登录 / 贴整串"，不是重试。
	cases := []struct {
		name         string
		token        string
		wantCall     int
		wantRotated  bool
		wantDecision RefreshDecision
	}{
		{"剩 30 天：force 越过得去", "S|1|R|" + millisAt(30*24*time.Hour) + "|X", 1, true, RefreshNotYet},
		{"过期时刻为负：force 也越不过去", "S|1|R|-1|X", 0, false, RefreshManualOnly},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			scope := scopeForTest()
			store := stateWith(t, authOf(PCAuthPrefix, "19900000000", c.token), "1990000000000000001")
			calls := 0
			counting := func(context.Context, string) (upstream.RefreshTokenResult, *core.Error) {
				calls++
				return upstream.RefreshTokenResult{
					Token: "NEW|1|R|" + millisAt(40*24*time.Hour) + "|X",
				}, nil
			}
			outcome, err := RefreshAuth(context.Background(), store, scope,
				store.state.AccountID, now(), 7.0, true, counting)
			if err != nil {
				t.Fatalf("不该报错: %v", err)
			}
			if calls != c.wantCall {
				t.Errorf("上游请求 %d 次，期望 %d 次", calls, c.wantCall)
			}
			if outcome.Rotated != c.wantRotated {
				t.Errorf("Rotated=%v，期望 %v", outcome.Rotated, c.wantRotated)
			}
			// 档位要如实：换了与否由 Rotated 说，判据本身不改口
			if outcome.Decision != c.wantDecision {
				t.Errorf("decision 是 %v，期望 %v", outcome.Decision, c.wantDecision)
			}
			if writes := len(store.rotations); writes != c.wantCall {
				t.Errorf("写库 %d 次，期望 %d 次", writes, c.wantCall)
			}
		})
	}
}

func TestRotatedCredentialKeepsEndAndPhoneFromTheStoredString(t *testing.T) {
	// 当前是 App 端（`mobile`）：新串必须还是 `mobile`，手机号不变，只换 token。
	scope := scopeForTest()
	store := stateWith(t, authOf(upstream.IOSAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "1990000000000000009")
	var seen string
	outcome, err := RefreshAuth(context.Background(), store, scope,
		store.state.AccountID, now(), 7.0, false, okSender("NEW|1|R|"+millisAt(30*24*time.Hour)+"|X", &seen))
	if err != nil {
		t.Fatalf("刷新失败: %v", err)
	}
	if len(store.rotations) != 1 {
		t.Fatalf("写库 %d 次", len(store.rotations))
	}
	if !outcome.Rotated || outcome.Decision != RefreshNeeded {
		t.Errorf("结论是 %v / Rotated=%v", outcome.Decision, outcome.Rotated)
	}
	call := store.rotations[0]

	decoded := decodeAuth(t, call.auth)
	got := strings.Split(decoded, ":")
	if len(got) != 3 {
		t.Fatalf("新串不是三段：%q", redactShape(decoded))
	}
	if got[0] != upstream.IOSAuthPrefix {
		t.Errorf("端别被换成了 %q，应逐字沿用 %q", got[0], upstream.IOSAuthPrefix)
	}
	if got[1] != "19900000000" {
		t.Errorf("手机号变成了 %q", got[1])
	}
	if !strings.HasPrefix(got[2], "NEW|") {
		t.Errorf("token 没换成新的：%q", redactShape(got[2]))
	}
	if call.jwt != "NEW|1|R|"+millisAt(30*24*time.Hour)+"|X" {
		t.Error("jwt_token 与整串第三段不是同一个值")
	}
	if call.expire == nil || *call.expire != now().Add(30*24*time.Hour).UnixMilli() {
		t.Errorf("auth_expire 没跟上新串：%v", call.expire)
	}
	if call.at != now() {
		t.Error("updated_at 没用调用方注入的 now")
	}
	if call.scope != scope || call.id != store.state.AccountID {
		t.Error("写到了别的范围/账号上")
	}
}

func TestPCAndAppNeverSwapTheirEndPrefixes(t *testing.T) {
	// 两端各刷一次，各自的前缀必须各自留住 —— 这条是"入参传前缀"那种写法的反向闸。
	for _, prefix := range []string{PCAuthPrefix, upstream.IOSAuthPrefix} {
		scope := scopeForTest()
		store := stateWith(t, authOf(prefix, "19900000000",
			"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "1990000000000000001")
		var seen string
		if _, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
			now(), 7.0, false, okSender("NEW|1|R|"+millisAt(30*24*time.Hour)+"|X", &seen)); err != nil {
			t.Fatalf("%s 刷新失败: %v", prefix, err)
		}
		got := strings.Split(decodeAuth(t, store.rotations[0].auth), ":")
		if got[0] != prefix {
			t.Errorf("前缀 %q 被写成了 %q", prefix, got[0])
		}
	}
}

func TestTokenContainingAColonNeverReachesTheStore(t *testing.T) {
	// `ParseAuth` 只取前三段（原版怪癖）：带冒号的新 token 拼进整串后会被截断，
	// 于是"刷成功"的账号立刻不可用。必须在写之前拦下。
	scope := scopeForTest()
	store := stateWith(t, authOf(PCAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "1990000000000000001")
	var seen string
	_, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, okSender("NEW|1|R|1|x:y", &seen))
	if err == nil {
		t.Fatal("带冒号的 token 应被拦下")
	}
	if len(store.rotations) != 0 {
		t.Errorf("拦下了却还是写了库：%d 次", len(store.rotations))
	}
	if !strings.Contains(err.Message(), "冒号") {
		t.Errorf("错误串没说明真正原因：%s", err.Message())
	}
}

func TestEmptyUpstreamTokenIsRejected(t *testing.T) {
	scope := scopeForTest()
	store := stateWith(t, authOf(PCAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "1990000000000000001")
	var seen string
	_, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, okSender("", &seen))
	if err == nil {
		t.Fatal("空 token 不能当成功")
	}
	if len(store.rotations) != 0 {
		t.Error("空 token 被写进了库")
	}
}

func TestMissingUserDomainIDStopsBeforeTheUpstream(t *testing.T) {
	// 请求体里就靠它定位用户域：缺了要么被拒、要么刷到**别人的**用户域。
	// 编一个更不能接受，所以这里判"刷不了"且一次都不打。
	scope := scopeForTest()
	store := stateWith(t, authOf(PCAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "")
	calls := 0
	_, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, func(context.Context, string) (upstream.RefreshTokenResult, *core.Error) {
			calls++
			return upstream.RefreshTokenResult{Token: "NEW"}, nil
		})
	if err == nil {
		t.Fatal("缺 userDomainId 时应拒绝")
	}
	if calls != 0 {
		t.Error("已经被拒绝，却还是打了上游")
	}
	if !strings.Contains(err.Message(), "重新登录") {
		t.Errorf("错误串没给出可行动作：%s", err.Message())
	}
	if len(store.rotations) != 0 {
		t.Error("失败还写了库")
	}
}

// 不刷的时候缺 userDomainId 不该报错：那一档根本不需要它，
// 报"刷不了"会把一个还能用的账号说成坏的。
func TestMissingUserDomainIDIsSilentWhenRefreshIsNotDue(t *testing.T) {
	scope := scopeForTest()
	store := stateWith(t, authOf(PCAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(30*24*time.Hour)+"|X"), "")
	var seen string
	outcome, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, okSender("NEW", &seen))
	if err != nil {
		t.Fatalf("不该报错: %v", err)
	}
	if outcome.Decision != RefreshNotYet {
		t.Errorf("实得 %v", outcome.Decision)
	}
}

func TestAccountWithoutAuthorizationStringIsToldToRelogin(t *testing.T) {
	// 端别与手机号都从整串取（见 [CredentialFromRefresh]），没有整串就没得取。
	// 老行（v7 之前）只有 `jwt_token` + `phone`，前缀要靠猜 —— 那种账号不刷。
	scope := scopeForTest()
	jwt := core.SecretValue("OLD|1|R|" + millisAt(2*24*time.Hour) + "|X")
	store := &rotatableStore{state: &core.AccountState{
		AccountID: core.AccountIDFor("19900000000", nil),
		JWTToken:  &jwt,
	}}
	var seen string
	_, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, okSender("NEW", &seen))
	if err == nil {
		t.Fatal("没有整串时应拒绝刷新")
	}
	if !strings.Contains(err.Message(), "重新登录") {
		t.Errorf("错误串没给出可行动作：%s", err.Message())
	}
	if strings.Contains(err.Message(), string(jwt)) {
		t.Errorf("错误串回显了凭据：%s", err.Message())
	}
	if len(store.rotations) != 0 {
		t.Error("没有整串却还是写了库")
	}
}

func TestStoreFailurePropagatesAndKeepsMessageFreeOfCredentials(t *testing.T) {
	scope := scopeForTest()
	store := stateWith(t, authOf(PCAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "1990000000000000001")
	store.rotateErr = core.NewNotFound("账号不存在")
	var seen string
	_, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, okSender("NEW|1|R|1|"+millisAt(30*24*time.Hour)+"|X", &seen))
	if err == nil {
		t.Fatal("写库失败应向上报错")
	}
	if strings.Contains(err.Message(), "NEW|") || strings.Contains(err.PublicMessage(), "NEW|") {
		t.Errorf("错误串里出现了新 token：%s / %s", err.Message(), err.PublicMessage())
	}
}

func TestUpstreamFailureLeavesTheStoredCredentialAlone(t *testing.T) {
	scope := scopeForTest()
	store := stateWith(t, authOf(PCAuthPrefix, "19900000000",
		"OLD|1|R|"+millisAt(2*24*time.Hour)+"|X"), "1990000000000000001")
	_, err := RefreshAuth(context.Background(), store, scope, store.state.AccountID,
		now(), 7.0, false, func(context.Context, string) (upstream.RefreshTokenResult, *core.Error) {
			return upstream.RefreshTokenResult{}, core.NewAuth("上游回 401")
		})
	if err == nil {
		t.Fatal("上游失败应向上报错")
	}
	if err.Kind() != core.KindAuth {
		t.Errorf("错误种类被改掉了：%v", err.Kind())
	}
	if len(store.rotations) != 0 {
		t.Error("刷新失败却还是换了库里的凭据")
	}
}

func TestCredentialFromRefreshWorksOnAStringWeJustBuilt(t *testing.T) {
	// 刷新结果本身要能被 [DecideRefresh] 再判一次（下一轮定时扫描走的就是这条路）。
	current := authOf(PCAuthPrefix, "19900000000", "OLD|1|R|"+millisAt(2*24*time.Hour)+"|X")
	credential, err := CredentialFromRefresh(current, "NEW|1|R|"+millisAt(30*24*time.Hour)+"|X")
	if err != nil {
		t.Fatalf("拼装失败: %v", err)
	}
	decision, derr := DecideRefresh(string(credential.Authorization), now(), 7.0)
	if derr != nil {
		t.Fatalf("新串判不了: %v", derr)
	}
	if decision != RefreshNotYet {
		t.Errorf("刷完 30 天后才该再动，实得 %v", decision)
	}
	if credential.ExpireMillis == nil {
		t.Error("新串带着过期时刻却没解析出来")
	}
}

func TestCredentialFromRefreshKeepsMissingExpireAsNil(t *testing.T) {
	// 上游有时回不带第 4 段的 token：这时 `auth_expire` 要写 NULL，
	// **不能保留旧值** —— 留着旧的等于宣称"新钥匙到某天失效"，而那是旧钥匙的日期。
	current := authOf(PCAuthPrefix, "19900000000", "OLD|1|R|"+millisAt(2*24*time.Hour)+"|X")
	credential, err := CredentialFromRefresh(current, "NEW|1|R")
	if err != nil {
		t.Fatalf("拼装失败: %v", err)
	}
	if credential.ExpireMillis != nil {
		t.Errorf("应为 nil，实得 %v", *credential.ExpireMillis)
	}
}

// TestDecisionCodesAreTheAPIShape：`Code()` 是出网的那三个值，改动节奏与中文文案不同。
//
// 判据是"三档各一个**稳定**值 + 越界值给 unknown"，而不是"非空"：
// 前端按这些串 `switch`，把它们改掉等于改 HTTP 契约（中文那句 `String()` 才是可以随手改的）。
func TestDecisionCodesAreTheAPIShape(t *testing.T) {
	cases := []struct {
		decision RefreshDecision
		want     string
	}{
		{RefreshNeeded, "needed"},
		{RefreshNotYet, "not-yet"},
		{RefreshManualOnly, "manual-only"},
		// 越界值：新档位漏配时给 `unknown`，而不是静默落到某一档上
		{RefreshDecision(99), "unknown"},
	}
	for _, c := range cases {
		if got := c.decision.Code(); got != c.want {
			t.Errorf("%v 的 Code 应是 %q，实得 %q", int(c.decision), c.want, got)
		}
	}
}

// decodeAuth 解出整串载荷（断言用）。
func decodeAuth(t *testing.T, auth string) string {
	t.Helper()
	raw, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(auth, "Basic "))
	if err != nil {
		t.Fatalf("整串解不出来: %v", err)
	}
	return string(raw)
}

// redactShape 只留下"多长、有几段"，不进断言消息之外的任何地方。
func redactShape(s string) string {
	return strings.Repeat("?", len(s))
}
