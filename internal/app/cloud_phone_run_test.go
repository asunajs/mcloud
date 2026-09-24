package app

// 红包派对**只读那一半**的编排测试：三枪的顺序与判据、钱怎么念、接线有没有真的用上票池。
//
// # 这批用例钉的五件事
//
//  1. **顺序**是"余额 → 清单 → 播报"，而三枪共用**同一次**换票（两枪）；
//  2. 前两枪失败 ⇒ 整趟失败并**短路**（第三枪不打）；播报失败 ⇒ 只留 nil；
//  3. 钱的单位换算是**这一层**的事，而且"元/分"两句都留 —— 除错了要能被看见；
//  4. `nil`（上游没给）与 `0`（上游说没有）**念法不同**，混起来的表现是
//     用户以为余额被清零；
//  5. 结论句里**不许**出现上游那串带尾号的掩码手机号（仓库口径：掩码不带尾号）。
//
// 外加接线判据（`TestCloudPhoneRunTwoRoundsShareOneTicket`）：接了池 ⇒ 两轮只换一次票，
// 并且带一个"不接池"的对照 —— 对照不成立说明那个开关压根没走进分岔。

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// ─────────────────────────────────────────────────────────────────────────────
// 判据 1 + 2：顺序、短路、播报的宽容
// ─────────────────────────────────────────────────────────────────────────────

func TestCollectCloudPhoneAssetsRunsThreeReadsInOrder(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
		cloudPhoneBalanceOK("210", "370"), cloudPhoneTaskListOK(), cloudPhoneToastOK(),
	}}
	src := cloudPhoneSource(client, cloudPhoneTestCredential())
	assets, perr := CollectCloudPhoneAssets(context.Background(), src)
	if perr != nil {
		t.Fatalf("三枪都该成功：%s", perr.Message())
	}
	want := []string{
		"redpacket/userAccountInfo", "redpacket/configTaskLoginList", "redpacket/userToastInfo",
	}
	got := client.urls()[2:]
	if len(got) != len(want) {
		t.Fatalf("业务请求该有 3 枪，实得 %v", client.urls())
	}
	for i, url := range got {
		if !strings.Contains(url, want[i]) {
			t.Errorf("第 %d 枪该打 %q，实得 %s", i+1, want[i], url)
		}
	}
	// ⭐ 三枪共用**一次**换票：多出来的那一枪不是"省下来的"，是漏接票池的证据。
	if n := client.count("tokenValidate"); n != 1 {
		t.Errorf("三枪只该换一次票，实得 %d 次", n)
	}
	if assets.Balance.CanAmount == nil || *assets.Balance.CanAmount != 210 {
		t.Errorf("余额实得 %v", assets.Balance.CanAmount)
	}
	if len(assets.Tasks.Daily) != 3 || len(assets.Tasks.Novice) != 1 || len(assets.Tasks.Monthly) != 1 {
		t.Errorf("清单张数不对：daily=%d novice=%d monthly=%d",
			len(assets.Tasks.Daily), len(assets.Tasks.Novice), len(assets.Tasks.Monthly))
	}
	if assets.Toast == nil || assets.Toast.IsShowCloud == nil || *assets.Toast.IsShowCloud != 1 {
		t.Errorf("播报位该读到 1，实得 %+v", assets.Toast)
	}
}

