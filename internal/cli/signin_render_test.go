package cli

import (
	"bytes"
	"encoding/json"
	"sort"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 渲染层的用例全部是**纯函数**级别的：入参是手搓的 [signinView]，出参是两个流里的字节。
//
// 所以这里既不需要 [refusingClient]（不联网）、也不需要 [instantWaiter]（不真睡）——
// 需要它们，说明被测的东西已经越过了渲染层的边界。
//
// ⚠ 判据一律落在**字节**上（`strings.Contains` / 整串相等 / 解一遍 JSON），
// 不落在"调用了哪个函数"上：断言"调用了掩码函数"在掩码函数被换掉时照样绿。

// ─────────────────────────────────────────────────────────────────────────────
// 脚手架
// ─────────────────────────────────────────────────────────────────────────────

// signinTestStatus 造一份最小可用的 infoV3 状态。
//
// 三个本月格 + 一个**非本月**的补位格：`cal` 里真的混着上/下月的格子，
// 而 `SignedOn` / `CountedMonthDays` 只看本月 —— 混一格进来才能测到那条。
func signinTestStatus(signedToday bool, mutate func(*upstream.SignInStatus)) *upstream.SignInStatus {
	status := &upstream.SignInStatus{
		BeanTotal:    7431,
		SignCount:    12,
		SignInPoints: 3,
		MonthDays:    12,
		Days: []upstream.SignInDay{
			{Day: 3, Signed: true, CurrentMonth: true},
			{Day: 4, Signed: true, CurrentMonth: true},
			{Day: 21, Signed: signedToday, CurrentMonth: true, Today: true},
			{Day: 30, Signed: true, CurrentMonth: false},
		},
	}
	if mutate != nil {
		mutate(status)
	}
	return status
}

func signinTestReport(beforeSigned, afterSigned bool, result *upstream.SignInResult) *app.SignInReport {
	return &app.SignInReport{
		Before: *signinTestStatus(beforeSigned, nil),
		After:  *signinTestStatus(afterSigned, nil),
		Result: result,
	}
}

func signinTestAccount(id, nickname, platform string, enabled bool) core.Account {
	account := core.NewAccount(id)
	account.Enabled = enabled
	if nickname != "" {
		account.Nickname = &nickname
	}
	if platform != "" {
		account.Platform = &platform
	}
	return account
}

// signinTestRender 跑一遍渲染，把两个流都拿回来。
func signinTestRender(t *testing.T, views []signinView, asJSON bool) (stdout, stderr string, err error) {
	t.Helper()
	var out, errOut bytes.Buffer
	err = renderSigninBatch(&output{stdout: &out, stderr: &errOut}, views, asJSON)
	return out.String(), errOut.String(), err
}

// signinTestKeys 按**出现顺序**取出顶层 JSON 的键。
//
// 只看缩进正好两个空格的那种行：`out.json` 用两空格缩进，顶层键就是这一档，
// 嵌套的键（4 空格起）与字符串值都不会被当成键。
func signinTestKeys(t *testing.T, encoded string) []string {
	t.Helper()
	var keys []string
	for _, line := range strings.Split(encoded, "\n") {
		if !strings.HasPrefix(line, "  \"") || strings.HasPrefix(line, "   ") {
			continue
		}
		key, _, ok := strings.Cut(line[2:], "\":")
		if !ok {
			continue
		}
		keys = append(keys, strings.TrimPrefix(key, "\""))
	}
	return keys
}

func signinTestStrPtr(s string) *string { return &s }
func signinTestInt64Ptr(n int64) *int64 { return &n }
func signinTestBoolPtr(b bool) *bool    { return &b }

// ─────────────────────────────────────────────────────────────────────────────
// 四态
// ─────────────────────────────────────────────────────────────────────────────

// TestSigninConclusionSeparatesTheFourStates 钉住"三种结局必须分开报"。
//
// `--force`（协议自检）那一档是最容易被合并掉的：它的 `Result` 非 nil、回查也是已签，
// 与"这次真签上了"在日志里长得一模一样 —— 而它**不是**这次签上的。
// 定时任务连跑一个月，只有分开才知道它到底有没有在干活。
func TestSigninConclusionSeparatesTheFourStates(t *testing.T) {
	cases := []struct {
		name    string
		before  bool
		after   bool
		result  *upstream.SignInResult
		want    string
		notWant string
	}{
		{
			name:   "这次真把今天从「未签」变成了「已签」",
			after:  true,
			result: &upstream.SignInResult{SignInPoints: 3},
			want:   "签到成功：+3 豆",
		},
		{
			name:    "来的时候就已签，没发请求",
			before:  true,
			after:   true,
			result:  nil,
			want:    "今天已经签过了，本次没有重复提交。",
			notWant: "签到成功",
		},
		{
			name:    "来的时候就已签，--force 又发了一次",
			before:  true,
			after:   true,
			result:  &upstream.SignInResult{SignInPoints: 3},
			want:    "今天本来就已签；按 --force 重发了一次 startSignIn，上游接受（协议自检通过）。",
			notWant: "签到成功",
		},
		{
			name:   "既没签上、也没有「今天已签」的凭据",
			result: &upstream.SignInResult{},
			want:   "⚠ 本次既未签上、也没有「今天已签」的凭据，请检查 infoV3 的返回值。",
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			stdout, _, err := signinTestRender(t, []signinView{{
				account: signinTestAccount("19****01", "", "", true),
				index:   1,
				mode:    signinModeSignIn,
				report:  signinTestReport(tc.before, tc.after, tc.result),
			}}, false)
			if err != nil {
				t.Fatalf("渲染失败: %v", err)
			}
			if !strings.Contains(stdout, tc.want) {
				t.Errorf("结论行应是 %q，实得:\n%s", tc.want, stdout)
			}
			if tc.notWant != "" && strings.Contains(stdout, tc.notWant) {
				t.Errorf("这一档不该出现 %q（四态被合并了），实得:\n%s", tc.notWant, stdout)
			}
		})
	}
}

