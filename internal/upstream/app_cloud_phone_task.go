// 红包派对（云机）族 · 任务清单与任务动作：`redpacket/configTaskList`、
// `redpacket/configTaskLoginList`（**有实拍**）、`userSign` / `userCompleteTask` /
// `userBrowse`（**零样本**），以及同一次会话里抓到的两条配置口与一条埋点口。
//
// 信封判据、鉴权头、`version` 那一套都在 [`app_cloud_phone.go`]（app_cloud_phone.go），
// 本文件不重复；那份文件头列了"39 条里只有 10 条是本族"这件事。
//
// # 实拍与零样本在本文件里怎么分
//
//	第 32 条 redpacket/configTaskList        body {"version":"SYS_CONFIG_Y"}
//	第 36 条 redpacket/configTaskLoginList   body {"version":"SYS_CONFIG_Y"}
//	第 30/33 条 activity/common/ruleInfo     body {"activityCode":"STORAGE_REDPACKET_RULE"}
//	第 38 条 rule/config/list                body {"codes":"STORAGE_REDPACKET_POP,…"}
//	第 31/34 条 bilog/upload                 body {"action":…,"activityId":…,"extra1":…}
//
//	零样本（成文 §2.4 第 373/374/379 行点了名，夹具里 grep 不到）：
//	userSign、userCompleteTask、userBrowse
//
// ⚠ 零样本那三条连**路径前缀**都是推的：实拍里 `redpacket/` 前缀只被那四条支持过，
// 而成文 §2.4 写任务动作时只给了尾段名。所以每条的路径各成**一个常量**，
// 404 时改一处即可（处置与 `msgPushObtainMethod` 同一个路子）。
package upstream

import (
	"context"

	"github.com/catlair/mcloud/internal/core"
)

// ─────────────────────────────────────────────────────────────────────────────
// 任务清单（两条实拍的口）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneActivityHomeURL 是活动页首页（**推**自第 9 条里那条未脱敏的跳转 URL：
// `https://cpactiv.buy.139.com/#/redEnvelopeParty/home?loginType=0&token=…&enableShare=1&channelSrc=red-mcloudapp-001`）。
//
// ⚠ 它**不含** token，因为带 token 的那一版才是实拍原样，而本仓库不许把凭据成文。
// 用途也只有一个：当 `referer` 的候选。实拍十条的 `Referer` **全被遮** ⇒
// "这一族的 referer 就是首页"这句**未核实**（WebView 里换页它就变）。
const CloudPhoneActivityHomeURL = CloudPhoneOrigin + "/#/redEnvelopeParty/home"

// CloudPhoneConfigTaskListRequest 构造 `POST …/redpacket/configTaskList`。
//
// 实拍（第 32 条，逐字，且 `content-length: 26` 与 body 字节数对得上）：
//
//	{"version":"SYS_CONFIG_Y"}
//
// # 它与 `configTaskLoginList` 是同一次会话里的**两条不同请求**
//
// 顺序上第 32 条在前、第 36 条在后，两条 body 逐字相同，而响应有两处差别
// （见 [CloudPhoneTaskList] 的 `SecondConfirmStatus` 与任务状态位）
// —— 正是 `AGENTS.md` §1.12 事实②那种"URL 不同而参数相同"的反面：
// **参数相同而 URL 不同**，所以两条都要有，不许合成一条。
func CloudPhoneConfigTaskListRequest(auth CloudPhoneAuth, referer string) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest("redpacket/configTaskList", nil, auth, referer)
}

// CloudPhoneConfigTaskLoginListRequest 构造 `POST …/redpacket/configTaskLoginList`。
//
// 实拍（第 36 条，逐字）：`{"version":"SYS_CONFIG_Y"}`。
//
// 成文 §2.4 第 373 行说主流程用的是 `configTaskLoginList`（"四张列表"），
// 也就是**登录后**那一版；`configTaskList` 是登录前/未登录态的同一份清单。
// ⚠ "先打哪一条"属编排，不在本层判。
func CloudPhoneConfigTaskLoginListRequest(auth CloudPhoneAuth, referer string) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest("redpacket/configTaskLoginList", nil, auth, referer)
}