// TestCollectCloudPhoneAssetsShortCircuitsOnFatalReads 钉住"前两枪任一失败就整趟失败，
// 且**不继续往下打**"。
//
// 播报那一枪排在最后正是为了这件事：它失败无所谓，所以不该因为它多花一次出网。
func TestCollectCloudPhoneAssetsShortCircuitsOnFatalReads(t *testing.T) {
	for _, tc := range []struct {
		name    string
		balance string
		tasks   string
	}{
		{"余额挂", cloudPhoneStatusBad, cloudPhoneTaskListOK()},
		{"清单挂", cloudPhoneBalanceOK("210", "370"), cloudPhoneStatusBad},
	} {
		t.Run(tc.name, func(t *testing.T) {
			client := &scriptedUpstream{responses: []string{
				ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"), tc.balance, tc.tasks, cloudPhoneToastOK(),
			}}
			src := cloudPhoneSource(client, cloudPhoneTestCredential())
			if _, perr := CollectCloudPhoneAssets(context.Background(), src); perr == nil {
				t.Fatal("前两枪失败时整趟该失败")
			}
			// ⭐ 短路的读数是"播报那一枪**没打**"，不是"总请求数少几枪"：
			// 清单挂时总数恰好也是 4（两枪换票 + 余额 + 清单），拿总数判会把它误成通过。
			if n := client.count("userToastInfo"); n != 0 {
				t.Errorf("前两枪失败之后不该继续打播报，实得 %d 次：%v", n, client.urls())
			}
		})
	}
}

