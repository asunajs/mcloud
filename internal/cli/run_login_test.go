package cli

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/app"
	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 本文件测 `login qr` / `login sms` 两条叶子的**命令行那一层**：
// 打了几次请求、终端上印了什么、退几。
//
// 协议与状态机在 `capabilities::login`，驱动在 `app::login` / `app::app_login`
// —— 那些各有自己的测试，这里**不**重复断言。
//
// 号码一律是 199 号段的合成号，凭据是 `SYNTH…` 合成串。

// ─────────────────────────────────────────────────────────────────────────────
// 假上游与运行器
// ─────────────────────────────────────────────────────────────────────────────

// loginScript 逐次返回脚本响应，并记下发出去的请求。
//
// 队列**空了就报错**：那代表"这次不该打上游"，让"有码就跳过发码"
// 与"超时后不再打扰上游"这类断言真能变红。
type loginScript struct {
	responses []any
	requests  []core.HTTPRequest
}

func (c *loginScript) Send(_ context.Context, request core.HTTPRequest) (core.HTTPResponse, error) {
	c.requests = append(c.requests, request)
	if len(c.responses) == 0 {
		return core.HTTPResponse{}, core.NewInternal("脚本已用完：这次不该再打上游")
	}
	body, err := core.MarshalJSON(c.responses[0])
	if err != nil {
		return core.HTTPResponse{}, err
	}
	c.responses = c.responses[1:]
	return core.NewResponse(200, body), nil
}

func (c *loginScript) sent() int { return len(c.requests) }

func (c *loginScript) urls() []string {
	out := make([]string, 0, len(c.requests))
	for _, r := range c.requests {
		out = append(out, r.URL)
	}
	return out
}

// cliRng 是确定性随机源（设备指纹与 tid 都要可复现）。
type cliRng struct{ next byte }

func (r *cliRng) FillBytes(dst []byte) {
	for i := range dst {
		dst[i] = r.next
		r.next++
	}
}

func cliNow() time.Time { return time.Date(2026, 9, 18, 0, 0, 0, 0, time.UTC) }

// runLoginCLI 跑一次完整命令行：真实解析、真实分发，
// 只有流 / 时钟 / 随机 / HTTP / 等待是假的。
//
// ⚠ 等待也要假：轮询那条循环一旦真睡，每条"轮询到成功"的用例都要等好几个真实
// 间隔。方案 §6.1 #72 说的就是这件事 —— 等待在 Go 里没有虚拟化定时器，
// 所以只能做成注入项。
func runLoginCLI(
	t *testing.T, http core.HTTPClient, stdin string, args ...string,
) (stdout, stderr string, code int) {
	t.Helper()
	return runCLIWithDeps(t, &Deps{
		Clock:  core.NewFixedClock(cliNow()),
		Random: &cliRng{next: 3},
		HTTP:   http,
	}, stdin, args...)
}

// loginTempPaths 给出一次运行要用的三个临时路径。
//
// 默认路径（`data/device.txt` 之类）会让测试**往仓库里写文件** ——
// 那不只是脏，它还会让"设备指纹已存在"成为跨用例的隐藏状态。
func loginTempPaths(t *testing.T) (db, deviceFile, appDeviceFile string) {
	t.Helper()
	dir := t.TempDir()
	return filepath.Join(dir, "state.db"),
		filepath.Join(dir, "device.txt"),
		filepath.Join(dir, "app-device.txt")
}

// ─────────────────────────────────────────────────────────────────────────────
// 夹具
// ─────────────────────────────────────────────────────────────────────────────

// cliUserInfo 是登录成功时上游回的那份会话（合成值）。
func cliUserInfo() map[string]any {
	return map[string]any{
		"account":   "19900000000",
		"authToken": "SYNTH001|1|RCS|1893456000000|AAAA",
		"userId":    "1990000000000000001",
	}
}

// cliAuthToken 是上面那份凭据里**绝不能出现在输出里**的那一段。
const cliAuthToken = "SYNTH001|1|RCS|1893456000000|AAAA"

// pcSuccessEnvelope 用**生产密钥**把会话封成 `data` 密文。
//
// ⚠ 必须走真实加密：CLI 这一层用的是 [upstream.NewLoginDataDecryptorProduction]，
// 没有"换个解密器"的缝 —— 于是夹具只能给出**真的**密文。
// 顺带这也让"密钥不对"这类回归在 CLI 层同样可见。
func pcSuccessEnvelope(t *testing.T) map[string]any {
	t.Helper()
	plain, err := core.MarshalJSON(cliUserInfo())
	if err != nil {
		t.Fatalf("序列化会话失败: %v", err)
	}
	wire, encErr := core.AESECBPKCS7EncryptHex(upstream.LoginAESKey, string(plain))
	if encErr != nil {
		t.Fatalf("加密会话失败: %v", encErr)
	}
	return map[string]any{"success": true, "code": "0000", "data": wire}
}

