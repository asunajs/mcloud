//! 滑块解算的测试。
//!
//! # 造题面的方式与参考实现一致
//!
//! 底图是 **8-bit 调色板 PNG**（与实拍同形态），拼图是 **RGBA PNG**，
//! 缺口是"原图压暗一半"，所以正确落点上 `底图 = 0.5 × 拼图` 精确成立 ——
//! 这正是口径①假设的那个关系。

use super::*;

// ── 造图 ────────────────────────────────────────────────────────────────

/// 确定性的纹理：有足够变化，方差不会退化。
fn texture_at(x: usize, y: usize) -> u8 {
    let v = (x * 7 + y * 13 + (x * y) % 31) % 256;
    (v as u8).wrapping_add(20)
}

/// 灰度调色板：`palette[i]` 的灰度就是 `i`。
fn gray_palette() -> Vec<[u8; 3]> {
    (0..256u16).map(|i| [i as u8, i as u8, i as u8]).collect()
}

/// 索引与亮度**反相关**的调色板（`palette[i]` 亮度 = `255 - i`）。
///
/// 用来钉"整数灰度用的是**索引**而不是亮度"：两种读法会给出**相反**的值，
/// 不可能同时通过。
fn inverted_palette() -> Vec<[u8; 3]> {
    (0..256u16).map(|i| [(255 - i) as u8; 3]).collect()
}

/// 把 8-bit 索引图编码成 PNG。
fn encode_indexed(w: u32, h: u32, palette: &[[u8; 3]], pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        let plte: Vec<u8> = palette.iter().flatten().copied().collect();
        enc.set_palette(std::borrow::Cow::Owned(plte));
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
    }
    out
}

/// 把 RGBA 图编码成 PNG。
fn encode_rgba(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
    }
    out
}

/// 造一对题面图：底图调色板图带"压暗一半"的缺口，拼图 RGBA 带同一块。
///
/// 返回（底图 PNG、拼图 PNG、真值 offset）。
fn synthetic_slide(
    bg_w: usize,
    bg_h: usize,
    block_w: usize,
    block_y0: usize,
    block_y1: usize,
    notch_x: usize,
) -> (Vec<u8>, Vec<u8>, i64) {
    let pz_w = block_w + 8; // 块右侧留 8 列透明

    // 底图：缺口处压暗一半。
    let mut bg_px = vec![0u8; bg_w * bg_h];
    for y in 0..bg_h {
        for x in 0..bg_w {
            let mut v = texture_at(x, y);
            if x >= notch_x && x < notch_x + block_w && y >= block_y0 && y < block_y1 {
                v /= 2;
            }
            bg_px[y * bg_w + x] = v;
        }
    }

    // 拼图：块在 x=0 起、y 与底图对齐；块的像素取**底图缺口处的原值 ×2**。
    // 也就是"块是从原图上抠下来的"，所以正确落点上 `底图 = 拼图 / 2`。
    let mut pz_px = vec![0u8; pz_w * bg_h * 4];
    for y in 0..bg_h {
        for x in 0..pz_w {
            let o = (y * pz_w + x) * 4;
            if x < block_w && y >= block_y0 && y < block_y1 {
                // 缺口值 ×2 还原出原值（缺口是 `v/2`，有取整误差，但纹理偶数居多）。
                let notch = texture_at(notch_x + x, y) / 2;
                let orig = notch * 2;
                pz_px[o] = orig;
                pz_px[o + 1] = orig;
                pz_px[o + 2] = orig;
                pz_px[o + 3] = 255;
            } else {
                pz_px[o + 3] = 0; // 透明
            }
        }
    }

    let bg_png = encode_indexed(bg_w as u32, bg_h as u32, &gray_palette(), &bg_px);
    let pz_png = encode_rgba(pz_w as u32, bg_h as u32, &pz_px);
    (bg_png, pz_png, notch_x as i64)
}

// ── 解码 ────────────────────────────────────────────────────────────────