// TestSigninFullPhoneNeverReachesTheOutput 钉住"完整手机号不许进输出"。
//
// 故意把完整号码塞进 `PhoneMasked`（库里那一列**本该**是掩码，这里当作"上游/旧数据
// 给了个没脱敏的值"）：它一旦被打印出来，就是"我们印了完整手机号"。
// 判据是**写出来的字节里没有那个串**，不是"调用了掩码函数"。
func TestSigninFullPhoneNeverReachesTheOutput(t *testing.T) {
	const full = "19900138000"
	account := signinTestAccount("19****00", "", "app", true)
	account.PhoneMasked = signinTestStrPtr(full)

	// ⚠ **两个**账号是刻意的：单账号时人看的那条路上一个账号标识都不打
	// （进度行归编排层、单账号又不打分隔行），"印的是掩码 id 而不是完整号码"
	// 这条判据就没有落点了 —— 那样它就变成一条**恒真**的断言。
	// 多账号的分隔行正是承载它的地方。
	views := []signinView{
		{
			account: account,
			index:   1,
			mode:    signinModeSignIn,
			report:  signinTestReport(false, true, &upstream.SignInResult{SignInPoints: 3}),
		},
		{
			account: signinTestAccount("13****01", "", "app", true),
			index:   2,
			mode:    signinModeSignIn,
			report:  signinTestReport(false, true, &upstream.SignInResult{SignInPoints: 3}),
		},
	}
	for _, asJSON := range []bool{false, true} {
		stdout, stderr, err := signinTestRender(t, views, asJSON)
		if err != nil {
			t.Fatalf("渲染失败: %v", err)
		}
		if strings.Contains(stdout+stderr, full) {
			t.Fatalf("完整手机号进了输出（asJSON=%v）:\n%s%s", asJSON, stdout, stderr)
		}
		// 人看的那条路上账号标识该是掩码 id。`--json` 的成功条目里**没有**
		// 账号字段（Rust 侧就没有，只有失败那一档才带 `account`），所以只查人看的那条。
		if !asJSON && !strings.Contains(stdout, "19****00") {
			t.Errorf("账号标识该是掩码 id，实得:\n%s", stdout)
		}
	}

	// `--list` 同一条规矩。
	var listOut, listErr bytes.Buffer
	renderSigninAccounts(&output{stdout: &listOut, stderr: &listErr}, []core.Account{account})
	if strings.Contains(listOut.String()+listErr.String(), full) {
		t.Fatalf("`--list` 把完整手机号打出来了:\n%s", listOut.String())
	}
	if !strings.Contains(listOut.String(), "19****00") {
		t.Errorf("`--list` 的账号列该是掩码 id，实得:\n%s", listOut.String())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 失败 vs 跳过
// ─────────────────────────────────────────────────────────────────────────────

// TestSigninFailureAndSkipAreTwoDifferentThings 钉住"没拿到报告"的两种处境分开。
//
// 失败是"做了没做成"（`✗`、走 stderr），跳过是"压根没做"（走 stdout）。
// 混成一个的话，定时任务连跑一个月也看不出它到底有没有在干活。
func TestSigninFailureAndSkipAreTwoDifferentThings(t *testing.T) {
	failed := signinView{
		account: signinTestAccount("19****01", "", "", true),
		index:   1,
		mode:    signinModeSignIn,
		failure: core.NewAuth("库里既没有整串凭据，也没有完整手机号"),
	}
	skipped := signinView{
		account: signinTestAccount("19****02", "", "", false),
		index:   2,
		mode:    signinModeSignIn,
		skipped: "账号已停用",
	}

	stdout, stderr, err := signinTestRender(t, []signinView{failed, skipped}, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if !strings.Contains(stderr, "✗ 账号 1 (19****01) 失败：凭据失效: 库里既没有整串凭据，也没有完整手机号") {
		t.Errorf("失败该走 stderr 且带 ✗ 与账号下标，实得 stderr:\n%s", stderr)
	}
	if strings.Contains(stdout, "✗") {
		t.Errorf("失败那行不该进 stdout，实得:\n%s", stdout)
	}
	if !strings.Contains(stdout, "账号 2 (19****02) 跳过：账号已停用") {
		t.Errorf("跳过该在 stdout 上单独说一句，实得:\n%s", stdout)
	}
	if strings.Contains(stderr, "跳过：") {
		t.Errorf("跳过不是错误，不该混进 stderr，实得:\n%s", stderr)
	}
}

// TestSigninJSONFailureEntryShape 钉住失败那一档的 JSON 形状（键名对位 Rust）。
func TestSigninJSONFailureEntryShape(t *testing.T) {
	view := signinView{
		account: signinTestAccount("19****01", "", "", true),
		index:   1,
		mode:    signinModeSignIn,
		failure: core.NewUpstream(615, "活动太火爆啦"),
	}
	stdout, _, err := signinTestRender(t, []signinView{view}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	var decoded map[string]any
	if uerr := json.Unmarshal([]byte(stdout), &decoded); uerr != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", uerr, stdout)
	}
	if decoded["ok"] != false {
		t.Errorf("失败那档的 ok 该是 false，实得 %v", decoded["ok"])
	}
	if decoded["account"] != "19****01" {
		t.Errorf("失败那档要能看出是哪个账号，实得 %v", decoded["account"])
	}
	if decoded["index"] != float64(1) {
		t.Errorf("失败那档要带上下标，实得 %v", decoded["index"])
	}
	if message, _ := decoded["error"].(string); !strings.Contains(message, "活动太火爆啦") {
		t.Errorf("失败那档要带上游的原文，实得 %v", decoded["error"])
	}
	if keys := signinTestKeys(t, stdout); !sort.StringsAreSorted(keys) {
		t.Errorf("JSON 键该按字典序（与 Rust 的 serde_json 一致），实得 %v", keys)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// `--list`
// ─────────────────────────────────────────────────────────────────────────────

// TestSigninListMatchesTheRustColumns 逐字钉住 `--list` 的表。
//
// 表头是**手写字面量**（Rust 那边也是手写的），所以它与下面的行格式对不齐 ——
// 这里连那点错位一起钉住：对齐目标是"与冻结的 Rust 输出逐字可 diff"。
func TestSigninListMatchesTheRustColumns(t *testing.T) {
	var stdout, stderr bytes.Buffer
	renderSigninAccounts(&output{stdout: &stdout, stderr: &stderr}, []core.Account{
		signinTestAccount("19****01", "阿一", "app", true),
		signinTestAccount("19****02", "", "", false),
	})
	want := "下标  账号          昵称            登录端   状态\n" +
		"   1  19****01      阿一              app     启用\n" +
		"   2  19****02      -               -       停用\n" +
		"\n不带 `--account` 直接跑，就是对**全部启用**的账号各签一次。\n"
	if got := stdout.String(); got != want {
		t.Errorf("`--list` 的表与 Rust 逐字对不上:\n got=%q\nwant=%q", got, want)
	}
	if stderr.Len() != 0 {
		t.Errorf("`--list` 没有要警告的东西，stderr 该是空的，实得 %q", stderr.String())
	}

	// 空库不是错误，但必须说清下一步做什么。
	var empty bytes.Buffer
	renderSigninAccounts(&output{stdout: &empty, stderr: &bytes.Buffer{}}, nil)
	want = "库里还没有账号。先登录一个：`mcloud login sms --client app`\n"
	if got := empty.String(); got != want {
		t.Errorf("空库该给一句「下一步」,\n got=%q\nwant=%q", got, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// `--json`
// ─────────────────────────────────────────────────────────────────────────────

// TestSigninJSONCarriesTheFourStates 钉住 `--json` 的键名与"状态类字段的口径"。
func TestSigninJSONCarriesTheFourStates(t *testing.T) {
	status := signinTestStatus(true, func(s *upstream.SignInStatus) {
		s.CloudTotal = signinTestInt64Ptr(5392)
	})
	view := signinView{
		account: signinTestAccount("19****01", "", "", true),
		index:   1,
		mode:    signinModeSignIn,
		report: &app.SignInReport{
			Before: *signinTestStatus(false, nil),
			After:  *status,
			Result: &upstream.SignInResult{SignInPoints: 3},
		},
	}
	stdout, _, err := signinTestRender(t, []signinView{view}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	var decoded map[string]json.RawMessage
	if uerr := json.Unmarshal([]byte(stdout), &decoded); uerr != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", uerr, stdout)
	}

	for _, key := range []string{
		"signedToday", "beanTotal", "cloudTotal", "signCount", "signInPoints",
		"monthDays", "countedMonthDays", "signedDays", "receiveList", "toReceive",
		"curMonthBackup", "curMonthBackupSignAccept",
		"signedNow", "alreadySigned", "beanBefore",
	} {
		if _, ok := decoded[key]; !ok {
			t.Errorf("JSON 少了键 %q:\n%s", key, stdout)
		}
	}
	// 四态的口径：`signedDays` 自己数 `cal`（只数本月），`monthDays` 是服务端报的。
	cases := map[string]string{
		"signedToday":      "true",
		"signedNow":        "true",
		"alreadySigned":    "false",
		"beanBefore":       "7431",
		"beanTotal":        "7431",
		"cloudTotal":       "5392",
		"signCount":        "12",
		"signInPoints":     "3",
		"monthDays":        "12",
		"countedMonthDays": "3",
		"signedDays":       "[3,4,21]",
		// 上游没给 `receiveList` 时是**空数组**不是 null ——
		// "一条都没有"与"这个字段不存在"是两件事。
		"receiveList":              "[]",
		"toReceive":                "null",
		"curMonthBackup":           "null",
		"curMonthBackupSignAccept": "null",
	}
	for key, want := range cases {
		// 抹掉缩进带来的换行与空格：这里比的是值，不是 `json.MarshalIndent` 怎么折行。
		if got := strings.Join(strings.Fields(string(decoded[key])), ""); got != want {
			t.Errorf("JSON[%q] 该是 %s，实得 %s", key, want, got)
		}
	}
	if keys := signinTestKeys(t, stdout); !sort.StringsAreSorted(keys) {
		t.Errorf("JSON 键该按字典序（与 Rust 的 serde_json 一致），实得 %v", keys)
	}

	// 本月一天都没签时两个列表字段必须是 `[]` 不是 `null`：nil 切片会被编成 null，
	// 而"这个月一条都没有"与"这个字段不存在"是两件事 —— Rust 那边给的是 `[]`。
	emptyView := signinView{
		account: signinTestAccount("19****01", "", "", true),
		mode:    signinModeStatus,
		status: signinTestStatus(false, func(s *upstream.SignInStatus) {
			s.Days = []upstream.SignInDay{{Day: 21, CurrentMonth: true, Today: true}}
		}),
	}
	stdout, _, err = signinTestRender(t, []signinView{emptyView}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	var empty map[string]json.RawMessage
	if uerr := json.Unmarshal([]byte(stdout), &empty); uerr != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", uerr, stdout)
	}
	for key, want := range map[string]string{
		"signedDays":       "[]",
		"receiveList":      "[]",
		"countedMonthDays": "0",
	} {
		if got := strings.Join(strings.Fields(string(empty[key])), ""); got != want {
			t.Errorf("空月份下 JSON[%q] 该是 %s，实得 %s", key, want, got)
		}
	}
}

// TestSigninJSONOmitsTheReportKeysWhenThereIsNoReport 钉住"没拿到报告就不给那三个键"。
//
// `signedNow` / `alreadySigned` / `beanBefore` 只有真的跑过签到才有意义 ——
// 只查状态时给一个 `false` 会让脚本以为"今天没签上"。
func TestSigninJSONOmitsTheReportKeysWhenThereIsNoReport(t *testing.T) {
	view := signinView{
		account: signinTestAccount("19****01", "", "", true),
		mode:    signinModeStatus,
		status:  signinTestStatus(true, nil),
	}
	stdout, _, err := signinTestRender(t, []signinView{view}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	var decoded map[string]json.RawMessage
	if uerr := json.Unmarshal([]byte(stdout), &decoded); uerr != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", uerr, stdout)
	}
	for _, key := range []string{"signedNow", "alreadySigned", "beanBefore"} {
		if _, ok := decoded[key]; ok {
			t.Errorf("只查状态时不该有 %q（那是报告才有的三个键）:\n%s", key, stdout)
		}
	}
	// `signedToday` 不是"报告才有的键"：它来自状态，必须一直在。
	if got := string(decoded["signedToday"]); got != "true" {
		t.Errorf("signedToday 该在，实得 %s", got)
	}
}

// TestSigninJSONSingleObjectVersusArray 钉住"单账号给对象、多账号给数组"，
// 以及"跳过的账号在 JSON 里没有条目"。
func TestSigninJSONSingleObjectVersusArray(t *testing.T) {
	signed := func(id string, index int) signinView {
		return signinView{
			account: signinTestAccount(id, "", "", true),
			index:   index,
			mode:    signinModeSignIn,
			report:  signinTestReport(false, true, &upstream.SignInResult{SignInPoints: 3}),
		}
	}

	stdout, _, err := signinTestRender(t, []signinView{signed("19****01", 1)}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if !strings.HasPrefix(strings.TrimSpace(stdout), "{") {
		t.Errorf("单账号该给一个对象（老脚本按对象读），实得:\n%s", stdout)
	}

	views := []signinView{
		signed("19****01", 1),
		signed("19****02", 2),
		{
			account: signinTestAccount("19****03", "", "", false),
			index:   3,
			mode:    signinModeSignIn,
			skipped: "账号已停用",
		},
	}
	stdout, stderr, err := signinTestRender(t, views, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if !strings.HasPrefix(strings.TrimSpace(stdout), "[") {
		t.Errorf("多账号该给一个数组，实得:\n%s", stdout)
	}
	var entries []map[string]any
	if uerr := json.Unmarshal([]byte(stdout), &entries); uerr != nil {
		t.Fatalf("stdout 该是一份 JSON: %v\n%s", uerr, stdout)
	}
	if len(entries) != 2 {
		t.Errorf("跳过的账号在 JSON 里没有条目（Rust 侧它根本不在 targets 里），该是 2 条，实得 %d:\n%s",
			len(entries), stdout)
	}
	if strings.Contains(stdout, "19****03") {
		t.Errorf("stdout 上只有 JSON，跳过的账号不该出现在里面:\n%s", stdout)
	}
	if !strings.Contains(stderr, "账号 3 (19****03) 跳过：账号已停用") {
		t.Errorf("`--json` 时跳过那句该退到 stderr，实得:\n%s", stderr)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 状态块
// ─────────────────────────────────────────────────────────────────────────────

// TestSigninStatusBlockKeepsTheTwoFiguresApart 钉住状态块的两个数：
// `beanTotal` 才是余额，`beforeTotal` 是那张冻结的快照 —— 相等时**不提**它。
func TestSigninStatusBlockKeepsTheTwoFiguresApart(t *testing.T) {
	render := func(mutate func(*upstream.SignInStatus)) string {
		t.Helper()
		stdout, _, err := signinTestRender(t, []signinView{{
			account: signinTestAccount("19****01", "", "", true),
			mode:    signinModeStatus,
			status:  signinTestStatus(true, mutate),
		}}, false)
		if err != nil {
			t.Fatalf("渲染失败: %v", err)
		}
		return stdout
	}

	block := render(nil)
	for _, want := range []string{
		"  今天：已签\n",
		"  豆余额：7431\n",
		"  连续签到：12 天\n",
		"  本月已签：3 天（服务端报 12）\n",
		"  本次可得：3 豆\n",
		"  自动备份：上游没给 `curMonthBackup`，判不了\n",
	} {
		if !strings.Contains(block, want) {
			t.Errorf("状态块该有 %q，实得:\n%s", want, block)
		}
	}

	// 相等 ⇒ 不提；不等 ⇒ 两个数都说出来。
	if got := render(func(s *upstream.SignInStatus) { s.CloudTotal = signinTestInt64Ptr(7431) }); strings.Contains(got, "另有一个数") {
		t.Errorf("两个口径相等时不该提那一行（天天打是纯噪音）:\n%s", got)
	}
	want := "  （另有一个数 5392：改名时结转进来的那笔，不是余额 —— 见 `--raw` 的 beforeTotal）\n"
	if got := render(func(s *upstream.SignInStatus) { s.CloudTotal = signinTestInt64Ptr(5392) }); !strings.Contains(got, want) {
		t.Errorf("两个口径不等时该把另一个也说出来，缺 %q:\n%s", want, got)
	}

	// 未签 + 有账目 + 备份三态。
	got := render(func(s *upstream.SignInStatus) {
		s.Days[2].Signed = false
		s.ReceiveList = []upstream.ReceiveItem{
			{CloudType: upstream.CloudTypeNextMonth, CloudNum: 240},
			{CloudType: upstream.CloudTypeNow, CloudNum: 6},
		}
		s.CurMonthBackup = signinTestBoolPtr(true)
		s.CurMonthBackupSignAccept = signinTestBoolPtr(false)
	})
	for _, want := range []string{
		"  今天：未签\n",
		"  可领取账目：下月可领取 240 豆（cloudType=2），可领取（立即生效） 6 豆（cloudType=0）\n",
		"  自动备份：本月已开启（翻倍机会还没领，可 `--multiple`）\n",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("状态块该有 %q，实得:\n%s", want, got)
		}
	}
}

// TestSigninStatusBlockSaysUnknownInsteadOfGuessing 钉住"服务端没标出今天"时不许猜。
func TestSigninStatusBlockSaysUnknownInsteadOfGuessing(t *testing.T) {
	stdout, _, err := signinTestRender(t, []signinView{{
		account: signinTestAccount("19****01", "", "", true),
		mode:    signinModeStatus,
		status: signinTestStatus(true, func(s *upstream.SignInStatus) {
			for i := range s.Days {
				s.Days[i].Today = false
			}
		}),
	}}, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if !strings.Contains(stdout, "  今天：未知（服务端没标出今天是哪一格）\n") {
		t.Errorf("没有格子标成「今天」时该说未知，实得:\n%s", stdout)
	}
	if strings.Contains(stdout, "今天：已签") || strings.Contains(stdout, "今天：未签") {
		t.Errorf("判不了的时候不能猜，实得:\n%s", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 进度行 / 分隔行 / 原始响应 / 汇总
// ─────────────────────────────────────────────────────────────────────────────

// TestSigninProgressLineNamesTheMode 钉住"正在做什么"那行的三种措辞与下标规则。
func TestSigninProgressLineNamesTheMode(t *testing.T) {
	account := signinTestAccount("19****01", "", "", true)
	cases := []struct {
		mode signinMode
		want string
	}{
		{signinModeSignIn, "账号 1 (19****01)：以 iOS App 身份签到"},
		{signinModeStatus, "账号 1 (19****01)：以 iOS App 身份查询签到状态"},
		{signinModeRaw, "账号 1 (19****01)：以 iOS App 身份取 infoV3 原始响应"},
	}
	for _, tc := range cases {
		if got := signinProgressLine(signinView{account: account, index: 1, mode: tc.mode}); got != tc.want {
			t.Errorf("mode=%d 的进度行应是 %q，实得 %q", tc.mode, tc.want, got)
		}
	}
	// `--phone` 定位的账号没有下标，打一个 0 出来只会让人以为它是第一个账号。
	if got := signinProgressLine(signinView{account: account, mode: signinModeSignIn}); got != "账号 19****01：以 iOS App 身份签到" {
		t.Errorf("没有下标时不该打下标，实得 %q", got)
	}
}

// TestSigninRawIsPassedThroughUnparsed 钉住 `--raw` 是"原样打出"。
func TestSigninRawIsPassedThroughUnparsed(t *testing.T) {
	const envelope = `{"code":0,"result":{"cal":[{"d":21,"s":true,"t":true}]}}`
	view := signinView{
		account: signinTestAccount("19****01", "", "", true),
		index:   1,
		mode:    signinModeRaw,
		raw:     json.RawMessage(envelope),
	}

	stdout, _, err := signinTestRender(t, []signinView{view}, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	// ⚠ 进度行**不在** stdout 里，也不在这个函数里打。
	//
	// 它的用处是"等待期间看得见进度"，所以必须在**发请求之前**由编排层用
	// `out.note`（stderr）打一次。渲染层是拿到结果之后才被调用的 ——
	// 在这里再打一遍就是同一个账号两行，而第二行落在结果**之后**，
	// 读起来像"它又要做一次"。
	//
	// ⚠ 断言写成"stdout 里没有它"而不是"stderr 里有它"：渲染层的入参里没有
	// stderr 通道，这个函数根本不该决定进度行去哪 —— 那是编排层的事。
	if strings.Contains(stdout, "以 iOS App 身份") {
		t.Errorf("进度行不该由渲染层打进 stdout（那是编排层的 stderr），实得:\n%s", stdout)
	}
	// 缩进版（`to_string_pretty` 那一档），不是紧凑的一整行。
	if !strings.Contains(stdout, "\n  \"code\": 0") {
		t.Errorf("`--raw` 该打缩进版，实得:\n%s", stdout)
	}
	if strings.Contains(stdout, "今天：") {
		t.Errorf("`--raw` 不解析，不该顺手打状态块，实得:\n%s", stdout)
	}

	stdout, _, err = signinTestRender(t, []signinView{view}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	var decoded map[string]any
	if uerr := json.Unmarshal([]byte(stdout), &decoded); uerr != nil {
		t.Fatalf("`--raw --json` 该是上游那份 JSON: %v\n%s", uerr, stdout)
	}
	if decoded["code"] != float64(0) {
		t.Errorf("`--raw --json` 该把上游那份原样嵌进去，实得:\n%s", stdout)
	}
}

// TestSigninBatchSeparatesMultiAccountRuns 钉住分隔行：多账号才有，`--json` 时走 stderr。
func TestSigninBatchSeparatesMultiAccountRuns(t *testing.T) {
	signed := func(id string, index int) signinView {
		return signinView{
			account: signinTestAccount(id, "", "", true),
			index:   index,
			mode:    signinModeSignIn,
			report:  signinTestReport(false, true, &upstream.SignInResult{SignInPoints: 3}),
		}
	}

	stdout, _, err := signinTestRender(t, []signinView{signed("19****01", 1)}, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if strings.Contains(stdout, "------") {
		t.Errorf("单账号不该有分隔行（Rust 侧 `multi = targets.len() > 1`）:\n%s", stdout)
	}

	stdout, _, err = signinTestRender(t, []signinView{signed("19****01", 1), signed("19****02", 2)}, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	for _, want := range []string{"------ 账号 1 (19****01) ------", "------ 账号 2 (19****02) ------"} {
		if !strings.Contains(stdout, want) {
			t.Errorf("多账号每段前该有 %q，实得:\n%s", want, stdout)
		}
	}

	jsonOut, stderr, err := signinTestRender(t, []signinView{signed("19****01", 1), signed("19****02", 2)}, true)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if strings.Contains(jsonOut, "------") {
		t.Errorf("`--json` 时分隔行不该进 stdout:\n%s", jsonOut)
	}
	if !strings.Contains(stderr, "------ 账号 1 (19****01) ------") {
		t.Errorf("`--json` 时分隔行该退到 stderr，实得:\n%s", stderr)
	}
	if !json.Valid([]byte(jsonOut)) {
		t.Errorf("stdout 该是一份干净的 JSON（否则 `| jq` 当场语法错）:\n%s", jsonOut)
	}
}

// TestSigninSummaryCountsEachState 钉住多账号的汇总。
//
// 两档的个数**刻意不对称**（2 : 1）：各一个的话，把两个计数写反的实现在这里
// 照样是绿的 —— 而那正是"看起来被断言了、其实没有"。
func TestSigninSummaryCountsEachState(t *testing.T) {
	views := []signinView{
		{
			account: signinTestAccount("19****01", "", "", true), index: 1, mode: signinModeSignIn,
			report: signinTestReport(false, true, &upstream.SignInResult{SignInPoints: 3}),
		},
		{
			account: signinTestAccount("19****02", "", "", true), index: 2, mode: signinModeSignIn,
			report: signinTestReport(false, true, &upstream.SignInResult{SignInPoints: 3}),
		},
		{
			account: signinTestAccount("19****03", "", "", true), index: 3, mode: signinModeSignIn,
			report: signinTestReport(true, true, nil),
		},
		{
			account: signinTestAccount("19****04", "", "", true), index: 4, mode: signinModeSignIn,
			failure: core.NewNetwork("connection reset"),
		},
		{
			account: signinTestAccount("19****05", "", "", false), index: 5, mode: signinModeSignIn,
			skipped: "账号已停用",
		},
	}

	summary := summarizeSignin(views)
	if summary.total != 5 || summary.signedNow != 2 || summary.alreadySigned != 1 ||
		summary.failed != 1 || summary.skipped != 1 || summary.unknown != 0 {
		t.Fatalf("汇总口径不对: %+v", summary)
	}

	stdout, _, err := signinTestRender(t, views, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	want := "共 5 个账号：签到成功 2 个，今天已签过 1 个，失败 1 个，跳过 1 个\n"
	if !strings.Contains(stdout, want) {
		t.Errorf("汇总行该是 %q，实得:\n%s", want, stdout)
	}

	// 单账号不打汇总（Rust 侧也没有这一行）。
	stdout, _, err = signinTestRender(t, views[:1], false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if strings.Contains(stdout, "个账号：") {
		t.Errorf("单账号不该打汇总:\n%s", stdout)
	}

	// `--status-only` 的批次不打签到汇总：那批没有"签没签上"这回事。
	statusOnly := []signinView{
		{account: signinTestAccount("19****01", "", "", true), index: 1, mode: signinModeStatus, status: signinTestStatus(true, nil)},
		{account: signinTestAccount("19****02", "", "", true), index: 2, mode: signinModeStatus, status: signinTestStatus(false, nil)},
	}
	stdout, _, err = signinTestRender(t, statusOnly, false)
	if err != nil {
		t.Fatalf("渲染失败: %v", err)
	}
	if strings.Contains(stdout, "个账号：") {
		t.Errorf("`--status-only` 不该报「签到成功 0 个」（那是把没发生的事说成结果）:\n%s", stdout)
	}
}
