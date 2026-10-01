//! 登录编排：把设备指纹、HTTP 传输、登录状态机与落库串起来。
//!
//! # 为什么需要这一层
//!
//! `mcloud_tasks::login` 里的状态机只依赖 `HttpClient` 与随机源，不碰存储；
//! 而"出码之前先去取机器级设备指纹""登录成功之后落盘"这两步在两边都要写。
//! 放在这里，CLI 与 WebUI 共用同一条路径 —— 否则两处迟早分叉，
//! 而分叉的表现是"网页能登录、命令行不能"这种最难查的差异。
//!
//! # 传输是**裸**的
//!
//! 登录时还没有任何凭据，所以这里用一个**不注入 authorization / jwttoken**
//! 的传输（只是一个 reqwest 客户端）。刻意不复用业务客户端：
//! 业务客户端绑定在账号上，而登录的产物正是账号。

use std::sync::Arc;

use mcloud_core::{Clock, Error, HttpClient, PcDeviceStore, PcPlatform, Result};
use mcloud_crypto::{OsRandom, Random, SharedRandom};
use mcloud_tasks::login::{
    self, AppSmsLogin, LoginResult, PasswordLogin, QrLogin, QrProgress, SmsLogin,
};

/// 重新导出：它是 App 登录 API 的一部分（调用方要 match 它的分支）。
///
/// 经本层转出，好让 `mcloud-server` 不必直接依赖 `mcloud-upstream`
/// —— 服务端只该看到组装层给出的东西。
pub use mcloud_upstream::app_login::AppSendSmsOutcome;
use mcloud_upstream::login::{self as proto, LoginDataDecryptor};

/// 登录服务：一个进程一份，服务所有登录请求。
pub struct LoginService {
    /// 裸传输（不注入任何凭据）。
    http: Arc<dyn HttpClient>,
    store: Arc<dyn PcDeviceStore>,
    /// 落库用的账号存储。
    accounts: Arc<dyn mcloud_core::AccountStore>,
    clock: Arc<dyn Clock>,
    rng: SharedRandom,
}

impl LoginService {
    pub fn new(
        http: Arc<dyn HttpClient>,
        store: Arc<dyn PcDeviceStore>,
        accounts: Arc<dyn mcloud_core::AccountStore>,
        clock: Arc<dyn Clock>,
        rng: Box<dyn Random>,
    ) -> Self {
        Self {
            http,
            store,
            accounts,
            clock,
            rng: SharedRandom::new(rng),
        }
    }

    /// 用 OS 熵源组装一个（生产路径）。
    pub fn with_os_random(
        http: Arc<dyn HttpClient>,
        store: Arc<dyn PcDeviceStore>,
        accounts: Arc<dyn mcloud_core::AccountStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self::new(http, store, accounts, clock, Box::new(OsRandom::new()))
    }

    /// 这台机器的 PC 设备指纹（没有就生成并落盘）。
    ///
    /// ⚠ **别拿它去出码或登录**：出码走 `Self::fresh_device`（每张码现生成
    /// 一枚），四条登录路径走 `Self::device_for_account`（按账号）。
    /// 这里读到的机器级那一枚只服务"查看 / 更换"两个入口
    /// （`GET /api/login/device`、`POST /api/login/device/rotate`）。
    pub async fn pc_device_id(&self) -> Result<String> {
        let mut rng = self.rng.clone();
        proto::load_or_create_pc_device(&*self.store, &mut rng).await
    }

    /// 换掉机器级的那一枚。
    ///
    /// ⚠ **不作废任何扫码会话**：出码用的是每码现生成的那枚，与这里无关；
    /// 也不影响任何账号 —— 登录按账号取设备。它改变的是随后
    /// `GET /api/login/device` 读到的值。
    pub async fn rotate_pc_device_id(&self) -> Result<String> {
        let mut rng = self.rng.clone();
        let fresh = proto::new_pc_device_id(&mut rng);
        self.store.set_pc_device_id(&fresh).await?;
        Ok(fresh)
    }

