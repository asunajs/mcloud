package app

// [PlanTaskStep] 与复查那三个纯函数的用例（#9 写侧的**判据面**）。
//
// 本文件一个请求都不发 —— 那是刻意的：这一层的每一格都是"看到这条任务就该怎么办"，
// 而它必须能在没有网络、没有真账号的情况下被逐条钉住。发请求之后的行为
// （发了几枪、顺序、失败还继不继续）在 `task_matrix_run_test.go`。
//
// 钉住的六件事，每一件都对应旧实现 `packages/core/src/tasks/app/tasks.ts` 的一处：
//
//  1. **default 分支盲点、且不过 `currstep` 门**（`tasks.ts:96-107`）——
//     把这道门误加到 default 上，表现是实拍那三条 `currstep` 4/6/8 的条目一发都不打，
//     界面上"待做 3 条"跑完还是 3 条，而没人知道为什么；
//  2. **beiyong1 走 `handleTaskClick` ⇒ 过门**（与上一条**方向相反**，两向都要有用例）；
//  3. **缺端点族 ⇒ 一条枪都不发**（本仓与旧实现的**唯一**行为差异，理由写在
//     `task_matrix_dispatch.go` 文件头：补一发打卡 = 拿用户的账谎报完成）；
//  4. **`month` 那道门只在严格读到 false 时挡**（旧 TS 那句是 `=== false`，
//     没读到 ≠ 未开备份）；
//  5. **runner 的前置打卡过门、玩法那一发不过门**（409 在 `currstep!=0` 时仍要跑玩法）；
//  6. **复查那四格 + 补打卡那两枚 key**（`randomCloudTask` 零实拍，必须在结论里看得见）。

import (
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/upstream"
)

// dispatchItem 造一条**待做**任务（`enable=1`、`state` 由参数给、`marketname` 恒云盘侧）。
//
// `state` 传空串 ⇒ 不给那一格（旧 TS 的门是 `undefined === "FINISH"` 不成立 ⇒ 保留，
// 这一档在 [TaskMatrix.Pending] 那边钉，这里只是让夹具能表达"上游没给"）。
func dispatchItem(id int64, group, name, state string, step int64) upstream.TaskV2Item {
	item := upstream.TaskV2Item{Enable: i64(1), CurrStep: i64(step)}
	value := id
	item.TaskID = &value
	g, n, s := group, name, state
	item.GroupID = &g
	item.Name = &n
	if s != "" {
		item.State = &s
	}
	market := upstream.MarketName
	item.MarketName = &market
	return item
}

// TestPlanTaskStepBlindClickSkipsTheStepGate ⭐ default 分支不过 `currstep` 门。
//
// 反向改坏它：给 [PlanTaskStep] 末尾那一发也套上 [taskClickGate] ⇒ 第二条（`currstep=6`）
// 变成 StepBlockedByRule，本条立刻红。
func TestPlanTaskStepBlindClickSkipsTheStepGate(t *testing.T) {
	for _, step := range []int64{0, 6} {
		item := dispatchItem(777, "day", "看个广告", "WAIT", step)
		plan := PlanTaskStep(item, taskStepInput{})
		if plan.Kind != StepClick {
			t.Errorf("currstep=%d 时 default 分支该盲点，实得 %v（%s）", step, plan.Kind, plan.Reason)
		}
		if plan.ClickKey != upstream.TaskClickKeyDefault {
			t.Errorf("那一发的 key 应是实拍的 task，实得 %q", plan.ClickKey)
		}
		// 旧实现在 default 那一支**不** sleep（`handleTaskClick` 里才有）。
		if plan.Paced {
			t.Errorf("default 盲点不该等：节奏与真实客户端差太远（currstep=%d）", step)
		}
	}
}

