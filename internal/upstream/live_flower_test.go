package upstream

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
	"github.com/catlair/mcloud/internal/golden"
)

// 小红花（`live_flower.go`）的黄金向量测试。
//
// # ⚠ 这些值**全是抓包实拍**，不是算出来的
//
// 与 `live_room_vectors_test.go` 那份**相反**：那边的值来自"原项目实现跑一遍"
// （因为 WS 那一段在未开播时结构性拿不到），而这一族的 `/ycloud/liveRoomFeedback/*`
// 是**普通 HTTP**，开播时点一下就能抓到 ⇒ 基准取**实拍**，不取原项目。
//
// 尤其 `200112`：它在代码注释里挂了很久的"零实拍、语义靠猜"，
// 2026-09-23 才第一次真的抓到（见 `docs/capture/app-live-redflower-samples.txt` §10.2）。
// 这条链上"我以为是 A、实际是 B"已经发生过两次（坑表 #131 / #132），
// 所以这里不写"和某个常量相等"这种自证式断言，而是拿**上游回的原包**去喂分类器。
//
// # 三条纪律
//
//  1. 请求侧比的是**逐字**（URL、头、body 空、无 content-type）——
//     这一族的三个请求 body 形态各不相同，是实拍钉住的（文件头那段）。
//  2. 响应侧喂的是**完整信封**，不是"构造一个 code" —— 后者测的是测试自己。
//  3. ⭐ `200112` 的语义**是明确的：按账号去重** —— **两个不同账号投同一条口令，
//     各自都会成功**（**用户口述**，2026-09-22 两次纠正 / 2026-09-23 定稿）
//     ⇒ ⚠ **不存在"口令被消耗"**。本文件仍**不**为它写"跨账号"类断言
//     （那要两个账号的实拍，本仓还没有），只钉"它归到已兑换那一类、且**不**被当成无效口令"。
//     ⚠⚠ 早先这里写的是"语义**未判定**，不做任何相关断言" —— **那句是错的**
//     （坑表 **#142**：把"本仓无实拍"当成了"这个说法没依据"）。
type liveRedflowerVectors struct {
	Source string `json:"$source"`
	Draw   struct {
		Code          string            `json:"code"`
		ExpectQuery   string            `json:"expectQuery"`
		ExpectReferer string            `json:"expectReferer"`
		ExpectHeaders map[string]string `json:"expectHeaders"`
	} `json:"draw"`
	DrawResponses []struct {
		Name                  string `json:"name"`
		Raw                   any    `json:"raw"`
		ExpectKind            string `json:"expectKind"`
		ExpectCode            int64  `json:"expectCode"`
		ExpectMessage         string `json:"expectMessage"`
		ExpectFlowerNum       int64  `json:"expectFlowerNum"`
		ExpectPrizeName       string `json:"expectPrizeName"`
		ExpectPrizeId         int64  `json:"expectPrizeId"`
		ExpectPrizeType       int64  `json:"expectPrizeType"`
		ExpectRejectedKouling bool   `json:"expectRejectedKouling"`
	} `json:"drawResponses"`
	UserInfo struct {
		Name                     string `json:"name"`
		Raw                      any    `json:"raw"`
		ExpectFlowerCnt          int64  `json:"expectFlowerCnt"`
		ExpectRedFlowerValidDate string `json:"expectRedFlowerValidDate"`
	} `json:"userInfo"`
}

func loadLiveRedflowerVectors(t *testing.T) liveRedflowerVectors {
	t.Helper()
	raw, err := golden.Load("live-redflower-vectors.json")
	if err != nil {
		t.Fatalf("读黄金向量失败: %v", err)
	}
	var out liveRedflowerVectors
	decoded, ok := core.ParseJSON(string(raw))
	if !ok {
		t.Fatal("黄金向量不是合法 JSON")
	}
	buf, err := core.MarshalJSON(decoded)
	if err != nil {
		t.Fatalf("重新序列化失败: %v", err)
	}
	if err := json.Unmarshal(buf, &out); err != nil {
		t.Fatalf("解析黄金向量失败: %v", err)
	}
	return out
}

// liveRedflowerHeader 按名取请求头（HTTP 头名大小写不敏感）。
func liveRedflowerHeader(req core.HTTPRequest, name string) (string, bool) {
	for _, pair := range req.Headers {
		if strings.EqualFold(pair[0], name) {
			return pair[1], true
		}
	}
	return "", false
}

