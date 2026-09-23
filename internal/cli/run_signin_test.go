package cli

// 本文件测 `mcloud signin` 的**编排层**：选账号、分支优先级、分页循环、
// 失败隔离、退出码、以及四个写开关"失败也不算账号失败"。
//
// 协议与业务判定在 `internal/upstream` / `internal/app` —— 那些各有自己的测试，
// 这里**不**重复断言；终端文案与 JSON 形状在 `signin_render_test.go`。
// 本文件只回答"这一层自己该做的判断做对了没有"。
//
// 库用**真实的 SQLite**（`mcloudio.Open`）：本层唯一会出错的就是
// "读出来什么、写进去什么"，换成内存假存储等于把被测对象换掉。
//
// 号码一律是 199 号段的合成号，凭据是 `SYNTH…` 合成串。

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 假上游
// ─────────────────────────────────────────────────────────────────────────────

// signinRoute 是一条「URL 里含这个子串 → 按顺序回这些响应」的路由。
//
// 按 URL 路由而不是纯顺序回放：一次 `--records-all` 有十来个请求，
// 顺序排出来既脆又读不懂；而"哪个接口被打了几次"恰恰是本层要断言的东西。
// 同一路由的响应**用完了就重复最后一条** —— 建会话那三条在
// "每个账号 × 每条分支"上都会重来一遍，各写 N 份只会让用例难改。
type signinRoute struct {
	match  string
	bodies []any
	used   int
}

// signinScript 是脚本化的上游。
type signinScript struct {
	routes   []*signinRoute
	requests []core.HTTPRequest
}

func (s *signinScript) route(match string, bodies ...any) *signinScript {
	s.routes = append(s.routes, &signinRoute{match: match, bodies: bodies})
	return s
}

func (s *signinScript) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	s.requests = append(s.requests, request)
	for _, r := range s.routes {
		if len(r.bodies) == 0 || !strings.Contains(request.URL, r.match) {
			continue
		}
		index := r.used
		if index >= len(r.bodies) {
			index = len(r.bodies) - 1
		}
		r.used++
		if text, ok := r.bodies[index].(string); ok {
			return core.NewResponse(200, []byte(text)), nil
		}
		body, err := core.MarshalJSON(r.bodies[index])
		if err != nil {
			return core.HTTPResponse{}, err
		}
		return core.NewResponse(200, body), nil
	}
	// 没匹配上就报错，而不是给一份空响应：一份"成功但什么都没有"的响应
	// 会让"这次不该打这个接口"这类断言静默通过。
	return core.HTTPResponse{}, core.NewInternal("脚本里没有匹配 " + request.URL + " 的响应")
}

func (s *signinScript) sent() int { return len(s.requests) }

// count 数一数 URL 里含某个子串的请求有几个 —— "发没发、发了几次"是本层的主要判据。
func (s *signinScript) count(substr string) int {
	n := 0
	for _, r := range s.requests {
		if strings.Contains(r.URL, substr) {
			n++
		}
	}
	return n
}

// header 取第一个命中该 URL 的请求上某个头的值（没有则空串）。
func (s *signinScript) header(urlMatch, name string) string {
	for _, r := range s.requests {
		if !strings.Contains(r.URL, urlMatch) {
			continue
		}
		for _, h := range r.Headers {
			if strings.EqualFold(h[0], name) {
				return h[1]
			}
		}
	}
	return ""
}

// headers 取**所有**请求上某个头的值（按发生顺序）。
func (s *signinScript) headers(name string) []string {
	out := make([]string, 0, len(s.requests))
	for _, r := range s.requests {
		for _, h := range r.Headers {
			if strings.EqualFold(h[0], name) {
				out = append(out, h[1])
			}
		}
	}
	return out
}

// session 装上"建会话"那三条的响应（顺序固定，见 app.OpenSignInSession）。
func (s *signinScript) session() *signinScript {
	return s.
		route("/user/querySpecTokenV2", map[string]any{
			"success": true, "code": "0000", "message": "请求成功",
			"data": map[string]any{"token": "SSO-TOKEN", "resultCode": "104000"},
		}).
		route("/ycloud/api/timestamp", map[string]any{
			"code": 0, "msg": "success", "result": 1789800128813,
		}).
		route("/ycloud/auth-service/auth/tyrzLogin", map[string]any{
			"code": 0, "msg": "success", "result": map[string]any{"token": "JWT"},
		})
}

// ─────────────────────────────────────────────────────────────────────────────
// 响应夹具
// ─────────────────────────────────────────────────────────────────────────────

// signinInfoV3 造一份 `infoV3` 响应。
//
// ⚠ 四个整数字段（`total` / `signInPoints` / `signCount` / `monthDays`）
// **一个都不能少**：协议层对它们是 `RequireInt64` 硬要求的，缺任一个整份解析就失败
// —— 于是用例会以"协议层报错"的形式挂掉，而真正想测的判据根本没跑到。
func signinInfoV3(signedToday bool, total int64, extra map[string]any) map[string]any {
	result := map[string]any{
		"cal": []any{
			map[string]any{"d": 21, "s": signedToday, "currentMonth": 1, "n": 50, "t": true},
		},
		"signInPoints": 3,
		"signCount":    7,
		"total":        total,
		"monthDays":    7,
	}
	for key, value := range extra {
		result[key] = value
	}
	return map[string]any{"code": 0, "msg": "success", "result": result}
}

// signinStartOK 是 `startSignIn` 的成功响应。
//
// `todaySignIn:false` 是**调用前**的快照 —— 协议层不拿它当结论（见 CloudSignIn），
// 这里照抓包写，正是为了让"信不信它"这件事在用例里保持可分辨。
func signinStartOK() map[string]any {
	return map[string]any{"code": 0, "msg": "success", "result": map[string]any{
		"todaySignIn": false, "signInPoints": 50, "signCount": 7,
	}}
}

// signinSignsToday 是签到主路上 `infoV3` 的**两份**响应：先「未签」、后「已签」。
//
// ⚠ 必须是两份、且按顺序：`CloudSignIn` 查两次 —— 签到前的快照 + 签到后的回查，
// 而**回查结果才是唯一的成败判据**（见 `app.CloudSignIn`；`startSignIn` 的
// `todaySignIn` 是调用前的快照，不能当结论）。
//
// 假上游对同一路由"用完了就重复最后一条"，所以只写一份 `false` 的实现会让回查
// 也读到 `false` —— 用例于是以「startSignIn 回了成功，但回查 infoV3 显示今天仍未签」
// 挂掉，而真正想测的判据（领取 / 备份好礼 / 复活卡 / 设备降级）根本没跑到。
//
// `extra` 会同时进两份：回查那份也要带 `receiveList` 之类的字段，
// 因为 `--receive` 的自动分流紧跟着主路，读的正是**最后**那份 `infoV3`。
func signinSignsToday(extra map[string]any) []any {
	return []any{signinInfoV3(false, 10502, extra), signinInfoV3(true, 10502, extra)}
}

// signinRecordsPage 造一页 `cloudRecordV2`。`total` 为 nil = 上游没给这个字段。
func signinRecordsPage(count int, total *int64) map[string]any {
	records := make([]any, 0, count)
	for i := 0; i < count; i++ {
		records = append(records, map[string]any{"i": i})
	}
	result := map[string]any{"records": records}
	if total != nil {
		result["total"] = *total
	}
	return map[string]any{"code": 0, "msg": "success", "result": result}
}

// ─────────────────────────────────────────────────────────────────────────────
// 库夹具
// ─────────────────────────────────────────────────────────────────────────────

// signinEnv 是一次运行要用的临时路径。
type signinEnv struct {
	dir           string
	db            string
	appDeviceFile string
}

