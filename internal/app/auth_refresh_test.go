package app

import (
	"context"
	"encoding/base64"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/capabilities"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 驱动层这一组只测**两端各自的那套头**与**什么时候一次都不发**：
//
//   - 端别由库里那一串 `Authorization` 决定，不由调用方指定 —— 走错支的表现是
//     "PC 的 token 配 App 的前缀"，而上游只回一个像"未登录"的错。
//   - 没到提前量 / 只能人工换 auth 两档下**零请求**：这两档都是正常结论，
//     发了就是白打上游，而且刷新会轮换真凭据。
//   - 报告里没有凭据（它会进 JSON 响应），所以每条断言都顺手检查错误串与报告文本。
//
// 判据本身（三档）在 capabilities 那组测；请求形状在 upstream 那组测。

// refreshStore 在 [fakeStore] 之上加"换凭据"的记录。
type refreshStore struct {
	fakeStore
	rotations []rotateCall
}

type rotateCall struct {
	auth   string
	jwt    string
	expire *int64
	at     time.Time
}

func (s *refreshStore) RotateAccountAuth(
	_ context.Context, _ core.Scope, _ core.AccountID,
	authorization, jwtToken core.SecretValue, authExpire *int64, at time.Time,
) error {
	s.rotations = append(s.rotations, rotateCall{
		auth: string(authorization), jwt: string(jwtToken), expire: authExpire, at: at,
	})
	return nil
}

// refreshAuthOf 造一份整串（端别 + 手机号 + token）。
func refreshAuthOf(prefix, phone, token string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte(prefix+":"+phone+":"+token))
}

// expireToken 造一个"从现在起 offset 后过期"的 token（第 4 段是**毫秒**）。
func expireToken(offset time.Duration) string {
	return "SYNTH|1|RCS|" + itoa64(testClock().Now().Add(offset).UnixMilli()) + "|AAAA"
}

// refreshDeps 把两端都备齐：一次运行只会用到其中一支，另一支留着不影响断言。
func refreshDeps(client core.HTTPClient, store *refreshStore) *RefreshDeps {
	device := upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3")
	return &RefreshDeps{
		Upstream:   upstream.NewUpstream(client),
		Store:      store,
		Scope:      core.ScopeLocal(),
		Clock:      testClock(),
		Random:     testRng(),
		AppDevice:  &device,
		PCPlatform: upstream.PlatformMacOS,
	}
}

// seededStore 造一份"能刷"的登录态：整串 + 用户域 + 设备标识。
func seededStore(prefix string, offset time.Duration) *refreshStore {
	auth := core.SecretValue(refreshAuthOf(prefix, "19900000000", expireToken(offset)))
	domain := core.UserDomainID("1990000000000000001")
	device := core.SecretValue("67F4C56A00000000000000000000315C")
	return &refreshStore{fakeStore: fakeStore{state: &core.AccountState{
		AccountID:     core.AccountIDFor("19900000000", nil),
		Authorization: &auth,
		UserDomainID:  &domain,
		DeviceID:      &device,
	}}}
}

func refreshOK(newToken string) string {
	return `{"success":true,"code":"0000","message":"请求成功","data":` +
		`{"token":"` + newToken + `","expireTime":2592000}}`
}

