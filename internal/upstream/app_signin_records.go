// 记录族：`cloudRecordV2`（豆的收支流水）/ `record`（奖品记录）/
// `backupgift/*`（备份好礼）。
//
// 对应 Rust 侧的 app_signin/records.rs + prize.rs + backup_gift.rs。
//
// # 这一族的共同点：**都是 GET + 查询参数，都不带 `isdeviceid`**
//
// 三条接口在 H5 里的调用点都是 `{params: e, headers:{showLoading:true,…}}`，
// 即**没有 body**、**没有设备头** —— 拦截器只在 H5 显式标了 `isDeviceId`
// 时才注入设备串，这里没标。漏发设备头在这三条上**不是**错误。
package upstream

import (
	"encoding/json"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 豆的收支流水
// ─────────────────────────────────────────────────────────────────────────────

// `cloudRecordV2` 的 `aiDou` —— **账本**。两个值，不是两种货币。
//
// ⚠ 云朵与「云盘专属 AI豆」是**同一个东西的两个名字**（2026-07-31 按 1:1 改名）。
// 这两档分开只是因为**记录跨了那次改名**：旧记录留在旧账本里，新记录进新账本。
// 别把它理解成"两个池子"或"两种货币" —— 那种理解会让人以为可以互相换算。
const (
	// RecordBookCloud 是改名之前的账本（`aiDou = 0`）。
	RecordBookCloud int64 = 0
	// RecordBookBean 是改名之后的账本（`aiDou = 1`）。
	RecordBookBean int64 = 1
)

// `cloudRecordV2` 的 `type` —— H5 那三个筛选页签。
const (
	// RecordFilterAll 是「全部」。
	RecordFilterAll int64 = 0
	// RecordFilterEarned 是「获得记录」。
	RecordFilterEarned int64 = 1
	// RecordFilterSpent 是「使用记录」。⚠ 是 **-1** 不是 2。
	RecordFilterSpent int64 = -1
)

// CloudRecordV2Request 构造 `GET /ycloud/signin/public/cloudRecordV2` ——
// **豆的收支流水**（审计线）。
//
// # 它解决什么问题
//
// `infoV3.total` 给的是**当前余额**，但它不告诉你这个数是怎么来的 ——
// 于是"余额对不上"这类问题只能靠猜。这条给的是**流水**。
//
// # 形状（H5 源码逐字，`139newsignin.cc383f1b.js`）
//
//	{aiDou: 0|1, type: p.value, pageNumber: F.value, pageSize: 10}
//
// ⚠ 函数名里那个 `AIBean` 是**误导**：同一个函数被两个页面共用，
// 靠 `aiDou` 切换账本（`pointRecord` 传 `0`、`AIBeanRecord` 传 `1`）。
func CloudRecordV2Request(
	jwt, referer string, aiDou, filter, pageNumber, pageSize int64,
) core.HTTPRequest {
	url := fmt.Sprintf(
		"%s/ycloud/signin/public/cloudRecordV2?aiDou=%d&type=%d&pageNumber=%d&pageSize=%d",
		BaseMcloud, aiDou, filter, pageNumber, pageSize)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// CloudRecords 是 `cloudRecordV2` 的解析结果 —— 一页流水。
//
// # 为什么条目是**原样的 `any`**
//
// H5 拿到 `records` 之后**直接丢给模板渲染**，从没读过条目里的任何具体字段名。
// ⇒ 本仓库**没有任何 `records[]` 的实拍**（也没人抓过）。
//
// 所以这里**不声明字段**：编一个 `amount` / `createTime` 出来，等于把猜测变成断言 ——
// 第一次真跑就会以一个看不懂的解析错误收场，而原因只是"名字猜错了"。
// 原样交出去，让调用方看着办。
type CloudRecords struct {
	// Records 是 `result.records` —— 本页流水条目，**原样**。
	Records []any
	// Total 是 `result.total` —— ⚠ **条目总数（分页用），不是金额**。
	//
	// 判据是 H5 的翻页逻辑：`records.length + … >= total` 就判"加载完了" ——
	// 能这么比的只有条数。
	// `nil` 表示上游没给，与"0 条"**不是一回事**。
	Total *int64
}

// ParseCloudRecords 解析 `cloudRecordV2` 的响应。
//
// ⚠ 没有 `records` 时给**空数组**而不是报错：这是审计接口，
// "这一档没有记录"是正常答案（例如改名前的账本本来就只到 2026-07-30）。
func ParseCloudRecords(envelope *Envelope) (CloudRecords, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return CloudRecords{}, perr
	}
	obj, _ := payload.(map[string]any)

	var total *int64
	if obj != nil {
		if value, ok := obj["total"]; ok {
			if n, ok := asInt64(value); ok {
				total = &n
			}
		}
	}
	return CloudRecords{Records: recordsArray(obj), Total: total}, nil
}

// recordsArray 取 `records` 数组；缺失或类型不对时给空数组。
//
// ⚠ 空数组与"解析失败"在这里**必须**是同一个结果：三个页签里总有两个是空的，
// 把"这一档没有记录"打成错误会让整条审计线在最常见的路径上报错。
func recordsArray(obj map[string]any) []any {
	if obj == nil {
		return []any{}
	}
	items, ok := obj["records"].([]any)
	if !ok {
		return []any{}
	}
	return items
}

// ─────────────────────────────────────────────────────────────────────────────
// 奖品记录
// ─────────────────────────────────────────────────────────────────────────────

// PrizeMarketName 是 `record` 的 `marketname` —— H5 两个奖品记录组件
// **都传这个值**（逐字核对过两处调用点）。
//
// ⚠ 名字里的 `_ex` **不是"兑换"**：这两个组件的页签是「待领取 / 已领取 / 已过期」，
// 是**奖品记录**不是兑换流水（兑换走的是 `exchangeV3` / `exchangeList`）。
// 别看到 `_ex` 就往"exchange"上想。
const PrizeMarketName = "sign_in_3_ex"

// `record` 的 `type` —— 页签。
//
// 这三个值是 H5 里**写死的字面量**
// （`[{id:1,btnText:"待领取"},{id:2,btnText:"已领取"},{id:10,btnText:"已过期"}]`），
// 不是从响应里读出来的 —— 所以它们是常量不是枚举探测。
const (
	// PrizeStatusPending 是「待领取」。
	PrizeStatusPending int64 = 1
	// PrizeStatusReceived 是「已领取」。
	PrizeStatusReceived int64 = 2
	// PrizeStatusExpired 是「已过期」。⚠ 是 **10** 不是 3。
	PrizeStatusExpired int64 = 10
)

// PrizeRecordRequest 构造 `GET /ycloud/signin/public/record` ——
// **奖品记录**（默认页签是「待领取」）。
//
// 这是旧项目 `printPendingPrizes`（`getUserPrizeLogPageV2`）在新接口上的对应物。
//
// # 分页判据（与流水那条**不同**）
//
// H5 逐字写的是 **`records.length < pageSize` ⇒ 到底了**
// （`t.length<10&&(X.value=!0)`）—— 它**不读 `total`**。
// 所以 [CloudRecords.Total] 在这条上**没有对应物**，别照搬。
//
// # 与旧接口的关系
//
// 旧项目打的 `.../checkPrize/getUserPrizeLogPageV2?currPage=&pageSize=` 在
// `m.mcloud.139.com` 上**仍然存在**，但**裸请求回 `501 参数校验异常`**
// （换 `pageNumber` 直接 `404`）—— 参数口径已经变了。新接口是这条。
func PrizeRecordRequest(
	jwt, referer, marketname string, status, pageNumber, pageSize int64,
) core.HTTPRequest {
	url := fmt.Sprintf(
		"%s/ycloud/signin/public/record?marketname=%s&type=%d&pageNumber=%d&pageSize=%d",
		BaseMcloud, PercentEncode(marketname), status, pageNumber, pageSize)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// PrizeRecordItem 是 `record` 里的一条**奖品记录**。
//
// # 字段依据（**有实拍**，2026-09-19，账号 159）
//
// 「待领取」页签回了一条。实拍条目长这样（**已去掉账号标识字段**）：
//
//	{"flag":1,"groupId":0,"type":1057,"source":0,"belonger":0,"device":0,
//	 "prizeId":260630014,"prizeName":"移动云盘1000万tokens叠加包",
//	 "insertTime":"2026-09-16T11:12:54.000+00:00",
//	 "expireTime":"2026-09-30T15:59:59.000+00:00",
//	 "updateTime":"2026-09-16T11:12:54.000+00:00",
//	 "market":"sign_in_3_ex","marketName":"sign_in_3_ex",
//	 "oid":1270932714,"orderType":0,"prizeSectionStatus":1,
//	 "reasons":"260630014","uorder":400,
//	 "linkUrl":"{\"saleType\":103,\"days\":\"0\",\"serviceId\":\"…\"}"}
//
// ⇒ 所以这里**声明**这几个字段。但仍保持指针（可为 nil）：
// 缺一个就少一行字，不该把整条查询打成解析错误
// （**空页才是最常见的答案** —— 三个页签里总有两个是空的）。
//
// ⚠ 条目里**带账号标识**（`userDomainId`，那个 19 位值）。所以本仓库
// **不留原样条目**：黄金向量只保留上面这几个字段，见
// `test/golden/app-signin-vectors.json`。
type PrizeRecordItem struct {
	// PrizeName 是 `prizeName` —— 奖品名。列表主文案。
	PrizeName *string
	// InsertTime 是 `insertTime` —— **中奖时间**。H5 用 dayjs 格式化成 `YYYY-MM-DD`。
	InsertTime *string
	// ExpireTime 是 `expireTime` —— **过期时间**。
	//
	// ⚠ 这里曾经写着"当前版 H5 没有这个字段"——**错了，已按实拍更正**。
	// H5 的**列表项模板**确实只显示「中奖时间」（`insertTime`），
	// 但响应里 `expireTime` **一直在**：实拍是 `2026-09-30T15:59:59.000+00:00`
	// （中奖后 14 天）。「模板没渲染它」**不等于**「响应没有它」——
	// 旧项目 `printPendingPrizes` 读这个字段是对的，当时是把两件事混成了一件。
	ExpireTime *string
	// Flag 是 `flag` —— **`1` = 待领取**。旧项目 `printPendingPrizes` 就是按它过滤的。
	//
	// 页签（请求参数 `type`）已经筛过一遍，所以它多半恒为 `1`；
	// 留着是因为它**是旧项目那条过滤的判据**，丢了就没人能解释"为什么这么筛"。
	Flag *int64
	// PrizeID 是 `prizeId` —— 奖品 id。点「去领取」时要回传给奖品详情接口。
	PrizeID *int64
	// Oid 是 `oid` —— 中奖记录 id（H5 里叫 `drawRecode`，是**同一个值换了个名字**）。
	//
	// 实拍是**数字**（`1270932714`），而 H5 把它拼进 URL 查询串 ——
	// 两种形态都能用，所以这里统一成字符串。
	Oid *string
	// MarketName 是 `marketName` —— 该奖品属于哪个活动（H5 里叫 `marketId`）。
	MarketName *string
	// PrizeType 是 `type` —— 奖品类型号（实拍 `1057`）。
	//
	// ⚠ 与请求参数那个"页签 `type`"（`1/2/10`）**同名不同物**，别混。
	PrizeType *int64
}

// PrizeRecords 是一页奖品记录。
type PrizeRecords struct {
	// Records 是 `result.records` —— 本页条目。
	Records []PrizeRecordItem
}

// ParsePrizeRecords 解析 `record` 的响应。
//
// ⚠ 没有 `records` 时给**空数组**而不是报错：三个页签里总有两个是空的
// （待领取/已领取/已过期），"这一档没有记录"正是最常见的**正常**答案。
func ParsePrizeRecords(envelope *Envelope) (PrizeRecords, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return PrizeRecords{}, perr
	}
	obj, _ := payload.(map[string]any)
	items := recordsArray(obj)

	out := PrizeRecords{Records: make([]PrizeRecordItem, 0, len(items))}
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		out.Records = append(out.Records, PrizeRecordItem{
			PrizeName:  optStrField(item, "prizeName"),
			InsertTime: optStrField(item, "insertTime"),
			ExpireTime: optStrField(item, "expireTime"),
			Flag:       optInt64Field(item, "flag"),
			PrizeID:    optInt64Field(item, "prizeId"),
			Oid:        optIDField(item, "oid"),
			MarketName: optStrField(item, "marketName"),
			PrizeType:  optInt64Field(item, "type"),
		})
	}
	return out, nil
}

