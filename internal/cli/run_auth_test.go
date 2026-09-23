package cli

import (
	"context"
	"encoding/base64"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/spf13/cobra"
	"github.com/spf13/pflag"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
	"github.com/catlair/mcloud/internal/upstream"
)

// 本文件测 `auth refresh` 的**命令行那一层**：挑谁、打几次、退几、印什么。
//
// 判据在 capabilities、头的准备在 app —— 那两层各有自己的用例，这里不重复断言。
// 这里要钉的是只有这一层才看得见的事：
//
//   - **没到点的账号一个请求都不发**（`--all` 扫全库时最容易出事的就是这条）。
//   - **一个账号失败不拦其余账号**，但退出码要红 —— 定时任务只看退出码。
//   - **终端上不许出现凭据**：这一层是整条链路上唯一会把结果**打印**出来的地方。
//
// 库用**真实的 SQLite**（与 run_smid_test.go 同理）：这一层判的就是"读出来什么、
// 写回去什么"，换成假存储等于把被测对象换掉。

// refreshSeed 是一个账号在库里的样子。
type refreshSeed struct {
	phone   string
	prefix  string
	since   time.Duration // token 第 4 段相对 [cliNow] 的偏移（负数 = 已过期）
	domain  bool          // 写不写 `user_domain_id`
	device  bool          // 写不写 `device_id`
	enabled bool
}

// authSeed 建库并塞进这些账号，返回库路径。
func authSeed(t *testing.T, seeds ...refreshSeed) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "state.db")
	store, err := mcloudio.Open(path)
	if err != nil {
		t.Fatalf("建临时库失败: %v", err)
	}
	defer store.Close()

	ctx := context.Background()
	for _, seed := range seeds {
		id := core.AccountIDFor(seed.phone, nil)
		account := core.Account{
			ID: id, Enabled: seed.enabled,
			Platform: authStrPtr(seed.prefix),
		}
		account.AuthExpire = authI64Ptr(cliNow().Add(seed.since).UnixMilli())
		if err := store.UpsertAccount(ctx, core.ScopeLocal(), &account); err != nil {
			t.Fatalf("写入账号失败: %v", err)
		}
		token := refreshToken(seed.since)
		authorization := core.SecretValue(authStringOf(seed.prefix, seed.phone, token))
		jwt := core.SecretValue(token)
		state := core.AccountState{
			AccountID: id, Authorization: &authorization, JWTToken: &jwt,
		}
		if seed.domain {
			domain := core.UserDomainID("1990000000000000001")
			state.UserDomainID = &domain
		}
		if seed.device {
			device := core.SecretValue("67F4C56A00000000000000000000315C")
			state.DeviceID = &device
		}
		if err := store.UpsertAccountState(ctx, core.ScopeLocal(), &state); err != nil {
			t.Fatalf("写入登录态失败: %v", err)
		}
	}
	return path
}

// appDue 是"App 端、该刷了、什么都齐"的那个账号。
func appDue() refreshSeed {
	return refreshSeed{
		phone: "19900000001", prefix: upstream.IOSAuthPrefix,
		since: 2 * 24 * time.Hour, domain: true, device: true, enabled: true,
	}
}

// refreshToken 造一个第 4 段是"cliNow + since"（**毫秒**）的合成 token。
func refreshToken(since time.Duration) string {
	return "SYNTH" + strings.Repeat("A", 3) + "|1|RCS|" +
		authI64Text(cliNow().Add(since).UnixMilli()) + "|AAAA"
}

func authStrPtr(s string) *string { return &s }

func authI64Ptr(v int64) *int64 { return &v }

func authI64Text(v int64) string { return strconv.FormatInt(v, 10) }

func authStringOf(prefix, phone, token string) string {
	return "Basic " + base64.StdEncoding.EncodeToString([]byte(prefix+":"+phone+":"+token))
}

// runAuthCLI 跑一次 `auth refresh`。
//
// ⚠ 三个"必须给"的默认值都要顶掉：`--db`（默认相对路径，会在包目录下建真库）、
// `--app-device-file`（同理，而且它会**写盘**）、时钟（不然过期时刻的断言会漂）。
func runAuthCLI(t *testing.T, script *loginScript, args ...string) (string, string, int) {
	t.Helper()
	dir := t.TempDir()
	argv := append([]string{"auth", "refresh",
		"--app-device-file", filepath.Join(dir, "app-device.txt")}, args...)
	var deps *Deps
	if script != nil {
		deps = &Deps{Clock: core.NewFixedClock(cliNow()), Random: &cliRng{next: 7}, HTTP: script}
	} else {
		deps = &Deps{Clock: core.NewFixedClock(cliNow()), Random: &cliRng{next: 7}}
	}
	return runCLIWithDeps(t, deps, "", argv...)
}