// appSuccessEnvelope 同 [pcSuccessEnvelope]，走 App 端那把钥匙（含末 10 位错位）。
func appSuccessEnvelope(t *testing.T) map[string]any {
	t.Helper()
	crypto := upstream.NewAppLoginCryptoProduction()
	plain, err := core.MarshalJSON(cliUserInfo())
	if err != nil {
		t.Fatalf("序列化会话失败: %v", err)
	}
	wire, perr := crypto.EncryptBody(string(plain))
	if perr != nil {
		t.Fatalf("加密会话失败: %v", perr)
	}
	return map[string]any{"success": true, "code": "0000", "data": wire}
}

func cliPuzzleEnvelope() map[string]any {
	return map[string]any{
		"success": true, "code": "0000",
		"data": map[string]any{
			"puzzle":      "iVBORw0KGgoAAAANSUhEUgAA",
			"puzzleWidth": "96",
			"picture":     "/9j/4AAQSkZJRgABAgAAAQAB",
			"picWidth":    "680",
			"picHeight":   "400",
			"picType":     "png",
			"puzzleLeft":  "OOBDgzP07ivqAWF0QQumGDO16EgXdAOr+1Hrqp/xJIg=",
		},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 扫码
// ─────────────────────────────────────────────────────────────────────────────

// `--no-wait` 只出码：**一个请求都不发**。
//
// 出码是本地拼字符串（抓包推翻的直觉假设里最重要的一个），
// 所以"只出码"在协议上等同于"不联网" —— 这条断言就是把它钉住。
func TestLoginQRWithNoWaitPrintsTheCodeAndSendsNothing(t *testing.T) {
	db, deviceFile, _ := loginTempPaths(t)
	http := &loginScript{}

	stdout, _, code := runLoginCLI(t, http, "",
		"login", "qr", "--no-wait", "--db", db, "--device-file", deviceFile)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}
	if http.sent() != 0 {
		t.Errorf("只出码不该打上游，实得 %d 次: %v", http.sent(), http.urls())
	}
	// 码里带着会话 id，且链接也打出来了（终端字体糊掉时的兜底）
	if !strings.Contains(stdout, "sID=") {
		t.Errorf("二维码载荷应带 sID: %q", stdout)
	}
	if !strings.Contains(stdout, "设备指纹") {
		t.Errorf("应提示设备指纹（换掉它会话就定位不到）: %q", stdout)
	}
}

func TestLoginQRWalksToSuccessAndPersistsTheAccount(t *testing.T) {
	db, deviceFile, _ := loginTempPaths(t)
	http := &loginScript{responses: []any{
		map[string]any{"success": false, "code": "200059541"},
		map[string]any{"success": false, "code": "200059548"},
		pcSuccessEnvelope(t),
	}}

	stdout, stderr, code := runLoginCLI(t, http, "",
		"login", "qr", "--db", db, "--device-file", deviceFile)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 3 {
		t.Errorf("应轮询到成功，实得 %d 次: %v", http.sent(), http.urls())
	}
	if !strings.Contains(stdout, "登录成功") {
		t.Errorf("应打印登录成功: %q", stdout)
	}
	if !strings.Contains(stdout, "19****00") {
		t.Errorf("应打印账号掩码: %q", stdout)
	}
	// 已扫码要给人一句进度
	if !strings.Contains(stdout, "已扫码，请在手机上确认登录") {
		t.Errorf("应打印中间态: %q", stdout)
	}
	// 字段名要打（上游的字段集合与文档不一致过），但**值**绝不能打
	if !strings.Contains(stdout, "登录返回的字段：account, authToken, userId") {
		t.Errorf("应按字典序列出字段名: %q", stdout)
	}
	if strings.Contains(stdout, cliAuthToken) || strings.Contains(stderr, cliAuthToken) {
		t.Error("凭据绝不能出现在任何输出里")
	}
}

// `--ttl-seconds 0` 让码当场过期：不该打扰上游，且要退运行错误。
//
// 用 TTL 造超时而不是真的等：状态机那边的超时语义已有自己的测试，
// 这里要验的是"CLI 把那个错误变成退 1 而不是别的"。
func TestLoginQRSurfacesAnExpiredCodeAsARunError(t *testing.T) {
	db, deviceFile, _ := loginTempPaths(t)
	http := &loginScript{}

	_, stderr, code := runLoginCLI(t, http, "",
		"login", "qr", "--db", db, "--device-file", deviceFile, "--ttl-seconds", "0")
	if code != ExitRun {
		t.Fatalf("超时是运行错误，应退 %d，实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if http.sent() != 0 {
		t.Errorf("超时不该再打扰上游，实得 %d 次", http.sent())
	}
	if !strings.Contains(stderr, "超时") {
		t.Errorf("错误信息该说清是超时: %q", stderr)
	}
}

// `--qr-out` 额外落一张图片 —— 它比终端字符画可靠，
// 而测试要验的是"真的落盘了、且是那张图"，不是"没报错"。
func TestLoginQROutWritesAnSVGFile(t *testing.T) {
	db, deviceFile, _ := loginTempPaths(t)
	qrOut := filepath.Join(t.TempDir(), "nested", "qr.svg")
	http := &loginScript{}

	stdout, _, code := runLoginCLI(t, http, "",
		"login", "qr", "--no-wait", "--db", db, "--device-file", deviceFile,
		"--qr-out", qrOut)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}
	raw, err := os.ReadFile(qrOut)
	if err != nil {
		t.Fatalf("二维码图片应已写入: %v", err)
	}
	svg := string(raw)
	if !strings.HasPrefix(svg, "<svg xmlns=\"http://www.w3.org/2000/svg\"") {
		t.Errorf("应是 SVG: %q", svg[:min(len(svg), 60)])
	}
	if !strings.Contains(stdout, qrOut) {
		t.Errorf("应告诉用户图片在哪: %q", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 短信（PC）
// ─────────────────────────────────────────────────────────────────────────────

func TestLoginSmsRequiresThePhoneNumber(t *testing.T) {
	http := &loginScript{}
	stdout, stderr, code := runLoginCLI(t, http, "", "login", "sms")
	if code != ExitUsage {
		t.Fatalf("缺 --phone 是用法错误，应退 %d，实得 %d\n  stderr=%q", ExitUsage, code, stderr)
	}
	if stdout != "" {
		t.Errorf("用法错误不该往 stdout 写东西: %q", stdout)
	}
}

// `--code` 已到手时**跳过发码**：再发一次会白刷今日配额、还要连带过一次滑块。
func TestLoginSmsSkipsSendingWhenTheCodeIsAlreadyGiven(t *testing.T) {
	db, deviceFile, _ := loginTempPaths(t)
	http := &loginScript{responses: []any{pcSuccessEnvelope(t)}}

	stdout, _, code := runLoginCLI(t, http, "",
		"login", "sms", "--client", "pc", "--phone", "19900000000", "--code", "119301",
		"--db", db, "--device-file", deviceFile)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}
	if http.sent() != 1 {
		t.Errorf("有码时只该发登录那一次，实得 %d 次: %v", http.sent(), http.urls())
	}
	if strings.Contains(stdout, "验证码已发送") {
		t.Errorf("不该再发一次码: %q", stdout)
	}
	if !strings.Contains(stdout, "登录成功") {
		t.Errorf("应登录成功: %q", stdout)
	}
}

// 不给码时：发码 → 从 stdin 读码 → 登录，共两次请求。
func TestLoginSmsReadsTheCodeFromStdinAndLogsIn(t *testing.T) {
	db, deviceFile, _ := loginTempPaths(t)
	http := &loginScript{responses: []any{
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 8}},
		pcSuccessEnvelope(t),
	}}

	stdout, stderr, code := runLoginCLI(t, http, "119301\n",
		"login", "sms", "--client", "pc", "--phone", "19900000000",
		"--db", db, "--device-file", deviceFile)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 2 {
		t.Fatalf("应是 发码 → 登录，实得 %d 次: %v", http.sent(), http.urls())
	}
	if !strings.HasSuffix(http.urls()[0], "/user/sms/getSmsCode") ||
		!strings.HasSuffix(http.urls()[1], "/user/thirdlogin") {
		t.Errorf("请求顺序不对: %v", http.urls())
	}
	if !strings.Contains(stdout, "今日剩余 8 次") {
		t.Errorf("应报配额余量: %q", stdout)
	}
	if !strings.Contains(stdout, "登录成功") {
		t.Errorf("应登录成功: %q", stdout)
	}
	if strings.Contains(stdout, cliAuthToken) || strings.Contains(stderr, cliAuthToken) {
		t.Error("凭据绝不能出现在任何输出里")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 短信（App）
// ─────────────────────────────────────────────────────────────────────────────

// 自动过滑块也用尽 → 交出 H5 地址并**停下**：不再问验证码，也不退错误码。
//
// 退 0 是刻意的：用户接下来要去浏览器里拖那一下，那不是失败。
// 退 1 会让人以为命令没跑成，于是重新执行 —— 而重新执行会再要一次滑块、把配额刷掉。
func TestLoginSmsAppFallsBackToTheH5URLAndStops(t *testing.T) {
	db, _, appDeviceFile := loginTempPaths(t)
	http := &loginScript{responses: []any{
		map[string]any{"code": "200059554", "data": map[string]any{"captchaType": 2}},
		cliPuzzleEnvelope(),
		map[string]any{"code": "200059553", "data": map[string]any{"captchaType": 1}},
		cliPuzzleEnvelope(),
		map[string]any{"code": "200059553", "data": map[string]any{"captchaType": 1}},
	}}

	stdout, stderr, code := runLoginCLI(t, http, "",
		"login", "sms", "--client", "app", "--phone", "19900000000",
		"--db", db, "--app-device-file", appDeviceFile)
	if code != ExitSuccess {
		t.Fatalf("交出兜底地址不是失败，应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "https://yun.139.com/fe/captcha/?data=") {
		t.Errorf("应给出 H5 地址: %q", stdout)
	}
	if !strings.Contains(stdout, "不要再重新执行本命令") {
		t.Errorf("要明确告诉用户别重跑: %q", stdout)
	}
	if strings.Contains(stderr, smsCodePrompt) {
		t.Error("码没发出来就不该再问用户要验证码")
	}
}

// `--send-only` 只发码就退出，并把当次的设备短标识打出来
// —— 下一步用 `--device-short` 传回去，两半才会被上游看成同一台设备形态。
func TestLoginSmsAppWithSendOnlyPrintsTheDeviceShort(t *testing.T) {
	db, _, appDeviceFile := loginTempPaths(t)
	http := &loginScript{responses: []any{
		map[string]any{"success": true, "code": "0000", "data": map[string]any{"remainGetTimes": 5}},
	}}

	stdout, stderr, code := runLoginCLI(t, http, "",
		"login", "sms", "--client", "app", "--phone", "19900000000", "--send-only",
		"--db", db, "--app-device-file", appDeviceFile)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 1 {
		t.Errorf("只该发码一次，实得 %d 次: %v", http.sent(), http.urls())
	}
	if !strings.Contains(stdout, "本次设备短标识") {
		t.Errorf("应把设备短标识打出来供下一步传回: %q", stdout)
	}
	if !strings.Contains(stdout, "发码完成") {
		t.Errorf("应说明已按 --send-only 退出: %q", stdout)
	}
	if strings.Contains(stderr, smsCodePrompt) {
		t.Error("--send-only 不该进入输验证码这一步")
	}
}

func TestLoginSmsAppLogsInWithTheCode(t *testing.T) {
	db, _, appDeviceFile := loginTempPaths(t)
	http := &loginScript{responses: []any{appSuccessEnvelope(t)}}

	stdout, _, code := runLoginCLI(t, http, "",
		"login", "sms", "--client", "app", "--phone", "19900000000", "--code", "119301",
		"--db", db, "--app-device-file", appDeviceFile)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d", code)
	}
	if http.sent() != 1 {
		t.Fatalf("有码时只该发登录那一次，实得 %d: %v", http.sent(), http.urls())
	}
	if !strings.HasSuffix(http.urls()[0], "/user/thirdlogin") {
		t.Errorf("应打 thirdlogin: %v", http.urls())
	}
	if !strings.Contains(stdout, "登录成功") {
		t.Errorf("应登录成功: %q", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 纯函数那一角
// ─────────────────────────────────────────────────────────────────────────────

// 字段清单要**排序**：上游字段集合会变，不排序时"少了一个字段"
// 会表现成整行重排，人工比对看不出来。
func TestLoginInfoFieldsAreSorted(t *testing.T) {
	got := loginInfoFields(map[string]any{"userId": 1, "account": "a", "authToken": "t"})
	want := "account, authToken, userId"
	if strings.Join(got, ", ") != want {
		t.Errorf("应排序，实得 %v 期望 %s", got, want)
	}
	if loginInfoFields(nil) != nil {
		t.Error("不是对象时该返回空清单，而不是 panic")
	}
}

// 两条端共用同一套"发完读一行"的约定 —— 这条钉住 `--code` 优先于发码，
// 以及"只发码"时不再往下读 stdin。
func TestObtainSmsCodePrefersTheGivenCode(t *testing.T) {
	called := false
	code, stopped, err := obtainSmsCode(
		&Deps{Stdin: strings.NewReader("")}, &output{},
		&SmsArgs{Code: "  119301  "}, false,
		func() (app.SmsCodeSent, bool, *core.Error) {
			called = true
			return app.SmsCodeSent{}, false, nil
		})
	if err != nil {
		t.Fatalf("不该出错: %v", err)
	}
	if called {
		t.Error("码已给就不该再发码")
	}
	if code != "119301" {
		t.Errorf("应去掉首尾空白，实得 %q", code)
	}
	if stopped {
		t.Error("有码时不该停下")
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