// signinAcct 是一个要写进库里的账号。
//
// ⚠ `auth` 与 `jwt` 同时给时，写入顺序是"先整串、后老行" —— 两者落在**同一行**上，
// 而 `UpsertAccountState` 是整体覆盖。这正是"库里既有整串又有 jwt"那种真实形态。
type signinAcct struct {
	id       string
	enabled  bool
	auth     string // 整串 Authorization（v7 那一路）
	jwt      string // 老行的 authToken
	phone    string // 完整手机号
	platform string
	// deviceID 非空 = 预绑一份设备特征 + deviceId，于是取 H5 deviceId 不打网络。
	deviceID string
}

// signinFixture 建一个带若干账号的临时库，返回三个临时路径。
func signinFixture(t *testing.T, accts ...signinAcct) signinEnv {
	t.Helper()
	dir := t.TempDir()
	env := signinEnv{
		dir:           dir,
		db:            filepath.Join(dir, "state.db"),
		appDeviceFile: filepath.Join(dir, "device-app.txt"),
	}

	store, err := mcloudio.Open(env.db)
	if err != nil {
		t.Fatalf("打开临时库失败: %v", err)
	}
	defer store.Close()

	ctx := context.Background()
	scope := core.ScopeLocal()
	for _, spec := range accts {
		account := core.Account{ID: core.NewAccountID(spec.id), Enabled: spec.enabled}
		if spec.platform != "" {
			platform := spec.platform
			account.Platform = &platform
		}
		if err := store.UpsertAccount(ctx, scope, &account); err != nil {
			t.Fatalf("写账号 %s 失败: %v", spec.id, err)
		}

		accountID := core.NewAccountID(spec.id)
		if spec.jwt != "" {
			token := core.NewSecretValue(spec.jwt)
			state := core.AccountState{AccountID: accountID, JWTToken: &token}
			if err := store.UpsertAccountState(ctx, scope, &state); err != nil {
				t.Fatalf("写登录态 %s 失败: %v", spec.id, err)
			}
		}
		var auth, phone *string
		if spec.auth != "" {
			auth = &spec.auth
		}
		if spec.phone != "" {
			phone = &spec.phone
		}
		if auth != nil || phone != nil {
			if err := store.FillAccountCredentials(ctx, scope, accountID, auth, phone); err != nil {
				t.Fatalf("写凭据 %s 失败: %v", spec.id, err)
			}
		}
		if spec.deviceID != "" {
			fingerprint := upstream.GenerateSmFingerprint(&cliRng{next: 7})
			raw, perr := fingerprint.ToJSONString()
			if perr != nil {
				t.Fatalf("序列化特征失败: %v", perr)
			}
			if err := store.SetSmidFingerprint(ctx, scope, accountID, raw); err != nil {
				t.Fatalf("绑定特征 %s 失败: %v", spec.id, err)
			}
			if err := store.SetSmidDeviceID(ctx, scope, accountID, spec.deviceID); err != nil {
				t.Fatalf("写 deviceId %s 失败: %v", spec.id, err)
			}
		}
	}
	return env
}

// signinOpen 打开夹具库做落库断言。
func signinOpen(t *testing.T, path string) *mcloudio.SqliteStore {
	t.Helper()
	store, err := mcloudio.Open(path)
	if err != nil {
		t.Fatalf("打开库失败: %v", err)
	}
	t.Cleanup(func() { store.Close() })
	return store
}

// signinDeviceID 是一份"形状对"的 H5 deviceId（`B` + 88 字符）。
func signinDeviceID(fill string) string { return "B" + strings.Repeat(fill, 88) }

// signinCLI 跑一次完整命令行：真实解析、真实分发，
// 只有流 / 时钟 / 随机 / HTTP 是假的。
func signinCLI(
	t *testing.T, http core.HTTPClient, args ...string,
) (stdout, stderr string, code int) {
	t.Helper()
	return runCLIWithDeps(t, &Deps{
		Clock:  core.NewFixedClock(cliNow()),
		Random: &cliRng{next: 3},
		HTTP:   http,
	}, "", args...)
}

// signinRun 是"带夹具路径的那条命令"。
func signinCLIAt(
	t *testing.T, http core.HTTPClient, env signinEnv, extra ...string,
) (stdout, stderr string, code int) {
	t.Helper()
	args := append([]string{
		"signin", "--db", env.db, "--app-device-file", env.appDeviceFile,
	}, extra...)
	return signinCLI(t, http, args...)
}

// ─────────────────────────────────────────────────────────────────────────────
// 选账号
// ─────────────────────────────────────────────────────────────────────────────

// `--list` 打的是**完整清单**（含停用），而且在选账号之前短路 ——
// 它要回答的是"--account 该填几"，与本次要跑谁无关。
func TestSigninListPrintsTheIndexTableAndIgnoresJSON(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: true},
		signinAcct{id: "19****02", enabled: false},
	)
	http := &signinScript{}

	stdout, stderr, code := signinCLIAt(t, http, env, "--list", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 0 {
		t.Errorf("`--list` 是纯查库，不该打上游，实得 %d 次", http.sent())
	}
	for _, want := range []string{"19****01", "19****02", "启用", "停用"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("清单里该有 %q:\n%s", want, stdout)
		}
	}
	// ⚠ 忽略 `--json`：Rust 那条分支直接打表格，不查 args.json。
	// 少了这条断言，一个"顺手也走 JSON 出口"的实现照样是绿的。
	if strings.HasPrefix(strings.TrimSpace(stdout), "{") {
		t.Errorf("`--list` 忽略 `--json`，stdout 不该是一份 JSON:\n%s", stdout)
	}
}

// 空库不是错误，但**必须说清下一步做什么** —— 直接"跑完什么都没打"
// 会让人以为签到成功了。
func TestSigninWithNoAccountsPointsAtLogin(t *testing.T) {
	env := signinFixture(t)
	http := &signinScript{}

	stdout, stderr, code := signinCLIAt(t, http, env)
	if code != ExitSuccess {
		t.Fatalf("空库是正常中间态，应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "库里还没有账号") {
		t.Errorf("该指去登录:\n%s", stdout)
	}
	if http.sent() != 0 {
		t.Errorf("一个账号都没选中就不该打上游，实得 %d 次", http.sent())
	}
}

// ⚠ "有账号、只是全停用"与"库是空的"**必须分开说**（刻意修正 Rust 的一句误导文案）：
// 前者指去重新登录，而用户明明登录过（`--list` 看得见）。
func TestSigninWithOnlyDisabledAccountsSaysWhyNothingRan(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: false},
		signinAcct{id: "19****02", enabled: false},
	)

	stdout, stderr, code := signinCLIAt(t, &signinScript{}, env)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "库里有 2 个账号，但没有启用中的") {
		t.Errorf("该说清是「全停用」而不是「没有账号」:\n%s", stdout)
	}
	if strings.Contains(stdout, "库里还没有账号") {
		t.Errorf("有账号却指去登录 —— 这正是要修掉的那句误导文案:\n%s", stdout)
	}
}

// `--account 0` 是**用法**错误（退 2）：下标从 1 起，写 0 一定是调用写错了。
//
// ⚠ 判据**不是**"解析期能不能判"—— 跨层校验发生在 RunE 里、性质仍然是用法错误
// （[ErrUsage] 的注释里 `--records-all` 那条就是这个道理）。判据是
// "**改调用**能不能好 / 重试能不能好"：换个下标立刻就好 ⇒ 用法错误。
func TestSigninAccountZeroIsAUsageError(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true})

	stdout, stderr, code := signinCLIAt(t, &signinScript{}, env, "--account", "0")
	if code != ExitUsage {
		t.Fatalf("下标写错是用法错误，应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "下标从 1 开始") {
		t.Errorf("该说清下标从 1 开始: %q", stderr)
	}
	if stdout != "" {
		t.Errorf("出错路径不该往 stdout 写东西，实得 %q", stdout)
	}
}