func refreshSuccessBody(newSince time.Duration) map[string]any {
	return map[string]any{
		"success": true, "code": "0000", "message": "请求成功",
		"data": map[string]any{
			"token": refreshToken(newSince), "expireTime": 2592000,
		},
	}
}

func openSeeded(t *testing.T, path string) *mcloudio.SqliteStore {
	t.Helper()
	store, err := mcloudio.Open(path)
	if err != nil {
		t.Fatalf("打开库失败: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })
	return store
}

func TestAuthRefreshRotatesTheStoredCredential(t *testing.T) {
	seed := appDue()
	db := authSeed(t, seed)
	script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}

	stdout, stderr, code := runAuthCLI(t, script, "--db", db,
		"--account", core.AccountIDFor(seed.phone, nil).String())
	if code != 0 {
		t.Fatalf("应退 0，实得 %d\n  stdout=%q stderr=%q", code, stdout, stderr)
	}
	if script.sent() != 1 {
		t.Fatalf("打了 %d 次上游，应为 1 次：%v", script.sent(), script.urls())
	}
	if !strings.Contains(stdout, "已换新凭据") || !strings.Contains(stdout, "手机 App") {
		t.Errorf("输出没说明白刷的是哪一支：%q", stdout)
	}
	// 输出里那一列是**日期**，不是秒：上游的 `expireTime`（秒、相对时长）漏进来时
	// 会印成 1970 年。
	if strings.Contains(stdout, "1970-") {
		t.Errorf("过期时刻看着像拿秒当毫秒印了：%q", stdout)
	}

	store := openSeeded(t, db)
	id := core.AccountIDFor(seed.phone, nil)
	state, err := store.GetAccountState(context.Background(), core.ScopeLocal(), id)
	if err != nil || state == nil {
		t.Fatalf("读不回登录态: %v", err)
	}
	if string(*state.Authorization) == authStringOf(seed.prefix, seed.phone, refreshToken(seed.since)) {
		t.Error("库里的整串没被换掉（刷新等于没生效）")
	}
	if string(*state.JWTToken) != refreshToken(30*24*time.Hour) {
		t.Error("jwt_token 没换成上游给的那个")
	}
	account, err := store.GetAccount(context.Background(), core.ScopeLocal(), id)
	if err != nil || account == nil {
		t.Fatalf("读不回账号: %v", err)
	}
	if account.AuthExpire == nil || *account.AuthExpire != cliNow().Add(30*24*time.Hour).UnixMilli() {
		t.Errorf("auth_expire 没跟上新串：%v", account.AuthExpire)
	}
}

// 整条链路唯一的"终端上会留下凭据"的机会就在这一层，所以每次成功都顺手断一遍。
func TestAuthRefreshOutputCarriesNoCredential(t *testing.T) {
	seed := appDue()
	db := authSeed(t, seed)
	oldAuth := authStringOf(seed.prefix, seed.phone, refreshToken(seed.since))
	newToken := refreshToken(30 * 24 * time.Hour)
	script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}

	stdout, stderr, _ := runAuthCLI(t, script, "--db", db,
		"--account", core.AccountIDFor(seed.phone, nil).String())
	for _, forbidden := range []string{oldAuth, newToken, seed.phone} {
		for _, stream := range []string{stdout, stderr} {
			if strings.Contains(stream, forbidden) {
				t.Errorf("输出里出现了凭据/完整手机号：%q", forbidden)
			}
		}
		// base64 那一份也要防：整串常被"顺手"编解码一次再印出来
		decoded, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(oldAuth, "Basic "))
		if err != nil {
			t.Fatalf("夹具本身就不对: %v", err)
		}
		for _, stream := range []string{stdout, stderr} {
			if strings.Contains(stream, string(decoded)) {
				t.Errorf("输出里出现了 auth 载荷原文")
			}
		}
	}
}

func TestAuthRefreshAllSkipsAccountsThatAreNotDue(t *testing.T) {
	due := appDue()
	notYet := refreshSeed{
		phone: "19900000002", prefix: upstream.IOSAuthPrefix,
		since: 30 * 24 * time.Hour, domain: true, device: true, enabled: true,
	}
	stopped := refreshSeed{
		phone: "19900000003", prefix: upstream.IOSAuthPrefix,
		since: 2 * 24 * time.Hour, domain: true, device: true, enabled: false,
	}
	db := authSeed(t, due, notYet, stopped)
	script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}

	stdout, stderr, code := runAuthCLI(t, script, "--db", db, "--all")
	if code != 0 {
		t.Fatalf("没到点不是失败，应退 0：%d stderr=%q", code, stderr)
	}
	if script.sent() != 1 {
		t.Fatalf("打了 %d 次上游（该刷的只有一个）：%v", script.sent(), script.urls())
	}
	if !strings.Contains(stdout, "还没到提前量") {
		t.Errorf("没到点的那个账号该有一行说明：%q", stdout)
	}
	if strings.Contains(stdout, core.AccountIDFor(stopped.phone, nil).String()) {
		t.Errorf("停用的账号也被扫了：%q", stdout)
	}
}

