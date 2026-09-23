// 滑块题面解算 —— 从 `getSlide` 的两张题面图里算出 `puzzleOffset`。
//
// # 它算的是什么
//
// `puzzleOffset` = **拼图块左边缘在底图（680×400 像素坐标）里的 x**，
// 可滑范围 `[0, 680-96] = [0, 584]`。
//
// 它是 `exchangeV3` 的**必填**参数：空串与 `null` 都回
// `{"code":501,"msg":"滑块验证参数不能为空"}`，而非空但错的值回 `514`。
// 而 `getSlide` 的 `puzzleLeft` 字段**恒为 `null`**（4 条样本完全一致）
// ⇒ 这个值**只能本地算**，服务端不给。
//
// # 为什么是两个算法，而不是一个
//
// 单个"取极值"的判据在特定输入上**会退化，而且退化的表现不是报错、
// 是给一个看起来很像答案的错答案**。本仓库实测栽过：最初那版用"比值
// `底图/拼图` 的变异系数最小"，在平滑背景（天空）上全域极差只有 `0.00003`
// —— 等于没测，却照样打印出一个"最优 x=0"。
//
// ⇒ 所以这里跑两个**互相独立**的度量，要求它们一致才采用，并且各自
// 都能打出"最优与次优差多少"这类自检量（见 [SlideSolution]）。
// 两个口径在唯一那张可复算的证据图上分别给 `320` / `321`（差 1px），
// 而原项目那份 `match.ts` 给 `321` —— **三个独立口径一致到 1 像素**。
//
// # 上游口径是照抄的，不是"顺手修好"的
//
// 两处看着像 bug 的口径（调色板索引当灰度、整数灰度的 `>>2` 与 `min(255,…)`
// 截断）都必须逐字保留，否则复现不出原版得分。理由写在各自的注释上，
// 证据在 docs/capture/app-exchange-samples.txt §5.3。
package core

import (
	"bytes"
	"fmt"
	"image"
	_ "image/png" // 注册 PNG 解码器；题面图实拍是 png（`picType: "png"`）
	"sort"
)

// ─────────────────────────────────────────────────────────────────────────────
// 题面图
// ─────────────────────────────────────────────────────────────────────────────

// SlideImage 是一张已解码的题面图（底图或拼图）。
//
// 三个平面都是 0..255、长度 `Width*Height`、按行优先。
//
// ⚠⚠ **为什么同时留浮点与整数两套灰度**：两个解算口径用的是**不同**的灰度，
// 而这不是"实现细节"，是**必须照抄的上游口径** —— 详见 [SlideImage.Gray8]。
type SlideImage struct {
	// Width / Height 是像素尺寸（底图实拍 680×400，拼图 96×400）。
	Width, Height int

	// Gray 是浮点亮度 `0.299R + 0.587G + 0.114B`。
	//
	// ⚠ 取值来自 `Color.RGBA()`，那是**已预乘 alpha** 的 16 位值（除以 257）。
	// 拼图的透明区域因此亮度为 0 —— 但两个解算口径都只在 `alpha > 128`
	// 的像素上取样，所以不影响结果。
	Gray []float64

	// Gray8 是**整数灰度** `(r*77 + g*150 + b*29) >> 8` —— 原项目 `toGrayFast`
	// 的逐字口径。
	//
	// ⚠⚠ 底图是 **8-bit colormap PNG**，而这里对它的口径与"展开成 RGB"**不同**：
	// 调色板图下 [DecodeSlideImage] 会把本字段**整片覆盖成调色板索引**，
	// 对应原项目 `base64ToImageData` 的 `channels === 1` 分支 ——
	// 它**直接把索引当灰度**，于是它的 Sobel 是在**索引图**上做的。
	//
	// 这看着像原版的一个 bug（索引不携带亮度），但**照抄它**才复现得出原版的
	// `41699 / 33011 / …` 与答案 `321`；不照抄的后果实测过：得分
	// `27400 / 20988 / …`、答案 `319` —— 值只差 1~2px，但**对拍就失效了**
	// （对不上任何已记录的数字）。⇒ 谁想"顺手修好"这一条，先读
	// docs/capture/app-exchange-samples.txt §5.3。
	Gray8 []uint8

	// Alpha 是 0..255 的透明度。拼图块用 `alpha > 128` 判"不透明"。
	Alpha []float64
}

// GrayAt 取浮点亮度。**越界返回 0**（不 panic）。
//
// 解算里到处是 `x±1` 的邻域取样，边界上给一个中性值比到处写判断更不容易写错。
func (m *SlideImage) GrayAt(x, y int) float64 {
	if x < 0 || y < 0 || x >= m.Width || y >= m.Height {
		return 0
	}
	return m.Gray[y*m.Width+x]
}