// CloudPhoneTaskItem 是四张任务表（`configTaskDailyList` / `configTaskMonthlyList` /
// `configTaskNoviceList` / `configTaskBuyList`）里的一条。
//
// # 声明哪些、不声明哪些（实拍 18 个键）
//
// 本层只声明"决定做不做这一件、以及做它要带什么"的那些键。
// **不声明**的六个：`configType`(2)、`createTime`、`updateTime`、`sort`、`taskLogo`、
// `taskExplain` —— 前四个是后台元数据，`taskLogo` 是图片地址，
// `taskExplain` 是给人看的句子（实拍"可获得0.2元"这种，见 [CloudPhoneTaskItem.PrizeAmount]
// 那段：它是 PrizeAmount 的**旁证**，不是判据）。
// 声明出来只会多一处会漂移的字段表（同 [FunaiModule] 那一段的理由）。
type CloudPhoneTaskItem struct {
	// TaskID 是 `id` —— 实拍是**字符串**（`"6"`、`"11"`）。
	// ⚠ 别按数字收：这一族的 id 与金额位都是字符串（`canAmount` 也是）。
	TaskID *string
	// TaskCode 是 `taskCode` —— 分发的依据（实拍 `DAILY_1`、`MONTHLY_1`…`MONTHLY_5`、
	// `NOVICE_1`…`NOVICE_3`）。成文 §2.4 第 373 行那张分发表就是按它走的。
	// ⚠ 缺它这一条**无法分发** ⇒ 交给调用方按"未知任务码"处理（成文 §1.4 表里
	// `cloud-phone-redpack` 那一行点了"未知任务码"这一档）。
	TaskCode *string
	// TaskName / TaskType 是 `taskName` / `taskType`（实拍 `DAILY` / `MONTHLY` / `NOVICE`）。
	// ⚠ `configTaskBuyList` 那一张表**零样本**（实拍是空数组），
	// 所以"Buy 表的 taskType 是什么"没有答案。
	TaskName *string
	TaskType *string
	// PrizeName 是 `prizeName`；PrizeType 是 `prizeType`（实拍**全是** `1` ⇒ 其它取值零样本）。
	PrizeName *string
	PrizeType *int64
	// PrizeAmount 是 `prizeAmount` —— 实拍**字符串**（`"20"`、`"30"`）。
	//
	// ⭐ 单位是**分**，这一条在同一条目内部就有旁证：`DAILY_1` 的 `prizeAmount` 是 `"20"`
	// 而 `taskExplain` 写"可获得0.2元"；`MONTHLY_3` 的 `"30"` 配"每次答对可获得 0.3元"。
	// 与成文 §2.4 第 369 行那句"单位分（代码 `/100` 显示元）"一致。
	// ⚠ 本层**不做**除法（展示归上层）。
	PrizeAmount *int64
	// PrizeAmountMany 是 `prizeAmountMany` —— 实拍**每一条都等于 `prizeAmount`**。
	//
	// 为什么还留着：它是上游**独立给的一个键**，"恒等"只是这两次样本的巧合。
	// 哪天两者分叉（比如"多次完成"的奖励不同），这里读到的就是另一个数 ——
	// 而**没有**任何测试会因此变红，所以宁可多一个字段，也不要"顺手只读一个"。
	// 它到底管不管"一次做几件"，与 `periodCanNum` 是什么关系：**未验证**。
	PrizeAmountMany *int64
	// PeriodCanNum / UserCanCompleteNum / UserCompleteNum 是
	// `periodCanNum` / `userCanCompleteNum` / `userCompleteNum`（实拍都是数字）。
	//
	// 实拍能说的是：`MONTHLY_3` 三条样本里 `periodCanNum: 3`、`userCanCompleteNum` 与
	// `userCompleteNum` 一个 3/0（还能做 3 次、做过 0 次）、一个 0/3（额度用光）。
	// ⚠ 但 `userCanCompleteNum = periodCanNum − userCompleteNum` 这条等式
	// 只在实拍的 9 个条目上成立 ⇒ 是**观察**，不是上游承诺；本层不替你算，三个读数都交出去。
	PeriodCanNum       *int64
	UserCanCompleteNum *int64
	UserCompleteNum    *int64
	// UserStatus 是 `userStatus` —— 本层**唯一**带档位语义的判据，见 [CloudPhoneUserStatus]。
	//
	// 实拍：第 36 条里出现 `0`（未做）与 `1`（已完成）；第 32 条（登录前那一次）**全是** `0`。
	// ⇒ 同一件任务在两条口上状态不同（中间用户真做过几件），这也是"参数相同而 URL 不同"
	// 那两条要分开实现的另一个理由。
	UserStatus *CloudPhoneUserStatus
	// JumpLink 是 `jumpLink` —— **可缺**：实拍 `NOVICE_2` / `NOVICE_3` / `MONTHLY_1` 有，
	// `DAILY_1`、`MONTHLY_2`、`MONTHLY_3`、`MONTHLY_5` **没有**这个键。
	//
	// 它是 URL 且可能带 token（实拍 `…activityCenter?token=<REDACTED>&loginType=1&ssoType=2`）
	// ⇒ ⚠ 不许进日志、不许进结论；只在"这一件要不要跳过去"的判据里用一下。
	JumpLink *string
}

// CloudPhoneSignDay 是 `configTaskSignList` 里的一天（实拍 44 天）。
//
// # ⚠ 这里的 `status` **不**是任务条目那个 `userStatus`
//
// 实拍取值：`0` 出现 37 次（今天与过去的全部日子，含 `isToday: 1` 那天），
// `2` 出现 7 次（**2026-07-01 起的未来七天**，而响应 `Date` 是 2026-06-30）。
// ⇒ 同一个整数 2 在这个字段上的位置读起来是"还没到那一天"，与成文 §2.4 第 379 行
// 给 `statusText` 那五档里的"`2` 未定义但会进执行分支"**不是一回事**。
// 所以这里刻意用裸 `*int64` 而不是 [CloudPhoneUserStatus]：**两本字典不许合并**。
// ⚠ `1`（已签？）与 `3`/`4` 在本样本里一件都没有（`totalSignTimes: 0`，那天还没签）。
type CloudPhoneSignDay struct {
	Date       *string
	Day        *int64
	IsToday    *int64
	IsWeek     *int64
	SignAmount *int64
	Status     *int64
}

