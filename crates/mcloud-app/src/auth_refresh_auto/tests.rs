//! 「凭据自动刷新定时」的组装层用例。
//!
//! 三条口径各钉一遍（都是**用户口径**里点名的）：
//!
//! 1. 默认种出来就是**开启**，且**算得出下一次触发** ✓；
//! 2. **用户关掉后重算不许打开** ✓；
//! 3. 预设时间改了 ⇒ **下一次触发时刻跟着变** ✓（那条定时的 cron 就是它 ✓）。

use std::sync::Arc;

use mcloud_core::auth_refresh::{
    auth_refresh_schedule_id, daily_expr, DEFAULT_AUTH_REFRESH_AT, MANAGED_BY_AUTH_REFRESH,
};
use mcloud_core::testkit::FixedClock;
use mcloud_core::{Account, AccountSettings, AuthInfo, Clock as _, DeviceId, Store, UserId};

use super::*;

/// 一个**临时目录**上的真存储（`JsonStore` 没有"内存版" ✓）——
/// 落盘一次正好顺带钉住"设置真的写进去了" ✓。
///
/// 返回 `(存储, 目录)`：目录由调用方清理 ✓。
fn store(tag: &str) -> (Arc<dyn Store>, std::path::PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mcloud-auth-refresh-auto-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let s = mcloud_store::JsonStore::open(&dir).expect("存储应能打开");
    (Arc::new(s), dir)
}

fn owner() -> UserId {
    UserId::new("u-1")
}

/// 一条账号（`expire_ms` 决定它"该刷"还是"该跳过" ✓）。
fn account(id: &str, expire_ms: i64) -> Account {
    Account {
        id: mcloud_core::AccountId::new(id),
        nickname: None,
        auth: AuthInfo {
            raw: "Basic cGM6MTk5MDAwMDAwMDA6dA==".into(),
            platform: "pc".into(),
            phone: "19900000000".into(),
            token: "t".into(),
            expire: expire_ms,
        },
        device_id: DeviceId("dev".into()),
        pc_platform: None,
        user_domain_id: Some("1990000000000000001".into()),
        owner: Some(owner()),
        enabled: true,
        settings: AccountSettings::default(),
    }
}

// ════════════ ① 默认：开启 + 算得出下一次 ════════════

