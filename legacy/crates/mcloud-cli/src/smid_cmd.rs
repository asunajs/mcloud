// 由 main.rs 拆出：CLI 的参数定义与各子命令实现。详见 main.rs 顶部的模块划分。
use crate::account::*;
use crate::support::*;
use anyhow::{Result, bail};
use mcloud_app::smid;
use mcloud_core::{Account, AccountId, Error as CoreError, Random, Scope, StateStore, md5_hex};
use mcloud_io::{ReqwestClient, SqliteStore, SystemClock, SystemRandom};
use mcloud_upstream::Upstream;
use mcloud_upstream::smid::SmFingerprint;
use std::path::Path;

// ═══════════════════════════════════════════════════════════════════════════
// 设备特征（smid）
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn smid_show(db: &Path) -> Result<()> {
    let store = open_local(db)?;
    let accounts = store.list_accounts(Scope::Local).await.map_err(core_err)?;
    if accounts.is_empty() {
        println!("还没有云盘账号。先跑：mcloud login qr");
        return Ok(());
    }

    let mut bound = 0;
    for account in &accounts {
        let title = match account.nickname.as_deref() {
            Some(nick) => format!("{}（{nick}）", account.id.as_str()),
            None => account.id.as_str().to_string(),
        };
        println!("{title}");

        let state = store
            .get_smid_state(Scope::Local, &account.id)
            .await
            .map_err(core_err)?;
        match state.as_ref().and_then(|s| s.fingerprint_json.as_deref()) {
            Some(raw) => {
                bound += 1;
                println!("  设备特征  {}", fingerprint_line(raw));
            }
            None => println!(
                "  设备特征  未绑定（下一次 `mcloud signin` 会自动绑一个并上报，\
                 也可以现在就做：`mcloud smid refresh --account …`）"
            ),
        }
        match state.and_then(|s| s.device_id) {
            Some(id) => println!("  deviceId  {}", mask_device_id(&id)),
            None => println!("  deviceId  还没有（上报后由风控服务给）"),
        }
    }
    println!("\n共 {} 个账号，已绑定设备特征 {bound} 个", accounts.len());
    Ok(())
}

pub(crate) async fn smid_reset(db: &Path, account: &str, report: bool) -> Result<()> {
    let store = open_local(db)?;
    let id = require_account(&store, account).await?;

    let mut rng = SystemRandom;
    let binding = smid::rotate_fingerprint(&store, Scope::Local, &id, &mut rng)
        .await
        .map_err(core_err)?;
    let json = binding.fingerprint.to_json_string().map_err(core_err)?;
    println!("已换上新的设备特征：{}", fingerprint_line(&json));
    println!("旧的 deviceId 同时作废（新长相配旧 id 会被认成同一台设备在撒谎）");

    if report {
        let device_id = report_device_id(&store, &id).await?;
        println!("已上报，新的 deviceId：{}", mask_device_id(&device_id));
    } else {
        println!("没有上报：下一次真正用到 deviceId 时会自动换一个新的");
    }
    Ok(())
}

pub(crate) async fn smid_refresh(db: &Path, account: &str) -> Result<()> {
    let store = open_local(db)?;
    let id = require_account(&store, account).await?;
    let device_id = report_device_id(&store, &id).await?;
    println!("已上报，新的 deviceId：{}", mask_device_id(&device_id));
    Ok(())
}

/// 上报换一个新的 `deviceId`（特征不变 —— 换特征走 `smid_reset`）。
pub(crate) async fn report_device_id(store: &SqliteStore, id: &AccountId) -> Result<String> {
    let client = ReqwestClient::new().map_err(core_err)?;
    let upstream = Upstream::new(&client);
    let mut rng = SystemRandom;
    smid::report_device_id(&upstream, store, Scope::Local, id, &SystemClock, &mut rng)
        .await
        .map_err(core_err)
}

