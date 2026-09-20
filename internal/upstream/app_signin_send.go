// 发送层：把"请求构造"与"响应解析"配成对，一次调用走完一条链路。
//
// 对应 Rust 侧 app_signin/send.rs（`impl Upstream` 那一组方法）。
//
// # 这一层**不判门槛**
//
// 写请求（`startSignIn` / `receiveV3` / `receiveTaskExpansion` / `multiple` /
// `backupgift/receive`）在这里一律直接发 —— 该不该发由**调用方**看
// `infoV3` 的状态决定。本层只保证"发出去、解析回来"。
//
// 理由：门槛判据是**业务**的（`cloudType` 三态、每月一次机会、备份是否开启），
// 混进协议层之后，同一条链路就有了两份真值。
package upstream

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
)

// AppQuerySpecToken 换 SSO token（`querySpecTokenV2`）。
func (u *Upstream) AppQuerySpecToken(
	ctx context.Context,
	profile *AppProfile,
	timestamp int64,
	authorization string,
	toSourceID string,
) (string, *core.Error) {
	request, perr := QuerySpecTokenRequest(
		profile, timestamp, authorization, QuerySpecTokenBody(toSourceID))
	if perr != nil {
		return "", perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return "", perr
	}
	return ParseSSOToken(envelope)
}

// AppH5Timestamp 取服务端毫秒时间戳（签名要用）。
//
// ⚠ 不能拿本地时钟顶替：签名里的 `x-timestamp` 必须与服务端逐字相同，
// 偏一点就是 `401`。
func (u *Upstream) AppH5Timestamp(ctx context.Context) (string, *core.Error) {
	envelope, perr := u.Call(ctx, H5TimestampRequest())
	if perr != nil {
		return "", perr
	}
	return ParseServerTimestamp(envelope)
}

