// 任务清单族第二口：`taskListV2`（**App 任务矩阵**那张表，按 group 分次拉取）。
//
// # 为什么与 `taskListV3` 分两份实现，而不是"同一件事换个版本号"
//
// 两口的 **body 与响应形状都不同**（AGENTS §1.12 事实②那种"URL 相近而端点不同"）：
//
//   - `taskListV3`（`app_task_list.go`）：body 多一枚 `client`，**没有** `group`，
//     响应的 `result` 是**数组**，顶层另有 `taskFinishState.taskFinishState`。
//   - 这一口：body 多一枚 `group`、**没有** `client`，响应的 `result` 是
//     **以所请求的 group 名为键的对象**（值为数组），顶层另有 `groupState.<group>.taskFinishState`。
//
// 把两者折成一个解析器的表现是：哪天 `result` 从数组变成对象，兼容代码会"顺手"两种都吃，
// 于是这条链在形状变化上**不会响**。所以各写一份，宁可重复八行取字段。
//
// # 证据（本仓夹具，两条同端点实拍）
//
// `test/fixtures/http/3d.json` 里两条 `POST m.mcloud.139.com/ycloud/signin/task/taskListV2`：
//
//   - body 逐字 `{"clientVersion":"12.6.0","group":"new","marketname":"sign_in_3"}` ⇒
//     响应 `{"code":0,"msg":"success","result":{},"groupState":{}}` —— **`result` 是空对象，不是空数组**；
//   - body 逐字 `{"clientVersion":"12.6.0","group":"beiyong1","marketname":"sign_in_3"}` ⇒
//     响应 `result.beiyong1` 三条（id 596/597/598，`state` 全 `FINISH`、`enable` 全 1、
//     `process` 全 0、`currstep` 分别 4/6/8），顶层 `groupState = {"beiyong1":{"taskFinishState":2}}`。
//
// 请求头面（同一份夹具）：`content-type: application/json;charset=UTF-8`、`appversion`、
// `activityid`、`jwttoken`、`deviceid`、`showloading`、`origin: https://m.mcloud.139.com` ——
// 后四个的**值**在夹具里是 `<REDACTED>`，所以这一层只钉"发出去了这些头名"，
// 不钉那几枚的取值（`appversion` / `activityid` 的取值出处见下）。
//
// # `appversion` 与 `activityid` 的取值出处
//
// 旧 TS `packages/core/src/tasks/app/helpers.ts:48-63` 每次 POST 都带
// `headers: { appversion: "12.6.0.0", activityid: <本次的 marketname> }`。
// ⚠ **`activityid` 跟着 `marketname` 走**，不是恒等于签到页那个 `sign_in_3` ——
// 所以这里在 [WithH5Headers]（它固定写 [MarketName]）之后**先剥再补**，
// 而不是"再 append 一枚同名头"（`core.WithHeader` 是追加，见 traits.go 那条"必须拷贝"的注释）。
//
// 两个市场名（旧 TS 同一份 builder 调两次）：`sign_in_3`（[MarketName]）与
// `newsign_139mail`（[TaskListV2EmailMarketName]）。
// ⚠ 旧 TS 还有**另一份** `taskListV2` 封装（`packages/core/src/apis/task.ts:37-46`）：
// 域名是 `caiyun.feixin.10086.cn:7071`、`clientVersion` 硬编码 `"11.2.1"`、只请求 `group:"month"`
// 一次、**不带**那两枚头，用途是"通知领云朵"那条链里找 id `1021` —— 那是**另一条链**，
// 不在本文件（它属于 #12 `notice`，而实拍里那一环**没有出现**，见 `notice_run.go` 文件头）。
package upstream

import (
	"context"
	"fmt"
	"sort"

	"github.com/catlair/mcloud/internal/core"
)

// TaskListV2EmailMarketName 是这一口的第二个市场名（邮箱那份任务表）。
//
// 旧 TS 用 `getAppTaskList(ctx, "sign_in_3" | "newsign_139mail")` 联合类型把它钉成两值，
// 本层不枚举成"只准这两个"（上游哪天加第三个市场名时，这里不该成为阻塞点），
// 但常量必须成文，因为 `activityid` 要跟着它变。
const TaskListV2EmailMarketName = "newsign_139mail"

