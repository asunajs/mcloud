package app

// 任务拓展（`task-expansion`）编排层的用例。
//
// # 这一层钉的是哪三件事
//
//  1. **写侧的门**：`curMonthBackup` 没读到 false ⇒ 一枪上传都不发；
//     `curMonthTaskRecordCount` 没读到 >0 ⇒ 一发领取都不发。
//     这两条链上各有一次**不可回滚的写**（前者在用户网盘里留一个文件，
//     后者改的是账），而"把'上游没说'读成'上游说了否'"编译得过、只在这里响。
//  2. **顺序与次数**：凑备份 ⇒ 传（三枪）→ 等 → **重读** → 用**重读那一份**的
//     `acceptDate` 去领。"传完不重读就去领"是最容易写顺手的版本，而它拿的是**旧**判据。
//     上限"至多传一个文件"也在这一组里（[taskExpansionRetries]）。
//  3. **实况不被抹掉**：上传过了就算后面挂了也要念出来（"刻意差别"第 3 条）、
//     领过了就算上游没报数也要念"已领"（第 2 条）、一枪没发就不空等（第 1 条）。
//
// # ⚠ 这里**没有**在验证上游
//
// 夹具响应除一条外**全是合成的**：读口有实拍（`teCaptured` 是
// `test/fixtures/http/3d.json` 里那两条的复刻），而"凑备份那一支""领取那一支"
// **本仓零实拍**（见 `task_expansion_run.go` 文件头"证据面"）。所以这些用例的
// 判据是**本层自己定的规矩**，不是"上游会这样回"。真跑那一趟才算后者。

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// 这一族自己的两个端点（上传那三枪复用 `task_matrix_run_test.go` 里那一份常量）。
const (
	teReadEndpoint  = "/ycloud/signin/page/taskExpansion"
	teClaimEndpoint = "/ycloud/signin/page/receiveTaskExpansion"
)

// teAcceptFirst / teAcceptSecond 是两份读数各自的 `acceptDate`。
//
// ⚠ 两条**必须不同**且都不参与出网以外的地方：它们的存在就是为了抓
// "领的是哪一份的 acceptDate"（[TaskExpansionRun.RunAccount] 用的是**结论那一次**）。
const (
	teAcceptFirst  = "SYNTH-ACCEPT-FIRST"
	teAcceptSecond = "SYNTH-ACCEPT-SECOND"
)

// teCaptured 是**实拍**那一份读数的复刻（夹具两条同形，且都没有 `acceptDate`）。
//
// 逐字照抄，不为测试方便改任何一个值：这一条用例的用处就是"实拍那一趟走的是
// 跳过凑备份 + 不去领那两支"，把 `curMonthBackup` 改成 false 它就变成另一回事了。
const teCaptured = `{"code":0,"msg":"success","result":{` +
	`"curMonthBackup":true,"curMonthBackupTaskAccept":false,` +
	`"curMonthTaskRecordCount":0,"nextMonthTaskRecordCount":358,"preMonthBackup":false}}`

// teClaimOK 是领取那一发的响应。**合成**（本仓零实拍）：朵数给不给都不知道，
// 所以"没给"那一档另有自己的用例，不靠这一条兜。
func teClaimOK(count int64) string {
	return `{"code":0,"msg":"success","result":{"cloudCount":` + itoa64(count) + `}}`
}

// teRead 造一份读数；参数为 nil ⇒ **响应里根本没有那个键**。
//
// 这是本文件最重要的一枚构造器：三条判据的分叉全在"缺"与"给了某个值"之间，
// 用 `false`/`0` 代替"缺"会让"把缺当否"那类写法**测不出来**。
func teRead(backup *bool, count, next *int64, accept *string) string {
	fields := []string{}
	if backup != nil {
		fields = append(fields, fmt.Sprintf(`"curMonthBackup":%t`, *backup))
	}
	if count != nil {
		fields = append(fields, `"curMonthTaskRecordCount":`+itoa64(*count))
	}
	if next != nil {
		fields = append(fields, `"nextMonthTaskRecordCount":`+itoa64(*next))
	}
	if accept != nil {
		fields = append(fields, `"acceptDate":"`+*accept+`"`)
	}
	return `{"code":0,"msg":"success","result":{` + strings.Join(fields, ",") + `}}`
}

func teBool(v bool) *bool { return &v }

func teInt(v int64) *int64 { return &v }

func teStr(v string) *string { return &v }

// ─────────────────────────────────────────────────────────────────────────────
// 夹具：带 deviceId 的库 + 记录型 waiter + 过程话
// ─────────────────────────────────────────────────────────────────────────────

// taskExpansionStore 默认给一份**已经绑好**的设备特征 + `deviceId`。
//
// 与 [poolSignInStore] 的关键差别：那一份 `GetSmidState` 恒回错误，而这一条链
// 缺 deviceId 时上传那一支会整段被挡（[TaskExpansionUploadBlocked]）⇒ 照抄它会让
// "传 → 等 → 重读 → 领"那几条主用例全停在挡下的那一格。
// 需要"换不到 deviceId"时另用 [taskExpansionStoreNoDevice]。
type taskExpansionStore struct {
	*poolSignInStore
	smid    *core.SmidState
	smidErr error
}

