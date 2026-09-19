// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use anyhow::{Context, Result};
use mcloud_app::login;
use mcloud_core::Error as CoreError;
use mcloud_upstream::login::PcPlatform;
use std::path::{Path, PathBuf};

/// 终端观察者：把二维码画出来，把进展印成一行。
pub(crate) struct TerminalObserver {
    pub(crate) device_id: String,
    /// 额外落一张图片（`--qr-out`）。
    pub(crate) qr_out: Option<PathBuf>,
}

impl login::LoginObserver for TerminalObserver {
    fn on_qr(&self, payload: &str) {
        // 图片优先：终端字符画只在等宽字体下成立，实际很容易变形，
        // 而"码扫不出来"会被用户理解成"这软件坏了"。
        if let Some(path) = &self.qr_out {
            match write_qr_svg(payload, path) {
                Ok(()) => println!(
                    "\n二维码已写入图片，**打开它扫描**（比终端字符画可靠）：\n  {}\n",
                    path.display()
                ),
                Err(err) => eprintln!("（写二维码图片失败：{err:#}）"),
            }
        }

        println!("用移动云盘 App 扫描下面的二维码：\n");
        match render_qr(payload) {
            Ok(art) => println!("{art}"),
            Err(err) => println!("（终端画不出二维码：{err}）"),
        }
        // 链接也打出来：有的终端字体/配色会把二维码糊掉，手输链接是兜底
        println!("{payload}");
        println!(
            "设备指纹 {}（换掉它服务端就定位不到本次会话）\n",
            self.device_id
        );
        println!("等待扫码…（Ctrl-C 取消）");
    }

    fn on_progress(&self, progress: &mcloud_capabilities::login::QrProgress) {
        use mcloud_capabilities::login::QrProgress;
        match progress {
            QrProgress::NotScanned => {}
            QrProgress::Scanned => println!("已扫码，请在手机上确认登录"),
            QrProgress::LoggedIn { .. } => {}
            // 限频会反复出现，所以给一句人话 + 说明"码还有效"，
            // 否则用户看到"失败"会以为要重开一张码
            QrProgress::Failed {
                reason: mcloud_capabilities::login::QrFailure::TooFrequent,
            } => println!("被限频（扫码登录过于频繁），等一会儿自动重试；二维码仍然有效，不用重开"),
            QrProgress::Failed { reason } => println!("失败：{reason:?}"),
        }
    }
}

/// 把二维码载荷渲染成终端可打印的字符画。
///
/// 用半高块字符（`Dense1x2`）：同样的信息量只占一半行数，
/// 否则一屏放不下一个足够大的二维码（太小手机扫不出来）。
///
/// **但它只在等宽字体 + 正确行距下成立**，所以同时提供
/// [`write_qr_svg`]：字符画变形的概率远高于"用户以为的偶然"。
pub(crate) fn render_qr(payload: &str) -> Result<String> {
    let code = qrcode::QrCode::new(payload.as_bytes()).context("二维码内容过长或无法编码")?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

/// 把二维码写成 SVG 图片。
///
/// 用 SVG 而不是 PNG：**不用引入图像编码库**（qrcode 自带的渲染器就够了），
/// 而浏览器与 macOS 预览都能直接打开，扫描体验与位图没差别。
pub(crate) fn write_qr_svg(payload: &str, path: &Path) -> Result<()> {
    let code = qrcode::QrCode::new(payload.as_bytes()).context("二维码内容过长或无法编码")?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        // 每模块 8px 左右：太小手机对焦困难，太大一屏放不下
        .min_dimensions(360, 360)
        .quiet_zone(true)
        .build();

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败 {}", parent.display()))?;
    }
    std::fs::write(path, svg).with_context(|| format!("写二维码失败 {}", path.display()))?;
    Ok(())
}

pub(crate) const fn platform_name(platform: PcPlatform) -> &'static str {
    match platform {
        PcPlatform::Windows => "Windows",
        PcPlatform::MacOs => "macOS",
    }
}

/// `mcloud_core::Error` → `anyhow`。
///
/// 只在顶层做这一次转换：内核的错误类型带着"能不能重试、能不能给用户看"的分类，
/// 在这里折成 anyhow 之前，`{err:#}` 打出来的仍然是它的 Display。
pub(crate) fn core_err(err: CoreError) -> anyhow::Error {
    anyhow::Error::new(err)
}
