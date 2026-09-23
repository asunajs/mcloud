package app

// 二维码渲染的两个出口（终端字符画 / SVG）。
//
// # 为什么这一组值得单独测
//
// qr.go 的注释把风险说得很直白：两个出口各写一遍，漂移的表现是**手机扫不出来**，
// 而扫不出来时人先怀疑手机、码、上游，最后才想到"两边画的不是同一张图"。
// 那种故障不报错，所以判据必须建立在"两边**必须给出同一张矩阵**"上，
// 而不是各自"看着像二维码"。
//
// # 用真上游载荷，不用 `"hello"`
//
// 载荷的形状决定矩阵的大小与容错后的长度；换成 hello 会把这条通路
// 最脆的那一段（长 URL + `-ENDIN` 里的连字符）测没了。

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/skip2/go-qrcode"
)

// qrPayloadFixture 是一条真实形态的扫码登录载荷。
//
// 与 `upstream.QRLoginURL` 拼出来的那条同形（`internal/upstream/login_test.go`
// 钉的是**串本身**，这里钉的是**这张串能不能被两个出口画成同一张码**）。
func qrPayloadFixture() string {
	const deviceID = "67F4C56A1B2C3D4E5F60718293A4B5C6-ENDIN"
	return fmt.Sprintf("https://yun.139.com/w/#/qrcLogin?dID=%s&sID=%s&cType=11&ver=%s",
		deviceID, "3f2b7c9e-1d4a-5b6c-7d8e-9f0a1b2c3d4e", "8.8.6.20260829")
}

// parseQRSVG 把一张 SVG 拆成"边长 + 每个深色模块的落点"。
//
// ⚠ 用正则而不是 XML 解析器：这里要断言的是**几何**（坐标、静区、模块尺寸），
// 而引入一个 SVG 库只会把"我们写的 rect 长什么样"这件事再包一层。
// 三条模式与 qr.go 里那三段字面量一一对应 —— 改了那边的排版，这里会红，
// 那是**想要的**红：排版漂了，前端 `<img>` 与手机对焦都可能受影响。
var (
	svgSize     = regexp.MustCompile(`width="(\d+)" height="(\d+)" viewBox="0 0 (\d+) (\d+)"`)
	svgWhiteBox = regexp.MustCompile(`<rect width="(\d+|\d+%)" height="(\d+|\d+%)" fill="#ffffff"/>`)
	// 一个深色模块 = `M x y h s v s h -s z`。RE2 不支持反向引用，
	// 所以三个 s 各自捕获再比相等（"这个模块是正方形的"也是断言之一）。
	svgModule = regexp.MustCompile(`M(\d+) (\d+)h(\d+)v(\d+)h-(\d+)z`)
)

func parseQRSVG(t *testing.T, svg []byte) (side int, modules [][3]int) {
	t.Helper()
	dims := svgSize.FindStringSubmatch(string(svg))
	if dims == nil {
		t.Fatalf("SVG 头没有可解析的 width/height/viewBox：%s", briefSVG(svg))
	}
	first, err := strconv.Atoi(dims[1])
	if err != nil {
		t.Fatalf("width 不是数字：%q", dims[1])
	}
	for i, raw := range dims[2:] {
		n, err := strconv.Atoi(raw)
		if err != nil || n != first {
			t.Fatalf("第 %d 个尺寸 %q 与 width %d 不一致（正方形与 viewBox 必须同边）",
				i+2, raw, first)
		}
	}
	side = first

	// path 必须是**闭合**的：`d="…"` 里的一切都是深色模块。
	start := strings.Index(string(svg), `<path fill="#000000" d="`)
	if start < 0 {
		t.Fatalf("没有黑底 path：%s", briefSVG(svg))
	}
	body := string(svg)[start:]
	if !strings.HasSuffix(strings.TrimSpace(body), "/>\n</svg>") {
		t.Errorf("SVG 结尾不合预期：%q", briefSVG(svg[len(svg)-40:]))
	}
	for _, m := range svgModule.FindAllStringSubmatch(body, -1) {
		x, _ := strconv.Atoi(m[1])
		y, _ := strconv.Atoi(m[2])
		s, _ := strconv.Atoi(m[3])
		if m[4] != m[3] || m[5] != m[3] {
			t.Fatalf("模块 (%d,%d) 的三段尺寸不等长（%s %s %s）—— 不是正方形模块", x, y, m[3], m[4], m[5])
		}
		modules = append(modules, [3]int{x, y, s})
	}
	if len(modules) == 0 {
		t.Fatal("一个深色模块都没解析出来 —— 上面那条正则与实现漂了，整组断言会因此全绿而什么都不测")
	}
	return side, modules
}