func (s *taskExpansionStore) GetSmidState(
	context.Context, core.Scope, core.AccountID,
) (*core.SmidState, error) {
	if s.smidErr != nil {
		return nil, s.smidErr
	}
	return s.smid, nil
}

func taskExpansionStoreBound() *taskExpansionStore {
	fingerprint := upstream.GenerateSmFingerprint(testRng())
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		panic("夹具造不出设备特征：" + perr.Message())
	}
	bound := core.NewSecretValue(text)
	device := core.NewSecretValue("SYNTH-DEVICE-199")
	return &taskExpansionStore{
		poolSignInStore: &poolSignInStore{*liveTaskStore()},
		smid: &core.SmidState{
			AccountID: core.NewAccountID(liveTestAccountID),
			// ⚠ 账号标识与 deviceId 都**不**出网：这一条路径是"库里有就直接返回"。
			FingerprintJSON: &bound, DeviceID: &device,
		},
	}
}

func taskExpansionStoreNoDevice() *taskExpansionStore {
	base := taskExpansionStoreBound()
	return &taskExpansionStore{
		poolSignInStore: base.poolSignInStore,
		smidErr:         errors.New("本用例换不到 deviceId"),
	}
}

// taskExpansionHarness 是一次运行的四件配套：跑的东西、响应脚本、等待记录、过程话。
type taskExpansionHarness struct {
	run    TaskExpansionRun
	script *liveTaskScript
	wait   *recordingWaiter
	events *liveTaskProgress
}

// teWait 是这一组用例里的"等一会儿"。取一个**既不是零也不是 20 秒**的值：
// 零会让"忘了等"与"等的是默认值"混在一起，20 秒会让断言看不出注入生效。
const teWait = 7 * time.Second

func taskExpansionFixture(store core.StateStore, routes ...liveTaskRoute) taskExpansionHarness {
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	events := &liveTaskProgress{}
	wait := &recordingWaiter{}
	world := signInWorldFor(store, script)
	world.Progress = events.handle()
	return taskExpansionHarness{
		run: TaskExpansionRun{
			SignInWorld: world, Wait: wait, BackupWait: teWait,
		},
		script: script, wait: wait, events: events,
	}
}

// teURLs 按发出顺序列出这一趟的全部 URL（"顺序"类断言用得上）。
func teURLs(script *liveTaskScript) []string {
	out := []string{}
	for _, request := range script.snapshot() {
		out = append(out, request.URL)
	}
	return out
}

// teHeaderCount 数某一枚 header 出现几次。
//
// 为什么数**次数**而不只看值：备份档要把 base 那枚 `x-yun-op-type: 1` **覆盖**成 `2`
// （见 `upstream.CloudFileOpHeaders`），追加式写法会同时发出 `1` 与 `2` 两号线 ——
// 值断言（"有 2"）抓不到它，计数才抓得到。
func teHeaderCount(request core.HTTPRequest, name string) []string {
	out := []string{}
	for _, pair := range request.Headers {
		if strings.EqualFold(pair[0], name) {
			out = append(out, pair[1])
		}
	}
	return out
}

// teCreateRequest 取这一趟第一条 `file/create` 请求（没有就回 false）。
func teCreateRequest(script *liveTaskScript) (core.HTTPRequest, bool) {
	for _, request := range script.snapshot() {
		if strings.Contains(request.URL, mxCreateEndpoint) {
			return request, true
		}
	}
	return core.HTTPRequest{}, false
}

