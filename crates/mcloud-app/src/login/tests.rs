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