// CloudPhoneTaskList 是两条清单口共同的解析结果。
//
// 实拍 `data` 十二个键（第 36 条）/ 十一个键（第 32 条）。不声明的只有
// `aiAppLink` / `aiH5Link` / `iconLink` / `iconUrl` 四个：实拍前两个是带着 token 的
// 第三方跳转地址（139 邮箱、云盘 H5），后两个是**空串** ⇒ 既不是判据也不该成文。
type CloudPhoneTaskList struct {
	// Daily / Monthly / Novice / Buy 是 `configTaskDailyList` / `…MonthlyList` /
	// `…NoviceList` / `…BuyList` 四张表。
	//
	// ⚠ `configTaskBuyList` 实拍**恒是空数组** ⇒ 元素形状零样本，
	// 这里按"同一张表的结构"解（键名逐字 `configTaskBuyList` 有实拍，条目没有）。
	Daily   []CloudPhoneTaskItem
	Monthly []CloudPhoneTaskItem
	Novice  []CloudPhoneTaskItem
	Buy     []CloudPhoneTaskItem
	// SignList 是 `configTaskSignList` —— 签到日历（实拍 44 项）。
	SignList []CloudPhoneSignDay
	// TotalSignTimes 是 `totalSignTimes`（实拍 `0`）。⚠ 它与 SignList 里
	// "已签的那几格"是什么关系**未验证**（本样本里一格都没签过）。
	TotalSignTimes *int64
	// WeixinPrize 是 `weixinPrize` —— 实拍**字符串** `"1"`（不是数字）。
	// ⚠ 语义未记录（微信类奖品开关？望文生义，本层不解释）。
	WeixinPrize *string
	// SecondConfirmStatus 是 `secondConfirmStatus`（实拍 `0`）。
	//
	// ⚠⚠ **只有 `configTaskLoginList` 给这一位**：第 36 条有、第 32 条**没有**
	// （两条响应的键集合就差这一个）。所以它必须是指针 ——
	// 缺它 ≠ 给了 0；把两条口都填成 0 会让人以为"未登录态也是二次确认关"。
	// 它管什么（二次确认弹窗？兑换前的确认？）零证据 ⇒ 不解释。
	SecondConfirmStatus *int64
}

// ParseCloudPhoneTaskList 解析两条清单口（同一形状）。
//
// 判据先看信封（[EnsureCloudPhoneOK]），`data` 缺失 ⇒ 交一个**全空**结果而不报错：
// 这条是只读清单，"上游什么都没给"要落成编排层的未知档，不是落成一次接口故障
// （与 [ParseNoticeTaskInfo] 同一处置）。
func ParseCloudPhoneTaskList(envelope *Envelope) (CloudPhoneTaskList, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneTaskList{}, perr
	}
	return CloudPhoneTaskList{
		Daily:               cloudPhoneTaskItems(data, "configTaskDailyList"),
		Monthly:             cloudPhoneTaskItems(data, "configTaskMonthlyList"),
		Novice:              cloudPhoneTaskItems(data, "configTaskNoviceList"),
		Buy:                 cloudPhoneTaskItems(data, "configTaskBuyList"),
		SignList:            cloudPhoneSignDays(data, "configTaskSignList"),
		TotalSignTimes:      optInt64Field(data, "totalSignTimes"),
		WeixinPrize:         cloudPhoneString(data, "weixinPrize"),
		SecondConfirmStatus: optInt64Field(data, "secondConfirmStatus"),
	}, nil
}

