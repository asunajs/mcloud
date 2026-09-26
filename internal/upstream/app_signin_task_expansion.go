// 任务拓展的**读口**：`GET /ycloud/signin/page/taskExpansion`（只读）。
//
// 对应旧实现 `packages/core/src/tasks/task-expansion.ts` 里 `getTaskExpansion` 那一步
// （请求构造在 `apis/signin.ts:54-58`）。**领取**那一口不在这里 —— 它早就随签到族落码，
// 见 [ReceiveTaskExpansionRequest] 与 [AppReceiveTaskExpansion]。
//
// # 为什么发送口在本文件，而不在 `app_signin_send.go`
//
// 那个文件的抬头写的是"**写请求**（startSignIn / receiveV3 / receiveTaskExpansion /
// multiple …）"。这一口是**只读**的，放过去会让"哪些口会改用户账上东西"那一格变得
// 要逐条读函数名才能判 —— 而那份清单是审核写操作时的入口。
package upstream

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
)

// TaskExpansion 是 `taskExpansion` 的解析结果（六格，**全部可选**）。
//
// # 这一族在做什么（旧实现的用法逐字，编排层的落点见 `internal/app/task_expansion_run.go`）
//
//   - `curMonthBackup === false` ⇒ 先往网盘**传一个 backup 档的文件**去"凑备份"，
//     等 `backupWaitTime`（旧实现默认 20 秒）之后**再读一次**；重试**只有一次**
//     （`timer` 从 1 递减），第二次还是 false 就只念一句"本月未开启备份，将无法获取翻倍奖励"。
//   - `curMonthTaskRecordCount > 0` ⇒ 拿**同一条响应里的** `acceptDate` 去领
//     （[AppReceiveTaskExpansion]），念 `cloudCount`。
//   - `nextMonthTaskRecordCount` **只打印**，不发任何请求。
//
// # 为什么六格全是指针
//
// 本仓夹具 `test/fixtures/http/3d.json` 里这个端点有两条记录，**请求面与响应面都有实拍**
// （两条是同一份读数：`curMonthBackup:true`、`curMonthBackupTaskAccept:false`、
// `curMonthTaskRecordCount:0`、`nextMonthTaskRecordCount:358`、`preMonthBackup:false`）。
// 那一条给了**五格**、**没给 `acceptDate`** ⇒ 六格里至少有一格是"上游可能就是不给"。
// 而这一口是**只读**的：把"缺字段"判成解析失败，表现是换一种上游措辞就整条能力报红
// （并触发 `onlyError` 推送），与 [ParseReceiveTaskExpansion] 同一个理由。
//
// ⚠ nil 与"读到 `false` / `0`"在两处判据上**不等价**：编排层必须能分清
// "上游说这个号没开备份"（⇒ 要凑）与"上游没说"（⇒ **不凑、也不领**）。
// 把"没读到"当"没开"的实际后果是往用户网盘写一个文件，而那是不可回滚的动作。
//
// ⚠ 反过来，**实拍那一条不给"凑备份"这一支任何证据**：它读到的就是 `curMonthBackup:true`
// ⇒ 旧实现在那一支上走的是"跳过凑、直接看要不要领"。"未开备份的号到底回什么形状"
// 仍是待验证（要补一条 `curMonthBackup:false` 的读数才可对拍）。
type TaskExpansion struct {
	// CurMonthBackup 是本月**是否已开启自动备份**（`curMonthBackup`）。
	// 旧实现唯一的用法就是它：`false` ⇒ 凑备份。
	CurMonthBackup *bool
	// PreMonthBackup 是上月是否开着备份（`preMonthBackup`）。
	//
	// ⚠ 旧实现**一个字都不读**它 ⇒ 这里带出来只为了排障时能看见上游说了什么，
	// 不参与任何判定，也不进对外文案。
	PreMonthBackup *bool
	// CurMonthTaskRecordCount 是本月攒下的任务记录数（旧实现判 `> 0` 才去领）。
	CurMonthTaskRecordCount *int64
	// CurMonthBackupTaskAccept 是本月的备份任务**是否已经领过**
	// （`curMonthBackupTaskAccept`）。
	//
	// 实拍给过这一格（`false`），但旧实现**一个字都不读**它（它读的是 `infoV3` 的
	// `curMonthBackupSignAccept`，那一格在 [SignInStatus]）⇒ 带出来不等于本层据此分支。
	CurMonthBackupTaskAccept *bool
	// NextMonthTaskRecordCount 是下月可领的朵数（旧实现只打印，不发请求）。
	NextMonthTaskRecordCount *int64
	// AcceptDate 是领取那一口要回带的日期串（来自**同一条**响应）。
	//
	// ⚠ 夹具那两条实拍**都没有这一格** ⇒ 本层读到 nil 是**缺省路径**，不是异常。
	// 同族的 `infoV3` 也被实拍过：它的 `receiveList[]` 里同样没有 `acceptDate`
	// （见 [ReceiveTaskExpansionRequest] 头注那条"被实拍推翻两次"）⇒
	// nil 时发裸 GET，与真实客户端一致。
	//
	// 空串在这里一律收成 nil：`?acceptDate=` 与不带参数是**两个不同的请求**，
	// 而"上游给了一个空串"更接近"没给"。
	AcceptDate *string
}

