//! 滑块题面解算 —— 从 `getSlide` 的两张题面图里算出 `puzzleOffset`。
//!
//! # 它算的是什么
//!
//! `puzzleOffset` = **拼图块左边缘在底图（680×400 像素坐标）里的 x**，
//! 可滑范围 `[0, 680 - 96] = [0, 584]`。
//!
//! 它是 `exchangeV3` 的**必填**参数：上游对"没带"回 `501 滑块验证参数不能为空`，
//! 对"带了但算错"回 `514 验证失败`。而 `getSlide` 的 `puzzleLeft` 字段
//! **恒为 `null`**（4 条实拍完全一致）⇒ 这个值**只能本地算**，服务端不给。
//!
//! # 为什么是两个口径，而不是一个
//!
//! 单个"取极值"的判据在特定输入上**会退化，而且退化的表现不是报错、
//! 是给一个看起来很像答案的错答案**。参考实现实测栽过：最初那版用"比值
//! 底图/拼图的变异系数最小"，在平滑背景（天空）上全域极差只有 `0.00003`
//! —— 等于没测，却照样打印出一个"最优 x=0"。
//!
//! ⇒ 所以这里跑两个**互相独立**的度量，要求它们一致（差 ≤ [`SLIDE_MAX_DEVIATION`]）
//! 才采用，否则报错**不发下一枪**。
//!
//! # ⚠⚠ 上游口径是照抄的，不是"顺手修好"的
//!
//! 三处看着像 bug 的口径都必须**逐字保留**，否则复现不出原版得分：
//!
//! 1. **调色板索引当灰度**（[`SlideImage::gray8`]）—— 底图是 8-bit 调色板 PNG，
//!    口径②用的是**索引**而不是真实亮度。参考实现的测试刻意把调色板做成
//!    "索引与亮度反相关"，好让两种读法给出**相反**的值：照抄能复现原版
//!    `41699 / 33011 / …` 与答案 `321`，"修好"之后是 `27400 / …` 与 `319`。
//! 2. **整数灰度的 `>> 2` 与 `min(255, …)` 截断**（[`sobel_magnitude`]）——
//!    省了得分会大 2.6 倍、名次也会变。
//! 3. **轮廓点循环从 `x = 1` 起**（[`solve_contour`]）—— 结构上永远看不到
//!    第 0 列。别改成 `0`：那会改变轮廓点集与得分。
//!
//! 证据：`mcloud/docs/capture/app-exchange-samples.txt` §5.3。

use crate::{CryptoError, Result};

/// 可滑范围：底图宽度 - 拼图宽度。
///
/// 两个口径分歧的上限（像素）。超过它不是"答案不准"，而是**题面或算法出了问题**，
/// 处置是**不发下一枪**：那一枪花的是真豆。
///
/// 实拍那张证据图上分歧恰好是 `1`（① `320` / ② `321`），所以这个上限留了余量。
pub const SLIDE_MAX_DEVIATION: i32 = 3;

/// 诊断量里各留前几名。
pub const SLIDE_TOP_N: usize = 5;

/// 不透明判定阈值：`alpha > 128` 才算不透明（**`128` 本身算透明**）。
const ALPHA_OPAQUE: f64 = 128.0;

/// 口径①至少要这么多样本才认为拟合有意义。
const AFFINE_MIN_SAMPLES: f64 = 100.0;

/// 双方差下限：任一边低于它就认为"平坦 ⇒ R² 没有意义"。
const AFFINE_MIN_VARIANCE: f64 = 1.0;

// ══════════════════════════════ 图像 ══════════════════════════════

/// 一张题面图的三份读数。
///
/// 三份并存不是冗余：口径①吃 [`Self::gray`]（浮点亮度），口径②吃
/// [`Self::gray8`]（整数灰度，**调色板图时是索引**），两者都必须先过
/// [`Self::alpha`] 的阈值。
#[derive(Debug, Clone)]
pub struct SlideImage {
    width: usize,
    height: usize,
    gray: Vec<f64>,
    gray8: Vec<u8>,
    alpha: Vec<f64>,
}