// TestPlanTaskStepNeverClickListBlocksTheBlindClick：`SKIP_TASK_LIST` 命中 ⇒ 一发都不发。
//
// ⚠ 这一条只钉"那一发盲点不发"，**不**钉"这条任务整体不做" —— 旧实现里 runner
// 照样跑（[TaskMatrixNeverClick] 那条注释写的就是这件事）。这里给的输入全是
// default 分支（那些 id 既无 runner 也不在 toolGaps 表里），所以判 StepBlockedByRule 成立。
func TestPlanTaskStepNeverClickListBlocksTheBlindClick(t *testing.T) {
	for _, id := range TaskMatrixNeverClick {
		item := dispatchItem(id, "day", "被旧表挡着的那条", "WAIT", 0)
		plan := PlanTaskStep(item, taskStepInput{})
		if plan.Kind != StepBlockedByRule {
			t.Errorf("id %d 在 SKIP 表里 ⇒ 不该发盲点，实得 %v", id, plan.Kind)
		}
		if plan.ClickKey != "" {
			t.Errorf("被挡的条目不该带 key（带了就会发出去），实得 %q", plan.ClickKey)
		}
		if !strings.Contains(plan.Reason, "SKIP_TASK_LIST") {
			t.Errorf("理由要指名是哪张表挡的，便于回查旧实现，实得 %q", plan.Reason)
		}
	}
}

// TestPlanTaskStepToolGapSendsNothing ⭐ 缺端点族 ⇒ 不打卡、不谎报。
//
// 这是本仓与旧实现之间**唯一**的行为差异，所以它值得单独一条：
// 把它改回 [StepClick]，本条的每一个 id 都会红（而红出来的是"发了打卡"这件事）。
//
// 同时钉"理由文本互不相同"—— 那张表的**全部价值**在于告诉人下次补哪一族，
// 十二条同一句「未实现」等于没写（§1 里"空话等于没写"那条的机检版）。
func TestPlanTaskStepToolGapSendsNothing(t *testing.T) {
	seen := map[string]int64{}
	for id, want := range taskMatrixToolGaps {
		item := dispatchItem(id, "day", "要缺的那族玩法", "WAIT", 0)
		plan := PlanTaskStep(item, taskStepInput{})
		if plan.Kind != StepBlockedToolGap {
			t.Errorf("id %d 应报成未接玩法，实得 %v", id, plan.Kind)
		}
		if plan.ClickKey != "" || plan.Paced {
			t.Errorf("id %d 一条枪都不该发（打卡 = 谎报完成），实得 key=%q paced=%v",
				id, plan.ClickKey, plan.Paced)
		}
		if plan.Reason != want {
			t.Errorf("id %d 的理由应逐字取自那张表，实得 %q", id, plan.Reason)
		}
		if prev, dup := seen[want]; dup {
			t.Errorf("id %d 与 id %d 的理由一模一样 ⇒ 界面上看不出该补哪一族", id, prev)
		}
		seen[want] = id
	}
	if len(seen) != len(taskMatrixToolGaps) {
		t.Errorf("理由条数应与表同源，实得 %d vs %d", len(seen), len(taskMatrixToolGaps))
	}
}

// TestPlanTaskStepToolGapCoversTheOldRunnerList：旧实现有 runner 的 id **一条都不能漏**登记。
//
// 为什么单独钉这一格：漏登记的表现**不是**报错，而是那条任务落到 default ⇒
// 白打一发打卡、把一件没做的事记成已做。那是这一族里最贵的一种错。
//
// ⚠ 名单出处是旧 TS `runners.ts` 的 `switch (task.id)`（本机能跑那份源码，
// 这份名单是按它逐条抄的）；哪天接上其中一族，就把那个 id **删掉**，
// 而不是留着 ⇒ 留下的话上一条用例仍然绿，而实际行为早已不是 toolGap。
func TestPlanTaskStepToolGapCoversTheOldRunnerList(t *testing.T) {
	oldRunnerIDs := []int64{
		106, 113, 522, 107, 434, 472, 473, 1004, 1014, 1020, 1028, 585,
		409, 1005,
	}
	for _, id := range oldRunnerIDs {
		_, wired := taskMatrixRunners[id]
		_, gap := taskMatrixToolGaps[id]
		if !wired && !gap {
			t.Errorf("id %d 在旧实现有 runner，这里两表都没登记 ⇒ 会落到 default 盲点", id)
		}
		if wired && gap {
			t.Errorf("id %d 同时在 runner 表与缺口表 ⇒ 分派先命中 runner，那条缺口永远不会报出来", id)
		}
	}
	// 已接上的那两条**不该**再出现在缺口表里（接上后没删 = 界面上永远显示"没接玩法"）。
	for id := range taskMatrixRunners {
		if _, gap := taskMatrixToolGaps[id]; gap {
			t.Errorf("id %d 已接 runner，仍留在缺口表里", id)
		}
	}
}

