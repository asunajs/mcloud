// 趣玩 AI（「全网趣玩AI赢好礼」）抽奖族：`funai/api/page/getmodule`（读判据）与
// `funai/api/page/lottery`（抽一次）。
//
// 对应旧实现里的 `play-ai-lottery` / `fun_ai` 那一条活动任务
// （`docs/business-feature-inventory.md` §2.4、`docs/caiyun-go-feature-inventory.md` 第 156 行）。
//
// # 这一族只实现两条请求 —— 因为只有这两条在链上
//
// 实拍（`test/fixtures/http/玩-ai-抽支付宝红包.json`，按文件顺序 0…11）里出现了六类请求。
// 已实现的两条：
//
//	page/getmodule  → 判据（顺序 2 与 7，两次响应同一形状）
//	page/lottery    → 抽一次（顺序 11）
//
// 没有实现的几类，以及**为什么**不实现（不是"忘了"，是判据上不需要）：
//
//   - `timed/getmodule`（顺序 3、8）：`result` 是**模块数组**，且每一项的
//     `drawTimes` / `drawsUsed` / `todayGetLotteryCount` **全是 `null`**
//     ⇒ 它回答不了"还剩几次"。给它编一个解析函数，下一步就是有人拿 `null` 当 `0`
//     去做判据。抽次数一律问 `page/getmodule`。
//   - `manager/commonMarketconfig/getByMarketName?marketName=National_playAI`（顺序 1）：
//     规则文本。⚠ 里面那句「每个用户每日最多可通过完成任务获得 5 次抽奖机会…当日有效，
//     过期失效」说的是**做任务能挣到几次**，不是"每天最多能抽几次" —— 与本链的每日上限
//     无关（上限与它的出处见 app 层 `FunaiDailyDrawLimit`）。
//   - `invite/getcode`（顺序 6、9）：邀请码，`result` 在实拍里已是脱敏后的串，
//     且本链不需要它 ⇒ 不实现。
//   - `market/market-advert/advert/getAdInfo`（顺序 4、5）、`img.mcloud.139.com` 上
//     那两份 `data.json`（顺序 0、10）：广告位与动画素材，纯前端资源。
//
// # ⚠ 这一页的 `jwttoken` **使用公共来源**（临时态，不是终判）
//
// 取向见 `AGENTS.md` §1.12：能按该页面自己的方式换到该页面的票就用它，公共来源是退路。
// 这一份抓包里 `page/getmodule` **自带** `jwttoken` 头，但**不含**换出这串票的那一步
// （十二项里没有 `getAccount(National_playAI)`、也没有活动门户的预热请求），
// 所以该页的特有取法**无从得知** ⇒ 这里按退路走：沿用签到页那一条建会话配方
// （接线点 `internal/app/signin_world.go` 的 `Source`，它固定用 `RecipeSignInPage`）。
//
// ⇒ 后续抓到该页的特有取法时，**主动更正**这一处：改用特有取法换该页的票，
// 并把这句注释改掉（不留"注释说公共、代码走特有"），同时把新查到的来源记进
// 对应的 `docs/capture/` 样本。
//
// # 请求头与实拍的三处已知差异（都**不带**判断作用，但要说清）
//
//   - `x-tingyun`：实拍每条都带（前端拨测 SDK 注入的链路追踪头）。本仓库**所有** H5 接口
//     都不发它（现状如此），上游是否据此判什么 ⇒ 未测试。
//   - `activityid`：实拍的值在这份归档里**已被脱敏**，看不出这一页带的是哪个。
//     这里沿用 [WithH5Headers] 的公共值 [MarketName]（`sign_in_3`）。要核它：
//     抓一条未脱敏的 funai 请求头比对，或直接真跑一次看是否回非 0。
//   - `deviceid`：实拍带，但值同样被脱敏（连形态都无法核对 —— 不能确定就是
//     [DeviceID] 换回来的那个 `"B"` + 88 字符 base64）。所以做成**可选参数**，
//     缺省不发，与 [ReceiveRevivalRewardRequest] 同一个处置理由：
//     服务端到底要不要它**未验证**，留着可选才能一次 A/B 试出来。
package upstream

import (
	"context"
	"fmt"
	"time"

	"github.com/catlair/mcloud/internal/core"
)

// FunaiMarketName 是这一活动的市场名 —— 实拍 `page/lottery` 响应里的 `marketName`，
// 也是规则文本那条接口（`getByMarketName?marketName=`）的取值。
//
// ⚠ 它**不进**本文件两条请求的参数：`page/lottery` 的 body 只有 `moduleId` 一个键（实拍
// `content-length: 23`）。留这个常量是为了核对响应、以及将来接规则文本那条时有出处。
const FunaiMarketName = "National_playAI"

