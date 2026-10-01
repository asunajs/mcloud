//! 判据测试。
//!
//! 判据是**纯函数**，所以能逐档钉住。这三档错了不会报错 ——
//! 只会"该刷的没刷"（直到上游回 90001）或"不该刷的天天刷"。

use super::*;

const DAY: i64 = 86_400_000;
const NOW: i64 = 1_893_456_000_000; // 某个固定的"现在"

// ── 三档 ────────────────────────────────────────────────────────────────

#[test]
fn needed_when_inside_the_lead_window() {
    // 还剩 3 天，提前量 7 天 ⇒ 该刷
    assert_eq!(decide(NOW + 3 * DAY, NOW, 7.0), RefreshDecision::Needed);
}

#[test]
fn not_yet_when_far_away() {
    // 还剩 30 天 ⇒ 不该刷，一个请求都不发
    assert_eq!(decide(NOW + 30 * DAY, NOW, 7.0), RefreshDecision::NotYet);
}

/// ⭐ 边界：刚好等于提前量时该刷（判据是 `<=`，不是 `<`）。
#[test]
fn exactly_at_the_lead_window_is_needed() {
    assert_eq!(decide(NOW + 7 * DAY, NOW, 7.0), RefreshDecision::Needed);
    // 多一毫秒就不刷
    assert_eq!(decide(NOW + 7 * DAY + 1, NOW, 7.0), RefreshDecision::NotYet);
}

#[test]
fn already_expired_is_needed() {
    assert_eq!(decide(NOW - DAY, NOW, 7.0), RefreshDecision::Needed);
}

/// ⭐ `expire == 0` 是 `Needed`，**不是** `ManualOnly`。
///
/// 参考实现两处不一致（一处当"跳过"、一处当"该刷"）。
/// 本仓取"该刷"：0 更可能是"解析不出"而不是"确实没有"，
/// 打一次不亏，漏刷才是问题。
#[test]
fn zero_expire_is_needed_not_manual() {
    assert_eq!(decide(0, NOW, 7.0), RefreshDecision::Needed);
}

/// ⭐ `ManualOnly` **只有** `expire < 0` 这一种情况。
#[test]
fn manual_only_is_strictly_negative() {
    assert_eq!(decide(-1, NOW, 7.0), RefreshDecision::ManualOnly);
    assert_eq!(decide(i64::MIN, NOW, 7.0), RefreshDecision::ManualOnly);
    // 0 与正数都不是
    assert_ne!(decide(0, NOW, 7.0), RefreshDecision::ManualOnly);
    assert_ne!(decide(NOW, NOW, 7.0), RefreshDecision::ManualOnly);
}

// ── 单位 ────────────────────────────────────────────────────────────────

/// ⭐ 判据按**毫秒**算。
///
/// 把毫秒当秒读的后果不是报错，而是**静默算错**：
/// `1893456000000` 会被当成公元 61968 年，"永远不需要刷新"。
#[test]
fn uses_milliseconds_not_seconds() {
    // 一个真实的 13 位毫秒时间戳（约 2030 年）
    let expire = 1_893_456_000_000;
    // 从这个时刻起 30 天
    let now = expire - 30 * DAY;
    assert_eq!(
        decide(expire, now, 7.0),
        RefreshDecision::NotYet,
        "还剩 30 天，不该刷"
    );
    // 若把 expire 当秒（即除以 1000），它相对于 now 会小到"早已过期" ⇒ Needed。
    // 这两种解释给出**相反**的结论，正是这个测试要钉住的。
    assert_eq!(decide(expire / 1000, now, 7.0), RefreshDecision::Needed);
}

// ── 提前量参数 ──────────────────────────────────────────────────────────

#[test]
fn lead_days_is_configurable() {
    let expire = NOW + 10 * DAY;
    assert_eq!(
        decide(expire, NOW, 7.0),
        RefreshDecision::NotYet,
        "7 天窗口外"
    );
    assert_eq!(
        decide(expire, NOW, 14.0),
        RefreshDecision::Needed,
        "14 天窗口内"
    );
}

/// 很小的提前量（等价于"到期当天才刷"）。
#[test]
fn tiny_lead_days_is_allowed() {
    let expire = NOW + DAY;
    assert_eq!(decide(expire, NOW, 0.001), RefreshDecision::NotYet);
    assert_eq!(decide(NOW + 60_000, NOW, 0.001), RefreshDecision::Needed);
}