// TestPlanTaskStepRunnerSequence：两条已接的 runner 走 [StepRunner]，且**前置那一发过门**。
func TestPlanTaskStepRunnerSequence(t *testing.T) {
	for id, wantKey := range map[int64]string{
		409: upstream.TaskClickKeySecond, 1005: upstream.TaskClickKeyDefault,
	} {
		item := dispatchItem(id, "day", "云朵中心入口", "WAIT", 0)
		plan := PlanTaskStep(item, taskStepInput{})
		if plan.Kind != StepRunner {
			t.Errorf("id %d 应走 runner，实得 %v（%s）", id, plan.Kind, plan.Reason)
		}
		if plan.Runner == "" {
			t.Errorf("id %d 的结论要能说出是谁做的（runner 名），实得空", id)
		}
		if plan.ClickKey != upstream.TaskClickKeyDefault {
			t.Errorf("id %d 的前置那一发该是通用 key=task，实得 %q", id, plan.ClickKey)
		}
		// 最后一发的 key 在执行层查表，但**必须**在表里看得见（否则两条 runner 无法区分）。
		if taskMatrixRunners[id].FinalKey != wantKey {
			t.Errorf("id %d 的最后一发应是 %q，实得 %q", id, wantKey, taskMatrixRunners[id].FinalKey)
		}
		// 旧实现在 runner 末尾 sleep（走的是 `handleTaskClick` 那一条路）。
		if !plan.Paced {
			t.Errorf("id %d 走 handleTaskClick ⇒ 旧实现在它末尾 sleep(500)，这里不该丢", id)
		}
	}
}

// TestPlanTaskStepRunnerWithoutPreambleClick ⭐ `currstep != 0` 时只跑玩法、不打卡。
//
// 旧实现那句 `clickTaskWithStep` 直接回 true ⇒ runner 照跑。反过来（把整条判成跳过）
// 的表现是"这个号的入口上报永远没做"，而结论里一条错都没有。
func TestPlanTaskStepRunnerWithoutPreambleClick(t *testing.T) {
	item := dispatchItem(409, "day", "云朵中心入口", "WAIT", 3)
	plan := PlanTaskStep(item, taskStepInput{})
	if plan.Kind != StepRunner {
		t.Fatalf("runner 该照跑（旧实现那一格回 true 后继续），实得 %v", plan.Kind)
	}
	if plan.ClickKey != "" {
		t.Errorf("currstep=3 ⇒ 前置那一发不发，实得 key=%q", plan.ClickKey)
	}
	if !strings.Contains(plan.Reason, "currstep") {
		t.Errorf("少发了一发要说清为什么，实得 %q", plan.Reason)
	}
}

// TestStepForBeiyong1UsesTheStepGate：beiyong1 那一支**过门**（与 default 相反）。
func TestStepForBeiyong1UsesTheStepGate(t *testing.T) {
	allowed := dispatchItem(596, "beiyong1", "备份一个文件", "WAIT", 0)
	plan := PlanTaskStep(allowed, taskStepInput{})
	if plan.Kind != StepClick || plan.ClickKey != upstream.TaskClickKeyDefault {
		t.Errorf("currstep=0 的 beiyong1 该打卡，实得 %+v", plan)
	}
	if !plan.Paced {
		t.Errorf("这一支走 handleTaskClick ⇒ 该等那 500ms，别与 default 的零间隔混成一个")
	}

	blocked := dispatchItem(597, "beiyong1", "备份一个文件", "WAIT", 4)
	plan = PlanTaskStep(blocked, taskStepInput{})
	if plan.Kind != StepBlockedByRule {
		t.Fatalf("currstep=4 该被门挡下（实拍三条正是 4/6/8），实得 %v", plan.Kind)
	}
	if !strings.Contains(plan.Reason, "4") {
		t.Errorf("理由要带上那个读数，否则界面上无从判断门是否判对，实得 %q", plan.Reason)
	}
}

