// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::Scope;

#[tokio::test]
async fn pipeline_roundtrip_keeps_steps_and_accounts() {
    let s = store();
    let mut original = pipeline("pipe-1");
    original.account_ids = vec![AccountId::new("19****01"), AccountId::new("19****02")];
    original.account_mode = AccountMode::Parallel;
    original.stop_on_error = false;
    s.upsert_pipeline(Scope::Local, &original).await.unwrap();

    let loaded = s
        .get_pipeline(Scope::Local, "pipe-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, original);
    assert_eq!(loaded.steps.len(), 2);
    assert_eq!(loaded.steps[1].continue_on_error, Some(true));
}

/// 组合与定时任务的状态枚举**不是同一个**（组合有 partial、没有 skipped），别合并。
#[test]
fn pipeline_status_is_not_schedule_status() {
    assert_eq!(
        pipeline_status_from_str("partial"),
        Some(PipelineStatus::Partial)
    );
    assert_eq!(pipeline_status_from_str("skipped"), None);
    assert_eq!(
        schedule_status_from_str("skipped"),
        Some(ScheduleStatus::Skipped)
    );
    assert_eq!(schedule_status_from_str("partial"), None);
}

#[tokio::test]
async fn unknown_status_string_reads_as_none_instead_of_a_wrong_value() {
    let s = store();
    s.upsert_schedule(Scope::Local, &schedule("sched-1"))
        .await
        .unwrap();
    {
        let conn = s.conn.lock().unwrap();
        conn.execute(
            "UPDATE schedules SET last_status = 'weird' WHERE id = 'sched-1'",
            [],
        )
        .unwrap();
    }
    assert_eq!(
        s.get_schedule(Scope::Local, "sched-1")
            .await
            .unwrap()
            .unwrap()
            .last_status,
        None
    );
}
