// ⚠⚠⚠ **这一族没有任何实拍样本** ⚠⚠⚠
//
// 下面每一条 URL、方法、body 形态、字段名，来源**只有一处**：
// `docs/business-feature-inventory.md` §2.3 对旧 TS（`mcloud-sign` 的
// `packages/core/src/tasks/msg-push.ts` + `apis/task.ts:14,18` + `types/task.ts:10`）
// 的**文字描述**。`test/fixtures/http/` 里搜不到 `msgPushOn` 的任何一个字 ——
// 2026-09-24 对 `test/fixtures/` **整棵树**重扫过一遍（31 份 + 那份同名镜像目录
// `test/fixtures/http/http/`），`msgPush` / `pushOn` / `onDuaration` /
// `obtainCode` 四个键名**全部零命中**（`docs/capture/` 同样零命中）。
//
// 三方核对的结果（**端点与字段**只有旧 TS 一处说话，这一点决定下面的处置）：
//   - 旧 TS：**有**这一族，且是本文件唯一说得出 URL / 方法 / 字段名的来源。
//   - 开源 Go 参考实现（caiyun）：**也有**这一族 ——
//     `docs/caiyun-go-feature-inventory.md` §2 第 90 行记着 `messagepush`
//     「消息推送奖励」（写操作，处置=搬），参数篇 §5 把它列在「直接搬语义」里。
//     ⚠ 但那两篇只记了**行为**（"未开启推送时只 Warn ⇒ 保持绿"），**没有**记它的
//     端点、方法与字段 ⇒ 它**帮不上形状**这一格的口径。
//     行为那一格我们也**不照抄**：旧项目把"通知没开"压成一句 Warn，
//     本仓库落成 [core.OutcomeManualRequired]（理由同 `signin_claims_run.go` 里
//     "未开自动备份"那一档 —— 不点出来就没人知道要去 App 里开）。
//     （参考项目当时 clone 在 `/tmp/caiyun-ref`、commit `1b2cf07`，本机现在没有；
//     要读它那份实现得重新 clone，别拿"搜不到"当"它没有"。）
//   - 本项目抓包：**零**。
//     ⇒ 所以本文件不"实现完就当验证过"，而是把每个未定项都做成**一处可改**的形态
//     （见 [msgPushObtainMethod]）。
//
// # 怎么把这一族从"照抄描述"升级成"验过"
//
// 按 `docs/capture-tools.md` 的通路抓一次「App 里开推送 → 领推送任务」的完整链路，
// 至少要到：① `task/status` 的响应原文（含 `pushOn` 在**关闭**态下的值）；
// ② `task/obtain` 的**方法**与**是否带 body**；③ `obtainCode` 非 0 的那条例文。
// 抓到之前，任何一次真发都是**未验证形态**打向真实账号 —— 所以这一族只挂在
// `internal/app/msg_push_run.go` 那条**手工点一次**的能力上（注册表里 `RunInBatch`
// 为假，不进编排）。
package upstream

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// msgPushTaskPath 是这一族的路径前缀。
//
// ⚠ 主机**选的是不带端口的 [BaseCaiyun]**，理由与"它可能该带 :7071"这条未验证一起写在这里：
// 本仓库里**有实拍**的 `/market/…` 接口只有备份好礼那两条
// （[BackupGiftInfoRequest] / [BackupGiftReceiveRequest]），它们用的是
// `https://caiyun.feixin.10086.cn`（**无端口**），且注释记着"裸请求回
// `{"code":90001,"msg":"未登录"}`"⇒ 与 `m.mcloud.139.com` 上那几条在同一道
// 鉴权网关后面。**同一前缀 `/market/` 就跟着它对齐**（不是"照抄旧项目"，是照抄
// 本仓库里唯一被实拍支持过的同类选择）。
// ⚠ 而 `msgPushOn` 到底在不在同一个入口上，**未验证** —— 真出问题（例如 404）
// 第一个要改的就是这里，把它换成 [BaseCaiyunYcloud] 试一次。
const msgPushTaskPath = "/market/msgPushOn/task"