// `--lead-days` 是这一层唯一能改变"打不打上游"的旋钮：默认 7 天时"还剩 8 天"不该发，
// 调到 10 天就该发。两个方向各钉一次。
func TestAuthRefreshLeadDaysIsTheKnob(t *testing.T) {
	seed := appDue()
	seed.since = 8 * 24 * time.Hour
	for _, c := range []struct {
		lead     string
		wantSent int
	}{
		{"7", 0},
		{"10", 1},
	} {
		db := authSeed(t, seed)
		script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}
		_, _, code := runAuthCLI(t, script, "--db", db, "--all", "--lead-days", c.lead)
		if code != 0 {
			t.Fatalf("lead=%s 退 %d", c.lead, code)
		}
		if script.sent() != c.wantSent {
			t.Errorf("--lead-days %s：打了 %d 次，应为 %d 次",
				c.lead, script.sent(), c.wantSent)
		}
	}
}

// TestAuthRefreshForceCrossesTheLeadWindow：`--force` 是"到不到点都刷"那一位。
//
// 与 [TestAuthRefreshLeadDaysIsTheKnob] 用同一份"剩 8 天"的库存：那里 `--lead-days 7`
// 一次都不打，这里加上 `--force` 必须打一次、换成，并且把"本未到点"印出来
// （同一句"已换新凭据"不区分这两件事，批量刷完人会以为提前量生效过）。
//
// 反向改坏它：把 `runAuthRefresh` 里那个 `Force: a.Force` 漏掉，本用例红在"打了 0 次"。
func TestAuthRefreshForceCrossesTheLeadWindow(t *testing.T) {
	seed := appDue()
	seed.since = 8 * 24 * time.Hour
	db := authSeed(t, seed)
	script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}

	stdout, _, code := runAuthCLI(t, script, "--db", db, "--all", "--force")
	if code != 0 {
		t.Fatalf("带 --force 退了 %d", code)
	}
	if script.sent() != 1 {
		t.Fatalf("打了 %d 次上游，期望 1 次（force 要越过提前量）", script.sent())
	}
	if !strings.Contains(stdout, "按 `--force` 换的") {
		t.Errorf("没把\"本未到提前量\"说出来：%q", stdout)
	}
}

func TestAuthRefreshBatchContinuesAfterAFailure(t *testing.T) {
	first := appDue()
	second := appDue()
	second.phone = "19900000011"
	db := authSeed(t, first, second)
	script := &loginScript{responses: []any{
		map[string]any{"success": false, "code": "9999", "message": "系统异常"},
		refreshSuccessBody(30 * 24 * time.Hour),
	}}

	stdout, _, code := runAuthCLI(t, script, "--db", db, "--all")
	if code != ExitRun {
		t.Errorf("有一个账号失败就该退 %d，实得 %d", ExitRun, code)
	}
	if script.sent() != 2 {
		t.Fatalf("第二个账号被第一个的失败拦住了（只打了 %d 次）", script.sent())
	}
	if !strings.Contains(stdout, "已换新凭据") {
		t.Errorf("成功那个账号那一行丢了：%q", stdout)
	}
	if strings.Contains(stdout, "9999") {
		t.Errorf("上游原文进了终端：%q", stdout)
	}

	// 失败那个账号的凭据必须**原样**：半途换掉比不换更糟
	store := openSeeded(t, db)
	state, err := store.GetAccountState(context.Background(), core.ScopeLocal(),
		core.AccountIDFor(first.phone, nil))
	if err != nil || state == nil {
		t.Fatalf("读不回登录态: %v", err)
	}
	if string(*state.Authorization) != authStringOf(first.prefix, first.phone, refreshToken(first.since)) {
		t.Error("刷新失败的账号被换掉了凭据")
	}
}

func TestAuthRefreshUsageErrors(t *testing.T) {
	db := authSeed(t, appDue())
	cases := []struct {
		name string
		argv []string
	}{
		{"不给刷谁", []string{"--db", db}},
		{"account 与 all 同时给", []string{"--db", db, "--account", "19****01", "--all"}},
		{"提前量给 0", []string{"--db", db, "--all", "--lead-days", "0"}},
		{"提前量给负数", []string{"--db", db, "--all", "--lead-days", "-1"}},
		{"平台名不认", []string{"--db", db, "--all", "--platform", "linux"}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			stdout, stderr, code := runAuthCLI(t, nil, c.argv...)
			if code != ExitUsage {
				t.Errorf("应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
			}
			if stdout != "" {
				t.Errorf("用法错误不该往 stdout 写东西：%q", stdout)
			}
		})
	}
}