// FunaiActivityTimeLayout 是 `startTime` / `endTime` 的**实拍形态**（逐字）：
//
//	startTime "2025-05-28T00:00:00"
//	endTime   "2026-12-31T23:59:59"
//
// ⇒ 是**不带时区、不带偏移**的本地时刻串（既不是 RFC 3339，也不是那两行注释里的 epoch）。
// 按什么形态判就先按什么形态解，别"顺手当 UTC"。
const FunaiActivityTimeLayout = "2006-01-02T15:04:05"

// FunaiActivityLocation 是把上面那两个**无时区**串解释成时刻时用的时区：东八区（固定 +8，
// 中国大陆无夏令时）。
//
// # 为什么是 +8 而不是 UTC
//
// [core.Clock] 的口径是"实现必须返回 UTC"（见 `core/traits.go`），而这两个串是活动页给
// 中国用户看的本地时刻。按 UTC 解释会把边界**整体推后 8 小时**：
// 快到 `endTime` 时多抽一次（该不发时发了），快到 `startTime` 时少抽一次。
//
// ⚠ "上游也是按东八区判的"这句**未验证** —— 上游没说这两个串是哪个时区写的。
// 要核它：在 `endTime` 前后各抓一次（或拿一个当期已结束的模块看它回什么），
// 比对服务端受理与拒绝的那条线落在哪个时刻上。
func FunaiActivityLocation() *time.Location { return time.FixedZone("UTC+8", 8*3600) }

// ─────────────────────────────────────────────────────────────────────────────
// 判据：`GET /ycloud/funai/api/page/getmodule`
// ─────────────────────────────────────────────────────────────────────────────

// FunaiModuleRequest 构造 `GET /ycloud/funai/api/page/getmodule`。
//
// 实拍（顺序 2、7）：**没有查询串**、**请求体是空的**（`”`）、
// **没有 `isdeviceid`**、**没有 `content-type`** —— 与记录族（[CloudRecordV2Request]）
// 同一类形状：`WithH5Headers` 只在**有 body** 时才补 content-type，所以这里裸 GET
// 出去就是"一个 body 都没有、一个 content-type 都没有"。
//
// `deviceID` 为什么可选、缺省不发，见本文件头「请求头与实拍的三处已知差异」第三条。
func FunaiModuleRequest(jwt, referer string, deviceID *string) core.HTTPRequest {
	request := WithH5Headers(
		core.NewRequestGet(fmt.Sprintf("%s/ycloud/funai/api/page/getmodule", BaseMcloud)),
		referer, &jwt, "true")
	if deviceID != nil {
		request = request.WithHeader("deviceid", *deviceID)
	}
	return request
}

// FunaiModule 是 `page/getmodule` 里本链要用到的那几个字段。
//
// # 为什么只声明这几个
//
// 实拍的 `result` 有 15 个键（另有 `title` / `subtitle` / `imageUrl` / `pullLink` /
// `flag` / `taskIds[]` 等 16 项任务列表）。本层的问题是"该不该抽、拿什么 id 去抽"，
// 任务列表与图片素材答不了它 —— 声明出来只会多一处会漂移的字段表。
//
// # ⚠ 为什么**不**读 `isDraw` / `drawTime` / `drawResult`
//
// 实拍里这三个都是 `null`（顺序 2 与 7 两次都一样，而这两次之间已经抽过一次了吗 —— 没有，
// 抽奖在顺序 11）。**它们的非 `null` 形态在本仓库没有任何样本**，
// 所以"`isDraw` 是不是今天抽过"、"`drawTime` 是不是上次抽奖时间"都只是望文生义。
//
// 判据因此只用**有实拍的三个计数**（`drawTimes` / `drawsUsed` / `todayGetLotteryCount`）。
// ⇒ 歧义登记（待验证）：这三个计数与那三个 null 字段之间是什么关系、
// 抽过一次之后哪几个会变，都要**当天抽一次再查一次**才能定（要测它：
// 在同一天内先查 `page/getmodule`、抽一次、立刻再查一次，比对两次响应逐字段的差异；
// 只能在活动窗口内做，窗外补不了样）。
type FunaiModule struct {
	// ModuleID 是 `result.moduleId` —— 当期模块 id，实拍 `1-250730`。
	//
	// ⚠ **必填**，且抽奖请求体里的值**必须来自这里**、不许写死：换一期活动它就换
	// （同一份抓包的 `timed/getmodule` 里同期另挂着 `2-260324-1` / `2-260306-1` /
	// `2-250730-1` 三个）。带一个过期 id 去抽，上游回什么**没有样本**。
	ModuleID string
	// DrawTimes 是 `result.drawTimes` —— 实拍 `1`。
	//
	// ⚠ **语义未验证**：它到底是"总共能抽几次"还是"当前可用次数"，一份样本定不了。
	// 本链只用它与 `DrawsUsed` 的差回答"还剩几次"，不对外解释它的名字。
	DrawTimes *int64
	// DrawsUsed 是 `result.drawsUsed` —— 实拍 `0`。语义同样未验证（同 [DrawTimes]）。
	DrawsUsed *int64
	// TodayGetLotteryCount 是 `result.todayGetLotteryCount` —— 实拍 `1`。
	//
	// 声明它是因为它在 `timed/getmodule` 那份里是 `null`（正是"那条不能当判据"的证据之一），
	// 但本链的判定**不**用它：一份样本说不出它和上面两个计数的关系。
	TodayGetLotteryCount *int64
	// StartTime / EndTime 是 `result.startTime` / `endTime` —— 活动期。
	//
	// `nil` = 上游没给**或**形态不认识 ⇒ 判不了（**不是**"没有边界"）。
	// 谁来判、判不了怎么处置在 app 层（`PlanFunai`）。
	StartTime *time.Time
	EndTime   *time.Time
}

