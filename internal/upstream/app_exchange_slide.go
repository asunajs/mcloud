// 滑块取题（`getSlide`）—— 兑换链上的**第一枪**，也是这一族唯一"不扣豆"的写侧请求。
//
// # 为什么单独一份文件（不并进 `app_exchange.go`）
//
// 它虽然只在兑换链上出现，但**三件事都与那一族不同**：
// 主机路径是 `/ycloud/auth-service/slide/` 而不是 `/ycloud/signin/page/`、
// `Content-Type` 是 form-urlencoded 而不是 JSON、响应载荷是**两张图**而不是业务数据。
// 混在一个文件里，最容易犯的错就是"顺手复用那一族的请求构造" ——
// 而那一复用就把这条链变成"另一个客户端"（下面 [GetSlideRequest] 逐条对着实拍）。
//
// # 这一条**只发题、不判卷**
//
// 答案由客户端算（[core.SolveSlide]，纯本地）。实拍 `result.puzzleLeft` **恒为 `null`**
// —— 字段存在，但服务端不给答案（§5.3 的字段表，4 条样本一致）。
// 所以本文件解析时**刻意不去读它**：读出一个永远为 nil 的指针，
// 下一个读者会以为"有值的时候可以用"，而那一天没有样本支撑。
//
// ⚠ 反向的纪律在 [ParseSlidePuzzle]：非 0 码在这里**是**错误，不做"码是数据"那一套 ——
// 那是 `exchangeV3` 的特例（它的码决定"能不能再来一次"）。取题失败没有可分流的档：
// 拿不到图就是解不了，只能不发下一枪。
//
// 证据：`docs/capture/app-exchange-samples.txt` §5.3（请求 4 条逐字相同 + 响应记录 13065 逐字）。
package upstream

