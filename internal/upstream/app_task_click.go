// App 任务矩阵的**写**侧最小两口：`task/click`（把某条任务记成"做了"）与
// `mrp …/market/signin/page/doTask`（"云朵中心入口"那一类任务的前置枪）。
//
// # 这一族在旧实现里的位置
//
// 旧 TS `packages/core/src/tasks/app/helpers.ts:8-33` 的 `clickTask` 是整条 `appTask`
// 链唯一的"上报"动作：分派器对每条待做任务先打它一次（或在 `currstep != 0` 时跳过），
// 专属 runner 再补各自的玩法。所以本文件是 #9 那一族写侧的地基 ——
// 上面那层（分发表 / 各 runner）都只经这一口上报。
//
// # 三个域、三条路径，**不是**同一个端点
//
// 本仓夹具里有**四条** `task/click` 实拍，形状一致（`{"code":0,"msg":"success"}`）
// 但**域名与路径不同**（AGENTS §1.12 事实②"URL 相近而端点不同"）：
//
// | 实拍 | 域 | 路径 | `key` | `id` |
// | --- | --- | --- | --- | --- |
// | `3d.json` | `m.mcloud.139.com` | `/ycloud/signin/task/click` | `task` | 597 |
// | `使用知识库.json` | `m.mcloud.139.com` | `/ycloud/signin/task/click` | `task` | 547 |
// | `开启通知领云朵.json` | `caiyun.feixin.10086.cn:7071` | `/ycloud/signin/task/click` | `task` | 1022 |
// | `mobile-events-…json` | `caiyun.feixin.10086.cn:7071` | `/ycloud/signin/task/click` | **`task2`** | 1005 |
// | `分享码通知.json` | `caiyun.feixin.10086.cn:7071` | **`/market/task/task/click`** | `rafflecodeTask` | 402 / 24011701 |
//
// ⇒ 本文件只发**第一行那一族**（App 主域 + `signin/task/click`），因为旧实现的
// `clickTask` 打的就是它（`helpers.ts:9`，`m.mcloud.139.com`）。
// `:7071` 那两口各有归属：`signin/task/click` 在 `app_notice.go`（"开启通知"那条链），
// `market/task/task/click` 属分享码那一族 —— 都**不在**这里，别"顺手复用"。
//
// # `key` 为什么是参数而不是常量
//
// 实拍里同一个 `id` 在不同客户端下用过 `task` 与 `task2` 两个值
// （上表第 3、4 行是同一路径的两个 key），而旧实现按调用点传
// （`runners.ts`：409 传 `"task2"`，其余省略 ⇒ 默认 `"task"`）。
// 把它钉成一个常量 = 将来接 409 时要么发错 key、要么再开一个函数。
// ⚠ **哪些 id 该配哪个 key 没有判据表**：实拍只告诉我们这两个值上游都收过，
// 没告诉我们"id 597 用 task 才对"。所以 key 由调用方（分派表）逐条带，出处写在那边。
//
// # `mrp …/page/doTask` —— **零实拍**
//
// `BaseMrpMcloud` 这个域在本仓夹具里只出现过 `pointsNotClaimed` 与 `appConfig` 两条，
// `market/signin/page/doTask` 这一条**一次样本都没有**。它的形状来自旧 TS
// `runners.ts:101-130`（409 / 1005 两个 runner 的第一枪，`client=app&deviceId=<urlencoded>`，
// 且**忽略响应**）。⚠ 因此：① 这一枪**未被断言能被上游接受**；
// ② 它的响应形状未知 ⇒ 这里**不判 `code`**，只把传输层与 JSON 解析失败交回去，
// 拿到第一个实拍样本后要回来补判定（旧实现连错误都吞，我们不能）。
package upstream

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// 打卡 `key` 参数的取值。
//
// 两个实拍值（[TaskClickKeyDefault] / [TaskClickKeySecond]）来自上表；
// [TaskClickKeyRandomCloud] **零实拍**，只有旧 TS `index.ts:57-61` 那两处
// （481 在 `currstep==2` 时、478 在未完成时各打一发）⇒ 带它的那条链在结论里
// 必须标成"未验证"，不能算"已经跑通"。
const (
	// TaskClickKeyDefault 是实拍里最常见的那一发（`key=task`）。
	TaskClickKeyDefault = "task"
	// TaskClickKeySecond 是旧实现给 409 的那一发；实拍见过它出现在同路径的另一个调用上。
	TaskClickKeySecond = "task2"
	// TaskClickKeyRandomCloud 是旧实现复查那一趟给 481 / 478 用的 key（零实拍）。
	TaskClickKeyRandomCloud = "randomCloudTask"
)

// MrpDoTaskClient 是 `page/doTask` 的 `client` 取值（旧实现字面量 `app`）。
//
// ⚠ 它**不是**从实拍来的（这一条零样本）。与签到族那个 `Client = "app"` 同值但不同源：
// 前者是 `infoV3`/`startSignIn` 的实测值，这一格只是旧实现怎么写就怎么带。
const MrpDoTaskClient = "app"