// MsgPushStatusRequest 构造 `GET …/market/msgPushOn/task/status` —— 查推送任务状态。
//
// 查询参数：**一个都不带**。零样本 ⇒ 与 `getTaskInfo` 那条同理（旧项目的调用点是
// 裸 GET），但**这是照描述写的，不是照实拍写的**。
func MsgPushStatusRequest(jwt, referer string) core.HTTPRequest {
	url := fmt.Sprintf("%s%s/status", BaseCaiyun, msgPushTaskPath)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// msgPushObtainMethod 是 `task/obtain` 用哪个方法发 —— **未定，A/B 只改这一处**。
//
// 旧 TS 的描述里写的是 `GET|POST`（两份记录都没定下来），而"同时发两种方法"是
// **绝对不行**的：那等于向一个真实账号发两次同一个写请求，而上游是否幂等**零样本**。
// ⇒ 这里钉成**一个**常量，缺省 `GET`（`WithJSONBody` 会把 body 与 `content-type`
// 一起带上，所以翻成 `POST` 时不需要动别的代码）。
// 怎么定：真跑一次，看哪个方法回的是可解析的信封而不是 `405` / `404`；
// 或者按 `docs/capture-tools.md` 抓一次「点领取」的流量，方法就在 `:method` 那一行。
const msgPushObtainMethod = core.MethodGet

// MsgPushObtainBodyType 是 body 里那个 `type` —— 实拍**零条**，
// 旧 TS 逐字是 `{type: 2}`（`apis/task.ts:18`）。做成常量是因为它同时出现在
// "判 `secondTaskStatus === 2` 才发"那条判据里，两处必须是同一个数。
const MsgPushObtainBodyType int64 = 2

// MsgPushObtainRequest 构造 `…/market/msgPushOn/task/obtain`，body 逐字 `{"type":2}`。
//
// ⚠ 这是一次**写**动作，门槛（`secondTaskStatus == 2`）由调用方判，见
// `internal/app/msg_push_run.go` 的 `PlanMsgPush`。
//
// # body 与方法的组合也是未验证的
//
// 描述只说了两件事：body 是 `{type:2}`、方法未定。而"GET 带 JSON body"
// 本身就是一个不常见形态（有的网关会吞掉）。翻 [msgPushObtainMethod] 即可，
// 不要在这里"顺手改成没 body"。
func MsgPushObtainRequest(jwt, referer string) (core.HTTPRequest, *core.Error) {
	request := core.NewRequestGet(fmt.Sprintf("%s%s/obtain", BaseCaiyun, msgPushTaskPath))
	request.Method = msgPushObtainMethod
	request, err := request.WithJSONBody(map[string]any{"type": MsgPushObtainBodyType})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithH5Headers(request, referer, &jwt, "true"), nil
}

// MsgPushDurationKey 是上游**自己拼错**的那个键名（`onDuaration`，正确拼法应是
// `onDuration`）。
//
// ⚠⚠ **不许"顺手改对"**：旧项目 `types/task.ts:10` 记着反序列化**必须按错的键名**收。
// 改成正确拼法的表现是"这个字段永远是 0"，而**不报错** —— 因为它只是没读到。
// 这条由 `app_msg_push_test.go` 里那节"同时给两个拼法"的用例钉住。
const MsgPushDurationKey = "onDuaration"

// MsgPushStatus 是 `task/status` 的解析结果。
//
// # 四个字段全部可选（指针）
//
// 这一族零样本 ⇒ 没有任何一个字段的存在性被证实过。"上游没给"要落成
// `PlanMsgPush` 的 Unknown 档（**一个写请求都不发**），不能落成"给了 0"。
// 尤其 `pushOn`：把它缺省成 `0` 等于**凭空造一条"通知已关闭"的待办**。
type MsgPushStatus struct {
	// PushOn 是 `pushOn` —— 旧描述：`0` ⇒ 消息通知已关闭（要人去 App 里打开）。
	// 非 0 到底有几种取值、分别是什么，**未测试**。
	PushOn *int64
	// OnDuaration 是 `onDuaration`（上游拼错的键名，见 [MsgPushDurationKey]）。
	//
	// 旧描述完全没说它是干什么的 ⇒ 本仓库**不解释**，只照抄读数。
	// 名字保留上游的拼写错误，是为了"看代码就知道在读哪个键"。
	OnDuaration *int64
	// FirstTaskStatus 是 `firstTaskStatus` —— 旧描述：`!= 3` ⇒ 首次奖励未领取
	// （旧项目**只打一句 info、不影响成功**，我们照这个口径，见 `msg_push_run.go`）。
	FirstTaskStatus *int64
	// SecondTaskStatus 是 `secondTaskStatus` —— 旧描述：**只有 `2` 才该发 `obtain`**。
	// ⚠ 其它值一个样本都没有，所以"3 是什么"这类问题在这里没有答案。
	SecondTaskStatus *int64
}

// ParseMsgPushStatus 解析 `task/status` 的响应。
//
// 缺 `result` **不报错**：与 [ParseNoticeTaskInfo] 同理 —— 判据读不出来要落成
// "什么都不发"那一档，而不是落成一次接口故障。
func ParseMsgPushStatus(envelope *Envelope) (MsgPushStatus, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return MsgPushStatus{}, perr
	}
	obj, _ := payload.(map[string]any)
	if obj == nil {
		return MsgPushStatus{}, nil
	}
	return MsgPushStatus{
		PushOn:           optInt64Field(obj, "pushOn"),
		OnDuaration:      optInt64Field(obj, MsgPushDurationKey),
		FirstTaskStatus:  optInt64Field(obj, "firstTaskStatus"),
		SecondTaskStatus: optInt64Field(obj, "secondTaskStatus"),
	}, nil
}

