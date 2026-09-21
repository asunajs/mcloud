// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::account::*;
use crate::args::*;
use crate::support::*;
use anyhow::Result;
use mcloud_app::{app_login, device, login};
use mcloud_core::{AccountId, Clock, account_label};
use mcloud_io::{ReqwestClient, SystemClock, SystemRandom};
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::{AppDevice, AppProfile};
use mcloud_upstream::app_login::AppLoginCrypto;
use mcloud_upstream::login::{LoginDataDecryptor, PcPlatform};

pub(crate) async fn login_qr(args: QrArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let platform: PcPlatform = args.platform.into();
    let mut rng = SystemRandom;
    let device_id = device::load_or_create(&args.device_file, &mut rng).map_err(core_err)?;

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);
    let observer = TerminalObserver {
        device_id: device_id.clone(),
        qr_out: args.qr_out.clone(),
    };

    println!(
        "正在以 {} 客户端身份登录（设备 {}）",
        platform_name(platform),
        device_id
    );

    if args.no_wait {
        let qr = mcloud_capabilities::login::QrLogin::start(
            platform,
            device_id.as_str(),
            &mut rng,
            SystemClock.now(),
        );
        println!("\n{}", render_qr(&qr.qr_payload())?);
        println!("{}", qr.qr_payload());
        return Ok(());
    }

    let outcome = login::qr_login(
        &store,
        &upstream,
        login::LoginContext::new(scope_for_new_accounts(&owner), platform, &device_id)
            .with_ttl(args.ttl_seconds),
        &LoginDataDecryptor::production(),
        &SystemClock,
        &mut rng,
        &observer,
    )
    .await
    .map_err(core_err)?;

    println!("\n登录成功，账号已保存：{}", outcome.summary());
    println!("（凭据只落在本地库，不会打印出来）");
    // 打印**字段名**（不打印值）：上游的响应形状与文档示例并不总是一致
    // （实测就少了一个 `userId`），而字段名不是秘密、值才是。
    let mut fields: Vec<&str> = outcome
        .user_info
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    fields.sort_unstable();
    if !fields.is_empty() {
        println!("登录返回的字段：{}", fields.join(", "));
    }
    Ok(())
}

pub(crate) async fn login_sms(args: SmsArgs) -> Result<()> {
    match args.client {
        ClientArg::Pc => login_sms_pc(args).await,
        ClientArg::App => login_sms_app(args).await,
    }
}

pub(crate) async fn login_sms_pc(args: SmsArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let platform: PcPlatform = args.platform.into();
    let mut rng = SystemRandom;
    let device_id = device::load_or_create(&args.device_file, &mut rng).map_err(core_err)?;

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);

    // 发码（被要求滑块时自动解答案重试）。这段循环在 `mcloud_app::login` 里，
    // 与 WebUI 共用同一份 —— 协议语义不该各写一遍。
    //
    // 但 `--code` 已经带着码来了就不再发：那会白刷一次今日配额，
    // 而且 PC 端发码还要连带解一次滑块。发码与登录是两次独立调用，有码时只跑后一半。
    let code = match &args.code {
        Some(code) => code.clone(),
        None => {
            let sent = login::sms_send_code(&upstream, platform, &device_id, &args.phone, &mut rng)
                .await
                .map_err(core_err)?;
            match sent.remain_get_times {
                Some(remain) => println!("验证码已发送（今日剩余 {remain} 次）"),
                None => println!("验证码已发送"),
            }
            if sent.solved_slider {
                println!("（被风控要求滑块，已自动解出缺口位置并重发）");
            }
            read_secret("请输入收到的 6 位验证码: ")?
        }
    };

    let outcome = login::sms_login(
        &store,
        &upstream,
        login::LoginContext::new(scope_for_new_accounts(&owner), platform, &device_id),
        login::SmsCredential {
            phone: &args.phone,
            code: code.trim(),
        },
        &LoginDataDecryptor::production(),
        &SystemClock,
        &mut rng,
    )
    .await
    .map_err(core_err)?;

    println!("\n登录成功，账号已保存：{}", outcome.summary());
    println!("（凭据只落在本地库，不会打印出来）");
    let mut fields: Vec<&str> = outcome
        .user_info
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    fields.sort_unstable();
    if !fields.is_empty() {
        println!("登录返回的字段：{}", fields.join(", "));
    }
    Ok(())
}