// TestStepForBeiyong1UploadNameKeepsClickButRecordsGap ⭐ 打卡照发、缺的那一半明说。
//
// 这一格是"部分能做"的唯一形态：旧实现是"打卡 + 真上传"，我们只有前半。
// 判反的两个方向都错 —— 整条挡掉 = 少做本可做的打卡；只当成功 = 界面上看不见还缺上传。
func TestStepForBeiyong1UploadNameKeepsClickButRecordsGap(t *testing.T) {
	item := dispatchItem(598, "beiyong1", "上传照片到云端", "WAIT", 0)
	plan := PlanTaskStep(item, taskStepInput{})
	if plan.Kind != StepClick {
		t.Errorf("打卡那一半仍该发，实得 %v", plan.Kind)
	}
	if plan.ToolGap == "" || !strings.Contains(plan.ToolGap, "上传") {
		t.Errorf("缺的上传那一半必须写进结论，实得 %q", plan.ToolGap)
	}
	// 名字不含"图/照"的 ⇒ 旧实现那一支也不会去上传 ⇒ 不该报缺口。
	plain := dispatchItem(599, "beiyong1", "开启自动备份", "WAIT", 0)
	if got := PlanTaskStep(plain, taskStepInput{}); got.ToolGap != "" {
		t.Errorf("名字没写上传的条目不该报上传缺口，实得 %q", got.ToolGap)
	}
}

// TestBackupGateMonthGroup：`month` 那道门的四档（含"没读到 ⇒ 不挡"）。
//
// 门槛那句逐字出自旧实现：`marketname === "sign_in_3" && curMonthBackup === false
// && new Date().getDate() < 20`。
func TestBackupGateMonthGroup(t *testing.T) {
	item := dispatchItem(801, "month", "每月签到 7 天", "WAIT", 0)
	cases := []struct {
		name string
		in   taskStepInput
		want TaskStepKind
	}{
		{"读到未开备份 · 20 号前", taskStepInput{BackupKnown: true, DayOfMonth: 15}, StepNeedsHuman},
		// 20 号**当天**不挡：旧实现那句是严格小于。
		{"读到未开备份 · 20 号", taskStepInput{BackupKnown: true, DayOfMonth: 20}, StepClick},
		{"读到已开备份", taskStepInput{BackupKnown: true, BackupOn: true, DayOfMonth: 15}, StepClick},
		// ⭐ 这一档是最容易改错的：旧 TS 那句是 `=== false`（严格等于），
		// "没读到"绝不能顺手当"未开"。
		{"没读到备份状态", taskStepInput{BackupKnown: false, DayOfMonth: 15}, StepClick},
		{"时钟没接（天数为 0）", taskStepInput{BackupKnown: true, DayOfMonth: 0}, StepClick},
	}
	for _, c := range cases {
		plan := PlanTaskStep(item, c.in)
		if plan.Kind != c.want {
			t.Errorf("%s：应 %v，实得 %v（%s）", c.name, c.want, plan.Kind, plan.Reason)
		}
		if c.want == StepNeedsHuman {
			if plan.ClickKey != "" || plan.Paced {
				t.Errorf("需人工那一档一条枪都不该发，实得 %+v", plan)
			}
			if !strings.Contains(plan.Reason, "自动备份") {
				t.Errorf("要人说清去做什么，实得 %q", plan.Reason)
			}
		}
	}
}

// TestBackupGateOnlyOnTheCloudSide：门只管 `sign_in_3` 那侧（旧实现字面如此）。
//
// ⚠ **为什么照抄而不是改成两侧都挡**：备份开关看起来是账号级的，
// 但旧实现只在云盘侧挡 ⇒ 改成两侧是**另一种行为**，而我们没有样本支持它。
// 未验证之处写在 `task_matrix_dispatch.go` 那条注释里。
func TestBackupGateOnlyOnTheCloudSide(t *testing.T) {
	item := dispatchItem(802, "month", "每月任务", "WAIT", 0)
	email := upstream.TaskListV2EmailMarketName
	item.MarketName = &email
	in := taskStepInput{BackupKnown: true, DayOfMonth: 15}
	if got := PlanTaskStep(item, in); got.Kind == StepNeedsHuman {
		t.Errorf("邮箱那侧的每月任务不该被云盘的备份门挡（旧实现只判 sign_in_3），实得 %+v", got)
	}
	// 条目**没给** marketname ⇒ 不等于 sign_in_3 ⇒ 不挡（与旧实现 `undefined` 同向）。
	item.MarketName = nil
	if got := PlanTaskStep(item, in); got.Kind == StepNeedsHuman {
		t.Errorf("marketname 缺席应落到不挡，实得 %+v", got)
	}
}