    /// 给**这个账号**取一枚 PC 设备指纹。
    ///
    /// # 一枚指纹只属于一个账号
    ///
    /// 在此之前四条登录路径都取那个**全局**文件（`data/pc_device.txt`），
    /// 于是这台机器上所有账号**共用一枚** —— 上游看到的是
    /// "一台设备登了多个账号"，那正是多账号最容易被关联的地方。
    ///
    /// * 该账号已有一枚、且**没有别的账号在共用它** ⇒ 沿用（设备身份要稳定，
    ///   频繁换设备本身也是风控信号）
    /// * 否则 ⇒ 单独生成一枚。两种情况下会走到这里：**新账号**；
    ///   以及**这一枚还被别人共着** —— 那正是历史遗留的那枚全局值，
    ///   于是老账号下次重新登录时**自然获得**一枚自己的，从此不再共用
    ///
    /// ⚠ 判据刻意用"**是否与别的账号重复**"，而不是"是否等于全局文件里那枚"：
    /// 后者在用户手动换过设备（`login device reset`）之后就失效了，
    /// 而"重复"这件事永远是当前事实。
    async fn device_for_account(&self, id: &mcloud_core::AccountId) -> Result<String> {
        let all = self.accounts.accounts().await?;
        if let Some(mine) = all.iter().find(|a| &a.id == id) {
            let own = mine.device_id.as_str();
            if !own.is_empty()
                && !all
                    .iter()
                    .any(|o| &o.id != id && o.device_id.as_str() == own)
            {
                return Ok(own.to_string());
            }
        }
        let mut rng = self.rng.clone();
        Ok(proto::new_pc_device_id(&mut rng))
    }

