// 由 legacy_import.rs 的单元测试按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::Scope;
use mcloud_core::{ConfigStore, StateStore};

// ── 幂等 ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn second_run_skips_unchanged_files() {
    let s = store();
    run_import(&s, &fixture_source()).await.unwrap();

    let again = run_import(&s, &fixture_source()).await.unwrap();
    assert_eq!(again.accounts, 0, "asign.json 没变，不重复写");
    assert_eq!(again.runs, 0);
    assert_eq!(again.pipelines, 0);
    assert_eq!(again.schedules, 0, "空数组也不该被当成有新内容");
    assert_eq!(
        again.skipped_unchanged.len(),
        LEGACY_DATA_FILES.len(),
        "6 个数据文件都应被跳过，实际 {:?}",
        again.skipped_unchanged
    );
    assert!(
        !again.has_issues(),
        "跳过不是问题：{:#?}",
        again.issue_lines()
    );

    // 库里的行数不变
    assert_eq!(
        s.list_runs(Scope::user(&uid()), 100).await.unwrap().len(),
        12
    );
    assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
}

#[tokio::test]
async fn re_import_after_touch_does_not_duplicate_rows() {
    let dir = copy_fixtures();
    let s = store();
    run_import(&s, &source_of(&dir)).await.unwrap();
    let before: Vec<String> = s
        .list_runs(Scope::user(&uid()), 100)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();

    // 改内容一个字节（末尾加换行）：sha256 变了 → 整份重跑。
    // 这一条验的才是真正的幂等：**台账失效之后**，UPSERT 主键仍然稳定。
    let text = std::fs::read_to_string(dir.path().join("history.json")).unwrap();
    write_file(&dir, "history.json", &format!("{text}\n"));

    let report = run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(report.runs, 12, "重跑会重新解析 12 条");
    assert!(
        !report
            .skipped_unchanged
            .contains(&"history.json".to_string()),
        "history.json 变了，不该被跳过"
    );

    let after: Vec<String> = s
        .list_runs(Scope::user(&uid()), 100)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(after, before, "行数不变，主键也不变");
    assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
    assert_eq!(
        s.list_pipelines(Scope::user(&uid())).await.unwrap().len(),
        1
    );
}

// ── 安全失败 ───────────────────────────────────────────────────────────

#[tokio::test]
async fn mask_collision_aborts_before_writing_anything() {
    let dir = copy_fixtures();
    // 两个手机号前 2 后 2 相同 → 掩码都是 19****16 → 会塌缩成同一个 AccountId
    write_file(
        &dir,
        "asign.json",
        r#"{
  "caiyun": [
{ "auth": "Basic MjoxOTkwMDAwMDAxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr", "nickname": "甲" },
{ "auth": "Basic MjoxOTkwMDAwMDIxNjpha3xia3xja3wxODkzNDU2MDAwMDAwfGZr", "nickname": "乙" }
  ]
}"#,
    );

    let s = store();
    let err = run_import(&s, &source_of(&dir))
        .await
        .expect_err("掩码冲突必须让迁移失败");
    assert!(
        matches!(err, LegacyImportError::Identity(_)),
        "应当是身份映射失败，实际 {err:?}"
    );

    // 关键：不是"导入了一半"，而是"什么都没动"
    assert!(
        s.list_accounts(Scope::user(&uid()))
            .await
            .unwrap()
            .is_empty(),
        "中止时不该写入任何账号"
    );
    assert!(
        s.list_runs(Scope::user(&uid()), 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        s.list_pipelines(Scope::user(&uid()))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        s.list_imports().unwrap().is_empty(),
        "中止时也不该留下台账，否则重跑会被误判成已导入"
    );
}

#[tokio::test]
async fn missing_file_is_reported_and_the_rest_still_imports() {
    let dir = copy_fixtures();
    remove_file(&dir, "schedules.json");

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    assert_eq!(report.runs, 12, "少一个文件不影响别的文件");
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::MissingFile { path } if path.ends_with("schedules.json"))),
        "缺失文件要报出来：{:#?}",
        report.issue_lines()
    );
}

#[tokio::test]
async fn corrupt_json_fails_loudly_instead_of_importing_halfway() {
    let dir = copy_fixtures();
    write_file(&dir, "history.json", "{ 这不是 JSON");

    let s = store();
    let err = run_import(&s, &source_of(&dir))
        .await
        .expect_err("整份 JSON 坏掉应当直接失败");
    assert!(
        matches!(err, LegacyImportError::Parse { .. }),
        "应当是解析错误，实际 {err:?}"
    );
}

#[tokio::test]
async fn one_bad_run_record_does_not_take_down_the_rest() {
    let dir = copy_fixtures();
    // 4 条记录，分别是：好的 / 类型错的 / 不是对象的 / 展示名认不出的
    write_file(
        &dir,
        "history.json",
        r#"{
  "runs": [
{ "task": "sign", "account": "账号1", "ok": true, "start": "2026-09-16T11:00:00Z", "end": "2026-09-16T11:00:01Z" },
{ "task": "sign", "account": "账号1", "ok": 1 },
"这不是对象",
{ "task": "sign", "account": "查无此人", "ok": true, "start": "2026-09-16T11:02:00Z", "end": "2026-09-16T11:02:01Z" }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    // 能归属的那条照常落库并可读
    let runs = s.list_runs(Scope::user(&uid()), 100).await.unwrap();
    assert_eq!(runs.len(), 1, "只有能归属的记录会出现在用户列表里");
    assert_eq!(runs[0].account_id, Some(id(ACC_2)), "「账号1」→ 19****02");

    // 展示名认不出的那条**仍然落库**（历史不能因为归属解析失败就丢掉），
    // 只是它无法归属，所以不出现在任何用户的列表里 ——
    // 它该被看见的地方是迁移报告，不是某个人的"我的历史"。
    let orphaned: i64 = {
        let conn = s.connection().lock().expect("数据库锁中毒");
        conn.query_row(
            "SELECT COUNT(*) FROM runs WHERE account_id IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(orphaned, 1, "无归属的旧记录仍要保留，否则历史就断了");

    let lines = report.issue_lines();
    assert!(
        lines.iter().any(|l| l.contains("history.json 第 2 条")),
        "下标要指得准：{lines:#?}"
    );
    assert!(lines.iter().any(|l| l.contains("第 3 条")));
    assert!(lines.iter().any(|l| l.contains("查无此人")));
}

#[tokio::test]
async fn bad_timestamp_is_reported_and_skipped() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "history.json",
        r#"{ "runs": [ { "task": "sign", "account": "账号1", "ok": true, "start": "昨天下午" } ] }"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(report.runs, 0);
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::BadRunTimestamp { .. })),
        "{:#?}",
        report.issue_lines()
    );
    assert!(
        s.list_runs(Scope::user(&uid()), 100)
            .await
            .unwrap()
            .is_empty()
    );
}
