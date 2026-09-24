// 任务清单族：`taskListV3`（签到页那张任务表）。
//
// 原来这条挂在 `app_signin_signin.go` 里，解析结果是**原样的 `[]any`** ——
// 那是"还没有人读它"时的临时态。现在它要做成一条能力（界面上"看一眼有哪些任务没做"），
// 所以要按实拍字段声明，并和 `infoV3` 那族一样给出**类型化的解析**。
//
// # 证据（两处，形状一致）
//
//   - 抓包：`docs/capture/app-signin-samples.txt` §3.3 —— body
//     `{"marketname":"sign_in_3","client":1,"clientVersion":"12.6.0"}`。
//   - 夹具：`test/fixtures/http/每月阅读任务.json` 里两条
//     `POST m.mcloud.139.com/ycloud/signin/task/taskListV3`，同一份 body（**没有 `client`**），
//     响应 18 条任务，`state` 只出现 `WAIT`(10) / `FINISH`(8)，
//     `groupid` 有 `time/month/hiddenabc/cloudEmail/hidden/day`，
//     `limitType` 有 `EVER/MONTH/DAY`，顶层另有 `taskFinishState.taskFinishState = 1`。
//
// ⚠ **两处 body 的差别**（有 `client` / 没有 `client`）没有解释 ⇒ 已登记成交用户确认的问题。
// 这里按**抓包那份**（带 `client`）实现：抓包是我们自己这一端真实发出的请求，
// 夹具是另一台设备上的客户端发的，两者都可能对，但抓包是"我们正在冒充的那个端"。
package upstream

import (
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// TaskListV3Request 构造 `POST /ycloud/signin/task/taskListV3` —— 任务列表。
//
// `client`：`1` = App/H5 已登录（真实客户端），`0` = 匿名。两者都能返回数据，
// 但字段取值会变（如 `button.canReceive`），所以做成参数而不是常量。
func TaskListV3Request(jwt, referer string, client uint8) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"client":        client,
		"clientVersion": TaskClientVersion,
		"marketname":    MarketName,
	}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/signin/task/taskListV3", BaseMcloud)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithH5Headers(request, referer, &jwt, "true"), nil
}

// `state` 的取值 —— 实拍 18 条里**只出现这两个**（不是枚举全集，是观测集）。
//
// ⚠ 解析时**不归一**成布尔："既不是 WAIT 也不是 FINISH"与"上游没给 `state`"
// 是两种需要分开说的答案（后者在 [TaskList] 的统计里进"状态未知"那一档）。
const (
	// TaskStateWait 是**未完成**。
	TaskStateWait = "WAIT"
	// TaskStateFinish 是**已完成**。
	TaskStateFinish = "FINISH"
)

// `limitType` 的取值 —— 实拍三个（同样只算观测集）。
//
// 它是"这条任务多久能做一次"，界面上按它分组用。
const (
	// TaskLimitDay 是每日任务。
	TaskLimitDay = "DAY"
	// TaskLimitMonth 是每月任务。
	TaskLimitMonth = "MONTH"
	// TaskLimitEver 是**一次性**任务（不是"永久可重复"—— 名字有误导）。
	TaskLimitEver = "EVER"
)