// cloudPhoneTaskItems 解一张表。
//
// ⚠ 三种"没给"都落成 nil，而且**故意不报错**：
//  1. 这个键不存在（第 32 条就没有 `secondConfirmStatus`，同理清单口哪天可以少给一张表）；
//  2. 给了 `null`；
//  3. 给了但**不是数组**（形状变了）。
//
// 为什么不报错：这是**只读**清单口，"读不出来"要由编排层落成"这一格不知道"，
// 而不是落成一次接口故障（同 [ParseNoticeTaskInfo] 的处置）。
// ⚠ 代价要说清：第 3 种（上游把数组换成对象）今天**没有任何断言会红**，
// 只会表现为"清单是空的"。把它做成错误会打断"上游少给一张表"这条正常路径，
// 两害相权选了"看得见的那一侧"——`app_cloud_phone_task_test.go` 里
// 有一条用例专门盯这一档，改成报错就会红，别顺手改。
func cloudPhoneTaskItems(data map[string]any, key string) []CloudPhoneTaskItem {
	if data == nil {
		return nil
	}
	raw, ok := data[key]
	if !ok || raw == nil {
		return nil
	}
	list, ok := raw.([]any)
	if !ok {
		return nil
	}
	out := make([]CloudPhoneTaskItem, 0, len(list))
	for _, item := range list {
		obj, _ := item.(map[string]any)
		out = append(out, CloudPhoneTaskItem{
			TaskID:             cloudPhoneString(obj, "id"),
			TaskCode:           cloudPhoneString(obj, "taskCode"),
			TaskName:           cloudPhoneString(obj, "taskName"),
			TaskType:           cloudPhoneString(obj, "taskType"),
			PrizeName:          cloudPhoneString(obj, "prizeName"),
			PrizeType:          optInt64Field(obj, "prizeType"),
			PrizeAmount:        optInt64Field(obj, "prizeAmount"),
			PrizeAmountMany:    optInt64Field(obj, "prizeAmountMany"),
			PeriodCanNum:       optInt64Field(obj, "periodCanNum"),
			UserCanCompleteNum: optInt64Field(obj, "userCanCompleteNum"),
			UserCompleteNum:    optInt64Field(obj, "userCompleteNum"),
			UserStatus:         cloudPhoneStatusAt(obj, "userStatus"),
			JumpLink:           cloudPhoneString(obj, "jumpLink"),
		})
	}
	return out
}

// cloudPhoneSignDays 解签到日历，处置同上。
func cloudPhoneSignDays(data map[string]any, key string) []CloudPhoneSignDay {
	list, ok := data[key].([]any)
	if !ok {
		return nil
	}
	out := make([]CloudPhoneSignDay, 0, len(list))
	for _, item := range list {
		obj, _ := item.(map[string]any)
		out = append(out, CloudPhoneSignDay{
			Date:       cloudPhoneString(obj, "date"),
			Day:        optInt64Field(obj, "day"),
			IsToday:    optInt64Field(obj, "isToday"),
			IsWeek:     optInt64Field(obj, "isWeek"),
			SignAmount: optInt64Field(obj, "signAmount"),
			Status:     optInt64Field(obj, "status"),
		})
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 任务码（实拍的九个 + 成文点到的一个；值逐字照抄）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneTaskCode 是 `taskCode` 的取值。
//
// # 为什么只有**值**这一层有名字、没有"该走哪条分支"的表
//
// 分发（浏览 / 安装 / 答题 / 手动）是编排层的活：成文 §2.4 第 373 行那张表里
// `BROWSE_TASKS{NOVICE_2,NOVICE_3,MONTHLY_1}` 这类归属**只有一句描述**，
// 而任务表会随活动期次换（同一份夹具里 `NOVICE_2` 与 `NOVICE_3` 的 `jumpLink`
// 指向**同一个**微信专辑地址，而 `MONTHLY_1` 指向活动中心 —— 三条并不是一类东西）。
// ⇒ 本层只把码成文，让"少了一档"至少能 grep 到出处。
type CloudPhoneTaskCode string

const (
	// CloudPhoneTaskCodeDaily1 是 `DAILY_1`：实拍"每日使用云机"（0.2 元）。
	// ⚠ 成文 §2.4 第 377 行点名它是那个**假成功**分支（旧实现在这里只打一句
	// "需手动完成…（需云机内实际使用）"然后 `return true`）。本层不参与那个决定。
	CloudPhoneTaskCodeDaily1 CloudPhoneTaskCode = "DAILY_1"
	// 浏览类三条（成文 §2.4 第 374 行那张集合里的成员，逐字）。
	CloudPhoneTaskCodeNovice2  CloudPhoneTaskCode = "NOVICE_2"
	CloudPhoneTaskCodeNovice3  CloudPhoneTaskCode = "NOVICE_3"
	CloudPhoneTaskCodeMonthly1 CloudPhoneTaskCode = "MONTHLY_1"
	// 实拍里其余的码：`NOVICE_1`（领取 30GB 定向流量）、`MONTHLY_2`（在云手机里安装应用）、
	// `MONTHLY_3`（趣味答题挑战）、`MONTHLY_4`（打开云手机消息通知）、
	// `MONTHLY_5`（体验云手机 AI 助手）。
	CloudPhoneTaskCodeNovice1  CloudPhoneTaskCode = "NOVICE_1"
	CloudPhoneTaskCodeMonthly2 CloudPhoneTaskCode = "MONTHLY_2"
	CloudPhoneTaskCodeMonthly3 CloudPhoneTaskCode = "MONTHLY_3"
	CloudPhoneTaskCodeMonthly4 CloudPhoneTaskCode = "MONTHLY_4"
	CloudPhoneTaskCodeMonthly5 CloudPhoneTaskCode = "MONTHLY_5"
)

// String 返回上游字面量（拼 body 用，不是给人看的档位名）。
func (c CloudPhoneTaskCode) String() string { return string(c) }

// ─────────────────────────────────────────────────────────────────────────────
// 任务动作：userSign / userCompleteTask / userBrowse（**全部零样本**）
// ─────────────────────────────────────────────────────────────────────────────

// 三条零样本口的路径。⚠ 前缀 `redpacket/` 是**按同族四条实拍推的**，
// 这一段本身没有实拍 ⇒ 404 时先改这三行，别改头、别改 body。
const (
	cloudPhoneUserSignPath     = "redpacket/userSign"
	cloudPhoneCompleteTaskPath = "redpacket/userCompleteTask"
	cloudPhoneUserBrowsePath   = "redpacket/userBrowse"
)

// CloudPhoneUserSignRequest 构造签到那一枪。
//
// ⚠⚠ **零样本**：夹具里 `userSign` 命中 0。形状只来自成文 §2.4 第 373 行
// "→ `userSign` 签到"这半句 —— 它只给了动作名，**没给 body 里有什么键**。
//
// # 所以这里不猜键名，把 data 交回调用方
//
// 交叉参考那边能看到的只有开源 Go 参考实现的**已知 bug**：
// `docs/caiyun-go-feature-inventory.md` 第 72 行（表内编号 160）与第 224 行、
// `docs/caiyun-go-parameters-and-decisions.md` 第 166 行（表 T-4）都记着
// "`userSign` 收了 `taskID` 但 `body := map[string]interface{}{}`，taskID 从未进请求体
// ⇒ 签到永远打在服务端默认项"，处置是**先修再搬**。
// ⇒ 两件事因此确定：① 这一枪**有**一个任务 id 维度的入参；② 参考实现那个空 body
// 是**缺陷不是形状**，不许照抄。但"该叫 `taskId` 还是 `id`、是数字还是字符串"两处
// 都没证据 ⇒ 本层写成 `data` 参数，键名由接线方按**新抓的一条样本**填。
// 猜一个键名的代价是静默：上游按"没传任务"处理，信封照样回 `"200"`。
func CloudPhoneUserSignRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneUserSignPath, data, auth, referer)
}

