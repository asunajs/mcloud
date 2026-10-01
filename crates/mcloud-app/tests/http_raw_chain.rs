//! 跨装饰器的 `request_raw` 通路：字节必须一字不差地穿过
//! `SmidClient → H5Client` 到达 leaf。
//!
//! # 为什么要有这一条
//!
//! 两个装饰器都实现了 `request`，而 [`mcloud_core::HttpClient::request_raw`] 的
//! 默认实现调的是 `self.request()` ⇒ 忘了转发**不报错**，只是把 leaf 的有损文本
//! 再编码一次。各自文件里的单元测试只能证明"我这一层转发了"，证明不了
//! "串起来之后字节还在"。这一条把两层串起来、对着一个会给出**非 UTF-8** 字节的
//! 假 leaf 断言。
//!
//! ⚠ 会话主机那条分支今天**明确报错**（见 `mcloud_upstream::h5_client`），
//! 所以这里用**不需要会话**的原生主机走通全程。

use std::sync::{Arc, Mutex};

use mcloud_app::smid::{SmidClient, SmidService};
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountId, AccountSettings, AuthInfo, Clock, DeviceId, HttpClient, HttpRequest,
    HttpResponse, Result, SmidState, SmidStore,
};
use mcloud_crypto::SeqRandom;
use mcloud_upstream::smid::SmFingerprint;
use mcloud_upstream::{H5Client, SessionCache, SessionRecipe};

/// 含 `0x89` / `0xFF` / `0x80`：`String` 装不下，任何有损一跳都改得掉它。
const RAW: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0x00, 0x80, 0xC3,
];

/// 假 leaf：`request_raw` 交回线上原样字节；`request` 故意交回**不同于**它的
/// 有损文本 —— 只要有哪一层落回默认实现，`got == RAW` 就会失败。
#[derive(Default)]
struct Leaf {
    raw_seen: Mutex<Vec<HttpRequest>>,
    text_seen: Mutex<Vec<HttpRequest>>,
}

#[async_trait::async_trait]
impl HttpClient for Leaf {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.text_seen.lock().unwrap().push(req);
        Ok(HttpResponse {
            status: 200,
            body: "\u{FFFD}PNG".into(),
            headers: Vec::new(),
        })
    }

    async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
        self.raw_seen.lock().unwrap().push(req);
        Ok(RAW.to_vec())
    }
}

/// smid 上报用的假传输：**永远打挂**。这一发不该用到它（库里有 id）。
struct NeverCalled;

#[async_trait::async_trait]
impl HttpClient for NeverCalled {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse> {
        unreachable!("库里有 deviceId，不该走 smid 上报")
    }
}

/// 库里已经绑好指纹与 deviceId 的假 store：取 id 这一步**不打网络**。
struct Seeded {
    fingerprint_json: String,
}

#[async_trait::async_trait]
impl SmidStore for Seeded {
    async fn get_smid_state(&self, _id: &AccountId) -> Result<Option<SmidState>> {
        Ok(Some(SmidState {
            fingerprint_json: Some(self.fingerprint_json.clone()),
            device_id: Some("B-FROM-STORE".into()),
        }))
    }

    async fn set_smid_fingerprint(&self, _id: &AccountId, _json: &str) -> Result<()> {
        unreachable!("库里有绑定，这一发不该写库")
    }

    async fn set_smid_device_id(&self, _id: &AccountId, _device_id: &str) -> Result<()> {
        unreachable!("库里有 deviceId，这一发不该写库")
    }
}

fn account() -> Account {
    Account {
        id: AccountId::new("19900000000"),
        nickname: None,
        auth: AuthInfo {
            raw: "cmF3".into(),
            platform: "ios".into(),
            phone: "19900000000".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("d".into()),
        pc_platform: None,
        user_domain_id: None,
        owner: None,
        enabled: true,
        settings: AccountSettings::default(),
    }
}

#[tokio::test]
async fn raw_bytes_survive_both_decorators_to_the_leaf() {
    let leaf = Arc::new(Leaf::default());

    // H5Client：原生主机（不需要会话）⇒ 直通。会话配方只是构造参数，这里用不到。
    let h5 = Arc::new(H5Client::new(
        leaf.clone(),
        account(),
        SessionRecipe::SignInPage,
        Arc::new(SessionCache::new()),
    ));

    // SmidClient：库里已有指纹 + deviceId ⇒ 取 id 不打网络（上报替身永远打挂）。
    let fingerprint = SmFingerprint::generate(&mut SeqRandom::new(7))
        .to_json_string()
        .unwrap();
    let smid = Arc::new(SmidService::new(
        Arc::new(NeverCalled),
        Arc::new(Seeded {
            fingerprint_json: fingerprint,
        }),
        Arc::new(FixedClock(0)) as Arc<dyn Clock>,
        Box::new(SeqRandom::new(1)),
    ));
    let client = SmidClient::new(h5, smid, AccountId::new("19900000000"));

    let got = client
        .request_raw(
            HttpRequest::get("https://user-njs.yun.139.com/user/querySpecTokenV2")
                .header("x-probe", "1"),
        )
        .await
        .unwrap();

    assert_eq!(got, RAW, "字节必须一字不差地穿过两层装饰器抵达 leaf");
    assert!(
        leaf.text_seen.lock().unwrap().is_empty(),
        "绝不能落到有损的 request() 那条路"
    );

    let seen = leaf.raw_seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "只该有一发");
    assert_eq!(
        seen[0].url,
        "https://user-njs.yun.139.com/user/querySpecTokenV2"
    );
    let header = |name: &str| {
        seen[0]
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(
        header("deviceid"),
        Some("B-FROM-STORE"),
        "SmidClient 注入的 deviceid 必须随转发一起到达 leaf"
    );
    assert_eq!(header("x-probe"), Some("1"), "调用方的头也要原样到达");
}