// ─────────────────────────────────────────────────────────────────────────────
// 判定（纯函数）
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskExpansionPredicatesKeepAbsentApart：⭐ 三格判定把"缺"与"给了否/给了 0"分开。
//
// 反向改法各有一条会红：`!= nil && !*p` 写成 `!(p != nil && *p)`（缺也去凑备份 ⇒ 多写
// 一个文件）；`> 0` 写成 `!= nil`（读到 0 就去领 ⇒ 多发一发写请求）。
func TestTaskExpansionPredicatesKeepAbsentApart(t *testing.T) {
	cases := []struct {
		name       string
		read       upstream.TaskExpansion
		wantUpload bool
		wantClaim  bool
		wantJudged bool
	}{
		{
			name: "实拍那一份：备份开着、记录数 0",
			read: upstream.TaskExpansion{
				CurMonthBackup: teBool(true), CurMonthTaskRecordCount: teInt(0),
			},
			wantUpload: false, wantClaim: false, wantJudged: true,
		},
		{
			name:       "两格都没读到 ⇒ 判不了、不凑、不领",
			read:       upstream.TaskExpansion{NextMonthTaskRecordCount: teInt(358)},
			wantUpload: false, wantClaim: false, wantJudged: false,
		},
		{
			name:       "只有 nextMonth 那一格 ⇒ 仍判不了（它不是判据）",
			read:       upstream.TaskExpansion{NextMonthTaskRecordCount: teInt(9)},
			wantUpload: false, wantClaim: false, wantJudged: false,
		},
		{
			// 这一档是整条链唯一会写用户网盘的入口 ⇒ 只认"读到的 false"。
			name:       "读到 false 才凑",
			read:       upstream.TaskExpansion{CurMonthBackup: teBool(false)},
			wantUpload: true, wantClaim: false, wantJudged: true,
		},
		{
			name:       "记录数 1 ⇒ 领；记录数 0 ⇒ 不领",
			read:       upstream.TaskExpansion{CurMonthTaskRecordCount: teInt(1)},
			wantUpload: false, wantClaim: true, wantJudged: true,
		},
		{
			// 负数**不许**当"有记录"：那是把没见过的形状读成判据成立。
			name:       "记录数读到负数 ⇒ 不领",
			read:       upstream.TaskExpansion{CurMonthTaskRecordCount: teInt(-1)},
			wantUpload: false, wantClaim: false, wantJudged: true,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := taskExpansionShouldUpload(c.read); got != c.wantUpload {
				t.Errorf("该不该凑备份：实得 %v，期望 %v", got, c.wantUpload)
			}
			if got := taskExpansionShouldClaim(c.read); got != c.wantClaim {
				t.Errorf("该不该领：实得 %v，期望 %v", got, c.wantClaim)
			}
			if got := taskExpansionJudged(c.read); got != c.wantJudged {
				t.Errorf("判不判得了：实得 %v，期望 %v", got, c.wantJudged)
			}
		})
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路：实拍那一档（两支都不走）
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskExpansionCapturedReadingSendsNoWrites：⭐ 实拍那一份读数 ⇒ **一枪写请求都不发**。
//
// 这一条是本文件唯一**有实拍 backing** 的链路用例（其余形状是合成的），
// 它钉的是"这一族在今天真实见过的那一个号上，只做了一次只读"。
func TestTaskExpansionCapturedReadingSendsNoWrites(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		liveTaskRoute{teReadEndpoint, []string{teCaptured}})
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("实拍那一档该走通：%v", perr)
	}
	if creates, puts, completes := mxUploadCounts(fixture.script); creates+puts+completes != 0 {
		t.Errorf("备份开着就一枪上传都不发，实得 create=%d PUT=%d complete=%d",
			creates, puts, completes)
	}
	if got := fixture.script.count(teClaimEndpoint); got != 0 {
		t.Errorf("记录数读到 0 就不该发领取，实得 %d 次", got)
	}
	if got := fixture.script.count(teReadEndpoint); got != 1 {
		t.Errorf("没凑备份就只读一遍，实得 %d 次", got)
	}
	if len(fixture.wait.durations) != 0 {
		t.Errorf("没凑备份不该等，实等了 %v", fixture.wait.durations)
	}
	if res.Reads != 1 || res.Status != TaskExpansionStatusNothing {
		t.Fatalf("实得 reads=%d status=%v，期望 1 / Nothing", res.Reads, res.Status)
	}
	summary := res.SummaryLine()
	for _, want := range []string{"curMonthBackup=true", "curMonthTaskRecordCount=0", "下月可领 358 朵"} {
		if !strings.Contains(summary, want) {
			t.Errorf("小结该带 %q，实得 %q", want, summary)
		}
	}
	// "下月可领"念的是**结论那一次**（这里两次指向同一份），且 0 / 缺都不念。
	if strings.Contains(summary, "凑备份") {
		t.Errorf("没凑过备份就不该在小结里出现这两个字：%q", summary)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 链路：凑备份那一支（合成形状）
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskExpansionUploadsWaitsReReadsThenClaims ⭐ 完整那一支的**顺序与次数**。
//
// 一处一处钉：三枪各一次 → 等**恰好一次**（且是注入的那个值）→ 重读一次 →
// 领一次。少任何一步都不会报错，只会让"这个月到底算不算凑上了"变成猜的。
func TestTaskExpansionUploadsWaitsReReadsThenClaims(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(5), nil, teStr(teAcceptFirst)),
				teRead(teBool(true), teInt(5), nil, teStr(teAcceptSecond)),
			}},
			liveTaskRoute{teClaimEndpoint, []string{teClaimOK(120)}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("这一趟该走通：%v", perr)
	}
	if creates, puts, completes := mxUploadCounts(fixture.script); creates != 1 ||
		puts != 1 || completes != 1 {
		t.Errorf("凑备份应是三枪各一次，实得 create=%d PUT=%d complete=%d（URL：%v）",
			creates, puts, completes, teURLs(fixture.script))
	}
	if got := fixture.script.count(teReadEndpoint); got != 2 {
		t.Errorf("传完**必须**重读一次，实得 %d 次", got)
	}
	if len(fixture.wait.durations) != 1 || fixture.wait.durations[0] != teWait {
		t.Errorf("应等恰好一次 %v，实得 %v", teWait, fixture.wait.durations)
	}
	if got := fixture.script.count(teClaimEndpoint); got != 1 {
		t.Errorf("领取该发一次，实得 %d", got)
	}
	if res.Status != TaskExpansionStatusClaimed || res.CloudCount == nil || *res.CloudCount != 120 {
		t.Fatalf("实得 status=%v count=%v", res.Status, res.CloudCount)
	}
	if res.Reads != 2 || res.BackupRetries != 1 || res.Upload != TaskExpansionUploadDone {
		t.Errorf("实得 reads=%d retries=%d upload=%v", res.Reads, res.BackupRetries, res.Upload)
	}
	if res.WaitedTime != teWait {
		t.Errorf("WaitedTime 该记下实际等了多久，实得 %v", res.WaitedTime)
	}
	// fileId 只进事件流、不进长期存的那句小结（取舍见 [TaskExpansionResult.uploadTail]）。
	if !strings.Contains(fixture.events.joined(), "fileId SYNTH-FID-1") {
		t.Errorf("事件流该念出 fileId 让人能去网盘里处置，实得 %q", fixture.events.joined())
	}
	summary := res.SummaryLine()
	if strings.Contains(summary, "SYNTH-FID") {
		t.Errorf("小结不该带 fileId：%q", summary)
	}
	if !strings.Contains(summary, "领到 120 朵") || !strings.Contains(summary, "凑备份传了一个文件，没删") {
		t.Errorf("小结该同时念出「领到」与「传过、没删」：%q", summary)
	}
	// 备份档那两枚头：值必须是 2/200，且**各只有一枚**（追加式会同时发 1 与 2）。
	create, ok := teCreateRequest(fixture.script)
	if !ok {
		t.Fatal("没看到 file/create 请求")
	}
	if got := teHeaderCount(create, "x-yun-op-type"); len(got) != 1 || got[0] != "2" {
		t.Errorf("备份档的 op-type 应恰好一枚且为 2，实得 %v", got)
	}
	if got := teHeaderCount(create, "x-yun-sub-op-type"); len(got) != 1 || got[0] != "200" {
		t.Errorf("备份档的 sub-op-type 应恰好一枚且为 200，实得 %v", got)
	}
}