// TaskExpansionRequest 构造 `GET /ycloud/signin/page/taskExpansion`。
//
// # 主机与路径：取**有实拍**的那一口，不取旧 TS 那一口
//
// 旧实现在 `mrp.mcloud.139.com/market/signin/page/taskExpansion`（`apis/signin.ts:56`），
// 而本仓夹具实拍到的是 **App 内 H5** 的
// `m.mcloud.139.com/ycloud/signin/page/taskExpansion`（`test/fixtures/http/3d.json`
// 两条，UA `MCloudApp/12.6.0`）。
// ⚠ 两口是否同一个后端**未测试**。本层按"只取自己取过证的那一口"落（坑表 **#151**：
// 对齐只能借形状，不能借事实 —— 同族里 `msgPushOn` 就是拿"前缀相同"推成"主机相同"
// 而错的），并把"另一口今天认不认"留成待验证。
//
// # 没有查询串
//
// 夹具那两条记录的 URL **逐字不带** `?client=`（与同族的 `infoV3?client=app`、
// `startSignIn?client=app` 不同），旧 TS 那句也是裸 URL ⇒ 两个来源在这格上一致。
// ⚠ 别"顺手对齐同族"补一个 `client=app`：那是**另一个请求**，而它的实拍里没有它。
//
// # `deviceID` 是可选
//
// 夹具的两条都带 `deviceid` 头（同 `jwttoken` / `activityid` / `showloading` / `referer`），
// 所以传了就带上。做成可选而不是必填，是因为本仓拿不到 deviceID 的账号是**存在**的
// （设备档案那一步失败时），那种情况下这一口仍该能读 —— 它是只读的，
// 少一个头最多是被上游拒，不像 [ReceiveV3Request] 那样"不带就回 615"。
func TaskExpansionRequest(jwt, referer string, deviceID *string) core.HTTPRequest {
	request := WithH5Headers(
		core.NewRequestGet(BaseMcloud+"/ycloud/signin/page/taskExpansion"),
		referer, &jwt, "true")
	if deviceID != nil {
		request = request.WithHeader("deviceid", *deviceID)
	}
	return request
}

// AppTaskExpansion 读一次任务拓展（**只读**，可重复调）。
//
// 重试那一半（凑备份之后再看一眼）**不在这里**，在编排层 —— 上游层只管一发一问。
func (u *Upstream) AppTaskExpansion(
	ctx context.Context, jwt, referer string, deviceID *string,
) (TaskExpansion, *core.Error) {
	envelope, perr := u.Call(ctx, TaskExpansionRequest(jwt, referer, deviceID))
	if perr != nil {
		return TaskExpansion{}, perr
	}
	return ParseTaskExpansion(envelope)
}

// ParseTaskExpansion 解析 `taskExpansion` 的响应。
//
// ⚠ **没有 `result` 不算失败**（理由见 [TaskExpansion] 那一段"为什么六格全是指针"）。
// 失败只有一种：信封不 OK（走 [EnsureOKWithHint]，其中 `90001` 在那一层转成
// "会话过期 ⇒ 可重放"，编排层据此刷票，而不是当成"这个号没这个任务"）。
//
// # 布尔与数字只认**自己的类型**
//
// `curMonthBackup` 只认 JSON 真正的 `true` / `false`（实拍那一条正是真布尔）。
// 万一上游改成 `1` / `0`，这里得到的是 nil ⇒ 调用方判"未知"而**不发那一发写**
// （凑备份会往用户网盘写文件）。
// 这与 [SignInStatus] 里那两个门槛字段的处理**同一档**，不是巧合：两处判的都是
// "要不要动用户的东西"。
func ParseTaskExpansion(envelope *Envelope) (TaskExpansion, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return TaskExpansion{}, err
	}
	out := TaskExpansion{}
	obj, ok := payload.(map[string]any)
	if !ok {
		// 没有 result（或它不是对象）⇒ 交回一份"什么都没读到"，**不报错**。
		return out, nil
	}
	if v, ok := obj["curMonthBackup"].(bool); ok {
		out.CurMonthBackup = &v
	}
	if v, ok := obj["preMonthBackup"].(bool); ok {
		out.PreMonthBackup = &v
	}
	if v, ok := obj["curMonthBackupTaskAccept"].(bool); ok {
		out.CurMonthBackupTaskAccept = &v
	}
	if v, ok := intAtOK(obj, "curMonthTaskRecordCount"); ok {
		out.CurMonthTaskRecordCount = &v
	}
	if v, ok := intAtOK(obj, "nextMonthTaskRecordCount"); ok {
		out.NextMonthTaskRecordCount = &v
	}
	if v, ok := obj["acceptDate"].(string); ok && v != "" {
		out.AcceptDate = &v
	}
	return out, nil
}
