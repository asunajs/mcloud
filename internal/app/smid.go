// 账号绑定的 smid 设备特征，以及"拿特征换 `deviceId`"的编排。
//
// 对应 Rust 侧 legacy/crates/mcloud-app/src/smid.rs。
//
// # 本层管的是**生命周期**
//
// 纯算法在 [upstream] 那一侧（`smid.go`），存储契约在 [core.StateStore]
// 的三条窄写方法上。中间有一件事谁都没接：**什么时候生成、什么时候复用、
// 什么时候作废** —— 本层就是那件事的答案：
//
//	首次使用  → 生成特征 → 落库（绑定）→ 上报换 deviceId → 落库
//	之后每次  → 读库复用，不打网络
//	主动更换  → 重新生成特征 + 清空旧 deviceId（等于换一台设备）
//	重新登录  → 这两列都不动（换凭据不该换设备长相）
//
// # 为什么绑定值得单独一层
//
// 旧实现（`packages/shared/src/smid/`）每次上报都现掷一份新特征，于是同一个
// 账号在风控服务眼里**每次都换一台设备** —— 那正是最容易触发核验的姿态。
// 绑定的实际收益就是上面那张图：上报从"每次请求"降到"每个账号一次"，
// 而换 id 时旧 id 会进特征体的 `box`，让服务端看出"还是那台机器在换 id"。
//
// 算法能被参数化（[upstream.SmFingerprint] 是个普通结构体）是这件事的前提：
// 它必须能原样序列化回库、再原样读出来。那条往返由上游的
// `TestTheBoundFingerprintRoundTripsThroughTheFeatureBody` 把住；
// 这里只负责"库里读不出来时怎么办"—— 见 [EnsureFingerprint]。
package app

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/upstream"
)

// BeijingTimeDigits 是上报用的 14 位时间串：东八区墙钟 `yyyyMMddHHmmss`。
//
// ⚠ 时区**必须**与特征体里写死的 `timezone: -480` 同一个来源，否则等于自报
// "我在东八区，可我的时间戳是别处来的"。所以这里复用 [core.LocalTZ]，
// 而不是自己再写一遍偏移量 —— "东八区"这个事实在项目里只有一处定义。
//
// 跨 UTC 午夜那条路（`23:30Z` → 东八区已是次日 `07:30`）由测试钉住：
// 按 UTC 拼串会差一天，而 `timezone` 字段仍写着 -480。
func BeijingTimeDigits(clock core.Clock) string {
	return clock.Now().In(core.LocalTZ()).Format("20060102150405")
}

// UploadContext 是一次上报的时间上下文。
//
// 单独成为一个类型是为了**可断言**：`OldDeviceID` 进的是特征体的 `box` 字段，
// 而真实上报体是加密的，从请求里看不出来。所以"取对了旧 id"这一半在本层断言，
// "把它放进 `box`"那一半在上游断言（那里是明文比对）。
type UploadContext struct {
	// TimeDigits 是 14 位东八区时间串。
	TimeDigits string
	// NowMillis 是 `Date.now()`：特征体里 `svm` 与 `pmf` 用它。
	NowMillis int64
	// OldDeviceID 是上一次的 `deviceId`（含 `B` 前缀）；**首次上报是空串**。
	OldDeviceID string
}

// NewUploadContext 按时钟组一份上下文。
func NewUploadContext(clock core.Clock, oldDeviceID string) UploadContext {
	now := clock.Now()
	return UploadContext{
		TimeDigits:  BeijingTimeDigits(clock),
		NowMillis:   now.UnixMilli(),
		OldDeviceID: oldDeviceID,
	}
}

// AsUpload 借给上游算法看的形式。
func (c UploadContext) AsUpload() upstream.SmUpload {
	return upstream.SmUpload{
		TimeDigits:  c.TimeDigits,
		NowMillis:   c.NowMillis,
		OldDeviceID: c.OldDeviceID,
	}
}

// FingerprintBinding 是一次特征绑定的结果。
type FingerprintBinding struct {
	// Fingerprint 是与库里一致的那份特征。
	Fingerprint upstream.SmFingerprint
	// State 是库里的两列（[core.SecretValue] 的打印已脱敏）。
	State core.SmidState
	// NewlyBound 是本次调用是否**新造**了特征（true = 库里原先没有）。
	NewlyBound bool
}

// DeviceID 是上一次上报换回的 `deviceId`；false = 还没上报过。
func (b FingerprintBinding) DeviceID() (string, bool) {
	if b.State.DeviceID == nil {
		return "", false
	}
	return b.State.DeviceID.Value(), true
}

// BoxValue 是进特征体 `box` 字段的那个值：**首次上报是空串**，
// 不是 null、也不是省略字段。
func (b FingerprintBinding) BoxValue() string {
	id, ok := b.DeviceID()
	if !ok {
		return ""
	}
	return id
}

// EnsureFingerprint 读这个账号绑定的设备特征；**没有就生成一份并落库**。
//
// 纯本地操作，不打网络。绑定一旦写成就不再变，除非显式 [RotateFingerprint]。
//
// # 库里那串读不出来时报错，不重掷
//
// 悄悄生成一份新特征看着能自愈，实际是把"这个账号从某天起换了一台设备"
// 这件事变成一次没有记录的事件 —— 以后查风控问题连起点都找不到。
// 报错让人决定。
//
// ⚠ 错误文案里**不回吐**库里那串：它就是这台设备的长相，日志里不该有第二份。
func EnsureFingerprint(
	ctx context.Context, store core.StateStore, scope core.Scope,
	accountID core.AccountID, rng core.Random,
) (FingerprintBinding, *core.Error) {
	state, err := store.GetSmidState(ctx, scope, accountID)
	if err != nil {
		return FingerprintBinding{}, asAppError(err, "读设备特征失败")
	}
	if state != nil && state.FingerprintJSON != nil {
		fingerprint, perr := upstream.SmFingerprintFromJSON(state.FingerprintJSON.Value())
		if perr != nil {
			return FingerprintBinding{}, core.NewParse(
				"账号绑定的设备特征无法解析，请用 `mcloud smid reset` 重新绑定（不会自动重掷）")
		}
		return FingerprintBinding{Fingerprint: fingerprint, State: *state}, nil
	}
	return bindNewFingerprint(ctx, store, scope, accountID, rng)
}

