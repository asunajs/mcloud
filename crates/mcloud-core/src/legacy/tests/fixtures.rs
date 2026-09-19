// 由 legacy.rs 的 `mod tests` 按主题拆出；夹具与 helper 见同目录 mod.rs。
use super::*;

// ───────────────────────── 夹具：端到端 ─────────────────────────

/// 账号配置 → 视图：手机号来自 auth 现场解析，标识走掩码。
#[test]
fn fixture_config_derives_identity_from_auth() {
    let config: LegacyConfig = load(fixture::CONFIG);
    assert_eq!(config.version, Some(2), "旧配置是 version 2");
    assert_eq!(config.caiyun.len(), 2);

    let views = describe_legacy_accounts(&config.caiyun);
    // 真实配置里**没有** phone 字段，号码只能从 auth 里解 —— 这正是 resolve_phone 的价值
    assert!(views.iter().all(|v| v.config_phone.is_none()));

    assert_eq!(views[0].nickname.as_deref(), Some("账号2"));
    assert_eq!(views[0].phone, "19900000001");
    assert_eq!(views[0].label, "账号2", "有昵称时展示名就是昵称");
    assert_eq!(views[0].legacy_key, "19900000001");
    assert_eq!(views[0].account_id.as_str(), "19****01");

    assert_eq!(views[1].nickname.as_deref(), Some("账号1"));
    assert_eq!(views[1].phone, "19900000002");
    assert_eq!(views[1].account_id.as_str(), "19****02");

    assert_eq!(
        views[0].platform.as_deref(),
        Some("2"),
        "平台号来自 auth 第 1 段"
    );

    // 序号从 1 起，与旧 findAccount 一致
    assert_eq!((views[0].index, views[1].index), (1, 2));

    // 夹具的 expire 刻意放在未来，免得这份测试随时间变成"凭据过期"。
    // 这里同时钉住**量级**：真实数据是 13 位毫秒，写成 10 位秒值会让下游整条链路的
    // 单位假设悄悄错掉（见 auth.rs 模块头）。
    assert!(
        views.iter().all(|v| v.auth_expire > 1_700_000_000_000),
        "夹具 auth 的 expire 应是毫秒量级且在未来"
    );
    assert!(
        views.iter().all(|v| v.auth_expire.to_string().len() == 13),
        "夹具应当长得像真实数据：13 位毫秒"
    );
}

/// **跨文件**：`state.json` 的键是手机号原文，必须能映射到与 `asign.json` 相同的 `AccountId`。
/// 这条挂了就意味着历史凭据会挂到错误的账号上。
#[test]
fn fixture_state_keys_map_onto_the_same_account_ids() {
    let state: LegacyStateFile = load(fixture::STATE);
    let views = fixture_views();
    let map = build_identity_map(&views).expect("夹具内不应有掩码冲突");

    assert_eq!(state.accounts.len(), 2);
    assert_eq!(map.len(), 2);

    let mut derived: Vec<&str> = views.iter().map(|v| v.account_id.as_str()).collect();
    let mut mapped: Vec<&str> = state.accounts.keys().map(|k| map[k].as_str()).collect();
    derived.sort_unstable();
    mapped.sort_unstable();
    assert_eq!(derived, mapped, "state.json 的键应恰好覆盖全部账号");

    // 四个状态字段都要能读出来；未识别字段应为空（夹具里不该有）
    for (key, account_state) in &state.accounts {
        assert!(!account_state.is_empty(), "{key} 的状态不该是空的");
        assert!(
            account_state.unknown_fields().is_empty(),
            "{key} 出现了未识别字段 {:?}",
            account_state.unknown_fields()
        );
        assert!(account_state.jwt_token.as_deref().unwrap_or("").len() > 500);
        assert!(account_state.device_id.as_deref().unwrap_or("").len() > 80);
        assert!(account_state.user_domain_id.is_some());
    }
}

/// **跨文件**：历史里的展示名必须唯一对应一个账号。
/// 展示名撞车（两个账号同名）会让历史无法归属 —— 这里是发现它的地方。
#[test]
fn fixture_history_labels_each_resolve_to_one_account() {
    let history: LegacyHistoryFile = load(fixture::HISTORY);
    assert_eq!(history.runs.len(), 12, "夹具只保留最后 12 条");

    let views = fixture_views();
    for run in &history.runs {
        assert!(!run.task.is_empty(), "每条历史都应记下任务名");
        let hits = find_by_label(&views, &run.account);
        assert_eq!(
            hits.len(),
            1,
            "展示名 {:?} 应唯一对应一个账号，实际命中 {}",
            run.account,
            hits.len()
        );
    }

    let tasks: Vec<&str> = history.runs.iter().map(|r| r.task.as_str()).collect();
    assert!(
        tasks.contains(&"auth-refresh"),
        "夹具里应有 auth-refresh 记录，实际 {tasks:?}"
    );
}

/// 日志条目同时保留数值 `level` 与语义 `type`，且**同一个 level 对应多个 type**。
#[test]
fn fixture_log_entries_keep_level_and_type_separately() {
    let history: LegacyHistoryFile = load(fixture::HISTORY);
    let mut combos = std::collections::BTreeSet::new();

    for run in &history.runs {
        for entry in &run.logs {
            assert!(entry.msg.is_some(), "日志条目应有 msg");
            assert!(entry.date.is_some(), "日志条目应有 date");
            combos.insert((entry.level, entry.kind.clone()));
        }
    }

    assert!(combos.contains(&(Some(3), Some("info".into()))));
    assert!(combos.contains(&(Some(0), Some("error".into()))));
    // 这是本测试存在的理由：level 3 之下不止一种语义，只看 level 会丢掉 start/success
    let level3: Vec<&Option<String>> = combos
        .iter()
        .filter(|(level, _)| *level == Some(3))
        .map(|(_, kind)| kind)
        .collect();
    assert!(
        level3.len() >= 2,
        "level=3 应对应多个 type，实际 {level3:?}"
    );
}