// TestCollectCloudPhoneAssetsToleratesToastFailure：播报那一格读不懂，
// 所以**不**拿它判整趟失败（理由见 [CollectCloudPhoneAssets] 函数头第三条）。
func TestCollectCloudPhoneAssetsToleratesToastFailure(t *testing.T) {
	client := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"),
		cloudPhoneBalanceOK("210", "370"), cloudPhoneTaskListOK(), cloudPhoneStatusBad,
	}}
	src := cloudPhoneSource(client, cloudPhoneTestCredential())
	assets, perr := CollectCloudPhoneAssets(context.Background(), src)
	if perr != nil {
		t.Fatalf("播报失败不该判整趟失败：%s", perr.Message())
	}
	if assets.Toast != nil {
		t.Errorf("播报失败时那一格该留 nil（≠ 0），实得 %+v", assets.Toast)
	}
	// 前两枪的读数**不受影响**：这一条挡住的是"顺手把整个函数包成忽略错误"。
	if assets.Balance.CanAmount == nil || *assets.Balance.CanAmount != 210 {
		t.Errorf("余额读数被播报那一枪带坏了：%v", assets.Balance.CanAmount)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据 3 + 4：钱怎么念
// ─────────────────────────────────────────────────────────────────────────────

func TestCloudPhoneYuanKeepsBothUnits(t *testing.T) {
	fen := int64(210)
	if got := cloudPhoneYuan(&fen); got != "2.10 元（210 分）" {
		t.Errorf("两句都该留着，实得 %q", got)
	}
	// 0 是上游说了"确实没有" ⇒ 念成 0.00 元，**不是**"未知"。
	zero := int64(0)
	if got := cloudPhoneYuan(&zero); got != "0.00 元（0 分）" {
		t.Errorf("零该念成零，实得 %q", got)
	}
	if got := cloudPhoneYuan(nil); !strings.Contains(got, "未知") {
		t.Errorf("缺位该念成未知，实得 %q", got)
	}
}

// TestCloudPhoneSummaryLineDistinguishesUnknownFromZero 把上一条接进整句结论：
// 两个账号（一个有钱、一个上游没给）念出来必须能分辨。
func TestCloudPhoneSummaryLineDistinguishesUnknownFromZero(t *testing.T) {
	fen := int64(370)
	withValue := CloudPhoneResult{
		Status: CloudPhoneDone,
		Assets: CloudPhoneAssets{Balance: upstream.CloudPhoneAccountInfo{
			CanAmount: &fen, TotalAmount: &fen,
		}},
	}
	line := withValue.SummaryLine()
	if !strings.Contains(line, "3.70 元（370 分）") {
		t.Errorf("结论句里没有换算后的金额：%q", line)
	}
	empty := CloudPhoneResult{Status: CloudPhoneDone}
	if got := empty.SummaryLine(); !strings.Contains(got, "未知") {
		t.Errorf("上游没给余额时该念未知，实得 %q", got)
	}
	// ⚠ "未知"绝不能念成"0.00"：那是一句会让人以为余额被清零的假话。
	if got := empty.SummaryLine(); strings.Contains(got, "0.00") {
		t.Errorf("把「没给」念成了「没有」：%q", got)
	}
}

// TestCloudPhoneSummaryLineDoesNotEchoUpstreamMask ⭐ 仓库口径：成文与输出里的手机号
// 一律掩码且**不带尾号**，而上游那个 `account` 字段是"前三位 + 星号 + 后四位"。
//
// 机检抓不到掩码，所以这一条是人写的测试 —— 删掉它，下一次"顺手把账号带上"
// 就会把带尾号的号码送进 `runs.message` 与事件流。
func TestCloudPhoneSummaryLineDoesNotEchoUpstreamMask(t *testing.T) {
	masked := "199****0001"
	fen := int64(210)
	res := CloudPhoneResult{
		Status: CloudPhoneDone,
		Assets: CloudPhoneAssets{Balance: upstream.CloudPhoneAccountInfo{
			Account: &masked, CanAmount: &fen,
		}},
	}
	line := res.SummaryLine()
	if strings.Contains(line, masked) || strings.Contains(line, "0001") {
		t.Errorf("结论句里出现了带尾号的手机号：%q", line)
	}
}

// TestCloudPhoneSummaryLineOnFailureCarriesReason 钉住失败那一支：
// 空 `Error` 也要有一句人话（`runs.message` 不能是"红包派对没读成："）。
func TestCloudPhoneSummaryLineOnFailureCarriesReason(t *testing.T) {
	withReason := CloudPhoneResult{Status: CloudPhoneFailed, Error: "上游回 status=500"}
	if got := withReason.SummaryLine(); !strings.Contains(got, "上游回 status=500") {
		t.Errorf("失败原因该进结论句，实得 %q", got)
	}
	bare := CloudPhoneResult{Status: CloudPhoneFailed}
	if got := bare.SummaryLine(); strings.HasSuffix(got, "：") || got == "" {
		t.Errorf("没有原因时也该有完整一句，实得 %q", got)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 判据：未完成的计数口径（只数 0）
// ─────────────────────────────────────────────────────────────────────────────

// TestCloudPhonePendingTasksCountsOnlyZero ⭐ `2` 那一档成文写着"未定义"，
// 缺 `userStatus` 也不是 0 —— 把它们折进"未完成"就是替上游编状态。
//
// 将来接写请求时这一格会**直接决定要不要发请求**，所以它现在必须只有一种算法。
func TestCloudPhonePendingTasksCountsOnlyZero(t *testing.T) {
	if got := cloudPhonePendingTasks(cloudPhoneTaskListForStatusTest()); got != 2 {
		t.Errorf("夹具里只有两条 userStatus=0，实得 %d", got)
	}
	// 空清单 ⇒ 0 而不是"未知"：这一格数的是条数，没有就是没有。
	if got := cloudPhonePendingTasks(upstream.CloudPhoneTaskList{}); got != 0 {
		t.Errorf("空清单该数成 0，实得 %d", got)
	}
}

// cloudPhoneTaskListForStatusTest 手搭一份清单（不走解析），让"四档状态各一条"这件事
// 与本文件的响应夹具**解耦** —— 改响应形状不该悄悄改掉这条判据。
//
// 三张表各放几条，是为了钉住"跨表合计"这一句（漏了某一张表也是 2，看不出来）。
func cloudPhoneTaskListForStatusTest() upstream.CloudPhoneTaskList {
	zero, one, two := upstream.CloudPhoneUserStatus(0), upstream.CloudPhoneUserStatus(1), upstream.CloudPhoneUserStatus(2)
	item := func(code string, status *upstream.CloudPhoneUserStatus) upstream.CloudPhoneTaskItem {
		return upstream.CloudPhoneTaskItem{TaskCode: &code, UserStatus: status}
	}
	return upstream.CloudPhoneTaskList{
		Novice: []upstream.CloudPhoneTaskItem{
			item("NOVICE_1", &zero), item("NOVICE_2", &one), item("NOVICE_3", nil),
		},
		Daily:   []upstream.CloudPhoneTaskItem{item("DAILY_1", &two)},
		Monthly: []upstream.CloudPhoneTaskItem{item("MONTHLY_1", &zero), item("MONTHLY_2", &two)},
		Buy:     []upstream.CloudPhoneTaskItem{item("BUY_1", nil)},
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 结论映射
// ─────────────────────────────────────────────────────────────────────────────

func TestCloudPhoneRunReportMapsBothStatuses(t *testing.T) {
	done := CloudPhoneResult{Status: CloudPhoneDone}.RunReport()
	if done.Outcome != core.OutcomeSuccess {
		t.Errorf("只读成功该记成功，实得 %v", done.Outcome)
	}
	// ⚠ 非 nil 的空切片：Rust 侧 `Vec::new()` 序列化成 `[]`，nil 会变 `null` 把前端炸掉。
	if done.ManualSteps == nil || len(done.ManualSteps) != 0 {
		t.Errorf("只读不产 ManualStep，但那一栏该是空数组而不是 null，实得 %+v", done.ManualSteps)
	}
	if done.Summary == nil || !strings.Contains(*done.Summary, "红包派对") {
		t.Errorf("成功也该带一句结论，实得 %+v", done.Summary)
	}
	failed := CloudPhoneResult{Status: CloudPhoneFailed, Error: "换票失败"}.RunReport()
	if failed.Outcome != core.OutcomeFailed {
		t.Errorf("失败该记失败，实得 %v", failed.Outcome)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 接线：跑一个账号那一段（含票池与过程话）
// ─────────────────────────────────────────────────────────────────────────────

// cloudPhoneRunStore 只补这一条链会碰的那几个存储入口，其余仍走 [fakeStore]
// 的"没重写就 panic"。
//
// ⚠ 与签到那侧 [poolSignInStore] 的**关键不同**：这里 `GetSmidState` 默认**回一份绑好的**
// 设备特征 + `deviceId`。签到那条对 [DeviceID] 失败是"降级继续"，而这一族换票那一枪
// 硬需要 `deviceId`（[CloudPhoneSessionSource] 拿不到它就报错，且**不发请求**）⇒
// 照抄那份夹具会让整条链在"取 deviceId"这一步失败，而失败会被读成"票刷不动"。
//
// `smidErr` 非空时改回错误：那是"库里压根没这台设备的指纹"那一档（下面有一条用例专门钉它）。
type cloudPhoneRunStore struct {
	fakeStore
	smid    *core.SmidState
	smidErr error
}

func (s *cloudPhoneRunStore) GetSmidState(
	context.Context, core.Scope, core.AccountID,
) (*core.SmidState, error) {
	if s.smidErr != nil {
		return nil, s.smidErr
	}
	return s.smid, nil
}

// cloudPhoneRunScript 是一次运行要的全部响应：两枪换票 + 三枪只读。
func cloudPhoneRunScript() *liveTaskScript {
	return liveTaskScriptWith(
		liveTaskRoute{"/user/querySpecToken", []string{ssoOK("SSO-1")}},
		liveTaskRoute{"user/tokenValidate", []string{
			cloudPhoneTicketOK("ACT-1"), cloudPhoneTicketOK("ACT-2"),
		}},
		liveTaskRoute{"redpacket/userAccountInfo", []string{cloudPhoneBalanceOK("210", "370")}},
		liveTaskRoute{"redpacket/configTaskLoginList", []string{cloudPhoneTaskListOK()}},
		liveTaskRoute{"redpacket/userToastInfo", []string{cloudPhoneToastOK()}},
	)
}

func cloudPhoneRunWith(script core.HTTPClient, pool *CloudPhoneSessionPool,
	progress RunProgress) CloudPhoneRun {
	auth := core.SecretValue(wholeAuth)
	fingerprint := upstream.GenerateSmFingerprint(testRng())
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		// 夹具搭不起来 = 前提就没了，与"没重写的存储入口被调到"同一处置：直接响。
		panic("夹具造不出设备特征：" + perr.Message())
	}
	bound := core.NewSecretValue(text)
	device := core.NewSecretValue("SYNTH-DEVICE-199")
	store := &cloudPhoneRunStore{
		fakeStore: fakeStore{state: &core.AccountState{Authorization: &auth}},
		smid: &core.SmidState{
			AccountID:       core.NewAccountID(cloudPhoneTestAccountID),
			FingerprintJSON: &bound, DeviceID: &device,
		},
	}
	return CloudPhoneRun{CloudPhoneWorld: CloudPhoneWorld{
		Store:    store,
		Scope:    core.ScopeLocal(),
		Upstream: upstream.NewUpstream(script),
		Device:   upstream.NewIPadAppDevice("67F4C56A00000000000000000000315C", "8A708867DBC3"),
		Clock:    testClock(),
		Random:   testRng(),
		Sessions: pool,
		Progress: progress,
	}}
}

const cloudPhoneTestAccountID = "19****01"

// TestCloudPhoneRunTwoRoundsShareOneTicket ⭐ 界面上连着点两次"盘点"那一格。
//
// 与 `signin_session_pool_test.go` 同名那一条同一个判据、同一个对照：
// 漏接池**不会**报错，只会让换票那两枪翻倍。
func TestCloudPhoneRunTwoRoundsShareOneTicket(t *testing.T) {
	rounds := func(withPool bool) (int, []string) {
		script := cloudPhoneRunScript()
		var pool *CloudPhoneSessionPool
		if withPool {
			pool = NewCloudPhoneSessionPool()
		}
		run := cloudPhoneRunWith(script, pool, nil)
		for i := 1; i <= 2; i++ {
			res, perr := run.RunAccount(context.Background(), core.NewAccountID(cloudPhoneTestAccountID))
			if perr != nil {
				t.Fatalf("第 %d 轮盘点失败：%s", i, perr.Message())
			}
			if res.Status != CloudPhoneDone {
				t.Fatalf("第 %d 轮结论不对：%s", i, res.SummaryLine())
			}
		}
		if withPool && pool.Entries() != 1 {
			t.Errorf("两轮过后池里该只有 1 条，实得 %d", pool.Entries())
		}
		var tickets []string
		for _, request := range script.snapshot() {
			for _, pair := range request.Headers {
				// 换票那一枪带的是**空串**（旧 TS `getHeaders("")`，见
				// [OpenCloudPhoneSession]）⇒ 只数"真带着一张票出去"的那些请求。
				if strings.EqualFold(pair[0], "token") && pair[1] != "" {
					tickets = append(tickets, pair[1])
				}
			}
		}
		return script.count("tokenValidate"), tickets
	}

	built, tickets := rounds(true)
	// 反向改坏点：把 `cloudPhoneSessionSource` 那句换成 `NewCloudPhoneSessionSource` ⇒ 这里变 2。
	if built != 1 {
		t.Errorf("接了池 ⇒ 两轮只该换一次票，实得 %d 次", built)
	}
	// 两轮 × 三枪 = 六条业务请求，全部带**同一张**票。
	if len(tickets) != 6 {
		t.Fatalf("六条业务请求都该带票，实得 %d 条：%v", len(tickets), tickets)
	}
	for i, ticket := range tickets {
		if ticket != "ACT-1" {
			t.Fatalf("第 %d 条业务请求换了票：%s（接了池就不该有第二张）", i+1, ticket)
		}
	}

	built, tickets = rounds(false)
	if built != 2 {
		t.Errorf("没接池时每轮该各换一次票（对照不成立说明上面那个开关没走进分岔），实得 %d", built)
	}
	if len(tickets) != 6 || tickets[0] == tickets[5] {
		t.Errorf("没接池时第二轮该换一张票，实得 %v", tickets)
	}
}

// TestCloudPhoneRunEmitsProgressAndReport 钉住"给人看的那两句"真的从 RunAccount 出去了：
// 成功一句 Info、失败一句 Warn，且 [CloudPhoneResult.RunReport] 用的是同一句结论。
func TestCloudPhoneRunEmitsProgressAndReport(t *testing.T) {
	var seen []string
	run := cloudPhoneRunWith(cloudPhoneRunScript(), nil, func(level core.LogLevel, message string) {
		seen = append(seen, level.String()+"|"+message)
	})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(cloudPhoneTestAccountID))
	if perr != nil {
		t.Fatalf("不该失败：%s", perr.Message())
	}
	if len(seen) != 1 || !strings.Contains(seen[0], "红包派对可兑 2.10 元") {
		t.Errorf("成功该念一句 Info，实得 %v", seen)
	}
	report := res.RunReport()
	if report.Summary == nil || *report.Summary != res.SummaryLine() {
		t.Errorf("报告里那句与结论句不是同一份：%+v", report.Summary)
	}

	// 失败那一支：换票就没成功 ⇒ 一句 Warn，且**不**是 Info。
	broken := &scriptedUpstream{responses: []string{
		ssoOK("SSO-1"), cloudPhoneTicketOK("ACT-1"), cloudPhoneStatusBad,
	}}
	var failed []string
	badRun := cloudPhoneRunWith(broken, nil, func(level core.LogLevel, message string) {
		failed = append(failed, level.String()+"|"+message)
	})
	res, perr = badRun.RunAccount(context.Background(), core.NewAccountID(cloudPhoneTestAccountID))
	if perr == nil {
		t.Fatal("余额那一枪回非 200，这一趟该失败")
	}
	if len(failed) != 1 || !strings.HasPrefix(failed[0], "warn|") {
		t.Errorf("失败该念一句 Warn，实得 %v", failed)
	}
	if !strings.Contains(failed[0], "没读成") {
		t.Errorf("失败那句该说清没读成，实得 %q", failed[0])
	}
	if res.Status != CloudPhoneFailed {
		t.Errorf("结论档位不对：%v", res.Status)
	}
}

// TestCloudPhoneWorldSourceFailsBeforeIO 钉住"库里没这个角色登录过"这一类失败
// 在**出网之前**就报出来（两类失败的处置全靠那一句区分，见 `signin_world.go` 头注第三条）。
func TestCloudPhoneWorldSourceFailsBeforeIO(t *testing.T) {
	script := cloudPhoneRunScript()
	run := cloudPhoneRunWith(script, nil, nil)
	store := run.Store.(*cloudPhoneRunStore)
	store.state = nil // 库里查不到登录态

	if _, perr := run.RunAccount(context.Background(), core.NewAccountID("19****99")); perr == nil {
		t.Fatal("没有登录态该失败")
	}
	if len(script.snapshot()) != 0 {
		t.Errorf("这类失败不该打任何上游请求，实得 %d 条", len(script.snapshot()))
	}
}

// TestCloudPhoneDeviceIDFailureIsNotSilentlyIgnored 钉住 [DeviceID] 失败**不降级**：
// 这一族换票硬需要 `deviceId`，所以它不像签到那条那样"少它只是领不到"。
func TestCloudPhoneDeviceIDFailureIsNotSilentlyIgnored(t *testing.T) {
	script := cloudPhoneRunScript()
	run := cloudPhoneRunWith(script, nil, nil)
	store := run.Store.(*cloudPhoneRunStore)
	// 库里读不出设备特征（真库里对应"这一列是坏的 / 迁移前留下的空行"）。
	store.smidErr = errors.New("读设备特征失败（夹具）")

	if _, perr := run.RunAccount(context.Background(), core.NewAccountID(cloudPhoneTestAccountID)); perr == nil {
		t.Fatal("取不到 deviceId 时不该假装成功")
	}
	// 关键在"没打活动站那一枪"：换票之前就该失败，而不是带着空 deviceId 去撞上游。
	if n := script.count("tokenValidate"); n != 0 {
		t.Errorf("取 deviceId 失败时不该换票，实得换票 %d 次", n)
	}
}