// Gray8At 取整数灰度（调色板图下是**调色板索引**）。越界返回 0。
func (m *SlideImage) Gray8At(x, y int) int {
	if x < 0 || y < 0 || x >= m.Width || y >= m.Height {
		return 0
	}
	return int(m.Gray8[y*m.Width+x])
}

// AlphaAt 取透明度。越界返回 0（= 透明）。
func (m *SlideImage) AlphaAt(x, y int) float64 {
	if x < 0 || y < 0 || x >= m.Width || y >= m.Height {
		return 0
	}
	return m.Alpha[y*m.Width+x]
}

// DecodeSlideImage 解一张 PNG 题面图。
//
// 两条口径差异都在这里落地：
//
//   - 浮点灰度按 `RGBA()`（预乘）算；
//   - 整数灰度按原版的 `(r*77 + g*150 + b*29) >> 8` 算，**调色板图另走
//     "索引即灰度"**（见 [SlideImage.Gray8]）。
func DecodeSlideImage(png []byte) (*SlideImage, error) {
	src, _, err := image.Decode(bytes.NewReader(png))
	if err != nil {
		return nil, fmt.Errorf("解 PNG 失败: %w", err)
	}
	bounds := src.Bounds()
	out := &SlideImage{Width: bounds.Dx(), Height: bounds.Dy()}
	if out.Width <= 0 || out.Height <= 0 {
		return nil, fmt.Errorf("题面图尺寸非法: %d×%d", out.Width, out.Height)
	}
	out.Gray = make([]float64, out.Width*out.Height)
	out.Gray8 = make([]uint8, out.Width*out.Height)
	out.Alpha = make([]float64, out.Width*out.Height)

	for y := 0; y < out.Height; y++ {
		for x := 0; x < out.Width; x++ {
			r, g, b, a := src.At(bounds.Min.X+x, bounds.Min.Y+y).RGBA()
			i := y*out.Width + x
			// `RGBA()` 给的是 0..65535（且已预乘 alpha）⇒ 除以 257 落到 0..255。
			out.Gray[i] = (0.299*float64(r) + 0.587*float64(g) + 0.114*float64(b)) / 257.0
			r8, g8, b8 := int(r>>8), int(g>>8), int(b>>8)
			out.Gray8[i] = uint8((r8*77 + g8*150 + b8*29) >> 8)
			out.Alpha[i] = float64(a) / 257.0
		}
	}

	// ⚠ 调色板图：整数灰度**换成索引**（见 [SlideImage.Gray8] 那段）。
	// 用 `PixOffset` 而不是自己算 `y*Stride` —— 后者只在 `Bounds().Min == {0,0}`
	// 时才对，而 `image.Decode` 并不保证这一点。
	if paletted, ok := src.(*image.Paletted); ok {
		for y := 0; y < out.Height; y++ {
			for x := 0; x < out.Width; x++ {
				out.Gray8[y*out.Width+x] = paletted.Pix[paletted.PixOffset(bounds.Min.X+x, bounds.Min.Y+y)]
			}
		}
	}
	return out, nil
}

// ─────────────────────────────────────────────────────────────────────────────
// 解算
// ─────────────────────────────────────────────────────────────────────────────

// SlideBox 是拼图块在拼图图里的包围盒（**含**四边）。
type SlideBox struct {
	MinX, MinY, MaxX, MaxY int
}

// Width / Height 是包围盒的像素宽高（含边界，所以 `MaxX-MinX+1`）。
func (b SlideBox) Width() int  { return b.MaxX - b.MinX + 1 }
func (b SlideBox) Height() int { return b.MaxY - b.MinY + 1 }

// Empty 报告"一个不透明像素都没有"。
func (b SlideBox) Empty() bool { return b.MaxX < b.MinX || b.MaxY < b.MinY }

// SlideCandidate 是一个候选落点与它的度量。
//
// ⚠ **`Metric` 一律"越小越好"** —— 这样两个口径能共用同一个排序。
// [SolveSlideContour] 的原始语义是"得分越大越好"，它内部取负后再放进来。
type SlideCandidate struct {
	Offset int
	Metric float64
}

