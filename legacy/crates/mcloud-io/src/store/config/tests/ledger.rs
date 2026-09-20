// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

#[tokio::test]
async fn import_ledger_records_and_reads_back() {
    let s = store();
    assert!(s.import_record("/old/state.json").unwrap().is_none());

    let record = ImportRecord {
        source: "/old/state.json".into(),
        sha256: "a".repeat(64),
        imported_at: "2026-09-16T22:00:00+00:00".into(),
        record_count: 2,
        issues: 1,
        raw: Some("{\"accounts\":{}}".into()),
    };
    s.record_import(&record).unwrap();
    assert_eq!(s.import_record("/old/state.json").unwrap().unwrap(), record);

    // 同一个源再写一次是覆盖，不是新增
    s.record_import(&ImportRecord {
        sha256: "b".repeat(64),
        ..record.clone()
    })
    .unwrap();
    let all = s.list_imports().unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].sha256.starts_with('b'));

    assert!(record.summary().contains("记录 2 条"));
    assert!(!record.summary().contains("accounts"), "摘要不得带出原文");
}
