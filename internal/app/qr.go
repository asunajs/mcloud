// 二维码渲染：一份实现，两个出口（终端字符画 / SVG 文件与 HTTP 响应）。
//
// # 为什么从 CLI 提到这里
//
// 出码这件事 CLI 与 WebUI 都要：前者打进终端或写文件，后者要把矩阵变成
// 一个 `<img>` 能引用的地址。两处各写一遍"留白几格、每模块几像素"，
// 漂移的表现是**手机扫不出来** —— 而扫不出来时人先怀疑手机、码、上游，
// 最后才会想到"两个出口画的不是同一张图"。
package app

import (
	"fmt"
	"strings"

	"github.com/skip2/go-qrcode"
)

// qrQuietModules 是二维码四周留白的模块数（规范要求的 4）。
//
// # 补记（2026-09-22）：实际画出来的是**两圈**
//
// [qrcode.Bitmap] 返回的位图**自带**那 4 格留白（go-qrcode 的 `symbol.size`
// 已经是 `symbolSize + 2*quietZoneSize`），下面又按这个常数加了一圈，
// 所以深色区离画布边缘是 **8** 格。规范只规定下限 4，多的一圈不违规，
// 代价是码在画布里略小一点。
//
// ⚠ 想"修平"之前先看 `qr_test.go` 的 `TestQRSVGKeepsTheQuietZone`：
// 它钉的是"起点在第 8 格、深色区不许越过第 8 格"。真会扫不出来的是两处都删。
const qrQuietModules = 4

// qrMinSidePixels 是 SVG 的最小边长。
//
// 太小手机对焦困难，太大一屏放不下；取 360 与旧的 Rust 实现同口径
// （那边是 `min_dimensions(360, 360)`）。
const qrMinSidePixels = 360

// QRText 把载荷渲染成终端可打印的字符画。
//
// 用半高块字符：同样的信息量只占一半行数，否则一屏放不下一个足够大的二维码
// （太小手机扫不出来）。
//
// ⚠ **它只在等宽字体 + 正确行距下成立**，所以要同时有 [QRSVG]：
// 字符画变形的概率远高于"用户以为的偶然"。
//
// # 补记（2026-09-22，写 qr_test.go 时实测）：**极性与终端配色绑定**
//
// go-qrcode 的 `ToSmallString(false)` 把**亮**模块画成 █、**黑**模块留成空格，
// 所以这张字符画只在"深色终端底 + 浅色前景"下才是可扫的；浅色终端上它是反的。
// 冻结基准那边是 `qrcode` crate 的 `render::<unicode::Dense1x2>()`
// （`mcloud-cli/src/support.rs`），极性由该 crate 的调色板决定，不走这个 bool ——
// 也就是说这一处**与基准不完全同形**，是换库带来的，不是有意改的行为。
//
// 没有顺手把它翻过来，是因为翻过来只是把"哪一类终端扫不出"换了一批，
// 而真正的解法是像基准那样带上 ANSI 底色。那条改动会动 CLI 的输出面，
// 留作决定（见 `qr_test.go` 的 `TestQRTextIsInvertedForDarkTerminals`：
// 极性变了会红在那里，不会悄悄漂）。
func QRText(payload string) (string, error) {
	code, err := qrcode.New(payload, qrcode.Medium)
	if err != nil {
		return "", fmt.Errorf("二维码内容过长或无法编码: %w", err)
	}
	return code.ToSmallString(false), nil
}

// QRSVG 把载荷写成一张 SVG 图片的字节。
//
// 用 SVG 而不是 PNG：浏览器与 macOS 预览都能直接打开，扫描体验与位图没差别，
// 而且不需要把图像编码库拖进来（这里按矩阵直接写 rect/path）。
//
// 底色**显式画白**：不画底色的 SVG 在深色界面下会是"黑码 + 深色底"，
// 那是手机一定扫不出来的组合。
func QRSVG(payload string) ([]byte, error) {
	code, err := qrcode.New(payload, qrcode.Medium)
	if err != nil {
		return nil, fmt.Errorf("二维码内容过长或无法编码: %w", err)
	}
	bitmap := code.Bitmap()
	if len(bitmap) == 0 || len(bitmap[0]) == 0 {
		return nil, fmt.Errorf("二维码矩阵是空的")
	}

	modules := len(bitmap) + qrQuietModules*2
	scale := 8
	if modules*scale < qrMinSidePixels {
		scale = (qrMinSidePixels + modules - 1) / modules
	}
	side := modules * scale

	var svg strings.Builder
	fmt.Fprintf(&svg,
		"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"%d\" height=\"%d\" "+
			"viewBox=\"0 0 %d %d\" shape-rendering=\"crispEdges\">\n",
		side, side, side, side)
	svg.WriteString("<rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/>\n")
	svg.WriteString("<path fill=\"#000000\" d=\"")
	for y, row := range bitmap {
		for x, dark := range row {
			if !dark {
				continue
			}
			fmt.Fprintf(&svg, "M%d %dh%dv%dh-%dz",
				(x+qrQuietModules)*scale, (y+qrQuietModules)*scale, scale, scale, scale)
		}
	}
	svg.WriteString("\"/>\n</svg>\n")
	return []byte(svg.String()), nil
}