impl SlideImage {
    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 浮点亮度（`0.299R + 0.587G + 0.114B`，输入是**预乘 alpha** 的 16 位值）。
    ///
    /// 越界返回 `0` 而不是 panic：解算器本身会大量访问边界外一列/一行
    /// （Sobel 的邻域、`offset + x` 的右缘），用哨兵值比在每个调用点判边界清楚。
    pub fn gray(&self, x: i64, y: i64) -> f64 {
        self.at(&self.gray, x, y).unwrap_or(0.0)
    }

    /// 整数灰度（或调色板图的**索引**）。
    pub fn gray8(&self, x: i64, y: i64) -> i32 {
        self.at(&self.gray8, x, y).map(i32::from).unwrap_or(0)
    }

    /// alpha（`0..~255`）。
    pub fn alpha(&self, x: i64, y: i64) -> f64 {
        self.at(&self.alpha, x, y).unwrap_or(0.0)
    }

    fn at<T>(&self, buf: &[T], x: i64, y: i64) -> Option<T>
    where
        T: Copy,
    {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return None;
        }
        buf.get(y as usize * self.width + x as usize).copied()
    }
}

/// 解一张 PNG 题面图。
///
/// # 逐字复刻上游的两处口径
///
/// - 浮点亮度用 **16 位预乘 alpha** 的值除以 `257`（不是先除再算）——
///   透明像素因此恒为 `0`，与 `color.Color::RGBA()` 的契约一致。
/// - `gray8` 用 `(r8*77 + g8*150 + b8*29) >> 8`，其中 `r8 = r16 >> 8`
///   （取 16 位值的**高字节**，不是 `/257`）。
/// - **调色板图把 `gray8` 换成索引**，见模块头第 1 条。
pub fn decode_slide_image(png: &[u8]) -> Result<SlideImage> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder
        .read_info()
        .map_err(|e| CryptoError::Parse(format!("解 PNG 失败（读头）：{e}")))?;

    // 调色板要在 `next_frame` 之前从 `Info` 里取（`OutputInfo` 不携带 PLTE）。
    let palette = reader.info().palette.clone();
    let source_indexed = reader.info().color_type == png::ColorType::Indexed;

    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| CryptoError::Parse(format!("解 PNG 失败（读帧）：{e}")))?;

    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 {
        return Err(CryptoError::Parse(format!(
            "题面图尺寸非法：{width}×{height}"
        )));
    }

    let n = width * height;
    let mut gray = vec![0.0f64; n];
    let mut gray8 = vec![0u8; n];
    let mut alpha = vec![0.0f64; n];

    // 每个像素取 (r16, g16, b16, a16)——**预乘 alpha**、0..65535。
    //
    // ⚠ 按 `info.color_type`（**输出**色型）分支：`png` 的默认变换不做
    // `EXPAND`/`ALPHA`，所以索引图**原样保留索引**（一个像素一个字节），
    // 而不是展开成 RGB。这一点是实测确认的 —— 别照抄"索引图会变成 RGB"的直觉，
    // 那会让偏移算错并越界。
    let rgb = |i: usize| -> (u16, u16, u16, u16) {
        match info.color_type {
            png::ColorType::Rgba => {
                let o = i * 4;
                premultiplied_8(
                    buf[o] as u16,
                    buf[o + 1] as u16,
                    buf[o + 2] as u16,
                    buf[o + 3] as u16,
                )
            }
            png::ColorType::Rgb => {
                let o = i * 3;
                premultiplied_8(buf[o] as u16, buf[o + 1] as u16, buf[o + 2] as u16, 255)
            }
            png::ColorType::GrayscaleAlpha => {
                let o = i * 2;
                let v = buf[o] as u16;
                premultiplied_8(v, v, v, buf[o + 1] as u16)
            }
            png::ColorType::Grayscale => {
                let v = buf[i] as u16;
                premultiplied_8(v, v, v, 255)
            }
            // 索引图：`buf[i]` 就是**调色板索引**。按 PLTE 查出颜色来算亮度
            // （与 Go 的 `Paletted.At()` 一致 —— 它也会查表拿到颜色，
            // 只有 `Gray8` 那份用索引，见模块头第 1 条）。
            png::ColorType::Indexed => {
                let idx = buf[i] as usize;
                let pal = palette.as_ref().map(|p| p.as_ref()).unwrap_or(&[]);
                let o = idx * 3;
                if o + 2 < pal.len() {
                    premultiplied_8(pal[o] as u16, pal[o + 1] as u16, pal[o + 2] as u16, 255)
                } else {
                    (0, 0, 0, 65535)
                }
            }
        }
    };

    for i in 0..n {
        let (r, g, b, a) = rgb(i);
        gray[i] = (0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b)) / 257.0;
        let (r8, g8, b8) = (i32::from(r >> 8), i32::from(g >> 8), i32::from(b >> 8));
        gray8[i] = ((r8 * 77 + g8 * 150 + b8 * 29) >> 8) as u8;
        alpha[i] = f64::from(a) / 257.0;
    }

    // ⚠⚠ 调色板图：整数灰度**换成索引**。
    //
    // `png` 的默认变换保留索引（一个像素一字节），所以这里直接就是原始索引
    // —— 与 Go 的 `Paletted.Pix[...]` 等价。
    //
    // 这是模块头第 1 条那个"看着像 bug"的口径：底图的 Sobel 跑在**索引**上，
    // 不是亮度。参考实现专门用"索引与亮度反相关"的调色板钉住它。
    if source_indexed && buf.len() >= n {
        gray8.copy_from_slice(&buf[..n]);
    }

    Ok(SlideImage {
        width,
        height,
        gray,
        gray8,
        alpha,
    })
}

