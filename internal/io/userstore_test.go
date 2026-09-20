package mcloudio

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// 系统用户与会话的存储测试。
//
// ⚠ 这里**不做**口令校验的测试 —— 校验是 [core.VerifyPassword] 的事（纯计算，
// 在 internal/core 有它自己的对拍）。存储层只负责把哈希原样搬进搬出，
// 所以这里断言的是"搬得对不对"，不是"口令对不对"。

// seqRandom 是确定性随机源：让口令哈希与会话 token 可复现。
//
// ⚠ 它是**这一层**的夹具，与 internal/core 里那几个 SeededRng 无关 ——
// 那种"就近复用同包里另一个模块的 RNG"的做法踩过坑（见方案 §6.1 #19）。
// 这里要的是"逐字节递增"，不是任何伪随机分布。
type seqRandom struct{ next byte }

func newSeqRandom(seed byte) *seqRandom { return &seqRandom{next: seed} }

func (r *seqRandom) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

func makeUser(t *testing.T, s *SqliteStore, name string, role core.Role) core.User {
	t.Helper()
	hash, err := core.HashPassword("correct-horse", newSeqRandom(7))
	if err != nil {
		t.Fatalf("算口令哈希: %v", err)
	}
	u := core.NewUser("u-"+name, name, hash, role, at(t, "2026-09-18T00:00:00Z"))
	if err := s.UpsertUser(context.Background(), &u); err != nil {
		t.Fatalf("建用户 %s: %v", name, err)
	}
	return u
}

func TestUserRoundTripsThroughSQLite(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)

	byID, err := s.GetUser(ctx, user.ID)
	if err != nil {
		t.Fatalf("按 id 查: %v", err)
	}
	if byID == nil {
		t.Fatal("按 id 应当查得到")
	}
	if *byID != user {
		t.Fatalf("按 id 查回来的不一致:\n实得 %+v\n期望 %+v", *byID, user)
	}
	byName, err := s.GetUserByName(ctx, "alice")
	if err != nil {
		t.Fatalf("按名字查: %v", err)
	}
	if byName == nil || *byName != user {
		t.Fatalf("按名字查回来的不一致: %+v", byName)
	}
	n, err := s.CountUsers(ctx)
	if err != nil {
		t.Fatalf("计数: %v", err)
	}
	if n != 1 {
		t.Fatalf("应有 1 个用户，实得 %d", n)
	}
	all, err := s.ListUsers(ctx)
	if err != nil {
		t.Fatalf("列表: %v", err)
	}
	if len(all) != 1 || all[0] != user {
		t.Fatalf("列表不符: %+v", all)
	}
	if !user.IsAdmin() {
		t.Fatal("这个用户应当是管理员")
	}

	// 口令哈希必须原样回得来（否则登录会莫名其妙失败）
	ok, err := core.VerifyPassword("correct-horse", user.PasswordHash)
	if err != nil {
		t.Fatalf("校验口令: %v", err)
	}
	if !ok {
		t.Fatal("刚存进去的口令应当验得过")
	}
}

func TestUserNameIsCaseInsensitivelyUnique(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	makeUser(t, s, "Alice", core.RoleAdmin)

	// 大小写不同的查询必须命中同一个人 —— 登录路径靠这条
	for _, name := range []string{"alice", "ALICE", "aLiCe"} {
		got, err := s.GetUserByName(ctx, name)
		if err != nil {
			t.Fatalf("按 %q 查: %v", name, err)
		}
		if got == nil {
			t.Fatalf("按 %q 应当查得到（大小写不敏感）", name)
		}
	}

	// 换个 id 但用大小写不同的同名 → 必须拒绝
	dup := core.NewUser("u-other", "aLiCe", core.PasswordHash("x"), core.RoleUser,
		at(t, "2026-09-18T00:00:00Z"))
	err := s.UpsertUser(ctx, &dup)
	assertKind(t, mustErr(t, err), core.KindBadRequest)
	if !strings.Contains(err.Error(), "已被占用") {
		t.Fatalf("错误信息应说明名字被占用，实得: %v", err)
	}

	// 报错不能把原来的人覆盖掉
	back, err := s.GetUserByName(ctx, "alice")
	if err != nil {
		t.Fatalf("复查: %v", err)
	}
	if back == nil || back.ID.String() != "u-Alice" {
		t.Fatalf("被拒之后原用户必须原样还在，实得 %+v", back)
	}
	n, err := s.CountUsers(ctx)
	if err != nil {
		t.Fatalf("计数: %v", err)
	}
	if n != 1 {
		t.Fatalf("不该多出一个人，实得 %d", n)
	}
}

