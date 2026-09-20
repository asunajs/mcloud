// 由 config.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::Scope;

#[tokio::test]
async fn schedule_roundtrip_keeps_every_field() {
    let s = store();
    let original = schedule("sched-1");
    s.upsert_schedule(Scope::Local, &original).await.unwrap();

    let loaded = s
        .get_schedule(Scope::Local, "sched-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, original);
}

#[tokio::test]
async fn schedule_target_absent_survives_as_none() {
    let s = store();
    let mut sc = schedule("sched-1");
    sc.target = None;
    s.upsert_schedule(Scope::Local, &sc).await.unwrap();
    assert_eq!(
        s.get_schedule(Scope::Local, "sched-1")
            .await
            .unwrap()
            .unwrap()
            .target,
        None
    );
}

/// `None` 必须读写为 NULL，不能被写成字符串 `"null"`。
#[tokio::test]
async fn absent_body_is_null_not_the_string_null() {
    let s = store();
    let mut sc = schedule("sched-1");
    sc.body = None;
    s.upsert_schedule(Scope::Local, &sc).await.unwrap();

    let raw: Option<String> = {
        let conn = s.conn.lock().unwrap();
        conn.query_row("SELECT body FROM schedules WHERE id = 'sched-1'", [], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert_eq!(raw, None, "没给参数应存 NULL");

    assert_eq!(
        s.get_schedule(Scope::Local, "sched-1")
            .await
            .unwrap()
            .unwrap()
            .body,
        None
    );
}

#[tokio::test]
async fn schedule_upsert_updates_in_place() {
    let s = store();
    let mut sc = schedule("sched-1");
    s.upsert_schedule(Scope::Local, &sc).await.unwrap();

    sc.name = "改名了".into();
    sc.enabled = false;
    sc.last_status = Some(ScheduleStatus::Skipped);
    s.upsert_schedule(Scope::Local, &sc).await.unwrap();

    let all = s.list_schedules(Scope::Local).await.unwrap();
    assert_eq!(all.len(), 1, "重复写入不应产生第二条");
    assert_eq!(all[0].name, "改名了");
    assert!(!all[0].enabled);
    assert_eq!(all[0].last_status, Some(ScheduleStatus::Skipped));
}

#[tokio::test]
async fn schedules_keep_insertion_order_when_created_at_ties() {
    let s = store();
    // 三个都没有 created_at —— 排序完全靠 rowid，也就是插入顺序
    for id in ["c", "a", "b"] {
        let mut sc = schedule(id);
        sc.created_at = None;
        s.upsert_schedule(Scope::Local, &sc).await.unwrap();
    }
    let ids: Vec<String> = s
        .list_schedules(Scope::Local)
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.id)
        .collect();
    assert_eq!(ids, vec!["c", "a", "b"], "同 created_at 时应保持插入顺序");
}

#[tokio::test]
async fn delete_schedule_reports_whether_a_row_went_away() {
    let s = store();
    s.upsert_schedule(Scope::Local, &schedule("sched-1"))
        .await
        .unwrap();
    assert!(s.delete_schedule(Scope::Local, "sched-1").await.unwrap());
    assert!(
        !s.delete_schedule(Scope::Local, "sched-1").await.unwrap(),
        "第二次应为 false"
    );
    assert!(s.list_schedules(Scope::Local).await.unwrap().is_empty());
}
