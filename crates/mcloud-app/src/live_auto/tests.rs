//! 「直播自动定时」的用例（§3 验收逐条对应）。
//!
//! # 为什么这里用**真 `JsonStore`**（不造替身）
//!
//! 这一层的判据一半在存储上：upsert 幂等（"只有一条"✓）、"手建的定时一个字节
//! 没变"（✓）、"账号多选不被重置"（✓）、"停用但没被删"（✓）——
//! 它们全是**落盘之后读回来**才成立的事。替身存储会让这几条退化成
//! "我调了 save_schedule"这种自我循环 ✗（`notify.rs` 的测试注释记着同一条取舍 ✓）。
//!
//! # ⚠ 「下一场」的两条来源都要测（用户口径 2026-10-08：「下一场应该从上游获取啊」✓）
//!
//! * **向上游现取**：用 [`FakeUpstream`] 造一个假上游（它把「门的三步」
//!   按账号应答出来 ✓）—— 六条验收（§3.1–§3.6）走的就是它 ✓；
//! * **池里那份兜底**：把 `http_for` 传 `None`（= 没接上游工厂 ✓）或让假上游
//!   全部报错 ⇒ 重算退到池里那份 ✓。
//!
//! ⚠ 这些用例**真跑**（不是"看着像"）：假上游是按**真的那四枪**（SSO → JWT →
//! 两个页签 ✓）应答的，URL 与头都按上游那套判 —— 请求构造改坏了这里会红 ✓。

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use mcloud_core::kouling::{KoulingPool, ProbeRoom};
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountId, AccountSettings, AuthInfo, DeviceId, Error, HttpClient, HttpRequest,
    HttpResponse, Result as CoreResult, Store, UserId, LIVE_ROOM_TASK, MANAGED_BY_LIVE_AUTO,
};
use mcloud_store::JsonStore;
use mcloud_upstream::live_consts::KOULING_ANCHOR;

/// 基准时刻：**2026-10-08 09:00:00（东八区）**。
///
/// 用 `parse_beijing_time` 现算而不是写一个魔数：写死一个毫秒数的话，
/// 它到底对应哪个钟点要靠心算，而"下一场 10:00 / 定时 11:00"这两条断言
/// 全都建立在这个时刻上 ✗。
fn now_ms() -> i64 {
    mcloud_core::parse_beijing_time("2026-10-08 09:00:00", mcloud_core::live_auto::LOCAL_FMT)
        .expect("基准时刻必须能解析")
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(now_ms()))
}

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "mcloud-liveauto-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// ⚠ 返回 `Arc<dyn Store>`（不是 `Arc<JsonStore>`）：
/// 本层的入口签名就是那个 trait 对象 ✓，而 `Store` 在作用域里之后
/// `save_schedule` / `kouling_pool` 这些**超 trait** 的方法都直接可用 ✓
/// —— 换成具体类型就得再 import 五六个分 trait（`ScheduleStore` / `UserStore` …）✗。
fn store_in(dir: &std::path::Path) -> Arc<dyn Store> {
    Arc::new(JsonStore::open(dir).expect("临时目录应当能打开"))
}

fn owner() -> UserId {
    UserId::new("u-live")
}

/// 往共享口令池里放一场**预告场**（`status == 0` 才是"下一场" ✓）。
///
/// ⚠ 主播昵称必须写成 [`KOULING_ANCHOR`]：池挑「下一场」时**按它筛**
/// （"下一场指到别人场次"那次的修法 ✓）⇒ 写别的名字等于没有下一场 ✗。
async fn seed_pool(store: &dyn Store, expect: &str) {
    let mut pool = KoulingPool::default();
    pool.normalize(now_ms());
    pool.note_schedule_anchor(KOULING_ANCHOR);
    pool.note_probe(
        &[ProbeRoom {
            room_id: 147608,
            title: "国庆场".into(),
            status: 0,
            expect_start_time: expect.into(),
            anchor_nickname: KOULING_ANCHOR.into(),
        }],
        false,
        now_ms(),
    );
    store
        .save_kouling_pool(&pool)
        .await
        .expect("存口令池应当成功");
}

/// 造一个属于 `owner` 的已启用云盘账号（"空 = 全部已启用账号"那条要用 ✓）。
async fn seed_account(store: &dyn Store, id: &str, owner: Option<UserId>) {
    seed_account_full(store, id, "raw", true, owner).await;
}

/// 同上，但**凭据自己给** —— 假上游要按 `auth.raw` 认出"这是哪个账号" ✓。
async fn seed_account_with_auth(store: &dyn Store, id: &str, raw: &str, owner: Option<UserId>) {
    seed_account_full(store, id, raw, true, owner).await;
}

/// 账号的完整造法（`raw` / `enabled` 都要能指定 ✓）。
async fn seed_account_full(
    store: &dyn Store,
    id: &str,
    raw: &str,
    enabled: bool,
    owner: Option<UserId>,
) {
    store
        .save_account(&Account {
            id: AccountId::new(id),
            nickname: None,
            auth: AuthInfo {
                raw: raw.into(),
                platform: "android".into(),
                phone: id.into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("dev".into()),
            pc_platform: None,
            user_domain_id: None,
            owner,
            enabled,
            settings: AccountSettings::default(),
        })
        .await
        .expect("存账号应当成功");
}

/// 库里那条自动定时（没有就 panic —— 用例要的正是"它必须已经建出来" ✓）。
async fn saved_auto(store: &dyn Store) -> Schedule {
    store
        .schedule(&live_auto_schedule_id(&owner()))
        .await
        .unwrap()
        .expect("那条自动定时必须已经建出来")
}

// ══════════════════════════ 假上游（门的三步）══════════════════════════

/// 假上游：把「门的三步」**按账号**应答出来（§3 验收要真跑，不能猜 ✓）。
///
/// # 它怎么认出"这是哪个账号"
///
/// 生产里每个账号是一个**独立客户端**（工厂按账号注入凭据 ✓），而假上游是
/// **一份**共享的 ⇒ 只能从请求自己身上认：
///
/// ```text
/// ① querySpecTokenV2      Authorization: <auth.raw>       ← 账号自己的凭据
/// ② cmpassLoginByCmToken  body.cmpassToken = "SSO|<raw>"  ← 第①枪换来的
/// ③ tabContentList        Authorization: "JWT|<raw>"      ← 第②枪换来的
/// ```
///
/// 每一步都把"是哪个账号"编码进产物（`SSO|…` / `JWT|…` ✓）——
/// 这顺带钉住了"票是**按账号**换的"这件事 ✓。
struct FakeUpstream {
    /// `auth.raw` → 这个账号的预告场时间（`None` = 上游没给这一格 ✓）。
    rooms: HashMap<String, Option<String>>,
    /// 每一枪都报网络错（"上游取不到"那两条 ✓）。
    dead: bool,
    /// 记下每一枪（断言"每个账号各调了一次" ✓）。
    seen: Mutex<Vec<HttpRequest>>,
}

/// 一个按账号分派的假上游。`rooms` 里没写的账号 = 上游不给它场次 ✓。
fn fake_upstream(rooms: &[(&str, Option<&str>)]) -> Arc<FakeUpstream> {
    Arc::new(FakeUpstream {
        rooms: rooms
            .iter()
            .map(|(raw, at)| ((*raw).to_string(), at.map(str::to_string)))
            .collect(),
        dead: false,
        seen: Mutex::new(Vec::new()),
    })
}

/// 一个"谁来都报网络错"的假上游（`dead` ✓）。
fn dead_upstream() -> Arc<FakeUpstream> {
    Arc::new(FakeUpstream {
        rooms: HashMap::new(),
        dead: true,
        seen: Mutex::new(Vec::new()),
    })
}

impl FakeUpstream {
    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// 这个账号**开过几次门**（= 打了几个 `querySpecTokenV2` ✓）。
    ///
    /// ⚠ 这是"每个账号各调了一次"那条验收的判据（§3.6 ✓）：只看总枪数的话，
    /// "只问了第一个账号、后两个根本没问"会漏过去 ✗。
    fn gate_runs_for(&self, raw: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.url.contains("querySpecTokenV2"))
            .filter(|r| auth_of(r).as_deref() == Some(raw))
            .count()
    }

    /// 总共开过几次门（= 一共问过几个账号 ✓）。
    fn gate_runs(&self) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.url.contains("querySpecTokenV2"))
            .count()
    }
}

