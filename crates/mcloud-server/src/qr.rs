//! 把二维码载荷渲染成给人看的形式（终端字符画 / SVG）。
//!
//! # 为什么放在服务端
//!
//! CLI 与 WebUI 都要展示同一张码。两边各写一遍渲染的后果**不是**样式不同，
//! 而是其中一边**画错**了（行列错位、静默区少一行）—— 表现是"码扫不出来"，
//! 而用户只会觉得是软件坏了。
//!
//! 放在服务端还顺带解决 WebUI 的问题：前端项目里**没有任何二维码库**，
//! 而在浏览器里现写一个编码器等于把"码对不对"这件事交给前端。
//! 服务端编码，前端只负责显示。
//!
//! # 两种渲染共用同一份矩阵
//!
//! [`Matrix`] 是唯一的中间产物：终端字符画与 SVG 都从它画出来。
//! 于是"静默区多大""边长取奇还是取偶"这类判断只存在一处。

use qrcode::{Color, QrCode};

/// 静默区宽度（模块数）。
///
/// 二维码规范要求四周留 4 模块。少了它，深色背景（终端、暗色主题）会与码的
/// 边缘糊在一起，扫码成功率明显下降。
const QUIET: usize = 4;

/// 渲染好的二维码矩阵（含静默区）。
pub struct Matrix {
    /// 边长（模块数）。**一定是偶数**，理由见 [`Matrix::side`]。
    side: usize,
    /// 按行存的深浅。`true` = 深色模块。
    dark: Vec<bool>,
    /// 不含静默区的边长（编码器给的那个）。
    code_width: usize,
}

impl Matrix {
    /// # ⚠ 边长会被凑成偶数
    ///
    /// 终端用**半块字符**画：一个字符格 = 上下两个像素行。偶数边长才能保证
    /// 每行都正好用完，否则最后一行只有上半格有内容，而渲染器会给它编出
    /// 一个**不存在的**下半行。二维码边长恒为奇数（21/25/29/…），
    /// 两侧各留 4 模块后仍是奇数，所以这里向上凑偶：
    /// 多出来的那一行/列落在静默区里，不碰码本身。
    pub fn new(payload: &str) -> mcloud_core::Result<Self> {
        let code = QrCode::new(payload.as_bytes()).map_err(|e| {
            mcloud_core::Error::Config(format!("二维码编码失败（载荷过长？）：{e}"))
        })?;
        let code_width = code.width();
        let side = (code_width + QUIET * 2 + 1) & !1;

        let mut dark = vec![false; side * side];
        for y in 0..side {
            for x in 0..side {
                // 落在静默区（或凑偶多出来的那一行/列）一律浅色
                if x < QUIET || y < QUIET {
                    continue;
                }
                let (px, py) = (x - QUIET, y - QUIET);
                if px >= code_width || py >= code_width {
                    continue;
                }
                if code[(px, py)] == Color::Dark {
                    dark[y * side + x] = true;
                }
            }
        }
        Ok(Self {
            side,
            dark,
            code_width,
        })
    }

    pub fn side(&self) -> usize {
        self.side
    }

