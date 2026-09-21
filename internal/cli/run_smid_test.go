package cli

import (
	"context"
	"path/filepath"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	mcloudio "github.com/catlair/mcloud/internal/io"
	"github.com/catlair/mcloud/internal/upstream"
)

// 本文件测 `smid show` / `reset` / `refresh` 三条叶子的**命令行那一层**：
// 挑账号、打什么、打多少、退几。
//
// 生命周期（什么时候生成 / 复用 / 作废）在 `app::smid`，
// 算法在 `upstream::smid` —— 那些各有自己的测试，这里**不**重复断言。
//
// 库用**真实的 SQLite**（`mcloudio.Open`）：本层唯一会出错的就是
// "读出来什么、写进去什么"，换成内存假存储等于把被测对象换掉。

// smidFixture 建一个带若干账号的临时库，返回库路径。
//
// 账号用脱敏手机号形状（`19****01`）—— 与真机上 `smid show` 里那一列同形，
// 于是"用户照着 show 手打 id"这条路径才被真的走到。
func smidFixture(t *testing.T, ids ...string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "state.db")
	store, err := mcloudio.Open(path)
	if err != nil {
		t.Fatalf("打开临时库失败: %v", err)
	}
	defer store.Close()

	ctx := context.Background()
	for _, id := range ids {
		account := core.Account{ID: core.NewAccountID(id), Enabled: true}
		if err := store.UpsertAccount(ctx, core.ScopeLocal(), &account); err != nil {
			t.Fatalf("写入账号 %s 失败: %v", id, err)
		}
	}
	return path
}

// smidBind 给某个账号绑一份特征（可选连带一个 deviceId）。
func smidBind(t *testing.T, dbPath, id string, withDeviceID bool) string {
	t.Helper()
	store, err := mcloudio.Open(dbPath)
	if err != nil {
		t.Fatalf("打开库失败: %v", err)
	}
	defer store.Close()

	ctx := context.Background()
	accountID := core.NewAccountID(id)
	fingerprint := upstream.GenerateSmFingerprint(&cliRng{next: 7})
	raw, perr := fingerprint.ToJSONString()
	if perr != nil {
		t.Fatalf("序列化特征失败: %v", perr)
	}
	if err := store.SetSmidFingerprint(ctx, core.ScopeLocal(), accountID, raw); err != nil {
		t.Fatalf("绑定特征失败: %v", err)
	}
	// deviceId 用 88 位合成串：真机那串是 `B` + 88 位，共 89 字符
	deviceID := "B" + strings.Repeat("A", 88)
	if withDeviceID {
		if err := store.SetSmidDeviceID(ctx, core.ScopeLocal(), accountID, deviceID); err != nil {
			t.Fatalf("写入 deviceId 失败: %v", err)
		}
	}
	return deviceID
}

// smidBindDeviceOnly 造一个**有 deviceId、但没绑特征**的账号。
//
// 库里真能出现这种行（只写过 deviceId 的那条 SQL 不碰 fingerprint 列），
// 而它正是"已绑定计数"最容易数错的那种：有 state 不等于绑了特征。
func smidBindDeviceOnly(t *testing.T, dbPath, id string) {
	t.Helper()
	store, err := mcloudio.Open(dbPath)
	if err != nil {
		t.Fatalf("打开库失败: %v", err)
	}
	defer store.Close()
	if err := store.SetSmidDeviceID(
		context.Background(), core.ScopeLocal(), core.NewAccountID(id),
		"B"+strings.Repeat("D", 88)); err != nil {
		t.Fatalf("写入 deviceId 失败: %v", err)
	}
}

