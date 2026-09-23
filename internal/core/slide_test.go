package core

import (
	"bytes"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"testing"
)

// ─────────────────────────────────────────────────────────────────────────────
// 合成题面 —— 不依赖任何外部文件，所以它**永远跑**
// ─────────────────────────────────────────────────────────────────────────────

// textureAt 是一张**确定性**的纹理（无随机源，两次跑逐字节相同）。
//
// 为什么必须"有纹理"：两个口径都在"两边都平坦"时**退化**（口径①直接跳过、
// 口径②的 Sobel 全零）。给一张纯色底图，测的就不是算法而是退化路径了。
//
// 取值恒为**偶数**且在 `[40, 238]` —— 偶数是为了让"压暗一半"能整除，
// 于是 `底图 = 0.5 × 拼图` **精确**成立、口径①在正确落点上 `1-R²` 恰好为 0。
func textureAt(x, y int) int {
	return 2 * (20 + (x*7+y*13+(x*y)%17*3)%100)
}

// grayPalette 是 256 级灰阶调色板 —— 索引与亮度**相等**。
//
// 用它当底图的调色板，是为了让"索引即灰度"这条口径在**两种解释下同结果**，
// 从而把"口径是否被改坏"这件事留给专门的
// [TestDecodeSlideImageKeepsPaletteIndexAsGray8] 去钉。
func grayPalette() color.Palette {
	pal := make(color.Palette, 256)
	for i := range pal {
		pal[i] = color.RGBA{uint8(i), uint8(i), uint8(i), 255}
	}
	return pal
}

// syntheticSlide 造一对题面图：底图是 **8-bit colormap PNG**（与实拍同形态），
// 拼图是 **RGBA PNG**，块在拼图里**从 x=0 起**、y 与底图对齐。
//
// 缺口是"原图压暗一半"，所以正确落点上 `底图 = 0.5 × 拼图` 精确成立 ——
// 这正是口径①假设的那个关系。
//
// 返回两张 PNG 的原文与"真值"（缺口左缘 = 块左缘在底图里的 x）。
func syntheticSlide(t *testing.T, bgW, bgH, blockW, blockY0, blockY1, notchX int) (bgPNG, pzPNG []byte, truth int) {
	t.Helper()
	pzW := blockW + 8 // 块右侧留 8 列透明，好让轮廓点出现在块的右缘

	bg := image.NewPaletted(image.Rect(0, 0, bgW, bgH), grayPalette())
	for y := 0; y < bgH; y++ {
		for x := 0; x < bgW; x++ {
			value := textureAt(x, y)
			if x >= notchX && x < notchX+blockW && y >= blockY0 && y < blockY1 {
				value /= 2 // 缺口 = 压暗一半
			}
			bg.SetColorIndex(x, y, uint8(value))
		}
	}

	pz := image.NewNRGBA(image.Rect(0, 0, pzW, bgH))
	for y := blockY0; y < blockY1; y++ {
		for x := 0; x < blockW; x++ {
			value := uint8(textureAt(notchX+x, y))
			pz.Set(x, y, color.NRGBA{R: value, G: value, B: value, A: 255})
		}
	}

	bgPNG = encodePNG(t, bg)
	pzPNG = encodePNG(t, pz)
	return bgPNG, pzPNG, notchX
}

func encodePNG(t *testing.T, m image.Image) []byte {
	t.Helper()
	var buf bytes.Buffer
	if err := png.Encode(&buf, m); err != nil {
		t.Fatalf("编码合成题面失败: %v", err)
	}
	return buf.Bytes()
}

