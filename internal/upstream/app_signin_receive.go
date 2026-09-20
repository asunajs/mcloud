// 领取族：`receiveV3`（领云朵）/ `receiveRevivalReward`（用复活卡·回馈卡）/
// `receiveTaskExpansion`（延期领取）/ `multiple`（签到翻倍）。
//
// 对应 Rust 侧的 app_signin/receive.rs + revival.rs。
//
// # 这四个接口的分工由 `cloudType` 决定
//
//	cloudType = 0  → 现在可领             → receiveV3
//	cloudType = 1  → 已到期、现在能领     → receiveTaskExpansion（要 acceptDate）
//	cloudType = 2  → 下月可领取           → **什么都不发**（H5 只弹一句 toast）
//
// 见 [ReceiveItem.CloudType] 里的分流源码与证据。
package upstream

import (
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 领云朵
// ─────────────────────────────────────────────────────────────────────────────

// ReceiveV3Request 构造 `POST /ycloud/signin/page/receiveV3` —— **领云朵**。
//
// # 为什么 `deviceID` 是必填
//
// 它是这一族里**已知唯一一个"不带就失败"**的接口（H5 标了 `isDeviceId: true`）。
// 所以让它凑不出空值 —— 漏发在编译期就暴露。
//
// ⚠ 这里要的是 **H5 那个 65 字节 blob**（iOS `SMSdk.getDeviceId()`），
// **不是** `smid` 产出的 `B<uuid>`。两者同名不同物。
//
// # `cloudType` 为什么是可选
//
// H5 是 `e.cloudType ?? ""` —— 缺省给的是**空串**，不是 `0`。
// 照抄，别"顺手改成 0"：`0` 是合法取值（[CloudTypeNow]，当前可领取）。
func ReceiveV3Request(
	jwt, referer, cloudID string, cloudType *int64, deviceID string,
) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"client":    Client,
		"cloudId":   cloudID,
		"cloudType": "",
		"deviceId":  deviceID,
	}
	if cloudType != nil {
		body["cloudType"] = *cloudType
	}
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/signin/page/receiveV3", BaseMcloud)).WithJSONBody(body)
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithH5Headers(request, referer, &jwt, "true").
		WithHeader("isdeviceid", "true"), nil
}

// ReceiveResult 是 `receiveV3` 的解析结果。
type ReceiveResult struct {
	// Receive 是本次领到的云朵数（`result.receive`）。
	//
	// H5 拿它做弹窗动画，所以它就是"这次进账多少"。
	// ⚠ 与 [SignInStatus.SignInPoints] 不是一回事：那个是"签到能拿多少"（还没到手）。
	Receive int64
}

// ParseReceiveV3 解析 `receiveV3` 的响应。
//
// 失败分支不走这里 —— `11029` / `614` / `615` 都由 [EnsureOKWithHint] 转成
// 带排查提示的错误。
func ParseReceiveV3(envelope *Envelope) (ReceiveResult, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return ReceiveResult{}, err
	}
	if payload == nil {
		return ReceiveResult{}, core.NewParse("receiveV3 响应没有 result")
	}
	receive, err := RequireInt64(payload, "receive")
	if err != nil {
		return ReceiveResult{}, err
	}
	return ReceiveResult{Receive: receive}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 复活卡 / 回馈卡
// ─────────────────────────────────────────────────────────────────────────────

// ReceiveRevivalRewardRequest 构造 `POST /ycloud/signin/page/receiveRevivalReward`。
//
// body 是**空对象** `"{}"`（与抓包的 `content-length: 2` 对齐）——
// 不是"没有 body"。没有查询参数、没有别的字段。
//
// `showLoading` 是 `"false"`：同页其余接口几乎都是 `true`，只有它是 `false`
// （H5 要自己画那张卡片的弹窗，不想要全局 loading 遮住）。
//
// # `deviceID`：两个证据不一致，两个分支都留着
//
//	H5 源码（iOS 12.6.0）        → 头里**没有** isDeviceId ⇒ 拦截器不注入
//	老抓包（Android WebView）    → 带一个 `deviceid` **头**
//
// 所以做成可选：nil = 照 H5 定义（缺省，推荐）；给了才补一个 `deviceid` 头。
// **哪个是服务端真正要求的，未验证** —— 这正是留成可选的意义：
// 真出问题时能一次 A/B 试出来（`receiveV3` 的必填就是这么定的）。
//
// ⚠ 这里**不加** `isdeviceid` 头（老抓包里也没有）—— 别照 [ReceiveV3Request] 抄。
func ReceiveRevivalRewardRequest(
	jwt, referer string, deviceID *string,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/signin/page/receiveRevivalReward", BaseMcloud)).
		WithJSONBody(map[string]any{})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = WithH5Headers(request, referer, &jwt, "false")
	if deviceID != nil {
		request = request.WithHeader("deviceid", *deviceID)
	}
	return request, nil
}