func briefSVG(svg []byte) string {
	s := strings.ReplaceAll(string(svg), "\n", " ")
	if len(s) > 220 {
		s = s[:220] + "…(截断)"
	}
	return s
}

// TestQRSVGIsSquareEnoughToFocus 钉住"最小边长"与"整张码是正方形"。
//
// 360 这个数抄自旧的 Rust 实现（`min_dimensions(360, 360)`）：手机对焦
// 需要一个物理尺寸下限，太小是**扫不出来**而不是"扫得慢"。
//
// # 两条载荷，因为实现有两条路
//
// 每模块 8px 是**默认**，只有撑不到 360 才把模块抬高。真实登录载荷（49 模块
// 的位图 + 留白 = 65 格）走默认那条：`65 × 8 = 520`，下限分支**根本不参与**。
// 只测它就等于把"抬高"那半条路留着没测 —— 所以第二条故意用一条极短载荷把它逼出来。
//
// ⚠ 那条 `"139"` **不是**生产载荷（登录 URL 再短也有 34 字符，落在默认那支）。
// 它在这里只为了走另一支，别把它读成"扫码载荷可以很短"。
//
// ⚠ 期望的画布宽度是**自己算**的（位图 + 两侧静区），不是从实现抄的常数：
// 拿 `qrQuietModules` 当期望值只测了"它等于它自己"，而这里要测的是
// "SVG 的几何真的按它排出来的"。
func TestQRSVGIsSquareEnoughToFocus(t *testing.T) {
	cases := []struct {
		name    string
		payload string
		// wantScale 是 0 表示"必须比 8 大，且取满足下限的最小值"。
		wantScale int
	}{
		{name: "真实登录载荷（默认 8px/模块）", payload: qrPayloadFixture(), wantScale: 8},
		{name: "极短载荷（非生产形状，只为逼出抬高那支）", payload: "139", wantScale: 0},
	}
	for _, tc := range cases {
		svg, err := QRSVG(tc.payload)
		if err != nil {
			t.Fatalf("%s：载荷应当编得进去：%v", tc.name, err)
		}
		if !strings.Contains(string(svg), `shape-rendering="crispEdges"`) {
			t.Errorf("%s：缺 crispEdges，浏览器缩放时会做插值，边缘发虚的码手机会拒", tc.name)
		}
		side, modules := parseQRSVG(t, svg)
		scale := modules[0][2]
		wide := qrCanvasModules(t, tc.payload)

		if side < qrBaselineMinPixels {
			t.Errorf("%s：边长 %d 小于下限 %d —— 手机会扫不出来",
				tc.name, side, qrBaselineMinPixels)
		}
		if side%scale != 0 {
			t.Errorf("%s：边长 %d 不是模块尺寸 %d 的整数倍 —— 会有一圈被裁掉的半模块", tc.name, side, scale)
		}
		if got := side / scale; got != wide {
			t.Errorf("%s：SVG 是 %d×%d 模块，画布应是 %d —— 尺寸或留白漂了", tc.name, got, got, wide)
		}

		if tc.wantScale > 0 {
			if scale != tc.wantScale {
				t.Errorf("%s：每模块应为 %d 像素，实得 %d", tc.name, tc.wantScale, scale)
			}
			continue
		}
		// 抬高支：scale 必须是"能让边长过下限"的最小整数。
		// 只断 `> 8` 的话，把系数改成 20 也照样绿 —— 那是把码画得糊大一圈。
		if want := (qrBaselineMinPixels + wide - 1) / wide; scale != want {
			t.Errorf("%s：%d 格画布的下限系数应是 %d，实得 %d", tc.name, wide, want, scale)
		}
	}
}

// qrSpecQuietModules 是规范要求的留白模块数。
//
// ⚠ **写成字面量**而不是引用 [qrQuietModules]：那条断言的作用正是
// "实现把留白改小了要变红"，拿实现的常数当期望值等于什么都没测。
const qrSpecQuietModules = 4

// qrBaselineMinPixels 抄自冻结基准
// `../mcloud-rust/crates/mcloud-cli/src/support.rs` 的 `min_dimensions(360, 360)`。
// 同理不引用 [qrMinSidePixels]。
const qrBaselineMinPixels = 360