// liveRedflowerEnvelope 把向量里的一个响应体还原成信封。
func liveRedflowerEnvelope(t *testing.T, raw any) *Envelope {
	t.Helper()
	body, err := core.MarshalJSON(raw)
	if err != nil {
		t.Fatalf("序列化向量响应体失败: %v", err)
	}
	envelope, cerr := ParseEnvelope(body)
	if cerr != nil {
		t.Fatalf("解析向量响应体失败: %s", cerr.Message())
	}
	return envelope
}

// TestLiveFlowerDrawRequestMatchesCapturedWire：`draw` 请求与实拍**逐字**一致。
//
// # 为什么要逐字比
//
// 这一族最容易"顺手改坏"的三处，全都**不会报错**：
//   - 口令不百分号编码 / 编成小写 hex ⇒ 服务端只回一个 `200108`，看起来像"口令抄错了"；
//   - 给 `draw` 补一个 `{}` body 与 `content-type` ⇒ 请求多发两个字节，通常无害，
//     但实拍是 `content-length: 0`、无 content-type，照抄才对得上；
//   - `referer` 里混进过期的 `token` ⇒ 只在日志与报错里多一处凭据。
//
// ⇒ 这三处必须由"和抓包逐字相同"来守，不能靠肉眼。
func TestLiveFlowerDrawRequestMatchesCapturedWire(t *testing.T) {
	v := loadLiveRedflowerVectors(t)
	const jwt = "<测试用 JWT，值与本测试无关>"

	req := RedFlowerDrawRequest(v.Draw.Code, jwt)

	want := BaseLiveRoomFeedback + LiveRoomDrawPath + v.Draw.ExpectQuery
	if req.URL != want {
		t.Errorf("draw 的 URL 与实拍不一致\n  实得 %q\n  应为 %q", req.URL, want)
	}
	if req.Method != core.MethodPost {
		t.Errorf("draw 应是 POST，实得 %v", req.Method)
	}
	// ⚠ 实拍 `content-length: 0`：body 必须**空**，不是一个 `{}`。
	if len(req.Body) != 0 {
		t.Errorf("draw 的 body 应为空（实拍 content-length: 0），实得 %d 字节 %q",
			len(req.Body), string(req.Body))
	}
	// ⚠ 实拍 draw **没有** content-type；`WithJSONBody` 会补一个，那就错了。
	if _, ok := liveRedflowerHeader(req, "content-type"); ok {
		t.Error("draw 不应带 content-type（实拍无此头）；带上说明走了 WithJSONBody")
	}

	for name, want := range v.Draw.ExpectHeaders {
		got, ok := liveRedflowerHeader(req, name)
		if !ok {
			t.Errorf("缺少请求头 %q", name)
			continue
		}
		if got != want {
			t.Errorf("请求头 %q 与实拍不一致\n  实得 %q\n  应为 %q", name, got, want)
		}
	}

	// `jwttoken` 是**小写**头名（实拍与 H5 拦截器都如此）。
	got, ok := liveRedflowerHeader(req, "jwttoken")
	if !ok {
		t.Fatal("缺少 jwttoken 头：这一族被上游判 90001 未登录就是因为它（见 capture §9）")
	}
	if got != jwt {
		t.Errorf("jwttoken 头应原样带上调用方给的值，实得 %q", got)
	}

	// ⭐ Referer **刻意不含** `token=`（凭据不进 URL），其余三段与实拍一致。
	if referer, ok := liveRedflowerHeader(req, "referer"); !ok {
		t.Error("缺少 referer 头")
	} else if referer != v.Draw.ExpectReferer {
		t.Errorf("referer 与实拍不一致\n  实得 %q\n  应为 %q", referer, v.Draw.ExpectReferer)
	}
}

// TestLiveFlowerKoulingEncodingMatchesCapture：口令的编码与实拍一致（大写 hex 百分号）。
//
// 单独成条：URL 那条断言会把"编码错了"和"路径拼错了"混成一句报错，
// 而这两件事的修法完全不同。
func TestLiveFlowerKoulingEncodingMatchesCapture(t *testing.T) {
	v := loadLiveRedflowerVectors(t)
	want := strings.TrimPrefix(v.Draw.ExpectQuery, "?code=")
	if got := PercentEncode(v.Draw.Code); got != want {
		t.Errorf("口令百分号编码与实拍不一致\n  实得 %q\n  应为 %q", got, want)
	}
}

