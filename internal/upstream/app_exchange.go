// 兑换族：`exchangeList`（只读清单）与 `exchangeV3`（下单）。滑块取题在
// [`app_exchange_slide.go`]（app_exchange_slide.go）—— 它在这条链上，但那是另一张口。
//
// # 这一族是全仓库**唯一**"写操作有三条真机实拍"的链路
//
// 逐字实拍、判据链与五个待核项都在 [`docs/capture/app-exchange-samples.txt`]
// (../docs/capture/app-exchange-samples.txt) §5：那里有 4 条 `exchangeV3`（3 错 1 成）、
// 3 条 `exchangeList`、以及"余额 10505 → 8705 == 商品价格 1800"这条对拍。
// 所以本文件与 `app_msg_push.go` 那一种"照抄旧项目文字描述"不同 —— **每一条 URL、
// 每一个字段名、每一个码都能在 §5 里指到一行原文**。
//
// 但**实拍齐了不等于口径齐了**，下面这些仍然是空的（本文件不替它们编答案）：
//
//   - `420` / `421`（用户级每日 / 每月配额）**一次都没露面**，`msg` 文案只有 H5 那张表；
//   - `smsCode` 究竟何时非空：**有意搁置**（用户 2026-09-22 裁定），四条实拍全是空串；
//   - `limit == 1` 的取值：本轮样本里**一件都没有**，只有 H5 的文案在支撑；
//   - `probability2` 里除 `limitMonthQuantity` / `monthGroupLimit` 之外那几个开关
//     （`limitBeforeSendChinaMobile` / `limitOfficial` / `multi`）各自管什么：**未验证**。
//
// # ⚠ 业务码**不是**错误
//
// [Upstream.Call] 只在传输层与非 2xx 上回错误，`code != 0` 一律交回调用方判 ——
// 这一族**必须**保留那个码：`514`（滑块算错）与 `2301`（重复兑奖）处置完全不同
// （前者可以重取滑块再来，后者再怎么试都不会成交），把它们折成一句"上游返回 code=N"
// 的错误，表现就是"该重试的没重试、不该重试的试了三次"。
// 所以下面的 [ParseExchangeResult] **不走** [EnsureOKWithHint]。
//
// # ⚠ 这一族会真扣云朵，且**不可回滚**
//
// 上游侧唯一的防线是它自己的五道闸（`501` → `514` → `407` → `415` → `2301`，
// 见 §5.5 ③），请求里**没有任何幂等键 / 客户端单号** —— `oid` 由服务端每次新生成，
// 不能当幂等依据（§5.2 末）。⇒ "超时之后要不要重发"这一题的正确答案是**不发**：
// 网络错误时上游可能已经成交（判据链第 2 条那句"余额差"才说得清成没成）。
// 这条口径落在领域层 `internal/app/exchange_run.go`，不在这里。
package upstream

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 常量（全部指得到实拍那一行）
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeClientVersion 是 `exchangeList` 的查询参数与 `exchangeV3` 请求体里的
// `clientVersion` —— 实拍 **`12.6.0`**（三段）。
//
// ⚠ 它**不等于**请求头 `appversion` 那个 `12.6.0.0`（四段，[H5AppVersion]）。
// 两个值在同一个请求里同时出现且不一样（§5.1 / §5.2），别互抄。
//
// 取值与 [TaskClientVersion]、[CaiyunAppVersion] 相同 —— 这里仍单列一个名字而不是
// 复用它们：这三处是**三个不同请求上的三个字段**，哪天上游按接口分档灰度，
// 它们会各自漂移；共用一个常量等于把"要不要跟着改"这道判断题替调用方做掉。
const ExchangeClientVersion = "12.6.0"

// ExchangeTimeLayout 是 `startTime` / `endTime` 的实拍形态（逐字）：
// `"2026-07-01 09-30-00"` / `"2026-12-31 23-59-59"`。
//
// ⚠ 时间部分的**分隔符也是 `-`** 而不是 `:`。写成 `15:04:05` 会解不出来，
// 而解不出来的表现是"这一件商品永远判成窗口未知"——不报错，只是不发。
const ExchangeTimeLayout = "2006-01-02 15-04-05"

// ExchangeTimeLocation 是把上面那两个**无时区**串解释成时刻时用的时区：东八区（固定 +8，
// 与 [FunaiActivityLocation] 同一条理由）。
//
// ⚠ 实拍那两个串**不带**时区信息。取 +8 的依据是同一次兑换的订单字段
// `insertTime: 2026-09-22T04:16:25.453+00:00` == 用户在真机上操作的 12:16:25（CST），
// 即**这一族的上游时刻按北京时间读写**。⚠ 但那是**带时区的 ISO 串**给的旁证，
// `startTime` 这类裸串**没有自己的时区证据** ⇒ 属推断。
//
// 它只影响"窗口开没开"这一档，而那一档判错的方向是**不发**（见领域层的窗口判定）。
func ExchangeTimeLocation() *time.Location { return time.FixedZone("UTC+8", 8*3600) }

