package upstream

// `taskListV3` 的解析用例。
//
// 字段值取自**实拍夹具**（`test/fixtures/http/每月阅读任务.json`，两条同端点请求，
// 响应 18 条任务），只留本层声明的那六个字段 + 顶层 `taskFinishState`。
// 钉的是"形状与字段名"，不钉全部 18 条 —— 条数由账号投放决定，会漂。

import (
	"strings"
	"testing"
)

// taskListFixture 是一条按实拍形状搭出来的响应。
//
// ⚠ 刻意**保留**那些本层不读的键（`button` / `icon` / `steps` / `prov` …）：
// 解析器要能容忍"上游给的比我们要的多"，把夹具削成只剩六个字段就等于没测这一条。
func taskListFixture() map[string]any {
	return map[string]any{
		"code": 0, "msg": "success",
		"taskFinishState": map[string]any{"taskFinishState": 1},
		"result": []any{
			map[string]any{
				"id": 120, "name": "30G流量0元领", "description": "上传下载免流量",
				"groupid": "time", "state": "WAIT", "limitType": "EVER",
				"enable": 1, "sort": 20, "process": 0, "currstep": 1, "currDay": 0,
				"recordid": 19930214509, "taskType": 0, "prov": "20;21;22",
				"icon": "https://img.mcloud.139.com//uop_prod/20260730/x.png",
				"button": map[string]any{
					"app": map[string]any{"text": "去查看", "action": "jump"},
				},
				"steps": []any{}, "stepTypeSet": []any{"click"},
			},
			map[string]any{
				"id": 596, "name": "珍藏童真时刻",
				"groupid": "beiyong1", "state": "FINISH", "limitType": "EVER",
				"process": 0, "currstep": 4, // ⚠ 已完成条目 process 仍是 0 —— 见 TaskItem 那条注释
				"steps": []any{},
			},
			map[string]any{
				"id": 402, "name": "开启APP通知", "description": "开启移动云盘APP消息通知",
				"groupid": "month", "state": "WAIT", "limitType": "MONTH",
			},
			// 上游"有时不给"那一类：没有 `state`、没有 `limitType`、也没有 `description`。
			map[string]any{"id": 700, "name": "缺字段的条目"},
		},
	}
}

func TestParseTaskListReadsTheCapturedShape(t *testing.T) {
	list, err := ParseTaskList(envelopeFrom(t, taskListFixture()))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if len(list.Items) != 4 {
		t.Fatalf("应解出 4 条，实得 %d", len(list.Items))
	}
	first := list.Items[0]
	if first.TaskID == nil || *first.TaskID != 120 {
		t.Errorf("id 应解成整数 120，实得 %v", first.TaskID)
	}
	if first.Name == nil || *first.Name != "30G流量0元领" {
		t.Errorf("name 应原样带出，实得 %v", first.Name)
	}
	if first.GroupID == nil || *first.GroupID != "time" {
		t.Errorf("groupid（**全小写**）应读出，实得 %v", first.GroupID)
	}
	if first.State == nil || *first.State != TaskStateWait {
		t.Errorf("state 应读出 WAIT，实得 %v", first.State)
	}
	if first.Description == nil || *first.Description != "上传下载免流量" {
		t.Errorf("description 应读出，实得 %v", first.Description)
	}
	if list.Items[2].LimitType == nil || *list.Items[2].LimitType != TaskLimitMonth {
		t.Errorf("limitType 应读出 MONTH，实得 %v", list.Items[2].LimitType)
	}
	// 顶层那个同名嵌套（`taskFinishState.taskFinishState`）与 result **平级**，
	// 只能从 Raw 取 —— 解错层这里会静默变 nil。
	if list.FinishState == nil || *list.FinishState != 1 {
		t.Errorf("顶层 taskFinishState.taskFinishState 应是 1，实得 %v", list.FinishState)
	}
}

// 缺字段的条目**不能**把整张清单打成解析错误：上游本来就是"有时给有时不给"，
// 而"这一条没给状态"与"这一条不存在"是两个答案 —— 前者要能在计数里被看见。
func TestParseTaskListKeepsItemsWithoutOptionalFields(t *testing.T) {
	list, err := ParseTaskList(envelopeFrom(t, taskListFixture()))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	last := list.Items[3]
	if last.State != nil {
		t.Errorf("没有 state 时必须是 nil，不能是空串：实得 %q", *last.State)
	}
	if last.LimitType != nil {
		t.Errorf("没有 limitType 时必须是 nil，实得 %q", *last.LimitType)
	}
	if last.Description != nil {
		t.Errorf("没有 description 时必须是 nil，实得 %q", *last.Description)
	}
	if last.TaskID == nil || *last.TaskID != 700 {
		t.Errorf("id 仍要读出，实得 %v", last.TaskID)
	}
}

// `result` 是**数组**。哪天上游改成对象，要在这里立刻响，
// 而不是"顺手兼容两种形状"—— 兼容会把形状变化藏起来。
func TestParseTaskListExpectsAnArray(t *testing.T) {
	_, err := ParseTaskList(envelopeFrom(t, map[string]any{
		"code": 0, "msg": "success", "result": map[string]any{"id": 1},
	}))
	if err == nil || !strings.Contains(err.Error(), "不是数组") {
		t.Errorf("result 是对象时应报「不是数组」，实得 %v", err)
	}
}

// 业务码非 0 时**不解析** result：这一条的错误要带上游的诊断提示（走 [EnsureOKWithHint]）。
func TestParseTaskListPropagatesBusinessCode(t *testing.T) {
	_, err := ParseTaskList(envelopeFrom(t, map[string]any{
		"code": 90001, "msg": "未登录",
	}))
	if err == nil {
		t.Fatal("90001 应报错")
	}
	if !strings.Contains(err.Error(), "未登录") {
		t.Errorf("上游原文要一路带到错误里，实得 %q", err.Error())
	}
}

// 顶层没有 `taskFinishState`（夹具里 `group` 为空的那两次就是 `{}` 或干脆没有）
// ⇒ nil，且**不影响**任务数组照解。
func TestParseTaskListToleratesMissingFinishState(t *testing.T) {
	raw := taskListFixture()
	delete(raw, "taskFinishState")
	list, err := ParseTaskList(envelopeFrom(t, raw))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if list.FinishState != nil {
		t.Errorf("缺顶层字段时应是 nil，实得 %v", *list.FinishState)
	}
	if len(list.Items) != 4 {
		t.Errorf("任务条数不该受影响，实得 %d", len(list.Items))
	}

	// 嵌套那一层形状不对（给成数组）时同样只降级成 nil。
	raw["taskFinishState"] = []any{1}
	list, err = ParseTaskList(envelopeFrom(t, raw))
	if err != nil {
		t.Fatalf("解析失败: %v", err)
	}
	if list.FinishState != nil {
		t.Errorf("嵌套形状不对时应是 nil，实得 %v", *list.FinishState)
	}
}