// SlideBlockBox 用 `alpha > 128` 求拼图块的包围盒。
//
// 返回 `Empty()` 为真的盒子表示整张拼图都透明 —— 那是"题面没取到"，
// 不是"块在 0 位置"。
func SlideBlockBox(pz *SlideImage) SlideBox {
	box := SlideBox{MinX: pz.Width, MinY: pz.Height, MaxX: -1, MaxY: -1}
	for y := 0; y < pz.Height; y++ {
		for x := 0; x < pz.Width; x++ {
			if pz.AlphaAt(x, y) <= 128 {
				continue
			}
			if x < box.MinX {
				box.MinX = x
			}
			if x > box.MaxX {
				box.MaxX = x
			}
			if y < box.MinY {
				box.MinY = y
			}
			if y > box.MaxY {
				box.MaxY = y
			}
		}
	}
	return box
}

// SolveSlideAffine 是口径①：对每个候选 offset 拟合 `底图 ≈ a·拼图 + b`，
// 取 `1-R²` **最小**者。返回按 metric 升序的候选与**退化跳过的候选数**。
//
// 位置对时"缺口 = 原图压暗"，即 `底图 = k·拼图` 精确成立 ⇒ `1-R²` 接近 0。
//
// ⚠ 刻意**不**用"比值 `底图/拼图` 的变异系数"那种写法：它在**平滑背景**上会退化
// （包注释记的那次）。回归版本对"两边都平坦"的候选返回**无效**
// （`varP` / `varB` 太小），于是它会被**跳过**而不是给出一个假的最小值 ——
// 这正是与那个坑的差别所在。⇒ 调用方**必须**把 `skipped` 打出来：
// 它等于"这张图上这个口径有多少候选是没法判的"，是判据可信度的一部分。
//
// ⚠ `box` 必须是**拼图块**的包围盒（[SlideBlockBox]），不是整张拼图的盒子。
func SolveSlideAffine(bg, pz *SlideImage, box SlideBox) ([]SlideCandidate, int) {
	if box.Empty() || bg.Width <= pz.Width {
		return nil, 0
	}
	candidates := make([]SlideCandidate, 0, bg.Width)
	skipped := 0
	for offset := 0; offset <= bg.Width-pz.Width; offset++ {
		var n, sumP, sumB, sumPP, sumBB, sumPB float64
		for y := box.MinY; y <= box.MaxY; y++ {
			for x := box.MinX; x <= box.MaxX; x++ {
				if pz.AlphaAt(x, y) <= 128 {
					continue
				}
				gx := offset + x
				if gx < 0 || gx >= bg.Width {
					continue
				}
				p := pz.GrayAt(x, y)
				b := bg.GrayAt(gx, y)
				n++
				sumP += p
				sumB += b
				sumPP += p * p
				sumBB += b * b
				sumPB += p * b
			}
		}
		// 样本太少 ⇒ 拟合没有意义（实测块约 95×96，正常远大于这个数）。
		if n < 100 {
			continue
		}
		meanP, meanB := sumP/n, sumB/n
		varP := sumPP/n - meanP*meanP
		varB := sumBB/n - meanB*meanB
		// 两边任一边平坦 ⇒ R² 没有意义。⚠ 这正是"退化"该被识别成**无效**的地方：
		// 不是给一个假答案，而是把候选从集合里摘掉。
		if varP < 1.0 || varB < 1.0 {
			skipped++
			continue
		}
		cov := sumPB/n - meanP*meanB
		candidates = append(candidates, SlideCandidate{offset, 1 - cov*cov/(varP*varB)})
	}
	sort.Slice(candidates, func(i, j int) bool {
		if candidates[i].Metric != candidates[j].Metric {
			return candidates[i].Metric < candidates[j].Metric
		}
		return candidates[i].Offset < candidates[j].Offset
	})
	return candidates, skipped
}

// SlideSobelMagnitude 是底图的边缘强度图 —— **逐字照抄原项目**：
//
//	灰度 `(r*77 + g*150 + b*29) >> 8` → 整数 Sobel → `min(255, (|gx|+|gy|) >> 2)`
//
// ⚠ 那个 `>> 2` 与 `min(255, …)` **不能省**：省了得分会大 2.6 倍、名次也会变
// （实测过 `109843 / 84272 / …` vs 原版的 `41699 / 33011 / …`）。
func SlideSobelMagnitude(bg *SlideImage) []float64 {
	out := make([]float64, bg.Width*bg.Height)
	for y := 1; y < bg.Height-1; y++ {
		for x := 1; x < bg.Width-1; x++ {
			tl, tc, tr := bg.Gray8At(x-1, y-1), bg.Gray8At(x, y-1), bg.Gray8At(x+1, y-1)
			ml, mr := bg.Gray8At(x-1, y), bg.Gray8At(x+1, y)
			bl, bc, br := bg.Gray8At(x-1, y+1), bg.Gray8At(x, y+1), bg.Gray8At(x+1, y+1)
			gx := -tl + tr - 2*ml + 2*mr - bl + br
			gy := -tl - 2*tc - tr + bl + 2*bc + br
			value := (absInt(gx) + absInt(gy)) >> 2
			if value > 255 {
				value = 255
			}
			out[y*bg.Width+x] = float64(value)
		}
	}
	return out
}