/// 把 8 位分量按 `color.RGBA` 的语义升到 16 位并**预乘 alpha**。
///
/// Go 的 `color.Color::RGBA()` 契约就是"返回预乘 alpha 的 16 位值"，
/// 且 `v*0x101` 是 8 位到 16 位的展开（`255 → 65535`）。
fn premultiplied_8(r: u16, g: u16, b: u16, a: u16) -> (u16, u16, u16, u16) {
    let a16 = a * 0x101;
    if a == 255 {
        return (r * 0x101, g * 0x101, b * 0x101, a16);
    }
    if a == 0 {
        return (0, 0, 0, 0);
    }
    // 预乘：`c * a / 255`，在 16 位域里即 `c16 * a16 / 65535`。
    let pm = |c: u16| -> u16 {
        let c16 = u32::from(c) * 0x101;
        ((c16 * u32::from(a16) + 32767) / 65535) as u16
    };
    (pm(r), pm(g), pm(b), a16)
}

// ══════════════════════════════ 包围盒 ══════════════════════════════

/// 拼图里不透明像素的包围盒（**四边都包含**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlideBox {
    pub min_x: i64,
    pub min_y: i64,
    pub max_x: i64,
    pub max_y: i64,
}

impl SlideBox {
    pub fn width(&self) -> i64 {
        self.max_x - self.min_x + 1
    }

    pub fn height(&self) -> i64 {
        self.max_y - self.min_y + 1
    }

    /// 空盒：哨兵初值没被任何像素改写过。
    pub fn is_empty(&self) -> bool {
        self.max_x < self.min_x || self.max_y < self.min_y
    }
}