// TestTaskExpansionClaimsFromTheReReadAcceptDate：⭐ 领的是**重读那一份**的 `acceptDate`。
//
// 两份读数各给一个不同的哨兵值 ⇒ "拿旧判据去领"这一类写法会当场红。
// 旧实现读的是解构出来的那一份（重入时是新的），这里逐字对齐。
func TestTaskExpansionClaimsFromTheReReadAcceptDate(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(3), nil, teStr(teAcceptFirst)),
				teRead(teBool(true), teInt(3), nil, teStr(teAcceptSecond)),
			}},
			liveTaskRoute{teClaimEndpoint, []string{teClaimOK(1)}},
		}, mxUploadRoutes(1)...)...)
	if _, perr := fixture.run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("这一趟该走通：%v", perr)
	}
	var claimURL string
	for _, url := range teURLs(fixture.script) {
		if strings.Contains(url, teClaimEndpoint) {
			claimURL = url
		}
	}
	if claimURL == "" {
		t.Fatal("领取那一发没打出去")
	}
	if strings.Contains(claimURL, teAcceptFirst) {
		t.Errorf("领的是**第一遍**的 acceptDate（旧判据）：%s", claimURL)
	}
	if !strings.Contains(claimURL, teAcceptSecond) {
		t.Errorf("领的不是重读那一份的 acceptDate：%s", claimURL)
	}
}

// TestTaskExpansionUploadsAtMostOneFile：⭐ 重读仍读到 false ⇒ **不再传第二个**，且报需人工。
//
// 这一条钉住 [taskExpansionRetries] 那条上限的**行为**（常量本身没有读它的分支，
// 上限由"只有第一遍那一次能进凑备份那一格"的结构保证 —— 见那个常量的注释）。
// 多传的表现是"每跑一次就往用户网盘里堆一个文件"，而这没有任何自动出口。
func TestTaskExpansionUploadsAtMostOneFile(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(0), nil, nil),
				teRead(teBool(false), teInt(0), nil, nil),
			}},
		}, mxUploadRoutes(3)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("凑不上不算失败：%v", perr)
	}
	if creates, _, _ := mxUploadCounts(fixture.script); creates != 1 {
		t.Errorf("至多传**一个**文件，实得 create 打了 %d 次", creates)
	}
	if !res.BackupStillOff {
		t.Error("重读仍读到 false ⇒ 要把这一格单独记下来（它和'领到了'可以并存）")
	}
	if res.Status != TaskExpansionStatusBackupManual {
		t.Fatalf("实得 %v，期望 BackupManual", res.Status)
	}
	report := res.RunReport()
	if report.Outcome != core.OutcomeManualRequired {
		t.Errorf("实得 %v，期望需人工", report.Outcome)
	}
	if len(report.ManualSteps) != 1 {
		t.Fatalf("待办条数实得 %d", len(report.ManualSteps))
	}
	step := report.ManualSteps[0]
	if step.Target != core.TargetMobileApp {
		t.Errorf("开备份只能在 App 里做，target 实得 %v", step.Target)
	}
	if step.Capability != "task-expansion" {
		t.Errorf("待办按能力去重，code 实得 %q", step.Capability)
	}
	if !strings.Contains(step.Instruction, "自动备份") {
		t.Errorf("待办要说清去哪儿做什么，实得 %q", step.Instruction)
	}
	// 没领到东西时也要看得见"这一趟传过文件"。
	if !strings.Contains(res.SummaryLine(), "凑备份传了一个文件，没删") {
		t.Errorf("小结要带上上传实况：%q", res.SummaryLine())
	}
}