/// 解析签到链路要用的那个 H5 `deviceId`（89 字符 base64、`B` 前缀）。
///
/// 顺序：**显式给的** → 库里存的 `smid_device_id`（没有就上报一次换一个）。
///
/// # 为什么两条路都不需要浏览器
///
/// 第二条走 [`mcloud_app::smid`]，它打的
/// `POST https://slw.h5cmpassport.com:9090/deviceprofile/v4` 与签到页
/// `fp.min.js` 是**同一个接口**：host、`organization`、`appId`、RSA 公钥
/// 逐字相同（签到页 HTML 里的 `window._smConf` 就是这份配置）。
/// 两边拿回来的都是 `"B" + detail.deviceId`，**同一个东西**。
///
/// ⇒ 以前那条"先用官方 App 打开一次签到页、再拿 `scripts/h5-device-id.py`
/// 从 WebView 的 `localStorage` 里读"的路子**不再需要**。
pub(crate) async fn resolve_device_id<C: mcloud_core::HttpClient>(
    upstream: &Upstream<'_, C>,
    store: &SqliteStore,
    scope: Scope<'_>,
    account_id: &AccountId,
    explicit: Option<&str>,
    rng: &mut impl Random,
) -> Result<String, CoreError> {
    match explicit {
        Some(id) => Ok(id.to_string()),
        None => smid::device_id(upstream, store, scope, account_id, &SystemClock, rng).await,
    }
}

/// `--account` 必须指向一个已有的账号。
///
/// 打错一个字就会凭空给一个不存在的账号绑上特征 —— 那一行永远没人读得到，
/// 只会留在库里让人困惑。所以查不到就直接报错，并把现有账号列出来。
pub(crate) async fn require_account(store: &SqliteStore, account: &str) -> Result<AccountId> {
    let id = AccountId::new(account);
    if store
        .get_account(Scope::Local, &id)
        .await
        .map_err(core_err)?
        .is_some()
    {
        return Ok(id);
    }
    let known: Vec<String> = store
        .list_accounts(Scope::Local)
        .await
        .map_err(core_err)?
        .iter()
        .map(|a: &Account| a.id.as_str().to_string())
        .collect();
    bail!(
        "没有账号 {account}（库里现有：{}）",
        if known.is_empty() {
            "还没有账号".to_string()
        } else {
            known.join(", ")
        }
    )
}

/// 设备特征的**摘要**行：能认出"是哪台设备"，但不把识别位整份抄进终端。
///
/// `canvas` 那一段就是这份特征的识别位（真机上由渲染结果决定、稳定不变），
/// 所以只进 `fp#` 那个摘要里。UA 不打：本项目只用一个公开常量 UA，
/// 打出来不带任何区分信息（`mcloud-upstream::smid::CAIYUN_ANDROID_WEB_UA`）。
pub(crate) fn fingerprint_line(raw: &str) -> String {
    let digest = &md5_hex(raw.as_bytes())[..8];
    match SmFingerprint::from_json_str(raw) {
        Ok(fp) => format!(
            "fp#{digest}  {} · {}×{} · dpr={} · 触屏 {} 点 · rtt {} · cpu {}",
            fp.platform,
            fp.screen_width,
            fp.screen_height,
            fp.dpr,
            fp.max_touch_points,
            fp.connection_rtt,
            fp.cpucount,
        ),
        Err(_) => {
            format!("fp#{digest}  （库里那串读不出来：`mcloud smid reset --account …` 重新绑定）")
        }
    }
}

/// `deviceId` 的展示形态：只留首尾。
///
/// 它是第三方风控服务给这台设备的标识 —— 完整值留在库里即可，
/// 终端上那一眼通常是被截图/被贴进 issue 的，所以按卡片号的规矩来。
pub(crate) fn mask_device_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    let keep = chars.len().min(4);
    let head = chars.first().copied().unwrap_or('-');
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("{head}…{tail}（{} 字符）", chars.len())
}
