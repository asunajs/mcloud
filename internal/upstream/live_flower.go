// 小红花（`liveRoomFeedback`）—— 口令抽小红花的六步链。
//
// 对应原项目 `packages/core/src/apis/live-room.ts` 与
// `packages/core/src/tasks/red-flower/standalone.ts`。
//
// # 这条链的六步（顺序固定）
//
//	① POST /ycloud/liveRoomFeedback/user/info            ← 余额，**终值核对也是它**
//	② POST /ycloud/visitlog/journaling            × 5    ← 埋点
//	③ POST /ycloud/liveRoomFeedback/firstParticipate/give ← 首次参与惊喜奖
//	④ POST /ycloud/liveRoomFeedback/redFlower/draw?code=  ← 逐个口令（**写**）
//	⑤ GET  /ycloud/liveRoomFeedback/redFlower/queryRevivalFlowers
//	⑥ POST /ycloud/liveRoomFeedback/user/info            ← 再取一次，前后对比
//
// # ⚠⚠ 三个请求的"请求体形态"**各不相同**，别统一成 JSON
//
// 2026-09-22 实拍（docs/capture/app-live-redflower-samples.txt §5.1）逐条钉住：
//
//	user/info              content-type: application/json                     body {}
//	visitlog/journaling    content-type: application/x-www-form-urlencoded    body module=uservisit&…
//	firstParticipate/give  **无 content-type**（content-length: 0）             body 空
//	redFlower/draw         **无 content-type**（content-length: 0）             body 空
//
// ⇒ `give` 与 `draw` **不能**走 `WithJSONBody`：那会补一个 `content-type: application/json`
// 并发出一个 `{}`（content-length: 2）。多发两个字节通常无害，但"照抄原项目"在这里
// 恰好是对的（原项目传的是 `undefined`），而**照抄直觉**（"POST 就该有 JSON body"）是错的。
//
// # 余额是 [RedFlowerBalance.FlowerCnt]，不是排行榜那个
//
// ⚠ `getRedFlowerTop10List` 的 `flowerCnt` 是**别人的**（实拍第一名 2055），
// 与 `app-exchange-samples.txt` §1.5「`beforeTotal` 不是余额」是同一类坑。
// 所以本文件**根本不提供**那个接口 —— 不提供就不会被误用。
//
// # ⚠⚠⚠ 口令（kouling）的五条事实（2026-09-23 用户纠正）
//
// 早先本文件与 `scripts/probe-live-room.js` 里有一句"**投第二次会回 `200112`"**，
// 那是把"口令一次性 / 会被消耗"这个**未验证的前提**当成了结论。以下五条是纠正后的口径，
// 写这一族代码前先读一遍：
//
//	① **只在本次直播有效**：直播时间外不可用，**上一场的口令也不能用**。
//	   上游原话就是 `200108` 那句"口令需当场直播获取才有效哦"。
//	   ⇒ ⚠⚠ **推论（2026-09-23 用户纠正）**：**任何"历史口令"都是废值** ——
//	   不要保存、不要跨场复用、**不要拿已结束场次的弹幕当口令来源**。
//	   所以那条"公开 CDN 的历史弹幕"（`docs/capture/app-live-ws-samples.txt` §9）
//	   **只能用来取证**（验抽取器 / 看形态 / 看出现次数），**不是一条取口令的路**。
//	② **全场通用**：所有用户共用同一批口令 ⇒ **不存在"消耗口令"这回事**，
//	   也不存在"我把口令用掉了别人就没得用"。
//	③ **每场两个固定口令**（小概率会多一个随机口令）⇒ 收集那一步拿到 2~3 条是正常形状，
//	   别把"只收到 1 条"当成已收全，也别把"收到 3 条"当成有脏数据。
//	④ **进直播间就会有人发口令**：任意用户（**包括自己和别人**）进入移动云盘直播间后，
//	   系统会**以该用户的名义**发一条口令 ⇒ 听弹幕收到的口令，**署名是谁都能用**；
//	   别按"发信人是不是我"去过滤。
//	⑤ **另一条口令在别的平台（小红书）**：那边没法鉴权，**不做自动化** ——
//	   由人去小红书找别人分享出来的那条，再手工喂进来（CLI 的 `--codes`）。
//	   ⇒ 这就是为什么"听弹幕"永远拿不全两条：**`--codes` 不是可选项，是另一条的必需入口**。
package upstream