// TestPlanTaskStepRejectsItemsWithoutID：没 id 的条目不发（旧实现会拼出 `id=undefined`）。
func TestPlanTaskStepRejectsItemsWithoutID(t *testing.T) {
	item := dispatchItem(1, "day", "上游没给 id", "WAIT", 0)
	item.TaskID = nil
	plan := PlanTaskStep(item, taskStepInput{})
	if plan.Kind != StepBlockedByRule || plan.ClickKey != "" {
		t.Errorf("无 id ⇒ 不发并说明原因，实得 %+v", plan)
	}
	if !strings.Contains(plan.Reason, "id") {
		t.Errorf("理由要指名缺的是 id，实得 %q", plan.Reason)
	}
}

// TestTaskClickGateIs434ExceptionOnlyAt22：那个没有任何解释的字面量例外。
//
// 直接测门，不走 [PlanTaskStep]：434 现在落在缺口表里 ⇒ 分派那一层根本到不了门
// （下面第二条用例就把这件事钉住，防止"例外其实早就失效了"没人发现）。
func TestTaskClickGateIs434ExceptionOnlyAt22(t *testing.T) {
	if _, ok := taskClickGate(434, dispatchItem(434, "day", "分享", "WAIT", 22), "task"); !ok {
		t.Error("434 在 currstep=22 该放行（旧实现 specialMap 的字面量）")
	}
	if _, ok := taskClickGate(434, dispatchItem(434, "day", "分享", "WAIT", 21), "task"); ok {
		t.Error("例外只在 22 那一格；21 该被门挡下")
	}
	if _, ok := taskClickGate(435, dispatchItem(435, "day", "分享", "WAIT", 22), "task"); ok {
		t.Error("22 这个例外只属于 434，别的 id 同一步数仍该挡")
	}
	if got := PlanTaskStep(dispatchItem(434, "day", "分享文件", "WAIT", 22), taskStepInput{}); got.Kind != StepBlockedToolGap {
		t.Errorf("434 今天仍该报玩法缺口（分享那一族未接），实得 %v ⇒ 门与缺口表的关系变了要回看这条", got.Kind)
	}
}

// TestPlanVerifyClick：复查那两枚补打卡（key 零实拍）。
func TestPlanVerifyClick(t *testing.T) {
	cases := []struct {
		name     string
		item     upstream.TaskV2Item
		wantKey  string
		wantCall bool
	}{
		{"481 在第二步", dispatchItem(481, "day", "随机云朵", "WAIT", 2), upstream.TaskClickKeyRandomCloud, true},
		{"481 步数不对", dispatchItem(481, "day", "随机云朵", "WAIT", 3), "", false},
		{"478 未完成", dispatchItem(478, "day", "随机云朵", "WAIT", 0), upstream.TaskClickKeyRandomCloud, true},
		{"478 已完成", dispatchItem(478, "day", "随机云朵", "FINISH", 0), "", false},
		{"别的 id 不补", dispatchItem(479, "day", "别的", "WAIT", 2), "", false},
	}
	for _, c := range cases {
		key, call := PlanVerifyClick(c.item)
		if call != c.wantCall || key != c.wantKey {
			t.Errorf("%s：应 (%q,%v)，实得 (%q,%v)", c.name, c.wantKey, c.wantCall, key, call)
		}
	}
}

// TestClassifyVerifiedTask：复查那四格 + 隐藏组 + 无 id 的两档。
func TestClassifyVerifiedTask(t *testing.T) {
	done := map[int64]bool{10: true, 11: true}
	cases := []struct {
		name string
		item upstream.TaskV2Item
		want TaskVerifyClass
	}{
		{"做了且认了", dispatchItem(10, "day", "a", "FINISH", 0), VerifyDoneNow},
		{"做了没认", dispatchItem(11, "day", "b", "WAIT", 0), VerifyFailed},
		{"没做已完成", dispatchItem(12, "day", "c", "FINISH", 0), VerifyAlreadyWas},
		{"没做也没完成", dispatchItem(13, "day", "d", "WAIT", 0), VerifyStillOpen},
		{"隐藏组", dispatchItem(14, "hidden", "e", "WAIT", 0), VerifyHidden},
		{"另一个隐藏组", dispatchItem(15, "hiddenabc", "f", "FINISH", 0), VerifyHidden},
	}
	for _, c := range cases {
		if got := ClassifyVerifiedTask(c.item, done); got != c.want {
			t.Errorf("%s：应 %v，实得 %v", c.name, c.want, got)
		}
	}
	// 无 id 的两档：不参与 `done` 判定，但仍要报"完成/未完成"。
	anonymous := dispatchItem(0, "day", "g", "WAIT", 0)
	anonymous.TaskID = nil
	if got := ClassifyVerifiedTask(anonymous, done); got != VerifyStillOpen {
		t.Errorf("无 id 未完成应 StillOpen，实得 %v", got)
	}
	anonymous.State = text("FINISH")
	if got := ClassifyVerifiedTask(anonymous, done); got != VerifyAlreadyWas {
		t.Errorf("无 id 已完成应 AlreadyWas，实得 %v", got)
	}
}