// smidDeviceIDResponse 是风控服务给上报的回包（`code` 是**数字** 1100）。
func smidDeviceIDResponse() map[string]any {
	return map[string]any{
		"code":   1100,
		"detail": map[string]any{"deviceId": strings.Repeat("C", 87)},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// show
// ─────────────────────────────────────────────────────────────────────────────

func TestSmidShowWithNoAccountsSaysWhatToRunNext(t *testing.T) {
	db := smidFixture(t)
	stdout, stderr, code := runCLI(t, "", "smid", "show", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("没有账号是正常中间态，应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	// 要说清下一步 —— 否则用户会以为命令坏了
	if !strings.Contains(stdout, "mcloud login qr") {
		t.Errorf("应告诉用户先去登录: %q", stdout)
	}
}

// 输出**只给摘要**：deviceId 是风控服务给这台设备的标识，
// 完整值留在库里；终端上那一眼通常是被截图/被贴进 issue 的那一份。
func TestSmidShowMasksTheDeviceIDAndCountsOnlyBoundAccounts(t *testing.T) {
	db := smidFixture(t, "19****01", "19****02", "19****03")
	full := smidBind(t, db, "19****01", true)
	// 03 只有 deviceId、没绑特征 —— 有 state 不等于绑了特征
	smidBindDeviceOnly(t, db, "19****03")

	stdout, stderr, code := runCLI(t, "", "smid", "show", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if !strings.Contains(stdout, "19****01") || !strings.Contains(stdout, "19****02") {
		t.Errorf("应列出账号: %q", stdout)
	}
	if !strings.Contains(stdout, "fp#") {
		t.Errorf("已绑定的账号应给出特征摘要: %q", stdout)
	}
	if !strings.Contains(stdout, "未绑定") {
		t.Errorf("未绑定的账号应说明下一次会自动绑: %q", stdout)
	}
	if !strings.Contains(stdout, "共 3 个账号，已绑定设备特征 1 个") {
		t.Errorf("只该数**绑了特征**的（有 deviceId 不算）: %q", stdout)
	}
	// ⚠ 完整 deviceId 绝不能出现
	if strings.Contains(stdout, full) {
		t.Errorf("deviceId 的**完整值**不该进终端: %q", stdout)
	}
	if !strings.Contains(stdout, "B…") || !strings.Contains(stdout, "（89 字符）") {
		t.Errorf("deviceId 应是首尾掩码并给出长度: %q", stdout)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// reset / refresh
// ─────────────────────────────────────────────────────────────────────────────

// `--report` 是**可选**的：不传就不打网络 —— 那是"换 id"之外最常见的用法，
// 而白打一次上报会白白让风控服务看到一次设备变化。
func TestSmidResetRebindsWithoutTouchingTheNetwork(t *testing.T) {
	db := smidFixture(t, "19****01")
	old := smidBind(t, db, "19****01", true)
	http := &loginScript{}

	stdout, stderr, code := runCLIWithDeps(t, &Deps{
		Clock:  mcloudio.SystemClock{},
		Random: &cliRng{next: 11},
		HTTP:   http,
	}, "", "smid", "reset", "--account", "19****01", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 0 {
		t.Errorf("不传 --report 就不该打上游，实得 %d 次: %v", http.sent(), http.urls())
	}
	if !strings.Contains(stdout, "已换上新的设备特征") {
		t.Errorf("应报告换上了新特征: %q", stdout)
	}
	if !strings.Contains(stdout, "旧的 deviceId 同时作废") {
		t.Errorf("应说清旧 id 已作废（否则用户会以为还在用）: %q", stdout)
	}
	if !strings.Contains(stdout, "没有上报") {
		t.Errorf("应说明这次没上报: %q", stdout)
	}
	if strings.Contains(stdout, old) {
		t.Errorf("旧的完整 deviceId 不该被打印: %q", stdout)
	}
}

// 换出来的特征**必须**与旧的不同 —— 否则"换一台设备"这件事根本没发生，
// 而输出上完全看不出来（两行长得一样）。
func TestSmidResetActuallyReplacesTheFingerprint(t *testing.T) {
	db := smidFixture(t, "19****01")
	smidBind(t, db, "19****01", true)
	before := smidDigestOf(t, db, "19****01")

	_, stderr, code := runCLI(t, "", "smid", "reset", "--account", "19****01", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	after := smidDigestOf(t, db, "19****01")
	if before == "" || after == "" {
		t.Fatalf("两头都该有一份特征（before=%q after=%q）", before, after)
	}
	if before == after {
		t.Errorf("特征该换掉了，实得同一份 %q", after)
	}
}

func TestSmidResetWithReportUploadsOnce(t *testing.T) {
	db := smidFixture(t, "19****01")
	smidBind(t, db, "19****01", true)
	http := &loginScript{responses: []any{smidDeviceIDResponse()}}

	stdout, stderr, code := runCLIWithDeps(t, &Deps{
		Clock:  mcloudio.SystemClock{},
		Random: &cliRng{next: 13},
		HTTP:   http,
	}, "", "smid", "reset", "--account", "19****01", "--report", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 1 {
		t.Errorf("只该上报一次，实得 %d 次: %v", http.sent(), http.urls())
	}
	if !strings.Contains(stdout, "已上报，新的 deviceId：B…") {
		t.Errorf("应给出**掩码后**的新 deviceId: %q", stdout)
	}
}

func TestSmidRefreshUploadsAndStoresTheNewDeviceID(t *testing.T) {
	db := smidFixture(t, "19****01")
	smidBind(t, db, "19****01", false)
	http := &loginScript{responses: []any{smidDeviceIDResponse()}}

	stdout, stderr, code := runCLIWithDeps(t, &Deps{
		Clock:  mcloudio.SystemClock{},
		Random: &cliRng{next: 17},
		HTTP:   http,
	}, "", "smid", "refresh", "--account", "19****01", "--db", db)
	if code != ExitSuccess {
		t.Fatalf("应退 0，实得 %d\n  stderr=%q", code, stderr)
	}
	if http.sent() != 1 {
		t.Fatalf("只该上报一次，实得 %d 次: %v", http.sent(), http.urls())
	}
	if !strings.Contains(stdout, "已上报，新的 deviceId") {
		t.Errorf("应报告新 id: %q", stdout)
	}

	// 落库那一半：库里现在该有一个 `B` 开头的 id
	store, err := mcloudio.Open(db)
	if err != nil {
		t.Fatalf("打开库失败: %v", err)
	}
	defer store.Close()
	state, err := store.GetSmidState(context.Background(), core.ScopeLocal(), core.NewAccountID("19****01"))
	if err != nil {
		t.Fatalf("读状态失败: %v", err)
	}
	if state == nil || state.DeviceID == nil {
		t.Fatal("上报后 deviceId 该落库了")
	}
	if !strings.HasPrefix(state.DeviceID.Value(), "B") {
		t.Errorf("deviceId 该带 `B` 前缀，实得 %q", state.DeviceID.Value())
	}
}

// `--account` 打错一个字就会凭空给一个不存在的账号绑上特征 ——
// 那一行永远没人读得到，只会留在库里让人困惑。所以要报错，并把现有账号列出来。
func TestSmidRejectsAnUnknownAccountAndNamesTheKnownOnes(t *testing.T) {
	db := smidFixture(t, "19****01", "19****02")

	_, stderr, code := runCLI(t, "", "smid", "refresh", "--account", "19****99", "--db", db)
	if code != ExitRun {
		t.Fatalf("账号不存在是运行错误，应退 %d，实得 %d\n  stderr=%q", ExitRun, code, stderr)
	}
	if !strings.Contains(stderr, "没有账号 19****99") {
		t.Errorf("应点名是哪个账号不存在: %q", stderr)
	}
	if !strings.Contains(stderr, "19****01") || !strings.Contains(stderr, "19****02") {
		t.Errorf("应把库里现有的账号列出来（用户是照着 show 手打的）: %q", stderr)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 摘要那两个出口
// ─────────────────────────────────────────────────────────────────────────────

// `canvas` 是这份特征的**识别位**，绝不能整份进终端。
func TestFingerprintLineSummarizesWithoutLeakingTheCanvas(t *testing.T) {
	fingerprint := upstream.GenerateSmFingerprint(&cliRng{next: 5})
	raw, perr := fingerprint.ToJSONString()
	if perr != nil {
		t.Fatalf("序列化特征失败: %v", perr)
	}

	line := fingerprintLine(raw)
	if !strings.HasPrefix(line, "fp#") {
		t.Errorf("应以 fp# 摘要开头: %q", line)
	}
	if !strings.Contains(line, fingerprint.Platform) || !strings.Contains(line, "触屏") {
		t.Errorf("应给出能认出这台设备的那几个维度: %q", line)
	}
	if strings.Contains(line, fingerprint.Canvas) {
		t.Errorf("canvas 是识别位，不该整份进终端: %q", line)
	}
	if strings.Contains(line, raw) {
		t.Error("不该把整串特征打出来")
	}
}

// 库里那串读不出来时：给摘要 + 一句怎么修，**不**回吐原串。
func TestFingerprintLineSurvivesAnUnreadableStoredFingerprint(t *testing.T) {
	line := fingerprintLine("{ 这不是 JSON ")
	if !strings.Contains(line, "读不出来") {
		t.Errorf("应说清库里那串读不出来: %q", line)
	}
	if !strings.Contains(line, "smid reset") {
		t.Errorf("应告诉用户怎么修: %q", line)
	}
	if strings.Contains(line, "这不是 JSON") {
		t.Error("不该回吐库里那串（它就是这台设备的长相）")
	}
}

func TestMaskDeviceIDKeepsOnlyTheEnds(t *testing.T) {
	// 真机形状：`B` + 88 位 = 89 字符
	id := "B" + strings.Repeat("A", 88)
	masked := maskDeviceID(id)
	if !strings.HasPrefix(masked, "B…") {
		t.Errorf("应保留首字符: %q", masked)
	}
	if !strings.HasSuffix(masked, "AAAA（89 字符）") {
		t.Errorf("应保留末 4 位并给出长度: %q", masked)
	}
	if strings.Contains(masked, id) {
		t.Error("不该带出完整值")
	}
	// 短于 4 的也不能越界
	if got := maskDeviceID("BX"); got != "B…BX（2 字符）" {
		t.Errorf("短串不该越界，实得 %q", got)
	}
	if got := maskDeviceID(""); got != "（0 字符）" {
		t.Errorf("空串该有兜底，实得 %q", got)
	}
}

// smidDigestOf 取库里那份特征的 fp# 摘要（用于比对"换没换"）。
func smidDigestOf(t *testing.T, dbPath, id string) string {
	t.Helper()
	store, err := mcloudio.Open(dbPath)
	if err != nil {
		t.Fatalf("打开库失败: %v", err)
	}
	defer store.Close()
	state, err := store.GetSmidState(context.Background(), core.ScopeLocal(), core.NewAccountID(id))
	if err != nil {
		t.Fatalf("读状态失败: %v", err)
	}
	if state == nil || state.FingerprintJSON == nil {
		return ""
	}
	line := fingerprintLine(state.FingerprintJSON.Value())
	const prefix = "fp#"
	if len(line) < len(prefix)+8 {
		return ""
	}
	return line[len(prefix) : len(prefix)+8]
}