#[test]
fn decodes_an_rgba_image() {
    // 2×2：红、绿、蓝、半透明黑。
    let px = vec![
        255, 0, 0, 255, //
        0, 255, 0, 255, //
        0, 0, 255, 255, //
        0, 0, 0, 128,
    ];
    let png = encode_rgba(2, 2, &px);
    let img = decode_slide_image(&png).unwrap();
    assert_eq!((img.width(), img.height()), (2, 2));
    assert_eq!(img.alpha(0, 0), 255.0, "不透明像素 alpha 是 255");
    assert!(
        img.gray(0, 0) > 70.0 && img.gray(0, 0) < 80.0,
        "红色亮度约 76"
    );
}

#[test]
fn palette_image_uses_the_index_not_the_luminance() {
    // ⭐ 这条钉住"看着像 bug 的上游口径"。
    //
    // 调色板刻意做成索引与亮度**反相关** ⇒ 两种读法给出相反的值。
    // 谁把 `gray8` 改成真实亮度（"顺手修好"），这条立刻炸。
    let palette = inverted_palette(); // palette[i] 亮度 = 255 - i
    let px = vec![200u8, 10];
    let png = encode_indexed(2, 1, &palette, &px);
    let img = decode_slide_image(&png).unwrap();

    assert_eq!(img.gray8(0, 0), 200, "整数灰度必须是**索引** 200");
    assert_eq!(img.gray8(1, 0), 10);
    // 真实亮度是反过来的：索引 200 的颜色亮度是 55
    assert!(
        img.gray(0, 0) < 60.0,
        "浮点亮度仍走真实颜色：{}",
        img.gray(0, 0)
    );
}

#[test]
fn rejects_garbage_and_zero_sized() {
    assert!(decode_slide_image(b"not a png").is_err());
    assert!(decode_slide_image(&[]).is_err());
}

// ── 包围盒 ──────────────────────────────────────────────────────────────

#[test]
fn block_box_is_inclusive_and_treats_128_as_transparent() {
    // 4×3，不透明区域是 x∈[1,2]、y∈[0,1]。
    let mut px = vec![0u8; 4 * 3 * 4];
    for y in 0..2 {
        for x in 1..3 {
            let o = (y * 4 + x) * 4;
            px[o] = 10;
            px[o + 1] = 10;
            px[o + 2] = 10;
            px[o + 3] = 255;
        }
    }
    // 再放一个 alpha 恰为 128 的像素 —— 它**不算**不透明。
    let o = (2 * 4 + 3) * 4;
    px[o + 3] = 128;

    let img = decode_slide_image(&encode_rgba(4, 3, &px)).unwrap();
    let b = block_box(&img);
    assert_eq!(
        b,
        SlideBox {
            min_x: 1,
            min_y: 0,
            max_x: 2,
            max_y: 1
        }
    );
    assert_eq!((b.width(), b.height()), (2, 2));
    assert!(!b.is_empty());
}

#[test]
fn fully_transparent_puzzle_gives_an_empty_box() {
    let px = [0u8; 4 * 4 * 4];
    let img = decode_slide_image(&encode_rgba(4, 4, &px)).unwrap();
    assert!(block_box(&img).is_empty());
}

// ── 解算 ────────────────────────────────────────────────────────────────

#[test]
fn solves_a_synthetic_puzzle_at_the_right_offset() {
    let (bg, pz, truth) = synthetic_slide(200, 120, 40, 30, 90, 123);
    let sol = solve_slide(&bg, &pz).unwrap();

    // 合成题的缺口是"整数除以 2"，这让相邻落点的边缘结构出现系统性并列
    // （`bg = t/2` 与 `pz = 2*(t/2)` 在奇偶上交替）⇒ 允许 ±1 的固有偏差。
    //
    // ⚠ 真正的逐像素对拍**不在这里**，而在真机那张证据图上
    // （见 `tests/slide_parity.rs`：期望 320 / 321 / 310 / 41699…）。
    // 这条只保证"整条链能把答案定位到真值附近"，不冒充精确判据。
    assert!(
        (sol.contour_offset - truth).abs() <= 1,
        "口径②应落在真值 ±1，实际 {}（真值 {truth}）",
        sol.contour_offset
    );
    assert!(
        (sol.affine_offset - truth).abs() <= 1,
        "口径①应落在真值 ±1，实际 {}",
        sol.affine_offset
    );
    assert!(
        sol.deviation <= i64::from(SLIDE_MAX_DEVIATION),
        "两口径应一致，实际差 {}",
        sol.deviation
    );
    assert!(
        (sol.offset - truth).abs() <= 1,
        "共识值 {} 应在真值 {truth} ±1 内",
        sol.offset
    );
}

