package app

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 假存储
// ─────────────────────────────────────────────────────────────────────────────

// userStore 是 [core.UserStore] 的**部分假实现**（未重写的方法一调到就 panic，
// 见 §6.1 #35）。
//
// ⚠ `sessions` 的键是 **token 的哈希** —— 与真实库一致。若这里图省事用明文
// 当键，下面那条"明文 token 从不落库"的断言就会空转（它比的就是这一列）。
type userStore struct {
	core.UserStore
	users    map[string]*core.User
	sessions map[string]*core.Session
}

func newUserStore() *userStore {
	return &userStore{users: map[string]*core.User{}, sessions: map[string]*core.Session{}}
}

func (s *userStore) UpsertUser(_ context.Context, user *core.User) error {
	copied := *user
	s.users[string(user.ID)] = &copied
	return nil
}

func (s *userStore) GetUserByName(_ context.Context, name string) (*core.User, error) {
	for _, u := range s.users {
		if u.Name == name {
			copied := *u
			return &copied, nil
		}
	}
	return nil, nil
}

func (s *userStore) GetUser(_ context.Context, id core.UserID) (*core.User, error) {
	if u, ok := s.users[string(id)]; ok {
		copied := *u
		return &copied, nil
	}
	return nil, nil
}

func (s *userStore) InsertSession(_ context.Context, session *core.Session) error {
	copied := *session
	s.sessions[session.TokenHash] = &copied
	return nil
}

func (s *userStore) GetSession(_ context.Context, tokenHash string) (*core.Session, error) {
	if session, ok := s.sessions[tokenHash]; ok {
		copied := *session
		return &copied, nil
	}
	return nil, nil
}

