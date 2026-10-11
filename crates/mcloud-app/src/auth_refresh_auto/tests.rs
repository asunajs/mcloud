//! 「凭据自动刷新定时」的组装层用例。
//!
//! 三条口径各钉一遍（都是**用户口径**里点名的）：
//!
//! 1. 默认种出来就是**开启**，且**算得出下一次触发** ✓；
//! 2. **用户关掉后重算不许打开** ✓；
//! 3. 预设时间改了 ⇒ **下一次触发时刻跟着变** ✓（那条定时的 cron 就是它 ✓）。
//!
//! ⚠ 2026-10-11 第二轮口径（「**要每用户一个钟点**」✓）又加了四条，
//! 钉的是"钟点按 owner 存"这件事：
//!
//! 4. 两个用户各设**不同**钟点 ⇒ 各自那条定时的 `expr` **互不影响** ✓；
//! 5. A 改钟点 ⇒ B 的定时与那一格**一个字节不变** ✓；
//! 6. 老实例的**实例级**那个值（`AppSettings::auth_refresh_at` ✓）是每个用户的
//!    **初始值** ✓，而"再跑一次迁移"（= 重开存储 ✓）**不覆盖**用户后来改的值 ✓；
//! 7. 没有 `auth-refresh.json` 的老目录照样读得进（回落默认值 ✓）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mcloud_core::auth_refresh::{
    auth_refresh_schedule_id, daily_expr, DEFAULT_AUTH_REFRESH_AT, MANAGED_BY_AUTH_REFRESH,
};
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountSettings, AppSettings, AuthInfo, Clock as _, DeviceId, Store, UserId,
};

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

/// 在**同一个目录**上再开一个存储 —— 等价于"重启一次" ✓。
///
/// ⚠ 它是"再跑一次迁移"的等价物：迁移没有写盘动作（读的时候回落 ✓），
/// 所以"第二遍迁移会不会盖掉用户改的值"这件事，只能在**重开存储**上看 ✓。
fn reopen(dir: &std::path::Path) -> Arc<dyn Store> {
    Arc::new(mcloud_store::JsonStore::open(dir).expect("存储应能再打开"))
}

fn owner() -> UserId {
    UserId::new("u-1")
}

/// 第二个用户（多用户那几条用 ✓）。
fn other() -> UserId {
    UserId::new("u-2")
}

/// 那条定时**整条**读回来（用来做"一个字节不变"的逐格比较 ✓）。
async fn schedule_of(store: &Arc<dyn Store>, who: &UserId) -> mcloud_core::Schedule {
    store
        .schedule(&auth_refresh_schedule_id(who))
        .await
        .expect("读定时")
        .unwrap_or_else(|| panic!("{who} 那条定时必须已经落库"))
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

    // ⭐ 而且**落进了他自己那一格**（`data/auth-refresh.json` ✓）—— 重启后还认这个钟点 ✓
    assert_eq!(
        store.auth_refresh_at(&owner()).await.unwrap().as_deref(),
        Some("22:30"),
        "预设时间要落到**这个用户自己**那一格（否则重启就回到默认值 ✗）"
    );
    // ⚠ 而**实例级那一格**（老键 ✓）**一个字节都不该动** —— 它是"还没自己设过的
    // 那些人"的初始值，写它等于把别人的钟点也改了 ✗。
    assert_eq!(
        store.settings().await.unwrap().auth_refresh_at,
        DEFAULT_AUTH_REFRESH_AT,
        "改自己的钟点不许动实例级那一格"
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
    // 他自己那一格还是"没设过"（没被写坏 ✓），实例级那一格也没被动 ✓
    assert_eq!(store.auth_refresh_at(&owner()).await.unwrap(), None);
    assert_eq!(
        store.settings().await.unwrap().auth_refresh_at,
        DEFAULT_AUTH_REFRESH_AT
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════ ④⑤ 每用户一个钟点（2026-10-11 第二轮口径 ✓）════════════

/// ⭐⭐ **核心验收**：两个用户各设**不同**钟点 ⇒ 各自那条定时的 `expr` **互不影响** ✓
///（改之前两格共用实例级那一格 ⇒ 两条永远同一个 `expr` ✗）。
#[tokio::test]
async fn two_users_keep_their_own_clocks() {
    let (store, dir) = store("per-user-clock");
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    seed_auth_refresh_schedule(&store, &other()).await.unwrap();

    // 开箱两条都按默认值 ✓
    assert_eq!(schedule_of(&store, &owner()).await.expr, "0 0 3 * * *");
    assert_eq!(schedule_of(&store, &other()).await.expr, "0 0 3 * * *");

    // A 设 22:30、B 设 07:15
    let a = reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("22:30".into()),
        }),
    )
    .await
    .expect("A 改得掉");
    let b = reschedule_auth_refresh(
        &store,
        &other(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("07:15".into()),
        }),
    )
    .await
    .expect("B 改得掉");

    assert_eq!(a.at, "22:30");
    assert_eq!(b.at, "07:15");
    // ⭐ 两条定时各按各自的钟点 ✓
    assert_eq!(schedule_of(&store, &owner()).await.expr, "0 30 22 * * *");
    assert_eq!(schedule_of(&store, &other()).await.expr, "0 15 7 * * *");
    // ⭐ 落盘也各是各的 ✓
    assert_eq!(
        store.auth_refresh_at(&owner()).await.unwrap().as_deref(),
        Some("22:30")
    );
    assert_eq!(
        store.auth_refresh_at(&other()).await.unwrap().as_deref(),
        Some("07:15")
    );
    // 读回来（= 界面那张卡）也各是各的 ✓
    assert_eq!(
        read_auth_refresh_state(&store, &owner()).await.unwrap().at,
        "22:30"
    );
    assert_eq!(
        read_auth_refresh_state(&store, &other()).await.unwrap().at,
        "07:15"
    );

    // 重启之后仍然各是各的 ✓
    drop(store);
    let s = reopen(&dir);
    assert_eq!(schedule_of(&s, &owner()).await.expr, "0 30 22 * * *");
    assert_eq!(schedule_of(&s, &other()).await.expr, "0 15 7 * * *");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ **A 改钟点 ⇒ B 的定时与那一格一个字节不变** ✓（多用户下最容易破的一条 ✓）。