import (
	"context"
	"encoding/base64"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// SlideFormContentType 是 `getSlide` 实拍的 `content-type`（§5.3 请求头逐字第二行）。
//
// ⚠⚠ 它**不等于**这一族别的请求用的 [H5ContentType]（`application/json;charset=UTF-8`）——
// 差别不是理论上的：探针（通路①）发这条时把它序列化成了 JSON，**也照样回 `code 0`**，
// 于是"我们这条请求的头与真机不一致"这件事在探针那一轮完全没露头。
// 文档为此专门写了一句"别拿探针的请求头当实拍"（§5.3）。
// 所以这里按**实拍**那一行构造，而不是按"能过的那一行"：
// 上游今天不校验的头，哪天开始校验时错的那一方是**静默失效**，不会报错。
const SlideFormContentType = "application/x-www-form-urlencoded;charset=UTF-8"

// GetSlideRequest 构造 `POST /ycloud/auth-service/slide/getSlide`。
//
// 实拍（§5.3，4 条**逐字相同**，`content-length: 2`）：
//
//	POST /ycloud/auth-service/slide/getSlide
//	content-type: application/x-www-form-urlencoded;charset=UTF-8
//	Accept: */*
//	activityid: sign_in_3
//	appversion: 12.6.0.0
//	Referer: <签到页地址（含本次 SSO token）>
//	showloading: true
//	jwttoken: <905 字符 JWT>
//
//	body: {}
//
// # 为什么 body 要手写 `{}` 而不是"空 body"
//
// `Content-Length: 2` 是实拍的一部分，而 H5 源码 `getSlidePuzzle({})` 传的就是空对象。
// 空 body 的 POST 在 HTTP 层合法，但那是**另一种**客户端形态。
//
// # `Referer` 是签到页那一条（不是兑换页）
//
// 实拍里这条请求紧跟在签到页的会话之后发出，Referer 与 `exchangeList` 同一个值。
// 调用方传进来的应当是同一个 [SignInSession] 的 referer —— 见 `internal/app` 那一层。
func GetSlideRequest(jwt, referer string) core.HTTPRequest {
	request := core.NewRequestPost(BaseMcloud + "/ycloud/auth-service/slide/getSlide")
	request.Body = core.Body([]byte("{}"))
	request = WithH5Headers(request, referer, &jwt, "true")
	// `WithH5Headers` 看见非空 body 会补 [H5ContentType]（那是给 JSON 那一族定的），
	// 而这一条实拍是 form-urlencoded ⇒ 剥掉再按实拍补回来。
	//
	// ⚠ 必须走包内的 [dropHeader] 而不是直接 `WithHeader`：后者是**追加**，
	// 结果是一个请求带两个 `content-type`，而 HTTP 客户端按各自规则合并 ——
	// 上游取哪一个**未测试**，而我们连"发出去了两个"都不会知道。
	request.Headers = dropHeader(request.Headers, "content-type")
	return request.WithHeader("Content-Type", SlideFormContentType)
}

// SlidePuzzle 是一次取题的**题面**：两张 PNG 原文，外加三个尺寸读数。
//
// # 尺寸为什么留着（解算器并不需要它们）
//
// [core.SolveSlide] 吃 PNG 原文，尺寸从 PNG 自己的 IHDR 里读。这三个数**只**用来
// 交叉自检：上游说底图 680 而图是别的尺寸时，说明题面与读数不同源，
// 那时"可滑范围 `[0, picWidth-puzzleWidth]`"这个前提就不成立了。
// 比对放在领域层（那里决定发不发），见 `internal/app/exchange_run.go`。
//
// ⚠ 三个尺寸在实拍里是**字符串**（`"680"` / `"400"` / `"96"`），不是数字 ——
// 与 `probability2` 那一族同一种异形（坑表 #105：按"都是数字"读会**静默归零**）。
// [asInt64] 两种形态都认，所以这里不做特殊处理，但**别**改成 `.(json.Number)`。
//
// # 为什么两张图可以是 `[]byte`（`encoding/json` 会把它编成 base64）
//
// 本仓库的 json tag 白名单怕的是"字段名 / 线格式被默默定成契约"。这两段是**二进制原文**，
// 而 Go 与 `serde` 对二进制的默认线格式**同为标准 base64**，编出来与上游那两段字符串
// 逐字相同 ⇒ 不存在"两个实现对不上"那一类坑。本文件唯一可能踩坑的是 [SlidePuzzle.PicType]
// （标量 `*string`），而它跟上游同名同形，且这一整个结构体**不进 JSON**
// （已登记在 `scripts/check-go-jsontags.cjs`，理由与兑换族那五条同一条）。
type SlidePuzzle struct {
	// PicWidth 是 `picWidth`（实拍 `"680"`）。上游没给则为 nil（**不**默认 680）。
	PicWidth *int64
	// PicHeight 是 `picHeight`（实拍 `"400"`）。
	PicHeight *int64
	// PuzzleWidth 是 `puzzleWidth`（实拍 `"96"`）。
	//
	// ⚠ 它比拼图块**不透明区的实际宽度大 1**：实拍块的包围盒宽 95（`i∈[0,94]`），
	// 而这一格报 96（§5.3 字段表）。所以拿它当"块宽"量边界会偏 1px ——
	// 解算器用的是**像素**包围盒（[core.SlideBlockBox]）而不是这个数。
	PuzzleWidth *int64
	// PicType 是 `picType`（实拍 `"png"`）。
	//
	// ⚠ 只有 `png` 一档样本。它若是别的值（`jpg`…），下面解出来的"PNG 原文"
	// 就解不开，那时**报错**而不是猜一种格式。
	PicType *string
	// PicturePNG 是 `picture` 解码后的**底图原文**（680×400、8-bit colormap）。
	PicturePNG []byte
	// PuzzlePNG 是 `puzzle` 解码后的**拼图原文**（96×400 RGBA，通高竖条）。
	PuzzlePNG []byte
}

// ParseSlidePuzzle 解析 `getSlide` 的响应。
//
// # 非 0 码在这里**就是**错误
//
// 走 [Envelope.EnsureOK]：`code != 0` 时拿不到题面，也就没有"该重试 / 不该重试"
// 可分流（那是 `exchangeV3` 的事，见 `ParseExchangeResult`）。
// ⚠ 这一条的**非 0 码零样本** —— 文案与语义都没实拍，所以不自造判据，
// 只把上游那句 `msg` 留在错误串里交回。
//
// # base64 的形态（今天新核到的一条事实）
//
// 两段都是**带填充的标准 base64**（`StdEncoding`）：实拍那条形如
// `iVBORw0K…gg==` / `iVBORw0K…YII=`，长度 280920 / 12856（都是 4 的倍数）。
// 2026-09-23 从 Reqable 的响应缓存里把记录 12992 那条原文取出来核过：
// 解码得 210688 / 9641 字节，且与 `target/slide-evidence/picture.png`、
// `puzzle.png` **逐字节相同**。
// ⇒ 所以这里**只**认 `StdEncoding`：无填充的 `RawStdEncoding` 零样本，
// 给它加兜底等于把"哪天上游换了编码"变成"我们静默少解了尾部几十字节"。
// 解不开就报错，错误里带上是哪一段 —— 因为表现必须是"这一枪不发"，
// 而不是"拿着一张残缺的图算出一个很像答案的错答案"（那是滑块**计失败次数**的那一坑）。
func ParseSlidePuzzle(envelope *Envelope) (SlidePuzzle, *core.Error) {
	payload, perr := envelope.EnsureOK()
	if perr != nil {
		return SlidePuzzle{}, perr
	}
	obj, ok := payload.(map[string]any)
	if !ok {
		return SlidePuzzle{}, core.NewParse("getSlide 的 result 不是对象")
	}
	picture, perr := decodeSlideImage(obj, "picture")
	if perr != nil {
		return SlidePuzzle{}, perr
	}
	puzzle, perr := decodeSlideImage(obj, "puzzle")
	if perr != nil {
		return SlidePuzzle{}, perr
	}
	return SlidePuzzle{
		PicWidth:    optInt64Field(obj, "picWidth"),
		PicHeight:   optInt64Field(obj, "picHeight"),
		PuzzleWidth: optInt64Field(obj, "puzzleWidth"),
		PicType:     optStrField(obj, "picType"),
		PicturePNG:  picture,
		PuzzlePNG:   puzzle,
	}, nil
}

// decodeSlideImage 取 `result` 里某一段 base64 并解码。
//
// 缺字段与"有字段但不是串"合并成同一句错误：两种情况下调用方要做的事**完全一样**
// （不发下一枪），而区分它们只在排障时有用 —— 那时错误串里的键名已经带上了。
func decodeSlideImage(obj map[string]any, key string) ([]byte, *core.Error) {
	text, ok := obj[key].(string)
	if !ok || text == "" {
		return nil, core.NewParse("getSlide 的 result 里缺 " + key + "（题面不全，解不了）")
	}
	decoded, err := base64.StdEncoding.DecodeString(text)
	if err != nil {
		return nil, core.NewParse(fmt.Sprintf("getSlide 的 %s 不是标准 base64：%v", key, err))
	}
	return decoded, nil
}

// AppGetSlide 取一次滑块题面（`getSlide`）。**只读**：它不提交答案，因而不计失败次数。
//
// ⚠ 但**别把它当免费的**：每次兑换都应当"取一次题 → 解 → 提交一次"。
// 重取不算失败（§1.4 那句"`getSlide` 只管发题，取一次不构成失败"），
// 可失败计数落在下一枪 `exchangeV3` 上，而那一枪才是有代价的。
func (u *Upstream) AppGetSlide(ctx context.Context, jwt, referer string) (SlidePuzzle, *core.Error) {
	envelope, perr := u.Call(ctx, GetSlideRequest(jwt, referer))
	if perr != nil {
		return SlidePuzzle{}, perr
	}
	return ParseSlidePuzzle(envelope)
}