// TestUpsertUserUpdatesInPlaceWithoutTouchingCreatedAt：
// `created_at` 记的是"何时创建"，不该被 upsert 改掉。
func TestUpsertUserUpdatesInPlaceWithoutTouchingCreatedAt(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleUser)

	user.Role = core.RoleAdmin
	user.PasswordHash = core.PasswordHash("新的哈希")
	if err := s.UpsertUser(ctx, &user); err != nil {
		t.Fatalf("二次写入: %v", err)
	}

	back, err := s.GetUser(ctx, user.ID)
	if err != nil {
		t.Fatalf("查回: %v", err)
	}
	if back == nil {
		t.Fatal("用户应当还在")
	}
	if back.Role != core.RoleAdmin {
		t.Fatalf("角色应当更新为 admin，实得 %s", back.Role)
	}
	if string(back.PasswordHash) != "新的哈希" {
		t.Fatalf("口令哈希应当更新，实得 %q", back.PasswordHash)
	}
	if !back.CreatedAt.Equal(user.CreatedAt) {
		t.Fatalf("created_at 记录的是「何时创建」，不该被 upsert 改掉：实得 %s 期望 %s",
			back.CreatedAt, user.CreatedAt)
	}
	n, err := s.CountUsers(ctx)
	if err != nil {
		t.Fatalf("计数: %v", err)
	}
	if n != 1 {
		t.Fatalf("不该插成两个人，实得 %d", n)
	}
}

func TestSessionStoresOnlyTheHash(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)

	token, session, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-19T00:00:00Z"), core.SessionWeb, newSeqRandom(1))
	if err != nil {
		t.Fatalf("签发会话: %v", err)
	}

	if session.TokenHash == token {
		t.Fatal("库里不能是明文")
	}
	if session.TokenHash != core.HashSessionToken(token) {
		t.Fatalf("库里应当存 sha256(token)：\n实得 %s\n期望 %s",
			session.TokenHash, core.HashSessionToken(token))
	}

	back, err := s.GetSession(ctx, session.TokenHash)
	if err != nil {
		t.Fatalf("查会话: %v", err)
	}
	if back == nil || *back != session {
		t.Fatalf("查回来的会话不一致: %+v", back)
	}
	if !back.IsValidAt(at(t, "2026-09-18T12:00:00Z")) {
		t.Fatal("到期前应当有效")
	}
	// 拿明文当哈希去查必须查不到 —— 反证"没存明文"
	plain, err := s.GetSession(ctx, token)
	if err != nil {
		t.Fatalf("拿明文查: %v", err)
	}
	if plain != nil {
		t.Fatal("拿明文当哈希查到了东西，说明库里存了明文")
	}
}

func TestExpiredSessionsAreSweptAndFreshOnesSurvive(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)

	_, stale, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-17T00:00:00Z"), core.SessionCLI, newSeqRandom(1))
	if err != nil {
		t.Fatalf("签发过期的: %v", err)
	}
	_, fresh, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-20T00:00:00Z"), core.SessionCLI, newSeqRandom(2))
	if err != nil {
		t.Fatalf("签发未过期的: %v", err)
	}

	removed, err := s.DeleteExpiredSessions(ctx, at(t, "2026-09-18T00:00:00Z"))
	if err != nil {
		t.Fatalf("清理: %v", err)
	}
	if removed != 1 {
		t.Fatalf("只该清掉过期那一条，实得 %d", removed)
	}
	got, err := s.GetSession(ctx, stale.TokenHash)
	if err != nil {
		t.Fatalf("查过期的: %v", err)
	}
	if got != nil {
		t.Fatal("过期的那条应当被清掉")
	}
	got, err = s.GetSession(ctx, fresh.TokenHash)
	if err != nil {
		t.Fatalf("查未过期的: %v", err)
	}
	if got == nil {
		t.Fatal("未过期的那条不该被动")
	}
}