// TestTaskExpansionClaimedWithBackupStillOffIsNotManual：⭐ 领到了就**不**降级成需人工。
//
// 这一条取态是 **agent 侧默认（非用户要求）**，取舍与理由写在
// [TaskExpansionResult.RunReport] 头上。它钉的是"别把已经办成的那一趟改成黄灯"：
// 备份那一格影响的是**下个月**，天天报待办而人去了也不改变本月 ⇒ 黄灯会没人看。
// 反向改法（把 BackupStillOff 也映射成 Manual）会让这一条红。
func TestTaskExpansionClaimedWithBackupStillOffIsNotManual(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(2), nil, teStr(teAcceptFirst)),
				teRead(teBool(false), teInt(2), nil, teStr(teAcceptSecond)),
			}},
			liveTaskRoute{teClaimEndpoint, []string{teClaimOK(9)}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("这一趟该走通：%v", perr)
	}
	if res.Status != TaskExpansionStatusClaimed || !res.BackupStillOff {
		t.Fatalf("夹具该是「领到了 + 备份仍未开」，实得 status=%v stillOff=%v",
			res.Status, res.BackupStillOff)
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
		t.Errorf("领到了就报成功，实得 %v", report.Outcome)
	}
	if !strings.Contains(res.SummaryLine(), "翻倍的前置没开") {
		t.Errorf("不降级成黄灯，但那一句仍要**念出来**：%q", res.SummaryLine())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// "刻意差别"逐条钉住
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskExpansionBlockedSendsNothingAndDoesNotWait：⭐ 换不到 deviceId ⇒ 一枪不发、**也不空等**。
//
// 旧实现那句 `backFile` 的返回值被丢掉，所以"备份失败"之后它照样睡 20 秒再重读；
// 而"一枪都没发"时那一等没有任何东西在等。少等只省 20 秒，多等一次的代价是
// "每一次真跑都要在无人知晓的情况下停 20 秒"。
// 这一条同时钉"仍重读"（偏差只压等，不压读）。
func TestTaskExpansionBlockedSendsNothingAndDoesNotWait(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreNoDevice(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(0), nil, nil),
			}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("挡下凑备份不该把整趟弄失败：%v", perr)
	}
	if creates, puts, completes := mxUploadCounts(fixture.script); creates+puts+completes != 0 {
		t.Errorf("前置缺就一枪都不发，实得 create=%d PUT=%d complete=%d",
			creates, puts, completes)
	}
	if len(fixture.wait.durations) != 0 {
		t.Errorf("一枪没发不该空等，实得 %v", fixture.wait.durations)
	}
	if res.Upload != TaskExpansionUploadBlocked {
		t.Errorf("档位该是 Blocked（本工具侧缺接线），实得 %v", res.Upload)
	}
	if res.Reads != 2 {
		t.Errorf("少等**不**少读：仍要重读一次拿最新判据，实得 reads=%d", res.Reads)
	}
	if !strings.Contains(res.SummaryLine(), "一枪没发") {
		t.Errorf("小结要念出「一枪没发」：跑的人得知道这一趟没往网盘写东西：%q",
			res.SummaryLine())
	}
}

// TestTaskExpansionUploadBlockedGuards ⭐ 凑备份那三枚守卫：缺一样就**一枪不发**（直接调那一层）。
//
// 这三格在链路用例里**到不了**：建会话那一枪就要用随机源（`core.UUIDv4` 在签名里），
// 把它设成 nil 时整趟在第一次读之前 panic（实测过一次）；`Credential` 为 nil 时
// [SignInWorld.Open] 早就报错了。所以这里只调 [TaskExpansionRun.uploadBackup] 那一层，
// 钉三件事：**不发**、**没有产物**、原因点名**缺的是哪一层**（本工具侧缺接线 ≠ 上游故障，
// 处置完全不同 —— 前者要接线，后者要重跑）。
// 反向改法是删掉任何一句守卫 ⇒ 那一格会 panic 在下一层的空指针上。
func TestTaskExpansionUploadBlockedGuards(t *testing.T) {
	deviceID := "SYNTH-DEVICE"
	cases := []struct {
		name       string
		random     core.Random
		opening    SignInOpening
		device     *string
		wantReason string
	}{
		{
			// 上传那一族的**每一枪**都带 `deviceid` 头 ⇒ 缺它就不发（这一族零样本，不赌）。
			name:       "缺 deviceId",
			random:     testRng(),
			device:     nil,
			wantReason: "deviceid",
		},
		{
			name:       "缺随机源",
			random:     nil,
			device:     &deviceID,
			wantReason: "随机源没接",
		},
		{
			name:       "缺 authorization",
			random:     testRng(),
			opening:    SignInOpening{},
			device:     &deviceID,
			wantReason: "authorization",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			run := TaskExpansionRun{SignInWorld: SignInWorld{Random: c.random}}
			upload, fileID, name, reason := run.uploadBackup(
				context.Background(), c.opening, c.device)
			if upload != TaskExpansionUploadBlocked {
				t.Errorf("实得 %v，期望 Blocked（原因 %q）", upload, reason)
			}
			if fileID != "" || name != "" {
				t.Errorf("一枪没发时不该有产物，实得 fileID=%q name=%q", fileID, name)
			}
			if !strings.Contains(reason, c.wantReason) {
				t.Errorf("原因要点名缺的是哪一层，实得 %q", reason)
			}
		})
	}
}