// ⚠ 负数的危害是 `accounts[-1]` 会 panic；它与 `--account 0` 是同一个错
// （下标从 1 起），必须合并成同一句话。⚠ 与越界一样退 **2**（用法错误）：
// 换个下标就能好，重试不会。
func TestSigninAccountNegativeDoesNotBecomeHuge(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true})

	_, stderr, code := signinCLIAt(t, &signinScript{}, env, "--account", "-1")
	if code != ExitUsage {
		t.Fatalf("应退 %d（而不是 panic 或天文数字），实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "下标从 1 开始") {
		t.Errorf("负数该被当成「从 1 开始」的同一个错: %q", stderr)
	}
}

func TestSigninAccountOutOfRangeNamesTheCount(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: true},
		signinAcct{id: "19****02", enabled: true},
	)

	_, stderr, code := signinCLIAt(t, &signinScript{}, env, "--account", "3")
	if code != ExitUsage {
		t.Fatalf("越界是用法错误（重试不会变好），应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "超出范围") || !strings.Contains(stderr, "一共 2 个账号") {
		t.Errorf("越界要报出总数（用户是照着 --list 手打的）: %q", stderr)
	}
}

// ⚠ 下标是"**完整列表**里的位置"，含停用账号。
//
// 一旦下标随启用状态变动，同一个数字在不同时刻会指到不同账号上。
// 探针打在"停用的那个账号**真的被跑了**"上：若实现先过滤 enabled，
// `--account 2` 要么越界、要么指到别人。
func TestSigninAccountIndexCountsDisabledRows(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH-ONE",
			deviceID: signinDeviceID("A")},
		signinAcct{id: "19****02", enabled: false, auth: "Basic SYNTH-TWO",
			deviceID: signinDeviceID("A")},
	)
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	stdout, stderr, code := signinCLIAt(t, http, env, "--account", "2")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	// ⚠ 查 stdout+stderr 合起来：这行是进度行，按约定走 stderr。
	// 只看 stdout 的话，进度行一挪到 stderr 这条断言就恒真了 —— 而"下标指到了谁"
	// 恰恰是它唯一要防的事。
	both := stdout + stderr
	if !strings.Contains(both, "账号 2 (19****02)") {
		t.Errorf("下标 2 该指向第二个账号（停用的那个）:\n%s", both)
	}
	if strings.Contains(both, "19****01") {
		t.Errorf("显式指定下标就不该去跑第一个账号:\n%s", both)
	}
	if !strings.Contains(stderr, "是停用状态") {
		t.Errorf("显式指定停用账号要提醒一句（否则排查没头没尾）: %q", stderr)
	}
}

// `--phone` 优先级最高，而且它**强制现拼** —— 不是"先查库、查不到再拼"。
//
// 探针打在发出去的 `Authorization` 上：库里存着一份**不一样的**整串，
// 走现拼那条路时发出去的必须是拼出来的那个（前缀 `mobile`），
// 而不是库里那份。把两条路合成一条的实现会发库里的整串，这条就红。
func TestSigninPhoneBeatsAccountAndForcesTheFreshCredential(t *testing.T) {
	const (
		phone = "19900000001"
		// 库里那份"服务端给的原文"，前缀故意不是 `mobile`。
		storedAuth = "Basic SYNTH-STORED-PC-PREFIX"
		token      = "SYNTH001|1|RCS|1893456000000|AAAA"
	)
	env := signinFixture(t,
		// 号码推出来的 id 就是掩码形式（与登录落库同源）。
		signinAcct{id: "19****01", enabled: true, jwt: token, auth: storedAuth,
			deviceID: signinDeviceID("A")},
		signinAcct{id: "19****02", enabled: true, auth: "Basic SYNTH-OTHER",
			deviceID: signinDeviceID("A")},
	)
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	stdout, stderr, code := signinCLIAt(t, http, env, "--phone", phone, "--account", "2")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}

	// `--phone` 定位的账号**没有下标**：打一个 `账号 0` 出来会让人以为它是第一个。
	//
	// ⚠ 查的是 **stdout+stderr 合起来**：这行是"正在做什么"的进度行，
	// 按约定走 stderr（编排层在**发请求之前**打）。只看 stdout 的话，
	// 这条断言在"进度行挪到 stderr"之后就恒真了 —— 而那正是它要防的事。
	both := stdout + stderr
	if !strings.Contains(both, "账号 19****01：以 iOS App 身份签到") {
		t.Errorf("`--phone` 该赢过 `--account`，且不打下标:\n%s", both)
	}
	if strings.Contains(both, "19****02") {
		t.Errorf("给了 `--phone` 就不该去跑 `--account` 指的那个:\n%s", both)
	}

	sent := http.header("/user/querySpecTokenV2", "Authorization")
	if want := upstream.AppAuthorization(phone, token); sent != want {
		t.Errorf("`--phone` 该强制现拼，实得 %q 期望 %q", sent, want)
	}
	if sent == storedAuth {
		t.Error("发出去的是库里那份整串 —— 两条路被合成一条了")
	}
	// 完整手机号**不进任何输出**（它是隐私，掩码 id 才是标识）。
	if strings.Contains(stdout+stderr, phone) {
		t.Errorf("完整手机号不该进输出:\n%s%s", stdout, stderr)
	}
}

// `--phone` 跑完**只记手机号**，不记上面现拼出来的整串。
//
// 理由不是洁癖：`authorization` 列的口径是"服务端给的原文"，把本地合成物写进去
// 等于给凭据伪造来源 —— 它会走整串优先路径、绕过前缀闸，此后每个请求都带着
// 一个我们自己安上去的前缀，且再没人核对。
func TestSigninPhoneRecordsOnlyTheNumber(t *testing.T) {
	const (
		phone = "19900000002"
		token = "SYNTH002|1|RCS|1893456000000|BBBB"
	)
	env := signinFixture(t, signinAcct{
		id: "19****02", enabled: true, jwt: token, deviceID: signinDeviceID("A"),
	})
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	if _, stderr, code := signinCLIAt(t, http, env, "--phone", phone); code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}

	state, err := signinOpen(t, env.db).
		GetAccountState(context.Background(), core.ScopeLocal(), core.NewAccountID("19****02"))
	if err != nil {
		t.Fatalf("读登录态失败: %v", err)
	}
	if state == nil || state.Phone == nil || state.Phone.Value() != phone {
		t.Errorf("手机号该记进库里（下次不必再输）: %+v", state)
	}
	if state.Authorization != nil {
		t.Errorf("拼出来的整串**不该**落库，实得 %q", state.Authorization.Value())
	}
}

// `--phone ""` 是调用写错了，早点说清楚（退 2），别一路走到看不懂的地方。
func TestSigninEmptyPhoneIsAUsageError(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true})

	_, stderr, code := signinCLIAt(t, &signinScript{}, env, "--phone", "")
	if code != ExitUsage {
		t.Fatalf("应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "--phone 给的是空串") {
		t.Errorf("该点名是 `--phone` 的问题: %q", stderr)
	}
}