// `limit` 的五档 —— 本账号对**这一件**奖品的可兑状态。
//
// # 每一档的证据强度不一样，别当成一张同级字典
//
//   - `0` / `2` / `4` **有字段依据**：`0` 是实拍里可兑那件的取值；`2` 由"成功兑掉 `251230053`
//     之后它的 `limit` 由 `0` 变 `2`"钉住（§5.4）；`4` 由 `probability2` 里的
//     `monthGroupLimit`（**数组**）+ `limitMonthQuantity: 1` 钉住 —— 同组另外三件全是 `4`，
//     而我兑过的只有那一件（§5.6 ②）。
//   - `1` **只有 H5 那句文案**（"明日再来"）。本轮样本里 `limit==1` 的一件都没有。
//   - `3` 两处文案**互相冲突**：H5 说"已兑换，查看奖品"，原项目 `mcloud-sign` 的
//     `resolvePrizeId` 说"暂不可用"。⇒ **存疑**，别挑一个信（处置见领域层：一样是不发）。
//
// ⚠ 这一族取值**不能与原项目那一份混用**：原项目走的是
// `/market/signin/page/exchangeList`（无 `/ycloud`），那是**交叉印证**、
// 不是一份可外推的字典（§5.1 那段补记已经把这条写死了）。
const (
	// ExchangeLimitAvailable 是 `0`：可兑换（H5 还要配合 `minRemainderCount != 0` 才亮按钮）。
	ExchangeLimitAvailable int64 = 0
	// ExchangeLimitTomorrow 是 `1`：文案"明日再来"。⚠ **零样本**，只有 H5 那句文案。
	ExchangeLimitTomorrow int64 = 1
	// ExchangeLimitExchanged 是 `2`：**这一件本身**本月已兑过。
	ExchangeLimitExchanged int64 = 2
	// ExchangeLimitUnknown3 是 `3`：两处文案冲突那一档（见上面的分组注释）。
	ExchangeLimitUnknown3 int64 = 3
	// ExchangeLimitGroupExchanged 是 `4`：**同组别的件**本月已兑（组名额用掉了）。
	ExchangeLimitGroupExchanged int64 = 4
)