// TaskListV2Groups 是旧实现按序请求的八个 group（`helpers.ts:44` 的字面量与顺序）。
//
// ⚠ 两处"类型与事实不一致"留在旧 TS 里，这里按**请求面**那一份为准：
// `hiddenabc` 出现在旧 TS 的 `GroupId` 类型与结果分组文案里，却**不在**这个数组中（从不请求）；
// `beiyong2` 在这个数组里，却**不在** `GroupId` 类型里。⇒ 以数组为准。
//
// ⚠ 顺序**是**行为的一部分：旧实现串行请求，同一 group 失败就跳过，
// 合并时先来的排在前面（去重按 `name` 保留排在前面的那条，见 `app` 层那份聚合）。
var TaskListV2Groups = []string{
	"new", "beiyong1", "beiyong2", "cloudEmail", "time", "day", "month", "hidden",
}

// TaskListV2Request 构造 `POST /ycloud/signin/task/taskListV2` —— **只读**。
//
// body 三键逐字：`marketname`、`clientVersion`（[TaskClientVersion]，三段 `12.6.0`）、`group`。
// ⚠ **没有** `client` —— 那是 V3 那一口的键；也别把 `clientVersion` 写成四段
// （四段那个是请求头 `appversion`，[H5AppVersion]，旧实现里两个拼法同时出现）。
func TaskListV2Request(jwt, referer, marketname, group string) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"marketname":    marketname,
		"clientVersion": TaskClientVersion,
		"group":         group,
	}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/signin/task/taskListV2", BaseMcloud)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = WithH5Headers(request, referer, &jwt, "true")
	// `activityid` 跟着本次的 marketname 走：先剥掉 WithH5Headers 里那枚固定的 `sign_in_3`，
	// 再补上这一支的。剥+补而不是再 append —— 同名两枚头会让"哪枚生效"变成传输层的事。
	request.Headers = dropHeader(request.Headers, "activityid")
	return request.WithHeader("activityid", marketname), nil
}

// TaskV2Item 是 `taskListV2` 响应里的一条任务。
//
// 前六个字段直接复用 [TaskItem]（两口的条目**是同一套 26 键**，实拍逐条比对过键集合）。
// 后面这四个是**这一口才读**的：旧 TS 的分发器要用 `enable !== 1` 与 `state === "FINISH"`
// 当"要不要做"的门，用 `currstep` 决定点击上报带哪一步，用 `marketname` 区分云盘/邮箱两侧。
//
// ⚠ 夹具那三条的 `process` **恒为 0**，而同三条的 `state` 已经是 `FINISH`、`currstep` 是 4/6/8
// ⇒ "进度到底读哪个字段"这一格**未验证**（与 `app_task_list.go` 里 [TaskItem] 那条注释同一个洞）。
// ⚠ "恒为 0" 只是**那一份夹具**的形状，不是这一格的性质：2026-09-25 两个真账号现场读到过
// `0 / 9 / 59 / 64 / 69 / 73 / 74 / 100`，且同一条在五遍拉表之间走出 `59 → 64 → 64 → 69 → 73`，
// 另一号同一条读到 `100` 而它当天念"已完成" ⇒ "完成度百分比"这一读法有了两组独立数
// （见 `docs/status.md`「同日第五趟 / 第六趟 / 第七趟真跑」）。
// ⚠ 但**仍是相关性**，而且步长不固定：522 一趟最多传 5 个（本仓那个常量），第六、七两趟都传满
// 5 个，第七趟传完之后复查读到 `69 → 73`（去重留下的那一份）与 `74`（另一市场名给的同一组里那一枚）
// ⇒ "每传一个涨一格"这一句**没被读数支持**。涨的是这一趟我们自己上传造成的、还是同一趟里别的动作，
// 逐条对照那一行今天能点名到"只有 522 这一条动了"，但"是几比几"仍没有上游声明。
// 2026-09-24 起它有**一个**消费者：app 层 522（循环上传）照旧实现那句
// `task.process >= 100` 读它当停表 ⇒ 不再是"纯交出不判"。⚠ 但那是**照抄旧实现的读法**，
// 不等于这一格的语义验过（仍登记在 `docs/pending-decisions.md` R15）。
type TaskV2Item struct {
	TaskItem
	// Enable 是 `enable` —— 旧实现的门是 `enable !== 1` ⇒ 跳过。
	Enable *int64
	// CurrStep 是 `currstep` —— 旧实现把它带进 `task/click` 的 `step` 参数。
	CurrStep *int64
	// Process 是 `process` —— 见上面那段：夹具里恒 0，但真账号现场读到过
	// `0/9/59/64/69/73/74/100`，同一条五遍之间 `59→64→64→69→73`（⚠ 同一趟里两个市场名对
	// 同一组给出 73 与 74 两种面值 ⇒ 涨几格取决于读哪一边），涨到 100 那条当天
	// 就是 `FINISH` ⇒ "会动"与"像完成度百分比"都有两组数，仍按**相关性**对待（R15）。
	Process *int64
	// MarketName 是 `marketname` —— 实拍 `sign_in_3`。
	// ⚠ 2026-09-25 真跑答出这一格（R13 / R14）：邮箱那一支（`newsign_139mail`）**回数据、且同形**，
	// 但它回来的条目里 `marketname` **混着**给 `newsign_139mail` 与 `sign_in_3` ⇒ 两边不是干净的两批表。
	MarketName *string
}