// TestClassifyVerifiedTaskDistinguishesFiredFromAcknowledged ⭐ 复查那四格的输入是"发过枪"，不是"上游认了"。
//
// 这一条钉的是**两半里的一半**：没发过枪的条目不该被念成"本次做完"（⇒ StillOpen），
// 而发过枪、复查仍显示未完成的落"做了没认"（⇒ VerifyFailed）而不是"仍待办"。
// 后一档与旧实现那处**刻意不同**：它把"过门却没发"的条目也记进 `doingList`，
// 于是复查会念成"本次成功"。
// ⚠ "发过枪但那一发失败"这一格不在这里 —— 它是执行层的事，
// 由 `TestTaskMatrixRunContinuesAfterClickFailure` 与
// `TestTaskMatrixRunEntryDeviceFailureDoesNotSinkTheRun` 钉（那个 `fired` 的赋值点在 [taskExecutor.apply]）。
func TestClassifyVerifiedTaskDistinguishesFiredFromAcknowledged(t *testing.T) {
	item := dispatchItem(20, "beiyong1", "被门挡着的那条", "WAIT", 6)
	if got := ClassifyVerifiedTask(item, map[int64]bool{}); got != VerifyStillOpen {
		t.Errorf("什么都没发 ⇒ 不该被念成本次做完，实得 %v", got)
	}
	if got := ClassifyVerifiedTask(item, map[int64]bool{20: true}); got != VerifyFailed {
		t.Errorf("发过枪而仍未完成 ⇒ 该落「做了没认」，实得 %v", got)
	}
}

// TestVerifyMarketLabel：复查句里"该去哪个 App"。
func TestVerifyMarketLabel(t *testing.T) {
	item := dispatchItem(21, "day", "h", "WAIT", 0)
	if got := VerifyMarketLabel(item); got != "云盘" {
		t.Errorf("sign_in_3 应念成云盘，实得 %q", got)
	}
	email := upstream.TaskListV2EmailMarketName
	item.MarketName = &email
	if got := VerifyMarketLabel(item); got != "邮箱" {
		t.Errorf("newsign_139mail 应念成邮箱，实得 %q", got)
	}
	item.MarketName = nil
	if got := VerifyMarketLabel(item); got != "云盘" {
		t.Errorf("marketname 缺席时该落在云盘（旧实现那侧的默认文案），实得 %q", got)
	}
}

// TestShareProgress：434 那句"第几天"靠名字里的 `N/7`。
func TestShareProgress(t *testing.T) {
	if n, ok := ShareProgress("分享文件有好礼（3/7）"); !ok || n != 3 {
		t.Errorf("应读出 3，实得 %d/%v", n, ok)
	}
	if _, ok := ShareProgress("分享文件有好礼"); ok {
		t.Error("名字里没有那个式子时不该硬凑一个数")
	}
	// 分母固定 7：`2/8` 不该被读成 2（旧实现那句正则就是这个形状）。
	if _, ok := ShareProgress("进度 2/8"); ok {
		t.Error("分母不是 7 的不应命中")
	}
}

// TestTaskStepKindString：五种处置都得有人话名（它直接进结论与界面）。
func TestTaskStepKindString(t *testing.T) {
	for _, k := range []TaskStepKind{StepClick, StepRunner, StepBlockedToolGap, StepBlockedByRule, StepNeedsHuman} {
		if got := k.String(); got == "" || got == "未知处置" {
			t.Errorf("处置 %d 没有人话名（界面上会是空串）", k)
		}
	}
	if TaskStepKind(99).String() != "未知处置" {
		t.Error("越界值应落在未知处置，而不是静默空串")
	}
}
