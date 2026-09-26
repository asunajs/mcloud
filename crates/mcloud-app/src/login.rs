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
use mcloud_tasks::login::{self, LoginResult, PasswordLogin, QrLogin, QrProgress, SmsLogin};
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
    /// ⚠ 必须在**出码之前**取到：二维码的 `dID` 就是它，
    /// 每次重新生成会让服务端定位不到扫码会话。
    pub async fn pc_device_id(&self) -> Result<String> {
        let mut rng = self.rng.clone();
        proto::load_or_create_pc_device(&*self.store, &mut rng).await
    }

    /// 换一台设备（旧的扫码会话会立即失效）。
    pub async fn rotate_pc_device_id(&self) -> Result<String> {
        let mut rng = self.rng.clone();
        let fresh = proto::new_pc_device_id(&mut rng);
        self.store.set_pc_device_id(&fresh).await?;
        Ok(fresh)
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// 出码。**零网络请求**（二维码是本地拼的）。
    pub async fn start_qr(&self, platform: PcPlatform) -> Result<QrLogin> {
        let device_id = self.pc_device_id().await?;
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
        let device_id = self.pc_device_id().await?;
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
        let device_id = self.pc_device_id().await?;
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
