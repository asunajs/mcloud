//! 真机题面图的**逐像素对拍** —— 这条才是解算器的权威判据。
//!
//! # 期望值从哪来
//!
//! 逐字来自 `mcloud/docs/capture/app-exchange-samples.txt` §5.3：
//!
//! | 量 | 期望 |
//! |---|---|
//! | 块包围盒 | `{0, 197, 94, 292}`（宽 95、高 96）|
//! | 口径① 落点 | `320` |
//! | 口径② 落点 | `321`（= 原项目 `match.ts` 的答案）|
//! | 轮廓点数 | `310` |
//! | 口径② 前五名 | 得分 `41699 / 33011 / 32069 / 31912 / 31506`，落点 `321 / 322 / 319 / 320 / 323` |
//! | 两口径分歧 | `1` px |
//!
//! # ⚠ 这条在缺证据图时会**跳过**
//!
//! 那两张图是**真机抓包产物**，体量大且含账号信息，**刻意不入库**
//! （上游仓库同样如此）。跳过时会把原因打出来 —— **别把"跳过"读成"通过"**。
//!
//! 要在本机复跑，把图放到 `test/fixtures/slide-evidence/{picture,puzzle}.png`。
//! 它们在抓包侧位于 `target/slide-evidence/`（也是 gitignore）。

use std::path::{Path, PathBuf};

use mcloud_crypto::slide::{self, SlideBox};

fn evidence_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` = crates/mcloud-crypto
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/slide-evidence")
}

fn read_evidence() -> Option<(Vec<u8>, Vec<u8>, PathBuf)> {
    let dir = evidence_dir();
    let picture = dir.join("picture.png");
    let puzzle = dir.join("puzzle.png");
    let (Ok(p), Ok(z)) = (std::fs::read(&picture), std::fs::read(&puzzle)) else {
        return None;
    };
    Some((p, z, dir))
}

#[test]
fn solves_the_real_captured_puzzle_with_parity() {
    let Some((picture, puzzle, dir)) = read_evidence() else {
        eprintln!(
            "⏭  跳过对拍：读不到 {}/{{picture,puzzle}}.png。\n\
             这不是通过。要复跑请把真机抓包的那两张题面图放到该目录（它们刻意不入库）。",
            evidence_dir().display()
        );
        return;
    };
    let _ = dir;

    let sol = slide::solve_slide(&picture, &puzzle).expect("真机题面必须解得出来");

    // 包围盒（宽 95、高 96）
    assert_eq!(
        sol.block,
        SlideBox {
            min_x: 0,
            min_y: 197,
            max_x: 94,
            max_y: 292
        },
        "块包围盒应与抓包一致"
    );

    // 两个独立口径各自的落点
    assert_eq!(sol.affine_offset, 320, "口径① 落点");
    assert_eq!(
        sol.contour_offset, 321,
        "口径② 落点（= 原项目 match.ts 的答案）"
    );
    assert_eq!(sol.contour_points, 310, "轮廓点数");
    assert_eq!(sol.deviation, 1, "两口径分歧恰好 1px");

    // 共识值 = 截断平均 (320 + 321) / 2 = 320
    assert_eq!(sol.offset, 320, "共识值");

    // ⭐ 前五名逐字命中 —— 这条才真正钉住"口径②是逐字移植的"。
    // 得分变了说明 Sobel 的 `>>2`/`min(255,…)` 或"索引当灰度"被改坏了。
    let want_scores = [41699.0, 33011.0, 32069.0, 31912.0, 31506.0];
    let want_offsets = [321i64, 322, 319, 320, 323];
    assert_eq!(sol.contour_top.len(), want_scores.len(), "应给出前五名");
    for (i, (&score, &offset)) in want_scores.iter().zip(want_offsets.iter()).enumerate() {
        let got = &sol.contour_top[i];
        assert_eq!(
            -got.metric,
            score,
            "口径② 第 {} 名得分（落点 {}）",
            i + 1,
            got.offset
        );
        assert_eq!(got.offset, offset, "口径② 第 {} 名落点", i + 1);
    }
}

#[test]
fn parity_ground_truth_constants_are_the_documented_ones() {
    // 把上面那条的期望值也钉在这里：证据图缺失时它仍会跑，
    // 于是"有人顺手改了期望值"这件事不会因为图不在而静默通过。
    assert_eq!(slide::SLIDE_MAX_DEVIATION, 3);
    assert_eq!(slide::SLIDE_TOP_N, 5);
}