// MsgPushObtain 是 `task/obtain` 的解析结果。
type MsgPushObtain struct {
	// ObtainCode 是 `result.obtainCode` —— 旧描述：成功判据是它**等于 0**。
	//
	// ⚠ 为什么是 `*int64` 而不是"缺了当 0"：缺了**不等于成功**，而这一条已经
	// 把写请求发出去了 —— 把"判不了"伪造成"成功"，用户就再也不会去查这一笔。
	// 处置见 `msg_push_run.go`（缺它 ⇒ 报失败结论，但**不**回错误）。
	ObtainCode *int64
}

// ParseMsgPushObtain 解析 `task/obtain` 的响应。
//
// ⚠ `code:0` **不是**这条的成功判据（旧描述点名 `result.obtainCode === 0`）——
// 所以本函数**不**替调用方判成败，只把读数交出去。
// 但外层 `code` 非 0 仍走 [EnsureOKWithHint] 报成错误：那一层是"这一枪打没打上"。
func ParseMsgPushObtain(envelope *Envelope) (MsgPushObtain, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return MsgPushObtain{}, perr
	}
	out := MsgPushObtain{}
	if obj, ok := payload.(map[string]any); ok {
		out.ObtainCode = optInt64Field(obj, "obtainCode")
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层（本文件自带：`app_signin_send.go` 有并发改动在飞，不动它）
// ─────────────────────────────────────────────────────────────────────────────

// AppMsgPushStatus 查推送任务状态（`task/status`）。**只读**。
func (u *Upstream) AppMsgPushStatus(
	ctx context.Context, jwt, referer string,
) (MsgPushStatus, *core.Error) {
	envelope, perr := u.Call(ctx, MsgPushStatusRequest(jwt, referer))
	if perr != nil {
		return MsgPushStatus{}, perr
	}
	return ParseMsgPushStatus(envelope)
}

// AppMsgPushObtain 领推送任务的第二次奖励（`task/obtain`）。**写请求，且零样本**。
//
// ⚠ 门槛**不在这里判**：只有 `secondTaskStatus == 2` 才该发（旧描述），
// 判据与它的局限都写在 `internal/app/msg_push_run.go`。
func (u *Upstream) AppMsgPushObtain(
	ctx context.Context, jwt, referer string,
) (MsgPushObtain, *core.Error) {
	request, perr := MsgPushObtainRequest(jwt, referer)
	if perr != nil {
		return MsgPushObtain{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return MsgPushObtain{}, perr
	}
	return ParseMsgPushObtain(envelope)
}