// TestDecodeSlideImageKeepsPaletteIndexAsGray8 钉住**那条看着像 bug 的上游口径**。
//
// 调色板刻意做成"索引与亮度**反相关**"（`palette[i]` 的亮度是 `255-i`）：
// 这样"索引即灰度"与"亮度即灰度"给出**相反**的值，两者不可能同时通过。
//
// ⚠ 这条是**有意的**：谁把 `Gray8` 改成用真实亮度（"顺手修好"），
// 口径②就复现不出原版的 `41699 / 33011 / …` 与答案 `321`
// —— 见 [SlideImage.Gray8] 那段与 docs/capture/app-exchange-samples.txt §5.3。
func TestDecodeSlideImageKeepsPaletteIndexAsGray8(t *testing.T) {
	pal := make(color.Palette, 256)
	for i := range pal {
		v := uint8(255 - i)
		pal[i] = color.RGBA{R: v, G: v, B: v, A: 255}
	}
	src := image.NewPaletted(image.Rect(0, 0, 2, 1), pal)
	src.SetColorIndex(0, 0, 200)
	src.SetColorIndex(1, 0, 30)

	img, err := DecodeSlideImage(encodePNG(t, src))
	if err != nil {
		t.Fatalf("解码失败: %v", err)
	}
	if img.Width != 2 || img.Height != 1 {
		t.Fatalf("尺寸 = %d×%d，期望 2×1", img.Width, img.Height)
	}
	if got := img.Gray8At(0, 0); got != 200 {
		t.Errorf("Gray8At(0,0) = %d，期望 **调色板索引** 200（改成真实亮度会得到 55）", got)
	}
	if got := img.Gray8At(1, 0); got != 30 {
		t.Errorf("Gray8At(1,0) = %d，期望 30（真实亮度会是 225）", got)
	}
	if got := img.GrayAt(0, 0); got < 54.9 || got > 55.1 {
		t.Errorf("GrayAt(0,0) = %.2f，期望 ≈55（= 调色板颜色的真实亮度）", got)
	}
	if got := img.AlphaAt(0, 0); got < 254.9 || got > 255.1 {
		t.Errorf("AlphaAt(0,0) = %.2f，期望 255", got)
	}
}

// TestSolveSlideOnSyntheticPair 是两个口径的**正向**用例：
// 已知缺口在 `notchX`，两个口径都得落在它附近（±1px），共识值同样。
func TestSolveSlideOnSyntheticPair(t *testing.T) {
	const (
		bgW, bgH = 200, 120
		blockW   = 32
		blockY0  = 40
		blockY1  = 80
		notchX   = 100
	)
	bgPNG, pzPNG, truth := syntheticSlide(t, bgW, bgH, blockW, blockY0, blockY1, notchX)

	sol, err := SolveSlide(bgPNG, pzPNG)
	if err != nil {
		t.Fatalf("解算失败: %v", err)
	}

	if want := (SlideBox{MinX: 0, MinY: blockY0, MaxX: blockW - 1, MaxY: blockY1 - 1}); sol.Box != want {
		t.Errorf("块包围盒 = %+v，期望 %+v", sol.Box, want)
	}
	// 口径①是"整块线性拟合"，构造上在正确落点恰好成立 ⇒ 要求它**精确**。
	if sol.AffineOffset != truth {
		t.Errorf("口径① 落点 = %d，期望精确的 %d（构造上该处 1-R² 应为 0，实测 %.6f）",
			sol.AffineOffset, truth, sol.AffineBest)
	}
	if sol.AffineBest > 1e-9 {
		t.Errorf("口径① 最优度量 = %.9f，期望 ≈0（底图 = 0.5×拼图 精确成立）", sol.AffineBest)
	}
	if sol.AffineSkipped != 0 {
		t.Errorf("口径① 退化跳过 %d 个候选，期望 0 —— 纹理图不该有退化候选", sol.AffineSkipped)
	}
	// 口径②是"轮廓点落在底图边缘上的能量和"，边缘是**两侧各有一列**的台阶
	// ⇒ 它天然有 ±1px 的歧义（真实证据图上就是 ①320 / ②321）。允许 1px。
	if d := absInt(sol.ContourOffset - truth); d > 1 {
		t.Errorf("口径② 落点 = %d，期望 %d±1", sol.ContourOffset, truth)
	}
	if sol.ContourPoints <= 0 {
		t.Errorf("轮廓点 = %d，期望 >0", sol.ContourPoints)
	}
	if d := absInt(sol.Offset - truth); d > 1 {
		t.Errorf("共识落点 = %d，期望 %d±1", sol.Offset, truth)
	}
	if sol.Deviation > SlideMaxDeviation {
		t.Errorf("两口径分歧 %dpx，超过上限 %d", sol.Deviation, SlideMaxDeviation)
	}
	// 自检量必须真的被填上 —— 它是判据可信度的现场证据，不是装饰。
	if len(sol.AffineTop) != SlideTopN || len(sol.ContourTop) != SlideTopN {
		t.Errorf("前 %d 名自检量缺失：① %d 条、② %d 条",
			SlideTopN, len(sol.AffineTop), len(sol.ContourTop))
	}

	// 换一个落点再跑一次：防止"答案恒等于某个常量"这类假通过。
	otherBG, otherPZ, otherTruth := syntheticSlide(t, bgW, bgH, blockW, 10, 50, 37)
	other, err := SolveSlide(otherBG, otherPZ)
	if err != nil {
		t.Fatalf("换落点后解算失败: %v", err)
	}
	if absInt(other.Offset-otherTruth) > 1 {
		t.Errorf("换落点后共识 = %d，期望 %d±1（题面变了答案必须跟着变）", other.Offset, otherTruth)
	}
}