func TestAppRefreshSendsTheStoredCredentialAndRotatesIt(t *testing.T) {
	store := seededStore(upstream.IOSAuthPrefix, 2*24*time.Hour)
	client := &scriptedUpstream{responses: []string{refreshOK(expireToken(30 * 24 * time.Hour))}}
	deps := refreshDeps(client, store)

	report, err := RefreshAccountAuth(context.Background(), deps, store.state.AccountID)
	if err != nil {
		t.Fatalf("刷新失败: %v", err)
	}
	if client.count(upstream.RefreshTokenPath) != 1 {
		t.Fatalf("刷新请求发了 %d 次（路径 %v）", client.count(upstream.RefreshTokenPath), client.urls())
	}
	request := client.requests[0]
	if headerText(request, "Authorization") != string(*store.state.Authorization) {
		t.Error("发出去的 Authorization 不是库里那一整串")
	}
	// 用户域同时出现在体里和 `x-yun-uni` 头里（实测两处都是它）
	if !strings.Contains(string(string(request.Body)), `"userDomainId":"1990000000000000001"`) {
		t.Errorf("请求体没带上用户域：%s", redactBody(string(request.Body)))
	}
	if headerText(request, "x-yun-uni") != "1990000000000000001" {
		t.Errorf("App 支少了 `x-yun-uni`：%q", headerText(request, "x-yun-uni"))
	}
	if !strings.Contains(string(string(request.Body)), `"clientType":"`+upstream.IOSClientType+`"`) {
		t.Errorf("App 支的 clientType 不是 %s：%s", upstream.IOSClientType, redactBody(string(request.Body)))
	}

	if !report.Rotated || report.Decision != capabilities.RefreshNeeded {
		t.Fatalf("结论是 %v / Rotated=%v", report.Decision, report.Rotated)
	}
	if report.End != upstream.IOSAuthPrefix {
		t.Errorf("End 实得 %q", report.End)
	}
	if len(store.rotations) != 1 {
		t.Fatalf("写库 %d 次", len(store.rotations))
	}
	call := store.rotations[0]
	if call.jwt != expireToken(30*24*time.Hour) {
		t.Error("jwt_token 没换成上游给的那个 token")
	}
	decoded := decodeRefreshAuth(t, call.auth)
	if !strings.HasPrefix(decoded, upstream.IOSAuthPrefix+":19900000000:") {
		t.Errorf("新串端别/手机号被改了：%q", redactBody(decoded))
	}
	if !strings.HasSuffix(decoded, "|AAAA") {
		t.Error("新串的 token 段不是新来的那个")
	}
	if call.expire == nil || *call.expire != testClock().Now().Add(30*24*time.Hour).UnixMilli() {
		t.Errorf("auth_expire 没跟上新串：%v", call.expire)
	}
	if !call.at.Equal(testClock().Now()) {
		t.Error("updated_at 没用注入的时钟")
	}
}

// 两端同一个接口、同一份库判据，差别只在头：这一条钉住"走对支"。
func TestPCRefreshUsesThePCProfileAndItsOwnClientType(t *testing.T) {
	store := seededStore(capabilities.PCAuthPrefix, 2*24*time.Hour)
	client := &scriptedUpstream{responses: []string{refreshOK(expireToken(30 * 24 * time.Hour))}}
	deps := refreshDeps(client, store)

	report, err := RefreshAccountAuth(context.Background(), deps, store.state.AccountID)
	if err != nil {
		t.Fatalf("刷新失败: %v", err)
	}
	request := client.requests[0]
	if client.count(upstream.RefreshTokenPath) != 1 {
		t.Fatalf("发了 %d 次", client.count(upstream.RefreshTokenPath))
	}
	// 平台由 deps 给（这里给 macOS），**不**是 App 的 412
	if want := deps.PCPlatform.ClientType(); !strings.Contains(
		string(string(request.Body)), `"clientType":"`+want+`"`) {
		t.Errorf("clientType 不是档案平台那个：%s", redactBody(string(request.Body)))
	}
	if strings.Contains(string(string(request.Body)), upstream.IOSClientType) {
		t.Errorf("PC 支带上了 App 的 clientType：%s", redactBody(string(request.Body)))
	}
	if headerText(request, "x-yun-tid") == "" {
		t.Error("PC 支少了 `x-yun-tid`（每个会话都要一个新的）")
	}
	if !strings.Contains(headerText(request, "x-DeviceInfo"), "67F4C56A") {
		t.Error("PC 档案没用库里的 `device_id`")
	}
	if report.End != capabilities.PCAuthPrefix {
		t.Errorf("End 实得 %q", report.End)
	}
	if len(store.rotations) != 1 {
		t.Fatalf("写库 %d 次", len(store.rotations))
	}
	if got := decodeRefreshAuth(t, store.rotations[0].auth); !strings.HasPrefix(got, "pc:") {
		t.Errorf("新串端别被换成了 App：%q", redactBody(got))
	}
}

func TestRefreshUnitsAreMillisOnBothSides(t *testing.T) {
	// `expireTime`（上游给的**秒**、而且是"还能用多久"）绝不能漏进报告：
	// 报告里两列都必须是 epoch 毫秒，且是 13 位。
	store := seededStore(upstream.IOSAuthPrefix, 2*24*time.Hour)
	client := &scriptedUpstream{responses: []string{refreshOK(expireToken(30 * 24 * time.Hour))}}
	report, err := RefreshAccountAuth(
		context.Background(), refreshDeps(client, store), store.state.AccountID)
	if err != nil {
		t.Fatalf("刷新失败: %v", err)
	}
	for name, value := range map[string]*int64{"before": report.BeforeExpire, "after": report.AfterExpire} {
		if value == nil {
			t.Fatalf("%s 是 nil", name)
		}
		if n := len(itoa64(*value)); n != 13 {
			t.Errorf("%s 应是 13 位毫秒，实得 %d 位：%d", name, n, *value)
		}
	}
	if report.AfterExpire != nil && *report.AfterExpire <= *report.BeforeExpire {
		t.Error("刷完过期时刻没往后挪（说明落的是旧串）")
	}
}

