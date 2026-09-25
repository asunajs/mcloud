// 公告任务族（「开启通知领云朵」）：`openEmailsms/*` 的两口 + 两个**未接入链路**的入账口。
//
// # 这一族的证据来自哪里（先看清"哪些是实拍、哪些是描述"）
//
// 实拍（`test/fixtures/http/开启通知领云朵.json` 第 0/3/6/7 条、
// `完成通知领云朵.json` 第 8 条、`mobile-events-…json` 第 71/87 条）：
//
//	GET …:7071/ycloud/signin/task/click?key=task&id=1022   → {"code":0,"msg":"success"}
//	GET …:7071/ycloud/openemailsms-service/openEmailsms/getTaskInfo
//	                                                       → result.taskOneInfo / taskTwoInfo
//	GET …:7071/ycloud/openemailsms-service/openEmailsms/reward → {"code":0,"msg":"success"}
//	GET …:7071/ycloud/signin/page/receiveTask?taskId=1021  → result = 100（**裸数字**）
//	GET …:7071/ycloud/signin/page/reportAppNoticeStatus?status=1 → {"code":0,"msg":"success"}
//
// 零样本的东西同样要说全，否则下面的判定看起来像"从数据里读出来的"：
// **没有任何一条非 0 码的响应**、**没有任何一条"未开启"的响应**、
// `day` / `canReceive` 两个字段 —— 旧 TS 据说是按 `Number(day) > 0 && canReceive === 1`
// 判要不要上报状态，但这两个字段**一条实拍都没有**，连它们在哪个响应里都不确定。
//
// # ⚠ 三方口径不一致（本报告不自行裁决，差异已列进交付说明）
//
// `docs/business-feature-inventory.md` §2.3 说旧 TS 的链路是
// `getTaskInfo` → `reward` → `taskListV2(clientVersion 11.2.1)` 找 id `1021`
// → `reportAppNoticeStatus?status=1`；而**实拍**里 `reward` 与
// `receiveTask?taskId=1021` 各在一份抓包中出现的顺序是「先 reward、后
// receiveTask」，且实拍里**没有** `taskListV2` 这一环。
// ⇒ 本文件把**两条都实现**（`reward` 进链、`ReceiveTaskRequest` /
// `ReportAppNoticeStatusRequest` 只实现不接链），判定按**实拍最强的那一条**
// （`getTaskInfo` 的四个字段）裁，差异交用户确认。
//
// # 头：这四条实拍都带 `deviceid`，本仓库仍不发
//
// 拦截器只在 H5 显式标 `isDeviceId` 时注入设备串，而这三口的 H5 调用点我们**没有**
// （只有 iOS WebView 的流量原文），所以按既有约定**不带** `isdeviceid`、也不带 `deviceid`。
// 服务端是否真的认这一条 ⇒ **未验证**（怎么测：拿一条不带 `deviceid` 的请求打
// `getTaskInfo`，看是不是照样回 `code:0`）。
package upstream