// TestSolveSlideAffineSkipsDegenerateInput 是那条**教训**的守卫：
// 判据在退化输入上必须"报无效"，**不是**"给一个看起来很像答案的错答案"。
//
// 本仓库实测栽过：最初那版用"比值 `底图/拼图` 的变异系数最小"，
// 在平滑背景（天空）上全域极差只有 `0.00003`，等于没测，
// 却照样打印出一个"最优 x=0"（docs/capture/app-exchange-samples.txt §5.3）。
func TestSolveSlideAffineSkipsDegenerateInput(t *testing.T) {
	const bgW, bgH, pzW = 200, 120, 40
	bg := image.NewPaletted(image.Rect(0, 0, bgW, bgH), grayPalette())
	for y := 0; y < bgH; y++ {
		for x := 0; x < bgW; x++ {
			bg.SetColorIndex(x, y, 128) // 纯色底图：没有任何纹理
		}
	}
	pz := image.NewNRGBA(image.Rect(0, 0, pzW, bgH))
	for y := 40; y < 80; y++ {
		for x := 0; x < 32; x++ {
			value := uint8(textureAt(x, y))
			pz.Set(x, y, color.NRGBA{R: value, G: value, B: value, A: 255})
		}
	}
	bgImg, err := DecodeSlideImage(encodePNG(t, bg))
	if err != nil {
		t.Fatalf("解码底图失败: %v", err)
	}
	pzImg, err := DecodeSlideImage(encodePNG(t, pz))
	if err != nil {
		t.Fatalf("解码拼图失败: %v", err)
	}

	candidates, skipped := SolveSlideAffine(bgImg, pzImg, SlideBlockBox(pzImg))
	if len(candidates) != 0 {
		t.Errorf("纯色底图上口径① 给出了 %d 个候选（首个落点 %d）—— 应当**全部判无效**",
			len(candidates), candidates[0].Offset)
	}
	if want := bgW - pzW + 1; skipped != want {
		t.Errorf("退化跳过 %d 个候选，期望 %d（= 全部候选）", skipped, want)
	}

	// 整条链路上也必须报错，而不是"用一个退化的答案继续"。
	if _, err := SolveSlide(encodePNG(t, bg), encodePNG(t, pz)); err == nil {
		t.Error("退化题面上 SolveSlide 应当报错（宁可报错也不要把一个荒谬坐标发上去）")
	}
}

// TestSolveSlideRejectsMalformedPair 挡掉三种"形状就不对"的输入。
//
// ⚠ 拼图是**通高竖条**、块保留原 y —— 不等高说明拿错图了，
// 而两个口径都会拿拼图的 y 直接索引底图 ⇒ 不挡就是越界。
func TestSolveSlideRejectsMalformedPair(t *testing.T) {
	okBG, okPZ, _ := syntheticSlide(t, 200, 120, 32, 40, 80, 100)

	t.Run("不等高", func(t *testing.T) {
		short := image.NewNRGBA(image.Rect(0, 0, 40, 60))
		for y := 0; y < 60; y++ {
			for x := 0; x < 32; x++ {
				short.Set(x, y, color.NRGBA{A: 255})
			}
		}
		if _, err := SolveSlide(okBG, encodePNG(t, short)); err == nil {
			t.Error("不等高应当报错")
		}
	})

	t.Run("底图不比拼图宽", func(t *testing.T) {
		narrow := image.NewNRGBA(image.Rect(0, 0, 40, 120))
		if _, err := SolveSlide(encodePNG(t, narrow), okPZ); err == nil {
			t.Error("没有可滑范围应当报错")
		}
	})

	t.Run("拼图全透明", func(t *testing.T) {
		blank := image.NewNRGBA(image.Rect(0, 0, 40, 120))
		if _, err := SolveSlide(okBG, encodePNG(t, blank)); err == nil {
			t.Error("全透明拼图应当报错")
		}
	})

	t.Run("不是PNG", func(t *testing.T) {
		if _, err := SolveSlide([]byte("not a png"), okPZ); err == nil {
			t.Error("非 PNG 应当报错")
		}
	})
}