// 库里没有这个账号时要**点名现有有哪些**：账号 id 是脱敏手机号，
// 用户是照着 `smid show` / WebUI 手打的，"打错了"比"真的没有"常见得多。
func TestAuthRefreshUnknownAccountListsWhatExists(t *testing.T) {
	seed := appDue()
	db := authSeed(t, seed)
	_, stderr, code := runAuthCLI(t, nil, "--db", db, "--account", "19****99")
	if code != ExitRun {
		t.Fatalf("找不到账号是运行错误（退 %d），实得 %d", ExitRun, code)
	}
	if !strings.Contains(stderr, "19****99") {
		t.Errorf("错误串没点名要找的那个：%q", stderr)
	}
	if !strings.Contains(stderr, core.AccountIDFor(seed.phone, nil).String()) {
		t.Errorf("错误串没列出库里现有的账号：%q", stderr)
	}
}

// PC 那一支要 `device_id`（拼设备档案用）；缺了要在**打上游之前**说清楚。
func TestAuthRefreshPCWithoutDeviceIDFailsBeforeDialing(t *testing.T) {
	seed := appDue()
	seed.prefix = "pc"
	seed.device = false
	db := authSeed(t, seed)
	script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}

	_, _, code := runAuthCLI(t, script, "--db", db,
		"--account", core.AccountIDFor(seed.phone, nil).String())
	if code != ExitRun {
		t.Fatalf("缺设备档案应退 %d，实得 %d", ExitRun, code)
	}
	if script.sent() != 0 {
		t.Errorf("档案都不齐却还是打了上游（%d 次）", script.sent())
	}
}

func TestAuthRefreshPCSendsWithThePlatformFlagAndRotates(t *testing.T) {
	seed := appDue()
	seed.prefix = "pc"
	db := authSeed(t, seed)
	script := &loginScript{responses: []any{refreshSuccessBody(30 * 24 * time.Hour)}}

	stdout, stderr, code := runAuthCLI(t, script, "--db", db, "--platform", "macos",
		"--account", core.AccountIDFor(seed.phone, nil).String())
	if code != 0 {
		t.Fatalf("应退 0：%d stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "PC 客户端") {
		t.Errorf("输出没说是以 PC 那一支刷的：%q", stdout)
	}
	if len(script.requests) != 1 {
		t.Fatalf("打了 %d 次", len(script.requests))
	}
	sent := string(script.requests[0].Body)
	if !strings.Contains(sent, `"clientType":"`+upstream.PlatformMacOS.ClientType()+`"`) {
		t.Error("--platform macos 没体现在发出去的 clientType 上")
	}
}

// TestNoFlagHelpTextUsesBackticks 把"帮助文本里不许有反引号"从一条命令扩到**整棵树**。
//
// 来源是坑 #115：pflag 把一条 flag 帮助文本里**第一对反引号**之间的内容当成
// **参数占位符**，于是 `--account` 那条印出来变成 "--account mcloud signin --list"
// 而不是 "--account int"。既有判据（TestLiveRoomHelpShowsFlagTypesNotBacktickText）
// 只盯 live-room 那一张 help —— 与本仓 #116 同一形状：**手写的覆盖面在"别人加了新
// flag"那天不会有任何信号**。本轮新加的 --force 就正好带着反引号（靠这条才当场变红）。
//
// 判据是**读注册好的 flag 文本**而不是跑 --help：要跑 help 就得为每个叶子造一份
// 可跑的临时库与参数（#116 里那份手写清单就是这么烂掉的），而反引号这件事与命令
// 能不能跑无关。
func TestNoFlagHelpTextUsesBackticks(t *testing.T) {
	var walk func(path string, cmd *cobra.Command)
	walk = func(path string, cmd *cobra.Command) {
		cmd.LocalFlags().VisitAll(func(f *pflag.Flag) {
			if strings.Contains(f.Usage, "`") {
				t.Errorf("%s 的 --%s 帮助文本里有反引号：%q\n"+
					"pflag 会把第一对反引号之间当成参数占位符，help 里印出来是错的形状 —— 去掉反引号即可",
					path, f.Name, f.Usage)
			}
		})
		for _, sub := range cmd.Commands() {
			walk(path+" "+sub.Name(), sub)
		}
	}
	walk("mcloud", buildRoot(t))
}
