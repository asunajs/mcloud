package capabilities

import (
	"context"
	"encoding/base64"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 粘贴 auth 落成账号这一组，测的是**两条链路必须一致**与**凭据不许外流**：
//
//   - 同一份凭据，登录拿到的与用户抄来的，拆出来的账号 id / 掩码 / 到期时间必须逐项相同。
//     不一致的表现是"明明登录过，粘贴之后变成另一个账号"（两份凭据各说半句）。
//   - 错误串是这条链路上唯一会走到响应体与日志里的文本，所以每条都断言它不含输入。

// ingestStore 是"添加账号"会碰到的三个存储入口。
//
// 与 [recordingStore] 同样的写法：内嵌接口，**没重写的方法一旦被调用就 panic** ——
// 粘贴路径不该碰 smid、不该碰运行记录，碰了就该当场响。
type ingestStore struct {
	core.StateStore
	order    []string
	existing *core.Account
}

func (s *ingestStore) GetAccount(_ context.Context, _ core.Scope, _ core.AccountID) (*core.Account, error) {
	return s.existing, nil
}

func (s *ingestStore) UpsertAccount(_ context.Context, _ core.Scope, _ *core.Account) error {
	s.order = append(s.order, "account")
	return nil
}

func (s *ingestStore) UpsertAccountState(_ context.Context, _ core.Scope, _ *core.AccountState) error {
	s.order = append(s.order, "state")
	return nil
}

func scopeForTest() core.Scope {
	owner := core.NewUserID("user-1")
	return core.ScopeUser(&owner)
}

// pastedAuth 造一份"用户从别处抄来"的整串（与 userInfo() 那份登录结果同源）。
func pastedAuth(prefix string) string {
	return "Basic " + base64.StdEncoding.EncodeToString(
		[]byte(prefix+":19900000000:SYNTH001|1|RCS|1893456000000|AAAA"))
}

func TestPastedAuthSplitsTheSameWayAsALoginResult(t *testing.T) {
	scope := scopeForTest()
	fromLogin, stateLogin, err := AccountAndStateFromLoginWithPrefix(
		scope, "dev-1", userInfo(), PCAuthPrefix, now())
	if err != nil {
		t.Fatalf("登录侧构造失败: %v", err)
	}
	fromPaste, statePaste, err := AccountAndStateFromAuth(scope, pastedAuth(PCAuthPrefix), now())
	if err != nil {
		t.Fatalf("粘贴侧构造失败: %v", err)
	}

	// 逐字段比：这些字段决定"两条链路是不是在往同一行上写"
	if fromPaste.ID != fromLogin.ID {
		t.Errorf("账号 id 不一致：%s vs %s", fromPaste.ID, fromLogin.ID)
	}
	if fromPaste.ID.String() == "" {
		t.Error("账号 id 为空")
	}
	if fromPaste.Platform == nil || *fromPaste.Platform != *fromLogin.Platform {
		t.Errorf("端别不一致：%s vs %s", strOf(fromPaste.Platform), strOf(fromLogin.Platform))
	}
	if fromPaste.PhoneMasked == nil || *fromPaste.PhoneMasked != *fromLogin.PhoneMasked {
		t.Errorf("掩码不一致：%s vs %s", strOf(fromPaste.PhoneMasked), strOf(fromLogin.PhoneMasked))
	}
	if fromPaste.AuthExpire == nil || *fromPaste.AuthExpire != *fromLogin.AuthExpire {
		t.Errorf("到期时间不一致：%v vs %v", fromPaste.AuthExpire, fromLogin.AuthExpire)
	}
	if statePaste.Authorization == nil || stateLogin.Authorization == nil {
		t.Fatal("两侧的 authorization 都不该为空")
	}
	// ⚠ 比的是 `.Value()` 而不是 `.String()` —— 后者的 Stringer **一律脱敏**
	// （`<redacted N 字符>`），拿它做相等断言等于只比了长度，凭据内容不同也照样绿。
	if statePaste.Authorization.Value() != stateLogin.Authorization.Value() {
		t.Error("同一份凭据拆出了两个字符串")
	}
	if statePaste.JWTToken.Value() != stateLogin.JWTToken.Value() {
		t.Error("同一份 token 拆出了两个字符串")
	}

	// 粘贴路径**没有**的东西必须保持没有：填一个猜出来的值，
	// 到了上游报 4xx 那天没人知道是"库里那列本来就是假的"。
	if statePaste.DeviceID != nil {
		t.Errorf("粘贴路径不该有 deviceId：%v", statePaste.DeviceID)
	}
	if statePaste.UserDomainID != nil {
		t.Errorf("粘贴路径不该有 userDomainId：%v", statePaste.UserDomainID)
	}
	if stateLogin.DeviceID == nil {
		t.Error("对照项坏了：登录路径应有 deviceId")
	}
}

func TestPastingWithOrWithoutTheBasicPrefixLandsTheSameString(t *testing.T) {
	scope := scopeForTest()
	withPrefix, stateA, err := AccountAndStateFromAuth(scope, pastedAuth(PCAuthPrefix), now())
	if err != nil {
		t.Fatalf("带前缀粘贴失败: %v", err)
	}
	bare := strings.TrimPrefix(pastedAuth(PCAuthPrefix), "Basic ")
	withoutPrefix, stateB, err := AccountAndStateFromAuth(scope, bare, now())
	if err != nil {
		t.Fatalf("不带前缀粘贴失败: %v", err)
	}

	if withPrefix.ID != withoutPrefix.ID {
		t.Errorf("同一个手机号抄成两次会撞成两个账号：%s vs %s", withPrefix.ID, withoutPrefix.ID)
	}
	if stateA.Authorization.Value() != stateB.Authorization.Value() {
		t.Error("落库的凭据整串随输入写法漂移")
	}
	if !strings.HasPrefix(stateB.Authorization.Value(), "Basic ") {
		t.Error("落库形状必须是 `Basic <base64>`，否则发请求时还得再拼一次")
	}
}

// App 端抄来的串照样接受（前缀自带、不设闸），但端别要照实记下来。
func TestPastedAppCredentialKeepsItsOwnPlatform(t *testing.T) {
	scope := scopeForTest()
	account, _, err := AccountAndStateFromAuth(scope, pastedAuth(upstream.IOSAuthPrefix), now())
	if err != nil {
		t.Fatalf("粘贴 App 串失败: %v", err)
	}
	if account.Platform == nil || *account.Platform != "mobile" {
		t.Errorf("端别应照抄字符串自带的那一段：%v", account.Platform)
	}
}

func TestUnusableAuthIsRejectedWithoutEchoingTheInput(t *testing.T) {
	scope := scopeForTest()
	// 两种"泄露形态"都要盯：原样的 base64，与解码后的载荷
	secretB64 := base64.StdEncoding.EncodeToString([]byte("pc:19900000000:SECRET-TOKEN"))

	cases := []struct {
		name, input string
	}{
		{"空串", ""},
		{"只有空白", "   "},
		{"不是 base64", "Basic 不是base64@@@"},
		{"没有手机号", "Basic " + base64.StdEncoding.EncodeToString([]byte("pc::SECRET-TOKEN"))},
		{"没有 token", "Basic " + base64.StdEncoding.EncodeToString([]byte("pc:19900000000:"))},
		{"只有 base64 但不是 auth 形状", "Basic " + base64.StdEncoding.EncodeToString([]byte("你好"))},
	}
	for _, tc := range cases {
		_, _, err := AccountAndStateFromAuth(scope, tc.input, now())
		if err == nil {
			t.Errorf("%s：应当被拒", tc.name)
			continue
		}
		// 补记（2026-09-22）：这里原本断 Parse。Parse 的语义是"上游响应不合形状"
		// （不重试、按服务端故障处理），而这一组全是"你贴的这串用不了" ——
		// HTTP 那一层据此给 500 + 类别文案，使用者看不到能改的原因。
		// 与下面那条"已存在 = BadRequest"统一成 BadRequest。
		if err.Kind() != core.KindBadRequest {
			t.Errorf("%s：类别应是 BadRequest（使用者的输入问题），实得 %v", tc.name, err.Kind())
		}
		// 这一条整组最重要：错误串会被写进响应体，也会进日志。
		if tc.input != "" && strings.Contains(err.Error(), tc.input) {
			t.Errorf("%s：错误串里回显了凭据本体", tc.name)
		}
		if strings.Contains(err.Error(), secretB64) || strings.Contains(err.Error(), "SECRET-TOKEN") {
			t.Errorf("%s：错误串里回显了凭据（base64 或解码后的载荷）", tc.name)
		}
	}
}

func TestPersistAuthRefusesToOverwriteAnExistingAccount(t *testing.T) {
	scope := scopeForTest()
	// 用粘贴路径自己算出来的那个 id 造"库里有这一行"：
	// 手写掩码会让断言跟着 MaskPhone 的口径漂移（漂移那天这条用例会假绿）。
	pasted, _, derr := AccountAndStateFromAuth(scope, pastedAuth(PCAuthPrefix), now())
	if derr != nil {
		t.Fatalf("先构造出对照账号: %v", derr)
	}
	existing := core.NewAccount(pasted.ID.String())

	// 反向改坏这里的方式：把 PersistAuth 里那段"已存在就报错"删掉
	store := &ingestStore{existing: &existing}
	_, err := PersistAuth(context.Background(), store, scope, pastedAuth(PCAuthPrefix), now())
	if err == nil {
		t.Fatal("已存在的账号应当被拒绝，而不是被覆盖")
	}
	if err.Kind() != core.KindBadRequest {
		t.Errorf("类别应是 BadRequest（用户的输入问题），实得 %v", err.Kind())
	}
	if len(store.order) != 0 {
		t.Errorf("被拒之后一次写入都不该发生：%v", store.order)
	}
}

func TestPersistAuthWritesAccountBeforeItsCredentials(t *testing.T) {
	scope := scopeForTest()
	store := &ingestStore{}
	id, err := PersistAuth(context.Background(), store, scope, pastedAuth(PCAuthPrefix), now())
	if err != nil {
		t.Fatalf("落库失败: %v", err)
	}
	if id.String() == "" {
		t.Fatal("应返回账号 id")
	}
	want := []string{"account", "state"}
	if len(store.order) != len(want) || store.order[0] != want[0] || store.order[1] != want[1] {
		t.Errorf("写入顺序应为 %v，实得 %v（凭据那条会校验账号归属，反了会被拒）", want, store.order)
	}
}

func TestPersistAuthRejectsBeforeTouchingTheStore(t *testing.T) {
	scope := scopeForTest()
	store := &ingestStore{}
	if _, err := PersistAuth(context.Background(), store, scope, "Basic 乱码@@", now()); err == nil {
		t.Fatal("解析不出来的串应当被拒")
	}
	if len(store.order) != 0 {
		t.Errorf("解析失败时一次写入都不该发生：%v", store.order)
	}
}

// strOf 取指针指向的文本，取不到给 `<nil>` —— 只为了失败信息可读。
func strOf(p *string) string {
	if p == nil {
		return "<nil>"
	}
	return *p
}