// `--force` 在**签到主路**上要真的透传下去（它的唯一用途是协议自检）：
// 今天已签也再发一次 `startSignIn`。不透传的实现会走"今天已经签过"那条捷径，
// 一个写请求都不发 —— 而那条捷径在输出上与"自检通过"长得很像。
func TestSigninForceIsPassedThroughOnTheMainPath(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	// 今天**已签**：不给 `--force` 时 `CloudSignIn` 当场返回，不发 `startSignIn`。
	http.session().
		route("page/infoV3", signinInfoV3(true, 10502, nil)).
		route("page/startSignIn", signinStartOK())

	stdout, stderr, code := signinCLIAt(t, http, env, "--force")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if got := http.count("/startSignIn"); got != 1 {
		t.Errorf("`--force` 该让今天已签也再发一次写请求，实得 %d 次", got)
	}
	if !strings.Contains(stdout, "按 --force 重发了一次 startSignIn") {
		t.Errorf("该报成「本来就已签 + 自检重发」:\n%s", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 分支优先级（互斥表管不到的那几对，只由顺序决定）
// ─────────────────────────────────────────────────────────────────────────────

// `--records` > `--status-only`。这两条**可以同时给**，谁生效只由 if 顺序决定
// —— 顺序写反了不会有任何报错，只是行为静默分叉。
func TestSigninRecordsWinsOverStatusOnly(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	// 两个账本各回一个空页：只为了让这条分支能跑完 —— 判据在下面几行。
	//
	// ⚠ 不能靠"不给路由于是必然失败"来认这条分支：那样第一个账本一失败
	// `runRecords` 就整条返回，第二个账本根本不会被请求 ——
	// "两个账本各一次"这条判据反而永远测不到。
	http.session().
		route("aiDou=0&", signinRecordsPage(0, nil)).
		route("aiDou=1&", signinRecordsPage(0, nil))

	_, stderr, code := signinCLIAt(t, http, env, "--records", "--status-only")
	if code != ExitSuccess {
		t.Fatalf("该走 records（只读分支）应退 %d，实得 %d\n  stderr=%q",
			ExitSuccess, code, stderr)
	}
	if !strings.Contains(stderr, "以 iOS App 身份取豆的收支流水") {
		t.Errorf("进度行该说是在取流水: %q", stderr)
	}
	if strings.Contains(stderr, "查询签到状态") {
		t.Errorf("`--status-only` 不该赢过 `--records`: %q", stderr)
	}
	if got := http.count("cloudRecordV2"); got != 2 {
		t.Errorf("两个账本各一次，实得 %d 次", got)
	}
	if got := http.count("page/infoV3"); got != 0 {
		t.Errorf("`--status-only` 没赢，就不该查 infoV3，实得 %d 次", got)
	}
	if got := http.count("/startSignIn"); got != 0 {
		t.Errorf("这条分支不该发写请求，实得 %d 次", got)
	}
}

// `--raw` > `--status-only`。
func TestSigninRawWinsOverStatusOnly(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("page/infoV3", `{"code":0,"msg":"success","result":{"raw":1}}`)

	stdout, stderr, code := signinCLIAt(t, http, env, "--raw", "--status-only")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stderr, "以 iOS App 身份取 infoV3 原始响应") {
		t.Errorf("进度行该说是在取原始响应: %q", stderr)
	}
	if strings.Contains(stderr, "查询签到状态") {
		t.Errorf("`--status-only` 不该赢过 `--raw`: %q", stderr)
	}
	if got := http.count("page/infoV3"); got != 1 {
		t.Errorf("`--raw` 只查一次 infoV3，实得 %d 次", got)
	}
	if got := http.count("/startSignIn"); got != 0 {
		t.Errorf("这条分支不该发写请求，实得 %d 次", got)
	}
	if !strings.Contains(stdout, `"raw": 1`) {
		t.Errorf("原始响应该原样打出来:\n%s", stdout)
	}
}

// `--prizes` > `--status-only`。
func TestSigninPrizesWinsOverStatusOnly(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("/public/record", `{"code":0,"msg":"success","result":{"records":[]}}`)

	_, stderr, code := signinCLIAt(t, http, env, "--prizes", "--status-only")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stderr, "以 iOS App 身份查待领取奖品") {
		t.Errorf("进度行该说是在查奖品: %q", stderr)
	}
	if strings.Contains(stderr, "查询签到状态") {
		t.Errorf("`--status-only` 不该赢过 `--prizes`: %q", stderr)
	}
	if got := http.count("/public/record"); got != 1 {
		t.Errorf("奖品只取一页，实得 %d 次", got)
	}
}

// `--force --status-only` 走 status-only，**`--force` 被整个忽略** ——
// 所以一个写请求都不该发。
func TestSigninForceIsIgnoredWhenStatusOnlyWins(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	stdout, stderr, code := signinCLIAt(t, http, env, "--force", "--status-only")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if got := http.count("/startSignIn"); got != 0 {
		t.Errorf("`--force` 在 status-only 下该被忽略，实得 %d 次写请求", got)
	}
	if !strings.Contains(stdout, "今天：已签") {
		t.Errorf("该打状态块:\n%s", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// `--records` 的分页循环
// ─────────────────────────────────────────────────────────────────────────────

// 默认只取一页：两个账本各一次 `cloudRecordV2`，页大小 50。
func TestSigninRecordsTakesOnePagePerBookByDefault(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&", signinRecordsPage(2, signinTestInt64Ptr(2))).
		route("aiDou=1&", signinRecordsPage(1, signinTestInt64Ptr(1)))

	stdout, stderr, code := signinCLIAt(t, http, env, "--records")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if got := http.count("aiDou=0&"); got != 1 {
		t.Errorf("不给 `--records-all` 时每个账本只取一页，实得 %d 次", got)
	}
	if got := http.count("aiDou=1&"); got != 1 {
		t.Errorf("不给 `--records-all` 时每个账本只取一页，实得 %d 次", got)
	}
	if got := http.count("pageSize=50"); got != 2 {
		t.Errorf("页大小该是 50，实得 %d 次带它的请求", got)
	}
	for _, want := range []string{"云朵账本（改名前）", "AI豆账本（改名后）", "共 2 条，本页 2 条"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("输出里该有 %q:\n%s", want, stdout)
		}
	}
}

// `--records-all` 翻到 `total` 收满为止。
func TestSigninRecordsAllStopsWhenTotalIsReached(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&",
			signinRecordsPage(50, signinTestInt64Ptr(60)),
			signinRecordsPage(10, signinTestInt64Ptr(60))).
		route("aiDou=1&", signinRecordsPage(1, signinTestInt64Ptr(1)))

	_, stderr, code := signinCLIAt(t, http, env, "--records", "--records-all")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if got := http.count("aiDou=0&"); got != 2 {
		t.Errorf("收满 60 条该在第 2 页停，实得 %d 页", got)
	}
	if got := http.count("aiDou=1&"); got != 1 {
		t.Errorf("第一个账本已收满，实得 %d 页", got)
	}
}

// ⚠ `total` 缺失时，"**这一页是空的**"是唯一的终止依据。
//
// 少了那个条件，上游不给 `total` 时 `--records-all` 会一路翻到 20 页上限 ——
// 而它不会报错，只是白打 18 次请求。
func TestSigninRecordsAllStopsOnAnEmptyPageWithoutTotal(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&", signinRecordsPage(50, nil), signinRecordsPage(0, nil)).
		route("aiDou=1&", signinRecordsPage(0, nil))

	_, stderr, code := signinCLIAt(t, http, env, "--records", "--records-all")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if got := http.count("aiDou=0&"); got != 2 {
		t.Errorf("空页就该停（`total` 没给），实得 %d 页", got)
	}
}

// 上限 20 页：上游 `total` 恒不回落到 0 时不能无限翻。
func TestSigninRecordsAllStopsAtThePageCap(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&", signinRecordsPage(1, signinTestInt64Ptr(1000))).
		route("aiDou=1&", signinRecordsPage(0, nil))

	_, stderr, code := signinCLIAt(t, http, env, "--records", "--records-all")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if got := http.count("aiDou=0&"); got != 20 {
		t.Errorf("`total` 永不满足时该在上限 20 页停，实得 %d 页", got)
	}
}

// ⚠ `total` 取**第一个非 nil** 的那个（`page.total.or(total)`），后到的不覆盖。
//
// 只在两页给的 `total` 不一致时才有差别（那是上游数据有问题），
// 所以探针只能这么造：第一页说 100、第二页说 999，输出必须是 100。
func TestSigninRecordsKeepsTheFirstTotalNotTheLast(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&",
			signinRecordsPage(50, signinTestInt64Ptr(100)),
			signinRecordsPage(50, signinTestInt64Ptr(999))).
		route("aiDou=1&", signinRecordsPage(0, nil))

	stdout, stderr, code := signinCLIAt(t, http, env, "--records", "--records-all")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "共 100 条") {
		t.Errorf("该保留**先到的** total（100），后到的 999 不该覆盖它:\n%s", stdout)
	}
	if strings.Contains(stdout, "共 999 条") {
		t.Errorf("total 被后一页覆盖了:\n%s", stdout)
	}
}