// TaskItem 是任务表里的一条。
//
// # 为什么只声明这六个字段
//
// 实拍一条有 26 个键（`button` / `buttonProperty` / `icon` / `steps` / `stepTypeSet` /
// `prov` / `whiteList` / `lastupdate` …），其中大半是**渲染与投放**用的：
// 链接、图标、按钮文案、省份白名单。本能力回答的问题是"有哪些任务、做没做"，
// 所以只取身份（`id` / `name`）、说明（`description`）、归类（`groupid` / `limitType`）、
// 状态（`state`）。
//
// ⚠ **没有**声明 `process` / `currstep` / `currDay` / `recordid`：
// 实拍里已完成的 `FINISH` 条目 `process` **仍是 0**（`{"state":"FINISH","process":0,"currstep":4}`），
// 所以"进度"这个口径在这几个字段上**说不通** —— 到底该读哪一个未验证。
// 等要做"按步数判断"那条链（`doTaskPost` 一族）时再回来按实拍补，并且要先补一条样本。
type TaskItem struct {
	// TaskID 是 `id` —— 任务 id。做任务上报（`doTaskPost` / `receiveTask?taskId=`）时要回传。
	TaskID *int64
	// Name 是 `name` —— 任务名（列表主文案）。
	Name *string
	// Description 是 `description` —— 一句话说明，常常写着奖励
	// （实拍有 `"+80云朵"`、`"开启备份，云端无损存储"`）。
	Description *string
	// GroupID 是 `groupid` —— 分组（实拍 `time` / `month` / `day` / `hidden` …）。
	//
	// ⚠ `hidden` 与 `hiddenabc` 这两组**也出现在响应里**。旧项目有没有按它过滤，
	// 本机没有旧源码可查 ⇒ 界面上要不要显示它们**待验证**；这里只照实交出去。
	GroupID *string
	// State 是 `state` —— 见 [TaskStateWait] / [TaskStateFinish]。
	State *string
	// LimitType 是 `limitType` —— 见 [TaskLimitDay] 那三档。
	LimitType *string
}

// TaskList 是 `taskListV3` 的解析结果。
type TaskList struct {
	// Items 是 `result` —— 任务数组。
	Items []TaskItem
	// FinishState 是顶层 `taskFinishState.taskFinishState`（实拍 `1`）。
	//
	// ⚠ **只存不判**：实拍那一条里它是 `1`，而同一份 `result` 里还有 10 条 `WAIT` ——
	// 所以它**不是**"全部任务都完成了"。到底是"当日任务完成度"还是别的口径，
	// 一个观测点定不下来 ⇒ 未验证，别拿它做判据。
	FinishState *int64
}

// ParseTaskList 解析 `taskListV3` 的响应。
//
// ⚠ `result` 必须是**数组**（这一条不带 `records` 那一层）。上游哪天改成对象，
// 要在这里立刻响，而不是"顺手兼容两种形状"—— 兼容会把形状变化藏起来。
func ParseTaskList(envelope *Envelope) (TaskList, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return TaskList{}, perr
	}
	items, ok := payload.([]any)
	if !ok {
		return TaskList{}, core.NewParse("taskListV3 的 result 不是数组（该接口不带信封字段，只有 result）")
	}

	out := TaskList{Items: make([]TaskItem, 0, len(items))}
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		out.Items = append(out.Items, TaskItem{
			TaskID:      optInt64Field(item, "id"),
			Name:        optStrField(item, "name"),
			Description: optStrField(item, "description"),
			GroupID:     optStrField(item, "groupid"),
			State:       optStrField(item, "state"),
			LimitType:   optStrField(item, "limitType"),
		})
	}
	// 顶层那个 `taskFinishState` 是**对象套对象**（`{"taskFinishState":1}`），
	// 与 `result` 平级 —— 所以只能从 [Envelope.Raw] 取，载荷里没有它。
	out.FinishState = nestedFinishState(envelope.Raw())
	return out, nil
}

// nestedFinishState 取 `taskFinishState.taskFinishState`；任一层缺失给 nil。
//
// 少一层就报错的代价是"任务清单看不成了"，而它本身**不参与任何判据**（见
// [TaskList.FinishState]）—— 所以它是可选字段。
func nestedFinishState(raw any) *int64 {
	obj, ok := raw.(map[string]any)
	if !ok {
		return nil
	}
	inner, ok := obj["taskFinishState"].(map[string]any)
	if !ok {
		return nil
	}
	if value, ok := asInt64(inner["taskFinishState"]); ok {
		return &value
	}
	return nil
}