/// 组合：夹具里没给账号字段 → 语义是"全部账号"，而不是"没有账号"。
#[test]
fn fixture_pipeline_migrates_legacy_account_field() {
    let file: LegacyPipelinesFile = load(fixture::PIPELINES);
    assert_eq!(file.pipelines.len(), 1);
    let pipeline = &file.pipelines[0];

    assert_eq!(pipeline.name, "轻量巡检");
    assert_eq!(pipeline.steps.len(), 2);
    assert_eq!(
        pipeline
            .steps
            .iter()
            .map(|s| s.task.as_str())
            .collect::<Vec<_>>(),
        vec!["prizes", "receive"]
    );
    assert_eq!(pipeline.stop_on_error, Some(true));
    assert_eq!(pipeline.last_status.as_deref(), Some("ok"));
    assert_eq!(pipeline.last_duration_ms, Some(2481));

    assert!(pipeline.accounts.is_none() && pipeline.legacy_account.is_none());
    assert_eq!(
        normalize_pipeline_accounts(
            pipeline.accounts.as_deref(),
            pipeline.legacy_account.as_ref()
        ),
        None,
        "没给账号 = 全部账号"
    );
}

/// 抢兑：窗口按旧规则转成 6 段 cron；夹具里 `bindings` 为空但字段必须都在。
#[test]
fn fixture_exchange_windows_become_six_field_cron() {
    let exchange: LegacyExchangeFile = load(fixture::EXCHANGE);
    assert!(!exchange.enabled);
    assert!(exchange.bindings.is_empty());
    assert!(exchange.schedule_ids.is_empty());
    assert!(
        exchange.updated_at.is_some(),
        "字段应被识别（不是塞进 ignored）"
    );

    let windows = normalize_exchange_windows(&exchange.windows).expect("夹具窗口应合法");
    assert_eq!(windows, vec!["09:59:40", "11:59:40"]);
    assert_eq!(
        windows
            .iter()
            .map(|w| exchange_window_to_cron(w).unwrap())
            .collect::<Vec<_>>(),
        vec!["40 59 9 * * *", "40 59 11 * * *"]
    );
}

#[test]
fn fixture_schedules_file_parses_even_when_empty() {
    let file: LegacySchedulesFile = load(fixture::SCHEDULES);
    assert!(file.schedules.is_empty(), "真实数据里定时任务列表为空");
}

#[test]
fn fixture_webui_normalizes_with_legacy_defaults() {
    let prefs = normalize_webui(&load::<LegacyWebuiFile>(fixture::WEBUI));
    assert!(prefs.remember_last_page);
    assert_eq!(prefs.last_page, "tasks");
    assert!(!prefs.log_expanded);
    assert!(!prefs.background_run);
    assert!(prefs.updated_at.is_some());
}

/// 夹具声明的"分配了哪些脱敏号码"必须与我们从 `asign.json` 推出来的号码一致。
///
/// 否则说明夹具与它的元信息脱节了 —— 那是最糟的情况：测试还在过，但依据已经错了。
///
/// 注意元信息里**只有脱敏号**：真实号码是 PII，不落盘（早先这里存的是
/// `{原号码: 脱敏号}`，等于把真实手机号写进了版本库）。
#[test]
fn fixture_phone_assignment_agrees_with_derived_phones() {
    let meta: Value = load(fixture::META);
    let mut declared: Vec<&str> = meta["$phoneAssignment"]["$assigned"]
        .as_array()
        .expect("$meta.json 应有 $phoneAssignment.$assigned")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let views = fixture_views();
    let mut derived: Vec<&str> = views.iter().map(|v| v.phone.as_str()).collect();
    declared.sort_unstable();
    derived.sort_unstable();
    assert_eq!(declared, derived);

    // 元信息里应记下各源文件的 sha256，避免有人以为夹具就是全部历史
    let sums = meta["$sourceSha256"].as_object().unwrap();
    assert!(sums.contains_key("history.json"));
}

/// 夹具里**不得**出现真实标识：11 位手机号与 19 位 `userDomainId`
/// 都必须是 `199` 开头的合成值。
///
/// 这条是给"元信息/注释里顺手写下原值"这类泄漏兜底的 —— 夹具是脱敏产物，
/// 出现真实值就说明脱敏被绕过了。生成器自己也有一道自检，
/// 这里再来一道是因为它会**编译进测试**：夹具被手改也能被发现，
/// 并且它会把 base64（`auth` 字段）里的载荷解码后再查一遍。
///
/// 判定规则、误报原因与剩余盲区见
/// [`crate::test_support::assert_only_synthetic_identifiers`]。
#[test]
fn fixtures_contain_no_real_identifiers() {
    for (name, text) in [
        ("$meta.json", fixture::META),
        ("asign.json", fixture::CONFIG),
        ("state.json", fixture::STATE),
        ("history.json", fixture::HISTORY),
        ("pipelines.json", fixture::PIPELINES),
        ("schedules.json", fixture::SCHEDULES),
        ("exchange.json", fixture::EXCHANGE),
        ("webui.json", fixture::WEBUI),
    ] {
        crate::test_support::assert_only_synthetic_identifiers(name, text);
    }
}