// `nil` 的 `total` 不是 0："上游没给"与"一条都没有"是两件事。
func TestSigninRecordsTotalMissingIsNotZero(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&", signinRecordsPage(2, nil)).
		route("aiDou=1&", signinRecordsPage(0, nil))

	stdout, stderr, code := signinCLIAt(t, http, env, "--records")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "共 上游没给 条") {
		t.Errorf("上游没给 total 时该说「没给」，而不是编一个 0:\n%s", stdout)
	}
	if !strings.Contains(stdout, "（本页没有记录）") {
		t.Errorf("空页不是错，该说一句:\n%s", stdout)
	}
}

// `--records --json`：两个账本各一个键（`cloud` 在前），
// 空数组给 `[]` 而不是 `null`，没给的 `total` 给 `null`。
func TestSigninRecordsJSONKeepsBothBooksAndEmptyArrays(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("aiDou=0&", signinRecordsPage(1, signinTestInt64Ptr(1))).
		route("aiDou=1&", `{"code":0,"msg":"success","result":{}}`)

	stdout, stderr, code := signinCLIAt(t, http, env, "--records", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	var decoded map[string]any
	if err := json.Unmarshal([]byte(stdout), &decoded); err != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", err, stdout)
	}
	for _, key := range []string{"cloud", "bean"} {
		if _, ok := decoded[key]; !ok {
			t.Errorf("少了账本 %q:\n%s", key, stdout)
		}
	}
	// "一条都没有"与"这个字段不存在"是两件事 —— 空的是 `[]`，没给的是 `null`。
	if !strings.Contains(stdout, `"records": []`) {
		t.Errorf("空账本该给 `[]` 而不是 `null`:\n%s", stdout)
	}
	if !strings.Contains(stdout, `"total": null`) {
		t.Errorf("上游没给 total 该给 `null`:\n%s", stdout)
	}
	if !strings.Contains(stdout, `"aiDou": 0`) || !strings.Contains(stdout, `"aiDou": 1`) {
		t.Errorf("两个账本都该带 aiDou:\n%s", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 设备形态
// ─────────────────────────────────────────────────────────────────────────────

// 显式给了 `--device-id` 就不该再打一次第三方风控去换。
// 别名 `--h5-device-id` 必须等价 —— 只判 `device-id` 的实现会在这里回落。
//
// ⚠ 账号**故意不预绑**设备特征：预绑了的话，走库里那条路也**不打网络**
// （`app.DeviceID` 有就直接返回），于是"到底有没有回落"根本测不出来 ——
// 探针会静默变绿。不预绑时，一旦回落就会去换一个（`/deviceprofile/v4`）。
func TestSigninExplicitDeviceIDIsNotRefetched(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH"})
	explicit := signinDeviceID("Z")

	for _, flag := range []string{"--device-id", "--h5-device-id"} {
		t.Run(flag, func(t *testing.T) {
			http := &signinScript{}
			http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

			_, stderr, code := signinCLIAt(t, http, env, flag, explicit)
			if code != ExitSuccess {
				t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
			}
			if got := http.count("/deviceprofile/v4"); got != 0 {
				t.Errorf("显式给了 deviceId 就不该再上报，实得 %d 次", got)
			}
			// 回落了的话，那次上报没有路由 → 必然降级，于是 stderr 上会留下这句。
			if strings.Contains(stderr, "取 H5 deviceId 失败") {
				t.Errorf("显式给了 deviceId 就不该再去换一个: %q", stderr)
			}
		})
	}
}

// ⚠ `deviceId` 解析失败**不致命**：降级成"不带"+ stderr 一句，签到照样跑。
//
// 真正需要它的 `--receive` 会从上游拿到 `615` —— 那时这句提示就是
// "为什么是 615"的唯一线索。提前把它变成致命错误会让日常签到挂掉。
func TestSigninDeviceIDFailureDegradesInsteadOfBlocking(t *testing.T) {
	// 故意不预绑设备特征：于是取 deviceId 会走"现绑 + 上报"那条路，
	// 而上报没有路由 → 必然失败。
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH"})
	http := &signinScript{}
	http.session().
		route("page/infoV3", signinSignsToday(nil)...).
		route("page/startSignIn", signinStartOK())

	stdout, stderr, code := signinCLIAt(t, http, env)
	if code != ExitSuccess {
		t.Fatalf("取 deviceId 失败不该让签到失败，应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stderr, "⚠ 取 H5 deviceId 失败，这次不带它") {
		t.Errorf("该降级成一句 stderr 警告: %q", stderr)
	}
	if !strings.Contains(stdout, "签到成功：+50 豆") {
		t.Errorf("签到本身该照跑:\n%s", stdout)
	}
}

// `--device-short` 给了就只做 `ToUpper`（Rust 的 `to_ascii_uppercase`，**不** trim），
// 而且整次运行只解析一次 —— 两个账号的设备串必须**逐字相同**。
func TestSigninDeviceShortIsUppercasedAndFixedForTheWholeRun(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH-1",
			deviceID: signinDeviceID("A")},
		signinAcct{id: "19****02", enabled: true, auth: "Basic SYNTH-2",
			deviceID: signinDeviceID("A")},
	)
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	_, stderr, code := signinCLIAt(t, http, env, "--device-short", "abcdef123456")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	deviceInfos := http.headers("x-DeviceInfo")
	if len(deviceInfos) < 2 {
		t.Fatalf("该有两个账号的设备串，实得 %d 条", len(deviceInfos))
	}
	for _, info := range deviceInfos {
		if !strings.Contains(info, "|ABCDEF123456|") {
			t.Errorf("`--device-short` 该被转成大写: %q", info)
		}
	}
	if deviceInfos[0] != deviceInfos[len(deviceInfos)-1] {
		t.Errorf("整次运行只该解析一次设备形态，实得两串不同:\n%q\n%q",
			deviceInfos[0], deviceInfos[len(deviceInfos)-1])
	}
}

// ⚠ `--device-short ""` 与"没给"必须分得开，而**给了就只做 ToUpper**：
// 顺手 trim 一下会让用户给的串变成另一个（多一个空格就是另一台设备）。
func TestSigninDeviceShortIsNotTrimmed(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	_, stderr, code := signinCLIAt(t, http, env, "--device-short", " abcdef123456 ")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if info := http.header("/user/querySpecTokenV2", "x-DeviceInfo"); !strings.Contains(info, "| ABCDEF123456 |") {
		t.Errorf("只该 ToUpper、不该 trim: %q", info)
	}
}

// App 端 `deviceId` 在**动手之前**解析一次：解析不出来就当场停，
// 一个账号都不跑、一个请求都不发 —— 而不是跑一半再发现设备不对。
func TestSigninAppDeviceIsResolvedBeforeAnyAccountRuns(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH"})
	// 把 deviceId 的落点指到一个"父路径是文件"的地方 → 写盘必然失败。
	env.appDeviceFile = filepath.Join(env.db, "device-app.txt")
	http := &signinScript{}

	_, stderr, code := signinCLIAt(t, http, env)
	if code != ExitRun {
		t.Fatalf("应退 %d，实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if http.sent() != 0 {
		t.Errorf("设备形态解析不出来就不该开始跑账号，实得 %d 个请求", http.sent())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 四个写开关
// ─────────────────────────────────────────────────────────────────────────────

// ⚠ `--receive` 的 `ok` = "**这次调用没出错**"，不等于"领到了"。
//
// 自动模式下每条账目各自的成败在 `items` 里，而 `ok` 仍是 `true` ——
// 把 `ok` 实现成"领到了"的实现会在这里给出 `ok:false`，甚至让账号整体失败。
func TestSigninReceiveFailureStaysDataNotAnAccountFailure(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().
		route("page/infoV3", signinSignsToday(map[string]any{
			"receiveList": []any{map[string]any{"cloudType": 0, "cloudNum": 6}},
		})...).
		route("page/startSignIn", signinStartOK()).
		// 上游对"活动太火爆"回 11029 —— 请求被处理了，只是这次没给。
		route("page/receiveV3", map[string]any{"code": 11029, "msg": "活动太火爆"})

	stdout, stderr, code := signinCLIAt(t, http, env, "--receive", "--json")
	if code != ExitSuccess {
		t.Fatalf("领取失败不该让账号失败，应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	var decoded map[string]any
	if err := json.Unmarshal([]byte(stdout), &decoded); err != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", err, stdout)
	}
	receive, ok := decoded["receive"].(map[string]any)
	if !ok {
		t.Fatalf("该有 receive 那个键:\n%s", stdout)
	}
	if receive["ok"] != true {
		t.Errorf("`ok` 是「这次调用没出错」，实得 %v", receive["ok"])
	}
	if receive["mode"] != "auto" {
		t.Errorf("没给 `--cloud-type` 就该走自动分流，实得 %v", receive["mode"])
	}
	items, _ := receive["items"].([]any)
	if len(items) != 1 {
		t.Fatalf("账目里有一条，items 就该有一条:\n%s", stdout)
	}
	entry, _ := items[0].(map[string]any)
	if entry["action"] != "failed" {
		t.Errorf("这一条该如实记成失败，实得 %v", entry["action"])
	}
}

// ⚠⚠ `--cloud-type` 是"显式模式"的**开关本身**，而 `0` 是**合法取值**（当前可领取）。
// 用 `a.CloudType != 0` 判的实现会把 `--cloud-type 0` 当成"没给"，
// 于是白发一次 infoV3、还多领一次账目里的东西。
func TestSigninExplicitReceiveIsSwitchedByCloudTypeZero(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	// ⚠ `receiveList` 里放一条**自动分流会真的去领**的账目：实现若把
	// `--cloud-type 0` 当成"没给"，就会多查一次 infoV3、再多发一次 receiveV3 ——
	// 于是"只发这一笔"这条判据在两处同时变红。
	http.session().
		route("page/infoV3", signinInfoV3(true, 10502, map[string]any{
			"receiveList": []any{map[string]any{"cloudType": 0, "cloudNum": 6}},
		})).
		route("page/receiveV3", map[string]any{"code": 0, "msg": "success",
			"result": map[string]any{"receive": 6}})

	stdout, stderr, code := signinCLIAt(t, http, env, "--receive", "--cloud-type", "0", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	// 主路查一次 infoV3（今天已签，于是不再发 startSignIn）；显式模式**不该再查第二次**
	// —— 那次查询只属于自动分流。
	if got := http.count("page/infoV3"); got != 1 {
		t.Errorf("显式模式只发这一笔，不该为自动分流再查 infoV3，实得 %d 次", got)
	}
	if got := http.count("page/receiveV3"); got != 1 {
		t.Errorf("显式模式只该发一次 receiveV3，实得 %d 次", got)
	}
	var decoded map[string]any
	if err := json.Unmarshal([]byte(stdout), &decoded); err != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", err, stdout)
	}
	receive, _ := decoded["receive"].(map[string]any)
	if receive["mode"] != "explicit" {
		t.Errorf("`--cloud-type 0` 该走显式模式，实得 %v", receive["mode"])
	}
	if receive["received"] != float64(6) {
		t.Errorf("该报出上游回的 result.receive，实得 %v", receive["received"])
	}
}

// `--receive` 那行要说清 deviceId **是从哪来的** ——
// 不带时上游回 `615`，而"为什么是 615"只有这一行说得出来。
func TestSigninReceiveNamesWhereTheDeviceCameFrom(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	explicit := signinDeviceID("Z")

	cases := []struct {
		name string
		flag []string
		want string
	}{
		{"显式给的", []string{"--device-id", explicit}, "用 --device-id 给的"},
		{"库里/新上报的", nil, "用库里/新上报的（BAAAAAAA…）"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			http := &signinScript{}
			http.session().
				route("page/infoV3", signinSignsToday(nil)...).
				route("page/startSignIn", signinStartOK())

			args := append([]string{"--receive"}, tc.flag...)
			stdout, stderr, code := signinCLIAt(t, http, env, args...)
			if code != ExitSuccess {
				t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
			}
			if !strings.Contains(stdout, tc.want) {
				t.Errorf("该说清 deviceId 的来源（%q）:\n%s", tc.want, stdout)
			}
			if !strings.Contains(stdout, "账目里没有可处理的条目") {
				t.Errorf("账目为空时该明说，而不是什么都不打:\n%s", stdout)
			}
		})
	}
}

// `--multiple` 的 `ok` / `claimed` 是**两个**字段：门槛不成立（没领）也是 `ok:true`，
// 而 `claimed` 才是"到底领到没有"。混成一个会让定时任务分不清
// "本月已经领过"与"这次领失败了"。
func TestSigninMultipleReportsClaimedSeparatelyFromOK(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})

	t.Run("门槛成立", func(t *testing.T) {
		http := &signinScript{}
		http.session().
			route("page/infoV3", signinInfoV3(true, 10502, map[string]any{
				"curMonthBackup": true, "curMonthBackupSignAccept": false,
			})).
			route("page/multiple", map[string]any{"code": 0, "msg": "success",
				"result": map[string]any{"multiple": 2, "cloudCount": 100}})

		stdout, stderr, code := signinCLIAt(t, http, env, "--multiple", "--json")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		var decoded map[string]any
		if err := json.Unmarshal([]byte(stdout), &decoded); err != nil {
			t.Fatalf("stdout 该是一份 JSON: %v\n%s", err, stdout)
		}
		extra, _ := decoded["multiple"].(map[string]any)
		if extra["ok"] != true || extra["claimed"] != true {
			t.Errorf("领到了该是 ok+claimed 都为 true，实得 %v", extra)
		}
	})

	t.Run("门槛不成立", func(t *testing.T) {
		http := &signinScript{}
		http.session().
			route("page/infoV3", signinInfoV3(true, 10502, map[string]any{
				"curMonthBackup": true, "curMonthBackupSignAccept": true,
			}))

		stdout, stderr, code := signinCLIAt(t, http, env, "--multiple", "--json")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if got := http.count("page/multiple"); got != 0 {
			t.Errorf("门槛不成立就一个请求都不该发，实得 %d 次", got)
		}
		var decoded map[string]any
		if err := json.Unmarshal([]byte(stdout), &decoded); err != nil {
			t.Fatalf("stdout 该是一份 JSON: %v\n%s", err, stdout)
		}
		extra, _ := decoded["multiple"].(map[string]any)
		if extra["ok"] != true {
			t.Errorf("没领**不是错**，ok 该是 true，实得 %v", extra["ok"])
		}
		if extra["claimed"] != false {
			t.Errorf("没领到该是 claimed:false，实得 %v", extra["claimed"])
		}
	})
}

// ⚠ 「本月没有备份好礼」（`curMonth` 为假）与「未开启自动备份」（`state=-1`）
// 是**两件事**，下一步动作也不同。把它们说成一回事，
// 会让人去追一个根本不存在的备份问题。
func TestSigninBackupGiftSeparatesNoGiftFromNoBackup(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})

	t.Run("未开备份", func(t *testing.T) {
		http := &signinScript{}
		// `--backup-gift` 跑在签到主路**之后**，主路不过就轮不到它 ——
		// 所以 infoV3 / startSignIn 也得装上（否则挂的是主路，判据根本没跑到）。
		http.session().
			route("page/infoV3", signinSignsToday(nil)...).
			route("page/startSignIn", signinStartOK()).
			route("backupgift/info", map[string]any{"code": 0, "msg": "success",
				"result": map[string]any{"state": -1, "curMonth": true}})

		stdout, stderr, code := signinCLIAt(t, http, env, "--backup-gift")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if !strings.Contains(stdout, "未开启自动备份") {
			t.Errorf("该说清是没开备份:\n%s", stdout)
		}
		if got := http.count("backupgift/receive"); got != 0 {
			t.Errorf("门槛不成立就不该发写请求，实得 %d 次", got)
		}
	})

	t.Run("本月没有这份礼", func(t *testing.T) {
		http := &signinScript{}
		http.session().
			route("page/infoV3", signinSignsToday(nil)...).
			route("page/startSignIn", signinStartOK()).
			route("backupgift/info", map[string]any{"code": 0, "msg": "success",
				"result": map[string]any{"state": 0, "curMonth": false}})

		stdout, stderr, code := signinCLIAt(t, http, env, "--backup-gift")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if !strings.Contains(stdout, "本月没有备份好礼") {
			t.Errorf("该说清是本月没有这份礼:\n%s", stdout)
		}
		if strings.Contains(stdout, "未开启自动备份") {
			t.Errorf("`curMonth` 为假**不是**未开备份 —— 两者被说成一回事了:\n%s", stdout)
		}
		if got := http.count("backupgift/receive"); got != 0 {
			t.Errorf("门槛不成立就不该发写请求，实得 %d 次", got)
		}
	})
}

// ⚠⚠ `--revive` 不可逆，所以它的 `deviceid` 头**只取显式 `--device-id`**、
// 故意不回落库：H5 的定义里没有 `isDeviceId`，默认就该一个头都不带。
// 库里明明有一份 id，探针就打在"它没被用上"。
func TestSigninReviveNeverFallsBackToTheStoredDevice(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	const reward = `{"code":0,"msg":"success","result":` +
		`{"cardType":1,"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}`

	t.Run("没给就不带", func(t *testing.T) {
		http := &signinScript{}
		http.session().
			route("page/infoV3", signinSignsToday(nil)...).
			route("page/startSignIn", signinStartOK()).
			route("receiveRevivalReward", reward)

		stdout, stderr, code := signinCLIAt(t, http, env, "--revive")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if got := http.header("receiveRevivalReward", "deviceid"); got != "" {
			t.Errorf("没显式给 `--device-id` 就不该带这个头，实得 %q", got)
		}
		if !strings.Contains(stdout, "用掉一张复活卡：本次 +114 豆，累计 571") {
			t.Errorf("该如实报出用掉的卡与收益:\n%s", stdout)
		}
	})

	t.Run("给了就带上", func(t *testing.T) {
		explicit := signinDeviceID("Z")
		http := &signinScript{}
		http.session().
			route("page/infoV3", signinSignsToday(nil)...).
			route("page/startSignIn", signinStartOK()).
			route("receiveRevivalReward", reward)

		_, stderr, code := signinCLIAt(t, http, env, "--revive", "--device-id", explicit)
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if got := http.header("receiveRevivalReward", "deviceid"); got != explicit {
			t.Errorf("显式给了就该带上那个头，实得 %q", got)
		}
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 输出流、失败隔离与退出码
// ─────────────────────────────────────────────────────────────────────────────

// ⚠ 进度行**一律走 stderr**（与 Rust 的刻意差异：Rust 非 `--json` 时走 stdout）。
// 而且它在**发请求之前**就打 —— 那才是这行的用处（等待期间看得见进度）。
func TestSigninProgressLineGoesToStderrBeforeTheResult(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	// 空脚本：第一个请求就失败，于是 stderr 上同时有进度行与失败行。
	http := &signinScript{}

	_, stderr, code := signinCLIAt(t, http, env)
	if code != ExitRun {
		t.Fatalf("应退 %d，实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	const progress = "账号 1 (19****01)：以 iOS App 身份签到"
	at := strings.Index(stderr, progress)
	if at < 0 {
		t.Fatalf("进度行该走 stderr，实得 stderr=%q", stderr)
	}
	if fail := strings.Index(stderr, "✗ 账号 1 (19****01) 失败"); fail < 0 {
		t.Errorf("失败该走 stderr 并带 ✗:\n%s", stderr)
	} else if fail < at {
		t.Errorf("进度行该在失败之前打（它本来就是「发请求之前」的那一行）:\n%s", stderr)
	}
}

// **一个账号失败不影响其余**：逐个跑、失败记 stderr、继续下一个，
// 最后用非零退出码统一报"有几个失败"。
func TestSigninOneAccountFailingDoesNotStopTheOthers(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH-1",
			deviceID: signinDeviceID("A")},
		signinAcct{id: "19****02", enabled: true, auth: "Basic SYNTH-2",
			deviceID: signinDeviceID("A")},
	)
	http := &signinScript{}
	// 第二个账号的建会话第一步就失败（第一条响应正常、第二条报错）。
	http.route("/user/querySpecTokenV2",
		map[string]any{"success": true, "code": "0000", "message": "请求成功",
			"data": map[string]any{"token": "SSO-TOKEN", "resultCode": "104000"}},
		map[string]any{"success": false, "code": "200059541"},
	).
		route("/ycloud/api/timestamp", map[string]any{"code": 0, "result": 1789800128813}).
		route("/ycloud/auth-service/auth/tyrzLogin", map[string]any{"code": 0,
			"result": map[string]any{"token": "JWT"}}).
		route("page/infoV3", signinInfoV3(true, 10502, nil))

	stdout, stderr, code := signinCLIAt(t, http, env)
	if code != ExitRun {
		t.Fatalf("有账号失败就该退 %d，实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "✗ 账号 2 (19****02) 失败") {
		t.Errorf("失败的那个该被点名:\n%s", stderr)
	}
	if !strings.Contains(stderr, "1 个账号失败（本次共 2 个）") {
		t.Errorf("退出码与汇总该同源:\n%s", stderr)
	}
	if !strings.Contains(stdout, "账号 1 (19****01)") ||
		!strings.Contains(stdout, "今天已经签过了，本次没有重复提交。") {
		t.Errorf("第一个账号该照常跑完:\n%s", stdout)
	}
}

// `--json` 的形状：**一个条目给对象、否则给数组**。
//
// ⚠ 判据是"产出条目的个数"（Rust 的 `results.len()`），不是目标账号个数 ——
// 而失败账号**也会 push 一个对象**（Rust `signin_run.rs:93-100` 同样如此），
// 所以"2 个账号里 1 个失败"仍然是**两个**条目 → 数组。
func TestSigninJSONObjectForOneAccountArrayForMany(t *testing.T) {
	one := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	two := signinFixture(t,
		signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH-1",
			deviceID: signinDeviceID("A")},
		signinAcct{id: "19****02", enabled: true, auth: "Basic SYNTH-2",
			deviceID: signinDeviceID("A")},
	)

	t.Run("一个账号给对象", func(t *testing.T) {
		http := &signinScript{}
		http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

		stdout, stderr, code := signinCLIAt(t, http, one, "--json")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if !strings.HasPrefix(strings.TrimSpace(stdout), "{") {
			t.Errorf("单账号该给一个对象（老脚本按对象读）:\n%s", stdout)
		}
		if !json.Valid([]byte(stdout)) {
			t.Errorf("stdout 该是一份干净的 JSON:\n%s", stdout)
		}
	})

	t.Run("两个账号给数组", func(t *testing.T) {
		http := &signinScript{}
		http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

		stdout, stderr, code := signinCLIAt(t, http, two, "--json")
		if code != ExitSuccess {
			t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
		}
		if !strings.HasPrefix(strings.TrimSpace(stdout), "[") {
			t.Errorf("多账号该给数组:\n%s", stdout)
		}
	})

	t.Run("两个账号一个失败仍是数组", func(t *testing.T) {
		http := &signinScript{}
		http.route("/user/querySpecTokenV2",
			map[string]any{"success": true, "code": "0000", "message": "请求成功",
				"data": map[string]any{"token": "SSO-TOKEN", "resultCode": "104000"}},
			map[string]any{"success": false, "code": "200059541"},
		).
			route("/ycloud/api/timestamp", map[string]any{"code": 0, "result": 1789800128813}).
			route("/ycloud/auth-service/auth/tyrzLogin", map[string]any{"code": 0,
				"result": map[string]any{"token": "JWT"}}).
			route("page/infoV3", signinInfoV3(true, 10502, nil))

		stdout, stderr, code := signinCLIAt(t, http, two, "--json")
		if code != ExitRun {
			t.Fatalf("应退 %d，实得 %d\n  stderr=%q", ExitRun, code, stderr)
		}
		var entries []any
		if err := json.Unmarshal([]byte(stdout), &entries); err != nil {
			t.Fatalf("失败账号也会产出条目，所以这里是数组: %v\n%s", err, stdout)
		}
		if len(entries) != 2 {
			t.Errorf("该有两个条目（一个结果 + 一个失败），实得 %d:\n%s", len(entries), stdout)
		}
		if !strings.Contains(stdout, `"ok": false`) {
			t.Errorf("失败那一档该带 ok:false:\n%s", stdout)
		}
	})
}

// ⚠ `--status-only` 落库记的是 `bean_total`（上游 `total`），**不是** `beforeTotal`。
//
// 后者是改名时结转进来的那笔、不随领取变化 —— 记它等于让趋势图永远是一条直线。
func TestSigninStatusOnlyRecordsTheBeanTotalNotTheBeforeTotal(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, map[string]any{
		"beforeTotal": 8027,
	}))

	stdout, stderr, code := signinCLIAt(t, http, env, "--status-only")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "余额已记录：10502 豆") {
		t.Errorf("该记余额（total=10502）:\n%s", stdout)
	}
	if strings.Contains(stdout, "余额已记录：8027 豆") {
		t.Errorf("记的是 beforeTotal（那张冻结的快照）—— 趋势图会变成直线:\n%s", stdout)
	}
}

// `--raw` 是**拿来看的**，不解析、不落库：记一条"余额未知"的历史只会往趋势图塞噪声。
func TestSigninRawDoesNotRecordBalance(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("page/infoV3", `{"code":0,"msg":"success","result":{"total":10502}}`)

	stdout, stderr, code := signinCLIAt(t, http, env, "--raw")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if strings.Contains(stdout, "余额已记录") {
		t.Errorf("`--raw` 不该落库:\n%s", stdout)
	}
	if !strings.Contains(stdout, `"total": 10502`) {
		t.Errorf("原始响应该原样打出来:\n%s", stdout)
	}
}

// `--prizes --json` 用 **typed 结构、不带 `Raw`**（刻意与 Rust 不同）：
// 上游原样条目里带 `userDomainId`（账号标识），而 `upstream.PrizeRecordItem`
// 就是为了不把它带出来才只留 typed 字段的 —— 安全优先于逐字对齐。
func TestSigninPrizesJSONIsTypedWithoutTheRawItem(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})
	http := &signinScript{}
	http.session().route("/public/record", `{"code":0,"msg":"success","result":{"records":[`+
		`{"flag":1,"type":1057,"prizeId":260630014,"prizeName":"移动云盘1000万tokens叠加包",`+
		`"insertTime":"2026-09-16T11:12:54.000+00:00",`+
		`"expireTime":"2026-09-30T15:59:59.000+00:00",`+
		`"marketName":"sign_in_3_ex","oid":1270932714,`+
		`"userDomainId":"1990000000000000001"}]}}`)

	stdout, stderr, code := signinCLIAt(t, http, env, "--prizes", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if strings.Contains(stdout, "userDomainId") || strings.Contains(stdout, "1990000000000000001") {
		t.Errorf("账号标识被带出来了:\n%s", stdout)
	}
	// 原样条目那个键叫 `raw`（Rust 侧 `item.raw`）—— 它存在就说明带了原样条目。
	if strings.Contains(stdout, `"raw"`) {
		t.Errorf("不该带原样条目（它含账号标识）:\n%s", stdout)
	}
	for _, want := range []string{`"marketName": "sign_in_3_ex"`, `"status": 1`,
		`"pageSize": 10`, `"prizeName"`, `"insertTime"`, `"expireTime"`, `"prizeId"`} {
		if !strings.Contains(stdout, want) {
			t.Errorf("typed 形状里该有 %s:\n%s", want, stdout)
		}
	}
	if !json.Valid([]byte(stdout)) {
		t.Errorf("stdout 该是一份干净的 JSON:\n%s", stdout)
	}
}