// TestClassifyRedFlowerDrawOnCapturedResponses：三份实拍响应 → 三种归类。
func TestClassifyRedFlowerDrawOnCapturedResponses(t *testing.T) {
	v := loadLiveRedflowerVectors(t)
	if len(v.DrawResponses) == 0 {
		t.Fatal("向量里没有 drawResponses；测试会静默地什么都没测")
	}
	for _, item := range v.DrawResponses {
		t.Run(item.Name, func(t *testing.T) {
			out := ClassifyRedFlowerDraw(liveRedflowerEnvelope(t, item.Raw))
			if got := out.Kind.String(); got != item.ExpectKind {
				t.Errorf("归类实得 %q，应为 %q", got, item.ExpectKind)
			}
			if out.Code == nil {
				t.Fatal("应解出业务码，实得 nil")
			}
			if *out.Code != item.ExpectCode {
				t.Errorf("业务码实得 %d，应为 %d", *out.Code, item.ExpectCode)
			}
			if out.Message != item.ExpectMessage {
				t.Errorf("上游原文实得 %q，应为 %q", out.Message, item.ExpectMessage)
			}
			// ⭐ 已兑换**不能**被当成无效口令：那会把一个**有效**口令从缓存里剔掉。
			if got := out.RejectedKouling(); got != item.ExpectRejectedKouling {
				t.Errorf("RejectedKouling() 实得 %v，应为 %v", got, item.ExpectRejectedKouling)
			}
			if item.ExpectKind != "succeeded" {
				return
			}
			if out.Result.FlowerNum != item.ExpectFlowerNum {
				t.Errorf("flowerNum 实得 %d，应为 %d", out.Result.FlowerNum, item.ExpectFlowerNum)
			}
			if out.Result.PrizeName != item.ExpectPrizeName {
				t.Errorf("prizeName 实得 %q，应为 %q", out.Result.PrizeName, item.ExpectPrizeName)
			}
			if out.Result.PrizeID != item.ExpectPrizeId {
				t.Errorf("prizeId 实得 %d，应为 %d", out.Result.PrizeID, item.ExpectPrizeId)
			}
			if out.Result.PrizeType != item.ExpectPrizeType {
				t.Errorf("prizeType 实得 %d，应为 %d", out.Result.PrizeType, item.ExpectPrizeType)
			}
			if !out.IsSuccess() {
				t.Error("code 0 且有 result 时应判成功")
			}
		})
	}
}