// 兑换下单的业务码。**只有标了"实拍"的那几个见过**，其余是从 H5 那张表抄来的名字。
//
// ⚠⚠ 这张表**不是全集**：`415` / `501` / `2301` 就都不在 H5 客户端认识的那六档里
// （它只认 `0 / 514 / 610 / 407 / 420 / 421 / 614 / 615`），而 `2301` 是**四位**、
// 其余全是三位 ⇒ **别按"码都是三位"的形状去写解析，也别按形状猜语义**。
const (
	// ExchangeCodeSliderParamMissing 是 `501 滑块验证参数不能为空`（实拍）。
	//
	// 触发条件已实测：`puzzleOffset` 传空串与传 `null` **逐字相同**地回这一码，
	// 即服务端按"有没有值"处理，不区分那两种形态（§6 第 1 条）。
	// 它是**第 1 道闸**：排在滑块正确性、用户资格、库存之前。
	ExchangeCodeSliderParamMissing int64 = 501
	// ExchangeCodeSliderFailed 是 `514 验证失败`（实拍 3 条）。
	//
	// 含义是"**offset 非空但算错了**"，不是"没带 offset"（那是 `501`）。
	// ⚠ 三条实拍的偏差分别是 59 / 47 / 176 像素，而**成功**那两条的偏差是 0 与 4
	// ⇒ 容忍度夹在约 ±4~9px；**不要主动加抖**（§5.3.1 ③：加 `+10` 抖那次被拒了，
	// 原本能过）。
	// 这一码的**代价是零**：三条 `514` 期间余额始终没动（§6 第 7 条）⇒ 只有这一档
	// 可以重取滑块再来一次。
	ExchangeCodeSliderFailed int64 = 514
	// ExchangeCodeNotTargetUser 是 `407`（实拍）。
	//
	// ⚠ 它是**账号级**闸门，不是奖品级：两件**不同**奖品（流量包 / 第三方权益）
	// 回的是**同一个码**。上游那句 `msg` 是「非移动用户不可领奖」
	// 而 H5 那张表里 `407` 的文案是"您非目标用户，无法兑换" —— **两种说法**，
	// 我们只有一号段的实测（电信号），所以按实拍那句读：**不是移动号码就兑不了任何奖**。
	ExchangeCodeNotTargetUser int64 = 407
	// ExchangeCodePrizeExhaustedToday 是 `415 奖品单日已耗尽`（实拍，**不在 H5 表里**）。
	//
	// ⚠ 说的是**奖品的池子**空了，不是"我这个账号的份额用完" —— 后者是 `420`。
	// 混这两档的后果是"明天再来"与"这件别再试了"说反。
	ExchangeCodePrizeExhaustedToday int64 = 415
	// ExchangeCodeDuplicated 是 `2301 重复兑奖`（实拍，**既不在 H5 表里、
	// 也不在参考项目那张贴文案白名单里**）。
	//
	// ⭐ 它是这条链上**服务端有幂等语义**的唯一实证：同一账号 + 同一 `prizeId`
	// 不重复发奖。⚠ 它挡的是**永久**还是**本月**仍无样本（要跨月才能分开）。
	ExchangeCodeDuplicated int64 = 2301

	// ExchangeCodeGeneric 是 `610`：H5 文案"兑换失败，请稍后再试或联系客服~"。**未观测**。
	ExchangeCodeGeneric int64 = 610
	// ExchangeCodeBusyA / ExchangeCodeBusyB 是 `614` / `615`：H5 给这两档**同一句文案**。
	//
	// ⚠⚠ 别照文案分类 —— 必须读 `code`。同两个数在**签到族**里另有已实证的含义
	// （[CodeReceiveFailedAlt] / [CodeReceiveFailed]：那条线是 deviceId 对不对），
	// 在**这一族**里究竟是不是同一回事，**没有样本** ⇒ 这里只登记名字，不做任何判定。
	ExchangeCodeBusyA int64 = 614
	ExchangeCodeBusyB int64 = 615

	// ExchangeCodeDailyQuota 是 `420`：H5 文案"该奖品每日限兑，您今日份额已用完"。
	// ⚠ **一次都没露面**。它**没有零豆替代**（要看到得同一件当天兑成功两次）。
	ExchangeCodeDailyQuota int64 = 420
	// ExchangeCodeMonthlyQuota 是 `421`：H5 文案"该奖品每月限兑，您本月份额已用完"。
	// ⚠ 同样**零样本**；探针候选 `251230052`（`limit 4` + 日余 49）**只被一道闸挡**
	// ⇒ 那一发**可能真扣 1800 豆**，要用户拍板才发（§5.6 ③）。
	ExchangeCodeMonthlyQuota int64 = 421
)

// ─────────────────────────────────────────────────────────────────────────────
// `exchangeList`（只读）
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeListRequest 构造 `GET /ycloud/signin/page/exchangeList` —— 兑换清单。**只读**。
//
// 查询串**只有** `client` 与 `clientVersion` 两个参数（§5.1 逐字），
// **不带 `deviceId`**（H5 那条只标了 `showLoading`，没标 `isDeviceId`）。
//
// ⚠ 主机是 [BaseMcloud]（`m.mcloud.139.com`），**不是**参考项目那个
// `/market/signin/page/exchangeList` —— 两条不是同一个接口，各自实测，别假设同一个 base。
func ExchangeListRequest(jwt, referer string) core.HTTPRequest {
	request := core.NewRequestGet(fmt.Sprintf(
		"%s/ycloud/signin/page/exchangeList?client=%s&clientVersion=%s",
		BaseMcloud, Client, ExchangeClientVersion))
	return WithH5Headers(request, referer, &jwt, "true")
}

// ExchangeMonthQuota 是 `probability2` 那个 JSON 串里**与限兑有关**的两个字段。
//
// # 为什么要单独一层（而不是把 probability2 当字符串交出去）
//
// `limit == 4` 这一档的**字段依据**就在这里：`monthGroupLimit` 列的是
// "每月只能兑其中一件"的那组 prizeId，配合 `limitMonthQuantity` 给出组额度（§5.6 ②）。
// 判"这件本月还能不能兑"离不开它。
//
// ⚠⚠ **这两个字段的类型不一样**：`limitMonthQuantity` 是**数字**、`monthGroupLimit`
// 是**字符串数组**。坑表 #105 记的就是"按'都是数字'读，后者**静默归零**且不报错" ——
// 于是"同组三件全是 4"这件事当时解释不了。所以这里**分别**解析，各走各的类型。
//
// ⚠ 同一个串里另有 `multi` / `limitBeforeSendChinaMobile` / `limitOfficial`
// 与两个配色字段（`btnBg` / `itemBg`）。**都不解析**：没有任何一条证据说明
// 它们在服务端怎么参与判定（§5.6 ② 那份逐字对照里四件只有配色不同）。
type ExchangeMonthQuota struct {
	// MonthQuantity 是 `limitMonthQuantity`：这个组每月能兑几件。nil = 上游没给。
	MonthQuantity *int64
	// GroupPrizeIDs 是 `monthGroupLimit`：同组互斥的那些 prizeId（**字符串**形态）。
	// nil 切片 = 上游没给这个键（与"给了空数组"不同，后者是 len==0 且非 nil）。
	GroupPrizeIDs []string
}