// TestSessionExpiringExactlyNowIsTreatedAsExpired 与
// [core.Session.IsExpiredAt]（`<=`）保持一致：宁可让用户重登，
// 也不要放行一个刚好到点的会话。
func TestSessionExpiringExactlyNowIsTreatedAsExpired(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)
	_, edge, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-18T00:00:00Z"), core.SessionCLI, newSeqRandom(3))
	if err != nil {
		t.Fatalf("签发: %v", err)
	}

	removed, err := s.DeleteExpiredSessions(ctx, at(t, "2026-09-18T00:00:00Z"))
	if err != nil {
		t.Fatalf("清理: %v", err)
	}
	if removed != 1 {
		t.Fatalf("刚好到点也算过期，实得清理 %d 条", removed)
	}
	got, err := s.GetSession(ctx, edge.TokenHash)
	if err != nil {
		t.Fatalf("查: %v", err)
	}
	if got != nil {
		t.Fatal("刚好到点的会话应当已被清掉")
	}
}

// TestLogoutRemovesExactlyOneSession：登出 WebUI 不该顺手把 CLI 的登录态一起干掉。
func TestLogoutRemovesExactlyOneSession(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)

	_, web, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-19T00:00:00Z"), core.SessionWeb, newSeqRandom(1))
	if err != nil {
		t.Fatalf("签发 web: %v", err)
	}
	_, cli, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-19T00:00:00Z"), core.SessionCLI, newSeqRandom(2))
	if err != nil {
		t.Fatalf("签发 cli: %v", err)
	}

	ok, err := s.DeleteSession(ctx, web.TokenHash)
	if err != nil {
		t.Fatalf("登出: %v", err)
	}
	if !ok {
		t.Fatal("登出应当真的删掉一行")
	}
	ok, err = s.DeleteSession(ctx, web.TokenHash)
	if err != nil {
		t.Fatalf("重复登出: %v", err)
	}
	if ok {
		t.Fatal("重复删除应返回 false")
	}
	got, err := s.GetSession(ctx, cli.TokenHash)
	if err != nil {
		t.Fatalf("查 cli: %v", err)
	}
	if got == nil {
		t.Fatal("登出 WebUI 不该顺手把 CLI 的登录态一起干掉")
	}
}

// TestAllSessionsOfOneUserCanBeRevokedWithoutTouchingOthers：只清自己的，
// 别人的会话不能动。
func TestAllSessionsOfOneUserCanBeRevokedWithoutTouchingOthers(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	alice := makeUser(t, s, "alice", core.RoleAdmin)
	bob := makeUser(t, s, "bob", core.RoleUser)

	for seed := byte(1); seed <= 3; seed++ {
		if _, _, err := IssueSession(ctx, s, alice.ID,
			at(t, "2026-09-19T00:00:00Z"), core.SessionWeb, newSeqRandom(seed)); err != nil {
			t.Fatalf("签发 alice 的会话 %d: %v", seed, err)
		}
	}
	_, bobSession, err := IssueSession(ctx, s, bob.ID,
		at(t, "2026-09-19T00:00:00Z"), core.SessionWeb, newSeqRandom(9))
	if err != nil {
		t.Fatalf("签发 bob 的会话: %v", err)
	}

	n, err := s.DeleteUserSessions(ctx, alice.ID)
	if err != nil {
		t.Fatalf("撤销 alice: %v", err)
	}
	if n != 3 {
		t.Fatalf("应撤销 3 条，实得 %d", n)
	}
	n, err = s.DeleteUserSessions(ctx, alice.ID)
	if err != nil {
		t.Fatalf("重复撤销: %v", err)
	}
	if n != 0 {
		t.Fatalf("重复撤销应为 0，实得 %d", n)
	}
	got, err := s.GetSession(ctx, bobSession.TokenHash)
	if err != nil {
		t.Fatalf("查 bob 的会话: %v", err)
	}
	if got == nil {
		t.Fatal("只清自己的，别人的会话不能动")
	}
}

// TestIssueSessionDoesNotStoreThePlaintextToken 单独钉住那条不变式：
// **漏掉哈希就是明文入库**，而这种事既不会编译失败也不会运行报错。
//
// 与 `TestSessionStoresOnlyTheHash` 的区别：那条看的是返回值与库里的一致性，
// 这条直接在库上按**明文**找一遍 —— 万一哪天 `IssueSession` 被改成
// "先插明文再补哈希"，前一条仍可能绿，这一条会红。
func TestIssueSessionDoesNotStoreThePlaintextToken(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)

	token, _, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-19T00:00:00Z"), core.SessionWeb, newSeqRandom(5))
	if err != nil {
		t.Fatalf("签发: %v", err)
	}

	rows, err := s.db.QueryContext(ctx, "SELECT token_hash FROM sessions")
	if err != nil {
		t.Fatalf("直查: %v", err)
	}
	defer func() { _ = rows.Close() }()
	seen := 0
	for rows.Next() {
		var hash string
		if err := rows.Scan(&hash); err != nil {
			t.Fatalf("扫描: %v", err)
		}
		seen++
		if hash == token {
			t.Fatalf("库里出现了明文 token：%s", hash)
		}
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("遍历: %v", err)
	}
	if seen != 1 {
		t.Fatalf("应只有 1 条会话，实得 %d", seen)
	}
}