// CloudPhoneUserCompleteTaskRequest 构造 `userCompleteTask`（"先记账"那一枪）。
//
// ⚠ **零样本**：夹具命中 0。成文 §2.4 第 373 行只写了"逐任务 `handleTask`：
// `userCompleteTask` → 按 `taskCode` 分发"，第 380 行另写了它的位置含义
// （**先记账后动作**：installApp / answerTopic 都先打这一枪再做实际动作）。
// body 的键名同样**未核实** ⇒ `data` 交回调用方。
func CloudPhoneUserCompleteTaskRequest(
	auth CloudPhoneAuth, referer string, data map[string]any,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneCompleteTaskPath, data, auth, referer)
}

// CloudPhoneCompleteTask 是 `userCompleteTask` 的解析结果。
type CloudPhoneCompleteTask struct {
	// Status 是 `data.status` —— 成文 §2.4 第 379 行：旧实现的失败判据是
	// `!isNil(data.data?.status) && status !== 1`，也就是**只认 1 为登记成功**。
	//
	// ⚠⚠ 两点要说清：
	//  1. **缺省**旧实现视为**成功**（`isNil` 那半句）。本层**刻意偏离**它 ——
	//     缺这里就是 nil，不折成 1、也不折成 0。理由是本仓的口径
	//     （"上游没给这一槽 ≠ 给了空槽"，先例见 [ParseNoticeTaskInfo] 与
	//     [MsgPushObtain.ObtainCode]）：把"判不了"伪造成"记账成功"，
	//     用户就再也看不到这一笔任务其实没登记上。
	//     这半句的偏离是**有意的**，别"顺手对齐旧实现"。
	//  2. 成文那句里的 `data.data` 是 **axios 响应**那层 `.data`（信封），
	//     也就是我们的 `data.status`。⚠ 但也存在"真是双层嵌套"的读法
	//     （第 375 行的 `thirdQueryUserRelationshipV2` 就明写要兼容 `data.data` 双层）。
	//     两种读法**零样本可分** ⇒ 这里按单层实现；真跑出"status 恒 nil"时
	//     第一个要试的就是改成读 `data.data.status`。
	Status *CloudPhoneUserStatus
}

// ParseCloudPhoneCompleteTask 解析"记账"那一枪的响应。
//
// `status` 与成文那张五档表**共用** [CloudPhoneUserStatus] 是有条件的：
// 成文第 379 行明说旧实现拿同一个 `statusText` 函数看这些位 ⇒ 复用档位名，
// 但"1 = 登记成功"这一格与任务表里"1 = 已完成"是否同源，仍**未验证**（零样本）。
func ParseCloudPhoneCompleteTask(envelope *Envelope) (CloudPhoneCompleteTask, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneCompleteTask{}, perr
	}
	return CloudPhoneCompleteTask{Status: cloudPhoneStatusAt(data, "status")}, nil
}