// ParseFunaiModule 解析 `page/getmodule` 的响应。
//
// 只有 `moduleId` 是必填（缺了就发不出抽奖请求），其余全是指针 ——
// 计数缺了要能落成"判不了"那一档，而不是被默认值伪造成"0 次 / 已用过"。
func ParseFunaiModule(envelope *Envelope) (FunaiModule, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return FunaiModule{}, perr
	}
	moduleID, perr := RequireStr(payload, "moduleId")
	if perr != nil {
		return FunaiModule{}, perr
	}
	if moduleID == "" {
		// 空串单独拦一句：[RequireStr] 认字符串形态，`"moduleId": ""` 它是放行的，
		// 而带着空 id 发出去的抽奖请求**一定**是另一个请求（实拍那个键值从不为空）。
		return FunaiModule{}, core.NewParse("响应字段 `moduleId` 为空，无法构造抽奖请求")
	}
	obj, _ := payload.(map[string]any)
	return FunaiModule{
		ModuleID:             moduleID,
		DrawTimes:            optInt64Field(obj, "drawTimes"),
		DrawsUsed:            optInt64Field(obj, "drawsUsed"),
		TodayGetLotteryCount: optInt64Field(obj, "todayGetLotteryCount"),
		StartTime:            funaiTimeField(obj, "startTime"),
		EndTime:              funaiTimeField(obj, "endTime"),
	}, nil
}

// funaiTimeField 取活动起止时刻。
//
// ⚠ **形态不认识一律给 `nil`，不报错**：这里报错会把一次**能抽**的机会变成解析失败，
// 而"上游换了时间格式"真正该表现为"判不了 ⇒ 不发请求"（那句结论是人能看懂的）。
// 非字符串（含 epoch 数字）也算形态不认识 —— 见 [optStrField]。
func funaiTimeField(obj map[string]any, key string) *time.Time {
	text := optStrField(obj, key)
	if text == nil {
		return nil
	}
	parsed, err := time.ParseInLocation(FunaiActivityTimeLayout, *text, FunaiActivityLocation())
	if err != nil {
		return nil
	}
	return &parsed
}

// ─────────────────────────────────────────────────────────────────────────────
// 抽一次：`POST /ycloud/funai/api/page/lottery`
// ─────────────────────────────────────────────────────────────────────────────

// FunaiLotteryRequest 构造 `POST /ycloud/funai/api/page/lottery` —— **抽一次**。
//
// # 请求体逐字（实拍顺序 11，`content-length: 23`）
//
//	{"moduleId":"1-250730"}
//
// 只有 `moduleId` **一个键**：没有 `marketName`、没有 `client`、没有查询串。
// `content-type: application/json;charset=UTF-8` 由 [WithH5Headers] 按 body 补上
// （带 charset 的那个，见 [H5ContentType] 那条"先剥后补"的说明）。
//
// # ⚠ 这是**写请求**，且门槛**不在这里判**
//
// 该不该发由调用方看 [FunaiModule] 的三个计数决定（`PlanFunai`）。
// 本层只保证"发出去、解析回来" —— 与 `receiveV3` 那一族的分工同一口径
// （理由见 `app_signin_send.go` 文件头）。
func FunaiLotteryRequest(
	jwt, referer, moduleID string, deviceID *string,
) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/ycloud/funai/api/page/lottery", BaseMcloud)).
		WithJSONBody(map[string]any{"moduleId": moduleID})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	request = WithH5Headers(request, referer, &jwt, "true")
	if deviceID != nil {
		request = request.WithHeader("deviceid", *deviceID)
	}
	return request, nil
}