// ExchangePrize 是清单里的一件商品。
//
// # 为什么只声明这些字段
//
// 实拍一条有 26 个键，其中 `imageUrl` / `memo` / `oid` / `canEdit` / `allowInside` /
// `areaCode` / `sort` / `updateTime` 是渲染与投放用的，本能力一个都不读。
// 留在这里的每一条都要么是**判据**（`limit` / `pOrder` / 三个余量 / `onLine` /
// 窗口两串），要么是判据的**字段依据**（`probability2` 那两个、`monthGroupLimit`）。
//
// ⚠ `count` / `dailyRemainderCount` / `minRemainderCount` 是**时刻量**：
// 实拍里它们会因为**别的用户**在同一分钟里兑掉而变化（`839 → 837`，§5.4）。
// ⇒ 只能用来判"现在还有没有"，**不能**当个人流水。真正只反映"我这个账号"的是 `limit`。
//
// 判据字段一律做成指针："上游没给"与"给了 0"在**这一族**是两种答案 ——
// 例如 `minRemainderCount` 缺了不等于"余量为 0（别试了）"。
type ExchangePrize struct {
	// PrizeID 是 `prizeId` —— 下单要回传的那个数。
	// ⚠ 实拍它是**数字**（`260630012`），不是字符串；按 string 断言会静默拿到空串。
	PrizeID int64
	// Name 是 `prizeName`。
	Name *string
	// GroupID 是 `groupId`（顶层那个分组键的**值**，实拍是数字）。
	GroupID *int64
	// Type 是 `type` —— 品类（`1057` tokens 包 / `1065` 云空间·会员 / `1035` 第三方权益）。
	//
	// H5 里有一条按它的拒绝：`D.value && 1035 == t.type ⇒ "当前客户端类型或版本不可兑换"`。
	// 我们只有 `1035` 这一档的实拍（兑成功那件就是它），**不据此做任何判定**。
	Type *int64
	// Price 是**兑换所需云朵数**。
	//
	// ⚠⚠ 实拍里这件有**两种拼法**：同一份响应里 `"POrder":50` 与 `"pOrder":50`
	// 同时存在（§5.1）。只读一个键的写法会**静默**把一部分商品价格判成"没给"，
	// 于是"余额够不够"这道判断对那几件恒假 ⇒ [parseExchangePrice] 两键都试。
	Price *int64
	// Limit 是本账号对**这一件**的可兑状态，见 [ExchangeLimitAvailable] 那五档。
	Limit *int64
	// Count 是当前剩余（`count`）；TotalCount 是总投放（`totalCount`）。
	Count      *int64
	TotalCount *int64
	// DailyCount / DailyRemainderCount 是**每日**限额与今日余量。
	DailyCount          *int64
	DailyRemainderCount *int64
	MinRemainderCount   *int64
	OnLine              *int64
	NextTimeStatus      *int64
	PrizeSectionStatus  *int64
	StartTime           *string
	EndTime             *string
	MarketName          *string
	Quota               *ExchangeMonthQuota
}

// CanExchangeNow 报告"按上游给的状态，这一件现在是不是可兑"。
//
// 判据是 H5 那条按钮逻辑：`limit == 0` **且** `minRemainderCount != 0`（§5.1 字段表）。
// ⚠ 两个判据**都要指针有值**才敢返回 true —— 缺任一个返回 false（"判不了"），
// 因为这一族 false 的代价只是"不发"，而 true 的代价是**真扣豆**。
// 想区分"判不了"与"确定不可兑"的调用方，直接读那两个字段自己判。
func (p ExchangePrize) CanExchangeNow() bool {
	if p.Limit == nil || p.MinRemainderCount == nil {
		return false
	}
	return *p.Limit == ExchangeLimitAvailable && *p.MinRemainderCount != 0
}

// ExchangeList 是 `exchangeList` 的解析结果。
type ExchangeList struct {
	// Groups 的键是 `groupId`（**字符串**形态：顶层键在 JSON 里就是串）。
	//
	// ⚠⚠ 键**不连续**：实拍九个是 `0,1,2,5,7,8,9,11,15`。
	// 按 `0..n` 遍历会静默漏掉一半，而且不报错 —— 所以这里给 map 而不是切片，
	// 遍历只能靠 [ExchangeList.All]。
	Groups map[string][]ExchangePrize
	// Order 是 [Groups] 的键按**数值**升序（`"10"` 排在 `"2"` 后面，不是字典序）。
	//
	// 存在的意义只有一个：界面与小结要**确定序**输出。JSON 对象没有顺序承诺，
	// Go 的 map 遍历又是随机的 —— 不给这一份，同一份响应两次渲染会给出两份不同清单。
	Order []string
}