// CloudPhoneUserBrowseRequest 构造浏览那一枪，body 逐字带一个 `taskCode`。
//
// ⚠ **零样本**：夹具命中 0。`taskCode` 这个键名有出处 —— 成文 §2.4 第 374 行
// 逐字写了 `userBrowse {taskCode}`（三份参考成文里唯一给出 body 键名的那一条）。
// ⚠ 但"它带 `version`"这半句只是按 §2.4 第 370 行那个统一形状推的（实拍支持的是
// `redpacket/` 那四条，不含这一条）。
//
// 成文同一段还记着旧实现是 `sleep(15000)` **之后**才发这一枪（浏览 15 秒）——
// 那个等待是编排层的事，本层只造请求。
func CloudPhoneUserBrowseRequest(
	auth CloudPhoneAuth, referer string, taskCode CloudPhoneTaskCode,
) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRequest(cloudPhoneUserBrowsePath,
		map[string]any{"taskCode": taskCode.String()}, auth, referer)
}

// ─────────────────────────────────────────────────────────────────────────────
// 两条配置口与一条埋点口（**有实拍**：第 30/33、38、31/34 条）
// ─────────────────────────────────────────────────────────────────────────────

// CloudPhoneActivityCodeRedpacketRule 是 `activity/common/ruleInfo` 的 `activityCode`
// 取值，实拍两条（第 30/33 条）逐字 `"STORAGE_REDPACKET_RULE"`。
// ⚠ 其它活动码**零样本**（这个键吃哪些别的值，成文与夹具都没说）。
const CloudPhoneActivityCodeRedpacketRule = "STORAGE_REDPACKET_RULE"

// CloudPhoneRuleInfoRequest 构造 `POST …/activity/common/ruleInfo`。
//
// 实拍 body（第 30/33 条，逐字，`content-length: 41` 与字节数一致）：
//
//	{"activityCode":"STORAGE_REDPACKET_RULE"}
//
// ⚠ 它**不带** `version` ⇒ 所以走 [CloudPhoneRawRequest] 而不是 [CloudPhoneRequest]
// （文件头那条复核结果：`version` 不是全族统一的）。
func CloudPhoneRuleInfoRequest(auth CloudPhoneAuth, referer, activityCode string) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRawRequest("activity/common/ruleInfo",
		map[string]any{"activityCode": activityCode}, auth, referer)
}

// CloudPhoneRuleText 是 ruleInfo / rule-config-list 两条口共同的载荷。
//
// 实拍两份都是**HTML 片段**（带 `<div class="rule_name">` 这类标签），
// ⚠ 本层原样交出、**不清洗也不渲染** —— 谁展示谁负责转义。
type CloudPhoneRuleText struct {
	Code     *string
	Title    *string
	RuleText *string
}

// ParseCloudPhoneRuleInfo 解析规则文本。
//
// 三个字段全可缺：这条是**纯展示**内容，读不出来只是"没有规则可看"，
// 不该把任何一条链打断。
func ParseCloudPhoneRuleInfo(envelope *Envelope) (CloudPhoneRuleText, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneRuleText{}, perr
	}
	return CloudPhoneRuleText{
		Code:     cloudPhoneString(data, "code"),
		Title:    cloudPhoneString(data, "title"),
		RuleText: cloudPhoneString(data, "ruleText"),
	}, nil
}

// CloudPhoneRuleCodes 是 `rule/config/list` 实拍的 `codes` 取值（逗号分隔，顺序照抄）。
const CloudPhoneRuleCodes = "STORAGE_REDPACKET_POP,STORAGE_REDPACKET_ROLL"

// CloudPhoneRuleConfigListRequest 构造 `POST …/rule/config/list`。
//
// 实拍 body（第 38 条，逐字）：`{"codes":"STORAGE_REDPACKET_POP,STORAGE_REDPACKET_ROLL"}`。
// ⚠ 它同样**不带** `version`。
//
// ⚠ 逐字对拍以夹具存的 **body 文本**为准：那一条的 `Content-Length: 56` 与 body
// 字节数**一致**（夹具十条里除第 29 条外全都一致 —— 那一条是 223 对 112，
// 因为它的 `deviceId` 与 `token` 两个值被换成了更短的占位串，**不是**夹具的错）。
// ⇒ 也就是说本族的 body 形状没有被脱敏动过手脚，可以放心按字节钉。
func CloudPhoneRuleConfigListRequest(auth CloudPhoneAuth, referer, codes string) (core.HTTPRequest, *core.Error) {
	return CloudPhoneRawRequest("rule/config/list",
		map[string]any{"codes": codes}, auth, referer)
}

// CloudPhoneAnnouncement 是 `rule/config/list` 的 `data.list[]` 一项。
//
// 实拍两项：`STORAGE_REDPACKET_POP`（弹窗通知，`readStatus` 1）与
// `STORAGE_REDPACKET_ROLL`（滚动通知，`readStatus` 0），两条 `status` 都是 `0`、
// `markNum` 都是 `0`。⚠ 各档语义**未验证**（"读没读过"是不是就是 `readStatus`，
// 只是两个取值配两段文案的巧合观察）。
type CloudPhoneAnnouncement struct {
	CloudPhoneRuleText
	AnnID      *string
	Status     *int64
	ReadStatus *int64
	MarkNum    *int64
}