import (
	"context"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 查状态 / 领奖励（`openEmailsms/*`）
// ─────────────────────────────────────────────────────────────────────────────

// NoticeTaskInfoPath 是这一族的路径前缀。
//
// ⚠ **两处大小写不能"顺手改对"**：主机段是全小写 `openemailsms-service`，
// 最后一段路径是驼峰 `openEmailsms`。实拍逐字如此（`开启通知领云朵.json` 第 3/6/7 条）。
// 改对任何一处的表现都是 `404`，而 404 看起来像"接口下线了"。
const noticeTaskPath = "/ycloud/openemailsms-service/openEmailsms"

// NoticeTaskInfoRequest 构造
// `GET …:7071/ycloud/openemailsms-service/openEmailsms/getTaskInfo` —— 查两个槽的状态。
//
// # 查询参数：按"**不带**"实现，且这是一个未验证的选择
//
// 实拍这条的完整 URL 被脱敏器压成了 `https://caiyun.feixin.10086.cn:<REDACTED>`
// （`match.key` 里只有路径，没有 `?`），⇒ **它到底吃不吃查询参数是未验证的**。
// 这里按"不带参数"实现，与旧项目的写法一致（旧项目的调用点是空 params）。
// 真要出问题（例如回了另一个活动的状态），改这一处加参数即可。
func NoticeTaskInfoRequest(jwt, referer string) core.HTTPRequest {
	url := fmt.Sprintf("%s%s/getTaskInfo", BaseCaiyunYcloud, noticeTaskPath)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// NoticeRewardRequest 构造 `GET …/openEmailsms/reward` —— **领公告任务的云朵**。
//
// ⚠ 这是一次**写**动作（会真的入账），门槛由调用方判，见 [NoticeTaskInfo] 那一段。
func NoticeRewardRequest(jwt, referer string) core.HTTPRequest {
	url := fmt.Sprintf("%s%s/reward", BaseCaiyunYcloud, noticeTaskPath)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// NoticeTaskSlot 是 `getTaskInfo` 的一个槽（`taskOneInfo` / `taskTwoInfo`）。
//
// # 四个字段**全部可选**，而且这不是偷懒
//
// 实拍只有**两条**读数：`taskOneInfo{cloudStatus:2, sum:200, taskStatus:3}` 与
// `taskTwoInfo{cloudStatus:1, progress:31, sum:100, taskStatus:3}`。
// ⇒ 两槽的 `taskStatus` **相同**而 `cloudStatus` **不同**，这本身就说明
// `cloudStatus` 不是"任务做没做完"，但我们**不知道**它是什么（"这个槽的通知
// 开没开"？"这个槽的云朵领没领"？两种读法都自洽）。
// 所以：① 字段用指针，"上游没给"与"给了 0"必须分得开（判据缺了就什么都不发）；
// ② 本仓库**不给** `cloudStatus==1 vs 2`、`taskStatus==3` 编语义，
// 只把读数**照抄进结论**（见 `internal/app/notice_run.go` 的 [NoticeResult]）。
type NoticeTaskSlot struct {
	// CloudStatus 是 `cloudStatus` —— 实拍读数 `1` / `2`，语义**未验证**。
	//
	// ⚠ 2026-09-25 真账号那一趟**仍然没答出这一格**：两个号判成的都是"没领"，
	// 于是拿到的读数是"已经领过之后"的 `cloudStatus`（那句判据把非"未领"值都读成
	// 已入账），"未领"那一值长什么样始终没有样本。
	// ⇒ 补样条件已写进 `internal/app/notice_run.go` 的 `noticeSlotReadings`：
	// 下一次读到"发得出去写请求"的那个形态，小结里就带着这一格的**前后两份**读数。
	// 问题登记在 `docs/pending-decisions.md` §三 R1。
	CloudStatus *int64
	// Sum 是 `sum` —— 实拍 `200` / `100`。看着像"这个槽值多少云朵"，
	// ⚠ 但**没有任何证据**说领到手的数等于它：`reward` 的实拍响应里**没有 `result`**。
	Sum *int64
	// Progress 是 `progress` —— 只在 `taskTwoInfo` 上出现（实拍 `31`）。
	// `taskOneInfo` **没有**这个键 ⇒ 它是可选的，不是 0。
	Progress *int64
	// TaskStatus 是 `taskStatus` —— 实拍两槽都是 `3`。
	TaskStatus *int64
}

// NoticeTaskInfo 是 `getTaskInfo` 的解析结果。
//
// 槽本身也是指针：上游**可以整槽不给**（旧 TS 的 `?.` 就是这么防的），
// 而"没给这一槽"与"给了但四个字段全缺"在判定上是两件事。
type NoticeTaskInfo struct {
	// TaskOneInfo 是 `result.taskOneInfo`。
	TaskOneInfo *NoticeTaskSlot
	// TaskTwoInfo 是 `result.taskTwoInfo`。
	TaskTwoInfo *NoticeTaskSlot
}

// ParseNoticeTaskInfo 解析 `getTaskInfo` 的响应。
//
// ⚠ 缺字段**不报错**：这条是判据来源，而"上游什么都没给"要落成
// [NoticePlanUnknown] 那一档（**一个请求都不发**），不是落成一次失败。
// 报错与"判不了"的代价不同 —— 前者会让用户以为接口坏了，后者才是实况。
func ParseNoticeTaskInfo(envelope *Envelope) (NoticeTaskInfo, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return NoticeTaskInfo{}, perr
	}
	obj, _ := payload.(map[string]any)
	return NoticeTaskInfo{
		TaskOneInfo: noticeSlotAt(obj, "taskOneInfo"),
		TaskTwoInfo: noticeSlotAt(obj, "taskTwoInfo"),
	}, nil
}

// noticeSlotAt 取一个槽对象；缺失、非对象一律 nil（= "上游没给这一槽"）。
func noticeSlotAt(obj map[string]any, key string) *NoticeTaskSlot {
	if obj == nil {
		return nil
	}
	raw, ok := obj[key].(map[string]any)
	if !ok {
		return nil
	}
	return &NoticeTaskSlot{
		CloudStatus: optInt64Field(raw, "cloudStatus"),
		Sum:         optInt64Field(raw, "sum"),
		Progress:    optInt64Field(raw, "progress"),
		TaskStatus:  optInt64Field(raw, "taskStatus"),
	}
}

// NoticeReward 是 `reward` 的解析结果。
//
// ⚠ 实拍响应**只有** `{"code":0,"msg":"success"}` —— 连 `result` 都没有，
// 所以这条**报不出"领到多少朵"**。"到底领没领上"只能靠**复查一次**
// [NoticeTaskInfoRequest]（链路的实现见 `internal/app/notice_run.go`）。
type NoticeReward struct {
	// CloudCount 是 `result` 里的云朵数。**恒 nil 的风险**：上游哪天补上这个字段
	// 就能报数，所以这里做成指针而不是一句注释了事。
	//
	// 取值口径与 `receiveTask` 一致（`result` 是**裸数字**时收下来），
	// ⚠ 但 `reward` 回裸数字这件事**没有样本**，只是"哪天给了别丢"。
	CloudCount *int64
}

// ParseNoticeReward 解析 `reward` 的响应。
//
// 没有 `result` **不算失败** —— 实拍就没有，成败判据只有 `code`。
func ParseNoticeReward(envelope *Envelope) (NoticeReward, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return NoticeReward{}, perr
	}
	if payload == nil {
		return NoticeReward{}, nil
	}
	if n, ok := asInt64(payload); ok {
		return NoticeReward{CloudCount: &n}, nil
	}
	return NoticeReward{}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 下面两口**只实现、不接进链路**：用哪一口入账由编排层（capability 接线）决定
// ─────────────────────────────────────────────────────────────────────────────

// ReceiveTaskRequest 构造 `GET …:7071/ycloud/signin/page/receiveTask?taskId=<n>`。
//
// # 为什么它值得单独存在
//
// 实拍（`完成通知领云朵.json` 第 8 条，`taskId=1021`）回的是
// `{"code":0,"msg":"success","result":100}` —— `result` 是**裸数字**，
// 也就是"**这一口报了到账数**"。而链路里那口 `reward` 什么都不报。
// ⇒ 所以「`receiveTask?taskId=1021` 是本族**唯一的实拍入账口**」这句话是有分量的：
// 只有它留下过"进了 100 朵"这个读数。用哪一口入账交编排层判，本层两条都给。
func ReceiveTaskRequest(jwt, referer string, taskID int64) core.HTTPRequest {
	url := fmt.Sprintf("%s/ycloud/signin/page/receiveTask?taskId=%d", BaseCaiyunYcloud, taskID)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// ReceiveTask 是 `receiveTask` 的解析结果。
type ReceiveTask struct {
	// CloudCount 是 `result` —— 这次入账的云朵数（实拍 `100`）。
	//
	// nil 表示上游没给数字（**不是 0**）：0 是"进账 0 朵"，nil 是"没说"。
	CloudCount *int64
}

// ParseReceiveTask 解析 `receiveTask` 的响应：`result` 是**裸数字**，不是对象。
//
// ⚠ 别按对象去取字段（`result.cloudCount` 之类）—— 那会让这条**永远报 0 朵**
// 且不报错，与"上游真的没发"看着一模一样。
func ParseReceiveTask(envelope *Envelope) (ReceiveTask, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return ReceiveTask{}, perr
	}
	if payload == nil {
		return ReceiveTask{}, nil
	}
	n, ok := asInt64(payload)
	if !ok {
		return ReceiveTask{}, nil
	}
	return ReceiveTask{CloudCount: &n}, nil
}

// AppNoticeStatusOn 是 `reportAppNoticeStatus` 的**唯一实拍**取值（`status=1`）。
//
// ⚠ 其它取值的语义**未验证** —— 别把"1 = 已开启通知"当结论写进文案，
// 只能说"上游在这个值上回过 success"。
const AppNoticeStatusOn int64 = 1

// ReportAppNoticeStatusRequest 构造
// `GET …:7071/ycloud/signin/page/reportAppNoticeStatus?status=1`。
//
// # 它来自**另一个客户端**
//
// 实拍在 `mobile-events-…json`（Android WebView，`x-requested-with: cn.cj.pe`），
// 而 `openEmailsms/*` 那几条来自 iOS WebView 流量 —— 两条不是同一个客户端。
// ⇒ "iOS 那套流程要不要上报这一口"**未验证**，所以它**不接进链路**。
// 旧 TS 那侧的条件是 `Number(day) > 0 && canReceive === 1`，
// 而 `day` / `canReceive` **零样本**（连在哪个响应里都不知道）。
func ReportAppNoticeStatusRequest(jwt, referer string, status int64) core.HTTPRequest {
	url := fmt.Sprintf("%s/ycloud/signin/page/reportAppNoticeStatus?status=%d",
		BaseCaiyunYcloud, status)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// SigninTaskClickKey 是 `task/click` 的 `key` 参数，实拍逐字 `task`。
const SigninTaskClickKey = "task"

// SigninTaskClickRequest 构造 `GET …:7071/ycloud/signin/task/click?key=task&id=<n>`。
//
// 实拍只有 `id=1022` 那一条（`开启通知领云朵.json` 第 0 条，就是「开启通知」这一步的
// 打卡）。⚠ 它与入账那条的 `taskId=1021` **是两个不同的 id**，
// 而"1021 与 1022 各自的任务是什么、为什么差 1"**没有任何成文依据** ——
// 别把这两个数当成同一个任务的两个阶段。
//
// 路径是 `signin/task/click` 而不是 `signin/page/…`：同一条链上两种前缀都出现过，
// 照抄实拍，不"统一"。
func SigninTaskClickRequest(jwt, referer string, taskID int64) core.HTTPRequest {
	url := fmt.Sprintf("%s/ycloud/signin/task/click?key=%s&id=%d",
		BaseCaiyunYcloud, SigninTaskClickKey, taskID)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层（本文件自带，不改 `app_signin_send.go`：那一份有并发的改动在飞）
// ─────────────────────────────────────────────────────────────────────────────

// AppNoticeTaskInfo 查公告任务两个槽的状态（`getTaskInfo`）。**只读**。
func (u *Upstream) AppNoticeTaskInfo(ctx context.Context, jwt, referer string) (NoticeTaskInfo, *core.Error) {
	envelope, perr := u.Call(ctx, NoticeTaskInfoRequest(jwt, referer))
	if perr != nil {
		return NoticeTaskInfo{}, perr
	}
	return ParseNoticeTaskInfo(envelope)
}

// AppNoticeReward 领公告任务的云朵（`reward`）。**写请求**。
//
// ⚠ 门槛**不在这里判**（[NoticeTaskSlot] 那四个字段读得出什么、读不出什么都在
// `internal/app/notice_run.go` 里判）。这条**报不出到账数**，见 [NoticeReward]。
func (u *Upstream) AppNoticeReward(ctx context.Context, jwt, referer string) (NoticeReward, *core.Error) {
	envelope, perr := u.Call(ctx, NoticeRewardRequest(jwt, referer))
	if perr != nil {
		return NoticeReward{}, perr
	}
	return ParseNoticeReward(envelope)
}

// AppReceiveTask 走 `receiveTask?taskId=<n>` 入账。**写请求**，且**这是本族唯一
// 报了到账数（`result` 裸数字）的入账口**。
func (u *Upstream) AppReceiveTask(
	ctx context.Context, jwt, referer string, taskID int64,
) (ReceiveTask, *core.Error) {
	envelope, perr := u.Call(ctx, ReceiveTaskRequest(jwt, referer, taskID))
	if perr != nil {
		return ReceiveTask{}, perr
	}
	return ParseReceiveTask(envelope)
}

// AppReportAppNoticeStatus 上报状态（`reportAppNoticeStatus?status=`）。
//
// 实拍响应没有 `result` ⇒ 只判 `code`。
func (u *Upstream) AppReportAppNoticeStatus(
	ctx context.Context, jwt, referer string, status int64,
) *core.Error {
	envelope, perr := u.Call(ctx, ReportAppNoticeStatusRequest(jwt, referer, status))
	if perr != nil {
		return perr
	}
	_, perr = EnsureOKWithHint(envelope)
	return perr
}

// AppSigninTaskClick 打卡（`signin/task/click`）。**写请求**。
func (u *Upstream) AppSigninTaskClick(
	ctx context.Context, jwt, referer string, taskID int64,
) *core.Error {
	envelope, perr := u.Call(ctx, SigninTaskClickRequest(jwt, referer, taskID))
	if perr != nil {
		return perr
	}
	_, perr = EnsureOKWithHint(envelope)
	return perr
}
