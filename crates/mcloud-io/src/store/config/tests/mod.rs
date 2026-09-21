// 配置存储单元测试的公共 helper 与数据构造器；主题用例见同目录其它文件。
use super::*;

use chrono::{TimeZone, Utc};

use mcloud_core::{Account, Role, Scope, StateStore, User, UserId, UserStore};

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("内存库应可打开")
}

/// 测试用户。v3 起账号与登录态都要求归属（`accounts.owner` 有外键指向 `users`），
/// 所以碰账号的用例都得先有个"主人"。
const TEST_USER: &str = "test-user";

fn uid() -> UserId {
    UserId::new(TEST_USER)
}

async fn ensure_user(s: &SqliteStore) {
    s.upsert_user(&User::new(
        TEST_USER,
        TEST_USER,
        "!",
        Role::Admin,
        Utc.with_ymd_and_hms(2026, 9, 16, 1, 0, 0).unwrap(),
    ))
    .await
    .unwrap();
}

/// 建一个归属于测试用户的账号。
async fn owned_account(s: &SqliteStore, id: &str) -> Account {
    ensure_user(s).await;
    let mut account = Account::new(id);
    account.owner = Some(uid());
    s.upsert_account(Scope::user(&uid()), &account)
        .await
        .unwrap();
    account
}

fn schedule(id: &str) -> Schedule {
    Schedule {
        id: id.into(),
        owner: None,
        name: "每日签到".into(),
        target: Some(ScheduleTarget::Capability {
            code: "sign".into(),
        }),
        cron: "0 30 7 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        account_ids: vec![AccountId::new("19****01")],
        body: Some(serde_json::json!({"codes": ["口令"]})),
        enabled: true,
        created_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 1, 0, 0).unwrap()),
        last_run_at: None,
        last_status: None,
        last_error: None,
    }
}

fn pipeline(id: &str) -> Pipeline {
    Pipeline {
        id: id.into(),
        owner: None,
        name: "轻量巡检".into(),
        description: Some("查奖品 + 领奖品".into()),
        steps: vec![
            PipelineStep {
                task: "prizes".into(),
                body: None,
                continue_on_error: None,
            },
            PipelineStep {
                task: "receive".into(),
                body: Some(serde_json::json!({"onlyNew": true})),
                continue_on_error: Some(true),
            },
        ],
        account_ids: Vec::new(),
        account_mode: AccountMode::Sequential,
        stop_on_error: true,
        created_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 1, 57, 41).unwrap()),
        updated_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 2, 24, 7).unwrap()),
        last_run_at: None,
        last_status: Some(PipelineStatus::Ok),
        last_error: None,
        last_duration_ms: Some(2481),
    }
}

mod isolation;
mod ledger;
mod pipelines;
mod prefs;
mod reopen;
mod schedules;
mod states;
