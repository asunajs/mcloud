//! 登录编排测试：设备指纹的生成/复用、平台解析、落库。

use std::sync::Mutex;

use async_trait::async_trait;
use mcloud_core::{
    Account, AccountId, Clock, HttpClient, HttpRequest, HttpResponse, PcDeviceStore, Result,
};

use super::*;

#[derive(Default)]
struct FakeDeviceStore {
    value: Mutex<Option<String>>,
    writes: Mutex<Vec<String>>,
}

#[async_trait]
impl PcDeviceStore for FakeDeviceStore {
    async fn pc_device_id(&self) -> Result<Option<String>> {
        Ok(self.value.lock().unwrap().clone())
    }
    async fn set_pc_device_id(&self, device_id: &str) -> Result<()> {
        self.writes.lock().unwrap().push(device_id.to_string());
        *self.value.lock().unwrap() = Some(device_id.to_string());
        Ok(())
    }
}

#[derive(Default)]
struct FakeAccounts {
    saved: Mutex<Vec<Account>>,
    states: Mutex<Vec<(String, serde_json::Value)>>,
}

#[async_trait]
impl mcloud_core::AccountStore for FakeAccounts {
    async fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.saved.lock().unwrap().clone())
    }
    async fn account(&self, id: &AccountId) -> Result<Option<Account>> {
        Ok(self
            .saved
            .lock()
            .unwrap()
            .iter()
            .find(|a| &a.id == id)
            .cloned())
    }
    async fn save_account(&self, account: &Account) -> Result<()> {
        self.saved.lock().unwrap().push(account.clone());
        Ok(())
    }
    async fn remove_account(&self, _id: &AccountId) -> Result<()> {
        Ok(())
    }
    async fn account_state(&self, _id: &AccountId) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()> {
        self.states
            .lock()
            .unwrap()
            .push((id.as_str().to_string(), state.clone()));
        Ok(())
    }
}

#[derive(Default)]
struct NoHttp {
    calls: Mutex<usize>,
}

#[async_trait]
impl HttpClient for NoHttp {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse> {
        *self.calls.lock().unwrap() += 1;
        Ok(HttpResponse {
            status: 200,
            body: "{}".into(),
            headers: Vec::new(),
        })
    }
}

struct FixedClock(i64);
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

fn service(
    devices: Arc<FakeDeviceStore>,
    accounts: Arc<FakeAccounts>,
    http: Arc<dyn HttpClient>,
) -> LoginService {
    LoginService::new(
        http,
        devices,
        accounts,
        Arc::new(FixedClock(1_700_000_000_000)),
        // 固定序列随机源：设备指纹可复现，便于断言形状
        Box::new(mcloud_crypto::SeqRandom::new(1)),
    )
}

/// 首次调用生成并**落盘**；再次调用复用同一个（这是它存在的全部意义）。
#[tokio::test]
async fn device_id_is_generated_once_then_reused() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let svc = service(devices.clone(), accounts, Arc::new(NoHttp::default()));

    let first = svc.pc_device_id().await.unwrap();
    assert_eq!(devices.writes.lock().unwrap().len(), 1, "第一次应当写盘");

    let second = svc.pc_device_id().await.unwrap();
    assert_eq!(first, second, "第二次必须复用，不能重新生成");
    assert_eq!(devices.writes.lock().unwrap().len(), 1, "不该再写一次");

    // 形状：32 位大写 hex + -ENDIN
    assert!(first.ends_with("-ENDIN"), "实得：{first}");
    assert_eq!(first.trim_end_matches("-ENDIN").len(), 32);
}

/// 显式换设备：必须落新值。
#[tokio::test]
async fn rotate_replaces_the_device_id() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let svc = service(devices.clone(), accounts, Arc::new(NoHttp::default()));

    let a = svc.pc_device_id().await.unwrap();
    let b = svc.rotate_pc_device_id().await.unwrap();
    assert_ne!(a, b, "换设备必须换出新值");
    assert_eq!(svc.pc_device_id().await.unwrap(), b, "之后读到的应是新值");
}

/// 出码**不发任何网络请求** —— 这是抓包推翻的直觉假设里最重要的一个。
#[tokio::test]
async fn start_qr_issues_no_network_request() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let http = Arc::new(NoHttp::default());
    let svc = service(devices, accounts, http.clone());

    let qr = svc.start_qr(PcPlatform::Windows).await.unwrap();
    assert_eq!(*http.calls.lock().unwrap(), 0, "出码不该发请求");
    // 二维码里的 dID 就是设备指纹
    assert!(
        qr.qr_payload().contains("-ENDIN"),
        "实得：{}",
        qr.qr_payload()
    );
}

/// 出码的平台差异一路到二维码上。
#[tokio::test]
async fn qr_payload_reflects_the_chosen_platform() {
    let mk = || {
        service(
            Arc::new(FakeDeviceStore::default()),
            Arc::new(FakeAccounts::default()),
            Arc::new(NoHttp::default()),
        )
    };
    let w = mk().start_qr(PcPlatform::Windows).await.unwrap();
    let m = mk().start_qr(PcPlatform::MacOs).await.unwrap();
    assert!(w.qr_payload().contains("ver=8.8.6.20260829"));
    assert!(m.qr_payload().contains("ver=4.2.1"));
    assert_ne!(w.qr_payload(), m.qr_payload());
}