// qrCanvasModules 独立算出"这张载荷画在 SVG 画布上该有多宽"。
//
// # 两份静区，不是一份
//
// go-qrcode 的 `Bitmap()` 返回的位图**自带**规范要求的 4 模块留白（它的
// `symbol.size` 已经是 `symbolSize + 2*quietZoneSize`），而 qr.go 在其上又加了
// 一圈 [qrQuietModules]。所以画布是 `位图宽 + 8`，深色区从第 **8** 格开始。
//
// 多出来的那一圈不是缺陷（规范只规定**下限** 4），代价是码在画布里略小。
// 这里把它钉成事实，是因为下一个读代码的人会先算成 4 再"顺手修平" ——
// 那样深色区起点会从第 8 格挪到第 4 格（仍合规），而这条会红，
// 逼他先读到这段话再决定。真会扫不出来的是**两处都删掉**：深色区贴边。
func qrCanvasModules(t *testing.T, payload string) int {
	t.Helper()
	code, err := qrcode.New(payload, qrcode.Medium)
	if err != nil {
		t.Fatalf("基准编码失败：%v", err)
	}
	return len(code.Bitmap()) + qrSpecQuietModules*2
}

// TestQRSVGKeepsTheQuietZone 钉住四周留白。
//
// # 为什么只断言起点与边界，不测"留白总共几格"
//
// 二维码的左上角定位符（finder pattern）在模块 (0,0) 处**必定是深色**，
// 所以"最近的深色点离边多远"有一个可证的**下界**（这里是位图自带的 4 格
// 加 qr.go 又画的 4 格，见 [qrCanvasModules]）。
// 反过来，最后一行/最后一列没有任何一个模块保证是深色（校正图在中间，
// 时间模式在第 6 行），所以只能断"不越过静区边界"，不能断"正好贴到边界"。
//
// ⇒ 把 `qrQuietModules` 改成 0、或者让深色区越过边界，这条都会红。
func TestQRSVGKeepsTheQuietZone(t *testing.T) {
	svg, err := QRSVG(qrPayloadFixture())
	if err != nil {
		t.Fatalf("%v", err)
	}
	side, modules := parseQRSVG(t, svg)

	scale := modules[0][2]
	for _, m := range modules {
		if m[2] != scale {
			t.Fatalf("一张码里有两种模块尺寸（%d 与 %d）—— 网格会错位，手机扫不出来", scale, m[2])
		}
	}

	minX, minY, maxX, maxY := side, side, 0, 0
	for _, m := range modules {
		if m[0] < minX {
			minX = m[0]
		}
		if m[1] < minY {
			minY = m[1]
		}
		if m[0] > maxX {
			maxX = m[0]
		}
		if m[1] > maxY {
			maxY = m[1]
		}
	}
	quiet := 2 * qrSpecQuietModules * scale
	if minX != quiet || minY != quiet {
		t.Errorf("深色区应从静区后第一格开始（%d），实得 x=%d y=%d", quiet, minX, minY)
	}
	if limit := side - quiet; maxX+scale > limit || maxY+scale > limit {
		t.Errorf("深色区越过了右侧/下侧静区（边界 %d，实得 x=%d y=%d）", limit, maxX, maxY)
	}
}

// TestQRSVGPutsTheWhiteBackgroundFirst 钉住"显式画白底"。
//
// 深色界面下不画底色 = 黑码配深色底 = **一定**扫不出来。所以顺序也要断：
// 白 rect 必须在黑 path **之前**，否则 SVG 的绘制次序会把码盖住。
func TestQRSVGPutsTheWhiteBackgroundFirst(t *testing.T) {
	svg, err := QRSVG(qrPayloadFixture())
	if err != nil {
		t.Fatalf("%v", err)
	}
	text := string(svg)
	box := svgWhiteBox.FindStringIndex(text)
	if box == nil {
		t.Fatalf("没有那张白底 rect：%s", briefSVG(svg))
	}
	black := strings.Index(text, `<path fill="#000000"`)
	if black < 0 {
		t.Fatalf("没有黑底 path：%s", briefSVG(svg))
	}
	if box[0] > black {
		t.Errorf("白底画在黑码**之后**（%d > %d），会把整张码盖掉", box[0], black)
	}
	if strings.Contains(text, "prefers-color-scheme") {
		t.Error("不该按主题切色：那正是「深色底下没有白」的那个故障")
	}
}