/// ⭐ 用户口径「**自动设置定时**」⇒ 种下去就是**开启**的，且 `nextAt` 有值 ✓。
///
/// ⚠ 这一条与「直播自动定时」**刻意不同**（那条种下去 `enabled=false` ✓）：
/// 那一条的时刻依赖**上游**，这一条的时刻**只依赖一个设置值** ✓。
#[tokio::test]
async fn seeding_is_on_and_has_a_next_time() {
    let (store, dir) = store("seed-on");
    assert!(
        seed_auth_refresh_schedule(&store, &owner())
            .await
            .expect("种得下去"),
        "第一次必须真的种出来"
    );

    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .expect("读定时")
        .expect("那条定时必须已经落库");
    assert_eq!(
        sch.managed_by.as_deref(),
        Some(MANAGED_BY_AUTH_REFRESH),
        "必须带系统维护标记（系统只动自己这条 ✓）"
    );
    assert_eq!(sch.managed_wanted, Some(true), "意愿 = 要（用户口径 ✓）");
    assert!(sch.enabled, "默认就是**开启**的（时刻当下算得出来 ✓）");
    assert_eq!(sch.task, mcloud_core::AUTH_REFRESH_TASK);
    assert_eq!(sch.timezone, "Asia/Shanghai");
    assert_eq!(
        sch.expr,
        daily_expr(DEFAULT_AUTH_REFRESH_AT).expect("默认钟点应能写成 cron"),
        "cron 必须正好是那个预设钟点"
    );
    assert!(sch.accounts.is_empty(), "空 = 该用户全部已启用账号");

    // 状态读回来：开启 + 下一次时刻有值 ✓
    let state = read_auth_refresh_state(&store, &owner())
        .await
        .expect("读状态");
    assert!(state.enabled, "默认开启：{state:?}");
    assert_eq!(state.at, DEFAULT_AUTH_REFRESH_AT);
    assert_eq!(
        state.next_at.as_deref(),
        Some("每天 03:00（东八区）"),
        "必须算得出下一次触发（用户口径点名的那条 ✓）"
    );
    assert_eq!(state.note, None, "正常状态不该有说明");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 种**第二次**一个字节都不动（幂等 ✓），尤其**不把用户关掉的意愿冲掉** ✓。
#[tokio::test]
async fn seeding_twice_changes_nothing() {
    let (store, dir) = store("seed-twice");
    assert!(seed_auth_refresh_schedule(&store, &owner()).await.unwrap());
    // 用户关掉它
    reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: Some(false),
            at: None,
        }),
    )
    .await
    .expect("关得掉");
    // 再种一次（= 下一次启动 / 下一次登录 ✓）
    assert!(
        !seed_auth_refresh_schedule(&store, &owner()).await.unwrap(),
        "已经有这条 ⇒ 不算新种出来"
    );
    let state = read_auth_refresh_state(&store, &owner()).await.unwrap();
    assert!(!state.enabled, "用户关掉的不能被种回去：{state:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════ ② 用户关掉后重算不许打开 ════════════

/// ⭐ 用户口径「**用户明确关掉后不许再打开**」—— 连算两轮都不许 ✓。
#[tokio::test]
async fn stays_off_once_the_user_turned_it_off() {
    let (store, dir) = store("stays-off");
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();

    for round in 0..2 {
        let state = reschedule_auth_refresh(
            &store,
            &owner(),
            Some(AuthRefreshPatch {
                wanted: Some(false),
                at: None,
            }),
        )
        .await
        .expect("关得掉");
        assert!(!state.enabled, "第 {round} 轮：关掉之后必须一直是关的");

        // 不带意愿的重算（= 只改时间 / 页面重进 ✓）同样不许把它打开 ✓
        let state = reschedule_auth_refresh(
            &store,
            &owner(),
            Some(AuthRefreshPatch {
                wanted: None,
                at: Some("22:30".to_string()),
            }),
        )
        .await
        .expect("改时间");
        assert!(
            !state.enabled,
            "第 {round} 轮：只改时间**不能**把用户关掉的开关打开：{state:?}"
        );
    }

    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sch.managed_wanted, Some(false), "意愿那一格也要如实留着");
    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════ ③ 改了预设时间 ⇒ 下一次跟着变 ════════════

/// ⭐ 用户口径「**预设时间改了 ⇒ 下一次触发时刻跟着变**」✓ ——
/// 判据落在那条定时的 `expr` 上（调度器读的就是它 ✓）。
#[tokio::test]
async fn changing_the_clock_changes_the_next_fire() {
    let (store, dir) = store("clock-change");
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let before = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.expr, "0 0 3 * * *");

    let state = reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("22:30".to_string()),
        }),
    )
    .await
    .expect("改得掉");
    assert_eq!(state.at, "22:30");
    assert_eq!(
        state.next_at.as_deref(),
        Some("每天 22:30（东八区）"),
        "下一次触发必须跟着变：{state:?}"
    );

    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.expr, "0 30 22 * * *",
        "那条定时的 cron 就是下一次触发时刻"
    );
    assert_eq!(after.managed_wanted, Some(true), "只改时间不该动意愿");

    // ⭐ 而且**落进了设置**（`data/settings.json` ✓）—— 重启后还认这个钟点 ✓
    assert_eq!(
        store.settings().await.unwrap().auth_refresh_at,
        "22:30",
        "预设时间要落到实例设置里（否则重启就回到默认值 ✗）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 非法钟点**当场拒**（不静默兜默认值 ✗ —— 那会让"我设了 22:30、它却在半夜刷"查不出来 ✓）。
#[tokio::test]
async fn a_bad_clock_is_rejected_not_defaulted() {
    let (store, dir) = store("bad-clock");
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    for bad in ["", "25:00", "12:60", "半夜"] {
        let err = reschedule_auth_refresh(
            &store,
            &owner(),
            Some(AuthRefreshPatch {
                wanted: None,
                at: Some(bad.to_string()),
            }),
        )
        .await
        .expect_err("非法钟点必须被拒");
        assert!(
            err.to_string().contains("HH:MM"),
            "错误里要说清正确形状：{err}"
        );
    }
    // 设置里还是原来那个值（没被写坏 ✓）
    assert_eq!(
        store.settings().await.unwrap().auth_refresh_at,
        DEFAULT_AUTH_REFRESH_AT
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════ 撞车：id 上是用户手建的定时 ⇒ 绝不碰 ════════════

/// `auto-auth-refresh-<owner>` 这个 id 用户**也能**手建 ⇒ 系统**一个字节都不动** ✗，
/// 只回一句可操作的说明 ✓（与 `live_auto` 同一条处置 ✓）。
#[tokio::test]
async fn a_handmade_schedule_on_that_id_is_never_touched() {
    let (store, dir) = store("squat");
    let id = auth_refresh_schedule_id(&owner());
    let handmade = mcloud_core::Schedule {
        id: id.clone(),
        name: "我自己建的".into(),
        task: "sign".into(),
        accounts: Vec::new(),
        expr: "0 9 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::json!({}),
        enabled: true,
        managed_by: None,
        managed_wanted: None,
        owner: Some(owner()),
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    };
    store.save_schedule(&handmade).await.unwrap();

    // 种：什么都不写 ✓
    assert!(!seed_auth_refresh_schedule(&store, &owner()).await.unwrap());
    // 重算：不碰它，并如实说明 ✓
    let state = reschedule_auth_refresh(&store, &owner(), None)
        .await
        .expect("不该报错");
    assert!(!state.enabled, "装不上 ⇒ 报「没开」（不报一个假的 true ✓）");
    assert!(state.next_at.is_none());
    assert!(
        state.note.as_deref().unwrap_or("").contains("手建"),
        "要说清为什么装不上：{state:?}"
    );

    // 那条手建的定时**逐格没变** ✓
    let after = store.schedule(&id).await.unwrap().unwrap();
    assert_eq!(after, handmade, "系统绝不能动用户手建的定时");
    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════ 钩子：到点该刷就刷、不该刷就跳过 ════════════

/// ⭐ 到点**该刷就刷**：账号已过期 ⇒ 打上游一枪、换新凭据、记 `ok` ✓。
#[tokio::test]
async fn hook_rotates_when_the_credential_is_due() {
    let (store, dir) = store("hook-rotate");
    let clock = Arc::new(FixedClock(1_700_000_000_000));
    let service = Arc::new(AuthRefreshService::new(
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        // 上游替身：回一份 `code=0000` + 新 token（形状照实拍 ✓）
        Arc::new(|_a: &Account| Arc::new(FakeRefresh) as Arc<dyn mcloud_core::HttpClient>),
    ));
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();

    // 已过期（expire 在过去）⇒ 判据是 Needed ⇒ 真刷 ✓
    let acct = account("acc-1", clock.now_ms() - 86_400_000);
    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, std::slice::from_ref(&acct), 0).await;

    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.last_status.as_deref(), Some("ok"), "{after:?}");
    assert!(after.last_error.is_none(), "成功时不该留旧错误：{after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 到点**不该刷就跳过**：还剩 300 天 ⇒ **一枪都不打上游**、记 `ok` ✓。
#[tokio::test]
async fn hook_skips_when_the_credential_is_not_due() {
    let (store, dir) = store("hook-skip");
    let clock = Arc::new(FixedClock(1_700_000_000_000));
    let service = Arc::new(AuthRefreshService::new(
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        // ⚠ 这个替身**一被调用就报网络错**：本用例要钉的正是"它根本不该被调用" ✓
        //（若判据被改宽成"到点必刷"，那条定时的 `last_status` 就会变成 `error` ✓）。
        Arc::new(|_a: &Account| Arc::new(DeadUpstream) as Arc<dyn mcloud_core::HttpClient>),
    ));
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();

    // 还剩 300 天 ⇒ NotYet ⇒ 跳过 ✓
    let acct = account("acc-1", clock.now_ms() + 300 * 86_400_000);
    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, std::slice::from_ref(&acct), 0).await;

    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.last_status.as_deref(),
        Some("ok"),
        "跳过是**正常**结局（不是失败）：{after:?}"
    );
    assert!(after.last_error.is_none(), "{after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 没有账号 ⇒ 记 `ok`、**不动 `last_run_ms`** ✓（这一趟什么都没做 ✓）。
#[tokio::test]
async fn hook_with_no_accounts_records_ok_and_does_not_touch_last_run() {
    let (store, dir) = store("hook-no-accounts");
    let clock = Arc::new(FixedClock(1_700_000_000_000));
    let service = Arc::new(AuthRefreshService::new(
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        Arc::new(|_a: &Account| Arc::new(DeadUpstream) as Arc<dyn mcloud_core::HttpClient>),
    ));
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();

    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, &[], 0).await;

    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.last_status.as_deref(), Some("ok"), "{after:?}");
    assert!(
        after.last_run_ms.is_none(),
        "什么都没做 ⇒ 不该动「上次运行」：{after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════ 上游替身 ════════════

/// 回一份**形状照实拍**的刷新应答（`code="0000"` + `data.token` ✓）。
struct FakeRefresh;

#[async_trait::async_trait]
impl mcloud_core::HttpClient for FakeRefresh {
    async fn request(
        &self,
        _req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        // token 自带过期段（`a|b|c|<expire_ms>|e` 是实拍形状 ✓）⇒
        // `expire_from_new_auth` 拿得到新过期时刻 ✓。
        let token = "SYNTH|1|RCS|1893456000000|payload";
        Ok(mcloud_core::HttpResponse {
            status: 200,
            body: format!(
                r#"{{"success":true,"code":"0000","message":"ok","data":{{"token":"{token}"}}}}"#
            ),
            headers: Vec::new(),
        })
    }
}

/// **一被调用就报网络错**的上游（用来钉"根本不该打它" ✓）。
struct DeadUpstream;

#[async_trait::async_trait]
impl mcloud_core::HttpClient for DeadUpstream {
    async fn request(
        &self,
        req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        Err(mcloud_core::Error::Network(format!(
            "不该打到上游（{}）",
            req.url
        )))
    }
}