// FunaiLottery 是 `page/lottery` 的解析结果 —— 这一次抽出的是什么。
//
// # 实拍（顺序 11，`code: 0`）
//
//	{"id":3228298,"isWin":1,"lotteryDate":"20260629",
//	 "lotteryTime":"2026-06-29T20:13:47.890625379","marketName":"National_playAI",
//	 "moduleId":"1-250730","prizeId":260107009,"prizeName":"30云朵",
//	 "prizeType":2,"prizeUrl":"…png","type":"1"}
//
// 全部字段是**指针**：这条**只有一档成功样本**，没有任何"失败但 `code` 仍为 0"的实拍，
// 所以没有一个字段是"缺了就说明响应坏了"的依据 —— 硬要求某个字段，表现是把一次
// 真成功的抽奖报成解析错误（同 [ParseReceiveTaskExpansion] 那条教训）。
//
// ⚠ 实拍里还有一个 `phone` 字段（归档时已脱敏）：它**不带进来**，
// 既不进结构也不进夹具 —— 见本仓库的凭据/账号标识纪律。
type FunaiLottery struct {
	// PrizeName 是 `result.prizeName` —— 抽出的是什么，实拍「30云朵」。
	// `nil` = 上游没说，**不是**"没中奖"。
	PrizeName *string
	// PrizeID 是 `result.prizeId`（实拍 `260107009`）。
	PrizeID *int64
	// PrizeType 是 `result.prizeType`（实拍 `2`）。⚠ 这套类型号的取值表**没有成文**，
	// 别拿它当"现金/云朵"的分类用；要判奖品种类先看 [PrizeName]。
	PrizeType *int64
	// IsWin 是 `result.isWin`（实拍 `1`）。
	//
	// ⚠ **不据此分支**：只有一档 `isWin = 1` 的样本，"`0` 是不是没中奖"未验证。
	// 本仓库把"`code == 0` 且请求发了"当成抽完了，抽出什么看 [PrizeName]。
	IsWin *int64
	// ModuleID 是 `result.moduleId` —— 上游认下的那个 id。
	//
	// 带出来只用于**排障对账**（拿它跟发出去的那个比一下就知道判据是否拿错了模块）；
	// 本链**不**据此分支 —— "上游回了另一个 moduleId"这种情况一个样本都没有。
	ModuleID *string
	// MarketName 是 `result.marketName` —— 实拍 `National_playAI`，
	// 与 [FunaiMarketName] 一致；只作核对用。
	MarketName *string
}

// ParseFunaiLottery 解析 `page/lottery` 的响应。
//
// `code` 非 0 由 [EnsureOKWithHint] 转成错误（**包括**"没有次数"那一类 —— 实拍里
// 这一码/这句话**一个样本都没有**，所以这里不猜具体码值，一律按失败交回调用方折叠）。
// 没有 `result` **不判失败**：成败已由 `code` 说过一次，这里再判一次会把一句
// 上游的原话换成一句我们编的解析错误。
func ParseFunaiLottery(envelope *Envelope) (FunaiLottery, *core.Error) {
	payload, perr := EnsureOKWithHint(envelope)
	if perr != nil {
		return FunaiLottery{}, perr
	}
	obj, _ := payload.(map[string]any)
	return FunaiLottery{
		PrizeName:  optStrField(obj, "prizeName"),
		PrizeID:    optInt64Field(obj, "prizeId"),
		PrizeType:  optInt64Field(obj, "prizeType"),
		IsWin:      optInt64Field(obj, "isWin"),
		ModuleID:   optStrField(obj, "moduleId"),
		MarketName: optStrField(obj, "marketName"),
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层
// ─────────────────────────────────────────────────────────────────────────────

// AppFunaiModule 读趣玩 AI 的判据（`page/getmodule`）。只读。
//
// `deviceID` 传 `nil` 是缺省路径（实拍带这个头但值已脱敏，服务端是否要它未验证）。
func (u *Upstream) AppFunaiModule(
	ctx context.Context, jwt, referer string, deviceID *string,
) (FunaiModule, *core.Error) {
	envelope, perr := u.Call(ctx, FunaiModuleRequest(jwt, referer, deviceID))
	if perr != nil {
		return FunaiModule{}, perr
	}
	return ParseFunaiModule(envelope)
}

// AppFunaiLottery 抽一次（`page/lottery`）。**写请求**，每天最多一次的额度由调用方管。
//
// `moduleID` 必须是 [AppFunaiModule] 的 `result.moduleId` —— 不许写死、不许从别处拿。
func (u *Upstream) AppFunaiLottery(
	ctx context.Context, jwt, referer, moduleID string, deviceID *string,
) (FunaiLottery, *core.Error) {
	request, perr := FunaiLotteryRequest(jwt, referer, moduleID, deviceID)
	if perr != nil {
		return FunaiLottery{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return FunaiLottery{}, perr
	}
	return ParseFunaiLottery(envelope)
}
