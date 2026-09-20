use super::*;
use mcloud_core::{
    AccountId, Error, HttpClient, HttpRequest, HttpResponse, Random, Scope, StateStore, User,
    UserId, UserStore, account_label,
};
use mcloud_io::SqliteStore;
use serde_json::{Value, json};
use std::cell::RefCell;

/// 确定性随机源：把给定字节循环填出去。
struct SeqRandom(u8);

impl Random for SeqRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for byte in dst.iter_mut() {
            *byte = self.0;
            self.0 = self.0.wrapping_add(1);
        }
    }
}
/// 逐次返回脚本响应的假客户端，并记下发出去的请求。
struct ScriptedClient {
    responses: RefCell<Vec<Value>>,
    seen: RefCell<Vec<HttpRequest>>,
}

impl ScriptedClient {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: RefCell::new(responses),
            seen: RefCell::new(Vec::new()),
        }
    }

    fn sent(&self) -> Vec<HttpRequest> {
        self.seen.borrow().clone()
    }

    fn url_of(&self, index: usize) -> String {
        self.seen.borrow()[index].url.clone()
    }

    fn header_of(&self, index: usize, name: &str) -> Option<String> {
        self.seen.borrow()[index]
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
}

impl HttpClient for ScriptedClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        self.seen.borrow_mut().push(request);
        let mut queue = self.responses.borrow_mut();
        if queue.is_empty() {
            return Err(Error::Internal("脚本已用完".into()));
        }
        Ok(HttpResponse::new(
            200,
            serde_json::to_vec(&queue.remove(0)).unwrap(),
        ))
    }
}

/// 黄金向量（真实抓包 + 真实响应）。
fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../test/golden/app-signin-vectors.json"
    ))
    .expect("黄金向量文件必须是合法 JSON")
}

/// 拿真实 `infoV3` 响应，把**服务端标为今天**的那一格改成指定状态。
///
/// 用真实响应而不是手搓一个最小 JSON：`cal` 里混着上/下月的补位格，
/// 手搓的样本会把"按 `d` 裸查会命中错的月份"这个坑绕过去，
/// 于是测试通过而实现在真实数据上出错。
fn info_v3_with_today_signed(signed: bool) -> Value {
    let mut raw = vectors()["infoV3"]["raw"].clone();
    let cal = raw["result"]["cal"]
        .as_array_mut()
        .expect("黄金向量的 cal 必须是数组");
    let mut touched = 0;
    for cell in cal.iter_mut() {
        if cell["t"] == json!(true) {
            cell["s"] = json!(signed);
            touched += 1;
        }
    }
    assert_eq!(touched, 1, "真实响应里应当恰好有一格被标为今天");
    raw
}

fn app_device() -> mcloud_upstream::app_client::AppDevice {
    mcloud_upstream::app_client::AppDevice::ipad("AAA00000000000000000000000000000", "AAA000000000")
}

/// 库里放一个"已登录"的 App 账号（**v6 之后的形态**：`account_state.phone` 有值）。
async fn store_with_login() -> (SqliteStore, Scope<'static>, UserId) {
    let s = SqliteStore::open_in_memory().unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-19T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let user = User::new("u1", "alice", "!", mcloud_core::Role::Admin, now);
    s.upsert_user(&user).await.unwrap();

    let account_id = AccountId::new(account_label(None, "19900000001"));
    let mut account = mcloud_core::Account::new(account_id.as_str());
    account.owner = Some(user.id.clone());
    // `platform` 是登录端（`parse_auth` 的第 1 段）—— App 端是 `mobile`。
    account.platform = Some("mobile".into());
    s.upsert_account(Scope::Local, &account).await.unwrap();
    s.upsert_account_state(
        Scope::Local,
        &mcloud_core::AccountState {
            account_id,
            jwt_token: Some("AUTHTOKEN".into()),
            user_domain_id: Some("1990000000000000002".into()),
            // v6 起登录会把完整手机号写进来 —— 这就是 `--account` 能省掉号码的依据。
            phone: Some("19900000001".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let owner = user.id.clone();
    (s, Scope::Local, owner)
}

/// 同上，但 `phone` 列是 `NULL` —— **v6 之前登录的老账号**。
///
/// 刻意不另写一份建库代码：这里用一次"整体覆盖"把 `phone` 清成 `NULL`，
/// 顺带钉住"覆盖写真的能清掉"这条语义（否则下面的用例可能只是"没写进去"的假象）。
async fn store_with_legacy_login() -> (SqliteStore, Scope<'static>, UserId) {
    let (store, scope, owner) = store_with_login().await;
    let id = AccountId::new(account_label(None, "19900000001"));
    let mut state = store.get_account_state(scope, &id).await.unwrap().unwrap();
    state.phone = None;
    store.upsert_account_state(scope, &state).await.unwrap();
    (store, scope, owner)
}

/// 把账号的登录端改成别的（用于验证"端不匹配就拒绝"那道闸）。
async fn set_platform(store: &SqliteStore, scope: Scope<'_>, platform: Option<&str>) {
    let id = AccountId::new(account_label(None, "19900000001"));
    let mut account = store.get_account(scope, &id).await.unwrap().unwrap();
    account.platform = platform.map(str::to_string);
    store.upsert_account(scope, &account).await.unwrap();
}

fn fixed_clock() -> mcloud_core::FixedClock {
    mcloud_core::FixedClock::new(
        chrono::DateTime::parse_from_rfc3339("2026-09-19T06:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    )
}

mod chain;
mod credential;
mod flow;
mod plan_backup_gift;
mod plan_multiple;
mod plan_receive;