/// 求拼图块的不透明包围盒。
///
/// 判据是 `alpha > 128`（**`128` 本身算透明**）。实拍那张证据图给
/// `{0, 197, 94, 292}`，即宽 95、高 96。
pub fn block_box(pz: &SlideImage) -> SlideBox {
    let mut b = SlideBox {
        min_x: pz.width() as i64,
        min_y: pz.height() as i64,
        max_x: -1,
        max_y: -1,
    };
    for y in 0..pz.height() as i64 {
        for x in 0..pz.width() as i64 {
            if pz.alpha(x, y) <= ALPHA_OPAQUE {
                continue;
            }
            b.min_x = b.min_x.min(x);
            b.max_x = b.max_x.max(x);
            b.min_y = b.min_y.min(y);
            b.max_y = b.max_y.max(y);
        }
    }
    b
}

// ══════════════════════════════ 候选 ══════════════════════════════

/// 一个落点候选。**两个口径共用**，约定 [`Self::metric`] 越小越好。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlideCandidate {
    pub offset: i64,
    pub metric: f64,
}

impl SlideCandidate {
    fn cmp_better(&self, other: &Self) -> std::cmp::Ordering {
        // 先比度量，再比落点（小者优先）——**显式**定序，不依赖排序是否稳定。
        self.metric
            .partial_cmp(&other.metric)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(self.offset.cmp(&other.offset))
    }
}

fn sort_candidates(v: &mut [SlideCandidate]) {
    v.sort_by(SlideCandidate::cmp_better);
}

// ══════════════════════════ 口径①：仿射残差 ══════════════════════════

/// 口径①：在块包围盒上，找使 `1 - R²` 最小的落点。
///
/// 返回（候选，**退化被摘掉的个数**）。
///
/// 两个"摘掉"的出口**计数方式不同**，这是照抄的：
/// 样本数不足（`n < 100`）**不计入**退化数，方差过小才计入。
pub fn solve_affine(bg: &SlideImage, pz: &SlideImage, b: SlideBox) -> (Vec<SlideCandidate>, usize) {
    let span = bg.width() as i64 - pz.width() as i64;
    if b.is_empty() || span <= 0 {
        return (Vec::new(), 0);
    }

    let mut candidates = Vec::with_capacity(span as usize + 1);
    let mut skipped = 0usize;

    for offset in 0..=span {
        let (mut n, mut sum_p, mut sum_b) = (0.0f64, 0.0f64, 0.0f64);
        let (mut sum_pp, mut sum_bb, mut sum_pb) = (0.0f64, 0.0f64, 0.0f64);

        for y in b.min_y..=b.max_y {
            for x in b.min_x..=b.max_x {
                if pz.alpha(x, y) <= ALPHA_OPAQUE {
                    continue;
                }
                let gx = offset + x;
                if gx >= bg.width() as i64 {
                    continue;
                }
                let p = pz.gray(x, y);
                let q = bg.gray(gx, y);
                n += 1.0;
                sum_p += p;
                sum_b += q;
                sum_pp += p * p;
                sum_bb += q * q;
                sum_pb += p * q;
            }
        }

        if n < AFFINE_MIN_SAMPLES {
            continue;
        }
        let mean_p = sum_p / n;
        let mean_b = sum_b / n;
        let var_p = sum_pp / n - mean_p * mean_p;
        let var_b = sum_bb / n - mean_b * mean_b;
        if var_p < AFFINE_MIN_VARIANCE || var_b < AFFINE_MIN_VARIANCE {
            skipped += 1;
            continue;
        }
        let cov = sum_pb / n - mean_p * mean_b;
        candidates.push(SlideCandidate {
            offset,
            metric: 1.0 - cov * cov / (var_p * var_b),
        });
    }

    sort_candidates(&mut candidates);
    (candidates, skipped)
}

// ══════════════════════════ 口径②：轮廓匹配 ══════════════════════════