// 余额落库那行**只在人看的模式打**：`--json` 的文档里没有它（Rust 同样不带）——
// 否则脚本消费的那份 JSON 里会混进一句人话。
func TestSigninCloudNoteIsHumanOnly(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true,
		auth: "Basic SYNTH", deviceID: signinDeviceID("A")})

	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))
	stdout, _, code := signinCLIAt(t, http, env, "--status-only")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}
	if !strings.Contains(stdout, "余额已记录：10502 豆") {
		t.Errorf("人看的模式该打那一行:\n%s", stdout)
	}

	http = &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))
	stdout, stderr, code := signinCLIAt(t, http, env, "--status-only", "--json")
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if strings.Contains(stdout+stderr, "余额已记录") {
		t.Errorf("`--json` 不该带那一行:\n%s%s", stdout, stderr)
	}
	if !json.Valid([]byte(stdout)) {
		t.Errorf("stdout 该是一份干净的 JSON:\n%s", stdout)
	}
}

// 多账号时每个账号前有一行分隔，否则几段输出会糊成一片。
func TestSigninAccountsAreSeparatedInMultiAccountRuns(t *testing.T) {
	env := signinFixture(t,
		signinAcct{id: "19****01", enabled: true, auth: "Basic SYNTH-1",
			deviceID: signinDeviceID("A")},
		signinAcct{id: "19****02", enabled: true, auth: "Basic SYNTH-2",
			deviceID: signinDeviceID("A")},
	)
	http := &signinScript{}
	http.session().route("page/infoV3", signinInfoV3(true, 10502, nil))

	stdout, stderr, code := signinCLIAt(t, http, env)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	for _, want := range []string{"------ 账号 1 (19****01) ------", "------ 账号 2 (19****02) ------"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("多账号每段前该有 %q:\n%s", want, stdout)
		}
	}
	if !strings.Contains(stdout, "共 2 个账号：") {
		t.Errorf("多账号该有一行汇总（定时任务日志里最该一眼看到的）:\n%s", stdout)
	}
}