#[tokio::test]
async fn changing_one_users_clock_leaves_the_other_untouched() {
    let (store, dir) = store("per-user-isolated");
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    seed_auth_refresh_schedule(&store, &other()).await.unwrap();
    reschedule_auth_refresh(
        &store,
        &other(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("07:15".into()),
        }),
    )
    .await
    .unwrap();

    // 改 A 之前，把 B 那一份**整条**记下来（逐格比较 ✓）
    let b_schedule = schedule_of(&store, &other()).await;
    let b_at = store.auth_refresh_at(&other()).await.unwrap();
    let b_state = read_auth_refresh_state(&store, &other()).await.unwrap();

    // A 连改两轮（一轮设时间、一轮翻开关 ✓）
    reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("22:30".into()),
        }),
    )
    .await
    .unwrap();
    reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: Some(false),
            at: None,
        }),
    )
    .await
    .unwrap();

    // ⭐ B 的定时**逐格**没变 ✓
    assert_eq!(
        schedule_of(&store, &other()).await,
        b_schedule,
        "改 A 的钟点/开关，绝不能动 B 那条定时"
    );
    assert_eq!(store.auth_refresh_at(&other()).await.unwrap(), b_at);
    assert_eq!(
        read_auth_refresh_state(&store, &other()).await.unwrap(),
        b_state
    );
    // 而 A 自己确实变了 ✓（否则上面那些断言可能只是"两边都没生效"✓）
    assert_eq!(schedule_of(&store, &owner()).await.expr, "0 30 22 * * *");
    assert!(!schedule_of(&store, &owner()).await.enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ **迁移**：盘上那个**实例级**的老值就是每个用户的**初始值** ✓，
/// 而且**再跑一次迁移**（= 重开存储 ✓）**不覆盖**用户后来改的值 ✓（幂等 ✓）。
#[tokio::test]
async fn the_instance_level_value_is_every_users_starting_point_and_migration_is_idempotent() {
    let (store, dir) = store("migration");
    // 盘上先有**实例级** `auth_refresh_at = 22:30`（升级前那个值 ✓）
    let settings = AppSettings {
        auth_refresh_at: "22:30".into(),
        ..AppSettings::default()
    };
    store.save_settings(&settings).await.expect("写老值");
    drop(store);

    // 第一次"迁移"：重开存储 ⇒ 每个用户读到的都是 22:30 ✓
    let s = reopen(&dir);
    seed_auth_refresh_schedule(&s, &owner()).await.unwrap();
    seed_auth_refresh_schedule(&s, &other()).await.unwrap();
    for who in [owner(), other()] {
        assert_eq!(
            read_auth_refresh_state(&s, &who).await.unwrap().at,
            "22:30",
            "{who} 的初始值必须是老那个实例级值（不许凭空回到 03:00 ✗）"
        );
        assert_eq!(schedule_of(&s, &who).await.expr, "0 30 22 * * *");
    }
    // 而且**没有任何写盘迁移**：那一格本来就是"没设过" ✓
    assert_eq!(s.auth_refresh_at(&owner()).await.unwrap(), None);

    // A 后来改成 07:15
    reschedule_auth_refresh(
        &s,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("07:15".into()),
        }),
    )
    .await
    .unwrap();
    drop(s);

    // ⭐ **再跑一次迁移**（重开存储）⇒ A 改过的值**不被覆盖** ✓，B 仍是初始值 ✓
    let s = reopen(&dir);
    assert_eq!(
        read_auth_refresh_state(&s, &owner()).await.unwrap().at,
        "07:15",
        "第二遍迁移不许把用户改过的值盖回去"
    );
    assert_eq!(schedule_of(&s, &owner()).await.expr, "0 15 7 * * *");
    assert_eq!(
        read_auth_refresh_state(&s, &other()).await.unwrap().at,
        "22:30"
    );
    // ⚠ 老键**还在**（它是"没设过的人"的来源 ✓）—— 没人去删它 ✓
    assert_eq!(s.settings().await.unwrap().auth_refresh_at, "22:30");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 用户**关掉**之后改钟点 ⇒ **仍然关** ✓（而且这只影响他自己 ✓）。
