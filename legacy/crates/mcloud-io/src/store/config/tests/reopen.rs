// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::Scope;

#[tokio::test]
async fn config_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcloud.db");
    {
        let s = SqliteStore::open(&path).unwrap();
        s.upsert_schedule(Scope::Local, &schedule("sched-1"))
            .await
            .unwrap();
        s.upsert_pipeline(Scope::Local, &pipeline("pipe-1"))
            .await
            .unwrap();
    }
    let s = SqliteStore::open(&path).unwrap();
    assert_eq!(s.list_schedules(Scope::Local).await.unwrap().len(), 1);
    assert_eq!(s.list_pipelines(Scope::Local).await.unwrap().len(), 1);
}