func (s *userStore) DeleteSession(_ context.Context, tokenHash string) (bool, error) {
	if _, ok := s.sessions[tokenHash]; !ok {
		return false, nil
	}
	delete(s.sessions, tokenHash)
	return true, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

func sessionNow() time.Time {
	return time.Date(2026, 9, 18, 0, 0, 0, 0, time.UTC)
}

// seededUserStore 造一个带一名用户的库。
func seededUserStore(t *testing.T, name, password string) *userStore {
	t.Helper()
	hash, err := core.HashPassword(password, &seqRandomApp{b: 3})
	if err != nil {
		t.Fatalf("哈希口令失败: %v", err)
	}
	store := newUserStore()
	user := core.NewUser("u-"+name, name, hash, core.RoleAdmin, sessionNow())
	if err := store.UpsertUser(context.Background(), &user); err != nil {
		t.Fatalf("写用户失败: %v", err)
	}
	return store
}

// ─────────────────────────────────────────────────────────────────────────────
// 登录
// ─────────────────────────────────────────────────────────────────────────────

// TestSignInThenTokenResolvesTheUser：签发明文 token → cookie 带回来 → 认出这个人。
func TestSignInThenTokenResolvesTheUser(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	user, token, session, perr := SignIn(context.Background(), store,
		"alice", "correct-horse", &seqRandomApp{b: 1}, sessionNow())
	if perr != nil {
		t.Fatalf("登录失败: %v", perr)
	}
	if token == "" {
		t.Fatal("应签发一个明文 token")
	}
	if user.Name != "alice" {
		t.Errorf("应认出 alice，实得 %q", user.Name)
	}
	if session.Kind != core.SessionWeb {
		t.Errorf("SignIn 应签 web 会话，实得 %v", session.Kind)
	}
	want := sessionNow().AddDate(0, 0, WebSessionDays)
	if !session.ExpiresAt.Equal(want) {
		t.Errorf("有效期应是 %d 天后，实得 %v", WebSessionDays, session.ExpiresAt)
	}

	got, gotSession, perr := CurrentByToken(context.Background(), store, token, sessionNow())
	if perr != nil {
		t.Fatalf("解析会话失败: %v", perr)
	}
	if got == nil {
		t.Fatal("刚签发的 token 应能解析出用户")
	}
	if got.Name != "alice" {
		t.Errorf("解析出的是 %q", got.Name)
	}
	if gotSession.TokenHash != core.HashSessionToken(token) {
		t.Error("会话行里存的应是 token 的哈希")
	}
}

// TestThePlaintextTokenNeverLandsInTheDatabase：
// 库里只有哈希，拿到库文件也无法直接冒充登录。
func TestThePlaintextTokenNeverLandsInTheDatabase(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	_, token, _, perr := SignIn(context.Background(), store,
		"alice", "correct-horse", &seqRandomApp{b: 1}, sessionNow())
	if perr != nil {
		t.Fatalf("登录失败: %v", perr)
	}
	for _, stored := range store.sessions {
		if strings.Contains(stored.TokenHash, token) {
			t.Fatalf("库里不该出现明文 token：%q", stored.TokenHash)
		}
		if stored.TokenHash != core.HashSessionToken(token) {
			t.Errorf("库里应是 sha256(token)，实得 %q", stored.TokenHash)
		}
	}
	if _, ok := store.sessions[token]; ok {
		t.Error("明文 token 不该能直接当键查到会话")
	}
}

// TestWrongPasswordAndUnknownUserAreIndistinguishable：
// 区分两者会让登录页变成用户名枚举器。
func TestWrongPasswordAndUnknownUserAreIndistinguishable(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	_, _, _, wrong := SignIn(context.Background(), store,
		"alice", "wrong-password", &seqRandomApp{b: 1}, sessionNow())
	_, _, _, unknown := SignIn(context.Background(), store,
		"nobody", "correct-horse", &seqRandomApp{b: 1}, sessionNow())
	if wrong == nil || unknown == nil {
		t.Fatal("两种都应失败")
	}
	if wrong.Message() != unknown.Message() {
		t.Errorf("文案必须逐字相同，实得 %q vs %q", wrong.Message(), unknown.Message())
	}
	if strings.Contains(wrong.Message(), "alice") || strings.Contains(unknown.Message(), "nobody") {
		t.Errorf("文案不该回显用户名，实得 %q", wrong.Message())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 过期 / 撤销
// ─────────────────────────────────────────────────────────────────────────────

// TestExpiredOrRevokedSessionIsTreatedAsLoggedOut：
// 没带 cookie、已过期、已撤销 —— 三种都导出同一个动作（去登录），刻意不区分。
func TestExpiredOrRevokedSessionIsTreatedAsLoggedOut(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	_, token, session, perr := SignIn(context.Background(), store,
		"alice", "correct-horse", &seqRandomApp{b: 1}, sessionNow())
	if perr != nil {
		t.Fatalf("登录失败: %v", perr)
	}

	// 到期**那一刻**就算过期（边界是闭的）
	atExpiry := session.ExpiresAt
	if user, _, _ := CurrentByToken(context.Background(), store, token, atExpiry); user != nil {
		t.Error("到期那一刻应算已过期 —— 宁可让人重登，也别放行一个刚好到点的会话")
	}
	// 到期前一刻还有效
	if user, _, _ := CurrentByToken(context.Background(), store, token, atExpiry.Add(-time.Second)); user == nil {
		t.Error("到期前还应有效")
	}
	// 撤销后（不是等它过期）
	if _, perr := SignOutToken(context.Background(), store, token); perr != nil {
		t.Fatalf("登出失败: %v", perr)
	}
	if user, _, _ := CurrentByToken(context.Background(), store, token, sessionNow()); user != nil {
		t.Error("撤销后应立刻失效，而不是等 cookie 自己过期")
	}
}

// TestSignOutRevokesImmediatelyAndIsIdempotent：
// 重复登出返回 false，不报错 —— 浏览器会重发。
func TestSignOutRevokesImmediatelyAndIsIdempotent(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	_, token, _, perr := SignIn(context.Background(), store,
		"alice", "correct-horse", &seqRandomApp{b: 1}, sessionNow())
	if perr != nil {
		t.Fatalf("登录失败: %v", perr)
	}
	first, perr := SignOutToken(context.Background(), store, token)
	if perr != nil {
		t.Fatalf("登出失败: %v", perr)
	}
	if !first {
		t.Error("第一次登出应真的删掉一行")
	}
	second, perr := SignOutToken(context.Background(), store, token)
	if perr != nil {
		t.Fatalf("重复登出不该报错: %v", perr)
	}
	if second {
		t.Error("重复登出应返回 false（没有再删掉东西）")
	}
}

// TestEmptyTokenIsNotASession：
// 空 token 直接算"没登录" —— `sha256("")` 是个合法哈希，
// 万一库里真有空串派生的那一行，就会变成"不带 cookie 也能登录"。
func TestEmptyTokenIsNotASession(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	// 故意在库里放一行"空串派生"的会话，看它能不能被命中
	blank := core.NewSession(core.HashSessionToken(""), core.NewUserID("u-alice"),
		sessionNow().Add(time.Hour), core.SessionWeb)
	if err := store.InsertSession(context.Background(), &blank); err != nil {
		t.Fatalf("写夹具失败: %v", err)
	}

	user, _, perr := CurrentByToken(context.Background(), store, "", sessionNow())
	if perr != nil {
		t.Fatalf("空 token 不该报错: %v", perr)
	}
	if user != nil {
		t.Error("空 token 决不能被当成一次登录")
	}
	// 登出同理：删空串的哈希可能命中别人
	deleted, perr := SignOutToken(context.Background(), store, "")
	if perr != nil {
		t.Fatalf("空 token 登出不该报错: %v", perr)
	}
	if deleted {
		t.Error("空 token 的登出不许删掉任何一行 —— 那等于没带 cookie 的人登出了别人")
	}
	if _, ok := store.sessions[core.HashSessionToken("")]; !ok {
		t.Error("上面那一趟不该动到库")
	}
}

// TestTwoUsersDoNotShareIdentity：两个人的 token 互不通用。
func TestTwoUsersDoNotShareIdentity(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	bob := core.NewUser("u-bob", "bob", store.users["u-alice"].PasswordHash,
		core.RoleUser, sessionNow())
	if err := store.UpsertUser(context.Background(), &bob); err != nil {
		t.Fatalf("写 bob 失败: %v", err)
	}
	_, aliceToken, _, perr := SignIn(context.Background(), store,
		"alice", "correct-horse", &seqRandomApp{b: 1}, sessionNow())
	if perr != nil {
		t.Fatalf("alice 登录失败: %v", perr)
	}
	_, bobToken, _, perr := SignIn(context.Background(), store,
		"bob", "correct-horse", &seqRandomApp{b: 2}, sessionNow())
	if perr != nil {
		t.Fatalf("bob 登录失败: %v", perr)
	}
	if aliceToken == bobToken {
		t.Fatal("两次签发的 token 不该相同")
	}
	who, _, perr := CurrentByToken(context.Background(), store, aliceToken, sessionNow())
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if who == nil || who.Name != "alice" {
		t.Errorf("alice 的 token 应认出 alice，实得 %v", who)
	}
}

// TestTheSessionRowRecordsTheKindThatIssuedIt：
// 库里那一行要如实记下"这是哪种会话" —— 有效期与携带方式由它决定。
func TestTheSessionRowRecordsTheKindThatIssuedIt(t *testing.T) {
	store := seededUserStore(t, "alice", "correct-horse")
	_, _, cliSession, perr := SignInWith(context.Background(), store,
		"alice", "correct-horse", core.SessionCLI, 365*24*time.Hour,
		&seqRandomApp{b: 1}, sessionNow())
	if perr != nil {
		t.Fatalf("登录失败: %v", perr)
	}
	if cliSession.Kind != core.SessionCLI {
		t.Errorf("库里应记下 cli，实得 %v", cliSession.Kind)
	}
	stored := store.sessions[cliSession.TokenHash]
	if stored == nil || stored.Kind != core.SessionCLI {
		t.Errorf("落库那一行应带上 kind，实得 %v", stored)
	}
	if stored.UserID != core.NewUserID("u-alice") {
		t.Errorf("会话行应绑到签发它的那个用户，实得 %q", stored.UserID)
	}
}