func TestNotYetAndManualOnlySendNothing(t *testing.T) {
	cases := []struct {
		name     string
		offset   time.Duration
		token    string
		want     capabilities.RefreshDecision
		wantCall int
	}{
		{"还有 30 天", 30 * 24 * time.Hour, "", capabilities.RefreshNotYet, 0},
		{"过期时刻为负", 0, "SYNTH|1|RCS|-1|AAAA", capabilities.RefreshManualOnly, 0},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			auth := refreshAuthOf(upstream.IOSAuthPrefix, "19900000000",
				func() string {
					if c.token != "" {
						return c.token
					}
					return expireToken(c.offset)
				}())
			current := core.SecretValue(auth)
			domain := core.UserDomainID("1990000000000000001")
			store := &refreshStore{fakeStore: fakeStore{state: &core.AccountState{
				AccountID: core.NewAccountID("199****0000"), Authorization: &current, UserDomainID: &domain,
			}}}
			client := &scriptedUpstream{responses: []string{refreshOK("WHATEVER")}}
			report, err := RefreshAccountAuth(
				context.Background(), refreshDeps(client, store), store.state.AccountID)
			if err != nil {
				t.Fatalf("这两档都是正常结论，不该报错: %v", err)
			}
			if report.Decision != c.want {
				t.Errorf("实得 %v，应为 %v", report.Decision, c.want)
			}
			if len(client.requests) != c.wantCall {
				t.Errorf("发了 %d 个请求，应为 %d", len(client.requests), c.wantCall)
			}
			if len(store.rotations) != 0 || report.Rotated {
				t.Error("没到点却还是换了库里的凭据")
			}
		})
	}
}

func TestMissingDeviceProfileFailsBeforeAnyRequest(t *testing.T) {
	cases := []struct {
		name       string
		prefix     string
		dropDevice bool
		dropApp    bool
		wantHint   string
	}{
		{"App 支没有设备档案", upstream.IOSAuthPrefix, false, true, "App 设备档案"},
		{"PC 支没有 device_id", capabilities.PCAuthPrefix, true, false, "device_id"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			store := seededStore(c.prefix, 2*24*time.Hour)
			if c.dropDevice {
				store.state.DeviceID = nil
			}
			client := &scriptedUpstream{responses: []string{refreshOK("WHATEVER")}}
			deps := refreshDeps(client, store)
			if c.dropApp {
				deps.AppDevice = nil
			}
			_, err := RefreshAccountAuth(context.Background(), deps, store.state.AccountID)
			if err == nil {
				t.Fatal("缺档案应报错")
			}
			if !strings.Contains(err.Message(), c.wantHint) {
				t.Errorf("错误串没指出缺的是哪一样：%s", err.Message())
			}
			if len(client.requests) != 0 {
				t.Error("档案都不齐却还是发了请求")
			}
			if len(store.rotations) != 0 {
				t.Error("报错却还是写了库")
			}
		})
	}
}

func TestUnknownEndPrefixIsRefusedWithBothSupportedNames(t *testing.T) {
	store := seededStore("android", 2*24*time.Hour)
	client := &scriptedUpstream{responses: []string{refreshOK("WHATEVER")}}
	_, err := RefreshAccountAuth(context.Background(), refreshDeps(client, store), store.state.AccountID)
	if err == nil {
		t.Fatal("认不出的端别应拒绝")
	}
	if !strings.Contains(err.Message(), capabilities.PCAuthPrefix) ||
		!strings.Contains(err.Message(), upstream.IOSAuthPrefix) {
		t.Errorf("错误串没列出支持的两端：%s", err.Message())
	}
	if len(client.requests) != 0 {
		t.Error("端别都不认却还是发了请求")
	}
}

func TestMissingUserDomainIDStopsBeforeAnyRequest(t *testing.T) {
	store := seededStore(upstream.IOSAuthPrefix, 2*24*time.Hour)
	store.state.UserDomainID = nil
	client := &scriptedUpstream{responses: []string{refreshOK("WHATEVER")}}
	_, err := RefreshAccountAuth(context.Background(), refreshDeps(client, store), store.state.AccountID)
	if err == nil {
		t.Fatal("没有用户域时应拒绝（否则就是去刷别人的用户域）")
	}
	if len(client.requests) != 0 {
		t.Error("已经被拒绝，却还是发了请求")
	}
}