// SolveSlideContour 是口径②：原项目 `packages/shared/src/slide/match.ts` 的
// `matchSlide` **逐字移植**。
//
//	① 底图做 Sobel 边缘检测（`|gx| + |gy|`）→ 边缘强度图；
//	② 拼图里取**轮廓像素**（`alpha > 128` 且四邻域存在透明像素的点）；
//	③ 每个候选 offset 把轮廓点平移过去、累加底图边缘强度，取**和最大**者。
//
// 返回按 metric 升序的候选（**内部取了负**，所以仍是"越小越好"）与轮廓点数。
//
// ⚠ 循环边界**照抄原版** `for (y=1; y<pzH-1; y++) for (x=1; x<pzW-1; x++)`
// —— 它结构上看不到拼图最左一列（`x=0`）。文档 §5.3 做过**判决性实验**
// （给拼图左边补一列透明）：轮廓点 `310 → 404`（正好是 `x=0` 那列的不透明像素数），
// 而落点只随坐标平移、**拼图落点纹丝不动**，且 321 那个点的得分反而**涨**了。
// ⇒ 这**不是**"约定差"，所以这里不"顺手修好"，否则就对不上原版了。
//
// ⚠ 原版只判上界（`if (bx < bgW)`），**不**跳左边界列 —— 同样照抄。
//
// ⚠ 前置条件：拼图与底图**等高**（拼图是通高竖条、块保留原 y）。
// [SolveSlide] 会先校验；直接调用本函数时自己保证。
func SolveSlideContour(bg, pz *SlideImage) ([]SlideCandidate, int) {
	if bg.Width <= pz.Width || bg.Height != pz.Height {
		return nil, 0
	}
	edges := SlideSobelMagnitude(bg)
	type point struct{ x, y int }
	points := make([]point, 0, 2048)
	for y := 1; y < pz.Height-1; y++ {
		for x := 1; x < pz.Width-1; x++ {
			if pz.AlphaAt(x, y) <= 128 {
				continue
			}
			if pz.AlphaAt(x-1, y) <= 128 || pz.AlphaAt(x+1, y) <= 128 ||
				pz.AlphaAt(x, y-1) <= 128 || pz.AlphaAt(x, y+1) <= 128 {
				points = append(points, point{x, y})
			}
		}
	}
	scores := make([]SlideCandidate, 0, bg.Width)
	for offset := 0; offset <= bg.Width-pz.Width; offset++ {
		var sum float64
		for _, pt := range points {
			gx := offset + pt.x
			// 原版只判上界。这里多判一次下界只是为了不越界读 ——
			// `offset >= 0` 且 `pt.x >= 1` 时它恒真，对结果没有影响。
			if gx >= 0 && gx < bg.Width {
				sum += edges[pt.y*bg.Width+gx]
			}
		}
		// 取**和最大**者 ⇒ 用负分排序，好与 [SolveSlideAffine] 共用 [SlideCandidate]。
		scores = append(scores, SlideCandidate{offset, -sum})
	}
	sort.Slice(scores, func(i, j int) bool {
		if scores[i].Metric != scores[j].Metric {
			return scores[i].Metric < scores[j].Metric
		}
		return scores[i].Offset < scores[j].Offset
	})
	return scores, len(points)
}

// SlideMaxDeviation 是两个口径允许的最大分歧（像素）。
//
// 为什么是 3：证据图上两个口径差 1px，而服务端的容忍窗口实测约 **±4~9px**
// （`0` 偏差也通过、`+10` 被拒）。⇒ 分歧到 3px 时结果仍在窗口内；
// 再大就说明"这张题面至少有一个口径没算对"，那时**宁可报错也不要发** ——
// 滑块接口**计失败次数**，答错一次就消耗一次机会。
const SlideMaxDeviation = 3