// TestTaskExpansionFailedUploadStillWaitsAndReReads：上传**发过、没成** ⇒ 仍等、仍重读。
//
// 与上一条是**一对**：区分两者的不是"成没成"，是"有没有把事件发出去"——
// create 已经占下坑时，服务端可能已经把这一次记进当月备份了。
func TestTaskExpansionFailedUploadStillWaitsAndReReads(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{mxCreateEndpoint, []string{`{"code":"9999","success":false,"message":"合成失败"}`}},
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(0), nil, nil),
				teRead(teBool(true), teInt(0), nil, nil),
			}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("上传失败该折成档位：%v", perr)
	}
	if len(fixture.wait.durations) != 1 {
		t.Errorf("create 已经发出去了 ⇒ 仍要等，实得 %v", fixture.wait.durations)
	}
	if res.Upload != TaskExpansionUploadFailed {
		t.Errorf("实得 %v，期望 Failed", res.Upload)
	}
	if !strings.Contains(res.SummaryLine(), "没传成") ||
		!strings.Contains(res.SummaryLine(), "合成失败") {
		t.Errorf("小结要念出「没传成」+ 上游那句话：%q", res.SummaryLine())
	}
	if res.Reads != 2 {
		t.Errorf("传失败也要重读（备份那一格可能已被别的东西刷上去了），实得 %d", res.Reads)
	}
}

// TestTaskExpansionReportsUploadWhenTheReReadFails：⭐ 重读挂了 ⇒ **交回错误**，
// 但"已经往网盘传了一个文件"这件事必须留着（"刻意差别"第 3 条）。
//
// 写成旧实现那样（`error` + `return`）的表现是可观察的：用户网盘里多了一个文件，
// 而历史页这一句是"获取任务扩展失败"，谁都想不到要先去网盘看一眼。
func TestTaskExpansionReportsUploadWhenTheReReadFails(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(7), nil, nil),
				`{"code":1,"msg":"系统繁忙，请稍后重试"}`,
			}},
			liveTaskRoute{teClaimEndpoint, []string{teClaimOK(1)}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("重读失败必须交回错误（判据没了就不能再发领取）")
	}
	if got := fixture.script.count(teClaimEndpoint); got != 0 {
		t.Errorf("判据没读到就不该发领取，实得 %d 次", got)
	}
	if res.Upload != TaskExpansionUploadDone || res.UploadName == "" {
		t.Errorf("上传实况要留着，实得 upload=%v name=%q", res.Upload, res.UploadName)
	}
	if res.Status != TaskExpansionStatusNone {
		t.Errorf("这一趟没走到结论，档位该是零值，实得 %v", res.Status)
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeFailed {
		t.Errorf("零值档位不许报成功（否则网盘里多一个文件而历史页是绿的），实得 %v",
			report.Outcome)
	}
	if !strings.Contains(res.SummaryLine(), "传了一个文件，没删") {
		t.Errorf("小结要念出「传过、没删」：%q", res.SummaryLine())
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("错误里该带上游那句话，实得 %q", perr.Error())
	}
}

// TestTaskExpansionClaimsWhenUpstreamReportsNoCount：⭐ 领到了但上游没报数 ⇒ 仍报"已领"。
//
// 旧实现那句是 truthy 判据（`result?.cloudCount`），于是"`code:0` 且没有 result"
// 那一趟它**一句都不念**，看起来像什么都没发生。写请求发出去且上游认了，
// 这是实况就得念出来；朵数是另一格，念"上游没报数"而不是 0（"刻意差别"第 2 条）。
func TestTaskExpansionClaimsWhenUpstreamReportsNoCount(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		liveTaskRoute{teReadEndpoint, []string{
			teRead(teBool(true), teInt(4), nil, teStr(teAcceptFirst))}},
		liveTaskRoute{teClaimEndpoint, []string{`{"code":0,"msg":"success"}`}})
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("上游没报数不是失败：%v", perr)
	}
	if got := fixture.script.count(teClaimEndpoint); got != 1 {
		t.Fatalf("领取该发一次，实得 %d", got)
	}
	if res.Status != TaskExpansionStatusClaimed {
		t.Fatalf("实得 %v，期望 Claimed", res.Status)
	}
	if res.CloudCount != nil {
		t.Errorf("没给数时该是 nil（nil ≠ 0），实得 %d", *res.CloudCount)
	}
	summary := res.SummaryLine()
	if !strings.Contains(summary, "已领（上游没报朵数）") {
		t.Errorf("实得 %q", summary)
	}
	if strings.Contains(summary, "领到 0 朵") {
		t.Errorf("「没说」不许念成「说了 0」：%q", summary)
	}
}