/// 底图的边缘强度图（整数 Sobel）。
///
/// # ⚠ 两处必须照抄的截断
///
/// `( |gx| + |gy| ) >> 2` 与 `min(255, …)`。省掉任何一处，得分都会大 2.6 倍
/// 且**名次会变**（实测 `109843 / 84272 / …` vs 原版 `41699 / 33011 / …`）。
///
/// 输入是 [`SlideImage::gray8`] —— 调色板图上那是**索引**，不是亮度。
pub fn sobel_magnitude(bg: &SlideImage) -> Vec<f64> {
    let (w, h) = (bg.width() as i64, bg.height() as i64);
    let mut out = vec![0.0f64; bg.width() * bg.height()];

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let tl = bg.gray8(x - 1, y - 1);
            let tc = bg.gray8(x, y - 1);
            let tr = bg.gray8(x + 1, y - 1);
            let ml = bg.gray8(x - 1, y);
            let mr = bg.gray8(x + 1, y);
            let bl = bg.gray8(x - 1, y + 1);
            let bc = bg.gray8(x, y + 1);
            let br = bg.gray8(x + 1, y + 1);

            let gx = -tl + tr - 2 * ml + 2 * mr - bl + br;
            let gy = -tl - 2 * tc - tr + bl + 2 * bc + br;
            let mut value = (gx.abs() + gy.abs()) >> 2;
            if value > 255 {
                value = 255;
            }
            out[(y * w + x) as usize] = f64::from(value);
        }
    }
    out
}

/// 口径②：原项目 `matchSlide` 的**逐字移植** —— 用拼图轮廓点去配底图边缘。
///
/// # ⚠ 轮廓点循环从 `x = 1` 起
///
/// 结构上永远看不到第 `0` 列。这不是要修的 off-by-one：参考实现专门做过
/// 反向实验（在左边补一列透明像素），证明改掉它会同时改变轮廓点集与得分。
///
/// 返回（候选，**轮廓点数**）。候选的 `metric` 是**取负的和**，
/// 这样"越小越好"的约定与口径①共用。
pub fn solve_contour(bg: &SlideImage, pz: &SlideImage) -> (Vec<SlideCandidate>, usize) {
    let span = bg.width() as i64 - pz.width() as i64;
    if span <= 0 || bg.height() != pz.height() {
        return (Vec::new(), 0);
    }

    let edges = sobel_magnitude(bg);

    // 轮廓点：自身不透明、且四邻至少一个透明。
    let mut points: Vec<(i64, i64)> = Vec::with_capacity(2048);
    for y in 1..pz.height() as i64 - 1 {
        for x in 1..pz.width() as i64 - 1 {
            if pz.alpha(x, y) <= ALPHA_OPAQUE {
                continue;
            }
            if pz.alpha(x - 1, y) <= ALPHA_OPAQUE
                || pz.alpha(x + 1, y) <= ALPHA_OPAQUE
                || pz.alpha(x, y - 1) <= ALPHA_OPAQUE
                || pz.alpha(x, y + 1) <= ALPHA_OPAQUE
            {
                points.push((x, y));
            }
        }
    }

    let mut scores = Vec::with_capacity(span as usize + 1);
    for offset in 0..=span {
        let mut sum = 0.0f64;
        for &(px, py) in &points {
            let gx = offset + px;
            // 只要上界：`gx >= 0` 恒真（`offset >= 0` 且 `px >= 1`），
            // 但下界判断留着以免越界读。
            if gx >= 0 && gx < bg.width() as i64 {
                sum += edges[(py * bg.width() as i64 + gx) as usize];
            }
        }
        scores.push(SlideCandidate {
            offset,
            metric: -sum,
        });
    }

    sort_candidates(&mut scores);
    (scores, points.len())
}

// ══════════════════════════════ 解 ══════════════════════════════