#[test]
fn solution_carries_the_diagnostic_quantities() {
    let (bg, pz, _) = synthetic_slide(200, 120, 40, 30, 90, 77);
    let sol = solve_slide(&bg, &pz).unwrap();

    // 块 40 宽；不透明行是 y ∈ [30, 90) ⇒ 高 60
    assert_eq!(sol.block.width(), 40);
    assert_eq!(sol.block.height(), 60);
    assert!(sol.affine_metric >= 0.0 && sol.affine_metric < 1.0);
    assert!(sol.contour_score > 0.0, "轮廓得分是边缘强度之和");
    assert!(sol.contour_points > 0);
    assert!(!sol.affine_top.is_empty());
    assert!(!sol.contour_top.is_empty());
    assert!(sol.affine_top.len() <= SLIDE_TOP_N);
    assert!(sol.contour_top.len() <= SLIDE_TOP_N);
    // top 必须已排序（小者优先）
    for w in sol.contour_top.windows(2) {
        assert!(w[0].metric <= w[1].metric, "contour_top 未排序");
    }
    for w in sol.affine_top.windows(2) {
        assert!(w[0].metric <= w[1].metric, "affine_top 未排序");
    }
}

#[test]
fn rejects_mismatched_heights_and_widths() {
    // 拼图与底图等高是前提（拼图是通高竖条）。
    let bg = encode_indexed(50, 30, &gray_palette(), &[10u8; 50 * 30]);
    let pz = encode_rgba(10, 20, &[255u8; 10 * 20 * 4]);
    let e = solve_slide(&bg, &pz).unwrap_err().to_string();
    assert!(e.contains("不等高"), "{e}");

    // 底图不比拼图宽 ⇒ 没有可滑范围。
    let bg = encode_indexed(10, 20, &gray_palette(), &[10u8; 10 * 20]);
    let pz = encode_rgba(10, 20, &[255u8; 10 * 20 * 4]);
    let e = solve_slide(&bg, &pz).unwrap_err().to_string();
    assert!(e.contains("没有可滑范围"), "{e}");
}

#[test]
fn flat_imagery_does_not_produce_a_confident_answer() {
    // 全平坦的底图与拼图：口径①会因方差退化而一个候选都没有。
    // 这条钉的是"判不了就报错"，而不是"给一个看起来很像答案的值"。
    let bg = encode_indexed(64, 32, &gray_palette(), &[42u8; 64 * 32]);
    let pz = encode_rgba(16, 32, &[255u8; 16 * 32 * 4]);
    let r = solve_slide(&bg, &pz);
    assert!(r.is_err(), "平坦题面必须报错，而不是给一个假答案：{r:?}");
}

#[test]
fn sobel_zeroes_the_border_and_never_exceeds_255() {
    let mut px = vec![0u8; 8 * 8];
    for (i, v) in px.iter_mut().enumerate() {
        *v = ((i * 37) % 256) as u8;
    }
    let img = decode_slide_image(&encode_indexed(8, 8, &gray_palette(), &px)).unwrap();
    let e = sobel_magnitude(&img);

    for i in 0..8 {
        assert_eq!(e[i], 0.0, "首行应为 0");
        assert_eq!(e[7 * 8 + i], 0.0, "末行应为 0");
        assert_eq!(e[i * 8], 0.0, "首列应为 0");
        assert_eq!(e[i * 8 + 7], 0.0, "末列应为 0");
    }
    assert!(
        e.iter().all(|&v| (0.0..=255.0).contains(&v)),
        "必须被截断到 255"
    );
}

#[test]
fn solve_is_deterministic() {
    let (bg, pz, _) = synthetic_slide(160, 100, 32, 20, 80, 55);
    let a = solve_slide(&bg, &pz).unwrap();
    let b = solve_slide(&bg, &pz).unwrap();
    assert_eq!(a, b, "同一题面必须给同一个答案");
}