// All 按 [ExchangeList.Order] 那个顺序摊平所有件。
func (l ExchangeList) All() []ExchangePrize {
	out := make([]ExchangePrize, 0, 32)
	for _, key := range l.Order {
		out = append(out, l.Groups[key]...)
	}
	return out
}

// Find 按 prizeId 定位一件。找不到给 false（不猜"大概是同名那件"）。
func (l ExchangeList) Find(prizeID int64) (ExchangePrize, bool) {
	for _, item := range l.All() {
		if item.PrizeID == prizeID {
			return item, true
		}
	}
	return ExchangePrize{}, false
}

// ParseExchangeList 解析 `exchangeList` 的响应。
//
// 非 0 码走错误（这一条是只读查询，没有"非 0 但仍是数据"的那种码）。
//
// ⚠ 顶层 `result` 必须是**对象、按 groupId 分组**（§6 第 6 条已实测）。
// 上游哪天改成数组，这里要**立刻响** —— 顺手兼容两种形状会把形状变化藏起来，
// 表现是"清单忽然空了但每一枪都成功"。
func ParseExchangeList(envelope *Envelope) (ExchangeList, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return ExchangeList{}, perr
	}
	groups, ok := payload.(map[string]any)
	if !ok {
		return ExchangeList{}, core.NewParse(
			"exchangeList 的 result 不是对象（实拍是按 groupId 分组的对象，该接口不给数组）")
	}

	out := ExchangeList{Groups: make(map[string][]ExchangePrize, len(groups))}
	for key, raw := range groups {
		items, ok := raw.([]any)
		if !ok {
			// 某一组不是数组：整份清单作废，而不是**跳过那一组**。
			// 跳过的代价是"用户要的奖品明明在上游、我们说查无此件" ——
			// 那比一次解析失败难发现得多。
			return ExchangeList{}, core.NewParse(
				"exchangeList 的分组 " + key + " 不是数组")
		}
		parsed := make([]ExchangePrize, 0, len(items))
		for _, item := range items {
			prize, perr := parseExchangePrize(item)
			if perr != nil {
				return ExchangeList{}, perr
			}
			parsed = append(parsed, prize)
		}
		out.Groups[key] = parsed
	}
	out.Order = sortedGroupKeys(out.Groups)
	return out, nil
}

// parseExchangePrize 解析一件。
//
// 只有 `prizeId` 是**硬**要求：它是下单的键，缺了这条就没法用，而"静默当作 0"
// 的后果是**打到另一件商品上**（真扣豆）。其余字段一律可选（缺 = nil）。
func parseExchangePrize(raw any) (ExchangePrize, *core.Error) {
	obj, ok := raw.(map[string]any)
	if !ok {
		return ExchangePrize{}, core.NewParse("exchangeList 里有一条商品不是对象")
	}
	prizeID, ok := asInt64(obj["prizeId"])
	if !ok {
		return ExchangePrize{}, core.NewParse("exchangeList 里有一条商品缺 prizeId")
	}
	quota, perr := parseMonthQuota(obj["probability2"])
	if perr != nil {
		return ExchangePrize{}, perr
	}
	return ExchangePrize{
		PrizeID:             prizeID,
		Name:                optStrField(obj, "prizeName"),
		GroupID:             optInt64Field(obj, "groupId"),
		Type:                optInt64Field(obj, "type"),
		Price:               parseExchangePrice(obj),
		Limit:               optInt64Field(obj, "limit"),
		Count:               optInt64Field(obj, "count"),
		TotalCount:          optInt64Field(obj, "totalCount"),
		DailyCount:          optInt64Field(obj, "dailyCount"),
		DailyRemainderCount: optInt64Field(obj, "dailyRemainderCount"),
		MinRemainderCount:   optInt64Field(obj, "minRemainderCount"),
		OnLine:              optInt64Field(obj, "onLine"),
		NextTimeStatus:      optInt64Field(obj, "nextTimeStatus"),
		PrizeSectionStatus:  optInt64Field(obj, "prizeSectionStatus"),
		StartTime:           optStrField(obj, "startTime"),
		EndTime:             optStrField(obj, "endTime"),
		MarketName:          optStrField(obj, "marketName"),
		Quota:               quota,
	}, nil
}

// parseExchangePrice 取价格，`pOrder` 与 `POrder` 两个键都认。
//
// 先试小写那个：实拍里两种拼法**同时存在**（§5.1 那条逐字里 `"POrder":50` 与
// `"pOrder":50` 都在），而 H5 消费点用的是 `t.pOrder`。
// 两个键都在且**值不同**时会取前者 —— 那种情况**没有样本**，
// 真要判就得先在活体上抓一份两键不同的清单。
func parseExchangePrice(obj map[string]any) *int64 {
	if price := optInt64Field(obj, "pOrder"); price != nil {
		return price
	}
	return optInt64Field(obj, "POrder")
}

