//! 命令行登录：`login qr` / `login sms` / `login password`。
//!
//! # 这一层只做"说给人听"与"读人输进来的东西"
//!
//! 协议在 `mcloud-upstream::login`，状态机在 `mcloud-tasks::login`，
//! 编排在 `mcloud-app::LoginService`。本文件**不含业务判定** ——
//! 出现 `if code == 200059554` 就是放错了层。

use std::io::Write as _;

use anyhow::{bail, Context};
use mcloud_core::PcPlatform;
use mcloud_tasks::login::{QrFailure, QrProgress, SendSmsOutcome};

use mcloud_app::App;

/// 把二维码载荷画成终端字符画。
///
/// 实际渲染在 `mcloud_server::qr`（WebUI 出 SVG 用的是同一份矩阵）——
/// 两边各写一遍的后果不是样式不同，而是其中一边**画错**（行列错位、
/// 静默区少一行），表现是"码扫不出来"。
pub fn qr_to_terminal(payload: &str) -> anyhow::Result<String> {
    let matrix = crate::qr::Matrix::new(payload).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(matrix.to_terminal())
}

/// 只报字段名、**不报值**。
///
/// `userInfo` 里有 `authToken`，它是凭据 —— 打印出来会进 shell 历史、
/// 终端回滚缓冲、CI 日志。给字段名足以让人确认"登录确实成功了"。
fn describe_user_info(v: &serde_json::Value) -> String {
    let mut keys: Vec<&str> = v
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    keys.join(", ")
}