// ─────────────────────────────────────────────────────────────────────────────
// 与真机证据图的对拍 —— 依赖 target/slide-evidence/，不在版本库里
// ─────────────────────────────────────────────────────────────────────────────

// evidenceDir 是那张可复算的证据题面图所在处。
//
// ⚠ **它有意不进版本库**（`/target/` 已在 `.gitignore` 里）：
// 它是从真机 `getSlide` 响应里解出来的原始素材，属于"日志可以留、仓库不能留"。
// 溯源与重跑方式见 `target/slide-evidence/README.md`。
const evidenceDir = "../../target/slide-evidence"

// TestSolveSlideEvidenceParity 拿**真机那张题面图**核对三个独立口径的一致性。
//
// 期望值逐字来自 docs/capture/app-exchange-samples.txt §5.3：
// 原项目 `match.ts` 给 `321`、得分前五名 `41699/33011/32069/31912/31506`；
// 本仓库两个口径给 `320`（①）/ `321`（②）。
//
// ⚠ 这条**在缺证据图时会跳过**，跳过的原因写在 skip 信息里 ——
// 别把"没跑"读成"通过了"。要在本机复跑：
//
//	ls target/slide-evidence/{picture,puzzle}.png
//
// 真丢了就去 Reqable 的大响应体缓存里捞（路径见 §7 第 3 步）。
func TestSolveSlideEvidenceParity(t *testing.T) {
	picturePath := filepath.Join(evidenceDir, "picture.png")
	puzzlePath := filepath.Join(evidenceDir, "puzzle.png")
	picturePNG, err := os.ReadFile(picturePath)
	if err != nil {
		t.Skipf("跳过对拍：读不到 %s（证据图刻意不入库，见 target/slide-evidence/README.md）: %v",
			picturePath, err)
	}
	puzzlePNG, err := os.ReadFile(puzzlePath)
	if err != nil {
		t.Skipf("跳过对拍：读不到 %s: %v", puzzlePath, err)
	}

	sol, err := SolveSlide(picturePNG, puzzlePNG)
	if err != nil {
		t.Fatalf("解算失败: %v", err)
	}

	if sol.Box != (SlideBox{MinX: 0, MinY: 197, MaxX: 94, MaxY: 292}) {
		t.Errorf("块包围盒 = %+v，期望 {0 197 94 292}（§5.3：宽 95、高 96）", sol.Box)
	}
	if sol.AffineOffset != 320 {
		t.Errorf("口径① 落点 = %d，期望 320", sol.AffineOffset)
	}
	if sol.ContourOffset != 321 {
		t.Errorf("口径② 落点 = %d，期望 321（= 原项目 matchSlide 的答案）", sol.ContourOffset)
	}
	if sol.ContourPoints != 310 {
		t.Errorf("轮廓点 = %d，期望 310", sol.ContourPoints)
	}
	wantScores := []float64{41699, 33011, 32069, 31912, 31506}
	wantOffsets := []int{321, 322, 319, 320, 323}
	for i, want := range wantScores {
		if got := -sol.ContourTop[i].Metric; got != want {
			t.Errorf("口径② 第 %d 名得分 = %.0f，期望 %.0f（原版 match.ts 的逐字输出）", i+1, got, want)
		}
		if got := sol.ContourTop[i].Offset; got != wantOffsets[i] {
			t.Errorf("口径② 第 %d 名落点 = %d，期望 %d", i+1, got, wantOffsets[i])
		}
	}
	// 分歧恰好 1px：超过上限就说明口径被改坏了（上限本身见 [SlideMaxDeviation]）。
	if sol.Deviation != 1 {
		t.Errorf("两口径分歧 = %dpx，期望 1px", sol.Deviation)
	}
}