/// 请求头上的 `Authorization`（大小写照原样比 —— 构造那边写的是大写 `A` ✓）。
fn auth_of(req: &HttpRequest) -> Option<String> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.clone())
}

/// 造一份"按账号分派"的客户端工厂。
///
/// ⚠ 生产那份会**真打上游**（`user-njs.yun.139.com` ✗）—— 单测里必须换掉 ✓
///（与 `AppBuilder::notify_http` 同一条理由 ✓）。
fn factory_of(fake: &Arc<FakeUpstream>) -> mcloud_engine::HttpClientFactory {
    let f = Arc::clone(fake);
    Arc::new(move |_a: &Account| Arc::clone(&f) as Arc<dyn HttpClient>)
}

#[async_trait::async_trait]
impl HttpClient for FakeUpstream {
    async fn request(&self, req: HttpRequest) -> CoreResult<HttpResponse> {
        self.seen.lock().unwrap().push(req.clone());
        if self.dead {
            // "网络错"这一档 ⇒ 调用方要退到池里那份 ✓（不是把整次重算判失败 ✗）。
            return Err(Error::Network("假上游：网络断了".into()));
        }
        let ok = |body: String| {
            Ok(HttpResponse {
                status: 200,
                body,
                headers: Vec::new(),
            })
        };

        if req.url.contains("querySpecTokenV2") {
            // ① 换 SSO：**双层信封**（`success` + `code` 是字符串 ✓）
            let raw = auth_of(&req).unwrap_or_default();
            return ok(serde_json::json!({
                "success": true,
                "code": "0000",
                "message": "",
                "data": { "token": format!("SSO|{raw}") },
            })
            .to_string());
        }
        if req.url.contains("cmpassLoginByCmToken") {
            // ② 用 SSO 换直播侧 JWT：`object` **就是 JWT 本身**（实拍如此 ✓）
            let sso = req
                .body_str()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| {
                    v.get("cmpassToken")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_default();
            let raw = sso.strip_prefix("SSO|").unwrap_or_default();
            return ok(serde_json::json!({
                "rtnCode": "0",
                "rtnMsg": null,
                "object": format!("JWT|{raw}"),
            })
            .to_string());
        }
        if req.url.contains("tabContentList") {
            // ③ 拉页签：按 JWT 认出账号 ⇒ 给**这个账号**的那一场。
            // ⚠ 两个页签都回同一间房（实拍：147606 同时在 127 与 129 里 ✓）——
            // `merge_rooms` 会去重成一条 ✓。
            let jwt = auth_of(&req).unwrap_or_default();
            let raw = jwt.strip_prefix("JWT|").unwrap_or_default();
            let data_list = match self.rooms.get(raw).and_then(|r| r.as_ref()) {
                Some(at) => serde_json::json!([{ "room": {
                    "id": 147608,
                    "title": "预告",
                    "status": 0,
                    "channelId": 134,
                    "expectStartTime": at,
                    "anchorId": 1,
                    "authorId": 1,
                    "anchor": { "nickname": KOULING_ANCHOR },
                }}]),
                // 没有场次 = `dataList` 空（**不是** `null`：两者都合法，这里挑常见的 ✓）
                None => serde_json::json!([]),
            };
            return ok(serde_json::json!({
                "rtnCode": "0",
                "rtnMsg": null,
                "object": { "dataList": data_list },
            })
            .to_string());
        }
        Err(Error::Other(format!("假上游不认识这一枪：{}", req.url)))
    }
}

/// 造一条**用户手建**的定时（`managed_by` 留空 ⇒ 系统绝不碰 ✓）。
async fn seed_handmade(store: &dyn Store, id: &str) -> Schedule {
    let s = Schedule {
        id: ScheduleId::new(id),
        name: "我自己的签到".into(),
        task: "sign".into(),
        accounts: vec!["13800000000".into()],
        expr: "30 7 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::json!({ "keep": "me" }),
        enabled: true,
        managed_by: None,
        managed_wanted: None,
        owner: Some(owner()),
        push_rules: Vec::new(),
        push_events: Vec::new(),
        last_fire_ms: Some(123),
        last_run_ms: Some(456),
        last_status: Some("ok".into()),
        last_error: None,
        last_note: None,
        run_count: 7,
    };
    store.save_schedule(&s).await.expect("存定时应当成功");
    s
}

/// 造一条**系统维护的**自动定时，并把"用户意愿"那一格**显式**留成给定的值 ✓。
///
/// ⚠ 为什么要单独造它：`reschedule_live_auto` 首次创建时**顺手**就把意愿落成
/// `Some(true)` 了 ✓ ⇒ 只测"首次创建"分不清"判据是 `None` ⇒ 开"与"落盘时写了 true"
/// 这两件事 ✗。老数据（这一格是 `None` 的那条 ✓，`serde(default)` 读回来就是它 ✓）
/// 走的正是**读**这条路 ⇒ 用它钉判据本身 ✓。
async fn seed_managed_auto(store: &dyn Store, managed_wanted: Option<bool>) -> Schedule {
    let s = Schedule {
        id: live_auto_schedule_id(&owner()),
        name: AUTO_LIVE_NAME.into(),
        task: LIVE_ROOM_TASK.into(),
        accounts: Vec::new(),
        expr: "0 0 11 8 10 *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::json!({}),
        enabled: false,
        managed_by: Some(MANAGED_BY_LIVE_AUTO.to_string()),
        managed_wanted,
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
    store.save_schedule(&s).await.expect("存定时应当成功");
    s
}

/// ⭐ §3.1（线上形态）：池里有一场 10:00 ⇒ 定时 = **11:00**，且是那条
/// `auto-live-<owner>`、带系统维护标记、跑 `live-room`。
///
/// ⚠ 线上池是**实例级**的 ⇒ "三个账号各有下一场"在集成层拿不到三个不同的值
/// （见模块头）。三个值那种形态由 `mcloud-core::live_auto` 的纯函数用例钉住 ✓；
/// 这一条钉的是**另一头**：算出来的时刻真的写进了那条定时 ✓。
#[tokio::test]
async fn sets_the_auto_schedule_to_next_live_plus_one_hour() {
    let dir = tmpdir("set");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;

    let state = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert!(state.enabled, "有下一场 ⇒ 要能跑：{state:?}");
    assert_eq!(state.at.as_deref(), Some("2026-10-08 11:00:00"));
    assert_eq!(state.note, None, "正常状态不必解释什么");

    let s = store
        .schedule(&live_auto_schedule_id(&owner()))
        .await
        .unwrap()
        .expect("那条自动定时必须已经建出来");
    assert_eq!(s.expr, "0 0 11 8 10 *", "11:00 那一刻写成 cron");
    assert_eq!(s.task, LIVE_ROOM_TASK, "跑的是直播任务");
    assert!(s.is_managed_by(MANAGED_BY_LIVE_AUTO), "必须带系统维护标记");
    // 首次创建就把意愿**落成明确的一格** ✓（用户口径 2026-10-08「小红花定时默认开启」✓）：
    // 这一格不靠"读的时候缺省"推断 —— 落盘那份自己说得清是开还是关 ✓。
    assert_eq!(s.managed_wanted, Some(true), "首次创建要落成明确的「开」");
    assert_eq!(s.owner.as_ref(), Some(&owner()));
    assert_eq!(
        s.timezone, "Asia/Shanghai",
        "cron 里是东八区墙钟 ⇒ 时区必须跟上"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.3：**一个都没有** ⇒ `enabled=false` ✓、**没被删掉** ✓、`id` 还是原来那个 ✓。
///
/// 还要钉住"停用之后 `accounts` 与用户的意愿都留着"—— 那是"下次算出来自动启用"
/// 能成立的前提（否则用户得重新挑一遍账号 ✗）。
#[tokio::test]
async fn no_next_live_disables_but_keeps_the_entry() {
    let dir = tmpdir("disable");
    let store = store_in(&dir);
    let clock = clock();
    // 先有一次"有下一场"，把条目与账号选择建出来
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    let patch = AutoLivePatch {
        wanted: None,
        accounts: Some(vec!["13800000000".into(), "13900000001".into()]),
    };
    reschedule_live_auto(&store, &clock, None, &owner(), Some(patch))
        .await
        .unwrap();
    let id = live_auto_schedule_id(&owner());
    let before = store.schedule(&id).await.unwrap().expect("刚建出来的");

    // 再把池清空（= 上游这次没给下一场）
    store
        .save_kouling_pool(&KoulingPool::default())
        .await
        .unwrap();
    let state = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();

    assert!(!state.enabled, "算不出下一场 ⇒ 停用：{state:?}");
    assert_eq!(state.at, None, "没有时刻就不给一个看起来像时间的占位值");
    assert!(
        state.note.as_deref().unwrap_or_default().contains("停用"),
        "要说清为什么停用：{:?}",
        state.note
    );
    let after = store
        .schedule(&id)
        .await
        .unwrap()
        .expect("停用 ≠ 删除：条目必须还在");
    assert!(!after.enabled);
    assert_eq!(after.id, before.id, "id 还是原来那个");
    assert_eq!(after.accounts, before.accounts, "账号多选必须留着");
    assert_eq!(
        after.managed_wanted, before.managed_wanted,
        "用户的意愿也要留着"
    );
    assert_eq!(after.run_count, before.run_count, "记账字段不动");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.4 幂等：连算两次 ⇒ **只有一条** `auto-live-<owner>` ✓、时刻一致 ✓。
#[tokio::test]
async fn rescheduling_twice_keeps_exactly_one_entry() {
    let dir = tmpdir("idem");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;

    let a = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    let b = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert_eq!(a, b, "同样的输入必须得到同样的状态");

    let all = store.schedules().await.unwrap();
    assert_eq!(all.len(), 1, "只有一条，不能每算一次多一条：{all:?}");
    assert_eq!(all[0].id, live_auto_schedule_id(&owner()));
    assert_eq!(all[0].expr, "0 0 11 8 10 *");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.5：**不碰手建的定时** —— 重算之后它**一个字节没变** ✓。
///
/// ⚠ 判据是"整条 JSON 逐字节相同"，不是"几个关键字段相同"：
/// 逐字段断言只能证明**我想到的那几格**没变 ✗，而这条要的恰恰是"系统没动它" ✓。
#[tokio::test]
async fn handmade_schedules_are_untouched_byte_for_byte() {
    let dir = tmpdir("handmade");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    let handmade = seed_handmade(store.as_ref(), "my-own-sign").await;
    let snapshot = serde_json::to_value(&handmade).unwrap();

    reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();

    let after = store
        .schedule(&ScheduleId::new("my-own-sign"))
        .await
        .unwrap()
        .expect("手建的定时必须还在");
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        snapshot,
        "手建的定时必须一个字节都没变"
    );
    assert_eq!(
        store.schedules().await.unwrap().len(),
        2,
        "手建 + 自动 = 两条"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.6：**用户的账号多选不被重置** ✓。
///
/// 用户口径：「保留用户对账号多选的修改 ✓ —— 别每次把账号列表重置 ✗」。
/// 所以"改过账号列表之后重算，列表保持"必须是**逐元素**相同，
/// 而且**顺序**也要保持（下拉里那个顺序是用户看惯的那个 ✓）。
#[tokio::test]
async fn user_account_picks_survive_rescheduling() {
    let dir = tmpdir("accounts");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    // 三个账号都在库里（否则"空 = 全部"那条会把它们算进去）
    seed_account(store.as_ref(), "13800000000", Some(owner())).await;
    seed_account(store.as_ref(), "13900000001", Some(owner())).await;
    seed_account(store.as_ref(), "13700000002", Some(owner())).await;

    // 用户只挑了两个（**没挑全** ⇒ 重算要是拿"全部账号"覆盖就露馅 ✓）
    let patch = AutoLivePatch {
        wanted: None,
        accounts: Some(vec!["13900000001".into(), "13800000000".into()]),
    };
    let first = reschedule_live_auto(&store, &clock, None, &owner(), Some(patch))
        .await
        .unwrap();
    assert_eq!(first.accounts, vec!["13900000001", "13800000000"]);

    // 池里的日程换一场（改期）⇒ 重算，账号列表必须原样
    seed_pool(store.as_ref(), "2026-10-08 12:00:00").await;
    let second = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert_eq!(
        second.accounts,
        vec!["13900000001", "13800000000"],
        "账号多选不能被重置、顺序也不能变"
    );
    assert_eq!(
        second.at.as_deref(),
        Some("2026-10-08 13:00:00"),
        "时刻要跟上改期"
    );
    let s = store
        .schedule(&live_auto_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.accounts, second.accounts);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 用户口径 2026-10-08「**小红花定时默认开启**」：用户**从没表过态**
///（`managed_wanted = None` ✓）＋ **算得出下一场** ⇒ 那条定时就是**开**的 ✓。
///
/// ⚠ 与"首次创建"分开钉（见 [`seed_managed_auto`] 的说明 ✓）：只测首次创建的话，
/// "判据是 `None` ⇒ 开"与"落盘时顺手写了 `Some(true)`"这两件事分不开 ✗ ——
/// 老数据（这一格读回来就是 `None` 的那条 ✓）走的正是**读**这条路 ✓。
#[tokio::test]
async fn missing_intent_counts_as_on_when_a_next_live_exists() {
    let dir = tmpdir("none-on");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    // 意愿那一格**显式**留成 `None` = 从没表过态 ✓
    seed_managed_auto(store.as_ref(), None).await;

    let state = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert!(state.enabled, "没表过态 ⇒ 默认开启：{state:?}");
    assert_eq!(state.at.as_deref(), Some("2026-10-08 11:00:00"));
    let s = saved_auto(store.as_ref()).await;
    assert!(s.enabled, "落盘那条也必须是开着的");
    assert_eq!(
        s.managed_wanted,
        Some(true),
        "重算要把「没表过态」落成明确的一格（不靠读的时候缺省推断）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⚠ **默认开 ≠ 无视"没有下一场"** ✗：`managed_wanted = None` ＋ **算不出**下一场
/// ⇒ 照样 `enabled = false` ✓（"想不想要"与"到点能不能跑"是两件事，
/// 见 `Schedule::managed_wanted` 的注释 ✓）。
#[tokio::test]
async fn missing_intent_still_needs_a_next_live() {
    let dir = tmpdir("none-noslot");
    let store = store_in(&dir);
    let clock = clock();
    // 一个账号都没有、池也是空的 ⇒ 上游与兜底两条路都给不出时刻 ✓
    seed_managed_auto(store.as_ref(), None).await;

    let state = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert!(
        !state.enabled,
        "算不出下一场 ⇒ 停用（与「想不想要」无关）：{state:?}"
    );
    assert_eq!(state.at, None, "没有时刻就不给占位值");
    assert!(
        !saved_auto(store.as_ref()).await.enabled,
        "落盘那条也得是停用的"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 用户关掉它 ⇒ **即使算得出下一场也保持停用** ✓（这就是 `managed_wanted`
/// 与 `enabled` 分成两格的理由，见 `Schedule::managed_wanted` 的注释）。
#[tokio::test]
async fn user_turning_it_off_sticks_across_rescheduling() {
    let dir = tmpdir("off");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;

    let off = reschedule_live_auto(
        &store,
        &clock,
        None,
        &owner(),
        Some(AutoLivePatch {
            wanted: Some(false),
            accounts: None,
        }),
    )
    .await
    .unwrap();
    assert!(!off.enabled, "用户关掉了：{off:?}");
    assert_eq!(
        off.at.as_deref(),
        Some("2026-10-08 11:00:00"),
        "时刻照样算出来给他看"
    );
    assert!(
        off.note.as_deref().unwrap_or_default().contains("关闭"),
        "要说清是用户关的：{:?}",
        off.note
    );

    // 下一次收尾重算（没有 patch）⇒ **不能**自己把开关打开 ✗
    let again = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert!(!again.enabled, "用户关掉的开关不能被系统打开：{again:?}");
    let s = store
        .schedule(&live_auto_schedule_id(&owner()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.managed_wanted, Some(false));
    assert!(!s.enabled);
    let _ = std::fs::remove_dir_all(&dir);
}

/// id 撞车：`auto-live-<owner>` 这个 id 上是**用户手建**的定时 ⇒ 系统**绝不碰** ✓，
/// 而且不会另建一条（否则那个 id 上就有两条，upsert 会互相覆盖 ✗）。
#[tokio::test]
async fn a_handmade_schedule_squatting_on_our_id_is_left_alone() {
    let dir = tmpdir("squat");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    let id = live_auto_schedule_id(&owner()).as_str().to_string();
    let squat = seed_handmade(store.as_ref(), &id).await;
    let snapshot = serde_json::to_value(&squat).unwrap();

    let state = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert!(!state.enabled, "不碰它，所以这一轮什么都没算");
    assert!(
        state
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("不会动它"),
        "要如实说明跳过了：{:?}",
        state.note
    );
    let after = store.schedule(&ScheduleId::new(id)).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        snapshot,
        "用户手建的那条一个字节都不能变"
    );
    assert_eq!(store.schedules().await.unwrap().len(), 1, "不能另建一条");
    let _ = std::fs::remove_dir_all(&dir);
}

/// "账号为空 = 全部已启用账号"（`Schedule::accounts` 的既有语义 ✓）——
/// 空列表时系统照样算得出时刻（它不该因为"没挑账号"就算不出来 ✗）。
#[tokio::test]
async fn empty_account_pick_falls_back_to_all_enabled_accounts() {
    let dir = tmpdir("all-accounts");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    seed_account(store.as_ref(), "13800000000", Some(owner())).await;

    let state = reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    assert!(state.enabled);
    assert!(
        state.accounts.is_empty(),
        "没挑 = 空列表，保持空（那是「全部已启用账号」的写法）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `GET`（[`read_auto_state`]）**只读**：它不改那条定时，而且能把当前状态说清 ✓。
#[tokio::test]
async fn read_state_is_read_only_and_explains_itself() {
    let dir = tmpdir("read");
    let store = store_in(&dir);
    let clock = clock();

    // 还没有那条定时
    let fresh = read_auto_state(&store, &clock, &owner()).await.unwrap();
    assert!(!fresh.enabled);
    assert!(fresh.note.is_some(), "要说清「还没有这条自动定时」");
    assert!(store.schedules().await.unwrap().is_empty());

    // 建出来之后：时刻从那条定时的 cron **反推**（不另存一份 ✓）
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    reschedule_live_auto(&store, &clock, None, &owner(), None)
        .await
        .unwrap();
    let before = store.schedules().await.unwrap();
    let read = read_auto_state(&store, &clock, &owner()).await.unwrap();
    assert!(read.enabled);
    assert_eq!(read.at.as_deref(), Some("2026-10-08 11:00:00"));
    assert_eq!(read.note, None);
    assert_eq!(
        store.schedules().await.unwrap(),
        before,
        "GET 不能顺手改那条定时"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ═══════════════ §3 验收：向上游**现取**（用户口径 2026-10-08）═══════════════
//
// ⚠ 上面那些用例把 `http_for` 传 `None`（= 没接上游工厂 ✓）⇒ 走的是**池里那份兜底**；
// 下面这些才是"向上游现取"那条路 ✓（假上游把门的三步按账号应答出来 ✓）。

/// ⭐ §3.1：三个账号各有下一场（10:00 / 12:00 / 14:00）⇒ 定时时刻 = **11:00**。
///
/// ⚠ 池里**刻意放一场 09:00**（池那条路会算出 10:00 ✓）⇒ 这条用例同时钉住
/// "**上游优先于池**"：兜底那条路要是被走成了首选，算出来的就是 10:00 ✗。
#[tokio::test]
async fn takes_the_earliest_next_live_from_upstream() {
    let dir = tmpdir("upstream-earliest");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 09:00:00").await;

    let raws = [
        "Basic 13800000000",
        "Basic 13900000001",
        "Basic 13700000002",
    ];
    for (raw, id) in raws
        .iter()
        .zip(["13800000000", "13900000001", "13700000002"])
    {
        seed_account_with_auth(store.as_ref(), id, raw, Some(owner())).await;
    }
    let fake = fake_upstream(&[
        (raws[0], Some("2026-10-08 10:00:00")),
        (raws[1], Some("2026-10-08 12:00:00")),
        (raws[2], Some("2026-10-08 14:00:00")),
    ]);
    let factory = factory_of(&fake);

    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), None)
        .await
        .unwrap();
    assert!(state.enabled, "有下一场 ⇒ 要能跑：{state:?}");
    assert_eq!(
        state.at.as_deref(),
        Some("2026-10-08 11:00:00"),
        "三个账号里**最早**那场（10:00）+ 1 小时 —— 不是池里那份（09:00 + 1h）"
    );
    assert_eq!(state.note, None, "正常状态不必解释什么");

    let s = saved_auto(store.as_ref()).await;
    assert_eq!(s.expr, "0 0 11 8 10 *", "11:00 那一刻写成 cron");
    assert!(s.enabled);
    // ⭐ §3.6 的另一半：**每个账号各开了一次门**（不许"优化"成只调一次 ✗）
    for raw in raws {
        assert_eq!(fake.gate_runs_for(raw), 1, "账号 {raw} 该恰好开一次门");
    }
    assert_eq!(fake.gate_runs(), 3, "三个账号各一次");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.2 前半：**只有一个账号有场次** ⇒ 就用它。
///
/// ⚠ 另外两个账号是"上游没给这一格"（`dataList` 空 ✓）—— **不是**错误，
/// 不该让整次重算失败、也不该被算成"没有下一场" ✗。
#[tokio::test]
async fn only_the_account_with_a_slot_decides_the_time() {
    let dir = tmpdir("upstream-one");
    let store = store_in(&dir);
    let clock = clock();

    let raws = [
        "Basic 13800000000",
        "Basic 13900000001",
        "Basic 13700000002",
    ];
    for (raw, id) in raws
        .iter()
        .zip(["13800000000", "13900000001", "13700000002"])
    {
        seed_account_with_auth(store.as_ref(), id, raw, Some(owner())).await;
    }
    let fake = fake_upstream(&[
        (raws[0], None),
        (raws[1], Some("2026-10-08 12:00:00")),
        (raws[2], None),
    ]);
    let factory = factory_of(&fake);

    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), None)
        .await
        .unwrap();
    assert!(state.enabled);
    assert_eq!(state.at.as_deref(), Some("2026-10-08 13:00:00"));
    assert_eq!(
        fake.gate_runs(),
        3,
        "三个账号**都**要问过（不能只问有场次的那个 ✗）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.2 后半：**所有账号都没有场次**（且池里那份也没有）⇒ `enabled=false`，
/// 而且**条目还在** ✓（停用 ≠ 删除 ✓）。
#[tokio::test]
async fn no_slot_from_any_account_disables_but_keeps_the_entry() {
    let dir = tmpdir("upstream-none");
    let store = store_in(&dir);
    let clock = clock();
    seed_account_with_auth(store.as_ref(), "13800000000", "Basic A", Some(owner())).await;
    seed_account_with_auth(store.as_ref(), "13900000001", "Basic B", Some(owner())).await;
    let fake = fake_upstream(&[("Basic A", None), ("Basic B", None)]);
    let factory = factory_of(&fake);

    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), None)
        .await
        .unwrap();
    assert!(!state.enabled, "算不出下一场 ⇒ 停用：{state:?}");
    assert_eq!(state.at, None, "没有时刻就不给一个看起来像时间的占位值");
    assert!(
        state.note.as_deref().unwrap_or_default().contains("停用"),
        "要说清为什么停用：{:?}",
        state.note
    );

    let s = saved_auto(store.as_ref()).await;
    assert!(!s.enabled);
    assert!(s.is_managed_by(MANAGED_BY_LIVE_AUTO), "条目还在、标记还在");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.3：**上游报错** ⇒ 用池里那份兜底 ✓（断言时刻来自池 ✓，且没有 panic ✓）。
///
/// ⚠ "上游报错"**不算这次重算失败**（它不会以 `Err` 冒出来 ✓）——
/// 所以这里能拿到一个 `Ok(state)`，而时刻是池里那份 ✓。
#[tokio::test]
async fn upstream_failure_falls_back_to_the_cached_pool() {
    let dir = tmpdir("upstream-dead");
    let store = store_in(&dir);
    let clock = clock();
    seed_pool(store.as_ref(), "2026-10-08 10:00:00").await;
    seed_account_with_auth(store.as_ref(), "13800000000", "Basic A", Some(owner())).await;
    seed_account_with_auth(store.as_ref(), "13900000001", "Basic B", Some(owner())).await;
    let fake = dead_upstream();
    let factory = factory_of(&fake);

    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), None)
        .await
        .expect("上游报错**不是**这次重算的失败");
    assert!(state.enabled);
    assert_eq!(
        state.at.as_deref(),
        Some("2026-10-08 11:00:00"),
        "时刻来自**池里那份**（10:00 + 1 小时）"
    );
    assert_eq!(
        fake.gate_runs(),
        2,
        "两个账号都去试过了（失败也得试过，不是「看到错就不问了」）"
    );
    assert_eq!(saved_auto(store.as_ref()).await.expr, "0 0 11 8 10 *");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.4：上游报错 **且** 池里也没有 ⇒ 停用 ✓。
#[tokio::test]
async fn upstream_failure_without_a_cached_pool_disables() {
    let dir = tmpdir("upstream-dead-empty-pool");
    let store = store_in(&dir);
    let clock = clock();
    // 池里**一场都没有**（从没探测过 ✓）
    seed_account_with_auth(store.as_ref(), "13800000000", "Basic A", Some(owner())).await;
    let fake = dead_upstream();
    let factory = factory_of(&fake);

    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), None)
        .await
        .unwrap();
    assert!(!state.enabled, "两条路都没有 ⇒ 停用：{state:?}");
    assert_eq!(state.at, None);
    assert!(
        state.note.as_deref().unwrap_or_default().contains("停用"),
        "要说清是哪种情况：{:?}",
        state.note
    );
    let s = saved_auto(store.as_ref()).await;
    assert!(!s.enabled, "停用但**不删**");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.5（+ §2.3）：`accounts` 为空 = **全部已启用账号** ⇒ 先展开成真实账号
/// 再逐个取 ✓（断言上游被调了 N 次 ✓）。
///
/// ⚠ 展开要**跳过停用的**（用户自己关掉的账号不该拿去打上游 ✓），
/// 所以这里放 3 个启用的 + 1 个停用的，期望正好 3 次 ✓。
#[tokio::test]
async fn an_empty_account_pick_expands_to_all_enabled_accounts() {
    let dir = tmpdir("upstream-expand");
    let store = store_in(&dir);
    let clock = clock();
    let raws = ["Basic A", "Basic B", "Basic C"];
    for (raw, id) in raws
        .iter()
        .zip(["13800000000", "13900000001", "13700000002"])
    {
        seed_account_with_auth(store.as_ref(), id, raw, Some(owner())).await;
    }
    // 第四个是**停用**的（不该被问 ✓）
    seed_account_full(
        store.as_ref(),
        "13600000003",
        "Basic OFF",
        false,
        Some(owner()),
    )
    .await;
    let fake = fake_upstream(&[
        (raws[0], Some("2026-10-08 14:00:00")),
        (raws[1], None),
        (raws[2], Some("2026-10-08 12:00:00")),
    ]);
    let factory = factory_of(&fake);

    // 不点名（`accounts` 空 ✓）= 全部已启用账号
    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), None)
        .await
        .unwrap();
    assert!(state.enabled);
    assert_eq!(
        state.at.as_deref(),
        Some("2026-10-08 13:00:00"),
        "三个启用账号里最早那场（12:00）+ 1 小时"
    );
    assert_eq!(
        fake.gate_runs(),
        3,
        "**空列表必须被展开**：三个启用的各问一次"
    );
    for raw in raws {
        assert_eq!(fake.gate_runs_for(raw), 1, "账号 {raw} 该恰好开一次门");
    }
    assert_eq!(
        fake.gate_runs_for("Basic OFF"),
        0,
        "停用的账号不该被拿去打上游"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ §3.6：**每个账号各调了一次**（调用计数 ✓）—— 免得以后有人
/// "优化"成只调一次 ✗。
///
/// # 为什么这条必须单独钉
///
/// 上游那份日程**今天是按主播的**（平台级、所有账号同一份 ✓，见
/// `mcloud_core::live_auto` 模块头的事实 ✓）⇒ "只调一次、把结果发给所有账号"
/// 看起来**结果一模一样**，而它正是要被挡住的那种"优化"：
/// 票是**每个账号自己的**（凭据坏了 / 未登录时只有那个账号取不到 ✓），
/// 而上游哪天按账号分时，这种写法会静默错掉 ✓。
#[tokio::test]
async fn every_picked_account_opens_the_gate_exactly_once() {
    let dir = tmpdir("upstream-per-account");
    let store = store_in(&dir);
    let clock = clock();
    let raws = ["Basic A", "Basic B"];
    for (raw, id) in raws.iter().zip(["13800000000", "13900000001"]) {
        seed_account_with_auth(store.as_ref(), id, raw, Some(owner())).await;
    }
    let fake = fake_upstream(&[
        (raws[0], Some("2026-10-08 12:00:00")),
        (raws[1], Some("2026-10-08 10:00:00")),
    ]);
    let factory = factory_of(&fake);

    // **点名**这两个账号（与"空列表展开"那条分开：那条测展开，这条测"各调一次" ✓）
    let patch = AutoLivePatch {
        wanted: None,
        accounts: Some(vec!["13800000000".into(), "13900000001".into()]),
    };
    let state = reschedule_live_auto(&store, &clock, Some(&factory), &owner(), Some(patch))
        .await
        .unwrap();
    assert_eq!(
        state.at.as_deref(),
        Some("2026-10-08 11:00:00"),
        "两个账号里最早那场（10:00）+ 1 小时"
    );
    assert_eq!(fake.gate_runs(), 2, "点名两个 ⇒ 上游被调两次");
    assert_eq!(fake.gate_runs_for(raws[0]), 1);
    assert_eq!(fake.gate_runs_for(raws[1]), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 接线：**运行收尾那一跳真的把上游接上了**。
///
/// # 为什么这条不能省
///
/// 重算的 `http_for` 是从 `Notifier` 上取的（`spawn_after_run` ✓），而它
/// **没接上也不会报错** —— 表现是"功能看着好的、其实退回读池里那份"✗
///（最不容易发现的一种退化 ✓）。所以这里从 `spawn_after_run` **走一遍**，
/// 并用"池里那份刻意给一个**不同**的时刻"来区分两条路 ✓：
/// 只有真的走了上游，`expr` 才会是上游那一刻 ✓。
#[tokio::test]
async fn the_finish_hook_asks_upstream_through_the_notifier() {
    let dir = tmpdir("finish-hook-upstream");
    let store = store_in(&dir);
    let clock = clock();
    // 池里那份（兜底）= 09:00 ⇒ 10:00；上游那份 = 12:00 ⇒ 13:00 ✓
    seed_pool(store.as_ref(), "2026-10-08 09:00:00").await;
    let raw = "Basic A";
    seed_account_with_auth(store.as_ref(), "13800000000", raw, Some(owner())).await;
    let fake = fake_upstream(&[(raw, Some("2026-10-08 12:00:00"))]);

    let notifier = Arc::new(
        // ⚠ 第一个 `http` 是**推送投递**用的裸客户端（这条用例没绑任何规则 ⇒ 一枪不发 ✓）；
        // 上游那条路走的是 `with_http_for` 那一份 ✓ —— 两者**不是一回事**（见 notify.rs ✓）。
        Notifier::new(Arc::clone(&fake) as Arc<dyn HttpClient>, Arc::clone(&clock))
            .with_store(Some(Arc::clone(&store)))
            .with_http_for(factory_of(&fake)),
    );

    // ⚠ 计数**必须在 spawn 之前**加上（与 `notify.rs` 那一处完全一样 ✓）——
    // 否则 `flush()` 可能在计数还没加上时就看到 0 而直接返回 ✗。
    let guard = notifier.begin_delivery();
    spawn_after_run(
        &notifier,
        LIVE_ROOM_TASK.to_string(),
        "13800000000".to_string(),
        tokio::runtime::Handle::current(),
        guard,
    );
    notifier.flush().await;

    let s = saved_auto(store.as_ref()).await;
    assert_eq!(
        s.expr, "0 0 13 8 10 *",
        "收尾那一跳必须走**上游**那份（12:00 + 1h），而不是池里那份（09:00 + 1h）"
    );
    assert_eq!(fake.gate_runs_for(raw), 1, "只该问这一个账号一次");
    let _ = std::fs::remove_dir_all(&dir);
}

// ═════════ §4 验收：**种下那条定时**（用户口径 2026-10-08「小红花定时默认开启」✓）═════════
//
// 要补的是"**每个用户开箱就有这条定时**"✓ —— 而过去这条条目**只有两个创建入口**：
// 跑完一次 `live-room` 任务，或显式 `PUT`（⇒ 别的用户压根没有它 ✗）。
// 这一组钉的正是**种子那条路**（`AppHandle::seed_defaults_for` ✓）：走一遍就有、
// 四格值对、顺手算一次；而**已经有就不再打上游** ✓。

/// 等后台那次"种完重算"落地（`seed_and_reschedule_auto` 是 **`spawn`** 出去的 ✓）。
///
/// ⚠ 为什么不能只让出一次时间片就断言：那个任务要过好几个 `await` 点
///（读存储 → 打假上游（它内部还 `spawn` 了一组 `JoinSet` ✓）→ 写存储 ✓），
/// 而 `#[tokio::test]` 默认是**单线程**运行时 ⇒ 每让出一次它才推进一步 ✓。
/// 这里最多让出 200 次（足够跑完上面那条链 ✓）；到点还没落地就**照常往下走**，
/// 让**断言**去报错（比在这里 panic 更容易看出是哪一条验收 ✗）。
async fn settle() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

/// ⭐ 种子那一步写下去的**四格值**（用户口径定稿 ✓）—— 钉的是"**重算前**"的形态。
///
/// | 格 | 期望 | 依据 |
/// |---|---|---|
/// | `managed_by` | `Some("live-auto")` | 系统只动自己这条，绝不碰手建的 ✓ |
/// | `managed_wanted` | `Some(true)` | 「小红花定时默认开启」= 意愿那一格是「要」✓ |
/// | `enabled` | `false` | 还没算出下一场；写 `true` 等于骗调度器 ✗ |
/// | `expr` | 占位 | 停用 ⇒ 永不触发 ✓ |
///
/// ⚠ 这条**刻意不走后台重算**（直接调 [`seed_auto_schedule`] ✓）：要钉的正是
/// "种下去、还没算"那一格（用户口径里的 `enabled == false` ✓）—— 走重算的话
/// 断言就变成"算完之后长什么样"，那是下一条用例的事 ✓。
#[tokio::test]
async fn seeding_writes_the_four_values_and_never_overwrites() {
    let dir = tmpdir("seed-values");
    let store = store_in(&dir);

    assert!(
        seed_auto_schedule(&store, &owner()).await.unwrap(),
        "第一次种：必须是「新种出来的」（调用方据此决定要不要顺手重算 ✓）"
    );
    let s = saved_auto(store.as_ref()).await;
    assert!(
        s.is_managed_by(MANAGED_BY_LIVE_AUTO),
        "要带系统维护标记（否则系统下次就不认它了 ✗）"
    );
    assert_eq!(
        s.managed_wanted,
        Some(true),
        "默认开启 = 意愿那一格是「要」"
    );
    assert!(!s.enabled, "先关着：还没算出下一场");
    assert_eq!(s.expr, SEED_PLACEHOLDER_EXPR, "占位（停用 ⇒ 永不触发 ✓）");
    assert_eq!(s.task, LIVE_ROOM_TASK);
    assert!(s.accounts.is_empty(), "空 = 全部已启用账号（既有语义 ✓）");
    assert_eq!(s.owner.as_ref(), Some(&owner()), "归属必须是这个用户");
    assert_eq!(s.timezone, "Asia/Shanghai", "与东八区墙钟配套 ✓");

    // 幂等：再种一次 ⇒ 不是"新种出来的"，而且**一个字节都没变** ✓
    assert!(
        !seed_auto_schedule(&store, &owner()).await.unwrap(),
        "已有 ⇒ 回 false（调用方因此一个上游请求都不发 ✓）"
    );
    assert_eq!(
        saved_auto(store.as_ref()).await,
        s,
        "已经有就一个字节都不动（尤其别把用户的意愿冲掉 ✗）"
    );
    assert_eq!(store.schedules().await.unwrap().len(), 1, "不会多出一条");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 种完**顺手算一次**（真打假上游 ✓）：有下一场 ⇒ `enabled=true`，
/// 且 `expr` = 那一场 **+ 1 小时** ✓。
#[tokio::test]
async fn seeding_then_rescheduling_enables_it_with_the_computed_time() {
    let dir = tmpdir("seed-resched");
    let store = store_in(&dir);
    let clock = clock();
    let raw = "Basic 13800000000";
    seed_account_with_auth(store.as_ref(), "13800000000", raw, Some(owner())).await;
    let fake = fake_upstream(&[(raw, Some("2026-10-08 10:00:00"))]);

    seed_and_reschedule_auto(&store, &clock, Some(&factory_of(&fake)), &owner())
        .await
        .unwrap();
    settle().await;

    let s = saved_auto(store.as_ref()).await;
    assert!(s.enabled, "算出下一场 ⇒ 要能跑：{s:?}");
    assert_eq!(
        s.expr,
        one_shot_expr("2026-10-08 11:00:00").unwrap(),
        "10:00 那一场 + 1 小时 = 11:00"
    );
    assert_eq!(s.managed_wanted, Some(true), "意愿那一格不受重算影响");
    assert_eq!(
        fake.gate_runs_for(raw),
        1,
        "只该问这一个账号一次（每账号 ≤4 枪 ✓）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 假上游**报错** ⇒ 条目**还在**、`enabled=false`，而且**没有 panic** ✓。
///
/// ⚠ 两件事分开钉，因为它们的判据不一样：
///
/// 1. **可观察的结果** —— 走种子那条路（后台 `spawn` ✓）之后，落盘那份是
///    "**停用但保留**"（用户口径 ✓）、`expr` 还是占位 ✓；
/// 2. **不 panic / 不冒泡** —— `spawn` 出去的任务里 panic **不会**让用例变红
///    （`JoinHandle` 被丢掉了 ✗）⇒ 这里额外**直接**调一次 [`reschedule_live_auto`]：
///    上游报错必须回 `Ok`（走池里那份兜底 ✓）、不是 `Err`、更不能 panic ✓
///    ——"上游抖一下"绝不该被记成"重算失败"✗。
#[tokio::test]
async fn a_dead_upstream_leaves_the_seeded_entry_disabled_and_intact() {
    let dir = tmpdir("seed-dead-upstream");
    let store = store_in(&dir);
    let clock = clock();
    let raw = "Basic 13800000000";
    seed_account_with_auth(store.as_ref(), "13800000000", raw, Some(owner())).await;
    let fake = dead_upstream();

    seed_and_reschedule_auto(&store, &clock, Some(&factory_of(&fake)), &owner())
        .await
        .unwrap();
    settle().await;

    let s = saved_auto(store.as_ref()).await;
    assert!(!s.enabled, "上游报错 ⇒ 停用：{s:?}");
    assert_eq!(s.expr, SEED_PLACEHOLDER_EXPR, "算不出时刻就不写一个假时刻");
    assert_eq!(s.managed_wanted, Some(true), "意愿照旧留着");
    assert!(
        fake.gate_runs_for(raw) >= 1,
        "要真的去试过 —— 否则这条用例钉的是「压根没跑」，不是「报错也活着」✗"
    );

    let state = reschedule_live_auto(&store, &clock, Some(&factory_of(&fake)), &owner(), None)
        .await
        .expect("上游报错不是重算失败：它走池里那份兜底 ✓（更不该 panic ✗）");
    assert!(!state.enabled);
    assert!(
        !saved_auto(store.as_ref()).await.enabled,
        "停用 ≠ 删除：条目必须还在"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 用户**显式关过**（`managed_wanted: Some(false)`）⇒ 再走一遍种子那条路 ⇒
/// **仍然关** ✓，而且**连上游都不打** ✓、整条**一个字节都不变** ✓。
///
/// ⚠ 这一格是"**意愿**"（用户写的 ✓），与 `enabled`（系统算的 ✓）**不是一回事**：
/// 覆盖它就会出现用户报过的那种"我关了的开关自己又开了"✗
///（见 `Schedule::managed_wanted` 的说明 ✓）。
#[tokio::test]
async fn an_explicit_user_off_is_never_overwritten_and_stops_upstream_calls() {
    let dir = tmpdir("seed-user-off");
    let store = store_in(&dir);
    let clock = clock();
    let raw = "Basic 13800000000";
    seed_account_with_auth(store.as_ref(), "13800000000", raw, Some(owner())).await;
    let fake = fake_upstream(&[(raw, Some("2026-10-08 10:00:00"))]);

    // ① 先种 + 算一次（有下一场 ⇒ 开 ✓）
    seed_and_reschedule_auto(&store, &clock, Some(&factory_of(&fake)), &owner())
        .await
        .unwrap();
    settle().await;
    assert!(saved_auto(store.as_ref()).await.enabled);

    // ② 用户在界面上把它**关掉**（那个动作落到的正是 `managed_wanted` ✓）
    let mut off = saved_auto(store.as_ref()).await;
    off.managed_wanted = Some(false);
    off.enabled = false;
    store.save_schedule(&off).await.unwrap();
    let calls_before = fake.gate_runs();

    // ③ 再走一遍"启动 / 登录"那条路（种子 + 顺手重算）
    seed_and_reschedule_auto(&store, &clock, Some(&factory_of(&fake)), &owner())
        .await
        .unwrap();
    settle().await;

    let after = saved_auto(store.as_ref()).await;
    assert_eq!(
        after.managed_wanted,
        Some(false),
        "用户明确关过 ⇒ 绝不能被冲掉 ✗"
    );
    assert!(
        !after.enabled,
        "意愿是关 ⇒ 就算上游有下一场也还是关（用户口径 ✓）"
    );
    assert_eq!(
        fake.gate_runs(),
        calls_before,
        "已经有这条了 ⇒ **一个上游请求都不该发**（否则每次登录都打一轮 ✗）"
    );
    assert_eq!(after, off, "整条一个字节都不该变");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 已经有「**开 + 有时刻**」的用户 ⇒ 再走两遍种子那条路，**上游一次都不打** ✓
///（用户口径：「别在每次启动时给所有用户都打一轮上游」✓）。
#[tokio::test]
async fn an_already_running_entry_costs_zero_upstream_calls() {
    let dir = tmpdir("seed-already-on");
    let store = store_in(&dir);
    let clock = clock();
    let raw = "Basic 13800000000";
    seed_account_with_auth(store.as_ref(), "13800000000", raw, Some(owner())).await;
    let fake = fake_upstream(&[(raw, Some("2026-10-08 10:00:00"))]);

    // 先有一条"开 + 有时刻"的（当成上次直播任务跑完算出来的那一条 ✓）
    seed_and_reschedule_auto(&store, &clock, Some(&factory_of(&fake)), &owner())
        .await
        .unwrap();
    settle().await;
    let before = saved_auto(store.as_ref()).await;
    assert!(before.enabled, "前置条件：这条是能跑的");
    let calls = fake.gate_runs();
    assert_eq!(calls, 1, "第一次「种 + 算」问了这个账号一次");

    // 再走两遍（= 每次启动 / 每次登录都会调它 ✓）⇒ 调用次数**一次都不涨** ✓
    for _ in 0..2 {
        seed_and_reschedule_auto(&store, &clock, Some(&factory_of(&fake)), &owner())
            .await
            .unwrap();
        settle().await;
    }
    assert_eq!(
        fake.gate_runs(),
        calls,
        "已经有「开 + 有时刻」⇒ 上游一次都不该再打"
    );
    assert_eq!(
        saved_auto(store.as_ref()).await,
        before,
        "那条定时也不该被动过"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 走**真的种子入口**（`AppHandle::seed_defaults_for` ✓ —— 服务启动与登录都调它 ✓）：
/// 新用户 ⇒ 落盘里就有那条 `auto-live-<owner>` ✓，而且**后台**那次重算真的发生了
///（假上游给一场 ⇒ `enabled=true`，`expr` = 那一场 + 1 小时 ✓）。
///
/// ⚠ 必须用 `AppBuilder::http_for` 注入假上游：生产那份工厂会把请求打到**真的上游**
///（`user-njs.yun.139.com` ✗，单测里既不可能也不该发生 ✓）。
/// ⚠ 这里用的是 `AppBuilder` 的**真时钟**（它没留注入时钟的口子 ✓）⇒ 那一场只能
/// **相对"现在"** 算：写死一个日期，等那天过去这条用例就会测出一个假结论 ✗。
#[tokio::test]
async fn the_real_seed_entry_point_seeds_and_then_computes_in_the_background() {
    let dir = tmpdir("seed-entry-point");
    let raw = "Basic 13800000000";
    // 相对"现在"两小时后开播（真时钟 ✓）⇒ 定时该落在三小时后 ✓
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let next_live =
        mcloud_core::live_auto::beijing_stamp(now_ms + 2 * mcloud_core::live_auto::ONE_HOUR_MS);
    let want_at = mcloud_core::live_auto::beijing_stamp(
        mcloud_core::parse_beijing_time(&next_live, mcloud_core::live_auto::LOCAL_FMT)
            .expect("造出来的时刻必须能解析")
            + mcloud_core::live_auto::ONE_HOUR_MS,
    );
    let fake = fake_upstream(&[(raw, Some(next_live.as_str()))]);

    let app = crate::AppBuilder::new()
        .data_dir(&dir)
        .http_for(factory_of(&fake))
        .build()
        .expect("应用应能组装");
    let owner = UserId::new("u-seed-entry");
    seed_account_with_auth(
        app.handle.store.as_ref(),
        "13800000000",
        raw,
        Some(owner.clone()),
    )
    .await;

    // ⭐ 就是这一句：登录 / 启动那条路给这个用户"开箱就有" ✓
    app.handle.seed_defaults_for(&owner).await.unwrap();

    let s = app
        .handle
        .store
        .schedule(&live_auto_schedule_id(&owner))
        .await
        .unwrap()
        .expect("种子入口必须把那条自动定时建出来");
    assert!(s.is_managed_by(MANAGED_BY_LIVE_AUTO));
    assert_eq!(s.managed_wanted, Some(true), "默认开启 ✓");
    assert_eq!(s.owner.as_ref(), Some(&owner));
    assert_eq!(s.task, LIVE_ROOM_TASK);

    // 后台那次重算（**不 await** ✓）落地后：时刻算出来了、条目被启用了 ✓
    settle().await;
    let s = app
        .handle
        .store
        .schedule(&live_auto_schedule_id(&owner))
        .await
        .unwrap()
        .expect("条目还在");
    assert!(s.enabled, "后台重算必须真的跑过（有下一场 ⇒ 启用 ✓）");
    assert_eq!(
        s.expr,
        one_shot_expr(&want_at).unwrap(),
        "expr 必须是「下一场（{next_live}）+ 1 小时」= {want_at}"
    );
    assert_eq!(fake.gate_runs_for(raw), 1, "这个账号被问了一次 ✓");
    let _ = std::fs::remove_dir_all(&dir);
}