// CloudPhoneRuleConfigList 是整份解析结果。
type CloudPhoneRuleConfigList struct {
	Items []CloudPhoneAnnouncement
}

// ParseCloudPhoneRuleConfigList 解析公告清单。`data.list` 不是数组 ⇒ 交 nil
// （同 [cloudPhoneTaskItems] 的处置：只读口不报错，但形状变了要能在结论里看出来）。
func ParseCloudPhoneRuleConfigList(envelope *Envelope) (CloudPhoneRuleConfigList, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneRuleConfigList{}, perr
	}
	raw, ok := data["list"].([]any)
	if !ok {
		return CloudPhoneRuleConfigList{}, nil
	}
	out := CloudPhoneRuleConfigList{Items: make([]CloudPhoneAnnouncement, 0, len(raw))}
	for _, item := range raw {
		obj, _ := item.(map[string]any)
		out.Items = append(out.Items, CloudPhoneAnnouncement{
			CloudPhoneRuleText: CloudPhoneRuleText{
				Code:     cloudPhoneString(obj, "code"),
				Title:    cloudPhoneString(obj, "title"),
				RuleText: cloudPhoneString(obj, "ruleText"),
			},
			AnnID:      cloudPhoneString(obj, "id"),
			Status:     optInt64Field(obj, "status"),
			ReadStatus: optInt64Field(obj, "readStatus"),
			MarkNum:    optInt64Field(obj, "markNum"),
		})
	}
	return out, nil
}

// CloudPhoneBilogActivityID 是两条 `bilog/upload` 实拍里 `activityId` 的取值，逐字
// `"cmVkRW52ZWxvcGVQYXJ0eQ=="` —— 它是 `redEnvelopeParty` 的 base64。
//
// ⚠ 原样带出，**不许**顺手换成明文或"重新编码一遍"：实拍就是这串。
const CloudPhoneBilogActivityID = "cmVkRW52ZWxvcGVQYXJ0eQ=="

// CloudPhoneBilogFields 是一条埋点的三个可选位。
//
// 实拍两条：第 31 条 `{"action":"首页-活动页曝光","extra1":"红包派对首页"}`、
// 第 34 条 `{"action":"登录-登录成功","extra1":"单点登录","extra2":"<掩码手机号>"}`。
// ⇒ `action` 是必填（两条都有），`extra1`/`extra2` 可缺；`extra2` 实拍那一枚是
// **账号标识**，本层的测试与注释一律用掩码形态，不许把号码印进任何成文。
type CloudPhoneBilogFields struct {
	Action string
	Extra1 string
	Extra2 string
}

// CloudPhoneBilogUploadRequest 构造 `POST …/bilog/upload`。
//
// ⚠ 这是**埋点**，不参与任何判据：实拍响应只有 `{"ret":1}`。
// 它要不要接进链路属**待拍板**（先例：兑换族那两条 `visitlog/journaling`
// 就被登记成"不接"，见 `docs/business-feature-inventory.md` §2.4 `exchange` 那一段末
// "与旧实现的三处刻意不同"）。本层只把请求造出来，不主张该发。
//
// body 的键集合随字段给不给而变 ⇒ 走 [CloudPhoneRawRequest]（这一条实拍**不带** `version`）。
func CloudPhoneBilogUploadRequest(
	auth CloudPhoneAuth, referer string, fields CloudPhoneBilogFields,
) (core.HTTPRequest, *core.Error) {
	body := map[string]any{
		"action":     fields.Action,
		"activityId": CloudPhoneBilogActivityID,
	}
	if fields.Extra1 != "" {
		body["extra1"] = fields.Extra1
	}
	if fields.Extra2 != "" {
		body["extra2"] = fields.Extra2
	}
	return CloudPhoneRawRequest("bilog/upload", body, auth, referer)
}

// CloudPhoneBilogRet 是埋点响应的读数。
type CloudPhoneBilogRet struct {
	// Ret 是 `data.ret` —— 实拍**两条都是 1**。⚠ 它是不是成功判据**未验证**
	// （没有任何一条非 1 的样本可以对照；成文也没提这一位）。
	Ret *int64
}