/// App（iOS）端的验证码登录。
///
/// # 与 PC 那条路最大的不同：**短信是那个 H5 页面发的**
///
/// 被要求滑块时，PC 端可以取图 → 算坐标 → 重发（所以 PC 有个自动重试循环）。
/// App 端没有这条路：原生侧只是把 H5 打开，**短信由 H5 发**，
/// 回调之后直接进 `thirdlogin`。所以这里把地址打给用户，
/// 并明确"不要再重新执行本命令"——否则会再要一次滑块、把配额刷掉。
///
/// # 补充：本函数里那个自动重试环**不是** App 自己的滑块被自动化了
///
/// `app_login::app_send_sms_code` 的环走的是 **PC 端那套**取题与编码
/// （`/user/verfycode` + `puzzle_verfycode_encrypt`，只是拿 App 的头发出去），
/// 属**围魏救赵**。App 端自身的滑块自动化**并未实现**，真机走的仍是上面那条 H5 路。
/// 所以别把"这里过掉了"读成"App 的滑块已攻克"。
pub(crate) async fn login_sms_app(args: SmsArgs) -> Result<()> {
    let store = open_local(&args.db)?;
    let owner = preferred_owner(&store).await.map_err(core_err)?;
    let mut rng = SystemRandom;

    let device_id =
        device::load_or_create_app_device_id(&args.app_device_file, &mut rng).map_err(core_err)?;
    // `deviceShort` **每次运行现生成**：实测同一 `deviceId` 在不同时间点换过 short，
    // 所以它不是一个可以持久化的"每安装指纹"（见 `mcloud_app::device` 的说明）。
    // 传 `--device-short` 可以把它钉住，用于把"发码 → 登录"两半连成一次会话。
    let device_short = match &args.device_short {
        Some(short) => short.to_ascii_uppercase(),
        None => device::new_app_device_short(&mut rng),
    };
    let profile = AppProfile::new(AppDevice::ipad(&device_id, &device_short));
    // 读回填与写落库**必须同一个范围**：范围不同会出现"读到某个账号的用户域、
    // 却把它发给另一个账号"，那比不带 `x-yun-uni` 更糟。
    let scope = scope_for_new_accounts(&owner);
    // 登录过一次之后 `userDomainId` 就在库里了，第二次起要带上（真机也一样）。
    // 首次登录查不到，档案保持不带 —— 见 `app_login::profile_with_stored_domain_id`。
    // 这里从手机号推 id：与下面 `app_sms_login` 落库时用的是同一个 `account_label`，
    // 所以"读回填"与"写落库"指的是同一行。
    let profile = app_login::profile_with_stored_domain_id(
        &store,
        scope,
        profile,
        &AccountId::new(account_label(None, &args.phone)),
    )
    .await
    .map_err(core_err)?;

    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);

    println!("正在以 iOS App 身份登录（deviceId {device_id}）");

    // 有码就不再发码。App 端这一步的代价比 PC 更大：`getSmsCode` 会再要一次滑块，
    // 而白刷一次今日配额；码已经到手时它纯粹是浪费。
    let code = match &args.code {
        Some(code) => code.clone(),
        None => {
            match app_login::app_send_sms_code(&upstream, &profile, &args.phone, &SystemClock)
                .await
                .map_err(core_err)?
            {
                app_login::AppCodeOutcome::Sent {
                    remain_get_times,
                    solved_slider,
                } => {
                    match remain_get_times {
                        Some(remain) => println!("验证码已发送（今日剩余 {remain} 次）"),
                        None => println!("验证码已发送"),
                    }
                    if solved_slider {
                        println!(
                            "（被风控要求滑块，已取图解出缺口坐标并自动重发 —— 全程没有开浏览器）"
                        );
                    }
                }
                // 只有**自动过滑块也失败**时才会走到这里。把坐标一并打出来：
                // 万一是服务端要的答案口径变了，这一行就是唯一线索。
                app_login::AppCodeOutcome::NeedCaptcha {
                    url,
                    captcha_type,
                    attempts,
                    last_answer,
                } => {
                    println!(
                        "自动过滑块失败（已试 {attempts} 次，captchaType={}，最后发上去的坐标 {last_answer:?}）。",
                        captcha_type.map_or("未知".to_string(), |t| t.to_string())
                    );
                    println!("改走兜底路径：请在浏览器打开下面这个地址，拖动拼图完成验证：");
                    println!("\n{url}\n");
                    println!(
                        "（**短信由该页面发出**，完成拖动后回到这里继续。不要再重新执行本命令）"
                    );
                }
            }

            if args.send_only {
                // 把当次的设备短标识打出来：下一步用 `--device-short` 传回来，
                // 两半才会被上游看成同一台设备形态。
                println!("\n本次设备短标识（继续时用 --device-short 传回）：{device_short}");
                println!("发码完成，已按 --send-only 退出。");
                return Ok(());
            }

            read_secret("请输入收到的 6 位验证码: ")?
        }
    };

    let outcome = app_login::app_sms_login(
        &store,
        &upstream,
        app_login::AppLoginContext {
            scope,
            profile: &profile,
            credential: login::SmsCredential {
                phone: &args.phone,
                code: code.trim(),
            },
        },
        &AppLoginCrypto::production(),
        &SystemClock,
        &mut rng,
    )
    .await
    .map_err(core_err)?;

    println!("\n登录成功，账号已保存：{}", outcome.summary());
    println!("（凭据只落在本地库，不会打印出来）");
    let mut fields: Vec<&str> = outcome
        .user_info
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    fields.sort_unstable();
    if !fields.is_empty() {
        println!("登录返回的字段：{}", fields.join(", "));
    }
    Ok(())
}