/// 落库：账号与状态都写，且平台进 JSON。
#[tokio::test]
async fn finish_qr_persists_platform() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let svc = service(devices, accounts.clone(), Arc::new(NoHttp::default()));

    let qr = svc.start_qr(PcPlatform::MacOs).await.unwrap();
    let info = serde_json::json!({"account":"19900000001","authToken":"tok"});
    let acc = svc.finish_qr(&qr, &info).await.unwrap();

    assert_eq!(acc.pc_platform, Some(PcPlatform::MacOs));
    assert_eq!(accounts.saved.lock().unwrap().len(), 1);
    let states = accounts.states.lock().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].1["pc_platform"], "macos");
    assert_eq!(
        states[0].1["device_id"],
        qr.client().device_id(),
        "状态里的设备号必须与出码用的一致"
    );
}

/// `tid` 每次不同（同一次会话内由调用方保持）。
#[test]
fn tid_is_fresh_each_call() {
    let svc = service(
        Arc::new(FakeDeviceStore::default()),
        Arc::new(FakeAccounts::default()),
        Arc::new(NoHttp::default()),
    );
    assert_ne!(svc.new_tid(), svc.new_tid());
    assert_eq!(svc.now_ms(), 1_700_000_000_000);
}

#[test]
fn platform_parsing_accepts_expected_spellings_and_rejects_junk() {
    assert_eq!(parse_platform("windows").unwrap(), PcPlatform::Windows);
    assert_eq!(parse_platform("macos").unwrap(), PcPlatform::MacOs);
    assert_eq!(parse_platform("Mac").unwrap(), PcPlatform::MacOs);
    assert_eq!(parse_platform("darwin").unwrap(), PcPlatform::MacOs);

    // 未知平台必须报错，不能静默落成 Windows
    let e = parse_platform("linux").unwrap_err();
    assert!(e.to_string().contains("linux"), "实得：{e}");
}

#[test]
fn platform_labels_are_user_facing() {
    assert_eq!(platform_label(PcPlatform::Windows), "Windows");
    assert_eq!(platform_label(PcPlatform::MacOs), "macOS");
}

// ═══════════════ 一枚指纹只属于一个账号 ═══════════════
//
// 背景：在此之前四条登录路径都取那个**全局**文件（`data/pc_device.txt`），
// 于是这台机器上所有账号共用一枚 —— 上游看到的是"一台设备登了多个账号"。

/// 造一个账号（`auth` 只是占位：这一族测试只看 `device_id` 那一格）。
fn account_with_device(id: &str, device: &str) -> mcloud_core::Account {
    mcloud_core::Account {
        id: AccountId::new(id),
        nickname: None,
        auth: mcloud_core::AuthInfo {
            raw: "raw".into(),
            platform: "pc".into(),
            phone: id.into(),
            token: "tok".into(),
            expire: 0,
        },
        device_id: mcloud_core::DeviceId(device.into()),
        pc_platform: None,
        user_domain_id: None,
        settings: mcloud_core::AccountSettings::default(),
    }
}

/// 两个账号都还背着**那枚历史遗留的全局值**时，后一个必须拿到**自己**的一枚。
///
/// ⭐ 这就是"老账号下次重新登录时自然获得一枚"的那条路径 ——
/// 判据是"这枚还被别人共着"，而不是"它等于全局文件里那枚"。
#[tokio::test]
async fn an_account_sharing_a_device_id_gets_its_own_on_next_login() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let legacy = "AAAA0000000000000000000000000000-ENDIN";
    accounts.saved.lock().unwrap().extend([
        account_with_device("138****0001", legacy),
        account_with_device("138****0002", legacy),
    ]);
    let svc = service(devices, accounts, Arc::new(NoHttp::default()));

    let got = svc
        .device_for_account(&AccountId::new("138****0001"))
        .await
        .unwrap();
    assert_ne!(got, legacy, "还和别人共用着 ⇒ 必须换一枚自己的");
    assert!(!got.is_empty());
}

/// 已经**独享**一枚的账号 ⇒ 沿用。设备身份要稳定，频繁换设备本身也是风控信号。
#[tokio::test]
async fn a_unique_device_id_is_reused() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let mine = "BBBB1111111111111111111111111111-ENDIN";
    accounts.saved.lock().unwrap().extend([
        account_with_device("138****0001", mine),
        account_with_device("138****0002", "CCCC2222222222222222222222222222-ENDIN"),
    ]);
    let svc = service(devices, accounts, Arc::new(NoHttp::default()));

    assert_eq!(
        svc.device_for_account(&AccountId::new("138****0001"))
            .await
            .unwrap(),
        mine
    );
}

/// 全新账号（库里查不到）⇒ 生成一枚。
#[tokio::test]
async fn a_brand_new_account_gets_a_fresh_device_id() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let svc = service(devices, accounts, Arc::new(NoHttp::default()));

    let got = svc
        .device_for_account(&AccountId::new("138****9999"))
        .await
        .unwrap();
    assert!(!got.is_empty());
}

/// 扫码出码时**还不知道是哪个账号** ⇒ 每次一枚新的，不共用全局那枚。
#[tokio::test]
async fn qr_allocates_a_fresh_device_id_per_code() {
    let devices = Arc::new(FakeDeviceStore::default());
    let accounts = Arc::new(FakeAccounts::default());
    let svc = service(devices, accounts, Arc::new(NoHttp::default()));

    let a = svc.start_qr(PcPlatform::Windows).await.unwrap();
    let b = svc.start_qr(PcPlatform::Windows).await.unwrap();
    assert_ne!(
        a.client().device_id(),
        b.client().device_id(),
        "两张码不该冒充同一台设备"
    );
}