// RevivalReward 是 `receiveRevivalReward` 的解析结果 —— **用掉一张卡，换回一批云朵**。
//
// 实拍响应（`test/fixtures/http/复活.json`）：
//
//	{"code":0,"msg":"success",
//	 "result":{"cardType":1,"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}
type RevivalReward struct {
	// CardType：`1` = **复活卡**（随机复活上一年失效的云朵），`2` = **回馈卡**。
	//
	// H5 用 `1 == cardType ? "复活卡弹窗" : "回馈卡弹窗"` 分流，
	// 两张卡连累计数的**文案单位都不同**。所以它是判据不是装饰。
	CardType int64
	// RewardClouds 是本次随机复活到的云朵数 —— 这就是"这次进账多少"。
	RewardClouds int64
	// TotalClouds 是累计数（复活卡口径下是"累计复活AI豆"）。
	//
	// ⚠ **别把它接进余额记录**：它到底是不是"云朵余额"口径**没有验证**
	// —— H5 文案说的是"复活卡把失效云朵**转换成 AI豆**"。
	// 现在接进去只会把两个口径混进同一条曲线。
	TotalClouds int64
	// LittleFirstTime 只影响一句文案（"下月再来可获得AI豆回馈卡！" vs "…复活卡！"）。
	//
	// 所以它是**附属字段**：服务端不给就给 0，不因此判整条响应失败 ——
	// 判据是"缺了它还能不能回答本函数的问题"，它答不了任何问题，只是换个措辞。
	LittleFirstTime int64
}

// ParseRevivalReward 解析 `receiveRevivalReward` 的响应。
func ParseRevivalReward(envelope *Envelope) (RevivalReward, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return RevivalReward{}, err
	}
	if payload == nil {
		return RevivalReward{}, core.NewParse("receiveRevivalReward 响应没有 result")
	}
	out := RevivalReward{}
	for _, spec := range []struct {
		key string
		dst *int64
	}{
		{"cardType", &out.CardType},
		{"rewardClouds", &out.RewardClouds},
		{"totalClouds", &out.TotalClouds},
	} {
		v, err := RequireInt64(payload, spec.key)
		if err != nil {
			return RevivalReward{}, err
		}
		*spec.dst = v
	}
	out.LittleFirstTime = intAt(payload, "littleFirstTime")
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 延期领取（cloudType == 1）
// ─────────────────────────────────────────────────────────────────────────────