// optStrField 取可选字符串字段。非字符串（含数字）一律当"没有"。
func optStrField(obj map[string]any, key string) *string {
	if obj == nil {
		return nil
	}
	text, ok := obj[key].(string)
	if !ok {
		return nil
	}
	return &text
}

// optInt64Field 取可选整数字段。
func optInt64Field(obj map[string]any, key string) *int64 {
	if obj == nil {
		return nil
	}
	value, ok := obj[key]
	if !ok {
		return nil
	}
	n, ok := asInt64(value)
	if !ok {
		return nil
	}
	return &n
}

// optIDField 取 `oid` 那种"可能是数字也可能是字符串"的字段，统一成字符串。
//
// # 为什么必须两种都收
//
// 实拍是数字，而 H5 直接把它塞进 URL 查询串 —— 上游两个形态都出现过。
// 只收其中一种的表现是"某些奖品点「去领取」时 id 是空的"，
// 而空 id 发上去的报错**不会**提示"id 为空"。
func optIDField(obj map[string]any, key string) *string {
	if obj == nil {
		return nil
	}
	switch value := obj[key].(type) {
	case string:
		return &value
	case float64:
		text := fmt.Sprintf("%d", int64(value))
		return &text
	case int64:
		text := fmt.Sprintf("%d", value)
		return &text
	case json.Number:
		text := value.String()
		return &text
	}
	return nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 备份好礼
// ─────────────────────────────────────────────────────────────────────────────

// BackupGiftInfoRequest 构造 `GET /market/backupgift/info` —— **备份好礼**的状态。
//
// # 为什么主机是 `caiyun.feixin.10086.cn` 而不是 `m.mcloud.139.com`
//
// 这个接口**不在新 H5 的签到页里** —— 新 H5 里 `backupGift` 只出现一次，
// 是一个指向**独立活动页**的 `popLink`
// （`/portal/caiyunEventPage/index.html?path=backupGifts`）。
// 所以它没有"新接口"可换，沿用的是旧项目那条。
//
// ⚠ 换主机是**有实测依据**的，不是照抄旧项目：裸请求
// `https://caiyun.feixin.10086.cn/market/backupgift/info` 回
// `{"code":90001,"msg":"未登录"}` —— 与 `m.mcloud.139.com` 上的
// `record` / `liveRoomFeedback` **回同一个信封**，说明两者在**同一道鉴权网关**后面。
// （若哪天它回 `404` 而不是 `90001`，就是换了主机，别以为"接口下线了"。）
func BackupGiftInfoRequest(jwt, referer string) core.HTTPRequest {
	url := fmt.Sprintf("%s/market/backupgift/info", BaseCaiyun)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// BackupGiftReceiveRequest 构造 `GET /market/backupgift/receive` —— **领备份好礼**。
//
// 门槛**不在这里判**：先读 [BackupGiftInfoRequest] 的 `state`，
// 只有 `state == 0`（可领）才发这条。
func BackupGiftReceiveRequest(jwt, referer string) core.HTTPRequest {
	url := fmt.Sprintf("%s/market/backupgift/receive", BaseCaiyun)
	return WithH5Headers(core.NewRequestGet(url), referer, &jwt, "true")
}

// `backupgift/info` 的 `state` —— 三态（旧项目 `backupGiftTask` 的
// `switch (state)` 逐字）。
const (
	// BackupGiftOff 是**未开启备份** ⇒ 不领，提示去 App 开。
	BackupGiftOff int64 = -1
	// BackupGiftClaimed 是本月**已领过** ⇒ 不领。
	BackupGiftClaimed int64 = 1
	// BackupGiftClaimable 是**可领** ⇒ 发 [BackupGiftReceiveRequest]。
	BackupGiftClaimable int64 = 0
)

// BackupGift 是 `backupgift/info` 的解析结果。
type BackupGift struct {
	// State 是 `result.state` —— 见 [BackupGiftOff] 那三态常量。
	//
	// ⚠ 它是**必填**（走 [RequireInt64]）：它就是这条接口的主判据 ——
	// 缺了它就无法回答"该不该领"，而这正是本结构存在的唯一理由。
	State int64
	// CurMonth 是 `result.curMonth` —— 本月**有没有**备份好礼。
	//
	// 旧项目先判它：`if (!curMonth) { "无备份礼物可领"; return; }` ——
	// 即它比 `State` **更靠前**，`false` 时连 `State` 都不用看。
	CurMonth bool
}

// ParseBackupGift 解析 `backupgift/info` 的响应。
func ParseBackupGift(envelope *Envelope) (BackupGift, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return BackupGift{}, perr
	}
	if payload == nil {
		return BackupGift{}, core.NewParse("backupgift/info 响应没有 result")
	}
	state, perr := RequireInt64(payload, "state")
	if perr != nil {
		return BackupGift{}, perr
	}
	obj, _ := payload.(map[string]any)
	return BackupGift{State: state, CurMonth: truthyField(obj, "curMonth")}, nil
}

// BackupGiftReceive 是 `backupgift/receive` 的解析结果 —— 这次领到多少豆。
//
// # 为什么 `result` 直接是个数字
//
// 旧项目的类型就写着 `result: number`，调用点读 `typeof receiveData?.result === "number"`
// 再把它当豆数打印。⇒ 这条的 `result` **不是对象**，别按对象去取字段。
type BackupGiftReceive struct {
	// Received 是 `result` —— 领到的豆数。上游没给数字时是 `nil`（≠ 0）。
	Received *int64
}

// ParseBackupGiftReceive 解析 `backupgift/receive` 的响应。
//
// `result` 缺失**不判失败** —— 成败由 `code` 决定。
func ParseBackupGiftReceive(envelope *Envelope) (BackupGiftReceive, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return BackupGiftReceive{}, perr
	}
	if payload == nil {
		return BackupGiftReceive{}, nil
	}
	n, ok := asInt64(payload)
	if !ok {
		return BackupGiftReceive{}, nil
	}
	return BackupGiftReceive{Received: &n}, nil
}

// truthyField 按"真值"收一个可能是布尔 / 数字 / 字符串的字段。
//
// `curMonth` 在旧项目里当布尔用（`if (!curMonth)`），所以三种形态都认，
// 缺了当 `false`。
func truthyField(obj map[string]any, key string) bool {
	if obj == nil {
		return false
	}
	switch value := obj[key].(type) {
	case bool:
		return value
	case float64:
		return value != 0
	case int64:
		return value != 0
	case string:
		return value != "" && value != "0" && value != "false"
	case json.Number:
		return value.String() != "0"
	}
	return false
}