// artRows 把字符画拆成"每行一个 rune 切片"，并确认只用了那四个字形。
//
// `ToSmallString` 一个字符竖着表示**两个**模块（上/下各一格）：
// █ 两格同色、▄ 只有下格、▀ 只有上格、空格两格都白。
// 冒出别的字形就说明换了编码，下面所有按格计数的断言都会失去意义。
func artRows(t *testing.T, art string) [][]rune {
	t.Helper()
	lines := strings.Split(strings.TrimRight(art, "\n"), "\n")
	rows := make([][]rune, len(lines))
	for i, line := range lines {
		rows[i] = []rune(line)
		for _, r := range rows[i] {
			if r != '█' && r != '▄' && r != '▀' && r != ' ' {
				t.Fatalf("字符画里出现预期外的字形 %q", r)
			}
		}
	}
	return rows
}

// cellsFromText 数出字符画里被**画成块**的那些模块格。
//
// ⚠ 不叫"深色"是有原因的：go-qrcode 的 `ToSmallString(false)` 走的是反色口径
// （见 [TestQRTextIsInvertedForDarkTerminals]），块 = 亮模块。
// 这个名字只陈述"哪些格被画了东西"，把极性交给上面那条去钉。
func cellsFromText(t *testing.T, art string) int {
	t.Helper()
	blocks := 0
	for _, line := range artRows(t, art) {
		for _, r := range line {
			switch r {
			case '█':
				blocks += 2
			case '▄', '▀':
				blocks++
			}
		}
	}
	return blocks
}

// TestQRTextIsInvertedForDarkTerminals 钉住字符画的**极性**。
//
// # 这不是洁癖，是一次实测发现
//
// go-qrcode 的 `ToString/ToSmallString` 写的是
// `if bits != inverseColor { 空白 } else { 块 }` —— 传 `false` 时，
// **亮**模块被画成 █，**黑**码是那些空格。也就是说这张字符画只在
// "深色终端底 + 浅色字"下才是可扫的二维码；浅色终端上它是反的。
//
// ⚠ 与冻结基准**有意不同**：那边是 `qrcode` crate 的
// `render::<unicode::Dense1x2>()`（`mcloud-cli/src/support.rs`），
// 极性由该 crate 自己的调色板决定，不走这个 bool。
// 换终端配色等于换"能不能扫出来"，所以这条必须显式红，不能藏在别处。
//
// 两个取证点都是手算出来的（位图自带 4 格静区，所以符号的原点在第 4 格）：
//   - 字符画第 0 行是**整行块** = 静区（亮）被画成块；
//   - 定位符左上角 (4,4) 与它下面那格 (4,5) 都是深色 → 那里是**空格**。
func TestQRTextIsInvertedForDarkTerminals(t *testing.T) {
	art, err := QRText(qrPayloadFixture())
	if err != nil {
		t.Fatalf("%v", err)
	}
	rows := artRows(t, art)

	for _, r := range rows[0] {
		if r != '█' {
			t.Fatalf("静区那行没有画满块（实得 %q）—— 极性变了，见这个用例的说明", string(rows[0]))
		}
	}
	// 行是两格一字符：字符画第 2 行对应位图第 4、5 行，第 4 个字符对应 x=4。
	if got := string(rows[2][4]); got != " " {
		t.Errorf("位图 (4,4)-(4,5) 两格都是深色（定位符左上角），字符画却画成 %q —— 极性变了", got)
	}
}