///
/// 与 [`stays_off_once_the_user_turned_it_off`] 的区别：那一条钉的是"同一个用户
/// 连算两轮"，这一条钉的是"关着的那个用户改了钟点 ⇒ 关的状态保住、而**别人不受影响**" ✓。
#[tokio::test]
async fn a_user_who_turned_it_off_stays_off_after_changing_the_clock() {
    let (store, dir) = store("off-then-clock");
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    seed_auth_refresh_schedule(&store, &other()).await.unwrap();

    // A 关掉；B 不动
    reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: Some(false),
            at: None,
        }),
    )
    .await
    .unwrap();
    let b_before = schedule_of(&store, &other()).await;

    // A 只改钟点（**不带** `wanted` ✓）
    let state = reschedule_auth_refresh(
        &store,
        &owner(),
        Some(AuthRefreshPatch {
            wanted: None,
            at: Some("22:30".into()),
        }),
    )
    .await
    .expect("改得掉");
    assert!(
        !state.enabled,
        "只改时间不能把用户关掉的开关打开：{state:?}"
    );
    let a = schedule_of(&store, &owner()).await;
    assert!(!a.enabled, "调度器读的那一格必须还是关的：{a:?}");
    assert_eq!(a.managed_wanted, Some(false), "意愿那一格如实留着 ✓");
    assert_eq!(a.expr, "0 30 22 * * *", "钟点还是要改掉的 ✓");
    // B 那条**一个字节不变** ✓
    assert_eq!(schedule_of(&store, &other()).await, b_before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ **旧数据兼容**：老目录上没有 `auth-refresh.json` ⇒ 读得进、按默认值走 ✓。
///
/// 这是"加一张表不让老实例失效"的保证 ✓（与 `settings.json` 缺字段同一条路 ✓）。
#[tokio::test]
async fn an_old_directory_without_the_new_table_still_works() {
    let (store, dir) = store("old-dir");
    // 前置条件：这个目录上**真的**还没有那个文件（老实例就是这样 ✓）
    assert!(!dir.join("auth-refresh.json").exists());

    assert_eq!(store.auth_refresh_at(&owner()).await.unwrap(), None);
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let state = read_auth_refresh_state(&store, &owner()).await.unwrap();
    assert_eq!(state.at, DEFAULT_AUTH_REFRESH_AT, "没有那一格 ⇒ 用默认值 ✓");
    assert!(state.enabled);
    assert_eq!(schedule_of(&store, &owner()).await.expr, "0 0 3 * * *");
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
        last_note: None,
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

/// ⭐⭐ 那个 id 上是**别人名下**的定时（不是"不归系统维护"，而是"不归**我**"）⇒
/// 同样**一个字节都不许动** ✗。
///
/// # 为什么这条必须单独钉
///
/// `ScheduleId` 是**全局唯一**的，而写的时候 `save_schedule` 是**按 id 全局覆盖**的。
/// 查的时候若按 `(owner, id)` 查（`schedule_for` ✗），别人名下那条同名定时就**查不到**
/// ⇒ 被当成"还没有"⇒ 就地建一条把它**覆盖掉** ✗ —— 表现是"B 手建的那条定时，
/// 被 A 打开一次设置页就没了" ✓。判据必须是**按 id 全局查**（与 seed 那一步同口径 ✓）。
#[tokio::test]
async fn a_schedule_of_another_user_on_that_id_is_never_touched() {
    let (store, dir) = store("squat-other");
    // A 的那条 id 上，放着一条**归 B** 的定时（带系统标记也不行 —— `owner` 才是归属 ✓）
    let id = auth_refresh_schedule_id(&owner());
    let mut foreign = seeded_auth_refresh_schedule(&other(), "22:30").expect("造得出来");
    foreign.id = id.clone();
    foreign.owner = Some(other());
    store.save_schedule(&foreign).await.unwrap();

    // A 重算 / 只读：都不碰它，并如实说明 ✓
    let state = reschedule_auth_refresh(&store, &owner(), None)
        .await
        .expect("不该报错");
    assert!(!state.enabled, "装不上 ⇒ 报「没开」（不报一个假的 true ✓）");
    assert!(state.next_at.is_none());
    assert!(
        state.note.as_deref().unwrap_or("").contains("手建"),
        "要说清为什么装不上：{state:?}"
    );
    assert_eq!(
        read_auth_refresh_state(&store, &owner()).await.unwrap(),
        state
    );

    // B 那条定时**逐格没变** ✓
    assert_eq!(
        store.schedule(&id).await.unwrap().unwrap(),
        foreign,
        "别人的定时一个字节都不能动"
    );
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
    // ⭐ 定时页那句人话（`last_note` ✓）：真换了 ⇒ 说得出换了几个 ✓。
    assert_eq!(
        after.last_note.as_deref(),
        Some("已换 1 个凭据"),
        "换了就要说得出换了几个（定时页那一行照它显示）：{after:?}"
    );
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
    // ⭐⭐ **本用例要钉的新东西**：那句"今天为什么没刷"必须落在 `last_note` 上 ✓
    // —— 它原先**只进服务端日志** ⇒ 定时页那一行只剩一个「成功」徽章 ✗
    //（用户口径 2026-10-11：「自动定时的跳过原因在定时页上看不出来」✗）。
    //
    // ⚠ 判据是**那句完整的人话**（含"还剩 300 天" ✓），不是"非空"✗：
    //   只说"有值"的话，写一句"已跳过"照样绿，而用户要的正是**还剩几天** ✓。
    // ⚠ 也**不许**顺手把状态改成 `skipped`：`ok` 是"跳过是正常结局"那条既有语义 ✓
    //   （上面那一条断言还在 ✓），别的定时类型也在用这三个词 ✓。
    assert_eq!(
        after.last_note.as_deref(),
        Some("未到提前量，跳过（还剩 300 天）"),
        "全跳过 ⇒ 定时页必须看得出「今天为什么没刷」：{after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ **自动定时那条路永远是 `force = false`** —— 手动改成强制时**不许把它带过去** ✗
///（2026-10-11 用户口径：手动点「刷新」要"**点击后强制刷新，不检测时间**" ✓，
/// 而这条定时是"到点**检查**，该刷才刷" ✓ —— 两件事，判据不同 ✓）。
///
/// 判据是**上游一次都没被打**（计数器 ✓），比"看 `last_status` 是不是 ok"更直接：
/// `last_status` 记错还能是别的原因，而"打了上游"只有一个原因 —— 判据被改宽了 ✓。
/// 顺带钉住 `last_status` 仍是 `ok`（"跳过"是**正常结局**，不是失败 ✓）。
#[tokio::test]
async fn hook_never_forces_even_when_the_credential_is_not_due() {
    let (store, dir) = store("hook-never-force");
    let clock = Arc::new(FixedClock(1_700_000_000_000));
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let service = Arc::new(AuthRefreshService::new(
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        Arc::new(move |_a: &Account| {
            Arc::new(CountingUpstream {
                calls: Arc::clone(&seen),
            }) as Arc<dyn mcloud_core::HttpClient>
        }),
    ));
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();

    // 还剩 300 天 ⇒ `NotYet` ⇒ 这条定时只该"检查"、**一枪都不打** ✓
    let acct = account("acc-1", clock.now_ms() + 300 * 86_400_000);
    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, std::slice::from_ref(&acct), 0).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "自动定时**不许**强制刷：未到提前量时上游一次都不能被打（`force` 必须还是 false）"
    );
    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.last_status.as_deref(),
        Some("ok"),
        "跳过是**正常**结局：{after:?}"
    );
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
    // ⚠ 这一格**每次触发都要写**（`Some` 或 `None` ✓）：不写的话，用户删光账号之后
    // 上一趟那句"已换 N 个"会**留着不动** ⇒ 定时页显示一件已经没发生的事 ✗。
    assert_eq!(
        after.last_note.as_deref(),
        Some("这个用户名下没有账号"),
        "「没有账号」也是结果，要说出来（并顺手清掉上一趟那句）：{after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 一个成功、一个跳过 ⇒ **两条都看得出**（换了一个 ✓ + 还剩几天 ✓）。
///
/// ⚠ 为什么要单独钉这一条：全跳过与全成功各有一条用例，但**混合**那一档最容易写成
/// "只报换了的那个"（跳过的那个被吞掉 ✗）或反过来 —— 而用户要看的是**这一趟的完整结论** ✓。
#[tokio::test]
async fn hook_mixed_rotate_and_skip_shows_both() {
    let (store, dir) = store("hook-mixed");
    let clock = Arc::new(FixedClock(1_700_000_000_000));
    let service = Arc::new(AuthRefreshService::new(
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        Arc::new(|_a: &Account| Arc::new(FakeRefresh) as Arc<dyn mcloud_core::HttpClient>),
    ));
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let sch = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();

    // ① 已过期 ⇒ 真刷 ✓；② 还剩 300 天 ⇒ 跳过 ✓
    let due = account("acc-due", clock.now_ms() - 86_400_000);
    let not_due = account("acc-fresh", clock.now_ms() + 300 * 86_400_000);
    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, &[due, not_due], 0).await;

    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.last_status.as_deref(), Some("ok"), "{after:?}");
    assert_eq!(
        after.last_note.as_deref(),
        Some("已换 1 个凭据、1 个未到提前量（还剩 300 天）"),
        "一个换了一个跳过 ⇒ 两句都要在：{after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 多账号**全都**没到提前量 ⇒ 说清有几个、以及**最快那个**还剩几天 ✓。
#[tokio::test]
async fn hook_all_skipped_says_how_many_and_the_nearest_deadline() {
    let (store, dir) = store("hook-all-skip");
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

    // 三个账号都在提前量之外；**12 天**那个最先到期 ⇒ 那句话说的是它 ✓。
    let a = account("acc-1", clock.now_ms() + 300 * 86_400_000);
    let b = account("acc-2", clock.now_ms() + 12 * 86_400_000);
    let c = account("acc-3", clock.now_ms() + 40 * 86_400_000);
    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, &[a, b, c], 0).await;

    let after = store
        .schedule(&auth_refresh_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.last_status.as_deref(),
        Some("ok"),
        "全跳过仍是正常结局：{after:?}"
    );
    assert_eq!(
        after.last_note.as_deref(),
        Some("3 个账号都未到提前量（最近还剩 12 天）"),
        "要说清几个、以及最快那个还剩几天：{after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 有账号**刷失败** ⇒ 那句说明写**空**（错误那一段自己就把汇总说全了 ✓）。
///
/// 不这么做的话，定时页会同时挂着 `lastError`（含"已换 0 个、未到提前量跳过 0 个、
/// 共 1 个；刷新失败：…"✓）与一句重复的 `lastNote` ⇒ 同一件事说两遍 ✗。
#[tokio::test]
async fn hook_clears_the_note_when_something_failed() {
    let (store, dir) = store("hook-note-cleared");
    let clock = Arc::new(FixedClock(1_700_000_000_000));
    let service = Arc::new(AuthRefreshService::new(
        Arc::clone(&store),
        Arc::clone(&clock) as Arc<dyn mcloud_core::Clock>,
        // 一被调用就报错 ⇒ 走到 `failures` 那一支 ✓
        Arc::new(|_a: &Account| Arc::new(DeadUpstream) as Arc<dyn mcloud_core::HttpClient>),
    ));
    seed_auth_refresh_schedule(&store, &owner()).await.unwrap();
    let sid = auth_refresh_schedule_id(&owner());
    let sch = store.schedule(&sid).await.unwrap().unwrap();

    // 先造一句"上一趟的说明"（模拟昨天全跳过那次 ✓）⇒ 本趟必须把它**清掉** ✓。
    store
        .record_schedule_note(&sid, Some("未到提前量，跳过（还剩 300 天）"))
        .await
        .unwrap();

    let due = account("acc-due", clock.now_ms() - 86_400_000);
    let hook = auth_refresh_hook(Arc::clone(&service), Arc::clone(&store));
    let _ = hook(&sch, std::slice::from_ref(&due), 0).await;

    let after = store.schedule(&sid).await.unwrap().unwrap();
    assert_eq!(after.last_status.as_deref(), Some("error"), "{after:?}");
    assert!(
        after
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("刷新失败")),
        "错误那一段要说得出失败原因：{after:?}"
    );
    assert!(
        after.last_note.is_none(),
        "有错误时那句说明必须清空（汇总已经在 `last_error` 里了）：{after:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ [`note_of`] 是**纯函数** ⇒ 三种形态逐条钉在它上面（上面几条用例钉的是"接上了" ✓）。
#[test]
fn note_of_says_what_happened_in_plain_words() {
    // 什么都没发生 ⇒ 不写（不是"写一句空的" ✗）
    assert_eq!(note_of(0, 0, None), None);

    // ① 全跳过：一个账号说"跳过"，多个账号说"都未到提前量（最近…）"
    assert_eq!(
        note_of(0, 1, Some(12.0)).as_deref(),
        Some("未到提前量，跳过（还剩 12 天）")
    );
    assert_eq!(
        note_of(0, 3, Some(12.0)).as_deref(),
        Some("3 个账号都未到提前量（最近还剩 12 天）")
    );
    // ② 真换了
    assert_eq!(note_of(2, 0, None).as_deref(), Some("已换 2 个凭据"));
    // ③ 混合
    assert_eq!(
        note_of(1, 2, Some(11.6)).as_deref(),
        Some("已换 1 个凭据、2 个未到提前量（最近还剩 12 天）")
    );
    // ⚠ 只有一个账号跳过时不写"最近"（只有一个候选 ⇒ 那两个字是废话 ✗）
    assert_eq!(
        note_of(2, 1, Some(300.0)).as_deref(),
        Some("已换 2 个凭据、1 个未到提前量（还剩 300 天）")
    );
    // 天数按**整天**显示（界面上写"11.6 天"是把内部精度漏给用户 ✗）
    assert_eq!(
        note_of(0, 1, Some(0.4)).as_deref(),
        Some("未到提前量，跳过（还剩 0 天）")
    );
    // 算不出剩余天数也照说（不写"0 天"糊过去 ✓）
    assert_eq!(note_of(0, 2, None).as_deref(), Some("2 个账号都未到提前量"));

    // ⚠ 措辞纪律（`AGENTS.md`「刷新凭据的正确说法」✓）：**只陈述结果**，
    // 不解释会怎样、不提"旧的怎么样" ✗。
    for bad in ["作废", "吊销", "不可逆", "撤不回来", "强制", "因为", "所以"] {
        let s = note_of(1, 1, Some(3.0)).unwrap();
        assert!(!s.contains(bad), "那句人话里不许出现「{bad}」：{s}");
    }
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

/// 会**记账**的上游：每被调用一次就 +1 ✓。
///
/// ⚠ 与 [`DeadUpstream`] 的分工：那个只能证明"打了就报错"（于是结论落在
/// `last_status` 上 ✓），这个能**直接数**"到底打了几枪" ✓ ——
/// 钉"`force` 必须还是 false"这件事，计数器比间接读数直接得多 ✓。
struct CountingUpstream {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl mcloud_core::HttpClient for CountingUpstream {
    async fn request(
        &self,
        _req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(mcloud_core::Error::Network(
            "不该打到上游（自动定时不许强制刷）".into(),
        ))
    }
}