// ── 提前量的来源：账号配置（不是写死的 7） ──────────────────────────────

/// 只当构造参数用的 store：这几条用例的判定**不碰库**
/// （not-yet 直接返回；该刷那一支在 `refresh_request` 就因缺用户域而 Err）。
fn unused_store() -> Arc<dyn Store> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mcloud-auth-refresh-{}-{nanos}",
        std::process::id()
    ));
    Arc::new(mcloud_store::JsonStore::open(&dir).expect("临时目录必须能建"))
}

/// 永远打挂的 HTTP 客户端。这几条用例的第一个失败点是缺 `userDomainId`
/// （`refresh_request`），它压根到不了这里；留着它只是为了"真打了网络也只会失败"。
struct NeverHttp;

#[async_trait::async_trait]
impl mcloud_core::HttpClient for NeverHttp {
    async fn request(
        &self,
        _req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        Err(mcloud_core::Error::Other("测试替身：不该真的打网络".into()))
    }
}

fn service() -> AuthRefreshService {
    let http_for: mcloud_engine::HttpClientFactory =
        Arc::new(|_a: &Account| Arc::new(NeverHttp) as Arc<dyn mcloud_core::HttpClient>);
    AuthRefreshService::new(
        unused_store(),
        Arc::new(mcloud_core::testkit::FixedClock(NOW)),
        http_for,
    )
}

/// 解析口径：显式覆盖 > 账号配置；账号默认是 **10**（不是参考实现的代码兜底 7）。
#[test]
fn lead_days_for_prefers_explicit_then_account_settings() {
    let mut acct = account_fixture();
    assert_eq!(lead_days_for(&acct, None), 10.0, "账号默认是 10 天");
    assert_eq!(lead_days_for(&acct, Some(3.0)), 3.0, "显式覆盖优先");

    acct.settings.refresh_token_days = 7;
    assert_eq!(lead_days_for(&acct, None), 7.0);
    assert_eq!(lead_days_for(&acct, Some(3.0)), 3.0);
}

/// ⭐ d) 提前量来自**账号配置** `refresh_token_days`（默认 10），而不是写死的 7。
///
/// 边界：还剩 8 天时，阈值 7 ⇒ 不刷；阈值 10 ⇒ 该刷 —— 这正是 7 与 10
/// 给出**相反**结论的那条线。
#[tokio::test]
async fn refresh_lead_days_comes_from_account_settings() {
    let mut acct = account_fixture();
    acct.auth.expire = NOW + 8 * DAY;
    let svc = service();

    // 账号配置 7 天 ⇒ 8 > 7 ⇒ not-yet，**一个请求都不发**。
    acct.settings.refresh_token_days = 7;
    let out = svc.refresh(&acct, None, false).await.expect("不该是 Err");
    assert!(
        matches!(out, RefreshOutcome::Skipped { .. }),
        "实得：{out:?}"
    );

    // 账号配置 10 天 ⇒ 8 <= 10 ⇒ 该刷 ⇒ 越过 not-yet 走到 rotate。
    // 这个账号没有 userDomainId ⇒ `refresh_request` 立刻 Err。
    acct.settings.refresh_token_days = 10;
    assert!(
        svc.refresh(&acct, None, false).await.is_err(),
        "阈值 10 时 8 天余量应判该刷（于是走到缺用户域那一支）"
    );
}

/// ⭐ 显式 `--lead-days` 仍然优先于账号配置。
#[tokio::test]
async fn explicit_lead_days_overrides_account_settings() {
    let mut acct = account_fixture();
    acct.auth.expire = NOW + 8 * DAY;
    acct.settings.refresh_token_days = 7; // 账号说不用刷
    let svc = service();
    assert!(
        svc.refresh(&acct, Some(10.0), false).await.is_err(),
        "显式 10 天应覆盖账号配置的 7 天"
    );
}

// ── 落库兜底 ────────────────────────────────────────────────────────────