// parseMonthQuota 解析 `probability2` —— 它本身是一个 **JSON 字符串**，要二次解码。
//
// 三条口径：
//   - 键不存在 / 值不是串 ⇒ nil（"上游没给"，不是"没有月限"）；
//   - 给了但**解不出 JSON** ⇒ 报错，不静默当"没有这一层"。理由：这一层是 `limit == 4`
//     的**字段依据**，把它读成"没给"会让"本月组内名额已用掉"这一档**静默判成可兑** ——
//     而可兑的后果是发一笔真扣豆的请求；
//   - 空串（`""`）按"没给"处理：它不是"一个合法的 JSON"，硬报错会让所有没配这个字段的
//     商品把整份清单打挂。⚠ 空串这一档**没有样本**，是按"最保守可用信息"给的处置。
func parseMonthQuota(raw any) (*ExchangeMonthQuota, *core.Error) {
	text, ok := raw.(string)
	if !ok {
		return nil, nil
	}
	if strings.TrimSpace(text) == "" {
		return nil, nil
	}
	value, ok := core.ParseJSON(text)
	if !ok {
		return nil, core.NewParse("probability2 不是合法 JSON（实拍它是一个 JSON 串，形态变了）")
	}
	obj, ok := value.(map[string]any)
	if !ok {
		return nil, core.NewParse("probability2 解出来不是对象")
	}
	quota := ExchangeMonthQuota{MonthQuantity: optInt64Field(obj, "limitMonthQuantity")}
	if items, ok := obj["monthGroupLimit"].([]any); ok {
		// 注意：**给了空数组**与**没这个键**是两种答案，所以在这里才建切片。
		quota.GroupPrizeIDs = make([]string, 0, len(items))
		for _, item := range items {
			// ⚠ 实拍是**字符串**形态的 prizeId（`"251230051"`），不是数字。
			if id, ok := asText(item); ok {
				quota.GroupPrizeIDs = append(quota.GroupPrizeIDs, id)
			}
		}
	}
	return &quota, nil
}

// asText 认"串或数字"两种形态的标量。
//
// 存在的理由与 [parseExchangePrice] 同源：这一族的 JSON 里同一语义的值有时是串、
// 有时是数字（`prizeId` 数字、`monthGroupLimit` 里的元素串、`picWidth` 串）。
// 只认一种的写法**不报错**，只是拿到空值。
func asText(value any) (string, bool) {
	switch typed := value.(type) {
	case string:
		return typed, true
	case json.Number:
		return typed.String(), true
	}
	return "", false
}

// ParseExchangeTime 解 `startTime` / `endTime` 那一族裸串。
//
// 解不出来返回 false（**不报错**）：调用方要的是"窗口判得了吗"这一格三态，
// 而不是让一个畸形时间串把整趟兑换打挂 —— 判不了就不发，那个处置本身是安全的。
func ParseExchangeTime(text string) (time.Time, bool) {
	parsed, err := time.ParseInLocation(ExchangeTimeLayout, strings.TrimSpace(text),
		ExchangeTimeLocation())
	if err != nil {
		return time.Time{}, false
	}
	return parsed, true
}

// sortedGroupKeys 把分组键按数值排序；非数字的键（上游哪天改形态）排在最后、按字典序。
func sortedGroupKeys(groups map[string][]ExchangePrize) []string {
	keys := make([]string, 0, len(groups))
	for key := range groups {
		keys = append(keys, key)
	}
	sort.Slice(keys, func(i, j int) bool {
		left, leftOK := strconv.Atoi(keys[i])
		right, rightOK := strconv.Atoi(keys[j])
		if leftOK == nil && rightOK == nil {
			return left < right
		}
		if leftOK == nil {
			return true
		}
		if rightOK == nil {
			return false
		}
		return keys[i] < keys[j]
	})
	return keys
}

// ─────────────────────────────────────────────────────────────────────────────
// `exchangeV3`（下单，**不可逆**）
// ─────────────────────────────────────────────────────────────────────────────