    /// 出码那一刻**还不知道是哪个账号** ⇒ 每次出码单独给一枚。
    ///
    /// 扫码的身份就是这台"设备"，而一张码只服务一次登录；共用全局那枚
    /// 会让第二个扫码的账号和第一个撞上同一台"设备"。
    async fn fresh_device(&self) -> Result<String> {
        let mut rng = self.rng.clone();
        Ok(proto::new_pc_device_id(&mut rng))
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// 出码。**零网络请求**（二维码是本地拼的）。
    pub async fn start_qr(&self, platform: PcPlatform) -> Result<QrLogin> {
        let device_id = self.fresh_device().await?;
        let mut rng = self.rng.clone();
        Ok(QrLogin::start(
            platform,
            device_id,
            &mut rng,
            self.clock.now_ms(),
        ))
    }

    /// 轮询一次扫码会话。
    pub async fn poll_qr(&self, qr: &mut QrLogin, tid: &str) -> Result<QrProgress> {
        qr.poll(
            &*self.http,
            tid,
            &LoginDataDecryptor::production(),
            self.clock.now_ms(),
        )
        .await
    }

    /// 扫码成功后落库。
    pub async fn finish_qr(
        &self,
        qr: &QrLogin,
        user_info: &serde_json::Value,
    ) -> Result<mcloud_core::Account> {
        let account =
            login::account_from_login(user_info, qr.client().platform(), qr.client().device_id())?;
        login::persist_login(
            &*self.accounts,
            &LoginResult {
                account: account.clone(),
            },
        )
        .await?;
        Ok(account)
    }

    /// 开始短信登录。
    pub async fn start_sms(&self, platform: PcPlatform, phone: &str) -> Result<SmsLogin> {
        let id = mcloud_core::AccountId::new(mcloud_upstream::account_id_for(phone));
        let device_id = self.device_for_account(&id).await?;
        let mut rng = self.rng.clone();
        Ok(SmsLogin::start(platform, device_id, phone, &mut rng))
    }

    /// 短信登录成功后落库。
    pub async fn finish_sms(
        &self,
        sms: &SmsLogin,
        user_info: &serde_json::Value,
    ) -> Result<mcloud_core::Account> {
        let account = login::account_from_login(
            user_info,
            sms.client().platform(),
            sms.client().device_id(),
        )?;
        login::persist_login(
            &*self.accounts,
            &LoginResult {
                account: account.clone(),
            },
        )
        .await?;
        Ok(account)
    }

    /// 账密登录（一次调用走完"登录 + 落库"）。
    pub async fn login_with_password(
        &self,
        platform: PcPlatform,
        username: &str,
        password: &str,
        tid: &str,
    ) -> Result<mcloud_core::Account> {
        let id = mcloud_core::AccountId::new(mcloud_upstream::account_id_for(username));
        let device_id = self.device_for_account(&id).await?;
        let pw = PasswordLogin::new(platform, device_id);
        let user_info = pw
            .login(
                &*self.http,
                tid,
                username,
                password,
                &LoginDataDecryptor::production(),
            )
            .await?;
        let account = login::account_from_login(&user_info, platform, pw.client().device_id())?;
        login::persist_login(
            &*self.accounts,
            &LoginResult {
                account: account.clone(),
            },
        )
        .await?;
        Ok(account)
    }

    pub fn http(&self) -> &dyn HttpClient {
        &*self.http
    }

    /// 生成一个链路追踪 id（`x-yun-tid`）。
    ///
    /// 每次请求都该是新的，但**同一次轮询会话**内保持一致更像真实客户端
    /// （客户端是在会话开始时生成一个）。
    pub fn new_tid(&self) -> String {
        let mut rng = self.rng.clone();
        rng.uuid_v4()
    }

    // ── App 端（手机客户端）登录 ──────────────────────────────────────

    /// 开始一次 App 端短信登录。
    ///
    /// # 复用同一个设备号
    ///
    /// App 与 PC 共用这台机器的 `device_id`：它就是一个 32 位大写 hex 的
    /// 客户端标识，两种形态都收。**不**为 App 另造一个 ——
    /// 那样同一个物理设备会在上游留下两个身份，而"为什么任务里的设备号
    /// 和登录时的不一样"会变成一个很难查的问题。
    ///
    /// ⚠ 与 PC 端不同，App 的设备串里 `x-DeviceInfo` 与 `x-yun-client-info`
    /// **不相等**（见 `mcloud_upstream::app_login`）。
    pub async fn start_app_sms(&self, phone: &str) -> Result<AppSmsLogin> {
        let id = mcloud_core::AccountId::new(mcloud_upstream::account_id_for(phone));
        let device_id = self.device_for_account(&id).await?;
        let mut rng = self.rng.clone();
        Ok(AppSmsLogin::start(device_id, phone, &mut rng))
    }

    /// App 端短信登录成功后落库（`auth` 前缀是 **`mobile`**）。
    pub async fn finish_app_sms(
        &self,
        sms: &AppSmsLogin,
        user_info: &serde_json::Value,
    ) -> Result<mcloud_core::Account> {
        let account = login::account_from_app_login(user_info, sms.device_id())?;
        login::persist_login(
            &*self.accounts,
            &LoginResult {
                account: account.clone(),
            },
        )
        .await?;
        Ok(account)
    }

    /// 现在是**微秒**时间戳（App 头 `x-yun-user-timestamp` 要它）。
    ///
    /// 与 [`Self::now_ms`] 分开而不是乘 1000：调用点写错单位是本条链路
    /// 里最容易发生、又最难看出来的一类错（服务端只当它是排序用的数）。
    pub fn now_micros(&self) -> i64 {
        self.clock.now_ms().saturating_mul(1000)
    }
}

/// 组装一个最小可用的登录传输（不注入任何凭据）。
pub fn bare_transport(timeout_secs: u64) -> Result<Arc<dyn HttpClient>> {
    let client = mcloud_upstream::ReqwestClient::new(mcloud_upstream::ClientProfile {
        timeout_secs,
        ..Default::default()
    })?;
    Ok(Arc::new(client) as Arc<dyn HttpClient>)
}

/// 把"平台"从命令行文本解析出来。
///
/// 默认给 **Windows**：它是抓包归档里唯一有完整证据的一方（mac 的设备串
/// 至今没有实测样本）。但**不静默** —— 调用方要把它回显给用户。
pub fn parse_platform(text: &str) -> Result<PcPlatform> {
    PcPlatform::parse(text)
        .ok_or_else(|| Error::Config(format!("未知平台 {text:?}（可用：windows | macos）")))
}

/// 平台 → 用户可读的名字。
pub fn platform_label(p: PcPlatform) -> &'static str {
    match p {
        PcPlatform::Windows => "Windows",
        PcPlatform::MacOs => "macOS",
    }
}

#[cfg(test)]
mod tests;
