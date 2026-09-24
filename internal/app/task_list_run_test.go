package app

// [TaskListRun] 的用例：只查不做、计数分档、分组小结。
//
// 这一层要钉住的是三件"抄一遍就会漂移"的事：
//
//  1. **只打查询那一口** —— 这条能力的边界就是"看"。同页那几个写端点
//     （`doTaskPost` / `receiveTask`）一旦在这里被顺手带上，
//     表现是"查一下任务清单，账号的任务被偷偷做掉了"，而界面文案还写着"只读"。
//  2. **状态三档分开**（未完成 / 已完成 / 未知）—— 把"没给状态"并进任一档，
//     都是一句凭空多出来的结论（并见 `app_task_list.go` 里那段"为什么不归一大小写"）。
//  3. **查询失败交回错误**，不自己折成失败结论 —— 编排层那条路才会多发一条
//     `errored` 事件、并把上游原文写进 `runs.message`。

import (
	"context"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

const taskListEndpoint = "/ycloud/signin/task/taskListV3"

// taskListBody 是响应原文。条目取自实拍夹具（见 `app_task_list_test.go` 抬头），
// 四档各一条：WAIT / FINISH / 另一个分组的 WAIT / 没有 state。
const taskListBody = `{"code":0,"msg":"success","taskFinishState":{"taskFinishState":1},"result":[` +
	`{"id":120,"name":"30G流量0元领","description":"上传下载免流量","groupid":"time","state":"WAIT","limitType":"EVER"},` +
	`{"id":596,"name":"珍藏童真时刻","groupid":"beiyong1","state":"FINISH","limitType":"EVER"},` +
	`{"id":402,"name":"开启APP通知","description":"开启移动云盘APP消息通知","groupid":"month","state":"WAIT","limitType":"MONTH"},` +
	`{"id":700,"name":"缺字段的条目"}]}`

func taskListRun(routes ...liveTaskRoute) (TaskListRun, *liveTaskScript) {
	script := liveTaskScriptWith(append(sessionRoutes(), routes...)...)
	return TaskListRun{SignInWorld: signInWorldFor(&poolSignInStore{*liveTaskStore()}, script)}, script
}

// TestTaskListRunOnlyReads：⭐ 只打 `taskListV3` 一次，写端点一次都不打。
func TestTaskListRunOnlyReads(t *testing.T) {
	run, script := taskListRun(liveTaskRoute{taskListEndpoint, []string{taskListBody}})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count(taskListEndpoint); n != 1 {
		t.Errorf("taskListV3 应被打 1 次，实得 %d", n)
	}
	// 这三个是"把任务做掉 / 领掉"那一族。它们出现在这条只读链上，
	// 后果不是报错而是**账号状态被改了**，所以逐点数一遍。
	for _, forbidden := range []string{"doTaskPost", "receiveTask", "openEmailsms/reward"} {
		if n := script.count(forbidden); n != 0 {
			t.Errorf("只读链不该打 %s，实得 %d 次", forbidden, n)
		}
	}
	if len(res.Items) != 4 {
		t.Fatalf("应带出 4 条任务，实得 %d", len(res.Items))
	}
	if res.FinishState == nil || *res.FinishState != 1 {
		t.Errorf("顶层 taskFinishState 要原样带出（只存不判），实得 %v", res.FinishState)
	}
	counts := res.Counts()
	if counts != (TaskListCounts{Total: 4, Wait: 2, Finished: 1, Unknown: 1}) {
		t.Errorf("三档计数漂移：实得 %+v", counts)
	}
	report := res.RunReport()
	if report.Outcome != core.OutcomeSuccess {
		t.Errorf("只读链恒成功，实得 %v", report.Outcome)
	}
	if report.ManualSteps == nil {
		t.Error("ManualSteps 必须是空 slice 而不是 nil（出网形状契约）")
	}
}

// TestTaskListSummaryLines：小结逐字。
//
// ⚠ 这四句会进 `runs.message` 与推送标题，改一个字就算界面漂移。
func TestTaskListSummaryLines(t *testing.T) {
	item := func(id int64, group, state string) upstream.TaskItem {
		out := upstream.TaskItem{}
		g, s := group, state
		out.TaskID = &id
		out.GroupID = &g
		if state != "" {
			out.State = &s
		}
		return out
	}
	cases := []struct {
		name  string
		items []upstream.TaskItem
		want  string
	}{
		{"空清单", nil, "任务清单是空的"},
		{
			"两档齐全并带分组",
			[]upstream.TaskItem{item(1, "time", "WAIT"), item(2, "beiyong1", "FINISH"), item(3, "month", "WAIT")},
			"任务 3 条：未完成 2、已完成 1；分组 month 1、time 1",
		},
		{
			"没有 state 的那条单列一档，不并进未完成",
			[]upstream.TaskItem{item(1, "time", ""), item(2, "time", "WAIT")},
			"任务 2 条：未完成 1、已完成 0、状态未知 1；分组 time 1",
		},
		{
			// 实拍一个账号就有 6 个分组（time/month/hiddenabc/cloudEmail/hidden/day）。
			// 不截会把一条通知变成一段清单（与 [PrizesResult.SummaryLine] 同一条理由）。
			"分组多于三个就截断",
			[]upstream.TaskItem{
				item(1, "cloudEmail", "WAIT"), item(2, "day", "WAIT"),
				item(3, "hidden", "WAIT"), item(4, "month", "WAIT"), item(5, "time", "WAIT"),
			},
			"任务 5 条：未完成 5、已完成 0；分组 cloudEmail 1、day 1、hidden 1…（另有 2 组）",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			res := TaskListResult{Account: core.NewAccountID(liveTestAccountID), Items: c.items}
			if got := res.SummaryLine(); got != c.want {
				t.Errorf("实得 %q\n期望 %q", got, c.want)
			}
		})
	}
}

// TestTaskListRunReturnsTheQueryError：上游业务失败 ⇒ 交回错误，不给"成功但空清单"。
//
// 反向改坏它（把错误折成空结果）会让界面上出现一句"任务清单是空的" ——
// 那是"没查成"被念成"查过了、没有任务"，比报错更糟。
func TestTaskListRunReturnsTheQueryError(t *testing.T) {
	run, _ := taskListRun(liveTaskRoute{taskListEndpoint, []string{
		`{"code":1,"msg":"系统繁忙，请稍后重试"}`,
	}})
	res, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID))
	if perr == nil {
		t.Fatal("查询失败必须交回错误")
	}
	if !strings.Contains(perr.Error(), "系统繁忙") {
		t.Errorf("上游原文要一路带到错误里，实得 %q", perr.Error())
	}
	if len(res.Items) != 0 || res.FinishState != nil {
		t.Errorf("失败时结论必须是零值，不能带上一次尝试的残值：实得 %+v", res)
	}
}

// TestTaskListRunReusesTheSharedSession：这条链**复用**签到页那一条会话，
// 不自建第二套建会话流程（AGENTS §1.12：票 = 账号 × 页面）。
func TestTaskListRunReusesTheSharedSession(t *testing.T) {
	run, script := taskListRun(liveTaskRoute{taskListEndpoint, []string{taskListBody}})
	if _, perr := run.RunAccount(context.Background(), core.NewAccountID(liveTestAccountID)); perr != nil {
		t.Fatalf("跑一趟失败: %v", perr)
	}
	if n := script.count("tyrzLogin"); n != 1 {
		t.Errorf("建会话应恰好一次，实得 %d", n)
	}
}