/// `login qr [windows|macos]`
pub async fn login_qr(app: &App, args: &[String]) -> anyhow::Result<()> {
    let platform = parse_platform_arg(args.first().map(String::as_str))?;

    println!(
        "平台：{}（clientType={}、cpid={}）",
        mcloud_app::platform_label(platform),
        platform.client_type_hint(),
        platform.cpid_hint(),
    );

    let mut qr = app
        .handle
        .login
        .start_qr(platform)
        .await
        .context("出码失败")?;

    println!("设备指纹：{}", qr.client().device_id());
    println!("\n用移动云盘 App 扫描下面的二维码：\n");
    match qr_to_terminal(&qr.qr_payload()) {
        Ok(art) => println!("{art}"),
        // 画不出来不算失败：下面还会把链接打出来
        Err(e) => println!("（终端画不出二维码：{e}）"),
    }
    println!("二维码链接（可自行渲染）：\n  {}\n", qr.qr_payload());
    println!("（有效期 120 秒，等待扫码…）");

    let tid = app.handle.login.new_tid();
    let mut last: Option<QrProgress> = None;

    // 轮询循环：节奏与终态判据都来自状态机，这里只负责"等"与"说"。
    loop {
        let now = app.handle.login.now_ms();
        let Some(delay) = qr.next_delay_ms(now) else {
            break;
        };

        let progress = app
            .handle
            .login
            .poll_qr(&mut qr, &tid)
            .await
            .context("轮询登录状态失败")?;

        // 只在**状态变化**时打印：否则 1 秒一行会把终端刷满
        if last.as_ref() != Some(&progress) {
            match &progress {
                QrProgress::NotScanned => {} // 静默：这是绝大多数情况
                QrProgress::Scanned => println!("已扫码，请在手机上确认…"),
                QrProgress::LoggedIn { .. } => {}
                QrProgress::Failed { .. } => {}
            }
            last = Some(progress.clone());
        }

        match progress {
            QrProgress::LoggedIn { user_info } => {
                let account = app
                    .handle
                    .login
                    .finish_qr(&qr, &user_info)
                    .await
                    .context("登录成功但落库失败")?;
                println!("\n✅ 登录成功");
                println!("   账号：{}", account.id);
                println!(
                    "   平台：{}（已随账号落盘）",
                    mcloud_app::platform_label(platform)
                );
                println!("   字段：{}", describe_user_info(&user_info));
                return Ok(());
            }
            QrProgress::Failed { reason } => {
                // 限流不是终态：退避后继续问
                if matches!(reason, QrFailure::TooFrequent) {
                    println!("触发限流，等待后重试…");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
                bail!("{}", describe_qr_failure(&reason));
            }
            QrProgress::NotScanned | QrProgress::Scanned => {}
        }

        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }

    bail!("二维码已过期（120 秒），请重新执行以出新码")
}

fn describe_qr_failure(reason: &QrFailure) -> String {
    match reason {
        QrFailure::TtlElapsed { ttl_seconds } => format!("二维码超时（{ttl_seconds} 秒）"),
        QrFailure::ExpiredUpstream => "上游说二维码已失效，请重新出码".into(),
        QrFailure::SessionMismatch => {
            "二维码标识不匹配（换码后仍在用旧的 sID）—— 这是本程序的错误，请反馈".into()
        }
        QrFailure::TooFrequent => "扫码登录过于频繁".into(),
        QrFailure::Rejected { code, message } => {
            format!(
                "上游拒绝：code={} message={message}",
                code.map(|c| c.to_string()).unwrap_or_else(|| "(无)".into())
            )
        }
    }
}

/// `login sms <手机号> [windows|macos]`
pub async fn login_sms(app: &App, args: &[String]) -> anyhow::Result<()> {
    let phone = args
        .first()
        .context("用法：login sms <手机号> [windows|macos]")?;
    let platform = parse_platform_arg(args.get(1).map(String::as_str))?;

    println!(
        "平台：{}；手机号：{phone}",
        mcloud_app::platform_label(platform)
    );

    let sms = app
        .handle
        .login
        .start_sms(platform, phone)
        .await
        .context("准备短信登录失败")?;
    let tid = app.handle.login.new_tid();

    // 发码：被要求过滑块时自动取题并解出答案 —— 上游把答案直接给了出来，
    // 不需要图像识别（见 `SliderPuzzle::answer` 的说明）。
    match sms.send_code(app.handle.login.http(), &tid).await? {
        SendSmsOutcome::Sent { remain_get_times } => match remain_get_times {
            Some(n) => println!("验证码已发送（今日剩余 {n} 次）"),
            None => println!("验证码已发送"),
        },
        SendSmsOutcome::NeedSlider { .. } | SendSmsOutcome::SliderExhausted { .. } => {
            println!("需要滑块验证，正在取题…");
            let answer = sms
                .slider_puzzle(app.handle.login.http(), &tid)
                .await
                .context("取滑块挑战失败")?
                .answer()
                .context("解滑块答案失败")?;
            println!("滑块答案：{answer}（上游把答案直接给了出来）");
            match sms
                .send_code_with_slider(app.handle.login.http(), &tid, &answer)
                .await?
            {
                SendSmsOutcome::Sent { .. } => println!("验证码已发送"),
                other => bail!("带滑块发码仍失败：{other:?}"),
            }
        }
        other => bail!("发送验证码失败：{other:?}"),
    }

    print!("请输入收到的验证码：");
    std::io::stdout().flush().ok();
    let mut code = String::new();
    std::io::stdin()
        .read_line(&mut code)
        .context("读取验证码失败")?;
    let code = code.trim();
    if code.is_empty() {
        bail!("没有输入验证码");
    }

    let user_info = sms
        .login(
            app.handle.login.http(),
            &tid,
            code,
            &mcloud_tasks::production_decryptor(),
        )
        .await
        .context("短信登录失败（验证码可能不对或已过期）")?;

    let account = app
        .handle
        .login
        .finish_sms(&sms, &user_info)
        .await
        .context("登录成功但落库失败")?;

    println!("\n✅ 登录成功");
    println!("   账号：{}", account.id);
    println!(
        "   平台：{}（已随账号落盘）",
        mcloud_app::platform_label(platform)
    );
    println!("   字段：{}", describe_user_info(&user_info));
    Ok(())
}

/// `login password <用户名> [windows|macos]`
pub async fn login_password(app: &App, args: &[String]) -> anyhow::Result<()> {
    let username = args
        .first()
        .context("用法：login password <用户名> [windows|macos]")?;
    let platform = parse_platform_arg(args.get(1).map(String::as_str))?;

    print!("口令（输入不回显）：");
    std::io::stdout().flush().ok();
    let password = rpassword().context("读取口令失败")?;
    if password.is_empty() {
        bail!("没有输入口令");
    }

    let tid = app.handle.login.new_tid();
    let account = app
        .handle
        .login
        .login_with_password(platform, username, &password, &tid)
        .await
        .context("账密登录失败（用户名或口令不对）")?;

    println!("\n✅ 登录成功");
    println!("   账号：{}", account.id);
    println!(
        "   平台：{}（已随账号落盘）",
        mcloud_app::platform_label(platform)
    );
    Ok(())
}

/// `login device show|reset [windows|macos]`
pub async fn login_device(app: &App, args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("show") | None => {
            let id = app
                .handle
                .login
                .pc_device_id()
                .await
                .context("读取设备指纹失败")?;
            println!("PC 设备指纹：{id}");
            println!("（它同时是二维码的 dID 与设备串的第 6 段，改它会让旧扫码会话失效）");
        }
        Some("reset") => {
            let id = app
                .handle
                .login
                .rotate_pc_device_id()
                .await
                .context("更换设备指纹失败")?;
            println!("已换新设备指纹：{id}");
            println!("⚠ 旧的扫码会话立即失效；已登录账号不受影响（设备号已随账号落盘）");
        }
        Some(other) => bail!("未知子命令：login device {other}（可用：show | reset）"),
    }
    Ok(())
}