// TestTheTwoRendersCoverTheSameCells 是这一组的**主断言**。
//
// 两个出口各自"看起来是二维码"是不够的 —— CLI 给终端、WebUI 给 `<img>`，
// 同一台手机上扫哪一张都该进同一个会话。这里把两边**合起来**钉：
//
//	SVG 画出的深色模块数 + 字符画画成块的格数 == 位图总格数
//
// 容错等级、载荷、矩阵尺寸任何一个只在一边改了，这个和就不成立。
//
// ⚠ 为什么写成"块数 + 深色数 == 总格数"而不是"块数 == 深色数"：
// 前者只要求两边编的是**同一张矩阵**，把极性留给
// [TestQRTextIsInvertedForDarkTerminals] 单独管。那样真漂了会红在**说漂的那条**上，
// 而不是两条一起红、让人去查错方向。
//
// ⚠ 为什么不需要反解二维码：仓库里没有解码器（`go-qrcode` 只编码不解码）。
// 这条挡得住 qr.go 注释点名的那类漂移（两个出口各写一遍），
// **挡不住**"两边同时被改成同一张错的码" —— 那种只能实机扫码（见 docs/status.md）。
func TestTheTwoRendersCoverTheSameCells(t *testing.T) {
	short := "https://yun.139.com/w/#/qrcLogin?dID=D&sID=S&cType=11&ver=1"
	for _, payload := range []string{qrPayloadFixture(), short} {
		art, err := QRText(payload)
		if err != nil {
			t.Fatalf("QRText(%d 字节) 失败：%v", len(payload), err)
		}
		svg, err := QRSVG(payload)
		if err != nil {
			t.Fatalf("QRSVG(%d 字节) 失败：%v", len(payload), err)
		}
		code, err := qrcode.New(payload, qrcode.Medium)
		if err != nil {
			t.Fatalf("基准编码失败：%v", err)
		}
		side := len(code.Bitmap())

		_, modules := parseQRSVG(t, svg)
		blocks, dark := cellsFromText(t, art), len(modules)
		if blocks+dark != side*side {
			t.Errorf("载荷 %d 字节：字符画 %d 格 + SVG %d 个深色模块 ≠ 位图 %d×%d —— "+
				"两个出口画的不是同一张矩阵（容错等级或载荷漂了一边）",
				len(payload), blocks, dark, side, side)
		}
	}
}

// TestQRTextIsHalfHeight 钉住"半高块"这个设计决定本身。
//
// 注释里的理由是"一屏放不下一个足够大的二维码"。`ToSmallString` 的口径是
// **一行两个模块高、一字符一个模块宽**：行数 ≈ 矩阵边长 / 2，行宽 = 矩阵边长。
// 换成整块字符（`ToString`）行数与列数都会翻倍，终端里那张码会滚出屏幕，
// 而用户看到的是"花屏"，不是"码太大"。
//
// ⚠ 字符画**不含**静区（`Bitmap()` 只有矩阵），所以这里比的是矩阵边长，
// 不是 [qrGridWidth]。
func TestQRTextIsHalfHeight(t *testing.T) {
	payload := qrPayloadFixture()
	art, err := QRText(payload)
	if err != nil {
		t.Fatalf("%v", err)
	}
	lines := strings.Split(strings.TrimRight(art, "\n"), "\n")
	if len(lines) == 0 || lines[0] == "" {
		t.Fatalf("字符画是空的：%q", briefSVG([]byte(art)))
	}
	code, err := qrcode.New(payload, qrcode.Medium)
	if err != nil {
		t.Fatalf("基准编码失败：%v", err)
	}
	side := len(code.Bitmap()) // 矩阵边长必为奇数（21/25/29…）
	if got, want := len(lines), (side+1)/2; got != want {
		t.Errorf("行数 %d，半高块应是 %d（矩阵 %d 行）", got, want, side)
	}
	for i, line := range lines {
		if got := utf8.RuneCountInString(line); got != side {
			t.Fatalf("第 %d 行宽 %d 字符，应是 %d（一字符一模块宽）", i, got, side)
		}
	}
}

// TestBothRendersFailTogether 钉住两个出口的**容错等级与错误口径**一致。
//
// 一条编不进去的载荷，如果一个出口报错、另一个照常出码，
// 那 WebUI 会显示一张**被截断**的码而 CLI 报"内容过长" —— 两者都不解释对方。
func TestBothRendersFailTogether(t *testing.T) {
	oversized := qrPayloadFixture() + "?blob=" + strings.Repeat("x", 3000)
	textErr := func() error { _, err := QRText(oversized); return err }()
	svgErr := func() error { _, err := QRSVG(oversized); return err }()

	if (textErr == nil) != (svgErr == nil) {
		t.Fatalf("两个出口对同一条载荷的判断不一致：QRText=%v QRSVG=%v", textErr, svgErr)
	}
	if textErr == nil {
		t.Fatal("3 KB 的载荷应编不进二维码（版本容量上限之内的最长码也没这么长）")
	}
	// ⚠ 报错**不许回显载荷**：那条 URL 里带着 dID 与 sID，
	// 而错误串会被写进日志与 HTTP 响应体。
	for _, err := range []error{textErr, svgErr} {
		if strings.Contains(err.Error(), "sID=") || strings.Contains(err.Error(), strings.Repeat("x", 40)) {
			t.Errorf("错误串把载荷带出来了：%v", err)
		}
	}
}