// TestClassifyRedFlowerDrawOnExpiredKouling：`200118` 的归类，以及**不许**被当成无效口令。
//
// # ⚠⚠ 这一条手里的证据**只有业务码**，没有上游原文
//
// 实拍（`docs/capture/app-live-redflower-samples.txt` §10.5）只留下了码：
// CLI 的 `--json` 出的是 `upstreamCode`，而 [RedFlowerDrawOutcome.Message]（上游原文）
// **解析了却从不输出**（`app` 层不送 `Progress`、`--json` 也只出码）。
// ⇒ 下面那个信封是**最小信封**（只有 `code` 一个字段），**它不是抓包原文**：
// `200118` 的 `msg` 本仓**仍无实拍**，别照着上面那份 `drawResponses` 的
// "raw + expectMessage" 写法把它搬过来 —— 那会把"我没抓到原文"写成"原文是这样"。
// ⭐ 对本函数**没有损失**：非 0 的分支**只看 `code`**，最小信封足以完整判定归类
// （换成抓包原文不会多验一个字节）。这也是它能这么写的前提，不是巧合。
//
// # 为什么要单独钉它
//
// `200118` 是 2026-09-23 才第一次见到的码（「这条口令我**认识**，但它**已经失效**了」）。
// 它与同场对照里的 `200108`（[RedFlowerDrawInvalidKouling]，"口令抄错了"）**语义相反**：
// 一个说明口令**对**、只是**场次窗口关了**；一个说明口令**错**。
// ⚠ 而两条路在**人读的输出**上都会含糊成"未分类/无效"这一类词 —— 那正是要防的。
//
// ⚠⚠ 所以这里钉的**先是两条不变量**（它们与文案无关、长期成立）：
//   - [RedFlowerDrawOutcome.RejectedKouling] **必须**为 `false`：判成"无效口令"会触发
//     一次毫无意义的 60 秒重听，而这条口令**是对的**；
//   - 它**必须**不是成功（没有 `result`，不能记成"换到 0 朵"）。
//
// ⭐ **归类也已经钉住**：它落进 [RedFlowerDrawKoulingExpired]（`"kouling_expired"`）。
// ⚠ 这一条是**绊线的兑现**：2026-09-23 立这条测试时它还在 `RedFlowerDrawOther` 里，
// 当时写明"将来为它单开一类，这一条会红、**红是预期的**" —— 同日 20:2x 开类时
// 它确实红了（报 `当前应落进 "other"，实得 "kouling_expired"`），本条随之改写。
// ⇒ 它现在守的是"**别被改回去**"：既不许回 `other`（文案会退回「未分类口令」），
// 更不许变成 `invalid_kouling`（那等于把上面第一条不变量反过来）。
func TestClassifyRedFlowerDrawOnExpiredKouling(t *testing.T) {
	// 最小信封：**只有** `code`（理由见函数头"证据只有业务码"那段）。
	envelope := liveRedflowerEnvelope(t, map[string]any{"code": CodeRedFlowerKoulingExpired})

	out := ClassifyRedFlowerDraw(envelope)

	if out.Code == nil {
		t.Fatalf("应解出业务码 %d，实得 nil", CodeRedFlowerKoulingExpired)
	}
	if *out.Code != CodeRedFlowerKoulingExpired {
		t.Fatalf("业务码实得 %d，应为 %d", *out.Code, CodeRedFlowerKoulingExpired)
	}
	// ⭐ 不变量一：**不许**被当成无效口令。
	if out.RejectedKouling() {
		t.Error("`200118`（口令认识、但本场已结束）**不该**被判成无效口令 ——\n" +
			"  它说明口令本身是**对的**；判成无效会触发一次毫无意义的 60 秒重听")
	}
	// ⭐ 不变量二：**不是**成功。
	if out.IsSuccess() {
		t.Error("`200118` 不是成功：响应里没有任何 `result`")
	}
	// ⭐ 归类：已单开一类（见函数头"绊线的兑现"那段）。
	if out.Kind != RedFlowerDrawKoulingExpired {
		t.Errorf("`200118` 应落进 %q，实得 %q\n"+
			"  · 若实得 %q：那是**错的** —— 它说明口令**对**、只是本场已结束"+
			"（见上面第一条不变量）；\n"+
			"  · 若实得 %q：文案会退回「未分类口令」，而那句话把人往"+
			"「口令抄错了」那边引（坑表 #140 做法 ④）。",
			RedFlowerDrawKoulingExpired.String(), out.Kind.String(),
			RedFlowerDrawInvalidKouling.String(), RedFlowerDrawOther.String())
	}
}

// TestParseRedFlowerBalanceOnCapturedUserInfo：余额取 `flowerCnt`，不是排行榜那个。
func TestParseRedFlowerBalanceOnCapturedUserInfo(t *testing.T) {
	v := loadLiveRedflowerVectors(t)
	balance, cerr := ParseRedFlowerBalance(liveRedflowerEnvelope(t, v.UserInfo.Raw))
	if cerr != nil {
		t.Fatalf("解析余额失败: %s", cerr.Message())
	}
	if balance.FlowerCnt != v.UserInfo.ExpectFlowerCnt {
		t.Errorf("flowerCnt 实得 %d，应为 %d", balance.FlowerCnt, v.UserInfo.ExpectFlowerCnt)
	}
	if balance.RedFlowerValidDate != v.UserInfo.ExpectRedFlowerValidDate {
		t.Errorf("有效期实得 %q，应为 %q", balance.RedFlowerValidDate, v.UserInfo.ExpectRedFlowerValidDate)
	}
}

// TestRedFlowerUserInfoRequestShape：余额请求的 body 是**空对象** `{}`，不是"没有 body"。
func TestRedFlowerUserInfoRequestShape(t *testing.T) {
	req, cerr := RedFlowerUserInfoRequest("jwt")
	if cerr != nil {
		t.Fatalf("构造 user/info 请求失败: %s", cerr.Message())
	}
	if got := string(req.Body); got != "{}" {
		t.Errorf("user/info 的 body 应是 {}（实拍 content-length: 2），实得 %q", got)
	}
	if ct, ok := liveRedflowerHeader(req, "content-type"); !ok || !strings.Contains(ct, "application/json") {
		t.Errorf("user/info 应带 JSON 的 content-type，实得 %q (found=%v)", ct, ok)
	}
}