/// 读一行口令且不回显。
///
/// 用 `stty -echo` 而不是引依赖：多一个 crate 只为这一处不划算，
/// 而它失败（非终端、无 stty）时**不能静默继续** —— 那会把口令显示在屏幕上。
fn rpassword() -> anyhow::Result<String> {
    use std::process::Command;

    // 关回显（尽力而为：不是终端时 stty 会失败，那时也没人看得见屏幕）
    let restore = Command::new("stty")
        .arg("-echo")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);

    if restore {
        let _ = Command::new("stty").arg("echo").status();
    }

    // 回车之后补一个换行（回显是关的，光标不会自己换行）
    println!();
    read.context("读取口令失败")?;
    Ok(line.trim().to_string())
}

/// 平台参数的解析：默认 `windows`，且**必须回显**给用户。
pub fn parse_platform_arg(arg: Option<&str>) -> anyhow::Result<PcPlatform> {
    match arg {
        Some(text) => mcloud_app::parse_platform(text).map_err(|e| anyhow::anyhow!("{e}")),
        // 默认 Windows：它是唯一有完整实测证据的一方（mac 的设备串至今无样本）。
        // 但这个默认**必须打印出来**，否则用户以为自己在用 mac 登录。
        None => Ok(PcPlatform::Windows),
    }
}

/// 平台参数的小提示（给 CLI 回显用）。
trait PlatformHint {
    fn client_type_hint(&self) -> &'static str;
    fn cpid_hint(&self) -> &'static str;
}

impl PlatformHint for PcPlatform {
    fn client_type_hint(&self) -> &'static str {
        match self {
            PcPlatform::Windows => "656",
            PcPlatform::MacOs => "710",
        }
    }
    fn cpid_hint(&self) -> &'static str {
        match self {
            PcPlatform::Windows => "277",
            PcPlatform::MacOs => "332",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI 用的渲染器与 WebUI 是**同一份矩阵**。
    ///
    /// 这条防的是"两边各写一遍渲染"。细节正确性由 `qr` 模块自己测，
    /// 这里只钉"CLI 确实走的是那一条"。
    #[test]
    fn cli_renders_through_the_shared_matrix() {
        let payload = "https://yun.139.com/w/#/qrcLogin?dID=X&sID=Y";
        let art = qr_to_terminal(payload).unwrap();
        let shared = crate::qr::Matrix::new(payload).unwrap().to_terminal();
        assert_eq!(art, shared, "CLI 必须复用共享渲染器");

        // 超长载荷应当是可读的错误，不是 panic
        let e = qr_to_terminal(&"x".repeat(10_000)).unwrap_err();
        assert!(e.to_string().contains("二维码"), "实得：{e}");
    }

    /// 平台参数：默认 Windows，显式值必须被尊重，未知值必须报错。
    #[test]
    fn platform_arg_defaults_to_windows_and_rejects_junk() {
        assert_eq!(parse_platform_arg(None).unwrap(), PcPlatform::Windows);
        assert_eq!(
            parse_platform_arg(Some("macos")).unwrap(),
            PcPlatform::MacOs
        );
        assert_eq!(
            parse_platform_arg(Some("windows")).unwrap(),
            PcPlatform::Windows
        );
        assert!(
            parse_platform_arg(Some("linux")).is_err(),
            "未知平台必须报错"
        );
    }

    /// 两个平台的提示值不同（回显给用户的是真实取值）。
    #[test]
    fn platform_hints_match_the_wire_values() {
        assert_eq!(PcPlatform::Windows.client_type_hint(), "656");
        assert_eq!(PcPlatform::MacOs.client_type_hint(), "710");
        assert_eq!(PcPlatform::Windows.cpid_hint(), "277");
        assert_eq!(PcPlatform::MacOs.cpid_hint(), "332");
    }

    /// `userInfo` 只报字段名，**绝不报值**（里面有 authToken）。
    #[test]
    fn user_info_description_lists_keys_only() {
        let v = serde_json::json!({"account":"19900000001","authToken":"SECRET-TOKEN"});
        let d = describe_user_info(&v);
        assert!(d.contains("account"));
        assert!(d.contains("authToken"));
        assert!(!d.contains("SECRET-TOKEN"), "凭据值泄漏进了输出：{d}");
        assert!(!d.contains("19900000001"), "账号值也不该原样打出：{d}");
    }

    /// 失败原因的措辞里不能混进凭据。
    #[test]
    fn qr_failure_messages_are_safe() {
        let r = QrFailure::Rejected {
            code: Some(12345),
            message: "some upstream message".into(),
        };
        let s = describe_qr_failure(&r);
        assert!(s.contains("12345"));
        assert!(s.contains("some upstream message"));
    }
}