func TestUpstreamFailureKeepsTheStoredCredentialAndLeaksNothing(t *testing.T) {
	store := seededStore(upstream.IOSAuthPrefix, 2*24*time.Hour)
	current := string(*store.state.Authorization)
	client := &scriptedUpstream{responses: []string{
		`{"success":false,"code":"9999","message":"系统异常"}`,
	}}
	_, err := RefreshAccountAuth(context.Background(), refreshDeps(client, store), store.state.AccountID)
	if err == nil {
		t.Fatal("上游判失败时应报错")
	}
	if len(store.rotations) != 0 {
		t.Error("刷新失败却还是换了库里的凭据")
	}
	// 错误串会进响应体与日志：既不能回显整串，也不能带 token
	for _, text := range []string{err.Message(), err.Error()} {
		if strings.Contains(text, current) {
			t.Errorf("错误串回显了凭据：%s", text)
		}
	}
}

func TestTransportFailureIsNotReportedAsSuccess(t *testing.T) {
	store := seededStore(upstream.IOSAuthPrefix, 2*24*time.Hour)
	client := &refreshFailingClient{err: errors.New("connection reset")}
	report, err := RefreshAccountAuth(
		context.Background(), refreshDeps(client, store), store.state.AccountID)
	if err == nil {
		t.Fatal("网络失败应报错")
	}
	if report.Rotated {
		t.Error("网络失败却报成刷成功了")
	}
	if len(store.rotations) != 0 {
		t.Error("网络失败却写了库")
	}
}

// TestRefreshOfAnAccountWeNeverLoggedIn 打的是"库里没有这一行"那条路：
// 报错文案要指向重新登录，而不是"刷新接口失败"那种误导方向。
func TestRefreshOfAnAccountWeNeverLoggedIn(t *testing.T) {
	store := &refreshStore{fakeStore: fakeStore{state: nil}}
	client := &scriptedUpstream{responses: []string{refreshOK("WHATEVER")}}
	_, err := RefreshAccountAuth(
		context.Background(), refreshDeps(client, store), core.NewAccountID("199****0000"))
	if err == nil {
		t.Fatal("库里没有登录态时应拒绝")
	}
	if !strings.Contains(err.Message(), "重新登录") {
		t.Errorf("错误串没给出路：%s", err.Message())
	}
	if len(client.requests) != 0 {
		t.Error("库里没有凭据却还是打了上游")
	}
}

// refreshFailingClient 模拟"请求发出去了但没拿到响应"（连接被重置那一类）。
type refreshFailingClient struct{ err error }

func (c *refreshFailingClient) Send(
	_ context.Context, _ core.HTTPRequest,
) (core.HTTPResponse, error) {
	return core.HTTPResponse{}, c.err
}

// headerText 只用值的那份头查找（[headerOf] 还带一个"有没有"）。
func headerText(request core.HTTPRequest, name string) string {
	value, _ := headerOf(request, name)
	return value
}

func decodeRefreshAuth(t *testing.T, auth string) string {
	t.Helper()
	raw, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(auth, "Basic "))
	if err != nil {
		t.Fatalf("整串解不出来: %v", err)
	}
	return string(raw)
}

// redactBody 把含凭据的文本压成"只有长度"的形状，供断言消息用。
func redactBody(value string) string { return strings.Repeat("?", len(value)) }

// ─────────────────────────────────────────────────────────────────────────────
// 服务端那份常驻装配（[AuthRefresher]）
// ─────────────────────────────────────────────────────────────────────────────