// SlideSolution 是一次解算的完整结果 —— 两个口径各自的最优、自检量，以及共识值。
//
// ⚠ **`Top` 不是"顺便打印一下"**：`Best` 与次优差多少，是这个判据**可信度**的唯一
// 现场证据。`1-R²` 那种度量在退化输入上不会报错、只会给一个很像答案的错答案
// （包注释那次），而"最优与次优几乎一样"正是它退化的外观。
type SlideSolution struct {
	// Offset 是共识值：两个口径一致时取均值。
	Offset int

	// Deviation 是两个口径最优落点的差（`|①-②|`）。
	Deviation int

	// Box 是拼图块的包围盒。
	Box SlideBox

	// AffineOffset / AffineBest 是口径①的最优与它的 `1-R²`；
	// AffineTop 是前几名（已排序），AffineSkipped 是**退化跳过**的候选数。
	AffineOffset  int
	AffineBest    float64
	AffineTop     []SlideCandidate
	AffineSkipped int

	// ContourOffset / ContourScore 是口径②的最优与它的**原始得分**（越大越好）；
	// ContourPoints 是轮廓点数，ContourTop 是前几名。
	ContourOffset int
	ContourScore  float64
	ContourPoints int
	ContourTop    []SlideCandidate
}

// SlideTopN 是 [SlideSolution] 里保留的候选个数（够看清"最优与次优差多少"）。
const SlideTopN = 5

// SlideSolutionFromImages 对两张已解码的题面图跑两个口径并求共识。
func SlideSolutionFromImages(bg, pz *SlideImage) (SlideSolution, error) {
	var out SlideSolution
	if bg.Height != pz.Height {
		return out, fmt.Errorf("拼图与底图不等高（%d vs %d）—— 拼图是通高竖条，块保留原 y",
			pz.Height, bg.Height)
	}
	if bg.Width <= pz.Width {
		return out, fmt.Errorf("底图不比拼图宽（%d vs %d）—— 没有可滑范围", bg.Width, pz.Width)
	}
	box := SlideBlockBox(pz)
	if box.Empty() {
		return out, fmt.Errorf("拼图里没有不透明像素（alpha 全 ≤128）")
	}
	out.Box = box

	affine, skipped := SolveSlideAffine(bg, pz, box)
	if len(affine) == 0 {
		return out, fmt.Errorf("口径①：没有一个有效候选（全部退化 —— 题面可能过于平坦）")
	}
	out.AffineOffset = affine[0].Offset
	out.AffineBest = affine[0].Metric
	out.AffineSkipped = skipped
	out.AffineTop = topN(affine)

	contour, points := SolveSlideContour(bg, pz)
	if len(contour) == 0 {
		return out, fmt.Errorf("口径②：一个候选都没有（轮廓点或尺寸不合法）")
	}
	out.ContourOffset = contour[0].Offset
	out.ContourScore = -contour[0].Metric
	out.ContourPoints = points
	out.ContourTop = topN(contour)

	out.Deviation = absInt(out.AffineOffset - out.ContourOffset)
	if out.Deviation > SlideMaxDeviation {
		return out, fmt.Errorf("⚠ 两个口径分歧 %dpx（① %d vs ② %d，上限 %d）⇒ 不要提交，先查题面",
			out.Deviation, out.AffineOffset, out.ContourOffset, SlideMaxDeviation)
	}
	out.Offset = (out.AffineOffset + out.ContourOffset) / 2
	return out, nil
}

// SolveSlide 是入口：吃两张 PNG 原文，给出 [SlideSolution]。
//
// ⚠ 它**不**做任何网络请求，也**不**替调用方决定"要不要加抖动"。
// 实测 `0` 偏差直接通过、`+10` 被拒 ⇒ **不要主动加抖**（曾经的建议正好相反，
// 见 docs/capture/app-exchange-samples.txt §5.3.1）。
func SolveSlide(picturePNG, puzzlePNG []byte) (SlideSolution, error) {
	var out SlideSolution
	bg, err := DecodeSlideImage(picturePNG)
	if err != nil {
		return out, fmt.Errorf("底图：%w", err)
	}
	pz, err := DecodeSlideImage(puzzlePNG)
	if err != nil {
		return out, fmt.Errorf("拼图：%w", err)
	}
	return SlideSolutionFromImages(bg, pz)
}

// topN 取前 n 名（候选已排序，拷贝一份免得调用方改到原切片）。
func topN(candidates []SlideCandidate) []SlideCandidate {
	n := len(candidates)
	if n > SlideTopN {
		n = SlideTopN
	}
	out := make([]SlideCandidate, n)
	copy(out, candidates[:n])
	return out
}

// absInt 是整数绝对值。`math.Abs` 走浮点，这里到处是像素坐标，用整数更直白。
func absInt(v int) int {
	if v < 0 {
		return -v
	}
	return v
}