// TaskListV2 是**一次**（一个 group 的）`taskListV2` 响应的解析结果。
type TaskListV2 struct {
	// Group 是**请求时**要的那个 group 名 —— 解析器不猜它，由调用方传进来。
	//
	// 为什么需要：实拍 `result` 这个对象里只有本次那一个键，但"只有一个键"是
	// 一次观测、不是契约（旧 TS 是把 `Object.values(result)` 全摊平，不看键名）。
	// 所以这里同样**全摊平**，而把请求的那名留着做日志与对账。
	Group string
	// Items 是把 `result` 的**所有**值摊平后的条目（键名不参与筛选）。
	Items []TaskV2Item
	// GroupFinishStates 是顶层 `groupState.<group>.taskFinishState`，按 group 存。
	//
	// ⚠ **只存不判**（这一条没变）：夹具里一次是 `{}`（group `new`）、一次是
	// `{"beiyong1":{"taskFinishState":2}}`（同一份响应里三条**全是** `FINISH`）
	// ⇒ 它不是"完成/未完成"那种二值口径。
	//
	// 2026-09-25 两个真账号各一趟、每趟拉表两遍，把这一格**逐组念出来**之后读到的是三档
	// （`0` 只配"该组一条都没完成"、`1` 只配"部分完成"、`2` 只配"全部完成"，
	// 18 次读数 / 14 种组合零例外）⇒ 它**读起来像"组内完成度"**。
	// ⚠ 那是**同日两号的相关性观察、不是因果**：没有把某一组从 0 推到 2 再看它跟不跟的补样，
	// 所以判据面照旧不读它（`docs/pending-decisions.md` R15、`docs/status.md`「同日第五趟真跑」）。
	//
	// ⚠ 键名：那十六次里每次响应只给**一枚键、且与请求的组名相同** ⇒ "键名会不会不一致"
	// 今天**没有反例、也没验过**（不写"不会"）。调用方因此**不按键名筛选**（见 [TaskListV2.Group]）。
	GroupFinishStates map[string]*int64
}