import (
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// 小红花族的常量。
const (
	// BaseLiveRoomFeedback 是小红花活动页的接口根（`/ycloud` 下，与签到同域）。
	BaseLiveRoomFeedback = "https://m.mcloud.139.com/ycloud"
	// LiveRoomMarketName 是**区分活动**的键。
	//
	// ⚠ 它出现在两个地方：`visitlog/journaling` 的 form 体里，以及页面 URL 的 `path`。
	// 同一个 `visitlog/journaling` 接口也服务别的活动（实拍另见 `National_Activity`，
	// 那边 `sourceid` 是空串）⇒ **`marketName` 才是键，路径不是**。
	LiveRoomMarketName = "National_LiveRoomFeedback"
	// LiveRoomJournalingSourceID 是埋点的 `sourceid`（实拍 `1007`）。
	LiveRoomJournalingSourceID = "1007"
	// LiveRoomDrawPath 是抽小红花的路径（口令走 **query string**）。
	LiveRoomDrawPath = "/liveRoomFeedback/redFlower/draw"

	// LiveRoomUserAgent 是小红花活动页（H5）的 UA。
	//
	// ⚠ 与 [LiveUserAgent]（直播侧那个）**不是一个值**：这里的系统版本是 `18_7`
	// （2026-09-22 实拍），那边是 `18_5`（2026-09-17 夹具）。同一台设备在两个时间点
	// 抓到的 UA 会随系统升级而变，所以它**不是端常量，是抓包时刻的取值**
	// —— 照抄最近一次实拍，别去"统一"成一份。
	LiveRoomUserAgent = "Mozilla/5.0 (iPad; CPU OS 18_7 like Mac OS X) AppleWebKit/605.1.15 " +
		"(KHTML, like Gecko) MCloudApp/12.6.0 iPad AppLanguage/zh-CN"
)

// 小红花族的业务码。
//
// # ⚠ 码**只判等、不判形状**
//
// 沿用 `app-exchange-samples.txt` §1.3 那条：那一族的 `2301` 是四位、其余三位
// ⇒ 别按"都是 2001xx"去推。
//
// # ⚠⚠⚠ 码必须**对上它自己的接口** —— 不许张冠李戴
//
// 2026-09-23 纠正：本文件早先把 `200110` 命名为 `CodeRedFlower…`，而它出自
// `firstParticipate/give`、**不是** `draw`。名字错了，下一个读代码的人就会拿 give 的码
// 去判 draw 的结果 —— 这正是"张冠李戴"最容易发生的地方（它不报错，只是判错）。
// 现在的归属：
//
//	| 码     | 出自哪个接口                    | 语义                        | 证据         |
//	| ------ | ------------------------------- | --------------------------- | ------------ |
//	| 200108 | `redFlower/draw?code=`          | 口令无效（**需当场直播获取**）| 实拍         |
//	| 200110 | `firstParticipate/give`         | 已参与过，无首次惊喜奖        | 实拍         |
//	| 200103 | `liveRoomFeedback/check?prizeId=` | 红花数不够                 | 实拍（4 发全）|
//	| 200112 | `redFlower/draw?code=`           | 已兑换                      | 实拍（2026-09-23）|
//	| 200118 | `redFlower/draw?code=`           | 口令已失效（**本场已结束**）  | 实拍（2026-09-23，见 [CodeRedFlowerKoulingExpired]）|
//
// # ⭐⭐ 客户端（H5）自己认的**全部**码 —— 供对拍，本文件只实现有实拍的那几个
//
// 来源：`har/liveRoomFeedback.0cdf4040.js`（H5 活动页的 JS，本仓库抓下来的）。
// 它的 `drawRedFlower` 回调里有一张**码 → 弹窗**的映射表（原文见
// `docs/capture/app-live-redflower-samples.txt` §10.6）。⭐ 这份表比上游 `msg` 有用：
// `msg` 只是一句话，而它给出**厂商自己的分类标签 + 用户文案 + 该走哪个弹窗**。
//
//	| 码     | 客户端标签          | 官方文案                                   | 本文件 |
//	| ------ | ------------------- | ------------------------------------------ | ------ |
//	| 200107 | `popEmptyCode`      | 请输入口令                                 | ❌ 未实现 |
//	| 200108 | `popErrorCode`      | 口令不对耶 / 口令需**单场**直播获取才有效哦~ | ✅ |
//	| 200109 | `popNotLive`        | 需在直播期间内兑换哦                        | ❌ 未实现 |
//	| 200118 | `popNotLive`        | 需在直播期间内兑换哦                        | ✅ |
//	| 200110 | `pop_invalidPhone`  | 您无法参与活动~                            | ✅（但那是 `give` 的码）|
//	| 200101 | `pop_invalidPhone`  | 您无法参与活动~                            | ❌ 未实现 |
//	| 609    | `pop_invalidPhone`  | 您无法参与活动~                            | ❌ 未实现 |
//	| 200111 | `popNotApp`         | 该活动仅限在云盘APP参与哦                   | ❌ 未实现 |
//	| 200112 | `popRepeatCode`     | 口令已兑换过了 / 每个口令只能兑换一次        | ✅ |
//	| 200114 | `pop_monthFlower`   | —                                          | ❌ 未实现 |
//	| 200115 | `pop_quarterFlower` | —                                          | ❌ 未实现 |
//
// ⚠⚠ 上表里有**两个**码映射到 `popNotLive`（`200109` 与 `200118`）⇒
// 「需在直播期间内兑换哦」那个弹窗**不是 `200118` 专属的**。
// ⚠ `200109` 在**什么情形下**出现，本仓**未测**（现有实拍里它一次都没出现过：
// 我们那两枪拿到的是 `200118`）—— 别按"没直播就回 `200109`"去推，那是猜的。
// 要核它得：在**窗口已关**且**没有任何直播活动**时投一条真口令，看回哪个码。
//
// ⚠⚠ **"客户端标签"是命名依据，不是行为契约**（坑表 **#136**）：它说明"这个码**叫什么**"，
// **不**说明"服务端会不会校验某个头"。两件事别混。
// ⚠ 表里没有的码，H5 直接 `$toast(o.msg)`（弹上游原文）⇒ 客户端也**没有**穷举所有码。
//
// ⚠ `200103` 那条接口（兑换提交那一步）本文件**不提供** —— 与下面"不提供
// `getRedFlowerTop10List`"是同一条纪律：不提供就不会被误用。
const (
	// CodeRedFlowerInvalidKouling 是 `200108`：**口令无效**。
	//
	// ⭐ 原项目**没记过**这个码（它只笼统写"其他数字 ⇒ 无效口令"），
	// 2026-09-22 实拍补上：`{"code":200108,"msg":"口令不对耶，口令需当场直播获取才有效哦"}`。
	// 上游这句"需当场直播获取"就是下面「口令只在本次直播有效」那条的证据。
	CodeRedFlowerInvalidKouling int64 = 200108
	// CodeFirstParticipateAlreadyJoined 是 `200110`：**已经参与过活动**，没有首次参与惊喜奖。
	//
	// ⚠⚠ 它出自 `POST firstParticipate/give`，**不属于 `draw`** ——
	// 早先它被命名为 `CodeRedFlowerNoFirstQualification`（张冠李戴，2026-09-23 改名，
	// 调用点 [ClassifyFirstParticipate] 一直是对的，错的是名字）。
	//
	// 实拍（2026-09-22）：`{"code":200110,"msg":"您已参与过活动，无法获得首次参与惊喜奖"}`。
	// 它是**静默**的一支 —— 不是错误，是"这次没有"。
	CodeFirstParticipateAlreadyJoined int64 = 200110
	// CodeRedFlowerAlreadyDrawn 是 `200112`：**已兑换**。
	//
	// ⭐ **2026-09-23 实拍**（此前标的是"零实拍"，取值只靠原项目字面量）：
	// 同一账号把**同一条**正确口令投第二次 ⇒
	// `{"code":200112,"msg":"口令已兑换过了，每个口令只能兑换一次!"}`。
	// 原始样本见 `docs/capture/app-live-redflower-samples.txt` §10.2。
	//
	// ⭐ **"每个口令只能兑换一次"是按账号算的** —— **用户口述**（2026-09-23 明确）：
	// **两个不同账号投同一条口令，各自都会成功** ⇒ 与文件头第 ② 条"口令全场通用、
	// 不存在消耗"**不冲突**（"一次"限制的是**账号 × 口令**这一对，不是口令本身）。
	// ⚠ 本仓**没有这一条的实拍**（2026-09-23 17:09 想补，撞上直播窗口关闭 ⇒ 回 `200118`）。
	// ⚠⚠ **别把"本仓没实拍"读成"这个说法是揣测"** —— 一手口述在本仓是**一等证据**
	// （与"用户拍板"同档），**不许**因为缺抓包就标成"未定"（坑表 **#142**）。
	//
	// 语义上它与 [CodeRedFlowerInvalidKouling] **不同**：已兑换说明口令本身是**对的**，
	// 所以它**不该**被当成无效口令（否则会把一个有效口令从缓存里剔掉，
	// 让同一批账号里后面的那几个白跑一趟）。
	CodeRedFlowerAlreadyDrawn int64 = 200112
	// CodeRedFlowerKoulingExpired 是 `200118`：**这条口令认识、但已经失效了**（本场已结束）。
	//
	// ⭐ **2026-09-23 17:09 实拍**（直播窗口已关、`gateOpen: false`）：
	// 两条**真但已过期**的口令各投一次 ⇒ 都回 `200118`。
	// ⭐⭐ **两条独立证据面同答**：
	//   ① **服务端实验**证明"服务端确实这么判"（上面那一枪 + 乱码对照）；
	//   ② **客户端那张表**给出官方命名 —— `200118` ⇒ **`popNotLive`**、
	//      文案「**需在直播期间内兑换哦**」（`har/liveRoomFeedback.0cdf4040.js`，
	//      原文见 `docs/capture/app-live-redflower-samples.txt` §10.6）。
	// ⚠⚠ 上游原文（[RedFlowerDrawOutcome.Message]）解析了却**从不输出**
	// ⇒ 别指望从我们自己的产物里读到它。
	// ⚠ 而"**口令级**还是**场次级**"这一格**只有实验能答**：同账号、同样"直播已关"，
	// 投**乱码 / 不存在的口令**仍回 `200108`
	// ⇒ `200118` 是**口令级**的码（"我认识它、它失效了"），**不是**场次级（"没直播一律挡"）。
	// ⭐ 它顺带证明：服务端**记得住过去场次的口令**（否则认不出来）—— 与文件头第 ① 条同向；
	// 且**"过期"判断排在"这个账号兑换过没有"之前** ⇒ 窗口外**问不出** `200112` 那条悬案。
	//
	// ⭐ **2026-09-23 20:2x 已开分支**：[ClassifyRedFlowerDraw] 把它落进
	// [RedFlowerDrawKoulingExpired]（`String()` = `"kouling_expired"`）⇒
	// `app` 收进 `RedFlowerReport.ExpiredCodes`、CLI 的人读输出打
	// 「**口令已失效（本场已结束）**」、`--json` 出 `expiredCodes`。
	// ⚠ 早先这里写着"本文件刻意不为它开分支 ⇒ CLI 打「未分类口令」" ——
	// **那句已过期**：它把"**直播结束了**"和"**口令抄错了**"印成同一句话，
	// 而后者（`200108`）才是"口令不对"。判据与做法见 `docs/migration-pitfalls.md` #140。
	//
	// ⭐ **两条不变量由测试钉住**：`TestClassifyRedFlowerDrawOnExpiredKouling`
	// （`live_flower_test.go`）—— 它**不许**被判成无效口令（否则白重听 60 秒）、
	// 也**不许**被判成成功。
	// ⚠ 该测试的信封是**最小信封**（只有 `code`）：`200118` 的上游原文**仍无实拍**
	// （[RedFlowerDrawOutcome.Message] 解析了却从不输出），别把 `msg` 写进向量
	// —— 那等于把"我没抓到"写成"原文是这样"。
	//
	// ⚠ 边界：只有**两条**真口令 + **两条**不存在口令的对照，全在**同一场已结束的直播 +
	// 同一个账号**上；"它对所有过期口令都这样吗"**没测**。
	CodeRedFlowerKoulingExpired int64 = 200118
)

// LiveRoomPageReferer 拼小红花活动页的 `Referer`。
//
// # ⚠ 与实拍原串**刻意**差一段
//
// 实拍原串是
// `…/portal/yunClound/index.html?path=National_LiveRoomFeedback&sourceid=1007&token=<JWT>&targetSourceId=001005`
// —— 中间那段 `token` 是 H5 的登录票据。纯协议实现里凭据走 `jwttoken` **头**
// （实拍也确实是这么带的），所以这里把 `token` 去掉：
// 一个会过期的凭据拼进 URL，只会让日志与错误信息里多一处凭据。
// `path` / `sourceid` / `targetSourceId` 三段保留（它们表达"我是从哪个入口进来的"）。
func LiveRoomPageReferer() string {
	return fmt.Sprintf("%s/portal/yunClound/index.html?path=%s&sourceid=%s&targetSourceId=%s",
		BaseMcloud, LiveRoomMarketName, LiveRoomJournalingSourceID, ToSourceID)
}

// WithLiveRoomHeaders 给小红花族的请求补上公共头。
//
// 逐个对应实拍（§5.1 的「公共请求头」段）：
//
//	activityid: sign_in_3     ⭐ 与签到同族，不是直播专属
//	appversion: 12.6.0.0
//	jwttoken:   <JWT>
//	user-agent: <iPad App WebView>
//	referer / origin
//
// ⚠ **不碰 `content-type`**：这一族里三个请求的 body 形态各不相同（见文件头），
// 由各构造函数自己决定。这与 [WithH5Headers]（那里统一补 JSON）是**不同**的做法，
// 差别是故意的 —— 那边一条链上全是 JSON，这边不是。
func WithLiveRoomHeaders(request core.HTTPRequest, jwt *string) core.HTTPRequest {
	request = request.
		WithHeader("Accept", "*/*").
		WithHeader("activityid", MarketName).
		WithHeader("appversion", H5AppVersion).
		WithHeader("Referer", LiveRoomPageReferer()).
		WithHeader("Origin", BaseMcloud).
		WithHeader("User-Agent", LiveRoomUserAgent)
	if jwt != nil {
		// 头名是小写 `jwttoken`（实拍与 H5 拦截器都如此）。
		request = request.WithHeader("jwttoken", *jwt)
	}
	return request
}

// ─────────────────────────────────────────────────────────────────────────────
// ① 余额
// ─────────────────────────────────────────────────────────────────────────────

// RedFlowerBalance 是 `user/info` 的解析结果 —— **小红花余额的唯一判据**。
//
// 实拍（2026-09-22 09:04:13）：
//
//	{"code":0,"msg":"success","result":{"isQuarterDrawed":0,"flowerCnt":235,
//	 "prizeId":0,"secondPrizeId":0,"redFlowerValidDate":"2026年12月31日",
//	 "hasNotReceivedRedFlower":0}}
type RedFlowerBalance struct {
	// FlowerCnt 是**小红花数量** —— 六步链的①与⑥两次都取它，前后相减才是"这次进账多少"。
	FlowerCnt int64
	// RedFlowerValidDate 是有效期，**中文串**（`2026年12月31日`），不是时间戳。
	//
	// 别拿它去 `time.Parse`：它没有时区、没有零填充，解析失败只会得到一句
	// 与业务无关的报错。它是**给人看**的。
	RedFlowerValidDate string
	// HasNotReceivedRedFlower 是"有没有还没领的小红花"。
	HasNotReceivedRedFlower int64
	// IsQuarterDrawed 是季度奖是否已抽。
	IsQuarterDrawed int64
	// PrizeID / SecondPrizeID 是一/二等奖 id（0 = 没中）。
	PrizeID       int64
	SecondPrizeID int64
}

// RedFlowerUserInfoRequest 构造 `POST /ycloud/liveRoomFeedback/user/info`。
//
// body 是**空对象** `{}`（对齐实拍的 `content-length: 2`），不是"没有 body"。
func RedFlowerUserInfoRequest(jwt string) (core.HTTPRequest, *core.Error) {
	request, err := core.NewRequestPost(
		fmt.Sprintf("%s/liveRoomFeedback/user/info", BaseLiveRoomFeedback)).
		WithJSONBody(map[string]any{})
	if err != nil {
		return request, asCoreError(err, "序列化请求体失败")
	}
	return WithLiveRoomHeaders(request, &jwt), nil
}

// ParseRedFlowerBalance 解析 `user/info`。
//
// # 为什么只硬要求 `flowerCnt`
//
// 它是**唯一**回答了本函数问题（"我有几朵"）的字段。其余五个都是附属信息
// —— 少一个只该让那一项留空，不该让整次查询失败（那样会把一次**成功**的请求
// 变成解析错误，而错误信息里还看不出是哪个字段）。
//
// ⚠ 但 `flowerCnt` 必须硬要求：把"字段没了"静默成 0 会让
// "余额 235 → 0"看起来像**一次真实的清零**，而那正是最该立刻报警的形状。
func ParseRedFlowerBalance(envelope *Envelope) (RedFlowerBalance, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return RedFlowerBalance{}, err
	}
	if payload == nil {
		return RedFlowerBalance{}, core.NewParse("user/info 响应没有 result")
	}
	flowerCnt, err := RequireInt64(payload, "flowerCnt")
	if err != nil {
		return RedFlowerBalance{}, err
	}
	return RedFlowerBalance{
		FlowerCnt:               flowerCnt,
		RedFlowerValidDate:      strAt(payload, "redFlowerValidDate"),
		HasNotReceivedRedFlower: intAt(payload, "hasNotReceivedRedFlower"),
		IsQuarterDrawed:         intAt(payload, "isQuarterDrawed"),
		PrizeID:                 intAt(payload, "prizeId"),
		SecondPrizeID:           intAt(payload, "secondPrizeId"),
	}, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// ② 埋点
// ─────────────────────────────────────────────────────────────────────────────

// LiveRoomJournalingEvents 是原项目在抽奖前发的 5 条埋点（顺序即发送顺序）。
//
// # ⚠ 实拍只证实了其中一条
//
// 2026-09-22 实拍抓到的只有 `National_LiveRoomFeedback_flowerBtn`
// —— 而它是**点按钮那一刻**发的（`journaling` → `draw` 相邻）。
// 其余四条（`pv` / `client` / `client_isApp` / `login`）在真实客户端里属于
// **页面加载期**，本轮抓包窗口里没出现（很可能是窗口之前就发过了）。
//
// 那为什么还照原项目发全 5 条？因为这四条是**服务端活动系统的"到场"证据**，
// 而"它们到底是不是必发"**没有证据**。两边的代价不对称：
// 多发 4 个埋点请求 = 一点点噪声；少发 = 可能整条抽奖链被服务端静默拒掉
// （而那时的表现是 `200108`，看起来像"口令抄错了"，会把人引到完全错误的方向）。
// ⇒ 照抄原项目，并把"未验证"这件事写在这里。
var LiveRoomJournalingEvents = []string{
	"National_LiveRoomFeedback_pv",
	"National_LiveRoomFeedback_client",
	"National_LiveRoomFeedback_client_isApp",
	"National_LiveRoomFeedback_login",
	"National_LiveRoomFeedback_flowerBtn",
}

// LiveRoomJournalingRequest 构造 `POST /ycloud/visitlog/journaling`。
//
// body 是 **form-urlencoded**（不是 JSON），逐字：
//
//	module=uservisit&optkeyword=<…>&sourceid=1007&marketName=National_LiveRoomFeedback
//
// ⚠ 参数顺序照抄原项目。form 体的顺序在服务端通常无所谓，但这里没有理由偏离
// —— 而"照抄"让下一次抓包对拍时能直接逐字节比。
//
// `jwt` 仍然要带：实拍 §5.1 那句"公共请求头（**六个请求全带**）"里就含 `jwttoken`，
// 埋点不是第七条外站请求，它同在 `/ycloud` 域内。
func LiveRoomJournalingRequest(optkeyword, jwt string) core.HTTPRequest {
	body := fmt.Sprintf("module=uservisit&optkeyword=%s&sourceid=%s&marketName=%s",
		optkeyword, LiveRoomJournalingSourceID, LiveRoomMarketName)
	request := core.NewRequestPost(
		fmt.Sprintf("%s/visitlog/journaling", BaseLiveRoomFeedback))
	request.Body = core.Body(body)
	request = request.
		WithHeader("Content-Type", "application/x-www-form-urlencoded;charset=UTF-8")
	return WithLiveRoomHeaders(request, &jwt)
}

// ─────────────────────────────────────────────────────────────────────────────
// ③ 首次参与惊喜奖
// ─────────────────────────────────────────────────────────────────────────────

// FirstParticipateKind 是 `firstParticipate/give` 的三种结果。
type FirstParticipateKind int

const (
	// FirstParticipateGained 是 `code == 0`：拿到了惊喜奖。
	FirstParticipateGained FirstParticipateKind = iota
	// FirstParticipateNoQualification 是 [CodeFirstParticipateAlreadyJoined]：
	// **已经参与过**，这一支是静默的（不是错误，原项目连日志都不打）。
	FirstParticipateNoQualification
	// FirstParticipateOther 是别的码：原样带回给调用方。
	//
	// 刻意**不**在这里判"该不该报错"：上游会在不通知的情况下加新码，
	// 硬编码成"未知错误"会让新码永远查不出来（同 [QrFailureKind] 那条纪律）。
	FirstParticipateOther
)

// FirstParticipateOutcome 是 `firstParticipate/give` 的解析结果。
type FirstParticipateOutcome struct {
	Kind FirstParticipateKind
	// FlowerNum 只在 [FirstParticipateGained] 下有效。
	FlowerNum int64
	// Code 是原始业务码（nil = 响应里没有 `code` 字段）。
	Code *int64
	// Message 是上游原文，**只进日志**。
	Message string
}

// FirstParticipateGiveRequest 构造 `POST /ycloud/liveRoomFeedback/firstParticipate/give`。
//
// ⚠ body **空**、**无 content-type**（实拍 `content-length: 0`）——
// 别调 `WithJSONBody`（那会发出一个 `{}`）。
func FirstParticipateGiveRequest(jwt string) core.HTTPRequest {
	return WithLiveRoomHeaders(
		core.NewRequestPost(fmt.Sprintf("%s/liveRoomFeedback/firstParticipate/give",
			BaseLiveRoomFeedback)), &jwt)
}

// ClassifyFirstParticipate 判定 `firstParticipate/give` 的结果。
//
// # 为什么**不**用 [Envelope.EnsureOK]
//
// 那个把"非 0 即失败"一律变成错误，而 `200110`（已参与过）是**正常**的一支 ——
// 每个账号第二次跑这条链都会拿到它。用 `EnsureOK` 的话，一次完全正常的运行
// 会以一句"上游返回 code=200110"结束，读起来像故障。
func ClassifyFirstParticipate(envelope *Envelope) FirstParticipateOutcome {
	out := FirstParticipateOutcome{}
	code, hasCode := envelope.Code()
	if hasCode {
		out.Code = &code
	}
	if msg, ok := envelope.Message(); ok {
		out.Message = msg
	}
	if !hasCode {
		out.Kind = FirstParticipateOther
		return out
	}
	switch code {
	case CodeOK:
		out.Kind = FirstParticipateGained
		payload, _ := envelope.Payload()
		out.FlowerNum = intAt(payload, "flowerNum")
	case CodeFirstParticipateAlreadyJoined:
		out.Kind = FirstParticipateNoQualification
	default:
		out.Kind = FirstParticipateOther
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// ④ 抽小红花（**写**）
// ─────────────────────────────────────────────────────────────────────────────

// RedFlowerDrawResult 是 `draw` 成功时的 `result`。
type RedFlowerDrawResult struct {
	// PrizeID 是奖品 id。
	PrizeID int64
	// FlowerNum 是**这一口令换到几朵**（不是余额）。
	FlowerNum int64
	// PrizeName 是奖品名（原项目把它打进日志：`获得 N 朵（<prizeName>）`）。
	PrizeName string
	// PrizeType 是奖品类型。
	PrizeType int64
}

// RedFlowerDrawKind 是 `draw` 的判定结果。
type RedFlowerDrawKind int

const (
	// RedFlowerDrawSucceeded 是 `code == 0` 且 `result` 非空。
	RedFlowerDrawSucceeded RedFlowerDrawKind = iota
	// RedFlowerDrawAlreadyDrawn 是 [CodeRedFlowerAlreadyDrawn]（`200112`）。
	//
	// ⚠ 它**不是**无效口令：口令本身是对的。别把它并进 `RejectedKouling()`。
	RedFlowerDrawAlreadyDrawn
	// RedFlowerDrawInvalidKouling 是 [CodeRedFlowerInvalidKouling]（`200108`）——
	// 实拍确认的"口令不对"。
	RedFlowerDrawInvalidKouling
	// RedFlowerDrawOther 是**别的**业务码。
	//
	// # ⚠ 这里与原项目**刻意**不同
	//
	// 原项目的判据是 `typeof code === "number"` ⇒ 一律当"无效口令"
	// （`standalone.ts:74`）。那是它**没记过 `200108`** 时的兜底写法。
	// 实拍补上 `200108` 之后，"其他数字"就是一个**尚未分类**的集合
	// —— 里面可能有"活动没开始""账号被限"等等，把它们都当成"口令错"
	// 会触发一次毫无意义的 60 秒重听。
	//
	// 判据换成"码只判等"（坑表 #106 那族）之后，代价是**可能漏掉一个
	// 将来新增的'口令无效'码**。那一侧的代价是"少重听一次"，而重听本身
	// 还有第二条路（[RedFlowerDrawInvalidKouling] 已经覆盖了实测的那一个）。
	RedFlowerDrawOther
	// RedFlowerDrawUnparsable 是响应里**没有 `code` 字段**。
	//
	// 原项目这一支是"warn 之后跳过、**不判无效**"（避免误伤）。
	// 单独一类而不是并进 [RedFlowerDrawOther]：前者是"上游说了个别的码"，
	// 后者是"上游这次没按信封说话"—— 两者的排查方向完全不同。
	RedFlowerDrawUnparsable
	// RedFlowerDrawKoulingExpired 是 [CodeRedFlowerKoulingExpired]（`200118`）——
	// **这条口令认识、但它已经失效了**（本场直播已结束）。
	//
	// ⭐ 2026-09-23 17:09 实拍（capture §10.5）：同账号、同样"直播已关"，
	// 投**真但已过期**的口令 ⇒ `200118`；投**乱码 / 不存在的口令** ⇒ `200108`
	// ⇒ 它**不是**"口令抄错了"，也**不是**"上游回了个没分类的码"。
	//
	// ⚠ 与 [RedFlowerDrawOther] 分开的理由在**人读的那句话**上：
	// 两者原本都打「未分类口令」，而这一格**最常见**的成因恰恰是"**直播已经结束了**"
	// —— 那句文案把人往"口令抄错了"（[RedFlowerDrawInvalidKouling]）那边引，
	// 排查方向一开始就错。
	//
	// ⚠⚠ 它**不是**无效口令：口令本身是**对的**（错的回 `200108`）。
	// 所以 [RedFlowerDrawOutcome.RejectedKouling] 必须对它返回 `false` ——
	// 否则会触发一次毫无意义的 60 秒重听。
	//
	// ⚠ 排在 `iota` 的**末尾**是刻意的：中间插入会改掉后面那些的数值，
	// 而数值一旦被人读去当判据就收不回来。
	RedFlowerDrawKoulingExpired
)

func (k RedFlowerDrawKind) String() string {
	switch k {
	case RedFlowerDrawSucceeded:
		return "succeeded"
	case RedFlowerDrawAlreadyDrawn:
		return "already_drawn"
	case RedFlowerDrawInvalidKouling:
		return "invalid_kouling"
	case RedFlowerDrawOther:
		return "other"
	case RedFlowerDrawUnparsable:
		return "unparsable"
	case RedFlowerDrawKoulingExpired:
		return "kouling_expired"
	}
	return ""
}

// RedFlowerDrawOutcome 是一次 `draw` 的结论。
type RedFlowerDrawOutcome struct {
	Kind   RedFlowerDrawKind
	Result RedFlowerDrawResult
	// Code 是原始业务码（nil = 没有这个字段）。
	Code *int64
	// Message 是上游原文，**只进日志**。
	Message string
}

// IsSuccess 报告这一次是否真的抽到了。
func (o RedFlowerDrawOutcome) IsSuccess() bool { return o.Kind == RedFlowerDrawSucceeded }

// RejectedKouling 报告**这一个口令是否应当被判为无效**（决定要不要重听一次）。
//
// 只有实拍确认的那一个码算 —— 见 [RedFlowerDrawOther] 的说明。
func (o RedFlowerDrawOutcome) RejectedKouling() bool {
	return o.Kind == RedFlowerDrawInvalidKouling
}

// RedFlowerDrawRequest 构造 `POST /ycloud/liveRoomFeedback/redFlower/draw?code=<urlencoded>`。
//
// # 口令走 **query string**，不是 body
//
// 实拍（§5.1 第 5 条）：`POST …/redFlower/draw?code=%E6%B5%8B%E8%AF%95`，body **空**
// （`content-length: 0`、无 content-type）。原项目也是 `undefined` 作 body。
//
// ⚠ 口令是**中文**，必须百分号编码：不编码的话 `&` / `=` / `+` 会把它截断成
// 另一个口令，而服务端只会回一个 `200108` —— 看起来像"口令抄错了"。
func RedFlowerDrawRequest(code, jwt string) core.HTTPRequest {
	return WithLiveRoomHeaders(
		core.NewRequestPost(fmt.Sprintf("%s%s?code=%s",
			BaseLiveRoomFeedback, LiveRoomDrawPath, PercentEncode(code))), &jwt)
}

// ClassifyRedFlowerDraw 判定 `draw` 的结果。
//
// **不返回错误**：六种结果里只有两种是"故障"（[RedFlowerDrawOther] /
// [RedFlowerDrawUnparsable]），而它们该不该中断整批由调用方决定 ——
// 逐个口令重试是正常路径，不该让一个口令的异常把其余口令一起带走。
func ClassifyRedFlowerDraw(envelope *Envelope) RedFlowerDrawOutcome {
	out := RedFlowerDrawOutcome{}
	code, hasCode := envelope.Code()
	if hasCode {
		out.Code = &code
	}
	if msg, ok := envelope.Message(); ok {
		out.Message = msg
	}
	if !hasCode {
		out.Kind = RedFlowerDrawUnparsable
		return out
	}
	switch code {
	case CodeOK:
		payload, _ := envelope.Payload()
		if payload == nil {
			// `code 0` 但**没有 result**：原项目的判据是 `code===0 && result`
			// —— 两个都成立才算成功。只有 code 而没 result，说明拿不到
			// "换到几朵"，那就不能记成一次成功（否则余额对不上却没有任何线索）。
			out.Kind = RedFlowerDrawUnparsable
			return out
		}
		out.Kind = RedFlowerDrawSucceeded
		out.Result = RedFlowerDrawResult{
			PrizeID:   intAt(payload, "prizeId"),
			FlowerNum: intAt(payload, "flowerNum"),
			PrizeName: strAt(payload, "prizeName"),
			PrizeType: intAt(payload, "prizeType"),
		}
	case CodeRedFlowerAlreadyDrawn:
		out.Kind = RedFlowerDrawAlreadyDrawn
	case CodeRedFlowerInvalidKouling:
		out.Kind = RedFlowerDrawInvalidKouling
	case CodeRedFlowerKoulingExpired:
		out.Kind = RedFlowerDrawKoulingExpired
	default:
		out.Kind = RedFlowerDrawOther
	}
	return out
}

// ─────────────────────────────────────────────────────────────────────────────
// ⑤ 复活花
// ─────────────────────────────────────────────────────────────────────────────

// QueryRevivalFlowersRequest 构造 `GET /ycloud/liveRoomFeedback/redFlower/queryRevivalFlowers`。
//
// ⚠ 它只回**复活花**的数量（实拍 `{"code":0,"msg":"success","result":0}`），
// **不是**小红花余额 —— 别拿它当余额（见文件头）。
func QueryRevivalFlowersRequest(jwt string) core.HTTPRequest {
	return WithLiveRoomHeaders(
		core.NewRequestGet(fmt.Sprintf("%s/liveRoomFeedback/redFlower/queryRevivalFlowers",
			BaseLiveRoomFeedback)), &jwt)
}

// ParseRevivalFlowers 解析 `queryRevivalFlowers`。
//
// `result` 是**裸数字**（不是对象），所以这里不能走 [RequireInt64]（它要 map）。
func ParseRevivalFlowers(envelope *Envelope) (int64, *core.Error) {
	payload, err := EnsureOKWithHint(envelope)
	if err != nil {
		return 0, err
	}
	value, ok := asInt64(payload)
	if !ok {
		return 0, core.NewParse("queryRevivalFlowers 的 result 不是整数")
	}
	return value, nil
}