// RotateFingerprint 是**主动更换**设备特征：生成一份新的并绑定，
// 旧的 `deviceId` 当场作废。
//
// 这是"绑定"的另一半：绑死而没有出口，等于把一次风控判定变成永久状态。
// 写新特征与清空旧 id 是**同一条 SQL**（见 [core.StateStore.SetSmidFingerprint]），
// 所以这里没有、也没法补第二步 —— 那条由
// `TestRotationReplacesTheFingerprintAndVoidsTheDeviceID` 钉住。
func RotateFingerprint(
	ctx context.Context, store core.StateStore, scope core.Scope,
	accountID core.AccountID, rng core.Random,
) (FingerprintBinding, *core.Error) {
	return bindNewFingerprint(ctx, store, scope, accountID, rng)
}

// bindNewFingerprint 生成 + 落库，两条路径共用的那一步。
func bindNewFingerprint(
	ctx context.Context, store core.StateStore, scope core.Scope,
	accountID core.AccountID, rng core.Random,
) (FingerprintBinding, *core.Error) {
	fingerprint := upstream.GenerateSmFingerprint(rng)
	text, perr := fingerprint.ToJSONString()
	if perr != nil {
		return FingerprintBinding{}, perr
	}
	if err := store.SetSmidFingerprint(ctx, scope, accountID, text); err != nil {
		return FingerprintBinding{}, asAppError(err, "写入设备特征失败")
	}
	secret := core.NewSecretValue(text)
	return FingerprintBinding{
		Fingerprint: fingerprint,
		State: core.SmidState{
			AccountID:       accountID,
			FingerprintJSON: &secret,
			DeviceID:        nil,
		},
		NewlyBound: true,
	}, nil
}

// ReportDeviceID 上报绑定的特征，换回一个新的 `deviceId` 并落库。
//
// **每次都真的打上游** —— 这是"换 id"那条路（刚 [RotateFingerprint] 过、
// 或上游明确说这个 id 作废时）。日常取 id 走 [DeviceID]。
func ReportDeviceID(
	ctx context.Context, up *upstream.Upstream, store core.StateStore, scope core.Scope,
	accountID core.AccountID, clock core.Clock, rng core.Random,
) (string, *core.Error) {
	binding, perr := EnsureFingerprint(ctx, store, scope, accountID, rng)
	if perr != nil {
		return "", perr
	}
	return uploadAndStore(ctx, up, store, scope, accountID, &binding, clock, rng)
}

// DeviceID 是这个账号现在能用的 `deviceid`：库里有就直接返回（**不打网络**），
// 没有才换一次。
//
// 业务请求该调的是这一个。绑定之所以省流量，省就在这里。
func DeviceID(
	ctx context.Context, up *upstream.Upstream, store core.StateStore, scope core.Scope,
	accountID core.AccountID, clock core.Clock, rng core.Random,
) (string, *core.Error) {
	binding, perr := EnsureFingerprint(ctx, store, scope, accountID, rng)
	if perr != nil {
		return "", perr
	}
	if id, ok := binding.DeviceID(); ok {
		return id, nil
	}
	return uploadAndStore(ctx, up, store, scope, accountID, &binding, clock, rng)
}

// uploadAndStore 组 body → 上报 → 落库。
//
// # 顺序是**先落库再返回**，不是反过来
//
// 落库失败会把这个刚换来的 id 丢掉，下次重新上报换一个 —— 那只是浪费一次请求。
// 反过来（先给请求用、再落库失败）会留下"用过了但库里没有"的状态，
// 那个更难查：请求侧以为自己带着 id，库里却查不到它是什么时候来的。
func uploadAndStore(
	ctx context.Context, up *upstream.Upstream, store core.StateStore, scope core.Scope,
	accountID core.AccountID, binding *FingerprintBinding,
	clock core.Clock, rng core.Random,
) (string, *core.Error) {
	// ⚠ 别把这个变量叫 `context` —— 它会把 `context` 那个**包**遮掉，
	// 而本函数后面还要用它往下传。遮掉之后报错指不到这里（§6.1 #38 同源）。
	uploadCtx := NewUploadContext(clock, binding.BoxValue())
	payload, perr := upstream.BuildSmData(binding.Fingerprint, uploadCtx.AsUpload(), rng)
	if perr != nil {
		return "", perr
	}
	deviceID, perr := up.SmidDeviceID(ctx, binding.Fingerprint, payload)
	if perr != nil {
		return "", perr
	}
	if err := store.SetSmidDeviceID(ctx, scope, accountID, deviceID); err != nil {
		return "", asAppError(err, "写入 deviceId 失败")
	}
	return deviceID, nil
}

// asAppError 把 [core.StateStore] 那类返回 `error` 的错误转成 [*core.Error]。
//
// ⚠ 不要在同一作用域里复用 `err` 这个名字接两种错误类型 —— 见 §6.1 #38。
func asAppError(err error, what string) *core.Error {
	if perr, ok := err.(*core.Error); ok {
		return perr
	}
	return core.NewInternal(what + ": " + err.Error())
}
