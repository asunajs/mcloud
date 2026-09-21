package app

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// fakeUserStore 用**内嵌接口**做部分假实现：没重写的方法一调用就 panic，
// 于是"初始化碰了它不该碰的存储入口"会当场响。
//
// 它同时记录写入次数 —— "失败不该留下半成品"这类断言只能靠数写次数来证明。
type fakeUserStore struct {
	core.StateStore
	core.UserStore

	users     map[string]core.User
	claimRows core.ClaimedRows
	claimErr  error
	writes    int
	claims    int
}

func newFakeUserStore() *fakeUserStore {
	return &fakeUserStore{users: make(map[string]core.User)}
}

func (s *fakeUserStore) CountUsers(_ context.Context) (uint64, error) {
	return uint64(len(s.users)), nil
}

func (s *fakeUserStore) UpsertUser(_ context.Context, user *core.User) error {
	s.writes++
	s.users[strings.ToLower(user.Name)] = *user
	return nil
}

func (s *fakeUserStore) GetUserByName(_ context.Context, name string) (*core.User, error) {
	if user, ok := s.users[strings.ToLower(name)]; ok {
		return &user, nil
	}
	return nil, nil
}

func (s *fakeUserStore) ClaimUnowned(_ context.Context, _ core.UserID) (core.ClaimedRows, error) {
	s.claims++
	if s.claimErr != nil {
		return core.ClaimedRows{}, s.claimErr
	}
	return s.claimRows, nil
}

func atInit() time.Time { return time.Date(2026, 9, 18, 0, 0, 0, 0, time.UTC) }

func initRng() *seqRng {
	return &seqRng{script: []byte{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16}}
}

func TestInitCreatesAnAdminAndReportsTheClaimedCount(t *testing.T) {
	store := newFakeUserStore()
	outcome, perr := InitFirstAdmin(context.Background(), store,
		"admin", "correct-horse", initRng(), atInit())
	if perr != nil {
		t.Fatalf("初始化失败: %v", perr)
	}
	if outcome.User.Name != "admin" {
		t.Errorf("用户名应原样保留，实得 %q", outcome.User.Name)
	}
	if !outcome.User.IsAdmin() {
		t.Error("首个用户必须是管理员 —— 否则没有任何人能建第二个人")
	}
	if outcome.Claimed != (core.ClaimedRows{}) {
		t.Errorf("新库没有无主账号，实得 %+v", outcome.Claimed)
	}
	if store.writes != 1 {
		t.Errorf("应只写一次用户，实得 %d", store.writes)
	}
	if store.claims != 1 {
		t.Errorf("应认领一次，实得 %d", store.claims)
	}

	// 落库了，而且口令能验回来（口令只以 argon2id 编码串形态存储）
	stored, err := store.GetUserByName(context.Background(), "admin")
	if err != nil || stored == nil {
		t.Fatalf("应按名字查到，实得 %v / %v", stored, err)
	}
	ok, err := core.VerifyPassword("correct-horse", stored.PasswordHash)
	if err != nil || !ok {
		t.Errorf("口令应能验回来: ok=%v err=%v", ok, err)
	}
	if string(stored.PasswordHash) == "correct-horse" {
		t.Error("口令绝不能明文存 —— 那就是把口令抄了一份在库里")
	}
}

// TestInitRefusesToRunTwice：加第二个用户是**另一件事**（涉及角色权限），
// 不能靠再跑一次初始化 —— 那会让"手滑多跑一次"变成"系统里多出一个管理员"。
func TestInitRefusesToRunTwice(t *testing.T) {
	store := newFakeUserStore()
	if _, perr := InitFirstAdmin(context.Background(), store,
		"admin", "correct-horse", initRng(), atInit()); perr != nil {
		t.Fatalf("首次失败: %v", perr)
	}
	_, perr := InitFirstAdmin(context.Background(), store,
		"admin2", "correct-horse", initRng(), atInit())
	if perr == nil {
		t.Fatal("第二次初始化必须被拒")
	}
	if !strings.Contains(perr.Message(), "已经有 1 个用户") {
		t.Errorf("报错要说出**有几个**，实得 %q", perr.Message())
	}
	// 而且不能真的建出第二个人
	if got := len(store.users); got != 1 {
		t.Errorf("不该建出第二个人，实得 %d 个", got)
	}
	if store.writes != 1 {
		t.Errorf("第二次一次写都不该发生，实得 %d 次", store.writes)
	}
}