// TestTaskExpansionFoldsClaimFailure：领取那一发失败 ⇒ **不**交回错误（发过了是实况），
// 档位与四态都落"失败"。
func TestTaskExpansionFoldsClaimFailure(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		liveTaskRoute{teReadEndpoint, []string{
			teRead(teBool(true), teInt(6), nil, teStr(teAcceptFirst))}},
		liveTaskRoute{teClaimEndpoint, []string{`{"code":1302,"msg":"未开启备份"}`}})
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("发过、失败，这一趟的实况该留在结论里而不是变成一句错误：%v", perr)
	}
	if res.Status != TaskExpansionStatusClaimFailed {
		t.Fatalf("实得 %v，期望 ClaimFailed", res.Status)
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeFailed {
		t.Errorf("实得 %v，期望失败", report.Outcome)
	}
	if !strings.Contains(res.SummaryLine(), "未开启备份") {
		t.Errorf("小结该带上游那句话（它是这一族第一条可能的实拍文案）：%q", res.SummaryLine())
	}
}

// TestTaskExpansionUnreadableSendsNothing：两格判据都没读到 ⇒ 一枪不发、报"判不了"。
//
// 反向改法是把"没读到"当 0 ⇒ 报"没得领"，那是一句**没根据的肯定话**
// （`AGENTS.md` §1.7）。
func TestTaskExpansionUnreadableSendsNothing(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		liveTaskRoute{teReadEndpoint, []string{teRead(nil, nil, teInt(358), nil)}})
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("判不了不是失败：%v", perr)
	}
	if res.Status != TaskExpansionStatusUnreadable {
		t.Fatalf("实得 %v，期望 Unreadable", res.Status)
	}
	if creates, puts, completes := mxUploadCounts(fixture.script); creates+puts+completes != 0 {
		t.Errorf("判不了不该发任何写请求，实得 create=%d PUT=%d complete=%d",
			creates, puts, completes)
	}
	if got := fixture.script.count(teClaimEndpoint); got != 0 {
		t.Errorf("判不了也不该领，实得 %d 次", got)
	}
	summary := res.SummaryLine()
	if !strings.Contains(summary, "判不了") || !strings.Contains(summary, "curMonthBackup=缺") {
		t.Errorf("实得 %q", summary)
	}
	if strings.Contains(summary, "下月可领 358 朵") {
		t.Errorf("判不了那一档不念下月（结论还没落地就念附送信息，会把注意力带走）：%q", summary)
	}
	if report := res.RunReport(); report.Outcome != core.OutcomeSuccess {
		t.Errorf("一枪没发的那一趟报成功（与 notice 的 Unknown 同一口径），实得 %v",
			report.Outcome)
	}
}

// TestTaskExpansionReadFailureReturnsError：读第一遍就挂 ⇒ 交回错误、零请求之外的东西都不发。
func TestTaskExpansionReadFailureReturnsError(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{`{"code":1,"msg":"系统繁忙，请稍后重试"}`}},
			liveTaskRoute{teClaimEndpoint, []string{teClaimOK(1)}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("读第一遍失败必须交回错误")
	}
	if creates, puts, completes := mxUploadCounts(fixture.script); creates+puts+completes != 0 {
		t.Errorf("判据没读到就不该上传，实得 %d/%d/%d", creates, puts, completes)
	}
	if got := fixture.script.count(teClaimEndpoint); got != 0 {
		t.Errorf("判据没读到就不该领，实得 %d 次", got)
	}
	if res.Status != TaskExpansionStatusNone || res.Reads != 0 {
		t.Errorf("实得 status=%v reads=%d，期望零值", res.Status, res.Reads)
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("错误里该带上游那句话，实得 %q", perr.Error())
	}
}