// AppH5Login 建会话（`tyrzLogin`，带签名）。
func (u *Upstream) AppH5Login(
	ctx context.Context, sign H5Signature, referer string, ssoToken string,
) (H5Login, *core.Error) {
	request, perr := TyrzLoginRequest(sign, referer, TyrzLoginBody(ssoToken))
	if perr != nil {
		return H5Login{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return H5Login{}, perr
	}
	return ParseJWTToken(envelope)
}

// AppSignInStatus 查签到状态（`infoV3`）。
func (u *Upstream) AppSignInStatus(ctx context.Context, jwt, referer string) (SignInStatus, *core.Error) {
	envelope, perr := u.Call(ctx, InfoV3Request(jwt, referer))
	if perr != nil {
		return SignInStatus{}, perr
	}
	return ParseSignInStatus(envelope)
}

// AppSignInStatusRaw 是 `infoV3` 的**原始响应体**，不做任何解析。
//
// # 为什么留一个不解析的入口
//
// [SignInStatus] 只保留我们**认识的**字段。协议还在摸（`receiveList[]`
// 的条目里到底还有哪些字段、顺序有没有含义），"只看得见的字段"会让排查
// 变成对着残缺数据猜 —— 这个入口就是为了那种时候。
//
// ⚠ 它不是给业务用的：调用方拿到的是上游原文，**字段随上游变**，
// 别把它接进任何判断里。
func (u *Upstream) AppSignInStatusRaw(ctx context.Context, jwt, referer string) (any, *core.Error) {
	envelope, perr := u.Call(ctx, InfoV3Request(jwt, referer))
	if perr != nil {
		return nil, perr
	}
	return envelope.Raw(), nil
}

// AppStartSignIn 执行签到（`startSignIn`）。
//
// ⚠ 它的响应是**调用前**的快照，不是成败判据 —— 要确认签到了没有，
// 得再回查一次 [Upstream.AppSignInStatus]。
func (u *Upstream) AppStartSignIn(
	ctx context.Context, jwt, referer string, deviceID *string,
) (SignInResult, *core.Error) {
	envelope, perr := u.Call(ctx, StartSignInRequest(jwt, referer, deviceID))
	if perr != nil {
		return SignInResult{}, perr
	}
	return ParseStartSignIn(envelope)
}

// AppTaskListV3 取任务列表（`taskListV3`）。
func (u *Upstream) AppTaskListV3(
	ctx context.Context, jwt, referer string, client uint8,
) ([]any, *core.Error) {
	request, perr := TaskListV3Request(jwt, referer, client)
	if perr != nil {
		return nil, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return nil, perr
	}
	return ParseTaskList(envelope)
}

// AppDoTaskPost 做任务（`doTaskPost`）。`deviceID` 必填。
func (u *Upstream) AppDoTaskPost(
	ctx context.Context, jwt, referer, deviceID string,
) (*Envelope, *core.Error) {
	request, perr := DoTaskPostRequest(jwt, referer, deviceID)
	if perr != nil {
		return nil, perr
	}
	return u.Call(ctx, request)
}

// AppReceiveV3 领云朵（`receiveV3`）。
//
// `deviceID` 是必填的 —— 它是这一族里**已知唯一一个"不带就失败"**的接口，
// 所以类型上不允许它为空（见 [ReceiveV3Request]）。
func (u *Upstream) AppReceiveV3(
	ctx context.Context,
	jwt, referer, cloudID string,
	cloudType *int64,
	deviceID string,
) (ReceiveResult, *core.Error) {
	request, perr := ReceiveV3Request(jwt, referer, cloudID, cloudType, deviceID)
	if perr != nil {
		return ReceiveResult{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return ReceiveResult{}, perr
	}
	return ParseReceiveV3(envelope)
}

// AppCloudRecords 查豆的收支流水（`cloudRecordV2`）。**只读**，用于对账 / 排障。
//
// `aiDou` 取 [RecordBookCloud] / [RecordBookBean]，
// `filter` 取 [RecordFilterAll] / [RecordFilterEarned] / [RecordFilterSpent]。
func (u *Upstream) AppCloudRecords(
	ctx context.Context,
	jwt, referer string,
	aiDou, filter, pageNumber, pageSize int64,
) (CloudRecords, *core.Error) {
	envelope, perr := u.Call(ctx, CloudRecordV2Request(jwt, referer, aiDou, filter, pageNumber, pageSize))
	if perr != nil {
		return CloudRecords{}, perr
	}
	return ParseCloudRecords(envelope)
}

// AppReceiveRevivalReward 用复活卡 / 回馈卡（`receiveRevivalReward`）。
//
// `deviceID` 可选 —— 两个证据不一致，见 [ReceiveRevivalRewardRequest]。
func (u *Upstream) AppReceiveRevivalReward(
	ctx context.Context, jwt, referer string, deviceID *string,
) (RevivalReward, *core.Error) {
	request, perr := ReceiveRevivalRewardRequest(jwt, referer, deviceID)
	if perr != nil {
		return RevivalReward{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return RevivalReward{}, perr
	}
	return ParseRevivalReward(envelope)
}

// AppReceiveTaskExpansion 延期领取（`receiveTaskExpansion`）—— `cloudType == 1` 那一条路。
//
// `acceptDate` 取自 `receiveList[]` 里**同一条**的 `acceptDate`。
func (u *Upstream) AppReceiveTaskExpansion(
	ctx context.Context, jwt, referer string, acceptDate *string,
) (ReceiveTaskExpansion, *core.Error) {
	envelope, perr := u.Call(ctx, ReceiveTaskExpansionRequest(jwt, referer, acceptDate))
	if perr != nil {
		return ReceiveTaskExpansion{}, perr
	}
	return ParseReceiveTaskExpansion(envelope)
}

// AppClaimMultiple 领**签到翻倍**（`multiple`）。**写请求**，每月只有一次机会。
//
// ⚠ 门槛**不在这里判**：调用方要先看 `infoV3` 的 `curMonthBackup` /
// `curMonthBackupSignAccept`。
func (u *Upstream) AppClaimMultiple(ctx context.Context, jwt, referer string) (MultipleResult, *core.Error) {
	envelope, perr := u.Call(ctx, MultipleRequest(jwt, referer))
	if perr != nil {
		return MultipleResult{}, perr
	}
	return ParseMultiple(envelope)
}

// AppPrizeRecords 查**奖品记录**（`record`）。只读。
func (u *Upstream) AppPrizeRecords(
	ctx context.Context,
	jwt, referer, marketname string,
	status, pageNumber, pageSize int64,
) (PrizeRecords, *core.Error) {
	envelope, perr := u.Call(ctx, PrizeRecordRequest(jwt, referer, marketname, status, pageNumber, pageSize))
	if perr != nil {
		return PrizeRecords{}, perr
	}
	return ParsePrizeRecords(envelope)
}

// AppBackupGift 查**备份好礼**状态（`backupgift/info`）。只读。
func (u *Upstream) AppBackupGift(ctx context.Context, jwt, referer string) (BackupGift, *core.Error) {
	envelope, perr := u.Call(ctx, BackupGiftInfoRequest(jwt, referer))
	if perr != nil {
		return BackupGift{}, perr
	}
	return ParseBackupGift(envelope)
}

// AppReceiveBackupGift 领**备份好礼**（`backupgift/receive`）。**写请求**，每月一次。
//
// ⚠ 门槛**不在这里判**：调用方要先看 [Upstream.AppBackupGift] 的
// [BackupGift.State] 与 [BackupGift.CurMonth]。
func (u *Upstream) AppReceiveBackupGift(
	ctx context.Context, jwt, referer string,
) (BackupGiftReceive, *core.Error) {
	envelope, perr := u.Call(ctx, BackupGiftReceiveRequest(jwt, referer))
	if perr != nil {
		return BackupGiftReceive{}, perr
	}
	return ParseBackupGiftReceive(envelope)
}