// ParseCloudPhoneBilogRet 解析埋点响应。
func ParseCloudPhoneBilogRet(envelope *Envelope) (CloudPhoneBilogRet, *core.Error) {
	data, perr := EnsureCloudPhoneOK(envelope)
	if perr != nil {
		return CloudPhoneBilogRet{}, perr
	}
	return CloudPhoneBilogRet{Ret: optInt64Field(data, "ret")}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 发送层（本文件的只读四条 + 写三条）
// ─────────────────────────────────────────────────────────────────────────────

// AppCloudPhoneConfigTaskList 查未登录态任务清单（`configTaskList`）。**只读**。
func (u *Upstream) AppCloudPhoneConfigTaskList(
	ctx context.Context, auth CloudPhoneAuth, referer string,
) (CloudPhoneTaskList, *core.Error) {
	return u.cloudPhoneTaskListAt(ctx, CloudPhoneConfigTaskListRequest, auth, referer)
}

// AppCloudPhoneConfigTaskLoginList 查登录后任务清单（`configTaskLoginList`）。**只读**。
//
// 成文 §2.4 第 373 行主流程用的是这一条。
func (u *Upstream) AppCloudPhoneConfigTaskLoginList(
	ctx context.Context, auth CloudPhoneAuth, referer string,
) (CloudPhoneTaskList, *core.Error) {
	return u.cloudPhoneTaskListAt(ctx, CloudPhoneConfigTaskLoginListRequest, auth, referer)
}

// cloudPhoneTaskListAt 是两条清单口共用的"发 + 解"。
//
// 为什么共用而不是抄两遍：两条的**响应形状逐字相同**（第 32/36 条实测只差
// `secondConfirmStatus` 一个键），分开写早晚两份解析漂移。请求侧的差别
// （路径）在各自的构造函数里，那一处**不**共用。
func (u *Upstream) cloudPhoneTaskListAt(
	ctx context.Context,
	build func(CloudPhoneAuth, string) (core.HTTPRequest, *core.Error),
	auth CloudPhoneAuth, referer string,
) (CloudPhoneTaskList, *core.Error) {
	request, perr := build(auth, referer)
	if perr != nil {
		return CloudPhoneTaskList{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneTaskList{}, perr
	}
	return ParseCloudPhoneTaskList(envelope)
}

// AppCloudPhoneRuleInfo 取规则文本（`activity/common/ruleInfo`）。**只读**。
func (u *Upstream) AppCloudPhoneRuleInfo(
	ctx context.Context, auth CloudPhoneAuth, referer, activityCode string,
) (CloudPhoneRuleText, *core.Error) {
	request, perr := CloudPhoneRuleInfoRequest(auth, referer, activityCode)
	if perr != nil {
		return CloudPhoneRuleText{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneRuleText{}, perr
	}
	return ParseCloudPhoneRuleInfo(envelope)
}

// AppCloudPhoneRuleConfigList 取公告清单（`rule/config/list`）。**只读**。
func (u *Upstream) AppCloudPhoneRuleConfigList(
	ctx context.Context, auth CloudPhoneAuth, referer, codes string,
) (CloudPhoneRuleConfigList, *core.Error) {
	request, perr := CloudPhoneRuleConfigListRequest(auth, referer, codes)
	if perr != nil {
		return CloudPhoneRuleConfigList{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneRuleConfigList{}, perr
	}
	return ParseCloudPhoneRuleConfigList(envelope)
}

// AppCloudPhoneUserCompleteTask 打"记账"那一枪。**写请求，且零样本**。
//
// ⚠ 成文 §2.4 第 380 行点名它的位置问题（**先记账后动作** ⇒ 后续动作失败时任务已登记完成）。
// 那个顺序**不在本层改**（这里只有一条请求），改在编排层：要么先做后记，
// 要么把"登记完成"当作显式的最后一步。
func (u *Upstream) AppCloudPhoneUserCompleteTask(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) (CloudPhoneCompleteTask, *core.Error) {
	request, perr := CloudPhoneUserCompleteTaskRequest(auth, referer, data)
	if perr != nil {
		return CloudPhoneCompleteTask{}, perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return CloudPhoneCompleteTask{}, perr
	}
	return ParseCloudPhoneCompleteTask(envelope)
}

// AppCloudPhoneUserBrowse 打浏览那一枪。**写请求，零样本**。
//
// ⚠ 调用前该等的 15 秒不在这里等（见 [CloudPhoneUserBrowseRequest]）。
// ⚠ 载荷不交出：这一枪响应里有什么键**一条样本都没有** ⇒ 只判信封。
// 这里**必须**判信封：[u.Call] 只在非 2xx 上回错误，而本族的信封判据在
// `header.status`（**没有** `code` 字段，见 [CloudPhoneSuccessStatus]），
// 不判就等于把"上游回了非 200"当成浏览成功。
func (u *Upstream) AppCloudPhoneUserBrowse(
	ctx context.Context, auth CloudPhoneAuth, referer string, taskCode CloudPhoneTaskCode,
) *core.Error {
	request, perr := CloudPhoneUserBrowseRequest(auth, referer, taskCode)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = EnsureCloudPhoneOK(envelope)
	return perr
}

// AppCloudPhoneUserSign 打签到那一枪。**写请求，零样本，且 body 键名未核实**。
//
// 为什么载荷不交出来：这一枪的响应里有什么键**一条样本都没有**，
// 声明字段就是凭空造表 ⇒ 只判信封（要读载荷得先抓一条，见函数注释出处）。
func (u *Upstream) AppCloudPhoneUserSign(
	ctx context.Context, auth CloudPhoneAuth, referer string, data map[string]any,
) *core.Error {
	request, perr := CloudPhoneUserSignRequest(auth, referer, data)
	if perr != nil {
		return perr
	}
	envelope, perr := u.Call(ctx, request)
	if perr != nil {
		return perr
	}
	_, perr = EnsureCloudPhoneOK(envelope)
	return perr
}