    /// 不含静默区的边长（排障与测试用）。
    pub fn code_width(&self) -> usize {
        self.code_width
    }

    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.side && y < self.side && self.dark[y * self.side + x]
    }

    /// 画成终端字符画（半块字符）。
    ///
    /// ⚠ 输出**不含反色转义**：终端主题有深有浅，硬编一个反色会在
    /// 浅色主题下画出白底白码。深浅由调用方的终端背景决定。
    pub fn to_terminal(&self) -> String {
        let mut out = String::with_capacity(self.side * (self.side / 2 + 1));
        let mut y = 0;
        while y < self.side {
            for x in 0..self.side {
                let top = self.is_dark(x, y);
                let bottom = self.is_dark(x, y + 1);
                out.push(match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
            out.push('\n');
            y += 2;
        }
        out
    }

    /// 画成 SVG。
    ///
    /// # 为什么用一条 `<path>` 而不是一堆 `<rect>`
    ///
    /// 逐模块画 `<rect>` 会产生上千个元素（一张 57×57 的码约 1300 个），
    /// 响应体大一倍且浏览器要做上千次布局。按**行内连续段**合并成
    /// 一个 path 的 `h`（水平线）指令，段数通常只有几百。
    ///
    /// # 颜色写死黑白
    ///
    /// 深色主题下也保持黑码白底：二维码是给**摄像头**读的，
    /// 跟着主题反转会降低识别率（很多扫码器假设"深色是数据"）。
    /// 外面套一层白色圆角容器比反转颜色可靠。
    pub fn to_svg(&self) -> String {
        let n = self.side;
        let mut path = String::new();
        for y in 0..n {
            let mut x = 0;
            while x < n {
                if !self.is_dark(x, y) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < n && self.is_dark(x, y) {
                    x += 1;
                }
                let run = x - start;
                // 相对指令：`h<run>` 画横线，`v1` 有厚度，`h-<run>` 回来闭合
                path.push_str(&format!("M{start} {y}h{run}v1h-{run}z"));
            }
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}" shape-rendering="crispEdges" role="img" aria-label="登录二维码"><rect width="{n}" height="{n}" fill="#ffffff"/><path d="{path}" fill="#000000"/></svg>"##
        )
    }

    /// SVG 的 data URL（给 `<img src=...>` 用）。
    ///
    /// 用 data URL 而不是让前端 `innerHTML` 塞 SVG 串：前者是**数据**，
    /// 浏览器按图片处理；后者是往 DOM 里注入标记，多开一条本不需要的路。
    /// 这里的内容确实只由我们生成（没有用户输入），但"确实安全"不如"结构上不可能出事"。
    ///
    /// base64 而不是 URL 编码：SVG 里有大量 `#`、`<`、`"`，
    /// 百分号编码后体积更大且更容易漏编码一个字符。
    pub fn to_svg_data_url(&self) -> String {
        use base64::Engine as _;
        let svg = self.to_svg();
        let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
        format!("data:image/svg+xml;base64,{b64}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str =
        "https://yun.139.com/w/#/qrcLogin?dID=62AA2BDA29F99744536E142A83CA5B03-ENDIN&sID=ABCDEF&cType=11&ver=4.2.1";

    #[test]
    fn side_is_even_and_covers_the_code_plus_quiet_zone() {
        let m = Matrix::new(PAYLOAD).unwrap();
        assert_eq!(m.code_width() % 2, 1, "二维码边长本来就是奇数");
        assert_eq!(m.side() % 2, 0, "渲染用的边长必须是偶数（半块字符）");
        assert!(
            m.side() >= m.code_width() + QUIET * 2,
            "静默区不够：{} < {}",
            m.side(),
            m.code_width() + QUIET * 2
        );
        // 只多出凑偶需要的那一行/列
        assert!(m.side() <= m.code_width() + QUIET * 2 + 1);
    }

    /// 静默区必须**真的**是浅色 —— 否则码与背景糊在一起。
    #[test]
    fn quiet_zone_is_light() {
        let m = Matrix::new(PAYLOAD).unwrap();
        for i in 0..m.side() {
            for q in 0..QUIET {
                assert!(!m.is_dark(i, q), "上边静默区有深色 ({i},{q})");
                assert!(!m.is_dark(i, m.side() - 1 - q), "下边有深色");
                assert!(!m.is_dark(q, i), "左边有深色");
                assert!(!m.is_dark(m.side() - 1 - q, i), "右边有深色");
            }
        }
    }

    /// 码区必须**有**深色（否则整张图是白的，编码显然错了）。
    #[test]
    fn code_area_has_dark_modules() {
        let m = Matrix::new(PAYLOAD).unwrap();
        let count = (0..m.side())
            .flat_map(|y| (0..m.side()).map(move |x| (x, y)))
            .filter(|(x, y)| m.is_dark(*x, *y))
            .count();
        assert!(count > 50, "深色模块太少：{count}");
    }

    /// 终端画：行数 = 边长 / 2，且宽度等于边长。
    #[test]
    fn terminal_art_is_square() {
        let m = Matrix::new(PAYLOAD).unwrap();
        let art = m.to_terminal();
        let lines: Vec<&str> = art.lines().collect();
        assert_eq!(lines.len(), m.side() / 2);
        for line in &lines {
            assert_eq!(line.chars().count(), m.side());
        }
    }

    /// 终端画能**无损**反解回矩阵。
    ///
    /// 这条证明半块字符的上下映射、静默区、行列对应都没错位 ——
    /// 而错位的表现是"码扫不出来"，靠肉眼看字符画发现不了。
    #[test]
    fn terminal_art_round_trips_to_the_exact_matrix() {
        let m = Matrix::new(PAYLOAD).unwrap();
        let art = m.to_terminal();

        let mut pixels: Vec<Vec<bool>> = Vec::new();
        for line in art.lines() {
            let mut top = Vec::new();
            let mut bottom = Vec::new();
            for ch in line.chars() {
                match ch {
                    '█' => {
                        top.push(true);
                        bottom.push(true);
                    }
                    '▀' => {
                        top.push(true);
                        bottom.push(false);
                    }
                    '▄' => {
                        top.push(false);
                        bottom.push(true);
                    }
                    ' ' => {
                        top.push(false);
                        bottom.push(false);
                    }
                    other => panic!("意外的字符 {other:?}（会打乱像素对齐）"),
                }
            }
            pixels.push(top);
            pixels.push(bottom);
        }

        assert_eq!(pixels.len(), m.side());
        for (y, row) in pixels.iter().enumerate() {
            for (x, got) in row.iter().enumerate() {
                assert_eq!(*got, m.is_dark(x, y), "像素 ({x},{y}) 不一致");
            }
        }
    }

    /// SVG 的**尺寸与矩阵一致**，且深色模块数对得上。
    #[test]
    fn svg_matches_the_matrix() {
        let m = Matrix::new(PAYLOAD).unwrap();
        let svg = m.to_svg();

        assert!(
            svg.starts_with("<svg "),
            "应当是 SVG：{}",
            &svg[..40.min(svg.len())]
        );
        assert!(svg.contains(&format!(r#"viewBox="0 0 {n} {n}""#, n = m.side())));
        // 白底必须有：透明底在深色主题下会变成"黑码黑底"
        assert!(svg.contains(r##"fill="#ffffff""##), "缺白底");
        assert!(svg.contains(r##"fill="#000000""##), "缺深色");
        // 只用 `<rect>` 铺底 + 一条 `<path>`，没有逐模块的 rect
        assert_eq!(svg.matches("<rect").count(), 1, "不该逐模块画 rect");
        assert_eq!(svg.matches("<path").count(), 1);

        // path 里的水平段总长必须等于深色模块数
        let dark_count = (0..m.side())
            .flat_map(|y| (0..m.side()).map(move |x| (x, y)))
            .filter(|(x, y)| m.is_dark(*x, *y))
            .count();
        let d = svg
            .split(r##"d=""##)
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let total: usize = d
            .split('M')
            .skip(1)
            .map(|seg| {
                // 形如 `12 3h4v1h-4z`
                let h = seg.split('h').nth(1).unwrap();
                h.split('v').next().unwrap().parse::<usize>().unwrap()
            })
            .sum();
        assert_eq!(total, dark_count, "SVG 覆盖的模块数与矩阵不一致");
    }

    /// data URL 能解回同一份 SVG。
    #[test]
    fn data_url_decodes_back_to_the_svg() {
        use base64::Engine as _;
        let m = Matrix::new(PAYLOAD).unwrap();
        let url = m.to_svg_data_url();
        let b64 = url.strip_prefix("data:image/svg+xml;base64,").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), m.to_svg());
    }

    /// 载荷过长 ⇒ 报 `Config`（→ 400），不是 panic。
    #[test]
    fn oversized_payload_is_a_config_error() {
        let huge = "x".repeat(10_000);
        let e = match Matrix::new(&huge) {
            Ok(_) => panic!("超长载荷不该编码成功"),
            Err(e) => e,
        };
        assert!(matches!(e, mcloud_core::Error::Config(_)), "实得：{e:?}");
        assert!(e.to_string().contains("二维码"));
    }

    /// 两个平台的载荷画出来当然不同（防止"渲染时把载荷弄丢"）。
    #[test]
    fn different_payloads_produce_different_art() {
        let a = Matrix::new("ver=1").unwrap();
        let b = Matrix::new("ver=2").unwrap();
        assert_ne!(a.to_terminal(), b.to_terminal());
        assert_ne!(a.to_svg(), b.to_svg());
    }
}