// ParseTaskListV2 解析一次 `taskListV2` 响应。
//
// `group` 只用于回填 [TaskListV2.Group]（见上）。
//
// ⚠ 与 V3 相反：这一口的 `result` 必须是**对象**。实拍里有两种合法形态 ——
// `result: {}`（这一组今天没有任务）与 `result: {"<group>": [...]}`。
// "`result` 键整个缺席"同样按空组处理：实拍那次 `new` 给的就是空对象，
// 而把"上游没给这一组"判成解析失败的代价是**整条聚合链**被一次空响应打断。
func ParseTaskListV2(envelope *Envelope, group string) (TaskListV2, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return TaskListV2{}, perr
	}
	out := TaskListV2{Group: group}
	// `result` 缺席 ⇒ 空组；存在 ⇒ 必须是对象（数组在这里就是"这不是这一口"）。
	var groups map[string]any
	root, ok := payload.(map[string]any)
	if ok {
		groups = root
	} else if payload != nil {
		return TaskListV2{}, core.NewParse(fmt.Sprintf(
			"taskListV2 的 result 不是按 group 分组的对象（实得 %T；这一口与 V3 不同形，别混解）", payload))
	}
	for _, name := range sortedResultGroupNames(groups) {
		raw := groups[name]
		items, ok := raw.([]any)
		if !ok {
			return TaskListV2{}, core.NewParse(fmt.Sprintf(
				"taskListV2 的 result.%s 不是数组，实得 %T", name, raw))
		}
		for _, rawItem := range items {
			item, _ := rawItem.(map[string]any)
			out.Items = append(out.Items, TaskV2Item{
				TaskItem: TaskItem{
					TaskID:      optInt64Field(item, "id"),
					Name:        optStrField(item, "name"),
					Description: optStrField(item, "description"),
					GroupID:     optStrField(item, "groupid"),
					State:       optStrField(item, "state"),
					LimitType:   optStrField(item, "limitType"),
				},
				Enable:     optInt64Field(item, "enable"),
				CurrStep:   optInt64Field(item, "currstep"),
				Process:    optInt64Field(item, "process"),
				MarketName: optStrField(item, "marketname"),
			})
		}
	}
	out.GroupFinishStates = parseGroupState(envelope.Raw())
	return out, nil
}

// sortedResultGroupNames 把 `result` 的键按字典序排出来。
//
// 存在的理由是**Go 的 map 遍历顺序是随机的**，而旧 TS 的 `Object.values()` 是插入序。
// 不排序的表现是：同一份响应两次解析给出不同顺序的条目，而下游"按 `name` 去重、
// 保留先来的那条"这件事就变成每次跑不一样 —— 测试要么偶发红，要么得写成"任意顺序都对"，
// 于是那条真正的判据（去重保留谁）根本没被钉住。
//
// ⚠ 这**不等于**复现上游的返回顺序（Go 的 `map[string]any` 在解码那一刻就已经丢了它）。
// 真要按上游顺序处理条目，得先把解析改成有序结构 —— 今天没有这种需求，所以这里只求确定性。
func sortedResultGroupNames(groups map[string]any) []string {
	keys := make([]string, 0, len(groups))
	for name := range groups {
		keys = append(keys, name)
	}
	sort.Strings(keys)
	return keys
}

// parseGroupState 取顶层 `groupState.<group>.taskFinishState`；任一层缺失就跳过那一格。
//
// 与 [nestedFinishState] 同理：它不参与判据，缺了不该让整次读取失败。
// ⚠ 返回的 map 对"没给 groupState"这一格是**空 map 而非 nil** —— 调用方按"没有这一组"读，
// 不要按"上游说不完整"读。
func parseGroupState(raw any) map[string]*int64 {
	out := map[string]*int64{}
	obj, ok := raw.(map[string]any)
	if !ok {
		return out
	}
	groups, ok := obj["groupState"].(map[string]any)
	if !ok {
		return out
	}
	for name, value := range groups {
		entry, ok := value.(map[string]any)
		if !ok {
			continue
		}
		if state, ok := asInt64(entry["taskFinishState"]); ok {
			value := state
			out[name] = &value
		}
	}
	return out
}

// AppTaskListV2 取**一个 group** 的任务表（`taskListV2`）。**只读**。
//
// 一次只请求一个 group 是这一口的形状（body 里就一枚 `group`），
// "那张表"是**十六次**的聚合结果 ⇒ 聚合不在本层（上游层只管一条请求一个响应），
// 在 `internal/app/task_matrix.go`。本层这里刻意不提供"帮我循环"的糖：
// 谁要 16 次、按什么顺序、失败几次算整体失败，那是业务口径，不是协议。
func (u *Upstream) AppTaskListV2(
	ctx context.Context, jwt, referer, marketname, group string,
) (TaskListV2, *core.Error) {
	request, perr := TaskListV2Request(jwt, referer, marketname, group)
	if perr != nil {
		return TaskListV2{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return TaskListV2{}, perr
	}
	return ParseTaskListV2(envelope, group)
}