// TestAuthRefresherTakesTheAppDeviceFromTheFileItCreates：
// 装配自己解析设备档案，且**文件是唯一持久化的那一份**。
//
// 三条各钉一件事：
//
//   - `leadDays` 给 0 走默认提前量（把它读成"永不刷新"的话，界面上每次点都没反应，
//     而 CLI 侧不会出现这个值 —— 只有这条路会传 0）；
//   - 设备文件是这次现生成的（夹具给的是空目录），并且**下一次调用复用同一份**：
//     每次现生成的表现是"上游按设备指纹定位会话时认成另一台机器"；
//   - `deviceShort` 反过来**每次现生成**（实测同 deviceId 两次不同），所以这里
//     断的是"deviceId 段在、整串不必相同"，不是"两次头完全一样"。
func TestAuthRefresherTakesTheAppDeviceFromTheFileItCreates(t *testing.T) {
	store := seededStore(upstream.IOSAuthPrefix, 2*24*time.Hour)
	client := &scriptedUpstream{responses: []string{
		refreshOK(expireToken(30 * 24 * time.Hour)),
		refreshOK(expireToken(29 * 24 * time.Hour)),
	}}
	path := filepath.Join(t.TempDir(), "device-app.txt")
	refresher := NewAuthRefresher(
		store, upstream.NewUpstream(client), testClock(), testRng(), path)
	ctx := context.Background()

	// 连着刷两次：第二次必须复用第一次落盘的那一份设备（脚本给两条响应）
	for range 2 {
		report, err := refresher.Refresh(
			ctx, core.ScopeLocal(), store.state.AccountID, 0, upstream.PlatformWindows, false)
		if err != nil {
			t.Fatalf("刷新失败: %v", err)
		}
		if !report.Rotated || report.Decision != capabilities.RefreshNeeded {
			t.Fatalf("leadDays=0 应落到默认提前量（7 天）并刷成，实得 %v / Rotated=%v",
				report.Decision, report.Rotated)
		}
	}
	if len(store.rotations) != 2 {
		t.Fatalf("写库 %d 次，期望 2 次", len(store.rotations))
	}

	raw, readErr := os.ReadFile(path)
	if readErr != nil {
		t.Fatalf("设备档案没落盘（%s）：%v", path, readErr)
	}
	deviceID := strings.TrimSpace(string(raw))
	if len(deviceID) != 32 {
		t.Fatalf("设备串应是 32 位 HEX，实得长度 %d", len(deviceID))
	}
	// 两次请求带的都是**文件里这一台**：头里那一段与盘上那一串必须同一份
	for i, request := range client.requests {
		if !strings.Contains(headerText(request, "x-DeviceInfo"), deviceID) {
			t.Errorf("第 %d 次请求的 `x-DeviceInfo` 里没有文件里那个 deviceId —— 指纹漂移了", i+1)
		}
	}
}

// TestAuthRefresherForceIgnoresTheLeadWindow 钉住 WebUI 那一路的口径：
// **还剩 45 天（远超默认 7 天提前量）时，带 force 必须真打上游并换成。**
//
// 两侧都钉，否则这条判据可以往两个方向烂掉：
//
//   - 把 force 一路传成 false（或在某一层把它丢了）⇒ 上半段红在"一次上游请求都没发"，
//     表现就是用户报的那句"点了说还早，什么都没发生"；
//   - 把 force 变成"永远刷"（连提前量都不判了）⇒ 下半段红，那条批量路
//     （CLI `--all` / 外部定时复用的同一条命令）会退化成每天把所有账号都换一遍 token。
//
// `Decision` 仍要如实报 `not-yet`：界面上"到没到点"没有读者，但 CLI 那句
// "按 --force 换的"靠这一对字段才拼得出来。
func TestAuthRefresherForceIgnoresTheLeadWindow(t *testing.T) {
	far := 45 * 24 * time.Hour

	forced := seededStore(upstream.IOSAuthPrefix, far)
	forcedClient := &scriptedUpstream{responses: []string{refreshOK(expireToken(far))}}
	forcedReport, err := NewAuthRefresher(
		forced, upstream.NewUpstream(forcedClient), testClock(), testRng(),
		filepath.Join(t.TempDir(), "device-app.txt"),
	).Refresh(context.Background(), core.ScopeLocal(), forced.state.AccountID,
		0, upstream.PlatformWindows, true)
	if err != nil {
		t.Fatalf("带 force 的刷新失败: %v", err)
	}
	if !forcedReport.Rotated {
		t.Fatalf("force 应越过提前量换成，实得 Rotated=false（decision %v）",
			forcedReport.Decision)
	}
	if forcedReport.Decision != capabilities.RefreshNotYet {
		t.Errorf("decision 要如实报判到的档位 not-yet，实得 %v", forcedReport.Decision)
	}
	if len(forcedClient.requests) != 1 {
		t.Errorf("上游请求 %d 次，期望 1 次", len(forcedClient.requests))
	}

	// 同一份库存、同一个时钟，只是不给 force：必须一个请求都不发、库里原样
	quiet := seededStore(upstream.IOSAuthPrefix, far)
	quietClient := &scriptedUpstream{responses: []string{refreshOK(expireToken(far))}}
	quietReport, err := NewAuthRefresher(
		quiet, upstream.NewUpstream(quietClient), testClock(), testRng(),
		filepath.Join(t.TempDir(), "device-app.txt"),
	).Refresh(context.Background(), core.ScopeLocal(), quiet.state.AccountID,
		0, upstream.PlatformWindows, false)
	if err != nil {
		t.Fatalf("不带 force 的刷新不该报错: %v", err)
	}
	if quietReport.Rotated || len(quietClient.requests) != 0 || len(quiet.rotations) != 0 {
		t.Fatalf("没给 force 就不该动手：Rotated=%v 请求 %d 次 写库 %d 次",
			quietReport.Rotated, len(quietClient.requests), len(quiet.rotations))
	}
}