// TaskClickRequest 构造 `GET m.mcloud.139.com/ycloud/signin/task/click?key=<key>&id=<n>`。
//
// **写请求**：它会让上游把这条任务记成"做过了"，云朵可能当场入账 ⇒ 不可回滚
// （没有"撤销打卡"这一口）。
//
// `taskID` 是**必填**：旧实现没有"id 为 0 也要发"的调用点，
// 而发一发 `id=0` 的打卡是拿用户的账去试探上游的容错。
func TaskClickRequest(jwt, referer, key string, taskID int64) (core.HTTPRequest, *core.Error) {
	if taskID <= 0 {
		return core.HTTPRequest{}, core.NewParse(fmt.Sprintf(
			"打卡需要正整数任务 id，实得 %d（不发 id=0 的试探请求）", taskID))
	}
	url := fmt.Sprintf("%s/ycloud/signin/task/click?key=%s&id=%d",
		BaseMcloud, PercentEncode(key), taskID)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true"), nil
}

// TaskClick 是 `task/click` 的解析结果。
type TaskClick struct {
	// Notice 是 `result.msg` —— 给人看的那句（实拍"开启通知"那一条里旧实现会把它打进日志）。
	//
	// ⚠ **四发实拍里没有任何一条带回 `result`**（全是 `{"code":0,"msg":"success"}`），
	// 所以这一格今天恒为 nil。声明它只是因为旧实现读它，
	// 而"上游不给"与"给了空串"在两头的读法不同 —— 用指针分开。
	Notice *string
	// Message 是顶层 `msg`（实拍恒 `success`）。
	//
	// ⚠ 它是**信封**的那一句，不是结论：`code != 0` 时它也可能是人话，
	// 而失败在 [ParseTaskClick] 里就已经拦下了，不会走到这里还带着它。
	Message *string
}

// ParseTaskClick 解析一发打卡的响应。
//
// 判据只有一条：`code == 0`（实拍四条全是这一档，旧实现也只看这一格）。
// ⚠ **没有**"已做完 / 不满足条件"这类细分码的样本 ⇒ 非 0 一律按失败交回，
// 别把某个非 0 码读成"其实算完成"。
func ParseTaskClick(envelope *Envelope) (TaskClick, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return TaskClick{}, perr
	}
	out := TaskClick{Message: optStrField(rawEnvelopeMap(envelope), "msg")}
	if result, ok := payload.(map[string]any); ok {
		out.Notice = optStrField(result, "msg")
	}
	// `result` 缺席（实拍全部如此）或不是对象 ⇒ Notice 留 nil，不报错：
	// 这一格只是给人看的一句提示，判据在上面那一句 `code`。
	return out, nil
}

// MrpDoTaskRequest 构造 `GET mrp.mcloud.139.com/market/signin/page/doTask?client=app&deviceId=<n>`。
//
// ⚠ **零实拍**（见文件头那一节）。`deviceID` 是 H5 那枚（与 `receiveV3` 同一口径），
// 旧实现做了 URL 编码，这里用 [PercentEncode] 同一条编码规则
// （`deviceId` 是 base64，`+` 不编码会变成空格 —— 见 `app_signin.go` 那条注释）。
//
// 空 `deviceID` 在请求构造这一层就拦：旧实现传的是 `ctx.state.deviceId || ""`，
// 也就是**会**发空串那一发；但那是"取不到就当没有"的偷懒，
// 而这一枪今天没有任何样本能告诉我们上游怎么处置空值。
func MrpDoTaskRequest(jwt, referer, deviceID string) (core.HTTPRequest, *core.Error) {
	if deviceID == "" {
		return core.HTTPRequest{}, core.NewParse(
			"入口上报（mrp doTask）需要 deviceId，实得空串 —— 旧实现会发空的那一发，这里不发")
	}
	url := fmt.Sprintf("%s/market/signin/page/doTask?client=%s&deviceId=%s",
		BaseMrpMcloud, PercentEncode(MrpDoTaskClient), PercentEncode(deviceID))
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true"), nil
}

// AppTaskClick 打一发任务打卡。**写请求**。
func (u *Upstream) AppTaskClick(
	ctx context.Context, jwt, referer, key string, taskID int64,
) (TaskClick, *core.Error) {
	request, perr := TaskClickRequest(jwt, referer, key, taskID)
	if perr != nil {
		return TaskClick{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return TaskClick{}, perr
	}
	return ParseTaskClick(envelope)
}

// AppMrpDoTask 打"云朵中心入口"那一发前置上报。**写请求，且零实拍**。
//
// 返回的错误只覆盖"传输层 / 不是 JSON / 信封里 `success=false`"这三类；
// **不判 `code`** —— 因为我们不知道这一口的失败码长什么样（文件头第二条）。
// 调用方（409 / 1005 那两条 runner）因此**不能**把它当"这一步成功了"的证据，
// 只能当"这一枪发出去了、没人拒绝"。
func (u *Upstream) AppMrpDoTask(
	ctx context.Context, jwt, referer, deviceID string,
) *core.Error {
	request, perr := MrpDoTaskRequest(jwt, referer, deviceID)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	if success, ok := envelope.Success(); ok && !success {
		_, perr = envelope.EnsureOK()
		return perr
	}
	return nil
}

// rawEnvelopeMap 取信封顶层（拿 `msg` 用）。不是对象时交回 nil，
// [optStrField] 对 nil map 读不出字段 ⇒ Message 留 nil。
func rawEnvelopeMap(envelope *Envelope) map[string]any {
	obj, _ := envelope.Raw().(map[string]any)
	return obj
}