// ExchangeV3Request 构造 `POST /ycloud/signin/page/exchangeV3`。
//
// # 请求体是**拼串**而不是 map —— 键序必须与实拍逐字一致
//
// `WithJSONBody` 会按**字母序**重排键，而实拍是
// `prizeId → client → clientVersion → puzzleOffset → smsCode → deviceId`（§5.2 逐字，
// 连 `content-length: 196` 都能反推）。这一族的对拍面**包括键序**，所以这里
// 用 `fmt.Sprintf` 拼，不用 map（同一条口径在 `receiveV3` 那族也守着）。
//
// # `puzzleOffset` 三态（实拍逐条对得上）
//
//   - 数字（正确值）⇒ 过滑块闸；
//   - `null` ⇒ `501 滑块验证参数不能为空`；
//   - 空串 `""` ⇒ **与 `null` 逐字相同的响应**（服务端按"没有值"处理，不区分二者）
//     ⇒ 所以这里只给 `nil` 一档，不给"发空串"的入口：那不是一个不同的取值。
//
// ⚠ [ExchangeOffset] 为 nil 时**照样把请求发出去**是有代价的：它必然回 `501`，
// 而那一枪的用途只剩"复现 `501`"。领域层不该走到这里（它拿不到 offset 就不该发）。
//
// # 两个 `WithH5Headers` 不管的头
//
// `isdeviceid: true` 与 `origin: https://m.mcloud.139.com` 是实拍里额外带的两个
// （H5 的 `isDeviceId: !0` 由拦截器翻译成头），少一个就不是"同一个客户端"了。
func ExchangeV3Request(jwt, referer, deviceID string, prizeID int64, offset *int64) core.HTTPRequest {
	offsetRaw := "null"
	if offset != nil {
		offsetRaw = strconv.FormatInt(*offset, 10)
	}
	body := fmt.Sprintf(
		`{"prizeId":%d,"client":"%s","clientVersion":"%s","puzzleOffset":%s,"smsCode":"","deviceId":"%s"}`,
		prizeID, Client, ExchangeClientVersion, offsetRaw, deviceID)

	request := core.NewRequestPost(fmt.Sprintf("%s/ycloud/signin/page/exchangeV3", BaseMcloud))
	request.Body = core.Body([]byte(body))
	request = WithH5Headers(request, referer, &jwt, "true")
	return request.WithHeader("isdeviceid", "true").WithHeader("origin", BaseMcloud)
}

// ExchangeOrder 是 `code == 0` 那一份 `result`（§5.2 逐字，18 个键里取这六个）。
//
// ⚠ 成功响应里**没有余额字段** —— "扣了多少豆"要回查 `infoV3` 的余额差，
// 判据链必须**跨两个接口**拼（§5.4 末那条提醒）。
type ExchangeOrder struct {
	// OrderID 是 `oid` —— 服务端新生成的订单号。
	//
	// ⚠ 它**不是幂等键**：每次下单都新产生。真正的幂等依据在服务端侧（`limit` 状态
	// 与 `2301` 那一档），不是请求里的任何字段。
	OrderID int64
	// PrizeID 是 `prizeId`（回显下单那一件）。
	PrizeID int64
	// PrizeName 是奖品名。
	//
	// ⚠ 原项目把"`code == 0` 但缺 `prizeName`"**记为失败**，并认定那是它的一处隐性 bug
	// （`business-feature-inventory.md` §2.5 `exchange` 的判据那一行）。我们照这个口径：
	// 所以它做成指针，**不在这里补空串** —— 补了就把那一档抹平了。
	PrizeName *string
	// Cost 是 `uorder` —— 这一笔实际扣掉的云朵数（实拍 `1800`，== 那件商品的 `pOrder`）。
	//
	// ⚠ 只有这一条**不等于**"余额真的扣了这么多"：判据是两次 `infoV3` 的差。
	// 它给的是"上游**说**扣了多少"，与余额差**对不上**时才是大事。
	Cost *int64
	// InsertTime 是 `insertTime`，**带 `+00:00` 的 ISO 串**（不是 Go 的 `Z` 形态）。
	InsertTime *string
	// ExpireTime 是奖品有效期，同一种形态。
	ExpireTime *string
	// MarketName 是 `sign_in_3_ex`（兑换族那个带 `_ex` 后缀的，与签到的 [MarketName] 不同值）。
	MarketName *string
	// LinkURL 是 `linkUrl` —— 实拍里它是 prizeId 的**字符串形态**，不是 URL。
	//
	// 界面上"去使用"那一档要用它，所以留着；⚠ 别按 URL 解析。
	LinkURL *string
}

// ExchangeResult 是 `exchangeV3` 的解析结果 —— **码是数据不是错误**。
type ExchangeResult struct {
	// Code 是业务码，见 [ExchangeCodeSliderFailed] 那一族。
	Code int64
	// Message 是上游那句 `msg`（「验证失败」「重复兑奖」「奖品单日已耗尽」…）。
	//
	// 它**参与判定**：`420`/`421`/`415` 这几档要靠文案分开（参考项目就是靠子串表），
	// 而 `514` 那一档上游还会给别的说法。⚠ 只进小结与日志，**不**进 API 响应。
	Message string
	// Order 只在 [Code] == 0 时非 nil。
	Order *ExchangeOrder
}

