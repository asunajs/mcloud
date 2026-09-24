package upstream

// 滑块取题（`getSlide`）的**协议层**用例。
//
// 这一层只钉三件事，三件的共同点是"错了不响，只是变成另一条请求 / 一张残缺题面"：
//
//  1. **Content-Type 只有一个，且是 form-urlencoded** —— 探针把它发成 JSON 也照样回
//     `code 0`（§5.3），所以"能过"证明不了"与真机同形"。而 `WithH5Headers` 会先按
//     JSON 补一个，忘了剥就是**同一个请求带两个 content-type**。
//  2. **body 是 `{}` 而不是空** —— 实拍 `content-length: 2`。
//  3. **尺寸是字符串、题面是带填充的标准 base64** —— 前者按数字读会静默拿到 nil；
//     后者若换成无填充的 `RawStdEncoding`，尾部字节的缺失**不会报错**，
//     只会让解算器在一张残缺的图上给出一个很像答案的错答案（而那一枪计失败次数）。
//
// ⚠ 夹具里的两段"图"是合成字节（够测解码，不代表 PNG 内容）；
// PNG 解码与解算的对拍锁在 `internal/core/slide_test.go`（那里有真机题面）。

import (
	"encoding/base64"
	"strings"
	"testing"

	"github.com/catlair/mcloud/internal/core"
)

// slideBody 拼一份 §5.3 形状的响应（`picture` / `puzzle` 给标准 base64）。
func slideBody(picture, puzzle string) string {
	return `{"code":0,"msg":"success","result":{` +
		`"picWidth":"680","picHeight":"400","picType":"png","puzzleWidth":"96",` +
		`"picture":"` + picture + `","puzzle":"` + puzzle + `","puzzleLeft":null}}`
}

// TestGetSlideRequestMatchesCapture ⭐⭐ 钉住"这一枪与真机同形"。
//
// 最容易被"顺手改好"的两处：
//   - 复用 [H5ContentType]（那是 JSON 那一族的）⇒ 形态偏离实拍，而**上游不校验、照样通**；
//   - 只 `WithHeader` 不先 `dropHeader` ⇒ 两个 content-type，取哪个由 HTTP 库决定。
func TestGetSlideRequestMatchesCapture(t *testing.T) {
	request := GetSlideRequest("JWT-1", "https://referer.example/signin")
	if request.Method != core.MethodPost {
		t.Errorf("应是 POST，实得 %s", request.Method)
	}
	if want := "https://m.mcloud.139.com/ycloud/auth-service/slide/getSlide"; request.URL != want {
		t.Errorf("这条走 /ycloud/auth-service/（不是 signin/page 那一张口），URL 实得 %q", request.URL)
	}
	if string(request.Body) != "{}" {
		t.Errorf("body 应逐字是 {}（实拍 content-length: 2），实得 %q", request.Body)
	}
	if n := countHeader(request.Headers, "content-type"); n != 1 {
		t.Fatalf("content-type 应**恰好一个**，实得 %d 个（WithHeader 是追加，重复不会报错）", n)
	}
	if got := headerValue(request.Headers, "content-type"); got != SlideFormContentType {
		t.Errorf("应是实拍的 form-urlencoded，实得 %q", got)
	}
	if got := headerValue(request.Headers, "isdeviceid"); got != "" {
		t.Errorf("实拍这条没有 isdeviceid（那是下单那一枪的），实得 %q", got)
	}
	if got := headerValue(request.Headers, "jwttoken"); got != "JWT-1" {
		t.Errorf("jwttoken 应带上那张票，实得 %q", got)
	}
	if got := headerValue(request.Headers, "activityid"); got != MarketName {
		t.Errorf("activityid 应是 %s，实得 %q", MarketName, got)
	}
}