// ⚠ 用法校验必须跑在"打不开库"这类**运行**错误**之前**：
// 否则 `--records-all` 单独给会把退出码从 2 变成 1，
// 而脚本正是靠这两个值的差判断"是我的调用写错了"还是"这次真的没跑成"。
//
// ⚠ 探针必须让"打不开库"**真的发生**：库路径的父路径指向一个**普通文件**
// （而不是一个不存在的目录 —— 那样 SQLite 会顺手把目录建出来，于是库照样打开，
// 这条判据就永远测不到顺序）。
func TestSigninRecordsAllAloneIsUsageErrorBeforeTheDBIsOpened(t *testing.T) {
	env := signinFixture(t, signinAcct{id: "19****01", enabled: true})
	blocker := filepath.Join(env.dir, "blocker")
	if err := os.WriteFile(blocker, []byte("x"), 0o600); err != nil {
		t.Fatalf("建阻塞文件失败: %v", err)
	}

	_, stderr, code := signinCLI(t, &signinScript{},
		"signin", "--records-all",
		"--db", filepath.Join(blocker, "state.db"),
		"--app-device-file", env.appDeviceFile)
	if code != ExitUsage {
		t.Fatalf("应退 %d（用法错误），实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if !strings.Contains(stderr, "--records-all 需要同时给 --records") {
		t.Errorf("该点名缺了哪个 flag: %q", stderr)
	}
}