// TestTaskExpansionWithNoCredentialDoesNotGoOut：库里没凭据 ⇒ 一次出网都不该有。
func TestTaskExpansionWithNoCredentialDoesNotGoOut(t *testing.T) {
	fixture := taskExpansionFixture(&poolSignInStore{fakeStore{state: nil}})
	if _, perr := fixture.run.RunAccount(context.Background(),
		core.NewAccountID(liveTestAccountID)); perr == nil {
		t.Fatal("库里没有凭据应该报错")
	}
	if len(fixture.script.snapshot()) != 0 {
		t.Errorf("取不到凭据就不该出网，实得 %d 条请求", len(fixture.script.snapshot()))
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// 播报与卫生
// ─────────────────────────────────────────────────────────────────────────────

// TestTaskExpansionEventsNeverLeakTicketOrAcceptDate：⭐ 出网的两句里不许有票与日期串。
//
// 事件流与小结会长期存在（历史页、推送），而 `acceptDate` 是上游给的一个可猜测形态的
// 参数（拼在领取那一条 URL 上）。这里只准念"acceptDate=有/缺"。
func TestTaskExpansionEventsNeverLeakTicketOrAcceptDate(t *testing.T) {
	fixture := taskExpansionFixture(taskExpansionStoreBound(),
		append([]liveTaskRoute{
			liveTaskRoute{teReadEndpoint, []string{
				teRead(teBool(false), teInt(2), teInt(358), teStr(teAcceptFirst)),
				teRead(teBool(true), teInt(2), teInt(358), teStr(teAcceptSecond)),
			}},
			liveTaskRoute{teClaimEndpoint, []string{teClaimOK(30)}},
		}, mxUploadRoutes(1)...)...)
	res, perr := fixture.run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("这一趟该走通：%v", perr)
	}
	heard := fixture.events.joined() + "\n" + res.SummaryLine()
	for _, forbidden := range []string{
		"JWT-1", "jwttoken", teAcceptFirst, teAcceptSecond, wholeAuth,
	} {
		if strings.Contains(heard, forbidden) {
			t.Errorf("播报里漏了 %q：%s", forbidden, heard)
		}
	}
	// 反向确认这一格**有**在念：只是不念值。
	if !strings.Contains(heard, "acceptDate=有") {
		t.Errorf("读数行要念 acceptDate 的\"有/缺\"，实得 %q", heard)
	}
}

// TestTaskExpansionResultMapsStatusToOutcome：档位 → 四态那张表（纯函数，不出网）。
//
// 漂移就发生在这一小张表上：加一档、或把零值落到 default 分支，都不会让别的地方变红。
func TestTaskExpansionResultMapsStatusToOutcome(t *testing.T) {
	cases := []struct {
		name      string
		res       TaskExpansionResult
		want      core.RunOutcome
		wantSteps int
	}{
		{
			name:      "领到了",
			res:       TaskExpansionResult{Status: TaskExpansionStatusClaimed, CloudCount: teInt(10)},
			want:      core.OutcomeSuccess,
			wantSteps: 0,
		},
		{
			name: "没得领也算成功",
			res:  TaskExpansionResult{Status: TaskExpansionStatusNothing, Reason: "本月记录数读到 0，不是 >0"},
			want: core.OutcomeSuccess,
		},
		{
			name: "判不了也算成功（一枪没发）",
			res:  TaskExpansionResult{Status: TaskExpansionStatusUnreadable, Reason: "两格都没读到"},
			want: core.OutcomeSuccess,
		},
		{
			name:      "凑过一次仍未开 ⇒ 需人工",
			res:       TaskExpansionResult{Status: TaskExpansionStatusBackupManual, Reason: "重读仍 false"},
			want:      core.OutcomeManualRequired,
			wantSteps: 1,
		},
		{
			name:      "发了但失败",
			res:       TaskExpansionResult{Status: TaskExpansionStatusClaimFailed, Error: "上游回 1302"},
			want:      core.OutcomeFailed,
			wantSteps: 0,
		},
		{
			// 零值那一档出现在"提前交回错误"的那几条路径上；报成功的话，
			// 表现是"网盘里多了一个文件、历史页那一格是绿的"。
			name:      "没走到结论 ⇒ 失败",
			res:       TaskExpansionResult{Upload: TaskExpansionUploadDone, UploadName: "asign-x.png"},
			want:      core.OutcomeFailed,
			wantSteps: 0,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			report := c.res.RunReport()
			if report.Outcome != c.want {
				t.Errorf("实得 %v，期望 %v", report.Outcome, c.want)
			}
			if len(report.ManualSteps) != c.wantSteps {
				t.Errorf("待办条数应 %d，实得 %d", c.wantSteps, len(report.ManualSteps))
			}
			if report.ManualSteps == nil {
				t.Error("ManualSteps 应为空 slice 而不是 nil（前端 .map 会炸）")
			}
			if report.Summary == nil {
				t.Error("每一档都要有一句小结，实得 nil")
			}
		})
	}
}

// TestTaskExpansionManualStepCarriesThisAccountsID：待办里那个账号是**本次的**。
//
// ⚠ 挡的改法是把账号做成一份共用变量再 `&` 出去 —— 多账号串行时全部待办指向
// 最后那个号，而单账号用例照样全绿。
func TestTaskExpansionManualStepCarriesThisAccountsID(t *testing.T) {
	for _, id := range []string{"19****01", "19****02"} {
		res := TaskExpansionResult{Account: core.NewAccountID(id),
			Status: TaskExpansionStatusBackupManual}
		step := res.RunReport().ManualSteps[0]
		if step.Account == nil || step.Account.String() != id {
			t.Errorf("待办应指向 %s，实得 %v", id, step.Account)
		}
	}
}