// TestParseSlidePuzzleReadsStringDimsAndPaddedBase64 ⭐ 实拍形态的题面要完整解出来。
//
// 尺寸那三个在实拍里是**字符串**（`"680"`）—— 用 `.(json.Number)` 断言会静默拿到 nil，
// 于是领域层那份"尺寸自检"永远看到"上游没给"。
func TestParseSlidePuzzleReadsStringDimsAndPaddedBase64(t *testing.T) {
	picture := base64.StdEncoding.EncodeToString([]byte(strings.Repeat("\x89PNG", 7)))
	puzzle := base64.StdEncoding.EncodeToString([]byte("\x89PNG\x0d\x0a"))
	parsed, perr := ParseSlidePuzzle(exchangeEnvelope(t, slideBody(picture, puzzle)))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	for _, c := range []struct {
		name string
		got  *int64
		want int64
	}{
		{"picWidth", parsed.PicWidth, 680},
		{"picHeight", parsed.PicHeight, 400},
		{"puzzleWidth", parsed.PuzzleWidth, 96},
	} {
		if c.got == nil || *c.got != c.want {
			t.Errorf("%s 应从**字符串**形态解出 %d，实得 %v", c.name, c.want, c.got)
		}
	}
	if parsed.PicType == nil || *parsed.PicType != "png" {
		t.Errorf("picType 应是 png，实得 %v", parsed.PicType)
	}
	if string(parsed.PicturePNG) != strings.Repeat("\x89PNG", 7) {
		t.Errorf("底图应是解码后的**原文**（不是 base64 串），实得 %d 字节", len(parsed.PicturePNG))
	}
	if string(parsed.PuzzlePNG) != "\x89PNG\x0d\x0a" {
		t.Errorf("拼图解码不符，实得 %d 字节", len(parsed.PuzzlePNG))
	}
}

// TestParseSlidePuzzleRejectsUnpaddedBase64 ⭐ 只认标准（带填充）base64。
//
// ⚠ 这一条钉的是"不给自己留 RawStdEncoding 兜底"：无填充串与截断串在解码器眼里
// 是同一类输入，兜底会把"上游换了编码"变成"我们默默少解了尾部几个字节"——
// 而少解的正是拼图竖条的**右半边**，解算器照样会给出一个数（只是答案错），
// 那一枪的代价是**计一次滑块失败**。
func TestParseSlidePuzzleRejectsUnpaddedBase64(t *testing.T) {
	padded := base64.StdEncoding.EncodeToString([]byte("0123456789"))
	unpadded := strings.TrimRight(padded, "=")
	for _, body := range []string{
		slideBody(unpadded, padded),
		slideBody(padded, unpadded),
		slideBody("not base64 !!!", padded),
		slideBody(``, padded),
	} {
		if _, perr := ParseSlidePuzzle(exchangeEnvelope(t, body)); perr == nil {
			t.Error("题面不全或编码不符时应报错，绝不能给一张残缺的图往下走")
		}
	}
}

// TestParseSlidePuzzleIgnoresPuzzleLeft ⭐ `puzzleLeft` **恒 null**，本层刻意不读它。
//
// 这里给它一个非 null 值，解析结果**不应**因此变化（结构体里根本没有这一格）——
// 那才是"不指望服务端给答案"的可执行表达：将来上游真给了值，也得先抓包确认语义，
// 而不是这里已经留了一个字段、有人顺手开始用它。
func TestParseSlidePuzzleIgnoresPuzzleLeft(t *testing.T) {
	picture := base64.StdEncoding.EncodeToString([]byte("A"))
	puzzle := base64.StdEncoding.EncodeToString([]byte("B"))
	body := strings.Replace(slideBody(picture, puzzle), `"puzzleLeft":null`, `"puzzleLeft":123`, 1)
	parsed, perr := ParseSlidePuzzle(exchangeEnvelope(t, body))
	if perr != nil {
		t.Fatalf("解析失败: %v", perr)
	}
	if len(parsed.PicturePNG) != 1 || len(parsed.PuzzlePNG) != 1 {
		t.Errorf("两段题面仍应各自解出 1 字节，实得 %d / %d",
			len(parsed.PicturePNG), len(parsed.PuzzlePNG))
	}
}

// TestParseSlidePuzzleNonZeroCodeIsError：取题失败**没有可分流的档** ——
// 拿不到图就是解不了，只能不发下一枪（与 `exchangeV3` 那套"码是数据"刻意不同）。
//
// ⚠ 这一条的非 0 码**零样本**，所以只钉"回错误"这个动作，不解释任何文案。
func TestParseSlidePuzzleNonZeroCodeIsError(t *testing.T) {
	if _, perr := ParseSlidePuzzle(
		exchangeEnvelope(t, `{"code":500,"msg":"x"}`)); perr == nil {
		t.Fatal("非 0 码应回错误")
	}
	// result 不是对象（例如直接回 true）⇒ 同样报错，不"当成没有题面"继续往下。
	if _, perr := ParseSlidePuzzle(
		exchangeEnvelope(t, `{"code":0,"msg":"success","result":true}`)); perr == nil {
		t.Fatal("result 不是对象应报错")
	}
}