// TestSessionsCascadeWithTheUser 钉住外键的**实际效果**。
//
// ⚠ 这条测的是"pragma 真的生效了"：`foreign_keys(1)` 写在 DSN 里而不是开库后
// `Exec`，是因为 pragma 是**按连接**生效的。若哪天有人把它挪到 `Exec`，
// 连接池换一条连接之后外键就静默失效 —— 那时删用户会留下孤儿会话，
// 而它们仍然能被 `GetSession` 查出来（一次"用户已删但仍能登录"）。
func TestSessionsCascadeWithTheUser(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)
	_, session, err := IssueSession(ctx, s, user.ID,
		at(t, "2026-09-19T00:00:00Z"), core.SessionWeb, newSeqRandom(1))
	if err != nil {
		t.Fatalf("签发: %v", err)
	}

	if _, err := s.db.ExecContext(ctx, "DELETE FROM users WHERE id = ?", user.ID.String()); err != nil {
		t.Fatalf("删用户: %v", err)
	}

	// 级联删掉时这条查不到；没级联时它会留下（那时这条断言变红）
	got, err := s.GetSession(ctx, session.TokenHash)
	if err != nil {
		t.Fatalf("查会话: %v", err)
	}
	if got != nil {
		t.Fatal("删用户之后不该还有会话 —— 否则是「用户已删但仍能登录」")
	}
}

// TestSessionInsertRefusesADuplicateHash：`InsertSession` 刻意**不做 UPSERT**。
//
// `token_hash` 是 256 位随机值的哈希，撞上同一个值只可能是调用方拿旧 token
// 重复签发 —— 那是错误用法，报出来比悄悄覆盖好。
func TestSessionInsertRefusesADuplicateHash(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)
	session := core.NewSession(core.HashSessionToken("t"),
		user.ID, at(t, "2026-09-19T00:00:00Z"), core.SessionWeb)

	if err := s.InsertSession(ctx, &session); err != nil {
		t.Fatalf("首次插入: %v", err)
	}
	if err := s.InsertSession(ctx, &session); err == nil {
		t.Fatal("重复 token_hash 应当报错，而不是悄悄覆盖")
	}
}

// TestDeleteExpiredSessionsComparesByStringOrder 钉住"字典序等价于时间序"的前提。
//
// 写入与比较**都走** RFC 3339 + UTC 时这条成立；任一侧改成本地时间或另一种
// 布局，边界就会静默错位 —— 而错位的表现只是"某条会话多活了几小时"。
func TestDeleteExpiredSessionsComparesByStringOrder(t *testing.T) {
	ctx := context.Background()
	s := store(t)
	user := makeUser(t, s, "alice", core.RoleAdmin)

	// 跨年、跨月、跨日各一条，确保字符串比较在进位处也正确。
	//
	// ⚠ 每条用**不同的** seed：`token_hash` 上有唯一约束，同一个 seed 会产出
	// 同一个 token ⇒ 第二次插入直接报 UNIQUE 冲突（这本身就是那条约束在工作）。
	for i, expires := range []string{
		"2025-12-31T23:59:59Z",
		"2026-01-01T00:00:00Z",
		"2026-09-30T23:59:59Z",
		"2026-10-01T00:00:00Z",
	} {
		if _, _, err := IssueSession(ctx, s, user.ID,
			at(t, expires), core.SessionCLI, newSeqRandom(byte(i+1))); err != nil {
			t.Fatalf("签发 %s: %v", expires, err)
		}
	}

	removed, err := s.DeleteExpiredSessions(ctx, at(t, "2026-09-30T23:59:59Z"))
	if err != nil {
		t.Fatalf("清理: %v", err)
	}
	// 前三条 <= 截止时刻（含边界那条），第四条在之后
	if removed != 3 {
		t.Fatalf("应清掉 3 条，实得 %d", removed)
	}
}