// OK 报告这一码是不是成功。
func (r ExchangeResult) OK() bool { return r.Code == CodeOK }

// ParseExchangeResult 解析 `exchangeV3` 的响应。**不走** [Envelope.EnsureOK]。
//
// # 为什么这里不能"非 0 即错误"
//
// `514` 可以重取滑块再来（代价为零），`2301` 再怎么试也不会成交，`415` 说明这件今天
// 别再试、明天还有 —— 三档的处置完全不同，而它们都是**正常业务响应**。
//
// # `msg` 缺失怎么办
//
// 认出来：实拍失败响应只有 `{"code":514,"msg":"验证失败"}`（**没有 result 键**）。
// `msg` 缺了不报错 —— 码才是判据，文案只是给人看的那一句。
func ParseExchangeResult(envelope *Envelope) (ExchangeResult, *core.Error) {
	code, ok := envelope.Code()
	if !ok {
		// 没有 code 就**判不了成交没成交**，而这一步花的是真豆 ——
		// 绝不能默认成成功。回错误让编排层把这一趟记成失败（原文进 runs.message）。
		return ExchangeResult{}, core.NewParse("exchangeV3 响应里没有 code，判不了成交与否")
	}
	message, _ := envelope.Message()
	result := ExchangeResult{Code: code, Message: message}
	if code != CodeOK {
		return result, nil
	}
	payload, ok := envelope.Payload()
	if !ok {
		return ExchangeResult{}, core.NewParse(
			"exchangeV3 回 code=0 但没有 result（上游说成功了却什么都没给）")
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return ExchangeResult{}, core.NewParse("exchangeV3 的 result 不是对象")
	}
	// `oid` 与 `prizeId` 缺任一个都**报错**：这一笔已经成交（不可逆），
	// 而订单号是将来对账、查奖品的唯一线索。少了它，"钱扣了、东西是什么"就只能靠猜。
	orderID, ok := asInt64(obj["oid"])
	if !ok {
		return ExchangeResult{}, core.NewParse("exchangeV3 成功的响应里缺 oid")
	}
	prizeID, ok := asInt64(obj["prizeId"])
	if !ok {
		return ExchangeResult{}, core.NewParse("exchangeV3 成功的响应里缺 prizeId")
	}
	result.Order = &ExchangeOrder{
		OrderID:    orderID,
		PrizeID:    prizeID,
		PrizeName:  optStrField(obj, "prizeName"),
		Cost:       optInt64Field(obj, "uorder"),
		InsertTime: optStrField(obj, "insertTime"),
		ExpireTime: optStrField(obj, "expireTime"),
		MarketName: optStrField(obj, "marketName"),
		LinkURL:    optStrField(obj, "linkUrl"),
	}
	return result, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppExchangeList 读兑换清单（`exchangeList`）。**只读**，可放心调。
//
// 它给的是"这个账号此刻看见的清单"：`limit` 与三个余量都是**账号 × 时刻**的读数，
// 所以领域层下单之前必须现读一次，不能拿上一趟缓存的那份（§5.6 就是两次现读对出来的）。
func (u *Upstream) AppExchangeList(
	ctx context.Context, jwt, referer string,
) (ExchangeList, *core.Error) {
	envelope, perr := u.Call(ctx, ExchangeListRequest(jwt, referer))
	if perr != nil {
		return ExchangeList{}, perr
	}
	return ParseExchangeList(envelope)
}

// AppExchangeV3 下**一次**兑奖单。真扣云朵，且**不可回滚**。
//
// # 为什么它不重试、也不在这里判成败
//
// 请求里没有幂等键（`oid` 由服务端每次新生成，见 [ExchangeOrder.OrderID]）。
// 一次网络超时之后，这一笔**可能已经成交也可能没有**，而我们分不清 ——
// 判据只有两次 `infoV3` 的余额差（§5.4 判据链第 2 条）。⇒ 自动重发的表现不是
// "多试一次"，而是**同一个奖品可能扣两次豆**（或者第二次撞上 `2301`，
// 反而让人以为第一次没成）。所以传输层错误一律**原样交回**，
// 由领域层把它折成"结果未知，去查余额差"的待办。
//
// 业务码走 [ExchangeResult.Code] 而不是错误通道，理由见 [ParseExchangeResult]。
func (u *Upstream) AppExchangeV3(
	ctx context.Context, jwt, referer, deviceID string, prizeID int64, offset *int64,
) (ExchangeResult, *core.Error) {
	envelope, perr := u.Call(ctx, ExchangeV3Request(jwt, referer, deviceID, prizeID, offset))
	if perr != nil {
		return ExchangeResult{}, perr
	}
	return ParseExchangeResult(envelope)
}