/// 一次解算的完整读数。
///
/// `top` 那两份**不参与**决定答案，是给排障用的自检量：
/// "最优与次优几乎一样"正是退化度量的样子，而度量本身不会报错。
#[derive(Debug, Clone, PartialEq)]
pub struct SlideSolution {
    /// 共识值：两口径一致时取**截断平均**。
    pub offset: i64,
    /// 两口径的差（绝对值）。
    pub deviation: i64,
    pub block: SlideBox,
    pub affine_offset: i64,
    /// 口径①的最优度量（`1 - R²`，越接近 0 越好）。
    pub affine_metric: f64,
    pub affine_skipped: usize,
    pub affine_top: Vec<SlideCandidate>,
    pub contour_offset: i64,
    /// 口径②的最优**原始得分**（越大越好，已从内部负分翻回来）。
    pub contour_score: f64,
    pub contour_points: usize,
    pub contour_top: Vec<SlideCandidate>,
}

fn top_n(v: &[SlideCandidate]) -> Vec<SlideCandidate> {
    v.iter().take(SLIDE_TOP_N).copied().collect()
}

/// 从两张已解码的题面图求 `puzzleOffset`。
///
/// 判不了就**报错**（调用方据此不发那一枪），绝不返回一个"猜的"值：
/// 那一枪花的是真豆。
pub fn solve_from_images(bg: &SlideImage, pz: &SlideImage) -> Result<SlideSolution> {
    if bg.height() != pz.height() {
        return Err(CryptoError::Parse(format!(
            "拼图与底图不等高（{} vs {}）—— 拼图是通高竖条，块保留原 y",
            pz.height(),
            bg.height()
        )));
    }
    if bg.width() <= pz.width() {
        return Err(CryptoError::Parse(format!(
            "底图不比拼图宽（{} vs {}）—— 没有可滑范围",
            bg.width(),
            pz.width()
        )));
    }

    let block = block_box(pz);
    if block.is_empty() {
        return Err(CryptoError::Parse(
            "拼图里没有不透明像素（alpha 全 ≤128）".into(),
        ));
    }

    let (affine, skipped) = solve_affine(bg, pz, block);
    let Some(best_affine) = affine.first().copied() else {
        return Err(CryptoError::Parse(
            "口径①：没有一个有效候选（全部退化 —— 题面可能过于平坦）".into(),
        ));
    };

    let (contour, points) = solve_contour(bg, pz);
    let Some(best_contour) = contour.first().copied() else {
        return Err(CryptoError::Parse(
            "口径②：一个候选都没有（轮廓点或尺寸不合法）".into(),
        ));
    };

    let deviation = (best_affine.offset - best_contour.offset).abs();
    if deviation > i64::from(SLIDE_MAX_DEVIATION) {
        return Err(CryptoError::Parse(format!(
            "⚠ 两个口径分歧 {deviation}px（① {} vs ② {}，上限 {SLIDE_MAX_DEVIATION}）⇒ 不要提交，先查题面",
            best_affine.offset, best_contour.offset
        )));
    }

    Ok(SlideSolution {
        offset: (best_affine.offset + best_contour.offset) / 2,
        deviation,
        block,
        affine_offset: best_affine.offset,
        affine_metric: best_affine.metric,
        affine_skipped: skipped,
        affine_top: top_n(&affine),
        contour_offset: best_contour.offset,
        contour_score: -best_contour.metric,
        contour_points: points,
        contour_top: top_n(&contour),
    })
}

/// 从两张 PNG 原文求 `puzzleOffset`。
///
/// # 不要加抖动
///
/// 参考实现里有个 `rand(-3..3)`，那是**风控规避**、不是容忍度补偿。
/// 而实拍明确否掉了"主动加抖"这条建议：偏差 `0` 的那次**通过了**，
/// 主动加 `+10` 的那次被拒（`514`）—— 加抖会把本来能过的提交弄失败。
pub fn solve_slide(picture_png: &[u8], puzzle_png: &[u8]) -> Result<SlideSolution> {
    let bg =
        decode_slide_image(picture_png).map_err(|e| CryptoError::Parse(format!("底图：{e}")))?;
    let pz =
        decode_slide_image(puzzle_png).map_err(|e| CryptoError::Parse(format!("拼图：{e}")))?;
    solve_from_images(&bg, &pz)
}

#[cfg(test)]
mod tests;