// TestInitRejectsWeakPasswordBeforeWritingAnything：
// 短口令必须在**写库之前**被拒 —— 否则留下一个"建了但登不进去"的半成品。
func TestInitRejectsWeakPasswordBeforeWritingAnything(t *testing.T) {
	store := newFakeUserStore()
	_, perr := InitFirstAdmin(context.Background(), store,
		"admin", "short", initRng(), atInit())
	if perr == nil {
		t.Fatal("短口令必须被拒")
	}
	if !strings.Contains(perr.Message(), "至少") {
		t.Errorf("报错要说出下限，实得 %q", perr.Message())
	}
	if store.writes != 0 {
		t.Errorf("失败不该留下半成品用户，实得 %d 次写入", store.writes)
	}
	if store.claims != 0 {
		t.Errorf("还没建人就不该认领，实得 %d 次", store.claims)
	}
}

func TestInitRejectsBadUsernames(t *testing.T) {
	for _, bad := range []string{"", "   ", "a b", "名字里有 空格"} {
		store := newFakeUserStore()
		_, perr := InitFirstAdmin(context.Background(), store,
			bad, "correct-horse", initRng(), atInit())
		if perr == nil {
			t.Errorf("%q 应被拒", bad)
			continue
		}
		if perr.Kind() != core.KindBadRequest {
			t.Errorf("%q 应是参数错误，实得 %v", bad, perr.Kind())
		}
		if store.writes != 0 {
			t.Errorf("%q 不该写库，实得 %d 次", bad, store.writes)
		}
	}
}

// TestInitTrimsTheNameButKeepsTheIdAligned：
// 名字前后空白要 trim，而 id 与 name 必须**同一个值** ——
// 变形（加哈希后缀之类）会把"改名"变成"换人"。
func TestInitTrimsTheNameButKeepsTheIdAligned(t *testing.T) {
	store := newFakeUserStore()
	outcome, perr := InitFirstAdmin(context.Background(), store,
		"  admin  ", "correct-horse", initRng(), atInit())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if outcome.User.Name != "admin" {
		t.Errorf("名字应 trim，实得 %q", outcome.User.Name)
	}
	if outcome.User.ID.String() != outcome.User.Name {
		t.Errorf("id 与 name 必须是同一个值，实得 %q 与 %q",
			outcome.User.ID.String(), outcome.User.Name)
	}
	if got := UserIDFromName("alice").String(); got != "alice" {
		t.Errorf("id 不做任何变形，实得 %q", got)
	}
}

// TestInitClaimsHistoryAfterTheUserExists：
// 认领发生在用户**写进去之后** —— 顺序反了会认领给一个尚不存在的 owner。
func TestInitClaimsHistoryAfterTheUserExists(t *testing.T) {
	store := newFakeUserStore()
	store.claimRows = core.ClaimedRows{Accounts: 3, Schedules: 2, Pipelines: 1}
	outcome, perr := InitFirstAdmin(context.Background(), store,
		"admin", "correct-horse", initRng(), atInit())
	if perr != nil {
		t.Fatalf("%v", perr)
	}
	if outcome.Claimed.Total() != 6 {
		t.Errorf("三笔应分别报出来再求和，实得 %+v", outcome.Claimed)
	}
	if outcome.Claimed.Accounts != 3 || outcome.Claimed.Schedules != 2 ||
		outcome.Claimed.Pipelines != 1 {
		t.Errorf("三笔必须分开（CLI 要分行打印），实得 %+v", outcome.Claimed)
	}
}

// TestInitReportsAClaimFailureButKeepsTheUser：
// 用户**已经写进去了**，所以认领失败不能当"什么都没发生" ——
// 那会让人再跑一次 init，而第二次必然被"已经有用户了"挡住，
// 于是这个账号永远认领不到自己的历史数据，且没有任何报错。
func TestInitReportsAClaimFailureButKeepsTheUser(t *testing.T) {
	store := newFakeUserStore()
	store.claimErr = errors.New("数据库只读")

	outcome, perr := InitFirstAdmin(context.Background(), store,
		"admin", "correct-horse", initRng(), atInit())
	if perr == nil {
		t.Fatal("认领失败必须报出来")
	}
	if outcome.User.Name != "admin" {
		t.Errorf("人已经建好了，结果里必须带着他 —— 否则调用方无从知道"+
			"「再跑一次 init」是错的方向，实得 %q", outcome.User.Name)
	}
	if store.writes != 1 {
		t.Errorf("用户应已落库，实得 %d 次写入", store.writes)
	}
}