/// ⭐ 明显不是凭据的串要被挡住。
///
/// "拼错了但看起来有值"是最难查的一类：库里、界面上都正常显示，
/// 只是每次请求都鉴权失败。
///
/// ⚠ 但 `parse_auth` 是**刻意宽容**的（对齐 Node 的 `Buffer.from(s,"base64")`：
/// 忽略字母表以外的字符）。所以"合法 base64 但内容没有冒号"这种输入
/// **它能解出来**（platform/phone/token 都是空串）—— 那不是缺陷，
/// 是"上游塞了个空白字符"不该让整条凭据不可用这条设计的代价。
///
/// 这个测试只钉住真正的硬边界：**空的 / 纯垃圾**过不去。
#[test]
fn rejects_clearly_unusable_auth() {
    assert!(assert_usable("").is_err(), "空串必须挡住");
    // 长度模 4 余 1 的 base64 是无解的
    assert!(assert_usable("!!!!!").is_err(), "解不开的必须挡住");
    // 正确的形状要能过
    let good =
        mcloud_crypto::b64_encode(b"mobile:19900000000:SYNTH001|1|RCS|1792391642025|payload");
    assert!(assert_usable(&good).is_ok());
}

/// 宽容是有意的：能解出内容就算可用（哪怕字段是空的）。
///
/// 记下来是为了让"为什么不更严"这件事**有据可查** ——
/// 不然下一个人会以为这里漏了校验，然后加上去，
/// 于是"上游多一个空白字符"变成"凭据整条不可用"。
#[test]
fn lenient_by_design() {
    let odd = mcloud_crypto::b64_encode(b"garbage");
    assert!(
        assert_usable(&odd).is_ok(),
        "合法 base64 一律算可用 —— 与 Node 的宽容解码对齐"
    );
}

// ── 刷新回来后，过期时刻有没有真的推进 ─────────────────────────────────
//
// ⚠ 这一格以前**没有测试**：三档判据（`decide()`）覆盖得很齐，但"刷新成功后
// `expire` 有没有真的前移"没人验。于是下面这个 bug 活了很久：
//
//   `rotate()` 拿 `format!("{}:{}:{}", platform, phone, token)` **明文**去解析，
//   而 `parse_auth` 的契约是"`Basic ` + base64 整串"。它不报错 —— 只是拿不到
//   expire ⇒ 库里 `expire` 停在旧值 ⇒ `decide()` 下一轮仍判"该刷"
//   ⇒ **每轮都真轮换凭据**（对上游是不可逆动作）。
//
// 2026-09-30 复核 qwen review 时查实并修掉；这两条测试是它的护栏。

fn account_fixture() -> Account {
    Account {
        id: mcloud_core::AccountId::new("19****01"),
        nickname: None,
        auth: mcloud_core::AuthInfo {
            raw: "Basic eA==".into(),
            platform: "mobile".into(),
            phone: "19900000001".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: mcloud_core::DeviceId("DEV+1/base=".into()),
        pc_platform: None,
        user_domain_id: None,
        owner: None,
        settings: mcloud_core::AccountSettings::default(),
    }
}

/// 正面：整串（`Basic ` + base64）能拿到 expire —— `rotate()` 现在喂的就是它。
#[test]
fn expire_comes_from_the_whole_auth_string() {
    let acct = account_fixture();
    let token = "SYNTH001|1|RCS|1792391642025|payload";
    let auth = auth_refresh::rebuild_authorization(&acct, token).expect("拼串应成功");
    assert_eq!(
        expire_from_new_auth(&auth),
        Some(1_792_391_642_025),
        "整串 base64 解出来的 token 第 4 段就是过期时刻"
    );
}

/// 反面：`platform:phone:token` 明文拼法**拿不到** expire —— 那正是曾经的写法。
///
/// 留着它是为了让"为什么不能那么拼"有据可查：`parse_auth` 走的是复刻 Node 的
/// 宽容解码，字母表以外的字符（`:` 就在其中）会被**直接丢掉**，解出来是垃圾。
#[test]
fn plaintext_join_never_yields_an_expire() {
    let acct = account_fixture();
    let token = "SYNTH001|1|RCS|1792391642025|payload";
    let plain = format!("{}:{}:{}", acct.auth.platform, acct.auth.phone, token);
    assert_eq!(
        expire_from_new_auth(&plain),
        None,
        "明文形式必须拿不到 expire（否则 rotate 当初的写法就不算 bug 了）"
    );
}