// ReceiveTaskExpansionRequest 构造
// `GET /ycloud/signin/page/receiveTaskExpansion` —— **延期领取**。
//
// **只有查询参数 `acceptDate`**、**没有 body**、**没有 `isDeviceId`**
// （H5 没标 ⇒ 拦截器不注入）。别照 [ReceiveV3Request] 抄那套。
//
// # `acceptDate` 为什么是可选（这条被实拍推翻过两次）
//
//   - H5 的调用点是 `receiveTaskExpansion({acceptDate: t})`，取自**同一条条目**；
//   - 而**实测两条账号的 `receiveList` 里都没有 `acceptDate` 这个字段**
//     （原始响应里只有 `cloudNum` / `cloudType`）⇒ 那个值是 `undefined`；
//   - axios 的默认序列化器对 `undefined` 是**整个键丢掉**（不是发 `acceptDate=`）
//     ⇒ 真实客户端发出去的是**裸 GET，一个查询参数都没有**。
//
// 所以 nil → 不带查询串（**与真实客户端一致，是缺省路径**）。
//
// ⚠ 别把 nil 写成空串 —— 那会发出 `?acceptDate=`，是**另一个请求**。
func ReceiveTaskExpansionRequest(jwt, referer string, acceptDate *string) core.HTTPRequest {
	url := fmt.Sprintf("%s/ycloud/signin/page/receiveTaskExpansion", BaseMcloud)
	if acceptDate != nil {
		url = fmt.Sprintf("%s?acceptDate=%s", url, PercentEncode(*acceptDate))
	}
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// ReceiveTaskExpansion 是 `receiveTaskExpansion` 的解析结果（月度任务奖励）。
//
// # 这条线是什么
//
// 「本月签到/做任务攒记录 → **下个月**结算 → 下个月来领」，且**要开着自动备份**。
// 所以它在 `receiveList[]` 里有两个状态（同一特性的两个时点）：
// `1` = 已到期、现在能领；`2` = 还没到期（只提示，不发请求）。
type ReceiveTaskExpansion struct {
	// CloudCount 是本次领到多少云朵（豆）。
	//
	// nil = 上游没给（**不是 0**）：0 是"领到 0 朵"，nil 是"没说"。
	//
	// ⚠ 本仓库**没有任何这条接口的实拍响应**，所以这里不用"缺了就判失败"那套约定：
	// 没有实拍还硬要求字段，等于把猜测变成断言 ——
	// 第一次真跑就会以一个看不懂的解析错误告终。
	CloudCount *int64
}

// ParseReceiveTaskExpansion 解析 `receiveTaskExpansion` 的响应。
//
// ⚠ **没有 `result` 不算失败。** 两个客户端都只拿 `code` 判成败：
// H5 的 `.then` 里只比 `data.code`（`0` / `1302` / `500`），
// 旧项目写的是 `receiveData?.result?.cloudCount` —— `?.` 就是"没有也无所谓"。
//
// 这里曾经把"没有 result"报成解析错误，那是**比客户端更严**：
// 真跑起来会以一句解析错误盖掉上游真正想说的话（比如 `1302` 未开备份）。
func ParseReceiveTaskExpansion(envelope *Envelope) (ReceiveTaskExpansion, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return ReceiveTaskExpansion{}, err
	}
	out := ReceiveTaskExpansion{}
	if v, ok := intAtOK(payload, "cloudCount"); ok {
		out.CloudCount = &v
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 签到翻倍
// ─────────────────────────────────────────────────────────────────────────────

// MultipleRequest 构造 `GET /ycloud/signin/page/multiple` —— **签到翻倍**。
//
// # 它领什么
//
// 「自动备份」每月给**一次**翻倍机会：本月开着备份、且这次机会还没用掉时，
// 领一次能把**今日签到奖励**翻 2–50 倍。
//
// ⚠ 与 [ReceiveTaskExpansionRequest] 同一个坑：H5 传的 `params` 是**空对象**，
// axios 默认序列化器**一个查询参数都不产生** ⇒ 发的是**裸 GET**，不是 `?`。
//
// # 门槛**必须调用方自己判**
//
// 旧项目的判据逐字：`if (curMonthBackup && !curMonthBackupSignAccept) { … }` ——
// 两个值都在 `infoV3` 里（[SignInStatus.CurMonthBackup] /
// [SignInStatus.CurMonthBackupSignAccept]）。**两个都成立才去领**：
// 每月只有一次机会，条件不成立还发，是**白白消耗**。
func MultipleRequest(jwt, referer string) core.HTTPRequest {
	return WithH5Headers(
		core.NewRequestGet(fmt.Sprintf("%s/ycloud/signin/page/multiple", BaseMcloud)),
		referer, &jwt, "true")
}

// MultipleResult 是 `multiple` 的解析结果 —— 这一次额外领到多少豆。
type MultipleResult struct {
	// CloudCount 是本次额外获得的豆数（`result.cloudCount`）。
	//
	// H5 逐字读它：`result.cloudCount` → 「额外获得 {cloudCount} 豆」。
	//
	// nil 的原因与 [ReceiveTaskExpansion.CloudCount] 同：**没有实拍**
	// （两个账号本月都已领过，复现不出来），且缺了不判失败 ——
	// 硬要求它会把一次**成功**的领取变成解析错误。
	CloudCount *int64
	// Multiple 是**倍数**（N 倍）。
	//
	// 旧项目逐字读了它并拼成一句日志：「成功获得 {multiple} 倍云朵，共计 {cloudCount}」
	// —— 即 **`multiple` 是倍数、`cloudCount` 是这次的合计**，两者不是一个量。
	// 只报 `cloudCount` 会让"翻了几倍"这个信息丢掉。
	Multiple *int64
}

// ParseMultiple 解析 `multiple` 的响应。
//
// `result` 缺失**不判失败** —— 理由同 [ParseReceiveTaskExpansion]。
func ParseMultiple(envelope *Envelope) (MultipleResult, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return MultipleResult{}, err
	}
	out := MultipleResult{}
	if v, ok := intAtOK(payload, "cloudCount"); ok {
		out.CloudCount = &v
	}
	if v, ok := intAtOK(payload, "multiple"); ok {
		out.Multiple = &v
	}
	return out, nil
}
